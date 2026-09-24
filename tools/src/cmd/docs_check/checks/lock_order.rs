//! `lock-order`, ported from check.py `code_mutex_statics` and `check_lock_order`
//! (L813-925): production `Mutex` statics in `kernel/src` versus the lock table of
//! `docs/kernel/deadlock-prevention.md` sections 3.3-3.4, the `Lock ordering` chain in
//! `CLAUDE.md`, and `lock ordering` comment blocks in kernel code.
//!
//! `split_outside_braces` replaces check.py's `re.split(r">(?![^{]*})", ...)` (the regex
//! crate has no lookaround). Accepted divergences: a rank cell counts only when it is ASCII
//! digits that fit `u64`; Python's `isdigit()` (check.py L854) and `int()` (L855) also accept
//! other Unicode decimal digits and values too large for `u64`, but for a cell where
//! `isdigit()` is true and `int()` raises (for example `²`, or more digits than CPython
//! 3.11+'s 4300-digit `int()` limit; see `pystr`), check.py exits 2 with an uncaught
//! `ValueError` while aios leaves the lock unranked. `\b`/`\w`/`\s` follow the regex crate's
//! Unicode classes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use super::Check;
use crate::cmd::docs_check::markdown::{section_body, table_rows};
use crate::cmd::docs_check::model::{Finding, Skip};
use crate::cmd::docs_check::repo::Repo;
use crate::{paths, pystr};

/// Test-only locks named as excluded in deadlock-prevention.md section 3.3.
pub const TEST_LOCKS: [&str; 2] = ["TEST_CHANNEL", "PI_TEST_CHANNEL"];

const DEADLOCK_DOC: &str = "docs/kernel/deadlock-prevention.md";

/// check.py `LOCK_NAME_RE` (L813), used with `findall`.
static LOCK_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+\b").expect("valid regex"));
/// check.py `STATIC_RE` (L814), applied with `re.match`.
static STATIC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?static\s+([A-Z][A-Z0-9_]*)\s*:\s*(.*)$")
        .expect("valid regex")
});
/// check.py L828 (`re.match`): an inline test module header.
static TEST_MOD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{").expect("valid regex")
});
/// check.py L831 (`re.search` on the static's type).
static MUTEX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bMutex\s*<").expect("valid regex"));
/// check.py L841: the section 3.3-3.4 body of the deadlock doc.
static TABLE_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^### 3\.3 ").expect("valid regex"));
static TABLE_STOP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^### 3\.5 |^## 4\.").expect("valid regex"));
/// check.py L847 (`re.fullmatch` on a table cell).
static LOCK_CELL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^`([A-Z][A-Z0-9_]*)(?:\[[^\]]*\])?`$").expect("valid regex"));
/// check.py L869 (`re.match` on each CLAUDE.md line).
static CHAIN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Lock ordering[^:]*:\s*(.*)$").expect("valid regex"));

/// Split at every `>` that is not inside `{...}`: check.py's `re.split(r">(?![^{]*})", s)`.
/// The lookahead fails exactly when a `}` comes before any `{` after the `>`, i.e. the `>`
/// sits inside a brace group.
pub fn split_outside_braces(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    for (i, c) in s.char_indices() {
        if c != '>' {
            continue;
        }
        let inside = s[i + 1..].chars().find(|&ch| ch == '{' || ch == '}') == Some('}');
        if !inside {
            parts.push(&s[start..i]);
            start = i + 1;
        }
    }
    parts.push(&s[start..]);
    parts
}

/// Production `Mutex` statics in `kernel/src`: name -> (file, 1-based line) of the first
/// definition in file order (check.py `code_mutex_statics`, L817-833). Files named
/// `tests.rs` or under a `tests/` directory are skipped, and a `#[cfg(test)]` line whose
/// next non-blank line opens a module ends the scan of that file.
pub fn code_mutex_statics(repo: &Repo) -> BTreeMap<String, (String, usize)> {
    let mut statics = BTreeMap::new();
    for f in repo.files() {
        if !(f.starts_with("kernel/src/") && f.ends_with(".rs")) {
            continue;
        }
        if paths::basename(f) == "tests.rs" || f.contains("/tests/") {
            continue;
        }
        let text = repo.text(f);
        let lines = pystr::splitlines(&text);
        for (i, line) in lines.iter().copied().enumerate() {
            if pystr::strip(line) == "#[cfg(test)]" {
                let next = lines[i + 1..]
                    .iter()
                    .copied()
                    .find(|l| !pystr::strip(l).is_empty())
                    .unwrap_or_default();
                if TEST_MOD_RE.is_match(next) {
                    break; // an inline test module runs to the end of the file by convention
                }
            }
            let Some(caps) = STATIC_RE.captures(line) else {
                continue;
            };
            let name = &caps[1];
            if MUTEX_RE.is_match(&caps[2]) && !TEST_LOCKS.contains(&name) {
                statics
                    .entry(name.to_string())
                    .or_insert_with(|| (f.clone(), i + 1));
            }
        }
    }
    statics
}

