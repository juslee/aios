//! `aios hook route-outcome` through the built binary: the three record kinds,
//! lenient reads, the join with `route-shadow`, and the output discipline (nothing
//! on stdout, exit 0 on every payload).

mod hook_support;

use std::path::PathBuf;

use hook_support::{outside_dir, run_hook, unique_dir, Run};
use serde_json::{json, Value};

const LOG: &str = "route-outcome.jsonl";

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

    /// Run the hook on `payload`; it must exit 0 with nothing on stdout.
    fn run(&self, payload: &Value) -> Run {
        let state = self.state.to_str().expect("the state path is UTF-8");
        let run = run_hook(
            &["route-outcome"],
            payload.to_string().as_bytes(),
            &[("AIOS_HOOK_STATE_DIR", state)],
            &self.cwd,
        );
        assert_eq!(run.code, Some(0), "{}", run.stderr);
        assert_eq!(run.stdout, "", "route-outcome never prints to stdout");
        run
    }

    fn records(&self) -> Vec<Value> {
        match std::fs::read_to_string(self.state.join(LOG)) {
            Ok(log) => log
                .lines()
                .map(|line| serde_json::from_str(line).expect("each line is JSON"))
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Run the hook and return the one record it logged.
    fn one(&self, payload: &Value) -> Value {
        let run = self.run(payload);
        assert_eq!(run.stderr, "", "a clean run is silent");
        let mut records = self.records();
        assert_eq!(records.len(), 1, "{records:?}");
        records.remove(0)
    }
}

fn completed() -> Value {
    json!({
        "session_id": "s1",
        "hook_event_name": "PostToolUse",
        "tool_name": "Agent",
        "tool_use_id": "toolu_77",
        "tool_input": {"subagent_type": "worker", "description": "d", "prompt": "p", "model": "sonnet"},
        "tool_response": {
            "status": "completed",
            "agentId": "a42",
            "content": [{"type": "text", "text": "done"}],
            "resolvedModel": "claude-sonnet-4-5",
            "modelsUsed": ["claude-sonnet-4-5"],
            "totalTokens": 1234,
            "totalDurationMs": 5678,
            "totalToolUseCount": 9,
            "usage": {"input_tokens": 1},
        },
    })
}

fn async_launched() -> Value {
    json!({
        "session_id": "s1",
        "hook_event_name": "PostToolUse",
        "tool_name": "Agent",
        "tool_use_id": "toolu_78",
        "tool_input": {"subagent_type": "Explore", "prompt": "p"},
        "tool_response": {
            "status": "async_launched",
            "agentId": "a43",
            "description": "d",
            "prompt": "p",
            "outputFile": "/tmp/out",
            "resolvedModel": "claude-haiku-4-5",
        },
    })
}

fn stop(agent_id: &str, message: &Value) -> Value {
    json!({
        "session_id": "s1",
        "hook_event_name": "SubagentStop",
        "agent_id": agent_id,
        "agent_type": "worker",
        "agent_transcript_path": "/tmp/t.jsonl",
        "last_assistant_message": message,
        "stop_hook_active": false,
    })
}

#[test]
fn a_completed_foreground_dispatch_is_a_launched_record_with_telemetry() {
    let env = Env::new("completed");
    let rec = env.one(&completed());
    assert_eq!(rec["kind"], "launched");
    assert!(rec["ts"].as_u64().unwrap() > 1_700_000_000);
    assert_eq!(rec["session_id"], "s1");
    assert_eq!(rec["tool_use_id"], "toolu_77");
    assert_eq!(rec["subagent_type"], "worker");
    assert_eq!(rec["model"], "sonnet");
    assert_eq!(rec["agent_id"], "a42");
    assert_eq!(rec["status"], "completed");
    assert_eq!(rec["resolved_model"], "claude-sonnet-4-5");
    assert_eq!(rec["models_used"], json!(["claude-sonnet-4-5"]));
    assert_eq!(rec["total_tokens"], 1234);
    assert_eq!(rec["total_duration_ms"], 5678);
    assert_eq!(rec["total_tool_use_count"], 9);
    assert_eq!(
        rec.get("caller_agent_id"),
        None,
        "dispatched by the main session"
    );
}

#[test]
fn an_async_launched_dispatch_has_no_telemetry() {
    let env = Env::new("async");
    let rec = env.one(&async_launched());
    assert_eq!(rec["kind"], "launched");
    assert_eq!(rec["agent_id"], "a43");
    assert_eq!(rec["status"], "async_launched");
    assert_eq!(rec["resolved_model"], "claude-haiku-4-5");
    assert_eq!(rec["subagent_type"], "Explore");
    assert_eq!(rec["model"], Value::Null, "no model was requested");
    for key in [
        "models_used",
        "total_tokens",
        "total_duration_ms",
        "total_tool_use_count",
    ] {
        assert_eq!(rec.get(key), None, "{key} is absent, not null");
    }
}

#[test]
fn telemetry_is_kept_only_in_its_documented_type() {
    let env = Env::new("telemetry-types");
    let mut payload = completed();
    payload["tool_response"]["modelsUsed"] = json!("claude-sonnet-4-5");
    payload["tool_response"]["totalTokens"] = json!("1234");
    payload["tool_response"]["totalDurationMs"] = json!({"huge": "x".repeat(10_000)});
    payload["tool_response"]["totalToolUseCount"] = json!([9]);
    let rec = env.one(&payload);
    for key in [
        "models_used",
        "total_tokens",
        "total_duration_ms",
        "total_tool_use_count",
    ] {
        assert_eq!(
            rec.get(key),
            None,
            "{key} has the wrong type and is left out"
        );
    }
    assert_eq!(rec["agent_id"], "a42", "the rest of the record is intact");

    let env = Env::new("telemetry-array");
    let mut payload = completed();
    payload["tool_response"]["modelsUsed"] = json!(["a", 7]);
    assert_eq!(
        env.one(&payload).get("models_used"),
        None,
        "an array that holds a non-string is not an array of model names"
    );

    let env = Env::new("telemetry-too-many");
    let mut payload = completed();
    payload["tool_response"]["modelsUsed"] = json!(vec!["m"; 17]);
    assert_eq!(
        env.one(&payload).get("models_used"),
        None,
        "an array over the entry cap is left out"
    );

    let env = Env::new("telemetry-long-name");
    let mut payload = completed();
    payload["tool_response"]["modelsUsed"] = json!(["x".repeat(129)]);
    assert_eq!(
        env.one(&payload).get("models_used"),
        None,
        "a model name over the byte cap is left out"
    );

    let env = Env::new("telemetry-at-cap");
    let mut payload = completed();
    payload["tool_response"]["modelsUsed"] = json!(vec!["x".repeat(128); 16]);
    let expected = payload["tool_response"]["modelsUsed"].clone();
    assert_eq!(
        env.one(&payload)["models_used"],
        expected,
        "an array exactly at both caps is kept"
    );
}

#[test]
fn caller_agent_id_is_present_only_when_a_subagent_dispatched() {
    let env = Env::new("caller");
    let mut payload = completed();
    payload["agent_id"] = json!("lead-1");
    payload["agent_type"] = json!("kernel-dev");
    assert_eq!(env.one(&payload)["caller_agent_id"], "lead-1");

    let env = Env::new("caller-none");
    assert_eq!(env.one(&completed()).get("caller_agent_id"), None);

    let env = Env::new("caller-failed");
    let mut failure = json!({
        "session_id": "s1", "hook_event_name": "PostToolUseFailure", "tool_name": "Agent",
        "tool_use_id": "toolu_1", "tool_input": {}, "error": "x", "agent_id": "lead-2",
    });
    assert_eq!(env.one(&failure)["caller_agent_id"], "lead-2");
    failure.as_object_mut().unwrap().remove("agent_id");
    let env = Env::new("caller-failed-none");
    assert_eq!(env.one(&failure).get("caller_agent_id"), None);
}

#[test]
fn a_launch_failure_cuts_the_error_on_a_character_boundary() {
    let env = Env::new("failed");
    // Three-byte characters: 512 is not a multiple of 3, so a byte cut would split one.
    let error = "€".repeat(300);
    let payload = json!({
        "session_id": "s1", "hook_event_name": "PostToolUseFailure", "tool_name": "Agent",
        "tool_use_id": "toolu_9", "tool_input": {"subagent_type": "worker"},
        "error": error, "is_interrupt": true,
    });
    let rec = env.one(&payload);
    assert_eq!(rec["kind"], "launch_failed");
    assert_eq!(rec["tool_use_id"], "toolu_9");
    assert_eq!(rec["subagent_type"], "worker");
    assert_eq!(rec["is_interrupt"], true);
    let kept = rec["error"].as_str().unwrap();
    assert_eq!(
        kept.len(),
        510,
        "170 whole characters, the most that fit in 512 bytes"
    );
    assert!(error.starts_with(kept));
}

#[test]
fn a_short_error_is_kept_whole() {
    let env = Env::new("failed-short");
    let payload = json!({
        "hook_event_name": "PostToolUseFailure", "tool_name": "Agent",
        "tool_input": {}, "error": "unknown agent type",
    });
    let rec = env.one(&payload);
    assert_eq!(rec["error"], "unknown agent type");
    assert_eq!(rec["is_interrupt"], Value::Null);
    assert_eq!(rec["session_id"], Value::Null);
}

#[test]
fn a_stop_cuts_a_long_multibyte_message_to_4000_characters() {
    let env = Env::new("stopped");
    let message = "日".repeat(4001);
    let rec = env.one(&stop("a42", &json!(message)));
    assert_eq!(rec["kind"], "stopped");
    assert_eq!(rec["session_id"], "s1");
    assert_eq!(rec["agent_id"], "a42");
    assert_eq!(rec["agent_type"], "worker");
    assert_eq!(rec["agent_transcript_path"], "/tmp/t.jsonl");
    assert_eq!(rec["stop_hook_active"], false);
    let kept = rec["last_assistant_message"].as_str().unwrap();
    assert_eq!(kept.chars().count(), 4000);
    assert!(message.starts_with(kept));
    assert_eq!(rec["message_chars"], 4001);
    assert_eq!(rec["message_truncated"], true);
}

#[test]
fn a_message_of_exactly_4000_characters_is_not_truncated() {
    let env = Env::new("stopped-exact");
    let message = "x".repeat(4000);
    let rec = env.one(&stop("a42", &json!(message)));
    assert_eq!(rec["last_assistant_message"], message.as_str());
    assert_eq!(rec["message_chars"], 4000);
    assert_eq!(rec["message_truncated"], false);
}

#[test]
fn two_stops_of_one_agent_are_two_records() {
    let env = Env::new("two-stops");
    let mut again = stop("a42", &json!("second"));
    again["stop_hook_active"] = json!(true);
    env.run(&stop("a42", &json!("first")));
    env.run(&again);
    env.run(&stop("a99", &json!("other agent")));
    let records = env.records();
    let of_a42: Vec<&Value> = records.iter().filter(|r| r["agent_id"] == "a42").collect();
    assert_eq!(of_a42.len(), 2);
    assert_eq!(of_a42[0]["stop_hook_active"], false);
    assert_eq!(of_a42[1]["stop_hook_active"], true);
    assert_eq!(records.len(), 3);
}

#[test]
fn other_tools_and_events_leave_no_record() {
    let env = Env::new("ignored");
    for payload in [
        json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":"ls"},"tool_response":{"stdout":""}}),
        json!({"hook_event_name":"PostToolUseFailure","tool_name":"Bash","tool_input":{},"error":"Exit code 1"}),
        json!({"hook_event_name":"PostToolUse","tool_name":"Task","tool_response":{"agentId":"x"}}),
        json!({"hook_event_name":"PreToolUse","tool_name":"Agent","tool_input":{"prompt":"p"}}),
        json!({"hook_event_name":"Stop","last_assistant_message":"m"}),
        json!({"hook_event_name":"SessionStart"}),
        json!({"tool_name":"Agent"}),
        json!({}),
    ] {
        let run = env.run(&payload);
        assert_eq!(run.stderr, "", "{payload}");
    }
    assert!(env.records().is_empty());
    assert!(!env.state.join(LOG).exists(), "no event, no file");
}

#[test]
fn an_event_without_a_record_never_resolves_the_state_dir() {
    // No `AIOS_HOOK_STATE_DIR`, and a cwd in no repository: resolving the state
    // directory would run `git rev-parse` and write a diagnostic. An event that
    // gets no record must not reach it, so the run stays silent.
    let cwd = outside_dir("ignored-no-git");
    for payload in [
        json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":"ls"}}),
        json!({"hook_event_name":"PreToolUse","tool_name":"Agent","tool_input":{"prompt":"p"}}),
        json!({"hook_event_name":"SessionStart"}),
    ] {
        let mut payload = payload;
        payload["cwd"] = json!(cwd.to_str().unwrap());
        let run = run_hook(
            &["route-outcome"],
            payload.to_string().as_bytes(),
            &[],
            &cwd,
        );
        assert_eq!(run.code, Some(0), "{payload}: {}", run.stderr);
        assert_eq!(run.stdout, "", "{payload}");
        assert_eq!(run.stderr, "", "{payload}");
    }
}

#[test]
fn an_unknown_state_dir_reports_on_stderr_only() {
    // A `.git` file pointing nowhere makes `git rev-parse` fail wherever the test
    // runs. The state directory is then unknown, so there is no log to write to:
    // the error goes to stderr, nothing to stdout, and the exit code stays 0.
    let cwd = unique_dir("unknown-state");
    std::fs::write(cwd.join(".git"), "gitdir: /nonexistent/aios-hook-test\n").unwrap();
    let mut payload = completed();
    payload["cwd"] = json!(cwd.to_str().unwrap());
    let run = run_hook(
        &["route-outcome"],
        payload.to_string().as_bytes(),
        &[],
        &cwd,
    );
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.stdout, "");
    assert!(run.stderr.contains("route-outcome"), "{}", run.stderr);
    assert!(run.stderr.contains("git rev-parse"), "{}", run.stderr);
    let written: Vec<_> = std::fs::read_dir(&cwd)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        written,
        vec![std::ffi::OsString::from(".git")],
        "no log was guessed"
    );
}

