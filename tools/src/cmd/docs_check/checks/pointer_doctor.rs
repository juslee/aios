//! `pointer-doctor` (check.py L1169-1335 at 33c6b3d): what the harness files under
//! `.claude/agents/`, `.claude/skills/` and `.claude/rules/` point at: CLAUDE.md
//! sections (missing, reduced to a pointer stub, or moved into a rule file),
//! rule files, `docs/` paths, `/skill` commands, agents and `tools:` entries.
//!
//! The section-name patterns (R62) are built from the same parts as check.py's
//! `TITLE_WORD`, `TITLE_LIST`, `BEFORE_CLAUDE_RE`, `AFTER_CLAUDE_RE` and
//! `LABELLED_ITEM_RE`; the `regex` crate's leftmost-first semantics give the
//! same matches and captures as Python's backtracking for them. Accepted
//! divergences: `\s` lacks U+001C..U+001F and `\b` uses the crate's Unicode word
//! definition (no tracked harness file contains either difference); `STUB_RE`'s
//! `(?i)` does not fold `ı` (U+0131) or `İ` (U+0130) to `i` as Python's
//! `re.IGNORECASE` does (verified with `python3 -c`), so a CLAUDE.md section
//! body containing one of those code points where "see"/"is in"/etc. would
//! otherwise match is not detected as a pointer stub here; no tracked CLAUDE.md
//! section body contains either code point; `\d` is `[0-9]` in `LABELLED_ITEM_RE`
//! (check.py L1215) and `RULE_REF_RE` (check.py L1317), so a list item numbered,
//! or a `rules/NN-` reference written, with non-ASCII Unicode decimal digits is
//! not recognised here, where Python's `\d` would; no tracked harness file uses
//! non-ASCII decimal digits in either position.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::cmd::docs_check::checks::harness::{project_agents, project_skills, SKILL_NAME};
use crate::cmd::docs_check::checks::Check;
use crate::cmd::docs_check::markdown::{
    clean_repo_path, code_spans, is_path_placeholder, prose_lines, HEADING_RE,
};
use crate::cmd::docs_check::model::Finding;
use crate::cmd::docs_check::repo::Repo;
use crate::pystr::{split_ws, splitlines, strip};

const CHECK: &str = "pointer-doctor";
const CLAUDE_MD: &str = "CLAUDE.md";
const HARNESS_PREFIXES: [&str; 3] = [".claude/agents/", ".claude/skills/", ".claude/rules/"];

/// Claude Code tool names accepted in agent `tools:` frontmatter (check.py
/// L70-77). `mcp__*` names are accepted as-is. MultiEdit is deliberately absent:
/// it is not a tool in current Claude Code releases.
pub const KNOWN_TOOLS: [&str; 34] = [
    "Agent",
    "AskUserQuestion",
    "Bash",
    "BashOutput",
    "CronCreate",
    "CronDelete",
    "CronList",
    "Edit",
    "EnterPlanMode",
    "EnterWorktree",
    "ExitPlanMode",
    "ExitWorktree",
    "Glob",
    "Grep",
    "KillShell",
    "LSP",
    "ListMcpResourcesTool",
    "Monitor",
    "NotebookEdit",
    "PushNotification",
    "Read",
    "ReadMcpResourceTool",
    "RemoteTrigger",
    "SendMessage",
    "Skill",
    "Task",
    "TaskOutput",
    "TaskStop",
    "TodoWrite",
    "ToolSearch",
    "WebFetch",
    "WebSearch",
    "Workflow",
    "Write",
];

/// Slash commands built into Claude Code rather than project skills (check.py L80-85).
pub const BUILTIN_COMMANDS: [&str; 29] = [
    "add-dir",
    "agents",
    "clear",
    "compact",
    "config",
    "context",
    "cost",
    "doctor",
    "exit",
    "goal",
    "help",
    "hooks",
    "init",
    "loop",
    "mcp",
    "memory",
    "model",
    "permissions",
    "plan",
    "resume",
    "review",
    "schedule",
    "security-review",
    "simplify",
    "code-review",
    "status",
    "statusline",
    "tasks",
    "todos",
];

/// Built-in subagent types that need no `.claude/agents/` definition (check.py L88).
pub const BUILTIN_AGENTS: [&str; 5] = [
    "general-purpose",
    "Explore",
    "Plan",
    "statusline-setup",
    "output-style-setup",
];

/// R62 `TITLE_WORD`.
const TITLE_WORD: &str = r"[A-Z][A-Za-z0-9&/-]*";

