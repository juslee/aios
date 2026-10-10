---
author: jl + claude
date: 2026-10-10
tags: [tooling, kernel]
status: final
---

# Lesson: Splitting a classifier's classes and adding an interleave mode to the soak harness (crash-fix step 1a)

Crash-fix step 1a changed only the soak harness (`aios soak`, `tools/src/cmd/soak/`): the subclasses WEDGE-STUCK, WEDGE-ALIVE, PANIC-LOCK and DEGRADED, the tripwire columns, the Fisher statistics and the `--arm` interleave mode that replaced `scripts/soak-matrix.sh`. What it delivered is in the [crash-fix ADR](../decisions/2026-09-22-jl-crash-fix-preemption-and-fp.md) ("Step 1a delivered, 2026-10-10" under "Review notes"), the design choices a later step needs in the same ADR ("Step 1a design decisions, 2026-10-10"), and how to use it in [developer guide §5.6](../../project/developer-guide.md). These are the lessons that apply beyond step 1a. The R4 port lessons ([Rust port parity](2026-09-24-jl-rust-port-parity-gotchas.md), [soak port](2026-09-29-jl-soak-port-awk-and-process-parity.md)) cover how the oracle came to exist.

## 1. Keep the oracle proof alive across an intended output change by folding to the old classes

