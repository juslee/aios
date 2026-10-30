//! The harness of the `aios loop review` scenarios (R2a, Task 6a): a fake
//! `claude`, and `run_loop`, which runs the real `aios` binary against real git
//! (a clone and a bare origin), the fake `gh` of `fake_gh` with scenario
//! fixtures, and the PATH jail.
//!
//! One scenario root (made under `CARGO_TARGET_TMPDIR`):
//!
//! ```text
//! <root>/bin/{gh,claude}      the fakes (first on PATH, as AIOS_LOOP_ANY_EXE requires)
//! <root>/jail/bin             the host tools the loop may run (git, kill, sh, ...)
//! <root>/origin.git           the bare origin; each PR head is refs/pull/<n>/head
//! <root>/clone                the main checkout the loop runs in
//! <root>/state                AIOS_LOOP_STATE_DIR
//! <root>/tree                 the --tree directory, when the scenario has one
//! <root>/claude/<stage>.json  what the fake claude prints for a stage
//! <root>/claude-calls/<SEQ>/  argv (NUL-separated), env, stdin, stage, k, pid of one call
//! ```
//!
//! The fake claude reads the `stage: <name>` line that opens every rendered
//! prompt. The K-th call of a stage prints `claude/<stage>.<K>.json` if there is
//! one, else `claude/<stage>.json`; `<stage>[.<K>].mode` makes it misbehave
//! instead (`error`, `empty`, `sleep`, `usage-limit`, `refusal`, `over-budget`).
//!
//! This module uses `super::{git, isolated, unique_dir}`, `fake_gh` and `jail`.

use std::collections::BTreeMap;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use regex::Regex;
use serde_json::{json, Value};

use super::fake_gh::{FakeGh, GhCall};
use super::{git, jail, unique_dir};

/// The fake `claude`. Its external commands are `jail::FAKE_CLAUDE_TOOLS` (checked
/// by `loop_fakes::the_fake_claude_runs_exactly_its_tool_list`).
pub const FAKE_CLAUDE: &str = r#"#!/bin/sh
# The fake claude of tools/tests/common/loop_fake.rs. The root is found from $0, never
# from the environment, because the tests strip every CLAUDE* and AIOS_* variable.
root=$(cd "${0%/*}/.." && pwd -P)

mkdir -p "$root/claude-calls" "$root/claude-state"
n=1
while :; do
    seq=$(printf '%06d' "$n")
    call=$root/claude-calls/$seq
    if mkdir "$call" 2>/dev/null; then
        break
    fi
    n=$((n + 1))
done

printf '%s\0' "$@" >"$call/argv"
env >"$call/env"
cat >"$call/stdin"
printf '%s\n' "$$" >"$call/pid"

stage=
read -r first <"$call/stdin"
case $first in
    "stage: "*) stage=${first#stage: } ;;
esac
printf '%s' "$stage" >"$call/stage"

k=1
while :; do
    if mkdir "$root/claude-state/$stage.$k" 2>/dev/null; then
        break
    fi
    k=$((k + 1))
done
printf '%s' "$k" >"$call/k"

pick() {
    if [ -f "$root/claude/$stage.$k.$1" ]; then
        printf '%s\n' "$root/claude/$stage.$k.$1"
    elif [ -f "$root/claude/$stage.$1" ]; then
        printf '%s\n' "$root/claude/$stage.$1"
    fi
}

mode=normal
f=$(pick mode)
if [ -n "$f" ]; then
    mode=$(cat "$f")
fi
case $mode in
    error)
        echo "fake claude: boom" >&2
        exit 1
        ;;
    empty)
        exit 0
        ;;
    sleep)
        sleep 300 &
        printf '%s\n' "$!" >"$call/sleep.pid"
        wait "$!"
        exit 0
        ;;
    usage-limit)
        printf '%s' '{"type":"result","subtype":"error_during_execution","is_error":true,"api_error_status":429,"result":"You have hit your limit","total_cost_usd":0}'
        exit 1
        ;;
    refusal)
        printf '%s' '{"type":"result","subtype":"success","is_error":false,"stop_reason":"refusal","total_cost_usd":0.001}'
        exit 0
        ;;
    over-budget)
        printf '%s' '{"type":"result","subtype":"error_max_budget_usd","is_error":true,"total_cost_usd":0.2}'
        exit 1
        ;;
