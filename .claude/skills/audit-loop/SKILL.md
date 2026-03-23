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

Check changed files against the base branch:

```bash
git diff --name-only main...HEAD
```

- **Docs-only mode**: If ALL changed files are `.md` files → run doc audit only
- **Full mode**: If ANY non-`.md` files are changed → run all three audits

## Audit Categories

### Docs-only mode (all changes are .md files)

1. **Doc audit**: Cross-reference errors, technical accuracy, naming consistency in all modified docs

### Full mode (any code changes)

1. **Doc audit**: Cross-reference errors, technical accuracy, naming consistency in all modified docs
2. **Code review**: Convention compliance, unsafe documentation, W^X enforcement, naming, dead code
3. **Security/bug review**: Logic errors, address confusion (virt vs phys), PTE bit correctness, race conditions

## Convergence Protocol

The audit uses a **two-level loop**: an inner loop that fixes until 0 issues, then an outer loop that restarts fresh to confirm no regressions. Done only when a fresh restart finds 0 issues on its first round.

```
OUTER LOOP:
  INNER LOOP (Round N):
    a. Run applicable audits (1 or 3 depending on mode)
    b. Count total issues found
    c. If >0 issues:
       - Fix all genuine issues
       - Commit and push fixes
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
