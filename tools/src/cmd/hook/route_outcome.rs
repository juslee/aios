//! `aios hook route-outcome`: log what happened to each subagent dispatch, so the
//! `route-shadow` records can be judged against it. Registered on `PostToolUse`
//! and `PostToolUseFailure` (matcher `Agent`) and on `SubagentStop`. Never prints
//! anything; fails open (see `OnError`).
//!
//! One JSON line per relevant event goes to `route-outcome.jsonl`, next to
//! `route-shadow.jsonl`. The join for the evaluation is
//! `route-shadow.tool_use_id = launched.tool_use_id` and
//! `launched.agent_id = stopped.agent_id`. The second equality is not documented
//! (`tool_response.agentId` and the `SubagentStop` `agent_id` are never said to be
//! one value), so it holds only once a real dispatch shows both fields equal; both
//! are stored as given. A subagent that a stop gate sends back to work stops
//! again, so the `stopped` records of one `agent_id` count its stop attempts; that
//! count uses `SubagentStop`'s own field and does not depend on the join.

use std::path::Path;

use anyhow::Result;
use clap::Args as ClapArgs;
use serde_json::{json, Map, Value};

use super::{append_line, cut_to_boundary, unix_seconds, Ctx, HookInput};

/// The dispatch tool's name in hook input.
const DISPATCH_TOOL: &str = "Agent";
/// A failure text is cut to this many bytes.
const MAX_ERROR_BYTES: usize = 512;
/// A subagent's last message is cut to this many characters.
const MAX_MESSAGE_CHARS: usize = 4000;
const LOG_FILE: &str = "route-outcome.jsonl";

#[derive(ClapArgs)]
pub struct Args {}

/// The records the hook writes for an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A dispatch started (`PostToolUse`, tool `Agent`).
    Launched,
    /// A dispatch failed to start (`PostToolUseFailure`, tool `Agent`).
    LaunchFailed,
    /// A subagent finished (`SubagentStop`).
    Stopped,
}

impl Kind {
    /// The record kind for a payload, or `None` for every other event and tool.
    pub fn from_input(input: &HookInput) -> Option<Kind> {
        let dispatch = input.tool_name.as_deref() == Some(DISPATCH_TOOL);
        match input.hook_event_name.as_deref() {
            Some("PostToolUse") if dispatch => Some(Kind::Launched),
            Some("PostToolUseFailure") if dispatch => Some(Kind::LaunchFailed),
            Some("SubagentStop") => Some(Kind::Stopped),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Launched => "launched",
            Kind::LaunchFailed => "launch_failed",
            Kind::Stopped => "stopped",
        }
    }
}

fn string_or_null(value: Option<&str>) -> Value {
    value.map_or(Value::Null, |s| Value::String(s.to_string()))
}

/// A string field of `tool_response`, null when absent or not a string.
fn response_str(input: &HookInput, key: &str) -> Value {
    string_or_null(input.tool_response.get(key).and_then(Value::as_str))
}

/// The fields every dispatch record starts with. `caller_agent_id` is present only
/// when a subagent dispatched the call.
fn dispatch_fields(kind: Kind, ts: u64, input: &HookInput) -> Map<String, Value> {
    let mut record = Map::new();
    record.insert("kind".into(), json!(kind.name()));
    record.insert("ts".into(), json!(ts));
    record.insert(
        "session_id".into(),
        string_or_null(input.session_id.as_deref()),
    );
    if let Some(caller) = input.agent_id.as_deref() {
        record.insert("caller_agent_id".into(), json!(caller));
    }
    record.insert(
        "tool_use_id".into(),
        string_or_null(input.tool_use_id.as_deref()),
    );
    record.insert(
        "subagent_type".into(),
        string_or_null(input.tool_str("subagent_type")),
    );
    record
}

