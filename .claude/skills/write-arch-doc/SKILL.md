---
name: write-arch-doc
description: >
  Interactive workflow for creating or updating AIOS architecture documents.
  Includes research phase for state-of-the-art improvements. Use when asked
  to write, create, or update an architecture doc for a subsystem.
argument-hint: "<topic-or-path>"
---

# Write / Update Architecture Document: $ARGUMENTS

Interactive, human-guided workflow for creating new architecture docs or updating
existing ones. Includes research for state-of-the-art improvements from OS research
and production systems.

## ⚠️ Critical Principle: Architecture Docs ≠ Implementation Docs

Architecture documents describe the **target design** (the vision/future state).
Phase docs (`docs/phases/`) track what has been implemented.

**Rules:**
- **NEVER** add implementation status banners ("as of Phase X", "Current state:",
  "Implementation status:", "Steps N-M planned for future phases")
- **NEVER** remove aspirational/future content (e.g., future Platform trait methods,
  Apple Silicon targets, `maxcpus=` options) just because it isn't implemented yet
- **DO** fix factual errors: wrong struct field names, wrong function names, wrong
  register addresses, wrong constant values — compare against actual code
- **DO** add research-informed improvements to a "Future Directions" section
- When in doubt: if content describes where the system is *going*, it stays.
  If content describes where the system *is*, it belongs in phase docs instead.

## Step 1: Discover & Detect Mode

Determine whether this is a CREATE or MAINTAIN operation:

1. Search CLAUDE.md Architecture Document Map for `$ARGUMENTS`
2. Glob for matching files: `docs/**/*$ARGUMENTS*.md`

**If a doc exists** → MAINTAIN mode:
- Read the existing architecture doc fully
- Read recent git log for implementation commits that may have changed this subsystem
- Compare doc's code references (struct names, function names, constants, file paths) against actual code
- Identify sections with **factual errors** (wrong names, addresses, types) vs sections that are
  aspirational/future-looking (these are correct by design and should NOT be changed)
- Identify sections that may be stale or incomplete

**If no doc exists** → CREATE mode:
- Read `docs/project/development-plan.md` — find which phases depend on this subsystem
- Read `docs/project/architecture.md` — find the high-level design for this subsystem
- Read 2-3 related architecture docs for style and cross-reference patterns

In both modes, read related architecture docs for cross-reference context.

## Step 2: Scope (Interactive)

Use AskUserQuestion to clarify scope with the user:

**CREATE mode — ask:**
- Target audience? (kernel dev, platform dev, application dev)
- Which aspects to cover? (propose sections based on discovery)
- Any specific design decisions or constraints to document?
- Known related docs to cross-reference?

**MAINTAIN mode — ask:**
- What triggered this update? (phase implementation, bug discovery, design change)
- Which sections need updating? (propose based on git diff analysis)
- Any new sections to add?

**Categorize proposed changes as:**
- **Factual corrections** — struct/function/constant names that don't match code (MUST fix)
- **Structural improvements** — better organization, missing cross-references, diagrams
- **Research additions** — new "Future Directions" content from external research
- **Remove implementation status** — delete any "as of Phase X" banners or "Current state" paragraphs

Present proposed scope summary. Iterate until user approves.

## Step 3: Worktree & Branch

Create an isolated worktree for this work:

1. Derive a sanitized `$TOPIC` slug from `$ARGUMENTS` for safe use in paths and branch names:
   - If `$ARGUMENTS` is a path, first take the basename without extension (e.g., `docs/kernel/memory.md` → `memory`)
   - Then: lowercase, replace spaces/non-alphanumeric with `-`, collapse repeats, trim leading/trailing `-`, restrict to `[a-z0-9-]` (e.g., `Shared Memory` → `shared-memory`)
2. Run: `git worktree add .claude/worktrees/docs-$TOPIC -b claude/docs-update-$TOPIC main`
3. All subsequent file operations happen in the worktree path

## Step 4: Outline / Change Plan (Interactive)

**CREATE mode:**
- Generate a section outline matching existing doc patterns
- Use the structure of `docs/kernel/memory.md` or `docs/kernel/hal.md` as a template:
  - Header with metadata (audience, scope, related docs)
  - Table of contents
  - Numbered sections with subsections
  - Mermaid v11 diagrams for architecture and data flow
  - Cross-reference index at the end
- Present outline to user for feedback
- Iterate until user approves the structure

**MAINTAIN mode:**
- List specific sections to update with proposed changes
- Describe what will change and why
- Present change plan to user for approval

## Step 5: Research & Improve

