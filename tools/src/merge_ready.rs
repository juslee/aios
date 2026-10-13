//! The merge-ready verdict: a port of the jq program in the former `scripts/agent/brief.sh` (blob at `2d4a9f72ebb8cd98a6659d8c75ebec91be23658f`, L201-241). brief and R2's loop call this; nothing reimplements it.
//!
//! Every function is pure: it takes `serde_json::Value`s (a `gh pr list` / `gh pr view` object and the
//! `gh issue list` array) and returns text or numbers, with no I/O. The jq semantics that matter are kept:
//!
//! - `//` falls through on `null` and `false` only, so `0` and `""` are kept ([`alt`]);
//! - `[[:cntrl:]]` becomes `?` ([`clean`]), and `tostring` of a non-string is its compact JSON;
//! - `scan("#([0-9]+)") | tonumber | unique` is sorted and de-duplicated ([`refs`]).
//!
//! Two deliberate differences from the jq program:
//!
//! - A `#<digits>` run is parsed exactly as `u64` (jq's `tonumber` lost precision beyond 2^53). A run that
//!   overflows `u64` is dropped: no PR or issue number is that large.
//! - [`blockers`] has one more entry than the program, `labelled needs-human`, in position 4 (it reads the
//!   `labels` field that [`PR_FIELDS`] already requests). An issue without an integer `number` is ignored by
//!   [`gates`]; `gh` always supplies one.
//!
//! The verdict needs four inputs, and `yes` needs all of them: the checks rollup (at least one check, all
//! passing), the count of open review threads (`None` is unknown and blocks), the needs-human issues naming
//! the PR (`None` is unknown and blocks) and the PR's own `needs-human` label; plus GitHub's own state: not a
//! draft, `MERGEABLE`, merge state `CLEAN` (or `DRAFT`), and no `CHANGES_REQUESTED`.

use std::collections::BTreeSet;

use serde_json::Value;

/// brief's `gh pr list --json` field list, byte for byte. R2's `pr view --json` is this plus `,` and its own
/// extra fields, so [`blockers`] sees the same shape from both.
pub const PR_FIELDS: &str = "number,title,headRefName,isDraft,mergeable,mergeStateStatus,reviewDecision,statusCheckRollup,author,labels";

/// brief's needs-human issue query argv, shared by R2's merge verdict.
pub const NEEDS_HUMAN_ISSUES_ARGS: [&str; 10] = [
    "issue",
    "list",
    "--state",
    "open",
    "--label",
    "needs-human",
    "--limit",
    "1000",
    "--json",
    "number,title,body,updatedAt",
];

static NULL: Value = Value::Null;

/// jq truthiness: everything except `null` and `false`.
fn truthy(v: &Value) -> bool {
    !matches!(v, Value::Null | Value::Bool(false))
}

/// `v[key]` as jq reads it: a missing key, or a non-object, is `null`.
fn field<'a>(v: &'a Value, key: &str) -> &'a Value {
    v.get(key).unwrap_or(&NULL)
}

