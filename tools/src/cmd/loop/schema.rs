//! The JSON shapes of the loop: the lens and verify output schemas handed to
//! `claude -p --json-schema`, the finding types they decode into, and the two
//! vocabularies they are checked against (the category taxonomy and rule 08's
//! tag list).
//!
//! The schemas use only the JSON-schema features the Task 1 probes exercised:
//! `type`, `properties`, `required`, `additionalProperties`, `enum`, `items`
//! and `["string","null"]` type unions.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The finding categories, in the order of LOOP section 4. A lens may only
/// report one of these; the config must list exactly these.
pub const CATEGORY_TAXONOMY: [&str; 9] = [
    "unsafe-doc",
    "lock-order",
    "concurrency",
    "error-handling",
    "shell-quoting",
    "doc-drift",
    "test-gap",
    "convention",
    "other",
];

/// The tags of `.claude/rules/08-knowledge-hive.md` (its `- Tags:` line).
/// The test `rule08_tags_match_the_rule` keeps this copy in step with the rule.
pub const RULE08_TAGS: [&str; 22] = [
    "kernel",
    "memory",
    "ipc",
    "sched",
    "storage",
    "platform",
    "security",
    "intelligence",
    "boot",
    "mmu",
    "smp",
    "drivers",
    "compositor",
    "gpu",
    "audio",
    "usb",
    "networking",
    "input",
    "wireless",
    "camera",
    "media",
    "tooling",
];

/// How serious a lens finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    MustFix,
    ShouldFix,
    Nit,
}

impl Severity {
    /// The name the schema and the reports use.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::MustFix => "must-fix",
            Severity::ShouldFix => "should-fix",
            Severity::Nit => "nit",
        }
    }
}

/// One lens finding, as the lens schema describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub id: String,
    pub lens: String,
    pub severity: Severity,
    pub category: String,
    pub file: String,
    pub line: u32,
    pub claim: String,
    pub evidence: String,
    pub suggested_fix: Option<String>,
}

/// The lens stage output: `{"findings": [...]}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LensOutput {
    pub findings: Vec<Finding>,
}

/// The verify stage's judgement of one finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerdictKind {
    Confirmed,
    Refuted,
}

/// Whether a finding concerns lines the PR changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    InDiff,
    PreExisting,
}

/// One verify verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verdict {
    pub id: String,
    pub verdict: VerdictKind,
    pub scope: Scope,
    pub reason: String,
}

/// The verify stage output: `{"verdicts": [...]}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyOutput {
    pub verdicts: Vec<Verdict>,
}

fn string() -> Value {
    json!({"type": "string"})
}

fn enum_of(values: &[&str]) -> Value {
    json!({"type": "string", "enum": values})
}

fn object(properties: Vec<(&str, Value)>) -> Value {
    let required: Vec<&str> = properties.iter().map(|(k, _)| *k).collect();
    let props: serde_json::Map<String, Value> = properties
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    json!({
        "type": "object",
        "properties": props,
        "required": required,
        "additionalProperties": false,
    })
}

/// The schema of a lens stage's output.
pub fn lens_schema() -> Value {
    let finding = object(vec![
        ("id", string()),
        ("lens", string()),
        ("severity", enum_of(&["must-fix", "should-fix", "nit"])),
        ("category", enum_of(&CATEGORY_TAXONOMY)),
        ("file", string()),
        ("line", json!({"type": "integer"})),
        ("claim", string()),
        ("evidence", string()),
        ("suggested_fix", json!({"type": ["string", "null"]})),
    ]);
    object(vec![(
        "findings",
        json!({"type": "array", "items": finding}),
    )])
}

/// The schema of the verify stage's output.
pub fn verify_schema() -> Value {
    let verdict = object(vec![
        ("id", string()),
        ("verdict", enum_of(&["confirmed", "refuted"])),
        ("scope", enum_of(&["in-diff", "pre-existing"])),
        ("reason", string()),
    ]);
    object(vec![(
        "verdicts",
        json!({"type": "array", "items": verdict}),
    )])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn required(schema: &Value) -> Vec<String> {
        schema["required"]
            .as_array()
            .expect("required array")
            .iter()
            .map(|v| v.as_str().expect("string").to_string())
            .collect()
    }

    #[test]
    fn taxonomy_has_nine_unique_entries() {
        let set: BTreeSet<&str> = CATEGORY_TAXONOMY.iter().copied().collect();
        assert_eq!(CATEGORY_TAXONOMY.len(), 9);
        assert_eq!(set.len(), 9);
    }

    #[test]
    fn rule08_tags_are_unique() {
        let set: BTreeSet<&str> = RULE08_TAGS.iter().copied().collect();
        assert_eq!(set.len(), RULE08_TAGS.len());
    }

    #[test]
    fn lens_schema_round_trips_one_finding() {
        let instance = json!({"findings": [{
            "id": "c1",
            "lens": "correctness",
            "severity": "must-fix",
            "category": "lock-order",
            "file": "kernel/src/ipc/channel.rs",
            "line": 42,
            "claim": "takes B then A",
            "evidence": "L40 locks B, L42 locks A",
            "suggested_fix": null,
        }]});
        let out: LensOutput = serde_json::from_value(instance.clone()).expect("decode");
        assert_eq!(out.findings[0].severity, Severity::MustFix);
        assert_eq!(out.findings[0].suggested_fix, None);
        assert_eq!(serde_json::to_value(&out).expect("encode"), instance);
    }

    #[test]
    fn lens_schema_requires_exactly_the_finding_fields() {
        let schema = lens_schema();
        assert_eq!(required(&schema), ["findings"]);
        let item = &schema["properties"]["findings"]["items"];
        assert_eq!(item["additionalProperties"], json!(false));
        let finding = Finding {
            id: "a".into(),
            lens: "b".into(),
            severity: Severity::Nit,
            category: "other".into(),
            file: "f".into(),
            line: 1,
            claim: "c".into(),
            evidence: "e".into(),
            suggested_fix: Some("s".into()),
        };
        let encoded = serde_json::to_value(&finding).expect("encode");
        let fields: BTreeSet<String> = encoded
            .as_object()
            .expect("object")
            .keys()
            .cloned()
            .collect();
        let required: BTreeSet<String> = required(item).into_iter().collect();
        assert_eq!(fields, required);
        let properties: BTreeSet<String> = item["properties"]
            .as_object()
            .expect("properties")
            .keys()
            .cloned()
            .collect();
        assert_eq!(properties, required);
    }

    #[test]
    fn lens_schema_category_enum_is_the_taxonomy() {
        let schema = lens_schema();
        let category = &schema["properties"]["findings"]["items"]["properties"]["category"];
        assert_eq!(category["enum"], json!(CATEGORY_TAXONOMY));
    }

    #[test]
    fn verify_schema_round_trips_one_verdict() {
        let instance = json!({"verdicts": [{
            "id": "c1",
            "verdict": "refuted",
            "scope": "pre-existing",
            "reason": "the line is unchanged",
        }]});
        let out: VerifyOutput = serde_json::from_value(instance.clone()).expect("decode");
        assert_eq!(out.verdicts[0].verdict, VerdictKind::Refuted);
        assert_eq!(out.verdicts[0].scope, Scope::PreExisting);
        assert_eq!(serde_json::to_value(&out).expect("encode"), instance);
        let schema = verify_schema();
        let item = &schema["properties"]["verdicts"]["items"];
        assert_eq!(required(item), ["id", "verdict", "scope", "reason"]);
        assert_eq!(
            item["properties"]["scope"]["enum"],
            json!(["in-diff", "pre-existing"])
        );
    }
}
