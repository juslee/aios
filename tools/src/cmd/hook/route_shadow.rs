//! `aios hook route-shadow`: ask Jev how a subagent dispatch would be routed and
//! log the answer next to what actually happened. Registered on `PreToolUse` for
//! the dispatch tool (`Agent`). Never decides anything; fails open (see `OnError`).

use anyhow::Result;
use clap::Args as ClapArgs;

use super::{Ctx, HookInput};

#[derive(ClapArgs)]
pub struct Args {}

/// The dispatch fields the hook reads; all are optional in the payload.
#[derive(Debug, PartialEq, Eq)]
pub struct Dispatch<'a> {
    pub subagent_type: Option<&'a str>,
    pub description: Option<&'a str>,
    pub prompt: Option<&'a str>,
    pub model: Option<&'a str>,
}

impl<'a> Dispatch<'a> {
    pub fn from_input(input: &'a HookInput) -> Dispatch<'a> {
        Dispatch {
            subagent_type: input.tool_str("subagent_type"),
            description: input.tool_str("description"),
            prompt: input.tool_str("prompt"),
            model: input.tool_str("model"),
        }
    }
}

pub fn run(_args: &Args, input: &HookInput, _ctx: &Ctx) -> Result<Option<String>> {
    let _dispatch = Dispatch::from_input(input);
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::hook::parse_input;

    #[test]
    fn the_dispatch_fields_are_read_leniently() {
        let input = parse_input(
            br#"{"tool_name":"Agent","tool_input":{"subagent_type":"worker","prompt":"do it","model":7}}"#,
        )
        .unwrap();
        assert_eq!(
            Dispatch::from_input(&input),
            Dispatch {
                subagent_type: Some("worker"),
                description: None,
                prompt: Some("do it"),
                model: None,
            }
        );
    }
}
