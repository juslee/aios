//! `aios soak --arm`: the interleave mode against fake arm checkouts (worktrees
//! of one fake repository), a fake QEMU that boots each arm's own script, and
//! fake `just`, `rustup` and `rustc` (see `common::soak_fake`).
//!
//! - `interleave_goldens_match_aios` runs every scenario of [`scenarios`] in
//!   parallel and compares the normalised stdout, stderr, output files and the
//!   fake rustup and just logs with
//!   `tests/golden/soak/interleave/<scenario>.golden`
//!   (`AIOS_BLESS_GOLDENS=1` rewrites them), then checks each scenario's
//!   properties: the rotation, the refusals before any boot, the build
//!   skipped with `--no-build`, the status line after every boot, the stops
//!   for harness errors and a changed QEMU, and the per-arm single-run files.
//! - `sigint_mid_round_leaves_a_stopped_report` covers a signal during a boot,
//!   and `sigint_during_a_probe_after_a_boot_is_the_signal_not_a_change` one
//!   that ends the QEMU check after a boot.
//! - `brief_lists_an_interleaved_run_once_and_never_as_main_soak` runs
//!   `scripts/agent/brief.sh` on a fixture `target/soak/`.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use common::soak::check_golden;
use common::soak_fake::{
    arm_golden_text, exception_script, no_stub_script, panic_script, run_arm_scenario,
    wait_for_signal_script, ArmOutcome, ArmScenario, At,
};
use common::{git, isolated, run_aios, unique_dir, TestRepo};

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

/// A scenario in which arm `a` panics and arm `b` takes an exception.
fn two_arms(name: &'static str, arms: &[&str], rest: &[&str]) -> ArmScenario {
    let mut sc = ArmScenario::new(name, arms, rest);
    sc.boots = vec![
        ("boot-a.sh".into(), panic_script()),
        ("boot-b.sh".into(), exception_script()),
    ];
    sc
}

/// A refusal: arms `a` and `b`, the given changes, `rest` after the arms.
fn refusal(
    name: &'static str,
    rest: &[&str],
    change: impl FnOnce(&mut ArmScenario),
) -> ArmScenario {
    let mut sc = two_arms(name, &["a", "b"], rest);
    change(&mut sc);
    sc
}