#[test]
fn wrong_typed_fields_read_as_null() {
    let env = Env::new("wrong-types");
    let payload = json!({
        "session_id": 5,
        "hook_event_name": "PostToolUse",
        "tool_name": "Agent",
        "agent_id": ["x"],
        "tool_use_id": {"id": 1},
        "tool_input": {"subagent_type": 7, "model": false},
        "tool_response": {"status": {"a": 1}, "agentId": 42, "resolvedModel": null},
    });
    let rec = env.one(&payload);
    assert_eq!(rec["kind"], "launched");
    for key in [
        "session_id",
        "tool_use_id",
        "subagent_type",
        "model",
        "agent_id",
        "status",
        "resolved_model",
    ] {
        assert_eq!(rec[key], Value::Null, "{key}");
    }
    assert_eq!(
        rec.get("caller_agent_id"),
        None,
        "a wrong-typed agent_id is absent"
    );

    let env = Env::new("wrong-types-stop");
    let payload = json!({
        "hook_event_name": "SubagentStop",
        "agent_id": 1, "agent_type": [], "agent_transcript_path": 2,
        "last_assistant_message": {"text": "x"}, "stop_hook_active": "yes",
    });
    let rec = env.one(&payload);
    for key in [
        "session_id",
        "agent_id",
        "agent_type",
        "agent_transcript_path",
        "last_assistant_message",
        "message_chars",
        "message_truncated",
        "stop_hook_active",
    ] {
        assert_eq!(rec[key], Value::Null, "{key}");
    }

    let env = Env::new("wrong-types-failure");
    let payload = json!({
        "hook_event_name": "PostToolUseFailure", "tool_name": "Agent",
        "tool_input": "not an object", "error": 3, "is_interrupt": "no",
    });
    let rec = env.one(&payload);
    assert_eq!(rec["error"], Value::Null);
    assert_eq!(rec["is_interrupt"], Value::Null);
    assert_eq!(rec["subagent_type"], Value::Null);

    let env = Env::new("response-not-object");
    let mut payload = completed();
    payload["tool_response"] = json!("a plain string");
    let rec = env.one(&payload);
    assert_eq!(rec["agent_id"], Value::Null);
    assert_eq!(rec["status"], Value::Null);
}

