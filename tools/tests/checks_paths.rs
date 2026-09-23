//! Path checks doc-map, repo-paths and just-recipes (check.py L701-784).
//!
//! The expected findings were recorded by calling check.py's own `check_doc_map`,
//! `check_repo_paths` and `check_just_recipes` on the same files (production order).

mod common;

use std::collections::BTreeSet;

use aios_tools::cmd::docs_check::checks::doc_map::{DocMap, DOC_MAP_ALLOWLIST, DOC_MAP_REL};
use aios_tools::cmd::docs_check::checks::just_recipes::{documented_recipes, JustRecipes};
use aios_tools::cmd::docs_check::checks::repo_paths::RepoPaths;
use aios_tools::cmd::docs_check::checks::{registry, Check};
use aios_tools::cmd::docs_check::model::{Finding, Skip};
use aios_tools::cmd::docs_check::repo::Repo;
use common::TestRepo;

const DOC_MAP_README_MD: &str = r#"# Readme
"#;
const DOC_MAP_DOCS_PROJECT_DOC_MAP_MD: &str = r#"# Doc Map

| Topic | Doc |
|---|---|
| Kernel | `docs/kernel/{alpha,beta}.md` |
| Nested | `docs/{kernel/{gamma,delta},project/plan}.md` |
| Directory | `docs/kernel/` |
| Code | `kernel/src/main.rs` |

<!-- `docs/commented.md` -->

```text
`docs/fenced.md`
```
"#;
const DOC_MAP_DOCS_KERNEL_ALPHA_MD: &str = r#"# Alpha
"#;
const DOC_MAP_DOCS_KERNEL_GAMMA_MD: &str = r#"# Gamma
"#;
const DOC_MAP_DOCS_KERNEL_UNLISTED_MD: &str = r#"# Unlisted
"#;
const DOC_MAP_DOCS_KERNEL_SUB_DEEP_MD: &str = r#"# Deep
"#;
const DOC_MAP_DOCS_KERNEL_DIAGRAM_SVG: &str = r#"<svg/>
"#;
const DOC_MAP_DOCS_PROJECT_PLAN_MD: &str = r#"# Plan
"#;
const DOC_MAP_DOCS_PHASES_00_BOOT_MD: &str = r#"# Phase 0: Boot
"#;
const DOC_MAP_DOCS_KNOWLEDGE_DECISIONS_2026_01_01_AB_CHOICE_MD: &str = r#"# Choice
"#;
const MISSING_README_MD: &str = r#"# Readme
"#;
const MISSING_DOCS_KERNEL_ALPHA_MD: &str = r#"# Alpha
"#;
const PATHS_CLAUDE_MD: &str = r#"# Project

Paths `kernel/src/main.rs`, `kernel/src/gone.rs`, `shared/src/lib.rs:12` and `kernel/src/main.rs::kernel_main`.
Ranges `scripts/tool.sh:3-5,9`, directories `uefi-stub/` and `kernel/src/missing/`.
Placeholders `kernel/src/<name>.rs`, `shared/BootInfo`, `kernel/src/NN-x.rs`, `kernel/src/Foo` and `docs/nope.md`.
Punctuation `kernel/src/gone2.rs).`, words `kernel/src/main.rs extra words` and prefix `kernelx/src/a.rs`.
Comments are not masked: <!-- `kernel/src/in-comment.rs` --> and ``kernel/src/double.rs`` counts.

```text
`kernel/src/fenced.rs`
```
"#;
const PATHS__CLAUDE_AGENTS_READER_MD: &str = r#"# Reader

Reads `kernel/src/agent-missing.rs` and `kernel/src/main.rs`.
"#;
const PATHS_DOCS_OTHER_MD: &str = r#"Not current state: `kernel/src/ignored.rs`.
"#;
const PATHS_DOCS_PROJECT_DEVELOPER_GUIDE_MD: &str = r#"# Developer Guide

Run `scripts/nope.py` first.

```text
`kernel/src/fenced2.rs`
```
"#;
const PATHS_DOCS_PHASES_01_MEMORY_MD: &str = r#"# Phase 1: Memory

## Milestone 3 — Done

Uses `kernel/src/merged-missing.rs`.

## Milestone 4 — Planned

Will add `kernel/src/future.rs`.
"#;
const PATHS_KERNEL_SRC_MAIN_RS: &str = r#"fn main() {}
"#;
const PATHS_SHARED_SRC_LIB_RS: &str = r#"//! Shared.
"#;
const PATHS_SCRIPTS_TOOL_SH: &str = r#"#!/bin/sh
"#;
const PATHS_UEFI_STUB_SRC_MAIN_RS: &str = r#"fn main() {}
"#;
const JUST_JUSTFILE: &str = r#"# Test justfile

set shell := ["bash", "-c"]
target := "aarch64"

default: build

build:
    echo build

# Documented in the README prose only
check: build
    echo check

[private]
helper:
    echo helper

[no-cd]
[private]
stacked:
    echo stacked

