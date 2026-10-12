//! `aios checkpoint`: the deterministic work-in-progress checkpoint behind
//! `/justin:pause` (the former `scripts/agent/checkpoint.sh` (blob at `faf6d20295136489d6ccdfa5a253480a9f757fcc`)).
//!
//! The binary commits and pushes in the current checkout only, and its bounds are code:
//!
//! - it commits only on a `claude/?*` branch, never on `main`, a detached HEAD, another
//!   branch, or during a merge, cherry-pick, revert, rebase or bisect, or with unmerged
//!   paths;
//! - the refspec it pushes is built only from a [`ClaudeBranch`]
//!   (`refs/heads/<b>:refs/heads/<b>`); the push is never forced and never
//!   `--no-verify`, and a branch outside `claude/*` is a hard error before any push;
//! - the secret scan ([`scan`]) fails closed: a git failure inside it restores the
//!   index, commits and pushes nothing, and exits 1 (divergence D9);
//! - wip commits are held back while a ready PR exists, and when `gh` cannot say.
//!
//! Output, exit status and side effects match the script in every scenario of
//! `tests/checkpoint_parity.rs`, except the divergences recorded there: D1 (the
//! messages start `checkpoint:`; new `--help` text), D5 (the `checkpoint it:` hint is a
//! `just --justfile <main>/justfile --working-directory . checkpoint` command that
//! works from any worktree), D8 (ASCII-only case folding in the path regex) and D9.
//! Bug-for-bug: the exit status of `git add -A` is not checked, the push is
//! attempted after a failed commit, and a failed index restore (`git read-tree`) on the
//! secret stop is not checked either: the output still says the index is as it was, and
//! the staging stays in place.
//!
//! Exit status: 0 no worktree holds work that is not on origin; 1 uncommitted files or
//! commits not on origin remain here or in another worktree, or the secret scan failed;
//! 3 stopped on a possible secret; 2 usage error.

mod others;
pub mod scan;

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::OnceLock;

use anyhow::{anyhow, bail, Result};
use regex::bytes::Regex;

use crate::gh::{GhCli, GhEnv, GhOutcome, GhRunner};
use crate::git;

use scan::{Hit, EXCLUDES};

/// The environment of the one `gh` call (`pr list --head`), as the script ran it.
const CHECKPOINT_GH_ENV: GhEnv = GhEnv {
    set: &[("GH_PROMPT_DISABLED", "1")],
    remove: &[],
};

/// Prints the number of each open PR that is ready for review.
const READY_PRS_JQ: &str = r##".[] | select(.isDraft | not) | "#\(.number)""##;

const HELP: &str = "\
aios checkpoint - deterministic work-in-progress checkpoint for /justin:pause

Usage: aios checkpoint [--handoff saved|not-saved] [--allow <path>]...

