//! The Docs drift section: `aios docs-check --json` runs in the background from the
//! start of the brief and is summarised at the end.
//!
//! The child is this very binary, spawned as `/proc/self/exe` on Linux, so a `just tools`
//! rename over the running binary during the brief cannot leave it pointing at a deleted
//! path, and as `current_exe()` elsewhere. It is the binary the caller chose and no shim
//! message can reach its stderr (divergence D2). Only the program can be
//! replaced, by `AIOS_BRIEF_DOCS_CHECK`, and only in a build with debug assertions (the
//! profile `cargo test` uses): the argv `docs-check --json` and the working directory
//! (the toplevel) stay the same, so the tests pin them.
//!
//! The stale note comes from the environment (D6): the shim exports
//! `AIOS_SHIM_STALE=1` when it runs a stale binary whose rebuild failed.

use std::ffi::OsString;
use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread::JoinHandle;

use anyhow::Result;
use regex::bytes::Regex;
use serde_json::Value;

use crate::cmd::checkpoint::scan::lines;

/// The variable that replaces the docs-check program (debug builds only).
const PROGRAM_VAR: &str = "AIOS_BRIEF_DOCS_CHECK";

/// The variable the shim exports for a stale binary.
const STALE_VAR: &str = "AIOS_SHIM_STALE";

/// What the docs-check child left.
pub struct DocsRun {
    /// The exit status as the shell would see it (`128 + signal` for a signal); `None`
    /// when the child never ran or its thread died.
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// This binary, as a path that still runs it after `just tools` renamed a new binary over
/// it: on Linux `current_exe()` reads `/proc/self/exe` and returns `<path> (deleted)`, so
/// the link itself is the program (exec through it reaches the running inode).
fn self_exe() -> std::io::Result<OsString> {
    if cfg!(target_os = "linux") {
        Ok(OsString::from("/proc/self/exe"))
    } else {
        std::env::current_exe().map(PathBuf::into_os_string)
    }
}

/// The program that runs docs-check.
fn program() -> std::io::Result<OsString> {
    if cfg!(debug_assertions) {
        if let Some(program) = std::env::var_os(PROGRAM_VAR).filter(|value| !value.is_empty()) {
            return Ok(program);
        }
    }
    self_exe()
}

/// Run `<program> docs-check --json` in `root`.
fn run_child(root: &Path) -> DocsRun {
    let spawned = program().and_then(|program| {
        Command::new(program)
            .args(["docs-check", "--json"])
            .current_dir(root)
            .stdin(Stdio::null())
            .output()
    });
    match spawned {
        Ok(output) => DocsRun {
            code: output
                .status
                .code()
                .or_else(|| output.status.signal().map(|signal| 128 + signal)),
            stdout: output.stdout,
            stderr: output.stderr,
        },
        Err(e) => DocsRun {
            code: Some(127),
            stdout: Vec::new(),
            stderr: format!("docs-check: cannot run docs-check: {e}\n").into_bytes(),
        },
    }
}

/// Start docs-check on a thread; it is the slowest local step.
pub fn start(root: &Path) -> JoinHandle<DocsRun> {
    let root = root.to_path_buf();
    std::thread::spawn(move || run_child(&root))
}

/// Whether the shim flagged the binary as stale.
pub fn stale_from_env() -> bool {
    std::env::var_os(STALE_VAR).is_some_and(|value| value == "1")
}

const STALE_NOTE: &str = " (from a stale aios binary whose rebuild failed; run `just tools`)";

/// `tail -n 1`: the last line, without its newline.
fn last_line(text: &[u8]) -> &[u8] {
    lines(text).last().unwrap_or_default()
}

/// The panic header of a Rust binary: `thread 'main' panicked` or, with a thread id,
/// `thread 'main' (123) panicked`.
fn panic_header() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^thread 'main'( \([0-9]+\))? panicked").expect("a valid panic regex")
    })
}

/// The line that says why docs-check failed (exit other than 0 and 1): the checker's
/// own `docs-check: ` line, then the panic header, then the last line (D2 dropped the
/// shim's `aios: ` step, which only the shim could print).
fn error_line(stderr: &[u8]) -> &[u8] {
    lines(stderr)
        .find(|line| line.starts_with(b"docs-check: "))
        .or_else(|| lines(stderr).find(|line| panic_header().is_match(line)))
        .unwrap_or_else(|| last_line(stderr))
}

