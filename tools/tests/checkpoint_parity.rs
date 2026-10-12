//! `aios checkpoint` parity with the former script of `/justin:pause` (R3, Tasks 2 and 3).
//!
//! The oracle is the script's blob at `agent::CHECKPOINT_ORACLE` (#166), read from git
//! history and run as `<bash> <script>` in a twin repository under the PATH jail. The
//! 52 scenarios of [`CASES`] each describe one repository state and one argv. The
//! goldens in `tests/golden/checkpoint/<name>.golden` were recorded from the oracle on
//! macOS (`AIOS_BLESS_GOLDENS=1 cargo test -p aios-tools --test checkpoint_parity --
//! --ignored record_checkpoint_goldens`), through `agent::oracle_renames()` (the
//! documented D1 and D5 renames) and `agent::normalize()`. 49 scenarios are
//! differential. `cli_help`, `scan_git_failure_fails_closed` and
//! `scan_git_failure_on_clean_tree` (`agent::DIVERGENT_SCENARIOS`) are aios-only: their
//! goldens were blessed from the port, one scenario at a time, and reviewed by hand.
//!
//! The tests:
//!
//! - one `#[test]` per scenario, named after it: the aios binary against its golden;
//! - `oracle_reproduces_checkpoint_goldens`: the oracle against the 49 goldens;
//! - `checkpoint_differential_against_oracle` and, where `/bin/bash` is 3.2 (macOS),
//!   `checkpoint_differential_under_bin_bash_3_2`: the oracle and aios in twin
//!   repositories over the 49 differential scenarios, comparing everything a golden
//!   holds plus the HEAD sha and its author and committer.
//!
//! A golden holds these sections, in order: `exit`, `stdout`, `stderr`, `origin refs`
//! (`git for-each-ref` of the bare origin), `HEAD message`, `index` (`git write-tree`
//! before and after the run), `upstream` (`branch.<b>.merge`) and `gh calls` (the
//! rendered argv, the cwd and the environment of every fake-gh call). It records the
//! scan's `<path>\t<reason>` lines through stdout and never the matched text.
//!
//! Every scenario also runs `assert_not_vacuous`, `assert_origin_invariants` and the
//! fake gh's `CHECKPOINT_GH_ALLOW` allowlist (the one shared checks, [`check_outcome`]).
//! Test data builds token-shaped strings from fragments (contract §10).

mod common;

use common::agent::{self, Outcome, Scenario, Sub, Twin, DIVERGENT_SCENARIOS};
use common::{isolated, parallel_map};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// One scenario: its name (also the golden's basename), the argv after `checkpoint`,
/// how to build the repository, and an optional extra assertion on the run.
struct Case {
    name: &'static str,
    args: &'static [&'static str],
    build: fn(&str) -> Scenario,
    check: Option<fn(&Outcome)>,
}

/// The scenario table. Each `scenario!(<name>, [<args>], <builder>[, <check>])` at
/// column 0 declares one entry (the builder takes the scenario's name) and one
/// `#[test]` of the same name, which runs aios on it against its golden.
macro_rules! scenarios {
    (@check) => {
        None
    };
    (@check $check:path) => {
        Some($check)
    };
    ($(scenario!($name:ident, [$($arg:expr),* $(,)?], $build:expr $(, $check:path)? $(,)?);)*) => {
        static CASES: &[Case] = &[$(Case {
            name: stringify!($name),
            args: &[$($arg),*],
            build: $build,
            check: scenarios!(@check $($check)?),
        }),*];

        $(
            #[test]
            fn $name() {
                aios_matches_golden(stringify!($name));
            }
        )*
    };
}

/// A `claude/x` branch checked out from `main`.
fn on_x(name: &str) -> Scenario {
    Scenario::checkpoint(name).branch("claude/x")
}

/// The wip hold's `gh pr list --head` call, answered with no ready PR (a draft PR is
/// filtered out by the call's own `--jq`, so the answer is the same empty output).
fn no_ready_pr(scenario: Scenario) -> Scenario {
    scenario.gh_fixture("pr-list-head", "").expects_gh(true)
}