esac
f=$(pick json)
if [ -z "$f" ]; then
    echo "fake claude: no output for stage $stage" >&2
    exit 99
fi
cat "$f"
"#;

/// The functions `FAKE_CLAUDE` defines (not host tools).
pub const FAKE_CLAUDE_FUNCTIONS: &[&str] = &["pick"];

/// One logged call of the fake claude.
#[derive(Debug, Clone)]
pub struct ClaudeCall {
    pub seq: String,
    pub argv: Vec<String>,
    /// `NAME=value` lines of the child's environment.
    pub env: Vec<String>,
    pub stdin: String,
    pub stage: String,
    /// The call's index among the calls of its stage, from 1.
    pub k: usize,
    /// The pid of the fake's shell.
    pub pid: u32,
}

impl ClaudeCall {
    /// The word after `flag` in the argv.
    pub fn flag(&self, flag: &str) -> Option<&str> {
        let at = self.argv.iter().position(|a| a == flag)?;
        self.argv.get(at + 1).map(String::as_str)
    }

    /// The words after `flag` up to the next `--option`.
    pub fn flag_values(&self, flag: &str) -> Vec<&str> {
        let Some(at) = self.argv.iter().position(|a| a == flag) else {
            return Vec::new();
        };
        self.argv[at + 1..]
            .iter()
            .take_while(|a| !a.starts_with("--"))
            .map(String::as_str)
            .collect()
    }

    pub fn model(&self) -> &str {
        self.flag("--model").unwrap_or("")
    }
}

/// A fake `claude` installed under a root directory.
pub struct FakeClaude {
    root: PathBuf,
}

impl FakeClaude {
    /// Create `<root>/bin/claude` and the output directory.
    pub fn install(root: &Path) -> FakeClaude {
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).expect("create the fake bin directory");
        std::fs::create_dir_all(root.join("claude")).expect("create the claude directory");
        let script = bin.join("claude");
        std::fs::write(&script, FAKE_CLAUDE).expect("write the fake claude");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("make the fake claude executable");
        FakeClaude {
            root: root.to_path_buf(),
        }
    }

    /// What the K-th call of `stage` prints (`None`: every call).
    pub fn output(&self, stage: &str, k: Option<usize>, text: &str) {
        let name = match k {
            Some(k) => format!("{stage}.{k}.json"),
            None => format!("{stage}.json"),
        };
        std::fs::write(self.root.join("claude").join(name), text).expect("write a stage output");
    }

    /// How the K-th call of `stage` misbehaves (`None`: every call).
    pub fn mode(&self, stage: &str, k: Option<usize>, mode: &str) {
        let name = match k {
            Some(k) => format!("{stage}.{k}.mode"),
            None => format!("{stage}.mode"),
        };
        std::fs::write(self.root.join("claude").join(name), mode).expect("write a stage mode");
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<ClaudeCall> {
        let dir = self.root.join("claude-calls");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut dirs: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
        dirs.sort();
        dirs.iter().filter_map(|d| read_claude_call(d)).collect()
    }
}

fn read_claude_call(dir: &Path) -> Option<ClaudeCall> {
    // A call directory is made before its files are written: skip one that is
    // still being filled.
    let text = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
    let argv_raw = std::fs::read(dir.join("argv")).ok()?;
    let mut argv: Vec<String> = argv_raw
        .split(|&b| b == 0)
        .map(|w| String::from_utf8_lossy(w).into_owned())
        .collect();
    argv.pop();
    Some(ClaudeCall {
        seq: dir.file_name()?.to_string_lossy().into_owned(),
        argv,
        env: text("env")?.lines().map(str::to_string).collect(),
        stdin: text("stdin")?,
        stage: text("stage")?,
        k: text("k")?.parse().ok()?,
        pid: text("pid")?.trim().parse().ok()?,
    })
}

