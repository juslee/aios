---
name: verify-phase
description: >
  Runs all quality gates and acceptance criteria for a completed phase.
  Use after phase implementation to validate everything passes.
argument-hint: "[phase-number]"
---

# Verify Phase $ARGUMENTS

Run all Quality Gates from CLAUDE.md:

1. **Compile gate**: `cargo build --target aarch64-unknown-none` — zero warnings
2. **Check gate**: `just check` (fmt + clippy + build) — zero warnings/errors
3. **Test gate**: `just test` (host-side unit tests) — all pass
4. **QEMU gate**: `just run` — match expected UART output from phase doc
5. **Objdump gate**: `cargo objdump -- -h` — sections at expected addresses
6. **EL gate**: boot diagnostics confirm EL=1, core=0

Read `docs/phases/` matching phase $ARGUMENTS and check each milestone's acceptance criteria.

Report per gate: PASS or FAIL with details.
