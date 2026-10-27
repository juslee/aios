//! The loop's stage runner (R2a, Task 5b): the envelope mapping against the
//! recorded Task 1 fixtures, the pinned argv, the cleared environment, the
//! stage settings and the time limit.

mod common;

use aios_tools::cmd::pr_loop::claude::{
    parse_envelope, validate, Claude, ClaudeCli, StageCall, StageKind, StageResult, ENV_ALLOWLIST,
};
use aios_tools::cmd::pr_loop::settings::{build, materialise};
use aios_tools::cmd::pr_loop::state::State;
use common::soak::check_golden_in;
use common::{git, unique_dir, TestRepo};
use serde_json::{json, Value};
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/loop/claude")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// The envelope
// ---------------------------------------------------------------------------

#[test]
fn success_array_form_is_ok() {
    // The event array: the last `result` event is the result.
    match parse_envelope(&fixture("success.json")) {
        StageResult::Ok {
            value,
            cost,
            denials,
        } => {
            assert_eq!(value, json!({"ok": true}));
            assert!((cost - 0.133268).abs() < 1e-9, "{cost}");
            assert!(denials.is_empty());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn fable_object_form_is_ok() {
    match parse_envelope(&fixture("fable.json")) {
        StageResult::Ok { value, cost, .. } => {
            assert_eq!(value, json!({"ok": true}));
            assert!((cost - 0.02532).abs() < 1e-9, "{cost}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn over_budget_fixture_is_over_budget() {
    match parse_envelope(&fixture("over_budget.json")) {
        StageResult::OverBudget { cost } => assert!((cost - 0.00996).abs() < 1e-9, "{cost}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn denied_fixture_is_ok_with_its_denials() {
    match parse_envelope(&fixture("denied.json")) {
        StageResult::Ok { denials, .. } => assert_eq!(
            denials,
            ["Bash: touch written.txt", "Bash: echo hi > echoed.txt"]
        ),
        other => panic!("{other:?}"),
    }
}

fn result_event(extra: Value) -> Vec<u8> {
    let mut event = json!({"type": "result", "total_cost_usd": 0.01, "permission_denials": []});
    for (k, v) in extra.as_object().expect("an object") {
        event[k] = v.clone();
    }
    event.to_string().into_bytes()
}

#[test]
fn empty_and_broken_output_maps_to_empty_or_error() {
    assert_eq!(parse_envelope(b""), StageResult::Empty);
    assert_eq!(parse_envelope(b"  \n"), StageResult::Empty);
    assert!(matches!(
        parse_envelope(b"{not json"),
        StageResult::Error(_)
    ));
    // Valid JSON without a result event.
    assert!(matches!(parse_envelope(b"[]"), StageResult::Error(_)));
    assert!(matches!(
        parse_envelope(br#"{"type":"system"}"#),
        StageResult::Error(_)
    ));
    // A success with no structured output and nothing denied.
    assert_eq!(
        parse_envelope(&result_event(
            json!({"subtype": "success", "is_error": false})
        )),
        StageResult::Empty
    );
}

#[test]
fn success_without_output_but_with_denials_is_permission_denied() {
    let event = result_event(json!({
        "subtype": "success",
        "is_error": false,
        "permission_denials": [{"tool_name": "Bash", "tool_input": {"command": "gh pr merge"}}],
    }));
    assert_eq!(
        parse_envelope(&event),
        StageResult::PermissionDenied(vec!["Bash: gh pr merge".to_string()])
    );
}

#[test]
fn refusal_and_usage_limit_mappings() {
    // Refusal: the API's stop reason, or a refusal in terminal_reason or subtype.
    for extra in [
        json!({"subtype": "success", "is_error": false, "stop_reason": "refusal"}),
        json!({"subtype": "error_during_execution", "is_error": true, "terminal_reason": "model_refusal"}),
        json!({"subtype": "refusal", "is_error": true}),
    ] {
        assert_eq!(
            parse_envelope(&result_event(extra.clone())),
            StageResult::Refusal,
            "{extra}"
        );
    }
    // Usage limit: a 429, or the CLI's limit text.
    for extra in [
        json!({"subtype": "success", "is_error": true, "api_error_status": 429, "result": "x"}),
        json!({"subtype": "success", "is_error": true, "result": "You've hit your limit"}),
        json!({"subtype": "error_during_execution", "is_error": true, "errors": ["Claude usage limit reached"]}),
    ] {
        assert_eq!(
            parse_envelope(&result_event(extra.clone())),
            StageResult::UsageLimit,
            "{extra}"
        );
    }
}

#[test]
fn unknown_error_subtype_is_an_error() {
    let event = result_event(json!({
        "subtype": "error_during_execution",
        "is_error": true,
        "errors": ["something odd"],
        "stop_reason": "end_turn",
    }));
    match parse_envelope(&event) {
        StageResult::Error(line) => {
            assert!(
                line.contains("error_during_execution") && line.contains("something odd"),
                "{line}"
            );
        }
        other => panic!("{other:?}"),
    }
    // An error flag on a success is an error too, not an Ok.
    let event = result_event(json!({
        "subtype": "success",
        "is_error": true,
        "result": "API Error: overloaded",
        "structured_output": {"findings": []},
    }));
    assert!(matches!(parse_envelope(&event), StageResult::Error(_)));
}

fn ok_with(value: Value) -> StageResult {
    StageResult::Ok {
        value,
        cost: 0.0,
        denials: Vec::new(),
    }
}

fn lens_finding(category: &str) -> Value {
    json!({
        "id": "c1", "lens": "correctness", "severity": "must-fix", "category": category,
        "file": "a.rs", "line": 3, "claim": "c", "evidence": "e", "suggested_fix": null,
    })
}

#[test]
fn a_category_outside_the_taxonomy_is_an_error() {
    let bad = ok_with(json!({"findings": [lens_finding("vibes")]}));
    match validate(StageKind::Lens, bad) {
        StageResult::Error(line) => assert!(line.contains("vibes"), "{line}"),
        other => panic!("{other:?}"),
    }
    let good = ok_with(json!({"findings": [lens_finding("lock-order")]}));
    assert!(matches!(
        validate(StageKind::Lens, good),
        StageResult::Ok { .. }
    ));
    // The wrong shape for the stage kind is an error as well.
    assert!(matches!(
        validate(StageKind::Verify, ok_with(json!({"findings": []}))),
        StageResult::Error(_)
    ));
    assert!(matches!(
        validate(StageKind::Verify, ok_with(json!({"verdicts": []}))),
        StageResult::Ok { .. }
    ));
    // Results that are not Ok pass through untouched.
    assert_eq!(
        validate(StageKind::Lens, StageResult::Timeout),
        StageResult::Timeout
    );
}

// ---------------------------------------------------------------------------
// The argv
// ---------------------------------------------------------------------------

fn env_of<'a>(pairs: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
    move |name| {
        pairs
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| OsString::from(v))
    }
}

const FULL_ENV: [(&str, &str); 7] = [
    ("PATH", "/usr/bin:/bin"),
    ("HOME", "/home/x"),
    ("USER", "x"),
    ("LANG", "C"),
    ("TERM", "dumb"),
    ("TMPDIR", "/tmp"),
    ("CLAUDE_CONFIG_DIR", "/cfg"),
];

fn call(kind: StageKind, model: &str, cwd: &Path, out: &Path) -> StageCall {
    StageCall {
        kind,
        model: model.to_string(),
        prompt: "stage: test\nreview this\n".to_string(),
        cwd: cwd.to_path_buf(),
        settings: PathBuf::from("/state/pr-7/stage-settings.json"),
        out: out.to_path_buf(),
        per_call_usd: 0.15,
        timeout: Duration::from_secs(30),
    }
}

fn argv_golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/loop/argv")
}

fn argv_text(kind: StageKind, model: &str) -> String {
    let cli = ClaudeCli::with_env("claude", env_of(&FULL_ENV)).expect("a runner");
    let argv = cli.argv(&call(
        kind,
        model,
        Path::new("/cwd"),
        Path::new("/out/round-01-x.json"),
    ));
    let mut text = String::new();
    for word in argv {
        text.push_str(&word.to_string_lossy());
        text.push('\n');
    }
    text
}

#[test]
fn lens_argv_matches_the_golden() {
    let text = argv_text(StageKind::Lens, "sonnet");
    if let Some(diff) = check_golden_in(&argv_golden_dir(), "lens.txt", text.as_bytes()) {
        panic!("{diff}");
    }
}

#[test]
fn verify_argv_matches_the_golden() {
    let text = argv_text(StageKind::Verify, "opus");
    if let Some(diff) = check_golden_in(&argv_golden_dir(), "verify.txt", text.as_bytes()) {
        panic!("{diff}");
    }
}

// ---------------------------------------------------------------------------
// A fake claude
// ---------------------------------------------------------------------------

/// Write an executable `#!/bin/sh` script `claude` into a fresh directory.
fn fake_claude(body: &str) -> (PathBuf, PathBuf) {
    let dir = unique_dir("loop-claude");
    let script = dir.join("claude");
    std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).expect("write the fake claude");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    (dir, script)
}

fn run_with(
    script: &Path,
    kind: StageKind,
    timeout: Duration,
    dir: &Path,
) -> (StageResult, PathBuf) {
    let cli = ClaudeCli::with_env(script, env_of(&FULL_ENV)).expect("a runner");
    let out = dir.join("run/round-01-x.json");
    let mut stage = call(kind, "sonnet", dir, &out);
    stage.timeout = timeout;
    (cli.run(&stage), out)
}

#[test]
fn env_is_exactly_the_allowlist() {
    // Variables that must not reach the child, set on this test process.
    std::env::set_var("AIOS_TEAM", "team-build");
    std::env::set_var("CLAUDECODE", "1");
    std::env::set_var("SSH_AUTH_SOCK", "/tmp/agent.sock");
    let (dir, script) = fake_claude(
        r#"env | sort > "$(dirname "$0")/env.out"
cat >/dev/null
printf '%s' '{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.01,"structured_output":{"findings":[]}}'"#,
    );
    let (result, _) = run_with(&script, StageKind::Lens, Duration::from_secs(30), &dir);
    assert!(matches!(result, StageResult::Ok { .. }), "{result:?}");
    let seen = std::fs::read_to_string(dir.join("env.out")).expect("the fake's env dump");
    // The shell itself adds PWD, SHLVL, _ and OLDPWD on some systems.
    let mut names: Vec<&str> = seen
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, _)| k))
        .filter(|k| !["PWD", "SHLVL", "_", "OLDPWD"].contains(k))
        .collect();
    names.sort_unstable();
    let mut want = ENV_ALLOWLIST.to_vec();
    want.sort_unstable();
    assert_eq!(names, want, "\n{seen}");
    assert!(seen.contains("CLAUDE_CONFIG_DIR=/cfg"), "{seen}");
}

#[test]
fn unset_claude_config_dir_refuses() {
    for pairs in [
        vec![("PATH", "/usr/bin")],
        vec![("PATH", "/usr/bin"), ("CLAUDE_CONFIG_DIR", "")],
    ] {
        let err = ClaudeCli::with_env("claude", env_of(&pairs)).expect_err("refuses");
        assert!(
            format!("{err:#}").contains("CLAUDE_CONFIG_DIR unset; refusing the default config"),
            "{err:#}"
        );
    }
}

#[test]
fn run_keeps_stdout_prompt_and_stderr_and_feeds_stdin() {
    let (dir, script) = fake_claude(
        r#"cat > "$(dirname "$0")/stdin.out"
echo oops >&2
printf '%s' '{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.02,"structured_output":{"verdicts":[]}}'"#,
    );
    let (result, out) = run_with(&script, StageKind::Verify, Duration::from_secs(30), &dir);
    assert!(
        matches!(result, StageResult::Ok { cost, .. } if (cost - 0.02).abs() < 1e-9),
        "{result:?}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("stdin.out")).expect("stdin"),
        "stage: test\nreview this\n"
    );
    assert!(std::fs::read_to_string(&out)
        .expect("stdout kept")
        .contains("verdicts"));
    assert_eq!(
        std::fs::read_to_string(out.with_extension("err")).expect("stderr kept"),
        "oops\n"
    );
    assert!(out.with_extension("prompt.md").exists());
}

#[test]
fn empty_output_and_failures_map_without_a_panic() {
    let (dir, script) = fake_claude("cat >/dev/null");
    assert_eq!(
        run_with(&script, StageKind::Lens, Duration::from_secs(30), &dir).0,
        StageResult::Empty
    );
    let (dir, script) = fake_claude("cat >/dev/null\necho boom >&2\nexit 7");
    match run_with(&script, StageKind::Lens, Duration::from_secs(30), &dir).0 {
        StageResult::Error(line) => {
            assert!(line.contains("exited 7") && line.contains("boom"), "{line}")
        }
        other => panic!("{other:?}"),
    }
    // The program cannot start.
    let dir = unique_dir("loop-claude-missing");
    let (result, _) = run_with(
        &dir.join("no-such-claude"),
        StageKind::Lens,
        Duration::from_secs(30),
        &dir,
    );
    assert!(matches!(result, StageResult::Error(_)), "{result:?}");
}

#[test]
fn timeout_gives_timeout() {
    let (dir, script) = fake_claude("cat >/dev/null\nsleep 30");
    let started = std::time::Instant::now();
    let (result, _) = run_with(&script, StageKind::Lens, Duration::from_secs(1), &dir);
    assert_eq!(result, StageResult::Timeout);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the group was stopped"
    );
}

// ---------------------------------------------------------------------------
// The stage settings
// ---------------------------------------------------------------------------

/// A repository whose `origin/main` holds `settings`.
fn repo_with_origin_settings(settings: &Value) -> TestRepo {
    let repo = TestRepo::with_files(
        "loop-settings",
        &[(
            ".claude/settings.json",
            &serde_json::to_string_pretty(settings).expect("encode"),
        )],
    );
    git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    repo
}

fn guard_entry() -> Value {
    json!({"matcher": "Bash|Monitor|Agent|EnterWorktree", "hooks": [
        {"type": "command", "timeout": 20,
         "command": "/usr/bin/python3 \"$CLAUDE_PROJECT_DIR\"/.claude/hooks/git-push-guard.py"}
    ]})
}

fn busy_settings() -> Value {
    json!({
        "effortLevel": "high",
        "env": {"RUSTUP_AUTO_INSTALL": "0", "SECRET": "x"},
        "enabledPlugins": {"a@m": true, "b@m": false},
        "extraKnownMarketplaces": {"m": {"source": "x"}},
        "hooks": {
            "PreToolUse": [
                guard_entry(),
                {"matcher": "Edit", "hooks": [{"type": "command", "command": "other-hook"}]},
            ],
            "SessionStart": [{"hooks": [{"type": "command", "command": "start"}]}],
        },
        "permissions": {
            "allow": ["Bash(just *)", "Edit(**)"],
            "ask": ["Bash(git push*)"],
            "deny": ["Bash(rm -rf *)"],
        },
    })
}

#[test]
fn stage_settings_have_exactly_the_expected_keys() {
    let repo = repo_with_origin_settings(&busy_settings());
    let state = State::at(unique_dir("loop-settings-state").join("state"));
    let path = materialise(repo.path(), &state, 7).expect("materialise");
    assert_eq!(path, state.root().join("pr-7/stage-settings.json"));
    let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let got: Value = serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("JSON");
    let main = repo.path().to_str().expect("UTF-8");
    assert_eq!(
        got,
        json!({
            "enabledPlugins": {"a@m": false, "b@m": false},
            "hooks": {"PreToolUse": [{
                "matcher": "Bash|Monitor|Agent|EnterWorktree",
                "hooks": [{
                    "type": "command",
                    "command": format!("/usr/bin/python3 \"{main}/.claude/hooks/git-push-guard.py\""),
                }],
            }]},
            "permissions": {
                "allow": [],
                "ask": ["Bash(git push*)"],
                "deny": ["Bash(rm -rf *)"],
            },
        })
    );
    // No temporary file is left next to it.
    let left: Vec<_> = std::fs::read_dir(path.parent().expect("a parent"))
        .expect("read_dir")
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    assert_eq!(left.len(), 1, "{left:?}");
}

#[test]
fn the_settings_come_from_origin_main_not_the_working_tree() {
    let repo = repo_with_origin_settings(&busy_settings());
    // A working-tree change the PR could make.
    repo.write(
        ".claude/settings.json",
        "{\"permissions\": {\"allow\": [\"Bash(*)\"]}}",
    );
    let state = State::at(unique_dir("loop-settings-wt").join("state"));
    let path = materialise(repo.path(), &state, 7).expect("materialise");
    let got: Value = serde_json::from_slice(&std::fs::read(path).expect("read")).expect("JSON");
    assert_eq!(got["permissions"]["allow"], json!([]));
    assert_eq!(got["permissions"]["deny"], json!(["Bash(rm -rf *)"]));
}

#[test]
fn no_guard_entry_is_an_error() {
    let mut settings = busy_settings();
    settings["hooks"]["PreToolUse"] = json!([
        {"matcher": "Edit", "hooks": [{"type": "command", "command": "other-hook"}]}
    ]);
    let repo = repo_with_origin_settings(&settings);
    let state = State::at(unique_dir("loop-settings-noguard").join("state"));
    let err = materialise(repo.path(), &state, 7).expect_err("no guard");
    assert!(format!("{err:#}").contains("git-push-guard.py"), "{err:#}");
    assert!(
        !state.stage_settings_file(7).exists(),
        "nothing was written"
    );
    // Missing hooks altogether, missing plugins and lists: the pure builder.
    assert!(build(&json!({}), Path::new("/m")).is_err());
    let minimal = json!({"hooks": {"PreToolUse": [guard_entry()]}});
    let built = build(&minimal, Path::new("/m")).expect("a minimal blob builds");
    assert_eq!(built["enabledPlugins"], json!({}));
    assert_eq!(
        built["permissions"],
        json!({"allow": [], "ask": [], "deny": []})
    );
}

#[test]
fn a_main_path_that_cannot_be_quoted_is_an_error() {
    let minimal = json!({"hooks": {"PreToolUse": [guard_entry()]}});
    assert!(build(&minimal, Path::new("/m\"x")).is_err());
}
