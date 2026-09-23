//! The docs-check fixture repository (R1 contract, fixture bundle format and variants).
//!
//! The fixture lives in text bundles under `tests/fixtures/docs-check/`: `base.txt`
//! and one `variants/<name>.txt` per variant. A bundle line starting with `@@@ ` is a
//! directive `@@@ <kind> <arg>`; every other line is content of the preceding `file`,
//! `append` or `prepend` directive. Bundles are `.txt` so that the real repository's
//! docs-check, Cargo and Claude Code never see the fixture's Markdown, `CLAUDE.md`,
//! `.claude/` or `.rs` files. `materialize_fixture` turns base plus one variant into a
//! throwaway git repository (removed when the returned `TestRepo` drops).

use super::TestRepo;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// One bundle directive with its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// `@@@ file <path>`: create or replace the file (parent directories created).
    File(String, String),
    /// `@@@ append <path>`: append to the file (created when missing).
    Append(String, String),
    /// `@@@ prepend <path>`: insert before the file's current content.
    Prepend(String, String),
    /// `@@@ delete <path>`: remove the file. Unused in R1; kept for the R4/R5 port fixtures.
    Delete(String),
    /// `@@@ commit <subject>` (base only): an empty commit after `Initial fixture`.
    Commit(String),
    /// `@@@ option <name>` (variants only); the only option is `no-phase-history`.
    Flag(String),
}

/// A fixture variant and the finding keys `aios docs-check --json` reports as new.
#[derive(Debug, Clone, Copy)]
pub struct Variant {
    pub name: &'static str,
    pub expect_new: &'static [&'static str],
}

/// Every variant, verified against scripts/docs/check.py: the base repository, one
/// single-drift variant per check (in CHECK_ORDER), then line-shift, skip and grown.
pub const VARIANTS: &[Variant] = &[
    Variant {
        name: "base",
        expect_new: &[],
    },
    Variant {
        name: "md-links",
        expect_new: &["md-links|docs/kernel/alpha.md|missing.md"],
    },
    Variant {
        name: "section-refs",
        expect_new: &["section-refs|docs/kernel/alpha.md|deadlock-prevention.md §9.9"],
    },
    Variant {
        name: "anchors",
        expect_new: &["anchors|docs/kernel/alpha.md|#no-such-heading"],
    },
    Variant {
        name: "wiki-links",
        expect_new: &["wiki-links|docs/kernel/alpha.md|nonexistent-note"],
    },
    Variant {
        name: "doc-map",
        expect_new: &["doc-map|docs/kernel/gamma.md|unlisted"],
    },
    Variant {
        name: "repo-paths",
        expect_new: &["repo-paths|CLAUDE.md|kernel/src/missing.rs"],
    },
    Variant {
        name: "just-recipes",
        expect_new: &["just-recipes|README.md|nope"],
    },
    Variant {
        name: "test-count",
        expect_new: &["test-count|docs/project/developer-guide.md|claimed:4"],
    },
    Variant {
        name: "lock-order",
        expect_new: &["lock-order|docs/kernel/deadlock-prevention.md|undocumented:GAMMA_LOCK"],
    },
    Variant {
        name: "milestone-status",
        expect_new: &["milestone-status|docs/phases/01-memory.md|M3:unchecked"],
    },
    Variant {
        name: "phase-count",
        expect_new: &["phase-count|README.md|claimed:4"],
    },
    Variant {
        name: "layout",
        expect_new: &["layout|CLAUDE.md|missing:kernel/src/extra.rs"],
    },
    Variant {
        name: "harness-tables",
        expect_new: &["harness-tables|CLAUDE.md|skills-table-stale:ghost"],
    },
    Variant {
        name: "pointer-doctor",
        expect_new: &["pointer-doctor|.claude/agents/worker.md|claude-md:Build Matrix"],
    },
    Variant {
        name: "knowledge-hygiene",
        expect_new: &["knowledge-hygiene|docs/knowledge/lessons/bad-name.md|name"],
    },
    Variant {
        name: "line-shift",
        expect_new: &[],
    },
    Variant {
        name: "skip",
        expect_new: &[],
    },
    Variant {
        name: "grown",
        expect_new: &["md-links|docs/kernel/alpha.md|old-spec.md"],
    },
];

/// `tools/tests/fixtures/docs-check`.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/docs-check")
}

