//! The hook protocol (git-push-guard.py L3474-3519, `emit` and `main`): one
//! PreToolUse payload on stdin, at most one JSON line on stdout, and exit 0, or
//! exit 2 with the reason on stderr for a deny.
//!
//! stdin is decoded as UTF-8 with the error handler Python's `sys.stdin` gets
//! from the environment ([`stdin_errors`]). Inputs Python reads differently are
//! accepted divergences, because Claude Code never sends them:
//!
//! - bytes that are not UTF-8, under the `surrogateescape` handler, become one
//!   U+FFFD each, where Python has one lone surrogate each (a `String` cannot
//!   hold one), so positions and decisions match but such a character in a
//!   reason is printed differently;
//! - a `PYTHONIOENCODING` that names an encoding other than UTF-8, or an error
//!   handler other than strict, surrogateescape, replace and ignore, is read as
//!   UTF-8 and strict; a locale name the C library does not know (which Python
//!   reads as the C locale) is read as a UTF-8 locale;
//! - a `cwd` that is truthy but not a string asks with a fixed `TypeError` text.
//!   Python fails somewhere later with a `TypeError` whose text depends on the
//!   command, or not at all (`ls`).

use std::collections::{BTreeSet, HashMap};
use std::io::{self, Write};

use serde_json::Value;

use super::analyzer::Ctx;
use super::pyjson::{self, LoadError};
use super::pyos::{self, Fault};
use super::spawn_rules::decide_tool;
use super::verdict::{self, Level, Tag, Verdict};
use crate::pystr;

/// The note a push deny ends with.
const PUSH_NOTE: &str = ". Pushes to main, force pushes, mirror pushes and admin merges are reserved for the user: push a claude/* branch and open a PR instead.";

/// What the hook does for one payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// No output, exit 0: a tool the guard does not judge, or level `none`.
    Pass,
    /// An ask or deny, with the rules behind it.
    Decide {
        level: Level,
        reason: String,
        tags: BTreeSet<Tag>,
    },
    /// An exception `main` does not catch: Python prints a traceback and
    /// exits 1, which Claude Code treats as a non-blocking error.
    Crash(String),
}

impl Response {
    pub fn exit_code(&self) -> u8 {
        match self {
            Response::Pass => 0,
            Response::Decide { level, .. } => u8::from(*level == Level::Deny) * 2,
            Response::Crash(_) => 1,
        }
    }

    fn ask(tag: Tag, reason: String) -> Response {
        Response::Decide {
            level: Level::Ask,
            reason,
            tags: BTreeSet::from([tag]),
        }
    }
}

/// The error handler of Python's `sys.stdin` (`errors=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdinErrors {
    Strict,
    SurrogateEscape,
    Replace,
    Ignore,
}

/// The handler CPython 3.13 gives `sys.stdin` in `env`: `PYTHONIOENCODING`'s
/// `:errors` part when it has one, else `surrogateescape` in UTF-8 mode
/// (`PYTHONUTF8=1`) or when the `LC_CTYPE` locale is C or POSIX (also when no
/// locale variable is set) or a locale-coercion target (`C.UTF-8`, `C.utf8`,
/// `UTF-8`; CPython's `config_get_stdio_errors`), else `strict`.
pub fn stdin_errors(env: &HashMap<String, String>) -> StdinErrors {
    let var = |k: &str| env.get(k).map(String::as_str).filter(|v| !v.is_empty());
    if let Some((_, errors)) = var("PYTHONIOENCODING").and_then(|v| v.split_once(':')) {
        match errors {
            "surrogateescape" => return StdinErrors::SurrogateEscape,
            "replace" => return StdinErrors::Replace,
            "ignore" => return StdinErrors::Ignore,
            "" => {}
            _ => return StdinErrors::Strict,
        }
    }
    if var("PYTHONUTF8") == Some("1") {
        return StdinErrors::SurrogateEscape;
    }
    let locale = var("LC_ALL")
        .or_else(|| var("LC_CTYPE"))
        .or_else(|| var("LANG"));
    match locale {
        None | Some("C" | "POSIX" | "C.UTF-8" | "C.utf8" | "UTF-8") => StdinErrors::SurrogateEscape,
        Some(_) => StdinErrors::Strict,
    }
}

