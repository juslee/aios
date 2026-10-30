//! `scripts/agent/loop-config.json`: the loop's models, budgets, globs,
//! trusted authors, category taxonomy and knowledge-hive selection.
//!
//! The file is strict: an unknown key is an error, and every value is checked
//! on load, so a bad config fails before any stage runs. Errors name the key.

use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use serde::Deserialize;

use super::schema::{CATEGORY_TAXONOMY, RULE08_TAGS};

/// The model aliases a stage may name.
pub const MODEL_ALIASES: [&str; 3] = ["sonnet", "opus", "fable"];

/// Which model each stage runs on.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Models {
    /// The default for the three review lenses.
    pub lens: String,
    /// The correctness and security lenses when the diff touches kernel core.
    pub lens_kernel_core: String,
    /// The verify stage.
    pub verify: String,
    /// The verify stage for a PR labelled `hard`.
    pub verify_hard: String,
    /// What a stage retries on once when a fable call errors.
    pub fallback_on_fable_error: String,
}

/// The knowledge-hive size cap.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HiveCap {
    pub notes: usize,
    pub bytes: usize,
}

/// The loop configuration.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub models: Models,
    /// The `--max-budget-usd` of one stage call.
    pub per_call_usd: f64,
    /// The spend cap of one PR.
    pub per_pr_usd: f64,
    pub stage_timeout_seconds: u64,
    pub kernel_core_globs: Vec<String>,
    pub trusted_authors: Vec<String>,
    pub category_taxonomy: Vec<String>,
    /// Glob to rule 08 tag; the tags of the changed paths select hive notes.
    pub path_tags: BTreeMap<String, String>,
    pub hive_cap: HiveCap,
    /// How many bytes of a failing check's log the facts keep.
    pub log_tail_bytes: usize,
}

impl Config {
    /// Parses and validates a config. Every error names the bad key.
    pub fn from_json(text: &str) -> Result<Config> {
        let config: Config = serde_json::from_str(text).context("loop-config")?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        let models = &self.models;
        for (key, alias) in [
            ("models.lens", &models.lens),
            ("models.lens_kernel_core", &models.lens_kernel_core),
            ("models.verify", &models.verify),
            ("models.verify_hard", &models.verify_hard),
            (
                "models.fallback_on_fable_error",
                &models.fallback_on_fable_error,
            ),
        ] {
            if !MODEL_ALIASES.contains(&alias.as_str()) {
                bail!("{key}: unknown model alias {alias:?} (known: sonnet, opus, fable)");
            }
        }
        for (key, usd) in [
            ("per_call_usd", self.per_call_usd),
            ("per_pr_usd", self.per_pr_usd),
        ] {
            if !usd.is_finite() || usd <= 0.0 {
                bail!("{key}: must be greater than 0, got {usd}");
            }
        }
        if self.per_call_usd > self.per_pr_usd {
            bail!("per_call_usd: exceeds per_pr_usd");
        }
        if self.stage_timeout_seconds == 0 {
            bail!("stage_timeout_seconds: must be greater than 0");
        }
        if self.log_tail_bytes == 0 {
            bail!("log_tail_bytes: must be greater than 0");
        }
        if self.hive_cap.notes == 0 {
            bail!("hive_cap.notes: must be greater than 0");
        }
        if self.hive_cap.bytes == 0 {
            bail!("hive_cap.bytes: must be greater than 0");
        }
        for glob in &self.kernel_core_globs {
            glob_to_regex(glob).with_context(|| format!("kernel_core_globs: {glob:?}"))?;
        }
        for (glob, tag) in &self.path_tags {
            glob_to_regex(glob).with_context(|| format!("path_tags: {glob:?}"))?;
            if !RULE08_TAGS.contains(&tag.as_str()) {
                bail!("path_tags: {glob:?} maps to {tag:?}, which is not a rule 08 tag");
            }
        }
        if self.category_taxonomy != CATEGORY_TAXONOMY {
            bail!(
                "category_taxonomy: must equal {:?}, in that order",
                CATEGORY_TAXONOMY
            );
        }
        Ok(())
    }
}