/// A private-key header, built from fragments: a literal one would stop every
/// `/justin:pause` on this branch (contract §10).
fn pem_header() -> String {
    format!("{}{}", "-----BEGIN RSA", " PRIVATE KEY-----")
}

/// A `git` wrapper in front of the jail's: the call whose space-joined argv matches the
/// shell `case` pattern prints one stderr line and exits 128; every other call runs the
/// real git, which the wrapper finds through `agent::REAL_BIN_ENV`
/// (`AIOS_TEST_REAL_BIN`, the jail's `bin` directory).
fn git_fails(scenario: Scenario, pattern: &str) -> Scenario {
    let script = format!(
        "#!/bin/sh\n\
         # Fails the git call matching the case pattern; every other call runs the real git.\n\
         case \" $* \" in\n\
         {pattern})\n\
         \x20   echo \"fatal: injected git failure\" >&2\n\
         \x20   exit 128\n\
         \x20   ;;\n\
         esac\n\
         exec \"${}/git\" \"$@\"\n",
        agent::REAL_BIN_ENV
    );
    scenario.fake_tool("git", &script)
}

/// `git <args>` at the root in setup, tolerating a non-zero exit: a conflicted merge,
/// cherry-pick or revert leaves the state a scenario wants but exits 1. A throwaway
/// shell alias wraps the command, because setup steps must succeed.
fn conflicting(scenario: Scenario, command: &str) -> Scenario {
    scenario.git(&[
        "-c",
        &format!("alias.conflicted=!git {command} || true"),
        "conflicted",
    ])
}

/// A `main` with `f.txt` (pushed), then `claude/y` and `claude/x` each changing it, on
/// `claude/x`: the base of every conflicting operation.
fn diverged_on_x(name: &str) -> Scenario {
    Scenario::checkpoint(name)
        .file("f.txt", "base\n")
        .commit("add f")
        .git(&["push", "-q", "origin", "main"])
        .branch("claude/y")
        .file("f.txt", "y\n")
        .commit("y changes f")
        .git(&["checkout", "-q", "main"])
        .branch("claude/x")
        .file("f.txt", "x\n")
        .commit("x changes f")
}

/// The run stopped on a possible secret: exit 3, and neither the index nor HEAD moved.
fn stopped_untouched(outcome: &Outcome) {
    assert_eq!(outcome.code, 3, "stdout:\n{}", outcome.stdout_str());
    let state = &outcome.repo_state;
    assert!(state.index_tree_before.is_some());
    assert_eq!(
        state.index_tree_before, state.index_tree_after,
        "the stop must restore the index exactly"
    );
    assert_eq!(state.head_before, state.head_after, "nothing was committed");
}

/// The staged and unstaged halves of the same file, a staged new file and a flagged
/// untracked one all survive a stop.
fn index_restored(outcome: &Outcome) {
    stopped_untouched(outcome);
    let status = &outcome.repo_state.status_after;
    for line in ["MM a.txt", "A  b.txt", "?? server.key"] {
        assert!(
            status.lines().any(|l| l == line),
            "no {line:?} in:\n{status}"
        );
    }
}

/// D9: a git failure inside the secret scan fails closed. The exit is 1 and stdout never
/// says `Safe to /clear`, whatever else is clean; the index is as it was, nothing was
/// committed, and origin did not move (not even for an unpushed non-wip commit). These
/// asserts stay in Rust, so a re-bless of the golden cannot weaken them.
fn failed_closed(outcome: &Outcome) {
    let stdout = outcome.stdout_str();
    assert_eq!(outcome.code, 1, "stdout:\n{stdout}");
    assert!(
        !stdout.lines().any(|l| l.starts_with("Safe to /clear")),
        "a failed scan must never say Safe to /clear:\n{stdout}"
    );
    let first = stdout.lines().next().unwrap_or_default();
    assert!(
        first.contains(
            "| wip: not committed (secret scan failed: fatal: injected git failure) \
             | pushed: no (secret scan failed) |"
        ),
        "the first line:\n{first}"
    );
    let state = &outcome.repo_state;
    assert!(state.index_tree_before.is_some());
    assert_eq!(
        state.index_tree_before, state.index_tree_after,
        "the index is as it was"
    );
    assert_eq!(state.head_before, state.head_after, "nothing was committed");
    assert_eq!(
        state.origin_refs_before, state.origin_refs_after,
        "nothing was pushed"
    );
    assert!(
        stdout.contains("/clear keeps all of it on disk; run /justin:start after."),
        "{stdout}"
    );
}

