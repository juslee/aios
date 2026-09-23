//! pointer-doctor and knowledge-hygiene on small committed repositories. The
//! expected findings were recorded from check.py's `check_pointer_doctor` and
//! `check_knowledge_hygiene` on the same files (production order, before
//! merging by key: "claude-md:Build Matrix" appears on two lines).

mod common;

use aios_tools::cmd::docs_check::checks::knowledge::KnowledgeHygiene;
use aios_tools::cmd::docs_check::checks::pointer_doctor::PointerDoctor;
use aios_tools::cmd::docs_check::checks::Check;
use aios_tools::cmd::docs_check::model::Finding;
use aios_tools::cmd::docs_check::repo::Repo;
use common::TestRepo;

const POINTER_FILES: &[(&str, &str)] = &[
    (
        "CLAUDE.md",
        r#"# Project

## Project Identity

Name: Fixture

## Architecture Document Map

Topic index lives in `docs/project/doc-map.md`.

## Key Technical Facts

- Fact one.
- Fact two.
- Fact three.

## Workspace Layout

Tree here.
"#,
    ),
    (
        ".claude/rules/01-code-conventions.md",
        r#"# Code Conventions

## Rust

- Use snake_case.
"#,
    ),
    (
        ".claude/rules/02-quality-gates.md",
        r#"# Quality Gates

Run the gates in CLAUDE.md order.
"#,
    ),
    (
        ".claude/agents/worker.md",
        r#"---
name: worker
description: Fixture worker.
tools: Read, Grep
---

# Worker

Does the work.
"#,
    ),
    (
        ".claude/agents/dev.md",
        r#"---
name: dev
description: Fixture agent.
tools: Read, Edit, MultiEdit, mcp__docs__search, Bash
---

# Dev

Follow the Code Conventions in CLAUDE.md and the Quality Gates from CLAUDE.md.
See the Architecture Document Map in CLAUDE.md for docs.
Read the Key Facts in CLAUDE.md, then the Glossary in CLAUDE.md.
Code Conventions in CLAUDE.md and Code Conventions from CLAUDE.md say the same.
Check `CLAUDE.md`: Workspace Layout, Key Technical Facts and Build Matrix.

```text
Old Section in CLAUDE.md is inside a fence.
```

## Update CLAUDE.md

1. Update: Workspace Layout, Build Matrix
- **Also**: Architecture Doc Map (the index)

## Other

1. Update: Nothing Here

Rules live in rules/03-git.md and `.claude/rules/01-code-conventions.md`.
Docs: `docs/missing/guide.md:12`, `docs/phases/NN-name.md`, `docs/project/doc-map.md`.
Skills: `/alpha`, `/help`, `/ghost`, `/other:thing`, `/kit:nope`, `/alpha --flag`.
Ask the `worker` agent, the `ghost` subagent, subagent_type: `Explore` or subagent_type: nobody.
"#,
    ),
    (
        ".claude/skills/alpha/SKILL.md",
        r#"---
name: alpha
description: Alpha skill.
---

# Alpha

Does alpha things.
"#,
    ),
    (
        ".claude/skills/pack/.claude-plugin/plugin.json",
        r#"{"name": "kit"}
"#,
    ),
    (
        ".claude/skills/pack/skills/go/SKILL.md",
        r#"---
name: go
description: Plugin skill.
---

# Go

Run `/kit:go` or `/kit:stop`.
"#,
    ),
    (
        "docs/project/doc-map.md",
        r#"# Doc Map
"#,
    ),
];

const KNOWLEDGE_FILES: &[(&str, &str)] = &[
    (
        "docs/knowledge/README.md",
        r#"# Knowledge
"#,
    ),
    (
        "docs/knowledge/plans/_template.md",
        r#"---
author: claude
---
"#,
    ),
    (
        "docs/knowledge/plans/2026-01-01-ab-plan.md",
        r#"# Plan without frontmatter
"#,
    ),
    (
        "docs/knowledge/lessons/2026-01-01-ab-good.md",
        r#"---
author: ab
date: 2026-01-01
tags: [kernel]
status: "final"
---

# Lesson
"#,
    ),
    (
        "docs/knowledge/lessons/Bad_Name.md",
        r#"---
author: ab
date: 2026-01-01
tags: [kernel]
status: final
---
"#,
    ),
    (
        "docs/knowledge/lessons/2026-01-02-ab-nofm.md",
        r#"# No frontmatter
"#,
    ),
    (
        "docs/knowledge/decisions/2026-01-03-ab-partial.md",
        r#"---
author: ab
status: wip
---
"#,
    ),
    (
        "docs/knowledge/discussions/2026-01-04-ab-talk.md",
        r#"---
author: ab
date: 2026-01-04
tags: [ipc]
status: active
---
"#,
    ),
    (
        "docs/knowledge/discussions/2026-01-05-ab-old.md",
        r#"---
author: ab
date: 2026-01-05
tags: [ipc]
status: 'archived'
---
"#,
    ),
    (
        "docs/knowledge/research/2026-01-06-abcd-long.md",
        r#"---
author: ab
date: 2026-01-06
tags: []
status:
---
"#,
    ),
    (
        "docs/other/notes.md",
        r#"# Not knowledge
"#,
    ),
];