/// jq `tostring`: a string as is, anything else as compact JSON (`null` is `"null"`).
fn tostring(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `(v // default)` rendered with `tostring`.
fn or_default(v: &Value, default: &str) -> String {
    if truthy(v) {
        tostring(v)
    } else {
        default.to_string()
    }
}

/// `gsub("[[:cntrl:]]"; "?")`: every control character (Unicode Cc) becomes `?`.
fn clean_str(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// jq `tostring | gsub("[[:cntrl:]]"; "?")`.
pub fn clean(v: &Value) -> String {
    clean_str(&tostring(v))
}

/// jq `[scan("#([0-9]+)") | .[0] | tonumber] | unique`: the `#<digits>` numbers in `text`, sorted and
/// de-duplicated. Leading zeros are dropped (`#0012` is 12); a run that overflows `u64` is skipped.
pub fn refs(text: &str) -> Vec<u64> {
    let bytes = text.as_bytes();
    let mut found = BTreeSet::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end > start {
                // `#` and ASCII digits are single bytes, so both slice points are char boundaries.
                if let Ok(n) = text[start..end].parse::<u64>() {
                    found.insert(n);
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    found.into_iter().collect()
}

/// Whether `v`, read as `(. // "")`, names `#<number>`; a non-string names nothing.
fn names(v: &Value, number: u64) -> bool {
    refs(v.as_str().unwrap_or("")).contains(&number)
}

/// jq `.k1 // .k2 // …`: the first of `keys` whose value is neither `null` nor `false`; when none is, the
/// last value (`null` for no keys), as jq's `//` yields its right-hand side.
pub fn alt<'a>(v: &'a Value, keys: &[&str]) -> &'a Value {
    let mut last = &NULL;
    for key in keys {
        last = field(v, key);
        if truthy(last) {
            return last;
        }
    }
    last
}

/// `.name // .context // "?"` of one rollup entry (a CheckRun has `name`, a StatusContext has `context`).
pub fn check_name(c: &Value) -> String {
    or_default(alt(c, &["name", "context"]), "?")
}

/// How one rollup entry counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    Pass,
    Fail,
    Pending,
}

/// The state of one rollup entry: `(.conclusion // .state // .status // "") | ascii_upcase`, then
/// `SUCCESS`/`NEUTRAL`/`SKIPPED` pass, `FAILURE`/`ERROR`/`CANCELLED`/`TIMED_OUT`/`ACTION_REQUIRED`/
/// `STARTUP_FAILURE` fail, and everything else (including `""`) is pending. `""` is truthy in jq, so an
/// empty `conclusion` does not fall through to `status`.
pub fn check_state(c: &Value) -> CheckState {
    let s = alt(c, &["conclusion", "state", "status"])
        .as_str()
        .unwrap_or("")
        .to_ascii_uppercase();
    match s.as_str() {
        "SUCCESS" | "NEUTRAL" | "SKIPPED" => CheckState::Pass,
        "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE" => {
            CheckState::Fail
        }
        _ => CheckState::Pending,
    }
}

/// The counts of a PR's rollup, and the names of its failing checks (one per failing entry).
struct Rollup {
    total: usize,
    pass: usize,
    failed: Vec<String>,
}

impl Rollup {
    /// `[.statusCheckRollup[]? | {name, state}]`, counted.
    fn of(pr: &Value) -> Rollup {
        let mut r = Rollup {
            total: 0,
            pass: 0,
            failed: Vec::new(),
        };
        for c in field(pr, "statusCheckRollup")
            .as_array()
            .into_iter()
            .flatten()
        {
            r.total += 1;
            match check_state(c) {
                CheckState::Pass => r.pass += 1,
                CheckState::Fail => r.failed.push(check_name(c)),
                CheckState::Pending => {}
            }
        }
        r
    }

    fn pending(&self) -> usize {
        self.total - self.pass - self.failed.len()
    }
}

/// The open needs-human issues that mention PR `pr`, as `(title gates, body-only names)`, each in issue
/// order: an issue whose title has `#<pr>` gates it; an issue with `#<pr>` only in its body names it.
pub fn gates(pr: u64, issues: &[Value]) -> (Vec<u64>, Vec<u64>) {
    let mut gated = Vec::new();
    let mut named = Vec::new();
    for issue in issues {
        let Some(number) = field(issue, "number").as_u64() else {
            continue;
        };
        if names(field(issue, "title"), pr) {
            gated.push(number);
        } else if names(field(issue, "body"), pr) {
            named.push(number);
        }
    }
    (gated, named)
}

/// Whether the PR carries the `needs-human` label (D10): any `.labels[].name == "needs-human"`.
pub fn labelled_needs_human(pr: &Value) -> bool {
    label_names(pr).any(|name| name == "needs-human")
}

/// `.labels[]?.name`, for the labels that have a string name.
fn label_names(pr: &Value) -> impl Iterator<Item = &str> {
    field(pr, "labels")
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l.get("name")?.as_str())
}

