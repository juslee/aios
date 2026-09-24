//! Report renderers of `aios docs-check`: check.py `describe`, `prune_notes`,
//! `render_text` and `render_markdown` (L1488-1573), the `--json` document built in
//! `main` (L1629-1657) and `--list-checks` (L1597-1600), at 33c6b3d.
//!
//! Each renderer returns check.py's bytes. `render_text` and `render_json` return
//! them without the final newline that Python's `print` adds; the caller adds it.
//! The exception is a non-string `reason` from a hand-edited baseline: its
//! rendering follows serde_json, not Python (listed in the `model` module doc).

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::cmd::docs_check::checks::Check;
use crate::cmd::docs_check::model::{self, Baseline, Comparison, Finding};

/// Findings listed in the prune note before it is truncated (check.py L1497).
const PRUNE_LIMIT: usize = 50;
/// New findings listed in the Markdown summary (check.py L1565).
const MARKDOWN_LIMIT: usize = 100;
/// Check-name column width in the text summary and `--list-checks` (check.py L1513,
/// L1516, L1520, L1599).
const NAME_WIDTH: usize = 18;
/// Total/new column width in the text summary (check.py L1513, L1516, L1520).
const COUNT_WIDTH: usize = 5;

/// Everything one rendering needs.
pub struct Report<'a> {
    /// Collated findings (check.py `run_checks` order).
    pub findings: &'a [Finding],
    pub cmp: &'a Comparison,
    /// Skip message of every selected check that could not run.
    pub skipped: &'a BTreeMap<&'static str, String>,
    /// Selected checks in CHECK_ORDER, skipped ones included.
    pub names: &'a [&'static str],
    pub baseline: &'a Baseline,
    /// Baseline path relative to the repository root.
    pub baseline_rel: &'a str,
}

impl Report<'_> {
    fn is_new(&self, f: &Finding) -> bool {
        self.cmp.new_keys.contains(&f.key())
    }

    fn new_findings(&self) -> Vec<&Finding> {
        self.findings.iter().filter(|f| self.is_new(f)).collect()
    }

    fn total_of(&self, name: &str) -> usize {
        self.findings.iter().filter(|f| f.check == name).count()
    }

    fn new_of(&self, name: &str) -> usize {
        self.findings
            .iter()
            .filter(|f| f.check == name && self.is_new(f))
            .count()
    }
}

/// check.py `describe` (L1488-1494): the finding text plus the grown and accepted notes.
pub fn describe(f: &Finding, cmp: &Comparison) -> String {
    let key = f.key();
    let mut text = f.text();
    if let Some(base) = cmp.grown.get(&key) {
        text.push_str(&format!(" [{} occurrences, baseline {base}]", f.count()));
    }
    if let Some(reason) = cmp.accepted.get(&key) {
        text.push_str(&format!(" [accepted: {}]", model::py_str(reason)));
    }
    text
}

/// check.py `prune_notes` (L1497-1502): resolved keys, then reduced keys, at most
/// `limit` lines plus a `... and N more` line.
pub fn prune_notes(cmp: &Comparison, limit: usize) -> Vec<String> {
    let mut notes: Vec<String> = cmp
        .resolved
        .iter()
        .map(|key| format!("  - {key}"))
        .collect();
    notes.extend(
        cmp.reduced
            .iter()
            .map(|(key, (base, current))| format!("  - {key} ({base} -> {current} occurrences)")),
    );
    if notes.len() > limit {
        let more = notes.len() - limit;
        notes.truncate(limit);
        notes.push(format!("  ... and {more} more"));
    }
    notes
}