/// Compiles a path glob to an anchored regex.
///
/// `/` is the separator. `**` matches any run of characters including `/`,
/// `*` matches any run of characters except `/`, and `{a,b}` matches either
/// alternative (no nesting). Every other character is literal.
pub fn glob_to_regex(glob: &str) -> Result<Regex> {
    let mut out = String::from("^");
    let mut in_braces = false;
    let mut chars = glob.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    out.push_str(".*");
                } else {
                    out.push_str("[^/]*");
                }
            }
            '{' => {
                if in_braces {
                    bail!("nested braces in glob {glob:?}");
                }
                in_braces = true;
                out.push_str("(?:");
            }
            '}' => {
                if !in_braces {
                    bail!("unmatched `}}` in glob {glob:?}");
                }
                in_braces = false;
                out.push(')');
            }
            ',' if in_braces => out.push('|'),
            other => out.push_str(&regex::escape(other.encode_utf8(&mut [0; 4]))),
        }
    }
    if in_braces {
        bail!("unclosed `{{` in glob {glob:?}");
    }
    out.push('$');
    Regex::new(&out).map_err(|e| anyhow!("glob {glob:?}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(glob: &str, path: &str) -> bool {
        glob_to_regex(glob).expect("compiles").is_match(path)
    }

    #[test]
    fn glob_table() {
        let rows: [(&str, &str, bool); 14] = [
            ("kernel/src/{arch,sched}/**", "kernel/src/sched/x.rs", true),
            (
                "kernel/src/{arch,sched}/**",
                "kernel/src/arch/a/b/c.S",
                true,
            ),
            ("kernel/src/{arch,sched}/**", "kernel/src/ipcx/a", false),
            ("kernel/src/{arch,sched}/**", "kernel/src/mm/a.rs", false),
            ("kernel/src/ipc/**", "kernel/src/ipc/a/b.rs", true),
            ("kernel/src/ipc/**", "kernel/src/ipcx/a.rs", false),
            ("**", "a/b/c", true),
            ("a/**/c", "a/b/x/c", true),
            ("*.rs", "main.rs", true),
            ("*.rs", "src/main.rs", false),
            ("src/*/mod.rs", "src/a/mod.rs", true),
            ("src/*/mod.rs", "src/a/b/mod.rs", false),
            ("a.b", "axb", false),
            ("a.b", "a.b", true),
        ];
        for (glob, path, want) in rows {
            assert_eq!(matches(glob, path), want, "{glob} vs {path}");
        }
    }

    #[test]
    fn glob_is_anchored() {
        assert!(!matches("tools/**", "x/tools/a"));
        assert!(!matches("tools", "tools/a"));
    }

    #[test]
    fn bad_globs_are_errors() {
        for glob in ["{a,b", "a}", "{a,{b,c}}"] {
            assert!(glob_to_regex(glob).is_err(), "{glob}");
        }
    }

    fn base() -> serde_json::Value {
        serde_json::from_str(include_str!("../../../../scripts/agent/loop-config.json"))
            .expect("committed config is JSON")
    }

    fn error_of(value: &serde_json::Value) -> String {
        format!("{:#}", Config::from_json(&value.to_string()).unwrap_err())
    }

    #[test]
    fn committed_config_validates() {
        Config::from_json(&base().to_string()).expect("valid");
    }

    #[test]
    fn unknown_key_is_named() {
        let mut value = base();
        value["bogus_key"] = serde_json::json!(1);
        assert!(error_of(&value).contains("bogus_key"));
    }

    #[test]
    fn bad_alias_is_named() {
        let mut value = base();
        value["models"]["verify"] = serde_json::json!("gpt");
        let err = error_of(&value);
        assert!(
            err.contains("models.verify") && err.contains("gpt"),
            "{err}"
        );
    }

    #[test]
    fn non_positive_budgets_are_rejected() {
        for (key, bad) in [
            ("per_call_usd", serde_json::json!(0)),
            ("per_pr_usd", serde_json::json!(-1)),
            ("stage_timeout_seconds", serde_json::json!(0)),
            ("log_tail_bytes", serde_json::json!(0)),
        ] {
            let mut value = base();
            value[key] = bad;
            assert!(error_of(&value).contains(key), "{key}");
        }
        for key in ["notes", "bytes"] {
            let mut value = base();
            value["hive_cap"][key] = serde_json::json!(0);
            assert!(error_of(&value).contains(&format!("hive_cap.{key}")));
        }
    }

    #[test]
    fn bad_glob_and_bad_tag_are_named() {
        let mut value = base();
        value["kernel_core_globs"] = serde_json::json!(["kernel/{a"]);
        assert!(error_of(&value).contains("kernel_core_globs"));
        let mut value = base();
        value["path_tags"]["tools/**"] = serde_json::json!("not-a-tag");
        let err = error_of(&value);
        assert!(
            err.contains("path_tags") && err.contains("not-a-tag"),
            "{err}"
        );
    }

    #[test]
    fn taxonomy_must_match_the_schema() {
        let mut value = base();
        value["category_taxonomy"] = serde_json::json!(["other"]);
        assert!(error_of(&value).contains("category_taxonomy"));
    }
}