/// Every golden scenario, in golden-file order.
fn scenarios() -> Vec<ArmScenario> {
    let mut v = vec![
        two_arms("two-arms", &["a", "b"], &["runs=3", "secs=35", "out=out"]),
        two_arms("a-a", &["a", "a"], &["runs=2", "secs=35", "out=out"]),
        two_arms(
            "no-build",
            &["a", "b"],
            &["--no-build", "runs=1", "secs=35", "out=out"],
        ),
        refusal(
            "allow-mixed",
            &[
                "--no-build",
                "--allow-mixed-toolchains",
                "runs=1",
                "secs=35",
                "out=out",
            ],
            |sc| {
                sc.arms[1].channel = Some("nightly-2026-10-01");
                sc.arms[1].rustc = "rustc 1.98.0-nightly (fake 2026-10-01)";
            },
        ),
        two_arms(
            "default-out",
            &["a", "b"],
            &["--no-build", "runs=1", "secs=35"],
        ),
    ];
    // Boots 3, 4 and 5 (B, A, A in ABBAAB) never reach the stub.
    let mut stop = two_arms(
        "harness-errors-stop",
        &["a", "b"],
        &["--no-build", "runs=3", "secs=35", "out=out"],
    );
    for n in 3..=5 {
        stop.boots.push((format!("boot-{n}.sh"), no_stub_script()));
    }
    v.push(stop);
    // Two in a row, then a good boot: the soak goes on.
    let mut two = two_arms(
        "harness-errors-two",
        &["a", "b"],
        &["--no-build", "runs=3", "secs=35", "out=out"],
    );
    for n in 3..=4 {
        two.boots.push((format!("boot-{n}.sh"), no_stub_script()));
    }
    v.push(two);
    let mut first = two_arms(
        "first-boot-no-stub",
        &["a", "b"],
        &["--no-build", "runs=2", "secs=35", "out=out"],
    );
    first.boots.push(("boot-2.sh".into(), no_stub_script()));
    v.push(first);
    let mut changes = two_arms(
        "qemu-changes",
        &["a", "b"],
        &["--no-build", "runs=2", "secs=35", "out=out"],
    );
    changes.flags = vec!["qemu-changes-at-2".into()];
    v.push(changes);

    // Refusals, each before any boot.
    let build = ["runs=1", "secs=35", "out=out"];
    let no_build = ["--no-build", "runs=1", "secs=35", "out=out"];
    for (suffix, rest) in [("", &build[..]), ("-no-build", &no_build[..])] {
        let named = |base: &str| -> &'static str {
            Box::leak(format!("refuse-{base}{suffix}").into_boxed_str())
        };
        v.push(refusal(named("channel"), rest, |sc| {
            sc.arms[1].channel = Some("nightly-2026-10-01");
        }));
        v.push(refusal(named("rustc"), rest, |sc| {
            sc.arms[1].rustc = "rustc 1.98.0-nightly (fake 2026-10-01)";
        }));
        v.push(refusal(named("firmware"), rest, |sc| {
            sc.arms[1].firmware = Some("fw2.fd");
        }));
        v.push(refusal(named("no-base"), rest, |sc| {
            sc.arms[1].at = At::First;
        }));
        v.push(refusal(named("no-rustup"), rest, |sc| sc.no_rustup = true));
    }
    v.push(refusal("refuse-rustup-fails", &build, |sc| {
        sc.flags = vec!["rustup-fails".into()];
    }));
    v.push(refusal("refuse-build-fails", &build, |sc| {
        sc.flags = vec!["build-fails".into()];
    }));
    v.push(ArmScenario::new("refuse-one-arm", &["a"], &["out=out"]));
    v.push(ArmScenario::new(
        "refuse-five-arms",
        &["a", "b", "a", "b", "a"],
        &["out=out"],
    ));
    v.push(ArmScenario::new(
        "refuse-report-only",
        &["a", "b"],
        &["--report-only", "out=out"],
    ));
    v
}

/// The arm column of `boots.tsv`, in boot order.
fn boot_order(o: &ArmOutcome) -> String {
    o.text("boots.tsv")
        .lines()
        .skip(1)
        .map(|l| l.split('\t').nth(2).expect("an arm cell").to_string())
        .collect()
}

/// The cell of column `name` in each row of a TSV file.
fn column(tsv: &str, name: &str) -> Vec<String> {
    let mut lines = tsv.lines();
    let header: Vec<&str> = lines.next().expect("a header").split('\t').collect();
    let i = header
        .iter()
        .position(|h| *h == name)
        .unwrap_or_else(|| panic!("no column {name}"));
    lines
        .map(|l| l.split('\t').nth(i).expect("a cell").to_string())
        .collect()
}

/// The top-level summary's status line.
fn status(md: &str) -> &str {
    md.lines()
        .find_map(|l| l.strip_prefix("**Status:** "))
        .unwrap_or_else(|| panic!("no status line in\n{md}"))
}

/// The class counts per arm in the top-level summary's table.
fn md_counts(md: &str) -> BTreeMap<(String, String), u64> {
    let at = md.find("### Classes per arm").expect("the class table");
    let mut lines = md[at..].lines().skip(2);
    let header: Vec<String> = lines
        .next()
        .expect("a header")
        .trim_matches('|')
        .split('|')
        .map(|c| c.trim().to_string())
        .collect();
    let mut counts = BTreeMap::new();
    for line in lines.skip(1) {
        if !line.starts_with('|') || line.starts_with("| **Total**") {
            break;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        for (arm, count) in header.iter().zip(&cells).skip(1) {
            let n: u64 = count.parse().expect("a count");
            if n > 0 {
                counts.insert((arm.clone(), cells[0].to_string()), n);
            }
        }
    }
    counts
}

/// The class counts per arm from `boots.tsv`.
fn tsv_counts(o: &ArmOutcome) -> BTreeMap<(String, String), u64> {
    let tsv = o.text("boots.tsv");
    let mut counts = BTreeMap::new();
    for (arm, class) in column(&tsv, "arm").into_iter().zip(column(&tsv, "class")) {
        *counts.entry((arm, class)).or_insert(0) += 1;
    }
    counts
}

/// A refusal: exit 2 with `message` in stderr, and no boot.
fn assert_refused(o: &ArmOutcome, name: &str, message: &str) {
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.code, 2, "{name}: {stderr}");
    assert!(stderr.contains(message), "{name}: {stderr}");
    assert_eq!(o.boots, 0, "{name}: QEMU booted");
}

