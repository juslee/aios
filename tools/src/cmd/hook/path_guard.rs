//! `aios hook path-guard`: deny edits under configured repository prefixes.
//! Registered on `PreToolUse` for the edit tools in `.claude/settings.json`, with
//! `--agent-type worker`. Fails closed: an error becomes a deny (see `OnError`).
//!
//! `--agent-type` (repeatable) limits the guard to the named agent types, compared
//! exactly (case-sensitive). Another type, or the main thread of a session not
//! launched with `--agent`, gets no decision, before any path is read. The
//! filter fails closed on identity: a payload with an `agent_id` or an `agent_type`
//! key whose `agent_type` is missing, empty or not a string cannot be told from the
//! guarded agent, so it is denied wherever it writes. An empty `--agent-type` value
//! is a registration mistake and an error, so it denies instead of guarding nobody.
//!
//! The tool name and then the agent type are decided first, so a malformed call from
//! another tool or agent type is skipped, not denied. After that, every path field of
//! the call (`file_path` and `notebook_path`) is checked, and the call is denied if any
//! one is under a prefix. A present field that is not a non-empty string, or a checked
//! tool with no path field, is an error. A path whose first component starts with `~`
//! is denied unexpanded: whether the tool expands it is not known.
//!
//! The target is resolved before it is compared: a relative path is joined to the
//! input `cwd` and walked component by component, each symlink replaced by its
//! target before the next component (a `..` included) is applied, as the OS does, so
//! neither a symlink nor a `..` behind one can hide a denied directory. The
//! repository root comes from the target, not from `cwd`: worktrees nest inside the
//! main checkout, so a root taken from `cwd` would see an edit in a nested worktree
//! as a path under `.claude/worktrees/` and never match `kernel/`. A target that is
//! in no repository gets no decision, but only when no `.git` entry exists above it
//! either. Git's answer is not taken on trust: the root must be the nearest ancestor
//! of the target that holds a `.git` entry, so a `core.worktree` or a rewritten
//! gitfile cannot move it, and a path with a `.git` component anywhere is denied
//! outright, so no edit tool can plant a gitfile below a deny prefix to move the
//! root under it. Bash can still write files, so this is a routing aid, not a
//! sandbox.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::Args as ClapArgs;

use super::{pre_tool_use_deny, Ctx, HookInput};

#[derive(ClapArgs)]
pub struct Args {
    /// Repository-relative directory prefix, ending in `/`, that edits may not touch (repeatable)
    #[arg(long = "deny", value_name = "PREFIX", required = true)]
    pub deny: Vec<String>,
    /// Agent type this guard decides for (repeatable). Without it, every caller is checked.
    #[arg(long = "agent-type", value_name = "NAME")]
    pub agent_type: Vec<String>,
    /// Text appended to the deny reason
    #[arg(long)]
    pub reason: Option<String>,
}

/// The tools whose target path this hook checks.
const CHECKED_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// How many symlinks one path may pass through before the target is treated as
/// unresolvable (the kernel allows 40 per lookup).
const MAX_LINK_DEPTH: usize = 40;

/// The `tool_input` fields that name a file a checked tool writes. `NotebookEdit`
/// writes `notebook_path`; the others write `file_path`. Every one present is
/// checked, whatever the tool, so a second field cannot carry the write past a guard
/// that read only the first.
const PATH_FIELDS: [&str; 2] = ["file_path", "notebook_path"];

/// Whether `input` is a call to a tool whose target path this hook checks.
fn is_checked_tool(input: &HookInput) -> bool {
    input
        .tool_name
        .as_deref()
        .is_some_and(|tool| CHECKED_TOOLS.contains(&tool))
}

/// Every path a checked tool is about to write: each of `PATH_FIELDS` present in
/// `tool_input`. An error when a present field is not a non-empty string, or when no
/// field is present.
fn target_paths(input: &HookInput) -> Result<Vec<&str>> {
    let tool = input.tool_name.as_deref().unwrap_or_default();
    let mut paths = Vec::new();
    for field in PATH_FIELDS {
        if let Some(value) = input.tool_input.get(field) {
            match value.as_str() {
                Some(path) if !path.is_empty() => paths.push(path),
                _ => bail!("{tool} has a {field} that is not a non-empty string"),
            }
        }
    }
    if paths.is_empty() {
        bail!("{tool} carries neither file_path nor notebook_path");
    }
    Ok(paths)
}

