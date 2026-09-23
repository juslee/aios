//! `test-count`, ported from check.py `check_test_count` (L787-810): host test counts stated
//! in whole-file current-state docs must equal the number of `#[test]` occurrences in
//! `shared/src/**.rs`.
//!
//! The count is textual, like check.py's `re.findall(r"#\[test\]")`: an attribute inside a
//! comment counts too. Claims are matched on the raw prose line (code spans included). A
//! claimed number is compared and printed as Python's `int()` would (`pystr::int_str`: leading
//! zeros dropped, any length). Accepted divergences: `[0-9]` replaces Python's `\d`, and the
//! regex crate's `\b` follows Unicode word characters that differ slightly from Python's.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use super::Check;
use crate::cmd::docs_check::markdown::prose_lines;
use crate::cmd::docs_check::model::Finding;
use crate::cmd::docs_check::repo::Repo;
use crate::pystr;

/// The three claim patterns of check.py L792-796 (`claim_res`), tried in this order on every line.
static CLAIM_RES: LazyLock<[Regex; 3]> = LazyLock::new(|| {
    [
        Regex::new(r"<!--\s*gen:test-count\s*-->\s*([0-9]+)").expect("valid regex"),
        Regex::new(
            r"(?i)\bcurrent(?:ly)?\b[^\n]{0,60}?\b([0-9]{2,5}) (?:host(?:-side)? |unit )?tests\b",
        )
        .expect("valid regex"),
        Regex::new(r"(?i)\bcurrent test distribution \(([0-9]+) tests\)").expect("valid regex"),
    ]
});

/// `test-count` (check.py `check_test_count`, L787-810).
pub struct TestCount;

impl Check for TestCount {
    fn name(&self) -> &'static str {
        "test-count"
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        let actual: usize = repo
            .files()
            .iter()
            .filter(|f| f.starts_with("shared/src/") && f.ends_with(".rs"))
            .map(|f| repo.text(f).matches("#[test]").count())
            .sum();
        let actual_str = actual.to_string();
        let mut out = Vec::new();
        for (rel, allowed) in repo.current_state_regions()? {
            if allowed.is_some() {
                continue; // phase docs record historical counts
            }
            let mut seen_lines: HashSet<(usize, String)> = HashSet::new();
            let text = repo.text(&rel);
            for (lineno, line) in prose_lines(&text) {
                for rx in CLAIM_RES.iter() {
                    for caps in rx.captures_iter(line) {
                        let claimed = pystr::int_str(&caps[1]);
                        if claimed != actual_str && seen_lines.insert((lineno, claimed.clone())) {
                            out.push(Finding::new(
                                "test-count",
                                rel.as_str(),
                                format!("claimed:{claimed}"),
                                format!(
                                    "states {claimed} tests; shared/src has {actual} \
                                     #[test] functions"
                                ),
                                lineno,
                            ));
                        }
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
        LazyLock::force(&CLAIM_RES);
    }

    #[test]
    fn claim_patterns_capture_the_stated_number() {
        let first = |i: usize, line: &str| -> Option<String> {
            CLAIM_RES[i].captures(line).map(|caps| caps[1].to_string())
        };
        assert_eq!(
            first(0, "n: <!-- gen:test-count -->0012").as_deref(),
            Some("0012")
        );
        assert_eq!(
            first(1, "Currently 99 host tests pass").as_deref(),
            Some("99")
        );
        assert_eq!(
            first(1, "currently 12 host-side tests").as_deref(),
            Some("12")
        );
        assert_eq!(first(1, "We currently run 4 host-side tests"), None);
        assert_eq!(first(1, "Currently 100000 tests"), None);
        assert_eq!(
            first(2, "Current test distribution (5 tests)").as_deref(),
            Some("5")
        );
    }
}
