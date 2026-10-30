//! The stage runner: one read-only `claude -p` call per stage.
//!
//! [`ClaudeCli`] runs `claude` under [`crate::proc::Supervisor`] (its own
//! process group, a time limit) with a cleared environment holding only
//! [`ENV_ALLOWLIST`]. The prompt goes in on stdin and stdout is kept in the
//! round's `round-01-<stage>.json`. The flags are pinned by the Task 1 probes
//! (D1 isolation, D2 `dontAsk`): the toolset (`--tools`) is the control, and
//! the deny list is a speed bump. The toolset has no Bash: an allowed `git
//! diff`, `git log` or `git show` can still write a file (`--output`), so the
//! git output a stage needs is gathered in Rust and put in its prompt.
//!
//! [`parse_envelope`] maps `claude --output-format json` output to a
//! [`StageResult`]; it is a pure function over the bytes, so the recorded
//! fixtures test it without a process.

use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::schema::{lens_schema, verify_schema, LensOutput, VerifyOutput, CATEGORY_TAXONOMY};
use crate::cmd::soak::signals::Interrupts;
use crate::proc::Supervisor;

/// The only variables a stage inherits.
pub const ENV_ALLOWLIST: [&str; 7] = [
    "PATH",
    "HOME",
    "USER",
    "LANG",
    "TERM",
    "TMPDIR",
    "CLAUDE_CONFIG_DIR",
];

/// SIGKILL follows SIGTERM by this long when a stage is over its time limit.
const KILL_AFTER: Duration = Duration::from_secs(10);

/// How often a running stage looks at the clock and at the terminal signals.
const POLL: Duration = Duration::from_millis(20);

/// What a stage's `--json-schema` and output check are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageKind {
    Lens,
    Verify,
}

impl StageKind {
    fn schema(self) -> Value {
        match self {
            StageKind::Lens => lens_schema(),
            StageKind::Verify => verify_schema(),
        }
    }
}

/// One stage call.
#[derive(Debug, Clone)]
pub struct StageCall {
    pub kind: StageKind,
    /// A model alias (`sonnet`, `opus`, `fable`).
    pub model: String,
    /// The prompt, sent on stdin.
    pub prompt: String,
    /// The round worktree: the stage's working directory.
    pub cwd: PathBuf,
    /// The materialised stage settings.
    pub settings: PathBuf,
    /// Where stdout is kept (`…/round-01-<stage>.json`). The prompt and stderr
    /// go next to it as `.prompt.md` and `.err`.
    pub out: PathBuf,
    /// `--max-budget-usd` of this call.
    pub per_call_usd: f64,
    pub timeout: Duration,
}

/// What one stage call came to.
#[derive(Debug, Clone, PartialEq)]
pub enum StageResult {
    /// The structured output, the call's cost, and the tool uses the
    /// permission mode denied (reported, not an error).
    Ok {
        value: Value,
        cost: f64,
        denials: Vec<String>,
    },
    /// No output: empty stdout, or a success without structured output.
    Empty,
    /// Anything else that went wrong, with a one-line reason.
    Error(String),
    /// The model refused the request.
    Refusal,
    /// The account's usage limit (or a 429).
    UsageLimit,
    /// No output, and the permission mode denied these tool uses.
    PermissionDenied(Vec<String>),
    /// The per-call budget ran out.
    OverBudget { cost: f64 },
    /// The stage ran out of time and was stopped.
    Timeout,
    /// A terminal signal arrived: the stage's process group was stopped (or
    /// never started). The value is the exit status the run ends with (129
    /// SIGHUP, 130 SIGINT, 131 SIGQUIT, 143 SIGTERM).
    Interrupted(u8),
}

/// Something that runs stages: [`ClaudeCli`], or a fake in a test. `Sync`
/// because the three lenses run on scoped threads.
pub trait Claude: Sync {
    fn run(&self, stage: &StageCall) -> StageResult;
}