/// `lock-order` (check.py `check_lock_order`, L836-925; the lock table part is L836-863).
pub struct LockOrder;

impl Check for LockOrder {
    fn name(&self) -> &'static str {
        "lock-order"
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        if !repo.is_file(DEADLOCK_DOC) {
            return Err(Skip(format!("{DEADLOCK_DOC} not found")).into());
        }
        let statics = code_mutex_statics(repo);
        let doc_text = repo.text(DEADLOCK_DOC);
        let body = section_body(&doc_text, &TABLE_START, &TABLE_STOP);
        let mut doc_locks: BTreeMap<String, usize> = BTreeMap::new();
        let mut ranks: HashMap<String, u64> = HashMap::new();
        for (lineno, cells) in table_rows(&body) {
            let Some(name) = cells
                .iter()
                .find_map(|c| LOCK_CELL_RE.captures(c).map(|caps| caps[1].to_string()))
            else {
                continue;
            };
            doc_locks.entry(name.clone()).or_insert(lineno);
            let rank = cells
                .first()
                .map(String::as_str)
                .filter(|c| pystr::is_ascii_digits(c))
                .and_then(pystr::parse_uint);
            if let Some(rank) = rank {
                ranks.insert(name, rank);
            }
        }
        let mut out = Vec::new();
        for (name, (file, line)) in &statics {
            if doc_locks.contains_key(name) {
                continue;
            }
            out.push(
                Finding::new(
                    "lock-order",
                    DEADLOCK_DOC,
                    format!("undocumented:{name}"),
                    format!("production lock {name} is not in §3.3/§3.4"),
                    0,
                )
                .with_detail(format!("defined at {file}:{line}")),
            );
        }
        for (name, line) in &doc_locks {
            if statics.contains_key(name) {
                continue;
            }
            out.push(Finding::new(
                "lock-order",
                DEADLOCK_DOC,
                format!("stale:{name}"),
                format!("§3.3/§3.4 lists {name}, which is not a Mutex static in kernel/src"),
                *line,
            ));
        }
        out.extend(chain_findings(repo, &statics, &ranks));
        out.extend(comment_findings(repo, &statics));
        Ok(out)
    }
}

/// The first `Lock ordering ...:` line of CLAUDE.md and its continuation lines (check.py
/// L864-897): unknown names, then pairs of groups ranked in the wrong order.
fn chain_findings(
    repo: &Repo,
    statics: &BTreeMap<String, (String, usize)>,
    ranks: &HashMap<String, u64>,
) -> Vec<Finding> {
    let text = repo.text("CLAUDE.md");
    let lines = pystr::splitlines(&text);
    let Some((first, head)) = lines.iter().copied().enumerate().find_map(|(i, line)| {
        CHAIN_RE
            .captures(line)
            .map(|caps| (i, caps.get(1).map_or("", |m| m.as_str())))
    }) else {
        return Vec::new();
    };
    // Continuation lines start with three spaces and are not blank.
    let mut chain: Vec<(usize, &str)> = vec![(first + 1, head)];
    for (j, next) in lines.iter().copied().enumerate().skip(first + 1) {
        if !next.starts_with("   ") || pystr::strip(next).is_empty() {
            break;
        }
        chain.push((j + 1, pystr::strip(next)));
    }
    // The first chain line naming each lock.
    let mut name_line: HashMap<&str, usize> = HashMap::new();
    for &(lineno, part) in &chain {
        for m in LOCK_NAME_RE.find_iter(part) {
            name_line.entry(m.as_str()).or_insert(lineno);
        }
    }
    let chain_text = chain
        .iter()
        .map(|(_, part)| *part)
        .collect::<Vec<_>>()
        .join(" ");
    if chain_text.is_empty() {
        return Vec::new();
    }
    let groups: Vec<Vec<&str>> = split_outside_braces(&chain_text)
        .into_iter()
        .map(|part| {
            LOCK_NAME_RE
                .find_iter(part)
                .map(|m| m.as_str())
                .collect::<Vec<_>>()
        })
        .filter(|group| !group.is_empty())
        .collect();
    let line_of = |name: &str| name_line.get(name).copied().unwrap_or(0);
    let mut out = Vec::new();
    for group in &groups {
        for &name in group {
            if !statics.contains_key(name) {
                out.push(Finding::new(
                    "lock-order",
                    "CLAUDE.md",
                    format!("unknown:{name}"),
                    format!(
                        "lock ordering names {name}, which is not a Mutex static in kernel/src"
                    ),
                    line_of(name),
                ));
            }
        }
    }
    for (i, earlier) in groups.iter().enumerate() {
        for later in &groups[i + 1..] {
            for &a in earlier {
                for &b in later {
                    let (Some(rank_a), Some(rank_b)) = (ranks.get(a), ranks.get(b)) else {
                        continue;
                    };
                    if rank_a > rank_b {
                        out.push(Finding::new(
                            "lock-order",
                            "CLAUDE.md",
                            format!("order:{a}>{b}"),
                            format!(
                                "CLAUDE.md orders {a} before {b}, §3.3 ranks them \
                                 {rank_a} and {rank_b}"
                            ),
                            line_of(b),
                        ));
                    }
                }
            }
        }
    }
    out
}

