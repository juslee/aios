//! `just-recipes`, ported from check.py `documented_recipes` and `check_just_recipes`
//! (L751-784): every `` `just X` `` code span in a current-state region must name a justfile
//! recipe, and every public recipe except `default` must appear in a table row of the README
//! "Build Commands" section or the developer guide's section 5.1.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use super::Check;
use crate::cmd::docs_check::markdown::{code_spans, prose_lines, section_body};
use crate::cmd::docs_check::model::{Finding, Skip};
use crate::cmd::docs_check::repo::Repo;
use crate::pystr;

/// check.py L754-755: the README and developer-guide sections that document recipes.
static README_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^## Build Commands").expect("valid regex"));
static README_STOP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^## ").expect("valid regex"));
static GUIDE_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^### 5\.1 ").expect("valid regex"));
static GUIDE_STOP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^#{2,3} ").expect("valid regex"));
/// check.py L762 (`re.finditer` over a table row).
static DOCUMENTED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`just ([A-Za-z0-9_-]+)").expect("valid regex"));
/// check.py L777 (`re.match` on a code span).
static JUST_SPAN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^just ([A-Za-z0-9_-]+)").expect("valid regex"));

/// Recipes named as `` `just X` `` in the table rows of the README "Build Commands" section and
/// the developer guide's section 5.1 (check.py `documented_recipes`, L751-764). A source file
/// that is not in the repository listing is skipped.
pub fn documented_recipes(repo: &Repo) -> BTreeSet<String> {
    let mut documented = BTreeSet::new();
    collect_documented(
        repo,
        "README.md",
        &README_START,
        &README_STOP,
        &mut documented,
    );
    collect_documented(
        repo,
        "docs/project/developer-guide.md",
        &GUIDE_START,
        &GUIDE_STOP,
        &mut documented,
    );
    documented
}

fn collect_documented(
    repo: &Repo,
    rel: &str,
    start: &Regex,
    stop: &Regex,
    documented: &mut BTreeSet<String>,
) {
    if !repo.is_file(rel) {
        return;
    }
    let text = repo.text(rel);
    for (_, line) in section_body(&text, start, stop) {
        if pystr::lstrip(line).starts_with('|') {
            for caps in DOCUMENTED_RE.captures_iter(line) {
                documented.insert(caps[1].to_string());
            }
        }
    }
}

/// `just-recipes` (check.py `check_just_recipes`, L767-784).
pub struct JustRecipes;

impl Check for JustRecipes {
    fn name(&self) -> &'static str {
        "just-recipes"
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        if !repo.is_file("justfile") {
            return Err(Skip("no justfile".to_string()).into());
        }
        let (names, public) = repo.justfile_recipes();
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
                    let Some(caps) = JUST_SPAN_RE.captures(&span) else {
                        continue;
                    };
                    let name = &caps[1];
                    if !names.contains(name) {
                        out.push(Finding::new(
                            "just-recipes",
                            rel.as_str(),
                            name,
                            format!("`just {name}` is not a justfile recipe"),
                            lineno,
                        ));
                    }
                }
            }
        }
        let documented = documented_recipes(repo);
        // sorted(public - documented - {"default"}): `public` is a BTreeSet, so it is sorted.
        for name in &public {
            if documented.contains(name) || name == "default" {
                continue;
            }
            out.push(Finding::new(
                "just-recipes",
                "justfile",
                format!("undocumented:{name}"),
                format!(
                    "public recipe `{name}` is missing from the README Build \
                     Commands and developer-guide §5.1 tables"
                ),
                0,
            ));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regexes_compile() {
        let all: [&LazyLock<Regex>; 6] = [
            &README_START,
            &README_STOP,
            &GUIDE_START,
            &GUIDE_STOP,
            &DOCUMENTED_RE,
            &JUST_SPAN_RE,
        ];
        // Forcing each LazyLock runs its Regex::new(...).expect("valid regex"): a
        // bad pattern panics here, at test time, rather than in production.
        for rx in all {
            LazyLock::force(rx);
        }
    }
}
