---
name: generate-phase-doc
description: >
  Generates a phase implementation doc from architecture docs.
  Use when asked to write or generate a phase doc for Phase N.
argument-hint: "[phase-number]"
---

# Generate Phase Doc for Phase $ARGUMENTS

Follow the Phase Doc Generation Workflow from CLAUDE.md:

1. Read `docs/project/development-plan.md` §8 for phase $ARGUMENTS name and deliverable
2. Identify relevant architecture docs (use Architecture Document Map in CLAUDE.md)
3. Read those architecture docs
4. Read the previous phase doc for milestone numbering continuity
5. Create branch `claude/phase-$ARGUMENTS-docs` from main
6. Generate `docs/phases/` with the correct `NN-name.md` filename
7. Follow the Phase 0/1 template structure exactly (see CLAUDE.md)
8. Milestone numbers: M(3*$ARGUMENTS+1) through M(3*$ARGUMENTS+3)
9. Run doc-auditor to validate (loop until clean)
10. Commit and create PR for review
