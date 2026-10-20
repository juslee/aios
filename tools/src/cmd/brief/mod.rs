//! `aios brief`: the deterministic session briefing behind `/justin:brief`
//! (the former `scripts/agent/brief.sh` (blob at `2d4a9f72ebb8cd98a6659d8c75ebec91be23658f`)).
//!
//! The report is Markdown: git state, open pull requests with their merge-ready verdict,
//! main CI, soak results, the host QEMU lock, the routing log, handoffs, knowledge notes,
//! needs-human issues, the next phase-doc step and the docs drift. Every section degrades
//! to a one-line notice when git, `gh` or the network is unavailable.
//!
//! The command resolves the toplevel of its working directory first, and every later git
//! call, file read and child runs from it, as the script's `cd "$ROOT"` did. Three things
//! are read from elsewhere, as the script read them: the host QEMU lock and the handoffs
//! come from the **main worktree**, and the `rel_path` labels are relative to it.
//!
//! Divergences from the script (accepted, pinned by the parity scenarios):
//!
//! - D1: messages start `brief:` and `--help` prints new text for `aios brief`;
//! - D2: the unreachable branches are gone ("jq is not installed", "docs-check
//!   unavailable", the docs-error preference for the shim's `aios: ` line);
//! - D3 and D7: collation is bytewise, and a soak tie keeps the first run;
//! - D4 and D10: the merge-ready verdict is [`crate::merge_ready`]'s: `#<n>` references
//!   beyond 2^53 are exact, and a PR labelled `needs-human` is not merge-ready;
//! - D6: the stale-binary note comes from `AIOS_SHIM_STALE=1`.
//!
//! Kept bug for bug: the epoch fallback of the date formatting ([`clock`]), the `Boots` row
//! of a soak summary that is printed whole when it has no closing ` |`, and `unpushed_desc`
//! counting commits on no remote against all remotes.
//!
//! Through the `just` recipes the binary inherits the shim's two git exports
//! (`GIT_NO_REPLACE_OBJECTS=1`, `GIT_GRAFT_FILE=/dev/null/no-grafts`), so replace refs
//! and grafts are off on that path, unlike the oracle script.
//!
//! Non-UTF-8 handling: shell keeps bytes, so branch names, worktree and phase-doc paths,
//! file contents and the commit text of a soak summary stay bytes from git or the file
//! system to the writer, and reach git as `OsStr`s (the only text that is ever arbitrary
//! bytes). The conversions that remain are safe because the value cannot hold other
//! bytes: the `origin/main` sha in the Main CI query and the two shas of a soak relation
//! (`git rev-parse` prints hex); the milestone digits of a PR title match (the
//! `[0-9]` run of the phase heading); and the JSON from `gh` and from the docs-check
//! child (UTF-8 by definition, and the script read it through `jq`, which is lossy
//! too). The unit tests of the byte paths are `phase`, `soak` and `crate::paths`;
//! `tests/brief_parity.rs` compares the oracle's bytes with ours.
//!
//! Clock seam: the time comes from the host's `date` and from file modification times
//! ([`clock`]), and the marker is written last.
//!
//! Exit status: 0 for a report; 2 for a usage error or outside a git repository.

mod clock;
mod docs;
mod git_section;
mod github;
mod local;
mod phase;
mod soak;

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{bail, Result};

use crate::gh::{GhCli, GhEnv};
use crate::git;
use crate::paths::normpath_bytes;

const HELP: &str = "\
aios brief - deterministic session briefing for /justin:brief (no LLM)

Usage: aios brief [--no-fetch]

Prints, as Markdown: git state (branch, dirty files, worktrees, unpushed commits),
open PRs with check status and a merge-ready verdict, main CI, the newest soak of a
main commit, the newest other soak and the newest interleaved (aios soak --arm) soak,
the per-session handoff files (.remember/handoff-<key>.md), the host QEMU lock, the
routing-log count, knowledge notes changed since the last session, open needs-human
issues, the next unchecked step of the current phase doc, and a one-line docs-check
summary.

