//! `aios soak --classify` on logs of both tripwire schema versions
//! (capability-lifetime step 11): a `v=1` line is decoded with the frozen
//! v1 catalogue and a `v=2` line with the current one, and a log of each
//! side by side joins them by key and index name. Any other version is kept
//! whole.
//!
//! The logs are shaped like a soak arm's `run-NN.log` (a stub banner, the
//! kernel banner, heartbeats and one tripwire line each); the `main` arm's
//! logs of the B1/B2 soaks are `v=1` lines of this shape, without the keys
//! and the index that capability-lifetime steps 8 to 10 appended.

mod common;

use aios_tools::cmd::soak::catalogue::{union_keys, Schema};
use common::{run_aios, unique_dir};
use std::path::Path;

/// A log whose last tripwire line is `tripwire`.
fn log(tripwire: &str) -> String {
    format!(
        "AIOS UEFI stub v0.1.0\nAIOS kernel booting\n[heartbeat] tick=0\n\
         [bench] === Gate 1 Benchmark ===\n{tripwire}\n[heartbeat] tick=1000\n"
    )
}

/// `main`'s arm (`v=1`, 15-wide sources and no `ubtok`): `cancel` is source
/// 14 and BOOT_LOG lock 8; one `selcall` and one SELECT_WAITERS.
const V1_LINE: &str = "[tripwire] v=1 src=hb cpu=0 t=1000 ncpu=4 tick=1000,0,0,0 \
    ubrun=0,0,0,2,0,0,0,0,0,0,0,0,0,0,7 lkph=0,0,0,0,0,0,0,1,5 twc=9 twn=1 twmax=50 n=11";

/// The branch's arm (`v=2`): `cancel` is source 10 and BOOT_LOG lock 7.
const V2_LINE: &str = "[tripwire] v=2 src=hb cpu=0 t=1000 ncpu=4 tick=1000,0,0,0 \
    ubrun=0,0,0,0,0,0,0,0,0,0,4,3 lkph=0,0,0,0,0,0,0,6 twc=9 twn=1 twmax=50 n=11";

/// Classify `logs` (name, text) with `--out out`, in a fresh directory;
/// returns the directory and the run.
fn classify(label: &str, logs: &[(&str, String)]) -> (std::path::PathBuf, common::Run) {
    let dir = unique_dir(label);
    for (name, text) in logs {
        std::fs::write(dir.join(name), text).expect("write a log");
    }
    let mut args = vec!["soak", "--report-only", "--classify", "--out", "out"];
    args.extend(logs.iter().map(|(n, _)| *n));
    let run = run_aios(&dir, &args);
    (dir, run)
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).expect("read an output file")
}

/// The `name` cell of the TSV row `row` (counted after the header).
fn cell<'a>(tsv: &'a str, row: usize, name: &str) -> &'a str {
    let mut lines = tsv.lines();
    let header: Vec<&str> = lines.next().expect("a header").split('\t').collect();
    let col = header
        .iter()
        .position(|h| *h == name)
        .unwrap_or_else(|| panic!("no column {name}"));
    lines
        .nth(row)
        .expect("a row")
        .split('\t')
        .nth(col)
        .expect("a cell")
}