/// The real runner.
#[derive(Clone)]
pub struct ClaudeCli {
    program: OsString,
    env: Vec<(&'static str, OsString)>,
    interrupts: Option<Arc<Interrupts>>,
}

impl std::fmt::Debug for ClaudeCli {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeCli")
            .field("program", &self.program)
            .field("env", &self.env)
            .field("interrupts", &self.interrupts.is_some())
            .finish()
    }
}

impl ClaudeCli {
    /// A runner for `claude` on PATH with the process environment.
    pub fn from_env() -> Result<ClaudeCli> {
        ClaudeCli::with_env("claude", |name| std::env::var_os(name))
    }

    /// A runner for `program`, taking the allowlisted variables from `get`.
    /// Fails without `CLAUDE_CONFIG_DIR`: a stage never runs on the default
    /// config (it may be another account).
    pub fn with_env(
        program: impl Into<OsString>,
        get: impl Fn(&str) -> Option<OsString>,
    ) -> Result<ClaudeCli> {
        if get("CLAUDE_CONFIG_DIR").is_none_or(|v| v.is_empty()) {
            bail!("CLAUDE_CONFIG_DIR unset; refusing the default config");
        }
        let env = ENV_ALLOWLIST
            .iter()
            .filter_map(|name| get(name).map(|value| (*name, value)))
            .collect();
        Ok(ClaudeCli {
            program: program.into(),
            env,
            interrupts: None,
        })
    }

    /// Watch `interrupts` while a stage runs: once a signal has arrived the
    /// stage's process group gets SIGTERM (SIGKILL after the grace period) and
    /// the call returns [`StageResult::Interrupted`]. A stage that has not
    /// started yet is not started.
    pub fn with_interrupts(mut self, interrupts: Arc<Interrupts>) -> ClaudeCli {
        self.interrupts = Some(interrupts);
        self
    }

    fn interrupted(&self) -> Option<u8> {
        self.interrupts.as_ref().and_then(|i| i.pending())
    }

    /// The arguments of `call`, in the pinned order.
    pub fn argv(&self, call: &StageCall) -> Vec<OsString> {
        let schema = serde_json::to_string(&call.kind.schema()).expect("a schema encodes");
        let usd = call.per_call_usd.to_string();
        let mut argv: Vec<OsString> = Vec::new();
        words(
            &mut argv,
            &["-p", "--output-format", "json", "--model", &call.model],
        );
        words(
            &mut argv,
            &["--json-schema", &schema, "--no-session-persistence"],
        );
        words(&mut argv, &["--max-budget-usd", &usd]);
        words(&mut argv, &["--restricted", "--disable-slash-commands"]);
        words(&mut argv, &["--permission-mode", "dontAsk", "--settings"]);
        argv.push(call.settings.clone().into_os_string());
        words(&mut argv, &["--tools", "Read,Grep,Glob"]);
        words(&mut argv, &["--allowedTools", "Read", "Grep", "Glob"]);
        words(
            &mut argv,
            &[
                "--disallowedTools",
                "Bash",
                "Edit",
                "Write",
                "Agent",
                "WebFetch",
                "WebSearch",
            ],
        );
        words(
            &mut argv,
            &[
                "--strict-mcp-config",
                "--mcp-config",
                r#"{"mcpServers":{}}"#,
            ],
        );
        argv
    }

