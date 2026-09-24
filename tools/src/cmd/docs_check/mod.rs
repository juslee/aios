//! `aios docs-check`: the deterministic docs drift checker, a port of
//! `scripts/docs/check.py` at 33c6b3d. `run` is check.py `main()` (L1585-1662);
//! `run_checks` is check.py `run_checks` (L1442-1456).
//!
//! Accepted divergence: check.py resolves the repository from its own
//! directory (L1576-1582, `os.path.dirname(os.path.abspath(__file__))`); `repo_root`
//! resolves it from the process working directory, because the binary has no script
//! directory. `just` runs recipes from the justfile's directory and the shim passes the
//! caller's working directory through, so both tools check the same checkout in every
//! supported invocation; the parity goldens run check.py from inside each materialized
//! repository for the same reason.
//!
//! Two more divergences need unusual directory names. check.py decodes
//! `git rev-parse --show-toplevel` with `text=True` (L1578), whose universal-newline
//! translation turns a carriage return inside the root's path into a newline: for such
//! a root check.py builds a path that does not exist and exits 2, where `repo_root`
//! keeps the bytes and aios checks the real root (exit 0 or 1). And `run_with` needs the
//! working directory's path as UTF-8 (for `paths::relpath`) and exits 2 when it is not,
//! where check.py does not read the working directory in the default case (the root
//! comes from `__file__`, L1577, and L1612's `relpath` of two absolute paths does not
//! consult it) and runs.
//!
//! CLI parsing divergences (argparse vs clap; verified against check.py at 33c6b3d): a
//! trailing bare `--` is a usage error in check.py (`unrecognized arguments: --`, exit 2),
//! where clap accepts it as the end of options and `aios docs-check` runs normally; and
//! `--help` prints argparse's text, not clap's. `--baseline`'s `allow_negative_numbers`
//! below narrows a third one: it makes aios take a value clap parses as a number
//! (`-1`, `-1.5`, `-1e5`, `-1.`) as the path, as argparse does, where `--baseline -1`
//! used to be rejected as an unknown flag. CPython 3.14's argparse also takes as the
//! path any value that starts with `-<digit>` or `-.<digit>` (`-.5`, `-1e`, `-2x.json`),
//! or that starts with `-` and contains a space (`-x y`); aios rejects those as an
//! unexpected argument and exits 2 where check.py runs and exits 0 or 1. A non-UTF-8
//! `--baseline` value is the same: check.py takes it through surrogateescape and runs,
//! clap exits 2. The `--baseline=<value>` form behaves the same in both tools.
//! (`allow_hyphen_values` would close the dash cases but also accept `--baseline --all`,
//! which argparse rejects.)

pub mod checks;
pub mod markdown;
pub mod model;
pub mod output;
pub mod repo;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

use anyhow::{anyhow, bail, Context};

use crate::{paths, proc, pystr};
use checks::Check;
use model::{Finding, Skip};
use repo::Repo;

// check.py's argparse options (L1586-1595). A plain comment, not a doc comment:
// clap would show a doc comment here in `aios docs-check --help`.
#[derive(clap::Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct Args {
    /// list every finding, not only new ones
    #[arg(long)]
    pub all: bool,
    /// print JSON instead of text
    #[arg(long)]
    pub json: bool,
    /// print a Markdown summary (for $GITHUB_STEP_SUMMARY)
    #[arg(long)]
    pub markdown: bool,
    /// comma-separated checks to run (default: all)
    #[arg(long, default_value = "", hide_default_value = true)]
    pub check: String,
    /// baseline file (default: scripts/docs/baseline.json)
    #[arg(long, allow_negative_numbers = true)]
    pub baseline: Option<String>,
    /// rewrite the baseline from the current findings (counts included; 'reason' on accepted false positives is kept)
    #[arg(long)]
    pub update_baseline: bool,
    /// list available checks
    #[arg(long)]
    pub list_checks: bool,
}

/// The result of running the selected checks.
pub struct CheckRun {
    /// Collated: merged by key and sorted (`model::collate`).
    pub findings: Vec<Finding>,
    /// Skip message per check that could not run.
    pub skipped: BTreeMap<&'static str, String>,
}

/// check.py L1602-1608 against `available`: an empty `check_arg` selects every
/// check; otherwise the comma-separated names (Python-stripped, empty parts
/// dropped) select checks in `available` order, each once. Unknown names, in the
/// order given and with repeats, are an error.
pub fn select_from(
    available: Vec<Box<dyn Check>>,
    check_arg: &str,
) -> anyhow::Result<Vec<Box<dyn Check>>> {
    if check_arg.is_empty() {
        return Ok(available);
    }
    let requested: Vec<&str> = check_arg
        .split(',')
        .map(pystr::strip)
        .filter(|name| !name.is_empty())
        .collect();
    let unknown: Vec<&str> = requested
        .iter()
        .copied()
        .filter(|name| !available.iter().any(|check| check.name() == *name))
        .collect();
    if !unknown.is_empty() {
        bail!(
            "unknown check(s): {} (see --list-checks)",
            unknown.join(", ")
        );
    }
    Ok(available
        .into_iter()
        .filter(|check| requested.contains(&check.name()))
        .collect())
}

