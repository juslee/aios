//! The Next phase-doc step section: the phase doc that is in progress (or the first one
//! that is not complete) and its first open task.
//!
//! The task scan is its own port of the script's awk. It does not reuse
//! `docs_check::checks::milestones::is_unchecked_task`, whose semantics differ.

use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use anyhow::Result;

use super::line;
use crate::cmd::checkpoint::scan::lines;

/// `grep -m 1 '^\*\*Status:\*\*' | sed 's/^\*\*Status:\*\* *//'`: also the status line
/// of an interleaved soak's `summary.md`.
pub fn status_of(doc: &[u8]) -> &[u8] {
    const PREFIX: &[u8] = b"**Status:**";
    lines(doc)
        .find(|row| row.starts_with(PREFIX))
        .map(|row| {
            let rest = &row[PREFIX.len()..];
            let start = rest.iter().position(|&b| b != b' ').unwrap_or(rest.len());
            &rest[start..]
        })
        .unwrap_or_default()
}

/// The phase docs, `docs/phases/[0-9]*.md`, in bytewise order.
fn phase_docs(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root.join("docs/phases")) else {
        return Vec::new();
    };
    let mut names: Vec<Vec<u8>> = entries
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name().as_bytes().to_vec())
        .filter(|name| {
            name.len() >= b"0.md".len() && name[0].is_ascii_digit() && name.ends_with(b".md")
        })
        .collect();
    names.sort_unstable();
    names
        .into_iter()
        .map(|name| String::from_utf8_lossy(&name).into_owned())
        .collect()
}

/// The phase doc to report: the first `In Progress`, else the first that is neither
/// `In Progress` nor `Complete`.
fn current_doc(root: &Path) -> Option<(String, Vec<u8>)> {
    let mut candidate: Option<(String, Vec<u8>)> = None;
    for name in phase_docs(root) {
        let doc = std::fs::read(root.join("docs/phases").join(&name)).unwrap_or_default();
        let status = status_of(&doc);
        if status.starts_with(b"In Progress") {
            return Some((name, status.to_vec()));
        }
        if !status.starts_with(b"Complete") && candidate.is_none() {
            candidate = Some((name, status.to_vec()));
        }
    }
    candidate
}

/// The awk over the doc: the milestone and step headings above the first open task box
/// (`- [ ] ...` or `* [ ] ...`, not a struck-through one), as the two output lines;
/// `None` when every box is checked. Also the milestone heading's text.
fn next_task(doc: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut milestone: &[u8] = b"";
    let mut step: &[u8] = b"";
    for row in lines(doc) {
        if let Some(heading) = row.strip_prefix(b"## ") {
            milestone = heading;
            step = b"";
        }
        if row.starts_with(b"### Step ") {
            // awk's substr($0, 5): everything after the `### ` of the heading.
            step = &row[4..];
        }
        let indent = row
            .iter()
            .take_while(|b| matches!(**b, b' ' | b'\t' | b'\r' | 0x0b | 0x0c))
            .count();
        let Some(task) = row[indent..]
            .strip_prefix(b"- [ ] ")
            .or_else(|| row[indent..].strip_prefix(b"* [ ] "))
        else {
            continue;
        };
        if has_struck(row) {
            continue;
        }
        let step = if step.is_empty() {
            &b"(no step heading)"[..]
        } else {
            step
        };
        return Some((
            [&b"- next: "[..], milestone, b" / ", step].concat(),
            [&b"- first open task: "[..], task].concat(),
        ));
    }
    None
}

/// Whether the row holds `[ ] ~~`, the mark of a struck-through task.
fn has_struck(row: &[u8]) -> bool {
    row.windows(6).any(|window| window == b"[ ] ~~")
}

/// The digits after `- next: Milestone `.
fn milestone_number(next: &[u8]) -> Option<Vec<u8>> {
    let rest = next.strip_prefix(b"- next: Milestone ")?;
    let digits: Vec<u8> = rest
        .iter()
        .copied()
        .take_while(u8::is_ascii_digit)
        .collect();
    (!digits.is_empty()).then_some(digits)
}

/// Print the section body, without the pull-request lookup. Returns the number of the
/// milestone the next open task is in (a `## Milestone <n>...` heading), which that
/// lookup needs.
pub fn print(root: &Path, out: &mut dyn Write) -> Result<Option<Vec<u8>>> {
    let Some((name, status)) = current_doc(root) else {
        line(
            out,
            &[b"- no phase doc in progress (all docs/phases/*.md are Complete)"],
        )?;
        return Ok(None);
    };
    line(
        out,
        &[format!("- docs/phases/{name}: ").as_bytes(), &status],
    )?;
    let doc = std::fs::read(root.join("docs/phases").join(&name)).unwrap_or_default();
    let Some((next, task)) = next_task(&doc) else {
        line(out, &[b"- every task box in this doc is checked"])?;
        return Ok(None);
    };
    line(out, &[&next])?;
    line(out, &[&task])?;
    Ok(milestone_number(&next))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_strips_the_marker_and_the_spaces_after_it() {
        assert_eq!(
            status_of(b"# P\n\n**Status:**   In Progress (M3)\nrest"),
            b"In Progress (M3)"
        );
        assert_eq!(status_of(b"**Status:**Complete"), b"Complete");
        assert_eq!(status_of(b"Status: nope"), b"");
        assert_eq!(status_of(b"**Status:**\t Tab"), b"\t Tab");
    }

    #[test]
    fn next_task_finds_the_first_open_box_under_its_headings() {
        let doc = b"## Milestone 41 - Alpha\n\n### Step 3: Do it\n\n- [x] done\n\
                    - [ ] ~~dropped~~\n  * [ ] the open task\n- [ ] later\n";
        let (next, task) = next_task(doc).expect("an open task");
        assert_eq!(next, b"- next: Milestone 41 - Alpha / Step 3: Do it");
        assert_eq!(task, b"- first open task: the open task");
        assert_eq!(milestone_number(&next), Some(b"41".to_vec()));
    }

    #[test]
    fn next_task_without_a_step_heading_and_when_all_are_checked() {
        let (next, _) = next_task(b"## Setup\n- [ ] x\n").expect("an open task");
        assert_eq!(next, b"- next: Setup / (no step heading)");
        assert_eq!(milestone_number(&next), None);
        assert!(next_task(b"## Setup\n- [x] a\n- [x] b\n").is_none());
        // A new milestone heading forgets the step; `Step` needs the `### ` prefix.
        let (next, _) = next_task(b"## A\n### Step 1\n## B\n- [ ] x\n").expect("an open task");
        assert_eq!(next, b"- next: B / (no step heading)");
        // The box needs a space after the bullet and after the brackets.
        assert!(next_task(b"-[ ] x\n- [ ]x\n").is_none());
    }

    #[test]
    fn milestone_number_takes_the_leading_digits() {
        assert_eq!(
            milestone_number(b"- next: Milestone 7x / s"),
            Some(b"7".to_vec())
        );
        assert_eq!(milestone_number(b"- next: Milestone x / s"), None);
        assert_eq!(milestone_number(b"- next: Phase 7"), None);
    }
}
