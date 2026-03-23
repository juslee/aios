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
4. Search the knowledge hive for relevant decisions that may affect phase planning:
    - Use Obsidian MCP search_notes with subsystem keywords
    - Review `docs/knowledge/decisions/` for prior architectural choices
    - Factor findings into milestone structure
5. Read the previous phase doc (use Glob for `docs/phases/*`) — note:
    - Its last milestone number (this phase continues sequentially from there)
    - Its "Unlocks" field (this phase should appear there)
    - Its structure and style (match it)

## Step 2: Create worktree

```bash
git checkout main && git pull origin main
git worktree add .claude/worktrees/phase-$ARGUMENTS-docs -b claude/phase-$ARGUMENTS-docs main
cd .claude/worktrees/phase-$ARGUMENTS-docs
```

All subsequent work (edits, commits, pushes) happens inside the worktree.

## Step 3: Generate the phase doc

Use the Write tool to create `docs/phases/NN-name.md` where NN is the zero-padded phase number and name matches `development-plan.md`.

Follow this structure exactly:

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

## Architecture References
| Topic | Document | Relevant Sections |
|---|---|---|
| <topic> | <doc path> | §N.N description |

## Milestones
| Milestone | Steps | Target | Observable result |
|---|---|---|---|
| M<number>: <name> | N steps | <what's built> | <verification command + expected output> |
| M<number>: <name> | N steps | <what's built> | <verification command + expected output> |
| M<number>: <name> | N steps | <what's built> | <verification command + expected output> |
(3+ milestones required — add more rows as needed based on phase complexity)

## Milestone <number> — <name>
### Step 1: <step name>
**What:** <description>
**Tasks:**
- [ ] <concrete task>
- [ ] <concrete task>
**Key reference:** <architecture doc> §N.N
**Acceptance:** <command> → <expected output>

### Step 2: ...
(each milestone MUST have 1+ implementation steps — repeat for each step)

### Step N: Shared crate refactoring
**What:** Move pure data structures from kernel/ to shared/, add host-side tests
**Tasks:**
- [ ] Review code written in kernel/ during this milestone
- [ ] Move types with no hardware deps to shared/src/
- [ ] Write host-side unit tests for moved code
**Acceptance:** `just check` + `just test` pass

## Milestone <number> — <name>
(repeat pattern)

## Decision Points
| Decision | Options | Recommendation | Rationale |
|---|---|---|---|
| <decision> | <options> | <recommendation> | <why> |

## Phase Completion Criteria
- [ ] `just check` — zero warnings
- [ ] `just test` — all pass, >N tests
- [ ] `just run` — <expected QEMU output>
- [ ] All milestones checked off above
```

**Key rules:**
- Every milestone SHOULD end with a shared crate refactoring step (include when the milestone produces types or logic that can move to `shared/`; skip if no sharable code was written)
- Acceptance criteria must be mechanical (run command → see output)
- Never duplicate architecture content — reference it by section number
- Milestone numbers continue sequentially from the previous phase's last milestone
- Each phase has 3+ milestones (variable count based on complexity), each with 1+ implementation steps

## Step 4: Verify the doc

Read the generated doc back and verify:
- Milestone numbers are correct (continue from previous phase)
- All architecture references point to real docs and sections
- Every step has an acceptance criteria block
- The shared crate refactoring step is present in every milestone
- Duration matches `development-plan.md`

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
- <N> milestones, <N> steps total
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
