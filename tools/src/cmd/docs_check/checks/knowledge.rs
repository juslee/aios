//! `knowledge-hygiene` (check.py L1350-1376 at 33c6b3d): `docs/knowledge/` file names
//! (`YYYY-MM-DD-initials-short-description.md`), YAML frontmatter keys and
//! status values, and an empty `plans/` directory.
//!
//! Accepted divergence from check.py (no tracked file reaches it): `\d` is
//! `[0-9]` in `KNOWLEDGE_NAME_RE`, so a file name written with a non-ASCII
//! Unicode decimal digit does not match here, where Python's `\d` would.
//!
//! `KNOWLEDGE_NAME_RE` ends `\n?$`, mirroring Python's non-MULTILINE `$` (which
//! also matches just before a final `\n`) per the path-anchor rule the other
//! checks apply: the regex crate's own `$` is `\z` and would not. `md_files`
//! yields only names ending `.md` (check.py L390), so no basename it produces
//! can itself end in `\n`; the `\n?$` form is kept for consistency with the
//! other path-matched regexes even though no input here reaches the difference.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::cmd::docs_check::checks::Check;
use crate::cmd::docs_check::markdown::parse_frontmatter;
use crate::cmd::docs_check::model::Finding;
use crate::cmd::docs_check::repo::Repo;
use crate::paths::basename;

const CHECK: &str = "knowledge-hygiene";
const KNOWLEDGE_DIR: &str = "docs/knowledge/";
const PLANS_DIR: &str = "docs/knowledge/plans/";
/// Frontmatter keys every note needs (check.py L93).
const KNOWLEDGE_KEYS: [&str; 4] = ["author", "date", "tags", "status"];
/// check.py L90.
const KNOWLEDGE_STATUSES: [&str; 3] = ["draft", "in-progress", "final"];
/// check.py L92: discussions are "draft or active", then "graduated".
const DISCUSSION_STATUSES: [&str; 2] = ["active", "graduated"];

/// R1 (check.py L94). `\n?$` matches Python's non-MULTILINE `$` on a tracked
/// basename ending in a trailing newline.
static KNOWLEDGE_NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}-[a-z]{2,3}-[a-z0-9][a-z0-9-]*\.md\n?$")
        .expect("valid regex")
});

/// docs/knowledge naming, frontmatter, and an empty plans/ dir.
pub struct KnowledgeHygiene;

impl Check for KnowledgeHygiene {
    fn name(&self) -> &'static str {
        CHECK
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        let mut out = Vec::new();
        for f in repo.md_files() {
            if !f.starts_with(KNOWLEDGE_DIR) {
                continue;
            }
            let base = basename(f);
            if base == "README.md" || base == "_template.md" {
                continue;
            }
            if f.starts_with(PLANS_DIR) {
                out.push(Finding::new(
                    CHECK,
                    f.as_str(),
                    "plans-not-empty",
                    "working plan present; distill it and remove it before the PR is ready",
                    0,
                ));
                continue;
            }
            if !KNOWLEDGE_NAME_RE.is_match(base) {
                out.push(Finding::new(
                    CHECK,
                    f.as_str(),
                    "name",
                    "file name is not YYYY-MM-DD-initials-short-description.md",
                    0,
                ));
            }
            let Some(frontmatter) = parse_frontmatter(&repo.text(f)) else {
                out.push(Finding::new(
                    CHECK,
                    f.as_str(),
                    "frontmatter",
                    "no YAML frontmatter",
                    0,
                ));
                continue;
            };
            for key in KNOWLEDGE_KEYS {
                if !frontmatter.contains_key(key) {
                    out.push(Finding::new(
                        CHECK,
                        f.as_str(),
                        format!("missing:{key}"),
                        format!("frontmatter lacks '{key}'"),
                        0,
                    ));
                }
            }
            let mut allowed: BTreeSet<&str> = KNOWLEDGE_STATUSES.into_iter().collect();
            if f.split('/').nth(2) == Some("discussions") {
                allowed.extend(DISCUSSION_STATUSES);
            }
            if let Some(status) = frontmatter.get("status") {
                if !status.is_empty() && !allowed.contains(status.as_str()) {
                    let listed = allowed.into_iter().collect::<Vec<_>>().join(", ");
                    out.push(Finding::new(
                        CHECK,
                        f.as_str(),
                        format!("status:{status}"),
                        format!("status '{status}' is not one of {listed}"),
                        0,
                    ));
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regexes_compile() {
        // Forcing each LazyLock runs its Regex::new(...).expect("valid regex"): a
        // bad pattern panics here, at test time, rather than in production.
        LazyLock::force(&KNOWLEDGE_NAME_RE);
    }

    #[test]
    fn knowledge_name_pattern() {
        assert!(KNOWLEDGE_NAME_RE.is_match("2026-09-22-jl-rust-agent-tools.md"));
        assert!(KNOWLEDGE_NAME_RE.is_match("2026-01-01-abc-x.md"));
        assert!(!KNOWLEDGE_NAME_RE.is_match("2026-01-06-abcd-long.md"));
        assert!(!KNOWLEDGE_NAME_RE.is_match("Bad_Name.md"));
        assert!(!KNOWLEDGE_NAME_RE.is_match("2026-01-01-ab--x.md.txt"));
        assert!(!KNOWLEDGE_NAME_RE.is_match("2026-01-01-ab-X.md"));
    }

    #[test]
    fn knowledge_name_re_matches_a_trailing_newline_basename() {
        // Python: re.match(r"^\d{4}-\d{2}-\d{2}-[a-z]{2,3}-[a-z0-9][a-z0-9-]*\.md$",
        //   "2026-01-01-ab-x.md\n").group(0) == "2026-01-01-ab-x.md".
        assert!(KNOWLEDGE_NAME_RE.is_match("2026-01-01-ab-x.md\n"));
    }
}
