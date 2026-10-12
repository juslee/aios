//! The GitHub sections: Open pull requests, Main CI and Needs human, and the pull-request
//! lookup of the phase section.
//!
//! Every call is a read through [`GhRunner`] (the real [`crate::gh::GhCli`] runs in the
//! toplevel and inherits the environment), with the argv of the script it replaces: the
//! parity differential compares the calls. They run in the script's order: the probe, the
//! needs-human issues and the review threads before the first section, then the pull-request
//! list, the tip's check suites and the run list, the two queue counts last. A call that
//! depends on another is not issued when that one failed.
//!
//! The merge-ready verdict is [`crate::merge_ready`]'s. GitHub text is printed as data with
//! control characters replaced. Output that `gh` gives and that is not the JSON the call
//! asks for is reported as `unreadable gh output`.

use std::cmp::Ordering;
use std::io::Write;
use std::path::Path;

use anyhow::Result;
use regex::Regex;
use serde_json::{Map, Value};

use super::line;
use crate::gh::{GhOutcome, GhRunner};
use crate::git;
use crate::merge_ready::{self, NEEDS_HUMAN_ISSUES_ARGS, PR_FIELDS};

/// The review-threads query: the unresolved threads of the newest 50 open PRs.
const THREADS_QUERY: &str = "
        query($owner: String!, $name: String!) {
          repository(owner: $owner, name: $name) {
            pullRequests(states: OPEN, first: 50, orderBy: {field: CREATED_AT, direction: DESC}) {
              nodes { number reviewThreads(first: 100) { nodes { isResolved } } }
            }
          }
        }";
/// Reduces the threads answer to `{"<number>": <open threads>}`.
const THREADS_JQ: &str = "[.data.repository.pullRequests.nodes[]
                  | {key: (.number | tostring), value: ([.reviewThreads.nodes[] | select(.isResolved | not)] | length)}]
                 | from_entries";
/// The origin/main tip's check suites.
const SUITES_QUERY: &str = "
        query($owner: String!, $name: String!, $oid: GitObjectID!) {
          repository(owner: $owner, name: $name) {
            object(oid: $oid) {
              ... on Commit {
                checkSuites(first: 100) {
                  nodes { status conclusion branch { name } workflowRun { event createdAt workflow { name } } }
                }
              }
            }
          }
        }";

const UNREADABLE: &[u8] = b"unreadable gh output";

/// The GitHub answers one brief needs, fetched in the script's order.
pub struct Github<R: GhRunner> {
    gh: R,
    /// Why GitHub is unavailable; `None` when the probe succeeded.
    why: Option<Vec<u8>>,
    /// The open needs-human issues, or the first line of the error that kept them away.
    issues: std::result::Result<Vec<Value>, Vec<u8>>,
    /// Open review threads by PR number, when the query worked.
    threads: Option<Map<String, Value>>,
    /// The open PRs, once the Open pull requests section has read them.
    prs: Option<Vec<Value>>,
}

/// `$(...)`: the output without its trailing newlines.
fn chomp(mut text: &[u8]) -> &[u8] {
    while let [rest @ .., b'\n'] = text {
        text = rest;
    }
    text
}

/// The probe: `gh api rate_limit`. `None` when GitHub can be used, else the reason.
fn probe(gh: &impl GhRunner) -> Option<Vec<u8>> {
    match gh.run(&["api", "rate_limit", "--jq", ".resources.core.remaining"]) {
        GhOutcome::Missing => Some(b"gh is not installed".to_vec()),
        GhOutcome::Ok(stdout) => {
            let remaining = chomp(&stdout);
            (remaining.is_empty() || remaining == b"0")
                .then(|| b"API rate limit exhausted".to_vec())
        }
        failed => {
            let first = failed.first_stderr_line();
            Some(if first.is_empty() {
                b"unknown error".to_vec()
            } else {
                first.to_vec()
            })
        }
    }
}

/// The stdout of a successful call as a JSON array.
fn array(stdout: &[u8]) -> Option<Vec<Value>> {
    match serde_json::from_slice(stdout) {
        Ok(Value::Array(items)) => Some(items),
        _ => None,
    }
}

