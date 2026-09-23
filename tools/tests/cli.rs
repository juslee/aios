//! `aios docs-check` command line and pipeline: usage errors, the repository root,
//! the output formats with zero checks, baseline handling, and `run_checks` /
//! `run_with` on stand-in checks. Expected bytes were recorded from check.py
//! (33c6b3d) on the same inputs; its `CHECK_FUNCS` was replaced by the same
//! stand-ins for the pipeline cases.

mod common;

use std::path::Path;
use std::process::Command;

use aios_tools::cmd::docs_check::checks::{registry, Check};
use aios_tools::cmd::docs_check::model::{Finding, Skip};
use aios_tools::cmd::docs_check::output::render_list_checks;
use aios_tools::cmd::docs_check::repo::Repo;
use aios_tools::cmd::docs_check::{run_checks, run_with, select_from, Args};
use common::{run_aios, Run, TestRepo};

const ZERO_TEXT: &str = concat!(
    "docs-check: 0 findings across 0 checks - 0 new, 0 baselined (0 accepted false positives), ",
    "0 resolved (baseline scripts/docs/baseline.json)\n",
    "\n",
    "  check              total   new\n",
    "\n",
    "No new drift since baseline.\n",
);

const ZERO_JSON: &str = r##"{
  "baseline": "scripts/docs/baseline.json",
  "summary": {
    "total": 0,
    "new": 0,
    "baselined": 0,
    "accepted": 0,
    "resolved": 0,
    "reduced": 0
  },
  "checks": {},
  "findings": [],
  "resolved": [],
  "reduced": {}
}
"##;

const ZERO_MARKDOWN: &str = concat!(
    "## Docs drift check\n",
    "\n",
    "**0 new** finding(s) since baseline; 0 total (0 accepted false positives), 0 resolved. ",
    "Report-only: fix new drift in this PR, or accept it with `just docs-check --update-baseline`.\n",
    "\n",
    "| Check | Total | New |\n",
    "|---|---:|---:|\n",
);

const HAND_BASELINE: &str = r##"{
  "comment": "hand written",
  "version": 1,
  "counts": {"md-links": 7},
  "findings": [
    {"key": "retired|README.md|x", "check": "retired", "file": "README.md", "target": "x", "message": "kept", "note": "extra"},
    {"key": "md-links|b.md|é.md", "check": "md-links", "file": "b.md", "target": "é.md", "message": "broken link -> é.md", "count": 2},
    {"key": "anchors|a.md|#x", "check": "anchors", "file": "a.md", "target": "#x", "message": "no heading", "reason": "accepted"}
  ]
}
"##;

const HAND_REWRITTEN: &str = r##"{
  "comment": "Accepted docs drift. A finding is new when its key is missing here or it occurs on more lines than 'count' (default 1). 'reason' marks an accepted false positive and survives regeneration. Regenerate with: just docs-check --update-baseline",
  "version": 1,
  "counts": {
    "md-links": 1,
    "anchors": 1
  },
  "findings": [
    {
      "key": "md-links|b.md|é.md",
      "check": "md-links",
      "file": "b.md",
      "target": "é.md",
      "message": "broken link -> é.md",
      "count": 2
    },
    {
      "key": "anchors|a.md|#x",
      "check": "anchors",
      "file": "a.md",
      "target": "#x",
      "message": "no heading",
      "reason": "accepted"
    },
    {
      "key": "retired|README.md|x",
      "check": "retired",
      "file": "README.md",
      "target": "x",
      "message": "kept",
      "note": "extra"
    }
  ]
}
"##;

const PIPELINE_BASELINE: &str = r##"{
  "comment": "pipeline fixture",
  "version": 1,
  "counts": {},
  "findings": [
    {"key": "milestone-status|README.md|latest:M3", "check": "milestone-status", "file": "README.md", "target": "latest:M3", "message": "README status does not mention the latest merged milestone M3"},
    {"key": "md-links|docs/old.md|z.md", "check": "md-links", "file": "docs/old.md", "target": "z.md", "message": "broken link -> z.md"},
    {"key": "anchors|docs/a.md|#gone", "check": "anchors", "file": "docs/a.md", "target": "#gone", "message": "no heading for anchor #gone in docs/a.md", "reason": "false positive"},
    {"key": "md-links|README.md|y.md", "check": "md-links", "file": "README.md", "target": "y.md", "message": "broken link -> y.md"}
  ]
}
"##;

