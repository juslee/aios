//! `layout` (check.py L1020-1082 at 33c6b3d): the `kernel/src` and `shared/src` directories
//! and top-level modules against the CLAUDE.md Workspace Layout tree and the
//! `kernel/src/<dir>/` lines of rule 05.
//!
//! Accepted divergence from check.py (no tracked file reaches it): regex `\s`
//! (`TREE_DIR_RE`, `TREE_PREFIX_RE`) does not match U+001C..U+001F here, where
//! Python's `\s` does.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::cmd::docs_check::checks::Check;
use crate::cmd::docs_check::markdown::section_body;
use crate::cmd::docs_check::model::Finding;
use crate::cmd::docs_check::repo::Repo;
use crate::paths::basename;

const CHECK: &str = "layout";
const CLAUDE_MD: &str = "CLAUDE.md";
const RULE_05: &str = ".claude/rules/05-file-placement.md";
/// A tree entry line (`├── `, `└── `) contains this box-drawing run.
const TREE_MARK: &str = "──";

/// R49: the Workspace Layout section of CLAUDE.md.
static LAYOUT_START_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^## Workspace Layout").expect("valid regex"));
static LAYOUT_STOP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^## ").expect("valid regex"));
/// R50: a directory entry `── name/`.
static TREE_DIR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"──\s+([A-Za-z0-9_.-]+)/").expect("valid regex"));
/// R51: module names after `(top-level)` and on its continuation lines.
static MODULE_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[a-z_][a-z0-9_.]*").expect("valid regex"));
/// R52: the tree-drawing indent of a continuation line (also used by harness-tables).
pub(crate) static TREE_PREFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[│\s]+").expect("valid regex"));
/// R53: `kernel/src/<dir>/` at the start of a line of rule 05.
static RULE_DIR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^kernel/src/([a-z0-9_]+)/").expect("valid regex"));

/// kernel/src and shared/src modules vs CLAUDE.md layout and rule 05.
pub struct Layout;

/// check.py L1020-1022: the lines of CLAUDE.md's `## Workspace Layout` section
/// (fences included), without line numbers.
pub fn layout_block(repo: &Repo) -> Vec<String> {
    let text = repo.text(CLAUDE_MD);
    section_body(&text, &LAYOUT_START_RE, &LAYOUT_STOP_RE)
        .into_iter()
        .map(|(_, line)| line.to_string())
        .collect()
}

/// check.py L1025-1051: (directory names, top-level module names) listed
/// between the tree entry naming `start` and the next tree entry naming `stop`.
pub fn tree_entries(
    lines: &[String],
    start: &str,
    stop: &str,
) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut dirs = BTreeSet::new();
    let mut mods = BTreeSet::new();
    let mut inside = false;
    let mut collecting = false;
    for line in lines {
        let entry = line.contains(TREE_MARK);
        if line.contains(start) && entry {
            inside = true;
            continue;
        }
        if inside && line.contains(stop) && entry {
            break;
        }
        if !inside {
            continue;
        }
        if entry {
            collecting = false;
            if let Some(caps) = TREE_DIR_RE.captures(line) {
                dirs.insert(caps[1].to_string());
            }
            if let Some((_, rest)) = line.split_once("(top-level)") {
                collecting = true;
                add_module_names(&mut mods, rest);
            }
        } else if collecting {
            add_module_names(&mut mods, &TREE_PREFIX_RE.replace(line, ""));
        }
    }
    (dirs, mods)
}

/// Every R51 name in `text`, with a trailing `.rs` removed.
fn add_module_names(mods: &mut BTreeSet<String>, text: &str) {
    for m in MODULE_NAME_RE.find_iter(text) {
        let name = m.as_str();
        mods.insert(name.strip_suffix(".rs").unwrap_or(name).to_string());
    }
}

/// `f.split("/")[2]` for tracked files at least one directory below `prefix`.
fn source_dirs(files: &[String], prefix: &str) -> BTreeSet<String> {
    files
        .iter()
        .filter(|f| f.starts_with(prefix) && f.matches('/').count() >= 3)
        .filter_map(|f| f.split('/').nth(2))
        .map(str::to_string)
        .collect()
}