/// jq's `tostring`: a string as it is, anything else as compact JSON.
fn tostring(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// jq's `//`: the right side when the left is null or false.
fn or_zero(value: &Value) -> Value {
    match value {
        Value::Null | Value::Bool(false) => Value::from(0),
        other => other.clone(),
    }
}

/// jq's `x > 0` for the values a summary holds: numbers compare as numbers, and the
/// types that sort after numbers (strings, arrays, objects) are greater than 0.
fn above_zero(value: &Value) -> bool {
    match value {
        Value::Number(n) => n.as_f64().is_some_and(|n| n > 0.0),
        Value::String(_) | Value::Array(_) | Value::Object(_) => true,
        Value::Null | Value::Bool(_) => false,
    }
}

/// jq's `a + b` for numbers and null (null is the identity).
fn sum(a: &Value, b: &Value) -> Value {
    match (a, b) {
        (Value::Null, other) | (other, Value::Null) => other.clone(),
        (Value::Number(a), Value::Number(b)) => match (a.as_i64(), b.as_i64()) {
            (Some(a), Some(b)) if a.checked_add(b).is_some() => Value::from(a + b),
            _ => Value::from(a.as_f64().unwrap_or(0.0) + b.as_f64().unwrap_or(0.0)),
        },
        _ => Value::Null,
    }
}

/// The one-line summary of a readable docs-check JSON document, or `None` when it is
/// not a document with a `summary` object.
fn summary(doc: &Value, note: &str) -> Option<String> {
    let summary = doc.get("summary")?.as_object()?;
    let field = |name: &str| summary.get(name).cloned().unwrap_or(Value::Null);
    let new = field("new");
    let mut line = format!(
        "- docs-check: {} new vs baseline, {} total ({} baselined, of which {} accepted false positives; {} resolved)",
        tostring(&new),
        tostring(&field("total")),
        tostring(&field("baselined")),
        tostring(&or_zero(&field("accepted"))),
        tostring(&field("resolved")),
    );
    if above_zero(&new) {
        let drifting: Vec<String> = doc
            .get("checks")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(name, check)| {
                let new = check.get("new").unwrap_or(&Value::Null);
                above_zero(&or_zero(new)).then(|| format!("{name} {}", tostring(new)))
            })
            .collect();
        line.push_str("; new in: ");
        line.push_str(&drifting.join(", "));
    }
    if above_zero(&sum(&field("resolved"), &or_zero(&field("reduced")))) {
        line.push_str(
            "; run `just docs-check --update-baseline` to prune resolved or reduced entries",
        );
    }
    line.push_str(note);
    Some(line)
}

/// `jq -e .`: the document when stdout holds JSON values and the last is neither null
/// nor false; the summary reads that last value.
fn readable(stdout: &[u8]) -> Option<Value> {
    let mut last = None;
    for value in serde_json::Deserializer::from_slice(stdout).into_iter::<Value>() {
        last = Some(value.ok()?);
    }
    last.filter(|value| !matches!(value, Value::Null | Value::Bool(false)))
}