const PIPELINE_TEXT: &str = r##"docs-check: 3 findings across 2 checks - 1 new, 2 baselined (1 accepted false positives), 1 resolved (baseline scripts/docs/baseline.json)

  check              total   new
  md-links               2     1
  anchors                1     0
  milestone-status    skip        no 'Phase N MK:' commits found on main's first-parent history

New drift since baseline:

[md-links]
 + docs/a.md:7 (also 3): broken link -> x.md

1 baselined finding(s) no longer occur or occur less often; prune them with `just docs-check --update-baseline`:
  - md-links|docs/old.md|z.md
"##;

const PIPELINE_REWRITTEN: &str = r##"{
  "comment": "Accepted docs drift. A finding is new when its key is missing here or it occurs on more lines than 'count' (default 1). 'reason' marks an accepted false positive and survives regeneration. Regenerate with: just docs-check --update-baseline",
  "version": 1,
  "counts": {
    "md-links": 2,
    "anchors": 1,
    "milestone-status": 1
  },
  "findings": [
    {
      "key": "md-links|README.md|y.md",
      "check": "md-links",
      "file": "README.md",
      "target": "y.md",
      "message": "broken link -> y.md"
    },
    {
      "key": "md-links|docs/a.md|x.md",
      "check": "md-links",
      "file": "docs/a.md",
      "target": "x.md",
      "message": "broken link -> x.md",
      "count": 2
    },
    {
      "key": "anchors|docs/a.md|#gone",
      "check": "anchors",
      "file": "docs/a.md",
      "target": "#gone",
      "message": "no heading for anchor #gone in docs/a.md",
      "reason": "false positive"
    },
    {
      "key": "milestone-status|README.md|latest:M3",
      "check": "milestone-status",
      "file": "README.md",
      "target": "latest:M3",
      "message": "README status does not mention the latest merged milestone M3"
    }
  ]
}
"##;

const NO_PHASE_HISTORY: &str = "no 'Phase N MK:' commits found on main's first-parent history";

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("UTF-8 output")
}

/// Runs aios in `dir` with git unable to find a repository above it.
fn aios_outside_git(dir: &Path, args: &[&str]) -> Run {
    let ceiling = dir.parent().expect("unique_dir has a parent");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aios"));
    common::isolated(&mut cmd)
        .current_dir(dir)
        .env("GIT_CEILING_DIRECTORIES", ceiling)
        .args(args);
    let out = cmd.output().expect("run aios");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: out.stdout,
        stderr: out.stderr,
    }
}

struct Fixed {
    name: &'static str,
    findings: Vec<Finding>,
}

impl Check for Fixed {
    fn name(&self) -> &'static str {
        self.name
    }

    fn run(&self, _repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        Ok(self.findings.clone())
    }
}

struct Skipped(&'static str);

impl Check for Skipped {
    fn name(&self) -> &'static str {
        self.0
    }

    fn run(&self, _repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        Err(Skip(NO_PHASE_HISTORY.to_string()).into())
    }
}

struct Failing;

impl Check for Failing {
    fn name(&self) -> &'static str {
        "md-links"
    }

    fn run(&self, _repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        Err(anyhow::anyhow!("stand-in failure"))
    }
}

/// md-links reports x.md on lines 7, 3, 7 and 0 (merged into 7, also 3) and y.md;
/// anchors reports #gone; milestone-status is skipped.
fn standins() -> Vec<Box<dyn Check>> {
    let x = |line| Finding::new("md-links", "docs/a.md", "x.md", "broken link -> x.md", line);
    vec![
        Box::new(Fixed {
            name: "md-links",
            findings: vec![
                x(7),
                x(3),
                x(7),
                x(0),
                Finding::new("md-links", "README.md", "y.md", "broken link -> y.md", 2),
            ],
        }),
        Box::new(Fixed {
            name: "anchors",
            findings: vec![Finding::new(
                "anchors",
                "docs/a.md",
                "#gone",
                "no heading for anchor #gone in docs/a.md",
                4,
            )],
        }),
        Box::new(Skipped("milestone-status")),
    ]
}

fn names(checks: &[Box<dyn Check>]) -> Vec<&'static str> {
    checks.iter().map(|check| check.name()).collect()
}