merge-ready is \"yes\" only for a non-draft PR with at least one check, every check
passed, GitHub mergeable with merge state CLEAN, no changes requested, no unresolved
review threads, no open needs-human issue naming it (#N in the issue title gates the PR;
#N in the body also blocks it) and no needs-human label on the PR. When the gates or the
review threads cannot be read, the PR is not merge-ready. The Docs check never fails on
drift, so a failing Docs check counts like any other failing check; drift is reported in
the Docs drift section, from a local docs-check run.

Every section degrades to a one-line notice when git, gh or the network is unavailable.
Text from GitHub (titles, branch names) is printed as data with control characters
replaced; it is never executed.

Side effects: `git fetch --prune origin` (skip with --no-fetch), a timestamp marker in
the git common dir ($GIT_COMMON_DIR/aios-agent/last-brief), and `aios docs-check --json`
run as a child. GitHub is only read.

Options:
  --no-fetch    do not fetch origin first
  -h, --help    print this text

Exit status: 0 after a report; 2 on a usage error or outside a git repository.
";

/// Arguments of `aios brief`. clap only collects them; [`run`] parses the raw argument
/// list itself, in argv order.
#[derive(clap::Args, Debug)]
pub struct Args {
    /// `--no-fetch` and `--help`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
    pub args: Vec<OsString>,
}

#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    /// Run the brief; `fetch` is false under `--no-fetch`.
    Run {
        fetch: bool,
    },
    Help,
    /// A usage error: the message after `brief: `.
    Usage(Vec<u8>),
}

/// Parse the arguments in argv order, as the script's `case` loop did: the first unknown
/// argument or `--help` ends the parse.
fn parse(args: &[OsString]) -> Parsed {
    let mut fetch = true;
    for arg in args {
        match arg.as_bytes() {
            b"--no-fetch" => fetch = false,
            b"-h" | b"--help" => return Parsed::Help,
            other => return Parsed::Usage([&b"unknown argument: "[..], other].concat()),
        }
    }
    Parsed::Run { fetch }
}

/// Where the brief is: the toplevel it runs from, the main worktree, the git common
/// directory and the worktrees.
pub struct Ctx {
    root: PathBuf,
    /// The first worktree `git worktree list` names; empty when git lists none.
    main: PathBuf,
    common: PathBuf,
    worktrees: Vec<PathBuf>,
}

impl Ctx {
    fn new(root: PathBuf) -> Ctx {
        let worktrees = git::worktrees(&root);
        let main = worktrees.first().cloned().unwrap_or_default();
        let common = git::out(&root, &["rev-parse", "--git-common-dir"])
            .map(|dir| {
                let joined = root.join(OsStr::from_bytes(&dir));
                PathBuf::from(OsString::from_vec(normpath_bytes(
                    joined.as_os_str().as_bytes(),
                )))
            })
            .unwrap_or_else(|| root.clone());
        Ctx {
            root,
            main,
            common,
            worktrees,
        }
    }

    /// A path relative to the main checkout when inside it.
    fn rel_path(&self, path: &Path) -> Vec<u8> {
        let (path, main) = (
            path.as_os_str().as_bytes(),
            self.main.as_os_str().as_bytes(),
        );
        if path == main {
            return b"(main checkout)".to_vec();
        }
        path.strip_prefix(main)
            .and_then(|rest| rest.strip_prefix(b"/"))
            .unwrap_or(path)
            .to_vec()
    }

    /// The marker the brief touches: the time of the last brief.
    fn marker(&self) -> PathBuf {
        self.common.join("aios-agent/last-brief")
    }
}

/// `printf '%s\n'` of the concatenated parts.
fn line(out: &mut dyn Write, parts: &[&[u8]]) -> Result<()> {
    out.write_all(&parts.concat())?;
    out.write_all(b"\n")?;
    Ok(())
}

/// A section heading: a blank line, `## <title>`, a blank line.
fn section(out: &mut dyn Write, title: &str) -> Result<()> {
    write!(out, "\n## {title}\n\n")?;
    Ok(())
}

/// Record that a brief ran. Errors are ignored: the marker only bounds the next
/// brief's knowledge window.
fn touch_marker(marker: &Path) {
    let Some(dir) = marker.parent() else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(marker)
    {
        let _ = file.set_modified(SystemTime::now());
    }
}

