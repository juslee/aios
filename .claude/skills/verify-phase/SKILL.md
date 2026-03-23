---
name: verify-phase
description: >
  Runs all quality gates and acceptance criteria for a completed phase.
  Use after phase implementation to validate everything passes.
argument-hint: "[phase-number]"
---

# Verify Phase $ARGUMENTS

Run all Quality Gates from CLAUDE.md. Stop at the first FAIL and report — do not continue to subsequent gates.

## Step 1: Find and read the phase doc

Use the Glob tool to find the phase doc:

```
docs/phases/$ARGUMENTS-*.md OR docs/phases/0$ARGUMENTS-*.md
```

Read the phase doc fully. Extract the acceptance criteria for each milestone — these are your verification targets.

## Step 2: Run quality gates in order

Run each gate sequentially. For each gate, report PASS or FAIL with the command output.

### Gate 1: Compile

```bash
cargo build --target aarch64-unknown-none 2>&1
```

**PASS condition:** Exit code 0 AND zero warnings in stderr. Use Grep to check for `warning:` in the output.

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

```bash
just run 2>&1
```

**PASS condition:** Compare UART output against the phase doc's acceptance criteria. For each milestone, check that the expected strings appear in the output. Use Grep on the captured output to verify each expected line.

**Timeout:** QEMU runs should complete within 30 seconds. If `just run` hangs, kill it and report FAIL.

### Gate 5: Objdump (section addresses)

```bash
cargo objdump -- -h 2>&1
```

**PASS condition:** `.text` section starts at the expected kernel VMA. Check against CLAUDE.md Key Technical Facts.

### Gate 6: EL verification

**PASS condition:** QEMU boot output (from Gate 4) contains `EL: 1` or equivalent EL1 confirmation, and `core: 0` or `CPU 0` in early boot messages.

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