/// `lock ordering` comment blocks in every `kernel/src/**.rs` file, test files included
/// (check.py L898-925): a `//` line mentioning "lock ordering" (any case) starts a block that
/// runs over the following non-empty `//` lines; each lock name that is neither a static nor
/// a test lock is reported once per block.
fn comment_findings(repo: &Repo, statics: &BTreeMap<String, (String, usize)>) -> Vec<Finding> {
    let mut out = Vec::new();
    for f in repo.files() {
        if !(f.starts_with("kernel/src/") && f.ends_with(".rs")) {
            continue;
        }
        let text = repo.text(f);
        let lines = pystr::splitlines(&text);
        let mut i = 0;
        while i < lines.len() {
            let s = pystr::strip(lines[i]);
            if !(s.starts_with("//") && s.to_lowercase().contains("lock ordering")) {
                i += 1;
                continue;
            }
            let mut block = vec![(i + 1, s)];
            let mut j = i + 1;
            while j < lines.len() {
                let t = pystr::strip(lines[j]);
                if !t.starts_with("//") || pystr::strip(t.trim_end_matches(['/', '!'])).is_empty() {
                    break;
                }
                block.push((j + 1, t));
                j += 1;
            }
            let mut reported: HashSet<&str> = HashSet::new();
            for &(ln, t) in &block {
                for m in LOCK_NAME_RE.find_iter(t) {
                    let name = m.as_str();
                    if !statics.contains_key(name)
                        && !TEST_LOCKS.contains(&name)
                        && reported.insert(name)
                    {
                        out.push(Finding::new(
                            "lock-order",
                            f.as_str(),
                            format!("unknown:{name}"),
                            format!(
                                "lock-ordering comment names {name}, which is not a Mutex static"
                            ),
                            ln,
                        ));
                    }
                }
            }
            i = j;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regexes_compile() {
        let all: [&LazyLock<Regex>; 8] = [
            &LOCK_NAME_RE,
            &STATIC_RE,
            &TEST_MOD_RE,
            &MUTEX_RE,
            &TABLE_START,
            &TABLE_STOP,
            &LOCK_CELL_RE,
            &CHAIN_RE,
        ];
        // Forcing each LazyLock runs its Regex::new(...).expect("valid regex"): a
        // bad pattern panics here, at test time, rather than in production.
        for rx in all {
            LazyLock::force(rx);
        }
    }

    #[test]
    fn split_outside_braces_matches_the_lookahead_split() {
        // Expected values are Python's re.split(r">(?![^{]*})", s).
        assert_eq!(split_outside_braces("A > B"), ["A ", " B"]);
        assert_eq!(
            split_outside_braces("A > {C, D > E} > F"),
            ["A ", " {C, D > E} ", " F"]
        );
        assert_eq!(split_outside_braces("A>>B"), ["A", "", "B"]);
        assert_eq!(split_outside_braces("A } > B"), ["A } ", " B"]);
        assert_eq!(split_outside_braces("{A > B"), ["{A ", " B"]);
        assert_eq!(split_outside_braces("A"), ["A"]);
        assert_eq!(split_outside_braces(""), [""]);
        // Extra inputs checked against Python's re.split, beyond the cases above:
        // a stray `}` before the next `>`, and a `>` immediately inside `{...}`.
        assert_eq!(split_outside_braces("A > B} > C"), ["A > B} ", " C"]);
        assert_eq!(split_outside_braces("A>{B>C}>D"), ["A", "{B>C}", "D"]);
    }

    #[test]
    fn lock_cells_and_statics_match_like_check_py() {
        let cell = |c: &str| LOCK_CELL_RE.captures(c).map(|caps| caps[1].to_string());
        assert_eq!(cell("`BETA_LOCK[cpu]`").as_deref(), Some("BETA_LOCK"));
        assert_eq!(cell("`ALPHA`").as_deref(), Some("ALPHA"));
        assert_eq!(cell("`lower_lock`"), None);
        assert_eq!(cell("`A_LOCK` x"), None);
        let caps = STATIC_RE
            .captures("pub(crate) static BETA_LOCK: [spin::Mutex<()>; 4] = x;")
            .expect("a static line");
        assert_eq!(&caps[1], "BETA_LOCK");
        assert!(MUTEX_RE.is_match(&caps[2]));
        assert!(TEST_MOD_RE.is_match("pub(crate) mod tests {"));
        assert!(!TEST_MOD_RE.is_match("fn helper() {}"));
    }
}
