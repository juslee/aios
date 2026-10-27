//! The loop's state and preflight (R2a, Task 5a): the kill switch, the
//! location check, the team and fork checks, the QEMU quiet window, the PR
//! lock and `findings.jsonl`.

mod common;

use aios_tools::cmd::pr_loop::github::{PrFacts, PrView};
use aios_tools::cmd::pr_loop::schema::Severity;
use aios_tools::cmd::pr_loop::state::{
    check_location, fork_check, quiet_check, team_owner_check, AnyExe, Disposition, FindingRecord,
    LockError, State,
};
use aios_tools::gh::{GhOutcome, GhRunner};
use common::{unique_dir, TestRepo};
use serde_json::json;
use std::cell::RefCell;
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// A runner that answers from a closure and counts its calls.
struct Fake<F: Fn(&[&str]) -> GhOutcome> {
    answer: F,
    calls: RefCell<Vec<String>>,
}

fn fake<F: Fn(&[&str]) -> GhOutcome>(answer: F) -> Fake<F> {
    Fake {
        answer,
        calls: RefCell::new(Vec::new()),
    }
}

impl<F: Fn(&[&str]) -> GhOutcome> GhRunner for Fake<F> {
    fn run(&self, args: &[&str]) -> GhOutcome {
        self.calls.borrow_mut().push(args.join(" "));
        (self.answer)(args)
    }
}

fn failed(stderr: &str) -> GhOutcome {
    GhOutcome::Failed {
        status: Some(1),
        stderr: stderr.as_bytes().to_vec(),
    }
}

fn is_label(args: &[&str]) -> bool {
    args.first() == Some(&"api")
        && args
            .get(1)
            .is_some_and(|a| a.ends_with("/labels/agent-stop"))
}

/// Label present, no open agent-stop issue.
fn all_clear(args: &[&str]) -> GhOutcome {
    if is_label(args) {
        GhOutcome::Ok(b"{\"name\":\"agent-stop\"}".to_vec())
    } else {
        GhOutcome::Ok(b"[]".to_vec())
    }
}

fn state() -> State {
    State::at(unique_dir("loop-state").join("state"))
}

#[test]
fn state_lives_under_the_git_common_dir() {
    if std::env::var_os("AIOS_LOOP_STATE_DIR").is_some() {
        return; // the developer's override would win in a debug build
    }
    let repo = TestRepo::with_files("loop-state-repo", &[("a", "x\n")]);
    let state = State::for_repo(repo.path()).expect("the state of a repository");
    let common = repo
        .path()
        .join(".git")
        .canonicalize()
        .expect("the common dir");
    assert_eq!(state.root(), common.join("aios-agent/loop"));
}

#[test]
fn clear_kill_switch_passes() {
    let gh = fake(all_clear);
    assert_eq!(state().kill_switch(&gh), Ok(()));
    assert_eq!(gh.calls.borrow().len(), 2);
}

#[test]
fn missing_agent_stop_label_stops() {
    let gh = fake(|args| {
        if is_label(args) {
            failed("gh: Not Found (HTTP 404)")
        } else {
            GhOutcome::Ok(b"[]".to_vec())
        }
    });
    let stop = state().kill_switch(&gh).expect_err("a missing label stops");
    assert!(stop.0.contains("agent-stop label"), "{stop}");
    assert!(stop.0.contains("the kill switch fails closed"), "{stop}");
}

#[test]
fn gh_error_on_kill_switch_stops() {
    // The label lookup fails.
    let gh = fake(|args| {
        if is_label(args) {
            failed("gh: HTTP 502")
        } else {
            GhOutcome::Ok(b"[]".to_vec())
        }
    });
    let stop = state().kill_switch(&gh).expect_err("an error stops");
    assert!(stop.0.contains("the kill switch fails closed"), "{stop}");
    // The issue query fails.
    let gh = fake(|args| {
        if is_label(args) {
            GhOutcome::Ok(b"{}".to_vec())
        } else {
            failed("gh: HTTP 500")
        }
    });
    let stop = state().kill_switch(&gh).expect_err("an error stops");
    assert!(stop.0.contains("the kill switch fails closed"), "{stop}");
    // gh is not installed.
    let gh = fake(|_| GhOutcome::Missing);
    assert!(state().kill_switch(&gh).is_err());
}