/// R62 `TITLE_LIST`: a run of Title Case section names joined by commas, "and",
/// or a parenthetical aside: "Code Conventions (`.claude/rules/`) and Quality Gates".
fn title_list() -> String {
    [
        "(",
        TITLE_WORD,
        r"(?:\s*\([^)]*\)|\s*,\s*|\s+and\s+|\s+|",
        TITLE_WORD,
        ")*)",
    ]
    .concat()
}

/// R62 `BEFORE_CLAUDE_RE`: "<Title Words> in CLAUDE.md".
static BEFORE_CLAUDE_RE: LazyLock<Regex> = LazyLock::new(|| {
    let pattern = [
        "((?:",
        TITLE_WORD,
        r"\s+){0,6}",
        TITLE_WORD,
        r#")["'”*]*\s*\(?\s*(?:in|from)\s+`?CLAUDE\.md\b"#,
    ]
    .concat();
    Regex::new(&pattern).expect("valid regex")
});
/// R62 `AFTER_CLAUDE_RE`: "CLAUDE.md: <title list>".
static AFTER_CLAUDE_RE: LazyLock<Regex> = LazyLock::new(|| {
    let pattern = [r#"`?CLAUDE\.md`?\s*(:)?\s*["“]?"#, title_list().as_str()].concat();
    Regex::new(&pattern).expect("valid regex")
});
/// R62 `LABELLED_ITEM_RE`: "2. Update: Workspace Layout, Key Technical Facts"
/// under a heading that names CLAUDE.md.
static LABELLED_ITEM_RE: LazyLock<Regex> = LazyLock::new(|| {
    let pattern = [
        r"^\s*(?:[-*+]|[0-9]+[.)])\s+(?:\*\*)?[A-Za-z][A-Za-z ]{0,30}?(?:\*\*)?:\s*",
        title_list().as_str(),
    ]
    .concat();
    Regex::new(&pattern).expect("valid regex")
});
/// R63.
static TITLE_SPLIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\([^)]*\)|\band\b").expect("valid regex"));
/// R61: a CLAUDE.md section body that only points elsewhere.
static STUB_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(lives in|moved to|are in|is in|see)\b").expect("valid regex")
});
/// R64: the frontmatter block (LF only, as check.py L1283).
static FRONTMATTER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)^---\n(.*?)\n---").expect("valid regex"));
/// R65.
static TOOLS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^tools:\s*(.*)$").expect("valid regex"));
/// R66.
static RULE_REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\.claude/)?rules/([0-9]{2}-[a-z0-9-]+\.md)").expect("valid regex")
});
/// R67: a code span that is a slash command.
static SKILL_SPAN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&["^/(", SKILL_NAME, r")(?:\s|$)"].concat()).expect("valid regex"));
/// R68: "`name` agent", "`name` subagent" or "subagent_type: name".
static AGENT_REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"`([a-z][a-z0-9-]*)`\s+(?:agent|subagent)\b|subagent_type:\s*`?([A-Za-z][A-Za-z0-9-]*)",
    )
    .expect("valid regex")
});

/// CLAUDE.md sections, rules, paths, skills, agents, tools named by .claude/.
pub struct PointerDoctor;

/// A candidate CLAUDE.md section name, as words. `Before` came from
/// "<words> in CLAUDE.md" (try dropping leading words), `After` from
/// "CLAUDE.md <words>" or a labelled list item (try dropping trailing words).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phrase {
    Before(Vec<String>),
    After(Vec<String>),
}

/// check.py `resolve_phrase` statuses: `Ok`/`Stub` carry the CLAUDE.md heading,
/// `Moved` the phrase and the rule file, `Missing` the whole phrase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Ok(String),
    Stub(String),
    Moved { section: String, rule: String },
    Missing(String),
    Ignore,
}

/// check.py L1169-1173: lowercase, `&` and every char outside `[a-z0-9 ]`
/// become spaces, drop "and"/"the", "doc" → "document", single spaces.
pub fn norm_section(name: &str) -> String {
    let mapped: String = name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == ' ' {
                c
            } else {
                ' '
            }
        })
        .collect();
    split_ws(&mapped)
        .into_iter()
        .filter(|w| *w != "and" && *w != "the")
        .map(|w| if w == "doc" { "document" } else { w })
        .collect::<Vec<_>>()
        .join(" ")
}

