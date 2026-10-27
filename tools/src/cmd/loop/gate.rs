//! The Option-A gate (owner decision on #204): a PR that changes any input of
//! the `aios` binary ([`crate::inputs`]) is never `ready`, whatever the lenses
//! say. The gate reads git, not GitHub; [`cross_check`] then makes sure
//! GitHub's file list does not contradict git's.

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

use anyhow::{bail, Result};

use super::git::{changed_paths, merge_base};
use crate::inputs::is_tools_input;

/// A repository-relative path in one form: no leading `./`, no `//`, no `.`
/// segments, no trailing `/`. A `..` segment or a leading `/` is an error (git
/// never prints one, so GitHub text that has one is not to be trusted).
pub fn normalise(path: &str) -> Result<String> {
    if path.starts_with('/') {
        bail!("path {path:?} is absolute");
    }
    let mut parts = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => bail!("path {path:?} has a .. segment"),
            s => parts.push(s),
        }
    }
    if parts.is_empty() {
        bail!("path {path:?} is empty");
    }
    Ok(parts.join("/"))
}

/// Every path that differs between the merge-base of `origin/main` and `head`
/// and `head`, both sides of a rename included. (A non-UTF-8 name is read
/// lossily: its prefix still decides the gate, so the answer fails closed.)
pub fn changed_set(repo: &Path, head: &str) -> Result<BTreeSet<String>> {
    let base = merge_base(repo, "origin/main", head)?;
    Ok(changed_paths(repo, &base, head)?
        .into_iter()
        .map(|c| c.path.to_string_lossy().into_owned())
        .collect())
}

/// The part of [`changed_set`] that is an input of the `aios` binary: sorted,
/// unique. Non-empty means the PR is gated.
pub fn gated_paths(repo: &Path, head: &str) -> Result<Vec<String>> {
    Ok(changed_set(repo, head)?
        .into_iter()
        .filter(|p| is_tools_input(p))
        .collect())
}

/// GitHub's file list disagrees with git's, or cannot be checked. The verdict
/// is needs-human.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub reason: String,
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GitHub and git disagree on the changed files: {}",
            self.reason
        )
    }
}

impl std::error::Error for Mismatch {}

fn mismatch(reason: String) -> Result<(), Mismatch> {
    Err(Mismatch { reason })
}

/// Check GitHub's view of the PR against git's:
/// - every GitHub path, normalised, is in `git_set` (GitHub may list fewer: a
///   rename shows only its new side there);
/// - `changed_files` is below 100, because GitHub lists at most 100 files, so a
///   larger PR cannot be compared;
/// - `changed_files` equals the number of files GitHub listed.
pub fn cross_check(
    git_set: &BTreeSet<String>,
    github_files: &[String],
    changed_files: u64,
) -> Result<(), Mismatch> {
    if changed_files >= 100 {
        return mismatch(format!(
            "changedFiles is {changed_files}; GitHub lists at most 100 files"
        ));
    }
    if changed_files != github_files.len() as u64 {
        return mismatch(format!(
            "changedFiles is {changed_files} but GitHub listed {} files",
            github_files.len()
        ));
    }
    for file in github_files {
        match normalise(file) {
            Ok(n) if git_set.contains(&n) => {}
            Ok(n) => return mismatch(format!("{n} is not among git's changed paths")),
            Err(e) => return mismatch(e.to_string()),
        }
    }
    Ok(())
}
