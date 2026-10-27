//! The loop's local state and its preflight checks.
//!
//! State lives under `<git-common-dir>/aios-agent/loop/`, shared by every
//! worktree of the repository:
//!
//! ```text
//! pr-<n>/lock/pid                      the per-PR lock (made by mkdir)
//! pr-<n>/stage-settings.json           the settings every stage runs under
//! pr-<n>/run-<epoch>/round-01-<stage>.json, report.json, report.md
//! findings.jsonl                       one record per verified finding
//! STOP                                 the kill-switch file
//! ```
//!
//! In a debug-assertion build `AIOS_LOOP_STATE_DIR` replaces the directory
//! (the scenario tests use it); a release build ignores it.
//!
//! The preflight checks here read local state and `gh`; none writes to GitHub.
//! Each returns a typed refusal so the caller picks the exit code: a
//! [`StopReason`] is exit 4, a [`Refusal`] exit 1, an `anyhow` error exit 2.

use std::ffi::OsString;
use std::fmt;
use std::io::ErrorKind;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::git::git_in;
use super::github::{issues_labelled, label_exists, PrView};
use super::schema::Severity;
use crate::cmd::hook::append_jsonl;
use crate::gh::GhRunner;

/// The variable that replaces the state directory (debug builds only).
pub const STATE_DIR_VAR: &str = "AIOS_LOOP_STATE_DIR";

/// The variable that lets a debug build run from any executable path. It holds
/// only together with [`STATE_DIR_VAR`] and a fake `gh` and `claude` first on
/// PATH (see [`check_location`]).
pub const ANY_EXE_VAR: &str = "AIOS_LOOP_ANY_EXE";

/// A lock directory that has no `pid` file yet is "starting" for this long;
/// after that it is a leftover of a crash and is taken over.
const LOCK_STARTING: Duration = Duration::from_secs(60);

/// The run must stop: the kill switch is set or cannot be read, or the QEMU
/// lock holds a quiet window. Exit 4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopReason(pub String);

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StopReason {}

/// The PR is not this session's to review. Exit 1; the text is printed as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal(pub String);

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refusal {}

/// Why [`State::lock`] did not return a lock.
#[derive(Debug)]
pub enum LockError {
    /// Another live review holds the PR (`None`: the lock is still starting).
    Held { pid: Option<u32> },
    /// The lock could not be taken for another reason.
    Failed(anyhow::Error),
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LockError::Held { pid: Some(pid) } => {
                write!(f, "another review holds the lock (pid {pid})")
            }
            LockError::Held { pid: None } => f.write_str("another review is taking the lock"),
            LockError::Failed(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for LockError {}

/// How a finding ended up after verify.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Disposition {
    Confirmed,
    DismissedByVerify,
    PreExisting,
}

/// One line of `findings.jsonl`: a verified finding of a review round.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingRecord {
    pub pr: u64,
    /// The PR head sha the round reviewed.
    pub head: String,
    pub lens: String,
    pub severity: Severity,
    pub category: String,
    pub file: String,
    pub line: u32,
    pub claim: String,
    /// The `version:` of the prompt that produced it.
    pub prompt_version: u32,
    pub disposition: Disposition,
}

/// The loop's state directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    root: PathBuf,
}

impl State {
    /// The state of the repository whose main checkout is `main`.
    pub fn for_repo(main: &Path) -> Result<State> {
        state_dir(&common_dir(main)?, override_dir().as_deref()).map(|root| State { root })
    }

