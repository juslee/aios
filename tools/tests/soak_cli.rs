//! `aios soak` command-line parity with the deleted `scripts/soak-qemu.sh`:
//! `--classify` output and exit status, and the usage errors, byte for byte
//! except for the documented `soak-qemu:` -> `soak:` message prefix.
//!
//! - `cli_goldens_match_aios` replays every case of `CASES` and compares
//!   `exit N`, stdout and stderr with `tests/golden/soak/cli/<case>.golden`.
//! - `record_cli_goldens_from_oracle` (ignored) records them from the oracle.
//! - `cli_differential_against_oracle` runs both tools on every case.
//!
//! Every case runs in a directory holding the corpus as `cases/<name>.txt`, and
//! none of them reaches the boot loop (tests/soak_harness.rs covers that).

mod common;

use aios_tools::cmd::soak::USAGE;
use common::soak::{
    check_golden, golden_dir, rename_prefix, run_oracle, synthetic_cases, write_cases,
};
use common::{isolated, run_aios, unique_dir, Run};
use std::path::{Path, PathBuf};

/// The command lines, by golden name. `ALL` stands for every corpus file.
const CASES: &[(&str, &[&str])] = &[
    ("classify-all", &["--classify", "ALL"]),
    (
        "classify-report-only",
        &[
            "--report-only",
            "--classify",
            "cases/clean-text.txt",
            "cases/panic-with-message.txt",
        ],
    ),
    (
        "classify-report-only-kv",
        &[
            "report_only=yes",
            "--classify",
            "cases/clean-text.txt",
            "cases/panic-with-message.txt",
        ],
    ),
    (
        "classify-stall-override",
        &[
            "--stall-secs",
            "5",
            "--classify",
            "cases/stall-override-wedge.txt",
            "cases/clean-text.txt",
        ],
    ),
    (
        "classify-stall-after-classify",
        &[
            "--classify",
            "--stall-secs=5",
            "cases/stall-override-wedge.txt",
        ],
    ),
    (
        "classify-stall-kv",
        &["stall_secs=40", "--classify", "cases/hb-stopped.txt"],
    ),
    (
        "classify-stall-leading-zeros",
        &[
            "--stall-secs",
            "007",
            "--classify",
            "cases/stall-override-wedge.txt",
        ],
    ),
    ("classify-no-files", &["--classify"]),
    (
        "classify-missing-file",
        &["--classify", "cases/clean-text.txt", "cases/nope.txt"],
    ),
    ("classify-directory", &["--classify", "cases"]),
    (
        "classify-dashdash",
        &["--classify", "--", "cases/clean-text.txt"],
    ),
    (
        "classify-option-after-file",
        &["--classify", "cases/clean-text.txt", "--report-only"],
    ),
    (
        "classify-stall-zero",
        &["--stall-secs", "0", "--classify", "cases/clean-text.txt"],
    ),
    (
        "classify-stall-not-a-number",
        &["--stall-secs", "1x", "--classify", "cases/clean-text.txt"],
    ),
    (
        "classify-stall-empty",
        &["stall_secs=", "--classify", "cases/clean-text.txt"],
    ),
    ("unknown-option", &["--bogus"]),
    ("unknown-short-option", &["-x"]),
    ("lone-dash", &["-"]),
    ("report-only-with-value", &["--report-only=1"]),
    ("unexpected-argument", &["foo"]),
    ("unexpected-after-dashdash", &["--", "foo"]),
    (
        "leading-dashdash-then-classify",
        &["--", "--classify", "cases/clean-text.txt"],
    ),
    ("needs-a-value", &["--runs"]),
    ("runs-zero", &["runs=0"]),
    ("runs-not-a-number", &["--runs", "abc"]),
    ("secs-empty", &["secs="]),
    (
        "stall-not-below-secs",
        &["--secs", "10", "--stall-secs", "10"],
    ),
    ("mode-unknown", &["mode=fast"]),
    ("report-only-not-a-boolean", &["report_only=maybe"]),
    (
        "boolean-error-before-help",
        &["report_only=maybe", "--help"],
    ),
];

/// A directory with the corpus written under `cases/`, and the case file paths.
fn corpus_dir(label: &str) -> (PathBuf, Vec<String>) {
    let dir = unique_dir(label);
    let rels = write_cases(&dir, &synthetic_cases());
    (dir, rels)
}

