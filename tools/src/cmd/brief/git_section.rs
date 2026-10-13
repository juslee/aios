//! The Git section: the checkout, the origin/main tip and every worktree.
//!
//! The GitHub probe is not here: it sits with the sections that need it.

use std::ffi::OsStr;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::Stdio;

use anyhow::Result;

use super::{line, Ctx};
use crate::git;

/// The branch checked out in `dir`, or `(detached)`.
pub fn branch_or_detached(dir: &Path) -> Vec<u8> {
    git::out(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .unwrap_or_else(|| b"(detached)".to_vec())
}

/// `git status --porcelain | wc -l`: the number of status lines, whatever git's exit.
fn dirty_count(dir: &Path) -> usize {
    git::output(dir, &["status", "--porcelain"])
        .map(|output| output.stdout.iter().filter(|&&b| b == b'\n').count())
        .unwrap_or(0)
}

/// `git rev-list --count <args>`, or `?` when git fails. The arguments are `OsStr`s so a
/// ref built from a branch name that is not UTF-8 reaches git as it is.
fn count(dir: &Path, args: &[&OsStr]) -> Vec<u8> {
    git::command(dir)
        .args(["rev-list", "--count"])
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            let mut stdout = output.stdout;
            while stdout.last() == Some(&b'\n') {
                stdout.pop();
            }
            stdout
        })
        .unwrap_or_else(|| b"?".to_vec())
}

/// How far `dir`'s branch is from origin. Unlike checkpoint, the commits on no remote
/// count against all remotes (`--remotes`), as the script did. Every ref git is asked
/// about is built from the branch's raw bytes, as the script's `$b` was.
pub fn unpushed_desc(dir: &Path) -> Vec<u8> {
    let Some(branch) = git::out(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"]) else {
        return b"detached HEAD".to_vec();
    };
    let remote = [&b"refs/remotes/origin/"[..], &branch].concat();
    let on_origin = git::command(dir)
        .args(["rev-parse", "-q", "--verify"])
        .arg(OsStr::from_bytes(&remote))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if on_origin {
        let ahead = [&b"origin/"[..], &branch, b"..HEAD"].concat();
        let behind = [&b"HEAD..origin/"[..], &branch].concat();
        let ahead = count(dir, &[OsStr::from_bytes(&ahead)]);
        let behind = count(dir, &[OsStr::from_bytes(&behind)]);
        [
            &ahead[..],
            b" unpushed, ",
            &behind,
            b" behind origin/",
            &branch,
        ]
        .concat()
    } else {
        let n = count(
            dir,
            &[
                OsStr::new("HEAD"),
                OsStr::new("--not"),
                OsStr::new("--remotes"),
            ],
        );
        [&b"not on origin; "[..], &n, b" commit(s) on no remote"].concat()
    }
}

/// Print the section body: the fetch (unless `--no-fetch`), the checkout, origin/main and
/// the worktree list.
pub fn print(ctx: &Ctx, fetch: bool, out: &mut dyn Write) -> Result<()> {
    let root = ctx.root.as_path();
    if fetch {
        let fetched = git::command(root)
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(["fetch", "--quiet", "--prune", "origin"])
            .output();
        match fetched {
            Ok(output) => {
                out.write_all(&output.stdout)?;
                if !output.status.success() {
                    let reason = crate::cmd::checkpoint::scan::first_line(&output.stderr);
                    line(
                        out,
                        &[b"- fetch failed (", reason, b"); remote refs may be stale"],
                    )?;
                }
            }
            Err(e) => {
                line(
                    out,
                    &[
                        b"- fetch failed (",
                        e.to_string().as_bytes(),
                        b"); remote refs may be stale",
                    ],
                )?;
            }
        }
    }
    let branch = branch_or_detached(root);
    let dirty = dirty_count(root).to_string();
    let head = git::out(root, &["log", "-1", "--format=%h %s"]).unwrap_or_default();
    line(
        out,
        &[
            b"- checkout: ",
            &ctx.rel_path(root),
            b" on `",
            &branch,
            b"` @ ",
            &head,
        ],
    )?;
    line(
        out,
        &[
            b"- uncommitted: ",
            dirty.as_bytes(),
            b" file(s); ",
            &unpushed_desc(root),
        ],
    )?;
    if git::ok(
        root,
        &["rev-parse", "-q", "--verify", "refs/remotes/origin/main"],
    ) {
        let tip = git::out(root, &["log", "-1", "--format=%h %s (%cr)", "origin/main"])
            .unwrap_or_default();
        line(out, &[b"- origin/main @ ", &tip])?;
        if git::ok(root, &["rev-parse", "-q", "--verify", "refs/heads/main"]) {
            let behind = count(root, &[OsStr::new("main..origin/main")]);
            if behind != b"0" {
                line(
                    out,
                    &[
                        b"- local main is ",
                        &behind,
                        b" commit(s) behind origin/main",
                    ],
                )?;
            }
        }
    }
    line(out, &[b"- worktrees:"])?;
    for worktree in &ctx.worktrees {
        if worktree.as_os_str().is_empty() {
            continue;
        }
        let label = ctx.rel_path(worktree);
        if !worktree.is_dir() {
            line(
                out,
                &[b"  - ", &label, b": missing (run `git worktree prune`)"],
            )?;
            continue;
        }
        let branch = branch_or_detached(worktree);
        let dirty = dirty_count(worktree).to_string();
        line(
            out,
            &[
                b"  - ",
                &label,
                b": `",
                &branch,
                b"`, ",
                dirty.as_bytes(),
                b" uncommitted, ",
                &unpushed_desc(worktree),
            ],
        )?;
    }
    Ok(())
}
