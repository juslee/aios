//! Subprocess capture, the port's single entry point for running `git`
//! (check.py L396-397 `Repo.git` and L1578 `repo_root`).

use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, Result};

/// Run `program args` in `cwd`, capturing stdout and stderr. The result is an
/// error only when the process cannot start; a non-zero exit is reported in the
/// returned `Output`, as `subprocess.run(..., capture_output=True)` does.
pub fn capture(program: &str, args: &[&str], cwd: &Path) -> Result<Output> {
    Command::new(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("cannot run {program} in {}", cwd.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_returns_both_streams_and_the_status() {
        let dir = std::env::temp_dir();
        let out =
            capture("sh", &["-c", "printf out; printf err >&2; exit 3"], &dir).expect("sh starts");
        assert_eq!(out.stdout, b"out");
        assert_eq!(out.stderr, b"err");
        assert_eq!(out.status.code(), Some(3));
    }

    #[test]
    fn capture_runs_in_the_given_directory() {
        let dir = std::fs::canonicalize(std::env::temp_dir()).expect("canonicalize the temp dir");
        let out = capture("sh", &["-c", "pwd -P"], &dir).expect("sh starts");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim_end(),
            dir.to_string_lossy()
        );
    }

    #[test]
    fn capture_errors_when_the_program_cannot_start() {
        let err = capture("aios-no-such-program", &[], &std::env::temp_dir())
            .expect_err("there is no such program");
        assert!(
            format!("{err:#}").contains("cannot run aios-no-such-program"),
            "{err:#}"
        );
    }
}
