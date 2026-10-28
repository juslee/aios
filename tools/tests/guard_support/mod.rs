#![allow(dead_code)]
//! The guard parity fixture (R5, plan step 2): replays the scenarios in
//! `tests/fixtures/guard/cases.jsonl` and runs a guard on each check.
//!
//! A scenario is one Python guard unit test. Its `setup` names the fixture the
//! test class builds (`repo`: a clone with `main` and `claude/x` pushed to a bare
//! origin; `team`: that plus the three worktrees of `TeamCase`), and its `steps`
//! are, in the test's own order, filesystem and git operations and checks.
//! A check is a hook call: a Bash payload (`bash`), an Agent, Task or
//! EnterWorktree payload (`tool`), or raw hook stdin (`stdin`), each with the
//! environment it adds and the decision the Python test saw (`expect`).
//! Paths are written `{ROOT}` (the scenario's temporary directory) and `{REPO}`
//! (this checkout).
//!
//! The root is under the system temporary directory, never under the
//! checkout: the guard treats paths containing `.claude` specially, and rule 03
//! puts every checkout under `.claude/worktrees/`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Map, Value};

/// The interpreter the PreToolUse hook entry in `.claude/settings.json` names.
pub const PYTHON: &str = "/usr/bin/python3";

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// The repository root this test binary was built from.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tools/ lives in the repository root")
        .to_path_buf()
}

pub fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/guard/cases.jsonl")
}

/// The shell parse table (plan step 3): one `{text, depth, commands | error}`
/// object per line, recorded from the Python guard's `parse_commands`.
pub fn shell_fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/guard/shell.jsonl")
}

pub fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/guard")
}

/// One Python unit test, as replayable steps.
pub struct Scenario {
    /// `Class.test_name`.
    pub id: String,
    pub setup: String,
    pub steps: Vec<Value>,
}

impl Scenario {
    pub fn class(&self) -> &str {
        self.id.split('.').next().expect("an id has a class")
    }

    pub fn method(&self) -> &str {
        self.id.split_once('.').expect("an id is Class.method").1
    }

    /// The checks, in order.
    pub fn checks(&self) -> impl Iterator<Item = &Value> {
        self.steps.iter().filter(|s| s.get("op").is_none())
    }
}

pub fn scenarios() -> Vec<Scenario> {
    let text = std::fs::read_to_string(fixture_path()).expect("read cases.jsonl");
    text.lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let v: Value = serde_json::from_str(line).expect("a JSON scenario");
            Scenario {
                id: v["id"].as_str().expect("an id").to_string(),
                setup: v["setup"].as_str().expect("a setup").to_string(),
                steps: v["steps"].as_array().expect("steps").clone(),
            }
        })
        .collect()
}

/// What one guard run printed and returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub exit: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Outcome {
    /// The decision: `none` when the hook printed nothing.
    pub fn level(&self) -> String {
        if self.stdout.is_empty() {
            return "none".to_string();
        }
        let v: Value = match serde_json::from_str(&self.stdout) {
            Ok(v) => v,
            Err(_) => return format!("unparsable stdout: {}", self.stdout),
        };
        v["hookSpecificOutput"]["permissionDecision"]
            .as_str()
            .unwrap_or("missing decision")
            .to_string()
    }

    pub fn to_json(&self) -> Value {
        json!({"exit": self.exit, "stdout": self.stdout, "stderr": self.stderr})
    }

    pub fn from_json(v: &Value) -> Outcome {
        Outcome {
            exit: v["exit"].as_i64().expect("exit") as i32,
            stdout: v["stdout"].as_str().expect("stdout").to_string(),
            stderr: v["stderr"].as_str().expect("stderr").to_string(),
        }
    }
}

/// The scenario's temporary directory, removed when dropped.
pub struct Root {
    path: PathBuf,
    repo: PathBuf,
}