/// `--classify --out` over an arm's logs writes that arm's `summary.tsv`.
fn assert_single_run_dir(o: &ArmOutcome, arm: &str, logs: &[&str]) {
    let dir = o.out.as_ref().expect("an output directory").join(arm);
    let mut args = vec!["soak", "--report-only", "--classify", "--out", "../again"];
    args.extend(logs);
    let run = run_aios(&dir, &args);
    assert_eq!(run.code, 0, "{}", String::from_utf8_lossy(&run.stderr));
    let again = std::fs::read_to_string(dir.join("../again/summary.tsv")).expect("summary.tsv");
    assert_eq!(again, o.text(&format!("{arm}/summary.tsv")), "{arm}");
    assert!(o
        .text(&format!("{arm}/summary.md"))
        .starts_with("## AIOS QEMU boot soak (text mode)\n"));
    let _ = std::fs::remove_dir_all(dir.join("../again"));
}

/// Checks on each scenario beyond its golden.
fn check(sc: &ArmScenario, o: &ArmOutcome) {
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(
        o.alive.is_empty(),
        "{}: still running: {:?}",
        sc.name,
        o.alive
    );
    assert!(
        !o.names()
            .iter()
            .any(|n| n.contains(".scratch.") || n.ends_with(".tmp")),
        "{}: scratch left: {:?}",
        sc.name,
        o.names()
    );
    let name = sc.name.trim_end_matches("-no-build");
    match name {
        "two-arms" => {
            assert_eq!(o.code, 0, "{stderr}");
            assert_eq!(boot_order(o), "ABBAAB");
            // Non-CLEAN boots in both arms: still exit 0 (report-only).
            assert_eq!(column(&o.text("arm-A/summary.tsv"), "class"), ["PANIC"; 3]);
            assert_eq!(
                column(&o.text("arm-B/summary.tsv"), "class"),
                ["EXCEPTION"; 3]
            );
            // Each arm was built once, in its own checkout, without the
            // harness's toolchain and target directory.
            assert_eq!(o.rustup_log.lines().count(), 2, "{}", o.rustup_log);
            assert!(o
                .rustup_log
                .lines()
                .all(|l| l.starts_with("toolchain install --no-self-update in ")
                    && l.ends_with(" RUSTUP_TOOLCHAIN=unset")));
            assert_eq!(o.just_log.lines().count(), 2, "{}", o.just_log);
            assert!(o
                .just_log
                .lines()
                .all(|l| l.ends_with(" RUSTUP_TOOLCHAIN=unset CARGO_TARGET_DIR=unset")));
            // The arm-base override, on stderr and in Settings.
            assert!(stderr.contains("AIOS_SOAK_MIN_ARM_BASE overrides the arm base"));
            let md = o.text("summary.md");
            assert!(md.contains("(AIOS_SOAK_MIN_ARM_BASE override; the release base is `7167d408f6a43ca9859fb6238f608ca6bdee37d6`, #196) |"), "{md}");
            // The status after each boot, as the next boot's QEMU found it.
            let seen: Vec<String> = o.before.iter().map(|(_, md)| status(md).to_string()).collect();
            let want: Vec<String> = (0..6).map(|k| format!("running ({k} of 6 boots)")).collect();
            assert_eq!(seen, want);
            assert_eq!(status(&md), "finished (6 boots)");
            assert_eq!(md_counts(&md), tsv_counts(o));
            assert_single_run_dir(o, "arm-A", &["run-01.log", "run-02.log", "run-03.log"]);
            assert_single_run_dir(o, "arm-B", &["run-01.log", "run-02.log", "run-03.log"]);
        }
        "a-a" => {
            assert_eq!(o.code, 0, "{stderr}");
            assert_eq!(boot_order(o), "ABBA");
            // Built once, and both labels boot the one snapshot.
            assert_eq!(o.rustup_log.lines().count(), 1, "{}", o.rustup_log);
            assert_eq!(o.just_log.lines().count(), 1, "{}", o.just_log);
            assert!(o.names().contains(&"arm-A/build.log"));
            assert!(!o.names().contains(&"arm-B/build.log"));
            let qemu = column(&o.text("arms.tsv"), "qemu_args");
            assert_eq!(qemu[0], qemu[1]);
            assert!(qemu[0].contains("/esp-A.img,"), "{}", qemu[0]);
        }
        "no-build" | "allow-mixed" | "default-out" => {
            assert_eq!(o.code, 0, "{stderr}");
            assert_eq!(boot_order(o), "AB");
            assert_eq!(o.rustup_log, "", "no rustup toolchain install");
            assert_eq!(o.just_log, "", "no just disk");
            let md = o.text("summary.md");
            let mixed = "| Toolchains | mixed allowed (--allow-mixed-toolchains)";
            assert_eq!(md.contains(mixed), name == "allow-mixed", "{md}");
            if name == "default-out" {
                let out = o.out.as_ref().expect("the default directory");
                let leaf = out.file_name().expect("a name").to_string_lossy();
                assert!(leaf.ends_with("-text-arms"), "{leaf}");
                assert!(out.parent().is_some_and(|p| p.ends_with("repo/target/soak")));
            }
        }
        "harness-errors-stop" => {
            assert_eq!(o.code, 2, "{stderr}");
            assert_eq!(boot_order(o), "ABBAA");
            let md = o.text("summary.md");
            assert_eq!(
                status(&md),
                "stopped (3 boots in a row where the UEFI stub never ran) after 5 of 6 boots"
            );
            assert_eq!(md_counts(&md), tsv_counts(o));
            assert!(!o.names().contains(&"arm-A/summary.md"));
        }
        "harness-errors-two" => {
            assert_eq!(o.code, 0, "{stderr}");
            assert_eq!(boot_order(o), "ABBAAB");
            assert_eq!(status(&o.text("summary.md")), "finished (6 boots)");
        }
        "first-boot-no-stub" => {
            assert_eq!(o.code, 2, "{stderr}");
            assert_eq!(boot_order(o), "A");
            assert_eq!(
                status(&o.text("summary.md")),
                "stopped (the UEFI stub never ran on arm B's first boot) after 1 of 4 boots"
            );
        }
        "qemu-changes" => {
            assert_eq!(o.code, 2, "{stderr}");
            // Boot 2 ran while QEMU changed: it is not counted.
            assert_eq!(o.boots, 2);
            assert_eq!(boot_order(o), "A");
            assert_eq!(
                status(&o.text("summary.md")),
                "stopped (the QEMU binary changed) after 1 of 4 boots"
            );
        }
        "refuse-channel" => assert_refused(o, sc.name, "the arms pin different toolchain channels (arm A: nightly-2026-10-09, arm B: nightly-2026-10-01); pass --allow-mixed-toolchains"),
        "refuse-rustc" => assert_refused(o, sc.name, "the arms' compilers differ (arm A: rustc 1.99.0-nightly (fake 2026-10-09), arm B: rustc 1.98.0-nightly (fake 2026-10-01)); pass --allow-mixed-toolchains"),
        "refuse-firmware" => assert_refused(o, sc.name, "the arms boot different firmware (arm A: "),
        "refuse-no-base" => assert_refused(o, sc.name, "arm B (../arms/b) does not contain "),
        "refuse-no-rustup" => assert_refused(
            o,
            sc.name,
            "rustup not found in PATH: installing each arm's pinned toolchain needs rustup 1.28 or later, and network access the first time",
        ),
        "refuse-rustup-fails" => assert_refused(o, sc.name, "arm A: rustup toolchain install failed in "),
        "refuse-build-fails" => assert_refused(o, sc.name, "arm A: build failed (just disk); full log: "),
        "refuse-one-arm" => assert_refused(o, sc.name, "--arm needs 2 to 4 arms, got 1"),
        "refuse-five-arms" => assert_refused(o, sc.name, "--arm needs 2 to 4 arms, got 5"),
        "refuse-report-only" => assert_refused(o, sc.name, "--report-only cannot be combined with --arm"),
        other => panic!("no checks for {other}"),
    }
    if sc.name.ends_with("-no-build") {
        assert_eq!(o.rustup_log, "", "{}: no rustup toolchain install", sc.name);
        assert_eq!(o.just_log, "", "{}: no just disk", sc.name);
    }
}