/// check.py L1176-1192: normalized `## ` heading → (heading, is a pointer stub).
/// A stub has at most two non-blank, non-`---` body lines and says where the
/// content lives now. Later headings with the same key win.
fn claude_sections(repo: &Repo) -> HashMap<String, (String, bool)> {
    let text = repo.text(CLAUDE_MD);
    let lines = splitlines(&text);
    let mut out = HashMap::new();
    for heading in repo.headings(CLAUDE_MD).iter() {
        if heading.level != 2 {
            continue;
        }
        let body: Vec<&str> = lines
            .iter()
            .skip(heading.line)
            .take_while(|line| !line.starts_with("## "))
            .filter(|line| {
                let s = strip(line);
                !s.is_empty() && s != "---"
            })
            .copied()
            .collect();
        let stub = body.len() <= 2 && STUB_RE.is_match(&body.join(" "));
        out.insert(norm_section(&heading.text), (heading.text.clone(), stub));
    }
    out
}

/// check.py L1195-1203: normalized `# `/`## ` heading → the first rule file
/// under `.claude/rules/` that has it.
fn rule_titles(repo: &Repo) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for f in repo.md_files() {
        if !f.starts_with(".claude/rules/") {
            continue;
        }
        for heading in repo.headings(f).iter() {
            if heading.level <= 2 {
                out.entry(norm_section(&heading.text))
                    .or_insert_with(|| f.clone());
            }
        }
    }
    out
}

/// check.py L1218-1234: split a TITLE_LIST match into section-name word lists.
/// "and" and parenthetical asides always separate names; commas only when
/// `all_parts` (after a colon), otherwise the list ends at the first comma.
fn split_title_list(text: &str, all_parts: bool) -> Vec<Vec<String>> {
    let chunks = text.split(',').take(if all_parts { usize::MAX } else { 1 });
    let mut out = Vec::new();
    for chunk in chunks {
        for part in TITLE_SPLIT_RE.split(chunk) {
            let words = split_ws(part);
            if !words.is_empty() {
                out.push(words.into_iter().map(str::to_string).collect());
            }
        }
    }
    out
}

/// check.py L1237-1244: phrases before and after each `CLAUDE.md` on a line.
fn section_candidates(line: &str) -> Vec<Phrase> {
    let mut phrases = Vec::new();
    for caps in BEFORE_CLAUDE_RE.captures_iter(line) {
        let words = split_ws(caps.get(1).map_or("", |m| m.as_str()));
        phrases.push(Phrase::Before(
            words.into_iter().map(str::to_string).collect(),
        ));
    }
    for caps in AFTER_CLAUDE_RE.captures_iter(line) {
        let list = caps.get(2).map_or("", |m| m.as_str());
        for words in split_title_list(list, caps.get(1).is_some()) {
            phrases.push(Phrase::After(words));
        }
    }
    phrases
}

/// check.py L1247-1249: the names of a labelled list item, as `After` phrases.
fn labelled_item_candidates(line: &str) -> Vec<Phrase> {
    match LABELLED_ITEM_RE.captures(line).and_then(|caps| caps.get(1)) {
        Some(list) => split_title_list(list.as_str(), true)
            .into_iter()
            .map(Phrase::After)
            .collect(),
        None => Vec::new(),
    }
}

/// check.py L1252-1270. Options are the suffixes (`Before`) or prefixes
/// (`After`) of the phrase, longest first; a CLAUDE.md section wins over a rule
/// heading; an unresolved single word is ignored.
pub fn resolve_phrase(
    p: &Phrase,
    sections: &HashMap<String, (String, bool)>,
    rules: &HashMap<String, String>,
) -> Resolution {
    let (words, before) = match p {
        Phrase::Before(words) => (words, true),
        Phrase::After(words) => (words, false),
    };
    let n = words.len();
    let options: Vec<&[String]> = if before {
        (0..n).map(|i| &words[i..]).collect()
    } else {
        (1..=n).rev().map(|j| &words[..j]).collect()
    };
    for opt in &options {
        if let Some((heading, stub)) = sections.get(&norm_section(&opt.join(" "))) {
            return if *stub {
                Resolution::Stub(heading.clone())
            } else {
                Resolution::Ok(heading.clone())
            };
        }
    }
    for opt in &options {
        let name = opt.join(" ");
        if let Some(rule) = rules.get(&norm_section(&name)) {
            // check.py returns "name|rule" and its caller splits at the first '|'.
            return match name.split_once('|') {
                Some((section, rest)) => Resolution::Moved {
                    section: section.to_string(),
                    rule: format!("{rest}|{rule}"),
                },
                None => Resolution::Moved {
                    section: name.clone(),
                    rule: rule.clone(),
                },
            };
        }
    }
    if n < 2 {
        Resolution::Ignore
    } else {
        Resolution::Missing(words.join(" "))
    }
}