/// check.py `render_text` (L1505-1542), without the final newline.
pub fn render_text(r: &Report<'_>, show_all: bool) -> String {
    let new = r.new_findings();
    let total = r.findings.len();
    let mut out = vec![
        format!(
            "docs-check: {total} findings across {} checks - {} new, {} baselined ({} accepted false positives), {} resolved (baseline {})",
            r.names.len().saturating_sub(r.skipped.len()),
            new.len(),
            total.saturating_sub(new.len()),
            r.cmp.accepted.len(),
            r.cmp.resolved.len(),
            r.baseline_rel,
        ),
        String::new(),
        format!("  {:<NAME_WIDTH$} {:>COUNT_WIDTH$} {:>COUNT_WIDTH$}", "check", "total", "new"),
    ];
    for &name in r.names {
        match r.skipped.get(name) {
            Some(message) => out.push(format!(
                "  {name:<NAME_WIDTH$} {:>COUNT_WIDTH$}        {message}",
                "skip"
            )),
            None => {
                let fresh = new.iter().filter(|f| f.check == name).count();
                out.push(format!(
                    "  {name:<NAME_WIDTH$} {:>COUNT_WIDTH$} {fresh:>COUNT_WIDTH$}",
                    r.total_of(name)
                ));
            }
        }
    }
    let shown: Vec<&Finding> = if show_all {
        r.findings.iter().collect()
    } else {
        new
    };
    if !shown.is_empty() {
        let heading = if show_all {
            "All findings ('+' = new since baseline, '~' = accepted false positive):"
        } else {
            "New drift since baseline:"
        };
        out.push(String::new());
        out.push(heading.to_string());
        let mut current: Option<&str> = None;
        for f in shown {
            if current != Some(f.check) {
                current = Some(f.check);
                out.push(format!("\n[{}]", f.check));
            }
            let key = f.key();
            let mark = if r.cmp.new_keys.contains(&key) {
                '+'
            } else if r.cmp.accepted.contains_key(&key) {
                '~'
            } else {
                ' '
            };
            out.push(format!(" {mark} {}: {}", f.location(), describe(f, r.cmp)));
        }
    } else if !show_all {
        out.push(String::new());
        out.push("No new drift since baseline.".to_string());
    }
    let notes = prune_notes(r.cmp, PRUNE_LIMIT);
    if !notes.is_empty() {
        out.push(String::new());
        out.push(format!(
            "{} baselined finding(s) no longer occur or occur less often; prune them with `just docs-check --update-baseline`:",
            r.cmp.resolved.len() + r.cmp.reduced.len()
        ));
        out.extend(notes);
    }
    out.join("\n")
}

/// check.py `render_markdown` (L1545-1573), ending with one newline.
pub fn render_markdown(r: &Report<'_>) -> String {
    let new = r.new_findings();
    let mut out = vec![
        "## Docs drift check".to_string(),
        String::new(),
        format!(
            "**{} new** finding(s) since baseline; {} total ({} accepted false positives), {} resolved. Report-only: fix new drift in this PR, or accept it with `just docs-check --update-baseline`.",
            new.len(),
            r.findings.len(),
            r.cmp.accepted.len(),
            r.cmp.resolved.len(),
        ),
        String::new(),
        "| Check | Total | New |".to_string(),
        "|---|---:|---:|".to_string(),
    ];
    for &name in r.names {
        match r.skipped.get(name) {
            Some(message) => out.push(format!("| {name} | skipped | {message} |")),
            None => {
                let fresh = new.iter().filter(|f| f.check == name).count();
                out.push(format!("| {name} | {} | {fresh} |", r.total_of(name)));
            }
        }
    }
    if !new.is_empty() {
        out.push(String::new());
        out.push("### New findings".to_string());
        out.push(String::new());
        for f in new.iter().take(MARKDOWN_LIMIT) {
            out.push(format!(
                "- `{}` {}: {}",
                f.check,
                f.location(),
                describe(f, r.cmp)
            ));
        }
        if new.len() > MARKDOWN_LIMIT {
            out.push(format!("- ... and {} more", new.len() - MARKDOWN_LIMIT));
        }
    }
    if !(r.cmp.resolved.is_empty() && r.cmp.reduced.is_empty()) {
        out.push(String::new());
        out.push(format!(
            "{} baselined finding(s) no longer occur or occur less often (run `just docs-check --update-baseline`).",
            r.cmp.resolved.len() + r.cmp.reduced.len()
        ));
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

#[derive(Serialize)]
struct JsonReport<'a> {
    baseline: &'a str,
    summary: JsonSummary,
    checks: Map<String, Value>,
    findings: Vec<JsonFinding<'a>>,
    resolved: &'a [String],
    reduced: Map<String, Value>,
}

#[derive(Serialize)]
struct JsonSummary {
    total: usize,
    new: usize,
    baselined: usize,
    accepted: usize,
    resolved: usize,
    reduced: usize,
}

#[derive(Serialize)]
struct JsonFinding<'a> {
    key: String,
    check: &'a str,
    file: &'a str,
    line: usize,
    also: &'a [usize],
    count: usize,
    baseline_count: Option<i64>,
    target: &'a str,
    message: &'a str,
    detail: &'a str,
    new: bool,
    accepted: Option<&'a Value>,
}