/// `aios brief [--no-fetch]` from `cwd`.
pub fn run(args: &[OsString], cwd: &Path, out: &mut dyn Write, err: &mut dyn Write) -> Result<u8> {
    let fetch = match parse(args) {
        Parsed::Run { fetch } => fetch,
        Parsed::Help => {
            out.write_all(HELP.as_bytes())?;
            return Ok(0);
        }
        Parsed::Usage(message) => {
            err.write_all(b"brief: ")?;
            err.write_all(&message)?;
            err.write_all(b"\n")?;
            return Ok(2);
        }
    };
    let Some(top) = git::out(cwd, &["rev-parse", "--show-toplevel"]) else {
        writeln!(err, "brief: not inside a git repository")?;
        return Ok(2);
    };
    if top.is_empty() {
        bail!("git printed an empty toplevel");
    }
    let ctx = Ctx::new(PathBuf::from(OsString::from_vec(top)));

    // docs-check is the slowest local step: start it before anything else.
    let docs = docs::start(&ctx.root);
    line(out, &[&clock::header()])?;

    section(out, "Git")?;
    git_section::print(&ctx, fetch, out)?;

    // The probe, the needs-human issues and the review threads come before the first
    // GitHub section, which prints them.
    let mut github = github::Github::open(GhCli {
        cwd: ctx.root.clone(),
        env: GhEnv::INHERIT,
    });
    section(out, "Open pull requests")?;
    github.pull_requests(out)?;

    section(out, "Main CI")?;
    github.main_ci(&ctx.root, out)?;

    section(out, "Soak")?;
    soak::print(&ctx, out)?;

    section(out, "Host QEMU lock")?;
    local::qemu_lock(&ctx, out, err)?;

    section(out, "Routing log")?;
    local::routing_log(&ctx, out)?;

    section(out, "Handoffs")?;
    let newest_handoff = local::handoffs(&ctx, out)?;

    section(out, "Knowledge notes changed since last session")?;
    local::knowledge(&ctx, newest_handoff.as_deref(), &ctx.marker(), out)?;

    section(out, "Needs human")?;
    github.needs_human(out)?;

    section(out, "Next phase-doc step")?;
    if let Some(milestone) = phase::print(&ctx.root, out)? {
        github.milestone_pr(&milestone, out)?;
    }

    section(out, "Docs drift")?;
    docs::print(docs, docs::stale_from_env(), out)?;

    touch_marker(&ctx.marker());
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn usage(message: &str) -> Parsed {
        Parsed::Usage(message.as_bytes().to_vec())
    }

    #[test]
    fn parser_table() {
        let table: Vec<(Vec<&str>, Parsed)> = vec![
            (vec![], Parsed::Run { fetch: true }),
            (vec!["--no-fetch"], Parsed::Run { fetch: false }),
            (
                vec!["--no-fetch", "--no-fetch"],
                Parsed::Run { fetch: false },
            ),
            (vec!["--help"], Parsed::Help),
            (vec!["-h"], Parsed::Help),
            (vec!["--no-fetch", "--help"], Parsed::Help),
            (vec!["--bogus"], usage("unknown argument: --bogus")),
            (vec!["positional"], usage("unknown argument: positional")),
            // Argv order: the first unknown argument or --help ends the parse.
            (
                vec!["--bogus", "--help"],
                usage("unknown argument: --bogus"),
            ),
            (vec!["--help", "--bogus"], Parsed::Help),
            (vec![""], usage("unknown argument: ")),
        ];
        for (argv, want) in table {
            assert_eq!(parse(&args(&argv)), want, "argv {argv:?}");
        }
    }

    fn ctx(main: &str) -> Ctx {
        Ctx {
            root: PathBuf::from(main),
            main: PathBuf::from(main),
            common: PathBuf::from(main).join(".git"),
            worktrees: vec![PathBuf::from(main)],
        }
    }

    #[test]
    fn rel_path_is_relative_to_the_main_checkout() {
        let ctx = ctx("/work/aios");
        assert_eq!(ctx.rel_path(Path::new("/work/aios")), b"(main checkout)");
        assert_eq!(ctx.rel_path(Path::new("/work/aios/wt-x")), b"wt-x");
        assert_eq!(
            ctx.rel_path(Path::new("/work/aios/target/soak/r1")),
            b"target/soak/r1"
        );
        // A sibling that merely shares the prefix is not inside the main checkout.
        assert_eq!(ctx.rel_path(Path::new("/work/aios2")), b"/work/aios2");
        assert_eq!(ctx.rel_path(Path::new("/elsewhere")), b"/elsewhere");
    }

    #[test]
    fn sections_start_with_a_blank_line() {
        let mut out = Vec::new();
        section(&mut out, "Git").expect("write");
        assert_eq!(out, b"\n## Git\n\n");
    }
}
