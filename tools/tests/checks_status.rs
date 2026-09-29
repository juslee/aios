//! milestone-status and phase-count on small committed repositories. The
//! expected findings were recorded from check.py's `check_milestone_status` and
//! `check_phase_count` on the same files and history (production order, before
//! merging by key).

mod common;

use aios_tools::cmd::docs_check::checks::milestones::{MilestoneStatus, PhaseCount};
use aios_tools::cmd::docs_check::checks::Check;
use aios_tools::cmd::docs_check::model::{Finding, Skip};
use aios_tools::cmd::docs_check::repo::Repo;
use common::TestRepo;

const STATUS_FILES: &[(&str, &str)] = &[
    (
        "docs/phases/00-foundation.md",
        r#"# Phase 0: Foundation

**Status:** In Progress

## Milestones

| Milestone | Steps |
|---|---|
| **M1 — Boot** | 1 |

## Milestone 1 — Boot

- [x] Entry stub
- [ ] Banner
  * [ ] Nested banner detail
- [ ] ~~Legacy console~~ — deferred to M4

## Milestone 2 — UART

- [x] Driver
"#,
    ),
    (
        "docs/phases/01-memory.md",
        r#"# Phase 1: Memory

**Status:**   Planned

## Milestone 3 — Allocator

- [ ] Buddy allocator

## Milestone 4 — Paging

- [ ] Page tables
"#,
    ),
    (
        "docs/phases/02-next.md",
        r#"# Phase 2: Next

**Status:** Complete

## Milestone 5 — Later

- [ ] Future work
"#,
    ),
    (
        "docs/phases/03-extra.md",
        r#"# Phase 3: Extra

## Milestone 6 — Extra

- [x] Done
"#,
    ),
    (
        "docs/phases/06-empty.md",
        r#"# Phase 6: Empty

**Status:** Planned
"#,
    ),
    (
        "README.md",
        r#"# Fixture

Status: M1–M3 merged.
"#,
    ),
    (
        "docs/project/development-plan.md",
        r#"# Development Plan

## 8. Phase Detail Reference

| Phase | Name | Tier | Weeks | Deliverable | Status |
|---|---|---|---|---|---|
| 0 | Foundation | 1 | 2 | Boots | Complete |
| 1 | Memory | 1 | 2 | Allocates | Planned |
| 04 | Later | 1 | 2 | Something | Complete |
| 5 | Orphan | 1 | 2 | Nothing | In Progress |
| 6 | Empty | 1 | 2 | Nothing | Planned |
| 7 | Short | 1 |
| x | Bad | 1 | 2 | Nothing | Complete |

## 8.1 Actual Progress

### Velocity Summary

| Phase | Planned | Actual | Speedup | Milestones |
|---|---|---|---|---|
| 0 (Foundation) | 2 weeks | 1 day | 10x | M1–M2 |
| 1 (Memory) | 2 weeks | 1 day | 10x | M3 |
"#,
    ),
];

const PHASE_FILES: &[(&str, &str)] = &[
    (
        "docs/project/development-plan.md",
        r#"# Development Plan

3 phases across 1 tier.

## 8. Phase Detail Reference

| Phase | Name | Status |
|---|---|---|
| 0 | Foundation | Complete |
| 1 | Memory | Complete |
| 02 | Next | Planned |
| — | Later | Planned |

The plan lists 03 phases across two tiers and 04 phases across one.

## 9. Other

| 3 | Not counted | x |
"#,
    ),
    (
        "README.md",
        r#"# Readme

We plan 5 phases across 2 tiers and v2 phases across none.

```text
7 phases across 1 tier in a fence.
```
"#,
    ),
    (
        ".claude/rules/07-milestone-numbering.md",
        r#"# Milestone Numbering

47 phases with variable milestones; 3 phases are done.
"#,
    ),
    (
        "CLAUDE.md",
        r#"# Project

Plan: 3 phases, then 12 phases later.
"#,
    ),
];

/// Subjects committed after "Initial", oldest first. "Docs: ..." is not a
/// milestone subject; merged milestones: M1, M2 (phase 0), M3 (1), M6 (3),
/// M9 (5, no phase doc) and M10 (6, a phase doc without milestone sections).
const STATUS_COMMITS: [&str; 7] = [
    "Phase 0 M1: Step 1 — boot stub",
    "Docs: tidy the phase docs",
    "Phase 0 M2: Step 2 — uart console",
    "Phase 1 M3: Step 1 — buddy allocator",
    "Phase 3 M6: Step 1 — extra",
    "Phase 5 M9: Step 1 — orphan",
    "Phase 6 M10: Step 1 — empty",
];

fn status_repo(label: &str) -> TestRepo {
    let repo = TestRepo::with_files(label, STATUS_FILES);
    for subject in STATUS_COMMITS {
        repo.commit(subject);
    }
    repo
}

fn open(repo: &TestRepo) -> Repo {
    Repo::open(repo.path_str()).expect("open the test repository")
}

fn skip_message(result: anyhow::Result<Vec<Finding>>) -> String {
    let err = result.expect_err("the check should be skipped");
    match err.downcast_ref::<Skip>() {
        Some(skip) => skip.0.clone(),
        None => panic!("expected a Skip, got: {err:#}"),
    }
}