/// check.py's `--json` document (L1629-1657), key order included, without the
/// final newline. Lists every finding with `show_all`, else only the new ones.
pub fn render_json(r: &Report<'_>, show_all: bool) -> anyhow::Result<String> {
    let mut checks = Map::new();
    for &name in r.names {
        let value = match r.skipped.get(name) {
            Some(message) => json!({ "skipped": message }),
            None => json!({ "total": r.total_of(name), "new": r.new_of(name) }),
        };
        checks.insert(name.to_string(), value);
    }
    let mut findings = Vec::new();
    for f in r.findings {
        let key = f.key();
        let new = r.cmp.new_keys.contains(&key);
        if !(show_all || new) {
            continue;
        }
        let baseline_count = r
            .baseline
            .get(&key)
            .map(model::baseline_count)
            .transpose()?;
        let accepted = r.cmp.accepted.get(&key);
        findings.push(JsonFinding {
            key,
            check: f.check,
            file: &f.file,
            line: f.line,
            also: &f.also,
            count: f.count(),
            baseline_count,
            target: &f.target,
            message: &f.message,
            detail: &f.detail,
            new,
            accepted,
        });
    }
    let mut reduced = Map::new();
    for (key, (base, current)) in &r.cmp.reduced {
        reduced.insert(key.clone(), json!({ "baseline": base, "current": current }));
    }
    let report = JsonReport {
        baseline: r.baseline_rel,
        summary: JsonSummary {
            total: r.findings.len(),
            new: r.cmp.new_keys.len(),
            baselined: r.findings.len().saturating_sub(r.cmp.new_keys.len()),
            accepted: r.cmp.accepted.len(),
            resolved: r.cmp.resolved.len(),
            reduced: r.cmp.reduced.len(),
        },
        checks,
        findings,
        resolved: &r.cmp.resolved,
        reduced,
    };
    Ok(serde_json::to_string_pretty(&report)?)
}

