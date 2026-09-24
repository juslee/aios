//! `repo-paths`, ported from check.py `check_repo_paths` (L724-748): code spans that start
//! with `kernel/`, `shared/`, `uefi-stub/` or `scripts/` in current-state regions must name
//! an existing path.
//!
//! Like check.py, code spans are read from the raw prose line (HTML comments are not
//! masked). Accepted divergences: `\S` in `REPO_PATH_RE` is Rust's, which treats
//! U+001C..U+001F as non-space; and `is_path_placeholder` counts a capital assigned
//! after Unicode 16 (e.g. U+A7CE) as uppercase, so such a path is skipped here where
//! check.py reports it (listed in the `markdown` module doc).

use std::sync::LazyLock;

use regex::Regex;

use super::Check;
use crate::cmd::docs_check::markdown::{
    clean_repo_path, code_spans, is_path_placeholder, prose_lines,
};
use crate::cmd::docs_check::model::Finding;
use crate::cmd::docs_check::repo::Repo;

/// check.py `REPO_PATH_RE` (L724), applied with `re.match`.
static REPO_PATH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^((?:kernel|shared|uefi-stub|scripts)/\S*)").expect("valid regex")
});

/// `repo-paths` (check.py `check_repo_paths`, L733-748).
pub struct RepoPaths;

impl Check for RepoPaths {
    fn name(&self) -> &'static str {
        "repo-paths"
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        let mut out = Vec::new();
        for (rel, allowed) in repo.current_state_regions()? {
            let text = repo.text(&rel);
            for (lineno, line) in prose_lines(&text) {
                if allowed
                    .as_ref()
                    .is_some_and(|lines| !lines.contains(&lineno))
                {
                    continue;
                }
                for span in code_spans(line) {
                    let Some(caps) = REPO_PATH_RE.captures(&span) else {
                        continue;
                    };
                    let path = clean_repo_path(&caps[1]);
                    if path.is_empty() || is_path_placeholder(&path) {
                        continue;
                    }
                    if !repo.exists(&path) {
                        out.push(Finding::new(
                            "repo-paths",
                            rel.as_str(),
                            path.as_str(),
                            format!("path does not exist: {path}"),
                            lineno,
                        ));
                    }
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
        LazyLock::force(&REPO_PATH_RE);
    }
}
