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

- **If plan mode IS active** → Execute **Planning Path** below, then call `ExitPlanMode`. Do NOT proceed to the Execution Path.
- **If plan mode is NOT active** → Skip the Planning Path entirely. Go straight to the **Execution Path**.

---

## Planning Path (plan mode only)

When plan mode is active, you can only read files and write to the system-assigned plan file. No edits, builds, commits, or worktree creation.

### P1. Research

1. Read `docs/phases/` and find the doc matching phase $ARGUMENTS (glob for `$ARGUMENTS-*.md` or `0$ARGUMENTS-*.md`)
2. Read all Architecture References listed in the phase doc
3. Read CLAUDE.md Code Conventions (`.claude/rules/`) and Quality Gates
4. Search the knowledge hive for relevant lessons and decisions:
    - Use Obsidian MCP search_notes with keywords from the phase doc
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

### P3. Exit Plan Mode

Call `ExitPlanMode` so the user can review the plan. **STOP HERE** — do not proceed to the Execution Path.

---

## Execution Path (normal mode)

### Phase 1: Session Prep

1. Run the session start checklist (from `.claude/rules/04-phase-workflow.md`):

```bash
brew upgrade qemu just
```

Update Rust nightly in `rust-toolchain.toml` if needed, then:

```bash
cargo update
just check
```

Commit `Cargo.lock` and `rust-toolchain.toml` to `main` only if changed (toolchain updates are the one exception to the no-commit-to-main rule).

### Phase 2: Worktree Setup

2. Create an isolated worktree for ALL subsequent work:

```bash
# Ensure we're on main and up to date
git checkout main && git pull origin main

# Create worktree with a new branch
# Branch name: claude/phase-$ARGUMENTS-MK-<short-description> (matches CLAUDE.md convention)
# Worktree path: .claude/worktrees/phase-$ARGUMENTS
git worktree add .claude/worktrees/phase-$ARGUMENTS -b claude/phase-$ARGUMENTS-MK-<short-description> main
```

3. **Switch working directory** to the worktree:

```bash
cd .claude/worktrees/phase-$ARGUMENTS
```

**IMPORTANT**: From this point forward, every file read/write, git command, build command, and test command MUST be executed inside the worktree directory. Do NOT operate in the main repo directory until `/merge-and-cleanup` at the end.

### Phase 3: Planning

Check whether a **plan already exists from a prior plan-mode session**. Look for the system plan file (the path from the earlier plan mode session, typically `~/.claude/plans/*.md`). Also check if context from the Planning Path is available in the current conversation.

**If a plan exists from plan mode:**

4. Copy the plan content into `docs/knowledge/plans/phase-$ARGUMENTS-description.md` inside the worktree
5. Commit and push as the first commit: `Phase $ARGUMENTS: working plan`
6. Apply any Phase Doc Reconciliation notes from the plan — edit the phase doc if changes were identified, commit and push

**If NO prior plan exists (skill invoked directly without plan mode):**

4. Read `docs/phases/` and find the doc matching phase $ARGUMENTS (glob for `$ARGUMENTS-*.md` or `0$ARGUMENTS-*.md`)
5. Read all Architecture References listed in the phase doc
6. Read CLAUDE.md Code Conventions and Quality Gates
7. Search the knowledge hive for relevant lessons and decisions:
    - Use Obsidian MCP search_notes with keywords from the phase doc
    - Review any matching docs/knowledge/lessons/ and docs/knowledge/decisions/
    - Factor known pitfalls into implementation approach
8. Write a working plan doc using the Write tool, based on the existing template:
    - Read `docs/knowledge/plans/_template.md` first — use its structure as the skeleton
    - Path: `docs/knowledge/plans/phase-$ARGUMENTS-description.md`
    - Fill in the template sections:
      - **Frontmatter**: set `author: claude`, `date: YYYY-MM-DD`, `tags: [relevant subsystem tags]`, `status: in-progress`, `phase: $ARGUMENTS`, `milestone: MK`
      - **Approach**: why this phase matters, current codebase state, key gaps, shared crate plan
      - **Progress**: for each step in the phase doc, write a checkbox item with granular sub-tasks (files to create/modify, types/traits/functions, acceptance commands)
      - **Code Structure Decisions**: naming, data structures, algorithms, deviations from arch docs (with rationale)
      - **Dependencies & Risks**: what must exist before this work starts, what could go wrong
    - This plan is your implementation roadmap — do NOT skip it
    - Verify: confirm the file was written before proceeding