/// `bytes.decode("utf-8", errors)`, one invalid sequence at a time (the
/// sequences CPython reports, which are Rust's `Utf8Error` ranges).
fn decode_stdin(bytes: &[u8], errors: StdinErrors) -> Result<String, Fault> {
    if errors == StdinErrors::Strict {
        return pyos::decode_utf8(bytes);
    }
    let mut out = String::new();
    let mut rest = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(text) => {
                out.push_str(text);
                return Ok(out);
            }
            Err(e) => {
                let (valid, after) = rest.split_at(e.valid_up_to());
                out.push_str(std::str::from_utf8(valid).expect("the valid prefix"));
                let bad = e.error_len().unwrap_or(after.len());
                match errors {
                    StdinErrors::SurrogateEscape => {
                        out.extend(std::iter::repeat_n('\u{fffd}', bad))
                    }
                    StdinErrors::Replace => out.push('\u{fffd}'),
                    StdinErrors::Ignore | StdinErrors::Strict => {}
                }
                rest = &after[bad..];
            }
        }
    }
}

/// Python's truthiness for a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `payload.get(key)`, with JSON null read as absent.
fn get<'a>(payload: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a Value> {
    payload.get(key).filter(|v| !v.is_null())
}

/// `main()` up to `emit`: the response to the raw stdin bytes, with `env` the
/// process environment and `getcwd` the process's working directory.
pub fn respond(
    input: io::Result<Vec<u8>>,
    env: &HashMap<String, String>,
    getcwd: impl Fn() -> io::Result<String>,
) -> Response {
    let unreadable = |exc: String| {
        Response::ask(
            verdict::ERROR,
            format!("git-push-guard could not read the hook payload ({exc})"),
        )
    };
    let text = match input {
        Ok(bytes) => match decode_stdin(&bytes, stdin_errors(env)) {
            Ok(text) => text,
            Err(f) => return unreadable(f.message),
        },
        Err(e) => return unreadable(pyos::os_error(&e).message),
    };
    let payload = match pyjson::loads(&text) {
        Ok(Value::Object(payload)) => payload,
        Ok(_) => return unreadable("payload is not a JSON object".to_string()),
        Err(LoadError::Value(msg)) => return unreadable(msg),
        Err(LoadError::Recursion(msg)) => return Response::Crash(format!("RecursionError: {msg}")),
    };
    let tool = payload.get("tool_name").and_then(Value::as_str);
    let verdict = match tool {
        Some("Agent" | "Task" | "EnterWorktree") => decide_tool(&payload, env),
        Some("Bash" | "Monitor") => match command_verdict(&payload, env, getcwd) {
            Some(result) => result,
            None => return Response::Pass,
        },
        _ => return Response::Pass,
    };
    match verdict {
        Ok(v) => verdict_response(v),
        Err(f) => Response::ask(verdict::ERROR, format!("git-push-guard error: {f}")),
    }
}

