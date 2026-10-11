//! `aios hook` plumbing: the stdin cap, input parsing, output discipline, exit
//! codes, session-id sanitising, the state directory and atomic writes.

mod hook_support;

use std::ffi::OsString;

use aios_tools::cmd::hook::{
    parse_input, read_capped, sanitize_id, state_key, write_atomic, Ctx, HookInput, MAX_INPUT_BYTES,
};
use hook_support::{git, make_git_repo, run_hook, unique_dir, SUBCOMMANDS};
use serde_json::Value;

fn deny_reason(stdout: &str) -> String {
    let v: Value = serde_json::from_str(stdout).expect("stdout is one JSON object");
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
    v["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .expect("a reason")
        .to_string()
}

fn input_with_session(id: &str) -> HookInput {
    let payload = serde_json::json!({ "session_id": id });
    parse_input(payload.to_string().as_bytes()).expect("parse")
}

#[test]
fn every_subcommand_exits_zero_on_valid_input_with_no_output() {
    let cwd = unique_dir("valid");
    // route-shadow logs even a payload it has no use for, so keep its log out of
    // the repository's real state directory.
    let state = unique_dir("valid-state");
    let env = [("AIOS_HOOK_STATE_DIR", state.to_str().expect("UTF-8 path"))];
    for args in SUBCOMMANDS {
        let run = run_hook(args, br#"{"tool_name":"Read","tool_input":{}}"#, &env, &cwd);
        assert_eq!(run.code, Some(0), "{args:?}: {}", run.stderr);
        assert_eq!(run.stdout, "", "{args:?}");
        assert_eq!(run.stderr, "", "{args:?}");
    }
}

#[test]
fn malformed_json_exits_zero_and_follows_each_error_policy() {
    let cwd = unique_dir("malformed");
    for args in SUBCOMMANDS {
        for input in [&b"{not json"[..], b"", b"[1,2]", b"\"text\"", b"\xff\xfe"] {
            let run = run_hook(args, input, &[], &cwd);
            assert_eq!(run.code, Some(0), "{args:?} {input:?}");
            assert!(run.stderr.contains(args[0]), "{args:?}: {}", run.stderr);
            if args[0] == "path-guard" {
                let reason = deny_reason(&run.stdout);
                assert!(reason.contains("path-guard"), "{reason}");
            } else {
                assert_eq!(run.stdout, "", "{args:?} fails open");
            }
        }
    }
}

#[test]
fn input_over_the_cap_is_an_input_error_and_still_exits_zero() {
    let cwd = unique_dir("overcap");
    // Valid JSON padded with whitespace to one byte over the cap.
    let mut input = br#"{"tool_name":"Read"}"#.to_vec();
    input.resize(MAX_INPUT_BYTES + 1, b' ');
    for args in SUBCOMMANDS {
        let run = run_hook(args, &input, &[], &cwd);
        assert_eq!(run.code, Some(0), "{args:?}");
        assert!(
            run.stderr.contains("larger than"),
            "{args:?}: {}",
            run.stderr
        );
        assert_eq!(run.stdout.is_empty(), args[0] != "path-guard", "{args:?}");
    }
}

#[test]
fn input_exactly_at_the_cap_is_accepted() {
    let cwd = unique_dir("atcap");
    let mut input = br#"{"tool_name":"Read"}"#.to_vec();
    input.resize(MAX_INPUT_BYTES, b' ');
    let run = run_hook(&["repeat-error"], &input, &[], &cwd);
    assert_eq!(run.code, Some(0));
    assert_eq!(run.stderr, "");
}

#[test]
fn read_capped_enforces_the_cap_exactly() {
    assert_eq!(read_capped(&b"abc"[..], 3).unwrap(), b"abc");
    assert_eq!(read_capped(&b""[..], 0).unwrap(), b"");
    let err = read_capped(&b"abcd"[..], 3).unwrap_err();
    assert!(err.to_string().contains("larger than 3 bytes"), "{err}");
}

#[test]
fn unknown_fields_and_wrong_types_are_tolerated() {
    let input = parse_input(
        br#"{"session_id":7,"cwd":["x"],"agent_id":null,"tool_name":"Bash","new_field":{"a":1},
             "is_interrupt":"yes","tool_input":{"command":5}}"#,
    )
    .expect("a lenient parse");
    assert_eq!(input.session_id, None);
    assert_eq!(input.cwd, None);
    assert_eq!(input.agent_id, None);
    assert_eq!(input.is_interrupt, None);
    assert_eq!(input.tool_name.as_deref(), Some("Bash"));
    assert_eq!(input.tool_str("command"), None);
}

#[test]
fn plain_session_ids_pass_through() {
    for id in ["abc", "A-b_9", "0123456789abcdef", &"x".repeat(128)] {
        assert_eq!(sanitize_id(id), id);
    }
}

#[test]
fn unsafe_session_ids_become_a_fixed_width_digest() {
    for id in [
        "../../x",
        "..",
        "a/b",
        "a\\b",
        "a b",
        "a.b",
        "x\0y",
        "é",
        "/etc/passwd",
        "",
        &"x".repeat(129),
    ] {
        let clean = sanitize_id(id);
        assert_eq!(clean.len(), 16, "{id:?} -> {clean}");
        assert!(
            clean
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "{clean}"
        );
        assert!(!clean.contains('.') && !clean.contains('/'), "{clean}");
    }
}

#[test]
fn the_digest_is_stable_and_separates_ids() {
    // FNV-1a 64-bit of the bytes, pinned so a build or std change cannot move state files.
    assert_eq!(sanitize_id(""), "cbf29ce484222325");
    assert_eq!(sanitize_id("../../x"), sanitize_id("../../x"));
    assert_ne!(sanitize_id("../../x"), sanitize_id("../../y"));
    assert_ne!(sanitize_id("a/b"), sanitize_id("a.b"));
}

#[test]
fn state_key_joins_session_and_agent_after_sanitising_both() {
    assert_eq!(state_key(&input_with_session("s1")), "s1");
    let sub = parse_input(br#"{"session_id":"s1","agent_id":"a2"}"#).unwrap();
    assert_eq!(state_key(&sub), "s1-a2");
    let hostile = parse_input(br#"{"session_id":"../../x","agent_id":"../y"}"#).unwrap();
    let key = state_key(&hostile);
    assert_eq!(key.len(), 16 + 1 + 16, "{key}");
    assert!(!key.contains('/') && !key.contains('.'), "{key}");
    let none = parse_input(b"{}").unwrap();
    assert_eq!(state_key(&none), sanitize_id(""));
}

#[test]
fn the_state_dir_override_wins_and_ignores_git() {
    let dir = unique_dir("override");
    let ctx = Ctx::new(Some(OsString::from(&dir)), None);
    let input = parse_input(br#"{"cwd":"/nonexistent/anywhere"}"#).unwrap();
    assert_eq!(ctx.state_dir(&input).unwrap(), dir);
}

#[test]
fn an_empty_override_counts_as_unset() {
    let repo = make_git_repo("emptyoverride");
    let ctx = Ctx::new(Some(OsString::new()), Some(repo.clone()));
    let got = ctx.state_dir(&HookInput::default()).unwrap();
    assert_eq!(got, repo.join(".git/aios-agent/hooks"));
}

#[test]
fn the_state_dir_is_under_the_git_common_dir_from_the_payload_cwd() {
    let repo = make_git_repo("commondir");
    let sub = repo.join("a/b");
    std::fs::create_dir_all(&sub).unwrap();
    let ctx = Ctx::new(None, Some(unique_dir("elsewhere")));
    let payload = serde_json::json!({ "cwd": sub.to_str().unwrap() });
    let input = parse_input(payload.to_string().as_bytes()).unwrap();
    assert_eq!(
        ctx.state_dir(&input).unwrap(),
        repo.join(".git/aios-agent/hooks")
    );
}

#[test]
fn every_worktree_shares_the_main_checkouts_state_dir() {
    let repo = make_git_repo("worktrees");
    let wt = unique_dir("wt").join("linked");
    git(
        &repo,
        &["worktree", "add", "-q", "-b", "side", wt.to_str().unwrap()],
    );
    let ctx = Ctx::new(None, None);
    let payload = serde_json::json!({ "cwd": wt.to_str().unwrap() });
    let input = parse_input(payload.to_string().as_bytes()).unwrap();
    assert_eq!(
        ctx.state_dir(&input).unwrap(),
        repo.join(".git/aios-agent/hooks")
    );
}

#[test]
fn the_state_dir_falls_back_to_the_process_cwd() {
    let repo = make_git_repo("fallback");
    let ctx = Ctx::new(None, Some(repo.clone()));
    assert_eq!(
        ctx.state_dir(&HookInput::default()).unwrap(),
        repo.join(".git/aios-agent/hooks")
    );
}

#[test]
fn the_state_dir_errors_when_git_fails_and_when_there_is_no_cwd() {
    // A `.git` file pointing nowhere makes git fail wherever the test runs, even
    // inside another repository.
    let broken = unique_dir("brokengit");
    std::fs::write(broken.join(".git"), "gitdir: /nonexistent/aios-hook-test\n").unwrap();
    let err = Ctx::new(None, Some(broken.clone()))
        .state_dir(&HookInput::default())
        .unwrap_err();
    assert!(err.to_string().contains("git rev-parse"), "{err}");

    let err = Ctx::new(None, None)
        .state_dir(&HookInput::default())
        .unwrap_err();
    assert!(err.to_string().contains("no cwd"), "{err}");

    let payload = serde_json::json!({ "cwd": broken.join("gone").to_str().unwrap() });
    let input = parse_input(payload.to_string().as_bytes()).unwrap();
    assert!(Ctx::new(None, None).state_dir(&input).is_err());
}

#[test]
fn write_atomic_creates_directories_replaces_and_leaves_no_temp_files() {
    let dir = unique_dir("atomic");
    let path = dir.join("sub/state.json");
    write_atomic(&path, b"one").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"one");
    write_atomic(&path, b"two").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"two");
    let names: Vec<_> = std::fs::read_dir(dir.join("sub"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![OsString::from("state.json")]);
}

#[test]
fn write_atomic_cleans_up_when_the_rename_fails() {
    let dir = unique_dir("atomicfail");
    // A directory at the target makes the rename fail after the temp file exists.
    let path = dir.join("target");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("keep"), b"x").unwrap();
    assert!(write_atomic(&path, b"data").is_err());
    let names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![OsString::from("target")]);
}
