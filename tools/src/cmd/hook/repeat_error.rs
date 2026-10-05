//! `aios hook repeat-error`: notice the same Bash command failing the same way
//! twice. Registered on `PostToolUseFailure` and `PostToolUse`, matcher `Bash`.
//! Fails open (see `OnError`).

use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use regex::Regex;
use serde::{Deserialize, Serialize};

use super::{additional_context, cut_to_boundary, state_key, write_atomic, Ctx, HookInput};

/// Signatures kept per state file; the oldest is evicted past this.
const MAX_ENTRIES: usize = 64;
/// The normalised error is cut to this many bytes before it is compared.
const MAX_ERROR_BYTES: usize = 512;
/// The command is quoted in the nudge cut to this many bytes.
const MAX_QUOTED_COMMAND_BYTES: usize = 200;
/// The nudge starts at this many failures with one signature.
const NUDGE_AT: u32 = 2;
/// State files untouched for this long are deleted on the next write.
const MAX_FILE_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(ClapArgs)]
pub struct Args {}

/// What a payload means to this hook.
#[derive(Debug, PartialEq, Eq)]
pub enum Event<'a> {
    /// A Bash command ran and failed: `PostToolUseFailure`, tool `Bash`.
    Failure { command: &'a str, error: &'a str },
    /// A Bash command succeeded: `PostToolUse`, tool `Bash`.
    Success { command: &'a str },
    /// Any other event or tool.
    Other,
}

impl<'a> Event<'a> {
    pub fn from_input(input: &'a HookInput) -> Event<'a> {
        if input.tool_name.as_deref() != Some("Bash") {
            return Event::Other;
        }
        let Some(command) = input.tool_str("command") else {
            return Event::Other;
        };
        match (input.hook_event_name.as_deref(), input.error.as_deref()) {
            (Some("PostToolUseFailure"), Some(error)) => Event::Failure { command, error },
            (Some("PostToolUse"), _) => Event::Success { command },
            _ => Event::Other,
        }
    }
}

/// One remembered failure: the normalised command and error, and how often the
/// pair has been seen. Entries are ordered oldest first.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    command: String,
    error: String,
    count: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    entries: Vec<Entry>,
}

impl State {
    /// A missing, unreadable or corrupt file reads as empty: the next write
    /// replaces it, so a bad file costs at most the counts it held.
    fn load(path: &Path) -> State {
        std::fs::read(path)
            .ok()
            .and_then(|raw| serde_json::from_slice(&raw).ok())
            .unwrap_or_default()
    }

    /// Count one more occurrence of the pair and return the new count. A pair seen
    /// again moves to the newest end; a new pair evicts the oldest when full.
    fn record(&mut self, command: &str, error: &str) -> u32 {
        let mut entry = match self
            .entries
            .iter()
            .position(|e| e.command == command && e.error == error)
        {
            Some(at) => self.entries.remove(at),
            None => Entry {
                command: command.to_string(),
                error: error.to_string(),
                count: 0,
            },
        };
        entry.count = entry.count.saturating_add(1);
        let count = entry.count;
        self.entries.push(entry);
        if self.entries.len() > MAX_ENTRIES {
            let excess = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..excess);
        }
        count
    }

    /// Drop every signature of `command`; true when any was there.
    fn forget(&mut self, command: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.command != command);
        self.entries.len() != before
    }
}

/// Whitespace runs collapsed to one space, ends trimmed.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn normalize_command(command: &str) -> String {
    collapse_whitespace(command)
}

/// The error with the parts that vary between identical failures masked: `0x`
/// hex numbers become `0x#` and digit runs of four or more become `#`
/// (timestamps, PIDs, durations, addresses). Short numbers such as exit codes and
/// line numbers stay, because they tell failures apart.
pub fn normalize_error(error: &str) -> String {
    static PATTERNS: OnceLock<(Regex, Regex)> = OnceLock::new();
    let (hex, digits) = PATTERNS.get_or_init(|| {
        (
            Regex::new(r"0[xX][0-9a-fA-F]+").expect("the hex pattern is valid"),
            Regex::new(r"[0-9]{4,}").expect("the digits pattern is valid"),
        )
    });
    let collapsed = collapse_whitespace(error);
    let masked = hex.replace_all(&collapsed, "0x#");
    let masked = digits.replace_all(&masked, "#");
    cut_to_boundary(&masked, MAX_ERROR_BYTES).to_string()
}

/// The nudge. It states facts and what the project expects, not commands, so the
/// text is not mistaken for an out-of-band instruction (see the hook contract).
fn nudge(command: &str, count: u32, in_subagent: bool) -> String {
    let quoted = cut_to_boundary(command, MAX_QUOTED_COMMAND_BYTES);
    let ellipsis = if quoted.len() < command.len() {
        "..."
    } else {
        ""
    };
    let lead = format!(
        "`{quoted}{ellipsis}` has failed {count} times with the same error, so running it again unchanged is unlikely to give a different result."
    );
    if in_subagent {
        format!(
            "{lead} The project expects a subagent in this position to stop and report back to its caller with the command, the error and what it changed between attempts, so the lead can get it reviewed."
        )
    } else {
        format!(
            "{lead} The project expects the next step to be asking the code-reviewer agent to diagnose it, given the command, the error and what changed between attempts, before another attempt."
        )
    }
}