    /// A state rooted at `root`, as given.
    pub fn at(root: impl Into<PathBuf>) -> State {
        State { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn stop_file(&self) -> PathBuf {
        self.root.join("STOP")
    }

    pub fn findings_file(&self) -> PathBuf {
        self.root.join("findings.jsonl")
    }

    pub fn pr_dir(&self, n: u64) -> PathBuf {
        self.root.join(format!("pr-{n}"))
    }

    pub fn stage_settings_file(&self, n: u64) -> PathBuf {
        self.pr_dir(n).join("stage-settings.json")
    }

    /// Create `pr-<n>/run-<epoch>` (the next free second if that name is taken)
    /// and return it.
    pub fn new_run_dir(&self, n: u64) -> Result<PathBuf> {
        let pr_dir = self.pr_dir(n);
        private_dir(&pr_dir)?;
        let mut epoch = epoch_secs();
        loop {
            let dir = pr_dir.join(format!("run-{epoch}"));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok(dir),
                Err(e) if e.kind() == ErrorKind::AlreadyExists => epoch += 1,
                Err(e) => {
                    return Err(e).with_context(|| format!("cannot create {}", dir.display()))
                }
            }
        }
    }

    /// Take the lock of PR `n`: `pr-<n>/lock/` made by `mkdir`, holding a
    /// `pid` file. A live pid refuses; a dead pid is taken over.
    pub fn lock(&self, n: u64) -> Result<PrLock, LockError> {
        let pr_dir = self.pr_dir(n);
        private_dir(&pr_dir).map_err(LockError::Failed)?;
        let lock = pr_dir.join("lock");
        let me = std::process::id();
        for _ in 0..3 {
            match std::fs::create_dir(&lock) {
                Ok(()) => {
                    let wrote = std::fs::write(lock.join("pid"), format!("{me}\n"));
                    if let Err(e) = wrote {
                        let _ = std::fs::remove_dir(&lock);
                        return Err(LockError::Failed(
                            anyhow!(e).context(format!("cannot write {}/pid", lock.display())),
                        ));
                    }
                    return Ok(PrLock { dir: lock, pid: me });
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                Err(e) => {
                    return Err(LockError::Failed(
                        anyhow!(e).context(format!("cannot create {}", lock.display())),
                    ))
                }
            }
            let seen = read_pid(&lock);
            match seen {
                Some(pid) if is_alive(pid).map_err(LockError::Failed)? => {
                    return Err(LockError::Held { pid: Some(pid) });
                }
                None if age(&lock) < LOCK_STARTING => return Err(LockError::Held { pid: None }),
                _ => take_over(&pr_dir, &lock, seen, me).map_err(LockError::Failed)?,
            }
        }
        Err(LockError::Failed(anyhow!(
            "cannot take {}: it keeps changing hands",
            lock.display()
        )))
    }

    /// Append one record to `findings.jsonl`.
    pub fn append_finding(&self, record: &FindingRecord) -> Result<()> {
        private_dir(&self.root)?;
        append_jsonl(&self.findings_file(), record)
    }

    /// The records of PR `n`, oldest first. A line that does not parse (a torn
    /// write) is skipped: the records only feed the prompts' prior findings.
    pub fn prior_for_pr(&self, n: u64) -> Result<Vec<FindingRecord>> {
        let path = self.findings_file();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        };
        Ok(text
            .lines()
            .filter_map(|line| serde_json::from_str::<FindingRecord>(line).ok())
            .filter(|record| record.pr == n)
            .collect())
    }

    /// The kill switch. Fails closed: the run is stopped when
    /// - the `STOP` file exists (checked first, before any `gh` call);
    /// - the `agent-stop` label is missing, or looking it up fails;
    /// - an open `agent-stop` issue exists, or looking for one fails.
    ///
    /// Run before the first stage and again before verify.
    pub fn kill_switch(&self, gh: &dyn GhRunner) -> Result<(), StopReason> {
        let stop = self.stop_file();
        if stop.exists() {
            return Err(StopReason(format!(
                "stopped: the STOP file exists ({})",
                stop.display()
            )));
        }
        match label_exists(gh, "agent-stop") {
            Ok(true) => {}
            Ok(false) => {
                return Err(StopReason(
                    "stopped: the agent-stop label does not exist; the kill switch fails closed"
                        .to_string(),
                ))
            }
            Err(e) => {
                return Err(StopReason(format!(
                    "stopped: cannot look up the agent-stop label ({e:#}); the kill switch fails closed"
                )))
            }
        }
        match issues_labelled(gh, "agent-stop") {
            Ok(open) if open.is_empty() => Ok(()),
            Ok(open) => {
                let list: Vec<String> = open.iter().map(|n| format!("#{n}")).collect();
                Err(StopReason(format!(
                    "stopped: open agent-stop issue(s) {}",
                    list.join(", ")
                )))
            }
            Err(e) => Err(StopReason(format!(
                "stopped: cannot list agent-stop issues ({e:#}); the kill switch fails closed"
            ))),
        }
    }
}

/// A held PR lock; dropping it releases the lock.
#[derive(Debug)]
pub struct PrLock {
    dir: PathBuf,
    pid: u32,
}

impl Drop for PrLock {
    fn drop(&mut self) {
        // Remove the lock only while it still names this process.
        if read_pid(&self.dir) == Some(self.pid) {
            let _ = std::fs::remove_file(self.dir.join("pid"));
            let _ = std::fs::remove_dir(&self.dir);
        }
    }
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `path` and its missing parents, mode 0700.
fn private_dir(path: &Path) -> Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .with_context(|| format!("cannot create {}", path.display()))
}

/// [`STATE_DIR_VAR`], in a debug-assertion build where it is set and not empty.
fn override_dir() -> Option<PathBuf> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var_os(STATE_DIR_VAR)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The state directory for a repository with git directory `common`, or the
/// override (which must be absolute).
fn state_dir(common: &Path, over: Option<&Path>) -> Result<PathBuf> {
    match over {
        Some(dir) if dir.is_absolute() => Ok(dir.to_path_buf()),
        Some(dir) => bail!("{STATE_DIR_VAR} must be absolute, got {}", dir.display()),
        None => Ok(common.join("aios-agent").join("loop")),
    }
}

/// `git rev-parse --path-format=absolute --git-common-dir`, run in `main`.
pub fn common_dir(main: &Path) -> Result<PathBuf> {
    let mut cmd = git_in(main);
    cmd.args(["rev-parse", "--path-format=absolute", "--git-common-dir"]);
    let out = cmd
        .output()
        .with_context(|| format!("cannot run git in {}", main.display()))?;
    if !out.status.success() {
        bail!(
            "git rev-parse --git-common-dir failed in {}: {}",
            main.display(),
            String::from_utf8_lossy(&out.stderr).trim_end()
        );
    }
    let text = String::from_utf8(out.stdout).context("the git common directory is not UTF-8")?;
    let text = text.trim_end_matches(['\n', '\r']);
    if text.is_empty() {
        bail!("git printed no common directory in {}", main.display());
    }
    Ok(PathBuf::from(text))
}

/// The pid in `lock/pid`, if there is a readable one.
fn read_pid(lock: &Path) -> Option<u32> {
    std::fs::read_to_string(lock.join("pid"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// How long ago `path` was modified (zero when that cannot be told, so an
/// unreadable lock counts as starting and refuses).
fn age(path: &Path) -> Duration {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .unwrap_or_default()
}

/// Whether process `pid` exists: `kill -0`, through the `kill` utility as
/// `proc.rs` does (this crate forbids `unsafe`). EPERM counts as alive.
fn is_alive(pid: u32) -> Result<bool> {
    let out = Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdin(Stdio::null())
        .output()
        .context("cannot run kill -0")?;
    Ok(out.status.success()
        || String::from_utf8_lossy(&out.stderr)
            .to_lowercase()
            .contains("not permitted"))
}

/// Remove a stale lock: move it to a private name (atomic), check that what
/// moved is the lock that was judged stale, then delete it.
fn take_over(pr_dir: &Path, lock: &Path, seen: Option<u32>, me: u32) -> Result<()> {
    let moved = pr_dir.join(format!("lock.stale.{me}"));
    let _ = std::fs::remove_dir_all(&moved);
    match std::fs::rename(lock, &moved) {
        Ok(()) => {}
        // Someone else removed it first: the next mkdir decides.
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("cannot move {}", lock.display())),
    }
    if read_pid(&moved) != seen {
        bail!(
            "the lock {} changed hands while it was taken over; it is left at {}",
            lock.display(),
            moved.display()
        );
    }
    std::fs::remove_dir_all(&moved).with_context(|| format!("cannot remove {}", moved.display()))
}

/// Only the `aios` that `just tools` installed in the main checkout may run the
/// loop, unless `--tree` is given: a PR cannot be the code that reviews it.
///
/// `exe` is the running executable. With `tree_mode` any executable runs. The
/// debug-only escape ([`AnyExe`]) additionally requires the fake root.
pub fn check_location(
    exe: &Path,
    main: &Path,
    tree_mode: bool,
    any_exe: Option<&AnyExe>,
) -> Result<()> {
    if tree_mode {
        return Ok(());
    }
    let expected = main.join("target/tools/installed/aios");
    let exe = exe
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", exe.display()))?;
    if expected.canonicalize().is_ok_and(|want| want == exe) {
        return Ok(());
    }
    if let Some(any) = any_exe {
        return any.check();
    }
    bail!(
        "aios loop runs only from {} (run: just tools), or with --tree DIR; this is {}",
        expected.display(),
        exe.display()
    )
}

/// [`check_location`] for the running executable.
pub fn self_location(main: &Path, tree_mode: bool) -> Result<()> {
    let exe = std::env::current_exe().context("cannot find the running executable")?;
    check_location(&exe, main, tree_mode, AnyExe::from_env().as_ref())
}

/// The debug-only escape from the location check: [`ANY_EXE_VAR`] `=1`. It
/// holds only with [`STATE_DIR_VAR`] `=<root>/state`, where the first `gh` and
/// the first `claude` on PATH are `<root>/bin/gh` and `<root>/bin/claude`, so a
/// test that runs the loop from the test binary can never reach a real `gh` or
/// `claude`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnyExe {
    pub state_dir: Option<PathBuf>,
    pub path: Option<OsString>,
}

impl AnyExe {
    /// `Some` in a debug-assertion build where [`ANY_EXE_VAR`] is `1`.
    pub fn from_env() -> Option<AnyExe> {
        if !cfg!(debug_assertions) || std::env::var_os(ANY_EXE_VAR).is_none_or(|v| v != "1") {
            return None;
        }
        Some(AnyExe {
            state_dir: std::env::var_os(STATE_DIR_VAR).map(PathBuf::from),
            path: std::env::var_os("PATH"),
        })
    }

    fn check(&self) -> Result<()> {
        let state = self
            .state_dir
            .as_deref()
            .ok_or_else(|| anyhow!("{ANY_EXE_VAR} needs {STATE_DIR_VAR}=<root>/state"))?;
        let root = state
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .ok_or_else(|| anyhow!("{STATE_DIR_VAR} has no existing parent directory"))?;
        let path = self.path.clone().unwrap_or_default();
        for tool in ["gh", "claude"] {
            let found = first_on_path(&path, tool)
                .and_then(|p| p.canonicalize().ok())
                .ok_or_else(|| anyhow!("{ANY_EXE_VAR}: no {tool} on PATH"))?;
            let want = root.join("bin").join(tool);
            if found != want.canonicalize().unwrap_or(want) {
                bail!(
                    "{ANY_EXE_VAR}: the first {tool} on PATH is {}, not the fake root's",
                    found.display()
                );
            }
        }
        Ok(())
    }
}

/// The first directory entry of `path` that holds an executable file `name`.
fn first_on_path(path: &OsString, name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path)
        .map(|dir| dir.join(name))
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// Teams that own PRs, by their PR label.
const TEAM_LABELS: [&str; 2] = ["team-build", "team-fix"];

/// Refuse a PR that carries the other team's label. `team` is `AIOS_TEAM`; a
/// session without it (or a solo one) reviews any PR.
pub fn team_owner_check(pr: &PrView, n: u64, team: Option<&str>) -> Result<(), Refusal> {
    let Some(team) = team.filter(|t| TEAM_LABELS.contains(t)) else {
        return Ok(());
    };
    match TEAM_LABELS
        .iter()
        .find(|label| **label != team && pr.has_label(label))
    {
        Some(owner) => Err(Refusal(format!(
            "pr #{n}: owned by {owner}; run it from that team's lead"
        ))),
        None => Ok(()),
    }
}

/// `Some(reason)` for a PR whose head is in a fork: the verdict is needs-human,
/// before anything is fetched.
pub fn fork_check(pr: &PrView) -> Option<&'static str> {
    pr.facts.is_cross_repository.then_some("fork PR")
}

/// Stop while the QEMU lock holds a quiet window (`mode=quiet` in
/// `qemu-lock.sh status`): a review spawns processes and would disturb a rate
/// measurement. This reads host state; it is not a safety gate, so a missing or
/// failing script only prints a warning and the run goes on.
pub fn quiet_check(main: &Path) -> Result<(), StopReason> {
    let script = main.join("scripts/agent/qemu-lock.sh");
    let out = Command::new("sh")
        .arg(&script)
        .arg("status")
        .current_dir(main)
        .stdin(Stdio::null())
        .output();
    let out = match out {
        Ok(out) if out.status.success() => out,
        Ok(out) => {
            eprintln!(
                "aios loop: warning: {} status failed ({}); not checking for a quiet window",
                script.display(),
                out.status
            );
            return Ok(());
        }
        Err(e) => {
            eprintln!(
                "aios loop: warning: cannot run {} ({e}); not checking for a quiet window",
                script.display()
            );
            return Ok(());
        }
    };
    if String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|line| line.trim() == "mode=quiet")
    {
        return Err(StopReason(
            "stopped: the QEMU lock holds a quiet window (qemu-lock.sh status shows mode=quiet)"
                .to_string(),
        ));
    }
    Ok(())
}
