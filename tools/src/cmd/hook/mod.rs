//! `aios hook <name>`: the Claude Code hook programs of the agent team.
//!
//! A hook reads one JSON object from stdin, writes at most one JSON object to
//! stdout, puts diagnostics on stderr and exits 0 on every path, so a broken hook
//! never blocks or nags a session. `path-guard` expresses its failure as a deny
//! instead of a non-zero exit. Parsing is lenient (every field optional, unknown
//! fields ignored), so a Claude Code release that adds or retypes fields changes
//! nothing.
//!
//! Two exits are outside the hook's control: a clap usage error (a bad
//! registration, never a bad payload) exits 2 before any code here runs, and a
//! hook timeout is handled by Claude Code.

pub mod path_guard;
pub mod repeat_error;
pub mod route_outcome;
pub mod route_shadow;

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};

use crate::proc;

/// Stdin larger than this is an input error. Tool results can be large, so the
/// cap is generous; it only stops a runaway writer from filling memory.
pub const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;

/// Claude Code caps each `additionalContext` string at 10,000 characters; the
/// builders stay under that in bytes.
pub const MAX_CONTEXT_BYTES: usize = 10_000;

/// The longest session-id file-name part kept as is; longer ids are hashed.
const MAX_ID_BYTES: usize = 128;

#[derive(Parser)]
pub struct Args {
    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// PostToolUseFailure and PostToolUse for Bash: notice the same command failing the same way twice
    RepeatError(repeat_error::Args),
    /// PreToolUse for the edit tools: deny edits under configured repository prefixes
    PathGuard(path_guard::Args),
    /// PreToolUse for the dispatch tool: log how Jev would route it, never decide
    RouteShadow(route_shadow::Args),
    /// PostToolUse and PostToolUseFailure for the dispatch tool, and SubagentStop: log what happened to each dispatch
    RouteOutcome(route_outcome::Args),
}

/// What a subcommand does when it cannot do its job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnError {
    /// No stdout, a diagnostic on stderr, exit 0.
    Open,
    /// A PreToolUse deny naming the error, exit 0.
    Deny,
}

impl Cmd {
    pub fn name(&self) -> &'static str {
        match self {
            Cmd::RepeatError(_) => "repeat-error",
            Cmd::PathGuard(_) => "path-guard",
            Cmd::RouteShadow(_) => "route-shadow",
            Cmd::RouteOutcome(_) => "route-outcome",
        }
    }

    pub fn on_error(&self) -> OnError {
        match self {
            Cmd::PathGuard(_) => OnError::Deny,
            Cmd::RepeatError(_) | Cmd::RouteShadow(_) | Cmd::RouteOutcome(_) => OnError::Open,
        }
    }
}

/// The hook input fields the subcommands use. Every field is optional and a field
/// of an unexpected type reads as absent, so only a payload that is not a JSON
/// object is an input error.
#[derive(Debug, Default, Deserialize)]
pub struct HookInput {
    #[serde(default, deserialize_with = "lenient_string")]
    pub session_id: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub cwd: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub hook_event_name: Option<String>,
    /// Present only when the hook fires inside a subagent.
    #[serde(default, deserialize_with = "lenient_string")]
    pub agent_id: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub agent_type: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub tool_name: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub tool_use_id: Option<String>,
    /// PostToolUseFailure only: the failure text.
    #[serde(default, deserialize_with = "lenient_string")]
    pub error: Option<String>,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub is_interrupt: Option<bool>,
    #[serde(default)]
    pub tool_input: Value,
    /// PostToolUse only: the tool's result, whose shape depends on the tool.
    #[serde(default)]
    pub tool_response: Value,
    /// SubagentStop only.
    #[serde(default, deserialize_with = "lenient_string")]
    pub agent_transcript_path: Option<String>,
    /// SubagentStop only: how the subagent ended.
    #[serde(default, deserialize_with = "lenient_string")]
    pub last_assistant_message: Option<String>,
    /// SubagentStop only: true while the subagent continues after a block.
    #[serde(default, deserialize_with = "lenient_bool")]
    pub stop_hook_active: Option<bool>,
}

impl HookInput {
    /// A string field of `tool_input`, or `None` when absent or not a string.
    pub fn tool_str(&self, key: &str) -> Option<&str> {
        self.tool_input.get(key).and_then(Value::as_str)
    }
}