fn launched(ts: u64, input: &HookInput) -> Value {
    let mut record = dispatch_fields(Kind::Launched, ts, input);
    record.insert("model".into(), string_or_null(input.tool_str("model")));
    record.insert("agent_id".into(), response_str(input, "agentId"));
    record.insert("status".into(), response_str(input, "status"));
    record.insert(
        "resolved_model".into(),
        response_str(input, "resolvedModel"),
    );
    // Only a completed (foreground) response carries run telemetry; it is kept as
    // the parsed JSON value, whatever its type.
    for (field, key) in [
        ("models_used", "modelsUsed"),
        ("total_tokens", "totalTokens"),
        ("total_duration_ms", "totalDurationMs"),
        ("total_tool_use_count", "totalToolUseCount"),
    ] {
        if let Some(value) = input.tool_response.get(key) {
            record.insert(field.into(), value.clone());
        }
    }
    Value::Object(record)
}

fn launch_failed(ts: u64, input: &HookInput) -> Value {
    let mut record = dispatch_fields(Kind::LaunchFailed, ts, input);
    record.insert("is_interrupt".into(), json!(input.is_interrupt));
    record.insert(
        "error".into(),
        string_or_null(
            input
                .error
                .as_deref()
                .map(|error| cut_to_boundary(error, MAX_ERROR_BYTES)),
        ),
    );
    Value::Object(record)
}

fn stopped(ts: u64, input: &HookInput) -> Value {
    let message = input.last_assistant_message.as_deref();
    let chars = message.map(|m| m.chars().count());
    let kept = message.map(|m| match m.char_indices().nth(MAX_MESSAGE_CHARS) {
        Some((end, _)) => &m[..end],
        None => m,
    });
    json!({
        "kind": Kind::Stopped.name(),
        "ts": ts,
        "session_id": input.session_id,
        "agent_id": input.agent_id,
        "agent_type": input.agent_type,
        "stop_hook_active": input.stop_hook_active,
        "agent_transcript_path": input.agent_transcript_path,
        "last_assistant_message": kept,
        "message_chars": chars,
        "message_truncated": chars.map(|n| n > MAX_MESSAGE_CHARS),
    })
}

/// The record for `input`, which is of kind `kind`.
pub fn record(kind: Kind, ts: u64, input: &HookInput) -> Value {
    match kind {
        Kind::Launched => launched(ts, input),
        Kind::LaunchFailed => launch_failed(ts, input),
        Kind::Stopped => stopped(ts, input),
    }
}

/// The record that reports an internal error once the state directory is known.
fn error_record(ts: u64, input: &HookInput, err: &anyhow::Error) -> Value {
    json!({
        "kind": "error",
        "ts": ts,
        "event": input.hook_event_name,
        "error": format!("{err:#}"),
    })
}

pub fn run(_args: &Args, input: &HookInput, ctx: &Ctx) -> Result<Option<String>> {
    let Some(kind) = Kind::from_input(input) else {
        return Ok(None);
    };
    let dir = ctx.state_dir(input)?;
    log_record(&dir.join(LOG_FILE), kind, input, append_line)?;
    Ok(None)
}

