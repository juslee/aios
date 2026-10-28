//! The merge verdict of a review round: `ready`, `pending` or `needs-human`.
//! In R2a it is reported, never written to GitHub.
//!
//! It is brief's `merge_ready::blockers` plus the loop's own checks, so `ready`
//! is never looser than brief's `merge-ready: yes`. `ready` needs all of:
//! - the review is `clean`;
//! - `blockers` is empty;
//! - no changed path gates the PR (Option A, [`super::gate`]);
//! - GitHub's file list matches git's;
//! - the PR head did not move during the round.
//!
//! (The kill switch is checked by the caller: a stopped run ends with exit 4
//! and has no verdict.)
//!
//! Anything else is `pending` when every reason is transient (checks still
//! running, GitHub still computing the merge state, the branch behind `main`, a
//! gh read that failed) and `needs-human` otherwise. A blocker text this module
//! does not know is `needs-human`: the classifier fails closed. A model stage
//! can add findings, never make a gated or blocked PR `ready`.

use serde_json::Value;

use super::gather::show_path;
use super::review::ReviewStatus;
use crate::merge_ready::blockers;

/// The verdict, with its reasons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Ready,
    /// Wait and review again: the reasons are transient.
    Pending(Vec<String>),
    NeedsHuman(Vec<String>),
}

impl Verdict {
    /// `ready`, `pending: <reasons>` or `needs-human: <reasons>`.
    pub fn line(&self) -> String {
        match self {
            Verdict::Ready => "ready".to_string(),
            Verdict::Pending(r) => format!("pending: {}", r.join("; ")),
            Verdict::NeedsHuman(r) => format!("needs-human: {}", r.join("; ")),
        }
    }

    pub fn is_ready(&self) -> bool {
        *self == Verdict::Ready
    }

    /// `ready`, `pending` or `needs-human`.
    pub fn class(&self) -> &'static str {
        match self {
            Verdict::Ready => "ready",
            Verdict::Pending(_) => "pending",
            Verdict::NeedsHuman(_) => "needs-human",
        }
    }

    pub fn reasons(&self) -> &[String] {
        match self {
            Verdict::Ready => &[],
            Verdict::Pending(r) | Verdict::NeedsHuman(r) => r,
        }
    }
}

/// `blockers` split into transient and not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Classified {
    pub pending: Vec<String>,
    pub needs_human: Vec<String>,
}

/// `N pending`, as `merge_ready::blockers` writes it.
fn is_pending_count(text: &str) -> bool {
    text.strip_suffix(" pending")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Split brief's blocker texts. Pending:
/// - `N pending`;
/// - `mergeable: UNKNOWN` and `merge state: UNKNOWN` (GitHub is still
///   computing);
/// - `merge state: BLOCKED` or `UNSTABLE`, only together with `N pending` (the
///   state is the pending checks, not a rule);
/// - `merge state: BEHIND` (an update of the branch fixes it);
/// - `review threads unknown` and `needs-human gates unknown` (a gh read
///   failed; reading again may succeed).
///
/// Every other text is needs-human, including one this function has never
/// seen.
pub fn classify(blockers: &[String]) -> Classified {
    let has_pending = blockers.iter().any(|b| is_pending_count(b));
    let mut out = Classified::default();
    for b in blockers {
        let transient = is_pending_count(b)
            || matches!(
                b.as_str(),
                "mergeable: UNKNOWN"
                    | "merge state: UNKNOWN"
                    | "merge state: BEHIND"
                    | "review threads unknown"
                    | "needs-human gates unknown"
            )
            || (has_pending
                && matches!(b.as_str(), "merge state: BLOCKED" | "merge state: UNSTABLE"));
        if transient {
            out.pending.push(b.clone());
        } else {
            out.needs_human.push(b.clone());
        }
    }
    out
}

/// What the verdict reads.
pub struct Inputs<'a> {
    pub review: ReviewStatus,
    /// The whole `gh pr view` object ([`super::github::PrView::raw`]).
    pub pr: &'a Value,
    /// The open needs-human issues; `None` when they could not be read.
    pub issues: Option<&'a [Value]>,
    /// The count of unresolved review threads; `None` when unknown.
    pub open_threads: Option<u64>,
    /// [`super::gather::Facts::gated`]: the paths that gate the PR.
    pub gated: &'a [String],
    /// [`super::gate::cross_check`]'s reason, if it failed.
    pub cross_check: Option<&'a str>,
    /// The PR head moved after the round fetched it.
    pub head_moved: bool,
}

