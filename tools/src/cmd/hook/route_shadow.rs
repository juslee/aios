//! `aios hook route-shadow`: ask Jev how a subagent dispatch would be routed and
//! log the answer next to what actually happened. Registered on `PreToolUse` for
//! the dispatch tool (`Agent`). Never decides anything and never prints; fails
//! open (see `OnError`).
//!
//! The call goes through the system `curl`, behind the `Transport` trait so unit
//! tests need no network. The API key reaches curl only on its stdin (`-K -`), so
//! it is in no argv, no log record and no diagnostic.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::Args as ClapArgs;
use serde::Serialize;
use serde_json::{json, Value};

use super::{append_jsonl, cut_to_boundary, unix_seconds, Ctx, HookInput};

/// The Jev model is pinned: its answer thresholds are tuned per version.
const JEV_MODEL: &str = "jev-1.13.0";
const DEFAULT_URL: &str = "https://api.typesafe.ai/v1/systemone";
/// The prompt sent to Jev and logged is cut to this many characters.
const MAX_PROMPT_CHARS: usize = 8000;
/// curl's whole-request limit in seconds; part 2 registers the hook with 6.
const CURL_MAX_TIME: &str = "4";
/// A response body quoted in an error record is cut to this many bytes.
const MAX_QUOTED_BYTES: usize = 200;
const LOG_FILE: &str = "route-shadow.jsonl";

#[derive(ClapArgs)]
pub struct Args {}

/// The dispatch fields the hook reads; all are optional in the payload.
#[derive(Debug, PartialEq, Eq)]
pub struct Dispatch<'a> {
    pub subagent_type: Option<&'a str>,
    pub description: Option<&'a str>,
    pub prompt: Option<&'a str>,
    pub model: Option<&'a str>,
}