/// Parse a bundle: strip one final `"\n"`, split on `"\n"`; content is the lines joined
/// with `"\n"` plus a final `"\n"`, or `""` when a directive has no content lines.
pub fn parse_bundle(text: &str) -> Vec<Op> {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let mut directives: Vec<(&str, &str, Vec<&str>)> = Vec::new();
    for line in body.split('\n') {
        if let Some(rest) = line.strip_prefix("@@@ ") {
            let (kind, arg) = rest.split_once(' ').unwrap_or((rest, ""));
            directives.push((kind, arg, Vec::new()));
        } else if let Some((_, _, content)) = directives.last_mut() {
            content.push(line);
        } else {
            panic!("bundle line before the first @@@ directive: {line:?}");
        }
    }
    directives
        .into_iter()
        .map(|(kind, arg, lines)| {
            let content = if lines.is_empty() {
                String::new()
            } else {
                format!("{}\n", lines.join("\n"))
            };
            let arg = arg.to_string();
            match kind {
                "file" => Op::File(arg, content),
                "append" => Op::Append(arg, content),
                "prepend" => Op::Prepend(arg, content),
                "delete" | "commit" | "option" => {
                    assert!(
                        lines.is_empty(),
                        "@@@ {kind} {arg} takes no content lines, found {}",
                        lines.len()
                    );
                    match kind {
                        "delete" => Op::Delete(arg),
                        "commit" => Op::Commit(arg),
                        _ => Op::Flag(arg),
                    }
                }
                other => panic!("unknown bundle directive @@@ {other} {arg}"),
            }
        })
        .collect()
}

/// Parse `tests/fixtures/docs-check/<rel>`.
pub fn read_bundle(rel: &str) -> Vec<Op> {
    let path = fixtures_dir().join(rel);
    let text =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    parse_bundle(&text)
}

/// Build the fixture repository for `variant` (`"base"` or a `variants/<name>.txt`):
/// write the base files, commit `Initial fixture`, add one empty commit per base
/// `@@@ commit` subject (unless the variant sets `no-phase-history`), apply the
/// variant's file operations and commit `Fixture variant: <variant>`.
pub fn materialize_fixture(variant: &str) -> TestRepo {
    let base = read_bundle("base.txt");
    let ops = if variant == "base" {
        Vec::new()
    } else {
        read_bundle(&format!("variants/{variant}.txt"))
    };
    let repo = TestRepo::new(&format!("fixture-{variant}"));
    let mut subjects = Vec::new();
    for op in &base {
        match op {
            Op::Commit(subject) => subjects.push(subject.as_str()),
            Op::Flag(name) => panic!("base.txt cannot set @@@ option {name}"),
            _ => apply(&repo, op),
        }
    }
    repo.commit("Initial fixture");
    let mut phase_history = true;
    for op in &ops {
        match op {
            Op::Flag(name) if name == "no-phase-history" => phase_history = false,
            Op::Flag(name) => panic!("variants/{variant}.txt: unknown @@@ option {name}"),
            Op::Commit(subject) => {
                panic!("variants/{variant}.txt: @@@ commit {subject} belongs in base.txt")
            }
            _ => {}
        }
    }
    if phase_history {
        for subject in subjects {
            repo.commit(subject);
        }
    }
    for op in ops.iter().filter(|op| !matches!(op, Op::Flag(_))) {
        apply(&repo, op);
    }
    repo.commit(&format!("Fixture variant: {variant}"));
    repo
}

/// Apply one file operation to the working tree (commits and options are handled by
/// `materialize_fixture`).
fn apply(repo: &TestRepo, op: &Op) {
    match op {
        Op::File(rel, content) => repo.write(rel, content),
        Op::Append(rel, content) => {
            let path = repo.path().join(rel);
            let mut file = fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&path)
                .unwrap_or_else(|e| panic!("cannot open {}: {e}", path.display()));
            file.write_all(content.as_bytes())
                .unwrap_or_else(|e| panic!("cannot append to {}: {e}", path.display()));
        }
        Op::Prepend(rel, content) => {
            let path = repo.path().join(rel);
            let old =
                fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
            let mut new = content.as_bytes().to_vec();
            new.extend_from_slice(&old);
            fs::write(&path, new)
                .unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
        }
        Op::Delete(rel) => {
            let path = repo.path().join(rel);
            fs::remove_file(&path)
                .unwrap_or_else(|e| panic!("cannot delete {}: {e}", path.display()));
        }
        Op::Commit(subject) => panic!("@@@ commit {subject} is applied by materialize_fixture"),
        Op::Flag(name) => panic!("@@@ option {name} is read by materialize_fixture"),
    }
}
