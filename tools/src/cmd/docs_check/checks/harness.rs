//! `harness-tables` (check.py L1086-1166 at 33c6b3d): the CLAUDE.md **Skills** and
//! **Agents** tables and the `skills/` and `agents/` lists of its Workspace
//! Layout tree against `.claude/skills` and `.claude/agents`.
//!
//! Accepted divergences from check.py: a skills-dir `plugin.json` is parsed with
//! `serde_json`, which rejects `NaN`/`Infinity` literals, lone surrogate escapes
//! and nesting deeper than 128 levels that Python's `json` module accepts; such a
//! file falls back to the directory name as the plugin name. No tracked
//! plugin.json uses them. `layout_list`'s per-call `entry` regex, and the shared
//! `TREE_PREFIX_RE` (defined in `layout.rs`), use `\s`, which here does not match
//! U+001C..U+001F where Python's `\s` does; no tracked line reaches it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::cmd::docs_check::checks::layout::{layout_block, TREE_PREFIX_RE};
use crate::cmd::docs_check::checks::Check;
use crate::cmd::docs_check::markdown::section_body;
use crate::cmd::docs_check::model::Finding;
use crate::cmd::docs_check::repo::Repo;
use crate::pystr::{lstrip, strip};

const CHECK: &str = "harness-tables";
const CLAUDE_MD: &str = "CLAUDE.md";

/// R54: a skill as a slash command without the slash: `name` or `plugin:name`.
pub const SKILL_NAME: &str = r"[a-z][a-z0-9-]*(?::[a-z][a-z0-9-]*)?";

static SKILL_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(SKILL_NAME).expect("valid regex"));
/// R56: first cell of a Skills table row (anchored, as check.py's `re.match`).
static SKILL_CELL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&["^`/(", SKILL_NAME, ")"].concat()).expect("valid regex"));
/// R56: first cell of an Agents table row.
static AGENT_CELL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^`([a-z0-9-]+)`").expect("valid regex"));
/// R55: a table section ends at the next bold label or `## ` heading.
static TABLE_STOP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\*\*|^## ").expect("valid regex"));
/// R59.
static PLUGIN_JSON_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\.claude/skills/([^/]+)/\.claude-plugin/plugin\.json$").expect("valid regex")
});
static PLUGIN_SKILL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\.claude/skills/([^/]+)/skills/([^/]+)/SKILL\.md$").expect("valid regex")
});
static SKILL_FILE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\.claude/skills/([^/]+)(?:/SKILL\.md)?$").expect("valid regex"));
static AGENT_FILE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\.claude/agents/([^/]+)\.md$").expect("valid regex"));

/// CLAUDE.md skills/agents tables and layout lists vs .claude/.
pub struct HarnessTables;

/// check.py L1116-1142: (skill names, plugin names) as slash commands without
/// the slash. `.claude/skills/<name>/SKILL.md` (or a tracked symlink
/// `.claude/skills/<name>`) is `<name>`; a skills-dir plugin
/// `.claude/skills/<dir>/.claude-plugin/plugin.json` plus
/// `.claude/skills/<dir>/skills/<skill>/SKILL.md` is `<plugin>:<skill>`, where
/// `<plugin>` is plugin.json's non-empty string `name`, else `<dir>`.
pub fn project_skills(repo: &Repo) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut plugins: BTreeMap<String, String> = BTreeMap::new();
    for f in repo.files() {
        if let Some(caps) = PLUGIN_JSON_RE.captures(f) {
            let dir = caps[1].to_string();
            let name = plugin_name(&repo.text(f)).unwrap_or_else(|| dir.clone());
            plugins.insert(dir, name);
        }
    }
    let mut skills = BTreeSet::new();
    for f in repo.files() {
        if let Some(caps) = PLUGIN_SKILL_RE.captures(f) {
            if let Some(plugin) = plugins.get(&caps[1]) {
                skills.insert(format!("{plugin}:{}", &caps[2]));
                continue;
            }
        }
        if let Some(caps) = SKILL_FILE_RE.captures(f) {
            if !plugins.contains_key(&caps[1]) {
                skills.insert(caps[1].to_string());
            }
        }
    }
    (skills, plugins.into_values().collect())
}

/// `json.loads(text).get("name")` when it is a non-empty string; `None` for
/// invalid JSON, a non-object, or a missing, empty or non-string name.
fn plugin_name(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    match value.get("name") {
        Some(serde_json::Value::String(name)) if !name.is_empty() => Some(name.clone()),
        _ => None,
    }
}

/// check.py L1147: `.claude/agents/<name>.md` names.
pub fn project_agents(repo: &Repo) -> BTreeSet<String> {
    repo.files()
        .iter()
        .filter_map(|f| AGENT_FILE_RE.captures(f))
        .map(|caps| caps[1].to_string())
        .collect()
}

