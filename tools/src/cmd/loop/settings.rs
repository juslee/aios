//! The settings file every stage runs under (`claude --settings <file>`).
//!
//! It is built from the `origin/main` blob of `.claude/settings.json`, never
//! from the PR's working tree, so a PR cannot loosen its own reviewer. It holds
//! exactly three things, and nothing else (no `env`, no other hook):
//! - `enabledPlugins`: every plugin main names, set to `false`;
//! - `hooks.PreToolUse`: main's guard, as one entry with an absolute path;
//! - `permissions`: an empty `allow`, and main's `ask` and `deny`.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};

use super::git::git_in;
use super::state::State;

/// The script main's guard hook runs.
const GUARD_SCRIPT: &str = "git-push-guard.py";

/// The tools the guard watches.
const GUARD_MATCHER: &str = "Bash|Monitor|Agent|EnterWorktree";

/// `git show origin/main:.claude/settings.json`, in `main`.
fn main_blob(main: &Path) -> Result<Value> {
    let mut cmd = git_in(main);
    cmd.args(["show", "origin/main:.claude/settings.json"]);
    let out = cmd
        .output()
        .with_context(|| format!("cannot run git in {}", main.display()))?;
    if !out.status.success() {
        bail!(
            "git show origin/main:.claude/settings.json failed: {}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .next()
                .unwrap_or("")
        );
    }
    serde_json::from_slice(&out.stdout).context("origin/main:.claude/settings.json is not JSON")
}

/// Whether `settings` has a `PreToolUse` command that runs the guard script.
fn has_guard(settings: &Value) -> bool {
    settings["hooks"]["PreToolUse"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|entry| entry["hooks"].as_array().into_iter().flatten())
        .any(|hook| {
            hook["command"]
                .as_str()
                .is_some_and(|c| c.contains(GUARD_SCRIPT))
        })
}

/// `settings[section][key]` as a list (missing is empty; any other shape is an
/// error).
fn list_of(settings: &Value, key: &str) -> Result<Value> {
    match &settings["permissions"][key] {
        Value::Null => Ok(json!([])),
        list @ Value::Array(_) => Ok(list.clone()),
        _ => bail!("origin/main:.claude/settings.json: permissions.{key} is not a list"),
    }
}

/// The stage settings built from main's settings `blob`. Exit 2 (an error
/// naming the guard) when `blob` has no guard entry.
pub fn build(blob: &Value, main: &Path) -> Result<Value> {
    if !has_guard(blob) {
        bail!(
            "origin/main:.claude/settings.json has no PreToolUse command that runs {GUARD_SCRIPT}; \
             refusing to run a stage without the guard"
        );
    }
    let main = main
        .to_str()
        .filter(|m| !m.contains('"'))
        .ok_or_else(|| anyhow!("the main checkout path is not usable in a hook command"))?;
    let plugins: Map<String, Value> = blob["enabledPlugins"]
        .as_object()
        .map(|plugins| {
            plugins
                .keys()
                .map(|name| (name.clone(), Value::Bool(false)))
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({
        "enabledPlugins": plugins,
        "hooks": {
            "PreToolUse": [{
                "matcher": GUARD_MATCHER,
                "hooks": [{
                    "type": "command",
                    "command": format!("/usr/bin/python3 \"{main}/.claude/hooks/{GUARD_SCRIPT}\""),
                }],
            }],
        },
        "permissions": {
            "allow": [],
            "ask": list_of(blob, "ask")?,
            "deny": list_of(blob, "deny")?,
        },
    }))
}

/// Build the stage settings of PR `pr` from `origin/main` and write them to
/// `pr-<n>/stage-settings.json`, mode 0600. Returns the file's path.
pub fn materialise(main: &Path, state: &State, pr: u64) -> Result<PathBuf> {
    let settings = build(&main_blob(main)?, main)?;
    let path = state.stage_settings_file(pr);
    let dir = path
        .parent()
        .context("the settings file has no directory")?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .create(dir)
        .with_context(|| format!("cannot create {}", dir.display()))?;
    let mut text = serde_json::to_string_pretty(&settings).context("cannot encode the settings")?;
    text.push('\n');
    let tmp = dir.join(format!(".stage-settings.json.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .with_context(|| format!("cannot write {}", tmp.display()))
        .and_then(|()| {
            std::fs::rename(&tmp, &path)
                .with_context(|| format!("cannot rename {} to {}", tmp.display(), path.display()))
        });
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written?;
    Ok(path)
}
