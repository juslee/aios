//! layout and harness-tables on small committed repositories. The expected
//! findings were recorded from check.py's `check_layout` and
//! `check_harness_tables` on the same files (production order).

mod common;

use aios_tools::cmd::docs_check::checks::harness::{project_agents, project_skills, HarnessTables};
use aios_tools::cmd::docs_check::checks::layout::{layout_block, tree_entries, Layout};
use aios_tools::cmd::docs_check::checks::Check;
use aios_tools::cmd::docs_check::model::Finding;
use aios_tools::cmd::docs_check::repo::Repo;
use common::TestRepo;
use std::collections::BTreeSet;

/// `.claude/skills/linked` is a plain file standing in for a tracked symlink
/// (the real repository tracks `.claude/skills/obsidian` as one); both are a
/// single tracked path that names the skill.
const FILES: &[(&str, &str)] = &[
    (
        "CLAUDE.md",
        r#"# Project

## Workspace Layout

```text
proj/
├── .claude/
│   ├── agents/           worker, helper
│   ├── skills/           alpha, linked, kit:go,
│   │                     bad:run, retired (plugin skills as plugin:skill)
│   └── rules/            01-code (auto-loaded)
├── kernel/src/           kernel
│   ├── mm/               memory
│   ├── ipc/              channels
│   └── (top-level)       main.rs, boot_phase,
│                         dtb
├── shared/src/           shared types
│   ├── kits/             kit traits
│   └── (top-level)       lib, boot, cap
├── uefi-stub/src/        stub
└── docs/                 docs
```

## Team

**Agents** (defined in `.claude/agents/`):

| Agent | Role |
| --- | --- |
| `worker` | Works |
| `ghost-agent` | Gone |

**Skills** (defined in `.claude/skills/`):

| Skill | Purpose |
| --- | --- |
| `/alpha` | First |
| `/kit:go` | Plugin skill |
| `/stale-skill` | Gone |

## Other

Nothing else.
"#,
    ),
    (
        ".claude/rules/05-file-placement.md",
        r#"# File Placement Rules

```
kernel/src/mm/                 Memory
kernel/src/old/                Removed
kernel/src/                    Entry
```
"#,
    ),
    (
        "kernel/src/main.rs",
        r#"fn main() {}
"#,
    ),
    (
        "kernel/src/boot_phase.rs",
        r#"//! Boot phases.
"#,
    ),
    (
        "kernel/src/mm/mod.rs",
        r#"//! Memory.
"#,
    ),
    (
        "kernel/src/sched/mod.rs",
        r#"//! Scheduler.
"#,
    ),
    (
        "kernel/src/sched/rq/mod.rs",
        r#"//! Run queues.
"#,
    ),
    (
        "shared/src/lib.rs",
        r#"//! Shared.
"#,
    ),
    (
        "shared/src/boot.rs",
        r#"//! Boot info.
"#,
    ),
    (
        "shared/src/kits/mod.rs",
        r#"//! Kits.
"#,
    ),
    (
        "shared/src/ipc/mod.rs",
        r#"//! IPC types.
"#,
    ),
    (
        ".claude/agents/worker.md",
        r#"# Worker
"#,
    ),
    (
        ".claude/agents/helper.md",
        r#"# Helper
"#,
    ),
    (
        ".claude/skills/alpha/SKILL.md",
        r#"# Alpha
"#,
    ),
    (
        ".claude/skills/alpha/reference.md",
        r#"# Alpha reference
"#,
    ),
    (
        ".claude/skills/linked",
        r#"../../elsewhere/linked
"#,
    ),
    (
        ".claude/skills/pack/.claude-plugin/plugin.json",
        r#"{"name": "kit"}
"#,
    ),
    (
        ".claude/skills/pack/skills/go/SKILL.md",
        r#"# Go
"#,
    ),
    (
        ".claude/skills/bad/.claude-plugin/plugin.json",
        r#"not json
"#,
    ),
    (
        ".claude/skills/bad/skills/run/SKILL.md",
        r#"# Run
"#,
    ),
    (
        ".claude/skills/unnamed/.claude-plugin/plugin.json",
        r#"{"name": ""}
"#,
    ),
    (
        ".claude/skills/unnamed/skills/x/SKILL.md",
        r#"# X
"#,
    ),
];

/// A repository without CLAUDE.md: every table is empty and the layout lists
/// are absent, so only the table comparisons report.
const BARE_FILES: &[(&str, &str)] = &[
    ("kernel/src/main.rs", "fn main() {}\n"),
    (".claude/agents/solo.md", "# Solo\n"),
    (".claude/skills/only/SKILL.md", "# Only\n"),
];

fn open(repo: &TestRepo) -> Repo {
    Repo::open(repo.path_str()).expect("open the test repository")
}