/// Run from `docs/`, the commit holds the file at the root as well as the one in
/// `docs/`, and nothing is left dirty.
fn subdirectory_commit(outcome: &Outcome) {
    let listed = outcome
        .twin()
        .git(&["ls-tree", "-r", "--name-only", "HEAD"]);
    for path in ["notes-root.txt", "docs/notes-sub.txt"] {
        assert!(
            listed.lines().any(|l| l == path),
            "HEAD lacks {path}:\n{listed}"
        );
    }
    assert_eq!(outcome.repo_state.status_after, "");
}

scenarios! {
// The 15 scenarios of #166.
scenario!(sensitive_name_new_doc, [],
    |n| on_x(n).file("docs/deploy-secret.md", "how to deploy\n"),
    stopped_untouched);
scenario!(pem_line_in_added_text, [],
    |n| on_x(n).file("notes/deploy-notes.txt", format!("{}\n", pem_header())),
    stopped_untouched);
scenario!(key_file_in_earlier_local_commit, [],
    |n| on_x(n)
        .file("certs/server.pem", "placeholder\n")
        .commit("add certs")
        .file("notes.txt", "work in progress\n"),
    stopped_untouched);
scenario!(allow_matches_flagged_path, ["--allow", "docs/deploy-secret.md"],
    |n| no_ready_pr(on_x(n).file("docs/deploy-secret.md", "how to deploy\n")));
scenario!(allow_matches_nothing, ["--allow", "docs/missing.md"],
    |n| no_ready_pr(on_x(n).file("notes.txt", "work in progress\n")));
scenario!(allow_dot_slash_prefix, ["--allow", "./docs/deploy-secret.md"],
    |n| no_ready_pr(on_x(n).file("docs/deploy-secret.md", "how to deploy\n")));
scenario!(other_worktree_dirty_from_claude_worktree, [],
    |n| Scenario::checkpoint(n)
        .worktree(".claude/worktrees/w1", "claude/w1")
        .worktree(".claude/worktrees/w2", "claude/w2")
        .file(".claude/worktrees/w2/dirty.txt", "dirty\n")
        .run_in_worktree(".claude/worktrees/w1"));
scenario!(other_worktree_dirty_from_main, [],
    |n| on_x(n)
        .worktree(".claude/worktrees/w1", "claude/w1")
        .file(".claude/worktrees/w1/work.txt", "work\n")
        .commit_in(".claude/worktrees/w1", "w1 work")
        .file(".claude/worktrees/w1/dirty.txt", "dirty\n"));
scenario!(gh_fails_holds_wip, [],
    |n| on_x(n)
        .file("notes.txt", "work in progress\n")
        .gh_fixture_with("pr-list-head", "", "gh: simulated outage\n", 1)
        .expects_gh(true));
scenario!(gh_missing_holds_wip, [],
    |n| on_x(n).file("notes.txt", "work in progress\n").gh_missing());
scenario!(ready_pr_holds_wip, [],
    |n| on_x(n)
        .file("notes.txt", "work in progress\n")
        .gh_fixture("pr-list-head", "#12\n")
        .expects_gh(true));
scenario!(draft_pr_pushes_wip, [],
    |n| no_ready_pr(on_x(n).file("notes.txt", "work in progress\n")));
scenario!(non_wip_commits_push_without_gh, [],
    |n| on_x(n).file("feature.txt", "a feature\n").commit("add feature"));
scenario!(merge_in_progress, [],
    |n| Scenario::checkpoint(n)
        .branch("claude/y")
        .file("y.txt", "y\n")
        .commit("y work")
        .git(&["checkout", "-q", "main"])
        .branch("claude/x")
        .file("x.txt", "x\n")
        .commit("x work")
        .git(&["merge", "-q", "--no-commit", "--no-ff", "claude/y"]));
scenario!(detached_head, [],
    |n| Scenario::checkpoint(n)
        .git(&["checkout", "-q", "--detach"])
        .file("notes.txt", "work in progress\n"));

// #166's five round-1 extras.
scenario!(env_in_new_untracked_dir, [],
    |n| on_x(n).file("config/.env", "MODE=dev\n"),
    stopped_untouched);
scenario!(pem_in_keys_dir, [],
    |n| on_x(n).file("keys/server.pem", "placeholder\n"),
    stopped_untouched);
scenario!(modified_tracked_creds_not_flagged, [],
    |n| no_ready_pr(
        Scenario::checkpoint(n)
            .file("config/credentials.yml", "user: a\n")
            .commit("add config")
            .git(&["push", "-q", "origin", "main"])
            .branch("claude/x")
            .file("config/credentials.yml", "user: b\n")));
scenario!(clean_tree_unpushed_pushes, [],
    |n| no_ready_pr(
        on_x(n)
            .file("a.txt", "a\n")
            .commit("wip: checkpoint claude/x")));
scenario!(real_conflict_merge, [],
    |n| conflicting(diverged_on_x(n), "merge -q claude/y"));

// The 24 gaps.
scenario!(commit_hook_fails, [],
    |n| on_x(n).file("notes.txt", "work in progress\n").file_mode(
        ".git/hooks/pre-commit",
        "#!/bin/sh\necho 'pre-commit: refused' >&2\nexit 1\n",
        0o755,
    ));
scenario!(push_rejected_non_ff, [],
    |n| no_ready_pr(
        on_x(n)
            .file("a.txt", "one\n")
            .commit("one")
            .git(&["push", "-q", "origin", "claude/x"])
            .git(&["reset", "-q", "--hard", "main"])
            .file("b.txt", "two\n")
            .commit("two")
            .file("notes.txt", "work in progress\n")));
scenario!(non_claude_branch, [],
    |n| Scenario::checkpoint(n)
        .branch("feature/x")
        .file("notes.txt", "work in progress\n"));
scenario!(on_main_dirty, [],
    |n| Scenario::checkpoint(n).file("notes.txt", "work in progress\n"));
// A rebase leaves HEAD detached, so the script's "rebase in progress" is only reachable
// with the state directory beside an attached HEAD: the scenario makes that directory.
scenario!(rebase_in_progress, [],
    |n| on_x(n)
        .file("notes.txt", "work in progress\n")
        .file(".git/rebase-merge/git-rebase-todo", "")
        .file(".git/rebase-merge/done", ""));
scenario!(cherry_pick_in_progress, [],
    |n| conflicting(diverged_on_x(n), "cherry-pick claude/y"));
scenario!(revert_in_progress, [],
    |n| conflicting(
        Scenario::checkpoint(n)
            .file("f.txt", "base\n")
            .commit("add f")
            .git(&["push", "-q", "origin", "main"])
            .branch("claude/x")
            .file("f.txt", "one\n")
            .commit("f is one")
            .file("f.txt", "two\n")
            .commit("f is two"),
        "revert --no-edit HEAD~1"));
scenario!(bisect_in_progress, [],
    |n| on_x(n)
        .file("notes.txt", "work in progress\n")
        .git(&["bisect", "start"]));
scenario!(unmerged_paths, [],
    |n| conflicting(diverged_on_x(n), "merge -q claude/y").remove(".git/MERGE_HEAD"));
scenario!(remember_and_pycache_excluded, [],
    |n| no_ready_pr(
        Scenario::checkpoint(n)
            .remove(".gitignore")
            .commit("drop the gitignore")
            .git(&["push", "-q", "origin", "main"])
            .branch("claude/x")
            .file(".remember/handoff-x.md", "handoff\n")
            .file("tools/__pycache__/m.cpython-313.pyc", "bytecode\n")
            .file("docs/real.txt", "real work\n")));
scenario!(index_restored_exactly, [],
    |n| Scenario::checkpoint(n)
        .file("a.txt", "v1\n")
        .commit("add a")
        .git(&["push", "-q", "origin", "main"])
        .branch("claude/x")
        .file("a.txt", "v2\n")
        .git(&["add", "a.txt"])
        .file("a.txt", "v3\n")
        .file("b.txt", "new\n")
        .git(&["add", "b.txt"])
        .file("server.key", "placeholder\n"),
    index_restored);
scenario!(quoted_non_ascii_paths, [],
    |n| on_x(n).file("données/secret-é.txt", "notes\n"),
    stopped_untouched);
scenario!(missing_other_worktree_dir, [],
    |n| on_x(n)
        .worktree(".claude/worktrees/w1", "claude/w1")
        .worktree(".claude/worktrees/w2", "claude/w2")
        .file(".claude/worktrees/w2/dirty.txt", "dirty\n")
        .git(&["-c", "alias.zap=!rm -rf .claude/worktrees/w1", "zap"]));
scenario!(allow_override_paragraph,
    ["--allow", "docs/other-secret.md", "--allow", "./docs/api-secret.md"],
    |n| no_ready_pr(
        on_x(n)
            .file("docs/api-secret.md", "api\n")
            .file("docs/other-secret.md", "other\n")));
scenario!(nothing_to_do_safe_to_clear, [], |n| on_x(n));
scenario!(handoff_saved, ["--handoff", "saved"], |n| on_x(n));
scenario!(handoff_not_saved, ["--handoff", "not-saved"], |n| on_x(n));
scenario!(tracking_origin_main_pushes_branch_only, [],
    |n| no_ready_pr(
        Scenario::checkpoint(n)
            .git(&[
                "worktree", "add", "-q", "--track", "-b", "claude/x",
                ".claude/worktrees/w1", "origin/main",
            ])
            .file(".claude/worktrees/w1/notes.txt", "work in progress\n")
            .run_in_worktree(".claude/worktrees/w1")));
scenario!(outside_repo, [], |n| Scenario::outside_repo(Sub::Checkpoint, n));
scenario!(write_tree_fails, [],
    |n| git_fails(on_x(n).file("notes.txt", "work in progress\n"), "*\" write-tree \"*"));
// An orphan branch with an empty index: HEAD does not resolve, and the scan's `log`
// calls (which the port must skip) have nothing to read.
scenario!(unborn_head, [],
    |n| Scenario::checkpoint(n)
        .git(&["checkout", "-q", "--orphan", "claude/x"])
        .git(&["rm", "-rf", "-q", "."]));
scenario!(allow_duplicates_kept_in_unused,
    ["--allow", "nowhere/a.txt", "--allow", "./nowhere/a.txt"],
    |n| no_ready_pr(on_x(n).file("notes.txt", "work in progress\n")));
scenario!(allow_on_blocked_branch_all_unused, ["--allow", "notes.txt"],
    |n| Scenario::checkpoint(n).file("notes.txt", "work in progress\n"));
scenario!(run_from_subdirectory, [],
    |n| no_ready_pr(
        on_x(n)
            .file("notes-root.txt", "at the root\n")
            .file("docs/notes-sub.txt", "in docs\n")
            .run_cwd("docs")),
    subdirectory_commit);

// The six CLI scenarios (D1: messages say `checkpoint:`; `cli_help` is aios-only).
scenario!(cli_allow_empty, ["--allow", ""], |n| Scenario::checkpoint(n));
scenario!(cli_allow_dash, ["--allow", "-x"], |n| Scenario::checkpoint(n));
scenario!(cli_handoff_bad, ["--handoff", "maybe"], |n| Scenario::checkpoint(n));
scenario!(cli_handoff_twice_last_wins, ["--handoff", "saved", "--handoff", "not-saved"],
    |n| on_x(n));
scenario!(cli_unknown, ["--bogus"], |n| Scenario::checkpoint(n));
scenario!(cli_help, ["--help"], |n| Scenario::checkpoint(n));

// The two D9 scenarios (aios-only): a git failure inside the secret scan fails closed.
scenario!(scan_git_failure_fails_closed, [],
    |n| git_fails(
        on_x(n)
            .file("feature.txt", "a feature\n")
            .commit("add feature")
            .file("notes.txt", "work in progress\n"),
        "*\" log -p \"*"),
    failed_closed);
scenario!(scan_git_failure_on_clean_tree, [],
    |n| git_fails(on_x(n), "*\" diff --cached --name-only --no-renames \"*"),
    failed_closed);
}