/// `#a, #b` for a list of issue numbers.
fn hash_list(numbers: &[u64]) -> String {
    numbers
        .iter()
        .map(|n| format!("#{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What stops a PR from merging, in brief's order (with `labelled needs-human` added in position 4); empty
/// means merge-ready. `issues` is the open needs-human issue list (`None` when it could not be read) and
/// `open_threads` the count of unresolved review threads (`None` when unknown).
pub fn blockers(pr: &Value, issues: Option<&[Value]>, open_threads: Option<u64>) -> Vec<String> {
    let (gated, named) = match (issues, field(pr, "number").as_u64()) {
        (Some(issues), Some(number)) => gates(number, issues),
        _ => (Vec::new(), Vec::new()),
    };
    let checks = Rollup::of(pr);
    let state = field(pr, "mergeStateStatus");

    let mut out = Vec::new();
    if truthy(field(pr, "isDraft")) {
        out.push("draft".to_string());
    }
    if !gated.is_empty() {
        out.push(format!("gated by {}", hash_list(&gated)));
    }
    if !named.is_empty() {
        out.push(format!("named in needs-human {}", hash_list(&named)));
    }
    if labelled_needs_human(pr) {
        out.push("labelled needs-human".to_string());
    }
    if issues.is_none() {
        out.push("needs-human gates unknown".to_string());
    }
    if checks.total == 0 {
        out.push("no checks reported".to_string());
    }
    if !checks.failed.is_empty() {
        out.push(format!("{} failing", checks.failed.len()));
    }
    if checks.pending() > 0 {
        out.push(format!("{} pending", checks.pending()));
    }
    let mergeable = field(pr, "mergeable");
    if mergeable.as_str() != Some("MERGEABLE") {
        out.push(format!("mergeable: {}", tostring(mergeable)));
    }
    if !matches!(state.as_str(), Some("CLEAN" | "DRAFT")) {
        out.push(format!("merge state: {}", tostring(state)));
    }
    if field(pr, "reviewDecision").as_str() == Some("CHANGES_REQUESTED") {
        out.push("changes requested".to_string());
    }
    match open_threads {
        None => out.push("review threads unknown".to_string()),
        Some(n) if n > 0 => out.push(format!("{n} unresolved review thread(s)")),
        Some(_) => {}
    }
    out
}

/// The three lines brief prints for one PR (none ends in a newline):
///
/// ```text
/// - #12 Title [`branch`, author] (draft) {team-build}
///   checks: 3 pass, 1 fail, 0 pending (failing: lint); mergeable: MERGEABLE / CLEAN; review: none; unresolved threads: 0
///   merge-ready: no (1 failing)
/// ```
pub fn pr_lines(pr: &Value, issues: Option<&[Value]>, open_threads: Option<u64>) -> [String; 3] {
    let checks = Rollup::of(pr);

    let teams: Vec<&str> = label_names(pr)
        .filter(|n| *n == "team-build" || *n == "team-fix")
        .collect();
    let team = if teams.is_empty() {
        " {no team}".to_string()
    } else {
        format!(" {{{}}}", teams.join(","))
    };
    let head = format!(
        "- #{} {} [`{}`, {}]{}{team}",
        tostring(field(pr, "number")),
        clean(field(pr, "title")),
        clean(field(pr, "headRefName")),
        clean_str(&or_default(field(field(pr, "author"), "login"), "?")),
        if truthy(field(pr, "isDraft")) {
            " (draft)"
        } else {
            ""
        },
    );

    let failing_names = if checks.failed.is_empty() {
        String::new()
    } else {
        let unique: BTreeSet<String> = checks.failed.iter().map(|n| clean_str(n)).collect();
        format!(
            " (failing: {})",
            unique.into_iter().collect::<Vec<_>>().join(", ")
        )
    };
    let review = match or_default(field(pr, "reviewDecision"), "").as_str() {
        "" => "none".to_string(),
        other => other.to_string(),
    };
    let threads = open_threads.map_or_else(|| "unknown".to_string(), |n| n.to_string());
    let line2 = format!(
        "  checks: {} pass, {} fail, {} pending{failing_names}; mergeable: {} / {}; review: {review}; unresolved threads: {threads}",
        checks.pass,
        checks.failed.len(),
        checks.pending(),
        tostring(field(pr, "mergeable")),
        tostring(field(pr, "mergeStateStatus")),
    );

    let blocked = blockers(pr, issues, open_threads);
    let line3 = if blocked.is_empty() {
        "  merge-ready: yes".to_string()
    } else {
        format!("  merge-ready: no ({})", blocked.join("; "))
    };
    [head, line2, line3]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// PR #5 with nothing in its way: one passing check, mergeable, clean, no labels.
    fn clean_pr() -> Value {
        json!({
            "number": 5,
            "title": "Add a thing",
            "headRefName": "claude/thing",
            "isDraft": false,
            "mergeable": "MERGEABLE",
            "mergeStateStatus": "CLEAN",
            "reviewDecision": "",
            "statusCheckRollup": [{"name": "build", "conclusion": "SUCCESS", "status": "COMPLETED"}],
            "author": {"login": "octo"},
            "labels": [{"name": "team-build"}],
        })
    }

    fn set(mut pr: Value, key: &str, v: Value) -> Value {
        pr[key] = v;
        pr
    }

    fn issue(number: u64, title: &str, body: &str) -> Value {
        json!({"number": number, "title": title, "body": body, "updatedAt": "2026-10-11T00:00:00Z"})
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn consts_are_briefs_argv() {
        assert_eq!(PR_FIELDS.split(',').count(), 10);
        assert!(PR_FIELDS.starts_with("number,title,headRefName,isDraft,"));
        assert!(PR_FIELDS.ends_with(",statusCheckRollup,author,labels"));
        assert_eq!(NEEDS_HUMAN_ISSUES_ARGS.join(" "), "issue list --state open --label needs-human --limit 1000 --json number,title,body,updatedAt");
    }

    #[test]
    fn clean_pr_is_ready() {
        let issues: Vec<Value> = vec![];
        assert_eq!(blockers(&clean_pr(), Some(&issues), Some(0)), strs(&[]));
        let lines = pr_lines(&clean_pr(), Some(&issues), Some(0));
        assert_eq!(
            lines,
            [
                "- #5 Add a thing [`claude/thing`, octo] {team-build}",
                "  checks: 1 pass, 0 fail, 0 pending; mergeable: MERGEABLE / CLEAN; review: none; unresolved threads: 0",
                "  merge-ready: yes",
            ]
        );
    }

    #[test]
    fn every_blocker_alone() {
        let none: Vec<Value> = vec![];
        let gate = vec![issue(7, "Gate: merge PR #5 last", "")];
        let named = vec![issue(8, "Unrelated", "see #5 and #50")];
        type Case<'a> = (&'a str, Value, Option<&'a [Value]>, Option<u64>, &'a str);
        let cases: Vec<Case> = vec![
            (
                "draft",
                set(clean_pr(), "isDraft", json!(true)),
                Some(&none),
                Some(0),
                "draft",
            ),
            ("gated", clean_pr(), Some(&gate), Some(0), "gated by #7"),
            (
                "named",
                clean_pr(),
                Some(&named),
                Some(0),
                "named in needs-human #8",
            ),
            (
                "labelled",
                set(clean_pr(), "labels", json!([{"name": "needs-human"}])),
                Some(&none),
                Some(0),
                "labelled needs-human",
            ),
            (
                "gates unknown",
                clean_pr(),
                None,
                Some(0),
                "needs-human gates unknown",
            ),
            (
                "no checks",
                set(clean_pr(), "statusCheckRollup", json!([])),
                Some(&none),
                Some(0),
                "no checks reported",
            ),
            (
                "failing",
                set(
                    clean_pr(),
                    "statusCheckRollup",
                    json!([{"name": "a", "conclusion": "SUCCESS"}, {"name": "b", "conclusion": "FAILURE"}]),
                ),
                Some(&none),
                Some(0),
                "1 failing",
            ),
            (
                "pending",
                set(
                    clean_pr(),
                    "statusCheckRollup",
                    json!([{"name": "a", "conclusion": "SUCCESS"}, {"name": "b", "status": "IN_PROGRESS", "conclusion": ""}]),
                ),
                Some(&none),
                Some(0),
                "1 pending",
            ),
            (
                "mergeable",
                set(clean_pr(), "mergeable", json!("CONFLICTING")),
                Some(&none),
                Some(0),
                "mergeable: CONFLICTING",
            ),
            (
                "merge state",
                set(clean_pr(), "mergeStateStatus", json!("BEHIND")),
                Some(&none),
                Some(0),
                "merge state: BEHIND",
            ),
            (
                "changes requested",
                set(clean_pr(), "reviewDecision", json!("CHANGES_REQUESTED")),
                Some(&none),
                Some(0),
                "changes requested",
            ),
            (
                "threads",
                clean_pr(),
                Some(&none),
                Some(2),
                "2 unresolved review thread(s)",
            ),
            (
                "threads unknown",
                clean_pr(),
                Some(&none),
                None,
                "review threads unknown",
            ),
        ];
        for (what, pr, issues, threads, want) in cases {
            assert_eq!(blockers(&pr, issues, threads), strs(&[want]), "{what}");
        }
    }

    #[test]
    fn all_blockers_together_keep_brief_order() {
        let issues = vec![
            issue(7, "Gate: merge PR #5", ""),
            issue(9, "Other gate #5", ""),
            issue(8, "Unrelated", "see #5"),
        ];
        let pr = json!({
            "number": 5, "isDraft": true, "mergeable": "CONFLICTING", "mergeStateStatus": "DIRTY",
            "reviewDecision": "CHANGES_REQUESTED",
            "statusCheckRollup": [
                {"name": "a", "conclusion": "FAILURE"}, {"name": "b", "conclusion": "TIMED_OUT"},
                {"name": "c", "status": "QUEUED"}, {"name": "d", "conclusion": "SUCCESS"}],
            "labels": [{"name": "needs-human"}],
        });
        assert_eq!(
            blockers(&pr, Some(&issues), Some(3)),
            strs(&[
                "draft",
                "gated by #7, #9",
                "named in needs-human #8",
                "labelled needs-human",
                "2 failing",
                "1 pending",
                "mergeable: CONFLICTING",
                "merge state: DIRTY",
                "changes requested",
                "3 unresolved review thread(s)",
            ])
        );
        // The other half of the twelve: unknown gates and an empty rollup exclude the rest.
        let pr = json!({
            "number": 5, "isDraft": true, "mergeable": "UNKNOWN", "mergeStateStatus": "UNKNOWN",
            "reviewDecision": "CHANGES_REQUESTED", "statusCheckRollup": [],
            "labels": [{"name": "needs-human"}],
        });
        assert_eq!(
            blockers(&pr, None, None),
            strs(&[
                "draft",
                "labelled needs-human",
                "needs-human gates unknown",
                "no checks reported",
                "mergeable: UNKNOWN",
                "merge state: UNKNOWN",
                "changes requested",
                "review threads unknown",
            ])
        );
    }

    #[test]
    fn draft_comes_first_so_a_draft_gated_pr_reads_no_draft_gated() {
        let issues = vec![issue(30, "Gate: merge PR #5 only after R3", "")];
        let pr = set(
            set(clean_pr(), "isDraft", json!(true)),
            "mergeStateStatus",
            json!("DRAFT"),
        );
        let lines = pr_lines(&pr, Some(&issues), Some(0));
        assert_eq!(lines[2], "  merge-ready: no (draft; gated by #30)");
        assert_eq!(
            lines[0],
            "- #5 Add a thing [`claude/thing`, octo] (draft) {team-build}"
        );
    }

    #[test]
    fn draft_state_is_not_a_blocker_and_null_mergeable_prints_null() {
        let none: Vec<Value> = vec![];
        let pr = set(clean_pr(), "mergeStateStatus", json!("DRAFT"));
        assert_eq!(blockers(&pr, Some(&none), Some(0)), strs(&[]));
        let pr = set(clean_pr(), "mergeable", Value::Null);
        assert_eq!(
            blockers(&pr, Some(&none), Some(0)),
            strs(&["mergeable: null"])
        );
        assert!(pr_lines(&pr, Some(&none), Some(0))[1].contains("; mergeable: null / CLEAN;"));
        // A missing mergeStateStatus is `null` too, and is not CLEAN.
        let mut pr = clean_pr();
        pr.as_object_mut().unwrap().remove("mergeStateStatus");
        assert_eq!(
            blockers(&pr, Some(&none), Some(0)),
            strs(&["merge state: null"])
        );
    }

    #[test]
    fn zero_threads_print_as_zero_not_unknown() {
        let none: Vec<Value> = vec![];
        assert!(pr_lines(&clean_pr(), Some(&none), Some(0))[1].ends_with("unresolved threads: 0"));
        assert!(
            pr_lines(&clean_pr(), Some(&none), None)[1].ends_with("unresolved threads: unknown")
        );
        assert!(pr_lines(&clean_pr(), Some(&none), Some(12))[1].ends_with("unresolved threads: 12"));
    }

    #[test]
    fn refs_are_sorted_unique_exact_u64() {
        assert_eq!(refs("#0012 and #12, #3 #3"), vec![3, 12]);
        assert_eq!(refs("a#7b ##8 #x #"), vec![7, 8]);
        assert_eq!(refs(""), Vec::<u64>::new());
        // D4: beyond 2^53 (9007199254740992) the digits stay exact.
        assert_eq!(refs("#9007199254740993"), vec![9_007_199_254_740_993]);
        assert_eq!(refs("#18446744073709551615"), vec![u64::MAX]);
        // Past u64 nothing can match a PR number, so the run is dropped.
        assert_eq!(refs("#18446744073709551616 #4"), vec![4]);
        // Non-ASCII text around the digits is fine.
        assert_eq!(refs("é#5é"), vec![5]);
    }

    #[test]
    fn clean_replaces_control_characters_and_tostrings() {
        assert_eq!(
            clean(&json!("a\tb\nc\u{1b}[0m\u{7f}\u{85}d")),
            "a?b?c?[0m??d"
        );
        assert_eq!(clean(&Value::Null), "null");
        assert_eq!(clean(&json!(12)), "12");
        assert_eq!(clean(&json!(true)), "true");
        assert_eq!(clean(&json!("héllo ✓")), "héllo ✓");
    }

    #[test]
    fn alt_falls_through_on_null_and_false_only() {
        let v = json!({"a": null, "b": false, "c": 0, "d": "", "e": "x"});
        assert_eq!(alt(&v, &["a", "e"]), &json!("x"));
        assert_eq!(alt(&v, &["b", "e"]), &json!("x"));
        assert_eq!(alt(&v, &["c", "e"]), &json!(0));
        assert_eq!(alt(&v, &["d", "e"]), &json!(""));
        assert_eq!(alt(&v, &["missing", "a"]), &Value::Null);
        assert_eq!(alt(&v, &["a", "b"]), &json!(false));
        assert_eq!(alt(&v, &[]), &Value::Null);
        assert_eq!(alt(&json!([1]), &["a"]), &Value::Null);
    }

    #[test]
    fn check_names_and_states() {
        assert_eq!(check_name(&json!({"name": "build"})), "build");
        assert_eq!(check_name(&json!({"context": "ci/x"})), "ci/x");
        assert_eq!(
            check_name(&json!({"name": null, "context": "ci/x"})),
            "ci/x"
        );
        assert_eq!(check_name(&json!({"name": false})), "?");
        assert_eq!(check_name(&json!({})), "?");
        let state = |v: Value| check_state(&v);
        for s in ["SUCCESS", "NEUTRAL", "SKIPPED", "success", "Skipped"] {
            assert_eq!(state(json!({"conclusion": s})), CheckState::Pass, "{s}");
        }
        for s in [
            "FAILURE",
            "ERROR",
            "CANCELLED",
            "TIMED_OUT",
            "ACTION_REQUIRED",
            "STARTUP_FAILURE",
            "failure",
        ] {
            assert_eq!(state(json!({"conclusion": s})), CheckState::Fail, "{s}");
        }
        // StatusContext: `state`, with no conclusion.
        assert_eq!(state(json!({"state": "SUCCESS"})), CheckState::Pass);
        assert_eq!(state(json!({"state": "PENDING"})), CheckState::Pending);
        assert_eq!(state(json!({"state": "ERROR"})), CheckState::Fail);
        // `null` and `false` fall through; `""` does not (it is truthy), so it is pending.
        assert_eq!(
            state(json!({"conclusion": null, "status": "FAILURE"})),
            CheckState::Fail
        );
        assert_eq!(
            state(json!({"conclusion": false, "state": "SUCCESS"})),
            CheckState::Pass
        );
        assert_eq!(
            state(json!({"conclusion": "", "status": "SUCCESS"})),
            CheckState::Pending
        );
        assert_eq!(state(json!({"status": "IN_PROGRESS"})), CheckState::Pending);
        assert_eq!(state(json!({"status": "COMPLETED"})), CheckState::Pending);
        assert_eq!(state(json!({})), CheckState::Pending);
        assert_eq!(state(json!({"conclusion": 5})), CheckState::Pending);
    }

    #[test]
    fn gates_split_title_from_body_only() {
        let issues = vec![
            issue(1, "Gate: merge PR #5", "unrelated"),
            issue(2, "no number", "waits for #5"),
            issue(3, "both: #5", "and #5 again"),
            issue(4, "other PR #50", "also #500"),
            issue(5, "#0005 with zeros", ""),
            json!({"number": 6, "title": null, "body": null}),
            json!({"title": "no issue number #5", "body": ""}),
            json!({"number": 7, "title": 5, "body": ["#5"]}),
        ];
        assert_eq!(gates(5, &issues), (vec![1, 3, 5], vec![2]));
        assert_eq!(gates(50, &issues), (vec![4], vec![]));
        assert_eq!(gates(99, &issues), (vec![], vec![]));
        assert_eq!(gates(5, &[]), (vec![], vec![]));
    }

    #[test]
    fn title_gate_wins_over_a_body_name_of_the_same_issue() {
        let issues = vec![issue(3, "both: #5", "and #5 again")];
        let blockers = blockers(&clean_pr(), Some(&issues), Some(0));
        assert_eq!(blockers, strs(&["gated by #3"]));
    }

    #[test]
    fn labelled_needs_human_blocks() {
        let none: Vec<Value> = vec![];
        assert!(!labelled_needs_human(&clean_pr()));
        let pr = set(
            clean_pr(),
            "labels",
            json!([{"name": "team-fix"}, {"name": "needs-human"}]),
        );
        assert!(labelled_needs_human(&pr));
        assert_eq!(
            blockers(&pr, Some(&none), Some(0)),
            strs(&["labelled needs-human"])
        );
        // Only the exact label counts, and odd shapes never panic.
        for labels in [
            json!([{"name": "needs-human-later"}]),
            json!([{"name": null}, {}, 3, "needs-human"]),
            json!(null),
            json!({"name": "needs-human"}),
        ] {
            assert!(!labelled_needs_human(&set(clean_pr(), "labels", labels)));
        }
        let mut no_labels = clean_pr();
        no_labels.as_object_mut().unwrap().remove("labels");
        assert!(!labelled_needs_human(&no_labels));
        // It is a blocker of its own, ahead of the unknown-gates blocker.
        assert_eq!(
            blockers(&pr, None, Some(0)),
            strs(&["labelled needs-human", "needs-human gates unknown"])
        );
    }

    #[test]
    fn pr_lines_header_variants() {
        let none: Vec<Value> = vec![];
        let pr = json!({
            "number": 12, "title": "Bad\ttitle\u{1b}", "headRefName": "claude/x\ny",
            "isDraft": false, "mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN",
            "reviewDecision": "APPROVED",
            "statusCheckRollup": [{"name": "build", "conclusion": "SUCCESS"}],
            "author": null,
            "labels": [{"name": "team-fix"}, {"name": "bug"}, {"name": "team-build"}, {"name": "team-fix"}],
        });
        let lines = pr_lines(&pr, Some(&none), Some(0));
        assert_eq!(
            lines[0],
            "- #12 Bad?title? [`claude/x?y`, ?] {team-fix,team-build,team-fix}"
        );
        assert!(lines[1].contains("; review: APPROVED;"));
        let pr = json!({"number": 13, "author": {"login": "a\u{7}b"}, "labels": []});
        assert_eq!(
            pr_lines(&pr, Some(&none), None)[0],
            "- #13 null [`null`, a?b] {no team}"
        );
    }

    #[test]
    fn failing_names_are_cleaned_unique_and_sorted() {
        let none: Vec<Value> = vec![];
        let pr = set(
            clean_pr(),
            "statusCheckRollup",
            json!([
                {"name": "zeta", "conclusion": "FAILURE"},
                {"name": "alpha\n", "conclusion": "CANCELLED"},
                {"name": "alpha?", "conclusion": "ERROR"},
                {"context": "ci/x", "state": "ERROR"},
                {"conclusion": "FAILURE"},
                {"name": "ok", "conclusion": "NEUTRAL"},
                {"name": "wait", "status": "QUEUED"},
            ]),
        );
        let lines = pr_lines(&pr, Some(&none), Some(1));
        assert_eq!(
            lines[1],
            "  checks: 1 pass, 5 fail, 1 pending (failing: ?, alpha?, ci/x, zeta); mergeable: MERGEABLE / CLEAN; review: none; unresolved threads: 1"
        );
        assert_eq!(
            lines[2],
            "  merge-ready: no (5 failing; 1 pending; 1 unresolved review thread(s))"
        );
    }

    #[test]
    fn missing_or_odd_fields_never_panic() {
        let none: Vec<Value> = vec![];
        for pr in [
            json!({}),
            json!(null),
            json!([]),
            json!("x"),
            json!({"statusCheckRollup": "x", "labels": 3}),
        ] {
            let b = blockers(&pr, Some(&none), Some(0));
            assert!(b.contains(&"no checks reported".to_string()), "{pr}");
            let lines = pr_lines(&pr, None, None);
            assert!(lines[2].starts_with("  merge-ready: no ("), "{pr}");
        }
        // No `number`: nothing can gate the PR, and the header prints jq's `null`.
        let issues = vec![issue(1, "Gate #5", "")];
        assert!(!blockers(&json!({}), Some(&issues), Some(0))
            .iter()
            .any(|b| b.starts_with("gated")));
        assert!(pr_lines(&json!({}), None, None)[0].starts_with("- #null "));
    }
}
