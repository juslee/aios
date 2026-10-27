//! The loop's committed config and stage prompts (R2a, Task 3).
//!
//! The config must validate through `Config::from_json`, the prompts must start
//! with a version line and name only placeholders the prompt renderer knows, a
//! malformed config must name its bad key, and the copy of rule 08's tag list in
//! `schema::RULE08_TAGS` must match the rule.

use aios_tools::cmd::pr_loop::config::Config;
use aios_tools::cmd::pr_loop::schema::RULE08_TAGS;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(rel: &str) -> String {
    let path = repo().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn committed() -> Value {
    serde_json::from_str(&read("scripts/agent/loop-config.json")).expect("config is JSON")
}

const PROMPTS: [&str; 4] = [
    "review-correctness",
    "review-security",
    "review-conventions",
    "verify",
];

#[test]
fn committed_config_validates() {
    let config = Config::from_json(&read("scripts/agent/loop-config.json")).expect("valid");
    assert_eq!(config.per_call_usd, 0.15);
    assert_eq!(config.per_pr_usd, 5.0);
}

#[test]
fn unknown_key_fails_and_is_named() {
    let mut value = committed();
    value["surprise_key"] = json!(true);
    let err = Config::from_json(&value.to_string()).unwrap_err();
    assert!(format!("{err:#}").contains("surprise_key"), "{err:#}");
}

#[test]
fn bad_alias_fails_and_is_named() {
    let mut value = committed();
    value["models"]["lens"] = json!("haiku");
    let err = Config::from_json(&value.to_string()).unwrap_err();
    let text = format!("{err:#}");
    assert!(
        text.contains("models.lens") && text.contains("haiku"),
        "{text}"
    );
}

#[test]
fn every_prompt_has_a_version_and_only_known_placeholders() {
    let placeholder = Regex::new(r"\{\{[a-z_]+\}\}").expect("regex");
    for name in PROMPTS {
        let rel = format!("scripts/agent/prompts/{name}.md");
        let text = read(&rel);
        assert_eq!(text.lines().next(), Some("version: 1"), "{rel}");
        let mut allowed: BTreeSet<&str> = [
            "{{facts}}",
            "{{prior_findings}}",
            "{{lessons}}",
            "{{untrusted}}",
        ]
        .into_iter()
        .collect();
        if name == "verify" {
            allowed.insert("{{findings}}");
        }
        let found: BTreeSet<&str> = placeholder.find_iter(&text).map(|m| m.as_str()).collect();
        assert!(
            found.is_subset(&allowed),
            "{rel}: {found:?} not within {allowed:?}"
        );
        for needed in [
            "{{facts}}",
            "{{prior_findings}}",
            "{{lessons}}",
            "{{untrusted}}",
        ] {
            assert!(found.contains(needed), "{rel} lacks {needed}");
        }
        assert_eq!(found.contains("{{findings}}"), name == "verify", "{rel}");
    }
}

#[test]
fn every_prompt_stays_short_and_read_only() {
    for name in PROMPTS {
        let rel = format!("scripts/agent/prompts/{name}.md");
        let text = read(&rel);
        assert!(text.lines().count() <= 80, "{rel} is over 80 lines");
        assert!(text.contains("read-only"), "{rel}");
        assert!(text.contains("untrusted fence"), "{rel}");
    }
}

#[test]
fn rule08_tags_match_the_rule() {
    let rule = read(".claude/rules/08-knowledge-hive.md");
    let line = rule
        .lines()
        .find_map(|l| l.strip_prefix("- Tags:"))
        .expect("rule 08 has a `- Tags:` line");
    let tags: Vec<&str> = line.split(',').map(str::trim).collect();
    assert_eq!(tags, RULE08_TAGS);
}