pub fn run(args: &Args, input: &HookInput, ctx: &Ctx) -> Result<Option<String>> {
    // Filter first: another tool, or another agent type, never has its path read.
    if !is_checked_tool(input) {
        return Ok(None);
    }
    match scope(&args.agent_type, input)? {
        Scope::Check => {}
        Scope::Skip => return Ok(None),
        Scope::Unidentified => {
            return Ok(Some(pre_tool_use_deny(&format!(
                "this call comes from a subagent (an agent_id or agent_type is set) whose \
                 agent_type is missing, empty or not a string, so path-guard cannot tell whether \
                 it applies (it applies to: {}); denied",
                args.agent_type.join(", ")
            ))))
        }
    }
    let targets = target_paths(input)?;
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
    for target in targets {
        if let Some(mut reason) = check_target(target, &cwd, &prefixes)? {
            if let Some(extra) = args.reason.as_deref().filter(|extra| !extra.is_empty()) {
                reason.push(' ');
                reason.push_str(extra);
            }
            return Ok(Some(pre_tool_use_deny(&reason)));
        }
    }
    Ok(None)
}

/// The deny reason for one target path, or `None` when it is allowed.
fn check_target(target: &str, cwd: &Path, prefixes: &[String]) -> Result<Option<String>> {
    let target = Path::new(target);
    if let Some(Component::Normal(first)) = target.components().next() {
        if first.as_encoded_bytes().starts_with(b"~") {
            // A shell expands a leading `~` to a home directory; this guard would join
            // it to `cwd` as a relative name. Whether the tool expands it is not known,
            // so the path cannot be placed and is denied.
            return Ok(Some(format!(
                "`{}` starts with `~`, which path-guard cannot place (it may mean a home \
                 directory); give an absolute path or one relative to the working directory.",
                target.display()
            )));
        }
    }
    let joined = if target.is_absolute() {
        target.to_path_buf()
    } else {
        if !cwd.is_dir() {
            bail!(
                "{} is relative and the input cwd {} is not a directory",
                target.display(),
                cwd.display()
            );
        }
        cwd.join(target)
    };
    let resolved = resolve(&joined, MAX_LINK_DEPTH)?;
    let Some(root) = repo_root(&existing_dir(&resolved))? else {
        return Ok(None);
    };
    // `repo_root` only returns an ancestor of the target, so a failure here means
    // the filesystem changed underneath the check: an error, not an allow.
    let relative = relative_to(&resolved, &root).with_context(|| {
        format!(
            "{} is not inside the repository root {} git reported for it",
            resolved.display(),
            root.display()
        )
    })?;
    let folded = relative.to_ascii_lowercase();
    let reason = if folded.split('/').any(|part| part == ".git") {
        // A `.git` entry at any depth decides where git puts the repository root:
        // the gitfile of a linked worktree, the git directory, or a gitfile planted
        // in a subdirectory so the root lands below a multi-component deny prefix.
        // No agent task has a reason to write one.
        format!(
            "`{relative}` is part of the repository's git metadata, which this agent may not \
             edit."
        )
    } else if let Some(prefix) = prefixes
        .iter()
        .find(|prefix| folded.starts_with(&prefix.to_ascii_lowercase()))
    {
        format!(
            "`{relative}` is under `{prefix}`, which this agent may not edit: kernel, UEFI stub \
             and shared code goes to kernel-dev. Hand this change back to your caller."
        )
    } else {
        return Ok(None);
    };
    Ok(Some(reason))
}

/// Who this call is for, under `--agent-type`.
enum Scope {
    /// Check the target.
    Check,
    /// Not one of the named agent types: no decision.
    Skip,
    /// A payload that carries identity keys but no usable `agent_type`: deny.
    Unidentified,
}

/// Without `--agent-type` every caller is checked. With it, only the named types
/// are. A usable `agent_type` (a non-empty string) decides alone. Otherwise the
/// payload is the main thread only if it has neither identity key; a present
/// `agent_id` or `agent_type` that is empty, `null` or of another JSON type cannot
/// be identified, so it is denied rather than read as absent. An empty flag value is
/// an error, so a registration mistake denies every checked call instead of guarding
/// nobody.
fn scope(types: &[String], input: &HookInput) -> Result<Scope> {
    if types.is_empty() {
        return Ok(Scope::Check);
    }
    if let Some(bad) = types.iter().find(|t| t.trim().is_empty()) {
        bail!("--agent-type {bad:?} is empty");
    }
    Ok(
        match input.agent_type.as_deref().filter(|t| !t.is_empty()) {
            Some(t) if types.iter().any(|name| name == t) => Scope::Check,
            Some(_) => Scope::Skip,
            None if input.identity_keys.agent_id || input.identity_keys.agent_type => {
                Scope::Unidentified
            }
            None => Scope::Skip,
        },
    )
}