#[test]
fn unknown_checks_fail_before_any_git_call() {
    let dir = common::unique_dir("cli-unknown");
    let _cleanup = TestRepo::adopt(dir.clone());
    let run = aios_outside_git(&dir, &["docs-check", "--check", "bogus,nope,bogus"]);
    assert_eq!(run.code, 2);
    assert_eq!(text(&run.stdout), "");
    assert_eq!(
        text(&run.stderr),
        "docs-check: unknown check(s): bogus, nope, bogus (see --list-checks)\n"
    );
}

#[test]
fn outside_a_repository_exits_2() {
    let dir = common::unique_dir("cli-norepo");
    let _cleanup = TestRepo::adopt(dir.clone());
    let run = aios_outside_git(&dir, &["docs-check"]);
    assert_eq!(run.code, 2);
    assert_eq!(text(&run.stdout), "");
    assert_eq!(
        text(&run.stderr),
        "docs-check: not inside a git repository\n"
    );
}

#[test]
fn list_checks_prints_the_registry_without_git() {
    let dir = common::unique_dir("cli-list");
    let _cleanup = TestRepo::adopt(dir.clone());
    let run = aios_outside_git(&dir, &["docs-check", "--list-checks"]);
    assert_eq!(run.code, 0);
    assert_eq!(text(&run.stdout), render_list_checks(&registry()));
    assert_eq!(text(&run.stderr), "");
}

#[test]
fn clap_usage_errors_exit_2() {
    let repo = TestRepo::with_files("cli-clap", &[("README.md", "# R\n")]);
    let run = run_aios(repo.path(), &["docs-check", "--no-such-flag"]);
    assert_eq!(run.code, 2);
    assert!(run.stdout.is_empty());
}

#[test]
fn zero_checks_render_every_format() {
    let repo = TestRepo::with_files("cli-zero", &[("README.md", "# R\n")]);
    let plain = run_aios(repo.path(), &["docs-check", "--check", ","]);
    assert_eq!((plain.code, text(&plain.stdout)), (0, ZERO_TEXT));
    let json = run_aios(
        repo.path(),
        &[
            "docs-check",
            "--che",
            ",",
            "--j",
            "--json",
            "--all",
            "--all",
        ],
    );
    assert_eq!((json.code, text(&json.stdout)), (0, ZERO_JSON));
    let markdown = run_aios(repo.path(), &["docs-check", "--check", " , ", "--markdown"]);
    assert_eq!((markdown.code, text(&markdown.stdout)), (0, ZERO_MARKDOWN));
}

#[test]
fn baseline_path_is_shown_relative_to_the_root() {
    let repo = TestRepo::with_files(
        "cli-baseline-rel",
        &[("README.md", "# R\n"), ("sub/keep.txt", "k\n")],
    );
    let run = run_aios(
        &repo.path().join("sub"),
        &[
            "docs-check",
            "--baseline",
            "../other/base.json",
            "--check",
            ",",
        ],
    );
    assert_eq!(run.code, 0);
    assert_eq!(
        text(&run.stdout),
        ZERO_TEXT.replace("scripts/docs/baseline.json", "other/base.json")
    );
    let empty = run_aios(
        repo.path(),
        &["docs-check", "--baseline", "", "--check", ","],
    );
    assert_eq!(text(&empty.stdout), ZERO_TEXT);
}

#[test]
fn baseline_accepts_a_leading_dash_as_a_path_like_argparse() {
    // check.py (argparse): `--baseline -1` takes "-1" as the path (verified with
    // python3 against `git show 33c6b3d:scripts/docs/check.py` in a scratch
    // repository: it runs normally, reporting against a baseline file named
    // "-1" that does not exist). clap's default `--baseline -1` treats "-1" as
    // an unrecognized flag instead; `allow_negative_numbers` on the `baseline`
    // arg closes that divergence.
    let repo = TestRepo::with_files("cli-baseline-dash", &[("README.md", "# R\n")]);
    let run = run_aios(
        repo.path(),
        &["docs-check", "--baseline", "-1", "--check", ","],
    );
    assert_eq!(run.code, 0);
    assert_eq!(
        text(&run.stdout),
        ZERO_TEXT.replace("scripts/docs/baseline.json", "-1")
    );
}

