---
name: implement-phase
description: >
  Implements an AIOS phase step by step from the phase doc.
  Use when asked to implement Phase N or work on a specific phase.
argument-hint: "[phase-number]"
---

# Implement AIOS Phase $ARGUMENTS

Follow the Phase Implementation Workflow from `.claude/rules/04-phase-workflow.md`.

## Step 0: Plan Mode Routing

Check whether **plan mode is active** (the system will have injected a reminder saying "Plan mode is active" and assigned a plan file path).

If plan mode is not active, call `EnterPlanMode` (load it with ToolSearch if needed) and wait for the user to accept. Then run the Planning Path. After the user approves the plan, run the Execution Path.

---

## Planning Path (plan mode only)

When plan mode is active, you can only read files and write to the system-assigned plan file. No edits, builds, commits, or worktree creation.

### P1. Research

1. Read `docs/phases/` and find the doc matching phase $ARGUMENTS (glob for `$ARGUMENTS-*.md` or `0$ARGUMENTS-*.md`)
2. Read all Architecture References listed in the phase doc
3. Read `.claude/rules/` (01-code-conventions, 02-quality-gates, 06-unsafe-documentation)
4. Search the knowledge hive for relevant lessons and decisions:
    - Grep `docs/knowledge/` (lessons, decisions) for keywords from the phase doc
    - Review any matching `docs/knowledge/lessons/` and `docs/knowledge/decisions/`
    - Factor known pitfalls into implementation approach
5. Read `docs/knowledge/plans/_template.md` — use its structure as the skeleton for the plan

### P2. Write Plan to System Plan File

Write the implementation plan to the **system-assigned plan file** (the path from the plan mode system message). Use the same structure as `docs/knowledge/plans/_template.md`:

- **Frontmatter**: set `author: claude`, `date: YYYY-MM-DD`, `tags: [relevant subsystem tags]`, `status: in-progress`, `phase: $ARGUMENTS`, `milestone: MK`
- **Approach**: why this phase matters, current codebase state, key gaps, shared crate plan
- **Progress**: for each step in the phase doc, write a checkbox item with granular sub-tasks (files to create/modify, types/traits/functions, acceptance commands)
- **Code Structure Decisions**: naming, data structures, algorithms, deviations from arch docs (with rationale)
- **Dependencies & Risks**: what must exist before this work starts, what could go wrong
- **Phase Doc Reconciliation**: note any changes needed to the phase doc (new steps, reordered steps, updated acceptance criteria, corrected references) — these will be applied during execution

The plan's first heading is `# Plan: Phase $ARGUMENTS MK — <description>` (the template's form); the plan gate reviews only plans whose first heading starts with `# Plan:`.

### P3. Exit Plan Mode

Call `ExitPlanMode`. The Fable plan gate reviews the plan first; when it blocks, fix each must-fix finding in the plan file and call `ExitPlanMode` again. After the user approves, continue with the Execution Path.

---

## Execution Path (normal mode)

### Phase 1: Session Prep

1. Run the session start checklist (from `.claude/rules/04-phase-workflow.md`):

```bash
just check
```

Toolchain, `Cargo.lock` and QEMU change only through Renovate PRs (rule 04, rule 11 Toolchain).

### Phase 2: Worktree Setup

2. Create an isolated worktree for ALL subsequent work:

```bash
# Ensure we're on main and up to date
git checkout main && git pull origin main

# Create worktree with a new branch
# Branch name: claude/phase-$ARGUMENTS-MK-<short-description> (convention in .claude/rules/03-git-workflow.md)
# Worktree path: .claude/worktrees/phase-$ARGUMENTS
git worktree add .claude/worktrees/phase-$ARGUMENTS -b claude/phase-$ARGUMENTS-MK-<short-description> main
```

3. The session stays in the main checkout. Writers are spawned with `isolation: "worktree"`, the worktree path and its tip; each works in its own temporary worktree from that tip, and the session fast-forwards this worktree to their commits (rule 11, Placement; `/justin:team`, Placement). The session reads the worktree with `git -C` and absolute paths, and pushes with `git -C <W> push -u origin <branch>`.

### Phase 3: Planning

The working plan is the plan file the user approved in plan mode (`<planFilePath>` from the `ExitPlanMode` call).

4. Spawn worker with the task: copy the approved plan file `<planFilePath>` unchanged to `docs/knowledge/plans/<YYYY-MM-DD>-jl-phase-$ARGUMENTS-<description>.md` and commit `Phase $ARGUMENTS: working plan`, with rule 08 frontmatter.
5. Fast-forward the worktree to its commit and push (`/justin:team`, Placement).
6. Apply the plan's Phase Doc Reconciliation notes through doc-writer.
7. Compare the plan against the current phase doc (`docs/phases/`):
    - If planning reveals changes needed: update the phase doc through doc-writer, fast-forward and push
    - If no changes needed: note "Phase doc verified — no updates required" and proceed

### Phase 4: Implementation