/// A successful `claude -p --output-format json` result carrying `value`.
pub fn envelope(value: &Value, cost: f64) -> String {
    json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "total_cost_usd": cost,
        "structured_output": value,
        "permission_denials": [],
    })
    .to_string()
}

/// One lens finding (the `id` and `lens` are overwritten by the loop).
pub fn finding(severity: &str, category: &str, file: &str, line: u32, claim: &str) -> Value {
    json!({
        "id": "x", "lens": "x", "severity": severity, "category": category,
        "file": file, "line": line, "claim": claim, "evidence": "the code", "suggested_fix": null,
    })
}

/// One verify verdict.
pub fn verdict(id: &str, verdict: &str, scope: &str) -> Value {
    json!({"id": id, "verdict": verdict, "scope": scope, "reason": "checked"})
}

// ---------------------------------------------------------------------------
// The scenario
// ---------------------------------------------------------------------------

/// One review thread of a PR.
#[derive(Debug, Clone)]
pub struct ThreadSpec {
    pub author: String,
    pub body: String,
    pub resolved: bool,
    /// Later comments of the thread as `(author, body)`.
    pub replies: Vec<(String, String)>,
}

/// One PR of a scenario. `files` are committed on the PR branch.
#[derive(Debug, Clone)]
pub struct PrSpec {
    pub number: u64,
    pub files: Vec<(String, String)>,
    pub labels: Vec<String>,
    pub cross_repo: bool,
    pub draft: bool,
    /// `(name, bucket)`, bucket one of `pass`, `fail`, `pending`.
    pub checks: Vec<(String, String)>,
    pub threads: Vec<ThreadSpec>,
    /// What GitHub lists as the changed files, when it differs from git's.
    pub github_files: Option<Vec<String>>,
    pub changed_files: Option<u64>,
}

/// The run id of the failing check's link: the one `run_log_failed.txt` is for.
pub const FAILING_RUN_ID: u64 = 40_448_856_000;

impl PrSpec {
    pub fn new(number: u64) -> PrSpec {
        PrSpec {
            number,
            files: vec![("notes/demo.txt".to_string(), format!("demo {number}\n"))],
            labels: vec!["team-build".to_string()],
            cross_repo: false,
            draft: false,
            checks: vec![("ci".to_string(), "pass".to_string())],
            threads: Vec::new(),
            github_files: None,
            changed_files: None,
        }
    }

    fn branch(&self) -> String {
        format!("claude/demo-{}", self.number)
    }
}

/// How the `agent-stop` label lookup answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    Present,
    /// HTTP 404.
    Missing,
    /// Any other gh failure.
    Failing,
}

/// Everything a scenario sets up.
#[derive(Debug, Clone)]
pub struct Scenario {
    pub name: &'static str,
    pub prs: Vec<PrSpec>,
    /// Run with `--tree <root>/tree`, the tree holding the real config and
    /// prompts, then these files over them.
    pub tree: Option<Vec<(String, String)>>,
    pub extra_args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub stop_file: bool,
    pub label: Label,
    pub stop_issues: Vec<u64>,
    /// The open needs-human issues, as `gh issue list` prints them.
    pub needs_human: Vec<Value>,
    /// Run with `AIOS_LOOP_ANY_EXE=1` (the default); without it the location
    /// check applies.
    pub any_exe: bool,
    /// Files committed on main besides the harness's own.
    pub main_files: Vec<(String, String)>,
    /// Lines for `findings.jsonl`, written before the run.
    pub prior: Vec<String>,
    outputs: BTreeMap<(String, Option<usize>), String>,
    modes: BTreeMap<(String, Option<usize>), String>,
}

impl Scenario {
    /// One PR (#1), clean lenses at $0.05 each, nothing wrong anywhere.
    pub fn new(name: &'static str) -> Scenario {
        let mut s = Scenario {
            name,
            prs: vec![PrSpec::new(1)],
            tree: None,
            extra_args: Vec::new(),
            env: Vec::new(),
            stop_file: false,
            label: Label::Present,
            stop_issues: Vec::new(),
            needs_human: Vec::new(),
            any_exe: true,
            main_files: Vec::new(),
            prior: Vec::new(),
            outputs: BTreeMap::new(),
            modes: BTreeMap::new(),
        };
        for lens in ["correctness", "security", "conventions"] {
            s.lens(lens, Vec::new(), 0.05);
        }
        s
    }

