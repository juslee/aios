//! The check table, findings, the baseline file and the current-vs-baseline
//! comparison: check.py L47, L96-153 and L1402-1485.
//!
//! `Baseline` is a `serde_json::Map` built with the `preserve_order` feature, so
//! it behaves like the Python dict check.py keeps baseline entries in: raw
//! entries keep every field and its order, and re-inserting a key keeps its
//! position (L1621).
//!
//! Accepted divergences (contract §1.9; no golden covers them): the `{exc}` text
//! in `cannot read baseline {path}: {exc}` is Rust's `std::io::Error` or
//! `serde_json::Error` message rather than CPython's, with the same prefix, path
//! and exit code; a baselined `count` beyond `i64` — as a JSON number or a
//! digit string — is saturated to `i64::MAX` or `i64::MIN` by sign, where
//! CPython compares the exact integer; a `count` string of non-ASCII decimal
//! digits (e.g. Arabic-Indic `"٣"`) is rejected here, where CPython's `int()`
//! accepts them; a `count` string with a control separator (U+001C-U+001F)
//! around its digits is accepted here, because `pystr::strip` treats those as
//! whitespace before parsing, where CPython's `int()` does not strip them and
//! rejects the string; and a non-string `reason` is rendered as compact JSON
//! rather than a Python `repr`. Malformed baselines that make check.py raise (a
//! top level that is not an object, a `findings` value that is not a list, an
//! entry without a string `key` or `check`) are errors here: both exit 2.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::pystr;

/// The baseline path, relative to the repository root (check.py L47).
pub const BASELINE_REL: &str = "scripts/docs/baseline.json";

/// The baseline's `comment` field (check.py L1435-1437).
pub const BASELINE_COMMENT: &str = "Accepted docs drift. A finding is new when its key is missing here or it occurs on more lines than 'count' (default 1). 'reason' marks an accepted false positive and survives regeneration. Regenerate with: just docs-check --update-baseline";

/// `(name, description)` for every check, in CHECK_ORDER (check.py L96-112).
pub const CHECKS: [(&str, &str); 15] = [
    (
        "md-links",
        "relative [text](path) links resolve to a tracked file or directory",
    ),
    (
        "section-refs",
        "[x.md](path) §N resolves to a numbered heading (hub subfolders included)",
    ),
    (
        "anchors",
        "#fragment links resolve to a GitHub-style heading slug",
    ),
    (
        "wiki-links",
        "[[Note]] links resolve to a note in the docs/ vault",
    ),
    (
        "doc-map",
        "doc-map.md paths exist and every architecture doc is listed",
    ),
    (
        "repo-paths",
        "backticked kernel/ shared/ uefi-stub/ scripts/ paths exist (current-state docs)",
    ),
    (
        "just-recipes",
        "backticked `just X` recipes exist; public recipes are documented",
    ),
    (
        "test-count",
        "stated host test counts match #[test] in shared/src",
    ),
    (
        "lock-order",
        "production Mutex statics vs deadlock-prevention.md §3.3-3.4 and CLAUDE.md",
    ),
    (
        "milestone-status",
        "merged 'Phase N MK:' milestones vs phase docs, README, development-plan",
    ),
    (
        "phase-count",
        "phase counts in prose match the development-plan §8 table",
    ),
    (
        "layout",
        "kernel/src and shared/src modules vs CLAUDE.md layout and rule 05",
    ),
    (
        "harness-tables",
        "CLAUDE.md skills/agents tables and layout lists vs .claude/ (plugin skills as plugin:skill)",
    ),
    (
        "pointer-doctor",
        "CLAUDE.md sections, rules, paths, skills, agents, tools named by .claude/",
    ),
    (
        "knowledge-hygiene",
        "docs/knowledge naming, frontmatter, and an empty plans/ dir",
    ),
];

/// The check names, in the order check.py runs and reports them (L113).
pub const CHECK_ORDER: [&str; 15] = {
    let mut names = [""; 15];
    let mut i = 0;
    while i < CHECKS.len() {
        names[i] = CHECKS[i].0;
        i += 1;
    }
    names
};

/// `CHECK_ORDER.index(check)`, or 99 for a name check.py does not know
/// (check.py L1425 uses the same fallback when sorting baseline entries).
pub fn check_rank(check: &str) -> usize {
    CHECK_ORDER
        .iter()
        .position(|name| *name == check)
        .unwrap_or(99)
}

