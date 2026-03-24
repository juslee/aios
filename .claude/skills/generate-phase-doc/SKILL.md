---
name: generate-phase-doc
description: >
  Generates a phase implementation doc from architecture docs.
  Use when asked to write or generate a phase doc for Phase N.
argument-hint: "[phase-number]"
---

# Generate Phase Doc for Phase $ARGUMENTS

Follow the Phase Doc Generation Workflow from CLAUDE.md:

## Step 1: Research

1. Read `docs/project/development-plan.md` §8 — find phase $ARGUMENTS name, duration, and deliverable
2. Identify relevant architecture docs using the Architecture Document Map in CLAUDE.md
3. Read those architecture docs fully — these are the source of truth for what this phase implements
4. Search the knowledge hive for relevant decisions and discussions that may affect phase planning:
    - Use Obsidian MCP search_notes with subsystem keywords
    - Review `docs/knowledge/decisions/` for prior architectural choices
    - Review `docs/knowledge/discussions/` for in-progress design explorations that may have graduated to architecture docs
    - Factor findings into milestone structure
5. Read the previous phase doc (use Glob for `docs/phases/*`) — note:
    - Its last milestone number (this phase continues sequentially from there)
    - Its "Unlocks" field (this phase should appear there)
    - Its structure and style (match it exactly)
6. **If plan mode is active**: Restrict to research + planning only — no builds, commits, or worktree creation. Write the plan to the system plan file, stop, and wait for user approval.

## Step 2: Create worktree

```bash
git checkout main && git pull origin main
git worktree add .claude/worktrees/phase-$ARGUMENTS-docs -b claude/phase-$ARGUMENTS-docs main
cd .claude/worktrees/phase-$ARGUMENTS-docs
```

All subsequent work (edits, commits, pushes) happens inside the worktree.

## Step 3: Generate the phase doc

Use the Write tool to create `docs/phases/NN-name.md` where NN is the zero-padded phase number and name matches `development-plan.md`.

Follow this structure exactly (matches established conventions from Phases 04/05):

```markdown
# Phase N: <Name>

**Tier:** <from development-plan.md>
**Duration:** <from development-plan.md>
**Deliverable:** <from development-plan.md>
**Status:** Planned
**Prerequisites:** <previous phase(s)>
**Unlocks:** <next phase(s)>

-----

## Objective

<2-3 paragraphs explaining what this phase builds and why>

-----

## Architecture References

These existing documents define the technical design. This phase doc focuses on implementation order and acceptance criteria — not duplicating the architecture.

| Topic | Document | Relevant Sections |
|---|---|---|
| <topic> | [doc-name](relative-path) | §N.N description |

-----

## Milestones

Milestones are numbered continuously across all phases. Phase <N-1> used M<X>–M<Y>; Phase <N> continues with M<Y+1>–M<Z>.

| Milestone | Steps | Target | Observable result |
|---|---|---|---|
| **M<number> — <name>** | <start>–<end> | <what's built> | <verification command + expected output> |
| **M<number> — <name>** | <start>–<end> | <what's built> | <verification command + expected output> |
| **M<number> — <name>** | <start>–<end> | <what's built> | <verification command + expected output> |

(3+ milestones required. Add more rows for complex phases — there is no upper limit. Scale milestone count to match the phase's complexity and duration.)

-----

## Milestone <number> — <name> (Target timeframe)

NOTE: Use bare number in headings (`## Milestone 13`), not the M-prefix (`## M13`). The M-prefix is only for the summary table (`**M13 — Name**`).

*Goal: <1-2 sentence description of what this milestone achieves and why>*

### Step <N>: <step name>

**What:** <description>

**Tasks:**
- [ ] <concrete task>
- [ ] <concrete task>

**Note:** <optional — include only when there's a non-obvious constraint, gotcha, or design rationale worth calling out>

**Key reference:** [doc-name](relative-path) §N.N

**Acceptance:** `<command>` → `<expected output>`

-----

### Step <N+1>: ...
(repeat for each step — put a `-----` separator between EVERY step, not just between milestones)

-----

### Step <last>: Shared crate refactoring