/// Whether the scenario's golden is aios-only (no oracle run records it).
fn is_divergent(name: &str) -> bool {
    DIVERGENT_SCENARIOS
        .iter()
        .any(|(divergent, _)| *divergent == name)
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/checkpoint")
}

/// `git <args>` in `dir`, for what a golden records about the repository after the
/// run. `None` when git fails (no repository, an unborn HEAD). The ceiling keeps a run
/// directory outside every checkout from finding one above it.
fn probe(dir: &Path, ceiling: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("git");
    isolated(&mut cmd)
        .env("GIT_CEILING_DIRECTORIES", ceiling)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null());
    let out = cmd.output().expect("run git");
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .trim_end_matches('\n')
            .to_string()
    })
}

fn section(out: &mut Vec<u8>, title: &str, body: &[u8]) {
    out.extend_from_slice(format!("== {title} ==\n").as_bytes());
    out.extend_from_slice(body);
    if !body.is_empty() && !body.ends_with(b"\n") {
        out.extend_from_slice(b"\n\\ no newline at end of output\n");
    }
}

fn tree_line(tree: &Option<String>) -> &str {
    tree.as_deref().unwrap_or("none")
}

/// The golden text of one run. An oracle run goes through `agent::oracle_renames()`
/// (the documented D1 and D5 renames); an aios run is recorded as it is.
fn render_golden(outcome: &Outcome, oracle: bool) -> Vec<u8> {
    let rename = |text: &[u8]| {
        if oracle {
            agent::oracle_renames(text, outcome)
        } else {
            text.to_vec()
        }
    };
    let state = &outcome.repo_state;
    let twin = outcome.twin();
    let ceiling = twin.root().parent().expect("the twin root has a parent");
    let git = |args: &[&str]| probe(twin.run_dir(), ceiling, args);
    let head_message = git(&["log", "-1", "--format=%B"]);
    let upstream = git(&["symbolic-ref", "--short", "-q", "HEAD"])
        .and_then(|branch| git(&["config", "--get", &format!("branch.{branch}.merge")]));

    let mut origin_refs = String::new();
    for (name, sha) in &state.origin_refs_after {
        origin_refs.push_str(&format!("{name} {sha}\n"));
    }
    let mut gh_calls = String::new();
    for (i, call) in outcome.gh_calls.iter().enumerate() {
        let env: Vec<String> = call
            .env
            .iter()
            .map(|(k, v)| format!("{k}={}", v.as_deref().unwrap_or("<unset>")))
            .collect();
        gh_calls.push_str(&format!(
            "{}: {}\n   cwd {}\n   env {}\n",
            i + 1,
            call.line().trim_end(),
            call.cwd.display(),
            env.join(" ")
        ));
    }
    if gh_calls.is_empty() {
        gh_calls.push_str("(none)\n");
    }

    let mut golden = Vec::new();
    section(
        &mut golden,
        "exit",
        format!("{}\n", outcome.code).as_bytes(),
    );
    section(&mut golden, "stdout", &rename(&outcome.stdout));
    section(&mut golden, "stderr", &rename(&outcome.stderr));
    section(&mut golden, "origin refs", origin_refs.as_bytes());
    section(
        &mut golden,
        "HEAD message",
        format!("{}\n", head_message.as_deref().unwrap_or("(none)")).as_bytes(),
    );
    section(
        &mut golden,
        "index",
        format!(
            "before {}\nafter {}\n",
            tree_line(&state.index_tree_before),
            tree_line(&state.index_tree_after)
        )
        .as_bytes(),
    );
    section(
        &mut golden,
        "upstream",
        format!("{}\n", upstream.as_deref().unwrap_or("(none)")).as_bytes(),
    );
    section(&mut golden, "gh calls", gh_calls.as_bytes());
    agent::normalize(&golden, outcome)
}