/// The `--list-checks` description of a check, or `""` when it is unknown.
pub fn description(check: &str) -> &'static str {
    CHECKS
        .iter()
        .find(|(name, _)| *name == check)
        .map_or("", |(_, text)| *text)
}

/// One drift finding (check.py L120-149). `message` goes into the baseline, so
/// it must not contain line numbers; volatile context goes in `detail`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub check: &'static str,
    pub file: String,
    pub target: String,
    pub message: String,
    pub line: usize,
    pub also: Vec<usize>,
    pub detail: String,
}

impl Finding {
    pub fn new(
        check: &'static str,
        file: impl Into<String>,
        target: impl Into<String>,
        message: impl Into<String>,
        line: usize,
    ) -> Finding {
        Finding {
            check,
            file: file.into(),
            target: target.into(),
            message: message.into(),
            line,
            also: Vec::new(),
            detail: String::new(),
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Finding {
        self.detail = detail.into();
        self
    }

    /// `f"{check}|{file}|{target}"`: no line numbers, so a finding that moves
    /// keeps its baseline entry.
    pub fn key(&self) -> String {
        format!("{}|{}|{}", self.check, self.file, self.target)
    }

    /// The number of distinct lines reporting this key (1 for file-level ones).
    pub fn count(&self) -> usize {
        1 + self.also.len()
    }

    pub fn location(&self) -> String {
        if self.line == 0 {
            return self.file.clone();
        }
        if self.also.is_empty() {
            return format!("{}:{}", self.file, self.line);
        }
        let also: Vec<String> = self.also.iter().map(usize::to_string).collect();
        format!("{}:{} (also {})", self.file, self.line, also.join(", "))
    }

    pub fn text(&self) -> String {
        if self.detail.is_empty() {
            self.message.clone()
        } else {
            format!("{} ({})", self.message, self.detail)
        }
    }
}

/// A check that cannot run in this environment (check.py's `Skip` exception).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip(pub String);

impl std::fmt::Display for Skip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Skip {}

/// Merge findings with equal keys and sort them, as check.py L1444-1455 does:
/// the first finding for a key is kept, later lines are appended to its `also`,
/// and the result is ordered by (check, file, line, target).
pub fn collate(findings: Vec<Finding>) -> Vec<Finding> {
    let mut order: Vec<Finding> = Vec::with_capacity(findings.len());
    let mut index: HashMap<String, usize> = HashMap::new();
    for f in findings {
        match index.entry(f.key()) {
            Entry::Vacant(slot) => {
                slot.insert(order.len());
                order.push(f);
            }
            Entry::Occupied(slot) => {
                let first = &mut order[*slot.get()];
                if f.line != 0 && f.line != first.line && !first.also.contains(&f.line) {
                    first.also.push(f.line);
                }
            }
        }
    }
    order.sort_by(|a, b| {
        (check_rank(a.check), &a.file, a.line, &a.target).cmp(&(
            check_rank(b.check),
            &b.file,
            b.line,
            &b.target,
        ))
    });
    order
}

/// Baseline entries by key, kept raw and in file order (check.py L1412).
pub type Baseline = Map<String, Value>;

/// `load_baseline` (check.py L1402-1412). `display` is the path as the error
/// message must print it: `--baseline` as given, or the root-joined default.
pub fn load_baseline(path: &Path, display: &str) -> Result<Baseline> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Baseline::new()),
        Err(err) => return Err(anyhow!("cannot read baseline {display}: {err}")),
    };
    let data: Value = match serde_json::from_slice(&bytes) {
        Ok(data) => data,
        Err(err) => return Err(anyhow!("cannot read baseline {display}: {err}")),
    };
    let Value::Object(object) = data else {
        bail!("baseline {display} is not a JSON object");
    };
    let mut entries = Baseline::new();
    match object.get("findings") {
        None => return Ok(entries),
        Some(Value::Array(items)) => {
            for item in items {
                let key = item
                    .get("key")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("baseline {display}: a findings entry has no 'key'"))?;
                entries.insert(key.to_string(), item.clone());
            }
        }
        Some(_) => bail!("baseline {display}: 'findings' is not a list"),
    }
    Ok(entries)
}