    pub fn pr(&mut self) -> &mut PrSpec {
        &mut self.prs[0]
    }

    /// What a lens prints on every call.
    pub fn lens(&mut self, lens: &str, findings: Vec<Value>, cost: f64) -> &mut Scenario {
        self.outputs.insert(
            (lens.to_string(), None),
            envelope(&json!({ "findings": findings }), cost),
        );
        self
    }

    /// What verify prints on every call.
    pub fn verify(&mut self, verdicts: Vec<Value>, cost: f64) -> &mut Scenario {
        self.outputs.insert(
            ("verify".to_string(), None),
            envelope(&json!({ "verdicts": verdicts }), cost),
        );
        self
    }

    /// What the K-th call of `stage` prints.
    pub fn output_of(&mut self, stage: &str, k: usize, text: String) -> &mut Scenario {
        self.outputs.insert((stage.to_string(), Some(k)), text);
        self
    }

    /// How a stage misbehaves (`k`: the K-th call, or every call).
    pub fn mode(&mut self, stage: &str, k: Option<usize>, mode: &str) -> &mut Scenario {
        self.modes.insert((stage.to_string(), k), mode.to_string());
        self
    }
}

// ---------------------------------------------------------------------------
// The rig
// ---------------------------------------------------------------------------

/// What the invariant compares before and after a run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Snapshot {
    origin_refs: String,
    clone_refs: String,
    head: String,
    head_ref: String,
    status: String,
}

/// A built scenario: repositories, fakes and jail, not yet run.
pub struct Rig {
    pub root: PathBuf,
    pub clone: PathBuf,
    pub origin: PathBuf,
    pub state: PathBuf,
    pub gh: FakeGh,
    pub claude: FakeClaude,
    /// The head sha of each PR.
    pub heads: BTreeMap<u64, String>,
    jail_bin: PathBuf,
    args: Vec<String>,
    env: Vec<(String, String)>,
    any_exe: bool,
    before: Snapshot,
}

fn repo_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("create directories");
    std::fs::write(&path, text).expect("write a file");
}

/// The prompts and config the loop reads, as committed in this repository.
const LOOP_FILES: [&str; 5] = [
    "scripts/agent/loop-config.json",
    "scripts/agent/prompts/review-correctness.md",
    "scripts/agent/prompts/review-security.md",
    "scripts/agent/prompts/review-conventions.md",
    "scripts/agent/prompts/verify.md",
];

/// main's `.claude/settings.json`: a guard hook, two permission lists and a plugin.
const SETTINGS: &str = r#"{
  "enabledPlugins": {"demo@market": true},
  "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "/usr/bin/python3 \"$CLAUDE_PROJECT_DIR/.claude/hooks/git-push-guard.py\""}]}]},
  "permissions": {"allow": ["Bash(ls:*)"], "ask": ["Bash(git push:*)"], "deny": ["Bash(rm -rf *)"]}
}
"#;

/// main's `scripts/agent/qemu-lock.sh`: no quiet window. A scenario replaces it
/// through `Scenario::main_files`.
pub const QEMU_LOCK_IDLE: &str = "#!/bin/sh\necho 'no lock'\n";

/// A lesson the knowledge-hive selection can find (tag `tooling`).
pub const LESSON_PATH: &str = "docs/knowledge/lessons/2026-01-01-jl-demo-lesson.md";
pub const LESSON_TEXT: &str = "---\nauthor: jl\ndate: 2026-01-01\ntags: [tooling]\nstatus: final\n---\n# Demo lesson\n\nDEMO-LESSON-MARKER\n";