/// check.py `--list-checks` (L1597-1600): `{name:<18} {description}` per check.
pub fn render_list_checks(checks: &[Box<dyn Check>]) -> String {
    let mut out = String::new();
    for check in checks {
        out.push_str(&format!(
            "{:<NAME_WIDTH$} {}\n",
            check.name(),
            check.describe()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::docs_check::repo::Repo;

    // Expected strings were recorded from check.py's render_text, render_markdown and
    // main()'s JSON document (33c6b3d) on the same findings and comparison.

    fn finding(
        check: &'static str,
        file: &str,
        target: &str,
        message: &str,
        line: usize,
        also: &[usize],
    ) -> Finding {
        let mut f = Finding::new(check, file, target, message, line);
        f.also = also.to_vec();
        f
    }

    fn sample_findings() -> Vec<Finding> {
        vec![
            finding(
                "md-links",
                "docs/a.md",
                "x.md",
                "broken link -> x.md",
                3,
                &[9],
            ),
            finding(
                "md-links",
                "docs/b.md",
                "old.md",
                "broken link -> old.md",
                5,
                &[6, 7],
            ),
            finding(
                "anchors",
                "docs/a.md",
                "#gone",
                "no heading for anchor #gone in docs/a.md",
                4,
                &[],
            ),
            finding(
                "pointer-doctor",
                ".claude/agents/w.md",
                "path:docs/r.md",
                "path does not exist: docs/r.md",
                0,
                &[],
            )
            .with_detail("cited on purpose"),
        ]
    }

    fn sample_cmp() -> Comparison {
        Comparison {
            new_keys: ["md-links|docs/a.md|x.md", "md-links|docs/b.md|old.md"]
                .into_iter()
                .map(String::from)
                .collect(),
            grown: BTreeMap::from([("md-links|docs/b.md|old.md".to_string(), 2)]),
            accepted: BTreeMap::from([(
                "anchors|docs/a.md|#gone".to_string(),
                Value::from("false positive: generated anchor"),
            )]),
            resolved: vec!["md-links|docs/c.md|gone.md".to_string()],
            reduced: BTreeMap::from([(
                "pointer-doctor|.claude/agents/w.md|path:docs/r.md".to_string(),
                (2, 1),
            )]),
        }
    }

    fn sample_baseline() -> Baseline {
        [
            json!({"key": "md-links|docs/b.md|old.md", "check": "md-links", "count": 2}),
            json!({"key": "anchors|docs/a.md|#gone", "check": "anchors",
                   "reason": "false positive: generated anchor"}),
            json!({"key": "pointer-doctor|.claude/agents/w.md|path:docs/r.md",
                   "check": "pointer-doctor", "count": 2}),
            json!({"key": "md-links|docs/c.md|gone.md", "check": "md-links"}),
            json!({"key": "milestone-status|README.md|latest:M9", "check": "milestone-status"}),
        ]
        .into_iter()
        .map(|entry| (entry["key"].as_str().unwrap_or_default().to_string(), entry))
        .collect()
    }

    fn sample_skipped() -> BTreeMap<&'static str, String> {
        BTreeMap::from([(
            "milestone-status",
            "no 'Phase N MK:' commits found on main's first-parent history".to_string(),
        )])
    }

    const NAMES: [&str; 4] = ["md-links", "anchors", "milestone-status", "pointer-doctor"];

    const TEXT_HEAD: &str = r"docs-check: 4 findings across 3 checks - 2 new, 2 baselined (1 accepted false positives), 1 resolved (baseline scripts/docs/baseline.json)

  check              total   new
  md-links               2     2
  anchors                1     0
  milestone-status    skip        no 'Phase N MK:' commits found on main's first-parent history
  pointer-doctor         1     0
";

    const TEXT_PRUNE: &str = r"
2 baselined finding(s) no longer occur or occur less often; prune them with `just docs-check --update-baseline`:
  - md-links|docs/c.md|gone.md
  - pointer-doctor|.claude/agents/w.md|path:docs/r.md (2 -> 1 occurrences)";

    const TEXT_NEW: &str = r"
New drift since baseline:

[md-links]
 + docs/a.md:3 (also 9): broken link -> x.md
 + docs/b.md:5 (also 6, 7): broken link -> old.md [3 occurrences, baseline 2]
";

    const TEXT_ALL: &str = r"
All findings ('+' = new since baseline, '~' = accepted false positive):

[md-links]
 + docs/a.md:3 (also 9): broken link -> x.md
 + docs/b.md:5 (also 6, 7): broken link -> old.md [3 occurrences, baseline 2]

[anchors]
 ~ docs/a.md:4: no heading for anchor #gone in docs/a.md [accepted: false positive: generated anchor]

[pointer-doctor]
   .claude/agents/w.md: path does not exist: docs/r.md (cited on purpose)
";

    const MARKDOWN: &str = r"## Docs drift check

**2 new** finding(s) since baseline; 4 total (1 accepted false positives), 1 resolved. Report-only: fix new drift in this PR, or accept it with `just docs-check --update-baseline`.

| Check | Total | New |
|---|---:|---:|
| md-links | 2 | 2 |
| anchors | 1 | 0 |
| milestone-status | skipped | no 'Phase N MK:' commits found on main's first-parent history |
| pointer-doctor | 1 | 0 |

### New findings

- `md-links` docs/a.md:3 (also 9): broken link -> x.md
- `md-links` docs/b.md:5 (also 6, 7): broken link -> old.md [3 occurrences, baseline 2]

2 baselined finding(s) no longer occur or occur less often (run `just docs-check --update-baseline`).
";

    const JSON_HEAD: &str = r##"{
  "baseline": "scripts/docs/baseline.json",
  "summary": {
    "total": 4,
    "new": 2,
    "baselined": 2,
    "accepted": 1,
    "resolved": 1,
    "reduced": 1
  },
  "checks": {
    "md-links": {
      "total": 2,
      "new": 2
    },
    "anchors": {
      "total": 1,
      "new": 0
    },
    "milestone-status": {
      "skipped": "no 'Phase N MK:' commits found on main's first-parent history"
    },
    "pointer-doctor": {
      "total": 1,
      "new": 0
    }
  },
  "findings": [
    {
      "key": "md-links|docs/a.md|x.md",
      "check": "md-links",
      "file": "docs/a.md",
      "line": 3,
      "also": [
        9
      ],
      "count": 2,
      "baseline_count": null,
      "target": "x.md",
      "message": "broken link -> x.md",
      "detail": "",
      "new": true,
      "accepted": null
    },
    {
      "key": "md-links|docs/b.md|old.md",
      "check": "md-links",
      "file": "docs/b.md",
      "line": 5,
      "also": [
        6,
        7
      ],
      "count": 3,
      "baseline_count": 2,
      "target": "old.md",
      "message": "broken link -> old.md",
      "detail": "",
      "new": true,
      "accepted": null
    }"##;

    const JSON_BASELINED: &str = r##",
    {
      "key": "anchors|docs/a.md|#gone",
      "check": "anchors",
      "file": "docs/a.md",
      "line": 4,
      "also": [],
      "count": 1,
      "baseline_count": 1,
      "target": "#gone",
      "message": "no heading for anchor #gone in docs/a.md",
      "detail": "",
      "new": false,
      "accepted": "false positive: generated anchor"
    },
    {
      "key": "pointer-doctor|.claude/agents/w.md|path:docs/r.md",
      "check": "pointer-doctor",
      "file": ".claude/agents/w.md",
      "line": 0,
      "also": [],
      "count": 1,
      "baseline_count": 2,
      "target": "path:docs/r.md",
      "message": "path does not exist: docs/r.md",
      "detail": "cited on purpose",
      "new": false,
      "accepted": null
    }"##;

    const JSON_TAIL: &str = r##"
  ],
  "resolved": [
    "md-links|docs/c.md|gone.md"
  ],
  "reduced": {
    "pointer-doctor|.claude/agents/w.md|path:docs/r.md": {
      "baseline": 2,
      "current": 1
    }
  }
}"##;

    fn with_sample<T>(render: impl FnOnce(&Report<'_>) -> T) -> T {
        let findings = sample_findings();
        let cmp = sample_cmp();
        let skipped = sample_skipped();
        let baseline = sample_baseline();
        render(&Report {
            findings: &findings,
            cmp: &cmp,
            skipped: &skipped,
            names: &NAMES,
            baseline: &baseline,
            baseline_rel: "scripts/docs/baseline.json",
        })
    }

    #[test]
    fn text_matches_check_py() {
        let text = with_sample(|r| render_text(r, false));
        assert_eq!(text, format!("{TEXT_HEAD}{TEXT_NEW}{TEXT_PRUNE}"));
        let all = with_sample(|r| render_text(r, true));
        assert_eq!(all, format!("{TEXT_HEAD}{TEXT_ALL}{TEXT_PRUNE}"));
    }

    #[test]
    fn markdown_matches_check_py() {
        assert_eq!(with_sample(render_markdown), MARKDOWN);
    }

    #[test]
    fn json_matches_check_py() {
        let new_only = with_sample(|r| render_json(r, false)).expect("render JSON");
        assert_eq!(new_only, format!("{JSON_HEAD}{JSON_TAIL}"));
        let all = with_sample(|r| render_json(r, true)).expect("render JSON");
        assert_eq!(all, format!("{JSON_HEAD}{JSON_BASELINED}{JSON_TAIL}"));
    }

    #[test]
    fn empty_reports() {
        let cmp = Comparison::default();
        let skipped = BTreeMap::new();
        let baseline = Baseline::new();
        let report = Report {
            findings: &[],
            cmp: &cmp,
            skipped: &skipped,
            names: &[],
            baseline: &baseline,
            baseline_rel: "scripts/docs/baseline.json",
        };
        let head = "docs-check: 0 findings across 0 checks - 0 new, 0 baselined (0 accepted false positives), 0 resolved (baseline scripts/docs/baseline.json)\n\n  check              total   new";
        assert_eq!(
            render_text(&report, false),
            format!("{head}\n\nNo new drift since baseline.")
        );
        assert_eq!(render_text(&report, true), head);
        assert_eq!(
            render_markdown(&report),
            "## Docs drift check\n\n**0 new** finding(s) since baseline; 0 total (0 accepted false positives), 0 resolved. Report-only: fix new drift in this PR, or accept it with `just docs-check --update-baseline`.\n\n| Check | Total | New |\n|---|---:|---:|\n"
        );
    }

    #[test]
    fn prune_notes_truncate_after_the_limit() {
        let cmp = Comparison {
            resolved: (0..51).map(|i| format!("k{i:02}")).collect(),
            reduced: BTreeMap::from([("r1".to_string(), (3, 1)), ("r0".to_string(), (2, 1))]),
            ..Comparison::default()
        };
        let notes = prune_notes(&cmp, 50);
        assert_eq!(notes.len(), 51);
        assert_eq!(notes[0], "  - k00");
        assert_eq!(notes[49], "  - k49");
        assert_eq!(notes[50], "  ... and 3 more");
        let short = prune_notes(&cmp, 60);
        assert_eq!(short[51], "  - r0 (2 -> 1 occurrences)");
        assert_eq!(short[52], "  - r1 (3 -> 1 occurrences)");
    }

    #[test]
    fn markdown_lists_at_most_100_new_findings() {
        let findings: Vec<Finding> = (0..101)
            .map(|i| {
                Finding::new(
                    "md-links",
                    format!("f{i:03}.md"),
                    "x.md",
                    "broken link -> x.md",
                    1,
                )
            })
            .collect();
        let cmp = Comparison {
            new_keys: findings.iter().map(Finding::key).collect(),
            ..Comparison::default()
        };
        let skipped = BTreeMap::new();
        let baseline = Baseline::new();
        let report = Report {
            findings: &findings,
            cmp: &cmp,
            skipped: &skipped,
            names: &["md-links"],
            baseline: &baseline,
            baseline_rel: "scripts/docs/baseline.json",
        };
        let text = render_markdown(&report);
        assert!(text.contains("| md-links | 101 | 101 |\n"), "{text}");
        assert!(
            text.ends_with("- `md-links` f099.md:1: broken link -> x.md\n- ... and 1 more\n"),
            "{text}"
        );
    }

    struct Named(&'static str);

    impl Check for Named {
        fn name(&self) -> &'static str {
            self.0
        }

        fn run(&self, _repo: &Repo) -> anyhow::Result<Vec<Finding>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn list_checks_pads_names_to_18() {
        let checks: Vec<Box<dyn Check>> = vec![
            Box::new(Named("md-links")),
            Box::new(Named("knowledge-hygiene")),
        ];
        assert_eq!(
            render_list_checks(&checks),
            "md-links           relative [text](path) links resolve to a tracked file or directory\nknowledge-hygiene  docs/knowledge naming, frontmatter, and an empty plans/ dir\n"
        );
    }
}