/// A `--deny` value as a directory prefix: a missing trailing `/` is added, so
/// `kernel` cannot match `kernel-notes/`. An empty or absolute prefix, or one with
/// an empty, `.` or `..` component (`kernel//`, `kernel/./`, `docs/../kernel/`), is
/// a registration mistake and an error, which denies every call rather than none:
/// a resolved path never has such components, so the prefix would match nothing.
fn normalise_prefix(raw: &str) -> Result<String> {
    let trimmed = raw.trim_start_matches("./");
    if trimmed.is_empty() || trimmed.starts_with('/') {
        bail!("--deny {raw:?} is not a repository-relative directory prefix");
    }
    let dir = trimmed.strip_suffix('/').unwrap_or(trimmed);
    if dir.split('/').any(|part| matches!(part, "" | "." | "..")) {
        bail!("--deny {raw:?} has an empty, `.` or `..` component and would match no path");
    }
    Ok(format!("{dir}/"))
}

/// The longest ancestor of `path` (itself included) that is an existing directory.
/// `path` is already resolved, so this is where a write would start creating
/// missing directories.
fn existing_dir(path: &Path) -> PathBuf {
    path.ancestors()
        .find(|ancestor| ancestor.is_dir())
        .unwrap_or(Path::new("/"))
        .to_path_buf()
}

/// Whether a failed `git rev-parse` says the directory is in no repository: exit 128
/// with git's "not a git repository (or any of the parent directories)" text, or its
/// mount-point variant "not a git repository (or any parent up to mount point ...)".
/// A pruned worktree says "not a git repository: `<path>`", which is neither.
fn is_no_repository(stderr: &str, code: Option<i32>) -> bool {
    code == Some(128)
        && stderr.contains("not a git repository (or any")
        && stderr.contains("parent")
}

/// The canonical top level of the work tree that contains `dir`, or `None` when
/// `dir` is in no git work tree. That needs both git's own "not a git repository"
/// answer for a directory search and a filesystem with no `.git` entry anywhere
/// above `dir`: anything that makes discovery stop early (a ceiling, a mount
/// boundary, an unreadable `.git`) must not turn a real repository into an allow.
/// Every other failure (a broken worktree link, a bare repository, the `.git`
/// directory itself, git missing) is an error, so the guard fails closed. So is a
/// top level that is not the nearest ancestor of `dir` holding a `.git` entry:
/// `core.worktree`, or a gitfile rewritten to point at a fabricated git directory,
/// could otherwise move the root above the real one.
fn repo_root(dir: &Path) -> Result<Option<PathBuf>> {
    // The message is matched below, so ask git for its untranslated text. The
    // variables that name a repository or limit its discovery are removed so
    // discovery always starts from `dir` and climbs as far as it needs, whatever the
    // hook process inherited (a session started from a git hook has them set).
    let marked = dir
        .ancestors()
        .find(|ancestor| ancestor.join(".git").symlink_metadata().is_ok());
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .env_remove("GIT_DISCOVERY_ACROSS_FILESYSTEM")
        .output()
        .with_context(|| format!("cannot run git in {}", dir.display()))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if is_no_repository(&stderr, out.status.code()) {
            return match marked {
                None => Ok(None),
                Some(marked) => bail!(
                    "git reports {} as in no repository, but {} exists",
                    dir.display(),
                    marked.join(".git").display()
                ),
            };
        }
        bail!(
            "git rev-parse --show-toplevel failed in {}: {}",
            dir.display(),
            stderr.trim_end()
        );
    }
    let top = String::from_utf8(out.stdout).context("the repository root is not valid UTF-8")?;
    let top = top.trim_end_matches(['\n', '\r']);
    if top.is_empty() {
        bail!("git printed no repository root in {}", dir.display());
    }
    let top = resolve(Path::new(top), MAX_LINK_DEPTH)?;
    match marked {
        Some(marked) if relative_to(marked, &top).as_deref() == Some("") => Ok(Some(top)),
        _ => bail!(
            "git reports {} as the repository root for {}, but the nearest .git entry is {}",
            top.display(),
            dir.display(),
            marked.map_or("absent".to_string(), |m| m
                .join(".git")
                .display()
                .to_string())
        ),
    }
}

/// One step of a path walk. A symlink target is split into steps and queued ahead of
/// the rest of the path, so `..` is applied to the directory the walk has really
/// reached, as the OS does.
enum Step {
    Root,
    Current,
    Parent,
    Name(OsString),
}

fn steps(path: &Path) -> Vec<Step> {
    path.components()
        .map(|component| match component {
            Component::Prefix(_) | Component::RootDir => Step::Root,
            Component::CurDir => Step::Current,
            Component::ParentDir => Step::Parent,
            Component::Normal(name) => Step::Name(name.to_os_string()),
        })
        .collect()
}

