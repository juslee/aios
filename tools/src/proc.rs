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
/// the caller never leaves the child behind. When the `kill` utility cannot be
/// run, or runs but fails while the leader still runs, the supervisor says so on
/// stderr and SIGKILLs the leader directly.
pub struct Supervisor {
    child: Child,
    /// `None` when the limit is too far ahead for an `Instant`: no limit.
    deadline: Option<Instant>,
    kill_after: Duration,
    term_sent: Option<Instant>,
    timed_out: bool,
    kill_sent: bool,
    status: Option<ExitStatus>,
    /// The program [`Supervisor::signal`] runs; tests replace it.
    kill_program: &'static str,
    /// The name the supervisor's warnings start with (`<who>: warning:`).
    who: &'static str,
}

impl Supervisor {
    /// Spawn `command` in a new process group with a time limit of `limit`. A
    /// limit too large for the clock (such as `--secs` near `i64::MAX`) means no
    /// limit, so nothing here can panic once the child exists.
    pub fn spawn(
        mut command: Command,
        limit: Duration,
        kill_after: Duration,
    ) -> std::io::Result<Supervisor> {
        command.process_group(0);
        let child = command.spawn()?;
        Ok(Supervisor {
            child,
            deadline: Instant::now().checked_add(limit),
            kill_after,
            term_sent: None,
            timed_out: false,
            kill_sent: false,
            status: None,
            kill_program: "kill",
            who: "aios",
        })
    }

    /// Start the supervisor's warnings with `who: warning:` instead of `aios: warning:`.
    pub fn warn_as(&mut self, who: &'static str) {
        self.who = who;
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
        if self.term_sent.is_none() && self.deadline.is_some_and(|d| now >= d) {
            self.timed_out = true;
            self.signal("TERM");
            self.term_sent = Some(now);
        }
        if let Some(sent) = self.term_sent {
            let due = sent.checked_add(self.kill_after).is_some_and(|t| now >= t);
            if !self.kill_sent && due && self.signal("KILL") {
                self.kill_sent = true;
            }
        }
        Ok(())
    }

    /// Send `signal` to the child's process group. When `kill` cannot run, or
    /// fails while the leader has not exited (a `kill` that rejects
    /// `-SIG -- -PGID`, or EPERM), say so on stderr and SIGKILL the leader with
    /// [`Child::kill`] instead, so the child never runs on without a limit; a
    /// failure there is reported too, and the next [`Supervisor::service`] tries
    /// again. Returns whether a signal went out, or the leader has exited.
    fn signal(&mut self, signal: &str) -> bool {
        let pid = self.id();
        let problem = match signal_group_with(self.kill_program, pid, signal) {
            Ok(status) if status.success() => return true,
            // kill failed. That is expected once the leader has exited (the
            // group is gone, or holds only the unreaped leader); otherwise
            // nothing was signalled and the child runs on.
            Ok(status) => match self.child.try_wait() {
                Ok(Some(exited)) => {
                    self.status = Some(exited);
                    return true;
                }
                Ok(None) => format!(
                    "{} -{signal} for process group {pid} failed ({status})",
                    self.kill_program
                ),
                Err(e) => format!("cannot wait for process {pid}: {e}"),
            },
            Err(e) => format!("{e:#}"),
        };
        self.warn(&format!(
            "{problem}; sending SIGKILL to process {pid} instead"
        ));
        match self.child.kill() {
            Ok(()) => {
                self.kill_sent = true;
                true
            }
            Err(e) => {
                self.warn(&format!("cannot SIGKILL process {pid}: {e}"));
                false
            }
        }
    }

    fn warn(&self, message: &str) {
        eprintln!("{}: warning: {message}", self.who);
    }