/// check.py L1089-1097: group 1 of `rx` (anchored at the cell start) on the
/// first cell of each table row between the line containing `marker` and the
/// next `**` label or `## ` heading of CLAUDE.md.
pub fn claude_table_names(repo: &Repo, marker: &str, rx: &Regex) -> BTreeSet<String> {
    let start = Regex::new(&regex::escape(marker)).expect("an escaped literal is a valid regex");
    let text = repo.text(CLAUDE_MD);
    let mut names = BTreeSet::new();
    for (_, line) in section_body(&text, &start, &TABLE_STOP_RE) {
        if !lstrip(line).starts_with('|') {
            continue;
        }
        let first = strip(line)
            .trim_matches('|')
            .split('|')
            .next()
            .map_or("", strip);
        if let Some(group) = rx.captures(first).and_then(|caps| caps.get(1)) {
            names.insert(group.as_str().to_string());
        }
    }
    names
}

/// check.py L1100-1113: the names on the Workspace Layout tree entry
/// `── <label>/` and its continuation lines (text before the first `(` only),
/// or `None` when the tree has no such entry.
pub fn layout_list(block: &[String], label: &str) -> Option<BTreeSet<String>> {
    let entry = Regex::new(&[r"──\s+", &regex::escape(label), r"/\s+(.*)$"].concat())
        .expect("an escaped label forms a valid regex");
    let mut names = BTreeSet::new();
    let mut found = false;
    for line in block {
        if line.contains("──") {
            if found {
                break;
            }
            if let Some(caps) = entry.captures(line) {
                found = true;
                add_skill_names(&mut names, &caps[1]);
            }
        } else if found {
            add_skill_names(&mut names, &TREE_PREFIX_RE.replace(line, ""));
        }
    }
    found.then_some(names)
}

/// Every SKILL_NAME match in `text` before its first `(`.
fn add_skill_names(names: &mut BTreeSet<String>, text: &str) {
    let head = text.split('(').next().unwrap_or(text);
    names.extend(
        SKILL_NAME_RE
            .find_iter(head)
            .map(|m| m.as_str().to_string()),
    );
}

impl Check for HarnessTables {
    fn name(&self) -> &'static str {
        CHECK
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        let (skills, _) = project_skills(repo);
        let agents = project_agents(repo);
        let table_skills = claude_table_names(repo, "**Skills**", &SKILL_CELL_RE);
        let table_agents = claude_table_names(repo, "**Agents**", &AGENT_CELL_RE);
        let block = layout_block(repo);
        let comparisons = [
            ("skill", &skills, Some(table_skills), "skills-table"),
            ("agent", &agents, Some(table_agents), "agents-table"),
            (
                "skill",
                &skills,
                layout_list(&block, "skills"),
                "layout-skills",
            ),
            (
                "agent",
                &agents,
                layout_list(&block, "agents"),
                "layout-agents",
            ),
        ];
        let mut out = Vec::new();
        for (kind, actual, listed, place) in comparisons {
            let Some(listed) = listed else {
                continue;
            };
            for name in actual.difference(&listed) {
                out.push(Finding::new(
                    CHECK,
                    CLAUDE_MD,
                    format!("{place}-missing:{name}"),
                    format!("CLAUDE.md {place} omits {kind} {name}"),
                    0,
                ));
            }
            for name in listed.difference(actual) {
                out.push(Finding::new(
                    CHECK,
                    CLAUDE_MD,
                    format!("{place}-stale:{name}"),
                    format!("CLAUDE.md {place} lists {kind} {name}, which is not in .claude/"),
                    0,
                ));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn regexes_compile() {
        let all: [&LazyLock<Regex>; 8] = [
            &SKILL_NAME_RE,
            &SKILL_CELL_RE,
            &AGENT_CELL_RE,
            &TABLE_STOP_RE,
            &PLUGIN_JSON_RE,
            &PLUGIN_SKILL_RE,
            &SKILL_FILE_RE,
            &AGENT_FILE_RE,
        ];
        // Forcing each LazyLock runs its Regex::new(...).expect("valid regex"): a
        // bad pattern panics here, at test time, rather than in production.
        for rx in all {
            LazyLock::force(rx);
        }
    }

    #[test]
    fn plugin_name_follows_json_get_name() {
        assert_eq!(plugin_name(r#"{"name": "kit"}"#), Some("kit".to_string()));
        assert_eq!(plugin_name(r#"{"name": ""}"#), None);
        assert_eq!(plugin_name(r#"{"name": 5}"#), None);
        assert_eq!(plugin_name(r#"["name"]"#), None);
        assert_eq!(plugin_name("not json"), None);
        assert_eq!(
            plugin_name("{\"name\": \"a\", \"name\": \"b\"}\n"),
            Some("b".to_string())
        );
    }

    #[test]
    fn layout_list_reads_entry_and_continuations() {
        let block: Vec<String> = [
            "├── .claude/",
            "│   ├── agents/           worker, helper",
            "│   ├── skills/           alpha, kit:go,",
            "│   │                     bad:run, retired (plugin skills as plugin:skill)",
            "│   └── rules/            01-code (auto-loaded)",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            layout_list(&block, "skills"),
            Some(names(&["alpha", "bad:run", "kit:go", "retired"]))
        );
        assert_eq!(
            layout_list(&block, "agents"),
            Some(names(&["helper", "worker"]))
        );
        assert_eq!(layout_list(&block, "hooks"), None);
    }
}