// --- jq semantics -----------------------------------------------------------------------

/// jq's order of values: null, false, true, numbers, strings, arrays, objects.
fn rank(v: &Value) -> u8 {
    match v {
        Value::Null => 0,
        Value::Bool(false) => 1,
        Value::Bool(true) => 2,
        Value::Number(_) => 3,
        Value::String(_) => 4,
        Value::Array(_) => 5,
        Value::Object(_) => 6,
    }
}

/// jq's comparison of two values (strings bytewise; containers by their JSON text, which
/// no GitHub answer here contains in a sorted position).
fn jq_cmp(a: &Value, b: &Value) -> Ordering {
    rank(a).cmp(&rank(b)).then_with(|| match (a, b) {
        (Value::Number(x), Value::Number(y)) => x
            .as_f64()
            .partial_cmp(&y.as_f64())
            .unwrap_or(Ordering::Equal),
        (Value::String(x), Value::String(y)) => x.as_bytes().cmp(y.as_bytes()),
        (Value::Array(_) | Value::Object(_), _) => a.to_string().cmp(&b.to_string()),
        _ => Ordering::Equal,
    })
}

/// jq string interpolation: a string as it is, anything else as compact JSON.
fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `v[0:n]` of a string (by characters); anything else interpolates as itself.
fn head(v: &Value, n: usize) -> String {
    match v {
        Value::String(s) => s.chars().take(n).collect(),
        other => text(other),
    }
}

/// `v.a.b.c` where a missing key, or a non-object, is `null`.
fn path<'a>(v: &'a Value, keys: &[&str]) -> &'a Value {
    keys.iter()
        .fold(v, |at, key| at.get(key).unwrap_or(&Value::Null))
}

// --- main CI ----------------------------------------------------------------------------

/// The newest push run of each workflow on main: the runs listed plus the push runs among
/// the tip's check suites, one line per workflow in name order. On a tie (the same run
/// from both sources) the suite, which comes last, wins.
fn ci_lines(mut entries: Vec<Value>, suites: &Value, tip: &str) -> Vec<String> {
    let nodes = path(
        suites,
        &["data", "repository", "object", "checkSuites", "nodes"],
    );
    for node in nodes.as_array().into_iter().flatten() {
        let run = path(node, &["workflowRun"]);
        if path(run, &["event"]) != "push" || path(node, &["branch", "name"]) != "main" {
            continue;
        }
        let lower = |v: &Value| Value::String(v.as_str().unwrap_or_default().to_ascii_lowercase());
        entries.push(serde_json::json!({
            "workflowName": path(run, &["workflow", "name"]),
            "status": lower(path(node, &["status"])),
            "conclusion": lower(path(node, &["conclusion"])),
            "headSha": tip,
            "createdAt": path(run, &["createdAt"]),
            "event": path(run, &["event"]),
        }));
    }
    if entries.is_empty() {
        return vec!["- no runs on main".to_string()];
    }
    let name = |run: &Value| path(run, &["workflowName"]).clone();
    entries.sort_by(|a, b| jq_cmp(&name(a), &name(b)));
    let mut newest: Vec<&Value> = Vec::new();
    for run in &entries {
        let same_workflow = newest
            .last()
            .is_some_and(|last| jq_cmp(&name(last), &name(run)) == Ordering::Equal);
        if !same_workflow {
            newest.push(run);
        } else if let Some(last) = newest.last_mut() {
            // max_by keeps the last of equal maxima.
            if jq_cmp(path(run, &["createdAt"]), path(last, &["createdAt"])) != Ordering::Less {
                *last = run;
            }
        }
    }
    newest
        .into_iter()
        .map(|run| {
            let state = if path(run, &["status"]) == "completed" {
                path(run, &["conclusion"])
            } else {
                path(run, &["status"])
            };
            let sha = path(run, &["headSha"]);
            let not_tip = if !tip.is_empty() && sha != tip {
                " - not the origin/main tip"
            } else {
                ""
            };
            format!(
                "- {}: {} @ {} ({}, {} UTC){not_tip}",
                merge_ready::clean(path(run, &["workflowName"])),
                text(state),
                head(sha, 7),
                text(path(run, &["event"])),
                head(path(run, &["createdAt"]), 16).replacen('T', " ", 1),
            )
        })
        .collect()
}

