//! The sections read from the machine: the host QEMU lock, the routing log, the handoff
//! files and the knowledge notes changed since the last session.
//!
//! Each reads from the worktree the script read it from: the lock and the handoffs from
//! the **main worktree**, the routing log from the git common directory, and the
//! knowledge notes from the **toplevel**. `run_from_linked_worktree` pins the split.

use std::ffi::OsString;
use std::io::Write;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use anyhow::Result;

use super::clock::{fmt_epoch, mtime, now};
use super::{line, Ctx};
use crate::cmd::checkpoint::scan::lines;
use crate::git;

/// The main worktree's path joined with `rest` as the script spelled it
/// (`"$MAIN_WT/rest"`): the plain string join, even when the main worktree is unknown.
fn in_main(ctx: &Ctx, rest: &str) -> PathBuf {
    let mut bytes = ctx.main.as_os_str().as_bytes().to_vec();
    bytes.push(b'/');
    bytes.extend_from_slice(rest.as_bytes());
    PathBuf::from(OsString::from_vec(bytes))
}

/// Host QEMU lock: `status` of the main checkout's wrapper, read-only, each line
/// prefixed `- `.
pub fn qemu_lock(ctx: &Ctx, out: &mut dyn Write, err: &mut dyn Write) -> Result<()> {
    let script = "scripts/agent/qemu-lock.sh";
    if !in_main(ctx, script).is_file() {
        return line(
            out,
            &[b"- no scripts/agent/qemu-lock.sh in the main checkout"],
        );
    }
    let cwd = if ctx.main.as_os_str().is_empty() {
        &ctx.root
    } else {
        &ctx.main
    };
    // The exit status is ignored, as it was behind the script's pipe.
    if let Ok(output) = Command::new("sh")
        .args([script, "status"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
    {
        err.write_all(&output.stderr)?;
        for status in lines(&output.stdout) {
            line(out, &[b"- ", status])?;
        }
    }
    Ok(())
}

/// The number of lines of `path` that satisfy `keep`; 0 when it cannot be read.
fn count_lines(path: &Path, keep: impl Fn(&[u8]) -> bool) -> usize {
    std::fs::read(path)
        .map(|data| lines(&data).filter(|row| keep(row)).count())
        .unwrap_or(0)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Routing log: the number of records, never their content. An empty
/// `AIOS_HOOK_STATE_DIR` counts as unset.
pub fn routing_log(ctx: &Ctx, out: &mut dyn Write) -> Result<()> {
    let dir = std::env::var_os("AIOS_HOOK_STATE_DIR")
        .filter(|value| !value.is_empty())
        .map_or_else(|| ctx.common.join("aios-agent/hooks"), PathBuf::from);
    let shadow = count_lines(&dir.join("route-shadow.jsonl"), |row| !row.is_empty());
    let launched = count_lines(&dir.join("route-outcome.jsonl"), |row| {
        contains(row, b"\"kind\":\"launched\"")
    });
    line(
        out,
        &[format!(
            "- route-shadow records: {shadow}; launched dispatches: {launched} (the Jev evaluation needs 200; agent-loop.md, Model routing)"
        )
        .as_bytes()],
    )
}

/// One handoff file, with what `ls -t` sorts by.
struct Handoff {
    path: PathBuf,
    name: Vec<u8>,
    modified: SystemTime,
}

/// The files `handoff-*.md` of `dir`, newest first; equal times by name (bytewise).
fn handoff_files(dir: &Path) -> Vec<Handoff> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<Handoff> = entries
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().as_bytes().to_vec();
            let matches = name.len() >= b"handoff-.md".len()
                && name.starts_with(b"handoff-")
                && name.ends_with(b".md");
            let path = entry.path();
            let modified = std::fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .ok()?;
            matches.then_some(Handoff {
                path,
                name,
                modified,
            })
        })
        .collect();
    files.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.name.cmp(&b.name))
    });
    files
}

/// `head -n 60`: the first 60 lines, the last without a newline when the file has none.
fn head_60(data: &[u8]) -> &[u8] {
    let mut seen = 0;
    for (i, &byte) in data.iter().enumerate() {
        if byte == b'\n' {
            seen += 1;
            if seen == 60 {
                return &data[..=i];
            }
        }
    }
    data
}

