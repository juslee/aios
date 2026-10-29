//! Rule 11's spawn rules for tool calls (git-push-guard.py L3322-3471): rule 3
//! (the shape of an Agent or Task spawn) and rule 4 rows 1-3 (who may call
//! EnterWorktree). A project agent type whose frontmatter declares
//! `isolation: worktree` must be spawned with call-level isolation.
//!
//! The environment is passed in as strings: a variable whose value is not UTF-8
//! is read with replacement characters, where Python keeps its bytes. Claude
//! Code sets `CLAUDE_PROJECT_DIR` from the project's path, and `AIOS_TEAM` is only
//! tested for being non-blank, which replacement does not change.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::analyzer::partition;
use super::pattern::re;
use super::pyos::{self, Fault};
use super::repo::Repos;
use super::verdict::{self, Verdict};
use crate::pystr;

/// `PROJECT_TYPE_RE.fullmatch`: a name that can be a file under
/// `.claude/agents/` (built-in types such as general-purpose match too, and
/// have no file).
static PROJECT_TYPE: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z0-9][A-Za-z0-9_.-]*\z"));
/// A frontmatter opening line that is `---` alone, or with a comment.
static PLAIN_OPENER: LazyLock<Regex> = LazyLock::new(|| re(r"^---(?:\s+#.*)?\z"));
static SINGLE_QUOTED: LazyLock<Regex> = LazyLock::new(|| re(r"^'((?:[^']|'')*)'\s*(?:#.*)?\z"));
static DOUBLE_QUOTED: LazyLock<Regex> = LazyLock::new(|| re(r#"^"([^"\\]*)"\s*(?:#.*)?\z"#));
static COMMENT: LazyLock<Regex> = LazyLock::new(|| re(r"\s#"));
static ISOLATION_LINE: LazyLock<Regex> = LazyLock::new(|| re(r#"(?m)^\s*["']?isolation["']?\s*:"#));

/// Frontmatter fields in file order: `None` holds the top-level lines that are
/// not a `key: value` pair, each `Some(line)`; a key's values are
/// [`yaml_scalar`]'s.
type Fields = Vec<(Option<String>, Vec<Option<String>>)>;

fn add_field(fields: &mut Fields, key: Option<String>, value: Option<String>) {
    match fields.iter_mut().find(|(k, _)| *k == key) {
        Some((_, values)) => values.push(value),
        None => fields.push((key, vec![value])),
    }
}

/// `len(line) - len(line.lstrip())`, in characters.
fn indent(line: &str) -> usize {
    line.chars().count() - pystr::lstrip(line).chars().count()
}

/// `frontmatter`: the top-level `key: value` pairs between the first two `---`
/// lines. A leading BOM and blank lines are skipped. The top level is the
/// indentation of the first entry; deeper lines belong to a nested block and
/// are ignored. Top-level lines that are not a `key: value` pair are kept under
/// the key `None`, so a caller can fail closed on them.
fn frontmatter(text: &str) -> Fields {
    let mut lines = pystr::splitlines(text.trim_start_matches('\u{feff}'));
    let blank = lines
        .iter()
        .take_while(|l| pystr::strip(l).is_empty())
        .count();
    lines.drain(..blank);
    let mut fields = Fields::new();
    let Some(first) = lines.first().map(|l| pystr::strip(l)) else {
        return fields;
    };
    if !first.starts_with("---") {
        return fields;
    }
    if !PLAIN_OPENER.is_match(first) {
        add_field(&mut fields, None, Some(first.to_string())); // `--- {flow}` and the like
    }
    let mut body: Vec<&str> = Vec::new();
    for line in &lines[1..] {
        let stripped = pystr::strip(line);
        if stripped == "---" || stripped == "..." {
            break;
        }
        if !stripped.is_empty() && !pystr::lstrip(line).starts_with('#') {
            body.push(pystr::rstrip(line));
        }
    }
    let Some(top) = body.first().map(|l| indent(l)) else {
        return fields;
    };
    for line in body {
        let depth = indent(line);
        if depth > top {
            continue;
        }
        let stripped = pystr::strip(line);
        match yaml_key(stripped) {
            Some((key, value)) if depth >= top => {
                add_field(&mut fields, Some(key.to_string()), yaml_scalar(value));
            }
            _ => add_field(&mut fields, None, Some(stripped.to_string())),
        }
    }
    fields
}

/// `yaml_key`: (key, rest) for a `key: rest` line with a plain or quoted key.
fn yaml_key(line: &str) -> Option<(&str, &str)> {
    let mut chars = line.chars();
    let quote = match chars.next() {
        // `line[:1] in "..."` is true for an empty line.
        None => return None,
        Some(q @ ('\'' | '"')) => q,
        Some(c) if "{[?-&!*|>%@`".contains(c) => return None,
        Some(_) => {
            let (key, colon, rest) = partition(line, ':');
            return colon.then(|| (pystr::strip(key), rest));
        }
    };
    let inner = chars.as_str();
    let end = inner.find(quote)?;
    let after = pystr::lstrip(&inner[end + quote.len_utf8()..]);
    let rest = after.strip_prefix(':')?;
    Some((&inner[..end], rest))
}

/// `yaml_scalar`: the string YAML reads from a one-line value, with anchors
/// (`&a`), tags (`!!str`), quotes and an unquoted ` # comment` removed. `None`
/// for anything it would read differently (an alias, a block or flow value, an
/// escape in a double-quoted string, an unterminated quote).
fn yaml_scalar(value: &str) -> Option<String> {
    let mut v = pystr::strip(value);
    while v.starts_with('!') || v.starts_with('&') {
        // `v.split(None, 1)`: the text after the first run of whitespace.
        let after = v.trim_start_matches(pystr::is_space);
        v = match after.find(pystr::is_space) {
            Some(at) => pystr::strip(&after[at..]),
            None => "",
        };
    }
    match v.chars().next() {
        Some('\'') => SINGLE_QUOTED.captures(v).map(|m| m[1].replace("''", "'")),
        Some('"') => DOUBLE_QUOTED.captures(v).map(|m| m[1].to_string()),
        Some(c) if "*|>[{@`%".contains(c) => None,
        Some('#') => Some(String::new()),
        _ => {
            let head = COMMENT.find(v).map_or(v, |m| &v[..m.start()]);
            Some(pystr::rstrip(head).to_string())
        }
    }
}

/// What the frontmatter says about isolation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Isolation {
    /// `isolation: worktree`, exactly.
    Worktree,
    /// An isolation key that is not readable as one plain value.
    Unclear,
}

/// `isolation_marker`: `None` when the frontmatter has no isolation key.
fn isolation_marker(fields: &Fields, text: &str) -> Option<Isolation> {
    let mut found: Vec<&Option<String>> = Vec::new();
    for (key, values) in fields {
        match key {
            None => {
                let mentions = values
                    .iter()
                    .flatten()
                    .any(|line| line.to_lowercase().contains("isolation"));
                if mentions {
                    return Some(Isolation::Unclear);
                }
            }
            Some(key) if key.to_lowercase() == "isolation" => {
                if key != "isolation" {
                    return Some(Isolation::Unclear);
                }
                found.extend(values);
            }
            Some(_) => {}
        }
    }
    if fields.is_empty() && ISOLATION_LINE.is_match(text) {
        return Some(Isolation::Unclear); // a key outside anything read as frontmatter
    }
    if found.is_empty() {
        return None;
    }
    let all_worktree = found.iter().all(|v| v.as_deref() == Some("worktree"));
    Some(if all_worktree {
        Isolation::Worktree
    } else {
        Isolation::Unclear
    })
}

/// `open(path, encoding="utf-8").read()`: the text, or the name of the
/// exception Python raises (an `OSError` subclass or `UnicodeDecodeError`).
fn read_agent_file(path: &str) -> Result<String, &'static str> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|e| pyos::os_error_name(&e))?;
    pyos::decode_strict(&bytes).map_err(|f| f.kind)
}

/// `project_dir`: `CLAUDE_PROJECT_DIR`, else the main worktree root of the
/// payload's cwd.
fn project_dir(
    payload: &serde_json::Map<String, Value>,
    env: &HashMap<String, String>,
) -> Result<Option<String>, Fault> {
    if let Some(base) = env.get("CLAUDE_PROJECT_DIR").filter(|b| !b.is_empty()) {
        return Ok(Some(base.clone()));
    }
    let roots = match payload.get("cwd") {
        Some(Value::String(cwd)) if !cwd.is_empty() => Repos::default().get(cwd, &[]).roots()?,
        _ => Vec::new(),
    };
    Ok(roots.last().cloned())
}

/// `tool_input.get(key) or ""` for a key already known to be a string or null.
fn input_str<'a>(input: &'a serde_json::Map<String, Value>, key: &str) -> &'a str {
    input.get(key).and_then(Value::as_str).unwrap_or("")
}

