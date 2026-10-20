//! The other worktrees that hold work not on origin. They are only listed: a background
//! agent may be working in one, so checkpoint never commits there.

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use super::scan::EXCLUDES;
use crate::git;

/// A worktree other than the current one that has uncommitted files or commits not on
/// origin.
#[derive(Debug, PartialEq, Eq)]
pub struct Other {
    /// The path as `git worktree list --porcelain` printed it.
    pub path: PathBuf,
    /// The checked-out branch, or `detached HEAD`.
    pub branch: Vec<u8>,
    /// Uncommitted files (lines of `status --porcelain`).
    pub dirty: usize,
    /// `rev-list --count` of the commits not on origin, as git printed it.
    pub unpushed: String,
}

/// The number of uncommitted files in `dir`: the lines of `status --porcelain` over
/// every untracked file, minus the excluded paths. `no_optional_locks` is for a
/// worktree that is not ours, so that no `index.lock` is taken from under an agent
/// working there.
pub fn dirty(dir: &Path, no_optional_locks: bool) -> usize {
    let mut args = Vec::new();
    if no_optional_locks {
        args.push("--no-optional-locks");
    }
    args.extend(["status", "--porcelain", "--untracked-files=all", "--", "."]);
    args.extend(EXCLUDES);
    git::output(dir, &args)
        .map(|output| output.stdout.iter().filter(|&&b| b == b'\n').count())
        .unwrap_or(0)
}

/// The commits of `dir` that are not on origin, as git counts them (`0` when git fails).
pub fn unpushed(dir: &Path) -> String {
    git::text(
        dir,
        &["rev-list", "--count", "HEAD", "--not", "--remotes=origin"],
    )
    .unwrap_or_else(|| "0".to_string())
}

/// The listed worktrees that exist as directories and are not `here`: a path that is
/// not a directory, or cannot be entered, is skipped.
pub fn candidates(here: &Path, listed: &[PathBuf]) -> Vec<PathBuf> {
    listed
        .iter()
        .filter(|path| !path.as_os_str().is_empty() && path.is_dir())
        .filter(|path| path.canonicalize().is_ok_and(|real| real != here))
        .cloned()
        .collect()
}

/// Every other worktree of `listed` holding work. `root` is the current toplevel.
pub fn list(root: &Path, listed: &[PathBuf]) -> Vec<Other> {
    let here = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    candidates(&here, listed)
        .into_iter()
        .filter_map(|path| {
            let branch = git::out(&path, &["symbolic-ref", "--quiet", "--short", "HEAD"])
                .unwrap_or_else(|| b"detached HEAD".to_vec());
            let (dirty, unpushed) = (dirty(&path, true), unpushed(&path));
            (dirty != 0 || unpushed != "0").then_some(Other {
                path,
                branch,
                dirty,
                unpushed,
            })
        })
        .collect()
}

/// The label of a worktree in the listing: `(main checkout)` for the main worktree,
/// its path relative to the main worktree below it, else the path as listed.
pub fn label(path: &Path, main: &Path) -> Vec<u8> {
    let (path, main) = (path.as_os_str().as_bytes(), main.as_os_str().as_bytes());
    if path == main {
        return b"(main checkout)".to_vec();
    }
    match path
        .strip_prefix(main)
        .and_then(|rest| rest.strip_prefix(b"/"))
    {
        Some(relative) => relative.to_vec(),
        None => path.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aios-checkpoint-others-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the directory");
        dir.canonicalize().expect("canonicalise the directory")
    }

    #[test]
    fn candidates_skip_a_missing_directory_and_the_current_one() {
        let base = dir("candidates");
        let (here, other, missing) = (base.join("here"), base.join("other"), base.join("gone"));
        std::fs::create_dir_all(&here).expect("create here");
        std::fs::create_dir_all(&other).expect("create other");
        let listed = vec![
            here.clone(),
            missing,
            PathBuf::new(),
            other.clone(),
            // The same directory under a second spelling is still the current one.
            base.join("other/../here"),
        ];
        assert_eq!(candidates(&here, &listed), vec![other]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn labels_are_relative_to_the_main_worktree() {
        let main = Path::new("/repo");
        assert_eq!(label(Path::new("/repo"), main), b"(main checkout)");
        assert_eq!(
            label(Path::new("/repo/.claude/worktrees/w1"), main),
            b".claude/worktrees/w1"
        );
        // A sibling that merely shares the prefix is not below the main worktree.
        assert_eq!(label(Path::new("/repo-two/w"), main), b"/repo-two/w");
        assert_eq!(label(Path::new("/elsewhere/w"), main), b"/elsewhere/w");
    }

    #[test]
    fn counts_are_zero_when_git_fails() {
        let base = dir("counts");
        // Not a repository (and the directory is under the temp dir, not a checkout).
        let missing = base.join("missing");
        assert_eq!(dirty(&missing, true), 0);
        assert_eq!(unpushed(&missing), "0");
        let _ = std::fs::remove_dir_all(&base);
    }
}