/// check.py `run_checks` (L1442-1456): runs each check in order; a `Skip` error
/// records the check as skipped, any other error aborts the run.
pub fn run_checks(repo: &Repo, checks: &[Box<dyn Check>]) -> anyhow::Result<CheckRun> {
    let mut findings = Vec::new();
    let mut skipped = BTreeMap::new();
    for check in checks {
        match check.run(repo) {
            Ok(found) => findings.extend(found),
            Err(err) => {
                let skip = err.downcast::<Skip>()?;
                skipped.insert(check.name(), skip.0);
            }
        }
    }
    Ok(CheckRun {
        findings: model::collate(findings),
        skipped,
    })
}

/// check.py `repo_root` (L1576-1582), run in `cwd`.
pub fn repo_root(cwd: &Path) -> anyhow::Result<String> {
    let output = proc::capture("git", &["rev-parse", "--show-toplevel"], cwd)?;
    if !output.status.success() {
        bail!("not inside a git repository");
    }
    let stdout = std::str::from_utf8(&output.stdout)
        .context("git rev-parse --show-toplevel printed output that is not valid UTF-8")?;
    Ok(pystr::strip(stdout).to_string())
}

/// The whole of check.py `main()` with the registered checks: writes stdout to
/// `out`, returns the exit code (0, or 1 when there is new drift; the caller in
/// `main.rs` maps an `Err` to 2). A Rust panic unwinds past this and exits the
/// process with code 101, where check.py's `__main__` catches every crash and
/// exits 2 instead; every consumer (`docs.yml`, `brief.sh`) already treats any
/// exit other than 0 or 1 as a checker error, so this divergence is not
/// observable as different behaviour outside the process.
pub fn run(args: &Args, cwd: &Path, out: &mut dyn Write) -> anyhow::Result<u8> {
    run_with(args, cwd, checks::registry(), out)
}

/// `run` with an explicit list of available checks (tests pass stand-in checks).
pub fn run_with(
    args: &Args,
    cwd: &Path,
    available: Vec<Box<dyn Check>>,
    out: &mut dyn Write,
) -> anyhow::Result<u8> {
    if args.list_checks {
        out.write_all(output::render_list_checks(&available).as_bytes())?;
        return Ok(0);
    }
    let selected = select_from(available, &args.check)?;
    let names: Vec<&'static str> = selected.iter().map(|check| check.name()).collect();
    let root = repo_root(cwd)?;
    let cwd_text = cwd
        .to_str()
        .ok_or_else(|| anyhow!("the current directory {} is not valid UTF-8", cwd.display()))?;
    let baseline_path = match args.baseline.as_deref() {
        Some(path) if !path.is_empty() => path.to_string(),
        _ => paths::join(&root, model::BASELINE_REL),
    };
    let baseline_rel = paths::relpath(&baseline_path, &root, cwd_text);
    let repo = Repo::open(&root)?;
    let CheckRun { findings, skipped } = run_checks(&repo, &selected)?;
    let ran: BTreeSet<&str> = names
        .iter()
        .copied()
        .filter(|name| !skipped.contains_key(name))
        .collect();
    let baseline_file = cwd.join(&baseline_path);
    let baseline = model::load_baseline(&baseline_file, &baseline_path)?;

    if args.update_baseline {
        let entries = model::updated_baseline(&findings, &baseline, &ran);
        model::write_baseline(&baseline_file, &entries)?;
        let mut message = format!(
            "docs-check: wrote {} findings to {baseline_rel}",
            entries.len()
        );
        if !skipped.is_empty() {
            let skipped_names: Vec<&str> = skipped.keys().copied().collect();
            message.push_str(&format!(" (skipped: {})", skipped_names.join(", ")));
        }
        writeln!(out, "{message}")?;
        return Ok(0);
    }

    let cmp = model::compare(&findings, &baseline, &ran)?;
    let report = output::Report {
        findings: &findings,
        cmp: &cmp,
        skipped: &skipped,
        names: &names,
        baseline: &baseline,
        baseline_rel: &baseline_rel,
    };
    if args.json {
        writeln!(out, "{}", output::render_json(&report, args.all)?)?;
    } else if args.markdown {
        out.write_all(output::render_markdown(&report).as_bytes())?;
    } else {
        writeln!(out, "{}", output::render_text(&report, args.all))?;
    }
    Ok(if cmp.new_keys.is_empty() { 0 } else { 1 })
}
