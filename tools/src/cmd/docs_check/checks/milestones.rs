//! `milestone-status` and `phase-count` (check.py L928-1017; line references are to
//! `scripts/docs/check.py` at 33c6b3d).
//!
//! milestone-status compares the milestones merged on main's first-parent
//! history (`Phase N MK:` subjects, see [`Repo::merged_milestones`]) with each
//! phase doc's `**Status:**` line and task checkboxes, the README status and the
//! development plan's §8 and §8.1 tables. phase-count compares "N phases" claims
//! in prose with the number of §8 table rows.
//!
//! Accepted divergences from check.py (no tracked file exercises them): `\d` is
//! `[0-9]` and `str.isdigit()` is ASCII-only, so non-ASCII decimal digits are not
//! numbers here; `\s` and `\b` follow the `regex` crate's Unicode classes; a §8
//! phase number that does not fit `u64` matches no phase doc or merged milestone,
//! while its `§8:phase-N` target is still printed exactly (`int_str`); past 4300
//! digits CPython's `int()` raises instead (L965, and L1014 for a claim), so
//! check.py exits 2 where aios reports the row or claim. A §8 row of 6 or more
//! cells whose first cell is `isdigit()`-true but `int()` raises (for example
//! `²`, L965) makes check.py's `int(cells[0])` raise; `run_checks` catches only
//! `Skip`, so the exception reaches check.py's `__main__` guard (L1665-1670),
//! which prints a traceback and exits 2. `is_ascii_digits` rejects `²` already,
//! so aios instead treats that row as having no digit first cell (like a header
//! row) and skips it, and the run completes normally. The same cell also
//! reaches phase-count's row count (L1000), which never calls `int()`:
//! check.py counts it toward `actual` because `isdigit()` is true, while
//! aios's ASCII-only count does not, so `actual` is one lower here for such
//! input.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;

use super::Check;
use crate::cmd::docs_check::markdown::{milestone_tokens, prose_lines, section_body, table_rows};
use crate::cmd::docs_check::model::{Finding, Skip};
use crate::cmd::docs_check::repo::Repo;
use crate::pystr::{int_str, is_ascii_digits, parse_uint, splitlines, strip};

const MILESTONE_STATUS: &str = "milestone-status";
const PHASE_COUNT: &str = "phase-count";
const PLAN: &str = "docs/project/development-plan.md";
const README: &str = "README.md";
const RULE_07: &str = ".claude/rules/07-milestone-numbering.md";
const CLAUDE_MD: &str = "CLAUDE.md";

/// check.py L940.
static STATUS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\*\*Status:\*\*\s*(.*)$").expect("valid regex"));
/// check.py L951 without its `(?!~~)` lookahead; [`is_unchecked_task`] applies it.
static UNCHECKED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*[-*] \[ \] ").expect("valid regex"));
/// check.py L962 and L1000: the development plan's §8 table.
static PLAN_START_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^## 8\. ").expect("valid regex"));
static PLAN_STOP_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^## ").expect("valid regex"));
/// check.py L970: the §8.1 Velocity Summary table.
static VELOCITY_START_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^### Velocity Summary").expect("valid regex"));
static VELOCITY_STOP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^#{2,3} ").expect("valid regex"));
/// check.py L1003-1004; `PHASES_RE` below is L1005-1006.
static PHASES_ACROSS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([0-9]+) phases across\b").expect("valid regex"));
static PHASES_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([0-9]+) phases\b").expect("valid regex"));

/// Merged `Phase N MK:` milestones vs phase docs, README and development plan.
pub struct MilestoneStatus;

/// "N phases" claims vs the development plan's §8 table.
pub struct PhaseCount;

/// check.py L951 `^\s*[-*] \[ \] (?!~~)`: an open task checkbox that is not a
/// struck-through ("deferred") item. `\s*` has no alternative that lets `[-*]`
/// match elsewhere, so testing the text after the one possible match is exact.
pub fn is_unchecked_task(line: &str) -> bool {
    UNCHECKED_RE
        .find(line)
        .is_some_and(|m| !line[m.end()..].starts_with("~~"))
}

/// check.py L979-989: at most one finding comparing a status text with the
/// milestones of a phase (`ms`) and the merged ones among them (`done`).
pub fn status_findings(
    rel: &str,
    target: &str,
    status: &str,
    ms: &BTreeSet<u64>,
    done: &BTreeSet<u64>,
    line: usize,
) -> Vec<Finding> {
    let s = status.to_lowercase();
    let message = if !ms.is_empty() && done == ms && !s.starts_with("complete") {
        format!(
            "all milestones ({}) are merged but status is '{status}'",
            fmt_ms(ms)
        )
    } else if !done.is_empty() && done != ms && s.starts_with("planned") {
        format!("{} merged but status is '{status}'", fmt_ms(done))
    } else if done.is_empty() && s.starts_with("complete") {
        format!("status is '{status}' but no milestone is merged")
    } else {
        return Vec::new();
    };
    vec![Finding::new(MILESTONE_STATUS, rel, target, message, line)]
}