**What:** Move pure data structures from kernel/ to shared/, add host-side tests

**Tasks:**
- [ ] Review code written in kernel/ during this milestone
- [ ] Move types with no hardware deps to shared/src/
- [ ] Write host-side unit tests for moved code

**Acceptance:** `just check` + `just test` pass

-----

## Milestone <number> — <name> (Target timeframe)

*Goal: ...*

(repeat pattern — steps continue numbering from where previous milestone left off)

-----

## Decision Points

| Decision | Options | Recommendation | Rationale |
|---|---|---|---|
| <decision> | <options> | <recommendation> | <why> |

-----

## Phase Completion Criteria

- [ ] `just check` — zero warnings
- [ ] `just test` — all pass, >N tests
- [ ] `just run` — <expected QEMU output>
- [ ] All milestones checked off above
```

**Key rules:**
- **Step numbering is continuous across milestones within a phase** — each step is a single atomic task. A milestone has as many steps as needed to reach its goal (not a fixed count). E.g., M19 might have Steps 1–6, M20 Steps 7–9, M21 Steps 10–15. Steps reset to 1 at the start of each new Phase.
- **Step ranges in the Milestones table** — show actual ranges like `1–6`, `7–9`, `10–15` (not fixed groups, not `N steps`). The range reflects however many steps the milestone actually needs.
- **Bold milestone names in the summary table** — use `**M16 — Name**` format.
- **`-----` horizontal rules** between metadata/objective, after architecture references, between every step within a milestone, between milestones, and before Decision Points.
- **Italic *Goal:* line** at the start of each milestone section.
- **Milestone numbering context paragraph** above the Milestones table explaining continuation from previous phase.
- **Milestone count scales with complexity** — 3 is the minimum, not the target. A 5-week phase with 3 major subsystems may need 4-5 milestones. A 2-week phase with a focused scope may need exactly 3. Let the architecture drive the count.
- Every milestone SHOULD end with a shared crate refactoring step (include when the milestone produces types or logic that can move to `shared/`; skip if no sharable code was written).
- **Note field is optional** — only include when there's a gotcha, constraint, or non-obvious design rationale.
- Acceptance criteria must be mechanical (run command → see output).
- Never duplicate architecture content — reference it by section number.
- Architecture references must use relative markdown links: `[doc-name](relative-path)`.

## Step 4: Verify the doc

Read the generated doc back and verify:
- Milestone numbers are correct (continue from previous phase)
- Step numbers are continuous across milestones (not resetting per milestone)
- Step ranges in the Milestones table match actual step numbers in each milestone section
- **Architecture references point to real docs** — Read each referenced doc to confirm the section numbers exist
- Every step has an Acceptance block
- The shared crate refactoring step is present in milestones that produce sharable code
- Duration matches `development-plan.md`
- `-----` horizontal rules are present between all major sections AND between every step within milestones
- Each milestone section starts with an italic *Goal:* line
- Milestone headings use bare number (`## Milestone 13`), not M-prefix (`## M13`)

## Step 5: Commit and push

```bash
git add docs/phases/NN-name.md
git commit -m "Phase $ARGUMENTS: <phase name> phase implementation doc"
git push -u origin claude/phase-$ARGUMENTS-docs
```

## Step 6: Audit and PR

1. Run `/audit-loop` — auto-detects docs-only mode, loops until 0 issues
2. Create PR using `gh pr create`:

```bash
gh pr create --title "Phase $ARGUMENTS: <phase name> implementation doc" --body "$(cat <<'EOF'
## Summary
- Generated phase implementation doc for Phase $ARGUMENTS
- <N> milestones (M<start>–M<end>), <N> steps total
- Continues from M<K> (Phase <N-1>)

## Architecture References
- <list key arch docs referenced>

## Quality Gates
- [ ] `/audit-loop` — 0 issues
EOF
)"
```

3. Run `/review-pr-comments`: wait for Copilot/reviewer comments, fix issues, reply, resolve conversations, push fixes
4. Run `/merge-and-cleanup`: squash merge the PR, delete remote/local branch, remove worktree, fast-forward main
