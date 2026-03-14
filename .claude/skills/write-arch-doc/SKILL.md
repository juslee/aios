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

### 5b. External Research (Recursive)
Use WebSearch to find relevant research. **Search recursively** — each round of results
should inform follow-up queries until no new relevant material is found.

**Round 1 — Broad survey:**
- Recent OS research papers from OSDI, SOSP, USENIX ATC, EuroSys, NDSS
- Production OS approaches from seL4, Fuchsia, Zircon, Redox, Hubris, Theseus, LionsOS
- Industry best practices and novel techniques relevant to this subsystem

Example searches:
- `"<subsystem> operating system" site:usenix.org OR site:acm.org`
- `"<subsystem>" seL4 OR Fuchsia OR Zircon design`
- `"<subsystem>" microkernel capability-based`
- `"<subsystem>" 2024 2025 OSDI SOSP research paper`

**Round 2 — Follow specific leads:**
- Each paper/system found in Round 1 may reference related work — search for those
- Search for the specific technique names discovered (e.g., "scheduling context donation",
  "differentiated isolation", "control-plane data-plane separation")
- Search for formal verification or correctness proofs of the subsystem's data structures

**Round 3+ — Recursive depth:**
- Continue until a round produces no new relevant material
- Typical depth: 3-7 rounds depending on subsystem maturity
- Track all sources for citation in the doc

### 5c. AI-Focused Research
Since AIOS is an AI-first OS, explicitly search for AI/ML improvements to this subsystem:

- `AI machine learning <subsystem> optimization operating system 2024 2025`
- `reinforcement learning <subsystem> kernel scheduling 2024 2025`
- `graph neural network <subsystem> anomaly detection security`
- `LLM agent operating system <subsystem> context management`
- `"learned index" "learned data structure" <subsystem> optimization`
- `AI prediction prefetch <subsystem> workload characterization`

**Categorize AI findings as:**
- **AIRS-dependent** — requires semantic understanding (belongs in §13-style "AI-Native" section)
- **Kernel-internal ML** — purely statistical, can run as frozen decision tree in kernel (belongs in §14-style "Future Directions")

**Lesson learned (ipc.md):** AI research often lives in different academic communities
(ML systems, security/lateral-movement, database/learned-indexes) — search across domains,
not just OS conferences. The most novel ideas come from cross-domain application.

### 5d. Improvement Proposals
Present findings to the user:
- What ideas from research or other OSes could AIOS adopt?
- Which improvements align with AIOS's AI-first vision?
- Categorize as: "incorporate now" vs "future phase work"
- Separately present AI-driven improvements with the AIRS-dependent vs kernel-internal split

### 5e. User Decision
Use AskUserQuestion to let the user choose:
- Which improvements to incorporate into the architecture doc
- Which to defer as future work (note in a "Future Directions" section)
- Whether AI improvements go into the main AI-native section or Future Directions
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

## Step 6b: Document Splitting (when doc exceeds ~2000 lines)

If the document has grown beyond ~2000 lines after updates, propose splitting it:

### When to Split
- Doc exceeds ~2000 lines (navigability threshold)
- Doc covers 3+ distinct subsystems that can stand alone
- User requests it

### Hub + Sub-Document Pattern

**Placement rule:** All hub sub-docs go in a subfolder named after the hub
(e.g., `docs/kernel/boot.md` → sub-docs go in `docs/kernel/boot/`).
If a hub name would match the parent directory, rename the hub to avoid redundancy
(e.g., `docs/security/model.md` instead of `docs/security/security.md`).

1. **Keep the original filename as a hub** — this preserves all existing external links
2. **Hub contents**: §1 Overview, Document Map table, Implementation Order, Cross-Reference Index
3. **Sub-document naming**: `{topic}/{subtopic}.md` in a subfolder (e.g., `memory/physical.md`, `boot/firmware.md`)
4. **Preserve original section numbers** across sub-files for cross-reference stability
5. **Sub-document header format** (subfolder case — note `../` back-link to hub):
   ```markdown
   # AIOS <Title>

   Part of: [<hub>.md](../<hub>.md) — <Hub Title>
   **Related:** [sibling.md](./sibling.md) — description, [other.md](./other.md) — description
   ```
   **Sub-document header format** (flat case — same-dir `./` link to hub):
   ```markdown
   # AIOS <Title>

   Part of: [<hub>.md](./<hub>.md) — <Hub Title>
   **Related:** [sibling.md](./sibling.md) — description
   ```
6. **Hub Document Map table** (subfolder case):
   ```markdown
   | Document | Sections | Content |
   |---|---|---|
   | **This file** | §1, §N | Overview and ... |
   | [subtopic.md](./<hub>/subtopic.md) | §2, §4 | Description |
   ```
7. **Hub Cross-Reference Index**: table mapping every `§N.N` to its sub-file location