**What happened.** The old classifier (the deleted `scripts/soak-qemu.sh`, read from git history) was the oracle for every classify, CLI and harness test. Step 1a changes its output on purpose, so a straight differential would fail on every split boot, and a normaliser that maps the new names back would exist only to hide intended differences. Instead every new class got a base class, and `base_line()` prints the same fields as `line()` with the base class's name in field 1. The differential then checks that `base_line()` is byte-identical to the oracle's line, and a unit test checks that `line()` and `base_line()` differ in field 1 only. The plan review found that this only holds if nothing else derives from the new class: `detail` took notes, and `lb` was computed from the class (`-` for CLEAN). Both now come from the base class, and the new data (IPC figures, tripwire lines, event counts, re-entry fields) rides outside the line. On run 167 the fold showed exactly the expected 10 WEDGE → WEDGE-STUCK and 2 WEDGE → WEDGE-ALIVE, on B1 (step 1b's single-arm baseline soak, [N2 baseline note](../research/2026-10-10-jl-crash-fix-1b-n2-baseline.md)) PANIC → PANIC-LOCK 3 and WEDGE → WEDGE-ALIVE 3, and over all 251 local logs under `target/soak/` no DEGRADED (the three local `(0 iters)` logs, `pr209-fix-198` runs 01 and 02 and `baseline-212df62-text` run 04, are PCZERO and stay so: the earlier fatal report decides) and no `base_line()` difference.

**Why it matters.** The fold keeps the strongest test there was (an independent implementation, real logs) while the output evolves, and it turns the step's acceptance into one mechanical assertion. The CLI and harness differentials could not be folded the same way: their outputs gained columns and table rows. They were deleted, and their goldens are now blessed from `aios`.

**How to apply.**
- When a refactor changes output on purpose, look for a projection under which old and new must agree exactly (here, the base class), and test through it. Do not write a normaliser.
- Before relying on the projection, list every field that is computed from the thing that changes (here `lb` as well as `detail`), and derive each one from the projected value.
- With the test, print the transition matrix and accept an exact set of changes (`AIOS_SOAK_EXPECT_CHANGES`), so the expected split is asserted, not read by eye from `--nocapture`.
- A test that breaks because of the change goes in the same commit as the change. `cli_differential_against_oracle` was not `#[ignore]`d, so it would have failed in the commit that renamed a class. Before that commit, grep the whole oracle plumbing (fake-environment PATH entries, copied scripts, rename parameters), not only the test functions.

## 2. A rule that tightens a class silently reclassifies under-specified fixtures

**What happened.** DEGRADED made CLEAN require the IPC line's `(10000 iters)`. None of the 22 CLEAN corpus cases had an IPC line, nor did the fake QEMU's clean tail or one `classify-stall-kv` case, so every one would have become DEGRADED. They modelled healthy boots, which always print the line, and each got the kernel's line before `=== Gate 1 Complete ===`. The real-log test also counted each run directory's `build.log` as a boot (62 logs for run 167's 60 boots); it matched `*.log`, and the fix makes it skip `build.log`.

**Why it matters.** Fixtures written for a looser rule carry only what that rule read. A new requirement turns them into cases of a different class, and a re-bless would have recorded that as correct.

**How to apply.** When a change adds a condition to a class, check every fixture of that class for the new evidence before re-blessing, and add the evidence where the fixture models a real boot. When a test globs the harness's output, check what else the directory holds; matching the harness's own file names (`run-*.log`) would be stricter than step 1a's fix, which still matches `*.log` and skips only `build.log`.

## 3. Re-blessing goldens can hide a weakened comparison

**What happened.** Adding `--classify --out` made the CLI test strip the case directory from each CLI golden blob through `String::from_utf8_lossy`. The two `clip-multibyte` lines were re-blessed with U+FFFD where `aios` prints a raw clipped `\xC3` byte. `aios` was unchanged; the test helper had weakened the comparison, and a `--word-diff` review of the re-bless missed it. Two test normalisers broke the same quiet way:
- the `summary.md` stall-cell normaliser matched the class with `[A-Z]+`, so a hyphenated class (PANIC-LOCK, WEDGE-*) kept a host-dependent cell;
- the per-boot-table normaliser also matched the pair report's non-CLEAN rows and masked their tick.

A third, the `summary.tsv` normaliser keyed on the literal 22 columns, was caught at plan review and changed to key on the header's column count before it could break.

**Why it matters.** A golden is only as strong as the transformation between the program's bytes and the file. A helper that rewrites bytes, or a normaliser whose pattern matches more or less than intended, changes what the test checks without failing it.

**How to apply.**
- Compare and rewrite goldens as bytes, never through a lossy string conversion, and pin non-UTF-8 pass-through with a test.
- After a re-bless that should only add output, check that the diff against the pre-change tree removes no line (`git diff <before> -- <golden dir>` shows additions only).
- Key normalisers on header names or counts, never on a literal width, and when a new output shape lands (a hyphenated name, a new table), check each normaliser against it.

## 4. Host-dependent inputs need a test hook that release builds never read

**What happened.** The interleave preflight refuses a host whose load1 is above its CPU count and an arm without 7167d40. On this Mac (load1 40–90 against 10 CPUs while the pair report was built) every fake-QEMU scenario would have been refused, CI's load would have varied the goldens, and the fake repository's commits cannot contain 7167d40. A fake `sysctl` on `PATH` cannot pin the load, because Linux reads `/proc/loadavg` directly. Both values got an environment override (`AIOS_SOAK_LOADAVG`, `AIOS_SOAK_MIN_ARM_BASE`) read only in debug-assertion builds (the profile of `cargo test` and CI's Tools job), with a stderr warning and a row in the report whenever one is in effect.

**Why it matters.** A safety check that tests cannot reach is either untested or bypassable. Gating the override on `debug_assertions` keeps it out of `just tools`, so out of `just soak` and CI's soak jobs, and the report row means no result can hide that it used one.

**How to apply.** For a check that reads host state, prefer passing the value as a parameter (the arm base is a preflight parameter, so unit tests pass their own). Where the binary itself must be pinned in an end-to-end test, read the override under `cfg(debug_assertions)` only, warn, and record it in the output. Never let an override reach a release build.

## 5. A signal reaches the probe as well as the supervisor

**What happened.** Interleave mode rechecks the QEMU binary and firmware (`sha256sum`, `qemu --version`) after every boot, and a change stops the soak without counting that boot. A terminal Ctrl-C reaches the probe's child too, so a Ctrl-C during the probe failed it, and the soak wrote "stopped (the QEMU binary changed)" and dropped a valid boot. Now a probe that fails while a signal is pending is the signal, and a probe that ran and disagrees is still a change.

**How to apply.** When a supervisor treats a child's failure as evidence (a change, a fault), check for a pending signal first: the terminal's process-group signal hits every child at once. Test it with a fake probe that interrupts itself.

## 6. The audit loop hardened provenance until each fix became the next round's finding

**What happened.** Implementation ran its ten code and docs tasks with one Opus implementer per task, with a Fable correctness reviewer and a Sonnet gate reviewer; every task was approved within 0–2 fix rounds. The audit loop (diverse read-only finders, three-skeptic verification, a single fixer) then ran its 8-round cap without two consecutive clean rounds. Confirmed findings per round: 16, 12, 6, 5, 8, 4, 7, 5, all fixed. Rounds 1–3 found ordinary defects (a status left at `running` after an error, symlink and HEAD pins, the `ctx=` parse of a torn line). From round 5 on, about half of the findings (most of rounds 6 and 8, a minority of round 7) were defects in the provenance hardening that round 4 and later rounds had added to the Harness row of the pair report; the rest were docs and help-text drift. The provenance findings were:
- its `-dirty` test (round 4 added a host-tools-inputs test beside the whole-tree `-dirty` mark the interleave mode first shipped with; round 6 dropped the whole-tree mark, since an arm's kernel edits had marked the classifier dirty);
- git replace refs and grafts that could fake both that test and the arm-base ancestry check (round 5);
- a stale-binary note (round 6) that round 7 rewrote to match the shim's stamp verdicts, including an edit reverted after the build;
- a TOCTOU between reading HEAD for the row and for the stamp check, and a `just tools` swapping the binary mid-soak (round 8: one pinned commit, a hard link made for the soak);
- a third copy of the tools build-input list (`host.rs` `TOOLS_INPUTS`, beside the justfile recipe and the `.claude/hooks/aios` shim), which round 8 tied to the other two with a drift test.

The lead stopped at the cap, with the owner's pre-approval of recommended choices: each fix added surface the next round audited. Two findings needed `.claude/` edits the fixer cannot make (the `justin:brief` Red bullet naming the interleaved soak entry as an experiment, and the shim header naming the third copy); the lead applied them (478d449).

**Why it matters.** A row that says which binary classified a soak is a provenance claim, and every strengthening of it (dirty test, stamp test, pinned HEAD, hard link) re-implements part of the shim's freshness logic. Each copy is new surface, so finders keep finding gaps between it and the original. The convergence signal was in the counts: the findings stopped falling after round 4 (16, 12, 6, 5, then 8, 4, 7, 5), and about half of them moved into code the loop itself wrote. The [path-guard lesson](2026-10-06-jl-path-guard-spec-correct-bypasses.md) is the opposite failure, verifiers rejecting real bypasses as design creep; both need the lead to judge whether a finding belongs to the step's requirement.

**How to apply.**
- Track where each round's findings land. When most of them fall in text or code an earlier round of the same loop added, stop and descope: record the remaining requirement (here, "the report names the classifier's commit and says when it may not match the binary") and take further hardening to an issue, not to round N + 1.
- Reuse a single implementation of a provenance test (the shim's stamp check) rather than porting it, or bound the claim the report makes so it needs no port.
- Every list that must match another list gets a drift test in the commit that creates the copy, and the comments that name the copies are updated with it.
- Plan the `.claude/` edits a step will need as lead-applied batches before the tasks that depend on them; a fixer that finds one mid-loop can only report it.

## 7. A dispatch on the same ref cancelled the push run's soak job

**What happened.** To check the new `runs` input, CI was dispatched (`workflow_dispatch`) on the branch with `runs=2`. The dispatch shared the `qemu-soak` job's concurrency group (job-level, keyed on the ref), so it cancelled the push run's `qemu-soak` job. Audit round 1 gave each dispatch soak a concurrency group of its own (developer guide §5.6 states the current behaviour).

**How to apply.** Before adding a manual input to a workflow whose jobs have a `concurrency` group keyed on the ref, decide whether a dispatch should cancel, be cancelled by, or run beside the push run, and key the group on `github.event_name` (or the run id) accordingly. Check the push run's jobs still finish after the first dispatch.

## 8. A soak arm named in a plan goes stale when `main` bumps the toolchain

**What happened.** The working plan's pre-PR smoke soak named af59149 as arm A against the branch. After `main`'s toolchain bump (a9422c9, nightly-2026-10-10) merged into the branch, af59149 pinned an older nightly, and the harness refuses arms with different channels. The lead used `main` ac6d26e as arm A instead. That is the same kernel source as the branch (no `kernel/` or `shared/` change; the built ELFs differ, `4560846c` against `40405da0`), so the smoke soak was an A/A of the kernel that exercised the harness, not a comparison. Host load1 was 28 before the builds and 31 after them, on 10 CPUs (per-boot means 20.5 for A and 14.9 for B), so it ran with `--ignore-load`, and the pair report flagged the arms' load means 38% apart ("redo the pair"). Result: 4 boots in ABBA order, status `finished`, exit 0; A gave PCZERO and CLEAN, B CLEAN and PANIC-LOCK (`ctx=irq-exit`); the regression guard passed (p 0.833). Arm B, the worktree under `.claude/worktrees/` inside the main checkout, drew the harness's parent `.cargo/config.toml` warning, as designed.

**Why it matters.** A pair is only valid when its arms share a channel, and a commit's channel is fixed while the branch's moves with every merge of `main`. Step 1b's own A/B (af59149 against e211d6d) is unaffected: both pin nightly-2026-10-09, and the harness's channel does not matter, since each arm builds with its own.

**How to apply.**
- Before a soak, re-read each arm's `rust-toolchain.toml`; when the base moved, pick a base on the same channel, or pass `--allow-mixed-toolchains` only for a pair whose point is the toolchain change.
- Put arm worktrees outside any checkout (`git worktree add --detach ../aios-<sha> <sha>`), so no parent cargo config joins their build.
- On this Mac the load rule will often refuse a soak. Find what drives the load (during step 1a it was other sessions' builds and tests; the [N2 baseline note](../research/2026-10-10-jl-crash-fix-1b-n2-baseline.md) records `fileproviderd` still running during B1) before overriding it, and treat a pair the report says to redo as a harness check only.

## 9. Smaller gotchas

- `rg` without `--hidden` skips `.github/` and `.claude/`, so a "no references left" check misses workflows and `.claude/CLAUDE.md`. Use `rg --hidden -g '!.git' -g '!target'`.
- `actionlint` flags `runs-on: ubuntu-26.04` as an unknown label: its runner list predates the image. Ignore that one message.
- `proc::tests::suspend_stops_the_group_and_moves_the_limit_back` (a 500 ms limit under a 1 s suspend) fails now and then at load1 near 40, mostly in parallel runs; rerun it alone before suspecting a change.
- The #240 experiment (the echo IPC pair runs when its server is queued on CPU 1) is recorded in the ADR's step 1a note. It supports #240's diagnosis (part of it is #200's missing timer IRQs on CPUs 1-3); #200's step is unchanged by it.
