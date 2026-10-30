//! The loop's test fakes (R2a, Task 6a): the fake `claude` runs exactly the tools
//! the jail links for it, logs and serves what a scenario says, and
//! `assert_read_only` catches what it is there to catch.

mod common;

use std::collections::BTreeSet;
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use aios_tools::cmd::pr_loop::claude::{parse_envelope, StageResult};
use common::loop_fake::{
    assert_read_only, envelope, FakeClaude, Rig, Scenario, FAKE_CLAUDE, FAKE_CLAUDE_FUNCTIONS,
};
use common::{git, jail, unique_dir};
use serde_json::json;

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn the_fake_claude_runs_exactly_its_tool_list() {
    let not_commands = [jail::SH_WORDS, FAKE_CLAUDE_FUNCTIONS, &["sh"]].concat();
    let commands = jail::script_commands(FAKE_CLAUDE, &not_commands);
    assert_eq!(commands, set(&jail::FAKE_CLAUDE_TOOLS));
}

/// A fake root with a jail, for running the fake claude by hand.
struct Bench {
    root: PathBuf,
    claude: FakeClaude,
    jail_bin: PathBuf,
}

impl Bench {
    fn new(label: &str) -> Bench {
        let root = unique_dir(label);
        let claude = FakeClaude::install(&root);
        let jail_bin = jail::bin(&root.join("jail"), &[]);
        Bench {
            root,
            claude,
            jail_bin,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(self.root.join("bin/claude"));
        cmd.args(args);
        jail::hermetic(&mut cmd, &self.root.join("bin"), &self.jail_bin);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    /// Run the fake with `prompt` on stdin: exit code, stdout, stderr.
    fn run(&self, args: &[&str], prompt: &str) -> (i32, String, String) {
        let mut child = self.command(args).spawn().expect("start the fake claude");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(prompt.as_bytes())
            .expect("write the prompt");
        let out = child.wait_with_output().expect("wait for the fake claude");
        jail::assert_no_command_not_found(&out.stderr);
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
}

#[test]
fn it_serves_the_output_of_the_stage_and_logs_the_call() {
    let bench = Bench::new("loop-fake-claude");
    bench.claude.output("security", None, "SECURITY-DEFAULT");
    bench.claude.output("security", Some(2), "SECURITY-SECOND");
    let (code, out, _) = bench.run(&["-p", "--model", "opus"], "stage: security\nbody\n");
    assert_eq!((code, out.as_str()), (0, "SECURITY-DEFAULT"));
    let (_, out, _) = bench.run(&["-p"], "stage: security\nbody\n");
    assert_eq!(out, "SECURITY-SECOND");
    // An unscripted stage is a harness bug: exit 99.
    let (code, _, err) = bench.run(&[], "stage: nothing\n");
    assert_eq!(code, 99);
    assert!(err.contains("no output for stage nothing"), "{err}");

    let calls = bench.claude.calls();
    assert_eq!(calls.len(), 3);
    assert_eq!(
        (calls[0].stage.as_str(), calls[0].k, calls[0].model()),
        ("security", 1, "opus")
    );
    assert_eq!((calls[1].stage.as_str(), calls[1].k), ("security", 2));
    assert_eq!(calls[0].stdin, "stage: security\nbody\n");
    assert_eq!(calls[0].argv, ["-p", "--model", "opus"]);
    assert!(calls[0].env.iter().any(|l| l.starts_with("PATH=")));
    assert!(calls[0].pid > 1);
}

#[test]
fn its_modes_print_what_the_stage_runner_expects() {
    let bench = Bench::new("loop-fake-claude-modes");
    for (mode, want_code) in [
        ("error", 1),
        ("empty", 0),
        ("usage-limit", 1),
        ("refusal", 0),
        ("over-budget", 1),
    ] {
        bench.claude.mode(mode, None, mode);
        let (code, out, err) = bench.run(&[], &format!("stage: {mode}\n"));
        assert_eq!(code, want_code, "{mode}: {err}");
        let parsed = parse_envelope(out.as_bytes());
        match mode {
            "error" => {
                assert_eq!(parsed, StageResult::Empty);
                assert!(err.contains("boom"), "{err}");
            }
            "empty" => assert_eq!(parsed, StageResult::Empty),
            "usage-limit" => assert_eq!(parsed, StageResult::UsageLimit),
            "refusal" => assert_eq!(parsed, StageResult::Refusal),
            "over-budget" => assert!(matches!(parsed, StageResult::OverBudget { .. })),
            other => unreachable!("{other}"),
        }
    }
}

#[test]
fn the_sleep_mode_sleeps_until_its_group_is_stopped() {
    let bench = Bench::new("loop-fake-claude-sleep");
    bench.claude.mode("slow", None, "sleep");
    let mut cmd = bench.command(&[]);
    let mut child = cmd.spawn().expect("start the fake claude");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"stage: slow\n")
        .expect("write the prompt");
    let pid = wait_for_call(&bench.claude).pid;
    assert!(child.try_wait().expect("try_wait").is_none(), "it sleeps");
    let status = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .expect("kill");
    assert!(status.success());
    let status = child.wait().expect("wait");
    assert!(!status.success(), "{status:?}");
}

fn wait_for_call(claude: &FakeClaude) -> common::loop_fake::ClaudeCall {
    for _ in 0..500 {
        if let Some(call) = claude.calls().into_iter().next() {
            return call;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("the fake claude logged no call");
}

#[test]
fn concurrent_calls_get_distinct_directories_and_counters() {
    let bench = Bench::new("loop-fake-claude-concurrent");
    bench
        .claude
        .output("lens", None, &envelope(&json!({"findings": []}), 0.01));
    let children: Vec<_> = (0..6)
        .map(|_| {
            let mut child = bench.command(&[]).spawn().expect("start");
            child
                .stdin
                .take()
                .expect("stdin")
                .write_all(b"stage: lens\n")
                .expect("write");
            child
        })
        .collect();
    for child in children {
        assert!(child.wait_with_output().expect("wait").status.success());
    }
    let calls = bench.claude.calls();
    let seqs: BTreeSet<_> = calls.iter().map(|c| c.seq.clone()).collect();
    let ks: BTreeSet<_> = calls.iter().map(|c| c.k).collect();
    assert_eq!(seqs.len(), 6);
    assert_eq!(ks, (1..=6).collect());
}

// ---------------------------------------------------------------------------
// assert_read_only
// ---------------------------------------------------------------------------

/// The panic message of `assert_read_only` on `rig` after `mutate`, if it panics.
fn violation(label: &'static str, mutate: impl FnOnce(&Rig)) -> Option<String> {
    let rig = Rig::new(&Scenario::new(label));
    mutate(&rig);
    let outcome = rig.outcome(0, b"", b"");
    catch_unwind(AssertUnwindSafe(|| assert_read_only(&outcome)))
        .err()
        .map(|payload| {
            payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default()
        })
}

fn run_fake(rig: &Rig, program: &str, args: &[&str]) {
    let mut cmd = Command::new(rig.root.join("bin").join(program));
    cmd.args(args)
        .current_dir(&rig.clone)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let jail_bin = rig.root.join("jail/bin");
    jail::hermetic(&mut cmd, &rig.root.join("bin"), &jail_bin);
    cmd.status().expect("run the fake");
}

#[test]
fn a_run_that_changed_nothing_passes() {
    assert_eq!(violation("loop-ro-ok", |_| {}), None);
}

#[test]
fn a_gh_write_is_caught() {
    let why = violation("loop-ro-gh-write", |rig| {
        rig.gh.fixture("merge", r"^pr merge ").out("");
        run_fake(rig, "gh", &["pr", "merge", "1"]);
    })
    .expect("a panic");
    assert!(why.contains("not a read"), "{why}");
}

#[test]
fn a_gh_call_that_skipped_the_loops_environment_is_caught() {
    // The hermetic run strips every GH_* variable and sets no NO_COLOR, which is
    // what a `Command::new("gh")` that bypassed `GhCli` would look like.
    let why = violation("loop-ro-gh-env", |rig| {
        rig.gh.fixture("view", r"^repo view ").out("{}");
        run_fake(rig, "gh", &["repo", "view", "--json", "owner,name"]);
    })
    .expect("a panic");
    assert!(why.contains("loop's environment"), "{why}");
}

#[test]
fn a_gh_call_without_a_fixture_is_caught() {
    let why = violation("loop-ro-gh-unknown", |rig| {
        run_fake(rig, "gh", &["pr", "list"]);
    })
    .expect("a panic");
    assert!(why.contains("no fixture"), "{why}");
}

#[test]
fn a_new_origin_ref_is_caught() {
    let why = violation("loop-ro-origin", |rig| {
        git(
            &rig.clone,
            &["push", "-q", "origin", "HEAD:refs/heads/sneaky"],
        );
    })
    .expect("a panic");
    assert!(why.contains("repositories changed"), "{why}");
}

#[test]
fn a_moved_head_or_a_dirty_clone_is_caught() {
    let why = violation("loop-ro-head", |rig| {
        git(&rig.clone, &["checkout", "-q", "-b", "elsewhere"]);
    })
    .expect("a panic");
    assert!(why.contains("repositories changed"), "{why}");
    let why = violation("loop-ro-dirty", |rig| {
        std::fs::write(rig.clone.join("stray.txt"), "x").expect("write");
    })
    .expect("a panic");
    assert!(why.contains("repositories changed"), "{why}");
}

#[test]
fn a_left_round_worktree_is_caught() {
    let why = violation("loop-ro-worktree", |rig| {
        std::fs::create_dir_all(rig.clone.join(".claude/worktrees/loop-pr1")).expect("mkdir");
    })
    .expect("a panic");
    assert!(why.contains("round directories are left"), "{why}");
}

#[test]
fn a_claude_call_with_a_shell_is_caught() {
    let why = violation("loop-ro-bash", |rig| {
        rig.claude.output("x", None, "{}");
        let mut cmd = Command::new(rig.root.join("bin/claude"));
        cmd.args(["--tools", "Read,Bash", "--allowedTools", "Read", "Bash"]);
        let jail_bin = rig.root.join("jail/bin");
        jail::hermetic(&mut cmd, &rig.root.join("bin"), &jail_bin);
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(b"stage: x\n")
            .expect("write");
        child.wait().expect("wait");
    })
    .expect("a panic");
    assert!(why.contains("Read,Grep,Glob"), "{why}");
}

#[test]
fn the_rig_is_a_real_clone_with_each_pr_head_on_origin() {
    let rig = Rig::new(&Scenario::new("loop-rig"));
    let sha = &rig.heads[&1];
    let on_origin = git(&rig.origin, &["rev-parse", "refs/pull/1/head"]);
    assert_eq!(on_origin.trim(), sha);
    assert_eq!(
        git(&rig.clone, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "main"
    );
    assert!(Path::new(&rig.clone)
        .join("scripts/agent/prompts/verify.md")
        .exists());
}
