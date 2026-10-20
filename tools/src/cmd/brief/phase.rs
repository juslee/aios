//! The Next phase-doc step section: the phase doc that is in progress (or the first one
//! that is not complete) and its first open task.
//!
//! The task scan is its own port of the script's awk. It does not reuse
//! `docs_check::checks::milestones::is_unchecked_task`, whose semantics differ.

use std::ffi::OsStr;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

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

/// The file names in `docs/phases` under `root`, as bytes: a name that is not UTF-8 is
/// kept as it is.
fn list_phases(root: &Path) -> Vec<Vec<u8>> {
    let Ok(entries) = std::fs::read_dir(root.join("docs/phases")) else {
        return Vec::new();
    };
    entries
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name().as_bytes().to_vec())
        .collect()
}

/// The path of the phase doc `name`.
fn doc_path(root: &Path, name: &[u8]) -> PathBuf {
    root.join("docs/phases").join(OsStr::from_bytes(name))
}

/// The content of the phase doc `name` under `root`; empty when it cannot be read.
fn read_doc(root: &Path, name: &[u8]) -> Vec<u8> {
    std::fs::read(doc_path(root, name)).unwrap_or_default()
}

/// The phase docs among `names`, `[0-9]*.md`, in bytewise order. The names stay bytes: a
/// name that is not UTF-8 is read and printed as it is.
fn phase_docs(mut names: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    names.retain(|name| {
        name.len() >= b"0.md".len() && name[0].is_ascii_digit() && name.ends_with(b".md")
    });
    names.sort_unstable();
    names
}

/// The phase doc to report: the first `In Progress`, else the first that is neither
/// `In Progress` nor `Complete`. `read` returns the content of a doc by name.
fn current_doc(names: Vec<Vec<u8>>, read: &dyn Fn(&[u8]) -> Vec<u8>) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut candidate: Option<(Vec<u8>, Vec<u8>)> = None;
    for name in phase_docs(names) {
        let doc = read(&name);
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
    print_from(list_phases(root), &|name| read_doc(root, name), out)
}

/// `print` over the names `docs/phases` lists and the reader of a doc by name; both are
/// seams for the unit test, which feeds a name that no file system need accept.
fn print_from(
    names: Vec<Vec<u8>>,
    read: &dyn Fn(&[u8]) -> Vec<u8>,
    out: &mut dyn Write,
) -> Result<Option<Vec<u8>>> {
    let Some((name, status)) = current_doc(names, read) else {
        line(
            out,
            &[b"- no phase doc in progress (all docs/phases/*.md are Complete)"],
        )?;
        return Ok(None);
    };
    line(out, &[b"- docs/phases/", &name, b": ", &status])?;
    let doc = read(&name);
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
    fn a_phase_doc_name_that_is_not_utf8_is_read_and_printed_as_it_is() {
        let root = Path::new("/repo");
        let name: &[u8] = b"07-caf\xe9.md";
        assert_eq!(
            doc_path(root, name).as_os_str().as_bytes(),
            b"/repo/docs/phases/07-caf\xe9.md"
        );
        // No file system: the listing and the reader are fed the bytes.
        let names = vec![b"README.txt".to_vec(), name.to_vec(), b"x.md".to_vec()];
        let read = |wanted: &[u8]| {
            assert_eq!(wanted, name, "only the phase doc is read");
            b"**Status:** In Progress\n## Milestone 9\n- [ ] the task\n".to_vec()
        };
        let mut out = Vec::new();
        let milestone = print_from(names, &read, &mut out).expect("print the section");
        assert_eq!(milestone, Some(b"9".to_vec()));
        assert_eq!(
            out,
            b"- docs/phases/07-caf\xe9.md: In Progress\n- next: Milestone 9 / (no step heading)\n- first open task: the task\n"
        );
    }

    #[test]
    fn phase_docs_keeps_digit_led_md_names_in_bytewise_order() {
        let names = vec![
            b"10-b.md".to_vec(),
            b"README.md".to_vec(),
            b"2-a.md".to_vec(),
            b"3.txt".to_vec(),
            b".md".to_vec(),
            b"1.md".to_vec(),
        ];
        assert_eq!(
            phase_docs(names),
            vec![b"1.md".to_vec(), b"10-b.md".to_vec(), b"2-a.md".to_vec()]
        );
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