fn open(repo: &TestRepo) -> Repo {
    Repo::open(repo.path_str()).expect("open the test repository")
}

#[test]
fn check_names() {
    assert_eq!(PointerDoctor.name(), "pointer-doctor");
    assert_eq!(KnowledgeHygiene.name(), "knowledge-hygiene");
}

#[test]
fn pointer_doctor_matches_check_py() {
    let repo = TestRepo::with_files("pointer-doctor", POINTER_FILES);
    let found = PointerDoctor
        .run(&open(&repo))
        .expect("pointer-doctor runs");
    let expected = vec![
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "tool:MultiEdit", "tools: lists unknown tool MultiEdit", 4),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "claude-md:Code Conventions", "points to CLAUDE.md 'Code Conventions', which now lives in .claude/rules/01-code-conventions.md", 9),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "claude-md:Quality Gates", "points to CLAUDE.md 'Quality Gates', which now lives in .claude/rules/02-quality-gates.md", 9),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "claude-md:Architecture Document Map", "points to CLAUDE.md 'Architecture Document Map', which is only a pointer stub now", 10),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "claude-md:Key Facts", "points to CLAUDE.md 'Key Facts', which is not a section of CLAUDE.md", 11),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "claude-md:Code Conventions", "points to CLAUDE.md 'Code Conventions', which now lives in .claude/rules/01-code-conventions.md", 12),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "claude-md:Build Matrix", "points to CLAUDE.md 'Build Matrix', which is not a section of CLAUDE.md", 13),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "claude-md:Build Matrix", "points to CLAUDE.md 'Build Matrix', which is not a section of CLAUDE.md", 21),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "claude-md:Architecture Document Map", "points to CLAUDE.md 'Architecture Document Map', which is only a pointer stub now", 22),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "rules:03-git.md", "rule file 03-git.md does not exist", 28),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "path:docs/missing/guide.md", "path does not exist: docs/missing/guide.md", 29),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "skill:/ghost", "/ghost is not a project skill or built-in command", 30),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "skill:/kit:nope", "/kit:nope is not a project skill or built-in command", 30),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "agent:ghost", "agent ghost is not defined in .claude/agents", 31),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "agent:nobody", "agent nobody is not defined in .claude/agents", 31),
        Finding::new("pointer-doctor", ".claude/skills/pack/skills/go/SKILL.md", "skill:/kit:stop", "/kit:stop is not a project skill or built-in command", 8),
    ];
    assert_eq!(found, expected);
}

#[test]
fn knowledge_hygiene_matches_check_py() {
    let repo = TestRepo::with_files("knowledge-hygiene", KNOWLEDGE_FILES);
    let found = KnowledgeHygiene
        .run(&open(&repo))
        .expect("knowledge-hygiene runs");
    let expected = vec![
        Finding::new(
            "knowledge-hygiene",
            "docs/knowledge/decisions/2026-01-03-ab-partial.md",
            "missing:date",
            "frontmatter lacks 'date'",
            0,
        ),
        Finding::new(
            "knowledge-hygiene",
            "docs/knowledge/decisions/2026-01-03-ab-partial.md",
            "missing:tags",
            "frontmatter lacks 'tags'",
            0,
        ),
        Finding::new(
            "knowledge-hygiene",
            "docs/knowledge/decisions/2026-01-03-ab-partial.md",
            "status:wip",
            "status 'wip' is not one of draft, final, in-progress",
            0,
        ),
        Finding::new(
            "knowledge-hygiene",
            "docs/knowledge/discussions/2026-01-05-ab-old.md",
            "status:archived",
            "status 'archived' is not one of active, draft, final, graduated, in-progress",
            0,
        ),
        Finding::new(
            "knowledge-hygiene",
            "docs/knowledge/lessons/2026-01-02-ab-nofm.md",
            "frontmatter",
            "no YAML frontmatter",
            0,
        ),
        Finding::new(
            "knowledge-hygiene",
            "docs/knowledge/lessons/Bad_Name.md",
            "name",
            "file name is not YYYY-MM-DD-initials-short-description.md",
            0,
        ),
        Finding::new(
            "knowledge-hygiene",
            "docs/knowledge/plans/2026-01-01-ab-plan.md",
            "plans-not-empty",
            "working plan present; distill it and remove it before the PR is ready",
            0,
        ),
        Finding::new(
            "knowledge-hygiene",
            "docs/knowledge/research/2026-01-06-abcd-long.md",
            "name",
            "file name is not YYYY-MM-DD-initials-short-description.md",
            0,
        ),
    ];
    assert_eq!(found, expected);
}