/// The verdict of a round.
pub fn verdict(i: &Inputs<'_>) -> Verdict {
    let classified = classify(&blockers(i.pr, i.issues, i.open_threads));
    let mut needs_human = classified.needs_human;
    let mut pending = classified.pending;
    for path in i.gated {
        needs_human.push(format!("gated path: {}", show_path(path)));
    }
    if let Some(reason) = i.cross_check {
        needs_human.push(reason.to_string());
    }
    match i.review {
        ReviewStatus::Clean => {}
        ReviewStatus::Findings => needs_human.push("the review has confirmed findings".to_string()),
        ReviewStatus::Incomplete => needs_human.push("the review is incomplete".to_string()),
    }
    if i.head_moved {
        pending.push("the PR head moved during the review".to_string());
    }
    if !needs_human.is_empty() {
        needs_human.extend(pending);
        Verdict::NeedsHuman(needs_human)
    } else if !pending.is_empty() {
        Verdict::Pending(pending)
    } else {
        Verdict::Ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A PR that `blockers` finds nothing wrong with.
    fn good_pr() -> Value {
        json!({
            "number": 7,
            "isDraft": false,
            "mergeable": "MERGEABLE",
            "mergeStateStatus": "CLEAN",
            "reviewDecision": "",
            "statusCheckRollup": [{"name": "ci", "conclusion": "SUCCESS"}],
            "labels": [],
        })
    }

    fn inputs<'a>(pr: &'a Value, gated: &'a [String]) -> Inputs<'a> {
        Inputs {
            review: ReviewStatus::Clean,
            pr,
            issues: Some(&[]),
            open_threads: Some(0),
            gated,
            cross_check: None,
            head_moved: false,
        }
    }

    #[test]
    fn a_clean_good_pr_is_ready() {
        let pr = good_pr();
        assert_eq!(verdict(&inputs(&pr, &[])), Verdict::Ready);
        assert_eq!(Verdict::Ready.line(), "ready");
    }

    #[test]
    fn pending_checks_give_pending_and_the_blocked_state_rides_along() {
        let mut pr = good_pr();
        pr["statusCheckRollup"] = json!([
            {"name": "a", "conclusion": "SUCCESS"},
            {"name": "b", "status": "IN_PROGRESS"},
        ]);
        pr["mergeStateStatus"] = json!("BLOCKED");
        let v = verdict(&inputs(&pr, &[]));
        assert_eq!(
            v,
            Verdict::Pending(vec![
                "1 pending".to_string(),
                "merge state: BLOCKED".to_string()
            ])
        );
        // BLOCKED with nothing pending is a rule, not a wait.
        let mut pr = good_pr();
        pr["mergeStateStatus"] = json!("BLOCKED");
        match verdict(&inputs(&pr, &[])) {
            Verdict::NeedsHuman(r) => assert_eq!(r, ["merge state: BLOCKED"]),
            other => panic!("{other:?}"),
        }
    }

    /// One PR built to make `blockers` print `text`, and the class it must get.
    struct Case {
        pr: Value,
        issues: Option<Vec<Value>>,
        threads: Option<u64>,
        text: String,
        pending: bool,
    }

    fn case(pr: Value, text: &str, pending: bool) -> Case {
        Case {
            pr,
            issues: Some(vec![]),
            threads: Some(0),
            text: text.to_string(),
            pending,
        }
    }

    fn with(pr: Value, key: &str, value: Value) -> Value {
        let mut pr = pr;
        pr[key] = value;
        pr
    }

    #[test]
    fn every_blocker_text_has_a_class() {
        let mut cases = vec![
            case(with(good_pr(), "isDraft", json!(true)), "draft", false),
            case(
                with(good_pr(), "labels", json!([{"name": "needs-human"}])),
                "labelled needs-human",
                false,
            ),
            case(
                with(good_pr(), "statusCheckRollup", json!([])),
                "no checks reported",
                false,
            ),
            case(
                with(
                    good_pr(),
                    "statusCheckRollup",
                    json!([{"name": "x", "conclusion": "FAILURE"}]),
                ),
                "1 failing",
                false,
            ),
            case(
                with(
                    good_pr(),
                    "statusCheckRollup",
                    json!([{"name": "x", "status": "QUEUED"}]),
                ),
                "1 pending",
                true,
            ),
            case(
                with(good_pr(), "reviewDecision", json!("CHANGES_REQUESTED")),
                "changes requested",
                false,
            ),
        ];
        let mut gated = case(good_pr(), "gated by #9", false);
        gated.issues = Some(vec![json!({"number": 9, "title": "gate #7", "body": ""})]);
        cases.push(gated);
        let mut named = case(good_pr(), "named in needs-human #9", false);
        named.issues = Some(vec![json!({"number": 9, "title": "t", "body": "see #7"})]);
        cases.push(named);
        let mut unknown_gates = case(good_pr(), "needs-human gates unknown", true);
        unknown_gates.issues = None;
        cases.push(unknown_gates);
        let mut unknown_threads = case(good_pr(), "review threads unknown", true);
        unknown_threads.threads = None;
        cases.push(unknown_threads);
        let mut threads = case(good_pr(), "2 unresolved review thread(s)", false);
        threads.threads = Some(2);
        cases.push(threads);
        for (value, pending) in [("UNKNOWN", true), ("CONFLICTING", false)] {
            cases.push(case(
                with(good_pr(), "mergeable", json!(value)),
                &format!("mergeable: {value}"),
                pending,
            ));
        }
        for (state, pending) in [
            ("UNKNOWN", true),
            ("BEHIND", true),
            ("DIRTY", false),
            ("HAS_HOOKS", false),
            ("BLOCKED", false),
            ("UNSTABLE", false),
        ] {
            cases.push(case(
                with(good_pr(), "mergeStateStatus", json!(state)),
                &format!("merge state: {state}"),
                pending,
            ));
        }
        for c in cases {
            let produced = blockers(&c.pr, c.issues.as_deref(), c.threads);
            assert!(
                produced.contains(&c.text),
                "{:?} not in {produced:?}",
                c.text
            );
            let class = classify(std::slice::from_ref(&c.text));
            assert_eq!(class.pending.len() == 1, c.pending, "{}", c.text);
            assert_eq!(class.needs_human.len() == 1, !c.pending, "{}", c.text);
        }
    }

    #[test]
    fn an_unknown_blocker_text_is_needs_human() {
        let c = classify(&["something new".to_string(), "3 pending".to_string()]);
        assert_eq!(c.needs_human, ["something new"]);
        assert_eq!(c.pending, ["3 pending"]);
        // "pending" must be a count.
        assert_eq!(classify(&["x pending".to_string()]).needs_human.len(), 1);
    }

    #[test]
    fn gate_cross_check_and_review_are_needs_human_whatever_else_holds() {
        let pr = good_pr();
        let gated = vec!["tools/x.rs".to_string(), "docs/\u{e9}.md".to_string()];
        match verdict(&inputs(&pr, &gated)) {
            Verdict::NeedsHuman(r) => {
                assert_eq!(r, ["gated path: tools/x.rs", "gated path: docs/\\u{e9}.md"]);
            }
            other => panic!("{other:?}"),
        }
        let mut i = inputs(&pr, &[]);
        i.cross_check = Some("GitHub and git disagree on the changed files: x");
        assert!(matches!(verdict(&i), Verdict::NeedsHuman(_)));
        let mut i = inputs(&pr, &[]);
        i.review = ReviewStatus::Findings;
        assert!(matches!(verdict(&i), Verdict::NeedsHuman(_)));
        i.review = ReviewStatus::Incomplete;
        assert!(matches!(verdict(&i), Verdict::NeedsHuman(_)));
        let mut i = inputs(&pr, &[]);
        i.head_moved = true;
        assert_eq!(
            verdict(&i),
            Verdict::Pending(vec!["the PR head moved during the review".to_string()])
        );
    }

    #[test]
    fn needs_human_lists_the_pending_reasons_after_its_own() {
        let mut pr = good_pr();
        pr["statusCheckRollup"] = json!([{"name": "x", "status": "QUEUED"}]);
        let v = verdict(&Inputs {
            review: ReviewStatus::Findings,
            ..inputs(&pr, &[])
        });
        assert_eq!(
            v.line(),
            "needs-human: the review has confirmed findings; 1 pending"
        );
    }
}