#[test]
fn nothing_reaches_stdout_on_any_path() {
    let env = Env::new("quiet");
    // `Env::run` asserts an empty stdout and exit 0; cover every record kind, an
    // ignored event and a payload the hook cannot use.
    env.run(&completed());
    env.run(&async_launched());
    env.run(&stop("a1", &json!("m")));
    env.run(&json!({"hook_event_name":"PostToolUseFailure","tool_name":"Agent","error":"e"}));
    env.run(&json!({"hook_event_name":"PostToolUse","tool_name":"Read"}));
    let run = run_hook(
        &["route-outcome"],
        b"{not json",
        &[("AIOS_HOOK_STATE_DIR", env.state.to_str().unwrap())],
        &env.cwd,
    );
    assert_eq!(run.code, Some(0));
    assert_eq!(run.stdout, "");
    assert!(run.stderr.contains("route-outcome"), "{}", run.stderr);
    assert_eq!(env.records().len(), 4);
}

#[test]
fn an_unwritable_state_dir_exits_zero_with_a_diagnostic() {
    let env = Env::new("unwritable");
    // The state "directory" is a regular file, so nothing can be created under it.
    let blocker = env.state.join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    let state = blocker.join("state");
    let run = run_hook(
        &["route-outcome"],
        completed().to_string().as_bytes(),
        &[("AIOS_HOOK_STATE_DIR", state.to_str().unwrap())],
        &env.cwd,
    );
    assert_eq!(run.code, Some(0));
    assert_eq!(run.stdout, "");
    assert!(run.stderr.contains("route-outcome"), "{}", run.stderr);
    assert!(!state.exists());
}