impl<'a> Dispatch<'a> {
    pub fn from_input(input: &'a HookInput) -> Dispatch<'a> {
        Dispatch {
            subagent_type: input.tool_str("subagent_type"),
            description: input.tool_str("description"),
            prompt: input.tool_str("prompt"),
            model: input.tool_str("model"),
        }
    }
}

/// The environment of the hook.
pub struct Settings {
    /// `AIOS_ROUTE_SHADOW=off`: do nothing at all.
    pub off: bool,
    /// `AIOS_JEV_URL`, else the TypeSafe endpoint.
    pub url: String,
    /// `TYPESAFE_API_KEY`; an empty value counts as unset.
    pub api_key: Option<String>,
}

impl Settings {
    pub fn from_env() -> Settings {
        let var = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        Settings {
            off: var("AIOS_ROUTE_SHADOW").is_some_and(|v| v.eq_ignore_ascii_case("off")),
            url: var("AIOS_JEV_URL").unwrap_or_else(|| DEFAULT_URL.to_string()),
            api_key: var("TYPESAFE_API_KEY"),
        }
    }
}

/// Sends one Jev request and returns the response body.
pub trait Transport {
    /// POST `body` as JSON to `url` with `api_key` as a bearer token. `scratch` is
    /// a directory the transport may keep a temporary file in.
    fn post(&self, url: &str, api_key: &str, body: &[u8], scratch: &Path) -> Result<Vec<u8>>;
}

/// `curl -sS --fail-with-body --max-time 4 -X POST <url> -H 'Content-Type:
/// application/json' -K - --data-binary @<body file>`, with the Authorization
/// header as a config line on stdin and the body in a 0600 file removed
/// afterwards. `--fail-with-body` makes an HTTP error status a curl failure
/// while keeping the body for the diagnostic.
pub struct CurlTransport;

impl Transport for CurlTransport {
    fn post(&self, url: &str, api_key: &str, body: &[u8], scratch: &Path) -> Result<Vec<u8>> {
        let config = curl_auth_config(api_key)?;
        let file = BodyFile::create(scratch, body)?;
        let mut child = Command::new("curl")
            .args(["-sS", "--fail-with-body", "--max-time", CURL_MAX_TIME])
            .args(["-X", "POST", url])
            .args(["-H", "Content-Type: application/json", "-K", "-"])
            .arg("--data-binary")
            .arg(format!("@{}", file.path.display()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("cannot run curl")?;
        let mut stdin = child.stdin.take().context("curl has no stdin")?;
        // A failed write means curl already exited; its status explains why.
        let _ = stdin.write_all(config.as_bytes());
        drop(stdin);
        let out = child.wait_with_output().context("cannot wait for curl")?;
        if !out.status.success() {
            // Scrub before cutting: a cut inside an echoed key would leave a
            // fragment that no later scrub recognises.
            let stderr = scrub(&String::from_utf8_lossy(&out.stderr), api_key);
            let reply = scrub(&String::from_utf8_lossy(&out.stdout), api_key);
            let mut message = format!("curl failed ({}): {}", out.status, stderr.trim());
            if !reply.trim().is_empty() {
                message.push_str("; response: ");
                message.push_str(cut_to_boundary(reply.trim(), MAX_QUOTED_BYTES));
            }
            bail!("{message}");
        }
        Ok(out.stdout)
    }
}

/// `text` with every occurrence of `key` replaced. Callers scrub before they cut
/// text to a length, because a cut inside the key leaves a fragment that no scrub
/// matches.
fn scrub(text: &str, key: &str) -> String {
    text.replace(key, "[redacted]")
}

/// The `-K -` config line that carries the bearer token. curl reads a quoted value
/// with backslash escapes, so `\` and `"` are escaped; a control character could
/// end the line early and is refused.
fn curl_auth_config(api_key: &str) -> Result<String> {
    if api_key.chars().any(char::is_control) {
        bail!("TYPESAFE_API_KEY contains a control character");
    }
    let escaped = api_key.replace('\\', "\\\\").replace('"', "\\\"");
    Ok(format!("header = \"Authorization: Bearer {escaped}\"\n"))
}

/// The request body on disk, mode 0600, removed on drop.
struct BodyFile {
    path: PathBuf,
}

impl BodyFile {
    fn create(dir: &Path, body: &[u8]) -> Result<BodyFile> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let path = dir.join(format!(
            ".route-shadow-{}-{}.body",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let file = BodyFile { path };
        let mut handle = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&file.path)
            .with_context(|| format!("cannot create {}", file.path.display()))?;
        handle
            .write_all(body)
            .with_context(|| format!("cannot write {}", file.path.display()))?;
        Ok(file)
    }
}

impl Drop for BodyFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The Jev request for one dispatch, and what was cut from the prompt.
pub struct Request {
    pub body: Value,
    /// The prompt as sent and logged.
    pub prompt: Option<String>,
    pub prompt_chars: usize,
    pub prompt_truncated: bool,
}

pub fn build_request(dispatch: &Dispatch) -> Request {
    let prompt_chars = dispatch.prompt.map_or(0, |p| p.chars().count());
    let prompt_truncated = prompt_chars > MAX_PROMPT_CHARS;
    let prompt = dispatch
        .prompt
        .map(|p| match p.char_indices().nth(MAX_PROMPT_CHARS) {
            Some((end, _)) => p[..end].to_string(),
            None => p.to_string(),
        });
    let body = json!({
        "model": JEV_MODEL,
        "state": {
            "subagent_type": dispatch.subagent_type,
            "description": dispatch.description,
            "prompt": prompt,
        },
        "questions": {
            "complexity": {
                "type": "score",
                "instructions": "How much reasoning does carrying out `prompt` need?",
                "criteria": [
                    "Mechanical: a lookup, a search with a clear target, a rename, formatting, or wording.",
                    "Routine: a change or check that follows an existing pattern in one area.",
                    "Hard: debugging with an unclear cause, a change across interacting parts, or a choice between designs.",
                ],
            },
            "low_level_hazard": {
                "type": "noul",
                "instructions": "Does `prompt` ask for changes to, or diagnosis of, code that involves concurrency, memory ordering, page tables or the MMU, interrupt or exception handling, lock ordering, or boot sequencing?",
            },
            "work_kind": {
                "type": "choice",
                "instructions": "What kind of work does `prompt` ask for?",
                "criteria": {
                    "read_only_search": "find or read code or docs without changing them",
                    "implement_change": "write or modify code, tests or configuration",
                    "review_or_audit": "judge existing work for defects or compliance",
                    "run_and_verify": "build, test, boot or measure and report the result",
                    "write_docs": "write or edit prose documentation",
                    "research_external": "gather information from outside the repository",
                    "other": "none of these",
                },
            },
        },
    });
    Request {
        body,
        prompt,
        prompt_chars,
        prompt_truncated,
    }
}

/// One line of `route-shadow.jsonl`.
#[derive(Serialize)]
struct Record<'a> {
    /// Unix seconds.
    ts: u64,
    session_id: Option<&'a str>,
    agent_id: Option<&'a str>,
    /// The `tool_use_id` of the dispatch; it joins the record to the dispatch's
    /// outcome later.
    tool_use_id: Option<&'a str>,
    subagent_type: Option<&'a str>,
    /// The model the dispatch asked for, not Jev's.
    model: Option<&'a str>,
    description: Option<&'a str>,
    /// The prompt as sent to Jev, possibly cut.
    prompt: Option<&'a str>,
    prompt_chars: usize,
    prompt_truncated: bool,
    jev_model: Option<String>,
    latency_ms: Option<u64>,
    /// The response's `answers` object, parsed and re-serialised with key order kept
    /// (number formatting is normalised, so `0.50` is logged as `0.5`).
    answers: Option<Value>,
    error: Option<String>,
}

/// What asking Jev produced.
struct Outcome {
    jev_model: Option<String>,
    latency_ms: Option<u64>,
    answers: Option<Value>,
    error: Option<String>,
}

impl Outcome {
    fn failed(message: String, latency_ms: Option<u64>) -> Outcome {
        Outcome {
            jev_model: None,
            latency_ms,
            answers: None,
            error: Some(message),
        }
    }
}

/// Send `request` and read the answers out of the response. Any failure is an
/// outcome with an error, never an `Err`, so it is logged like a success. The key
/// is scrubbed from every message.
fn ask(
    transport: &dyn Transport,
    settings: &Settings,
    request: &Request,
    scratch: &Path,
) -> Outcome {
    let Some(key) = settings.api_key.as_deref() else {
        return Outcome::failed("TYPESAFE_API_KEY is not set".to_string(), None);
    };
    let body = request.body.to_string();
    let started = Instant::now();
    let reply = transport.post(&settings.url, key, body.as_bytes(), scratch);
    let latency = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let raw = match reply {
        Ok(raw) => raw,
        Err(err) => return Outcome::failed(scrub(&format!("{err:#}"), key), Some(latency)),
    };
    let parsed: Value = match serde_json::from_slice(&raw) {
        Ok(value) => value,
        Err(err) => {
            let text = scrub(&String::from_utf8_lossy(&raw), key);
            let quoted = cut_to_boundary(text.trim(), MAX_QUOTED_BYTES);
            let message = format!("the Jev response is not JSON ({err}): {quoted}");
            return Outcome::failed(scrub(&message, key), Some(latency));
        }
    };
    match parsed.get("answers") {
        Some(answers) if answers.is_object() => Outcome {
            jev_model: parsed
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string),
            latency_ms: Some(latency),
            answers: Some(answers.clone()),
            error: None,
        },
        _ => {
            let text = scrub(&parsed.to_string(), key);
            let quoted = cut_to_boundary(&text, MAX_QUOTED_BYTES);
            let message = format!("the Jev response has no answers object: {quoted}");
            Outcome::failed(scrub(&message, key), Some(latency))
        }
    }
}

pub fn run(_args: &Args, input: &HookInput, ctx: &Ctx) -> Result<Option<String>> {
    run_with(input, ctx, &Settings::from_env(), &CurlTransport)
}

/// `run` with its environment and transport passed in.
pub fn run_with(
    input: &HookInput,
    ctx: &Ctx,
    settings: &Settings,
    transport: &dyn Transport,
) -> Result<Option<String>> {
    if settings.off {
        return Ok(None);
    }
    let dir = ctx.state_dir(input)?;
    let dispatch = Dispatch::from_input(input);
    let request = build_request(&dispatch);
    let outcome = ask(transport, settings, &request, &dir);
    let record = Record {
        ts: unix_seconds(),
        session_id: input.session_id.as_deref(),
        agent_id: input.agent_id.as_deref(),
        tool_use_id: input.tool_use_id.as_deref(),
        subagent_type: dispatch.subagent_type,
        model: dispatch.model,
        description: dispatch.description,
        prompt: request.prompt.as_deref(),
        prompt_chars: request.prompt_chars,
        prompt_truncated: request.prompt_truncated,
        jev_model: outcome.jev_model,
        latency_ms: outcome.latency_ms,
        answers: outcome.answers,
        error: outcome.error,
    };
    append_jsonl(&dir.join(LOG_FILE), &record)?;
    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::cmd::hook::parse_input;

    const KEY: &str = "sk-unit-SECRET";

    /// A transport that records its calls and replays one canned reply.
    struct Fake {
        reply: Result<String, String>,
        calls: RefCell<Vec<(String, String, Value)>>,
    }

    impl Fake {
        fn replying(reply: &str) -> Fake {
            Fake {
                reply: Ok(reply.to_string()),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn failing(message: &str) -> Fake {
            Fake {
                reply: Err(message.to_string()),
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl Transport for Fake {
        fn post(&self, url: &str, api_key: &str, body: &[u8], _scratch: &Path) -> Result<Vec<u8>> {
            self.calls.borrow_mut().push((
                url.to_string(),
                api_key.to_string(),
                serde_json::from_slice(body).expect("the request body is JSON"),
            ));
            match &self.reply {
                Ok(text) => Ok(text.clone().into_bytes()),
                Err(message) => bail!("{message}"),
            }
        }
    }

    fn settings(key: Option<&str>) -> Settings {
        Settings {
            off: false,
            url: "http://jev.test/v1/systemone".to_string(),
            api_key: key.map(str::to_string),
        }
    }

    fn state_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "aios-route-shadow-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the state directory");
        dir
    }

    fn payload(prompt: &str) -> HookInput {
        let payload = json!({
            "session_id": "s1",
            "agent_id": "a9",
            "tool_name": "Agent",
            "tool_use_id": "toolu_77",
            "tool_input": {
                "subagent_type": "worker",
                "description": "rename it",
                "prompt": prompt,
                "model": "sonnet",
            },
        });
        parse_input(payload.to_string().as_bytes()).expect("the payload parses")
    }

    /// Run the hook against `transport` and return the one logged record.
    fn shadow(input: &HookInput, key: Option<&str>, transport: &Fake) -> Value {
        let dir = state_dir("run");
        let ctx = Ctx::new(Some(dir.clone().into()), None);
        let out = run_with(input, &ctx, &settings(key), transport).expect("the hook runs");
        assert_eq!(out, None, "the hook never prints a decision");
        let log = std::fs::read_to_string(dir.join(LOG_FILE)).expect("the log exists");
        let mut lines = log.lines();
        let record = serde_json::from_str(lines.next().expect("one record")).expect("JSON");
        assert_eq!(lines.next(), None, "exactly one record");
        record
    }

    #[test]
    fn the_dispatch_fields_are_read_leniently() {
        let input = parse_input(
            br#"{"tool_name":"Agent","tool_input":{"subagent_type":"worker","prompt":"do it","model":7}}"#,
        )
        .unwrap();
        assert_eq!(
            Dispatch::from_input(&input),
            Dispatch {
                subagent_type: Some("worker"),
                description: None,
                prompt: Some("do it"),
                model: None,
            }
        );
    }

    #[test]
    fn the_request_pins_the_model_and_asks_the_three_questions() {
        let input = payload("rename the helper");
        let fake = Fake::replying(r#"{"model":"jev-1.13.0","answers":{}}"#);
        shadow(&input, Some(KEY), &fake);
        let calls = fake.calls.borrow();
        let (url, key, body) = &calls[0];
        assert_eq!(url, "http://jev.test/v1/systemone");
        assert_eq!(key, KEY);
        assert_eq!(body["model"], "jev-1.13.0");
        assert_eq!(
            body["state"],
            json!({
                "subagent_type": "worker",
                "description": "rename it",
                "prompt": "rename the helper",
            })
        );
        let questions = &body["questions"];
        assert_eq!(questions.as_object().unwrap().len(), 3);
        assert_eq!(questions["complexity"]["type"], "score");
        assert_eq!(
            questions["complexity"]["criteria"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            questions["complexity"]["instructions"],
            "How much reasoning does carrying out `prompt` need?"
        );
        assert_eq!(questions["low_level_hazard"]["type"], "noul");
        assert!(questions["low_level_hazard"]["instructions"]
            .as_str()
            .unwrap()
            .contains("lock ordering, or boot sequencing?"));
        assert_eq!(questions["work_kind"]["type"], "choice");
        let options: Vec<&str> = questions["work_kind"]["criteria"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            options,
            [
                "read_only_search",
                "implement_change",
                "review_or_audit",
                "run_and_verify",
                "write_docs",
                "research_external",
                "other"
            ]
        );
    }

    #[test]
    fn answers_are_stored_parsed_with_the_dispatch_identity() {
        let reply = r#"{"model":"jev-1.13.0","answers":{"work_kind":{"type":"choice","choice":"other","confidence":0.5,"extra":[1,2]},"complexity":{"type":"score","score":1.5}},"usage":{"input_tokens":1,"output_tokens":2}}"#;
        let fake = Fake::replying(reply);
        let record = shadow(&payload("rename the helper"), Some(KEY), &fake);
        let expected: Value = serde_json::from_str(reply).unwrap();
        assert_eq!(record["answers"], expected["answers"]);
        assert_eq!(
            record["answers"].to_string(),
            expected["answers"].to_string(),
            "key order is kept too"
        );
        assert_eq!(record["jev_model"], "jev-1.13.0");
        assert_eq!(record["error"], Value::Null);
        assert!(record["latency_ms"].is_u64());
        assert!(record["ts"].as_u64().unwrap() > 1_700_000_000);
        assert_eq!(record["session_id"], "s1");
        assert_eq!(record["agent_id"], "a9");
        assert_eq!(record["tool_use_id"], "toolu_77");
        assert_eq!(record["subagent_type"], "worker");
        assert_eq!(record["model"], "sonnet");
        assert_eq!(record["description"], "rename it");
        assert_eq!(record["prompt"], "rename the helper");
        assert_eq!(record["prompt_chars"], 17);
        assert_eq!(record["prompt_truncated"], false);
    }

    #[test]
    fn answers_are_reserialised_so_number_formatting_is_normalised() {
        let reply = r#"{"model":"jev-1.13.0","answers":{"z":{"score":0.50},"a":{"score":1.0}}}"#;
        let fake = Fake::replying(reply);
        let record = shadow(&payload("rename the helper"), Some(KEY), &fake);
        assert_eq!(
            record["answers"].to_string(),
            r#"{"z":{"score":0.5},"a":{"score":1.0}}"#,
            "0.50 is logged as 0.5 and key order is kept"
        );
    }

    #[test]
    fn a_long_prompt_is_cut_by_characters_and_flagged() {
        // Two-byte characters: a byte cut at 8000 would be wrong, a character cut is not.
        let prompt = "é".repeat(MAX_PROMPT_CHARS + 5);
        let fake = Fake::replying(r#"{"model":"jev-1.13.0","answers":{}}"#);
        let record = shadow(&payload(&prompt), Some(KEY), &fake);
        let sent = fake.calls.borrow()[0].2["state"]["prompt"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(sent.chars().count(), MAX_PROMPT_CHARS);
        assert_eq!(record["prompt"], sent.as_str());
        assert_eq!(record["prompt_chars"], MAX_PROMPT_CHARS + 5);
        assert_eq!(record["prompt_truncated"], true);

        let exact = "x".repeat(MAX_PROMPT_CHARS);
        let record = shadow(&payload(&exact), Some(KEY), &fake);
        assert_eq!(record["prompt_truncated"], false);
        assert_eq!(record["prompt_chars"], MAX_PROMPT_CHARS);
    }

    #[test]
    fn a_missing_key_is_an_error_record_and_no_request() {
        let fake = Fake::replying("{}");
        let record = shadow(&payload("p"), None, &fake);
        assert!(fake.calls.borrow().is_empty());
        assert_eq!(record["error"], "TYPESAFE_API_KEY is not set");
        assert_eq!(record["answers"], Value::Null);
        assert_eq!(record["latency_ms"], Value::Null);
        assert_eq!(record["prompt"], "p");
    }

    #[test]
    fn a_transport_failure_is_an_error_record_without_the_key() {
        let fake = Fake::failing(&format!("curl failed: bad key {KEY}"));
        let record = shadow(&payload("p"), Some(KEY), &fake);
        let error = record["error"].as_str().unwrap();
        assert!(error.contains("curl failed"), "{error}");
        assert!(!error.contains(KEY), "{error}");
        assert_eq!(record["answers"], Value::Null);
        assert!(record["latency_ms"].is_u64());
    }

    #[test]
    fn a_bad_response_is_an_error_record() {
        for reply in [
            "not json",
            r#"{"model":"jev-1.13.0"}"#,
            r#"{"answers":[1]}"#,
        ] {
            let fake = Fake::replying(reply);
            let record = shadow(&payload("p"), Some(KEY), &fake);
            assert!(record["error"].is_string(), "{reply}: {record}");
            assert_eq!(record["answers"], Value::Null, "{reply}");
        }
    }

    #[test]
    fn a_key_straddling_the_quote_cut_is_scrubbed_before_the_cut() {
        // The key starts 5 bytes before the 200-byte cut, so cutting first would
        // leave a fragment that `replace` does not match. After the scrub the
        // marker itself straddles the cut, which is harmless.
        let filler = "x".repeat(MAX_QUOTED_BYTES - 5);
        for reply in [
            format!("{filler}{KEY}"),
            format!(r#"{{"error":"{filler}{KEY}"}}"#),
        ] {
            let fake = Fake::replying(&reply);
            let record = shadow(&payload("p"), Some(KEY), &fake);
            let error = record["error"].as_str().unwrap();
            assert!(!error.contains(&KEY[..4]), "{error}");
        }
    }

    #[test]
    fn scrub_replaces_every_occurrence() {
        assert_eq!(scrub("a K b K", "K"), "a [redacted] b [redacted]");
    }

    #[test]
    fn off_does_nothing_at_all() {
        let dir = state_dir("off");
        let ctx = Ctx::new(Some(dir.clone().into()), None);
        let fake = Fake::replying("{}");
        let off = Settings {
            off: true,
            ..settings(Some(KEY))
        };
        let out = run_with(&payload("p"), &ctx, &off, &fake).expect("the hook runs");
        assert_eq!(out, None);
        assert!(fake.calls.borrow().is_empty());
        assert!(!dir.join(LOG_FILE).exists());
    }

    #[test]
    fn records_are_appended() {
        let dir = state_dir("append");
        let ctx = Ctx::new(Some(dir.clone().into()), None);
        let fake = Fake::replying(r#"{"model":"jev-1.13.0","answers":{}}"#);
        for _ in 0..3 {
            run_with(&payload("p"), &ctx, &settings(Some(KEY)), &fake).unwrap();
        }
        let log = std::fs::read_to_string(dir.join(LOG_FILE)).unwrap();
        assert_eq!(log.lines().count(), 3);
    }

    #[test]
    fn the_auth_config_line_is_escaped_and_refuses_control_characters() {
        assert_eq!(
            curl_auth_config(r#"a"b\c"#).unwrap(),
            "header = \"Authorization: Bearer a\\\"b\\\\c\"\n"
        );
        assert!(curl_auth_config("a\nb").is_err());
    }

    #[test]
    fn the_body_file_is_private_and_removed_on_drop() {
        let dir = state_dir("body");
        let file = BodyFile::create(&dir, b"{}").unwrap();
        let path = file.path.clone();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(std::fs::read(&path).unwrap(), b"{}");
        drop(file);
        assert!(!path.exists());
    }
}
