# Phase Implementation Workflow

When implementing Phase N:

1. **SESSION PREP**: run the session start checklist (below)
2. **WORKTREE**: Create isolated worktree at `.claude/worktrees/phase-N` — writers work inside it; the lead stays in the main checkout (rule 11)
3. **READ**: Phase doc → architecture docs → `.claude/rules/*` conventions → knowledge hive (lessons, decisions)
4. **PLAN**: write the working plan in plan mode from `docs/knowledge/plans/_template.md`. Leaving plan mode runs the Fable plan gate (rule 11, Reviews); after the owner approves, worker commits the plan file unchanged as the branch's first commit. Identify milestone, list files to create/modify, verify dependencies, track the steps with TaskCreate
5. **RECONCILE**: Compare plan against phase doc — update phase doc if needed, commit before implementation
6. **IMPLEMENT**: One step at a time. Each step is atomic — complete it fully before moving on. Every step has an "Acceptance:" block — this is your done condition. Update working plan with issues/decisions/lessons as you go.
7. **VERIFY**: Run acceptance criteria commands after each step
8. **COMMIT + PUSH**: the writer commits on its temporary branch after each step passes; the lead fast-forwards the branch worktree to it (kernel-dev ranges after the lead's Fable review, rule 11) and pushes (not batched)
9. **UPDATE DOCS** after each milestone:
   - In the change's own commit:
     - `.claude/CLAUDE.md`: Key Technical Facts and the lock order
     - Phase doc: Check off completed tasks, update Status field
     - Architecture docs: corrections or deviations from spec (owner approval for content)
     - Dead code cleanup: remove `#[allow(dead_code)]` if unused or if code is now used
   - In the ship pass:
     - `.claude/CLAUDE.md`: Workspace Layout and the agent and skill tables
     - `docs/project/doc-map.md`: topic index entries for new or moved architecture docs
     - README.md: Project Structure, Build Commands, status text
     - Developer guide: file sizes, test counts, new patterns
   - `/audit-loop` runs once before `gh pr ready` (rule 02). Fix all issues, commit.
10. **FINAL GATE**: Run `/verify-phase` + `/audit-loop` one final time before PR — must be 0 issues
11. **DISTILL**: Read working plan, extract lessons/decisions to knowledge hive, delete plan
12. **PR**: Open the PR as a draft, run `/audit-loop`, mark it ready, run `/review-pr-comments`, then hand off: report the PR URL and check status and ask the user to run `/merge-and-cleanup` (user-invocable only; never merge yourself)

**PLAN MODE**: step 4 always runs in plan mode. If the session is not in plan mode, `/implement-phase` calls EnterPlanMode first. The plan gate reviews the plan when the session calls ExitPlanMode.

**BLOCKED?** Read the referenced architecture doc section. Architecture docs are the source of truth. Never invent register offsets, struct fields, or memory addresses.

## Session Start Checklist

Before any implementation work:

1. `just check` shows zero warnings.
2. `rust-toolchain.toml`, `Cargo.lock` and QEMU change only through Renovate PRs (rule 11, Toolchain). No session bumps or installs them.