_hidden:
    echo hidden

undoc:
    echo undoc

[positional-arguments]
docs-check *args:
    echo "$@"

@quiet:
    echo quiet
"#;
const JUST_README_MD: &str = r#"# Readme

## Build Commands

| Command | Description |
|---|---|
| `just build` | Build |
| `just docs-check --all` | Docs drift |

Prose `just check` is not a table row.

## Other

| `just undoc` | outside the section |

Run `just nope`, `just build`, `just helper` and `just _hidden`; `just` alone and `justify` are not recipes.
"#;
const JUST_DOCS_PROJECT_DEVELOPER_GUIDE_MD: &str = r#"# Developer Guide

## 5. Build

### 5.1 Just Commands

| Command | What it does |
|---|---|
| `just quiet` | Quiet |

Also run `just gone`.

### 5.2 Tests

| `just check` | after the section |
"#;
const JUST__CLAUDE_SKILLS_S_SKILL_MD: &str = r#"# Skill

Run `just ghost` then `just stacked`.
"#;
const JUST_DOCS_OTHER_MD: &str = r#"Run `just other-missing`.
"#;
const NO_JUST_README_MD: &str = r#"Run `just build`.
"#;

fn open(t: &TestRepo) -> Repo {
    Repo::open(t.path_str()).expect("open the test repository")
}

fn finding(check: &'static str, file: &str, target: &str, message: &str, line: usize) -> Finding {
    Finding::new(check, file, target, message, line)
}

#[test]
fn doc_map_reports_missing_listed_paths_then_unlisted_docs() {
    let t = TestRepo::with_files(
        "doc-map",
        &[
            ("README.md", DOC_MAP_README_MD),
            ("docs/project/doc-map.md", DOC_MAP_DOCS_PROJECT_DOC_MAP_MD),
            ("docs/kernel/alpha.md", DOC_MAP_DOCS_KERNEL_ALPHA_MD),
            ("docs/kernel/gamma.md", DOC_MAP_DOCS_KERNEL_GAMMA_MD),
            ("docs/kernel/unlisted.md", DOC_MAP_DOCS_KERNEL_UNLISTED_MD),
            ("docs/kernel/sub/deep.md", DOC_MAP_DOCS_KERNEL_SUB_DEEP_MD),
            ("docs/kernel/diagram.svg", DOC_MAP_DOCS_KERNEL_DIAGRAM_SVG),
            ("docs/project/plan.md", DOC_MAP_DOCS_PROJECT_PLAN_MD),
            ("docs/phases/00-boot.md", DOC_MAP_DOCS_PHASES_00_BOOT_MD),
            (
                "docs/knowledge/decisions/2026-01-01-ab-choice.md",
                DOC_MAP_DOCS_KNOWLEDGE_DECISIONS_2026_01_01_AB_CHOICE_MD,
            ),
        ],
    );
    let repo = open(&t);
    let got = DocMap.run(&repo).expect("doc-map runs");
    let want = vec![
        finding(
            "doc-map",
            "docs/project/doc-map.md",
            "missing:docs/kernel/beta.md",
            "listed path does not exist: docs/kernel/beta.md",
            5,
        ),
        finding(
            "doc-map",
            "docs/project/doc-map.md",
            "missing:docs/kernel/delta.md",
            "listed path does not exist: docs/kernel/delta.md",
            6,
        ),
        finding(
            "doc-map",
            "docs/project/doc-map.md",
            "missing:docs/commented.md",
            "listed path does not exist: docs/commented.md",
            10,
        ),
        finding(
            "doc-map",
            "docs/kernel/sub/deep.md",
            "unlisted",
            "docs/kernel/sub/deep.md is not listed in docs/project/doc-map.md",
            0,
        ),
        finding(
            "doc-map",
            "docs/kernel/unlisted.md",
            "unlisted",
            "docs/kernel/unlisted.md is not listed in docs/project/doc-map.md",
            0,
        ),
    ];
    assert_eq!(got, want);
}

#[test]
fn doc_map_reports_a_missing_doc_map_once() {
    let t = TestRepo::with_files(
        "doc-map-missing",
        &[
            ("README.md", MISSING_README_MD),
            ("docs/kernel/alpha.md", MISSING_DOCS_KERNEL_ALPHA_MD),
        ],
    );
    let repo = open(&t);
    let got = DocMap.run(&repo).expect("doc-map runs");
    let want = vec![finding(
        "doc-map",
        "docs/project/doc-map.md",
        "missing",
        "docs/project/doc-map.md does not exist",
        0,
    )];
    assert_eq!(got, want);
    assert_eq!(DOC_MAP_REL, "docs/project/doc-map.md");
    assert_eq!(DOC_MAP_ALLOWLIST, ["docs/project/doc-map.md"]);
}

