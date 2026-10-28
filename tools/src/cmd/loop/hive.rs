//! The knowledge-hive notes a round hands the lenses as lessons.
//!
//! The tags of the changed paths (`path_tags`: a glob to a rule 08 tag) select
//! notes under `docs/knowledge/lessons/` and `docs/knowledge/decisions/` whose
//! frontmatter `tags:` share a tag. Notes with `status: final` come first, then
//! the newest (the date in the file name), and the list is capped by
//! `hive_cap`: at most `notes` notes, each cut to `bytes / notes` bytes, so the
//! whole stays within `bytes`. A note that does not fit is recorded in
//! [`Lessons::dropped`], not silently lost. The notes are read from
//! `origin/main` (or `--tree`), so a PR cannot write its own lessons.

use std::collections::BTreeSet;

use super::config::{glob_to_regex, Config};
use super::gather::{cap_bytes, Source};

/// The directories searched, in order.
const DIRS: [&str; 2] = ["docs/knowledge/lessons", "docs/knowledge/decisions"];

/// The most note files read per directory (the newest by name).
const SCAN_CAP: usize = 400;

/// One selected note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// Repository-relative path.
    pub path: String,
    pub status: String,
    pub tags: Vec<String>,
    /// The note's text, cut to its share of the byte cap.
    pub text: String,
    pub truncated: bool,
}

/// The lessons of one round.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lessons {
    /// The tags the changed paths selected, sorted.
    pub tags: Vec<String>,
    pub notes: Vec<Note>,
    /// Paths of matching notes that did not fit the note cap.
    pub dropped: Vec<String>,
    /// Why the notes could not be read at all.
    pub unreadable: Option<String>,
}

impl Lessons {
    /// The text for `{{lessons}}`.
    pub fn render(&self) -> String {
        if self.notes.is_empty() {
            return "(no notes selected)".to_string();
        }
        let mut out = String::new();
        for n in &self.notes {
            out.push_str(&format!(
                "### {} (status {}, tags {})\n{}\n{}\n",
                n.path,
                n.status,
                n.tags.join(", "),
                n.text.trim_end(),
                if n.truncated { "[cut]" } else { "" }
            ));
        }
        if !self.dropped.is_empty() {
            out.push_str(&format!(
                "\nNot included (cap): {}\n",
                self.dropped.join(", ")
            ));
        }
        out
    }
}

/// The rule 08 tags that the changed `paths` select through `path_tags`.
pub fn tags_for(config: &Config, paths: &BTreeSet<String>) -> BTreeSet<String> {
    config
        .path_tags
        .iter()
        .filter(|(glob, _)| {
            glob_to_regex(glob).is_ok_and(|re| paths.iter().any(|p| re.is_match(p)))
        })
        .map(|(_, tag)| tag.clone())
        .collect()
}

/// `(status, tags)` of a note's frontmatter. A note without frontmatter has
/// neither.
pub fn frontmatter(text: &str) -> (String, Vec<String>) {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (String::new(), Vec::new());
    }
    let mut status = String::new();
    let mut tags = Vec::new();
    for line in lines {
        let line = line.trim();
        if line == "---" {
            break;
        }
        if let Some(v) = line.strip_prefix("status:") {
            status = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("tags:") {
            tags = v
                .trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .map(|t| t.trim().trim_matches(['"', '\'']).to_string())
                .filter(|t| !t.is_empty())
                .collect();
        }
    }
    (status, tags)
}