#[test]
fn interleave_goldens_match_aios() {
    let all = scenarios();
    let outcomes = parallel(&all, run_arm_scenario);
    let mut diffs = Vec::new();
    for (sc, o) in all.iter().zip(&outcomes) {
        if let Some(d) = check_golden(
            &format!("interleave/{}.golden", sc.name),
            &arm_golden_text(o),
        ) {
            diffs.push(d);
        }
    }
    for (sc, o) in all.iter().zip(&outcomes) {
        check(sc, o);
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n\n"));
}

#[test]
fn sigint_mid_round_leaves_a_stopped_report() {
    // Boot 3 is arm B's second boot (ABBA): it waits for the signal.
    let mut sc = two_arms(
        "interrupt",
        &["a", "b"],
        &["--no-build", "runs=2", "secs=35", "out=out"],
    );
    sc.boots
        .push(("boot-3.sh".into(), wait_for_signal_script()));
    sc.interrupt = Some(("INT", "arm-B/run-02.log"));
    let o = run_arm_scenario(&sc);
    assert_eq!(o.code, 130, "{}", String::from_utf8_lossy(&o.stderr));
    assert!(o.alive.is_empty(), "still running: {:?}", o.alive);
    assert_eq!(
        o.names(),
        [
            "arm-A/run-01.log",
            "arm-A/summary.tsv",
            "arm-B/run-01.log",
            "arm-B/run-02.log",
            "arm-B/summary.tsv",
            "arms.tsv",
            "boots.tsv",
            "summary.md",
        ]
    );
    assert_eq!(boot_order(&o), "AB");
    let md = o.text("summary.md");
    assert_eq!(status(&md), "stopped (SIGINT) after 2 of 4 boots");
    assert_eq!(md_counts(&md), tsv_counts(&o));
    assert_eq!(column(&o.text("arm-A/summary.tsv"), "class"), ["PANIC"]);
}

#[test]
fn sigint_during_a_probe_after_a_boot_is_the_signal_not_a_change() {
    // A terminal Ctrl-C ends the sha256sum that checks QEMU after boot 1: the
    // signal stops the soak, boot 1 (which ran to its end) is counted, and
    // nothing says QEMU changed.
    let mut sc = two_arms(
        "interrupt-sha256",
        &["a", "b"],
        &["--no-build", "runs=2", "secs=35", "out=out"],
    );
    sc.flags = vec!["sha256-interrupted-after-1".into()];
    let o = run_arm_scenario(&sc);
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.code, 130, "{stderr}");
    assert!(!stderr.contains("soak: error:"), "{stderr}");
    assert_eq!(o.boots, 1);
    assert_eq!(boot_order(&o), "A");
    let md = o.text("summary.md");
    assert_eq!(status(&md), "stopped (SIGINT) after 1 of 4 boots");
    assert_eq!(md_counts(&md), tsv_counts(&o));
    assert_eq!(column(&o.text("arm-A/summary.tsv"), "class"), ["PANIC"]);
}