9. Commit the plan as the **first commit** on the feature branch:
    - `git add docs/knowledge/plans/phase-$ARGUMENTS-*.md`
    - Commit: `Phase $ARGUMENTS: working plan`
    - Push: `git push -u origin HEAD`
10. Compare the plan against the current phase doc (`docs/phases/`):
    - If planning reveals changes needed: update the phase doc, commit and push
    - If no changes needed: note "Phase doc verified — no updates required" and proceed

### Phase 4: Implementation

11. Read the phase doc and create a TodoWrite entry for EACH step listed, grouped by milestone. Use the exact step names from the phase doc — do not paraphrase or invent steps.
12. For each milestone:
    For each step within the milestone (including the shared crate refactoring step baked into the phase doc):
    a. Read the step's acceptance criteria from the phase doc BEFORE writing any code
    b. Consult your working plan doc (`docs/knowledge/plans/phase-$ARGUMENTS-*.md`) for the approach, key decisions, and files to modify
    c. Implement the step using Edit/Write tools — complete the full step, no partial work
    d. Run the step's acceptance criteria commands (build, test, QEMU as applicable)
    e. If any gate fails: read the error, fix the root cause, re-run — do not skip
    f. Commit and push: `Phase $ARGUMENTS MN: Step X — <step description>`
    g. Mark the TodoWrite item as completed
    h. **Update the working plan doc**: record any issues encountered, decisions made, or lessons learned in the corresponding sections — do this as you go, not at the end
    After all steps in milestone complete:
    i. Update CLAUDE.md, README.md, phase doc (check off completed tasks)
    j. Dead code cleanup: Grep for `#[allow(dead_code)]` across `kernel/src/` and `shared/src/`. Remove the item if truly unused, or remove just the attribute if now used.
    k. Run `/audit-loop` — recursive triple audit (doc, code review, security/bug review) until 0 issues. Fix all issues found.
    l. Commit and push: `Phase $ARGUMENTS MN: update docs`

### Phase 5: Final Verification

⛔ **GATE: Do NOT proceed to Knowledge Distillation or PR until ALL of the following pass:**

13. Run `/verify-phase $ARGUMENTS` — build/test/QEMU quality gates must all pass
14. Run `/audit-loop` one final time — must return 0 issues across all three categories (doc, code, security/bug). If any issues found, fix them, commit, and re-run until clean.
15. Update the phase doc Status to "Complete", check off all Phase Completion Criteria, commit and push

### Phase 6: Knowledge Distillation

17. Read the working plan doc (`docs/knowledge/plans/phase-$ARGUMENTS-*.md`) and distill:
    - **Lessons** (bugs hit, surprises, workarounds, platform quirks) → Write each to `docs/knowledge/lessons/YYYY-MM-DD-cl-phase-$ARGUMENTS-description.md` with frontmatter: author, date, tags, status: final
    - **Decisions** (why X over Y, trade-offs made, architecture choices) → Write each to `docs/knowledge/decisions/YYYY-MM-DD-cl-phase-$ARGUMENTS-description.md` with frontmatter: author, date, tags, status: final
    - The plan's "Issues Encountered", "Decisions Made", and "Lessons Learned" sections (filled during Phase 4) are your primary source — distill from those
    - If nothing was learned (unlikely), note "No new lessons or decisions" and skip the writes
    - Delete the working plan doc (`git rm docs/knowledge/plans/phase-$ARGUMENTS-*.md`)
    - Commit and push: `Phase $ARGUMENTS: knowledge distillation`

### Phase 7: PR, Review & Merge

18. Create PR to main using `gh pr create` with this structure:

```bash
gh pr create --title "Phase $ARGUMENTS: <phase name from phase doc>" --body "$(cat <<'EOF'
## Summary
- <what was implemented — milestones and key deliverables>
- <notable decisions or deviations from phase doc>

## Quality Gates
- [ ] `just check` — zero warnings
- [ ] `just test` — all pass
- [ ] `just run` — QEMU acceptance criteria met
- [ ] `/audit-loop` — 0 issues

## Phase Doc
`docs/phases/<phase-doc-filename>.md`
EOF
)"
```

19. Run `/review-pr-comments`: wait for Copilot/reviewer comments, fix issues, reply, resolve conversations, push fixes
20. Run `/merge-and-cleanup`: squash merge the PR, delete remote/local branch, remove worktree, fast-forward main
    - `/merge-and-cleanup` auto-detects the worktree, removes it, and returns to the main repo
