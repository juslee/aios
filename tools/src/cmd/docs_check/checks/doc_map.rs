//! `doc-map`, ported from check.py `check_doc_map` (L701-721): every `docs/...` path in a code
//! span of `docs/project/doc-map.md` (brace-expanded) must exist, and every Markdown file under
//! `docs/` outside `docs/phases/` and `docs/knowledge/` must be listed.
//!
//! Like check.py, code spans are read from the raw prose line, so a code span inside a
//! single-line HTML comment still counts as listed.

use std::collections::HashSet;

use super::Check;
use crate::cmd::docs_check::markdown::{brace_expand, code_spans, prose_lines};
use crate::cmd::docs_check::model::Finding;
use crate::cmd::docs_check::repo::Repo;

/// The doc map itself.
pub const DOC_MAP_REL: &str = "docs/project/doc-map.md";
/// Docs under `docs/` that are indexes rather than topics, so the doc map need not list them.
pub const DOC_MAP_ALLOWLIST: [&str; 1] = [DOC_MAP_REL];

/// `doc-map` (check.py `check_doc_map`, L701-721).
pub struct DocMap;

impl Check for DocMap {
    fn name(&self) -> &'static str {
        "doc-map"
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        if !repo.is_file(DOC_MAP_REL) {
            return Ok(vec![Finding::new(
                "doc-map",
                DOC_MAP_REL,
                "missing",
                "docs/project/doc-map.md does not exist",
                0,
            )]);
        }
        let mut out = Vec::new();
        let mut listed: HashSet<String> = HashSet::new();
        let text = repo.text(DOC_MAP_REL);
        for (lineno, line) in prose_lines(&text) {
            for span in code_spans(line) {
                if !span.starts_with("docs/") {
                    continue;
                }
                for path in brace_expand(&span) {
                    if !repo.exists(&path) {
                        out.push(Finding::new(
                            "doc-map",
                            DOC_MAP_REL,
                            format!("missing:{path}"),
                            format!("listed path does not exist: {path}"),
                            lineno,
                        ));
                    }
                    listed.insert(path);
                }
            }
        }
        for f in repo.md_files() {
            if !f.starts_with("docs/")
                || f.starts_with("docs/phases/")
                || f.starts_with("docs/knowledge/")
            {
                continue;
            }
            if listed.contains(f) || DOC_MAP_ALLOWLIST.contains(&f.as_str()) {
                continue;
            }
            out.push(Finding::new(
                "doc-map",
                f.as_str(),
                "unlisted",
                format!("{f} is not listed in {DOC_MAP_REL}"),
                0,
            ));
        }
        Ok(out)
    }
}