#[test]
fn repo_paths_reports_missing_paths_in_current_state_regions() {
    let t = TestRepo::with_files(
        "repo-paths",
        &[
            ("CLAUDE.md", PATHS_CLAUDE_MD),
            (".claude/agents/reader.md", PATHS__CLAUDE_AGENTS_READER_MD),
            ("docs/other.md", PATHS_DOCS_OTHER_MD),
            (
                "docs/project/developer-guide.md",
                PATHS_DOCS_PROJECT_DEVELOPER_GUIDE_MD,
            ),
            ("docs/phases/01-memory.md", PATHS_DOCS_PHASES_01_MEMORY_MD),
            ("kernel/src/main.rs", PATHS_KERNEL_SRC_MAIN_RS),
            ("shared/src/lib.rs", PATHS_SHARED_SRC_LIB_RS),
            ("scripts/tool.sh", PATHS_SCRIPTS_TOOL_SH),
            ("uefi-stub/src/main.rs", PATHS_UEFI_STUB_SRC_MAIN_RS),
        ],
    );
    // Milestone 3 is merged, so only its section of the phase doc is current state.
    t.commit("Phase 1 M3: Step 1 — allocator");
    let repo = open(&t);
    let got = RepoPaths.run(&repo).expect("repo-paths runs");
    let want = vec![
        finding(
            "repo-paths",
            ".claude/agents/reader.md",
            "kernel/src/agent-missing.rs",
            "path does not exist: kernel/src/agent-missing.rs",
            3,
        ),
        finding(
            "repo-paths",
            "CLAUDE.md",
            "kernel/src/gone.rs",
            "path does not exist: kernel/src/gone.rs",
            3,
        ),
        finding(
            "repo-paths",
            "CLAUDE.md",
            "kernel/src/missing/",
            "path does not exist: kernel/src/missing/",
            4,
        ),
        finding(
            "repo-paths",
            "CLAUDE.md",
            "kernel/src/gone2.rs",
            "path does not exist: kernel/src/gone2.rs",
            6,
        ),
        finding(
            "repo-paths",
            "CLAUDE.md",
            "kernel/src/in-comment.rs",
            "path does not exist: kernel/src/in-comment.rs",
            7,
        ),
        finding(
            "repo-paths",
            "CLAUDE.md",
            "kernel/src/double.rs",
            "path does not exist: kernel/src/double.rs",
            7,
        ),
        finding(
            "repo-paths",
            "docs/project/developer-guide.md",
            "scripts/nope.py",
            "path does not exist: scripts/nope.py",
            3,
        ),
        finding(
            "repo-paths",
            "docs/phases/01-memory.md",
            "kernel/src/merged-missing.rs",
            "path does not exist: kernel/src/merged-missing.rs",
            5,
        ),
    ];
    assert_eq!(got, want);
}

#[test]
fn just_recipes_reports_unknown_recipes_then_undocumented_public_ones() {
    let t = TestRepo::with_files(
        "just-recipes",
        &[
            ("justfile", JUST_JUSTFILE),
            ("README.md", JUST_README_MD),
            (
                "docs/project/developer-guide.md",
                JUST_DOCS_PROJECT_DEVELOPER_GUIDE_MD,
            ),
            (".claude/skills/s/SKILL.md", JUST__CLAUDE_SKILLS_S_SKILL_MD),
            ("docs/other.md", JUST_DOCS_OTHER_MD),
        ],
    );
    let repo = open(&t);
    let got = JustRecipes.run(&repo).expect("just-recipes runs");
    let want = vec![
        finding(
            "just-recipes",
            ".claude/skills/s/SKILL.md",
            "ghost",
            "`just ghost` is not a justfile recipe",
            3,
        ),
        finding(
            "just-recipes",
            "README.md",
            "nope",
            "`just nope` is not a justfile recipe",
            16,
        ),
        finding(
            "just-recipes",
            "docs/project/developer-guide.md",
            "gone",
            "`just gone` is not a justfile recipe",
            11,
        ),
        finding(
            "just-recipes",
            "justfile",
            "undocumented:check",
            "public recipe `check` is missing from the README Build \
             Commands and developer-guide §5.1 tables",
            0,
        ),
        finding(
            "just-recipes",
            "justfile",
            "undocumented:undoc",
            "public recipe `undoc` is missing from the README Build \
             Commands and developer-guide §5.1 tables",
            0,
        ),
    ];
    assert_eq!(got, want);
    let documented: BTreeSet<String> = ["build", "docs-check", "quiet"]
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(documented_recipes(&repo), documented);
}

#[test]
fn just_recipes_skips_without_a_justfile() {
    let t = TestRepo::with_files("just-recipes-skip", &[("README.md", NO_JUST_README_MD)]);
    let repo = open(&t);
    let err = JustRecipes
        .run(&repo)
        .expect_err("just-recipes needs a justfile");
    assert_eq!(
        err.downcast_ref::<Skip>(),
        Some(&Skip("no justfile".to_string()))
    );
}

#[test]
fn registry_continues_with_the_path_checks() {
    let names: Vec<&str> = registry().iter().map(|c| c.name()).collect();
    assert_eq!(names[4..7], ["doc-map", "repo-paths", "just-recipes"]);
}