#[test]
fn open_agent_stop_issue_stops() {
    let gh = fake(|args| {
        if is_label(args) {
            GhOutcome::Ok(b"{}".to_vec())
        } else {
            GhOutcome::Ok(b"[{\"number\":12}]".to_vec())
        }
    });
    let stop = state().kill_switch(&gh).expect_err("an open issue stops");
    assert!(stop.0.contains("#12"), "{stop}");
}

#[test]
fn stop_file_stops_before_any_gh_call() {
    let state = state();
    std::fs::create_dir_all(state.root()).expect("create the state dir");
    std::fs::write(state.stop_file(), "").expect("write STOP");
    let gh = fake(all_clear);
    let stop = state.kill_switch(&gh).expect_err("STOP stops");
    assert!(stop.0.contains("STOP"), "{stop}");
    assert_eq!(
        gh.calls.borrow().len(),
        0,
        "no gh call before the STOP file"
    );
}

// ---------------------------------------------------------------------------
// Self-location
// ---------------------------------------------------------------------------

fn write_exe(path: &Path) {
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("create the directory");
    std::fs::write(path, "#!/bin/sh\n").expect("write an executable");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

#[test]
fn release_exe_elsewhere_refuses_without_tree() {
    let main = unique_dir("loop-loc-main");
    let elsewhere = unique_dir("loop-loc-else").join("aios");
    write_exe(&elsewhere);
    let err = check_location(&elsewhere, &main, false, None).expect_err("elsewhere refuses");
    assert!(
        format!("{err:#}").contains("target/tools/installed/aios"),
        "{err:#}"
    );
    // The installed binary is fine.
    let installed = main.join("target/tools/installed/aios");
    write_exe(&installed);
    check_location(&installed, &main, false, None).expect("the installed binary runs");
}

#[test]
fn tree_mode_runs_from_any_exe() {
    let main = unique_dir("loop-loc-tree");
    let elsewhere = unique_dir("loop-loc-tree-else").join("aios");
    write_exe(&elsewhere);
    check_location(&elsewhere, &main, true, None).expect("--tree runs from anywhere");
}

/// `<root>/bin/{gh,claude}` and `<root>/state`'s parent.
fn fake_root() -> PathBuf {
    let root = unique_dir("loop-loc-root");
    for tool in ["gh", "claude"] {
        write_exe(&root.join("bin").join(tool));
    }
    root
}

fn path_of(dirs: &[&Path]) -> OsString {
    std::env::join_paths(dirs).expect("join PATH")
}

#[test]
fn any_exe_escape_needs_the_fake_root() {
    let main = unique_dir("loop-loc-any");
    let exe = unique_dir("loop-loc-any-exe").join("aios");
    write_exe(&exe);
    let root = fake_root();
    let state_dir = root.join("state");
    let good = AnyExe {
        state_dir: Some(state_dir.clone()),
        path: Some(path_of(&[&root.join("bin"), Path::new("/usr/bin")])),
    };
    check_location(&exe, &main, false, Some(&good)).expect("the fake root passes");

    // No state dir.
    let no_state = AnyExe {
        state_dir: None,
        path: good.path.clone(),
    };
    assert!(check_location(&exe, &main, false, Some(&no_state)).is_err());

    // Another gh and claude come first on PATH.
    let other = fake_root();
    let wrong = AnyExe {
        state_dir: Some(state_dir.clone()),
        path: Some(path_of(&[&other.join("bin"), &root.join("bin")])),
    };
    let err = check_location(&exe, &main, false, Some(&wrong)).expect_err("a wrong first gh");
    assert!(
        format!("{err:#}").contains("not the fake root's"),
        "{err:#}"
    );

    // No claude on PATH at all.
    let gh_only = unique_dir("loop-loc-gh-only");
    write_exe(&gh_only.join("bin/gh"));
    let partial = AnyExe {
        state_dir: Some(gh_only.join("state")),
        path: Some(path_of(&[&gh_only.join("bin")])),
    };
    assert!(check_location(&exe, &main, false, Some(&partial)).is_err());

    // Without the escape the same exe refuses.
    assert!(check_location(&exe, &main, false, None).is_err());
}

// ---------------------------------------------------------------------------
// Team owner and fork
// ---------------------------------------------------------------------------

fn pr_with(labels: &[&str], cross: bool) -> PrView {
    let raw = json!({"number": 7});
    let facts: PrFacts = serde_json::from_value(json!({
        "labels": labels.iter().map(|l| json!({"name": l})).collect::<Vec<_>>(),
        "isCrossRepository": cross,
    }))
    .expect("PrFacts");
    PrView { raw, facts }
}

#[test]
fn other_teams_pr_is_refused() {
    let pr = pr_with(&["team-fix"], false);
    let refusal = team_owner_check(&pr, 7, Some("team-build")).expect_err("refused");
    assert_eq!(
        refusal.0,
        "pr #7: owned by team-fix; run it from that team's lead"
    );
    let pr = pr_with(&["team-build"], false);
    let refusal = team_owner_check(&pr, 7, Some("team-fix")).expect_err("refused");
    assert!(refusal.0.contains("owned by team-build"), "{refusal}");
    // Its own team's PR, an unlabelled PR, and a session without a team pass.
    assert!(team_owner_check(&pr_with(&["team-build"], false), 7, Some("team-build")).is_ok());
    assert!(team_owner_check(&pr_with(&[], false), 7, Some("team-build")).is_ok());
    assert!(team_owner_check(&pr_with(&["team-fix"], false), 7, None).is_ok());
    assert!(team_owner_check(&pr_with(&["team-fix"], false), 7, Some("some-solo")).is_ok());
}

#[test]
fn fork_pr_is_needs_human() {
    assert_eq!(fork_check(&pr_with(&[], true)), Some("fork PR"));
    assert_eq!(fork_check(&pr_with(&[], false)), None);
}

// ---------------------------------------------------------------------------
// The QEMU quiet window
// ---------------------------------------------------------------------------

fn main_with_lock_script(body: Option<&str>) -> PathBuf {
    let main = unique_dir("loop-quiet");
    if let Some(body) = body {
        let script = main.join("scripts/agent/qemu-lock.sh");
        std::fs::create_dir_all(script.parent().expect("a parent")).expect("create scripts/agent");
        std::fs::write(script, body).expect("write the script");
    }
    main
}

#[test]
fn quiet_window_stops() {
    let main = main_with_lock_script(Some(
        "echo team=team-fix\necho mode=quiet\necho state=live\n",
    ));
    let stop = quiet_check(&main).expect_err("a quiet window stops");
    assert!(stop.0.contains("quiet window"), "{stop}");
}

#[test]
fn free_or_boot_lock_does_not_stop() {
    let free = main_with_lock_script(Some("echo free\n"));
    assert_eq!(quiet_check(&free), Ok(()));
    let boot = main_with_lock_script(Some("echo mode=boot\n"));
    assert_eq!(quiet_check(&boot), Ok(()));
}

#[test]
fn missing_or_failing_lock_script_does_not_stop() {
    assert_eq!(quiet_check(&main_with_lock_script(None)), Ok(()));
    let failing = main_with_lock_script(Some("echo mode=quiet\nexit 3\n"));
    assert_eq!(quiet_check(&failing), Ok(()));
}

// ---------------------------------------------------------------------------
// The PR lock
// ---------------------------------------------------------------------------

/// The pid of a process that has exited.
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("sh")
        .args(["-c", "exit 0"])
        .spawn()
        .expect("spawn sh");
    let pid = child.id();
    child.wait().expect("wait");
    pid
}

