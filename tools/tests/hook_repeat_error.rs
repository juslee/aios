//! `aios hook repeat-error`: the second identical Bash failure gets a nudge, a
//! success clears the command, and nothing else produces output.

mod hook_support;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use hook_support::{run_hook, unique_dir, Run};
use serde_json::{json, Value};

/// A throwaway state directory and working directory for one test.
struct Env {
    state: PathBuf,
    cwd: PathBuf,
}

impl Env {
    fn new(label: &str) -> Env {
        Env {
            state: unique_dir(&format!("{label}-state")),
            cwd: unique_dir(&format!("{label}-cwd")),
        }
    }

    fn run(&self, payload: &Value) -> Run {
        let state = self.state.to_str().expect("the state path is UTF-8");
        let run = run_hook(
            &["repeat-error"],
            payload.to_string().as_bytes(),
            &[("AIOS_HOOK_STATE_DIR", state)],
            &self.cwd,
        );
        assert_eq!(run.code, Some(0), "{}", run.stderr);
        run
    }

    fn state_files(&self) -> Vec<PathBuf> {
        let dir = self.state.join("repeat-error");
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|entries| entries.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        files.sort();
        files
    }
}

fn failure(session: &str, command: &str, error: &str) -> Value {
    json!({
        "session_id": session,
        "hook_event_name": "PostToolUseFailure",
        "tool_name": "Bash",
        "tool_use_id": "toolu_01",
        "tool_input": { "command": command },
        "error": error,
    })
}

fn success(session: &str, command: &str) -> Value {
    json!({
        "session_id": session,
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": command },
        "tool_response": { "stdout": "ok", "stderr": "", "interrupted": false, "isImage": false },
    })
}

fn in_subagent(mut payload: Value, agent: &str) -> Value {
    payload["agent_id"] = json!(agent);
    payload["agent_type"] = json!("worker");
    payload
}

/// The `additionalContext` of a run, checking the output shape on the way.
fn context(run: &Run) -> String {
    let v: Value = serde_json::from_str(&run.stdout).expect("stdout is one JSON object");
    assert_eq!(
        v["hookSpecificOutput"]["hookEventName"],
        "PostToolUseFailure"
    );
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("a context string")
        .to_string()
}

#[test]
fn the_first_failure_is_silent_and_the_second_identical_one_nudges() {
    let env = Env::new("second");
    let first = env.run(&failure(
        "s1",
        "cargo test",
        "Exit code 1\nassertion failed",
    ));
    assert_eq!(first.stdout, "");
    assert_eq!(first.stderr, "");

    let second = env.run(&failure(
        "s1",
        "cargo test",
        "Exit code 1\nassertion failed",
    ));
    let text = context(&second);
    assert!(text.contains("`cargo test`"), "{text}");
    assert!(text.contains("2 times"), "{text}");

    let third = env.run(&failure(
        "s1",
        "cargo test",
        "Exit code 1\nassertion failed",
    ));
    assert!(context(&third).contains("3 times"));
}

#[test]
fn digits_hex_and_timestamps_do_not_make_failures_differ() {
    let env = Env::new("masked");
    let err = |pid: &str, addr: &str, ts: &str| {
        format!("Exit code 101\nthread panicked at {addr} (pid {pid}) at {ts}, exit 101")
    };
    env.run(&failure(
        "s1",
        "just  test",
        &err("12345", "0xdeadbeef", "1700000000"),
    ));
    let second = env.run(&failure(
        "s1",
        "just test",
        &err("67890", "0x7ffee4b1c000", "1700000042"),
    ));
    assert!(context(&second).contains("2 times"));
}

#[test]
fn a_different_error_or_command_does_not_match() {
    let env = Env::new("different");
    env.run(&failure(
        "s1",
        "cargo build",
        "Exit code 1\ncannot find value x",
    ));
    let other_error = env.run(&failure(
        "s1",
        "cargo build",
        "Exit code 1\nunresolved import y",
    ));
    assert_eq!(
        other_error.stdout, "",
        "a different error is a new signature"
    );
    let other_exit = env.run(&failure(
        "s1",
        "cargo build",
        "Exit code 2\ncannot find value x",
    ));
    assert_eq!(other_exit.stdout, "", "a short number is not masked");
    let other_command = env.run(&failure(
        "s1",
        "cargo check",
        "Exit code 1\ncannot find value x",
    ));
    assert_eq!(other_command.stdout, "");
}

#[test]
fn a_success_resets_the_commands_signatures() {
    let env = Env::new("reset");
    env.run(&failure("s1", "cargo test", "Exit code 1\nboom"));
    let ok = env.run(&success("s1", "cargo   test"));
    assert_eq!(ok.stdout, "");
    let again = env.run(&failure("s1", "cargo test", "Exit code 1\nboom"));
    assert_eq!(again.stdout, "", "the count started over");

    // Another command's failures survive a success.
    env.run(&failure("s1", "just check", "Exit code 1\nfmt"));
    env.run(&success("s1", "cargo test"));
    let kept = env.run(&failure("s1", "just check", "Exit code 1\nfmt"));
    assert!(context(&kept).contains("2 times"));
}

#[test]
fn a_success_with_no_failure_on_record_writes_nothing() {
    let env = Env::new("quiet-success");
    let ok = env.run(&success("s1", "ls"));
    assert_eq!(ok.stdout, "");
    assert!(env.state_files().is_empty());
}