Commits and pushes only in the current checkout (a linked worktree is fine):
 1. Commits only on a claude/* branch, never on main or a detached HEAD, and not
    while a merge, cherry-pick, revert, rebase or bisect is in progress, or paths
    are unmerged.
 2. Stages everything that is not gitignored, except .remember/ and __pycache__/.
    Before any commit it scans the staged changes and the local commits not yet on
    origin: a path new to the repo that looks like a secret (.env, *.pem, *.key,
    id_rsa*, *credential*, *secret*, ...) or an added line that looks like a
    private key or access token stops the checkpoint, and so does a git failure
    inside the scan. On a stop the index is restored exactly and nothing is
    committed or pushed. --allow <path> (repeatable, exact repo-relative path as
    listed) lets a flagged path through; pass it only for paths the user confirmed
    are safe to publish.
 3. Commits \"wip: checkpoint <branch>\" when anything is staged.
 4. Pushes the branch under its own name when it has commits that are not on
    origin, also when the tree was already clean. Never forced, never with
    --no-verify. Wip commits stay local while the branch has an open PR that is
    ready for review (each push to it starts CI and the Claude review), and also
    when gh cannot tell whether such a PR exists.
 5. Lists every other worktree that has uncommitted files or commits not on
    origin. It never commits there: a background agent may be working in it.
 6. Prints one checkpoint line, then \"Safe to /clear\" only when no worktree holds
    work that is not on origin.

Options:
  --handoff saved|not-saved   record in the checkpoint line whether the handoff
                              file was written (default: not recorded)
  --allow <path>              let one flagged repo-relative path through
  -h, --help                  print this text

Exit status: 0 no worktree holds work that is not on origin; 1 uncommitted files or
commits not on origin remain here or in another worktree (the output says where and
why), or the secret scan failed; 3 stopped on a possible secret; 2 usage error.
";

/// Arguments of `aios checkpoint`. clap only collects them; [`run`] parses the raw
/// argument list itself, in argv order.
#[derive(clap::Args, Debug)]
pub struct Args {
    /// `--handoff saved|not-saved`, `--allow <path>` (repeatable) and `--help`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
    pub args: Vec<OsString>,
}

/// A branch name matching `claude/?*`: the only kind of branch checkpoint pushes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeBranch(String);

impl ClaudeBranch {
    /// `Some` only for `claude/` followed by at least one character.
    pub fn new(name: &str) -> Option<ClaudeBranch> {
        name.strip_prefix("claude/")
            .filter(|rest| !rest.is_empty())
            .map(|_| ClaudeBranch(name.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The push refspec: the branch under its own name, never `HEAD:` or a bare branch.
fn push_refspec(branch: &ClaudeBranch) -> String {
    format!("refs/heads/{0}:refs/heads/{0}", branch.as_str())
}

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
struct Options {
    /// `not recorded`, `saved` or `not saved`.
    handoff: &'static str,
    /// The `--allow` paths in argv order, each without one leading `./`; an argument
    /// holding newlines gives one entry per non-empty line.
    allow: Vec<Vec<u8>>,
}

#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    Run(Options),
    Help,
    /// A usage error: the message after `checkpoint: `.
    Usage(Vec<u8>),
}

/// Parse the arguments in argv order, as the script's `case` loop did: the first
/// error or `--help` ends the parse.
fn parse(args: &[OsString]) -> Parsed {
    let mut options = Options {
        handoff: "not recorded",
        allow: Vec::new(),
    };
    let mut args = args.iter().map(|arg| arg.as_bytes());
    while let Some(arg) = args.next() {
        match arg {
            b"--allow" => match args.next() {
                Some(path) if !path.is_empty() && !path.starts_with(b"-") => {
                    let path = path.strip_prefix(b"./").unwrap_or(path);
                    options.allow.extend(
                        path.split(|&b| b == b'\n')
                            .filter(|line| !line.is_empty())
                            .map(<[u8]>::to_vec),
                    );
                }
                _ => return Parsed::Usage(b"--allow takes a repo-relative path".to_vec()),
            },
            b"--handoff" => match args.next() {
                Some(b"saved") => options.handoff = "saved",
                Some(b"not-saved") => options.handoff = "not saved",
                _ => return Parsed::Usage(b"--handoff takes 'saved' or 'not-saved'".to_vec()),
            },
            b"-h" | b"--help" => return Parsed::Help,
            other => return Parsed::Usage([&b"unknown argument: "[..], other].concat()),
        }
    }
    Parsed::Run(options)
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// `paste -sd ',' - | sed 's/,/, /g'`: the items joined by `, ` (and every comma inside
/// an item widened too, as the script's sed did).
fn comma_list(items: &[&[u8]]) -> Vec<u8> {
    let joined = items.join(&b','); // [&[u8]]::join(&u8)
    let mut out = Vec::with_capacity(joined.len());
    for byte in joined {
        out.push(byte);
        if byte == b',' {
            out.push(b' ');
        }
    }
    out
}

/// `branch` could not be pushed or committed on: the operation that blocks a commit.
fn in_progress(root: &Path) -> Option<Vec<u8>> {
    let verify = |name: &str| git::ok(root, &["rev-parse", "-q", "--verify", name]);
    let exists = |name: &str| {
        git::text(root, &["rev-parse", "--git-path", name])
            .filter(|path| !path.is_empty())
            .is_some_and(|path| root.join(path).exists())
    };
    let what = if verify("MERGE_HEAD") {
        "merge in progress"
    } else if verify("CHERRY_PICK_HEAD") {
        "cherry-pick in progress"
    } else if verify("REVERT_HEAD") {
        "revert in progress"
    } else if exists("rebase-merge") || exists("rebase-apply") {
        "rebase in progress"
    } else if exists("BISECT_LOG") {
        "bisect in progress"
    } else if git::out(root, &["ls-files", "-u"]).is_some_and(|unmerged| !unmerged.is_empty()) {
        "unmerged paths"
    } else {
        return None;
    };
    Some(what.as_bytes().to_vec())
}

/// Why `git push` was rejected: the first line that names the rejection or starts
/// `error:`, `fatal:` or `remote:`, else the first non-empty line; without git's
/// leading ` ! ` marker, and with runs of spaces squeezed.
fn push_reason(stderr: &[u8]) -> Vec<u8> {
    static NAMED: OnceLock<Regex> = OnceLock::new();
    let named = NAMED.get_or_init(|| {
        Regex::new(r"(?-u)\[(remote )?rejected\]|^(error|fatal|remote):").expect("a valid regex")
    });
    let line = scan::lines(stderr)
        .find(|line| named.is_match(line))
        .or_else(|| scan::lines(stderr).find(|line| !line.is_empty()))
        .unwrap_or_default();
    let start = line
        .iter()
        .position(|b| !(b.is_ascii_whitespace() || *b == b'!' || *b == 0x0b))
        .unwrap_or(line.len());
    let mut squeezed = Vec::with_capacity(line.len());
    for &byte in &line[start..] {
        if byte != b' ' || squeezed.last() != Some(&b' ') {
            squeezed.push(byte);
        }
    }
    squeezed
}

/// How the run ended before the final lines.
#[derive(Debug, PartialEq, Eq)]
enum End {
    /// Nothing special: the output and the exit status follow the final tally.
    Normal,
    /// A possible secret stopped the checkpoint (exit 3).
    Secret,
    /// A git call inside the secret scan failed (D9): never "Safe to /clear".
    ScanFailed,
}

/// What the commit and push step left.
struct Saved {
    wip: Vec<u8>,
    pushed: Vec<u8>,
    /// The flagged paths `--allow` let through, and the ones it did not.
    allowed: Vec<Hit>,
    secrets: Vec<Hit>,
    end: End,
}

/// Whether the branch may be committed on, and if not, why.
enum Gate {
    Blocked(Vec<u8>),
    Open(ClaudeBranch),
}

struct Checkpoint<'a> {
    root: &'a Path,
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
}

impl Checkpoint<'_> {
    /// Run `git <args>` and pass its output on, as the script let it through. `true`
    /// when it exited 0.
    fn passthrough(&mut self, args: &[&str]) -> Result<bool> {
        match git::output(self.root, args) {
            Ok(output) => {
                self.forward(&output)?;
                Ok(output.status.success())
            }
            Err(e) => {
                writeln!(self.err, "checkpoint: cannot run git: {e}")?;
                Ok(false)
            }
        }
    }

    fn forward(&mut self, output: &Output) -> Result<()> {
        self.out.write_all(&output.stdout)?;
        self.err.write_all(&output.stderr)?;
        Ok(())
    }

    /// Restore the index to the tree `snapshot` (`git read-tree`).
    fn restore_index(&mut self, snapshot: &str) -> Result<()> {
        self.passthrough(&["read-tree", snapshot])?;
        Ok(())
    }

    fn gate(&self, branch: &[u8]) -> Gate {
        if branch.is_empty() {
            return Gate::Blocked(b"detached HEAD".to_vec());
        }
        if let Some(claude) = std::str::from_utf8(branch).ok().and_then(ClaudeBranch::new) {
            return match in_progress(self.root) {
                Some(operation) => Gate::Blocked(operation),
                None => Gate::Open(claude),
            };
        }
        if branch == b"main" {
            return Gate::Blocked(b"on main".to_vec());
        }
        Gate::Blocked(cat(&[branch, b" is not a claude/* branch"]))
    }

    /// Stage, scan, commit and push on an open branch.
    fn save(&mut self, branch: &ClaudeBranch, allow: &[Vec<u8>]) -> Result<Saved> {
        let mut saved = Saved {
            wip: Vec::new(),
            pushed: Vec::new(),
            allowed: Vec::new(),
            secrets: Vec::new(),
            end: End::Normal,
        };
        let Some(pre_index) = git::text(self.root, &["write-tree"]) else {
            saved.wip = b"not committed (cannot snapshot the index)".to_vec();
            saved.pushed = b"no (index error)".to_vec();
            return Ok(saved);
        };

        let mut add = vec!["add", "-A", "--", "."];
        add.extend(EXCLUDES);
        self.passthrough(&add)?;

        match scan::scan(self.root) {
            Err(failure) => {
                self.restore_index(&pre_index)?;
                saved.wip = cat(&[b"not committed (secret scan failed: ", &failure.line, b")"]);
                saved.pushed = b"no (secret scan failed)".to_vec();
                saved.end = End::ScanFailed;
                return Ok(saved);
            }
            Ok(hits) => {
                let (allowed, secrets): (Vec<Hit>, Vec<Hit>) = hits
                    .into_iter()
                    .partition(|(path, _)| allow.iter().any(|entry| entry == path));
                saved.allowed = allowed;
                saved.secrets = secrets;
            }
        }
        if !saved.secrets.is_empty() {
            self.restore_index(&pre_index)?;
            saved.wip = b"not committed (possible secret)".to_vec();
            saved.pushed = b"no (possible secret)".to_vec();
            saved.end = End::Secret;
            return Ok(saved);
        }

        if git::ok(self.root, &["diff", "--cached", "--quiet"]) {
            saved.wip = b"clean".to_vec();
        } else {
            saved.wip = self.commit(branch, &saved.allowed, &pre_index)?;
        }
        saved.pushed = self.push(branch.as_str())?;
        Ok(saved)
    }

    /// Commit the staged changes; the `wip:` text of the checkpoint line.
    fn commit(
        &mut self,
        branch: &ClaudeBranch,
        allowed: &[Hit],
        pre_index: &str,
    ) -> Result<Vec<u8>> {
        let files = git::output(self.root, &["diff", "--cached", "--name-only"])
            .map(|output| output.stdout.iter().filter(|&&b| b == b'\n').count())
            .unwrap_or(0);
        let subject = format!("wip: checkpoint {}", branch.as_str());
        let mut command = git::command(self.root);
        command.args([
            "commit",
            "-q",
            "-m",
            subject.as_str(),
            "-m",
            "Paused with /justin:pause; not a finished step.",
        ]);
        if !allowed.is_empty() {
            let paths: BTreeSet<&[u8]> = allowed.iter().map(|(path, _)| path.as_slice()).collect();
            let paths: Vec<&[u8]> = paths.into_iter().collect();
            let override_note = cat(&[
                b"Secret scan overridden with --allow (confirmed safe by the user): ",
                &comma_list(&paths),
            ]);
            command.arg("-m").arg(OsStr::from_bytes(&override_note));
        }
        command.args(["-m", "Co-Authored-By: Claude <noreply@anthropic.com>"]);
        match command.output() {
            Ok(output) if output.status.success() => {
                self.out.write_all(&output.stdout)?;
                Ok(format!("committed {files} file(s)").into_bytes())
            }
            failed => {
                let reason = match &failed {
                    Ok(output) => {
                        self.out.write_all(&output.stdout)?;
                        scan::first_line(&output.stderr).to_vec()
                    }
                    Err(e) => e.to_string().into_bytes(),
                };
                self.restore_index(pre_index)?;
                Ok(cat(&[b"not committed (git commit failed: ", &reason, b")"]))
            }
        }
    }

    /// Push the branch when it has commits that are not on origin, unless a wip commit
    /// would reach a PR that is ready for review (or `gh` cannot say). The `pushed:`
    /// text of the checkpoint line.
    fn push(&mut self, branch: &str) -> Result<Vec<u8>> {
        // The hard check behind the type: nothing but a claude/* branch is ever pushed.
        let claude = ClaudeBranch::new(branch)
            .ok_or_else(|| anyhow!("refusing to push {branch:?}: not a claude/* branch"))?;
        if others::unpushed(self.root) == "0" {
            return Ok(b"no (nothing to push)".to_vec());
        }
        if let Some(hold) = self.hold(&claude)? {
            return Ok(cat(&[b"no (", &hold, b"; wip kept local)"]));
        }
        let pushed = git::command(self.root)
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(["push", "-q", "-u", "origin"])
            .arg(push_refspec(&claude))
            .output();
        match pushed {
            Ok(output) if output.status.success() => {
                self.out.write_all(&output.stdout)?;
                Ok(b"yes".to_vec())
            }
            Ok(output) => {
                self.out.write_all(&output.stdout)?;
                Ok(cat(&[
                    b"no (rejected: ",
                    &push_reason(&output.stderr),
                    b")",
                ]))
            }
            Err(e) => Ok(cat(&[b"no (rejected: ", e.to_string().as_bytes(), b")"])),
        }
    }

    /// Why the push is held back, when a wip commit would go to a ready PR or the PR
    /// state is unknown. Fail closed: without a definite "no ready PR", wip stays local.
    fn hold(&mut self, branch: &ClaudeBranch) -> Result<Option<Vec<u8>>> {
        let log = git::output(
            self.root,
            &["log", "--format=%s", "HEAD", "--not", "--remotes=origin"],
        );
        let has_wip = match log {
            Ok(output) => {
                self.err.write_all(&output.stderr)?;
                scan::lines(&output.stdout).any(|subject| subject.starts_with(b"wip:"))
            }
            Err(_) => false,
        };
        if !has_wip {
            return Ok(None);
        }
        let gh = GhCli {
            cwd: self.root.to_path_buf(),
            env: CHECKPOINT_GH_ENV,
        };
        let outcome = gh.run(&[
            "pr",
            "list",
            "--head",
            branch.as_str(),
            "--state",
            "open",
            "--json",
            "number,isDraft",
            "--jq",
            READY_PRS_JQ,
        ]);
        Ok(match &outcome {
            GhOutcome::Missing => Some(b"PR state unknown: gh is not installed".to_vec()),
            GhOutcome::Failed { .. } => {
                Some(cat(&[b"PR state unknown: ", outcome.first_stderr_line()]))
            }
            GhOutcome::Refused { reason } => {
                Some(format!("PR state unknown: {reason}").into_bytes())
            }
            GhOutcome::Ok(stdout) => {
                let mut ready = stdout.as_slice();
                while let Some(rest) = ready.strip_suffix(b"\n") {
                    ready = rest;
                }
                (!ready.is_empty()).then(|| {
                    let numbers: Vec<&[u8]> = ready.split(|&b| b == b'\n').collect();
                    cat(&[
                        b"open PR ",
                        &comma_list(&numbers),
                        b" is ready for review; mark it draft to push wip commits",
                    ])
                })
            }
        })
    }

    fn describe(&mut self, hits: &[Hit]) -> Result<()> {
        for (path, reason) in hits {
            self.out.write_all(b"  - ")?;
            self.out.write_all(path)?;
            writeln!(self.out, " ({reason})")?;
        }
        Ok(())
    }

    fn line(&mut self, text: &[u8]) -> Result<()> {
        self.out.write_all(text)?;
        self.out.write_all(b"\n")?;
        Ok(())
    }

    fn run(&mut self, options: &Options) -> Result<u8> {
        let root = self.root;
        let branch =
            git::out(root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).unwrap_or_default();
        let dirty = others::dirty(root, false);

        let saved = match self.gate(&branch) {
            Gate::Blocked(block) => Saved {
                wip: if dirty == 0 {
                    b"clean".to_vec()
                } else {
                    cat(&[b"not committed (", &block, b")"])
                },
                pushed: cat(&[b"no (", &block, b")"]),
                allowed: Vec::new(),
                secrets: Vec::new(),
                end: End::Normal,
            },
            Gate::Open(claude) => self.save(&claude, &options.allow)?,
        };

        let dirty = others::dirty(root, false);
        let unpushed = others::unpushed(root);
        let short = git::text(root, &["rev-parse", "--short", "HEAD"])
            .unwrap_or_else(|| "none".to_string());
        let shown: &[u8] = if branch.is_empty() {
            b"(detached)"
        } else {
            &branch
        };
        let header = cat(&[
            b"checkpoint: ",
            shown,
            b" @ ",
            short.as_bytes(),
            b" | wip: ",
            &saved.wip,
            b" | pushed: ",
            &saved.pushed,
            b" | handoff: ",
            options.handoff.as_bytes(),
        ]);
        self.line(&header)?;

        if saved.end != End::ScanFailed {
            if !saved.allowed.is_empty() {
                self.line(b"Allowed by --allow (confirmed safe by the user):")?;
                self.describe(&saved.allowed)?;
            }
            let allowed_paths: BTreeSet<&[u8]> = saved
                .allowed
                .iter()
                .map(|(path, _)| path.as_slice())
                .collect();
            let unused: Vec<&[u8]> = options
                .allow
                .iter()
                .map(Vec::as_slice)
                .filter(|entry| !allowed_paths.contains(entry))
                .collect();
            if !unused.is_empty() {
                let note = cat(&[b"--allow matched no flagged path: ", &comma_list(&unused)]);
                self.line(&note)?;
            }
        }
        if saved.end == End::Secret {
            self.line(
                b"Stopped before committing: these look like secrets (the repository is public):",
            )?;
            self.describe(&saved.secrets)?;
            self.line(b"Nothing was committed or pushed and the index is as it was. Remove or gitignore them. If the user confirms a listed path is safe to publish, rerun with --allow <path> for each confirmed path, exactly as listed (/justin:pause asks first).")?;
            return Ok(3);
        }

        let listed = git::worktrees(root);
        let main = listed.first().cloned().unwrap_or_default();
        let others = others::list(root, &listed);
        if !others.is_empty() {
            self.line(b"Other worktrees hold work that is not on origin (left untouched: an agent may be working there):")?;
            for other in &others {
                let label = others::label(&other.path, &main);
                let summary = cat(&[
                    b"  - ",
                    &label,
                    b" on ",
                    &other.branch,
                    format!(
                        ": {} uncommitted file(s), {} commit(s) not on origin",
                        other.dirty, other.unpushed
                    )
                    .as_bytes(),
                ]);
                self.line(&summary)?;
                let claude = other
                    .branch
                    .strip_prefix(b"claude/")
                    .is_some_and(|rest| !rest.is_empty());
                let hint = if claude {
                    cat(&[
                        b"    checkpoint it: (cd '",
                        other.path.as_os_str().as_bytes(),
                        b"' && just --justfile '",
                        main.as_os_str().as_bytes(),
                        b"/justfile' --working-directory . checkpoint)",
                    ])
                } else {
                    b"    not a claude/* branch: commit or push it by hand".to_vec()
                };
                self.line(&hint)?;
            }
        }
        let scan_failed = saved.end == End::ScanFailed;
        if !scan_failed && dirty == 0 && unpushed == "0" && others.is_empty() {
            self.line("Safe to /clear — run /justin:start after".as_bytes())?;
            return Ok(0);
        }
        if dirty != 0 || unpushed != "0" {
            let root_text = root.as_os_str().as_bytes();
            let note = cat(&[
                format!(
                    "Not fully saved: {dirty} uncommitted file(s) and {unpushed} commit(s) not on origin remain only in "
                )
                .as_bytes(),
                root_text,
                b".",
            ]);
            self.line(&note)?;
        }
        if !others.is_empty() {
            writeln!(
                self.out,
                "Not fully saved: {} other worktree(s) listed above hold work that is not on origin.",
                others.len()
            )?;
        }
        self.line(b"/clear keeps all of it on disk; run /justin:start after.")?;
        Ok(1)
    }
}

/// `aios checkpoint [--handoff saved|not-saved] [--allow PATH]...` from `cwd`.
pub fn run(args: &[OsString], cwd: &Path, out: &mut dyn Write, err: &mut dyn Write) -> Result<u8> {
    let options = match parse(args) {
        Parsed::Run(options) => options,
        Parsed::Help => {
            out.write_all(HELP.as_bytes())?;
            return Ok(0);
        }
        Parsed::Usage(message) => {
            err.write_all(b"checkpoint: ")?;
            err.write_all(&message)?;
            err.write_all(b"\n")?;
            return Ok(2);
        }
    };
    let Some(top) = git::out(cwd, &["rev-parse", "--show-toplevel"]) else {
        writeln!(err, "checkpoint: not inside a git repository")?;
        return Ok(2);
    };
    let root = PathBuf::from(OsString::from_vec(top));
    // Every git call, gh call and pathspec below runs from the toplevel, as the
    // script's `cd "$ROOT"` did.
    if root.as_os_str().is_empty() {
        bail!("git printed an empty toplevel");
    }
    Checkpoint {
        root: &root,
        out,
        err,
    }
    .run(&options)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn run_options(handoff: &'static str, allow: &[&str]) -> Parsed {
        Parsed::Run(Options {
            handoff,
            allow: allow.iter().map(|a| a.as_bytes().to_vec()).collect(),
        })
    }

    fn usage(message: &str) -> Parsed {
        Parsed::Usage(message.as_bytes().to_vec())
    }

    #[test]
    fn parser_table() {
        let allow_msg = "--allow takes a repo-relative path";
        let handoff_msg = "--handoff takes 'saved' or 'not-saved'";
        let table: Vec<(Vec<&str>, Parsed)> = vec![
            (vec![], run_options("not recorded", &[])),
            (vec!["--handoff", "saved"], run_options("saved", &[])),
            (
                vec!["--handoff", "saved", "--handoff", "not-saved"],
                run_options("not saved", &[]),
            ),
            (
                vec!["--allow", "./a/b", "--allow", "c", "--allow", "././d"],
                run_options("not recorded", &["a/b", "c", "./d"]),
            ),
            (
                vec!["--allow", "x\n\ny"],
                run_options("not recorded", &["x", "y"]),
            ),
            (vec!["--allow", "./"], run_options("not recorded", &[])),
            (vec!["--allow", ""], usage(allow_msg)),
            (vec!["--allow", "-x"], usage(allow_msg)),
            (vec!["--allow"], usage(allow_msg)),
            (vec!["--handoff", "maybe"], usage(handoff_msg)),
            (vec!["--handoff"], usage(handoff_msg)),
            (vec!["--bogus"], usage("unknown argument: --bogus")),
            (vec!["positional"], usage("unknown argument: positional")),
            (vec!["--help"], Parsed::Help),
            (vec!["-h"], Parsed::Help),
            // Argv order: the first error or --help wins.
            (vec!["--handoff", "x", "--help"], usage(handoff_msg)),
            (vec!["--help", "--bogus"], Parsed::Help),
            (
                vec!["--bogus", "--help"],
                usage("unknown argument: --bogus"),
            ),
        ];
        for (argv, want) in table {
            assert_eq!(parse(&args(&argv)), want, "argv {argv:?}");
        }
    }

    #[test]
    fn claude_branch_matches_the_case_pattern() {
        assert!(ClaudeBranch::new("claude/x").is_some());
        assert!(ClaudeBranch::new("claude/phase-1/m2").is_some());
        assert!(ClaudeBranch::new("claude/").is_none());
        assert!(ClaudeBranch::new("claude").is_none());
        assert!(ClaudeBranch::new("main").is_none());
        assert!(ClaudeBranch::new("feature/claude/x").is_none());
        assert!(ClaudeBranch::new("Claude/x").is_none());
        let branch = ClaudeBranch::new("claude/x").expect("a claude branch");
        assert_eq!(
            push_refspec(&branch),
            "refs/heads/claude/x:refs/heads/claude/x"
        );
    }

    #[test]
    fn comma_list_widens_every_comma() {
        assert_eq!(comma_list(&[]), b"");
        assert_eq!(comma_list(&[b"a"]), b"a");
        assert_eq!(comma_list(&[b"a", b"b", b"a"]), b"a, b, a");
        // The script's sed widened commas inside an item too.
        assert_eq!(comma_list(&[b"x,y", b"z"]), b"x, y, z");
    }

    #[test]
    fn push_reason_follows_the_script() {
        let rejected = b"To /origin\n ! [rejected]        claude/x -> claude/x (non-fast-forward)\nerror: failed to push\n";
        assert_eq!(
            push_reason(rejected),
            b"[rejected] claude/x -> claude/x (non-fast-forward)"
        );
        // `error:`, `fatal:` and `remote:` lines count too; the first match wins.
        assert_eq!(
            push_reason(b"hint: something\nfatal: no such remote\nerror: later\n"),
            b"fatal: no such remote"
        );
        // Otherwise the first non-empty line.
        assert_eq!(push_reason(b"\n  odd   output\n"), b"odd output");
        assert_eq!(push_reason(b""), b"");
        assert_eq!(push_reason(b"\n\n"), b"");
    }
}
