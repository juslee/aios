//! `aios hook path-guard`: deny edits under configured repository prefixes.
//! Registered on `PreToolUse` for the edit tools, in the frontmatter of the
//! `worker` agent only. Fails closed: an error becomes a deny (see `OnError`).
//!
//! The target is resolved before it is compared: a relative path is joined to the
//! input `cwd`, `.` and `..` are folded lexically, and the longest existing
//! ancestor is canonicalised so a symlink cannot hide a denied directory. A target
//! outside the repository gets no decision. Bash can still write files, so this is
//! a routing aid, not a sandbox.

use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Args as ClapArgs;

use super::{pre_tool_use_deny, Ctx, HookInput};
use crate::proc;

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

/// How many symlinks a dangling-link chain may pass through before the target is
/// treated as unresolvable (the kernel allows 40 per lookup).
const MAX_LINK_DEPTH: usize = 40;

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

pub fn run(args: &Args, input: &HookInput, ctx: &Ctx) -> Result<Option<String>> {
    let Some(target) = target_path(input)? else {
        return Ok(None);
    };
    let prefixes = args
        .deny
        .iter()
        .map(|raw| normalise_prefix(raw))
        .collect::<Result<Vec<_>>>()?;
    let cwd = input
        .cwd
        .as_deref()
        .map(PathBuf::from)
        .or_else(|| ctx.process_cwd.clone())
        .context("the hook input has no cwd and the process has no working directory")?;
    let root = repo_root(&cwd)?;
    let resolved = resolve(&cwd.join(target), MAX_LINK_DEPTH)?;
    let Some(relative) = relative_to(&resolved, &root) else {
        return Ok(None);
    };
    let folded = relative.to_ascii_lowercase();
    let Some(prefix) = prefixes
        .iter()
        .find(|prefix| folded.starts_with(&prefix.to_ascii_lowercase()))
    else {
        return Ok(None);
    };
    let mut reason = format!(
        "`{relative}` is under `{prefix}`, which this agent may not edit: kernel, UEFI stub and \
         shared code goes to kernel-dev. Hand this change back to your caller."
    );
    if let Some(extra) = args.reason.as_deref().filter(|extra| !extra.is_empty()) {
        reason.push(' ');
        reason.push_str(extra);
    }
    Ok(Some(pre_tool_use_deny(&reason)))
}

/// A `--deny` value as a directory prefix: a missing trailing `/` is added, so
/// `kernel` cannot match `kernel-notes/`. An empty or absolute prefix is a
/// registration mistake and an error, which denies every call rather than none.
fn normalise_prefix(raw: &str) -> Result<String> {
    let trimmed = raw.trim_start_matches("./");
    if trimmed.is_empty() || trimmed.starts_with('/') {
        bail!("--deny {raw:?} is not a repository-relative directory prefix");
    }
    Ok(if trimmed.ends_with('/') {
        trimmed.to_string()
    } else {
        format!("{trimmed}/")
    })
}

/// The canonical top level of the repository that contains `cwd`.
fn repo_root(cwd: &Path) -> Result<PathBuf> {
    let out = proc::capture("git", &["rev-parse", "--show-toplevel"], cwd)?;
    if !out.status.success() {
        bail!(
            "git rev-parse --show-toplevel failed in {}: {}",
            cwd.display(),
            String::from_utf8_lossy(&out.stderr).trim_end()
        );
    }
    let top = String::from_utf8(out.stdout).context("the repository root is not valid UTF-8")?;
    let top = top.trim_end_matches(['\n', '\r']);
    if top.is_empty() {
        bail!("git printed no repository root in {}", cwd.display());
    }
    resolve(Path::new(top), MAX_LINK_DEPTH)
}

/// Fold `.` and `..` out of `path` without touching the filesystem. A `..` at the
/// root stays at the root.
fn normalise_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(
                    out.components().next_back(),
                    None | Some(Component::RootDir | Component::Prefix(_))
                ) {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `path` with `.` and `..` folded and symlinks resolved as far as it exists: the
/// longest existing ancestor is canonicalised and the components that do not exist
/// yet are appended. A dangling symlink is followed to where a write would create
/// the file, up to `depth` links.
fn resolve(path: &Path, depth: usize) -> Result<PathBuf> {
    let path = normalise_lexically(path);
    if !path.is_absolute() {
        bail!("{} is not an absolute path", path.display());
    }
    let mut base = path.as_path();
    let mut missing = Vec::new();
    let mut resolved = loop {
        match std::fs::canonicalize(base) {
            Ok(real) => break real,
            Err(err) => {
                if std::fs::symlink_metadata(base).is_ok() {
                    // The entry exists but does not resolve: a dangling or looping
                    // symlink, or a lookup that is not permitted.
                    if depth == 0 {
                        bail!("{} passes through too many symlinks", path.display());
                    }
                    let link = std::fs::read_link(base)
                        .with_context(|| format!("cannot resolve {}: {err}", base.display()))?;
                    let parent = base.parent().unwrap_or(base);
                    break resolve(&parent.join(link), depth - 1)?;
                }
                let (Some(parent), Some(name)) = (base.parent(), base.file_name()) else {
                    bail!("cannot resolve {}: {err}", path.display());
                };
                missing.push(name);
                base = parent;
            }
        }
    };
    for name in missing.into_iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

/// `path` relative to `root` as a `/`-joined string, or `None` when it is outside.
/// Components are compared ASCII case-insensitively, as a macOS volume would.
fn relative_to(path: &Path, root: &Path) -> Option<String> {
    let mut path_parts = path.components();
    for root_part in root.components() {
        let part = path_parts.next()?;
        let same = part
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&root_part.as_os_str().to_string_lossy());
        if !same {
            return None;
        }
    }
    let parts: Vec<_> = path_parts
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(parts.join("/"))
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

    #[test]
    fn lexical_normalisation_folds_dots_and_stops_at_the_root() {
        let n = |p: &str| normalise_lexically(Path::new(p));
        assert_eq!(n("/r/docs/../kernel/./x.rs"), Path::new("/r/kernel/x.rs"));
        assert_eq!(n("/../../a"), Path::new("/a"));
        assert_eq!(n("/a/b/.."), Path::new("/a"));
    }

    #[test]
    fn relative_to_is_component_wise_and_case_insensitive() {
        let root = Path::new("/r/Repo");
        assert_eq!(
            relative_to(Path::new("/r/repo/Kernel/x.rs"), root).as_deref(),
            Some("Kernel/x.rs")
        );
        assert_eq!(relative_to(Path::new("/r/Repo"), root).as_deref(), Some(""));
        assert_eq!(relative_to(Path::new("/r/Repo2/x"), root), None);
        assert_eq!(relative_to(Path::new("/r"), root), None);
    }

    #[test]
    fn prefixes_gain_a_trailing_slash_and_reject_nonsense() {
        assert_eq!(normalise_prefix("kernel/").unwrap(), "kernel/");
        assert_eq!(normalise_prefix("./kernel").unwrap(), "kernel/");
        assert!(normalise_prefix("").is_err());
        assert!(normalise_prefix("/kernel/").is_err());
    }
}
