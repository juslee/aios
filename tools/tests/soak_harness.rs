//! `aios soak`'s boot loop against a fake QEMU and a fake `just` (see
//! `common::soak_fake::scenarios`), compared with the deleted `scripts/soak-qemu.sh`.
//!
//! - `harness_goldens_match_aios` runs every scenario with aios, in parallel, and
//!   compares the normalised stdout, stderr, output files and QEMU arguments with
//!   `tests/golden/soak/harness/<scenario>.golden`. It also checks the raw
//!   timing the normalisation hides.
//! - `record_harness_goldens_from_oracle` (ignored) records the goldens from the
//!   oracle; it needs `timeout` or `gtimeout` on PATH.
//! - `harness_differential_against_oracle` (ignored, about 30 s) runs both.
//! - `sighup_and_sigquit_stop_qemu_and_clean_up` covers the two signals the
//!   script did not handle (the port exits 129 or 131 instead of leaving QEMU
//!   running).
//! - `a_signal_that_ends_a_setup_step_exits_with_its_status` covers a Ctrl-C
//!   that kills `just create-data-disk` or `sha256sum` (130, not a setup error).
//! - `an_interrupt_after_the_last_boot_skips_the_summary` covers a signal that
//!   arrives once no QEMU runs (the script's trap exited 130 at once).
//! - `ctrl_z_stops_qemu_with_the_harness` covers SIGTSTP during a boot: QEMU
//!   stops with the harness and its time limit waits for the resume (the
//!   script's `timeout` killed it at the limit while bash was stopped).

mod common;

use common::soak::{check_golden, golden_dir};
use common::soak_fake::{golden_text, run_scenario, scenarios, suspended, Outcome, Scenario, Tool};

/// Run `f` over `items` on scoped threads, keeping the input order.
fn parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    std::thread::scope(|s| {
        let handles: Vec<_> = items.iter().map(|item| s.spawn(|| f(item))).collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("a scenario thread"))
            .collect()
    })
}

/// The value of `key=` in a run log's `[soak] meta` footer.
fn footer_value(o: &Outcome, log: &str, key: &str) -> i64 {
    let (_, data) = o
        .files
        .iter()
        .find(|(n, _)| n == log)
        .unwrap_or_else(|| panic!("no {log}"));
    let text = String::from_utf8_lossy(data);
    let footer = text
        .lines()
        .find(|l| l.starts_with("[soak] meta "))
        .expect("a footer");
    let prefix = format!("{key}=");
    footer
        .split(' ')
        .find_map(|w| w.strip_prefix(prefix.as_str()))
        .unwrap_or_else(|| panic!("no {key}"))
        .parse()
        .expect("a number")
}

/// Checks on what the normalisation replaces with placeholders.
fn check_raw(sc: &Scenario, o: &Outcome) {
    let in_range = |key: &str, lo: i64, hi: i64| {
        let v = footer_value(o, "run-01.log", key);
        assert!(
            (lo..=hi).contains(&v),
            "{}: {key}={v}, expected {lo}..={hi}",
            sc.name
        );
    };
    match sc.name {
        "clean-timeout" => {
            in_range("qemu_rc", 124, 124);
            in_range("elapsed", 3, 4);
            in_range("hb_last_advance", 1, 2);
            in_range("g1done", 1, 2);
        }
        "kill-after" => {
            in_range("qemu_rc", 137, 137);
            in_range("elapsed", 12, 13);
        }
        "panic-exit" => {
            in_range("qemu_rc", 1, 1);
            in_range("elapsed", 0, 2);
            in_range("kstart", 0, 2);
        }
        _ => {}
    }
    assert!(
        o.alive.is_empty(),
        "{}: still running: {:?}",
        sc.name,
        o.alive
    );
    if let Some(names) = &o.listing {
        assert!(
            !names.iter().any(|n| n.starts_with(".scratch.")),
            "{}: scratch left: {names:?}",
            sc.name
        );
    }
}