/// `int(entry.get("count", 1))` (check.py L1477).
pub fn baseline_count(entry: &Value) -> Result<i64> {
    let Some(value) = entry.get("count") else {
        return Ok(1);
    };
    match value {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else if n.as_u64().is_some() {
                Ok(i64::MAX)
            } else if let Some(f) = n.as_f64() {
                Ok(f.trunc() as i64)
            } else {
                Err(anyhow!("baseline count {value} is not an integer"))
            }
        }
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::String(s) => python_int(pystr::strip(s))
            .ok_or_else(|| anyhow!("baseline count {value} is not an integer")),
        _ => Err(anyhow!("baseline count {value} is not an integer")),
    }
}

/// `int(s)` for the forms a baseline `count` string can take: an optional sign,
/// ASCII digits, and single underscores between digits. A magnitude beyond
/// `i64` saturates to `i64::MAX`/`i64::MIN` by sign rather than failing, to
/// match the same clamp `baseline_count`'s numeric path already applies.
fn python_int(s: &str) -> Option<i64> {
    let (sign, digits) = match s.strip_prefix('-') {
        Some(rest) => (-1i64, rest),
        None => (1i64, s.strip_prefix('+').unwrap_or(s)),
    };
    if digits.is_empty() {
        return None;
    }
    let bytes = digits.as_bytes();
    let mut clean = String::with_capacity(digits.len());
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'_' {
            let between = i > 0
                && bytes[i - 1].is_ascii_digit()
                && bytes.get(i + 1).is_some_and(u8::is_ascii_digit);
            if !between {
                return None;
            }
            continue;
        }
        if !b.is_ascii_digit() {
            return None;
        }
        clean.push(char::from(*b));
    }
    Some(saturating_magnitude(&clean, sign))
}

/// A run of ASCII digits, signed and saturated to `i64::MAX`/`i64::MIN`: the
/// magnitude accumulates in `u128` with saturating arithmetic, so even a
/// pathologically long digit string cannot overflow or panic before the
/// final clamp is applied.
fn saturating_magnitude(digits: &str, sign: i64) -> i64 {
    let mut magnitude: u128 = 0;
    for b in digits.bytes() {
        let digit = u128::from(b - b'0');
        magnitude = magnitude.saturating_mul(10).saturating_add(digit);
    }
    if sign < 0 {
        let min_magnitude = i64::MIN.unsigned_abs() as u128;
        if magnitude >= min_magnitude {
            i64::MIN
        } else {
            -(magnitude as i64)
        }
    } else if magnitude > i64::MAX as u128 {
        i64::MAX
    } else {
        magnitude as i64
    }
}

/// `entry.get("check")` when it is a string (check.py L1484, L1619).
pub fn entry_check(entry: &Value) -> Option<&str> {
    entry.get("check").and_then(Value::as_str)
}

/// Python truthiness, which decides whether a `reason` marks a finding accepted.
pub fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// A JSON value as an f-string would print it: a string verbatim, anything else
/// as compact JSON (an accepted divergence from Python's `repr`).
pub fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The baseline record for a finding, keeping an accepted-false-positive
/// `reason` across rewrites (check.py L1415-1422).
pub fn baseline_entry(f: &Finding, old: Option<&Value>) -> Value {
    let mut entry = Map::new();
    entry.insert("key".to_string(), Value::String(f.key()));
    entry.insert("check".to_string(), Value::String(f.check.to_string()));
    entry.insert("file".to_string(), Value::String(f.file.clone()));
    entry.insert("target".to_string(), Value::String(f.target.clone()));
    entry.insert("message".to_string(), Value::String(f.message.clone()));
    if f.count() > 1 {
        entry.insert("count".to_string(), Value::from(f.count()));
    }
    let reason = old
        .and_then(|entry| entry.get("reason"))
        .filter(|value| py_truthy(value));
    if let Some(reason) = reason {
        entry.insert("reason".to_string(), reason.clone());
    }
    Value::Object(entry)
}

/// The entries `--update-baseline` writes (check.py L1619-1621): baselined
/// entries of checks that did not run are kept raw, the checks that ran are
/// replaced by their current findings.
pub fn updated_baseline(
    findings: &[Finding],
    baseline: &Baseline,
    ran: &BTreeSet<&str>,
) -> Baseline {
    let mut entries = Baseline::new();
    for (key, entry) in baseline {
        if entry_check(entry).is_none_or(|check| !ran.contains(check)) {
            entries.insert(key.clone(), entry.clone());
        }
    }
    for f in findings {
        let key = f.key();
        let old = baseline.get(&key).cloned();
        entries.insert(key, baseline_entry(f, old.as_ref()));
    }
    entries
}