    /// Suspend the child for as long as `stopped` runs, which stops the caller
    /// itself until it is continued (Ctrl-Z): SIGSTOP the child's process group,
    /// run `stopped`, then SIGCONT the group and move the time limit (and a
    /// SIGKILL that is due) back by the time spent stopped, which the stopped
    /// child did not use. When the group cannot be stopped, say so and return
    /// without running `stopped`: a caller stopped while the child runs on
    /// would leave the child without a limit.
    pub fn suspend(&mut self, stopped: impl FnOnce()) -> Result<()> {
        self.service()?;
        if self.status.is_some() {
            stopped();
            return Ok(());
        }
        let pid = self.id();
        match signal_group_with(self.kill_program, pid, "STOP") {
            Ok(status) if status.success() => {}
            Ok(status) => {
                // As in signal: a leader that has exited ended the group.
                if let Ok(Some(exited)) = self.child.try_wait() {
                    self.status = Some(exited);
                    stopped();
                } else {
                    self.warn(&format!(
                        "{} -STOP for process group {pid} failed ({status}); not suspending",
                        self.kill_program
                    ));
                }
                return Ok(());
            }
            Err(e) => {
                self.warn(&format!("{e:#}; not suspending"));
                return Ok(());
            }
        }
        let since = Instant::now();
        stopped();
        let paused = since.elapsed();
        match signal_group_with(self.kill_program, pid, "CONT") {
            Ok(status) if status.success() => {}
            // A group that stays stopped still ends at the limit: SIGKILL
            // ends a stopped process.
            Ok(status) => self.warn(&format!(
                "{} -CONT for process group {pid} failed ({status})",
                self.kill_program
            )),
            Err(e) => self.warn(&format!("{e:#}")),
        }
        self.deadline = self.deadline.and_then(|d| d.checked_add(paused));
        self.term_sent = self.term_sent.map(|t| t.checked_add(paused).unwrap_or(t));
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
            self.signal("TERM");
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
            if let Err(e) = self.terminate().and_then(|()| self.wait()) {
                let pid = self.id();
                self.warn(&format!("{e:#}; sending SIGKILL to process {pid}"));
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }
}

/// Send `signal` (a name such as `TERM`) to process group `pgid` with the POSIX
/// `kill` utility: std can only SIGKILL its own child, not a group, and this
/// crate forbids `unsafe`. A group that has already exited is not an error.
pub fn signal_group(pgid: u32, signal: &str) -> Result<()> {
    signal_group_with("kill", pgid, signal)?;
    Ok(())
}

/// Run `program -SIGNAL -- -PGID` (the `kill` utility, or a test's stand-in)
/// and return its exit status: an error only when it cannot run. It runs in its
/// own process group, so a terminal signal (Ctrl-C, Ctrl-Z) meant for the
/// harness never stops it while the harness waits for it.
fn signal_group_with(program: &str, pgid: u32, signal: &str) -> Result<ExitStatus> {
    Command::new(program)
        .arg(format!("-{signal}"))
        .arg("--")
        .arg(format!("-{pgid}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .status()
        .with_context(|| format!("cannot run {program} -{signal} for process group {pgid}"))
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
        supervise_for(
            script,
            Duration::from_millis(limit_ms),
            Duration::from_millis(kill_after_ms),
        )
    }

    fn supervise_for(script: &str, limit: Duration, kill_after: Duration) -> (i32, Duration) {
        let start = Instant::now();
        let mut s = Supervisor::spawn(sh(script), limit, kill_after).expect("sh starts");
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
    fn a_limit_beyond_the_clock_is_no_limit() {
        for limit in [Duration::from_secs(i64::MAX as u64), Duration::MAX] {
            assert_eq!(supervise_for("exit 3", limit, Duration::MAX).0, 3);
        }
    }

    /// A supervisor that signals with `kill_program` instead of `kill`.
    fn with_kill(kill_program: &'static str, script: &str, limit: Duration) -> Supervisor {
        let mut s =
            Supervisor::spawn(sh(script), limit, Duration::from_secs(5)).expect("sh starts");
        s.kill_program = kill_program;
        s
    }

    /// No `kill` utility, and one that runs but signals nothing (`false`).
    const BROKEN_KILLS: [&str; 2] = ["aios-no-such-program", "false"];

    #[test]
    fn a_broken_kill_still_lets_the_time_limit_sigkill_the_leader() {
        for program in BROKEN_KILLS {
            let start = Instant::now();
            let mut s = with_kill(program, "exec sleep 30", Duration::from_millis(300));
            let pid = s.id().to_string();
            assert_eq!(s.wait().expect("wait"), 137, "{program}");
            assert!(
                start.elapsed() < Duration::from_secs(4),
                "{program}: {:?}",
                start.elapsed()
            );
            assert!(gone(&pid), "{program}: child {pid} survived");
        }
    }

    #[test]
    fn a_broken_kill_still_lets_terminate_and_drop_sigkill_the_leader() {
        for program in BROKEN_KILLS {
            let mut s = with_kill(program, "exec sleep 30", Duration::from_secs(60));
            s.terminate().expect("terminate");
            assert_eq!(s.wait().expect("wait"), 137, "{program}");
            let s = with_kill(program, "exec sleep 30", Duration::from_secs(60));
            let pid = s.id().to_string();
            drop(s);
            assert!(gone(&pid), "{program}: child {pid} survived the drop");
        }
    }

    /// `ps`'s state letter for `pid` (`T` when stopped), or empty.
    fn state(pid: &str) -> String {
        let out = Command::new("ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
            .expect("run ps");
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .chars()
            .take(1)
            .collect()
    }

    #[test]
    fn suspend_stops_the_group_and_moves_the_limit_back() {
        let start = Instant::now();
        let mut s = Supervisor::spawn(
            sh("exec sleep 30"),
            Duration::from_millis(500),
            Duration::from_secs(5),
        )
        .expect("sh starts");
        let pid = s.id().to_string();
        let mut seen = String::new();
        s.suspend(|| {
            seen = state(&pid);
            // Longer than the limit: a stopped child must not use it up.
            std::thread::sleep(Duration::from_millis(1_000));
        })
        .expect("suspend");
        assert_eq!(seen, "T", "the child was not stopped");
        s.service().expect("service");
        assert_eq!(s.exit_code(), None, "the limit ran out while stopped");
        assert_ne!(state(&pid), "T", "the child was not continued");
        assert_eq!(s.wait().expect("wait"), 124);
        assert!(
            start.elapsed() >= Duration::from_millis(1_500),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn suspend_does_not_stop_the_caller_when_the_group_cannot_be_stopped() {
        for program in BROKEN_KILLS {
            let mut s = with_kill(program, "exec sleep 30", Duration::from_secs(60));
            let mut ran = false;
            s.suspend(|| ran = true).expect("suspend");
            assert!(
                !ran,
                "{program}: the caller would stop while the child runs"
            );
        }
    }

    #[test]
    fn signalling_a_group_that_is_gone_is_not_an_error() {
        // No host can allocate this process group id: Linux pids stop at
        // PID_MAX_LIMIT (4194304 on 64-bit), macOS's at 99999. A smaller id
        // such as 999999 can be a live group on Linux, which would get SIGTERM.
        signal_group(i32::MAX as u32, "TERM").expect("kill ran");
    }
}
