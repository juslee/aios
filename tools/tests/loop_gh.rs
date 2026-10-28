//! The loop's gh reads (R2a, Task 4c), against a fake [`GhRunner`] and the
//! recorded fixtures of Task 1.

use aios_tools::cmd::pr_loop::github::{
    issues_labelled, issues_needs_human, json, label_exists, pr_checks, pr_view, repo_slug,
    review_threads, run_id_of, run_log_failed, Slug, LOOP_GH_ENV,
};
use aios_tools::gh::{is_read, GhOutcome, GhRunner};
use serde_json::{json as j, Value};
use std::cell::RefCell;
use std::path::PathBuf;

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/loop/gh")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A runner that answers from a closure and records every argv. Each call must
/// pass the read allowlist, as the real runner would require.
struct Fake<F: Fn(&[&str]) -> GhOutcome> {
    answer: F,
    calls: RefCell<Vec<Vec<String>>>,
}

fn fake<F: Fn(&[&str]) -> GhOutcome>(answer: F) -> Fake<F> {
    Fake {
        answer,
        calls: RefCell::new(Vec::new()),
    }
}

impl<F: Fn(&[&str]) -> GhOutcome> GhRunner for Fake<F> {
    fn run(&self, args: &[&str]) -> GhOutcome {
        assert_eq!(is_read(args), Ok(()), "not a read: {args:?}");
        self.calls
            .borrow_mut()
            .push(args.iter().map(|a| a.to_string()).collect());
        (self.answer)(args)
    }
}

fn ok(data: Vec<u8>) -> GhOutcome {
    GhOutcome::Ok(data)
}

fn failed(stderr: &str) -> GhOutcome {
    GhOutcome::Failed {
        status: Some(1),
        stderr: stderr.as_bytes().to_vec(),
    }
}

#[test]
fn loop_gh_env_strips_the_repo_and_host_and_quiets_gh() {
    assert_eq!(LOOP_GH_ENV.remove, ["GH_REPO", "GH_HOST"]);
    for pair in [
        ("GH_PROMPT_DISABLED", "1"),
        ("GH_NO_UPDATE_NOTIFIER", "1"),
        ("NO_COLOR", "1"),
    ] {
        assert!(LOOP_GH_ENV.set.contains(&pair), "{pair:?}");
    }
}

#[test]
fn pr_view_fixture_deserialises() {
    let r = fake(|_| ok(fixture("pr_view.json")));
    let v = pr_view(&r, 257).expect("pr view");
    assert_eq!(v.facts.head_ref_oid.len(), 40);
    assert_eq!(v.facts.base_ref_name, "main");
    assert!(!v.facts.is_cross_repository);
    assert_eq!(v.facts.changed_files, 835);
    assert!(!v.facts.files.is_empty());
    assert!(v.facts.files.iter().all(|f| !f.path.is_empty()));
    assert!(v.raw["reviews"].is_array());
    assert!(!v.has_label("agent-stop"));
    let argv = r.calls.borrow()[0].clone();
    assert_eq!(&argv[..4], ["pr", "view", "257", "--json"]);
    assert!(
        argv[4].starts_with("number,title,headRefName,"),
        "{}",
        argv[4]
    );
    assert!(argv[4].ends_with(",commits,reviews"), "{}", argv[4]);
}

#[test]
fn pr_checks_fixture_deserialises_with_run_ids() {
    let r = fake(|_| ok(fixture("checks.json")));
    let checks = pr_checks(&r, 257).expect("pr checks");
    assert!(!checks.is_empty());
    assert!(checks
        .iter()
        .all(|c| !c.name.is_empty() && c.run_id.is_some()));
    assert_eq!(checks[0].name, "claude-review");
    assert_eq!(checks[0].run_id, Some(39384126399));
    assert_eq!(
        r.calls.borrow()[0],
        ["pr", "checks", "257", "--json", "name,state,bucket,link"]
    );
}

#[test]
fn run_id_of_reads_run_links_only() {
    assert_eq!(
        run_id_of("https://github.com/o/r/actions/runs/123/job/456"),
        Some(123)
    );
    assert_eq!(
        run_id_of("https://github.com/o/r/actions/runs/123"),
        Some(123)
    );
    assert_eq!(run_id_of("https://example.com/status"), None);
    assert_eq!(run_id_of("https://github.com/o/r/actions/runs/x"), None);
    assert_eq!(run_id_of(""), None);
}