fn lenient_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) => Some(s),
        _ => None,
    })
}

fn lenient_bool<'de, D: Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
    Ok(Value::deserialize(d)?.as_bool())
}

/// Read `reader` to the end, failing when it holds more than `cap` bytes.
pub fn read_capped(reader: impl Read, cap: usize) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    reader
        .take(cap as u64 + 1)
        .read_to_end(&mut buf)
        .context("cannot read the hook input")?;
    if buf.len() > cap {
        bail!("hook input is larger than {cap} bytes");
    }
    Ok(buf)
}

/// Parse one hook payload.
pub fn parse_input(raw: &[u8]) -> Result<HookInput> {
    let value: Value = serde_json::from_slice(raw).context("hook input is not valid JSON")?;
    if !value.is_object() {
        bail!("hook input is not a JSON object");
    }
    serde_json::from_value(value).context("hook input has unexpected field types")
}

/// `PreToolUse` output that denies the call; `reason` is shown to Claude.
pub fn pre_tool_use_deny(reason: &str) -> String {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    })
    .to_string()
}

/// `PostToolUse` or `PostToolUseFailure` output that adds `text` next to the tool
/// result, cut to the context cap.
pub fn additional_context(event: &str, text: &str) -> String {
    json!({
        "hookSpecificOutput": {
            "hookEventName": event,
            "additionalContext": cut_to_boundary(text, MAX_CONTEXT_BYTES),
        }
    })
    .to_string()
}