/// Handoffs: each file's first 60 lines, newest first. Returns the newest one, which
/// bounds the knowledge window.
pub fn handoffs(ctx: &Ctx, out: &mut dyn Write) -> Result<Option<PathBuf>> {
    let dir = in_main(ctx, ".remember");
    let files = handoff_files(&dir);
    if files.is_empty() {
        out.write_all(b"- no handoff files in ")?;
        out.write_all(dir.as_os_str().as_bytes())?;
        out.write_all(b" (written by /justin:pause)\n")?;
        return Ok(None);
    }
    for file in &files {
        let role = file.name.strip_suffix(b".md").unwrap_or(&file.name);
        let role = role.strip_prefix(b"handoff-").unwrap_or(role);
        line(
            out,
            &[
                role,
                b" (",
                &fmt_epoch(mtime(&file.path), "+%Y-%m-%d %H:%M"),
                b"):",
            ],
        )?;
        out.write_all(b"\n")?;
        out.write_all(head_60(&std::fs::read(&file.path).unwrap_or_default()))?;
        out.write_all(b"\n")?;
    }
    Ok(files.into_iter().next().map(|file| file.path))
}

/// awk's `$NF`: the last blank-separated field of `row`, empty for a blank row.
fn last_field(row: &[u8]) -> &[u8] {
    row.split(|&b| b == b' ' || b == b'\t')
        .rfind(|field| !field.is_empty())
        .unwrap_or_default()
}

/// The sorted, deduplicated note names: the files committed since `since` and the
/// uncommitted ones, without templates and blank lines.
fn knowledge_notes(root: &Path, since: i64) -> Vec<Vec<u8>> {
    let log = git::output(
        root,
        &[
            "log",
            "--all",
            &format!("--since=@{since}"),
            "--name-only",
            "--format=",
            "--",
            "docs/knowledge",
        ],
    )
    .map(|output| output.stdout)
    .unwrap_or_default();
    let status = git::output(root, &["status", "--porcelain", "--", "docs/knowledge"])
        .map(|output| output.stdout)
        .unwrap_or_default();
    let mut notes: Vec<Vec<u8>> = lines(&log).map(<[u8]>::to_vec).collect();
    notes.extend(lines(&status).map(|row| [last_field(row), b" (uncommitted)"].concat()));
    notes.retain(|note| !note.is_empty() && !contains(note, b"/_template.md"));
    notes.sort_unstable();
    notes.dedup();
    notes
}

/// Knowledge notes changed since the last session: since the newest handoff, else the
/// brief marker, else 7 days ago.
pub fn knowledge(
    ctx: &Ctx,
    newest_handoff: Option<&Path>,
    marker: &Path,
    out: &mut dyn Write,
) -> Result<()> {
    let (since, label) = if let Some(handoff) = newest_handoff {
        (mtime(handoff), "last handoff")
    } else if marker.is_file() {
        (mtime(marker), "last brief")
    } else {
        (now() - 7 * 86400, "7 days ago (no handoff or marker yet)")
    };
    line(
        out,
        &[
            format!("- since {label}: ").as_bytes(),
            &fmt_epoch(since, "+%Y-%m-%d %H:%M"),
        ],
    )?;
    let notes = knowledge_notes(&ctx.root, since);
    if notes.is_empty() {
        return line(out, &[b"- none"]);
    }
    for note in notes.iter().take(25) {
        line(out, &[b"- ", note])?;
    }
    if notes.len() > 25 {
        line(
            out,
            &[format!("- ... and {} more", notes.len() - 25).as_bytes()],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_60_keeps_an_unterminated_last_line() {
        let text: String = (1..=65).map(|i| format!("line {i}\n")).collect();
        let head = head_60(text.as_bytes());
        assert_eq!(head.iter().filter(|&&b| b == b'\n').count(), 60);
        assert!(head.ends_with(b"line 60\n"));
        assert_eq!(head_60(b"a\nb"), b"a\nb");
        assert_eq!(head_60(b""), b"");
    }

    #[test]
    fn last_field_is_awk_dollar_nf() {
        assert_eq!(
            last_field(b"?? docs/knowledge/draft.md"),
            b"docs/knowledge/draft.md"
        );
        assert_eq!(last_field(b"R  old.md -> new.md"), b"new.md");
        // A quoted path with a space keeps its last word only.
        assert_eq!(last_field(b"?? \"docs/knowledge/a b.md\""), b"b.md\"");
        assert_eq!(last_field(b"  \t "), b"");
        assert_eq!(last_field(b""), b"");
    }

    #[test]
    fn contains_finds_a_substring() {
        assert!(contains(
            b"say \"kind\":\"launched\" now",
            b"\"kind\":\"launched\""
        ));
        assert!(!contains(
            b"\"kind\": \"launched\"",
            b"\"kind\":\"launched\""
        ));
        assert!(!contains(b"", b"x"));
    }
}