/// Delete the state files in `dir` that were last modified before `now - max_age`.
/// Best effort: a file that cannot be inspected or removed is left alone.
fn remove_stale(dir: &Path, now: SystemTime, max_age: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .ok()
            .filter(|meta| meta.is_file())
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > max_age);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

pub fn run(_args: &Args, input: &HookInput, ctx: &Ctx) -> Result<Option<String>> {
    let event = Event::from_input(input);
    if event == Event::Other || input.is_interrupt == Some(true) {
        return Ok(None);
    }
    let dir = ctx.state_dir(input)?.join("repeat-error");
    let path = dir.join(format!("{}.json", state_key(input)));
    match event {
        Event::Failure { command, error } => {
            let mut state = State::load(&path);
            let command = normalize_command(command);
            let count = state.record(&command, &normalize_error(error));
            let json = serde_json::to_vec(&state).context("cannot encode the state")?;
            write_atomic(&path, &json)?;
            remove_stale(&dir, SystemTime::now(), MAX_FILE_AGE);
            Ok((count >= NUDGE_AT).then(|| {
                additional_context(
                    "PostToolUseFailure",
                    &nudge(&command, count, input.agent_id.is_some()),
                )
            }))
        }
        Event::Success { command } => {
            // Most Bash calls succeed, so nothing is written unless a failure of
            // this command is on record.
            let mut state = State::load(&path);
            if state.forget(&normalize_command(command)) {
                let json = serde_json::to_vec(&state).context("cannot encode the state")?;
                write_atomic(&path, &json)?;
                remove_stale(&dir, SystemTime::now(), MAX_FILE_AGE);
            }
            Ok(None)
        }
        Event::Other => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::hook::parse_input;

    fn assert_event(json: &str, expected: Event<'_>) {
        let input = parse_input(json.as_bytes()).unwrap();
        assert_eq!(Event::from_input(&input), expected, "{json}");
    }

    #[test]
    fn a_failed_bash_command_is_a_failure() {
        assert_event(
            r#"{"hook_event_name":"PostToolUseFailure","tool_name":"Bash",
                "tool_input":{"command":"cargo test"},"error":"Exit code 1\nboom"}"#,
            Event::Failure {
                command: "cargo test",
                error: "Exit code 1\nboom",
            },
        );
    }

    #[test]
    fn a_successful_bash_command_is_a_success() {
        assert_event(
            r#"{"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":"ls"}}"#,
            Event::Success { command: "ls" },
        );
    }

    #[test]
    fn other_tools_and_events_are_other() {
        for json in [
            r#"{"hook_event_name":"PostToolUse","tool_name":"Read","tool_input":{"command":"x"}}"#,
            r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"x"}}"#,
            r#"{"hook_event_name":"PostToolUseFailure","tool_name":"Bash","tool_input":{}}"#,
            r#"{}"#,
        ] {
            assert_event(json, Event::Other);
        }
    }

    #[test]
    fn digits_hex_and_long_numbers_are_masked_but_short_ones_kept() {
        assert_eq!(
            normalize_error("pid 12345 at 0xDEADbeef in 1700000000 ms, exit 101, line 42"),
            "pid # at 0x# in # ms, exit 101, line 42"
        );
        assert_eq!(normalize_error("  a \n\t b  "), "a b");
    }

    #[test]
    fn the_normalised_error_is_cut_on_a_character_boundary() {
        let long = "é".repeat(400);
        let cut = normalize_error(&long);
        assert!(cut.len() <= MAX_ERROR_BYTES);
        assert!(cut.chars().all(|c| c == 'é'));
    }

    #[test]
    fn record_evicts_the_oldest_past_the_cap_and_refreshes_a_repeat() {
        let mut state = State::default();
        for n in 0..MAX_ENTRIES {
            assert_eq!(state.record(&format!("c{n}"), "e"), 1);
        }
        assert_eq!(state.record("c0", "e"), 2, "a repeat counts up");
        assert_eq!(state.record("new", "e"), 1);
        assert_eq!(state.entries.len(), MAX_ENTRIES);
        // c0 was refreshed, so c1 was the oldest and went.
        assert!(state.entries.iter().any(|e| e.command == "c0"));
        assert!(!state.entries.iter().any(|e| e.command == "c1"));
    }

    #[test]
    fn forget_drops_every_signature_of_one_command() {
        let mut state = State::default();
        state.record("a", "x");
        state.record("a", "y");
        state.record("b", "x");
        assert!(state.forget("a"));
        assert!(!state.forget("a"));
        assert_eq!(state.entries.len(), 1);
    }

    #[test]
    fn nudge_quotes_a_long_command_cut() {
        let command = "x".repeat(300);
        let text = nudge(&command, 3, false);
        assert!(
            text.contains(&format!("`{}...`", "x".repeat(200))),
            "{text}"
        );
        assert!(text.contains("3 times"));
    }
}