/// The Bash and Monitor branch of `main`: `None` when the payload carries no
/// command (exit 0, no output).
fn command_verdict(
    payload: &serde_json::Map<String, Value>,
    env: &HashMap<String, String>,
    getcwd: impl Fn() -> io::Result<String>,
) -> Option<Result<Verdict, Fault>> {
    let command = match payload.get("tool_input") {
        Some(Value::Object(input)) => input.get("command").and_then(Value::as_str),
        _ => None,
    };
    let command = command.filter(|c| !c.is_empty())?;
    let cwd = match payload.get("cwd") {
        Some(Value::String(cwd)) if !cwd.is_empty() => cwd.clone(),
        Some(v) if truthy(v) => {
            return Some(Err(Fault::new(
                "TypeError",
                "the hook payload's cwd is not a string",
            )))
        }
        _ => match getcwd() {
            Ok(cwd) => cwd,
            Err(e) => return Some(Err(pyos::os_error(&e))),
        },
    };
    let ctx = Ctx {
        agent_id: get(payload, "agent_id").cloned(),
        agent_type: get(payload, "agent_type").cloned(),
        team: pystr::strip(env.get("AIOS_TEAM").map_or("", String::as_str)).to_string(),
        cwd: cwd.clone(),
    };
    Some(super::decide(command, &cwd, &ctx))
}

fn verdict_response(v: Verdict) -> Response {
    if v.level == Level::None {
        return Response::Pass;
    }
    let mut reason = format!("git-push-guard: {}", v.reasons.join("; "));
    if v.level == Level::Deny && v.push_deny {
        reason.push_str(PUSH_NOTE);
    }
    Response::Decide {
        level: v.level,
        reason,
        tags: v.tags,
    }
}

/// `json.dumps(s)` (`ensure_ascii`): a quoted string with every character
/// outside space..`~` escaped.
fn dumps_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
    out
}