/// Append the record of `kind` through `append`, which writes one text to the log.
/// When that fails the failure is appended as an error record, so it is in the log;
/// only when that fails too is the original error returned, for stderr.
///
/// The error record starts with a newline: a failed append may have written part of
/// its line (a short write on a full volume), and the newline ends that fragment
/// instead of gluing the error record onto it. Readers skip blank lines and lines
/// that are not JSON; the fragment is lost either way, the error record is not.
fn log_record(
    path: &Path,
    kind: Kind,
    input: &HookInput,
    append: impl Fn(&Path, &str) -> Result<()>,
) -> Result<()> {
    let ts = unix_seconds();
    match append(path, &format!("{}\n", record(kind, ts, input))) {
        Ok(()) => Ok(()),
        Err(err) => {
            let text = format!("\n{}\n", error_record(ts, input, &err));
            append(path, &text).map_err(|_| err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::hook::parse_input;

    fn input(payload: &Value) -> HookInput {
        parse_input(payload.to_string().as_bytes()).expect("the payload parses")
    }

    #[test]
    fn only_dispatch_events_and_subagent_stops_have_a_kind() {
        let kind = |payload: Value| Kind::from_input(&input(&payload));
        assert_eq!(
            kind(json!({"hook_event_name":"PostToolUse","tool_name":"Agent"})),
            Some(Kind::Launched)
        );
        assert_eq!(
            kind(json!({"hook_event_name":"PostToolUseFailure","tool_name":"Agent"})),
            Some(Kind::LaunchFailed)
        );
        assert_eq!(
            kind(json!({"hook_event_name":"SubagentStop"})),
            Some(Kind::Stopped)
        );
        assert_eq!(
            kind(json!({"hook_event_name":"PostToolUse","tool_name":"Bash"})),
            None
        );
        assert_eq!(
            kind(json!({"hook_event_name":"PostToolUseFailure","tool_name":"Task"})),
            None
        );
        assert_eq!(kind(json!({"hook_event_name":"Stop"})), None);
        assert_eq!(kind(json!({"tool_name":"Agent"})), None);
    }

    #[test]
    fn a_message_is_cut_on_a_character_boundary() {
        let message = "é".repeat(MAX_MESSAGE_CHARS + 1);
        let payload = json!({"hook_event_name":"SubagentStop","last_assistant_message":message});
        let rec = record(Kind::Stopped, 1, &input(&payload));
        assert_eq!(
            rec["last_assistant_message"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            MAX_MESSAGE_CHARS
        );
        assert_eq!(rec["message_chars"], MAX_MESSAGE_CHARS + 1);
        assert_eq!(rec["message_truncated"], true);
    }

    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("aios-route-outcome-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// An appender that fails its first call and passes the rest to `append_line`.
    fn failing_once(
        calls: &std::cell::RefCell<Vec<String>>,
    ) -> impl Fn(&Path, &str) -> Result<()> + '_ {
        move |path, text| {
            calls.borrow_mut().push(text.to_string());
            if calls.borrow().len() == 1 {
                anyhow::bail!("disk is on fire");
            }
            append_line(path, text)
        }
    }

    #[test]
    fn a_failed_append_is_logged_as_an_error_record() {
        let dir = scratch_dir("fallback");
        let log = dir.join(LOG_FILE);
        let payload = json!({"hook_event_name":"SubagentStop","agent_id":"a1"});
        let calls = std::cell::RefCell::new(Vec::new());
        log_record(&log, Kind::Stopped, &input(&payload), failing_once(&calls)).unwrap();

        let calls = calls.into_inner();
        assert_eq!(calls.len(), 2);
        let first: Value = serde_json::from_str(calls[0].trim_end()).unwrap();
        assert_eq!(first["kind"], "stopped");
        assert!(
            calls[1].starts_with('\n'),
            "the error record ends a partial line"
        );
        let text = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 1, "only the error record reached the file");
        let rec: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(rec["kind"], "error");
        assert_eq!(rec["event"], "SubagentStop");
        assert_eq!(rec["error"], "disk is on fire");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_error_record_does_not_extend_a_partial_line() {
        let dir = scratch_dir("partial");
        let log = dir.join(LOG_FILE);
        // What a short write leaves behind: a line without its newline.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &log,
            "{\"kind\":\"stopped\",\"last_assistant_message\":\"trunc",
        )
        .unwrap();
        let payload = json!({"hook_event_name":"SubagentStop"});
        let calls = std::cell::RefCell::new(Vec::new());
        log_record(&log, Kind::Stopped, &input(&payload), failing_once(&calls)).unwrap();

        let text = std::fs::read_to_string(&log).unwrap();
        let parsed: Vec<Option<Value>> = text
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str(l).ok())
            .collect();
        assert_eq!(parsed.len(), 2, "{text}");
        assert!(parsed[0].is_none(), "the fragment stays a line of its own");
        assert_eq!(parsed[1].as_ref().unwrap()["kind"], "error");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_successful_append_writes_no_error_record() {
        let dir = scratch_dir("ok");
        let log = dir.join(LOG_FILE);
        let payload = json!({"hook_event_name":"SubagentStop"});
        log_record(&log, Kind::Stopped, &input(&payload), append_line).unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(text.starts_with("{\"kind\":\"stopped\""), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unwritable_log_returns_the_original_error() {
        let dir = scratch_dir("dir");
        // The log path is a directory, so neither the record nor the error record
        // can be appended.
        std::fs::create_dir_all(dir.join(LOG_FILE)).unwrap();
        let payload = json!({"hook_event_name":"SubagentStop"});
        let err = log_record(
            &dir.join(LOG_FILE),
            Kind::Stopped,
            &input(&payload),
            append_line,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("cannot append to"), "{err:#}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