/// Select the notes for the `changed` paths.
pub fn select(source: &Source, config: &Config, changed: &BTreeSet<String>) -> Lessons {
    let wanted = tags_for(config, changed);
    let mut lessons = Lessons {
        tags: wanted.iter().cloned().collect(),
        ..Lessons::default()
    };
    if wanted.is_empty() {
        return lessons;
    }
    let mut candidates: Vec<(Note, String)> = Vec::new();
    for dir in DIRS {
        let names = match source.list(dir) {
            Ok(names) => names,
            Err(e) => {
                lessons.unreadable = Some(format!("{e:#}"));
                continue;
            }
        };
        for name in names
            .iter()
            .rev()
            .filter(|n| n.ends_with(".md") && *n != "README.md")
            .take(SCAN_CAP)
        {
            let path = format!("{dir}/{name}");
            let Ok(text) = source.read(&path) else {
                continue;
            };
            let (status, tags) = frontmatter(&text);
            if !tags.iter().any(|t| wanted.contains(t)) {
                continue;
            }
            candidates.push((
                Note {
                    path,
                    status,
                    tags,
                    text,
                    truncated: false,
                },
                name.clone(),
            ));
        }
    }
    // final first, then newest: file names start with the date.
    candidates.sort_by(|(a, an), (b, bn)| {
        let a_final = a.status == "final";
        let b_final = b.status == "final";
        b_final.cmp(&a_final).then_with(|| bn.cmp(an))
    });
    let per_note = config.hive_cap.bytes / config.hive_cap.notes.max(1);
    for (i, (mut note, _)) in candidates.into_iter().enumerate() {
        if i >= config.hive_cap.notes {
            lessons.dropped.push(note.path);
            continue;
        }
        let kept = cap_bytes(&note.text, per_note).len();
        note.truncated = kept < note.text.len();
        note.text.truncate(kept);
        lessons.notes.push(note);
    }
    lessons
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config::from_json(include_str!("../../../../scripts/agent/loop-config.json"))
            .expect("the committed config is valid")
    }

    fn paths(p: &[&str]) -> BTreeSet<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn paths_select_tags() {
        let tags = tags_for(
            &config(),
            &paths(&["kernel/src/ipc/a.rs", "tools/x.rs", "docs/a.md"]),
        );
        assert_eq!(tags.into_iter().collect::<Vec<_>>(), ["ipc", "tooling"]);
        assert!(tags_for(&config(), &paths(&["docs/a.md"])).is_empty());
    }

    #[test]
    fn frontmatter_reads_status_and_tags() {
        let (s, t) = frontmatter("---\nauthor: x\ntags: [kernel, mmu]\nstatus: final\n---\nbody");
        assert_eq!(s, "final");
        assert_eq!(t, ["kernel", "mmu"]);
        assert_eq!(frontmatter("no frontmatter"), (String::new(), Vec::new()));
        let (_, t) = frontmatter("---\ntags: tooling, ipc\n---\n");
        assert_eq!(t, ["tooling", "ipc"]);
    }

    fn tree_with(notes: &[(&str, &str, &str)]) -> (std::path::PathBuf, Source) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "aios-hive-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(DIRS[0])).expect("mkdir");
        for (name, status, body) in notes {
            let text = format!("---\ntags: [tooling]\nstatus: {status}\n---\n{body}\n");
            std::fs::write(dir.join(DIRS[0]).join(name), text).expect("write a note");
        }
        (dir.clone(), Source::Tree(dir))
    }

    #[test]
    fn final_first_then_newest_and_capped() {
        let (dir, source) = tree_with(&[
            ("2026-01-01-a-old-final.md", "final", "old final"),
            ("2026-03-01-b-new-draft.md", "draft", "new draft"),
            ("2026-02-01-c-mid-final.md", "final", "mid final"),
        ]);
        let mut cfg = config();
        cfg.hive_cap.notes = 2;
        let lessons = select(&source, &cfg, &paths(&["tools/x.rs"]));
        let order: Vec<&str> = lessons.notes.iter().map(|n| n.path.as_str()).collect();
        assert_eq!(
            order,
            [
                "docs/knowledge/lessons/2026-02-01-c-mid-final.md",
                "docs/knowledge/lessons/2026-01-01-a-old-final.md",
            ]
        );
        assert_eq!(
            lessons.dropped,
            ["docs/knowledge/lessons/2026-03-01-b-new-draft.md"]
        );
        assert!(lessons.render().contains("Not included (cap)"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn each_note_is_cut_to_its_share_of_the_byte_cap() {
        let long = "x".repeat(5000);
        let (dir, source) = tree_with(&[("2026-01-01-a.md", "final", &long)]);
        let mut cfg = config();
        cfg.hive_cap.notes = 2;
        cfg.hive_cap.bytes = 1000;
        let lessons = select(&source, &cfg, &paths(&["tools/x.rs"]));
        assert_eq!(lessons.notes.len(), 1);
        assert_eq!(lessons.notes[0].text.len(), 500);
        assert!(lessons.notes[0].truncated);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn no_tag_no_notes_and_a_missing_directory_is_reported() {
        let (dir, source) = tree_with(&[("2026-01-01-a.md", "final", "x")]);
        assert!(select(&source, &config(), &paths(&["docs/a.md"]))
            .notes
            .is_empty());
        // The decisions directory is absent in the tree: reported, lessons still found.
        let lessons = select(&source, &config(), &paths(&["tools/x.rs"]));
        assert_eq!(lessons.notes.len(), 1);
        assert!(lessons.unreadable.is_some());
        let _ = std::fs::remove_dir_all(dir);
    }
}