/// check.py L1282-1290: unknown tools in the `tools:` frontmatter line; line
/// numbers count the frontmatter body from 2.
fn tool_findings(rel: &str, text: &str, out: &mut Vec<Finding>) {
    let Some(body) = FRONTMATTER_RE.captures(text).and_then(|caps| caps.get(1)) else {
        return;
    };
    for (i, line) in splitlines(body.as_str()).into_iter().enumerate() {
        let Some(list) = TOOLS_RE.captures(line).and_then(|caps| caps.get(1)) else {
            continue;
        };
        for tool in list
            .as_str()
            .split(',')
            .map(strip)
            .filter(|t| !t.is_empty())
        {
            if !KNOWN_TOOLS.contains(&tool) && !tool.starts_with("mcp__") {
                out.push(Finding::new(
                    CHECK,
                    rel,
                    format!("tool:{tool}"),
                    format!("tools: lists unknown tool {tool}"),
                    i + 2,
                ));
            }
        }
    }
}

/// Facts about `.claude/` and CLAUDE.md shared by every harness file.
struct Context<'a> {
    repo: &'a Repo,
    sections: HashMap<String, (String, bool)>,
    rules: HashMap<String, String>,
    skills: BTreeSet<String>,
    plugins: BTreeSet<String>,
    agents: BTreeSet<String>,
}

/// check.py L1299-1334 for one prose line of `rel`.
fn line_findings(
    ctx: &Context<'_>,
    rel: &str,
    lineno: usize,
    line: &str,
    phrases: &[Phrase],
    out: &mut Vec<Finding>,
) {
    let mut line_keys: HashSet<String> = HashSet::new();
    for phrase in phrases {
        let (target, message) = match resolve_phrase(phrase, &ctx.sections, &ctx.rules) {
            Resolution::Stub(name) => (
                format!("claude-md:{name}"),
                format!("points to CLAUDE.md '{name}', which is only a pointer stub now"),
            ),
            Resolution::Moved { section, rule } => (
                format!("claude-md:{section}"),
                format!("points to CLAUDE.md '{section}', which now lives in {rule}"),
            ),
            Resolution::Missing(name) => (
                format!("claude-md:{name}"),
                format!("points to CLAUDE.md '{name}', which is not a section of CLAUDE.md"),
            ),
            Resolution::Ok(_) | Resolution::Ignore => continue,
        };
        let finding = Finding::new(CHECK, rel, target, message, lineno);
        if line_keys.insert(finding.key()) {
            out.push(finding);
        }
    }
    for caps in RULE_REF_RE.captures_iter(line) {
        let file = caps.get(1).map_or("", |m| m.as_str());
        if !ctx.repo.is_file(&format!(".claude/rules/{file}")) {
            out.push(Finding::new(
                CHECK,
                rel,
                format!("rules:{file}"),
                format!("rule file {file} does not exist"),
                lineno,
            ));
        }
    }
    for span in code_spans(line) {
        if span.starts_with("docs/") {
            let path = clean_repo_path(split_ws(&span).first().copied().unwrap_or(""));
            if !is_path_placeholder(&path) && !ctx.repo.exists(&path) {
                out.push(Finding::new(
                    CHECK,
                    rel,
                    format!("path:{path}"),
                    format!("path does not exist: {path}"),
                    lineno,
                ));
            }
        }
        if let Some(name) = SKILL_SPAN_RE.captures(&span).and_then(|caps| caps.get(1)) {
            let name = name.as_str();
            // /<plugin>:<skill> is checked only for this repo's plugins; others are installed per user.
            let checked = match name.split_once(':') {
                Some((plugin, _)) => ctx.plugins.contains(plugin),
                None => true,
            };
            if checked && !ctx.skills.contains(name) && !BUILTIN_COMMANDS.contains(&name) {
                out.push(Finding::new(
                    CHECK,
                    rel,
                    format!("skill:/{name}"),
                    format!("/{name} is not a project skill or built-in command"),
                    lineno,
                ));
            }
        }
    }
    for caps in AGENT_REF_RE.captures_iter(line) {
        let Some(name) = caps.get(1).or_else(|| caps.get(2)).map(|m| m.as_str()) else {
            continue;
        };
        if !ctx.agents.contains(name) && !BUILTIN_AGENTS.contains(&name) {
            out.push(Finding::new(
                CHECK,
                rel,
                format!("agent:{name}"),
                format!("agent {name} is not defined in .claude/agents"),
                lineno,
            ));
        }
    }
}