/// check.py L992-993: `M1, M2, M10` in numeric order.
pub fn fmt_ms(ms: &BTreeSet<u64>) -> String {
    ms.iter()
        .map(|m| format!("M{m}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The first `**Status:**` line: (1-based line, stripped status), or (0, "").
fn status_line<'a>(lines: &[&'a str]) -> (usize, &'a str) {
    for (i, &line) in lines.iter().enumerate() {
        if let Some(caps) = STATUS_RE.captures(line) {
            return (i + 1, strip(caps.get(1).map_or("", |m| m.as_str())));
        }
    }
    (0, "")
}

impl Check for MilestoneStatus {
    fn name(&self) -> &'static str {
        MILESTONE_STATUS
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        let merged = repo.merged_milestones()?;
        let Some(&latest) = merged.keys().next_back() else {
            return Err(Skip(
                "no 'Phase N MK:' commits found on main's first-parent history".to_string(),
            )
            .into());
        };
        let merged_set: BTreeSet<u64> = merged.keys().copied().collect();
        let mut out = Vec::new();
        let mut phase_ms: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
        for (phase, rel) in repo.phase_docs() {
            let sections = repo.milestone_sections(&rel);
            let ms: BTreeSet<u64> = sections.keys().copied().collect();
            phase_ms.insert(phase, ms.clone());
            let text = repo.text(&rel);
            let lines = splitlines(&text);
            let (status_lineno, status) = status_line(&lines);
            let done: BTreeSet<u64> = ms.intersection(&merged_set).copied().collect();
            out.extend(status_findings(
                &rel,
                "status",
                status,
                &ms,
                &done,
                status_lineno,
            ));
            for num in &done {
                let Some(&(lo, hi)) = sections.get(num) else {
                    continue;
                };
                // "- [ ] ~~task~~ — deferred to MK" is closed here, as in brief.sh.
                let unchecked: Vec<usize> = (lo..=hi)
                    .filter(|&n| n >= 1 && lines.get(n - 1).copied().is_some_and(is_unchecked_task))
                    .collect();
                if let Some(&first) = unchecked.first() {
                    out.push(Finding::new(
                        MILESTONE_STATUS,
                        rel.as_str(),
                        format!("M{num}:unchecked"),
                        format!(
                            "merged milestone M{num} still has {} unchecked task(s)",
                            unchecked.len()
                        ),
                        first,
                    ));
                }
            }
        }
        if repo.is_file(README) && !milestone_tokens(&repo.text(README)).contains(&latest) {
            out.push(Finding::new(
                MILESTONE_STATUS,
                README,
                format!("latest:M{latest}"),
                format!("README status does not mention the latest merged milestone M{latest}"),
                0,
            ));
        }
        if repo.is_file(PLAN) {
            let text = repo.text(PLAN);
            for (lineno, cells) in table_rows(&section_body(&text, &PLAN_START_RE, &PLAN_STOP_RE)) {
                if cells.len() < 6 || !is_ascii_digits(&cells[0]) {
                    continue;
                }
                // check.py: `phase_ms.get(phase) or {m for m, p in merged.items() if p == phase}`.
                let phase = parse_uint(&cells[0]);
                let ms: BTreeSet<u64> = match phase.and_then(|p| phase_ms.get(&p)) {
                    Some(ms) if !ms.is_empty() => ms.clone(),
                    _ => merged
                        .iter()
                        .filter(|&(_, &p)| Some(p) == phase)
                        .map(|(&m, _)| m)
                        .collect(),
                };
                let done: BTreeSet<u64> = ms.intersection(&merged_set).copied().collect();
                out.extend(status_findings(
                    PLAN,
                    &format!("§8:phase-{}", int_str(&cells[0])),
                    &cells[5],
                    &ms,
                    &done,
                    lineno,
                ));
            }
            let mut covered: BTreeSet<u64> = BTreeSet::new();
            for (_, cells) in
                table_rows(&section_body(&text, &VELOCITY_START_RE, &VELOCITY_STOP_RE))
            {
                if cells.len() >= 5 {
                    covered.extend(milestone_tokens(&cells[4]));
                }
            }
            for num in merged_set.difference(&covered) {
                out.push(Finding::new(
                    MILESTONE_STATUS,
                    PLAN,
                    format!("§8.1:M{num}"),
                    format!("§8.1 Velocity Summary has no row covering merged milestone M{num}"),
                    0,
                ));
            }
        }
        Ok(out)
    }
}