/// Basenames without `.rs` of the `.rs` files directly in `prefix`.
fn source_modules(files: &[String], prefix: &str) -> BTreeSet<String> {
    files
        .iter()
        .filter(|f| f.starts_with(prefix) && f.matches('/').count() == 2)
        .filter_map(|f| basename(f).strip_suffix(".rs"))
        .map(str::to_string)
        .collect()
}

impl Check for Layout {
    fn name(&self) -> &'static str {
        CHECK
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        let files = repo.files();
        let kdirs = source_dirs(files, "kernel/src/");
        let kmods = source_modules(files, "kernel/src/");
        let sdirs = source_dirs(files, "shared/src/");
        let smods = source_modules(files, "shared/src/");
        let block = layout_block(repo);
        let (ldirs, lmods) = tree_entries(&block, "kernel/src/", "shared/src/");
        let (sd, sm) = tree_entries(&block, "shared/src/", "uefi-stub/");
        let groups = [
            ("kernel dir", &kdirs, &ldirs, "kernel/src/", "/"),
            ("kernel module", &kmods, &lmods, "kernel/src/", ".rs"),
            ("shared dir", &sdirs, &sd, "shared/src/", "/"),
            ("shared module", &smods, &sm, "shared/src/", ".rs"),
        ];
        let mut out = Vec::new();
        for (label, actual, listed, prefix, suffix) in groups {
            for name in actual.difference(listed) {
                let path = format!("{prefix}{name}{suffix}");
                out.push(Finding::new(
                    CHECK,
                    CLAUDE_MD,
                    format!("missing:{path}"),
                    format!("Workspace Layout does not list {label} {path}"),
                    0,
                ));
            }
            for name in listed.difference(actual) {
                let path = format!("{prefix}{name}{suffix}");
                out.push(Finding::new(
                    CHECK,
                    CLAUDE_MD,
                    format!("stale:{path}"),
                    format!("Workspace Layout lists {path}, which does not exist"),
                    0,
                ));
            }
        }
        if repo.is_file(RULE_05) {
            let text = repo.text(RULE_05);
            let rdirs: BTreeSet<String> = RULE_DIR_RE
                .captures_iter(&text)
                .map(|caps| caps[1].to_string())
                .collect();
            for name in kdirs.difference(&rdirs) {
                out.push(Finding::new(
                    CHECK,
                    RULE_05,
                    format!("missing:kernel/src/{name}/"),
                    format!("rule 05 does not list kernel/src/{name}/"),
                    0,
                ));
            }
            for name in rdirs.difference(&kdirs) {
                out.push(Finding::new(
                    CHECK,
                    RULE_05,
                    format!("stale:kernel/src/{name}/"),
                    format!("rule 05 lists kernel/src/{name}/, which does not exist"),
                    0,
                ));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn regexes_compile() {
        let all: [&LazyLock<Regex>; 6] = [
            &LAYOUT_START_RE,
            &LAYOUT_STOP_RE,
            &TREE_DIR_RE,
            &MODULE_NAME_RE,
            &TREE_PREFIX_RE,
            &RULE_DIR_RE,
        ];
        // Forcing each LazyLock runs its Regex::new(...).expect("valid regex"): a
        // bad pattern panics here, at test time, rather than in production.
        for rx in all {
            LazyLock::force(rx);
        }
    }

    #[test]
    fn tree_entries_reads_dirs_and_top_level_modules() {
        let block: Vec<String> = [
            "```text",
            "├── kernel/src/           kernel",
            "│   ├── mm/               memory",
            "│   └── (top-level)       main.rs, boot_phase,",
            "│                         dtb",
            "├── shared/src/           shared types",
            "│   ├── kits/             kit traits",
            "│   └── (top-level)       lib, boot",
            "├── uefi-stub/src/        stub",
            "│   └── (top-level)       never read",
            "```",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            tree_entries(&block, "kernel/src/", "shared/src/"),
            (names(&["mm"]), names(&["boot_phase", "dtb", "main"]))
        );
        assert_eq!(
            tree_entries(&block, "shared/src/", "uefi-stub/"),
            (names(&["kits"]), names(&["boot", "lib"]))
        );
        assert_eq!(
            tree_entries(&block, "absent/", "uefi-stub/"),
            (names(&[]), names(&[]))
        );
    }
}