/// Write `text` to `root/rel`, creating its directory, with mtime `stamp`
/// (`touch -t` format).
fn put(root: &Path, rel: &str, text: &str, stamp: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
    std::fs::write(&path, text).expect("write");
    let ok = Command::new("touch")
        .args(["-t", stamp])
        .arg(&path)
        .status()
        .expect("run touch");
    assert!(ok.success(), "touch {rel}");
}

#[test]
fn brief_lists_an_interleaved_run_once_and_never_as_main_soak() {
    let repo = TestRepo::with_files("brief-soak", &[("README", "x\n")]);
    let root = repo.path();
    git(root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let sha = git(root, &["rev-parse", "--short", "HEAD"])
        .trim()
        .to_string();
    let single_md = format!(
        "## AIOS QEMU boot soak (text mode)\n\n| Setting | Value |\n|---|---|\n| Commit | `{sha}` (kernel ELF sha256 `0`) |\n| Boots | 2 x 75s, stall limit 15s, data disk fresh per boot |\n"
    );
    let single_tsv = "run\tmode\tclass\n01\ttext\tCLEAN\n02\ttext\tPANIC\n";
    put(
        root,
        "target/soak/20261001-000000-text/summary.md",
        &single_md,
        "202610010000",
    );
    put(
        root,
        "target/soak/20261001-000000-text/summary.tsv",
        single_tsv,
        "202610010000",
    );
    // A newer interleaved run whose arms are main's tip: its arm directories
    // would be the newest main soak if they were read as runs.
    let il = "target/soak/20261002-000000-text-arms";
    put(
        root,
        &format!("{il}/arms.tsv"),
        &format!("arm\tcheckout\troot\tcommit\nA\t../a\t/a\t{sha}\nB\t../b\t/b\t{sha}\n"),
        "202610020000",
    );
    put(
        root,
        &format!("{il}/boots.tsv"),
        "round\tposition\tarm\trun\tmode\tclass\n1\t1\tA\t01\ttext\tCLEAN\n1\t2\tB\t01\ttext\tPANIC\n2\t1\tB\t02\ttext\tCLEAN\n2\t2\tA\t02\ttext\tCLEAN\n",
        "202610020000",
    );
    put(
        root,
        &format!("{il}/summary.md"),
        "## AIOS QEMU interleaved soak (text mode)\n\n**Status:** finished (4 boots)\n",
        "202610020000",
    );
    for arm in ["A", "B"] {
        put(
            root,
            &format!("{il}/arm-{arm}/summary.md"),
            &single_md,
            "202610020000",
        );
        put(
            root,
            &format!("{il}/arm-{arm}/summary.tsv"),
            single_tsv,
            "202610020000",
        );
    }

    // gh fails, so every GitHub section degrades; no network is used.
    let bin = unique_dir("brief-bin");
    std::fs::write(
        bin.join("gh"),
        "#!/bin/sh\necho 'fake gh: offline' >&2\nexit 1\n",
    )
    .expect("gh");
    let chmod = Command::new("chmod")
        .arg("755")
        .arg(bin.join("gh"))
        .status()
        .expect("chmod");
    assert!(chmod.success());
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/agent/brief.sh");
    let mut path = bin.into_os_string();
    path.push(":/usr/bin:/bin:/usr/sbin:/sbin");
    let mut cmd = Command::new("bash");
    let out = isolated(&mut cmd)
        .arg(&script)
        .arg("--no-fetch")
        .current_dir(root)
        .env("PATH", &path)
        .output()
        .expect("run brief.sh");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let soak = text
        .split("\n## Soak\n")
        .nth(1)
        .and_then(|s| s.split("\n## ").next())
        .unwrap_or_else(|| panic!("no Soak section in\n{text}"));
    assert!(
        soak.contains("- main soak: target/soak/20261001-000000-text (worktree on `main`"),
        "{soak}"
    );
    assert_eq!(
        soak.matches("20261002-000000-text-arms").count(),
        1,
        "{soak}"
    );
    assert!(
        soak.contains(
            "- newest interleaved soak: target/soak/20261002-000000-text-arms (worktree on `main`"
        ),
        "{soak}"
    );
    assert!(
        soak.contains("\n  - status: finished (4 boots)\n"),
        "{soak}"
    );
    assert!(
        soak.contains(&format!(
            "\n  - CLEAN per arm: arm A `{sha}` 2/2; arm B `{sha}` 1/2\n"
        )),
        "{soak}"
    );
    assert!(
        !soak.contains("arm-A") && !soak.contains("newest other soak"),
        "{soak}"
    );
}