#[test]
fn check_names() {
    assert_eq!(MilestoneStatus.name(), "milestone-status");
    assert_eq!(PhaseCount.name(), "phase-count");
}

#[test]
fn milestone_status_matches_check_py() {
    let repo = status_repo("ms-status");
    let found = MilestoneStatus
        .run(&open(&repo))
        .expect("milestone-status runs");
    let expected = vec![
        Finding::new(
            "milestone-status",
            "docs/phases/00-foundation.md",
            "status",
            "all milestones (M1, M2) are merged but status is 'In Progress'",
            3,
        ),
        Finding::new(
            "milestone-status",
            "docs/phases/00-foundation.md",
            "M1:unchecked",
            "merged milestone M1 still has 2 unchecked task(s)",
            14,
        ),
        Finding::new(
            "milestone-status",
            "docs/phases/01-memory.md",
            "status",
            "M3 merged but status is 'Planned'",
            3,
        ),
        Finding::new(
            "milestone-status",
            "docs/phases/01-memory.md",
            "M3:unchecked",
            "merged milestone M3 still has 1 unchecked task(s)",
            7,
        ),
        Finding::new(
            "milestone-status",
            "docs/phases/02-next.md",
            "status",
            "status is 'Complete' but no milestone is merged",
            3,
        ),
        Finding::new(
            "milestone-status",
            "docs/phases/03-extra.md",
            "status",
            "all milestones (M6) are merged but status is ''",
            0,
        ),
        Finding::new(
            "milestone-status",
            "README.md",
            "latest:M10",
            "README status does not mention the latest merged milestone M10",
            0,
        ),
        Finding::new(
            "milestone-status",
            "docs/project/development-plan.md",
            "§8:phase-1",
            "M3 merged but status is 'Planned'",
            8,
        ),
        Finding::new(
            "milestone-status",
            "docs/project/development-plan.md",
            "§8:phase-4",
            "status is 'Complete' but no milestone is merged",
            9,
        ),
        Finding::new(
            "milestone-status",
            "docs/project/development-plan.md",
            "§8:phase-5",
            "all milestones (M9) are merged but status is 'In Progress'",
            10,
        ),
        Finding::new(
            "milestone-status",
            "docs/project/development-plan.md",
            "§8:phase-6",
            "all milestones (M10) are merged but status is 'Planned'",
            11,
        ),
        Finding::new(
            "milestone-status",
            "docs/project/development-plan.md",
            "§8.1:M6",
            "§8.1 Velocity Summary has no row covering merged milestone M6",
            0,
        ),
        Finding::new(
            "milestone-status",
            "docs/project/development-plan.md",
            "§8.1:M9",
            "§8.1 Velocity Summary has no row covering merged milestone M9",
            0,
        ),
        Finding::new(
            "milestone-status",
            "docs/project/development-plan.md",
            "§8.1:M10",
            "§8.1 Velocity Summary has no row covering merged milestone M10",
            0,
        ),
    ];
    assert_eq!(found, expected);
}

#[test]
fn milestone_status_skips_without_phase_commits() {
    let repo = TestRepo::with_files("ms-nophase", STATUS_FILES);
    repo.commit("Docs: no phase subjects");
    assert_eq!(
        skip_message(MilestoneStatus.run(&open(&repo))),
        "no 'Phase N MK:' commits found on main's first-parent history"
    );
}

#[test]
fn milestone_status_skips_a_shallow_clone() {
    let source = status_repo("ms-shallow-src");
    let dir = common::unique_dir("ms-shallow");
    let dest = dir.to_str().expect("UTF-8 temp path").to_string();
    let url = format!("file://{}", source.path_str());
    common::git(&dir, &["clone", "-q", "--depth", "1", &url, &dest]);
    let clone = TestRepo::adopt(dir);
    assert_eq!(
        skip_message(MilestoneStatus.run(&open(&clone))),
        "shallow clone: git history unavailable (use fetch-depth: 0)"
    );
}

#[test]
fn phase_count_matches_check_py() {
    let repo = TestRepo::with_files("phase-count", PHASE_FILES);
    let found = PhaseCount.run(&open(&repo)).expect("phase-count runs");
    let expected = vec![
        Finding::new(
            "phase-count",
            "docs/project/development-plan.md",
            "claimed:04",
            "says 04 phases; development-plan §8 lists 3",
            14,
        ),
        Finding::new(
            "phase-count",
            "README.md",
            "claimed:5",
            "says 5 phases; development-plan §8 lists 3",
            3,
        ),
        Finding::new(
            "phase-count",
            ".claude/rules/07-milestone-numbering.md",
            "claimed:47",
            "says 47 phases; development-plan §8 lists 3",
            3,
        ),
        Finding::new(
            "phase-count",
            "CLAUDE.md",
            "claimed:12",
            "says 12 phases; development-plan §8 lists 3",
            3,
        ),
    ];
    assert_eq!(found, expected);
}

#[test]
fn phase_count_skips_without_plan() {
    let repo = TestRepo::with_files(
        "phase-noplan",
        &[("README.md", "5 phases across 2 tiers.\n")],
    );
    assert_eq!(
        skip_message(PhaseCount.run(&open(&repo))),
        "docs/project/development-plan.md not found"
    );
}
