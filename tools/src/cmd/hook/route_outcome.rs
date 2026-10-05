//! `aios hook route-outcome`: log what happened to each subagent dispatch, so the
//! `route-shadow` records can be judged against it. Registered on `PostToolUse`
//! and `PostToolUseFailure` (matcher `Agent`) and on `SubagentStop`. Never prints
//! anything; fails open (see `OnError`).
//!
//! One JSON line per relevant event goes to `route-outcome.jsonl`, next to
//! `route-shadow.jsonl`. The join for the evaluation is
//! `route-shadow.tool_use_id = launched.tool_use_id` and
//! `launched.agent_id = stopped.agent_id`. A subagent that a stop gate sends back
//! to work stops again, so the `stopped` records of one `agent_id` count its stop
//! attempts.

use std::path::Path;

use anyhow::Result;
use clap::Args as ClapArgs;
use serde_json::{json, Map, Value};

use super::{append_jsonl, cut_to_boundary, unix_seconds, Ctx, HookInput};

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
    log_record(&dir.join(LOG_FILE), kind, input)?;
    Ok(None)
}

/// Append the record of `kind`. When that fails the failure is appended as an
/// error record, so it is in the log; only when that fails too is the original
/// error returned, for stderr.
fn log_record(path: &Path, kind: Kind, input: &HookInput) -> Result<()> {
    let ts = unix_seconds();
    match append_jsonl(path, &record(kind, ts, input)) {
        Ok(()) => Ok(()),
        Err(err) => append_jsonl(path, &error_record(ts, input, &err)).map_err(|_| err),
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

    #[test]
    fn the_error_record_names_the_event_and_the_error() {
        let dir = std::env::temp_dir().join(format!("aios-route-outcome-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let payload = json!({"hook_event_name":"PostToolUseFailure","tool_name":"Agent"});
        let hook_input = input(&payload);
        let err = anyhow::anyhow!("disk is on fire");
        append_jsonl(&dir.join(LOG_FILE), &error_record(7, &hook_input, &err)).unwrap();
        let log = std::fs::read_to_string(dir.join(LOG_FILE)).unwrap();
        let rec: Value = serde_json::from_str(log.trim_end()).unwrap();
        assert_eq!(rec["kind"], "error");
        assert_eq!(rec["event"], "PostToolUseFailure");
        assert_eq!(rec["error"], "disk is on fire");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unwritable_log_returns_the_original_error() {
        let dir =
            std::env::temp_dir().join(format!("aios-route-outcome-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // The log path is a directory, so neither the record nor the error record
        // can be appended.
        std::fs::create_dir_all(dir.join(LOG_FILE)).unwrap();
        let payload = json!({"hook_event_name":"SubagentStop"});
        let err = log_record(&dir.join(LOG_FILE), Kind::Stopped, &input(&payload)).unwrap_err();
        assert!(format!("{err:#}").contains("cannot append to"), "{err:#}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
