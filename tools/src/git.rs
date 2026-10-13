//! git as the former `scripts/agent/brief.sh` (blob at `2d4a9f7`) ran it, and as
//! the former `scripts/agent/checkpoint.sh` (blob at `faf6d20`) did: the inherited
//! environment, unchanged.
//!
//! No `LC_ALL`, no `GIT_NO_REPLACE_OBJECTS`, no `GIT_GRAFT_FILE` and no
//! `GIT_TERMINAL_PROMPT` by default: the scripts set none of them, so setting them
//! here would be an unlisted divergence. A caller adds a per-call extra with
//! `command(cwd).env(..)` (for example checkpoint's `GIT_TERMINAL_PROMPT=0` on its
//! push). Calls that feed a security decision use the hardened constructor in
//! `cmd::soak::host`, not this module.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// `git` in `cwd` with stdin null and the environment untouched.
pub fn command(cwd: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd).stdin(Stdio::null());
    cmd
}

/// `$(git ...)`: stdout without its trailing newlines; `None` on a spawn error or a
/// non-zero exit. Stderr is dropped.
pub fn out(cwd: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = command(cwd)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut stdout = output.stdout;
    while stdout.last() == Some(&b'\n') {
        stdout.pop();
    }
    Some(stdout)
}

/// [`out`] when the output is UTF-8, else `None`.
pub fn text(cwd: &Path, args: &[&str]) -> Option<String> {
    String::from_utf8(out(cwd, args)?).ok()
}

/// Whether `git args` exits 0. Output is dropped.
pub fn ok(cwd: &Path, args: &[&str]) -> bool {
    command(cwd)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// `git args` with stdout and stderr captured, whatever the exit status.
pub fn output(cwd: &Path, args: &[&str]) -> std::io::Result<Output> {
    command(cwd).args(args).output()
}

/// The `worktree ` lines of `git worktree list --porcelain`, the first being the
/// main worktree; empty when git fails.
pub fn worktrees(cwd: &Path) -> Vec<PathBuf> {
    out(cwd, &["worktree", "list", "--porcelain"])
        .map(|porcelain| parse_worktrees(&porcelain))
        .unwrap_or_default()
}

/// `awk '/^worktree /{print substr($0, 10)}'` over the porcelain bytes, so a path
/// that is not UTF-8 survives.
fn parse_worktrees(porcelain: &[u8]) -> Vec<PathBuf> {
    porcelain
        .split(|&b| b == b'\n')
        .filter_map(|line| line.strip_prefix(b"worktree "))
        .map(|path| PathBuf::from(OsStr::from_bytes(path)))
        .collect()
}

/// The checked-out branch (`symbolic-ref --short -q HEAD`); `None` when detached.
pub fn branch(cwd: &Path) -> Option<String> {
    text(cwd, &["symbolic-ref", "--short", "-q", "HEAD"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A throwaway repository with one commit, removed when dropped. Setup git runs
    /// with the ambient configuration masked, because only the code under test is
    /// meant to inherit its environment.
    struct Repo(PathBuf);

    impl Repo {
        fn new(label: &str) -> Repo {
            static N: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "aios-git-{label}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create the repository directory");
            let repo = Repo(dir);
            repo.git(&["init", "-q", "-b", "main"]);
            repo.git(&["commit", "-q", "--allow-empty", "-m", "first"]);
            repo
        }

        fn git(&self, args: &[&str]) {
            let status = Command::new("git")
                .args([
                    "-c",
                    "user.name=aios-test",
                    "-c",
                    "user.email=aios-test@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .current_dir(&self.0)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .status()
                .expect("run git");
            assert!(status.success(), "git {args:?} failed");
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn git_command_sets_no_env() {
        let cmd = command(Path::new("."));
        assert_eq!(cmd.get_envs().count(), 0);
        assert_eq!(cmd.get_program(), "git");
        assert_eq!(cmd.get_current_dir(), Some(Path::new(".")));
    }

    #[test]
    fn out_chomps_trailing_newlines() {
        let repo = Repo::new("chomp");
        std::fs::write(repo.0.join("blob"), "x\n\n\n").expect("write the blob");
        let hash = out(&repo.0, &["hash-object", "-w", "blob"]).expect("hash the blob");
        let hash = String::from_utf8(hash).expect("a hex hash");
        // `git cat-file blob` prints the three newlines; `$(...)` and out() drop them.
        assert_eq!(
            out(&repo.0, &["cat-file", "blob", &hash]),
            Some(b"x".to_vec())
        );
        assert_eq!(
            text(&repo.0, &["cat-file", "blob", &hash]),
            Some("x".into())
        );
        // A final line without a newline is returned whole, and empty output is Some.
        assert_eq!(
            text(&repo.0, &["log", "-1", "--format=%s"]),
            Some("first".into())
        );
        assert_eq!(out(&repo.0, &["log", "-1", "--format="]), Some(Vec::new()));
        assert!(ok(&repo.0, &["rev-parse", "--verify", "-q", "HEAD"]));
    }

    #[test]
    fn out_is_none_on_failure() {
        let repo = Repo::new("fail");
        assert_eq!(
            out(&repo.0, &["rev-parse", "--verify", "-q", "no-such-ref"]),
            None
        );
        assert_eq!(text(&repo.0, &["no-such-subcommand"]), None);
        assert!(!ok(
            &repo.0,
            &["rev-parse", "--verify", "-q", "no-such-ref"]
        ));
        assert_eq!(out(&repo.0.join("missing"), &["status"]), None);
        assert!(!ok(&repo.0.join("missing"), &["status"]));
        assert!(output(&repo.0.join("missing"), &["status"]).is_err());
        let failed = output(&repo.0, &["rev-parse", "--verify", "no-such-ref"]).expect("spawn");
        assert!(!failed.status.success());
        assert!(!failed.stderr.is_empty());
    }

    #[test]
    fn worktrees_reads_porcelain_bytes() {
        let porcelain = b"worktree /main/checkout\nHEAD 0123\nbranch refs/heads/main\n\n\
                          worktree /wt/\xff\xfe-not-utf8\nHEAD 4567\ndetached\n\n\
                          worktree  spaced path\n";
        let parsed = parse_worktrees(porcelain);
        assert_eq!(
            parsed,
            vec![
                PathBuf::from("/main/checkout"),
                PathBuf::from(OsStr::from_bytes(b"/wt/\xff\xfe-not-utf8")),
                PathBuf::from(" spaced path"),
            ]
        );
        assert_eq!(parsed[1].to_str(), None);
        assert!(parse_worktrees(b"").is_empty());
        // The real command lists the main worktree first.
        let repo = Repo::new("worktrees");
        let listed = worktrees(&repo.0);
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].canonicalize().expect("canonicalise"),
            repo.0.canonicalize().expect("canonicalise")
        );
        assert!(worktrees(&repo.0.join("missing")).is_empty());
    }

    #[test]
    fn branch_is_none_when_detached() {
        let repo = Repo::new("branch");
        assert_eq!(branch(&repo.0), Some("main".into()));
        repo.git(&["checkout", "-q", "--detach"]);
        assert_eq!(branch(&repo.0), None);
    }
}