/// The invariants every scenario shares, and the scenario's own check.
fn check_outcome(case: &Case, outcome: &Outcome) {
    agent::assert_not_vacuous(outcome);
    agent::assert_origin_invariants(outcome);
    if let Some(gh) = outcome.twin().fake_gh() {
        gh.assert_calls_match(agent::CHECKPOINT_GH_ALLOW);
    }
    if let Some(check) = case.check {
        check(outcome);
    }
}

/// Run `case`'s oracle in its twin and apply the invariants every scenario shares.
fn run_case(case: &Case) -> Outcome {
    let scenario = (case.build)(case.name);
    let outcome = agent::run_oracle(&scenario, case.args);
    check_outcome(case, &outcome);
    outcome
}

/// The cases whose golden an oracle run records.
fn differential_cases() -> Vec<&'static Case> {
    CASES
        .iter()
        .filter(|case| !is_divergent(case.name))
        .collect()
}

fn first_difference(want: &str, got: &str) -> String {
    let want: Vec<&str> = want.split('\n').collect();
    let got: Vec<&str> = got.split('\n').collect();
    for i in 0..want.len().max(got.len()) {
        let (w, g) = (want.get(i).copied(), got.get(i).copied());
        if w != g {
            return format!(
                "line {}: expected {:?}, got {:?}",
                i + 1,
                w.unwrap_or("<end of output>"),
                g.unwrap_or("<end of output>")
            );
        }
    }
    "the goldens differ".to_string()
}

