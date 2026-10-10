//! `aios soak` on the command line: `--classify` output and exit status, and
//! the usage errors.
//!
//! - `cli_goldens_match_aios` replays every case of `CASES` and compares
//!   `exit N`, stdout and stderr (and, for a case with `--out DIR` or
//!   `out=DIR`, the files written there) with
//!   `tests/golden/soak/cli/<case>.golden`.
//!   The goldens were recorded from the deleted `scripts/soak-qemu.sh` (R4's
//!   oracle) and are kept by aios since crash-fix step 1a split its classes;
//!   `AIOS_BLESS_GOLDENS=1` rewrites them from aios.
//! - `help_prints_the_usage_whatever_came_before` and
//!   `classify_works_outside_a_git_checkout` cover the rest.
//!
//! Every case runs in a directory holding the corpus as `cases/<name>.txt`, and
//! none of them reaches the boot loop (tests/soak_harness.rs covers that).

mod common;

use aios_tools::cmd::soak::USAGE;
use common::soak::{check_golden, synthetic_cases, write_cases};
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
    (
        "classify-out",
        &[
            "--report-only",
            "--classify",
            "--out",
            "out-a",
            "cases/clean-text.txt",
            "cases/tripwire-panic-lock-with-events.txt",
            "cases/tripwire-clean-g1-then-hb.txt",
            "cases/tripwire-other-schema-last-wedge-alive.txt",
            "cases/clean-text-log-only.txt",
        ],
    ),
    (
        "classify-out-kv-stall",
        &[
            "out=out-b",
            "--stall-secs",
            "5",
            "--classify",
            "cases/stall-override-wedge.txt",
            "cases/clean-text.txt",
        ],
    ),
    (
        "classify-out-not-empty",
        &["--classify", "--out", "cases", "cases/clean-text.txt"],
    ),
    (
        "classify-out-is-a-file",
        &[
            "--classify",
            "--out",
            "cases/clean-text.txt",
            "cases/clean-text.txt",
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

/// The `--out DIR` or `out=DIR` of a case, if any.
fn out_of(case: &[&str]) -> Option<String> {
    case.iter().enumerate().find_map(|(i, a)| {
        if *a == "--out" {
            case.get(i + 1).map(|v| v.to_string())
        } else {
            a.strip_prefix("out=").map(str::to_string)
        }
    })
}

/// `data` with `dir` (canonical or as given) replaced by `<DIR>`.
fn without_dir(data: &[u8], dir: &Path) -> Vec<u8> {
    let canonical = std::fs::canonicalize(dir).expect("canonicalize the case dir");
    let mut text = String::from_utf8_lossy(data).into_owned();
    for form in [canonical.as_path(), dir] {
        text = text.replace(&*form.to_string_lossy(), "<DIR>");
    }
    text.into_bytes()
}

/// `exit N`, stdout and stderr, then each file in `out` (a `--out`
/// directory the case created): the golden format. `dir` is the case's
/// working directory.
fn outcome(run: &Run, dir: &Path, out: Option<&str>) -> Vec<u8> {
    let mut g = format!("exit {}\n--- stdout\n", run.code).into_bytes();
    g.extend_from_slice(&run.stdout);
    g.extend_from_slice(b"--- stderr\n");
    g.extend_from_slice(&run.stderr);
    if let Some(out) = out {
        let mut names: Vec<String> = std::fs::read_dir(dir.join(out))
            .map(|entries| {
                entries
                    .map(|e| {
                        e.expect("an entry")
                            .file_name()
                            .to_string_lossy()
                            .into_owned()
                    })
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        for name in names {
            g.extend(format!("--- file {out}/{name}\n").into_bytes());
            g.extend(std::fs::read(dir.join(out).join(&name)).expect("read an output file"));
        }
    }
    without_dir(&g, dir)
}

fn run_aios_soak(dir: &Path, args: &[String]) -> Run {
    let mut full = vec!["soak"];
    full.extend(args.iter().map(String::as_str));
    run_aios(dir, &full)
}

#[test]
fn cli_goldens_match_aios() {
    let (dir, all) = corpus_dir("soak-cli-goldens");
    let diffs: Vec<String> = CASES
        .iter()
        .filter_map(|(name, case)| {
            // Only an output directory the case creates is part of its golden.
            let out = out_of(case).filter(|o| !dir.join(o).exists());
            let run = run_aios_soak(&dir, &args_of(case, &all));
            check_golden(
                &format!("cli/{name}.golden"),
                &outcome(&run, &dir, out.as_deref()),
            )
        })
        .collect();
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
        out.stdout.starts_with(b"a.log  WEDGE-STUCK "),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let _ = std::fs::remove_dir_all(&parent);
}
