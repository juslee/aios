//! `test-count`, ported from check.py `check_test_count` (L787-810): host test counts stated
//! in whole-file current-state docs must equal the number of `#[test]` occurrences in
//! `shared/src/**.rs`.
//!
//! The count is textual, like check.py's `re.findall(r"#\[test\]")`: an attribute inside a
//! comment counts too. Claims are matched on the raw prose line (code spans included). A
//! claimed number is compared and printed as Python's `int()` would (`pystr::int_str`: leading
//! zeros dropped, deferring to that helper's own note on CPython 3.11+'s 4300-digit limit).
//! The claim patterns are check.py's, compiled with `crate::pyre::compile`: `\d` takes any
//! Unicode decimal digit (the claim prints through `int_str` in ASCII, as `int()` would), and
//! `\s` includes U+001C..U+001F (only U+001F can sit inside a prose line, because
//! `splitlines()` breaks at U+001C..U+001E).
//! Accepted divergences: the regex crate's `\b` follows Unicode word characters that differ
//! slightly from Python's; and the case-insensitive `(?i)` claim patterns do not fold `ı` (U+0131) or `İ` (U+0130) to `i`,
//! as Python's `re.IGNORECASE` does, so a claim spelled with one of those characters (e.g.
//! "unıt tests") is not reported.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use super::Check;
use crate::cmd::docs_check::markdown::prose_lines;
use crate::cmd::docs_check::model::Finding;
use crate::cmd::docs_check::repo::Repo;
use crate::{pyre, pystr};

/// The three claim patterns of check.py L792-796 (`claim_res`), tried in this order on every line.
static CLAIM_RES: LazyLock<[Regex; 3]> = LazyLock::new(|| {
    [
        pyre::compile(r"<!--\s*gen:test-count\s*-->\s*(\d+)").expect("valid regex"),
        pyre::compile(
            r"(?i)\bcurrent(?:ly)?\b[^\n]{0,60}?\b(\d{2,5}) (?:host(?:-side)? |unit )?tests\b",
        )
        .expect("valid regex"),
        pyre::compile(r"(?i)\bcurrent test distribution \((\d+) tests\)").expect("valid regex"),
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
        // Forcing each LazyLock runs its pyre::compile(...).expect("valid regex"): a
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

    #[test]
    fn case_folding_does_not_reach_turkish_dotted_and_dotless_i() {
        // check.py's re.IGNORECASE folds ı (U+0131) and İ (U+0130) to plain ASCII `i`, so
        // "unıt"/"dıstribution" still match; the regex crate's (?i) does not, so these claims
        // go unreported (accepted divergence, documented above).
        let first = |i: usize, line: &str| -> Option<String> {
            CLAIM_RES[i].captures(line).map(|caps| caps[1].to_string())
        };
        assert_eq!(first(1, "Currently 12 unıt tests"), None);
        assert_eq!(first(2, "Current test dıstribution (5 tests)"), None);
    }

    #[test]
    fn claim_patterns_use_pythons_digit_and_space_classes() {
        // check.py's \s matches U+001F and its \d every Unicode decimal digit
        // (run against check.py at 33c6b3d): both claims below are matched there,
        // and the claimed number prints through int() as ASCII.
        let first = |i: usize, line: &str| -> Option<String> {
            CLAIM_RES[i].captures(line).map(|caps| caps[1].to_string())
        };
        assert_eq!(
            first(0, "<!--\u{1f}gen:test-count-->12").as_deref(),
            Some("12")
        );
        assert_eq!(
            first(0, "<!-- gen:test-count -->\u{1f}١٢").as_deref(),
            Some("١٢")
        );
        assert_eq!(pystr::int_str("١٢"), "12");
    }
}