    /// The command for `call`, without its stdio.
    pub fn command(&self, call: &StageCall) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(self.argv(call)).current_dir(&call.cwd).env_clear();
        for (name, value) in &self.env {
            cmd.env(name, value);
        }
        cmd
    }

    fn exec(&self, call: &StageCall) -> Result<StageResult> {
        if let Some(code) = self.interrupted() {
            return Ok(StageResult::Interrupted(code));
        }
        let dir = call
            .out
            .parent()
            .context("the stage output has no directory")?;
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let prompt = side_file(&call.out, "prompt.md");
        let err = side_file(&call.out, "err");
        std::fs::write(&prompt, &call.prompt)
            .with_context(|| format!("cannot write {}", prompt.display()))?;
        let open = |path: &Path| {
            File::create(path).with_context(|| format!("cannot create {}", path.display()))
        };
        let mut cmd = self.command(call);
        cmd.stdin(
            File::open(&prompt).with_context(|| format!("cannot open {}", prompt.display()))?,
        )
        .stdout(open(&call.out)?)
        .stderr(open(&err)?);
        let started = Instant::now();
        let mut child = Supervisor::spawn(cmd, call.timeout, KILL_AFTER)
            .with_context(|| format!("cannot start {}", self.program.to_string_lossy()))?;
        child.warn_as("aios loop");
        let code = loop {
            child.service()?;
            if let Some(code) = child.exit_code() {
                break code;
            }
            if let Some(signal) = self.interrupted() {
                child.terminate()?;
                child.wait()?;
                return Ok(StageResult::Interrupted(signal));
            }
            std::thread::sleep(POLL);
        };
        // 124 and 137 are what Supervisor reports after its limit; a child
        // that died of SIGKILL on its own also reads 137, so the clock decides.
        if matches!(code, 124 | 137) && started.elapsed() >= call.timeout {
            return Ok(StageResult::Timeout);
        }
        let stdout = std::fs::read(&call.out)
            .with_context(|| format!("cannot read {}", call.out.display()))?;
        let parsed = parse_envelope(&stdout);
        if parsed == StageResult::Empty && code != 0 {
            let stderr = std::fs::read(&err).unwrap_or_default();
            let line = String::from_utf8_lossy(stderr.split(|&b| b == b'\n').next().unwrap_or(&[]))
                .into_owned();
            return Ok(StageResult::Error(format!("claude exited {code}: {line}")));
        }
        Ok(parsed)
    }
}

impl Claude for ClaudeCli {
    fn run(&self, call: &StageCall) -> StageResult {
        match self.exec(call) {
            Ok(result) => validate(call.kind, result),
            Err(e) => StageResult::Error(format!("{e:#}")),
        }
    }
}

fn words(argv: &mut Vec<OsString>, ws: &[&str]) {
    argv.extend(ws.iter().map(OsString::from));
}

/// `round-01-lens.json` with its extension replaced by `ext`.
fn side_file(out: &Path, ext: &str) -> PathBuf {
    out.with_extension(ext)
}

/// Check a successful result against the stage's schema types: the shape, and
/// for a lens that every category is in the taxonomy. Any other result passes
/// through.
pub fn validate(kind: StageKind, result: StageResult) -> StageResult {
    let StageResult::Ok { value, .. } = &result else {
        return result;
    };
    match check_shape(kind, value) {
        Ok(()) => result,
        Err(reason) => StageResult::Error(reason),
    }
}

fn check_shape(kind: StageKind, value: &Value) -> std::result::Result<(), String> {
    match kind {
        StageKind::Lens => {
            let out: LensOutput = serde_json::from_value(value.clone())
                .map_err(|e| format!("the lens output does not match its schema: {e}"))?;
            for finding in &out.findings {
                if !CATEGORY_TAXONOMY.contains(&finding.category.as_str()) {
                    return Err(format!(
                        "finding {}: category {:?} is not in the taxonomy",
                        finding.id, finding.category
                    ));
                }
            }
            Ok(())
        }
        StageKind::Verify => serde_json::from_value::<VerifyOutput>(value.clone())
            .map(|_| ())
            .map_err(|e| format!("the verify output does not match its schema: {e}")),
    }
}

