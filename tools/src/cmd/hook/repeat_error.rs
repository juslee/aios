//! `aios hook repeat-error`: notice the same Bash command failing the same way
//! twice. Registered on `PostToolUseFailure` and `PostToolUse`, matcher `Bash`.
//! Fails open (see `OnError`).

use anyhow::Result;
use clap::Args as ClapArgs;

use super::{Ctx, HookInput};

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

pub fn run(_args: &Args, input: &HookInput, _ctx: &Ctx) -> Result<Option<String>> {
    let _event = Event::from_input(input);
    Ok(None)
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
}
