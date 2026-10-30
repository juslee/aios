//! The loop's git calls, all built on [`crate::cmd::soak::host::git`] (the
//! hardened builder: `LC_ALL=C`, no replace objects, no grafts). Each call
//! feeds a decision (the gate, the diff the lenses read), so none uses the
//! inherit policy of `crate::git`.
//!
//! Paths are bytes on Unix. They stay `PathBuf`s here and every list comes
//! from `-z` output; nothing is converted lossily.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};

use crate::cmd::soak::host::git;

/// One entry of `git diff --name-status --no-renames -z`: the status letter
/// (`A`, `M`, `D`, `T`, ...) and the path. A rename shows as a `D` of the old
/// path and an `A` of the new one, so both sides are listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedPath {
    pub status: char,
    pub path: PathBuf,
}

/// A hardened `git` that runs in `repo`, never prompts for credentials and
/// reads no stdin.
pub fn git_in(repo: &Path) -> Command {
    let mut cmd = git();
    cmd.current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null());
    cmd
}

/// Run `cmd`, returning stdout; a failure carries the first stderr line.
fn stdout_of(mut cmd: Command, what: &str) -> Result<Vec<u8>> {
    let out = cmd
        .output()
        .with_context(|| format!("could not run git ({what})"))?;
    if out.status.success() {
        return Ok(out.stdout);
    }
    let line = out.stderr.split(|&b| b == b'\n').next().unwrap_or_default();
    Err(anyhow!(
        "git {what} failed ({}): {}",
        out.status
            .code()
            .map_or_else(|| "signal".to_string(), |c| c.to_string()),
        String::from_utf8_lossy(line)
    ))
}

/// `git merge-base <a> <b>`: the commit sha.
pub fn merge_base(repo: &Path, a: &str, b: &str) -> Result<String> {
    let mut cmd = git_in(repo);
    cmd.args(["merge-base", a, b]);
    let out = stdout_of(cmd, "merge-base")?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// The paths that differ between `base` and `head`, both sides of a rename
/// included, in git's order.
pub fn changed_paths(repo: &Path, base: &str, head: &str) -> Result<Vec<ChangedPath>> {
    let mut cmd = git_in(repo);
    cmd.args([
        "-c",
        "core.quotePath=false",
        "diff",
        "--name-status",
        "--no-renames",
        "-z",
        base,
        head,
    ]);
    let out = stdout_of(cmd, "diff --name-status")?;
    parse_name_status(&out)
}

/// Parse `-z` name-status output: `STATUS NUL PATH NUL`, repeated.
fn parse_name_status(out: &[u8]) -> Result<Vec<ChangedPath>> {
    let mut fields = out.split(|&b| b == 0).collect::<Vec<_>>();
    if fields.last().is_some_and(|f| f.is_empty()) {
        fields.pop();
    }
    if fields.len() % 2 != 0 {
        bail!("git diff --name-status -z printed an odd number of fields");
    }
    fields
        .chunks(2)
        .map(|pair| {
            let status = pair[0]
                .first()
                .map(|&b| char::from(b))
                .ok_or_else(|| anyhow!("git diff printed an empty status"))?;
            Ok(ChangedPath {
                status,
                path: PathBuf::from(OsString::from_vec(pair[1].to_vec())),
            })
        })
        .collect()
}

/// `git fetch origin pull/<n>/head`, then the sha of what it fetched.
pub fn fetch_pr_head(repo: &Path, n: u64) -> Result<String> {
    let mut cmd = git_in(repo);
    cmd.args(["fetch", "--quiet", "origin"])
        .arg(format!("pull/{n}/head"));
    stdout_of(cmd, "fetch")?;
    let mut cmd = git_in(repo);
    cmd.args(["rev-parse", "--verify", "FETCH_HEAD^{commit}"]);
    let out = stdout_of(cmd, "rev-parse FETCH_HEAD")?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// `git worktree add --detach <path> <sha>`.
pub fn worktree_add_detached(repo: &Path, path: &Path, sha: &str) -> Result<()> {
    let mut cmd = git_in(repo);
    cmd.args(["worktree", "add", "--detach", "--quiet"])
        .arg(path)
        .arg(sha);
    stdout_of(cmd, "worktree add")?;
    Ok(())
}

/// `loop-pr<digits>`: the only name a round worktree has.
fn is_round_name(name: &OsStr) -> bool {
    name.as_bytes()
        .strip_prefix(b"loop-pr")
        .is_some_and(|d| !d.is_empty() && d.iter().all(u8::is_ascii_digit))
}

/// `path` as git lists it: the parent canonicalised (so a symlinked temp
/// directory compares equal) and the file name kept, which also works when the
/// directory itself is gone.
fn comparable(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => parent
            .canonicalize()
            .map_or_else(|_| path.to_path_buf(), |p| p.join(name)),
        _ => path.to_path_buf(),
    }
}

/// Whether `git worktree list --porcelain -z` names `want` (a [`comparable`] path).
fn is_listed(repo: &Path, want: &Path) -> Result<bool> {
    let mut cmd = git_in(repo);
    cmd.args(["worktree", "list", "--porcelain", "-z"]);
    let out = stdout_of(cmd, "worktree list")?;
    Ok(out
        .split(|&b| b == 0)
        .filter_map(|field| field.strip_prefix(b"worktree "))
        .any(|p| comparable(Path::new(OsStr::from_bytes(p))) == want))
}

/// Throw a round worktree away: `reset --hard`, `clean -fd`, then
/// `worktree remove` (never `--force`). It refuses unless the path is one the
/// caller recorded, is named `loop-pr<n>`, and is listed by
/// `git worktree list --porcelain`. A directory that is already gone gets
/// `git worktree prune` and counts as gone.
pub fn worktree_discard(repo: &Path, path: &Path, recorded: &[PathBuf]) -> Result<()> {
    if !recorded.iter().any(|r| r == path) {
        bail!("refusing to discard {}: not recorded", path.display());
    }
    if !path.file_name().is_some_and(is_round_name) {
        bail!(
            "refusing to discard {}: not named loop-pr<n>",
            path.display()
        );
    }
    let want = comparable(path);
    if !is_listed(repo, &want)? {
        bail!(
            "refusing to discard {}: not a listed worktree",
            path.display()
        );
    }
    if !path.exists() {
        let mut cmd = git_in(repo);
        cmd.args(["worktree", "prune"]);
        stdout_of(cmd, "worktree prune")?;
        if is_listed(repo, &want)? {
            bail!("{} is still listed after a prune", path.display());
        }
        return Ok(());
    }
    let mut cmd = git_in(path);
    cmd.args(["reset", "--hard", "--quiet"]);
    stdout_of(cmd, "reset --hard")?;
    let mut cmd = git_in(path);
    cmd.args(["clean", "-fdq"]);
    stdout_of(cmd, "clean")?;
    let mut cmd = git_in(repo);
    cmd.args(["worktree", "remove"]).arg(path);
    stdout_of(cmd, "worktree remove")?;
    Ok(())
}
