---
name: verify-phase
description: >
  Runs all quality gates and acceptance criteria for a completed phase.
  Use after phase implementation to validate everything passes.
argument-hint: "[phase-number]"
---

# Verify Phase $ARGUMENTS

Run all Quality Gates from `.claude/rules/02-quality-gates.md`. Stop at the first FAIL and report — do not continue to subsequent gates.

## Step 1: Find and read the phase doc

Use the Glob tool to find the phase doc:

```
docs/phases/$ARGUMENTS-*.md
docs/phases/0$ARGUMENTS-*.md
```

Read the phase doc fully. Extract the acceptance criteria for each milestone — these are your verification targets.

## Step 2: Run quality gates in order

Run each gate sequentially. For each gate, report PASS or FAIL with the command output.

Gates 1–3 run in the worktree. A lead in the main checkout gets them from the writer's latest gate report at the head sha, or spawns a writer with "run the gates and report".

### Gate 1: Compile

```bash
cargo build --target aarch64-unknown-none 2>&1
```

**PASS condition:** Exit code 0 AND zero warnings in the output. Use Grep to check for `warning:` in the output.

### Gate 2: Check (fmt + clippy + build)

```bash
just check 2>&1
```

**PASS condition:** Exit code 0 AND zero warnings, zero errors.

### Gate 3: Test (host-side unit tests)

```bash
just test 2>&1
```

**PASS condition:** Exit code 0 AND output contains `test result: ok` with zero failures. Grep for `test result:` to verify.

### Gate 4: QEMU boot

Spawn the verifier (rule 11: `isolation: "worktree"`; the worktree path, the sha to boot (its tip) and the team in the prompt) in mode `boot`: one text boot, plus one gpu boot when the phase needs the display, with the phase's UART lines as acceptance. Its run directories land in `<W>/target/soak/`; remove its temporary worktree afterwards (`/justin:team`, Placement).

**PASS condition:** the verifier reports `PASS`, with every expected UART line of the phase doc's acceptance criteria matched. `DEFERRED` means the host was loaded: re-run later.

### Gate 5: Objdump (section addresses)

The verifier runs the sysroot `llvm-objdump -h` command from rule 02 in the same spawn:

```bash
"$(rustc --print sysroot)"/lib/rustlib/*/bin/llvm-objdump -h target/aarch64-unknown-none/debug/kernel
```

**PASS condition:** `.text.boot` is at VMA `0xffff000000080000` and LMA `0x40080000` (Key Technical Facts in `.claude/CLAUDE.md`).

### Gate 6: EL verification

**PASS condition:** the EL and core lines in the boot log in the verifier's run directory (from Gate 4) contain `EL: 1` or equivalent EL1 confirmation, and `core: 0` or `CPU 0` in early boot messages.

## Step 3: Per-milestone acceptance

After all gates pass, read each milestone's specific acceptance criteria from the phase doc. Verify each one was met by the Gate 4 QEMU output or Gate 3 test output.

## Step 4: Report

Print a summary table:

```
| Gate      | Status | Details              |
|-----------|--------|----------------------|
| Compile   | PASS   |                      |
| Check     | PASS   |                      |
| Test      | PASS   | N tests, 0 failures  |
| QEMU      | PASS   | All criteria matched |
| Objdump   | PASS   | .text @ 0xFFFF...    |
| EL        | PASS   | EL=1, core=0         |
```

If ANY gate failed, clearly state which gate failed and include the error output.