#[test]
fn harness_goldens_match_aios() {
    let all = scenarios();
    let outcomes = parallel(&all, |sc| run_scenario(Tool::Aios, sc));
    let mut diffs = Vec::new();
    for (sc, o) in all.iter().zip(&outcomes) {
        check_raw(sc, o);
        if let Some(d) = check_golden(
            &format!("harness/{}.golden", sc.name),
            &golden_text(o, false),
        ) {
            diffs.push(d);
        }
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n\n"));
}

#[test]
#[ignore = "rewrites tests/golden/soak/harness/ from the oracle (needs timeout or gtimeout)"]
fn record_harness_goldens_from_oracle() {
    let all = scenarios();
    let outcomes = parallel(&all, |sc| run_scenario(Tool::Oracle, sc));
    std::fs::create_dir_all(golden_dir().join("harness")).expect("golden dir");
    for (sc, o) in all.iter().zip(&outcomes) {
        std::fs::write(
            golden_dir().join(format!("harness/{}.golden", sc.name)),
            golden_text(o, true),
        )
        .expect("write a golden");
    }
}

#[test]
#[ignore = "runs every scenario under bash and aios (about 30 s; needs timeout or gtimeout)"]
fn harness_differential_against_oracle() {
    let all = scenarios();
    let pairs = parallel(&all, |sc| {
        (run_scenario(Tool::Oracle, sc), run_scenario(Tool::Aios, sc))
    });
    let mut diffs = Vec::new();
    for (sc, (oracle, aios)) in all.iter().zip(&pairs) {
        let want = golden_text(oracle, true);
        let got = golden_text(aios, false);
        if want != got {
            diffs.push(format!(
                "{}\n--- oracle\n{}\n--- aios\n{}",
                sc.name,
                String::from_utf8_lossy(&want),
                String::from_utf8_lossy(&got)
            ));
        }
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n\n"));
}

/// The golden scenario called `name`.
fn scenario(name: &str) -> Scenario {
    scenarios()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no scenario {name}"))
}

#[test]
fn sighup_and_sigquit_stop_qemu_and_clean_up() {
    let cases = [
        ("interrupt-hup", "HUP", 129),
        ("interrupt-quit", "QUIT", 131),
    ];
    let outcomes = parallel(&cases, |&(name, signal, _)| {
        let sc = Scenario {
            name,
            interrupt: Some(signal),
            ..scenario("interrupt-int")
        };
        run_scenario(Tool::Aios, &sc)
    });
    for ((name, _, code), o) in cases.iter().zip(&outcomes) {
        assert_eq!(
            o.code,
            *code,
            "{name}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(o.alive.is_empty(), "{name}: still running: {:?}", o.alive);
        assert_eq!(
            o.listing.as_deref(),
            Some(
                &[
                    "run-01.log".to_string(),
                    "run-02.log".to_string(),
                    "summary.tsv".to_string()
                ][..]
            ),
            "{name}"
        );
    }
}

#[test]
fn a_signal_that_ends_a_setup_step_exits_with_its_status() {
    let cases = [
        (
            "interrupt-data-disk",
            &["--no-build", "--reuse-data", "runs=1", "secs=35", "out=out"][..],
            &["data-disk-interrupted"][..],
        ),
        (
            "interrupt-sha256",
            &["--no-build", "runs=1", "secs=35", "out=out"][..],
            &["sha256-interrupted"][..],
        ),
    ];
    let outcomes = parallel(&cases, |&(name, args, flags)| {
        let sc = Scenario {
            name,
            args,
            flags,
            ..scenario("panic-exit")
        };
        run_scenario(Tool::Aios, &sc)
    });
    for ((name, _, _), o) in cases.iter().zip(&outcomes) {
        // 130 and no `soak: error:`, as the script's trap exited.
        assert_eq!(
            o.code,
            130,
            "{name}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&o.stderr), "", "{name}: stderr");
        assert!(o.argv.is_empty(), "{name}: QEMU booted");
        assert_eq!(o.listing.as_deref(), Some(&[][..]), "{name}");
    }
}

#[test]
fn an_interrupt_after_the_last_boot_skips_the_summary() {
    let sc = Scenario {
        name: "interrupt-after-last-boot",
        args: &["--no-build", "runs=1", "secs=30", "out=out"],
        flags: &["uname-interrupts"],
        ..scenario("panic-exit")
    };
    let o = run_scenario(Tool::Aios, &sc);
    assert_eq!(o.code, 130, "{}", String::from_utf8_lossy(&o.stderr));
    assert!(o.alive.is_empty(), "still running: {:?}", o.alive);
    assert_eq!(
        o.listing.as_deref(),
        Some(&["run-01.log".to_string(), "summary.tsv".to_string()][..])
    );
}

#[test]
fn ctrl_z_stops_qemu_with_the_harness() {
    let o = run_scenario(Tool::Aios, &suspended());
    // Status 0 without report_only: the boot is CLEAN.
    assert_eq!(o.code, 0, "{}", String::from_utf8_lossy(&o.stdout));
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("run 01/1  CLEAN "),
        "{}",
        String::from_utf8_lossy(&o.stdout)
    );
    assert!(o.alive.is_empty(), "still running: {:?}", o.alive);
    // The limit ran out only after the resume (124), and every time the
    // classifier reads is QEMU's running time: the 3.5 s or more spent
    // stopped is in neither the elapsed time nor the heartbeat's last advance.
    assert_eq!(footer_value(&o, "run-01.log", "qemu_rc"), 124);
    let elapsed = footer_value(&o, "run-01.log", "elapsed");
    assert!((3..=4).contains(&elapsed), "elapsed={elapsed}");
    let advance = footer_value(&o, "run-01.log", "hb_last_advance");
    assert!(
        (1..=elapsed).contains(&advance),
        "hb_last_advance={advance}"
    );
    let gap = footer_value(&o, "run-01.log", "hb_max_gap");
    assert!(gap <= 2, "hb_max_gap={gap}");
    assert!(
        o.listing
            .as_ref()
            .is_some_and(|l| l.iter().any(|n| n == "summary.md")),
        "{:?}",
        o.listing
    );
}
