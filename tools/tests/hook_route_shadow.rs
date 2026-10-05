//! `aios hook route-shadow` against a fake Jev server on 127.0.0.1, through the
//! system `curl`: the request, the log record, the key staying out of every
//! output, and the error paths. The hook never prints to stdout and exits 0.

mod hook_support;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use hook_support::{run_hook, unique_dir, Run};
use serde_json::{json, Value};

const KEY: &str = "sk-integration-SECRET-4711";

const ANSWERS: &str = r#"{"model":"jev-1.13.0","answers":{"complexity":{"type":"score","score":2.4,"confidence":0.8},"low_level_hazard":{"type":"noul","noul":0.07},"work_kind":{"type":"choice","choice":"implement_change","confidence":0.9}},"usage":{"input_tokens":300,"output_tokens":30}}"#;

/// What the fake server saw of one request.
struct Seen {
    request_line: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// How the fake server answers.
#[derive(Clone)]
enum Reply {
    Ok(String),
    Status(u16, String),
    /// Read the request, then close without a response.
    Close,
    /// Read the request, then say nothing until the client gives up (or 8 s pass).
    Hang,
}

/// A one-thread HTTP server on 127.0.0.1 that answers every request the same way.
struct Server {
    url: String,
    stop: Arc<AtomicBool>,
    seen: Arc<Mutex<Vec<Seen>>>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    fn start(reply: Reply) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1");
        listener
            .set_nonblocking(true)
            .expect("non-blocking listener");
        let port = listener.local_addr().expect("local address").port();
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let thread = {
            let (stop, seen) = (Arc::clone(&stop), Arc::clone(&seen));
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if let Some(request) = serve(stream, &reply) {
                                seen.lock().expect("seen lock").push(request);
                            }
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
            })
        };
        Server {
            url: format!("http://127.0.0.1:{port}/v1/systemone"),
            stop,
            seen,
            thread: Some(thread),
        }
    }

    /// Stop the server and return the requests it saw.
    fn finish(mut self) -> Vec<Seen> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread
            .take()
            .expect("server thread")
            .join()
            .expect("join");
        std::mem::take(&mut *self.seen.lock().expect("seen lock"))
    }
}