impl Rig {
    pub fn new(s: &Scenario) -> Rig {
        let root = unique_dir(&format!("loop-{}", s.name));
        let gh = FakeGh::install(&root);
        let claude = FakeClaude::install(&root);
        let jail_bin = jail::bin(&root.join("jail"), &["git", "kill"]);
        let origin = root.join("origin.git");
        let clone = root.join("clone");
        let state = root.join("state");
        std::fs::create_dir_all(&origin).expect("create the origin");
        git(&origin, &["init", "-q", "--bare", "-b", "main"]);
        std::fs::create_dir_all(&clone).expect("create the clone");
        git(&clone, &["init", "-q", "-b", "main"]);
        git(&clone, &["config", "core.excludesFile", "/dev/null"]);
        git(
            &clone,
            &["remote", "add", "origin", origin.to_str().expect("UTF-8")],
        );

        // main.
        write(&clone, ".claude/settings.json", SETTINGS);
        write(&clone, "docs/project/doc-map.md", "# Doc map\n");
        write(&clone, "README.md", "demo\n");
        write(&clone, LESSON_PATH, LESSON_TEXT);
        write(&clone, "scripts/agent/qemu-lock.sh", QEMU_LOCK_IDLE);
        for rel in LOOP_FILES {
            write(&clone, rel, &repo_file(rel));
        }
        for (rel, text) in &s.main_files {
            write(&clone, rel, text);
        }
        git(&clone, &["add", "-A"]);
        git(&clone, &["commit", "-q", "-m", "base"]);
        git(&clone, &["push", "-q", "-u", "origin", "main"]);

        // The PR heads.
        let mut heads = BTreeMap::new();
        for pr in &s.prs {
            git(&clone, &["checkout", "-q", "-b", &pr.branch(), "main"]);
            for (rel, text) in &pr.files {
                write(&clone, rel, text);
            }
            git(&clone, &["add", "-A"]);
            git(
                &clone,
                &["commit", "-q", "-m", &format!("PR {} work", pr.number)],
            );
            let sha = git(&clone, &["rev-parse", "HEAD"]).trim().to_string();
            git(
                &clone,
                &[
                    "push",
                    "-q",
                    "origin",
                    &format!("HEAD:refs/pull/{}/head", pr.number),
                ],
            );
            git(&clone, &["checkout", "-q", "main"]);
            git(&clone, &["branch", "-q", "-D", &pr.branch()]);
            heads.insert(pr.number, sha);
        }

        // The --tree directory.
        let mut args: Vec<String> = Vec::new();
        if let Some(files) = &s.tree {
            let tree = root.join("tree");
            for rel in LOOP_FILES {
                write(&tree, rel, &repo_file(rel));
            }
            for (rel, text) in files {
                write(&tree, rel, text);
            }
            args.push("--tree".to_string());
            args.push(tree.to_str().expect("UTF-8").to_string());
        }
        args.extend(s.extra_args.iter().cloned());

        // State.
        std::fs::create_dir_all(&state).expect("create the state directory");
        if s.stop_file {
            std::fs::write(state.join("STOP"), "stop\n").expect("write STOP");
        }
        if !s.prior.is_empty() {
            std::fs::write(state.join("findings.jsonl"), s.prior.join("\n") + "\n")
                .expect("write findings.jsonl");
        }

        // The claude fake.
        for ((stage, k), text) in &s.outputs {
            claude.output(stage, *k, text);
        }
        for ((stage, k), mode) in &s.modes {
            claude.mode(stage, *k, mode);
        }

        install_gh_fixtures(&gh, s, &heads);

        let mut rig = Rig {
            root,
            clone,
            origin,
            state,
            gh,
            claude,
            heads,
            jail_bin,
            args: Vec::new(),
            env: s.env.clone(),
            any_exe: s.any_exe,
            before: Snapshot::default(),
        };
        let mut all: Vec<String> = s.prs.iter().map(|p| p.number.to_string()).collect();
        all.extend(args);
        rig.args = all;
        rig.before = rig.snapshot();
        rig
    }

    fn snapshot(&self) -> Snapshot {
        let refs = |dir: &Path| git(dir, &["for-each-ref", "--format=%(refname) %(objectname)"]);
        Snapshot {
            origin_refs: refs(&self.origin),
            clone_refs: refs(&self.clone),
            head: git(&self.clone, &["rev-parse", "HEAD"]),
            head_ref: git(&self.clone, &["symbolic-ref", "-q", "HEAD"]),
            status: git(&self.clone, &["status", "--porcelain"]),
        }
    }