#[test]
fn the_wording_differs_in_a_subagent_and_the_main_session() {
    let env = Env::new("wording");
    let payload = failure("s1", "cargo test", "Exit code 1\nboom");
    env.run(&payload);
    let main = context(&env.run(&payload));
    assert!(main.contains("code-reviewer"), "{main}");
    assert!(!main.contains("report back"), "{main}");

    let sub = in_subagent(payload, "agent-7");
    env.run(&sub);
    let nudged = context(&env.run(&sub));
    assert!(nudged.contains("report back to its caller"), "{nudged}");
    assert!(!nudged.contains("code-reviewer"), "{nudged}");

    // The main session and the subagent keep separate counters.
    let names: Vec<String> = env
        .state_files()
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["s1-agent-7.json", "s1.json"]);
}

#[test]
fn state_is_kept_per_session() {
    let env = Env::new("sessions");
    env.run(&failure("s1", "cargo test", "Exit code 1\nboom"));
    let other = env.run(&failure("s2", "cargo test", "Exit code 1\nboom"));
    assert_eq!(other.stdout, "");
}

#[test]
fn the_state_file_keeps_at_most_64_signatures() {
    let env = Env::new("evict");
    for n in 0..65 {
        let run = env.run(&failure("s1", &format!("cmd{n}"), "Exit code 1\nboom"));
        assert_eq!(run.stdout, "");
    }
    let files = env.state_files();
    assert_eq!(files.len(), 1);
    let state: Value = serde_json::from_slice(&std::fs::read(&files[0]).unwrap()).unwrap();
    assert_eq!(state["entries"].as_array().unwrap().len(), 64);

    // cmd0 was the oldest and is gone, so its repeat is a first failure again.
    let evicted = env.run(&failure("s1", "cmd0", "Exit code 1\nboom"));
    assert_eq!(evicted.stdout, "");
    // cmd64 is still on record.
    let kept = env.run(&failure("s1", "cmd64", "Exit code 1\nboom"));
    assert!(context(&kept).contains("2 times"));
}

#[test]
fn a_corrupt_state_file_fails_open_and_recovers() {
    let env = Env::new("corrupt");
    env.run(&failure("s1", "cargo test", "Exit code 1\nboom"));
    let file = env.state_files().remove(0);
    std::fs::write(&file, b"{not json").unwrap();

    let after = env.run(&failure("s1", "cargo test", "Exit code 1\nboom"));
    assert_eq!(after.stdout, "", "the count restarts instead of failing");
    assert_eq!(after.stderr, "");
    let again = env.run(&failure("s1", "cargo test", "Exit code 1\nboom"));
    assert!(context(&again).contains("2 times"), "the file recovered");
}

#[test]
fn an_unusable_state_directory_fails_open() {
    let env = Env::new("unwritable");
    // A file where the state directory should be: nothing can be written.
    let blocker = env.state.join("repeat-error");
    std::fs::write(&blocker, b"in the way").unwrap();
    let run = env.run(&failure("s1", "cargo test", "Exit code 1\nboom"));
    assert_eq!(run.stdout, "");
    assert!(run.stderr.contains("repeat-error"), "{}", run.stderr);
}

#[test]
fn other_tools_events_and_interrupts_produce_no_output_and_no_state() {
    let env = Env::new("ignored");
    let mut read = failure("s1", "cat x", "Exit code 1\nboom");
    read["tool_name"] = json!("Read");
    let mut pre = failure("s1", "cargo test", "Exit code 1\nboom");
    pre["hook_event_name"] = json!("PreToolUse");
    let mut interrupted = failure("s1", "cargo test", "Exit code 130\nInterrupted");
    interrupted["is_interrupt"] = json!(true);
    let mut no_command = failure("s1", "x", "boom");
    no_command["tool_input"] = json!({});
    for payload in [read, pre, interrupted, no_command] {
        for _ in 0..3 {
            let run = env.run(&payload);
            assert_eq!(run.stdout, "", "{payload}");
        }
    }
    assert!(env.state_files().is_empty());
}

#[test]
fn a_session_id_cannot_name_a_path_outside_the_state_directory() {
    let env = Env::new("hostile");
    let payload = failure("../../escape", "cargo test", "Exit code 1\nboom");
    env.run(&payload);
    env.run(&payload);
    let files = env.state_files();
    assert_eq!(files.len(), 1);
    assert!(files[0].starts_with(env.state.join("repeat-error")));
    assert!(!env.state.parent().unwrap().join("escape.json").exists());
}

#[test]
fn state_files_untouched_for_a_week_are_deleted_on_the_next_write() {
    let env = Env::new("cleanup");
    env.run(&failure("old", "cargo test", "Exit code 1\nboom"));
    env.run(&failure("recent", "cargo test", "Exit code 1\nboom"));
    let old = env.state.join("repeat-error").join("old.json");
    set_age(&old, Duration::from_secs(8 * 24 * 60 * 60));
    set_age(
        &env.state.join("repeat-error").join("recent.json"),
        Duration::from_secs(6 * 24 * 60 * 60),
    );

    env.run(&failure("new", "cargo test", "Exit code 1\nboom"));
    assert!(!old.exists(), "the 8-day-old file is gone");
    assert!(env.state.join("repeat-error").join("recent.json").exists());
    assert!(env.state.join("repeat-error").join("new.json").exists());
}

fn set_age(path: &Path, age: Duration) {
    let file = std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open the state file");
    file.set_modified(SystemTime::now() - age)
        .expect("set the modification time");
}