fn names(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn check_names() {
    assert_eq!(Layout.name(), "layout");
    assert_eq!(HarnessTables.name(), "harness-tables");
}

#[test]
fn layout_tree_reads_the_workspace_layout_section() {
    let repo = TestRepo::with_files("layout-tree", FILES);
    let block = layout_block(&open(&repo));
    assert_eq!(
        tree_entries(&block, "kernel/src/", "shared/src/"),
        (names(&["ipc", "mm"]), names(&["boot_phase", "dtb", "main"]))
    );
    assert_eq!(
        tree_entries(&block, "shared/src/", "uefi-stub/"),
        (names(&["kits"]), names(&["boot", "cap", "lib"]))
    );
}

#[test]
fn layout_matches_check_py() {
    let repo = TestRepo::with_files("layout", FILES);
    let found = Layout.run(&open(&repo)).expect("layout runs");
    let expected = vec![
        Finding::new(
            "layout",
            "CLAUDE.md",
            "missing:kernel/src/sched/",
            "Workspace Layout does not list kernel dir kernel/src/sched/",
            0,
        ),
        Finding::new(
            "layout",
            "CLAUDE.md",
            "stale:kernel/src/ipc/",
            "Workspace Layout lists kernel/src/ipc/, which does not exist",
            0,
        ),
        Finding::new(
            "layout",
            "CLAUDE.md",
            "stale:kernel/src/dtb.rs",
            "Workspace Layout lists kernel/src/dtb.rs, which does not exist",
            0,
        ),
        Finding::new(
            "layout",
            "CLAUDE.md",
            "missing:shared/src/ipc/",
            "Workspace Layout does not list shared dir shared/src/ipc/",
            0,
        ),
        Finding::new(
            "layout",
            "CLAUDE.md",
            "stale:shared/src/cap.rs",
            "Workspace Layout lists shared/src/cap.rs, which does not exist",
            0,
        ),
        Finding::new(
            "layout",
            ".claude/rules/05-file-placement.md",
            "missing:kernel/src/sched/",
            "rule 05 does not list kernel/src/sched/",
            0,
        ),
        Finding::new(
            "layout",
            ".claude/rules/05-file-placement.md",
            "stale:kernel/src/old/",
            "rule 05 lists kernel/src/old/, which does not exist",
            0,
        ),
    ];
    assert_eq!(found, expected);
}

#[test]
fn layout_without_claude_md_lists_every_module_as_missing() {
    let repo = TestRepo::with_files("layout-bare", BARE_FILES);
    let found = Layout.run(&open(&repo)).expect("layout runs");
    let expected = vec![Finding::new(
        "layout",
        "CLAUDE.md",
        "missing:kernel/src/main.rs",
        "Workspace Layout does not list kernel module kernel/src/main.rs",
        0,
    )];
    assert_eq!(found, expected);
}

#[test]
fn project_skills_and_agents_follow_the_skills_dir_rules() {
    let test_repo = TestRepo::with_files("harness-skills", FILES);
    let repo = open(&test_repo);
    assert_eq!(
        project_skills(&repo),
        (
            names(&["alpha", "bad:run", "kit:go", "linked", "unnamed:x"]),
            names(&["bad", "kit", "unnamed"])
        )
    );
    assert_eq!(project_agents(&repo), names(&["helper", "worker"]));
}

#[test]
fn harness_tables_matches_check_py() {
    let repo = TestRepo::with_files("harness", FILES);
    let found = HarnessTables
        .run(&open(&repo))
        .expect("harness-tables runs");
    let expected = vec![
        Finding::new(
            "harness-tables",
            "CLAUDE.md",
            "skills-table-missing:bad:run",
            "CLAUDE.md skills-table omits skill bad:run",
            0,
        ),
        Finding::new(
            "harness-tables",
            "CLAUDE.md",
            "skills-table-missing:linked",
            "CLAUDE.md skills-table omits skill linked",
            0,
        ),
        Finding::new(
            "harness-tables",
            "CLAUDE.md",
            "skills-table-missing:unnamed:x",
            "CLAUDE.md skills-table omits skill unnamed:x",
            0,
        ),
        Finding::new(
            "harness-tables",
            "CLAUDE.md",
            "skills-table-stale:stale-skill",
            "CLAUDE.md skills-table lists skill stale-skill, which is not in .claude/",
            0,
        ),
        Finding::new(
            "harness-tables",
            "CLAUDE.md",
            "agents-table-missing:helper",
            "CLAUDE.md agents-table omits agent helper",
            0,
        ),
        Finding::new(
            "harness-tables",
            "CLAUDE.md",
            "agents-table-stale:ghost-agent",
            "CLAUDE.md agents-table lists agent ghost-agent, which is not in .claude/",
            0,
        ),
        Finding::new(
            "harness-tables",
            "CLAUDE.md",
            "layout-skills-missing:unnamed:x",
            "CLAUDE.md layout-skills omits skill unnamed:x",
            0,
        ),
        Finding::new(
            "harness-tables",
            "CLAUDE.md",
            "layout-skills-stale:retired",
            "CLAUDE.md layout-skills lists skill retired, which is not in .claude/",
            0,
        ),
    ];
    assert_eq!(found, expected);
}

#[test]
fn harness_tables_without_claude_md_skips_the_layout_lists() {
    let repo = TestRepo::with_files("harness-bare", BARE_FILES);
    let found = HarnessTables
        .run(&open(&repo))
        .expect("harness-tables runs");
    let expected = vec![
        Finding::new(
            "harness-tables",
            "CLAUDE.md",
            "skills-table-missing:only",
            "CLAUDE.md skills-table omits skill only",
            0,
        ),
        Finding::new(
            "harness-tables",
            "CLAUDE.md",
            "agents-table-missing:solo",
            "CLAUDE.md agents-table omits agent solo",
            0,
        ),
    ];
    assert_eq!(found, expected);
}