    /// The `aios loop review` command of this scenario.
    pub fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_aios"));
        cmd.current_dir(&self.clone).args(["loop", "review"]);
        cmd.args(&self.args);
        jail::hermetic(&mut cmd, &self.gh.bin(), &self.jail_bin);
        cmd.env("CLAUDE_CONFIG_DIR", self.root.join("claude-config"))
            .env("AIOS_LOOP_STATE_DIR", &self.state);
        if self.any_exe {
            cmd.env("AIOS_LOOP_ANY_EXE", "1");
        }
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    /// Finish with the output of a run that already happened.
    pub fn outcome(self, code: i32, stdout: &[u8], stderr: &[u8]) -> Outcome {
        let after = self.snapshot();
        let gh_calls = self.gh.calls();
        let claude_calls = self.claude.calls();
        Outcome {
            code,
            stdout: String::from_utf8_lossy(stdout).into_owned(),
            stderr: String::from_utf8_lossy(stderr).into_owned(),
            gh_calls,
            claude_calls,
            after,
            rig: self,
        }
    }
}

fn pr_json(pr: &PrSpec, head: &str) -> Value {
    let rollup: Vec<Value> = pr
        .checks
        .iter()
        .map(|(name, bucket)| match bucket.as_str() {
            "pass" => json!({"name": name, "status": "COMPLETED", "conclusion": "SUCCESS"}),
            "fail" => json!({"name": name, "status": "COMPLETED", "conclusion": "FAILURE"}),
            _ => json!({"name": name, "status": "IN_PROGRESS", "conclusion": ""}),
        })
        .collect();
    let files: Vec<String> = pr
        .github_files
        .clone()
        .unwrap_or_else(|| pr.files.iter().map(|(rel, _)| rel.clone()).collect());
    let labels: Vec<Value> = pr.labels.iter().map(|l| json!({"name": l})).collect();
    json!({
        "number": pr.number,
        "title": format!("Demo PR {}", pr.number),
        "headRefName": pr.branch(),
        "isDraft": pr.draft,
        "mergeable": "MERGEABLE",
        "mergeStateStatus": "CLEAN",
        "reviewDecision": "",
        "statusCheckRollup": rollup,
        "author": {"login": "juslee"},
        "labels": labels,
        "headRefOid": head,
        "baseRefName": "main",
        "isCrossRepository": pr.cross_repo,
        "changedFiles": pr.changed_files.unwrap_or(files.len() as u64),
        "files": files.iter().map(|p| json!({"path": p})).collect::<Vec<_>>(),
        "commits": [],
        "reviews": [],
    })
}

fn checks_json(pr: &PrSpec) -> Value {
    Value::Array(
        pr.checks
            .iter()
            .enumerate()
            .map(|(i, (name, bucket))| {
                let run = if bucket == "fail" {
                    FAILING_RUN_ID
                } else {
                    111 + i as u64
                };
                json!({
                    "name": name,
                    "bucket": bucket,
                    "state": bucket.to_uppercase(),
                    "link": format!("https://github.com/o/r/actions/runs/{run}/job/{}", 7 + i),
                })
            })
            .collect(),
    )
}

fn threads_json(pr: &PrSpec) -> Value {
    let nodes: Vec<Value> = pr
        .threads
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mut comments = vec![json!({
                "databaseId": 100 + i as u64,
                "author": {"login": t.author},
                "body": t.body,
            })];
            for (j, (author, body)) in t.replies.iter().enumerate() {
                comments.push(json!({
                    "databaseId": 200 + 10 * i as u64 + j as u64,
                    "author": {"login": author},
                    "body": body,
                }));
            }
            json!({
                "id": format!("PRRT_{}_{i}", pr.number),
                "isResolved": t.resolved,
                "isOutdated": false,
                "path": "notes/demo.txt",
                "line": 1,
                "comments": {"nodes": comments},
            })
        })
        .collect();
    json!({"data": {"repository": {"pullRequest": {"reviewThreads": {
        "pageInfo": {"hasNextPage": false, "endCursor": null},
        "nodes": nodes,
    }}}}})
}

