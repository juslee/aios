//! Subprocesses: `capture`, docs-check's single entry point for running `git`
//! (check.py L396-397 `Repo.git` and L1578 `repo_root`), and `Supervisor`, which
//! runs a child under a time limit in its own process group the way
//! `timeout --kill-after` did for the soak harness.

use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

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

/// A child run the way `timeout --kill-after=K T cmd` runs one: as the leader of
/// its own process group, which gets SIGTERM at the time limit T and SIGKILL K
/// later. [`Supervisor::exit_code`] reports what `timeout` exits with: 124 when
/// the limit ran out, 137 when SIGKILL was needed, otherwise the child's exit
/// code, or 128 + the signal that killed it.
///
/// Dropping a supervisor whose child is still running sends the group SIGTERM
/// and waits for it (SIGKILL after the grace period), so an error or a panic in
/// the caller never leaves the child behind.
pub struct Supervisor {
    child: Child,
    deadline: Instant,
    kill_after: Duration,
    term_sent: Option<Instant>,
    timed_out: bool,
    kill_sent: bool,
    status: Option<ExitStatus>,
}

impl Supervisor {
    /// Spawn `command` in a new process group with a time limit of `limit`.
    pub fn spawn(
        mut command: Command,
        limit: Duration,
        kill_after: Duration,
    ) -> std::io::Result<Supervisor> {
        command.process_group(0);
        let child = command.spawn()?;
        Ok(Supervisor {
            child,
            deadline: Instant::now() + limit,
            kill_after,
            term_sent: None,
            timed_out: false,
            kill_sent: false,
            status: None,
        })
    }

    /// The child's process id, which is also its process group id.
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Reap the child if it has exited; otherwise send whichever signal is due.
    pub fn service(&mut self) -> Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        if let Some(status) = self.child.try_wait().context("cannot wait for the child")? {
            self.status = Some(status);
            return Ok(());
        }
        let now = Instant::now();
        if self.term_sent.is_none() && now >= self.deadline {
            self.timed_out = true;
            signal_group(self.id(), "TERM")?;
            self.term_sent = Some(now);
        }
        if let Some(sent) = self.term_sent {
            if !self.kill_sent && now >= sent + self.kill_after {
                signal_group(self.id(), "KILL")?;
                self.kill_sent = true;
            }
        }
        Ok(())
    }

    /// `timeout`'s exit status once the child has been reaped.
    pub fn exit_code(&self) -> Option<i32> {
        let status = self.status?;
        Some(
            if self.kill_sent || (self.timed_out && status.signal() == Some(9)) {
                137
            } else if self.timed_out {
                124
            } else if let Some(code) = status.code() {
                code
            } else {
                128 + status.signal().unwrap_or(0)
            },
        )
    }

    /// Send the group SIGTERM now, as the harness does when it is interrupted;
    /// SIGKILL follows after the grace period if the child is still running.
    pub fn terminate(&mut self) -> Result<()> {
        self.service()?;
        if self.status.is_none() && self.term_sent.is_none() {
            signal_group(self.id(), "TERM")?;
            self.term_sent = Some(Instant::now());
        }
        Ok(())
    }

    /// Block until the child has exited, sending the signals that fall due.
    pub fn wait(&mut self) -> Result<i32> {
        loop {
            self.service()?;
            if let Some(code) = self.exit_code() {
                return Ok(code);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        if self.status.is_none() {
            let _ = self.terminate();
            let _ = self.wait();
        }
    }
}

/// Send `signal` (a name such as `TERM`) to process group `pgid` with the POSIX
/// `kill` utility: std can only SIGKILL its own child, not a group, and this
/// crate forbids `unsafe`. A group that has already exited is not an error.
pub fn signal_group(pgid: u32, signal: &str) -> Result<()> {
    Command::new("kill")
        .arg(format!("-{signal}"))
        .arg("--")
        .arg(format!("-{pgid}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("cannot run kill -{signal}"))?;
    Ok(())
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

    /// Whether process `pid` still exists (`kill -0`).
    fn alive(pid: &str) -> bool {
        Command::new("kill")
            .args(["-0", pid])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// Wait up to 5 s for `pid` to disappear; its reaper may need a moment.
    fn gone(pid: &str) -> bool {
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            if !alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn sh(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]).stdin(Stdio::null());
        command
    }

    fn supervise(script: &str, limit_ms: u64, kill_after_ms: u64) -> (i32, Duration) {
        let start = Instant::now();
        let mut s = Supervisor::spawn(
            sh(script),
            Duration::from_millis(limit_ms),
            Duration::from_millis(kill_after_ms),
        )
        .expect("sh starts");
        let code = s.wait().expect("wait");
        (code, start.elapsed())
    }

    #[test]
    fn a_child_that_finishes_in_time_keeps_its_exit_code() {
        let (code, took) = supervise("exit 3", 5_000, 1_000);
        assert_eq!(code, 3);
        assert!(took < Duration::from_secs(4), "{took:?}");
    }

    #[test]
    fn a_child_killed_by_a_signal_reports_128_plus_the_signal() {
        assert_eq!(supervise("kill -KILL $$", 5_000, 1_000).0, 137);
        assert_eq!(supervise("kill -SEGV $$", 5_000, 1_000).0, 139);
    }

    #[test]
    fn the_time_limit_stops_the_child_with_124() {
        let (code, took) = supervise("exec sleep 30", 500, 5_000);
        assert_eq!(code, 124);
        assert!(took < Duration::from_secs(4), "{took:?}");
    }

    #[test]
    fn a_child_that_ignores_sigterm_gets_sigkill_and_137() {
        let (code, took) = supervise("trap '' TERM; while :; do sleep 1; done", 300, 700);
        assert_eq!(code, 137);
        assert!(took >= Duration::from_millis(1_000), "{took:?}");
        assert!(took < Duration::from_secs(5), "{took:?}");
    }

    #[test]
    fn the_whole_process_group_is_stopped() {
        let dir = std::env::temp_dir().join(format!("aios-proc-group-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let pidfile = dir.join("child.pid");
        let script = format!("sleep 30 & echo $! >'{}'; exec sleep 30", pidfile.display());
        let (code, _) = supervise(&script, 500, 5_000);
        assert_eq!(code, 124);
        let child = std::fs::read_to_string(&pidfile).expect("the child wrote its pid");
        assert!(
            gone(child.trim()),
            "background child {} survived",
            child.trim()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn terminate_stops_the_child_before_the_limit() {
        let mut s = Supervisor::spawn(
            sh("exec sleep 30"),
            Duration::from_secs(60),
            Duration::from_secs(5),
        )
        .expect("sh starts");
        s.terminate().expect("terminate");
        // Not a timeout: the child's own status, SIGTERM.
        assert_eq!(s.wait().expect("wait"), 143);
    }

    #[test]
    fn dropping_a_supervisor_stops_its_child() {
        let s = Supervisor::spawn(
            sh("exec sleep 30"),
            Duration::from_secs(60),
            Duration::from_secs(5),
        )
        .expect("sh starts");
        let pid = s.id().to_string();
        drop(s);
        assert!(gone(&pid), "child {pid} survived the drop");
    }

    #[test]
    fn signalling_a_group_that_is_gone_is_not_an_error() {
        signal_group(999_999, "TERM").expect("kill ran");
    }
}