impl<R: GhRunner> Github<R> {
    /// Probe GitHub, then read the needs-human issues and the review threads when it can
    /// be used.
    pub fn open(gh: R) -> Github<R> {
        let why = probe(&gh);
        let mut issues = Err(Vec::new());
        let mut threads = None;
        if why.is_none() {
            issues = match gh.run(&NEEDS_HUMAN_ISSUES_ARGS) {
                GhOutcome::Ok(stdout) => array(&stdout).ok_or_else(Vec::new),
                failed => Err(failed.first_stderr_line().to_vec()),
            };
            let query = format!("query={THREADS_QUERY}");
            let outcome = gh.run(&[
                "api",
                "graphql",
                "-F",
                "owner={owner}",
                "-F",
                "name={repo}",
                "-f",
                &query,
                "--jq",
                THREADS_JQ,
            ]);
            if let GhOutcome::Ok(stdout) = outcome {
                if let Ok(Value::Object(map)) = serde_json::from_slice(&stdout) {
                    threads = Some(map);
                }
            }
        }
        Github {
            gh,
            why,
            issues,
            threads,
            prs: None,
        }
    }

    /// `- GitHub unavailable: <why>` when the probe failed; whether it did.
    fn unavailable(&self, out: &mut dyn Write) -> Result<bool> {
        let Some(why) = &self.why else {
            return Ok(false);
        };
        let why: &[u8] = if why.is_empty() {
            b"unknown error"
        } else {
            why
        };
        line(out, &[b"- GitHub unavailable: ", why])?;
        Ok(true)
    }

    /// The Open pull requests section body.
    pub fn pull_requests(&mut self, out: &mut dyn Write) -> Result<()> {
        if self.unavailable(out)? {
            return Ok(());
        }
        let mut args = vec!["pr", "list", "--state", "open", "--limit", "30", "--json"];
        args.push(PR_FIELDS);
        let prs = match self.gh.run(&args) {
            GhOutcome::Ok(stdout) => match array(&stdout) {
                Some(prs) => prs,
                None => {
                    line(out, &[b"- GitHub unavailable: ", UNREADABLE])?;
                    return Ok(());
                }
            },
            failed => {
                line(
                    out,
                    &[b"- GitHub unavailable: ", failed.first_stderr_line()],
                )?;
                return Ok(());
            }
        };
        if prs.is_empty() {
            line(out, &[b"- none"])?;
        }
        let issues = self.issues.as_ref().ok().map(Vec::as_slice);
        for pr in &prs {
            let open_threads = self
                .threads
                .as_ref()
                .and_then(|threads| threads.get(&text(path(pr, &["number"]))))
                .and_then(Value::as_u64);
            for row in merge_ready::pr_lines(pr, issues, open_threads) {
                line(out, &[row.as_bytes()])?;
            }
        }
        self.prs = Some(prs);
        Ok(())
    }