/// `decide_tool`: rule 3 (Agent spawn shape) and rule 4 rows 1-3
/// (EnterWorktree), for a payload whose `tool_name` is Agent, Task or
/// EnterWorktree.
pub fn decide_tool(
    payload: &serde_json::Map<String, Value>,
    env: &HashMap<String, String>,
) -> Result<Verdict, Fault> {
    let mut verdict = Verdict::default();
    if payload.get("tool_name").and_then(Value::as_str) == Some("EnterWorktree") {
        if payload.get("agent_id").is_some_and(|v| !v.is_null()) {
            verdict.rule_deny(
                verdict::R4,
                "agents never call EnterWorktree: isolation: \"worktree\" already placed you in your own worktree; start with git reset --hard <tip> (rule 11)",
            );
        } else if env
            .get("AIOS_TEAM")
            .is_some_and(|t| !pystr::strip(t).is_empty())
        {
            verdict.rule_deny(
                verdict::R4,
                "team leads stay in the main checkout (rule 11)",
            );
        }
        return Ok(verdict);
    }
    let tag = verdict::R3;
    let Some(Value::Object(input)) = payload.get("tool_input") else {
        verdict.ask(
            tag,
            "the Agent call has no readable input; the guard cannot judge the spawn",
        );
        return Ok(verdict);
    };
    for key in ["subagent_type", "name", "isolation", "model"] {
        if input
            .get(key)
            .is_some_and(|v| !v.is_null() && !v.is_string())
        {
            verdict.ask(
                tag,
                format!("the Agent call's {key} is not a string; the guard cannot judge the spawn"),
            );
            return Ok(verdict);
        }
    }
    let agent_type = input_str(input, "subagent_type");
    let name = input_str(input, "name");
    let isolation = input_str(input, "isolation");
    let model = input_str(input, "model");
    if !name.is_empty() && isolation.is_empty() {
        verdict.rule_deny(
            tag,
            "a named spawn without isolation becomes a teammate where agent teams are on; drop the name or add isolation (rule 11)",
        );
    }
    if !PROJECT_TYPE.is_match(agent_type) {
        return Ok(verdict);
    }
    let Some(base) = project_dir(payload, env)? else {
        verdict.ask(
            tag,
            "cannot find the project's agent definitions; the guard cannot judge the spawn",
        );
        return Ok(verdict);
    };
    let path = pyos::join(
        &pyos::join(&pyos::join(&base, ".claude"), "agents"),
        &format!("{agent_type}.md"),
    );
    if !pyos::lexists(&path) {
        return Ok(verdict);
    }
    let text = match read_agent_file(&path) {
        Ok(text) => text,
        Err(kind) => {
            verdict.ask(
                tag,
                format!(
                    "cannot read the agent definition {path} ({kind}); the guard cannot judge the spawn"
                ),
            );
            return Ok(verdict);
        }
    };
    let marker = isolation_marker(&frontmatter(&text), &text);
    if isolation != "worktree" {
        match marker {
            Some(Isolation::Worktree) => verdict.rule_deny(
                tag,
                format!(
                    "{agent_type} is a writer or the verifier: spawn it with isolation: \"worktree\" (rule 11)"
                ),
            ),
            Some(Isolation::Unclear) => verdict.ask(
                tag,
                format!(
                    "{agent_type}'s frontmatter has an isolation key the guard cannot read as `isolation: worktree`; spawn it with isolation: \"worktree\" or fix the file (rule 11)"
                ),
            ),
            None => {}
        }
    }
    if !model.is_empty() {
        verdict.rule_deny(
            tag,
            "the agent's frontmatter sets its model; do not pass model (rule 11)",
        );
    }
    Ok(verdict)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::guard::verdict::Level;
    use std::path::Path;

    /// Frontmatter fields as literals: (key, values) in file order.
    type RawFields = Vec<(Option<&'static str>, Vec<Option<&'static str>>)>;
    /// (payload JSON, environment, level, reasons).
    type SpawnRow = (
        &'static str,
        &'static [(&'static str, &'static str)],
        Level,
        &'static [&'static str],
    );

    fn fields(rows: RawFields) -> Fields {
        rows.into_iter()
            .map(|(k, vs)| {
                (
                    k.map(str::to_string),
                    vs.into_iter().map(|v| v.map(str::to_string)).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn frontmatter_reads_like_python() {
        // frontmatter(text) and isolation_marker(fields, text) on CPython 3.13.
        let rows: Vec<(&str, RawFields, Option<Isolation>)> = vec![
            (
                "---\nname: w\nisolation: worktree\n---\nbody",
                vec![
                    (Some("name"), vec![Some("w")]),
                    (Some("isolation"), vec![Some("worktree")]),
                ],
                Some(Isolation::Worktree),
            ),
            (
                "\u{feff}\n\n---\nisolation: 'worktree' # c\n---",
                vec![(Some("isolation"), vec![Some("worktree")])],
                Some(Isolation::Worktree),
            ),
            (
                "---\nisolation: \"worktree\"\nisolation: x\n---",
                vec![(Some("isolation"), vec![Some("worktree"), Some("x")])],
                Some(Isolation::Unclear),
            ),
            (
                "---\nIsolation: worktree\n---",
                vec![(Some("Isolation"), vec![Some("worktree")])],
                Some(Isolation::Unclear),
            ),
            (
                "--- {isolation: worktree}\n---",
                vec![(None, vec![Some("--- {isolation: worktree}")])],
                Some(Isolation::Unclear),
            ),
            (
                "---\n  a: 1\n  isolation: worktree\nb: 2\n---",
                vec![
                    (Some("a"), vec![Some("1")]),
                    (Some("isolation"), vec![Some("worktree")]),
                    (None, vec![Some("b: 2")]),
                ],
                Some(Isolation::Worktree),
            ),
            (
                "---\na:\n  isolation: worktree\n---",
                vec![(Some("a"), vec![Some("")])],
                None,
            ),
            (
                "---\n- isolation: worktree\n---",
                vec![(None, vec![Some("- isolation: worktree")])],
                Some(Isolation::Unclear),
            ),
            (
                "no frontmatter\nisolation: worktree\n",
                vec![],
                Some(Isolation::Unclear),
            ),
            (
                "---\n---\n  'isolation' : worktree",
                vec![],
                Some(Isolation::Unclear),
            ),
            (
                "---\nisolation: !!str &a worktree\n---",
                vec![(Some("isolation"), vec![Some("worktree")])],
                Some(Isolation::Worktree),
            ),
            (
                "---\nisolation: *alias\n---",
                vec![(Some("isolation"), vec![None])],
                Some(Isolation::Unclear),
            ),
            (
                "---\nisolation: \"work\\x\"\n---",
                vec![(Some("isolation"), vec![None])],
                Some(Isolation::Unclear),
            ),
            (
                "---\nisolation: 'it''s'\n---",
                vec![(Some("isolation"), vec![Some("it's")])],
                Some(Isolation::Unclear),
            ),
            (
                "---\nisolation:\n---",
                vec![(Some("isolation"), vec![Some("")])],
                Some(Isolation::Unclear),
            ),
            (
                "---\nisolation: #c\n---",
                vec![(Some("isolation"), vec![Some("")])],
                Some(Isolation::Unclear),
            ),
            (
                "---\nisolation: worktree#x\n---",
                vec![(Some("isolation"), vec![Some("worktree#x")])],
                Some(Isolation::Unclear),
            ),
            (
                "---\nisolation: worktree #x\n---",
                vec![(Some("isolation"), vec![Some("worktree")])],
                Some(Isolation::Worktree),
            ),
            (
                "---\r\nisolation: worktree\r\n...\r\n",
                vec![(Some("isolation"), vec![Some("worktree")])],
                Some(Isolation::Worktree),
            ),
            (
                "---\n# isolation: x\nmodel: m\n---",
                vec![(Some("model"), vec![Some("m")])],
                None,
            ),
            (
                "---\n\"isolation\": worktree\n---",
                vec![(Some("isolation"), vec![Some("worktree")])],
                Some(Isolation::Worktree),
            ),
            (
                "---\n'iso: x\n---",
                vec![(None, vec![Some("'iso: x")])],
                None,
            ),
            (
                "---\nisolation:\tworktree\u{1f}#x\n---",
                vec![(Some("isolation"), vec![Some("worktree")])],
                Some(Isolation::Worktree),
            ),
            ("", vec![], None),
            ("---", vec![], None),
            (
                "x\n---\nisolation: worktree",
                vec![],
                Some(Isolation::Unclear),
            ),
            (
                "--- # c\n:x: y\n---",
                vec![(Some(""), vec![Some("x: y")])],
                None,
            ),
            (
                "---\nisolation: & \n---",
                vec![(Some("isolation"), vec![Some("")])],
                Some(Isolation::Unclear),
            ),
            (
                "---\n\u{2003}isolation: worktree\n\u{2003}model: m\n---",
                vec![
                    (Some("isolation"), vec![Some("worktree")]),
                    (Some("model"), vec![Some("m")]),
                ],
                Some(Isolation::Worktree),
            ),
            (
                "---\nISOLAT\u{130}ON: worktree\n---",
                vec![(Some("ISOLAT\u{130}ON"), vec![Some("worktree")])],
                None,
            ),
        ];
        for (text, want, marker) in rows {
            let want = fields(want);
            let got = frontmatter(text);
            assert_eq!(got, want, "frontmatter({text:?})");
            assert_eq!(
                isolation_marker(&got, text),
                marker,
                "isolation_marker({text:?})"
            );
        }
    }

    /// A project with one agent file of each kind `decide_tool` reads.
    fn project(dir: &Path) {
        let agents = dir.join(".claude").join("agents");
        fs::create_dir_all(agents.join("dir.md")).expect("agents dir");
        let files: [(&str, &[u8]); 4] = [
            (
                "writer.md",
                b"---\nname: writer\nisolation: worktree\n---\n",
            ),
            ("reader.md", b"---\nname: reader\n---\n"),
            ("odd.md", b"---\nIsolation: worktree\n---\n"),
            ("bad.md", b"---\n\xff\n"),
        ];
        for (name, body) in files {
            fs::write(agents.join(name), body).expect("agent file");
        }
        std::os::unix::fs::symlink(agents.join("nowhere"), agents.join("dangling.md"))
            .expect("symlink");
    }

    #[test]
    fn spawn_decisions_match_python() {
        // decide_tool(payload, env) on CPython 3.13, in the same project layout;
        // {base} is the project directory.
        let rows: &[SpawnRow] = &[
            ("{\"tool_name\": \"EnterWorktree\"}", &[], Level::None, &[]),
            ("{\"tool_name\": \"EnterWorktree\", \"agent_id\": \"a1\"}", &[], Level::Deny, &["agents never call EnterWorktree: isolation: \"worktree\" already placed you in your own worktree; start with git reset --hard <tip> (rule 11)"]),
            ("{\"tool_name\": \"EnterWorktree\", \"agent_id\": null}", &[("AIOS_TEAM", " team-fix ")], Level::Deny, &["team leads stay in the main checkout (rule 11)"]),
            ("{\"tool_name\": \"EnterWorktree\", \"agent_id\": false}", &[], Level::Deny, &["agents never call EnterWorktree: isolation: \"worktree\" already placed you in your own worktree; start with git reset --hard <tip> (rule 11)"]),
            ("{\"tool_name\": \"EnterWorktree\"}", &[("AIOS_TEAM", " \t")], Level::None, &[]),
            ("{\"tool_name\": \"Agent\"}", &[("CLAUDE_PROJECT_DIR", "{base}")], Level::Ask, &["the Agent call has no readable input; the guard cannot judge the spawn"]),
            ("{\"tool_name\": \"Agent\", \"tool_input\": [1]}", &[("CLAUDE_PROJECT_DIR", "{base}")], Level::Ask, &["the Agent call has no readable input; the guard cannot judge the spawn"]),
            ("{\"tool_name\": \"Agent\", \"tool_input\": {\"name\": 5}}", &[("CLAUDE_PROJECT_DIR", "{base}")], Level::Ask, &["the Agent call's name is not a string; the guard cannot judge the spawn"]),
            ("{\"tool_name\": \"Agent\", \"tool_input\": {\"model\": null, \"subagent_type\": \"writer\"}}", &[("CLAUDE_PROJECT_DIR", "{base}")], Level::Deny, &["writer is a writer or the verifier: spawn it with isolation: \"worktree\" (rule 11)"]),
            ("{\"tool_name\": \"Agent\", \"tool_input\": {\"subagent_type\": \"writer\", \"isolation\": \"worktree\"}}", &[("CLAUDE_PROJECT_DIR", "{base}")], Level::None, &[]),
            ("{\"tool_name\": \"Task\", \"tool_input\": {\"subagent_type\": \"writer\", \"name\": \"w\"}}", &[("CLAUDE_PROJECT_DIR", "{base}")], Level::Deny, &["a named spawn without isolation becomes a teammate where agent teams are on; drop the name or add isolation (rule 11)", "writer is a writer or the verifier: spawn it with isolation: \"worktree\" (rule 11)"]),
            ("{\"tool_name\": \"Agent\", \"tool_input\": {\"subagent_type\": \"reader\", \"name\": \"r\", \"model\": \"m\"}}", &[("CLAUDE_PROJECT_DIR", "{base}")], Level::Deny, &["a named spawn without isolation becomes a teammate where agent teams are on; drop the name or add isolation (rule 11)", "the agent's frontmatter sets its model; do not pass model (rule 11)"]),
            ("{\"tool_name\": \"Agent\", \"tool_input\": {\"subagent_type\": \"odd\"}}", &[("CLAUDE_PROJECT_DIR", "{base}")], Level::Ask, &["odd's frontmatter has an isolation key the guard cannot read as `isolation: worktree`; spawn it with isolation: \"worktree\" or fix the file (rule 11)"]),
            ("{\"tool_name\": \"Agent\", \"tool_input\": {\"subagent_type\": \"bad\"}}", &[("CLAUDE_PROJECT_DIR", "{base}")], Level::Ask, &["cannot read the agent definition {base}/.claude/agents/bad.md (UnicodeDecodeError); the guard cannot judge the spawn"]),
            ("{\"tool_name\": \"Agent\", \"tool_input\": {\"subagent_type\": \"dir\"}}", &[("CLAUDE_PROJECT_DIR", "{base}")], Level::Ask, &["cannot read the agent definition {base}/.claude/agents/dir.md (IsADirectoryError); the guard cannot judge the spawn"]),
            ("{\"tool_name\": \"Agent\", \"tool_input\": {\"subagent_type\": \"dangling\"}}", &[("CLAUDE_PROJECT_DIR", "{base}")], Level::Ask, &["cannot read the agent definition {base}/.claude/agents/dangling.md (FileNotFoundError); the guard cannot judge the spawn"]),
            ("{\"tool_name\": \"Agent\", \"tool_input\": {\"subagent_type\": \"missing\"}}", &[("CLAUDE_PROJECT_DIR", "{base}")], Level::None, &[]),
            ("{\"tool_name\": \"Agent\", \"tool_input\": {\"subagent_type\": \"../x\"}}", &[("CLAUDE_PROJECT_DIR", "{base}")], Level::None, &[]),
            ("{\"tool_name\": \"Agent\", \"tool_input\": {\"subagent_type\": \"writer\"}}", &[], Level::Ask, &["cannot find the project's agent definitions; the guard cannot judge the spawn"]),
            ("{\"tool_name\": \"Agent\", \"tool_input\": {\"subagent_type\": \"writer\"}, \"cwd\": \"{base}\"}", &[("CLAUDE_PROJECT_DIR", "")], Level::Ask, &["cannot find the project's agent definitions; the guard cannot judge the spawn"]),
            ("{\"tool_name\": \"Agent\", \"tool_input\": {\"subagent_type\": \"general-purpose\", \"isolation\": \"x\"}}", &[("CLAUDE_PROJECT_DIR", "{base}")], Level::None, &[]),
        ];
        let dir = std::env::temp_dir().join(format!("aios-guard-spawn-{}", std::process::id()));
        project(&dir);
        let base = dir.to_string_lossy().into_owned();
        for &(payload, env, level, reasons) in rows {
            let payload = payload.replace("{base}", &base);
            let Value::Object(payload) = serde_json::from_str(&payload).expect("json") else {
                panic!("an object");
            };
            let env: HashMap<String, String> = env
                .iter()
                .map(|(k, v)| (k.to_string(), v.replace("{base}", &base)))
                .collect();
            let v = decide_tool(&payload, &env).expect("no fault");
            let reasons: Vec<String> = reasons.iter().map(|r| r.replace("{base}", &base)).collect();
            assert_eq!(v.level, level, "{payload:?} {env:?}");
            assert_eq!(v.reasons, reasons, "{payload:?} {env:?}");
            assert!(!v.push_deny);
        }
        fs::remove_dir_all(&dir).expect("remove the project");
    }

    #[test]
    fn the_project_comes_from_the_cwd_repository() {
        // Python: deny for a writer spawned from a subdirectory of a git
        // repository, with no CLAUDE_PROJECT_DIR.
        let dir = std::env::temp_dir().join(format!("aios-guard-spawn-git-{}", std::process::id()));
        project(&dir);
        fs::create_dir_all(dir.join("sub")).expect("sub");
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&dir)
            .status()
            .expect("git");
        assert!(init.success());
        let payload = serde_json::json!({
            "tool_name": "Agent",
            "tool_input": {"subagent_type": "writer"},
            "cwd": dir.join("sub").to_string_lossy(),
        });
        let v =
            decide_tool(payload.as_object().expect("object"), &HashMap::new()).expect("no fault");
        assert_eq!(v.level, Level::Deny);
        assert_eq!(
            v.reasons,
            ["writer is a writer or the verifier: spawn it with isolation: \"worktree\" (rule 11)"]
        );
        fs::remove_dir_all(&dir).expect("remove the repository");
    }
}