Before writing, research state-of-the-art approaches for this subsystem:

### 5a. Internal Analysis
- Review AIOS's current implementation and architecture for this topic
- Identify strengths, gaps, and areas where the design could be improved
- Note any deviations between current code and documented architecture

### 5b. External Research
Use WebSearch to find:
- Recent OS research papers from OSDI, SOSP, USENIX ATC, EuroSys
- Production OS approaches from seL4, Fuchsia, Zircon, Redox, Hubris, Theseus
- Industry best practices and novel techniques relevant to this subsystem

Example searches:
- `"<subsystem> operating system" site:usenix.org OR site:acm.org`
- `"<subsystem>" seL4 OR Fuchsia OR Zircon design`
- `"<subsystem>" microkernel capability-based`

### 5c. Improvement Proposals
Present findings to the user:
- What ideas from research or other OSes could AIOS adopt?
- Which improvements align with AIOS's AI-first vision?
- Categorize as: "incorporate now" vs "future phase work"

### 5d. User Decision
Use AskUserQuestion to let the user choose:
- Which improvements to incorporate into the architecture doc
- Which to defer as future work (note in a "Future Directions" section)
- Document accepted improvements with citations/references

## Step 6: Write / Update (Interactive, section by section)

Write each major section, presenting to the user for review after each:

1. Write one section at a time
2. Show the section to the user, ask for feedback
3. Iterate until the user approves that section
4. Move to the next section

**Writing guidelines:**
- Cross-reference other architecture docs — never duplicate content
- Use Mermaid v11 diagrams for architecture, data flow, and state machines
- Reference specific code paths where implemented (e.g., `kernel/src/mm/buddy.rs`)
- Incorporate accepted research improvements from Step 5
- For design trade-offs with multiple valid approaches: ask the user for their input
- Commit incrementally if the doc is large (one commit per major section)

**Content rules (CRITICAL):**
- **Keep aspirational content** — future API methods, planned platform targets, design goals
  that aren't yet implemented. This is the *architecture* doc, not the *status* doc.
- **Remove implementation status language** — delete any "as of Phase X", "Currently only X
  is implemented", "Steps N-M are planned for future phases", "Implementation status:"
- **Fix factual references only** — if the doc says `UART_BASE: AtomicU64` but code says
  `UART_BASE_ADDR: AtomicUsize`, fix the doc. If the doc describes a future API that
  doesn't exist yet, leave it.
- **All code blocks must have language specifiers** — use `rust`, `asm`, `text`, etc.
  Never leave bare ``` fences (causes markdown lint failures).

## Step 7: Cross-reference Updates

1. Add or update the entry in CLAUDE.md Architecture Document Map
2. Update any existing docs that should reference this new/updated doc
3. Ensure phase docs that reference this subsystem have correct pointers
4. If `docs/project/developer-guide.md` exists, check its cross-reference index

## Step 8: Audit Loop (Mandatory)

Run doc-auditor to validate the document:

1. Spawn the doc-auditor agent on all modified docs
2. Fix all issues found (cross-reference errors, terminology, technical accuracy)
3. Re-audit until clean (max 10 passes)
4. Commit audit fixes

**Common issues the auditor catches (from experience):**
- **Naming mismatches**: Doc says `UART_BASE` but code says `UART_BASE_ADDR` — fix doc to match code
- **Type mismatches**: Doc says `AtomicU64` but code says `AtomicUsize` — fix doc to match code
- **File path errors**: Doc references `mm/asid.rs` but the symbol actually lives in `mm/uspace.rs`
- **Unfenced code blocks**: Bare ``` without language specifier — add `rust`, `asm`, `text`, etc.
- **Double blank lines**: Left behind after removing sections — collapse to single blank line
- **Stale cross-references**: Links to sections/docs that were renamed or restructured

## Step 9: Commit + PR

1. Commit with message: `Docs: Add <topic> architecture document` (CREATE)
   or `Docs: Update <topic> architecture document` (MAINTAIN)
2. Push branch to remote
3. Create PR to `main` using `gh pr create`
4. Wait 3-5 minutes for Copilot/reviewer comments
5. Address review comments, commit fixes
6. Report PR URL to user

## TodoWrite Template

Create these todo items at the start:

```text
1. Discover & detect mode (CREATE or MAINTAIN)
2. Scope discussion with user
3. Create worktree and branch
4. Present outline / change plan for approval
5. Research state-of-the-art improvements
6. Write / update document (section by section)
7. Update cross-references (CLAUDE.md, related docs)
8. Run doc-auditor loop until clean
9. Commit, push, and create PR
```