/// Wait for the child and print the section body.
pub fn print(handle: JoinHandle<DocsRun>, stale: bool, out: &mut dyn Write) -> Result<()> {
    let run = handle.join().unwrap_or(DocsRun {
        code: None,
        stdout: Vec::new(),
        stderr: Vec::new(),
    });
    let rc = run
        .code
        .map_or_else(|| "?".to_string(), |code| code.to_string());
    let note = if stale { STALE_NOTE } else { "" };
    if !matches!(run.code, Some(0 | 1)) {
        out.write_all(
            format!("- docs-check failed (exit {rc}, a checker error, not drift): ").as_bytes(),
        )?;
        out.write_all(error_line(&run.stderr))?;
        out.write_all(b"\n")?;
        return Ok(());
    }
    let line = readable(&run.stdout).and_then(|doc| summary(&doc, note));
    match line {
        Some(line) => writeln!(out, "{line}")?,
        None => {
            out.write_all(
                format!("- docs-check ran (exit {rc}) but its JSON output is unreadable: ")
                    .as_bytes(),
            )?;
            out.write_all(last_line(&run.stderr))?;
            writeln!(out, "{note}")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(json: &str) -> Value {
        serde_json::from_str(json).expect("valid JSON")
    }

    #[test]
    fn self_exe_is_the_proc_link_on_linux_and_current_exe_elsewhere() {
        let exe = self_exe().expect("this binary's path");
        if cfg!(target_os = "linux") {
            assert_eq!(exe, OsString::from("/proc/self/exe"));
        } else {
            assert_eq!(exe, std::env::current_exe().expect("current_exe"));
        }
        // The path must also run: this lib test binary is the libtest harness,
        // which lists its tests for `--list` and exits 0.
        let status = Command::new(&exe)
            .arg("--list")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("spawn self_exe()");
        assert!(status.success(), "self_exe() {exe:?} --list: {status}");
    }

    #[test]
    fn error_line_prefers_the_checker_then_the_panic_then_the_last_line() {
        let both = b"noise\nthread 'main' panicked at x\ndocs-check: broken\ntail\n";
        assert_eq!(error_line(both), b"docs-check: broken");
        let panic = b"cargo noise\nthread 'main' (4242) panicked at src/x.rs:1\ntail\n";
        assert_eq!(
            error_line(panic),
            b"thread 'main' (4242) panicked at src/x.rs:1"
        );
        // The shim's `aios: ` line is no longer preferred (D2).
        assert_eq!(error_line(b"aios: shim\nlast\n"), b"last");
        assert_eq!(error_line(b"one\ntwo"), b"two");
        assert_eq!(error_line(b""), b"");
    }

    #[test]
    fn last_line_is_tail_n_1() {
        assert_eq!(last_line(b"a\nb\n"), b"b");
        assert_eq!(last_line(b"a\nb"), b"b");
        assert_eq!(last_line(b"a\n\n"), b"");
        assert_eq!(last_line(b""), b"");
    }

    #[test]
    fn summary_follows_the_jq_program() {
        let clean = doc(
            r#"{"summary":{"new":0,"total":3,"baselined":3,"accepted":1,"resolved":0},"checks":{}}"#,
        );
        assert_eq!(
            summary(&clean, "").as_deref(),
            Some("- docs-check: 0 new vs baseline, 3 total (3 baselined, of which 1 accepted false positives; 0 resolved)")
        );
        // `accepted` is absent: `// 0`. New drift lists the checks in document order.
        let drift = doc(
            r#"{"summary":{"new":3,"total":5,"baselined":2,"resolved":1,"reduced":2},
                "checks":{"zeta":{"new":2},"alpha":{"new":0},"mid":{"new":1}}}"#,
        );
        assert_eq!(
            summary(&drift, " (note)").as_deref(),
            Some("- docs-check: 3 new vs baseline, 5 total (2 baselined, of which 0 accepted false positives; 1 resolved); new in: zeta 2, mid 1; run `just docs-check --update-baseline` to prune resolved or reduced entries (note)")
        );
        assert_eq!(summary(&doc("[1]"), ""), None);
        assert_eq!(summary(&doc(r#"{"summary":3}"#), ""), None);
    }

    #[test]
    fn readable_is_jq_dash_e() {
        assert!(readable(br#"{"a":1}"#).is_some());
        assert!(readable(b"").is_none());
        assert!(readable(b"null").is_none());
        assert!(readable(b"false").is_none());
        assert!(readable(b"{").is_none());
        assert!(readable(b"{} not json").is_none());
        assert_eq!(readable(b"1 2"), Some(Value::from(2)));
    }

    #[test]
    fn jq_helpers() {
        assert_eq!(or_zero(&Value::Null), Value::from(0));
        assert_eq!(or_zero(&Value::Bool(false)), Value::from(0));
        assert_eq!(or_zero(&Value::from(4)), Value::from(4));
        assert!(above_zero(&Value::from(1)));
        assert!(!above_zero(&Value::from(0)));
        assert!(!above_zero(&Value::Null));
        assert!(above_zero(&Value::String(String::new())));
        assert_eq!(sum(&Value::Null, &Value::from(2)), Value::from(2));
        assert_eq!(sum(&Value::from(1), &Value::from(2)), Value::from(3));
    }
}