8. Read the phase doc and create a task with TaskCreate for EACH step listed, grouped by milestone. Use the exact step names from the phase doc — do not paraphrase or invent steps.
9. For each milestone:
    For each step within the milestone (including the shared crate refactoring step baked into the phase doc):
    a. Read the step's acceptance criteria from the phase doc BEFORE writing any code
    b. Consult your working plan doc (`docs/knowledge/plans/*-jl-phase-$ARGUMENTS-*.md`) for the approach, key decisions, and files to modify
    c. Record the tip (`git -C <W> rev-parse HEAD`), then spawn kernel-dev (or worker or doc-writer for non-kernel steps) with the step text, the tip and the commit message `Phase $ARGUMENTS MN: Step X — <step description>` — the full step, no partial work
    d. Run `/justin:team`'s Placement on the writer's range: the range check, the Fable review (code-reviewer, `rules` and `bugs`) for a kernel-dev range, at most three rounds, then the writer's gate output at its head, the fast-forward and the clean-up. Route boots to the verifier on the new tip; a `DEFERRED` boot is retried later.
    e. If any gate fails: read the error, fix the root cause through a fresh writer, re-run — do not skip
    f. Push from the worktree with the explicit refspec (Placement's last step)
    g. Mark the task completed
    (A lead never edits files in a worktree, rule 11: every update and commit below, and in Phase 5, is done by a spawned worker or doc-writer, then `/justin:team`'s Placement on its range.)
    h. **Update the working plan doc**: record any issues encountered, decisions made, or lessons learned in the corresponding sections — do this as you go, not at the end
    After all steps in milestone complete (follow the rule 04 split: change-describing docs go in each step's commit; inventory sections go in the ship pass, Phase 5):
    i. Update the docs that describe the milestone's change: Key Technical Facts in `.claude/CLAUDE.md`, the phase doc (check off completed tasks)
    j. Dead code cleanup: Grep for `#[allow(dead_code)]` across `kernel/src/` and `shared/src/`. Remove the item if truly unused, or remove just the attribute if now used.
    k. Commit and push: `Phase $ARGUMENTS MN: update docs`

    The audit runs once, before `gh pr ready` (rule 02).

### Phase 5: Final Verification

⛔ **GATE: Do NOT proceed to Knowledge Distillation or PR until ALL of the following pass:**

10. Run `/verify-phase $ARGUMENTS` — build/test/QEMU quality gates must all pass
11. Simplify, once per PR: spawn simplifier with the PR range (merge-base with `origin/main` to head) and the tip, then run `/justin:team`'s Placement on its range. A change to kernel, stub or shared bytes (`KERNEL-BYTES: yes`) needs a verifier gate boot before the fast-forward
12. Run `/audit-loop` on the simplified head (it must converge before the PR is marked ready)
13. Ship pass: worker, in the foreground, updates the inventory sections (rule 11): Workspace Layout and the agent and skill tables in `.claude/CLAUDE.md`, `docs/project/doc-map.md`, README, developer-guide counts
14. Update the phase doc Status to "Complete", check off all Phase Completion Criteria
15. Update `docs/project/development-plan.md`: mark phase $ARGUMENTS as complete, update §8.1 Actual Progress with dates and deliverables
16. Commit and push

### Phase 6: Knowledge Distillation

17. Read the working plan doc (`docs/knowledge/plans/*-jl-phase-$ARGUMENTS-*.md`) and distill:
    - **Lessons** (bugs hit, surprises, workarounds, platform quirks) → Write each to `docs/knowledge/lessons/YYYY-MM-DD-cl-phase-$ARGUMENTS-description.md` with frontmatter: author, date, tags, status: final
    - **Decisions** (why X over Y, trade-offs made, architecture choices) → Write each to `docs/knowledge/decisions/YYYY-MM-DD-cl-phase-$ARGUMENTS-description.md` with frontmatter: author, date, tags, status: final
    - The plan's "Issues Encountered", "Decisions Made", and "Lessons Learned" sections (filled during Phase 4) are your primary source — distill from those
    - If nothing was learned (unlikely), note "No new lessons or decisions" and skip the writes
    - Delete the working plan doc (`git rm docs/knowledge/plans/*-jl-phase-$ARGUMENTS-*.md`)
    - Commit and push: `Phase $ARGUMENTS: knowledge distillation`

### Phase 7: PR, Review & Hand-off

18. Create the PR to main as a draft using `gh pr create --draft` (add `--label <team>` when `AIOS_TEAM` is set) with this structure:

```bash
gh pr create --title "Phase $ARGUMENTS: <phase name from phase doc>" --body "$(cat <<'EOF'
## Summary
- <what was implemented — milestones and key deliverables>
- <notable decisions or deviations from phase doc>

## Quality Gates
- [ ] `just check` — zero warnings
- [ ] `just test` — all pass
- [ ] verifier boot PASS with the phase's UART lines (run directory)
- [ ] `/audit-loop` — 0 issues

## Phase Doc
`docs/phases/<phase-doc-filename>.md`
EOF
)"
```

⛔ **GATE: Do NOT skip steps 19-20. Your part of the phase is NOT complete until the hand-off.**

19. Run `gh pr ready`, then `/review-pr-comments`: wait 3-7 minutes for Copilot/reviewer comments, then fix issues, reply, and resolve every conversation. Push fixes.
20. Hand off and stop. Merging is user-only (`/merge-and-cleanup` has `disable-model-invocation: true`; see `.claude/rules/03-git-workflow.md`):
    - Report the PR URL and the output of `gh pr checks <number>`
    - Ask the user to run `/merge-and-cleanup` once they approve; it preserves soak results, squash merges, deletes the branches, removes the worktree and fast-forwards main
    - Do not merge, push to `main`, or repeat those steps another way
