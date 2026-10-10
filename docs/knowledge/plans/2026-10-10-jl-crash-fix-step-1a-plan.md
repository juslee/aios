---
author: jl + claude
date: 2026-10-10
tags: [tooling, kernel]
status: in-progress
phase: crash-fix
milestone: step-1a
---

# Plan: Crash fix step 1a — soak classes, tripwire columns, interleave mode

## Approach

Step 1a of the [boot-crash fix ADR](../decisions/2026-09-22-jl-crash-fix-preemption-and-fp.md) is harness only: no kernel change. It makes the soak harness measure what every later step's acceptance is stated in. Step 1b (merged as e211d6d, #238) is waiting for it: its interleaved A/B acceptance soak needs the subclasses, the tripwire columns and the interleave mode, and until they exist the ADR's 2026-10-06 note makes every pair classify by hand.

The harness is the Rust `aios soak` from R4 (#230, `tools/src/cmd/soak/`), so everything lands there and in its tests (`tools/tests/soak_*`). The owner approved starting step 1a on 2026-10-10 at 10:57. Branch `claude/crash-fix-step-1a-soak-classes`, from `main` e211d6d.

**What exists today.**

- `classify.rs`: a byte-exact port of the deleted `scripts/soak-qemu.sh` classifier (oracle: its blob at `212df62`). Six classes: PCZERO, PANIC, EXCEPTION, WEDGE, INCONCLUSIVE, CLEAN. The WEDGE subtype and the panic message are only in the free-text `detail` and `first_fatal` fields.
- `report.rs`: the console line, `--classify` output, `summary.tsv` (22 columns), `summary.md` (class table, Wilson interval, per-boot table, per-boot load mean and max).
- `runner.rs`: one ESP image per invocation, booted `runs` times in a row under `proc::Supervisor`; no external `timeout`.
- Tests: classifier, CLI and harness goldens recorded from the oracle, differential tests that run the oracle from git history, and a fake QEMU (`tools/tests/common/soak_fake.rs`).
- `scripts/soak-matrix.sh` (1,344 lines, bash) and `.github/workflows/soak-matrix.yml`: interleaved 2–4-arm soaks. Each arm is booted and classified by its own harness, so a pre-1a arm reports the old six classes. The script already has the version checks, rotation, pairwise Fisher (CLEAN vs not CLEAN only) and per-arm load mean and max.

**Facts checked for this plan (2026-10-10, read-only, no boots).**

- Run 167 (`target/soak/167`, 12 directories of 5 boots, 60 boots): today's `aios soak --classify`, built from e211d6d, reproduces all 60 classes recorded in its `summary.tsv` files. All 25 CLEAN boots print `(10000 iters)` on the IPC line. All 9 PANICs are `frame.rs:51` or `channel.rs:249`, none `lock re-entry:`.
- B1 (`target/soak/pr238-20261010-094339-b1-{text,gpu}`, 30 boots): today's classifier reproduces every recorded class. All 12 CLEAN boots print `(10000 iters)`. The 3 text WEDGEs read "heartbeat alive but the Gate 1 bench never completed". The 3 text PANICs are `lock re-entry:` (runs 05, 07, 11).
- A throwaway awk prototype of the §6.5 parser contract ([observability.md](../../kernel/observability.md) §6.5: the last line whose token count equals its `n`, a missing key is 0) reproduced all 28 non-class columns of the [B1 research note](../research/2026-10-10-jl-crash-fix-1b-n2-baseline.md)'s per-boot TSV for all 30 rows, with an empty diff. The contract is enough; no heuristics are needed.
- The `[tripwire-ev]` lines in B1: 2 `kind=stuck` lines, no `kind=self`.
- `kernel/src/bench.rs:26`: `const IPC_ITERATIONS: usize = 10_000;`. The IPC line prints `avg_us()`, which is `avg_ns() / 1000` in integer arithmetic (`bench.rs:90`), so `avg=` is a whole number of µs, truncated.
- `shared/src/tripwire.rs`: `PREFIX_TOKENS = 5` (`v src cpu t ncpu`) and `Key::COUNT = 61`; `GOLDEN_FULL` ends `n=66` (5 + 61), `GOLDEN_NONZERO` `n=14` (5 + 9).
- Both arms of step 1b's A/B soak (af59149, e211d6d) pin `nightly-2026-10-09`, and both contain 7167d40 (#196). Between them, the justfile and the soak code differ only in a 5-line change to `classify.rs`.
- `just docs-check` on the branch before this plan: no new drift.

**Facts checked for the review revision (2026-10-10, read-only).**

- `classify.rs` `finish()` pushes every note into `detail`, and computes `lb` from the class (`"-"` for CLEAN and INCONCLUSIVE). A DEGRADED note in `detail`, or an `lb` computed from the new class, would break the fold differential (M1).
- `soak_cli.rs` `cli_differential_against_oracle` is not `#[ignore]`d, so it runs in every `cargo test -p aios-tools`: it breaks in the commit that renames a class and has to go in that commit (M2). The other oracle users: `record_classify_goldens_from_oracle` (`soak_classify.rs:73`), `record_cli_goldens_from_oracle` (`soak_cli.rs:184`), `record_harness_goldens_from_oracle` and `harness_differential_against_oracle` (`soak_harness.rs:121`, `:136`), `run_oracle` and `rename_prefix` (`common/soak.rs:124`, `:139`), and in `common/soak_fake.rs`: `Tool::Oracle`, `ambient_timeout`, the `oracle-bin` PATH entry, `PRE_QEMU_TOOLS`/`pre_qemu_tools` (the oracle's pre-QEMU tools), the copy of the oracle into each fake repository (`:484`) and `golden_text`'s `rename` parameter. `oracle_script()` stays: `oracle_awk()` reads it for the classifier differential.
- `host::git_rev` uses `git status --porcelain --untracked-files=no`, so dropping the untracked oracle copy from the fake repositories changes no `-dirty` in any golden.
- The aios binary's build inputs are listed in exactly two places that must agree: `justfile:221` (`tools` recipe) and `.claude/hooks/aios:198`, each with matching OS/editor-file exclude pathspecs (`justfile:228–231`, shim `:224–227`) and a comment that names the inputs (justfile recipe comment, shim header `:9–11`). `tools/tests/shim.rs` builds a sandbox from the real justfile and shim and lists the inputs in its stale tests (`:254`, `:502`, `:1257`).
- `.claude/CLAUDE.md:243` names `soak-matrix.sh`; without `--hidden`, `rg` skips `.github/` and `.claude/`, so a plain `rg 'soak-matrix\.sh'` misses both workflow and CLAUDE.md hits.
- `scripts/agent/brief.sh` (`:383`) treats every directory under `target/soak/` that holds a `summary.md` or `summary.tsv` as a run, and picks the newest finished run of a clean `origin/main` commit as "main soak".
- A QEMU spawn failure in `runner.rs` writes a note to the log and records status 127; such a log has no `AIOS UEFI stub` line, so the "stub never ran" test covers it.
- `shared/` has documented `unsafe` (`lock.rs`, `input.rs`, `storage.rs`); `tools/` keeps `#![forbid(unsafe_code)]`, which covers only its own code.

-----

## Scope: every ADR requirement, and where it lands

Sources: "Order and why" row 1a; "Soak protocol (every soaked step)"; "Step 1a — harness only"; "Rules for every step PR"; the "Pending acceptance soak" part of "Step 1b decisions, 2026-10-10"; and the 2026-10-09 errata ("PANIC-LOCK is two lines", "Re-entry is decided in every context", "The H1 rule cannot be applied as written", "`scripts/soak-qemu.sh` is gone").

| # | Requirement (ADR wording, shortened) | Where it lands | Task |
|---|---|---|---|
| R1 | WEDGE-STUCK: the heartbeat is stuck or stopped | `classify.rs`: a `Class` enum. Mapped from today's WEDGE branches: "no heartbeat …" (all three variants), "heartbeat stuck at tick 0 …", "heartbeat never advanced past tick 0" and "heartbeat stopped at tick N …" become WEDGE-STUCK | T3 |
| R2 | WEDGE-ALIVE: the heartbeat is alive, the bench incomplete | Same place: "heartbeat alive but the Gate 1 bench never completed/started". "gpu markers missing" is counted as WEDGE-ALIVE, as the 2026-10-06 note rules; its detail keeps the text, so the PR can name it | T3 |
| R3 | DEGRADED: `=== Gate 1 Complete ===` printed, but the IPC line reports fewer than `(10000 iters)`; CLEAN then requires 10000 | `classify.rs`: parse the first `[bench] IPC round-trip (same core): avg=… (N iters)` line. The threshold is a constant, with a drift test against `kernel/src/bench.rs` `IPC_ITERATIONS`. An unreadable count is DEGRADED (Q5 A). The reason is visible only through `ipc_iters` (`-` when unreadable) and the report cells built from it, never in `detail` (D1) | T4 |
| R4 | PANIC-LOCK: a PANIC whose joined `first_fatal` contains `lock re-entry:` (the erratum makes it two lines) | `classify.rs`: refines PANIC after the scan, with the same earliest-report rule. The `ctx=`, `holder_irqs=` and lock name are parsed from the joined text into columns (Q3) | T3 |
| R5 | Tripwire line: the last value of each key in `summary.tsv` columns, reported per arm. Parser contract: the last complete line, `src=panic`/`exc` allowed; the whole line stored beside the columns; the `full_line_golden` Full line is the fixture | New `tools/src/cmd/soak/tripwire.rs` (parser and columns), used by the `classify.rs` scan. Columns in `report.rs`, including `tw_v` (the schema version) and the whole lines `g1_line` and `tw_line`. The fixture is the `GOLDEN_FULL` literal, extracted from `shared/src/tripwire.rs` by the test at run time, the way `oracle_awk()` extracts the awk heredoc (no copy) | T5, T6 |
| R6 | Fisher one-sided tests in the report: the regression guard (CLEAN lower in the new arm, p < 0.05), "class removed" (0 in the new arm and p < 0.05), raw differences recorded but never gated on | New `tools/src/cmd/soak/stats.rs` (hypergeometric with log-factorials, no new crate), plus the pair report in `pair.rs` (with a `new` marker beside `removed`) | T7, T9 |
| R7 | Per-arm load mean and max; redo the pair if the arm means differ by more than 25%; do not start while load1 is above the CPU count | `pair.rs` (report and verdict); the start check in the interleave runner, taken before the arm builds, with the post-build value recorded too (Q6) | T9 |
| R8 | Interleave mode: alternate two ESP images per boot in one host session | New `tools/src/cmd/soak/interleave.rs`, on top of a `runner.rs` refactor (`boot_once`, `Arm`). CLI: `--arm DIR`, repeatable (Q1, Q8). Report-only by default; the top-level `summary.md` is rewritten after every boot with a status; 3 harness-error boots in a row stop the soak | T2, T8 |
| R9 | Refuse a pair whose arms differ in QEMU version, firmware or toolchain channel, plus compiler (the 2026-10-06 note adds `rustc --version`) | `interleave.rs` preflight. QEMU: one binary for all arms, with its version line and sha256 rechecked before every boot (a mid-soak change stops the soak). Firmware: each arm's `just --evaluate edk2_fw` must name one absolute path, and its sha256 is rechecked per boot. Channel: each arm's `rust-toolchain.toml`. Compiler: `rustc --version` in each arm after its build. `rustup` is required. `--allow-mixed-toolchains` overrides the last two and marks the summary (soak-matrix behaviour, kept for toolchain-bump pairs such as #161's) | T8 |
| R10 | A `timeout` check that accepts ubuntu-26.04's `timeout` | Already delivered; no code (see "The `timeout` item" below) | — |
| R11 | A `runs` input for the CI `workflow_dispatch` (`ci.yml:8` has none), so one commit can be soaked several times | `.github/workflows/ci.yml` `qemu-soak` job: input passed through `env`, never `${{ }}` inside a script, defaulting to 5 on push and pull_request (`RUNS=${RUNS:-5}`); step time limit computed from it; job limit covers the maximum (Q7) | T10 |
| R12 | Each step PR passes rule 02's gates and `/audit-loop`, and updates the docs it makes stale | T11, T12 | T11, T12 |
| R13 | Acceptance: `--classify` on the run-167 logs reproduces every boot's class except the WEDGE split and CLEAN→DEGRADED; no boots needed | The fold differential (T3, extended to DEGRADED in T4) and the real-log check below | T3, T4, T12 |
| R14 (1b pending soak) | Step 1b's A/B: interleaved 20 text and 10 gpu boots per arm, 1b against af59149, one QEMU, one firmware and one toolchain; H3 per the restated rule; PANIC-LOCK as in R4; per-boot last complete line plus the whole line | Delivered by R1–R9. The H3 rule needs PANIC-LOCK `ctx`/`holder_irqs` and `[tripwire-ev] kind=self` counts by `ctx` (Q3). H1 waits for the owner's restatement, and both candidates ("only mismatches before the `src=g1` line" and "`elrmm` > 1, leaving out boots whose first fatal report is a PANIC") need the `src=g1` line's values, so that line is stored whole (`g1_line`) beside `g1_elrmm` (Q3, review S5) | T5, T6 |
| R15 | The step-4 Gate 1 comparison: the mean of per-boot IPC averages over each arm's CLEAN boots, per mode | `ipc_avg_us` column (T4; integer µs, see "Facts checked"); per-arm mean in the pair report (T9) | T4, T9 |
| R16 | Step 1b's "every non-CLEAN boot has a tripwire line or fatal dump" | `tw_src` column; the pair report counts non-CLEAN boots with neither, labelled as meaningful only for arms that print tripwire lines | T6, T9 |

**The `timeout` item (R10) is moot.**

- #192 (aa1f128, merged 2026-09-24) made `soak-qemu.sh` accept uutils `timeout`. The ADR's #169 bullet under "Other PRs" already says it "delivers step 1a's `timeout` item".
- R4 (#230, 1a5c363) deleted `soak-qemu.sh`. `aios soak` stops QEMU itself (`tools/src/proc.rs` `Supervisor`: a process group, SIGTERM at the limit, SIGKILL 10 s later, statuses 124 and 137). Nothing under `tools/src` runs `timeout`, and developer-guide §5.6 says "No external `timeout` is needed".
- CI run 38022214523 (main e211d6d, 2026-10-10): the `QEMU boot soak (report-only)` job ran on `ubuntu-26.04`, and its "Soak (5 boots x 90 s)" step succeeded. A setup failure turns that step red, as on #169.
- `timeout` has two users left: `scripts/soak-matrix.sh` `find_timeout` (only for pre-R4 `soak-qemu.sh` arms) and the oracle harness recordings (`soak_fake.rs` symlinks the ambient `timeout`). Both go (Q1 A in T10, Q4 A in T3). Step 1a records the item as delivered and changes no code for it.

**Non-goals.**

- No kernel or `shared/` change.
- No decision rule for H1; the owner restates it. 1a only stores what either candidate needs.
- No re-report of a finished interleaved run from its logs, beyond `--classify --out` for one group of logs (T6). (The per-boot rewrite of the top-level `summary.md` from `boots.tsv` during a run, S3, is not a re-report tool: it runs only inside the soak.)
- No `[smp]` line parsing.

-----

## Acceptance

### A1. Run 167 (the ADR's step-1a acceptance)

The mechanism is a fold. Every new class has a base class: WEDGE-STUCK and WEDGE-ALIVE fold to WEDGE, PANIC-LOCK to PANIC, DEGRADED to CLEAN, and every other class to itself. The acceptance then reads: for every log, `base_line()` (D1) is byte-identical to the oracle's line. Since `base_line()` differs from `line()` only in field 1, that says `fold(new class)` equals the oracle's class and every other field, `detail` included, is unchanged. No normaliser is involved. T3 turns the existing `classify_differential_on_real_logs` (ignored, `AIOS_SOAK_REAL_LOGS`) into that check. It also prints a transition matrix (oracle class → new class, with counts) and the per-boot list of every boot whose class changed.

```sh
AIOS_SOAK_REAL_LOGS=/Users/juslee/dev/aios/target/soak/167 \
  cargo test -p aios-tools --test soak_classify -- --ignored classify_differential_on_real_logs --nocapture
```

Expected:

- WEDGE → WEDGE-STUCK: 10. main text r1/01, r1/02, r2/04, r3/02 and r4/04; main gpu r2/04; #161 text r1/01, r1/05, r2/02 and r3/01.
- WEDGE → WEDGE-ALIVE: 2. main gpu r1/05 and #161 text r4/02.
- CLEAN → DEGRADED: 0 (from T4 on). Every CLEAN boot prints `(10000 iters)`.
- PANIC → PANIC-LOCK: 0.
- Every other boot keeps its class.

That matches the ADR's "Soak evidence" table (main text 5/0, main gpu 1/1, #161 text 4/1). The test fails on any other transition or any `base_line()` difference. The PR body carries the matrix and the 12-row list from `--nocapture`.

The transition matrix also needs the recorded classes. A second check, independent of the oracle, compares each log's new class with the `class` column of the `summary.tsv` beside it, under the same fold. It runs as part of the same ignored test for any directory that has a `summary.tsv`.

### A2. B1 logs (step 1b's single-arm baseline)

```sh
cargo run -q -p aios-tools -- soak --classify --out "$S/b1-text" \
  /Users/juslee/dev/aios/target/soak/pr238-20261010-094339-b1-text/run-*.log
```

The same for gpu. Expected classes:

- text: CLEAN 11, WEDGE-ALIVE 3 (runs 12, 13, 20), PCZERO 1, EXCEPTION 2, PANIC-LOCK 3 (runs 05, 07, 11; `ctx=irq-exit`).
- gpu: CLEAN 1, EXCEPTION 9.
- DEGRADED: 0.

The tripwire columns must reproduce the research note's per-boot TSV. A projection of `summary.tsv` to that TSV's 28 non-class columns gives sums for the per-CPU keys `elrmm` and `irqsw`; index 1 of `ubrun`, `ubrbl` and `starved` (`reply`, Normal); the four `n2` kinds; and indices THREAD_TABLE 0, CURRENT_THREAD 1 and WAKEUP_ERRORS 3 of `lkph` and `lkself`. Diffed against the TSV block extracted from the note (with its class column folded), it must be empty. The projection is a short awk kept in the PR body, not committed. The prototype of this check already ran with an empty diff on raw logs (see "Facts checked").

The per-class tripwire table in the new `summary.md` must also match the note's "Counters by class" table, with WEDGE read as WEDGE-ALIVE and PANIC as PANIC-LOCK. That comparison is by eye, cell by cell, and the PR records it.

### A3. Gates (rule 02, tools crate)

Every task's commit (T1–T12, each one on its own, not only the last) passes:

- `cargo fmt --check -p aios-tools`
- `cargo clippy -p aios-tools --all-targets -- -D warnings`
- `cargo test -p aios-tools` (debug profile, as CI's Tools job runs it)
- `just check`
- `just test` (unchanged; `shared/` is not touched)
- `cargo run -q -p aios-tools -- docs-check`: no new drift except this plan's own knowledge-hygiene finding (a non-empty `plans/`), which goes when the plan is distilled

So a commit never leaves a test that the next commit fixes: whatever a change breaks (a golden, an oracle differential, a corpus count) is fixed or deleted in that same commit. The `.claude/hooks/aios` shim runs the main checkout's build, so on the branch docs-check always runs through `cargo run`. `cargo objdump` and `just run` do not apply: no kernel change. Before the PR, one attended smoke soak (T12) exercises the interleave runner on real QEMU.

-----

## Design decisions

**D1. Class model.** `Classification::class` becomes `Class`, an enum. T3 adds `PcZero`, `PanicLock`, `Panic`, `Exception`, `WedgeStuck`, `WedgeAlive`, `Inconclusive`, `Clean`; T4 adds `Degraded`, in the commit that can produce it and carries its corpus cases (M2).

- `Class::ALL` is the summary order, with subclasses next to their base: PCZERO, PANIC-LOCK, PANIC, EXCEPTION, WEDGE-STUCK, WEDGE-ALIVE, INCONCLUSIVE, DEGRADED (from T4), CLEAN.
- `name()` is the printed name, and `base()` the oracle class.
- Precedence is unchanged; the subclasses only refine within a base class.
- DEGRADED is decided last, at the point where today's code returns CLEAN. So it can never mask a fatal report, a WEDGE or an INCONCLUSIVE.
- **`line()` and `base_line()`.** `line()` keeps its 11 tab-joined fields, with `class.name()` in field 1. `base_line()` is the same 11 fields with `class.base().name()` in field 1. Fields 2–11 are one computation shared by both, and it is the oracle's:
  - no refinement adds, removes or rewords a note in `detail`: the WEDGE-STUCK and WEDGE-ALIVE notes are the oracle's texts, a PANIC-LOCK has the oracle's PANIC notes, and a DEGRADED boot has exactly the notes the oracle gives that boot as CLEAN (for example the "log-only" note);
  - `lb` tests the base class, so a DEGRADED boot gets `-` as a CLEAN one does, and WEDGE-STUCK, WEDGE-ALIVE and PANIC-LOCK get the oracle's `yes`/`no`;
  - `first` and `info` are the scan's, untouched.
  So `base_line()` is byte-identical to the oracle's line, which the fold differential checks, and a unit test asserts that `line()` and `base_line()` differ in field 1 only, for every corpus case.
- The new data rides outside `line()`: `ipc` (avg in µs and iterations, each `None` when unreadable), `tripwire` (the last complete line), `g1` (the last complete `src=g1` line), `events` (the `[tripwire-ev]` counts) and `reentry` (`ctx`, `holder_irqs` and the lock, for PANIC-LOCK).
- **Why a boot is DEGRADED** shows only through `ipc`: the `ipc_iters` column (the count, or `-` when unreadable) and the report cells built from it. Where the console line and the per-boot table print `detail` for CLEAN, they print `IPC <N> iters` or `IPC iteration count unreadable` for DEGRADED, from `ipc`, so `detail` stays the oracle's.
- The enum replaces the stringly `&'static str` class, so every `match` on a class is exhaustive. That costs nothing, since the oracle parity now goes through `base()`.

**D2. Tripwire parsing.** One pass, inside the existing `Scan::line`.

- A line contributes from its first `[tripwire] ` (a line printed after another CPU's partial output starts mid-line; `[tripwire-ev]` never matches).
- It is complete when every token after `[tripwire]` is `key=value`, the last is `n=N`, and N equals the number of `key=value` tokens before `n=`. That count includes the 5 prefix tokens (`v`, `src`, `cpu`, `t`, `ncpu`) and excludes `[tripwire]` itself and `n=`: `GOLDEN_FULL` has `n=66` = 5 + 61 keys, `GOLDEN_NONZERO` `n=14` = 5 + 9. A test pins both counts and the rule.
- The last complete line wins, whatever its version, and so does the last complete `src=g1` line.
- A `v=1` line is decoded: the prefix into `tw_src`, `tw_cpu`, `tw_t` and `tw_ncpu`, the keys into `tw_<key>`.
- Any other schema version is kept whole in `tw_line`, its version in `tw_v`, and every other `tw_*` column reads `-`. Nothing is written to `detail` (D1), so the schema note lives only in the `tw_*` columns.
- A key missing from a `v=1` line is 0, which matters for `NonZero` heartbeat lines. A log with no complete line has `-` in every tripwire column, `tw_v` included, not 0: no line is not the same as zero counts.
- Value counts are not validated against key widths. The contract does not require it, and a check could only turn data into `-`.

**D3. Columns** (exact set: Q3, plus `tw_v` from M1 and `g1_line` from S5). New `summary.tsv` columns are appended after the existing 22, so positional readers (`scripts/agent/brief.sh` reads `$2` and `$3`) keep working, and the whole lines go last.

- `ipc_avg_us` and `ipc_iters` (`-` when unreadable)
- `reentry_lock`, `reentry_ctx` and `reentry_holder_irqs`
- `ev_ph`, `ev_self`, `ev_self_irq` (`kind=self` with `ctx=irq` or `irq-exit`) and `ev_stuck`
- `tw_v`, `tw_src`, `tw_cpu`, `tw_t` and `tw_ncpu`
- one `tw_<key>` column per key, in `Key::ALL` order, holding the comma list as printed (a missing key is `0`)
- `g1_elrmm` (the `src=g1` line's `elrmm` when that line is `v=1`, else `-`; H1 candidate 1)
- `g1_line`, the whole last complete `src=g1` line, then `tw_line`, the whole last complete line

That is 39 + `Key::COUNT` new columns: 100 in all today (22 + 39 + 61), not the 98 Q3 counted; `tw_v` and `g1_line` are the two additions. `tw_t` beside `last_tick` shows a stale line, one whose CPU 0 stopped long before the end. The key list comes from `shared::tripwire::Key::ALL` (Q2). A test fails when the catalogue and the parsed `GOLDEN_FULL` disagree in order or names.

**D4. Report layout.**

- **Single run** (`summary.md`, unchanged except as follows):
  - the class table lists the 8 classes (T3), then 9 (T4);
  - a "Gate 1 IPC" line gives the mean of `ipc_avg_us` over CLEAN boots (integer µs per boot, so the mean has 1 µs resolution per boot);
  - a "Tripwire counters by class" table follows (B1's format: rows are `key[index name]` with a non-zero value in some boot, columns are the classes present, cells are "boots non-zero / sum", or "/ max" for gauges);
  - the per-boot table gains a `tw_src` column (`v=N` for a line of another schema version).
  - Index names come from `shared` (`WakeSource`, `LockClass`, `SchedulerClass`, `N2Kind`, `BadchanSite`; CPU n for per-CPU keys).
- **Interleaved run** (top-level `summary.md`), rewritten from `boots.tsv` and `arms.tsv` after every boot (S3), so a signal or a CI time limit still leaves a report of the boots so far:
  0. **Status:** `running (k of N boots)`, `stopped (<reason>) after k of N boots`, or `finished (N boots)`. Reasons: the signal's name, "3 boots in a row where the UEFI stub never ran", "the QEMU binary changed", "the firmware changed".
  1. **Settings:** design, rotation, progress, QEMU version and sha256, firmware and sha256, host, load1 before the builds, after the builds and at the end, the load override if given, the arm-base override if given (D5), and the harness's own commit.
  2. **Arms:** label, checkout, commit (`-dirty`), channel, rustc and kernel ELF sha256.
  3. **Classes per arm:** the 9 classes, Total, and the CLEAN rate with its Wilson interval over conclusive boots.
  4. **Load:** per-arm load1 mean and max. The 25% verdict reads "redo the pair" when it fails.
  5. **Pair tests,** one block per pair (earlier arm = previous, later arm = new):
     - the regression guard: CLEAN counts, one-sided p toward the new arm being lower, verdict `fails` or `passes`;
     - one row per class, plus each `--combine` group: counts, one-sided p (fewer in new), one-sided p (more in new), two-sided p, a `removed` marker (0 in new and one-sided p (fewer) < 0.05) and a `new` marker (0 in previous and one-sided p (more) < 0.05);
     - raw differences beside the p values;
     - a Bonferroni note when there is more than one pair.
  6. **Gate 1 IPC:** per arm, the mean of `ipc_avg_us` over CLEAN boots, with n, and the `G1PASS` count.
  7. **Tripwire per arm:** as above, with arms as columns. The count of non-CLEAN boots with neither a complete tripwire line nor a fatal report is R16's check, labelled "meaningful only for an arm whose kernel prints tripwire lines (step 1b, e211d6d, or later); for an older arm every non-CLEAN boot without a fatal report counts". An arm with no complete tripwire line in any boot shows `n/a (no tripwire line in any boot)` in that cell instead of a count.
  8. **Non-CLEAN boots:** round, arm, class, last tick, first fatal line or detail (for DEGRADED, the `ipc` cell of D1), `tw_src` and log.

  Fisher runs on conclusive boots only, as `soak-matrix.sh` does.
- **Step 1b's combined test.** It needs WEDGE-STUCK + PANIC-LOCK compared two-sided. `main` has no lock detector, so its PANIC-LOCK count is 0 and the combined count equals its WEDGE-STUCK. So a generic `--combine CLASS+CLASS` row computes exactly the ADR's test: `--combine WEDGE-STUCK+PANIC-LOCK`. No step-specific code.

**D5. Interleave CLI and layout** (Q1, Q8).

- `aios soak --arm DIR --arm DIR [--arm DIR [--arm DIR]] [options]`, with an `arm=DIR` alias for `just soak`.
- Each DIR is a git checkout (a worktree is fine). Labels are A–D in the order given; the first is "previous" in every pair.
- The same DIR twice is an A/A control, built once.
- Without `--arm` nothing changes. A single `--arm` is a usage error.
- **Exit status (S7).** Interleave mode is report-only: it exits 0 when the soak ran to the end, whatever the classes; 1 only with `--fail-on-regression` when some pair's regression guard fails; 2 on a usage, preflight or setup error and on the S4 stop; 128 + n on a signal, as single mode. `--report-only` (`report_only=1`) together with `--arm` is a usage error naming `--fail-on-regression`, so no command carries a flag that does nothing. Every documented command (T12's smoke soak, step 1b's A/B, `soak-matrix.yml`) passes neither.
- **Host tools.** `git`, `just`, `rustup`, `rustc`, `qemu-system-aarch64` and `mcopy` must be on `PATH`; a missing one is refused before any build with exit 2. For `rustup` the message carries `soak-matrix.sh`'s hint: "installing each arm's pinned toolchain needs rustup 1.28 or later, and network access the first time". `rustup` is required with `--no-build` too, because the `rustc --version` check relies on rustup choosing each arm's toolchain from its directory.
- **Arm base (M3).** Every arm must contain `MIN_ARM_BASE` = 7167d408f6a43ca9859fb6238f608ca6bdee37d6 (#196; strict-NX firmware faults every older kernel), checked with `git merge-base --is-ancestor` in the arm. The preflight takes the base as a parameter, so unit tests pass their own. The binary passes `MIN_ARM_BASE`, except that a build with debug assertions (the profile `cargo test -p aios-tools` and CI's Tools job use) reads `AIOS_SOAK_MIN_ARM_BASE` (a full 40-hex SHA) when it is set. Release builds (`just tools`, so `just soak`, the shim and CI's soak jobs) never read the variable. When the override is in effect, the run warns on stderr and Settings names it, so no report can hide it.
- Per arm, in this order:
  1. the channel from DIR's `rust-toolchain.toml`, and the arm-base check;
  2. `rustup toolchain install --no-self-update` in DIR (rustup reads DIR's `rust-toolchain.toml`, as `soak-matrix.sh` does), skipped with `--no-build`;
  3. `just disk` in DIR, with `RUSTUP_TOOLCHAIN` and `CARGO_TARGET_DIR` unset, skipped with `--no-build`;
  4. `rustc --version` in DIR, also with `--no-build` (N6);
  5. `just --evaluate disk_img`, `kernel_elf` and `edk2_fw` read in DIR;
  6. the ESP snapshot to `.scratch.*/esp-X.img`, and the kernel ELF sha256 from that snapshot;
  7. the git rev, with `-dirty`.
  The channel, rustc, firmware and arm-base comparisons all run with `--no-build` (N6); only the toolchain install and the build are skipped.
- One harness, the running `aios`, boots and classifies every arm with its own QEMU arguments. An arm's justfile `run` recipe is not used, the same as single mode today.
- Rounds rotate the arm order by one each round (A B, B A, A B, …).
- Each boot gets a fresh data disk.
- **Harness-error boots (S4).** A boot whose log has no `AIOS UEFI stub` line is a harness error: the stub never ran, or QEMU could not be spawned (status 127, as `runner.rs` records it). Each arm's first boot keeps single mode's check and stops at once. After that, `MAX_HARNESS_ERRORS_IN_A_ROW` = 3 such boots in a row, counted in boot order across arms, stop the soak with exit 2 and the status "stopped (3 boots in a row where the UEFI stub never ran)". Those boots stay in `boots.tsv` (they classify INCONCLUSIVE, so Fisher already excludes them).
- Output, default `target/soak/<timestamp>-<mode>-arms`: under `target/soak/`, so `/merge-and-cleanup` copies it and `/justin:brief` sees it (S8, T8).
  - `arm-X/run-NN.log` and `summary.tsv` (row by row), plus `summary.md` and `build.log`, per arm: each is a normal single-run directory, so every single-run reader works on it. An arm's `summary.md` is written when the soak finishes, as in single mode, so a stopped soak leaves none;
  - top-level `summary.md` (the pair report, rewritten after every boot through a temporary file and a rename), `boots.tsv` (round, position, arm, then the `summary.tsv` columns, row by row) and `arms.tsv`.
- Signals behave as in single mode, except that the top-level `summary.md` is rewritten once more with the "stopped" status before the exit.

**D6. Fisher without new dependencies.**

- `ln_fact` is a cumulative `Vec<f64>` of ln k.
- `hyper(x)` = exp(ln C(r1,x) + ln C(r2,c1−x) − ln C(n,c1)).
- One-sided low = Σ x ≤ a, one-sided high = Σ x ≥ a.
- Two-sided = Σ of hyper(x) ≤ hyper(a)·(1+1e−7), R's convention and `soak-matrix.sh`'s.
- Each result is clamped to 1.
- Tests:
  - an exact oracle in the test, from u128 binomials (C(60,30) ≈ 1.2e17 fits), over every table up to 30 + 30, with relative error below 1e−9;
  - the ADR's figures at 30 boots per arm: 4→0 p = 0.0562, 5→0 p = 0.0261, 9→0 p = 0.0010;
  - developer-guide §5.6's two-sided figures: 6/20 vs 12/20 p = 0.111, 6/20 vs 18/20 p = 0.000244.
- `wilson` moves from `report.rs` into `stats.rs`, unchanged.

**D7. `scripts/soak-matrix.sh` and its workflow** (Q1 A). The interleave mode replaces the script, which is deleted in T10, under the owner's no-legacy rule.

- Two interleave implementations would be the duplicate that rule forbids.
- The script cannot serve step 1b's A/B anyway: it classifies each arm with that arm's own harness, and both arms predate 1a, so the subclasses and columns would be missing. That is why the 2026-10-06 note classifies by hand.
- `.github/workflows/soak-matrix.yml` keeps its name, inputs and trust model: full 40-hex SHAs only, this repository's PR heads fetched, no `rust-cache`, the computed step limit.
- Its soak step changes:
  - each arm becomes `git worktree add --detach "$RUNNER_TEMP/arm-$L" "$sha"`;
  - the harness is built from the dispatching ref (`just tools`);
  - the step runs `just soak --arm … runs= secs= mode= --ignore-load out="$MATRIX_OUT"` (plus `--allow-mixed-toolchains` when that input is set). No `report_only`: interleave mode is report-only (D5). `--ignore-load` because the runner is single-tenant and its load at the start reflects the job's own setup steps; the loads are still recorded.
- Every arm is then classified by the dispatching ref's classifier: the intended change, and the summary records the harness commit.
- `soak-matrix.sh` features that go:
  - pre-R4 arms booted by `soak-qemu.sh`, and the `timeout` probe with them. Every arm is now booted by one harness, and any arm with a 7167d40-or-later ESP boots;
  - ref resolution inside the tool. Local users pass checkouts, and CI creates the worktrees in YAML.

**D8. Oracle parity tests after the split** (Q4 A), all in T3's commit (M2):

- Keep the classifier differential (synthetic corpus and real logs) against the oracle through `base_line()`. It is the acceptance's mechanism, and it proves 1a changed nothing but the intended refinements. It keeps `oracle_script()`, `oracle_awk()` and `oracle_classify()`.
- Re-bless `classify.golden` and the CLI and harness goldens from aios (`AIOS_BLESS_GOLDENS=1`), reviewing each diff.
- Delete everything that runs or records the whole oracle script, since those outputs now differ by design:
  - `soak_classify.rs`: `record_classify_goldens_from_oracle`;
  - `soak_cli.rs`: `cli_differential_against_oracle` (not ignored today, so it breaks in this commit), `record_cli_goldens_from_oracle`, `run_oracle_renamed`;
  - `soak_harness.rs`: `record_harness_goldens_from_oracle`, `harness_differential_against_oracle`;
  - `common/soak.rs`: `run_oracle`, `rename_prefix`;
  - `common/soak_fake.rs`: `Tool::Oracle` (and with it the `Tool` enum, if `Aios` is its only variant left), `ambient_timeout`, the `oracle-bin` PATH entry, `PRE_QEMU_TOOLS` and `pre_qemu_tools` (the `no-qemu` scenario's PATH becomes the fake `bin` alone: aios runs nothing before its QEMU lookup, and that scenario's golden must not change), the copy of the oracle into each fake repository (`:484`; `git_rev` ignores untracked files, so no `-dirty` changes), and `golden_text`'s `rename` parameter;
  - the module docs of `soak_classify.rs`, `soak_cli.rs`, `soak_harness.rs` and `common/soak.rs`, rewritten to say what the tests now check.
- CI's Tools job keeps `fetch-depth: 0`, since the classify oracle and docs-check's `check.py` oracle still read history (`ci.yml:61` stays accurate).

**D9. The catalogue comes from `shared`** (Q2 A). `aios-tools` depends on `shared` (a path dependency) and reads `Key::ALL`, `name()`, `width()` and `is_gauge()`, and the index enums' names, from it.

- One source of truth: when the kernel adds a key, the tools build picks it up, and the `GOLDEN_FULL` test catches an order change.
- The cost is `sha2` (no default features, MIT/Apache, already in `Cargo.lock`) as a transitive dependency of the host tool, and rule 01's approved-dependency sentence amended. `tools/` keeps `#![forbid(unsafe_code)]`; it covers only tools' own code, and `shared`'s documented `unsafe` comes with the dependency, which the amended sentence says.
- **`shared/` becomes a build input of the aios binary (S1).** Without it, a `shared/` change that alters the catalogue would leave the installed `aios` "fresh" while it parses with the old keys. So `shared` joins the input lists, with its OS/editor-file excludes, in the `tools` recipe (`justfile:221`, `:228–231` and its comment) and in the shim (`.claude/hooks/aios:198`, `:224–227` and its header), in one commit, since the two must match. The shim half is lead-applied (see "Lead-applied .claude edits").

-----

## Tasks

One commit each, `Crash fix step 1a: <desc>`, pushed after its checks pass (rule 03). Every task's commit runs the A3 gates on its own. "Bless" means `AIOS_BLESS_GOLDENS=1 cargo test -p aios-tools --test <file>`, then reading the golden diff before committing. Implementers never edit `.claude/`: where a task needs a `.claude/` change, the lead applies it in the worktree before that task's commit (see "Lead-applied .claude edits"), and the task's commit includes it.

- [x] **T1. Working plan.** This file.
  - Check: `cargo run -q -p aios-tools -- docs-check`: no new drift except knowledge-hygiene's non-empty `plans/`.
- [x] **T2. Runner refactor: `Arm` and `boot_once`.** No change in behaviour.
  - Files: `runner.rs`. An `Arm` holds the checkout root, firmware, the ESP snapshot path, the kernel sha line and the git rev. `boot_once(arm, cfg, n, …) -> BootOutcome` runs one QEMU boot and writes the log and footer. The loop in `build_and_boot` calls it.
  - Tests: the existing unit and harness tests.
  - Check: `cargo test -p aios-tools` with `git status --short tools/tests/golden` empty, so no golden changed.
- [x] **T3. Class enum (8 classes), WEDGE split, PANIC-LOCK, oracle tests retired.**
  - Files:
    - `classify.rs`: `Class` without `Degraded`, refinement, `base_line()` (D1), `reentry` parsing;
    - `report.rs`: the 8-class table and counts;
    - `mod.rs`: the `USAGE` class list;
    - `scripts/agent/brief.sh`: its `summary.tsv` class counts gain WEDGE-STUCK, WEDGE-ALIVE and PANIC-LOCK, so WEDGE is not read as 0;
    - `justfile` comment line 137;
    - developer-guide §5.6 "Classes" table;
    - `tools/tests/fixtures/soak/synthetic.txt`: new cases for each WEDGE-STUCK branch, each WEDGE-ALIVE branch, gpu markers missing, a two-line `lock re-entry:` panic, and a `lock re-entry:` after an earlier EXCEPTION, which stays EXCEPTION;
    - `soak_classify.rs`: the fold differential (synthetic and real logs) through `base_line()`, plus the A1 transition matrix and the `summary.tsv` cross-check;
    - every oracle deletion in D8, in this same commit: `record_classify_goldens_from_oracle`, `cli_differential_against_oracle`, `record_cli_goldens_from_oracle`, `run_oracle_renamed`, `record_harness_goldens_from_oracle`, `harness_differential_against_oracle`, `run_oracle` and `rename_prefix` (`common/soak.rs`), and `Tool::Oracle`, `ambient_timeout`, the `oracle-bin` PATH entry, `PRE_QEMU_TOOLS`/`pre_qemu_tools`, the oracle copy and `golden_text`'s `rename` parameter (`common/soak_fake.rs`), with the four module docs;
    - the classify, CLI and harness goldens re-blessed.
  - Tests: one unit test per mapping; the `line()`/`base_line()` field-1-only test. The corpus check "every class has at least 5 cases" iterates `Class::ALL`, so it covers the 8 classes here.
  - Check: A1 run on `target/soak/167` prints exactly the expected matrix. The golden diffs contain only class tokens, class-table rows and the `USAGE` class list (`git diff --word-diff tools/tests/golden`). The `no-qemu` harness golden is unchanged. `rg -n 'run_oracle|rename_prefix|Tool::Oracle|ambient_timeout|from_oracle|against_oracle' tools/tests` finds only `classify_differential_against_oracle`.
- [x] **T4. DEGRADED and the IPC line.**
  - Files: `classify.rs` (`Class::Degraded`; IPC parse into `ipc`; DEGRADED where CLEAN was returned, with no note added to `detail`), `report.rs` (`ipc_avg_us` and `ipc_iters` columns; the 9-class table; the DEGRADED cell `IPC <N> iters` / `IPC iteration count unreadable` in the console line and per-boot table), `mod.rs` `USAGE`, `scripts/agent/brief.sh` (DEGRADED count), developer-guide §5.6 heartbeat rule 3 and the classes table (noting that `avg=` is integer µs), corpus cases (0 iters with G1DONE, which is step 1b's Gate 1 FAIL; 9999; 10000; no IPC line; an IPC line cut before `(N iters)`; a fatal boot with 0 iters, which stays fatal, as `pr209-fix-198` shows; at least 5 DEGRADED cases in all, for the corpus check), goldens re-blessed.
  - Tests: unit tests; the field-1-only test now covers DEGRADED cases (their `detail` and `lb` equal the oracle's CLEAN output); a drift test that reads `kernel/src/bench.rs` and asserts `IPC_ITERATIONS: usize = 10_000`.
  - Check: A1 still shows CLEAN → DEGRADED 0 and no `base_line()` difference. The A2 classes hold.
- [x] **T5. Tripwire parser, event counts, `shared` as a dependency and build input.**
  - Files:
    - new `tripwire.rs` (`parse_line`, `LastLines { last, g1 }`, `EventCounts`), wired into the `Scan` in `classify.rs`;
    - `tools/Cargo.toml`: the `shared` path dependency (Q2);
    - `justfile`: `shared` in the `tools` recipe's `inputs` (`:221`), its OS/editor-file exclude pathspecs (`:228–231`, the same five patterns as `tools/`), and the recipe comment that names the anchored excludes;
    - `tools/tests/shim.rs`: the `Sandbox` writes `shared/src/lib.rs`; `shared/src/lib.rs` joins the committed-change list (`:1257`); a `shim-stale-shared` case joins the stale-input cases (`:502`); `shared/.DS_Store` joins the OS/editor-file case;
    - lead-applied in this commit: E1–E4, the `.claude/hooks/aios` input list, excludes and header, and rule 01's dependency sentence (see "Lead-applied .claude edits"). The shim tests copy the worktree's shim and justfile, so both halves must be in the tree before the tests run.
  - Tests:
    - `GOLDEN_FULL` extracted from `shared/src/tripwire.rs` parses complete (`n=66` = 5 prefix tokens + 61 keys), with its keys in `Key::ALL` order;
    - `GOLDEN_NONZERO` parses (`n=14` = 5 + 9), and its missing keys read 0;
    - a line whose `n` counts `[tripwire]`, or leaves out the prefix tokens, is incomplete (N1);
    - an incomplete last line falls back to the previous complete one;
    - a line that starts mid-line is accepted;
    - `[tripwire-ev]` is not a tripwire line;
    - a complete `v=2` line is kept whole with `tw_v` 2 and every decoded column `-`, and `detail` is unchanged;
    - `src=panic` and `src=exc` are accepted;
    - a log with no line gives `-`;
    - event lines count by kind, and by ctx for `self`.
  - Check: unit tests pass, the shim tests pass. A scratch run over B1's text logs shows `tw_src` `panic` for runs 05, 07 and 11.
- [x] **T6. Tripwire columns, `--classify --out`, per-class table.**
  - Files:
    - `report.rs`: the D3 columns, the "Tripwire counters by class" table, the Gate 1 IPC line. `--classify --out` writes through the same `report::tsv_row` and `report::summary_head` as a soak (N7): `SummaryInfo` gets the values a set of logs cannot give (secs, load at start and end, commit) as `-` or from the footers, and no second writer exists;
    - `mod.rs`: `--classify --out DIR` writes `summary.tsv` and `summary.md` for the given logs, with timing from each footer; `USAGE`;
    - `runner.rs`: rows carry the new columns;
    - `tools/tests/common/soak_fake.rs`: the `summary.tsv` normaliser keys on the header's column count instead of the literal 22;
    - developer-guide §5.6 output table;
    - observability §6.5 "Parser contract" (names `aios soak` as the consumer and lists the columns);
    - goldens re-blessed.
  - Tests: `tsv_row` field count equals the header count (22 + 39 + `Key::COUNT`); unit tests of the table on synthetic classifications; CLI goldens for `--classify --out`; a test that `--classify --out` and a fake soak of the same logs write identical `summary.tsv` rows apart from the timing columns.
  - Check: A2 in full, meaning the projection diff is empty for 30 of 30 rows and the per-class table matches the note's.
- [x] **T7. Statistics module.**
  - Files: new `stats.rs` (`fisher_low`, `fisher_high`, `fisher_two`, `wilson` moved from `report.rs`).
  - Tests: as in D6.
  - Check: `cargo test -p aios-tools stats`.
- [x] **T8. Interleave mode.**
  - Files: new `interleave.rs` (D5, including the status rewrite of the top-level `summary.md` after every boot, S3, and the harness-error stop, S4), plus `mod.rs` (`--arm`, `arm=`, `--allow-mixed-toolchains`; `--report-only` with `--arm` refused; `USAGE` with the exit statuses of D5), `runner.rs` (shared setup), `host.rs` (sha256 of the resolved QEMU binary, `rust-toolchain.toml` channel, `rustc --version` in a directory, `merge-base --is-ancestor`), `scripts/agent/brief.sh` (S8: a directory holding `arms.tsv` is one interleaved run, listed as "newest interleaved soak" with its status line and per-arm CLEAN counts from `boots.tsv`; directories whose parent holds `arms.tsv` are skipped; an interleaved run never counts as "main soak"), `tools/tests/common/soak_fake.rs` (fake arm checkouts: worktrees of one fake repository, each with its own fake `just` and a `.fake-rustc` file; fake `rustup` that logs its arguments and directory and fails under a flag; fake `rustc` that prints the arm's `.fake-rustc`; QEMU that differs per arm; `AIOS_SOAK_MIN_ARM_BASE` set to the fake repository's second commit).
  - Tests (fake-QEMU scenarios):
    - two arms with the boot order ABBAAB… recorded in `boots.tsv`;
    - A/A with the same dir twice;
    - refusals, each with exit 2 before any boot: channel mismatch, rustc mismatch, firmware mismatch, an arm without the base (a worktree at the fake repository's first commit), no `rustup` on PATH (the message carries the rustup 1.28 hint), `rustup toolchain install` failing, one `--arm`, five `--arm`, `--report-only` with `--arm`;
    - with `--no-build`: no `rustup toolchain install` and no `just disk` in the fake logs, but the channel, rustc, firmware and arm-base refusals still fire (N6);
    - `--allow-mixed-toolchains` marks the summary;
    - the arm-base override is named in Settings and on stderr;
    - non-CLEAN boots in both arms still exit 0 (report-only, S7);
    - a QEMU sha change mid-soak stops the soak, that boot is not counted, and the status says why;
    - 3 boots in a row without the stub line (after each arm's first boot) stop the soak with exit 2 and the S4 status; 2 in a row followed by a good boot do not (S4);
    - SIGINT mid-round leaves per-arm `run-NN.log` and `summary.tsv` intact, no per-arm `summary.md`, and a top-level `summary.md` with status "stopped (SIGINT) after k of N boots" whose class counts match `boots.tsv` (S3);
    - after each boot of a normal run, the top-level `summary.md` reads `running (k of N boots)` (checked from the fake QEMU of boot k + 1), and `finished` at the end;
    - per-arm directories hold valid single-run files;
    - `brief.sh` on a fixture `target/soak/` with one interleaved run and one single run lists the interleaved run once and never as "main soak" (a shell test beside the existing brief tests, or a `tools/tests` test that runs it, whichever already exists).
  - Check: the scenario goldens pass. In `cargo run -q -p aios-tools -- soak --help`, the `--arm` text names the checks and the exit statuses.
- [x] **T9. Pair report and load rule.**
  - Files: new `pair.rs` (D4 interleaved report from per-arm rows; `--combine`; `removed` and `new` markers; the R16 label), `interleave.rs` (load1 is read before the arm builds, and the soak refuses with exit 2 when it is above the CPU count unless `--ignore-load`, per Q6; load1 after the builds and at the end are recorded; the per-boot load is recorded as today; `--fail-on-regression`), `mod.rs` (`--combine`, `--ignore-load`, `--fail-on-regression`; `USAGE`).
  - Tests:
    - the report on synthetic arms: regression guard pass and fail at p = 0.05 boundaries, "removed" 5→0 yes and 4→0 no, "new" 0→5 yes and 0→4 no, a combined row, INCONCLUSIVE excluded from the denominators;
    - the 25% verdict at 24% and 26%;
    - the Gate 1 mean over CLEAN only;
    - the R16 count, its label, and `n/a` for an arm with no tripwire line in any boot;
    - the load refusal fires before any fake `just disk` runs (S2), `--ignore-load` overrides it, and Settings shows all three loads;
    - `--fail-on-regression` exits 1 when the guard fails and 0 when it passes;
    - a golden of a fake two-arm run's `summary.md`.
  - Check: goldens and units pass. A dry run of the step-1b A/B command line (`--help` shows `--combine`) is documented in the PR.
- [ ] **T10. CI: `runs` input, `soak-matrix.yml` on aios, delete `scripts/soak-matrix.sh`.**
  - Files:
    - `.github/workflows/ci.yml`: `workflow_dispatch.inputs.runs` (number, default 5, 1–60), passed as `RUNS` through `env`; the soak step uses `RUNS=${RUNS:-5}`, since `inputs.runs` is empty on push and pull_request (S6); a plan step validates it (digits only, leading zeros dropped, 1–60, as `soak-matrix.yml` does) and computes the step limit as ceil(runs × (90 + 15) / 60) + 5 min (110 at 60 boots); the job's `timeout-minutes` rises from 30 to 130, covering the 60-boot step plus 20 min of setup, summary and upload (Q7);
    - `.github/workflows/soak-matrix.yml`: D7, with `--ignore-load` and no `report_only` (S2, S7);
    - `scripts/soak-matrix.sh` deleted;
    - the developer-guide §5.6 interleave paragraph rewritten;
    - `docs/project/agent-loop.md` (its `target/soak-matrix/` sentence, `:63`);
    - lead-applied in this commit: the `.claude/CLAUDE.md` layout edit E5 (see "Lead-applied .claude edits").
  - Check:
    - `actionlint` if installed, else `python3 -I -c 'import yaml…'` over both workflows;
    - `rg -n --hidden -g '!.git' -g '!.claude/worktrees' -g '!docs/knowledge/**' -g '!target' 'soak-matrix\.sh' .` returns nothing (N4: without `--hidden` it would skip `.github/` and `.claude/`);
    - docs-check `repo-paths` has no new finding.
  - The CI behaviour itself is checked after push: a `workflow_dispatch` of CI on the branch with `runs=2`, and a `soak-matrix.yml` dispatch with two arms (`runs=2`), both read from the job summary; the branch's own push run shows the 5-boot default. These runs are on CI, not local QEMU.
- [ ] **T11. Docs sweep.**
  - Files:
    - the ADR, a review note "Step 1a delivered, <date>": what landed, the R10 evidence, that the 2026-10-06 hand-classification note and the `soak-matrix.sh` paths in "Soak protocol" are superseded (in-place pointer only), and the step-1b A/B command line;
    - developer-guide §5.6 consistency pass (classes, columns, interleave, exit statuses, statistics, CI input);
    - observability §6.5 (if T6 left anything);
    - `README.md` (`just soak` row: classes and `--arm`);
    - `docs/knowledge/discussions/2026-09-22-jl-rust-agent-tools.md` only if it states current behaviour that changed.
  - No `.claude/` edit is left for T11: rule 01 (E4) lands in T5 and the CLAUDE.md layout (E5) in T10.
  - Check: docs-check no new drift (plan finding aside); `/audit-loop` docs scope.
- [ ] **T12. Final gate and distill.**
  - A1 and A2 re-run on the final head, with output pasted into the PR.
  - An attended local smoke soak, with the owner's go-ahead because it is host-exclusive and about 5 minutes: `just soak --arm <af59149 worktree> --arm . runs=2 secs=75`. It must show 4 boots, rotation ABBA, the pair report with status `finished`, per-arm directories, and exit 0. Only this task's own QEMU PID may be stopped.
  - `/verify-phase`-equivalent gates (A3), then `/audit-loop` until a clean round.
  - Distill this plan into lessons and decisions, then delete it.
  - Push, open the PR, run `/review-pr-comments`, and hand off for `/merge-and-cleanup`.

After the merge (not step 1a's PR): step 1b's A/B soak runs with `just soak --arm <af59149> --arm <e211d6d> runs=20 secs=75 --combine WEDGE-STUCK+PANIC-LOCK`, then the same with `mode=gpu runs=10`. Both are report-only by D5, as the T12 smoke soak is.

-----

## Lead-applied .claude edits

`.claude/` is protected (rule 08): implementers never edit it. The lead applies each edit below in the worktree before the named task's commit, and that commit carries it. Two batches, so two permission prompts. Line numbers are at `main` e211d6d.

**Batch 1, before T5's commit.** The shim must match the justfile's `tools` recipe, which T5 changes, and `tools/tests/shim.rs` copies both from the worktree.

*E1. `.claude/hooks/aios:198`, the input list.* From:

```sh
inputs='tools Cargo.lock Cargo.toml rust-toolchain.toml rust-toolchain .cargo justfile'
```

To:

```sh
inputs='tools shared Cargo.lock Cargo.toml rust-toolchain.toml rust-toolchain .cargo justfile'
```

*E2. `.claude/hooks/aios`, after line 225* (the second `tools/**` exclude line of `dirty_cause`'s `git status`), insert the same five patterns for `shared/`, as T5 adds them after `justfile:229`:

```sh
        ':(exclude,glob)shared/**/.DS_Store' ':(exclude,glob)shared/**/*.swp' ':(exclude,glob)shared/**/*.swo' \
        ':(exclude,glob)shared/**/*~' ':(exclude,glob)shared/**/*.rs.bk' \
```

*E3. `.claude/hooks/aios:9–11`, the header's input list.* From:

```sh
# records HEAD's tree entries for the build inputs (tools/, Cargo.lock,
# Cargo.toml, rust-toolchain.toml, a legacy rust-toolchain file if there is
# one, .cargo/, and the justfile, whose recipe writes the stamp), the git hash
```

To:

```sh
# records HEAD's tree entries for the build inputs (tools/, shared/ (the
# tripwire key catalogue aios-tools links), Cargo.lock, Cargo.toml,
# rust-toolchain.toml, a legacy rust-toolchain file if there is one, .cargo/,
# and the justfile, whose recipe writes the stamp), the git hash
```

*E4. `.claude/rules/01-code-conventions.md:39`, the host-tools dependency sentence (Q2 A).* From:

```markdown
- Host tools (`tools/`, package `aios-tools`, binary `aios`) are a std crate built for the host: the `no_std`/`no_main` rules and the `no_std` dependency rule do not apply there. Its dependencies are the approved set clap, anyhow, serde, serde_json, regex and signal-hook (R4) (`time` is added by the first PR that uses it), and it forbids `unsafe`.
```

To:

```markdown
- Host tools (`tools/`, package `aios-tools`, binary `aios`) are a std crate built for the host: the `no_std`/`no_main` rules and the `no_std` dependency rule do not apply there. Its dependencies are the approved set clap, anyhow, serde, serde_json, regex and signal-hook (R4) (`time` is added by the first PR that uses it), plus the workspace's own `shared` crate by path (crash-fix step 1a: the tripwire key catalogue), which brings in `sha2` (no default features) transitively and makes `shared/` a build input of the binary. It forbids `unsafe` in its own code; `shared`'s documented `unsafe` comes with the dependency.
```

**Batch 2, before T10's commit.** T10 deletes `scripts/soak-matrix.sh`, and its `rg --hidden` check covers `.claude/`. The soak modules all exist by then, so the layout line T11 would otherwise touch is batched here, and T11 needs no `.claude/` edit.

*E5. `.claude/CLAUDE.md:242–245`, Workspace Layout.* From:

```text
│                         src/cmd/soak/ (boot soak harness, just soak), tests/ (goldens, fixtures)
├── scripts/              soak-matrix.sh (interleaved multi-revision soak; CI: soak-matrix.yml),
│                         agent/ (brief, checkpoint),
│                         docs/baseline.json (accepted docs drift)
```

To:

```text
│                         src/cmd/soak/ (boot soak harness, just soak: classify, tripwire, stats,
│                         interleave --arm (2-4 checkouts; CI: soak-matrix.yml), pair report),
│                         tests/ (goldens, fixtures); depends on shared/ (tripwire key catalogue)
├── scripts/              agent/ (brief, checkpoint),
│                         docs/baseline.json (accepted docs drift)
```

-----

## Docs this makes stale

| Doc | Change | Task |
|---|---|---|
| `docs/project/developer-guide.md` §5.6 (and the `just soak` row at :1559) | Classes table, heartbeat rule 3 (DEGRADED; `avg=` is integer µs), output files, the `--classify --out`, interleave (exit statuses, status line), statistics and CI `runs` paragraphs; the `soak-matrix.sh` paragraph is replaced | T3, T4, T6, T10, T11 |
| `docs/kernel/observability.md` §6.5 | "Parser contract" names its consumer and the columns | T6 |
| Crash-fix ADR | A "Step 1a delivered" review note; pointers where "Soak protocol" names `soak-matrix.sh` and the hand classification | T11 |
| `.claude/CLAUDE.md` | Layout: the `scripts/` line loses `soak-matrix.sh`; the `tools/` line names the soak modules (lead-applied, batch 2) | T10 |
| `.claude/rules/01-code-conventions.md` | The tools dependency sentence (lead-applied, batch 1) | T5 |
| `.claude/hooks/aios` | `shared` in the build inputs, excludes and header (lead-applied, batch 1) | T5 |
| `justfile` | The `soak` comment's class list (line 137, T3); `shared` in the `tools` recipe's inputs, excludes and comment (T5) | T3, T5 |
| `tools/tests/shim.rs` | `shared/` in the sandbox and the stale-input cases | T5 |
| `README.md` | The `just soak` row | T11 |
| `docs/project/agent-loop.md` | The `scripts/soak-matrix.sh` / `target/soak-matrix/` sentence (:63) | T10 |
| `scripts/agent/brief.sh` | Class counts read from `summary.tsv` (T3, T4); the interleaved layout (T8) | T3, T4, T8 |
| `tools/src/cmd/soak/mod.rs` `USAGE` | Every new option, class and exit status (in the commit that adds it) | T3–T9 |

No developer-guide, README or `.claude/CLAUDE.md` sentence enumerates the aios build inputs (checked with `rg`); the enumerations are the shim's and the justfile's, covered above.

-----

## Open questions for the owner

**Answered 2026-10-10 11:31 (owner): every recommendation taken: Q1 A, Q2 A, Q3 A, Q4 A, Q5 A, Q6 refuse-with-override and the min-based ratio, Q7 A, Q8 A.** The questions stay below as the record of the options.

Each has options and a recommendation. Nothing below is decided by this plan, and the tasks that depend on a question wait for its answer: Q1 and Q8 gate T8 and T10; Q2 gates T5; Q3 gates T5 and T6; Q4 gates T3; Q5 gates T4; Q6 gates T9; Q7 gates T10.

**Q1. What happens to `scripts/soak-matrix.sh` and `soak-matrix.yml`?**

- A. The `aios soak --arm` interleave mode replaces the script, which is deleted; `soak-matrix.yml` keeps its trust model, creates the arm worktrees in YAML and runs the dispatching ref's `just soak --arm …`. Every arm is classified by one, current classifier.
- B. Port the whole script into aios, including ref resolution, worktree creation and `rustup` install from refs (`aios soak --ref SHA …`); delete the script.
- C. Keep the script as the orchestrator and add aios's interleave mode beside it, with the script calling the newest classifier. That leaves two interleave implementations.
- **Recommendation: A.** C breaks the no-legacy rule. B moves about 300 lines of ref-safety logic into Rust for a convenience that `git worktree add` already gives. Only A classifies pre-1a arms (step 1b's A/B) with the new classes.

**Q2. Where does the harness get the tripwire key catalogue?**

- A. `aios-tools` depends on `shared` (a path dependency, pulling `sha2` in transitively); rule 01's tools dependency sentence is amended.
- B. No dependency: tools keeps its own copy of key names, widths and index names, checked by a test that parses `shared/src/tripwire.rs` and `shared/src/lock.rs` as text.
- C. A dev-dependency on `shared` only, with tools' copy checked against it in tests.
- **Recommendation: A.** One source of truth and no duplicate catalogue. B and C keep a copy that only a test keeps honest.

**Q3. Which columns go into `summary.tsv`?**

- A. Every key: `tw_<key>` for all 61 keys in `Key::ALL` order (comma lists as printed); plus `tw_src`, `tw_cpu`, `tw_t`, `tw_ncpu`, `g1_elrmm` and the whole line `tw_line`; plus the H3 inputs `ev_ph`, `ev_self`, `ev_self_irq` and `ev_stuck` from `[tripwire-ev]` lines and `reentry_lock`, `reentry_ctx` and `reentry_holder_irqs` from a PANIC-LOCK; plus `ipc_avg_us` and `ipc_iters`. 98 columns in all.
- B. A selected subset: B1's 28 columns plus the H3 and IPC inputs; the whole line covers the rest.
- C. B1's subset only, with no event or re-entry columns; H3 is read by hand from the logs.
- **Recommendation: A.** The ADR body says "each key"; selection needs judgement that every later step would revisit; the H3 rule as restated in the errata and either H1 candidate can then be computed mechanically.

**Q4. What happens to the R4 oracle parity tests once the output differs by design?**

- A. Keep the classifier differential against the oracle through the fold (synthetic and real logs). Re-bless the CLI, harness and classify goldens from aios. Delete the oracle recorders and the CLI and harness differentials (and their ambient-`timeout` plumbing).
- B. Keep every differential, normalising the class names and new columns back to the oracle's format.
- C. Retire the oracle entirely, classifier included.
- **Recommendation: A.** The fold differential is the step's own acceptance. B keeps a normaliser whose only job is to hide intended differences. C loses the proof that only the intended refinements changed.

**Q5. A boot printed `=== Gate 1 Complete ===`, but its IPC iteration count cannot be read (no IPC line, or one cut before `(N iters)`). What is it?**

- A. DEGRADED, with the detail "IPC iteration count unreadable". CLEAN requires a readable 10000, as the ADR says "CLEAN then requires 10000 iterations".
- B. CLEAN, with a detail note.
- C. A separate class.
- **Recommendation: A.** It never overstates CLEAN. In the 114 local logs with G1DONE, none lacked a readable count, so it should stay rare and visible.

**Q6. How does the harness enforce the load rule?**

(i) At the start of an interleaved soak, when load1 is above the host CPU count:

- refuse with exit 2 unless `--ignore-load` is given;
- or warn only;
- or wait until it drops.

(ii) "Arm means differ by more than 25%":

- |mean_A − mean_B| / min(mean_A, mean_B) > 0.25;
- or relative to the mean of the two.

The verdict is reported. The harness never stops a running soak over it.

- **Recommendation: refuse with an override, and the min-based ratio** (the stricter reading).

**Q7. What does the CI `workflow_dispatch` `runs` input mean?**

- A. Boots per soak (`just soak runs=`), default 5, with the soak step's time limit computed from it, capped so the job stays within a fixed maximum (for example 60 boots at 90 s). Several runs of one commit are several dispatches.
- B. The number of repeated 5-boot soaks, as a job matrix on separate runners.
- **Recommendation: A.** It matches `soak-matrix.yml`'s `runs` and the harness option. The ADR's "3 runs of 5 boots" is then three dispatches, or one dispatch of 15 boots if the owner accepts pooling.

**Q8. How many arms does the interleave mode take?**

- A. 2–4, as `soak-matrix.sh` does. The rate-settling soak in "Step 1b decisions" needs three kernels in one session.
- B. Exactly 2, as the ADR's "two ESP images" says.
- **Recommendation: A.** The rotation and pairwise tests generalise at no cost, and B would need a second mechanism for the rate-settling soak.

-----

## Dependencies & Risks

- **Depends on:**
  - the owner's answers to Q1–Q8 (given 2026-10-10);
  - R4's soak port on `main` (present);
  - the run-167 and B1 logs on this Mac (present, read-only);
  - the lead applying the two `.claude/` batches before T5's and T10's commits.
- **Golden churn hides an unintended change.** T3, T4 and T6 re-bless goldens (the class table twice: 8 rows in T3, 9 in T4). Mitigation: the fold differential runs on every commit through `base_line()`, which leaves `detail` and every other field oracle-identical; each re-bless is its own commit with a word-diff review; T2's refactor must change no golden.
- **Losing the CLI and harness oracle** (Q4 A) leaves those formats guarded only by aios-blessed goldens. They were ported byte for byte and verified in R4; from 1a on they evolve on purpose.
- **One harness boots old arms.** The harness's QEMU arguments, not an arm's justfile, are used. They are identical for af59149 and e211d6d. A future arm whose `run` recipe changes devices would be booted differently from its `just run`. `arms.tsv` records the arguments, and the developer guide says so.
- **The arm-base test override.** `AIOS_SOAK_MIN_ARM_BASE` could let a pre-#196 arm through. Mitigation: only debug-assertion builds read it (never `just tools`, so never `just soak` or CI's soak jobs), and an override is printed and recorded in Settings. A `cargo test --release -p aios-tools` would ignore it and the fake-arm scenarios would refuse their arms; the gate is the debug `cargo test`, as in CI.
- **`shared/` as a build input** makes the main checkout's `aios` stale after any `shared/` change, kernel-only ones included, so the shim rebuilds more often. That is the price of D9's single source of truth; the alternative is a binary that parses with an old catalogue.
- **Tripwire lines torn by unlocked UART output.** The `n` check rejects them, and the parser falls back to an older complete line. `tw_t` against `last_tick` shows the staleness. Under the contract a stale line is still correct data.
- **Schema drift.** A key added in `shared` changes the column set with no tools edit (D9). That is wanted, but it changes `summary.tsv`'s width; readers must go by header name (`brief.sh` uses positions 2 and 3, which never move). A new schema version shows as `tw_v` with `-` columns, never as a silent misparse.
- **Load-rule refusal blocks a soak on a busy Mac or runner.** The check runs before the builds (which raise the load themselves), the override flag exists, the refusal message names it, and `soak-matrix.yml` passes it.
- **A long CI soak holds a runner.** The `qemu-soak` job limit rises to 130 min to cover a 60-boot dispatch; push and pull_request runs still get the 5-boot step limit (14 min), so a hang there is cut as early as today.
- **Fisher precision.** The log-factorial sum is fine to n in the thousands; the exact u128 oracle covers up to 60.
- **`.claude/` permission prompts** (rule 08) for the shim, rule 01 and `CLAUDE.md`. They are batched into two lead-applied sets (before T5 and T10), so an unattended run can stall there.
- **Scope creep** (the audit-loop lesson): extras such as re-reporting an interleaved run from its logs, an `[smp]` parser or H1 verdict code stay out. Audit findings that ask for them go to the owner as descoping questions.
- **docs-check `repo-paths`.** It fails if a current-state doc still names `scripts/soak-matrix.sh` after T10. T10 updates those docs in the same commit.
- **The T12 smoke soak is host-exclusive.** It is attended and needs the owner's go-ahead. It stops only its own QEMU PID, never `pkill`.

-----

## Issues Encountered

- **Fable review of the plan, 2026-10-10.** Every finding was checked against the code before the plan changed; all hold. Where the code showed more than the finding said:
  - M1: besides `detail`, `lb` is computed from the class (`"-"` for CLEAN), so `base_line()` also needs `lb` from the base class; D1 defines both.
  - M2: the oracle plumbing in `soak_fake.rs` is wider than `Tool::Oracle` and `ambient_timeout`: `PRE_QEMU_TOOLS`/`pre_qemu_tools`, the oracle copy into each fake repository and `golden_text`'s `rename` parameter exist only for the oracle too, so D8 and T3 delete them.
  - S1: no developer-guide, README or `.claude/CLAUDE.md` sentence enumerates the aios build inputs (`rg` for the input list and for "provenance"/"stamp"); the only enumerations are the shim's (`:9–11`, `:198`, `:224–227`) and the justfile's (`:221`, `:228–231`, recipe comment). `tools/tests/shim.rs` also lists the inputs, so T5 updates it.
  - S7: a `--report-only` that is accepted and ignored with `--arm` would be a flag that does nothing; the plan refuses it instead, so the documented commands carry no such flag.
  - N4: besides `.github/`, a plain `rg` also skips `.claude/CLAUDE.md:243`, so the old check would have passed with that line stale.
- T3: A1 on run 167 printed exactly the expected matrix: WEDGE → WEDGE-STUCK 10 and WEDGE → WEDGE-ALIVE 2 (the 12 boots listed under A1), every other boot unchanged, no `base_line()` difference, and all 60 classes agree with their `summary.tsv` under the fold. The same test on B1 (text and gpu) shows PANIC → PANIC-LOCK 3 (text runs 05, 07, 11) and WEDGE → WEDGE-ALIVE 3, nothing else; `--classify` on B1 gives the A2 classes (DEGRADED aside, which is T4's).
- T3: the real-log test counted each run directory's `build.log` as a boot (62 logs for run 167's 60 boots, both INCONCLUSIVE in either classifier). It now skips `build.log`.
- T3: `scripts/soak-matrix.sh` (`KNOWN_CLASSES`, and its detail-or-first-fatal `case`) knows only the six old names. An arm at T3 or later shows its subclasses as extra classes, with `first_fatal` instead of `detail` for a WEDGE-STUCK or WEDGE-ALIVE row. Not changed: T10 deletes the script, and nothing runs it on this branch before then.
- T4: A1 on run 167 is unchanged from T3 (CLEAN → DEGRADED 0, no `base_line()` difference), and A2's classes hold on B1 (text CLEAN 11, WEDGE-ALIVE 3, PCZERO 1, EXCEPTION 2, PANIC-LOCK 3; gpu CLEAN 1, EXCEPTION 9; DEGRADED 0). The fold differential over every log under `target/soak/` (251 logs) shows no DEGRADED and no `base_line()` difference. The three local `(0 iters)` logs (`pr209-fix-198` runs 01 and 02, `baseline-212df62-text` run 04) are PCZERO and stay so.
- T5: the scratch run over B1's text logs shows `tw_src` `panic` for runs 05, 07 and 11 (all PANIC-LOCK), `exc` for the EXCEPTION and PCZERO boots, `hb` elsewhere; `[tripwire-ev]` counts give the 2 `kind=stuck` lines (runs 05 and 11) and no `kind=self`, as "Facts checked" says.
- T5: `proc::tests::suspend_stops_the_group_and_moves_the_limit_back` failed once with host load1 at 39.9 (a 500 ms limit under a 1 s suspend) and passed on rerun; `proc.rs` is untouched by step 1a. The gate run that counts passed in full.
- T6: A2 in full on the final T6 tree. `--classify --out` over B1's logs gives text CLEAN 11, WEDGE-ALIVE 3, PCZERO 1, EXCEPTION 2, PANIC-LOCK 3, gpu CLEAN 1, EXCEPTION 9, DEGRADED 0. The projection of `summary.tsv` to the research note's 28 non-class columns (an awk over the columns by header name, summing `elrmm` and `irqsw`, folding the class) diffs empty against the note's TSV for 30 of 30 rows. Every cell of the note's "Counters by class" table matches the new per-class table (WEDGE read as WEDGE-ALIVE, PANIC as PANIC-LOCK), and the rows the note leaves out (`lkph`, `lkself`, `orphan`, `n1`, `scanstall`, `misrep`, `wakefl`) are absent, as all-zero rows are. The table also shows what the note did not tabulate: `lkpho[TIMEOUT_QUEUE]` 2/2 in the text EXCEPTIONs and `nowaker_now` 3/1 (max) in the WEDGE-ALIVEs.
- T6: the harness goldens' `summary.md` normaliser matched the class cell with `[A-Z]+`, so a hyphenated class (PANIC-LOCK, WEDGE-*) kept its raw stall cell; no scenario had one before T6's `tripwire` scenario. It now matches `[A-Z-]+`.
- T6 (review fix): T6's `--out` support in `soak_cli.rs` stripped the case directory from the whole golden blob through `String::from_utf8_lossy`, for every case, so `cli/classify-all.golden`'s two `clip-multibyte` lines were re-blessed with U+FFFD in place of the raw clipped `\xC3` byte aios prints; aios was unchanged, only the comparison weakened, and the word-diff review missed it. `without_dir` now replaces the directory byte for byte (`replace_bytes`), a test pins that non-UTF-8 bytes pass through, and the re-blessed CLI goldens remove no line against the pre-T6 tree (`git diff 342a103^ -- tools/tests/golden/soak/cli` is additions only).
- T4: none of the 22 CLEAN corpus cases had an IPC line, so under Q5 A each would have become DEGRADED. They model healthy boots, which always print it, so each got the kernel's `(10000 iters)` line just before `=== Gate 1 Complete ===`, as did `hb-stopped` (CLEAN under the `classify-stall-kv` CLI scenario's `stall_secs=40`) and the fake QEMU's `CLEAN_TAIL`. The line matches none of the oracle's rules, so the fold differential still covers those cases unchanged.
- T8: no brief test existed (neither a shell test nor a `tools/tests` one), so `brief_lists_an_interleaved_run_once_and_never_as_main_soak` in `tools/tests/soak_interleave.rs` runs `scripts/agent/brief.sh --no-fetch` on a fixture repository (a failing fake `gh`, no `.claude/hooks/aios`, so no network and no docs-check).
- T8: the gates ran with the host load averages near 94 (1 min) and 128 (5 min), from other sessions; every test passed, the fake-QEMU scenarios included.
- T9: with the host's real load, every interleave scenario would have been refused on this Mac (load1 near 40-90 against 10 CPUs), and on CI the load and the 25% verdict in the goldens would vary; hence the `AIOS_SOAK_LOADAVG` override below. `proc::tests::suspend_stops_the_group_and_moves_the_limit_back` failed twice in parallel runs at load1 near 40 and passed alone three times; the full gate run passed.
- T9: the fake harness's per-boot-table normaliser (`| N | CLASS | ... |`, the stall cell) also matched the pair report's non-CLEAN rows (`| round | arm letter | class | tick |`) and masked their tick; it now requires a class name of two or more letters in the second cell. The single-mode harness goldens are unchanged.
- T9: the step-1b A/B command line parses: `aios soak --arm /nonexistent-a --arm /nonexistent-b runs=20 secs=75 --combine WEDGE-STUCK+PANIC-LOCK` reaches the preflight ("arm A: /nonexistent-a is not a directory", exit 2), and `--help` lists `--combine`.

## Decisions Made

- Owner, 2026-10-10 11:31: every recommendation in "Open questions" taken.
  - Q1 A: `aios soak --arm` replaces `scripts/soak-matrix.sh`, which is deleted; `soak-matrix.yml` keeps its trust model, makes the arm worktrees in YAML and runs `just soak --arm …`.
  - Q2 A: `aios-tools` depends on `shared` by path; rule 01's tools-dependency sentence is amended.
  - Q3 A: every tripwire key as a column, plus the line, the H3 event and re-entry inputs and the IPC figures, appended after the existing 22.
  - Q4 A: keep the fold differential against the oracle; re-bless the CLI, harness and classify goldens; delete the other differentials and the oracle recorders.
  - Q5 A: an unreadable IPC iteration count after Gate 1 is DEGRADED.
  - Q6: refuse above the CPU count unless `--ignore-load`; the 25% rule is |mean_A − mean_B| / min(mean_A, mean_B).
  - Q7 A: `runs` is boots per soak, default 5, with a derived and capped time limit.
  - Q8 A: 2–4 arms.
- Plan revision after the Fable review, 2026-10-10 (within the owner's answers; no new owner question):
  - Q5 A's "detail" is carried by `ipc_iters` and the report cells, not `detail`, which stays oracle-identical (M1).
  - Q3 A gains `tw_v` (the schema version, replacing D2's detail note) and `g1_line`: 100 columns, not 98 (M1, S5).
  - `Class::Degraded` lands in T4, and every oracle deletion of Q4 A in T3 (M2).
  - The arm base is a preflight parameter, overridable by `AIOS_SOAK_MIN_ARM_BASE` in debug-assertion builds only (M3).
  - `shared/` is a build input of the aios binary (S1); `.claude/` edits are lead-applied in two batches.
  - Interleave mode is report-only, with `--fail-on-regression` as the opt-in gate (S7); the load check runs before the builds (S2); the top-level `summary.md` carries a status and is rewritten after every boot (S3); 3 harness-error boots in a row stop the soak (S4); `brief.sh` reads the interleaved layout (S8).
  - CI `runs` defaults to 5 outside `workflow_dispatch`, is capped at 60, and the job limit is 130 min (S6).
- T2: `boot_once(arm, cfg, log, data, interrupts) -> Result<BootOutcome>` takes the log path and the data-disk path instead of the boot number `n`, so the caller names the log (`run-NN.log` now, `arm-X/run-NN.log` in T8) and owns the data disk. It makes the fresh data disk (with `cfg.fresh_data`), boots, appends the footer and returns `Booted(Boot { rc, elapsed, progress, load1, text })` or `Interrupted(code)`; `Boot::timing()` builds the `summary.tsv` timing fields. The first-boot stub check, classification and rows stay in the caller, since interleave mode applies its own harness-error rule (S4). `Arm::root` is set but not yet read in single mode; T8's per-arm builds read it.

- T3: `Class::base()` returns a separate `Base` enum (the oracle's six classes, with `name()`), because WEDGE has no `Class` variant. `report.rs` picks a boot's trailing text by `class.base()`, exhaustively, so a subclass prints exactly what its base class did. The scan's first fatal report is an `Option<Class>` (`PcZero`, `Exception` or `Panic`) instead of a string.
- T3: `reentry` is parsed by three regexes after `lock re-entry: ` in the joined first fatal line: the lock (`[A-Z][A-Z0-9_]*` plus an optional `[n]`), `ctx=[a-z-]+` and `holder_irqs=on|off`. Each field is an `Option`, so a message broken by another CPU's output keeps what it can read and nothing is guessed.
- T3: every WEDGE branch the plan names already had corpus cases (`nohb-boot-complete`, `nohb-boot-incomplete`, `nohb-kernel-never-started`, `tick0-after-bench`, `tick0-never-advanced`, `hb-stopped`, `bench-never-completed`, `bench-never-started`, `gpu-markers-missing`), so T3 added no WEDGE cases. It added 8 `lock-reentry-*` cases: 6 PANIC-LOCK (the B1 shapes, a per-CPU lock, before the first heartbeat, after a blank line, two CPUs, a message broken by other output), one after an EXCEPTION (stays EXCEPTION) and one after an earlier non-lock panic (stays PANIC). The corpus is 116 cases.
- T3: the fake harness's `Tool` enum went with `Tool::Oracle`, since `Aios` was its only variant left; `run_scenario` takes the scenario alone. The `no-qemu` golden is unchanged with the fake `bin` as its whole PATH.
- T3: `brief.sh` prints PANIC-LOCK, WEDGE-STUCK and WEDGE-ALIVE counts, plus a WEDGE count only when a run from before 1a records one.
- T4: the IPC figures are `Classification::ipc: Ipc { avg_us, iters }`, from the first line holding `[bench] IPC round-trip (same core): `: `avg=` needs its ` us` after the digits, the count is ` (N iters)` anywhere after the prefix, and a number that does not fit `u64` is unreadable. CLEAN requires `iters == Some(IPC_ITERATIONS)` exactly (never overstates CLEAN, Q5's reasoning); the drift test parses `const IPC_ITERATIONS: usize = …;` from `kernel/src/bench.rs` and compares it with the classifier's constant instead of a literal, so the assertion follows the constant.
- T4 (deviation, smallest one keeping D1): where the console line and the per-boot table print a CLEAN boot's `detail`, a DEGRADED boot prints `IPC <N> iters` or `IPC iteration count unreadable` and then, unless the detail is `-`, `; <detail>`, instead of the IPC text alone. The detail of a CLEAN-base boot carries notes such as "qemu exited before the time limit" and the log-only note, which the IPC text alone would hide; `detail` itself is still the oracle's.
- T4: `summary.tsv` gains `ipc_avg_us` and `ipc_iters` after `log` (24 columns). The fake harness's `summary.tsv` normaliser keyed on the literal 22; it now counts `report::TSV_HEADER`'s columns, pulled forward from T6 because the timing columns would otherwise reach the goldens unnormalised.
- T4: corpus cases `ipc-10000-iters-clean`, `ipc-0-iters-gate1-fail-degraded`, `ipc-9999-iters-degraded`, `ipc-no-line-degraded`, `ipc-line-cut-degraded`, `ipc-0-iters-log-only-degraded`, `ipc-first-line-decides-degraded` (a guest that booted twice: the first line's 0 decides) and `ipc-0-iters-pczero-stays-fatal`: 6 DEGRADED, corpus 124 cases.
- T5: `Classification` carries `tripwire: LastLines { last, g1 }` and `events: EventCounts` (D1 named three fields; `g1` lives inside `LastLines`, as the T5 entry names the type). A `Line` keeps the whole text from `[tripwire]` on (trailing blanks dropped), its `v=` and `src=` values (`None` when absent) and, for `v=1` only, a `V1` with `cpu`, `t`, `ncpu` and one value per `Key::ALL` entry (`0` when missing). A `v=1` key the catalogue does not know is left to the whole line. "key=value" means both sides non-empty, and `n=` must be all digits. The `src=g1` check compares with `LineSrc::G1.name()` and the IRQ contexts with `Ctx::Irq`/`Ctx::IrqExit` names from `shared`; the `[tripwire-ev]` kinds (`ph`, `self`, `stuck`) are the kernel's private `EventKind`, so they are literals with a pointer to it.
- T5: each `[tripwire-ev] ` in a line starts its own event (two CPUs' events on one line count twice); an event counts only when its first token is a known `kind=`, and `self` counts as IRQ context only from its `ctx=` token, so a torn line is never guessed.
- T5: the `GOLDEN_FULL` and `GOLDEN_NONZERO` fixtures are read from `shared/src/tripwire.rs` at test time by a small Rust string-literal decoder (line continuations, `\n`, `\\`, `\"`; any other escape fails the test), the way `oracle_awk()` extracts the heredoc. The unit tests live in `tripwire.rs`, which reads the file through `CARGO_MANIFEST_DIR` as `docs_check/model.rs` does.
- T4 (review fix): the `just soak` recipe comment in `justfile` still listed the 8 T3 classes. It now lists DEGRADED too, and `class_lists_match_class_all` (`tools/tests/soak_classify.rs`) asserts that this comment and the developer guide's `just soak` row both list exactly `Class::ALL`, in order, so a later class cannot leave either behind.

- T6 (deviation, arithmetic): the plan's "22 + 39 + `Key::COUNT`" counts the old 22 twice. The new non-key columns are 17 (the 2 IPC columns from T4, then 3 re-entry, 4 event, 5 `tw_` prefix, `g1_elrmm`, `g1_line`, `tw_line`), so `summary.tsv` has 22 + 17 + 61 = 100 columns, the plan's own total. `report.rs` names them in three lists (`SCRIPT_COLUMNS`, `STEP_1A_COLUMNS` of 14 ahead of the keys, `LINE_COLUMNS` of 3 after them); the header is built at run time because the `tw_<key>` names come from `Key::ALL`, and the test asserts 22 + 14 + `Key::COUNT` + 3.
- T6 (deviation, smallest one keeping D4 and A2): a per-CPU key gets a row without an index that sums its CPUs (max for a gauge), then the `key[CPU n]` rows D4 names. The note's table, which A2 compares cell by cell, sums `elrmm` and `irqsw` over CPUs, and "boots non-zero" of a sum is not derivable from per-CPU rows.
- T6: `shared` has no names or list for `SchedulerClass`, and step 1a changes no `shared` code, so the table names `starved` indices by the enum's derived `Debug` (`Normal`) over a 4-entry list in `SchedulerClass as usize` order, which a test pins. Past the end of an index set (a later kernel), a row is labelled with the bare number.
- T6: one writer. `report::Tally` gathers what `summary.md` needs per boot; `summary_head(info, &tally)` (now ending with the Gate 1 IPC line, so it is on stdout too) and `summary_tail(&tally)` (the tripwire table, then the per-boot table with its new `Tripwire` column) build the file; `runner::write_summary` writes it and prints the head, and `runner::fresh_out_dir` holds the new-or-empty check. The soak and `--classify --out` both call them. `BootTiming`'s fields are `Option`s, and `runner::Footer` reads them back from a log's `[soak] meta` line; `SummaryInfo::fresh_data` is an `Option` (`data disk -` for logs).
- T6: `--classify --out` numbers the boots in the order given, puts each path as given in `log`, takes `mode`, `--secs` and the stall limit from the footers when they agree (`mixed` when they differ, `-` when none has it; `--stall-secs` wins), and names the logs' directory in the Logs row when they share one (`-` otherwise). The harness test compares the whole `summary.tsv` of a fake soak with `--classify --out` over its logs, timing columns included (they come from the footers), plus both `summary.md` tables, which is stronger than the plan's "apart from the timing columns".
- T6: a tab inside a stored whole line (`g1_line`, `tw_line`) becomes a space, since the parser splits tokens at blanks and tabs alike and a raw tab would break the row. A table value that is not a decimal `u64` counts as 0 there; `summary.tsv` keeps it as printed.
- T6: three corpus cases with tripwire output (`tripwire-panic-lock-with-events`, `tripwire-clean-g1-then-hb` with a torn last line, `tripwire-other-schema-last-wedge-alive`), so the fold differential also covers logs with `[tripwire]` and `[tripwire-ev]` lines and the `--classify --out` CLI golden shows populated columns; corpus 127 cases. A fake-QEMU `tripwire` scenario (a PANIC-LOCK boot, then a CLEAN one with `g1` and `hb` lines) gives the harness golden the same.
- T7: `fisher_low`, `fisher_high` and `fisher_two` take the table as `(a, b, c, d)` = `[[a, b], [c, d]]`, `soak-matrix.sh`'s argument order, with one row per arm and the class (or group) count first; T9 puts the new arm in row 1, so the regression guard and "removed" are `fisher_low` and "new" is `fisher_high`. The log-factorials are built per call up to the table's n (a few dozen `ln`s per test at soak sizes). The exact oracle's two-sided p compares the integer weights C(r1, x)·C(r2, c1 − x) with no tolerance; it agrees with the float version's R tolerance within 1e−9 on all 246,016 tables up to 30 + 30, so no tie within 1e−7 that is not exact occurs there. The developer guide's 6/20 vs 18/20 figure is 0.000244 (2.444e−4), pinned to six decimals.
- T8 (deviation, smallest one keeping D5): each arm's `just --evaluate edk2_fw` is read and compared in the preflight, before any build, instead of at step 5 after the build, so a firmware mismatch is refused without building; `disk_img` and `kernel_elf` are still read after the build. The `rustc --version` check stays after each arm's build (step 4), so with `--no-build` a rustc refusal leaves the output directory with empty `arm-X/` directories, as a single-mode setup error leaves `build.log`.
- T8: the top-level `summary.md` carries D4's sections 0 to 3 (status, settings, arms, classes per arm with each arm's CLEAN rate over conclusive boots); the load verdict, pair tests, Gate 1 IPC, tripwire and non-CLEAN sections are T9's `pair.rs`. Settings already records load1 (all three averages) before and after the builds and at the end; T9 adds the refusal.
- T8: `--report-only` with `--arm` is refused with "an interleaved soak is always report-only (it exits 0 whatever the classes)"; T9 names `--fail-on-regression` in it when it adds the flag, so no message names a flag that does not exist yet. Also usage errors, so no flag silently does nothing: `--reuse-data` with `--arm` (D5: every boot gets a fresh data disk), `--allow-mixed-toolchains` without `--arm`, `--arm` with `--classify`, and an empty `arm=`.
- T8: QEMU and the firmware are rechecked before and after every boot. QEMU is the path `PATH` resolves `qemu-system-aarch64` to at the preflight, canonicalised; every interleaved boot runs that path (`Arm::qemu`; single mode keeps running `qemu-system-aarch64` from `PATH`), and a change is a different version line or sha256. A change found after a boot stops the soak (exit 2) without counting that boot: it is in neither `summary.tsv` nor `boots.tsv`, and its `run-NN.log` stays.
- T8: an arm's first boot without the stub line writes the status "stopped (the UEFI stub never ran on arm X's first boot)" and exits 2 with single mode's message, and, as in single mode, that boot is not recorded. Later stub-less boots are recorded (INCONCLUSIVE) and counted toward the S4 streak.
- T8: an A/A control builds and snapshots its checkout once (`esp-A.img`, `arm-A/build.log` only); both labels boot that snapshot. Two `--arm` directories inside one checkout are the same arm checkout. `boots.tsv`'s `log` column is `arm-X/run-NN.log` (relative to the top level), where each arm's `summary.tsv` keeps `run-NN.log`. `arms.tsv` columns: `arm checkout root commit channel rustc kernel firmware qemu qemu_args`.
- T8: the Settings "Harness" row is the git rev (with `-dirty`) of the checkout that holds the running `aios` binary, and the binary's path. The arm-base override prints a stderr warning and names both SHAs in Settings.
- T8: shared setup in `runner.rs`: `build_esp` (appends to `build.log`, takes the variables to unset), `snapshot_esp`, `esp_kernel_sha` and `write_summary_file`; `host.rs` gains `sha256`, `qemu_version`, `toolchain_channel`, `rustc_version` and `contains_commit`. The single-mode goldens are unchanged.
- T8: the fake environment: one fake QEMU picks `boot-N.sh`, else `boot-<arm>.sh` for the arm its ESP image names (`ESP image arm=<name>`, written by the fake `just disk` from the checkout's `.fake-arm`), else `boot.sh`; it copies the top-level `summary.md` to `summary-before-N.md` (the S3 check) and changes its own file under `qemu-changes-at-N`. The fake repository's commits have fixed dates, so the goldens' commit ids and `AIOS_SOAK_MIN_ARM_BASE` are the same on every run. 24 interleave goldens under `tools/tests/golden/soak/interleave/`.
- T8 (review fix): the `--allow-mixed-toolchains needs --arm` check now runs before the `--classify` branch, so `--classify --allow-mixed-toolchains` is refused too instead of being ignored; `arm_usage_errors` covers it. In a plain soak it now precedes the `--runs`, `--secs` and `--mode` checks (only `--stall-secs` comes first); no golden passes the flag without `--arm`.
- T8 (review fix 2): a QEMU or firmware probe (`sha256sum`, `qemu --version`) that cannot run while a signal is pending is the signal, not a change. A terminal Ctrl-C reaches the probe's child too, so before, a Ctrl-C during the probe after a boot wrote "stopped (the QEMU binary changed)" and dropped that boot. Now `Fixed::changed` returns no change for a failed probe with a signal pending; the probe before a boot is followed by a pending check, and after a boot the boot is counted and the next pending check writes "stopped (SIGINT)". A probe that ran and disagrees is still a change, signal or not. `sigint_during_a_probe_after_a_boot_is_the_signal_not_a_change` covers it with a fake `sha256sum` that interrupts once after boot 1 (as single mode's `interrupt-sha256`).

- T9 (deviation, test hook): the load is pinned in tests by `AIOS_SOAK_LOADAVG` (`host::LOADAVG_VAR`), which replaces `host::loadavg()` (so the check, the Settings loads and every per-boot `load1`) in debug-assertion builds only, as `AIOS_SOAK_MIN_ARM_BASE` does (D5, M3). Linux reads `/proc/loadavg` directly, so a fake `sysctl` or `getconf` on `PATH` cannot pin it. A soak that reads it warns on stderr (both modes), and the interleaved Settings "Load average" row names it; release builds (`just soak`, CI) never read it.
- T9: the load check runs in the preflight, after the arm, channel, firmware and QEMU checks and before the output directory is made, so a refusal leaves no directory and runs no `rustup` or `just disk`; that reading is the "before the builds" load. load1 equal to the CPU count passes ("above" is strict). An unreadable load or CPU count is not refused: a stderr warning, and Settings' new "Load check" row says "not checked"; the row also records "passed" or "skipped (--ignore-load)" (with "above the host's CPU count" when it was).
- T9: pairs are every two arms (i < j), the earlier one previous, as `soak-matrix.sh` paired them; D4's "earlier arm = previous" for 3-4 arms. The T8 `USAGE` sentence "A is the previous arm of every pair" is reworded to match. The Bonferroni note gives 0.05/pairs.
- T9: the regression guard and both markers use `pair::significant(p)`, p < 0.05·(1 − 1e−9): a table such as 3/3 against 0/3 has p = 1/20 exactly, which the log-factorial sum can put a hair below 0.05; a test pins that it passes. INCONCLUSIVE has no row in the pair tables (it is out of every denominator) and `--combine` refuses it, an unknown class, a class named twice, or a single class. p values print as C's `%.3g` (`soak-matrix.sh`'s format). The rate difference is in percentage points, rounded, `0 pp` without a sign. An arm without conclusive boots makes its pair's tests `n/a`, and an `n/a` guard does not count as failing for `--fail-on-regression`.
- T9: `--combine` (also `--combine=`, `combine=`), `--ignore-load` and `--fail-on-regression` without `--arm` are usage errors ("<option> needs --arm"), like `--allow-mixed-toolchains`, so no flag is accepted and ignored. `--report-only` with `--arm` now names `--fail-on-regression` in its refusal. With `--fail-on-regression`, a failed guard prints "soak: a pair's regression guard failed (--fail-on-regression): exit 1" on stdout after the report paths; every finished interleaved soak prints one console line per pair (guard verdict and mean-load verdict).
- T9: the per-arm tripwire table reuses the single-run one: `report::counter_table(columns, boots)` takes any column grouping (classes for `summary.md`, arms for the pair report), and `report::tripwire_table` now calls it; its output is unchanged (pinned test). `report::boot_text` (the per-boot table's fatal-line-or-detail cell) and `report::tripwire_src` are shared with the non-CLEAN table. The R16 count treats a boot as having a fatal report when its class's base is PCZERO, PANIC or EXCEPTION, and counts every non-CLEAN class (DEGRADED included) with no complete tripwire line of any version.
- T9: the pair report is built from `pair::Boot` records kept in memory in boot order (the same values `boots.tsv` gets), not by re-reading `boots.tsv`, as T8's class counts are. Four scenarios added: `refuse-load` (exit 2, no rustup, no `just disk`, no output directory), `ignore-load`, `fail-on-regression-fails` (4 CLEAN boots with tripwire lines against 4 PANICs, p = 1/70, exit 1, with a `--combine` row and the per-arm tripwire table) and `fail-on-regression-passes`; 28 interleave goldens.
- T9 (review fix, refines D4 section 7 and the R16 bullet above): the R16 count leaves INCONCLUSIVE boots out, as every other test in the report does. Their causes (the stub never ran, QEMU killed by a signal, the boot cut short) carry no kernel result and can carry neither a tripwire line nor a fatal report, so counting them would charge a harness error (S4 records up to two stub-less boots per streak) to step 1b's kernel. The column header reads "Conclusive non-CLEAN boots ...", the note and `USAGE` say INCONCLUSIVE boots are never counted, and `the_r16_count_and_its_label` has an uncounted INCONCLUSIVE boot.

## Lessons Learned

(to be filled during implementation)