#[test]
fn a_v1_and_a_v2_log_decode_side_by_side_with_their_own_catalogue() {
    let (dir, run) = classify(
        "soak-tripwire-versions",
        &[("v1.log", log(V1_LINE)), ("v2.log", log(V2_LINE))],
    );
    assert_eq!(run.code, 0, "{}", String::from_utf8_lossy(&run.stderr));
    let stdout = String::from_utf8(run.stdout).expect("UTF-8");
    // The version changes no classification.
    let class = |name: &str| {
        stdout
            .lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .unwrap_or_else(|| panic!("no result line for {name}"))
            .to_string()
    };
    assert_eq!(class("v1.log"), "WEDGE-ALIVE");
    assert_eq!(class("v2.log"), "WEDGE-ALIVE");

    // summary.tsv: one row per log, each with its own version and its
    // values as printed; a late key a v=1 line lacks reads 0.
    let tsv = read(&dir, "out/summary.tsv");
    for (row, version) in [(0, "1"), (1, "2")] {
        assert_eq!(cell(&tsv, row, "tw_v"), version);
        assert_eq!(cell(&tsv, row, "tw_src"), "hb");
        assert_eq!(cell(&tsv, row, "tw_twc"), "9");
        assert_eq!(cell(&tsv, row, "tw_bw"), "0");
    }
    assert_eq!(cell(&tsv, 0, "tw_ubrun"), "0,0,0,2,0,0,0,0,0,0,0,0,0,0,7");
    assert_eq!(cell(&tsv, 1, "tw_ubrun"), "0,0,0,0,0,0,0,0,0,0,4,3");

    // summary.md: both boots are in one WEDGE-ALIVE column, and each
    // counter value lands in the row its own catalogue names.
    let md = read(&dir, "out/summary.md");
    for want in [
        "| Boots with a `v=1` or `v=2` line | 2 |\n",
        "| `ubrun[selcall]` | 1/2 |\n",
        "| `ubrun[cancel]` | 2/11 |\n",
        "| `ubrun[selftest]` | 1/3 |\n",
        "| `lkph[SELECT_WAITERS]` | 1/1 |\n",
        "| `lkph[BOOT_LOG]` | 2/11 |\n",
    ] {
        assert!(md.contains(want), "no {want:?} in\n{md}");
    }
    // v=2 source 10 is `cancel`, not v=1's `pwait`, and v=2 lock 7 is
    // BOOT_LOG, not v=1's SELECT_WAITERS: no row reads the other's name.
    for unwanted in ["ubrun[pwait]", "ubrun[to]", "lkph[NOTIFICATION_TABLE]"] {
        assert!(!md.contains(unwanted), "{unwanted} in\n{md}");
    }
}

#[test]
fn the_tsv_header_has_a_column_for_every_key_of_both_catalogues() {
    let (dir, run) = classify("soak-tripwire-header", &[("v1.log", log(V1_LINE))]);
    assert_eq!(run.code, 0);
    let tsv = read(&dir, "out/summary.tsv");
    let header: Vec<&str> = tsv.lines().next().expect("a header").split('\t').collect();
    for schema in Schema::ALL {
        for key in schema.keys() {
            let column = format!("tw_{}", key.name);
            assert!(header.contains(&column.as_str()), "no {column}");
        }
    }
    // One column per key of the union, each once.
    for key in union_keys() {
        let column = format!("tw_{}", key.name);
        assert_eq!(
            header.iter().filter(|h| **h == column).count(),
            1,
            "{column}"
        );
    }
}

#[test]
fn a_line_of_any_other_version_is_kept_whole_and_not_decoded() {
    let line = "[tripwire] v=99 src=hb cpu=0 t=1000 ncpu=4 tick=1000,0,0,0 twc=9 n=7";
    let (dir, run) = classify("soak-tripwire-other", &[("v99.log", log(line))]);
    assert_eq!(run.code, 0);
    let tsv = read(&dir, "out/summary.tsv");
    assert_eq!(cell(&tsv, 0, "tw_v"), "99");
    for name in ["tw_src", "tw_tick", "tw_twc", "tw_ubrun"] {
        assert_eq!(cell(&tsv, 0, name), "-", "{name}");
    }
    assert_eq!(cell(&tsv, 0, "tw_line"), line);
    let md = read(&dir, "out/summary.md");
    assert!(
        md.contains("No boot has a complete `v=1` or `v=2` tripwire line.\n"),
        "{md}"
    );
    assert!(
        md.contains("| WEDGE-ALIVE |") && md.contains("| v=99 |"),
        "{md}"
    );
}
