---
name: build-team
description: >
  Bootstraps the AIOS development team for autonomous operation.
  Creates team, spawns specialist agents, assigns initial tasks.
---

# Build AIOS Development Team

## Step 1: Create team

Use the TeamCreate tool to create team "aios-dev". If TeamCreate fails, report the error and stop.

## Step 2: Spawn team-lead

Use the Agent tool to spawn a `team-lead` agent (subagent_type: `team-lead`) with this prompt:
> "You are the AIOS team lead. Read CLAUDE.md to understand project state, conventions, and the current phase. Then determine what work is next."

Verify the agent was spawned by checking for a response.

## Step 3: Team-lead gathers context

The team-lead should:
1. Read CLAUDE.md — understand workspace layout, technical facts, completed phases
2. Search the knowledge hive for relevant context:
    - Use Obsidian MCP search_notes for lessons and decisions related to current work
    - Read `docs/knowledge/plans/` for any in-progress working plans (Glob for `docs/knowledge/plans/*.md`)
3. Check current phase progress:
    - Read the latest phase doc (Glob for `docs/phases/*.md`, sort by name, read the last one)
    - Check which milestones are marked complete (look for `[x]` vs `[ ]` in task checkboxes)
    - Determine the next incomplete milestone or phase

## Step 4: Spawn specialist agents

The team-lead spawns agents as needed using the Agent tool. Only spawn agents that are needed for the current task:

| Agent | subagent_type | When to spawn |
|-------|---------------|---------------|
| `kernel-dev` | `kernel-dev` | Code implementation tasks |
| `doc-writer` | `doc-writer` | Phase doc generation |
| `code-reviewer` | `code-reviewer` | Quality validation after implementation |
| `verifier` | `verifier` | QEMU boot testing |
| `doc-auditor` | `doc-auditor` | Documentation quality checks |

Do NOT spawn all agents upfront. Spawn them incrementally as tasks require them.

## Step 5: Assign first task

The team-lead assigns the first available task from the current phase:
- If a phase is in progress → assign the next incomplete step
- If a phase is complete → check if the next phase doc exists
  - If yes → start `/implement-phase` for that phase
  - If no → start `/generate-phase-doc` for that phase

Report to the user: what team was created, which agents were spawned, and what task was assigned.
