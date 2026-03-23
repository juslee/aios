---
name: audit-loop
description: >
  Recursive audit that loops until a clean round with 0 issues.
  Auto-detects scope: docs-only (.md files) runs doc audit only;
  code changes run full triple audit (doc, code review, security/bug review).
  Use before creating a PR.
---

# Recursive Audit Loop

Auto-detect which audits to run based on changed files, then loop until 0 issues.

## Scope Detection

Run this Bash command to get the list of changed files:

```bash
git diff --name-only main...HEAD
```

- **Docs-only mode**: If ALL changed files are `.md` files → run doc audit only
- **Full mode**: If ANY non-`.md` files are changed → run all three audits

## How to Run Each Audit

Spawn Agent tool subagents for each audit type. Run all applicable audits in parallel within each round.

### 1. Doc audit (both modes)

Spawn a `doc-auditor` agent with this prompt:
> "Audit all modified docs for: cross-reference errors (broken links, wrong section numbers), technical accuracy (struct/function/constant names matching actual code), naming consistency, bare code fences (opening ``` without language specifier), stale cross-references to split docs. List of modified docs: [paste file list]. Report each issue with file path, line number, and what's wrong. Return the total issue count."

### 2. Code review (full mode only)

Spawn a `code-reviewer` agent with this prompt:
> "Review all modified code files for: convention compliance (see .claude/rules/01-code-conventions.md), unsafe documentation (every unsafe block needs SAFETY comment with invariant + maintainer + violation consequence), W^X enforcement (no page both writable and executable), naming conventions (snake_case functions, CamelCase types, SCREAMING_SNAKE constants), dead code (#[allow(dead_code)] that can be removed). List of modified files: [paste file list]. Report each issue with file path, line number, and what's wrong. Return the total issue count."

### 3. Security/bug review (full mode only)

Spawn a `code-reviewer` agent with this prompt:
> "Review all modified code files for: logic errors, address confusion (virtual vs physical addresses — check every pointer cast and address arithmetic), PTE bit correctness (permissions, attributes), race conditions (shared state accessed without proper synchronization), integer overflow in address calculations, missing barrier instructions (DSB/ISB after MMU/TLB operations). List of modified files: [paste file list]. Report each issue with file path, line number, and what's wrong. Return the total issue count."

## Convergence Protocol

The audit uses a **two-level loop**: an inner loop that fixes until 0 issues, then an outer loop that restarts fresh to confirm no regressions. Done only when a fresh restart finds 0 issues on its first round.

```
OUTER LOOP:
  INNER LOOP (Round N):
    a. Spawn applicable audit agents (1 or 3 depending on mode) in parallel
    b. Collect results, count total issues found across all agents
    c. If >0 issues:
       - Fix all genuine issues using Edit tool
       - Commit and push: "Audit round N: fix <summary>"
       - Re-check scope (fixes may have added non-.md files — upgrade to full mode if needed)
       - Go to Round N+1
    d. If 0 issues:
       - Exit inner loop → restart outer loop (fresh audit from scratch)

  OUTER LOOP EXIT:
    - If the fresh restart finds 0 issues on its FIRST round → DONE
    - Otherwise, enter inner loop again to fix new issues
```

**Example (full)**:
- Round 1: 4 issues → fix, commit, push
- Round 2: 2 issues → fix, commit, push
- Round 3: 0 issues → restart fresh
- Round 4: 2 issues → fix, commit, push (previous fixes introduced regressions)
- Round 5: 0 issues → restart fresh
- Round 6: 0 issues → **done** (fresh start was clean)

**Example (docs-only)**:
- Round 1: 2 doc issues → fix, commit, push
- Round 2: 0 issues → restart fresh
- Round 3: 0 issues → **done**

## Guidelines

- Fix all genuine issues. Do not dismiss issues without clear justification.
- Each fix round gets its own commit: `Audit round N: fix <summary of changes>`
- If an issue is a false positive, document why and skip it.
- Maximum 10 rounds. If not converging after 10 rounds, stop and report to user.
