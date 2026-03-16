---
name: build-team
description: >
  Bootstraps the AIOS development team for autonomous operation.
  Creates team, spawns specialist agents, assigns initial tasks.
---

# Build AIOS Development Team

1. Create team "aios-dev" using TeamCreate
2. Spawn `team-lead` agent as the orchestrator
3. Team-lead reads CLAUDE.md to understand project state
4. Team-lead searches the knowledge hive for relevant context:
   - Use Obsidian MCP search_notes for lessons and decisions related to current work
   - Review docs/knowledge/plans/ for any in-progress working documents
   - Factor known pitfalls into task assignments
5. Team-lead checks current phase progress (which milestones complete, what's next)
6. Team-lead spawns specialist agents as needed:
   - `kernel-dev` for code implementation
   - `doc-writer` for phase doc generation
   - `code-reviewer` for quality validation
   - `verifier` for QEMU testing
   - `doc-auditor` for documentation quality
7. Team-lead assigns first available task from the current phase