/// `emit`: write the response and return the exit code.
pub fn write(response: &Response, out: &mut dyn Write, err: &mut dyn Write) -> io::Result<u8> {
    match response {
        Response::Pass => {}
        Response::Decide { level, reason, .. } => {
            writeln!(
                out,
                "{{\"hookSpecificOutput\": {{\"hookEventName\": \"PreToolUse\", \"permissionDecision\": {}, \"permissionDecisionReason\": {}}}}}",
                dumps_str(level.as_str()),
                dumps_str(reason)
            )?;
            out.flush()?;
            if *level == Level::Deny {
                writeln!(err, "{reason}")?;
            }
        }
        Response::Crash(message) => writeln!(err, "{message}")?,
    }
    Ok(response.exit_code())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn dumps_matches_python() {
        // json.dumps on CPython 3.13.
        assert_eq!(
            dumps_str("a\"b\\c/\n\r\t\u{8}\u{c}\u{1}\u{7f} \u{e9}\u{1f600} ~"),
            r#""a\"b\\c/\n\r\t\b\f\u0001\u007f \u00e9\ud83d\ude00 ~""#
        );
    }

    #[test]
    fn a_deny_writes_json_and_its_reason_and_exits_2() {
        let response = Response::Decide {
            level: Level::Deny,
            reason: "git-push-guard: no".to_string(),
            tags: BTreeSet::from([verdict::PUSH]),
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(write(&response, &mut out, &mut err).expect("writes"), 2);
        assert_eq!(
            String::from_utf8(out).expect("utf-8"),
            "{\"hookSpecificOutput\": {\"hookEventName\": \"PreToolUse\", \"permissionDecision\": \"deny\", \"permissionDecisionReason\": \"git-push-guard: no\"}}\n"
        );
        assert_eq!(err, b"git-push-guard: no\n");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(
            write(&Response::Pass, &mut out, &mut err).expect("writes"),
            0
        );
        assert!(out.is_empty() && err.is_empty());
    }

    #[test]
    fn stdin_errors_follow_cpython() {
        use StdinErrors::*;
        for (pairs, want) in [
            (&[][..], SurrogateEscape),
            (&[("LANG", "C")][..], SurrogateEscape),
            (&[("LANG", "en_US.UTF-8")][..], Strict),
            (
                &[("LANG", "en_US.UTF-8"), ("LC_CTYPE", "UTF-8")][..],
                SurrogateEscape,
            ),
            (
                &[("LC_CTYPE", "C.UTF-8"), ("LC_ALL", "en_US.UTF-8")][..],
                Strict,
            ),
            (&[("LC_ALL", ""), ("LANG", "POSIX")][..], SurrogateEscape),
            (
                &[("LANG", "en_US.UTF-8"), ("PYTHONUTF8", "1")][..],
                SurrogateEscape,
            ),
            (
                &[("LANG", "C"), ("PYTHONIOENCODING", "utf-8:strict")][..],
                Strict,
            ),
            (
                &[("LANG", "C"), ("PYTHONIOENCODING", "utf-8")][..],
                SurrogateEscape,
            ),
            (&[("PYTHONIOENCODING", ":replace")][..], Replace),
            (&[("PYTHONIOENCODING", "utf-8:ignore")][..], Ignore),
        ] {
            assert_eq!(stdin_errors(&env(pairs)), want, "{pairs:?}");
        }
        // bytes.decode("utf-8", errors) on CPython 3.13, with U+FFFD for each
        // escaped byte.
        let bytes = b"a\xe2\x82(b\xff";
        assert_eq!(
            decode_stdin(bytes, SurrogateEscape).expect("decodes"),
            "a\u{fffd}\u{fffd}(b\u{fffd}"
        );
        assert_eq!(
            decode_stdin(bytes, Replace).expect("decodes"),
            "a\u{fffd}(b\u{fffd}"
        );
        assert_eq!(decode_stdin(bytes, Ignore).expect("decodes"), "a(b");
        assert_eq!(
            decode_stdin(bytes, Strict).map_err(|f| f.message),
            Err(
                "'utf-8' codec can't decode bytes in position 1-2: invalid continuation byte"
                    .to_string()
            )
        );
    }

    /// HookProtocol.test_internal_error_asks and
    /// TeamRules.test_hook_internal_error_in_a_team_rule_asks inject an
    /// exception; here a real one does it: Python 3.13's realpath raises
    /// ValueError for a NUL, and the hook asks with its text.
    #[test]
    fn an_internal_error_asks() {
        let payload = br#"{"tool_name":"Bash","tool_input":{"command":"gh pr create --body-file b.md"},"cwd":"/tmp/a\u0000b"}"#;
        let response = respond(Ok(payload.to_vec()), &env(&[]), || Ok("/".to_string()));
        assert_eq!(
            response,
            Response::Decide {
                level: Level::Ask,
                reason: "git-push-guard error: ValueError: lstat: embedded null character in path"
                    .to_string(),
                tags: BTreeSet::from([verdict::ERROR]),
            }
        );
        let missing = || Err(io::Error::from_raw_os_error(2));
        let payload = br#"{"tool_name":"Bash","tool_input":{"command":"ls"}}"#;
        assert_eq!(
            respond(Ok(payload.to_vec()), &env(&[]), missing),
            Response::Decide {
                level: Level::Ask,
                reason:
                    "git-push-guard error: FileNotFoundError: [Errno 2] No such file or directory"
                        .to_string(),
                tags: BTreeSet::from([verdict::ERROR]),
            }
        );
    }

    #[test]
    fn payloads_without_a_command_pass() {
        for payload in [
            &br#"{}"#[..],
            br#"{"tool_name":"Write","tool_input":{"command":"git push origin main"}}"#,
            br#"{"tool_name":"Bash","tool_input":[]}"#,
            br#"{"tool_name":"Bash","tool_input":{"command":""}}"#,
            br#"{"tool_name":"Monitor","tool_input":{"ws":{"url":"x"}}}"#,
        ] {
            let response = respond(Ok(payload.to_vec()), &env(&[]), || Ok("/".to_string()));
            assert_eq!(
                response,
                Response::Pass,
                "{}",
                String::from_utf8_lossy(payload)
            );
        }
        let crash = respond(Ok("[".repeat(9998).into_bytes()), &env(&[]), || {
            Ok("/".to_string())
        });
        assert_eq!(crash.exit_code(), 1);
    }
}
