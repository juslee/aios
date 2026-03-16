# Contributing to AIOS

## Before You Code

Every change lives inside a phase. Read the phase doc (`docs/phases/NN-*.md`) for the relevant phase before writing code. The acceptance criteria in the phase doc are the contract.

## Code Conventions

All conventions are defined in [CLAUDE.md](CLAUDE.md). Key points:

- `#![no_std]` in kernel/ and shared/
- Every `unsafe` block requires a `// SAFETY:` comment documenting the invariant, who maintains it, and what happens if violated
- Naming: `snake_case` functions, `CamelCase` types, `SCREAMING_SNAKE` constants
- No TODO comments — complete implementations only
- Zero warnings from clippy and the compiler

## Commit Style

- Milestone commits: `Phase N MN: <Milestone name>`
- Example: `Phase 0 M1: Compiles — aarch64 ELF with zero warnings`
- Smaller intermediate commits are fine but must also be clippy-clean

## Branching

All work happens on `claude/*` branches. Never commit directly to `main`. All branches merge via PR.

## Architecture Docs

Architecture docs (`docs/kernel/`, `docs/platform/`, etc.) are immutable during phase implementation. If you discover an architecture doc needs changing, that requires a separate PR with justification.

## Testing

- Host-side unit tests for all logic that can be tested without QEMU
- QEMU integration tests for boot path
- Never disable, comment out, or skip a failing test

## Knowledge Hive

The `docs/` directory is an Obsidian vault with shared knowledge in `docs/knowledge/`:

- **decisions/** — Architecture Decision Records (why we chose X over Y)
- **lessons/** — Hard-won lessons (bugs, gotchas, platform quirks)
- **research/** — Deep-dive research notes
- **plans/** — Ephemeral working docs (deleted after distilling)

Naming: `YYYY-MM-DD-initials-short-description.md`. See [docs/knowledge/README.md](docs/knowledge/README.md) for full conventions.

To browse visually (optional): install [Obsidian](https://obsidian.md), open `docs/` as a vault. The `.obsidian/` config folder is gitignored.

## License

All contributions must be BSD-2-Clause compatible. No GPL dependencies in kernel/ or shared/.