/// Read one request from `stream` and answer it; `None` when the client sent
/// nothing usable.
fn serve(mut stream: TcpStream, reply: &Reply) -> Option<Seen> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_string(), v.trim().to_string()))
        .collect();
    let find = |name: &str| {
        headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
    if find("Expect").is_some_and(|v| v.eq_ignore_ascii_case("100-continue")) {
        stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").ok()?;
    }
    let length: usize = find("Content-Length")?.parse().ok()?;
    while buf.len() < header_end + length {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = buf[header_end..header_end + length].to_vec();
    let (status, text) = match reply {
        Reply::Ok(text) => (200, text.as_str()),
        Reply::Status(code, text) => (*code, text.as_str()),
        Reply::Hang => {
            // A read returns 0 as soon as curl closes its end after `--max-time`.
            stream.set_read_timeout(Some(Duration::from_secs(8))).ok()?;
            let _ = stream.read(&mut chunk);
            return Some(Seen {
                request_line,
                headers,
                body,
            });
        }
        Reply::Close => {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return Some(Seen {
                request_line,
                headers,
                body,
            });
        }
    };
    let response = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
    Some(Seen {
        request_line,
        headers,
        body,
    })
}

struct Env {
    state: PathBuf,
    cwd: PathBuf,
}

impl Env {
    fn new(label: &str) -> Env {
        Env {
            state: unique_dir(&format!("{label}-state")),
            cwd: unique_dir(&format!("{label}-cwd")),
        }
    }

    /// Run the hook with the given extra environment and check the invariants
    /// every run shares: exit 0, nothing on stdout, the key nowhere in stderr.
    fn run(&self, payload: &Value, extra: &[(&str, &str)]) -> Run {
        let state = self.state.to_str().expect("the state path is UTF-8");
        let mut env = vec![
            ("AIOS_HOOK_STATE_DIR", state),
            ("NO_PROXY", "127.0.0.1"),
            ("no_proxy", "127.0.0.1"),
        ];
        env.extend_from_slice(extra);
        let run = run_hook(
            &["route-shadow"],
            payload.to_string().as_bytes(),
            &env,
            &self.cwd,
        );
        assert_eq!(run.code, Some(0), "{}", run.stderr);
        assert_eq!(run.stdout, "", "the hook never prints to stdout");
        assert!(!run.stderr.contains(KEY), "the key leaked to stderr");
        run
    }

    fn log_path(&self) -> PathBuf {
        self.state.join("route-shadow.jsonl")
    }

    fn records(&self) -> Vec<Value> {
        let Ok(text) = std::fs::read_to_string(self.log_path()) else {
            return Vec::new();
        };
        text.lines()
            .map(|l| serde_json::from_str(l).expect("a log line is JSON"))
            .collect()
    }

    /// Every file in the state directory: only the log may be left, so a body
    /// file was removed.
    fn state_entries(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.state)
            .expect("read the state directory")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn assert_key_not_in_log(&self) {
        let log = std::fs::read_to_string(self.log_path()).unwrap_or_default();
        assert!(!log.contains(KEY), "the key leaked to the log");
    }
}

fn dispatch(prompt: &str) -> Value {
    json!({
        "session_id": "sess-1",
        "hook_event_name": "PreToolUse",
        "tool_name": "Agent",
        "tool_use_id": "toolu_42",
        "agent_id": "agent-7",
        "tool_input": {
            "subagent_type": "worker",
            "description": "update the docs",
            "prompt": prompt,
            "model": "sonnet",
        },
    })
}

fn only(seen: &[Seen]) -> &Seen {
    assert_eq!(seen.len(), 1, "exactly one request");
    &seen[0]
}

#[test]
fn a_dispatch_is_sent_to_jev_and_the_answers_are_logged() {
    let env = Env::new("ok");
    let server = Server::start(Reply::Ok(ANSWERS.to_string()));
    env.run(
        &dispatch("write the README section"),
        &[("AIOS_JEV_URL", &server.url), ("TYPESAFE_API_KEY", KEY)],
    );
    let seen = server.finish();
    let request = only(&seen);

    assert!(
        request.request_line.starts_with("POST /v1/systemone "),
        "{}",
        request.request_line
    );
    assert_eq!(
        request.header("Authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    assert_eq!(request.header("Content-Type"), Some("application/json"));
    let body: Value = serde_json::from_slice(&request.body).expect("the body is JSON");
    assert_eq!(body["model"], "jev-1.13.0");
    assert_eq!(body["state"]["subagent_type"], "worker");
    assert_eq!(body["state"]["description"], "update the docs");
    assert_eq!(body["state"]["prompt"], "write the README section");
    let questions = body["questions"].as_object().expect("questions");
    let ids: Vec<&str> = questions.keys().map(String::as_str).collect();
    assert_eq!(ids, ["complexity", "low_level_hazard", "work_kind"]);

    let records = env.records();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    let expected: Value = serde_json::from_str(ANSWERS).unwrap();
    assert_eq!(record["answers"], expected["answers"]);
    assert_eq!(record["jev_model"], "jev-1.13.0");
    assert_eq!(record["error"], Value::Null);
    assert!(record["latency_ms"].is_u64(), "{record}");
    assert_eq!(record["session_id"], "sess-1");
    assert_eq!(record["agent_id"], "agent-7");
    assert_eq!(record["tool_use_id"], "toolu_42");
    assert_eq!(record["subagent_type"], "worker");
    assert_eq!(record["model"], "sonnet");
    assert_eq!(record["description"], "update the docs");
    assert_eq!(record["prompt"], "write the README section");
    assert_eq!(record["prompt_chars"], 24);
    assert_eq!(record["prompt_truncated"], false);
    env.assert_key_not_in_log();
    assert_eq!(
        env.state_entries(),
        ["route-shadow.jsonl"],
        "body file left behind"
    );
}

#[test]
fn a_long_prompt_is_cut_in_the_request_and_flagged_in_the_log() {
    let env = Env::new("long");
    let server = Server::start(Reply::Ok(ANSWERS.to_string()));
    let prompt = "word ".repeat(2000);
    env.run(
        &dispatch(&prompt),
        &[("AIOS_JEV_URL", &server.url), ("TYPESAFE_API_KEY", KEY)],
    );
    let seen = server.finish();
    let body: Value = serde_json::from_slice(&only(&seen).body).unwrap();
    let sent = body["state"]["prompt"].as_str().unwrap();
    assert_eq!(sent.chars().count(), 8000);
    let record = &env.records()[0];
    assert_eq!(record["prompt_chars"], 10_000);
    assert_eq!(record["prompt_truncated"], true);
    assert_eq!(record["prompt"], sent);
}

#[test]
fn a_server_error_is_an_error_record_and_the_key_stays_out() {
    let env = Env::new("http500");
    // The server echoes the key back, as a careless API might.
    let server = Server::start(Reply::Status(
        500,
        format!("{{\"error\":\"boom for {KEY}\"}}"),
    ));
    env.run(
        &dispatch("p"),
        &[("AIOS_JEV_URL", &server.url), ("TYPESAFE_API_KEY", KEY)],
    );
    assert_eq!(server.finish().len(), 1);
    let records = env.records();
    assert_eq!(records.len(), 1);
    let error = records[0]["error"].as_str().expect("an error message");
    assert!(error.contains("500"), "{error}");
    assert_eq!(records[0]["answers"], Value::Null);
    env.assert_key_not_in_log();
    assert_eq!(env.state_entries(), ["route-shadow.jsonl"]);
}

#[test]
fn a_key_echoed_across_the_quote_cut_leaves_no_fragment_in_the_log() {
    let env = Env::new("http500-cut");
    // 190 bytes of preamble, so the 200-byte quote cut falls inside the key.
    let server = Server::start(Reply::Status(
        500,
        format!("{}{KEY}{}", "x".repeat(190), "y".repeat(50)),
    ));
    env.run(
        &dispatch("p"),
        &[("AIOS_JEV_URL", &server.url), ("TYPESAFE_API_KEY", KEY)],
    );
    assert_eq!(server.finish().len(), 1);
    let records = env.records();
    assert_eq!(records.len(), 1);
    let error = records[0]["error"].as_str().expect("an error message");
    assert!(error.contains("[redacted]"), "{error}");
    let log = std::fs::read_to_string(env.log_path()).expect("the log exists");
    assert!(!log.contains(&KEY[..6]), "a key fragment leaked: {log}");
}

#[test]
fn a_server_that_closes_early_is_an_error_record() {
    let env = Env::new("close");
    let server = Server::start(Reply::Close);
    env.run(
        &dispatch("p"),
        &[("AIOS_JEV_URL", &server.url), ("TYPESAFE_API_KEY", KEY)],
    );
    assert_eq!(server.finish().len(), 1);
    let records = env.records();
    assert_eq!(records.len(), 1);
    assert!(records[0]["error"].is_string(), "{}", records[0]);
    assert_eq!(records[0]["answers"], Value::Null);
    assert!(records[0]["latency_ms"].is_u64());
    env.assert_key_not_in_log();
    assert_eq!(env.state_entries(), ["route-shadow.jsonl"]);
}

#[test]
fn a_server_that_never_answers_is_cut_off_by_the_time_limit() {
    let env = Env::new("hang");
    let server = Server::start(Reply::Hang);
    let started = Instant::now();
    env.run(
        &dispatch("p"),
        &[("AIOS_JEV_URL", &server.url), ("TYPESAFE_API_KEY", KEY)],
    );
    let elapsed = started.elapsed();
    // The plan registers the hook with a 6 s timeout; curl's `--max-time 4` is
    // what keeps a stalled server inside it.
    assert!(
        elapsed < Duration::from_millis(5500),
        "the hook took {elapsed:?}"
    );
    assert_eq!(server.finish().len(), 1, "the request reached the server");
    let records = env.records();
    assert_eq!(records.len(), 1);
    let error = records[0]["error"].as_str().expect("an error message");
    assert!(
        error.contains("(28)") && error.to_ascii_lowercase().contains("timed out"),
        "{error}"
    );
    assert_eq!(records[0]["answers"], Value::Null);
    env.assert_key_not_in_log();
    assert_eq!(env.state_entries(), ["route-shadow.jsonl"]);
}

#[test]
fn an_unreachable_server_is_an_error_record() {
    let env = Env::new("down");
    // Bind and drop a listener to get a port nothing listens on.
    let port = TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("address")
        .port();
    let url = format!("http://127.0.0.1:{port}/v1/systemone");
    env.run(
        &dispatch("p"),
        &[("AIOS_JEV_URL", &url), ("TYPESAFE_API_KEY", KEY)],
    );
    let records = env.records();
    assert_eq!(records.len(), 1);
    assert!(records[0]["error"].is_string(), "{}", records[0]);
    env.assert_key_not_in_log();
}

#[test]
fn a_reply_without_answers_is_an_error_record() {
    let env = Env::new("noanswers");
    let server = Server::start(Reply::Ok("{\"model\":\"jev-1.13.0\"}".to_string()));
    env.run(
        &dispatch("p"),
        &[("AIOS_JEV_URL", &server.url), ("TYPESAFE_API_KEY", KEY)],
    );
    server.finish();
    let records = env.records();
    assert!(records[0]["error"].as_str().unwrap().contains("no answers"));
}

#[test]
fn off_sends_nothing_and_logs_nothing() {
    let env = Env::new("off");
    let server = Server::start(Reply::Ok(ANSWERS.to_string()));
    env.run(
        &dispatch("p"),
        &[
            ("AIOS_ROUTE_SHADOW", "off"),
            ("AIOS_JEV_URL", &server.url),
            ("TYPESAFE_API_KEY", KEY),
        ],
    );
    assert!(server.finish().is_empty());
    assert!(env.records().is_empty());
    assert!(!Path::new(&env.log_path()).exists());
}

#[test]
fn a_missing_key_is_an_error_record_and_no_request() {
    let env = Env::new("nokey");
    let server = Server::start(Reply::Ok(ANSWERS.to_string()));
    env.run(&dispatch("p"), &[("AIOS_JEV_URL", &server.url)]);
    assert!(server.finish().is_empty());
    let records = env.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["error"], "TYPESAFE_API_KEY is not set");
    assert_eq!(records[0]["answers"], Value::Null);
    assert_eq!(records[0]["tool_use_id"], "toolu_42");
}

#[test]
fn input_that_is_not_json_prints_nothing_and_exits_zero() {
    let env = Env::new("garbage");
    let state = env.state.to_str().unwrap();
    let run = run_hook(
        &["route-shadow"],
        b"not json",
        &[("AIOS_HOOK_STATE_DIR", state), ("TYPESAFE_API_KEY", KEY)],
        &env.cwd,
    );
    assert_eq!(run.code, Some(0));
    assert_eq!(run.stdout, "");
    assert!(!run.stderr.contains(KEY));
}
