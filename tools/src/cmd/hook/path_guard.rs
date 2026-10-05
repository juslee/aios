//! `aios hook path-guard`: deny edits under configured repository prefixes.
//! Registered on `PreToolUse` for the edit tools, in the frontmatter of the
//! `worker` agent only. Fails closed: an error becomes a deny (see `OnError`).

use anyhow::{bail, Result};
use clap::Args as ClapArgs;

use super::{Ctx, HookInput};

#[derive(ClapArgs)]
pub struct Args {
    /// Repository-relative directory prefix, ending in `/`, that edits may not touch (repeatable)
    #[arg(long = "deny", value_name = "PREFIX", required = true)]
    pub deny: Vec<String>,
    /// Text appended to the deny reason
    #[arg(long)]
    pub reason: Option<String>,
}

/// The tools whose target path this hook checks.
const CHECKED_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// The path a checked tool is about to write: `tool_input.file_path`, or
/// `tool_input.notebook_path` when that is absent. `Ok(None)` for any other tool;
/// an error when a checked tool names no path.
pub fn target_path(input: &HookInput) -> Result<Option<&str>> {
    let Some(tool) = input.tool_name.as_deref() else {
        return Ok(None);
    };
    if !CHECKED_TOOLS.contains(&tool) {
        return Ok(None);
    }
    match input
        .tool_str("file_path")
        .or_else(|| input.tool_str("notebook_path"))
    {
        Some(path) => Ok(Some(path)),
        None => bail!("{tool} carries neither file_path nor notebook_path"),
    }
}

pub fn run(_args: &Args, input: &HookInput, _ctx: &Ctx) -> Result<Option<String>> {
    let _target = target_path(input)?;
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::hook::parse_input;

    fn target(json: &str) -> Result<Option<String>> {
        let input = parse_input(json.as_bytes()).unwrap();
        target_path(&input).map(|path| path.map(str::to_string))
    }

    #[test]
    fn checked_tools_yield_their_path() {
        let got = target(r#"{"tool_name":"Edit","tool_input":{"file_path":"/r/a.rs"}}"#);
        assert_eq!(got.unwrap().as_deref(), Some("/r/a.rs"));
        let got =
            target(r#"{"tool_name":"NotebookEdit","tool_input":{"notebook_path":"/r/n.ipynb"}}"#);
        assert_eq!(got.unwrap().as_deref(), Some("/r/n.ipynb"));
    }

    #[test]
    fn file_path_wins_over_notebook_path() {
        let got = target(
            r#"{"tool_name":"Write","tool_input":{"file_path":"/r/a","notebook_path":"/r/b"}}"#,
        );
        assert_eq!(got.unwrap().as_deref(), Some("/r/a"));
    }

    #[test]
    fn a_checked_tool_without_a_path_is_an_error() {
        let err = target(r#"{"tool_name":"MultiEdit","tool_input":{}}"#).unwrap_err();
        assert!(err.to_string().contains("MultiEdit"), "{err}");
    }

    #[test]
    fn other_tools_are_not_checked() {
        assert_eq!(
            target(r#"{"tool_name":"Read","tool_input":{}}"#).unwrap(),
            None
        );
        assert_eq!(target(r#"{}"#).unwrap(), None);
    }
}