fn fixture_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/loop/gh")
        .join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn install_gh_fixtures(gh: &FakeGh, s: &Scenario, heads: &BTreeMap<u64, String>) {
    let label = gh.fixture(
        "label-agent-stop",
        r"^api repos/\{owner\}/\{repo\}/labels/agent-stop ",
    );
    match s.label {
        Label::Present => {
            label.out(fixture_file("label_agent_stop.json"));
        }
        Label::Missing => {
            label
                .err(fixture_file("label_404.err"))
                .rc(fixture_file("label_404.rc")
                    .trim()
                    .parse()
                    .expect("an exit code"));
        }
        Label::Failing => {
            label.err("gh: Internal Server Error (HTTP 500)\n").rc(1);
        }
    }
    let stop_issues: Vec<Value> = s.stop_issues.iter().map(|n| json!({"number": n})).collect();
    gh.fixture("issues-agent-stop", r"^issue list --label agent-stop ")
        .out(Value::Array(stop_issues).to_string());
    gh.fixture(
        "issues-needs-human",
        r"^issue list --state open --label needs-human ",
    )
    .out(Value::Array(s.needs_human.clone()).to_string());
    gh.fixture("repo-view", r"^repo view ")
        .out(r#"{"owner":{"login":"o"},"name":"r"}"#);
    gh.fixture(
        "run-log",
        &format!(r"^run view {FAILING_RUN_ID} --log-failed "),
    )
    .out(fixture_file("run_log_failed.txt"));
    for pr in &s.prs {
        let n = pr.number;
        gh.fixture(&format!("pr-view-{n}"), &format!(r"^pr view {n} "))
            .out(pr_json(pr, &heads[&n]).to_string());
        gh.fixture(&format!("pr-checks-{n}"), &format!(r"^pr checks {n} "))
            .out(checks_json(pr).to_string());
        gh.fixture(
            &format!("threads-{n}"),
            &format!(r"^api graphql .*-F n={n} "),
        )
        .out(threads_json(pr).to_string());
    }
}

// ---------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------

/// A started run.
pub struct Running {
    pub rig: Rig,
    pub child: Child,
}

/// Start the scenario's `aios loop review` without waiting for it.
pub fn start_loop(s: &Scenario) -> Running {
    let rig = Rig::new(s);
    let child = rig.command().spawn().expect("start the aios binary");
    Running { rig, child }
}

impl Running {
    pub fn wait(self) -> Outcome {
        let out = self.child.wait_with_output().expect("wait for aios");
        self.rig
            .outcome(out.status.code().unwrap_or(-1), &out.stdout, &out.stderr)
    }
}

/// Run the scenario to the end.
pub fn run_loop(s: &Scenario) -> Outcome {
    start_loop(s).wait()
}

/// What a run left behind.
pub struct Outcome {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    pub gh_calls: Vec<GhCall>,
    pub claude_calls: Vec<ClaudeCall>,
    after: Snapshot,
    pub rig: Rig,
}

impl Outcome {
    /// The calls of one stage, in order.
    pub fn stage_calls(&self, stage: &str) -> Vec<&ClaudeCall> {
        self.claude_calls
            .iter()
            .filter(|c| c.stage == stage)
            .collect()
    }

    /// The run directories of PR `n`, oldest first.
    pub fn run_dirs(&self, n: u64) -> Vec<PathBuf> {
        let dir = self.rig.state.join(format!("pr-{n}"));
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.as_bytes().starts_with(b"run-"))
            })
            .collect();
        dirs.sort();
        dirs
    }

    /// The only run directory of PR `n`.
    pub fn run_dir(&self, n: u64) -> PathBuf {
        let dirs = self.run_dirs(n);
        assert_eq!(dirs.len(), 1, "run directories of pr {n}: {dirs:?}");
        dirs[0].clone()
    }

    /// `report.json` of PR `n`.
    pub fn report(&self, n: u64) -> Value {
        let path = self.run_dir(n).join("report.json");
        serde_json::from_slice(
            &std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
        )
        .expect("report.json is JSON")
    }

    /// The records of `findings.jsonl`.
    pub fn records(&self) -> Vec<Value> {
        std::fs::read_to_string(self.rig.state.join("findings.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).expect("a JSON line"))
            .collect()
    }

    /// `text` with the volatile parts replaced: the scenario root, run epochs,
    /// short head shas and full shas.
    pub fn normalise(&self, text: &str) -> String {
        let root = self.rig.root.to_str().expect("UTF-8");
        let text = text.replace(root, "<ROOT>");
        let epoch = Regex::new(r"run-[0-9]+").expect("regex");
        let short = Regex::new(r"@ [0-9a-f]{7}:").expect("regex");
        let full = Regex::new(r"[0-9a-f]{40}").expect("regex");
        let heading = Regex::new(r"(?m)^(# Review of PR #[0-9]+ at )[0-9a-f]{7}$").expect("regex");
        let text = heading.replace_all(&text, "${1}<SHA7>");
        let text = epoch.replace_all(&text, "run-<EPOCH>");
        let text = short.replace_all(&text, "@ <SHA7>:");
        full.replace_all(&text, "<SHA>").into_owned()
    }
}

/// The read-only invariant of every scenario: every fake-gh call is a read on
/// `gh::is_read`'s allowlist, ran with the loop's gh environment in the main
/// checkout, and matched a fixture; origin's refs, the clone's
/// refs, HEAD and working tree are as they were; no `loop-pr*` worktree is left;
/// and every `claude` call had the read-only toolset.
pub fn assert_read_only(o: &Outcome) {
    o.rig.gh.assert_all_matched();
    for call in &o.gh_calls {
        let argv: Vec<String> = call
            .argv
            .iter()
            .map(|w| String::from_utf8_lossy(w).into_owned())
            .collect();
        let words: Vec<&str> = argv.iter().map(String::as_str).collect();
        if let Err(why) = aios_tools::gh::is_read(&words) {
            panic!("a gh call is not a read ({why}): {}", call.line());
        }
        // It went through `GhCli` with the loop's environment, in the main checkout.
        for (name, want) in [
            ("GH_PROMPT_DISABLED", Some("1")),
            ("GH_NO_UPDATE_NOTIFIER", Some("1")),
            ("NO_COLOR", Some("1")),
            ("GH_REPO", None),
            ("GH_HOST", None),
        ] {
            assert_eq!(
                call.env.get(name).cloned().flatten().as_deref(),
                want,
                "gh was not run with the loop's environment ({name}): {}",
                call.line()
            );
        }
        let main = o.rig.clone.canonicalize().expect("the clone exists");
        assert_eq!(
            call.cwd.canonicalize().ok().as_ref(),
            Some(&main),
            "a gh call ran outside the main checkout: {}",
            call.line()
        );
    }
    assert_eq!(o.after, o.rig.before, "the repositories changed");
    let worktrees = git(&o.rig.clone, &["worktree", "list", "--porcelain"]);
    assert!(
        !worktrees.contains("loop-pr"),
        "a round worktree is left:\n{worktrees}"
    );
    let leftover = o.rig.clone.join(".claude/worktrees");
    let names: Vec<_> = std::fs::read_dir(&leftover)
        .map(|d| d.filter_map(Result::ok).map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(names.is_empty(), "round directories are left: {names:?}");
    for call in &o.claude_calls {
        assert_eq!(
            call.flag("--tools"),
            Some("Read,Grep,Glob"),
            "{:?}",
            call.argv
        );
        assert_eq!(
            call.flag_values("--allowedTools"),
            ["Read", "Grep", "Glob"],
            "{:?}",
            call.argv
        );
        for denied in ["Bash", "Edit", "Write", "Agent", "WebFetch", "WebSearch"] {
            assert!(
                call.flag_values("--disallowedTools").contains(&denied),
                "{denied} is not disallowed: {:?}",
                call.argv
            );
        }
    }
}