/// Map the stdout of `claude -p --output-format json` to a [`StageResult`].
///
/// The CLI prints either one result object or an array of events; for an array
/// the last element with `type == "result"` is the result. Mapping:
/// - `error_max_budget_usd` is [`StageResult::OverBudget`];
/// - a success with `structured_output` is [`StageResult::Ok`], its
///   `permission_denials` kept alongside;
/// - an error whose text or `api_error_status` says the account's usage limit
///   (or a 429) is [`StageResult::UsageLimit`];
/// - `stop_reason == "refusal"`, or a refusal in `terminal_reason` or
///   `subtype`, is [`StageResult::Refusal`];
/// - a success without output is [`StageResult::PermissionDenied`] when
///   something was denied, else [`StageResult::Empty`];
/// - every other error subtype (and `is_error` on a success) is
///   [`StageResult::Error`].
pub fn parse_envelope(stdout: &[u8]) -> StageResult {
    if stdout.iter().all(u8::is_ascii_whitespace) {
        return StageResult::Empty;
    }
    let value: Value = match serde_json::from_slice(stdout) {
        Ok(v) => v,
        Err(e) => return StageResult::Error(format!("claude printed invalid JSON: {e}")),
    };
    let result = match &value {
        Value::Array(events) => events.iter().rev().find(|e| e["type"] == "result"),
        Value::Object(_) if value["type"] == "result" => Some(&value),
        _ => None,
    };
    match result {
        Some(result) => classify(result),
        None => StageResult::Error("claude printed no result event".to_string()),
    }
}

fn classify(r: &Value) -> StageResult {
    let cost = r["total_cost_usd"].as_f64().unwrap_or(0.0);
    let subtype = r["subtype"].as_str().unwrap_or("");
    let is_error = r["is_error"].as_bool().unwrap_or(false);
    let denials = denials_of(r);
    if subtype == "error_max_budget_usd" {
        return StageResult::OverBudget { cost };
    }
    let success = subtype == "success" && !is_error;
    if success && !r["structured_output"].is_null() {
        return StageResult::Ok {
            value: r["structured_output"].clone(),
            cost,
            denials,
        };
    }
    if is_usage_limit(r, subtype, is_error) {
        return StageResult::UsageLimit;
    }
    if is_refusal(r, subtype) {
        return StageResult::Refusal;
    }
    if success {
        return if denials.is_empty() {
            StageResult::Empty
        } else {
            StageResult::PermissionDenied(denials)
        };
    }
    StageResult::Error(error_line(r, subtype))
}

/// The text fields that can say why a call failed, lower-cased.
fn failure_text(r: &Value, subtype: &str, is_error: bool) -> String {
    let mut text = vec![subtype.to_string()];
    if let Some(reason) = r["terminal_reason"].as_str() {
        text.push(reason.to_string());
    }
    for e in r["errors"].as_array().into_iter().flatten() {
        if let Some(e) = e.as_str() {
            text.push(e.to_string());
        }
    }
    if is_error {
        if let Some(result) = r["result"].as_str() {
            text.push(result.to_string());
        }
    }
    text.join("\n").to_lowercase()
}

fn is_usage_limit(r: &Value, subtype: &str, is_error: bool) -> bool {
    if r["api_error_status"].as_u64() == Some(429) {
        return true;
    }
    let text = failure_text(r, subtype, is_error);
    ["usage limit", "hit your limit", "limit reached"]
        .iter()
        .any(|needle| text.contains(needle))
}

fn is_refusal(r: &Value, subtype: &str) -> bool {
    r["stop_reason"].as_str() == Some("refusal")
        || r["terminal_reason"]
            .as_str()
            .is_some_and(|t| t.to_lowercase().contains("refus"))
        || subtype.to_lowercase().contains("refus")
}

/// `claude: <subtype>: <first error or result text>`, at most 200 characters
/// of detail.
fn error_line(r: &Value, subtype: &str) -> String {
    let detail = r["errors"]
        .as_array()
        .and_then(|e| e.first())
        .and_then(Value::as_str)
        .or_else(|| r["result"].as_str())
        .unwrap_or("");
    let detail: String = detail
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(200)
        .collect();
    format!("claude: {subtype}: {detail}")
}

/// `Tool: command` for each denied tool use.
fn denials_of(r: &Value) -> Vec<String> {
    r["permission_denials"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| {
            let tool = d["tool_name"].as_str().unwrap_or("?");
            let input = &d["tool_input"];
            let what = input["command"]
                .as_str()
                .map_or_else(|| input.to_string(), str::to_string);
            format!("{tool}: {what}")
        })
        .collect()
}