fn assert_table_is_sound() {
    assert_eq!(CASES.len(), 52, "the checkpoint scenario table");
    let mut names: Vec<&str> = CASES.iter().map(|case| case.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), CASES.len(), "scenario names are unique");
    for (divergent, _) in DIVERGENT_SCENARIOS {
        let in_checkpoint = [
            "cli_help",
            "scan_git_failure_fails_closed",
            "scan_git_failure_on_clean_tree",
        ]
        .contains(divergent);
        assert_eq!(
            in_checkpoint,
            CASES.iter().any(|case| case.name == *divergent),
            "{divergent}: DIVERGENT_SCENARIOS and the table disagree"
        );
    }
}

/// Every differential scenario's oracle output equals its golden. Runs on every host:
/// the goldens were recorded on macOS, and CI's ubuntu-26.04 reproduces them with GNU
/// tools (a scenario that cannot goes in `agent::PLATFORM_SCENARIOS`).
#[test]
fn oracle_reproduces_checkpoint_goldens() {
    assert_table_is_sound();
    let dir = golden_dir();
    let cases = differential_cases();
    assert_eq!(cases.len(), 49, "the non-divergent scenarios");

    let known: Vec<String> = CASES
        .iter()
        .map(|case| format!("{}.golden", case.name))
        .collect();
    for entry in std::fs::read_dir(&dir).expect("read the golden directory") {
        let file = entry
            .expect("a directory entry")
            .file_name()
            .to_string_lossy()
            .into_owned();
        assert!(known.contains(&file), "{file} is a golden of no scenario");
    }

    let failures: Vec<String> = parallel_map(&cases, |case| {
        if !agent::oracle_runs_here(case.name) {
            return None;
        }
        let got = String::from_utf8_lossy(&render_golden(&run_case(case), true)).into_owned();
        let path = dir.join(format!("{}.golden", case.name));
        let want = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "{}: {e} (record it: AIOS_BLESS_GOLDENS=1 cargo test -p aios-tools --test \
                 checkpoint_parity -- --ignored record_checkpoint_goldens)",
                path.display()
            )
        });
        (want != got).then(|| format!("{}: {}", case.name, first_difference(&want, &got)))
    })
    .into_iter()
    .flatten()
    .collect();
    assert!(
        failures.is_empty(),
        "the oracle no longer reproduces these goldens:\n{}",
        failures.join("\n")
    );
}