    /// The Main CI section body.
    pub fn main_ci(&self, root: &Path, out: &mut dyn Write) -> Result<()> {
        if self.unavailable(out)? {
            return Ok(());
        }
        let tip = git::text(
            root,
            &["rev-parse", "-q", "--verify", "refs/remotes/origin/main"],
        )
        .unwrap_or_default();
        let mut suites = Value::Null;
        let mut failure: Option<Vec<u8>> = None;
        if !tip.is_empty() {
            let (oid, query) = (format!("oid={tip}"), format!("query={SUITES_QUERY}"));
            let outcome = self.gh.run(&[
                "api",
                "graphql",
                "-F",
                "owner={owner}",
                "-F",
                "name={repo}",
                "-F",
                &oid,
                "-f",
                &query,
            ]);
            match outcome {
                GhOutcome::Ok(stdout) => {
                    suites = serde_json::from_slice(&stdout).unwrap_or(Value::Null);
                }
                failed => failure = Some(failed.first_stderr_line().to_vec()),
            }
        }
        // The run list is not asked for after a failed suites query.
        if failure.is_none() {
            let outcome = self.gh.run(&[
                "run",
                "list",
                "--branch",
                "main",
                "--event",
                "push",
                "--limit",
                "40",
                "--json",
                "workflowName,status,conclusion,headSha,createdAt,event",
            ]);
            match outcome {
                GhOutcome::Ok(stdout) => match array(&stdout) {
                    Some(runs) => {
                        for row in ci_lines(runs, &suites, &tip) {
                            line(out, &[row.as_bytes()])?;
                        }
                    }
                    None => failure = Some(UNREADABLE.to_vec()),
                },
                failed => failure = Some(failed.first_stderr_line().to_vec()),
            }
        }
        if let Some(why) = failure {
            line(out, &[b"- GitHub unavailable: ", &why])?;
        }
        Ok(())
    }

    /// The Needs human section body: the open issues and the queue counts.
    pub fn needs_human(&self, out: &mut dyn Write) -> Result<()> {
        if self.unavailable(out)? {
            return Ok(());
        }
        match &self.issues {
            Ok(issues) if issues.is_empty() => line(out, &[b"- none"])?,
            Ok(issues) => {
                let mut sorted: Vec<&Value> = issues.iter().collect();
                sorted.sort_by(|a, b| jq_cmp(path(a, &["number"]), path(b, &["number"])));
                for issue in sorted {
                    let row = format!(
                        "- #{} {} (updated {})",
                        merge_ready::clean(path(issue, &["number"])),
                        merge_ready::clean(path(issue, &["title"])),
                        head(path(issue, &["updatedAt"]), 10),
                    );
                    line(out, &[row.as_bytes()])?;
                }
            }
            Err(why) => line(out, &[b"- GitHub unavailable: ", why])?,
        }
        let count = |label: &str| -> Vec<u8> {
            let outcome = self.gh.run(&[
                "issue", "list", "--state", "open", "--label", label, "--limit", "1000", "--json",
                "number", "--jq", "length",
            ]);
            match outcome {
                GhOutcome::Ok(stdout) => chomp(&stdout).to_vec(),
                _ => b"?".to_vec(),
            }
        };
        let (ready, working) = (count("agent-ready"), count("agent-working"));
        line(
            out,
            &[
                b"- queue: ",
                &ready,
                b" agent-ready, ",
                &working,
                b" agent-working",
            ],
        )
    }

