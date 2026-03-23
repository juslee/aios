---
name: implement-phase
description: >
  Implements an AIOS phase step by step from the phase doc.
  Use when asked to implement Phase N or work on a specific phase.
argument-hint: "[phase-number]"
---

# Implement AIOS Phase $ARGUMENTS

Follow the Phase Implementation Workflow from CLAUDE.md:

## Phase 1: Research & Planning

1. Read `docs/phases/` and find the doc matching phase $ARGUMENTS (glob for `$ARGUMENTS-*.md` or `0$ARGUMENTS-*.md`)
2. Read all Architecture References listed in the phase doc
3. Read CLAUDE.md Code Conventions and Quality Gates
4. Search the knowledge hive for relevant lessons and decisions:
    - Use Obsidian MCP search_notes with keywords from the phase doc
    - Review any matching docs/knowledge/lessons/ and docs/knowledge/decisions/
    - Factor known pitfalls into implementation approach
5. Write a working plan doc using the Write tool:
    - Path: `docs/knowledge/plans/phase-$ARGUMENTS-description.md`
    - Required frontmatter: `author: claude`, `date: YYYY-MM-DD`, `tags: [...]`, `status: in-progress`
    - Start with a **Context** section: why this phase matters, current codebase state, what exists to build on
    - For each step in the phase doc, write a **### Step N** section containing:
      - **Files to create/modify:** explicit paths
      - **Types/traits/functions to define:** names, signatures, field layouts
      - **Key decisions:** naming, data structures, algorithms, deviations from arch docs (with rationale)
      - **Reuse:** existing code to delegate to or wrap (with file paths)
      - **Tests:** what to test, expected counts
      - **Acceptance:** the exact commands and expected output
    - End with a **Risks & Mitigations** table and a **Verification** checklist
    - This plan is your implementation roadmap — do NOT skip it
    - Verify: confirm the file was written before proceeding to Phase 2

## Phase 2: Phase Doc Reconciliation

6. Compare the plan against the current phase doc (`docs/phases/`):
    - If planning reveals changes needed (new steps, reordered steps, updated acceptance criteria, corrected references):
      update the phase doc using the Edit tool to match the plan
    - If no changes are needed, note "Phase doc verified — no updates required" and proceed
    - If changes were made: commit and push phase doc updates before any implementation begins
    - This ensures the phase doc is the accurate source of truth for implementation

## Phase 3: Session Prep & Worktree

7. Run the session start checklist (from `.claude/rules/04-phase-workflow.md`):

```bash
brew upgrade qemu just
```

Update Rust nightly in `rust-toolchain.toml` if needed, then:

```bash
cargo update
just check
```

Commit `Cargo.lock` and `rust-toolchain.toml` if changed.

8. Create an isolated worktree for all implementation work:

```bash
# Ensure we're on main and up to date
git checkout main && git pull origin main

# Create worktree with a new branch
# Branch name: claude/phase-$ARGUMENTS-MK-<short-description> (matches CLAUDE.md convention)
# Worktree path: .claude/worktrees/phase-$ARGUMENTS
git worktree add .claude/worktrees/phase-$ARGUMENTS -b claude/phase-$ARGUMENTS-MK-<short-description> main
```

9. **Switch working directory** to the worktree. ALL subsequent work (implementation, commits, pushes, quality gates) happens inside the worktree:

```bash
cd .claude/worktrees/phase-$ARGUMENTS
```

**IMPORTANT**: From this point forward, every file edit, git command, build command, and test command MUST be executed inside the worktree directory. Do NOT operate in the main repo directory until `/merge-and-cleanup` at the end.

## Phase 4: Implementation

10. Read the phase doc and create a TodoWrite entry for EACH step listed, grouped by milestone. Use the exact step names from the phase doc — do not paraphrase or invent steps.
11. For each milestone:
    For each step within the milestone (including the shared crate refactoring step baked into the phase doc):
    a. Read the step's acceptance criteria from the phase doc BEFORE writing any code
    b. Consult your working plan doc for the approach, key decisions, and files to modify
    c. Implement the step using Edit/Write tools — complete the full step, no partial work
    d. Run the step's acceptance criteria commands (build, test, QEMU as applicable)
    e. If any gate fails: read the error, fix the root cause, re-run — do not skip
    f. Commit and push: `Phase $ARGUMENTS MN: Step X — <step description>`
    g. Mark the TodoWrite item as completed
    After all steps in milestone complete:
    h. Update CLAUDE.md, README.md, phase doc (check off completed tasks)
    i. Commit and push: `Phase $ARGUMENTS MN: update docs`

## Phase 5: Verify & Audit

12. Dead code cleanup: use the Grep tool to search for `#[allow(dead_code)]` across `kernel/src/` and `shared/src/`. For each match: remove the item if truly unused, or remove just the attribute if the code is now used. Commit and push.
13. Run `/verify-phase $ARGUMENTS` — build/test/QEMU quality gates must all pass
14. Run `/audit-loop` — recursive triple audit (doc, code review, security/bug review) that loops until 0 issues
15. Update the phase doc Status to "Complete", check off all Phase Completion Criteria, commit and push

## Phase 6: Knowledge Distillation

16. Read the working plan doc (`docs/knowledge/plans/phase-$ARGUMENTS-*.md`) and distill:
    - **Lessons** (bugs hit, surprises, workarounds, platform quirks) → Write each to `docs/knowledge/lessons/YYYY-MM-DD-cl-phase-$ARGUMENTS-description.md` with frontmatter: author, date, tags, status: final
    - **Decisions** (why X over Y, trade-offs made, architecture choices) → Write each to `docs/knowledge/decisions/YYYY-MM-DD-cl-phase-$ARGUMENTS-description.md` with frontmatter: author, date, tags, status: final
    - If nothing was learned (unlikely), note "No new lessons or decisions" and skip the writes
    - Delete the working plan doc
    - Commit and push

## Phase 7: PR, Review & Merge

17. Create PR to main using `gh pr create` with this structure:

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

18. Run `/review-pr-comments`: wait for Copilot/reviewer comments, fix issues, reply, resolve conversations, push fixes
19. Run `/merge-and-cleanup`: squash merge the PR, delete remote/local branch, remove worktree, fast-forward main
    - `/merge-and-cleanup` auto-detects the worktree, removes it, and returns to the main repo