### Execution
- Create the subfolder (if needed) and sub-documents in parallel (independent files — use background agents)
- Group related sections together (e.g., physical memory + heap, virtual memory + shared memory)
- Update CLAUDE.md Architecture Document Map to point to specific sub-files
- Run bare code fence check on all new files (agents often create bare ``` fences)
- External docs linking to the hub still work — readers land on navigation page

### What NOT to Do
- Don't update every external doc that links to the hub — the hub serves as a redirect
- Don't renumber sections — stability is more important than sequential ordering within a sub-file

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
  Especially common when agents create sub-documents during splits. Run a programmatic check:
  track open/close fence state and verify every opening fence has a language specifier.
- **Double blank lines**: Left behind after removing sections — collapse to single blank line
- **Stale cross-references**: Links to sections/docs that were renamed or restructured
- **Cross-file section refs after split**: `§N.N` references must point to the correct sub-file
- **Explicit padding fields**: If code uses implicit compiler padding (no `_padding` field),
  the doc should NOT add an explicit `_padding` field — describe padding in a comment instead
- **Aspirational enum variants in "as implemented" blocks**: If a code block says "as implemented in X"
  but includes enum variants that don't exist in code, comment them out with `// --- Target design ---`
- **Lock-free claims**: Verify whether data structures are actually lock-free. `MessageRing` under
  a Mutex is NOT lock-free even if it's a ring buffer. Be precise about concurrency properties.
- **Markdown lint: lists after paragraphs**: Lists must have a blank line before them when they
  follow a paragraph. Common in "This means:" followed by bullet points.

**Bare code fence detection (CRITICAL — run BEFORE audit agent):**
Run this in the worktree directory to find bare opening fences:
```bash
python3 -c "
fence = chr(96)*3
lines = open('<file>').readlines()
inside = False
for i, line in enumerate(lines, 1):
    stripped = line.strip()
    if not inside:
        if stripped.startswith(fence):
            inside = True
            if stripped == fence:
                print(f'{i}: bare opening fence')
    else:
        if stripped == fence:
            inside = False
"
```
Fix all bare fences by adding `text`, `rust`, `asm`, or `mermaid` as appropriate.

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
5. Research state-of-the-art improvements (recursive + AI-focused)
6. Write / update document (section by section)
7. Update cross-references (CLAUDE.md, related docs)
8. Run doc-auditor loop until clean
9. Update this skill with lessons learned
10. Commit, push, and create PR
```

## Lessons Learned

Accumulated from actual doc updates. Each entry includes the doc and the lesson.

### From `docs/kernel/ipc.md` (2026-03-13)

**Research depth matters.** 7 rounds of recursive web search across ~20 queries found material
that a single round would have missed. Cross-domain searches (security/GNN, database/learned-indexes,
ML-systems/RL-scheduling) produced the most novel ideas. Always search beyond the OS community.

**AI improvements split into two categories.** AIRS-dependent features (need semantic understanding)
go into the "AI-Native" section (§13). Kernel-internal ML features (purely statistical, frozen
decision trees) go into "Future Directions" (§14). This split is architecturally important because
kernel-internal ML has no AIRS dependency — it works even if AIRS is offline.

**"As implemented" blocks must be exact.** When a code block says "as implemented in `shared/src/ipc.rs`",
every field, type, and variant must match the actual code. Implicit compiler padding should NOT be
shown as an explicit `_padding` field. Aspirational enum variants must be commented out with
`// --- Target design ---` markers.

**Bare code fence detection is fragile.** Python scripts that track open/close fence state can
produce false positives if the working directory is wrong or line numbers shift after edits.
Always run detection from the worktree directory with an absolute path, and verify the fix worked.

**Markdown lint: blank lines before lists.** Every list that follows a paragraph needs a blank line
separator. This is easy to miss when writing "This means:\n- item". Add the blank line proactively.

**Section renumbering cascade.** Adding new subsections (e.g., §13.7-13.9) requires renumbering
the summary section (§13.7 → §13.10). Always check that existing cross-references from other docs
use section numbers that didn't change — new sections should be inserted before summary/table sections,
not in the middle of numbered content that other docs reference.

### From `docs/kernel/scheduler.md` (2026-03-13)

**Time slice consistency cascade.** Changing one time slice value (e.g., Interactive 4ms→10ms) requires
updating 6+ locations across the doc: the class description, SchedulerConfig defaults, timer table,
low-battery mode halved values, input boost duration, and WFQ example code/prose. Use grep for the
old numeric value (e.g., `4_000_000`, `4ms`, `4 ms`) to find all instances before editing.

**Timer register naming matters.** The doc originally used virtual timer registers (CNTV_*) but code
uses physical timer (CNTP_*). This is a subtle factual error easily missed in Mermaid diagrams and
prose. Always verify which timer the kernel actually uses by checking ThreadContext field names.

**Doc splits create stale cross-references.** When memory.md was split into sub-documents
(memory/physical.md, memory/reclamation.md, etc.), cross-references from other docs like
scheduler.md still pointed to the old `memory.md §8`. The auditor caught this — always grep
for `memory.md §` (or any recently-split doc) across all docs during audit.

**Research parallelism works well.** Three parallel research agents (production OS survey, academic
papers, AI-focused) found overlapping material (EEVDF, seL4 MCS, sched_ext, ghOSt appeared in
multiple reports), which validates key themes. Unique finds came from the AI agent (LAKE, PWU,
OS-R1, two-stage phase detection). The overlap provides confidence; the unique finds provide novelty.

**Insert new subsections before cross-refs/references, not after.** When adding §16.7-16.8 to the
AI-Driven Scheduling section, inserting before the Cross-References (§16.7→§16.9) and References
(§16.8→§16.10) sections kept the renumbering contained to two utility sections that no external
doc references by number.