fn plant_lock(state: &State, n: u64, pid: &str) -> PathBuf {
    let lock = state.pr_dir(n).join("lock");
    std::fs::create_dir_all(&lock).expect("create the lock");
    std::fs::write(lock.join("pid"), format!("{pid}\n")).expect("write the pid");
    lock
}

#[test]
fn dead_pid_lock_is_taken_over() {
    let state = state();
    let lock = plant_lock(&state, 7, &dead_pid().to_string());
    let held = state.lock(7).expect("a dead holder is taken over");
    let pid = std::fs::read_to_string(lock.join("pid")).expect("read the pid");
    assert_eq!(pid.trim(), std::process::id().to_string());
    drop(held);
    assert!(!lock.exists(), "dropping the lock releases it");
}

#[test]
fn live_pid_lock_refuses() {
    let state = state();
    let lock = plant_lock(&state, 7, &std::process::id().to_string());
    match state.lock(7) {
        Err(LockError::Held { pid }) => assert_eq!(pid, Some(std::process::id())),
        other => panic!("expected Held, got {other:?}"),
    }
    assert!(lock.exists(), "a refused lock is left alone");
}

#[test]
fn lock_is_exclusive_and_per_pr() {
    let state = state();
    let first = state.lock(7).expect("first lock");
    assert!(matches!(state.lock(7), Err(LockError::Held { .. })));
    let other = state.lock(8).expect("another PR is independent");
    drop(first);
    drop(other);
    state.lock(7).expect("free after release");
}