impl Check for PointerDoctor {
    fn name(&self) -> &'static str {
        CHECK
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        let (skills, plugins) = project_skills(repo);
        let ctx = Context {
            repo,
            sections: claude_sections(repo),
            rules: rule_titles(repo),
            skills,
            plugins,
            agents: project_agents(repo),
        };
        let mut out = Vec::new();
        for rel in repo.md_files() {
            if !HARNESS_PREFIXES
                .iter()
                .any(|&prefix| rel.starts_with(prefix))
            {
                continue;
            }
            let text = repo.text(rel);
            tool_findings(rel, &text, &mut out);
            let mut under_claude_heading = false;
            for (lineno, line) in prose_lines(&text) {
                let heading = HEADING_RE.captures(line);
                if let Some(caps) = &heading {
                    under_claude_heading =
                        caps.get(2).is_some_and(|m| m.as_str().contains(CLAUDE_MD));
                }
                let mut phrases = if line.contains(CLAUDE_MD) {
                    section_candidates(line)
                } else {
                    Vec::new()
                };
                if under_claude_heading && heading.is_none() {
                    phrases.extend(labelled_item_candidates(line));
                }
                line_findings(&ctx, rel, lineno, line, &phrases, &mut out);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn before(items: &[&str]) -> Phrase {
        Phrase::Before(words(items))
    }

    fn after(items: &[&str]) -> Phrase {
        Phrase::After(words(items))
    }

    #[test]
    fn regexes_compile() {
        let all: [&LazyLock<Regex>; 10] = [
            &BEFORE_CLAUDE_RE,
            &AFTER_CLAUDE_RE,
            &LABELLED_ITEM_RE,
            &TITLE_SPLIT_RE,
            &STUB_RE,
            &FRONTMATTER_RE,
            &TOOLS_RE,
            &RULE_REF_RE,
            &SKILL_SPAN_RE,
            &AGENT_REF_RE,
        ];
        // Forcing each LazyLock runs its Regex::new(...).expect("valid regex"): a
        // bad pattern panics here, at test time, rather than in production.
        for rx in all {
            LazyLock::force(rx);
        }
    }

    #[test]
    fn norm_section_matches_check_py() {
        assert_eq!(
            norm_section("Architecture Doc Map"),
            "architecture document map"
        );
        assert_eq!(norm_section("The Code & Conventions"), "code conventions");
        assert_eq!(norm_section("Key-Technical_Facts!"), "key technical facts");
        assert_eq!(norm_section("and the"), "");
        assert_eq!(norm_section("ÉTAT Doc"), "tat document");
    }

    #[test]
    fn section_candidates_match_check_py() {
        assert_eq!(
            section_candidates(
                "Follow the Code Conventions in CLAUDE.md and the Quality Gates from CLAUDE.md."
            ),
            vec![
                before(&["Code", "Conventions"]),
                before(&["Quality", "Gates"])
            ]
        );
        assert_eq!(
            section_candidates(
                "Check `CLAUDE.md`: Workspace Layout, Key Technical Facts and Build Matrix."
            ),
            vec![
                after(&["Workspace", "Layout"]),
                after(&["Key", "Technical", "Facts"]),
                after(&["Build", "Matrix"])
            ]
        );
        assert_eq!(
            section_candidates("See CLAUDE.md Workspace Layout, then the rest."),
            vec![after(&["Workspace", "Layout"])]
        );
        assert_eq!(
            section_candidates("Read \"Key Facts\" (in CLAUDE.md) today."),
            vec![before(&["Key", "Facts"])]
        );
        assert_eq!(
            section_candidates(
                "The CLAUDE.md “Code Conventions (`.claude/rules/`) and Quality Gates” sections."
            ),
            vec![
                after(&["Code", "Conventions"]),
                after(&["Quality", "Gates"])
            ]
        );
    }

    #[test]
    fn labelled_item_candidates_match_check_py() {
        assert_eq!(
            labelled_item_candidates("1. Update: Workspace Layout, Key Technical Facts"),
            vec![
                after(&["Workspace", "Layout"]),
                after(&["Key", "Technical", "Facts"])
            ]
        );
        assert_eq!(
            labelled_item_candidates("- **Also**: Architecture Doc Map (the index)"),
            vec![after(&["Architecture", "Doc", "Map"])]
        );
        assert_eq!(
            labelled_item_candidates("   * Note:Build Matrix and Test Plan"),
            vec![after(&["Build", "Matrix"]), after(&["Test", "Plan"])]
        );
        assert_eq!(
            labelled_item_candidates("Update: Workspace Layout"),
            Vec::new()
        );
    }

    #[test]
    fn split_title_list_matches_check_py() {
        assert_eq!(
            split_title_list("Workspace Layout, Key Facts and Build (x, y) Matrix", true),
            vec![
                words(&["Workspace", "Layout"]),
                words(&["Key", "Facts"]),
                words(&["Build", "(x"]),
                words(&["y)", "Matrix"])
            ]
        );
        assert_eq!(
            split_title_list("Workspace Layout, then more", false),
            vec![words(&["Workspace", "Layout"])]
        );
    }

    #[test]
    fn resolve_phrase_matches_check_py() {
        let sections: HashMap<String, (String, bool)> = [
            ("workspace layout", "Workspace Layout", false),
            (
                "architecture document map",
                "Architecture Document Map",
                true,
            ),
        ]
        .into_iter()
        .map(|(key, heading, stub)| (key.to_string(), (heading.to_string(), stub)))
        .collect();
        let rules: HashMap<String, String> = [
            ("code conventions", ".claude/rules/01-code-conventions.md"),
            ("b c", ".claude/rules/09-x.md"),
        ]
        .into_iter()
        .map(|(key, file)| (key.to_string(), file.to_string()))
        .collect();
        let resolve = |p: Phrase| resolve_phrase(&p, &sections, &rules);
        assert_eq!(
            resolve(before(&["Follow", "Workspace", "Layout"])),
            Resolution::Ok("Workspace Layout".to_string())
        );
        assert_eq!(
            resolve(after(&["Workspace", "Layout", "Rules"])),
            Resolution::Ok("Workspace Layout".to_string())
        );
        assert_eq!(
            resolve(before(&["Architecture", "Doc", "Map"])),
            Resolution::Stub("Architecture Document Map".to_string())
        );
        assert_eq!(
            resolve(after(&["Code", "Conventions", "Now"])),
            Resolution::Moved {
                section: "Code Conventions".to_string(),
                rule: ".claude/rules/01-code-conventions.md".to_string()
            }
        );
        assert_eq!(
            resolve(after(&["Build", "Matrix"])),
            Resolution::Missing("Build Matrix".to_string())
        );
        assert_eq!(resolve(before(&["Glossary"])), Resolution::Ignore);
        // check.py splits "b|c)|<rule>" at the first '|'.
        assert_eq!(
            resolve(after(&["b|c)"])),
            Resolution::Moved {
                section: "b".to_string(),
                rule: "c)|.claude/rules/09-x.md".to_string()
            }
        );
    }

    #[test]
    fn case_folding_does_not_reach_turkish_dotted_and_dotless_i() {
        // check.py's re.IGNORECASE folds ı (U+0131) and İ (U+0130) to plain ASCII `i`, so
        // "ıs ın" matches r"\b(...|is in|...)\b" (verified with python3 -c); the regex
        // crate's (?i) does not, so this stub body goes undetected (accepted divergence,
        // documented above).
        assert!(!STUB_RE.is_match("content ıs ın here"));
        // Same divergence, uppercase: check.py reports a stub for "CONTENT İS İN
        // HERE" too (verified with python3 -c: re.IGNORECASE folds İ to i, so
        // r"\b(...|is in|...)\b" matches "İS İN"); the regex crate's (?i) does not.
        assert!(!STUB_RE.is_match("CONTENT İS İN HERE"));
    }

    #[test]
    fn ascii_only_digit_class_does_not_reach_non_ascii_decimal_digits() {
        // check.py's \d matches any Unicode decimal digit, so LABELLED_ITEM_RE
        // (L1215) treats "١. Update: ..." as a labelled item and RULE_REF_RE's
        // finditer (L1317) matches "rules/٠١-x.md" (verified with python3 -c);
        // [0-9] here does not, so both go unrecognised (accepted divergence,
        // documented above).
        assert!(labelled_item_candidates("١. Update: Arabic Digit Item").is_empty());
        assert!(!RULE_REF_RE.is_match("rules/٠١-x.md"));
    }
}