#[test]
fn the_log_is_appended_across_runs() {
    let env = Env::new("append");
    env.run(&completed());
    env.run(&async_launched());
    let records = env.records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["agent_id"], "a42");
    assert_eq!(records[1]["agent_id"], "a43");
}

#[test]
fn the_join_from_shadow_to_launch_to_stop_holds() {
    // One state directory, as in a real session: the real route-shadow logs the
    // dispatch (no key, so an error record with no request: only the fields the
    // join uses matter), route-outcome the rest.
    let env = Env::new("join");
    let dispatch = json!({
        "session_id": "s1",
        "hook_event_name": "PreToolUse",
        "tool_name": "Agent",
        "tool_use_id": "toolu_77",
        "tool_input": {"subagent_type": "worker", "description": "d", "prompt": "p"},
    });
    let shadow_run = run_hook(
        &["route-shadow"],
        dispatch.to_string().as_bytes(),
        &[("AIOS_HOOK_STATE_DIR", env.state.to_str().unwrap())],
        &env.cwd,
    );
    assert_eq!(shadow_run.code, Some(0), "{}", shadow_run.stderr);
    env.run(&completed());
    env.run(&stop("a42", &json!("first try")));
    env.run(&stop("a42", &json!("after the gate")));
    env.run(&async_launched());
    env.run(&stop("a43", &json!("explored")));

    let read = |name: &str| -> Vec<Value> {
        std::fs::read_to_string(env.state.join(name))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    };
    let shadows = read("route-shadow.jsonl");
    assert_eq!(shadows.len(), 1);
    assert_eq!(shadows[0]["error"], "TYPESAFE_API_KEY is not set");
    let outcomes = read(LOG);
    let launched = outcomes
        .iter()
        .find(|r| {
            r["kind"] == "launched"
                && !shadows[0]["tool_use_id"].is_null()
                && r["tool_use_id"] == shadows[0]["tool_use_id"]
        })
        .expect("the dispatch's launched record");
    let stops: Vec<&Value> = outcomes
        .iter()
        .filter(|r| r["kind"] == "stopped" && r["agent_id"] == launched["agent_id"])
        .collect();
    assert_eq!(launched["agent_id"], "a42");
    assert_eq!(stops.len(), 2, "two stop attempts of that agent");
    assert_eq!(stops[1]["last_assistant_message"], "after the gate");
}