#[derive(Serialize)]
struct BaselineFile<'a> {
    comment: &'a str,
    version: u32,
    counts: Map<String, Value>,
    findings: Vec<&'a Value>,
}

/// The whole baseline file, final newline included (check.py L1424-1439).
pub fn render_baseline(entries: &Baseline) -> Result<String> {
    let mut ordered: Vec<(usize, &str, &str, &Value)> = Vec::with_capacity(entries.len());
    for entry in entries.values() {
        let check = entry_check(entry)
            .ok_or_else(|| anyhow!("baseline entry without a string 'check': {entry}"))?;
        let key = entry
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("baseline entry without a string 'key': {entry}"))?;
        ordered.push((check_rank(check), key, check, entry));
    }
    ordered.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));

    let mut counts: HashMap<&str, usize> = HashMap::new();
    for (_, _, check, _) in &ordered {
        *counts.entry(*check).or_insert(0) += 1;
    }
    let mut counts_json = Map::new();
    for name in CHECK_ORDER {
        if let Some(n) = counts.get(name) {
            counts_json.insert(name.to_string(), Value::from(*n));
        }
    }

    let file = BaselineFile {
        comment: BASELINE_COMMENT,
        version: 1,
        counts: counts_json,
        findings: ordered.iter().map(|(_, _, _, entry)| *entry).collect(),
    };
    let mut text = serde_json::to_string_pretty(&file).context("serialize the baseline")?;
    text.push('\n');
    Ok(text)
}

/// Write the baseline file (check.py L1437-1439).
pub fn write_baseline(path: &Path, entries: &Baseline) -> Result<()> {
    let text = render_baseline(entries)?;
    std::fs::write(path, text).with_context(|| format!("cannot write baseline {}", path.display()))
}

/// Current findings against the baseline (check.py L1459-1485).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Comparison {
    /// Keys missing from the baseline, or occurring on more lines than it says.
    pub new_keys: BTreeSet<String>,
    /// Key to baselined count, for keys whose count grew.
    pub grown: BTreeMap<String, i64>,
    /// Key to reason, for baselined false positives.
    pub accepted: BTreeMap<String, Value>,
    /// Baselined keys (of checks that ran) that no longer occur, sorted.
    pub resolved: Vec<String>,
    /// Key to (baselined count, current count), for keys that shrank.
    pub reduced: BTreeMap<String, (i64, i64)>,
}