impl Root {
    pub fn new(label: &str) -> Root {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base =
            std::env::temp_dir().join(format!("aios-guard-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("create the scenario root");
        // The guard resolves paths, so every spelling it prints is the real one.
        let path = base.canonicalize().expect("canonical root");
        std::fs::create_dir_all(path.join("home")).expect("create HOME");
        Root {
            path,
            repo: repo_root(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `{ROOT}` and `{REPO}` replaced.
    pub fn expand(&self, text: &str) -> String {
        text.replace("{ROOT}", self.path.to_str().expect("UTF-8 root"))
            .replace("{REPO}", self.repo.to_str().expect("UTF-8 checkout"))
    }

    /// The inverse of `expand`, for anything a guard printed.
    pub fn normalize(&self, text: &str) -> String {
        text.replace(self.path.to_str().expect("UTF-8 root"), "{ROOT}")
            .replace(self.repo.to_str().expect("UTF-8 checkout"), "{REPO}")
    }

    fn expand_value(&self, v: &Value) -> Value {
        match v {
            Value::String(s) => Value::String(self.expand(s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.expand_value(x)).collect()),
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, x)| (self.expand(k), self.expand_value(x)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// The environment every process in a scenario starts from: the Python
    /// tests' (`GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`), with
    /// HOME inside the root and no ambient git, Claude Code or team variables.
    pub fn base_env(&self, cmd: &mut Command) {
        cmd.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("LANG", "C.UTF-8")
            .env("HOME", self.path.join("home"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn git(root: &Root, cwd: &Path, args: &[String]) {
    let out = git_output(root, cwd, args);
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `git-fail`: a git command the scenario expects to stop with an error, such
/// as a rebase that stops on a conflict.
fn git_failing(root: &Root, cwd: &Path, args: &[String]) {
    let out = git_output(root, cwd, args);
    assert!(
        !out.status.success(),
        "git {args:?} in {} was expected to fail",
        cwd.display()
    );
}

fn git_output(root: &Root, cwd: &Path, args: &[String]) -> std::process::Output {
    let mut cmd = Command::new("git");
    root.base_env(&mut cmd);
    cmd.env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .args(args)
        .current_dir(cwd);
    cmd.output().expect("run git")
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .expect("an array")
        .iter()
        .map(|s| s.as_str().expect("a string").to_string())
        .collect()
}

/// `RepoCase.setUp` and `TeamCase.setUp`, as the operations they run.
fn setup_ops(name: &str) -> Vec<Value> {
    let g = |cwd: &str, args: &[&str]| json!({"op": "git", "cwd": cwd, "args": args});
    let mut ops = vec![
        g(
            "{ROOT}",
            &["init", "-q", "--bare", "-b", "main", "{ROOT}/origin.git"],
        ),
        g("{ROOT}", &["init", "-q", "-b", "main", "{ROOT}/work"]),
        g(
            "{ROOT}/work",
            &["remote", "add", "origin", "{ROOT}/origin.git"],
        ),
        g(
            "{ROOT}/work",
            &["commit", "-q", "--allow-empty", "-m", "base"],
        ),
        g("{ROOT}/work", &["push", "-q", "-u", "origin", "main"]),
        g(
            "{ROOT}/work",
            &["switch", "-q", "-c", "claude/x", "--track", "origin/main"],
        ),
        g(
            "{ROOT}/work",
            &["commit", "-q", "--allow-empty", "-m", "work"],
        ),
        // The guard only reads config, so origin can name GitHub with no network.
        g(
            "{ROOT}/work",
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/juslee/aios.git",
            ],
        ),
        g(
            "{ROOT}/work",
            &["remote", "add", "scratch", "{ROOT}/origin.git"],
        ),
    ];
    match name {
        "repo" => {}
        "team" => ops.extend([
            g(
                "{ROOT}/work",
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    "worktree-agent-x",
                    "{ROOT}/work/.claude/worktrees/agent-x",
                ],
            ),
            g(
                "{ROOT}/work",
                &["worktree", "add", "-q", "-b", "claude/w", "{ROOT}/W"],
            ),
            g(
                "{ROOT}/work",
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    "not-agent-y",
                    "{ROOT}/work/.claude/worktrees/agent-y",
                ],
            ),
        ]),
        other => panic!("unknown setup {other}"),
    }
    ops
}

/// Apply one operation step.
fn apply(root: &Root, op: &Value) {
    let path = |key: &str| PathBuf::from(root.expand(op[key].as_str().expect("a path")));
    match op["op"].as_str().expect("an op") {
        "git" => {
            let args: Vec<String> = strings(&op["args"])
                .iter()
                .map(|a| root.expand(a))
                .collect();
            git(root, &path("cwd"), &args);
        }
        "git-fail" => {
            let args: Vec<String> = strings(&op["args"])
                .iter()
                .map(|a| root.expand(a))
                .collect();
            git_failing(root, &path("cwd"), &args);
        }
        "write" => {
            let p = path("path");
            let data = match (op.get("text"), op.get("base64")) {
                (Some(t), None) => root.expand(t.as_str().expect("text")).into_bytes(),
                (None, Some(b)) => decode_base64(b.as_str().expect("base64")),
                _ => panic!("a write has text or base64: {op}"),
            };
            if op.get("append") == Some(&Value::Bool(true)) {
                let mut fh = std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(&p)
                    .expect("open for append");
                fh.write_all(&data).expect("append");
            } else {
                std::fs::write(&p, data).expect("write");
            }
        }
        "copy" => {
            std::fs::copy(path("from"), path("path")).expect("copy");
        }
        "mkdir" => {
            let p = path("path");
            if op["parents"] == Value::Bool(true) {
                std::fs::create_dir_all(&p).expect("mkdir -p");
            } else {
                std::fs::create_dir(&p).expect("mkdir");
            }
        }
        "chmod" => {
            use std::os::unix::fs::PermissionsExt;
            let mode = op["mode"].as_u64().expect("a mode") as u32;
            std::fs::set_permissions(path("path"), std::fs::Permissions::from_mode(mode))
                .expect("chmod");
        }
        "symlink" => {
            std::os::unix::fs::symlink(path("target"), path("path")).expect("symlink");
        }
        "remove" => std::fs::remove_file(path("path")).expect("remove"),
        "rmdir" => std::fs::remove_dir(path("path")).expect("rmdir"),
        other => panic!("unknown op {other}"),
    }
}

fn decode_base64(s: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes().filter(|&c| c != b'=') {
        let v = ALPHABET.iter().position(|&a| a == c).expect("base64") as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

/// The tests' `too_deep()`: `command` under `times` levels of `$( )` or `sh -c`.
pub fn wrap(mut command: String, form: &str, times: u64) -> String {
    for _ in 0..times {
        command = match form {
            "subst" => format!("echo $({command})"),
            "shc" => format!("sh -c '{}'", command.replace('\'', "'\\''")),
            other => panic!("unknown wrap form {other}"),
        };
    }
    command
}

/// A check's hook stdin and the environment it adds.
pub fn hook_input(root: &Root, check: &Value) -> (Vec<u8>, Vec<(String, String)>) {
    let env: Vec<(String, String)> = check
        .get("env")
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .map(|(k, v)| (k.clone(), root.expand(v.as_str().expect("an env value"))))
                .collect()
        })
        .unwrap_or_default();
    let stdin = if let Some(command) = check.get("bash") {
        let mut command = command.as_str().expect("a command").to_string();
        if let Some(w) = check.get("wrap") {
            command = wrap(
                command,
                w["form"].as_str().expect("a form"),
                w["times"].as_u64().expect("times"),
            );
        }
        let mut payload = Map::new();
        payload.insert("tool_name".into(), json!("Bash"));
        payload.insert(
            "tool_input".into(),
            json!({"command": root.expand(&command)}),
        );
        let cwd = check
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or("{ROOT}/work");
        payload.insert("cwd".into(), json!(root.expand(cwd)));
        payload.insert("hook_event_name".into(), json!("PreToolUse"));
        for key in ["agent_id", "agent_type"] {
            if let Some(v) = check.get(key) {
                payload.insert(key.into(), root.expand_value(v));
            }
        }
        Value::Object(payload).to_string().into_bytes()
    } else if let Some(tool) = check.get("tool") {
        root.expand_value(tool).to_string().into_bytes()
    } else {
        root.expand(check["stdin"].as_str().expect("raw stdin"))
            .into_bytes()
    };
    (stdin, env)
}

/// Run a guard command (`argv`) on one check.
pub fn run_guard(root: &Root, argv: &[&str], check: &Value) -> Outcome {
    let (stdin, env) = hook_input(root, check);
    let mut cmd = Command::new(argv[0]);
    root.base_env(&mut cmd);
    cmd.args(&argv[1..])
        .envs(env)
        .current_dir(root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("start the guard");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(&stdin)
        .expect("write the payload");
    let out = child.wait_with_output().expect("wait for the guard");
    Outcome {
        exit: out.status.code().unwrap_or(-1),
        stdout: root.normalize(&String::from_utf8_lossy(&out.stdout)),
        stderr: root.normalize(&String::from_utf8_lossy(&out.stderr)),
    }
}

/// The Python guard of this checkout, as the hook entry runs it.
pub fn python_guard() -> Vec<String> {
    vec![
        PYTHON.to_string(),
        repo_root()
            .join(".claude/hooks/git-push-guard.py")
            .to_str()
            .expect("UTF-8 path")
            .to_string(),
    ]
}

/// Replay `scenario`, running `argv` on every check; one outcome per check.
pub fn replay(scenario: &Scenario, argv: &[&str]) -> Vec<Outcome> {
    let root = Root::new(scenario.class());
    for op in setup_ops(&scenario.setup) {
        apply(&root, &op);
    }
    let mut outcomes = Vec::new();
    for step in &scenario.steps {
        if step.get("op").is_some() {
            apply(&root, step);
        } else {
            outcomes.push(run_guard(&root, argv, step));
        }
    }
    outcomes
}

/// `tests/golden/guard/<Class>.json`: method -> outcomes, in check order.
pub fn golden_path(class: &str) -> PathBuf {
    golden_dir().join(format!("{class}.json"))
}

pub fn read_goldens(class: &str) -> Result<Map<String, Value>, String> {
    let path = golden_path(class);
    let text = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "{}: {e} (record it: AIOS_BLESS_GOLDENS=1 cargo test -p aios-tools --test \
             guard_parity -- --ignored record)",
            path.display()
        )
    })?;
    match serde_json::from_str(&text) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(format!("{}: not a JSON object", path.display())),
    }
}

/// Every Bash command the scenarios run, wrapped as the hook receives it, in
/// fixture order and without repeats.
pub fn bash_commands(all: &[Scenario]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for s in all {
        for check in s.checks() {
            let Some(command) = check.get("bash").and_then(Value::as_str) else {
                continue;
            };
            let mut command = command.to_string();
            if let Some(w) = check.get("wrap") {
                command = wrap(
                    command,
                    w["form"].as_str().expect("a form"),
                    w["times"].as_u64().expect("times"),
                );
            }
            if seen.insert(command.clone()) {
                out.push(command);
            }
        }
    }
    out
}

/// Run the Python guard's own `parse_commands` on each `(text, depth)`: one JSON
/// line per input, `{text, depth, commands}` or `{text, depth, error}` (the
/// `ParseError` message).
pub fn python_parse(inputs: &[(String, usize)]) -> Vec<String> {
    const SCRIPT: &str = r#"
import importlib.util, json, sys
spec = importlib.util.spec_from_file_location("guard", sys.argv[1])
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)
for text, depth in json.load(sys.stdin):
    case = {"text": text, "depth": depth}
    try:
        case["commands"] = [
            {"words": c.words, "expanded": c.expanded, "heredocs": c.heredocs,
             "stdin_files": c.stdin_files, "out_files": c.out_files,
             "piped_in": c.piped_in, "src": c.src, "bodies": c.bodies}
            for c in guard.parse_commands(text, depth)]
    except guard.ParseError as exc:
        case["error"] = str(exc)
    print(json.dumps(case, ensure_ascii=False, sort_keys=True))
"#;
    let guard = repo_root().join(".claude/hooks/git-push-guard.py");
    let mut child = Command::new(PYTHON)
        .arg("-I")
        .arg("-c")
        .arg(SCRIPT)
        .arg(&guard)
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start python");
    let payload = serde_json::to_vec(inputs).expect("serialise the inputs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(&payload)
        .expect("write the inputs");
    let out = child.wait_with_output().expect("wait for python");
    assert!(out.status.success(), "python parse_commands failed");
    String::from_utf8(out.stdout)
        .expect("UTF-8 output")
        .lines()
        .map(str::to_string)
        .collect()
}
