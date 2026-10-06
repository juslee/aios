//! Classifier parity with `CLASSIFY_AWK` in the deleted `scripts/soak-qemu.sh`.
//!
//! - `classify_goldens_match_aios` compares the classifier line of every case in
//!   `tests/fixtures/soak/synthetic.txt` with `tests/golden/soak/classify.golden`
//!   (`<case>\t<11 fields>` per line), recorded from the oracle.
//! - `record_classify_goldens_from_oracle` (ignored) rewrites that file from the
//!   oracle: the script at `ORACLE_COMMIT`, read from git history.
//! - `classify_differential_against_oracle` runs the oracle and aios side by side
//!   on every case; it keeps running after the switch-over deleted the script.
//! - `classify_differential_on_real_logs` (ignored) does the same for every
//!   `*.log` under the directories in `AIOS_SOAK_REAL_LOGS` (colon-separated):
//!   real soak logs are never committed (owner decision, 2026-09-29).

mod common;

use aios_tools::cmd::soak::classify::{classify, CLASSES};
use common::soak::{check_golden, golden_dir, oracle_classify, synthetic_cases, write_cases};
use common::unique_dir;
use std::path::{Path, PathBuf};

/// `<case>\t<classifier line>\n` for every case, the classify.golden format.
fn golden_lines<'a>(rows: impl IntoIterator<Item = (&'a str, Vec<u8>)>) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, line) in rows {
        out.extend_from_slice(name.as_bytes());
        out.push(b'\t');
        out.extend(line);
        out.push(b'\n');
    }
    out
}

#[test]
fn corpus_covers_every_class_and_stays_public_safe() {
    let cases = synthetic_cases();
    assert_eq!(cases.len(), 108, "update this count with the corpus");
    for class in CLASSES {
        let n = cases
            .iter()
            .filter(|c| classify(&c.log, c.stall).class == class)
            .count();
        assert!(n >= 5, "only {n} {class} cases");
    }
    let text = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/soak/synthetic.txt"),
    )
    .expect("the corpus");
    assert!(
        !text.windows(7).any(|w| w == b"/Users/"),
        "a local path in the corpus"
    );
    assert!(
        !text.windows(6).any(|w| w == b"/home/"),
        "a local path in the corpus"
    );
}

#[test]
fn classify_goldens_match_aios() {
    let cases = synthetic_cases();
    let actual = golden_lines(
        cases
            .iter()
            .map(|c| (c.name.as_str(), classify(&c.log, c.stall).line())),
    );
    if let Some(diff) = check_golden("classify.golden", &actual) {
        panic!("{diff}");
    }
}

#[test]
#[ignore = "rewrites tests/golden/soak/classify.golden from the oracle"]
fn record_classify_goldens_from_oracle() {
    let dir = unique_dir("soak-record-classify");
    let cases = synthetic_cases();
    let rels = write_cases(&dir, &cases);
    let golden = golden_lines(
        cases
            .iter()
            .zip(&rels)
            .map(|(c, rel)| (c.name.as_str(), oracle_classify(&dir.join(rel), c.stall))),
    );
    std::fs::create_dir_all(golden_dir()).expect("golden dir");
    std::fs::write(golden_dir().join("classify.golden"), golden).expect("write the golden");
}

#[test]
fn classify_differential_against_oracle() {
    let dir = unique_dir("soak-diff-classify");
    let cases = synthetic_cases();
    let rels = write_cases(&dir, &cases);
    let mut diffs = Vec::new();
    for (case, rel) in cases.iter().zip(&rels) {
        let want = oracle_classify(&dir.join(rel), case.stall);
        let got = classify(&case.log, case.stall).line();
        if want != got {
            diffs.push(format!(
                "{}\n  oracle {}\n  aios   {}",
                case.name,
                String::from_utf8_lossy(&want),
                String::from_utf8_lossy(&got)
            ));
        }
    }
    assert!(
        diffs.is_empty(),
        "{} of {} cases differ:\n{}",
        diffs.len(),
        cases.len(),
        diffs.join("\n")
    );
}

/// Every `*.log` below `dir`, sorted.
fn logs_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.map(|e| e.expect("an entry").path()).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            logs_under(&p, out);
        } else if p.extension().is_some_and(|e| e == "log") {
            out.push(p);
        }
    }
}

#[test]
#[ignore = "needs local real soak logs: AIOS_SOAK_REAL_LOGS=dir[:dir...]"]
fn classify_differential_on_real_logs() {
    let dirs = std::env::var_os("AIOS_SOAK_REAL_LOGS")
        .expect("set AIOS_SOAK_REAL_LOGS to directories of soak logs");
    let mut logs = Vec::new();
    for dir in std::env::split_paths(&dirs) {
        logs_under(&dir, &mut logs);
    }
    assert!(!logs.is_empty(), "no *.log files under AIOS_SOAK_REAL_LOGS");
    let mut counts = std::collections::BTreeMap::new();
    let mut diffs = Vec::new();
    for log in &logs {
        let raw = std::fs::read(log).expect("read a log");
        let got = classify(&raw, None);
        *counts.entry(got.class).or_insert(0) += 1;
        let want = oracle_classify(log, None);
        if want != got.line() {
            diffs.push(log.display().to_string());
        }
    }
    eprintln!("{} real logs: {counts:?}", logs.len());
    assert!(
        diffs.is_empty(),
        "{} logs differ:\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}