fn args_of(case: &[&str], all: &[String]) -> Vec<String> {
    case.iter()
        .flat_map(|a| {
            if *a == "ALL" {
                all.to_vec()
            } else {
                vec![a.to_string()]
            }
        })
        .collect()
}

/// `exit N`, stdout and stderr: the golden format.
fn outcome(run: &Run) -> Vec<u8> {
    let mut g = format!("exit {}\n--- stdout\n", run.code).into_bytes();
    g.extend_from_slice(&run.stdout);
    g.extend_from_slice(b"--- stderr\n");
    g.extend_from_slice(&run.stderr);
    g
}

fn run_aios_soak(dir: &Path, args: &[String]) -> Run {
    let mut full = vec!["soak"];
    full.extend(args.iter().map(String::as_str));
    run_aios(dir, &full)
}

fn run_oracle_renamed(dir: &Path, args: &[String]) -> Run {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut run = run_oracle(dir, &refs, &[]);
    run.stderr = rename_prefix(&run.stderr);
    run
}

#[test]
fn cli_goldens_match_aios() {
    let (dir, all) = corpus_dir("soak-cli-goldens");
    let diffs: Vec<String> = CASES
        .iter()
        .filter_map(|(name, case)| {
            check_golden(
                &format!("cli/{name}.golden"),
                &outcome(&run_aios_soak(&dir, &args_of(case, &all))),
            )
        })
        .collect();
    assert!(diffs.is_empty(), "{}", diffs.join("\n\n"));
}

#[test]
#[ignore = "rewrites tests/golden/soak/cli/ from the oracle"]
fn record_cli_goldens_from_oracle() {
    let (dir, all) = corpus_dir("soak-cli-record");
    std::fs::create_dir_all(golden_dir().join("cli")).expect("golden dir");
    for (name, case) in CASES {
        let run = run_oracle_renamed(&dir, &args_of(case, &all));
        std::fs::write(
            golden_dir().join(format!("cli/{name}.golden")),
            outcome(&run),
        )
        .expect("write a golden");
    }
}

#[test]
fn cli_differential_against_oracle() {
    let (dir, all) = corpus_dir("soak-cli-diff");
    let mut diffs = Vec::new();
    for (name, case) in CASES {
        let args = args_of(case, &all);
        let want = outcome(&run_oracle_renamed(&dir, &args));
        let got = outcome(&run_aios_soak(&dir, &args));
        if want != got {
            diffs.push(format!(
                "{name}\n--- oracle\n{}\n--- aios\n{}",
                String::from_utf8_lossy(&want),
                String::from_utf8_lossy(&got)
            ));
        }
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n\n"));
}

#[test]
fn help_prints_the_usage_whatever_came_before() {
    let dir = unique_dir("soak-cli-help");
    for args in [
        &["--help"][..],
        &["-h"],
        &["--runs", "x", "-h"],
        &["--classify", "-h"],
    ] {
        let mut full = vec!["soak"];
        full.extend_from_slice(args);
        let run = run_aios(&dir, &full);
        assert_eq!(run.code, 0, "{args:?}");
        assert_eq!(run.stdout, USAGE.as_bytes(), "{args:?}");
        assert!(run.stderr.is_empty(), "{args:?}");
    }
}

#[test]
fn classify_works_outside_a_git_checkout() {
    // Not unique_dir: that lives under target/, inside this checkout. The ceiling
    // stops git's search at the directory itself, wherever the temp dir is.
    let parent = std::env::temp_dir().join(format!("aios-soak-cli-nogit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(parent.join("logs")).expect("create the temp dir");
    let parent = std::fs::canonicalize(&parent).expect("canonicalize the temp dir");
    let dir = parent.join("logs");
    std::fs::write(dir.join("a.log"), "AIOS UEFI stub\n").expect("write a log");
    let run_in_dir = |program: &str, args: &[&str]| {
        let mut cmd = std::process::Command::new(program);
        isolated(&mut cmd)
            .env("GIT_CEILING_DIRECTORIES", &parent)
            .current_dir(&dir)
            .args(args);
        cmd.output().expect("run a program in the temp dir")
    };
    let git = run_in_dir("git", &["rev-parse", "--show-toplevel"]);
    assert!(
        !git.status.success(),
        "{} is inside a git checkout",
        dir.display()
    );
    let out = run_in_dir(
        env!("CARGO_BIN_EXE_aios"),
        &["soak", "--report-only", "--classify", "a.log"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stdout.starts_with(b"a.log  WEDGE "),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let _ = std::fs::remove_dir_all(&parent);
}