/// `path` resolved the way the OS resolves it for a write: component by component,
/// each symlink replaced by its target before the next component is applied, so a
/// `..` that follows a link folds against the directory the link points to, not the
/// link's lexical parent. Components that do not exist yet are appended, and a
/// dangling symlink is followed to where a write would create the file. At most
/// `max_links` symlinks are followed in total. A `..` after a component that does not
/// exist is an error (the OS fails that lookup with ENOENT), so the guard denies it.
fn resolve(path: &Path, max_links: usize) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("{} is not an absolute path", path.display());
    }
    let mut queue: VecDeque<Step> = steps(path).into();
    let mut resolved = PathBuf::new();
    let mut missing = false;
    let mut links = 0;
    while let Some(step) = queue.pop_front() {
        match step {
            Step::Root => {
                resolved = PathBuf::from("/");
                missing = false;
            }
            Step::Current => {}
            Step::Parent => {
                if missing {
                    bail!(
                        "{} has a `..` after a component that does not exist",
                        path.display()
                    );
                }
                resolved.pop();
            }
            Step::Name(name) => {
                let next = resolved.join(&name);
                if missing {
                    resolved = next;
                    continue;
                }
                match std::fs::symlink_metadata(&next) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        if links == max_links {
                            bail!("{} passes through too many symlinks", path.display());
                        }
                        links += 1;
                        let target = std::fs::read_link(&next)
                            .with_context(|| format!("cannot resolve {}", next.display()))?;
                        // A relative target is relative to `resolved`, the directory
                        // that really holds the link, because the link's own name was
                        // not appended.
                        for step in steps(&target).into_iter().rev() {
                            queue.push_front(step);
                        }
                    }
                    // Canonicalising an entry that is not a link only normalises its
                    // spelling (a case-insensitive volume reports the stored case).
                    Ok(_) => resolved = std::fs::canonicalize(&next).unwrap_or(next),
                    Err(_) => {
                        missing = true;
                        resolved = next;
                    }
                }
            }
        }
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

    fn targets(json: &str) -> Result<Vec<String>> {
        let input = parse_input(json.as_bytes()).unwrap();
        target_paths(&input).map(|paths| paths.into_iter().map(str::to_string).collect())
    }

    #[test]
    fn both_no_repository_wordings_are_recognised() {
        let parents = "fatal: not a git repository (or any of the parent directories): .git\n";
        let mount = "fatal: not a git repository (or any parent up to mount point /mnt/x)\n\
                     Stopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set).\n";
        let pruned = "fatal: not a git repository: '/r/.git/worktrees/gone'\n";
        assert!(is_no_repository(parents, Some(128)));
        assert!(is_no_repository(mount, Some(128)));
        assert!(!is_no_repository(pruned, Some(128)));
        assert!(!is_no_repository(parents, Some(1)));
        assert!(!is_no_repository(parents, None));
    }

    #[test]
    fn checked_tools_yield_their_path() {
        let got = targets(r#"{"tool_name":"Edit","tool_input":{"file_path":"/r/a.rs"}}"#);
        assert_eq!(got.unwrap(), ["/r/a.rs"]);
        let got =
            targets(r#"{"tool_name":"NotebookEdit","tool_input":{"notebook_path":"/r/n.ipynb"}}"#);
        assert_eq!(got.unwrap(), ["/r/n.ipynb"]);
    }

    #[test]
    fn every_present_path_field_is_yielded() {
        let got = targets(
            r#"{"tool_name":"Write","tool_input":{"file_path":"/r/a","notebook_path":"/r/b"}}"#,
        );
        assert_eq!(got.unwrap(), ["/r/a", "/r/b"]);
    }

    #[test]
    fn a_checked_tool_without_a_path_is_an_error() {
        let err = targets(r#"{"tool_name":"MultiEdit","tool_input":{}}"#).unwrap_err();
        assert!(err.to_string().contains("MultiEdit"), "{err}");
    }

    #[test]
    fn a_present_field_that_is_not_a_non_empty_string_is_an_error() {
        for bad in ["5", "null", "\"\"", "[\"/r/a\"]"] {
            let json = format!(
                r#"{{"tool_name":"Edit","tool_input":{{"file_path":"/r/a","notebook_path":{bad}}}}}"#
            );
            assert!(targets(&json).is_err(), "{bad}");
        }
    }

    #[test]
    fn only_checked_tools_are_checked() {
        let read = parse_input(br#"{"tool_name":"Read","tool_input":{}}"#).unwrap();
        assert!(!is_checked_tool(&read));
        assert!(!is_checked_tool(&parse_input(b"{}").unwrap()));
        let edit = parse_input(br#"{"tool_name":"Edit","tool_input":{}}"#).unwrap();
        assert!(is_checked_tool(&edit));
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
        assert_eq!(normalise_prefix("kernel/arch").unwrap(), "kernel/arch/");
        for bad in [
            "kernel//",
            "kernel/./",
            "docs/../kernel/",
            "kernel/..",
            ".",
            "./.",
            "//",
        ] {
            assert!(normalise_prefix(bad).is_err(), "{bad}");
        }
    }
}
