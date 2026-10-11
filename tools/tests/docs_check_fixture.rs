//! The docs-check fixture repository: every drift variant injects exactly its own new
//! finding, and the base, line-shift, skip and grown variants behave as check.py did.

mod common;

use aios_tools::cmd::docs_check::model::CHECK_ORDER;
use common::fixture::{fixtures_dir, materialize_fixture, parse_bundle, Op, VARIANTS};
use common::{run_aios, TestRepo};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;

/// The three findings of the base fixture (`--all`).
const BASE_KEYS: [&str; 3] = [
    "md-links|docs/kernel/alpha.md|old-spec.md",
    "md-links|docs/kernel/beta.md|older-spec.md",
    "pointer-doctor|.claude/agents/worker.md|path:docs/retired/guide.md",
];

/// Run `aios docs-check --json <extra>` in `repo`; return the exit code and the JSON.
fn docs_check_json(repo: &TestRepo, extra: &[&str]) -> (i32, Value) {
    let mut args = vec!["docs-check", "--json"];
    args.extend_from_slice(extra);
    let run = run_aios(repo.path(), &args);
    let json = serde_json::from_slice(&run.stdout).unwrap_or_else(|e| {
        panic!(
            "aios {} printed invalid JSON ({e}); stderr: {}",
            args.join(" "),
            String::from_utf8_lossy(&run.stderr)
        )
    });
    (run.code, json)
}

/// The `key` of every listed finding.
fn finding_keys(json: &Value) -> BTreeSet<String> {
    json["findings"]
        .as_array()
        .expect("findings is an array")
        .iter()
        .map(|f| {
            f["key"]
                .as_str()
                .expect("finding key is a string")
                .to_string()
        })
        .collect()
}

/// The listed finding with this key.
fn finding<'a>(json: &'a Value, key: &str) -> &'a Value {
    json["findings"]
        .as_array()
        .expect("findings is an array")
        .iter()
        .find(|f| f["key"] == key)
        .unwrap_or_else(|| panic!("no finding {key} in {json}"))
}

#[test]
fn bundle_parser_splits_directives_and_content() {
    let ops = parse_bundle(
        "@@@ commit Phase 0 M1: Step 1 — boot\n@@@ file a/b.md\n# B\n\ntext\n@@@ append c.md\n@@@ prepend d.md\n\n\n@@@ delete e.md\n@@@ option no-phase-history\n",
    );
    assert_eq!(
        ops,
        vec![
            Op::Commit("Phase 0 M1: Step 1 — boot".to_string()),
            Op::File("a/b.md".to_string(), "# B\n\ntext\n".to_string()),
            Op::Append("c.md".to_string(), String::new()),
            Op::Prepend("d.md".to_string(), "\n\n".to_string()),
            Op::Delete("e.md".to_string()),
            Op::Flag("no-phase-history".to_string()),
        ]
    );
}

#[test]
#[should_panic(expected = "takes no content lines")]
fn bundle_parser_rejects_content_after_commit() {
    parse_bundle("@@@ commit Phase 0 M1: x\nstray content\n");
}

#[test]
#[should_panic(expected = "a bundle path must be a plain relative path")]
fn bundle_parser_rejects_a_path_that_escapes_the_tree() {
    parse_bundle("@@@ file ../x\n");
}

#[test]
#[should_panic(expected = "a bundle path must be a plain relative path")]
fn bundle_parser_rejects_an_absolute_path() {
    parse_bundle("@@@ file /abs\n");
}

#[test]
#[should_panic(expected = "needs a path, found none")]
fn bundle_parser_rejects_an_empty_path() {
    parse_bundle("@@@ file \n");
}

#[test]
fn variants_cover_every_bundle_in_check_order() {
    let mut files: Vec<String> = fs::read_dir(fixtures_dir().join("variants"))
        .expect("variants directory is readable")
        .map(|entry| {
            let name = entry.expect("readable entry").file_name();
            let name = name.to_str().expect("UTF-8 file name").to_string();
            name.strip_suffix(".txt")
                .unwrap_or_else(|| panic!("variants/{name} is not a .txt bundle"))
                .to_string()
        })
        .collect();
    files.sort();
    let mut names: Vec<&str> = VARIANTS
        .iter()
        .map(|v| v.name)
        .filter(|name| *name != "base")
        .collect();
    names.sort_unstable();
    assert_eq!(files, names);
    let drift: Vec<&str> = VARIANTS[1..16].iter().map(|v| v.name).collect();
    assert_eq!(drift, CHECK_ORDER);
}

#[test]
fn each_variant_reports_exactly_its_new_keys() {
    let mut failures = Vec::new();
    for variant in VARIANTS {
        let repo = materialize_fixture(variant.name);
        let (code, json) = docs_check_json(&repo, &[]);
        let want_code = if variant.expect_new.is_empty() { 0 } else { 1 };
        let want: BTreeSet<String> = variant.expect_new.iter().map(|k| k.to_string()).collect();
        let got = finding_keys(&json);
        if code != want_code || got != want {
            failures.push(format!(
                "{}: exit {code} (want {want_code}), new keys {got:?} (want {want:?})",
                variant.name
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn base_lists_three_findings_one_resolved_one_reduced() {
    let repo = materialize_fixture("base");
    let (code, json) = docs_check_json(&repo, &["--all"]);
    assert_eq!(code, 0);
    let want: BTreeSet<String> = BASE_KEYS.iter().map(|k| k.to_string()).collect();
    assert_eq!(finding_keys(&json), want);
    assert_eq!(
        json["resolved"],
        json!(["anchors|docs/kernel/alpha.md|#gone"])
    );
    assert_eq!(
        json["reduced"],
        json!({"md-links|docs/kernel/beta.md|older-spec.md": {"baseline": 2, "current": 1}})
    );
    let accepted = finding(&json, BASE_KEYS[2]);
    assert_eq!(
        accepted["accepted"],
        "false positive: the agent cites the retired guide on purpose"
    );
}

#[test]
fn line_shift_moves_lines_but_keeps_keys() {
    let repo = materialize_fixture("line-shift");
    let (code, json) = docs_check_json(&repo, &["--all"]);
    assert_eq!(code, 0);
    let old = finding(&json, BASE_KEYS[0]);
    assert_eq!(old["line"], 15);
    assert_eq!(old["also"], json!([19]));
    assert_eq!(old["new"], false);
}

#[test]
fn skip_variant_skips_milestone_status() {
    let repo = materialize_fixture("skip");
    let (code, json) = docs_check_json(&repo, &[]);
    assert_eq!(code, 0);
    assert_eq!(
        json["checks"]["milestone-status"],
        json!({"skipped": "no 'Phase N MK:' commits found on main's first-parent history"})
    );
}

#[test]
fn grown_variant_reports_the_extra_occurrence() {
    let repo = materialize_fixture("grown");
    let (code, json) = docs_check_json(&repo, &[]);
    assert_eq!(code, 1);
    let grown = finding(&json, BASE_KEYS[0]);
    assert_eq!(grown["line"], 12);
    assert_eq!(grown["also"], json!([16, 17]));
    assert_eq!(grown["count"], 3);
    assert_eq!(grown["baseline_count"], 2);
    assert_eq!(grown["new"], true);
}