/// The longest prefix of `s` that is at most `max` bytes and ends on a character
/// boundary.
pub fn cut_to_boundary(s: &str, max: usize) -> &str {
    let mut end = max.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Process-level inputs of the hooks, passed in so tests need no environment.
pub struct Ctx {
    /// `AIOS_HOOK_STATE_DIR`: the state directory itself, replacing the git one.
    pub state_override: Option<PathBuf>,
    /// The working directory used when the payload carries no `cwd`.
    pub process_cwd: Option<PathBuf>,
}

impl Ctx {
    pub fn from_env() -> Ctx {
        Ctx::new(
            std::env::var_os("AIOS_HOOK_STATE_DIR"),
            std::env::current_dir().ok(),
        )
    }

    /// An empty override counts as unset.
    pub fn new(state_override: Option<OsString>, process_cwd: Option<PathBuf>) -> Ctx {
        Ctx {
            state_override: state_override
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
            process_cwd,
        }
    }

    /// The directory the hooks keep state in: `<git common dir>/aios-agent/hooks`,
    /// shared by every worktree of the repository, unless overridden. Git runs in
    /// the payload's `cwd`, else in the process working directory.
    pub fn state_dir(&self, input: &HookInput) -> Result<PathBuf> {
        if let Some(dir) = &self.state_override {
            return Ok(dir.clone());
        }
        let cwd = input
            .cwd
            .as_deref()
            .map(PathBuf::from)
            .or_else(|| self.process_cwd.clone())
            .context("the hook input has no cwd and the process has no working directory")?;
        let out = proc::capture(
            "git",
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            &cwd,
        )?;
        if !out.status.success() {
            bail!(
                "git rev-parse --git-common-dir failed in {}: {}",
                cwd.display(),
                String::from_utf8_lossy(&out.stderr).trim_end()
            );
        }
        let common =
            String::from_utf8(out.stdout).context("the git common directory is not valid UTF-8")?;
        let common = common.trim_end_matches(['\n', '\r']);
        if common.is_empty() {
            bail!("git printed no common directory in {}", cwd.display());
        }
        Ok(Path::new(common).join("aios-agent").join("hooks"))
    }
}

/// A session id as a file-name part. Ids made only of `[A-Za-z0-9_-]` and at most
/// 128 bytes pass through; anything else, including the empty id, becomes the
/// FNV-1a 64-bit digest of the original as 16 hex digits, so a crafted id can
/// never name a path outside the state directory.
pub fn sanitize_id(id: &str) -> String {
    let plain = !id.is_empty()
        && id.len() <= MAX_ID_BYTES
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if plain {
        id.to_string()
    } else {
        format!("{:016x}", fnv1a64(id.as_bytes()))
    }
}

/// FNV-1a, 64-bit: fixed by definition, so the digest is stable across builds
/// (unlike `std`'s `DefaultHasher`).
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The per-session, per-agent state-file stem: `<session>` in the main session,
/// `<session>-<agent>` inside a subagent. Both parts are sanitised.
pub fn state_key(input: &HookInput) -> String {
    let session = sanitize_id(input.session_id.as_deref().unwrap_or(""));
    match input.agent_id.as_deref() {
        Some(agent) => format!("{session}-{}", sanitize_id(agent)),
        None => session,
    }
}

/// Seconds since the Unix epoch, 0 if the clock is before it.
pub fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Append `record` as one JSON line to the log at `path`, creating the file and its
/// directory. The line goes out in a single write, so records of concurrent hooks
/// do not interleave.
pub fn append_jsonl(path: &Path, record: &impl Serialize) -> Result<()> {
    let mut line = serde_json::to_string(record).context("cannot encode the log record")?;
    line.push('\n');
    append_line(path, &line)
}

/// Append `text`, which ends in a newline, to the file at `path` in one write,
/// creating the file and its directory.
pub fn append_line(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .with_context(|| format!("cannot append to {}", path.display()))
}

/// Write `bytes` to `path` through a temporary file in the same directory and a
/// rename, creating the directory if needed. A reader sees the old or the new
/// content, never a partial file. Two writers racing on one path can lose an
/// update; the hooks accept that rather than lock.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    let name = path
        .file_name()
        .with_context(|| format!("{} has no file name", path.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let mut tmp_name = OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let tmp = dir.join(tmp_name);
    let written = std::fs::write(&tmp, bytes)
        .with_context(|| format!("cannot write {}", tmp.display()))
        .and_then(|()| {
            std::fs::rename(&tmp, path)
                .with_context(|| format!("cannot rename {} to {}", tmp.display(), path.display()))
        });
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// Run one hook: read and parse stdin, dispatch, print at most one JSON object.
/// Never fails: an error becomes the subcommand's error policy (see `OnError`).
/// A closed stdout or stderr is ignored, as nothing could report it.
pub fn run(
    args: &Args,
    ctx: &Ctx,
    stdin: impl Read,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) {
    let name = args.command.name();
    let result = read_capped(stdin, MAX_INPUT_BYTES)
        .and_then(|raw| parse_input(&raw))
        .and_then(|input| dispatch(&args.command, &input, ctx));
    let output = match result {
        Ok(output) => output,
        Err(err) => {
            let _ = writeln!(stderr, "aios hook {name}: {err:#}");
            match args.command.on_error() {
                OnError::Open => None,
                OnError::Deny => Some(pre_tool_use_deny(&format!(
                    "{name} could not check this call, so it is denied: {err:#}"
                ))),
            }
        }
    };
    if let Some(json) = output {
        let _ = writeln!(stdout, "{json}");
    }
    let _ = stdout.flush();
}

fn dispatch(cmd: &Cmd, input: &HookInput, ctx: &Ctx) -> Result<Option<String>> {
    match cmd {
        Cmd::RepeatError(args) => repeat_error::run(args, input, ctx),
        Cmd::PathGuard(args) => path_guard::run(args, input, ctx),
        Cmd::RouteShadow(args) => route_shadow::run(args, input, ctx),
        Cmd::RouteOutcome(args) => route_outcome::run(args, input, ctx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cut_to_boundary_never_splits_a_character() {
        assert_eq!(cut_to_boundary("abcdef", 3), "abc");
        assert_eq!(cut_to_boundary("abc", 10), "abc");
        // "é" is two bytes: cutting inside it backs up to before it.
        assert_eq!(cut_to_boundary("aé", 2), "a");
        assert_eq!(cut_to_boundary("aé", 3), "aé");
        assert_eq!(cut_to_boundary("é", 0), "");
    }

    #[test]
    fn fnv1a64_matches_the_published_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn additional_context_is_cut_to_the_cap() {
        let long = "x".repeat(MAX_CONTEXT_BYTES + 50);
        let out: Value = serde_json::from_str(&additional_context("PostToolUse", &long)).unwrap();
        let text = out["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert_eq!(text.len(), MAX_CONTEXT_BYTES);
    }
}