pub fn compare(
    findings: &[Finding],
    baseline: &Baseline,
    ran: &BTreeSet<&str>,
) -> Result<Comparison> {
    let mut cmp = Comparison::default();
    let mut current: BTreeSet<String> = BTreeSet::new();
    for f in findings {
        let key = f.key();
        current.insert(key.clone());
        let Some(entry) = baseline.get(&key) else {
            cmp.new_keys.insert(key);
            continue;
        };
        let base = baseline_count(entry)?;
        let count = f.count() as i64;
        match count.cmp(&base) {
            std::cmp::Ordering::Greater => {
                cmp.new_keys.insert(key.clone());
                cmp.grown.insert(key.clone(), base);
            }
            std::cmp::Ordering::Less => {
                cmp.reduced.insert(key.clone(), (base, count));
            }
            std::cmp::Ordering::Equal => {}
        }
        if let Some(reason) = entry.get("reason").filter(|value| py_truthy(value)) {
            cmp.accepted.insert(key, reason.clone());
        }
    }
    cmp.resolved = baseline
        .iter()
        .filter(|(key, entry)| {
            entry_check(entry).is_some_and(|check| ran.contains(check)) && !current.contains(*key)
        })
        .map(|(key, _)| key.clone())
        .collect();
    cmp.resolved.sort();
    Ok(cmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// A private directory under the system temp dir, removed when dropped.
    struct TempTree {
        root: PathBuf,
    }

    impl TempTree {
        fn new(label: &str) -> TempTree {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("aios-tools-{label}-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("create the temp tree");
            TempTree { root }
        }

        fn join(&self, name: &str) -> PathBuf {
            self.root.join(name)
        }

        fn write(&self, name: &str, content: &str) -> PathBuf {
            let path = self.join(name);
            std::fs::write(&path, content).expect("write a temp file");
            path
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the tools crate has a parent directory")
            .to_path_buf()
    }

    fn finding(check: &'static str, file: &str, target: &str, line: usize) -> Finding {
        Finding::new(check, file, target, format!("{target} drifted"), line)
    }

    /// The bytes check.py's write_baseline produces for the entries built in
    /// `updated_baseline_keeps_unrelated_and_accepted_entries` (recorded).
    const EXPECTED_BASELINE: &str = r##"{
  "comment": "Accepted docs drift. A finding is new when its key is missing here or it occurs on more lines than 'count' (default 1). 'reason' marks an accepted false positive and survives regeneration. Regenerate with: just docs-check --update-baseline",
  "version": 1,
  "counts": {
    "md-links": 1,
    "anchors": 1
  },
  "findings": [
    {
      "key": "md-links|a.md|t",
      "check": "md-links",
      "file": "a.md",
      "target": "t",
      "message": "new message",
      "count": 2,
      "reason": "accepted on purpose"
    },
    {
      "key": "anchors|a.md|#x",
      "check": "anchors",
      "file": "a.md",
      "target": "#x",
      "message": "gone"
    }
  ]
}
"##;

    #[test]
    fn the_model_check_table_matches_check_py() {
        assert_eq!(CHECKS.len(), 15);
        assert_eq!(CHECK_ORDER.len(), 15);
        for (i, (name, _)) in CHECKS.iter().enumerate() {
            assert_eq!(CHECK_ORDER[i], *name);
        }
        assert_eq!(CHECK_ORDER[0], "md-links");
        assert_eq!(CHECK_ORDER[14], "knowledge-hygiene");
        assert_eq!(
            description("repo-paths"),
            "backticked kernel/ shared/ uefi-stub/ scripts/ paths exist (current-state docs)"
        );
        assert_eq!(
            description("just-recipes"),
            "backticked `just X` recipes exist; public recipes are documented"
        );
        assert_eq!(description("nope"), "");
        assert_eq!(check_rank("md-links"), 0);
        assert_eq!(check_rank("knowledge-hygiene"), 14);
        assert_eq!(check_rank("nope"), 99);
        assert_eq!(BASELINE_REL, "scripts/docs/baseline.json");
    }

    #[test]
    fn model_finding_key_count_location_and_text() {
        let mut f = Finding::new(
            "md-links",
            "docs/a.md",
            "./b.md",
            "broken link -> ./b.md",
            12,
        );
        assert_eq!(f.key(), "md-links|docs/a.md|./b.md");
        assert_eq!(f.count(), 1);
        assert_eq!(f.location(), "docs/a.md:12");
        assert_eq!(f.text(), "broken link -> ./b.md");

        f.also = vec![20, 31];
        assert_eq!(f.count(), 3);
        assert_eq!(f.location(), "docs/a.md:12 (also 20, 31)");

        let file_level = Finding::new("layout", "CLAUDE.md", "stale:kernel/src/x/", "m", 0);
        assert_eq!(file_level.location(), "CLAUDE.md");

        let detailed = Finding::new(
            "lock-order",
            "docs/kernel/deadlock-prevention.md",
            "undocumented:A_LOCK",
            "production lock A_LOCK is not in §3.3/§3.4",
            0,
        )
        .with_detail("defined at kernel/src/a.rs:5");
        assert_eq!(
            detailed.text(),
            "production lock A_LOCK is not in §3.3/§3.4 (defined at kernel/src/a.rs:5)"
        );
    }

    #[test]
    fn model_skip_prints_only_its_message() {
        let skip = Skip("no justfile".to_string());
        assert_eq!(skip.to_string(), "no justfile");
        let boxed: Box<dyn std::error::Error> = Box::new(skip);
        assert_eq!(boxed.to_string(), "no justfile");
    }

    #[test]
    fn model_collate_merges_keys_and_sorts_like_run_checks() {
        let findings = vec![
            finding("anchors", "b.md", "t2", 9),
            finding("md-links", "b.md", "t1", 5),
            finding("md-links", "b.md", "t1", 5), // same line: ignored
            finding("md-links", "b.md", "t1", 0), // line 0: ignored
            finding("md-links", "b.md", "t1", 7), // appended to `also`
            finding("md-links", "b.md", "t1", 7), // already in `also`
            finding("md-links", "a.md", "t0", 3),
            finding("md-links", "b.md", "t0", 5),
        ];
        let out = collate(findings);
        let keys: Vec<String> = out.iter().map(Finding::key).collect();
        assert_eq!(
            keys,
            vec![
                "md-links|a.md|t0".to_string(),
                "md-links|b.md|t0".to_string(),
                "md-links|b.md|t1".to_string(),
                "anchors|b.md|t2".to_string(),
            ]
        );
        assert_eq!(out[2].also, vec![7]);
        assert_eq!(out[2].count(), 2);
        assert_eq!(out[2].location(), "b.md:5 (also 7)");
    }

    #[test]
    fn model_load_baseline_reads_entries_and_reports_failures() {
        let tree = TempTree::new("model-baseline");

        assert!(load_baseline(&tree.join("nope.json"), "nope.json")
            .expect("a missing baseline is empty")
            .is_empty());

        let good = tree.write(
            "good.json",
            r#"{"findings": [{"key": "a|b|c", "check": "md-links", "count": 2},
                            {"key": "d|e|f", "check": "anchors"}]}"#,
        );
        let entries = load_baseline(&good, "good.json").expect("a valid baseline loads");
        assert_eq!(
            entries.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["a|b|c", "d|e|f"],
            "entries keep file order"
        );
        assert_eq!(baseline_count(&entries["a|b|c"]).expect("count"), 2);
        assert_eq!(baseline_count(&entries["d|e|f"]).expect("default count"), 1);
        assert_eq!(entry_check(&entries["d|e|f"]), Some("anchors"));

        let broken = tree.write("broken.json", "{");
        let err = load_baseline(&broken, "broken.json").expect_err("invalid JSON");
        assert!(
            format!("{err:#}").starts_with("cannot read baseline broken.json: "),
            "{err:#}"
        );

        let err = load_baseline(&tree.root, "a-directory").expect_err("a directory");
        assert!(
            format!("{err:#}").starts_with("cannot read baseline a-directory: "),
            "{err:#}"
        );

        let list = tree.write("list.json", "[]");
        assert!(load_baseline(&list, "list.json").is_err());

        let keyless = tree.write("keyless.json", r#"{"findings": [{"check": "md-links"}]}"#);
        assert!(load_baseline(&keyless, "keyless.json").is_err());

        let empty = tree.write("empty.json", r#"{"version": 1}"#);
        assert!(load_baseline(&empty, "empty.json")
            .expect("no findings key")
            .is_empty());
    }

    #[test]
    fn model_baseline_count_follows_python_int() {
        let with_count = |value: Value| json!({ "count": value });
        assert_eq!(baseline_count(&json!({})).expect("default"), 1);
        assert_eq!(baseline_count(&with_count(json!(3))).expect("int"), 3);
        assert_eq!(baseline_count(&with_count(json!(2.9))).expect("float"), 2);
        assert_eq!(baseline_count(&with_count(json!(true))).expect("bool"), 1);
        assert_eq!(
            baseline_count(&with_count(json!(" 1_0 "))).expect("string"),
            10
        );
        assert!(baseline_count(&with_count(json!("x"))).is_err());
        assert!(baseline_count(&with_count(json!(null))).is_err());
        assert!(baseline_count(&with_count(json!([]))).is_err());
        assert_eq!(
            baseline_count(&with_count(json!("99999999999999999999"))).expect("string overflow"),
            i64::MAX,
            "a string count beyond i64::MAX saturates, like the JSON-number path"
        );
        assert_eq!(
            baseline_count(&with_count(json!("-9223372036854775808"))).expect("string i64::MIN"),
            i64::MIN,
            "a string count at exactly i64::MIN parses exactly, not off by one"
        );
        assert_eq!(
            baseline_count(&with_count(json!(u64::MAX))).expect("json number overflow"),
            i64::MAX,
            "a JSON number beyond i64::MAX is clamped by the existing numeric path"
        );
    }

    #[test]
    fn model_python_truthiness_and_text() {
        assert!(!py_truthy(&json!(null)));
        assert!(!py_truthy(&json!("")));
        assert!(!py_truthy(&json!(0)));
        assert!(!py_truthy(&json!(false)));
        assert!(!py_truthy(&json!([])));
        assert!(!py_truthy(&json!({})));
        assert!(py_truthy(&json!("x")));
        assert!(py_truthy(&json!(1)));
        assert_eq!(py_str(&json!("plain text")), "plain text");
        assert_eq!(py_str(&json!({"a": 1})), "{\"a\":1}");
    }

    #[test]
    fn model_updated_baseline_keeps_unrelated_and_accepted_entries() {
        let mut baseline = Baseline::new();
        baseline.insert(
            "md-links|a.md|t".to_string(),
            json!({"key": "md-links|a.md|t", "check": "md-links", "file": "a.md",
                   "target": "t", "message": "old message", "count": 3,
                   "reason": "accepted on purpose"}),
        );
        baseline.insert(
            "anchors|a.md|#x".to_string(),
            json!({"key": "anchors|a.md|#x", "check": "anchors", "file": "a.md",
                   "target": "#x", "message": "gone"}),
        );

        let mut f = Finding::new("md-links", "a.md", "t", "new message", 4);
        f.also = vec![9];
        let ran: BTreeSet<&str> = ["md-links"].into_iter().collect();
        let updated = updated_baseline(&[f], &baseline, &ran);

        assert_eq!(updated.len(), 2);
        assert_eq!(updated["anchors|a.md|#x"]["message"], json!("gone"));
        let entry = &updated["md-links|a.md|t"];
        assert_eq!(entry["message"], json!("new message"));
        assert_eq!(entry["count"], json!(2));
        assert_eq!(entry["reason"], json!("accepted on purpose"));

        assert_eq!(
            render_baseline(&updated).expect("render"),
            EXPECTED_BASELINE
        );
    }

    #[test]
    fn model_render_baseline_reproduces_the_repository_baseline() {
        let path = repo_root().join("scripts/docs/baseline.json");
        let raw = std::fs::read_to_string(&path).expect("read scripts/docs/baseline.json");
        let entries =
            load_baseline(&path, "scripts/docs/baseline.json").expect("load the baseline");
        assert!(!entries.is_empty());
        assert_eq!(render_baseline(&entries).expect("render"), raw);
    }

    // Bytes-on-disk parity: proves write_baseline writes exactly what
    // render_baseline produces (no extra newline, no BOM, no truncation).
    // The neighbouring `model_render_baseline_reproduces_the_repository_baseline`
    // test is what pins `render_baseline` itself against the real baseline file.
    #[test]
    fn model_write_baseline_writes_the_rendered_bytes() {
        let tree = TempTree::new("model-write");
        let path = tree.join("out.json");
        let mut entries = Baseline::new();
        entries.insert(
            "md-links|a.md|t".to_string(),
            json!({"key": "md-links|a.md|t", "check": "md-links", "file": "a.md",
                   "target": "t", "message": "m"}),
        );
        write_baseline(&path, &entries).expect("write the baseline");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read it back"),
            render_baseline(&entries).expect("render")
        );
    }

    #[test]
    fn model_compare_marks_new_grown_reduced_accepted_and_resolved() {
        let mut baseline = Baseline::new();
        for (key, entry) in [
            ("md-links|a.md|same", json!({"check": "md-links"})),
            (
                "md-links|a.md|grown",
                json!({"check": "md-links", "count": 2, "reason": "known"}),
            ),
            (
                "md-links|a.md|shrunk",
                json!({"check": "md-links", "count": 4}),
            ),
            ("md-links|a.md|gone", json!({"check": "md-links"})),
            ("anchors|a.md|skipped", json!({"check": "anchors"})),
        ] {
            baseline.insert(key.to_string(), entry);
        }

        let same = Finding::new("md-links", "a.md", "same", "m", 1);
        let mut grown = Finding::new("md-links", "a.md", "grown", "m", 1);
        grown.also = vec![2, 3];
        let mut shrunk = Finding::new("md-links", "a.md", "shrunk", "m", 1);
        shrunk.also = vec![2];
        let fresh = Finding::new("md-links", "a.md", "fresh", "m", 1);

        let ran: BTreeSet<&str> = ["md-links"].into_iter().collect();
        let cmp = compare(&[same, grown, shrunk, fresh], &baseline, &ran).expect("compare");

        let expected_new: BTreeSet<String> = [
            "md-links|a.md|fresh".to_string(),
            "md-links|a.md|grown".to_string(),
        ]
        .into_iter()
        .collect();
        assert_eq!(cmp.new_keys, expected_new);
        assert_eq!(cmp.grown.get("md-links|a.md|grown"), Some(&2));
        assert_eq!(cmp.reduced.get("md-links|a.md|shrunk"), Some(&(4, 2)));
        assert_eq!(
            cmp.accepted.get("md-links|a.md|grown"),
            Some(&json!("known"))
        );
        assert_eq!(cmp.resolved, vec!["md-links|a.md|gone".to_string()]);
    }
}