fn case_named(name: &str) -> &'static Case {
    CASES
        .iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("no scenario named {name}"))
}

/// aios on scenario `name` against its golden. The goldens of the three aios-only
/// scenarios are blessed from this run (`AIOS_BLESS_GOLDENS=1 cargo test -p aios-tools
/// --test checkpoint_parity <name>`, one scenario at a time, reviewed by hand); the 49
/// differential goldens are recorded from the oracle only, so a bless run leaves them.
fn aios_matches_golden(name: &str) {
    let case = case_named(name);
    let scenario = (case.build)(case.name);
    let outcome = agent::run_aios(&scenario, "checkpoint", case.args);
    check_outcome(case, &outcome);
    let got = String::from_utf8_lossy(&render_golden(&outcome, false)).into_owned();
    let path = golden_dir().join(format!("{name}.golden"));
    if is_divergent(name) && common::soak::bless() {
        std::fs::create_dir_all(golden_dir()).expect("create the golden directory");
        std::fs::write(&path, &got).expect("write an aios-only golden");
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e} (an aios-only golden is blessed with AIOS_BLESS_GOLDENS=1 cargo test -p \
             aios-tools --test checkpoint_parity {name})",
            path.display()
        )
    });
    assert!(
        want == got,
        "{name}: aios no longer matches its golden: {}\n--- stdout\n{}\n--- stderr\n{}",
        first_difference(&want, &got),
        outcome.stdout_str(),
        outcome.stderr_str()
    );
}