/// check.py L1009-1016 for one source file: every `rx` claim on a prose line
/// whose number differs from `actual`; a claim too large for `u64` differs too,
/// and past 4300 digits check.py's `int()` (L1014) raises, so check.py exits 2
/// where aios reports the claim.
fn phase_claims(repo: &Repo, rel: &str, rx: &Regex, actual: usize, out: &mut Vec<Finding>) {
    if !repo.is_file(rel) {
        return;
    }
    let text = repo.text(rel);
    for (lineno, line) in prose_lines(&text) {
        for caps in rx.captures_iter(line) {
            let claimed = caps.get(1).map_or("", |m| m.as_str());
            if parse_uint(claimed) != u64::try_from(actual).ok() {
                out.push(Finding::new(
                    PHASE_COUNT,
                    rel,
                    format!("claimed:{claimed}"),
                    format!("says {claimed} phases; development-plan §8 lists {actual}"),
                    lineno,
                ));
            }
        }
    }
}

impl Check for PhaseCount {
    fn name(&self) -> &'static str {
        PHASE_COUNT
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        if !repo.is_file(PLAN) {
            return Err(Skip(format!("{PLAN} not found")).into());
        }
        let plan = repo.text(PLAN);
        let actual = table_rows(&section_body(&plan, &PLAN_START_RE, &PLAN_STOP_RE))
            .iter()
            .filter(|(_, cells)| cells.first().is_some_and(|c| is_ascii_digits(c)))
            .count();
        let mut out = Vec::new();
        phase_claims(repo, PLAN, &PHASES_ACROSS_RE, actual, &mut out);
        phase_claims(repo, README, &PHASES_ACROSS_RE, actual, &mut out);
        phase_claims(repo, RULE_07, &PHASES_RE, actual, &mut out);
        phase_claims(repo, CLAUDE_MD, &PHASES_RE, actual, &mut out);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ms: &[u64]) -> BTreeSet<u64> {
        ms.iter().copied().collect()
    }

    #[test]
    fn regexes_compile() {
        let all: [&LazyLock<Regex>; 8] = [
            &STATUS_RE,
            &UNCHECKED_RE,
            &PLAN_START_RE,
            &PLAN_STOP_RE,
            &VELOCITY_START_RE,
            &VELOCITY_STOP_RE,
            &PHASES_ACROSS_RE,
            &PHASES_RE,
        ];
        // Forcing each LazyLock runs its Regex::new(...).expect("valid regex"): a
        // bad pattern panics here, at test time, rather than in production.
        for rx in all {
            LazyLock::force(rx);
        }
    }

    #[test]
    fn unchecked_task_skips_struck_through_items() {
        assert!(is_unchecked_task("- [ ] task"));
        assert!(is_unchecked_task("  * [ ] x"));
        assert!(!is_unchecked_task("- [ ] ~~old~~ — deferred"));
        assert!(!is_unchecked_task("- [x] y"));
        assert!(!is_unchecked_task("+ [ ] plus is not a task marker here"));
        assert!(!is_unchecked_task("- [ ]no space"));
        assert!(is_unchecked_task("\t- [ ] ~ single tilde"));
    }

    #[test]
    fn status_findings_match_check_py() {
        let rel = "docs/phases/00-a.md";
        let one = |target: &str, message: &str, line: usize| {
            vec![Finding::new("milestone-status", rel, target, message, line)]
        };
        assert_eq!(
            status_findings(
                rel,
                "status",
                "In Progress",
                &set(&[1, 2]),
                &set(&[1, 2]),
                3
            ),
            one(
                "status",
                "all milestones (M1, M2) are merged but status is 'In Progress'",
                3
            )
        );
        assert_eq!(
            status_findings(
                rel,
                "status",
                "Complete (2026-01-01)",
                &set(&[1, 2]),
                &set(&[1, 2]),
                3
            ),
            Vec::new()
        );
        assert_eq!(
            status_findings(rel, "status", "PLANNED", &set(&[1, 2]), &set(&[2]), 3),
            one("status", "M2 merged but status is 'PLANNED'", 3)
        );
        assert_eq!(
            status_findings(rel, "status", "In Progress", &set(&[1, 2]), &set(&[2]), 3),
            Vec::new()
        );
        assert_eq!(
            status_findings(rel, "status", "complete", &set(&[1, 2]), &set(&[]), 3),
            one(
                "status",
                "status is 'complete' but no milestone is merged",
                3
            )
        );
        assert_eq!(
            status_findings(rel, "status", "Planned", &set(&[]), &set(&[]), 0),
            Vec::new()
        );
        assert_eq!(
            status_findings(rel, "status", "", &set(&[7]), &set(&[7]), 0),
            one(
                "status",
                "all milestones (M7) are merged but status is ''",
                0
            )
        );
    }

    #[test]
    fn fmt_ms_sorts_numerically() {
        assert_eq!(fmt_ms(&set(&[12, 3, 7])), "M3, M7, M12");
        assert_eq!(fmt_ms(&set(&[])), "");
    }
}