    /// The `open PR for M<n>` line of the phase section, for the milestone `milestone`
    /// (its decimal digits) the next open task is in.
    pub fn milestone_pr(&self, milestone: &[u8], out: &mut dyn Write) -> Result<()> {
        let digits = String::from_utf8_lossy(milestone);
        let Some(prs) = &self.prs else {
            return line(
                out,
                &[
                    b"- open PR for M",
                    milestone,
                    b": unknown (GitHub unavailable)",
                ],
            );
        };
        // A "Phase N MK:" (or any "MK") PR title means that milestone is already in flight.
        let in_flight = Regex::new(&format!("(^|[^A-Za-z0-9])M{digits}([^0-9]|$)"))?;
        let mut found = false;
        for pr in prs {
            let title = path(pr, &["title"]);
            if !title.as_str().is_some_and(|t| in_flight.is_match(t)) {
                continue;
            }
            found = true;
            let row = format!(
                "- open PR for M{digits}: #{} {} (its merge-ready line is under Open pull requests)",
                merge_ready::clean(path(pr, &["number"])),
                merge_ready::clean(title),
            );
            line(out, &[row.as_bytes()])?;
        }
        if !found {
            line(out, &[b"- open PR for M", milestone, b": none"])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A runner that answers by a word its argv contains, and logs the argv.
    struct Fake {
        answers: Vec<(&'static str, GhOutcome)>,
        calls: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new(answers: Vec<(&'static str, GhOutcome)>) -> Fake {
            Fake {
                answers,
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl GhRunner for Fake {
        fn run(&self, args: &[&str]) -> GhOutcome {
            let joined = args.join(" ");
            self.calls.borrow_mut().push(joined.clone());
            self.answers
                .iter()
                .find(|(key, _)| joined.contains(key))
                .map(|(_, outcome)| outcome.clone())
                .unwrap_or_else(|| panic!("unexpected gh call: {joined}"))
        }
    }

    fn ok(text: &str) -> GhOutcome {
        GhOutcome::Ok(text.as_bytes().to_vec())
    }

    fn failed(stderr: &str) -> GhOutcome {
        GhOutcome::Failed {
            status: Some(1),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    fn rendered(f: impl FnOnce(&mut Vec<u8>) -> Result<()>) -> String {
        let mut out = Vec::new();
        f(&mut out).expect("write");
        String::from_utf8(out).expect("utf-8")
    }

    #[test]
    fn the_probe_names_each_reason() {
        let why = |outcome| probe(&Fake::new(vec![("rate_limit", outcome)]));
        assert_eq!(why(ok("4999\n")), None);
        assert_eq!(why(ok("0\n")), Some(b"API rate limit exhausted".to_vec()));
        // An empty answer reads as 0, as `${remaining:-0}` did.
        assert_eq!(why(ok("")), Some(b"API rate limit exhausted".to_vec()));
        assert_eq!(
            why(GhOutcome::Missing),
            Some(b"gh is not installed".to_vec())
        );
        assert_eq!(why(failed("offline\nmore")), Some(b"offline".to_vec()));
        assert_eq!(why(failed("")), Some(b"unknown error".to_vec()));
    }

    #[test]
    fn an_unavailable_probe_stops_every_call() {
        let gh = Fake::new(vec![("rate_limit", failed("offline"))]);
        let mut github = Github::open(gh);
        let text = rendered(|out| {
            github.pull_requests(out)?;
            github.main_ci(Path::new("."), out)?;
            github.needs_human(out)?;
            github.milestone_pr(b"4", out)
        });
        assert_eq!(
            text,
            "- GitHub unavailable: offline\n".repeat(3)
                + "- open PR for M4: unknown (GitHub unavailable)\n"
        );
        assert_eq!(github.gh.calls.borrow().len(), 1);
    }

    #[test]
    fn pull_requests_read_threads_by_number() {
        let pr = r#"[{"number":7,"title":"Phase 4 M9: x","headRefName":"claude/x","isDraft":false,
            "mergeable":"MERGEABLE","mergeStateStatus":"CLEAN","reviewDecision":"",
            "statusCheckRollup":[{"name":"ci","conclusion":"SUCCESS"}],"author":{"login":"a"},
            "labels":[{"name":"team-fix"}]}]"#;
        let gh = Fake::new(vec![
            ("rate_limit", ok("10\n")),
            ("label needs-human", ok("[]")),
            ("graphql", ok(r#"{"7":0}"#)),
            ("pr list", ok(pr)),
        ]);
        let mut github = Github::open(gh);
        let text = rendered(|out| github.pull_requests(out));
        assert!(text.contains("unresolved threads: 0"), "{text}");
        assert!(text.ends_with("  merge-ready: yes\n"), "{text}");
        let hit = rendered(|out| github.milestone_pr(b"9", out));
        assert!(
            hit.starts_with("- open PR for M9: #7 Phase 4 M9: x ("),
            "{hit}"
        );
        assert_eq!(
            rendered(|out| github.milestone_pr(b"1", out)),
            "- open PR for M1: none\n"
        );
        // M9 must not match inside M91.
        assert_eq!(
            rendered(|out| github.milestone_pr(b"91", out)),
            "- open PR for M91: none\n"
        );
    }

    #[test]
    fn a_failed_issue_list_makes_the_gates_unknown() {
        let gh = Fake::new(vec![
            ("rate_limit", ok("10\n")),
            ("label needs-human", failed("boom")),
            ("graphql", failed("no threads")),
            ("pr list", ok("[]")),
            ("agent-ready", ok("3\n")),
            ("agent-working", failed("x")),
        ]);
        let mut github = Github::open(gh);
        assert_eq!(rendered(|out| github.pull_requests(out)), "- none\n");
        assert_eq!(
            rendered(|out| github.needs_human(out)),
            "- GitHub unavailable: boom\n- queue: 3 agent-ready, ? agent-working\n"
        );
    }

    #[test]
    fn needs_human_lists_issues_by_number() {
        let issues = r#"[{"number":9,"title":"B\u0001","body":"","updatedAt":"2026-01-02T03:04:05Z"},
            {"number":3,"title":"A","body":"","updatedAt":"2025-12-31T00:00:00Z"}]"#;
        let gh = Fake::new(vec![
            ("rate_limit", ok("10\n")),
            ("label needs-human", ok(issues)),
            ("graphql", ok("{}")),
            ("agent-", ok("0\n")),
        ]);
        let github = Github::open(gh);
        assert_eq!(
            rendered(|out| github.needs_human(out)),
            "- #3 A (updated 2025-12-31)\n- #9 B? (updated 2026-01-02)\n\
             - queue: 0 agent-ready, 0 agent-working\n"
        );
    }

    fn run(workflow: &str, status: &str, conclusion: &str, sha: &str, at: &str) -> Value {
        serde_json::json!({"workflowName": workflow, "status": status, "conclusion": conclusion,
            "headSha": sha, "createdAt": at, "event": "push"})
    }

    #[test]
    fn ci_keeps_the_newest_run_and_lets_the_tip_suite_win_a_tie() {
        let tip = "0123456789abcdef0123456789abcdef01234567";
        let suites = serde_json::json!({"data": {"repository": {"object": {"checkSuites": {"nodes": [
            {"status": "COMPLETED", "conclusion": "SUCCESS", "branch": {"name": "main"},
             "workflowRun": {"event": "push", "createdAt": "2026-01-02T00:00:00Z",
                             "workflow": {"name": "CI"}}},
            {"status": "COMPLETED", "conclusion": "SUCCESS", "branch": {"name": "other"},
             "workflowRun": {"event": "push", "createdAt": "2026-01-03T00:00:00Z",
                             "workflow": {"name": "Skipped"}}},
            {"status": "COMPLETED", "conclusion": "SUCCESS", "branch": {"name": "main"},
             "workflowRun": null}
        ]}}}}});
        let runs = vec![
            run(
                "CI",
                "completed",
                "failure",
                "aaaaaaaaaa",
                "2026-01-02T00:00:00Z",
            ),
            run(
                "Docs",
                "in_progress",
                "",
                "bbbbbbbbbb",
                "2026-01-01T10:20:30Z",
            ),
        ];
        assert_eq!(
            ci_lines(runs, &suites, tip),
            vec![
                "- CI: success @ 0123456 (push, 2026-01-02 00:00 UTC)".to_string(),
                "- Docs: in_progress @ bbbbbbb (push, 2026-01-01 10:20 UTC) - not the origin/main tip"
                    .to_string(),
            ]
        );
        assert_eq!(
            ci_lines(Vec::new(), &Value::Null, ""),
            vec!["- no runs on main".to_string()]
        );
        // Without a tip, no run is "not the tip".
        let runs = vec![run(
            "CI",
            "completed",
            "success",
            "cccccccccc",
            "2026-01-01T00:00:00Z",
        )];
        assert_eq!(
            ci_lines(runs, &Value::Null, ""),
            vec!["- CI: success @ ccccccc (push, 2026-01-01 00:00 UTC)".to_string()]
        );
    }

    #[test]
    fn jq_order_puts_null_first_and_compares_strings_bytewise() {
        assert_eq!(jq_cmp(&Value::Null, &Value::from("a")), Ordering::Less);
        assert_eq!(jq_cmp(&Value::from("B"), &Value::from("a")), Ordering::Less);
        assert_eq!(jq_cmp(&Value::from(2), &Value::from(10)), Ordering::Less);
        assert_eq!(head(&Value::Null, 7), "null");
        assert_eq!(head(&Value::from("héllo wörld"), 7), "héllo w");
    }
}