/// Everything a golden holds, plus the HEAD sha and its author and committer (the
/// pinned `aios-test` identity of the `wip` commit).
fn observed(outcome: &Outcome, oracle: bool) -> String {
    let twin = outcome.twin();
    let ceiling = twin.root().parent().expect("the twin root has a parent");
    let head = probe(
        twin.run_dir(),
        ceiling,
        &["log", "-1", "--format=%H|%an <%ae>|%cn <%ce>"],
    );
    format!(
        "{}== HEAD ==\n{}\n",
        String::from_utf8_lossy(&render_golden(outcome, oracle)),
        head.as_deref().unwrap_or("(none)")
    )
}

/// The oracle (run by `bash`) and aios, in twin repositories, over the 49 differential
/// scenarios.
fn differential(bash: &Path) {
    assert_table_is_sound();
    let cases = differential_cases();
    assert_eq!(cases.len(), 49, "the non-divergent scenarios");
    let failures: Vec<String> = parallel_map(&cases, |case| {
        if !agent::oracle_runs_here(case.name) {
            return None;
        }
        let scenario = (case.build)(case.name);
        let oracle = agent::run_oracle_with(&scenario, Twin::Oracle, bash, case.args);
        check_outcome(case, &oracle);
        let aios = agent::run_aios(&scenario, "checkpoint", case.args);
        check_outcome(case, &aios);
        let (want, got) = (observed(&oracle, true), observed(&aios, false));
        (want != got).then(|| format!("{}: {}", case.name, first_difference(&want, &got)))
    })
    .into_iter()
    .flatten()
    .collect();
    assert!(
        failures.is_empty(),
        "aios differs from the oracle in these scenarios:\n{}",
        failures.join("\n")
    );
}

#[test]
fn checkpoint_differential_against_oracle() {
    differential(&agent::require_tools().bash);
}

/// The same with the oracle run by macOS's `/bin/bash` 3.2, which the script supports.
#[test]
fn checkpoint_differential_under_bin_bash_3_2() {
    let bash = Path::new("/bin/bash");
    let version = Command::new(bash).arg("--version").output();
    let is_3_2 = version.as_ref().is_ok_and(|out| {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .is_some_and(|line| line.contains("version 3.2"))
    });
    if !is_3_2 {
        println!("no /bin/bash 3.2 here");
        return;
    }
    differential(bash);
}

/// Record the 49 differential goldens from the oracle. Review the diff.
#[test]
#[ignore = "rewrites the goldens from the oracle: AIOS_BLESS_GOLDENS=1 ... -- --ignored record_checkpoint_goldens"]
fn record_checkpoint_goldens() {
    assert!(
        common::soak::bless(),
        "recording rewrites the goldens: set AIOS_BLESS_GOLDENS=1"
    );
    assert_table_is_sound();
    let dir = golden_dir();
    std::fs::create_dir_all(&dir).expect("create the golden directory");
    let cases = differential_cases();
    let goldens = parallel_map(&cases, |case| render_golden(&run_case(case), true));
    for (case, golden) in cases.iter().zip(goldens) {
        std::fs::write(dir.join(format!("{}.golden", case.name)), golden).expect("write a golden");
    }
}