#[test]
fn fresh_lock_without_a_pid_is_still_starting() {
    let state = state();
    std::fs::create_dir_all(state.pr_dir(7).join("lock")).expect("create the lock");
    assert!(matches!(state.lock(7), Err(LockError::Held { pid: None })));
}

#[test]
fn run_dirs_are_distinct() {
    let state = state();
    let a = state.new_run_dir(7).expect("first run dir");
    let b = state.new_run_dir(7).expect("second run dir");
    assert_ne!(a, b);
    assert!(a.starts_with(state.pr_dir(7)) && b.starts_with(state.pr_dir(7)));
}

// ---------------------------------------------------------------------------
// findings.jsonl
// ---------------------------------------------------------------------------

fn record(pr: u64, claim: &str, disposition: Disposition) -> FindingRecord {
    FindingRecord {
        pr,
        head: "abc1234".to_string(),
        lens: "correctness".to_string(),
        severity: Severity::MustFix,
        category: "lock-order".to_string(),
        file: "kernel/src/ipc/channel.rs".to_string(),
        line: 42,
        claim: claim.to_string(),
        prompt_version: 1,
        disposition,
    }
}

#[test]
fn findings_jsonl_round_trip() {
    let state = state();
    assert_eq!(state.prior_for_pr(7).expect("no file yet"), []);
    let a = record(7, "takes B then A", Disposition::Confirmed);
    let b = record(8, "another PR", Disposition::PreExisting);
    let c = record(7, "not a bug", Disposition::DismissedByVerify);
    for r in [&a, &b, &c] {
        state.append_finding(r).expect("append");
    }
    // A torn line is skipped.
    let path = state.findings_file();
    let mut text = std::fs::read_to_string(&path).expect("read findings.jsonl");
    text.push_str("{\"pr\":7,\"head\n");
    std::fs::write(&path, text).expect("write findings.jsonl");
    assert_eq!(state.prior_for_pr(7).expect("read back"), vec![a, c]);
    assert_eq!(
        state.prior_for_pr(8).expect("read back"),
        vec![record(8, "another PR", Disposition::PreExisting)]
    );
    let line = std::fs::read_to_string(&path).expect("read");
    assert!(
        line.contains("\"disposition\":\"dismissed-by-verify\""),
        "{line}"
    );
    assert!(line.contains("\"severity\":\"must-fix\""), "{line}");
}