#[test]
fn unreadable_baseline_exits_2() {
    let repo = TestRepo::with_files("cli-bad-baseline", &[("other/bad.json", "{")]);
    let run = run_aios(
        repo.path(),
        &["docs-check", "--baseline", "other/bad.json", "--check", ","],
    );
    assert_eq!(run.code, 2);
    assert_eq!(text(&run.stdout), "");
    assert!(
        text(&run.stderr).starts_with("docs-check: cannot read baseline other/bad.json: "),
        "{}",
        text(&run.stderr)
    );
}

#[test]
fn update_baseline_with_zero_checks_keeps_and_sorts_every_entry() {
    let repo = TestRepo::with_files(
        "cli-update-zero",
        &[
            ("README.md", "# R\n"),
            ("scripts/docs/baseline.json", HAND_BASELINE),
        ],
    );
    let run = run_aios(
        repo.path(),
        &["docs-check", "--check", ",", "--update-baseline"],
    );
    assert_eq!(
        (run.code, text(&run.stdout)),
        (
            0,
            "docs-check: wrote 3 findings to scripts/docs/baseline.json\n"
        )
    );
    let written = std::fs::read_to_string(repo.path().join("scripts/docs/baseline.json"))
        .expect("read the rewritten baseline");
    assert_eq!(written, HAND_REWRITTEN);
}

#[test]
fn select_from_orders_deduplicates_and_rejects_unknown_names() {
    assert_eq!(
        names(&select_from(standins(), "").expect("empty selects all")),
        ["md-links", "anchors", "milestone-status"]
    );
    assert_eq!(
        names(&select_from(standins(), "milestone-status, md-links,md-links").expect("known")),
        ["md-links", "milestone-status"]
    );
    assert!(select_from(standins(), " , ").expect("no names").is_empty());
    let err = select_from(standins(), "anchors,bogus,,nope")
        .err()
        .expect("unknown names are an error");
    assert_eq!(
        err.to_string(),
        "unknown check(s): bogus, nope (see --list-checks)"
    );
}

#[test]
fn run_checks_merges_keys_and_collects_skips() {
    let repo = TestRepo::with_files("cli-run-checks", &[("README.md", "# R\n")]);
    let r = Repo::open(repo.path_str()).expect("open the test repository");
    let run = run_checks(&r, &standins()).expect("stand-in checks run");
    let got: Vec<(String, usize, Vec<usize>)> = run
        .findings
        .iter()
        .map(|f| (f.key(), f.line, f.also.clone()))
        .collect();
    assert_eq!(
        got,
        [
            ("md-links|README.md|y.md".to_string(), 2, vec![]),
            ("md-links|docs/a.md|x.md".to_string(), 7, vec![3]),
            ("anchors|docs/a.md|#gone".to_string(), 4, vec![]),
        ]
    );
    assert_eq!(run.skipped.len(), 1);
    assert_eq!(run.skipped["milestone-status"], NO_PHASE_HISTORY);

    let failing: Vec<Box<dyn Check>> = vec![Box::new(Failing)];
    let err = run_checks(&r, &failing)
        .err()
        .expect("a failing check fails the run");
    assert_eq!(err.to_string(), "stand-in failure");
}

#[test]
fn run_with_matches_check_py_on_stand_in_checks() {
    let repo = TestRepo::with_files(
        "cli-pipeline",
        &[
            ("README.md", "# R\n"),
            ("scripts/docs/baseline.json", PIPELINE_BASELINE),
        ],
    );
    let args = Args {
        check: "milestone-status,anchors,md-links".to_string(),
        ..Args::default()
    };
    let mut out = Vec::new();
    let code = run_with(&args, repo.path(), standins(), &mut out).expect("the pipeline runs");
    assert_eq!(code, 1);
    assert_eq!(text(&out), PIPELINE_TEXT);

    let update = Args {
        update_baseline: true,
        ..args
    };
    let mut out = Vec::new();
    let code = run_with(&update, repo.path(), standins(), &mut out).expect("the update runs");
    assert_eq!(code, 0);
    assert_eq!(
        text(&out),
        "docs-check: wrote 4 findings to scripts/docs/baseline.json (skipped: milestone-status)\n"
    );
    let written = std::fs::read_to_string(repo.path().join("scripts/docs/baseline.json"))
        .expect("read the rewritten baseline");
    assert_eq!(written, PIPELINE_REWRITTEN);
}