#[test]
fn repo_slug_reads_owner_login_and_name() {
    let r = fake(|_| ok(br#"{"owner":{"id":"X","login":"juslee"},"name":"aios"}"#.to_vec()));
    assert_eq!(
        repo_slug(&r).expect("slug"),
        Slug {
            owner: "juslee".into(),
            name: "aios".into()
        }
    );
    let r = fake(|_| ok(b"{}".to_vec()));
    assert!(repo_slug(&r).is_err());
}

#[test]
fn review_threads_fixture_deserialises() {
    let r = fake(|_| ok(fixture("threads_p1.json")));
    let slug = Slug {
        owner: "juslee".into(),
        name: "aios".into(),
    };
    let threads = review_threads(&r, &slug, 144).expect("threads");
    assert_eq!(threads.len(), 9);
    assert!(threads.iter().all(|t| !t.id.is_empty()));
    assert!(threads[0].is_resolved && threads[0].is_outdated);
    assert_eq!(threads[0].path.as_deref(), Some("kernel/src/input/mod.rs"));
    assert_eq!(threads[0].line, None);
    assert_eq!(
        threads[0].comments[0].author.as_deref(),
        Some("copilot-pull-request-reviewer")
    );
    assert!(threads[0].comments[0].database_id.is_some());
    assert_eq!(r.calls.borrow().len(), 1, "hasNextPage is false: one call");
}

fn page(first: usize, count: usize, next: Option<&str>) -> Vec<u8> {
    let nodes: Vec<Value> = (first..first + count)
        .map(|i| {
            j!({"id": format!("T{i}"), "isResolved": false, "isOutdated": false,
                "path": "a.rs", "line": i,
                "comments": {"nodes": [{"databaseId": i, "author": {"login": "juslee"}, "body": "b"}]}})
        })
        .collect();
    serde_json::to_vec(
        &j!({"data": {"repository": {"pullRequest": {"reviewThreads": {
        "pageInfo": {"hasNextPage": next.is_some(), "endCursor": next},
        "nodes": nodes}}}}}),
    )
    .expect("serialise")
}

#[test]
fn review_threads_pages_past_100() {
    let r = fake(|args| {
        if args.contains(&"c=CURSOR1") {
            ok(page(100, 3, None))
        } else {
            ok(page(0, 100, Some("CURSOR1")))
        }
    });
    let slug = Slug {
        owner: "o".into(),
        name: "n".into(),
    };
    let threads = review_threads(&r, &slug, 5).expect("threads");
    assert_eq!(threads.len(), 103);
    assert_eq!(threads[102].id, "T102");
    let calls = r.calls.borrow();
    assert_eq!(calls.len(), 2);
    assert!(!calls[0].iter().any(|a| a.starts_with("c=")));
    assert!(calls[1].contains(&"c=CURSOR1".to_string()));
    for c in calls.iter() {
        assert!(c.contains(&"owner=o".to_string()) && c.contains(&"name=n".to_string()));
        assert!(c.contains(&"n=5".to_string()));
    }
}

#[test]
fn review_threads_stops_on_a_cursor_that_does_not_move() {
    let r = fake(|_| ok(page(0, 1, Some("SAME"))));
    let slug = Slug {
        owner: "o".into(),
        name: "n".into(),
    };
    let err = review_threads(&r, &slug, 5).expect_err("a stuck cursor");
    assert!(err.to_string().contains("no new cursor"), "{err}");
    assert_eq!(r.calls.borrow().len(), 2);
}

#[test]
fn issues_needs_human_fixture_deserialises() {
    let r = fake(|_| ok(fixture("issues.json")));
    let issues = issues_needs_human(&r).expect("issues");
    assert!(!issues.is_empty());
    assert!(issues[0]["number"].is_u64() || issues[0]["body"].is_string());
    assert_eq!(
        r.calls.borrow()[0],
        aios_tools::merge_ready::NEEDS_HUMAN_ISSUES_ARGS
    );
}

#[test]
fn label_exists_maps_200_404_and_other_failures() {
    let r = fake(|_| ok(fixture("label_agent_stop.json")));
    assert!(label_exists(&r, "agent-stop").expect("exists"));
    assert_eq!(
        r.calls.borrow()[0],
        ["api", "repos/{owner}/{repo}/labels/agent-stop"]
    );

    let stderr = String::from_utf8(fixture("label_404.err")).expect("utf-8");
    let r = fake(move |_| failed(&stderr));
    assert!(!label_exists(&r, "agent-stop").expect("a 404 is false"));

    let r = fake(|_| failed("gh: Bad credentials (HTTP 401)"));
    let err = label_exists(&r, "agent-stop").expect_err("a 401 is an error");
    assert!(err.to_string().contains("HTTP 401"), "{err}");
    let r = fake(|_| failed("error connecting to api.github.com"));
    assert!(label_exists(&r, "agent-stop").is_err());
    let r = fake(|_| GhOutcome::Missing);
    assert!(label_exists(&r, "agent-stop").is_err());

    // A name that is not a plain label name never reaches gh.
    let r = fake(|_| ok(Vec::new()));
    assert!(label_exists(&r, "a/b").is_err());
    assert!(label_exists(&r, "").is_err());
    assert!(r.calls.borrow().is_empty());
}

#[test]
fn issues_labelled_fixture_deserialises() {
    let r = fake(|_| ok(fixture("agent_stop_empty.json")));
    assert!(issues_labelled(&r, "agent-stop")
        .expect("issues")
        .is_empty());
    assert_eq!(
        r.calls.borrow()[0],
        [
            "issue",
            "list",
            "--label",
            "agent-stop",
            "--state",
            "open",
            "--json",
            "number"
        ]
    );
    let r = fake(|_| ok(br#"[{"number":12},{"number":7}]"#.to_vec()));
    assert_eq!(issues_labelled(&r, "agent-stop").expect("issues"), [12, 7]);
}

#[test]
fn run_log_failed_fixture_is_text() {
    let r = fake(|_| ok(fixture("run_log_failed.txt")));
    let text = run_log_failed(&r, 40448856000).expect("log");
    assert!(!text.is_empty());
    assert_eq!(
        r.calls.borrow()[0],
        ["run", "view", "40448856000", "--log-failed"]
    );
}

#[test]
fn json_maps_every_gh_outcome() {
    let args = ["pr", "view", "1"];
    let parsed = json(&fake(|_| ok(b"{\"a\":1}".to_vec())), &args).expect("ok");
    assert_eq!(parsed["a"], 1);

    let err = json(&fake(|_| ok(b"not json".to_vec())), &args).expect_err("invalid JSON");
    assert!(err.to_string().contains("invalid JSON"), "{err}");

    let err = json(&fake(|_| GhOutcome::Missing), &args).expect_err("missing");
    assert_eq!(err.to_string(), "gh is not installed");

    let err = json(
        &fake(|_| failed("HTTP 502: Bad Gateway\nsecond line")),
        &args,
    )
    .expect_err("failed");
    let msg = err.to_string();
    assert!(
        msg.contains("gh pr view failed: HTTP 502: Bad Gateway"),
        "{msg}"
    );
    assert!(!msg.contains("second line"), "{msg}");

    let err = json(
        &fake(|_| GhOutcome::Refused {
            reason: "gh command is not on the read allowlist",
        }),
        &args,
    )
    .expect_err("refused");
    let msg = err.to_string();
    assert!(msg.starts_with("internal error"), "{msg}");
    assert!(msg.contains("read allowlist"), "{msg}");
}

#[test]
fn every_read_argv_passes_is_read() {
    // The fake asserts `is_read` on each call; drive every read once.
    let r = fake(|args| match args[0] {
        "repo" => ok(br#"{"owner":{"login":"o"},"name":"n"}"#.to_vec()),
        "pr" if args[1] == "view" => ok(fixture("pr_view.json")),
        "pr" => ok(fixture("checks.json")),
        "api" if args[1] == "graphql" => ok(fixture("threads_p1.json")),
        "api" => ok(fixture("label_agent_stop.json")),
        "issue" => ok(b"[]".to_vec()),
        _ => ok(fixture("run_log_failed.txt")),
    });
    let slug = repo_slug(&r).expect("slug");
    pr_view(&r, 1).expect("pr view");
    pr_checks(&r, 1).expect("pr checks");
    review_threads(&r, &slug, 1).expect("threads");
    issues_needs_human(&r).expect("needs-human");
    label_exists(&r, "agent-stop").expect("label");
    issues_labelled(&r, "agent-stop").expect("labelled");
    run_log_failed(&r, 1).expect("log");
    assert_eq!(r.calls.borrow().len(), 8);
}

#[test]
fn no_loop_source_spawns_gh_itself() {
    // Every gh call of the loop goes through `gh::GhCli`, which runs `is_read`
    // before it spawns; a `Command::new("gh")` anywhere under cmd/loop would
    // skip that.
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/cmd/loop");
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).expect("read src/cmd/loop") {
        let path = entry.expect("a directory entry").path();
        if path.extension().is_some_and(|e| e == "rs") {
            seen += 1;
            let text = std::fs::read_to_string(&path).expect("read a source file");
            assert!(
                !text.contains("Command::new(\"gh\")"),
                "{} spawns gh itself",
                path.display()
            );
        }
    }
    assert!(seen >= 10, "only {seen} source files found");
}
