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
- `kernel/src/bench.rs:26`: `const IPC_ITERATIONS: usize = 10_000;`.
- Both arms of step 1b's A/B soak (af59149, e211d6d) pin `nightly-2026-10-09`, and both contain 7167d40 (#196). Between them, the justfile and the soak code differ only in a 5-line change to `classify.rs`.
- `just docs-check` on the branch before this plan: no new drift.

-----

## Scope: every ADR requirement, and where it lands

Sources: "Order and why" row 1a; "Soak protocol (every soaked step)"; "Step 1a — harness only"; "Rules for every step PR"; the "Pending acceptance soak" part of "Step 1b decisions, 2026-10-10"; and the 2026-10-09 errata ("PANIC-LOCK is two lines", "Re-entry is decided in every context", "The H1 rule cannot be applied as written", "`scripts/soak-qemu.sh` is gone").

| # | Requirement (ADR wording, shortened) | Where it lands | Task |
|---|---|---|---|
| R1 | WEDGE-STUCK: the heartbeat is stuck or stopped | `classify.rs`: a `Class` enum. Mapped from today's WEDGE branches: "no heartbeat …" (all three variants), "heartbeat stuck at tick 0 …", "heartbeat never advanced past tick 0" and "heartbeat stopped at tick N …" become WEDGE-STUCK | T3 |
| R2 | WEDGE-ALIVE: the heartbeat is alive, the bench incomplete | Same place: "heartbeat alive but the Gate 1 bench never completed/started". "gpu markers missing" is counted as WEDGE-ALIVE, as the 2026-10-06 note rules; its detail keeps the text, so the PR can name it | T3 |
| R3 | DEGRADED: `=== Gate 1 Complete ===` printed, but the IPC line reports fewer than `(10000 iters)`; CLEAN then requires 10000 | `classify.rs`: parse the first `[bench] IPC round-trip (same core): avg=… (N iters)` line. The threshold is a constant, with a drift test against `kernel/src/bench.rs` `IPC_ITERATIONS`. An unreadable count is owner question Q5 | T4 |
| R4 | PANIC-LOCK: a PANIC whose joined `first_fatal` contains `lock re-entry:` (the erratum makes it two lines) | `classify.rs`: refines PANIC after the scan, with the same earliest-report rule. The `ctx=`, `holder_irqs=` and lock name are parsed from the joined text into columns (Q3) | T3 |
| R5 | Tripwire line: the last value of each key in `summary.tsv` columns, reported per arm. Parser contract: the last complete line, `src=panic`/`exc` allowed; the whole line stored beside the columns; the `full_line_golden` Full line is the fixture | New `tools/src/cmd/soak/tripwire.rs` (parser and columns), used by the `classify.rs` scan. Columns in `report.rs`. The fixture is the `GOLDEN_FULL` literal, extracted from `shared/src/tripwire.rs` by the test at run time, the way `oracle_awk()` extracts the awk heredoc (no copy) | T5, T6 |
| R6 | Fisher one-sided tests in the report: the regression guard (CLEAN lower in the new arm, p < 0.05), "class removed" (0 in the new arm and p < 0.05), raw differences recorded but never gated on | New `tools/src/cmd/soak/stats.rs` (hypergeometric with log-factorials, no new crate), plus the pair report in `pair.rs` | T7, T9 |
| R7 | Per-arm load mean and max; redo the pair if the arm means differ by more than 25%; do not start while load1 is above the CPU count | `pair.rs` (report and verdict); the start check in the interleave runner (Q6) | T9 |
| R8 | Interleave mode: alternate two ESP images per boot in one host session | New `tools/src/cmd/soak/interleave.rs`, on top of a `runner.rs` refactor (`boot_once`, `Arm`). CLI: `--arm DIR`, repeatable (Q1, Q8) | T2, T8 |
| R9 | Refuse a pair whose arms differ in QEMU version, firmware or toolchain channel, plus compiler (the 2026-10-06 note adds `rustc --version`) | `interleave.rs` preflight. QEMU: one binary for all arms, with its version line and sha256 rechecked before every boot (a mid-soak change stops the soak). Firmware: each arm's `just --evaluate edk2_fw` must name one absolute path, and its sha256 is rechecked per boot. Channel: each arm's `rust-toolchain.toml`. Compiler: `rustc --version` in each arm after its build. `--allow-mixed-toolchains` overrides the last two and marks the summary (soak-matrix behaviour, kept for toolchain-bump pairs such as #161's) | T8 |
| R10 | A `timeout` check that accepts ubuntu-26.04's `timeout` | Already delivered; no code (see "The `timeout` item" below) | — |
| R11 | A `runs` input for the CI `workflow_dispatch` (`ci.yml:8` has none), so one commit can be soaked several times | `.github/workflows/ci.yml` `qemu-soak` job: input passed through `env`, never `${{ }}` inside a script; step time limit computed from it (Q7) | T10 |
| R12 | Each step PR passes rule 02's gates and `/audit-loop`, and updates the docs it makes stale | T11, T12 | T11, T12 |
| R13 | Acceptance: `--classify` on the run-167 logs reproduces every boot's class except the WEDGE split and CLEAN→DEGRADED; no boots needed | The fold differential (T3) and the real-log check below | T3, T12 |
| R14 (1b pending soak) | Step 1b's A/B: interleaved 20 text and 10 gpu boots per arm, 1b against af59149, one QEMU, one firmware and one toolchain; H3 per the restated rule; PANIC-LOCK as in R4; per-boot last complete line plus the whole line | Delivered by R1–R9. The H3 rule needs PANIC-LOCK `ctx`/`holder_irqs` and `[tripwire-ev] kind=self` counts by `ctx` (Q3). H1 waits for the owner's restatement, and both candidates ("only mismatches before the `src=g1` line" and "`elrmm` > 1, leaving out boots whose first fatal report is a PANIC") need the `src=g1` line's values, so that line is stored too (Q3) | T5, T6 |
| R15 | The step-4 Gate 1 comparison: the mean of per-boot IPC averages over each arm's CLEAN boots, per mode | `ipc_avg_us` column (T4); per-arm mean in the pair report (T9) | T4, T9 |
| R16 | Step 1b's "every non-CLEAN boot has a tripwire line or fatal dump" | `tw_src` column; the pair report counts non-CLEAN boots with neither | T6, T9 |

**The `timeout` item (R10) is moot.**

- #192 (aa1f128, merged 2026-09-24) made `soak-qemu.sh` accept uutils `timeout`. The ADR's #169 bullet under "Other PRs" already says it "delivers step 1a's `timeout` item".
- R4 (#230, 1a5c363) deleted `soak-qemu.sh`. `aios soak` stops QEMU itself (`tools/src/proc.rs` `Supervisor`: a process group, SIGTERM at the limit, SIGKILL 10 s later, statuses 124 and 137). Nothing under `tools/src` runs `timeout`, and developer-guide §5.6 says "No external `timeout` is needed".
- CI run 38022214523 (main e211d6d, 2026-10-10): the `QEMU boot soak (report-only)` job ran on `ubuntu-26.04`, and its "Soak (5 boots x 90 s)" step succeeded. A setup failure turns that step red, as on #169.
- `timeout` has two users left: `scripts/soak-matrix.sh` `find_timeout` (only for pre-R4 `soak-qemu.sh` arms) and the oracle harness recordings (`soak_fake.rs` symlinks the ambient `timeout`). Both go if Q1 and Q4 are answered as recommended. Step 1a records the item as delivered and changes no code for it.

**Non-goals.**

- No kernel or `shared/` change.
- No decision rule for H1; the owner restates it. 1a only stores what either candidate needs.
- No re-report of a finished interleaved run from its logs, beyond `--classify --out` for one group of logs (T6).
- No `[smp]` line parsing.

-----

## Acceptance

### A1. Run 167 (the ADR's step-1a acceptance)

The mechanism is a fold. Every new class has a base class: WEDGE-STUCK and WEDGE-ALIVE fold to WEDGE, PANIC-LOCK to PANIC, DEGRADED to CLEAN, and every other class to itself. The acceptance then reads: for every log, `fold(new class)` equals the oracle's class, and every other classifier field is byte-identical. T3 turns the existing `classify_differential_on_real_logs` (ignored, `AIOS_SOAK_REAL_LOGS`) into that check. It also prints a transition matrix (oracle class → new class, with counts) and the per-boot list of every boot whose class changed.

```sh
AIOS_SOAK_REAL_LOGS=/Users/juslee/dev/aios/target/soak/167 \
  cargo test -p aios-tools --test soak_classify -- --ignored classify_differential_on_real_logs --nocapture
```

Expected:

- WEDGE → WEDGE-STUCK: 10. main text r1/01, r1/02, r2/04, r3/02 and r4/04; main gpu r2/04; #161 text r1/01, r1/05, r2/02 and r3/01.
- WEDGE → WEDGE-ALIVE: 2. main gpu r1/05 and #161 text r4/02.
- CLEAN → DEGRADED: 0. Every CLEAN boot prints `(10000 iters)`.
- PANIC → PANIC-LOCK: 0.
- Every other boot keeps its class.

That matches the ADR's "Soak evidence" table (main text 5/0, main gpu 1/1, #161 text 4/1). The test fails on any other transition or any other field difference. The PR body carries the matrix and the 12-row list from `--nocapture`.

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

Every code commit passes:

- `cargo fmt --check -p aios-tools`
- `cargo clippy -p aios-tools --all-targets -- -D warnings`
- `cargo test -p aios-tools`
- `just check`
- `just test` (unchanged; `shared/` is not touched)
- `cargo run -q -p aios-tools -- docs-check`: no new drift except this plan's own knowledge-hygiene finding (a non-empty `plans/`), which goes when the plan is distilled

The `.claude/hooks/aios` shim runs the main checkout's build, so on the branch docs-check always runs through `cargo run`. `cargo objdump` and `just run` do not apply: no kernel change. Before the PR, one attended smoke soak (T12) exercises the interleave runner on real QEMU.

-----

## Design decisions

**D1. Class model.** `Classification::class` becomes `Class`, an enum: `PcZero`, `PanicLock`, `Panic`, `Exception`, `WedgeStuck`, `WedgeAlive`, `Inconclusive`, `Degraded`, `Clean`.

- `Class::ALL` is the summary order, with subclasses next to their base: PCZERO, PANIC-LOCK, PANIC, EXCEPTION, WEDGE-STUCK, WEDGE-ALIVE, INCONCLUSIVE, DEGRADED, CLEAN.
- `name()` is the printed name, and `base()` the oracle class.
- Precedence is unchanged; the subclasses only refine within a base class.
- DEGRADED is decided last, at the point where today's code returns CLEAN. So it can never mask a fatal report, a WEDGE or an INCONCLUSIVE.
- `Classification::line()` keeps its 11 fields, with the new name in field 1. `base_line()` gives the oracle's line, which the fold differential compares.
- The new data rides outside `line()`: `ipc` (avg in µs and iterations), `tripwire` (the last complete line), `g1` (the last complete `src=g1` line), `events` (the `[tripwire-ev]` counts) and `reentry` (`ctx`, `holder_irqs` and the lock, for PANIC-LOCK).
- The enum replaces the stringly `&'static str` class, so every `match` on a class is exhaustive. That costs nothing, since the oracle parity now goes through `base()`.

**D2. Tripwire parsing.** One pass, inside the existing `Scan::line`.

- A line contributes from its first `[tripwire] ` (a line printed after another CPU's partial output starts mid-line; `[tripwire-ev]` never matches).
- It is complete when every token after the prefix is `key=value`, the last is `n=N`, and N equals the token count before it.
- It is kept only if `v=1`. Another schema version is kept as raw text, with a detail note and every key column `-`.
- The last complete line wins, and so does the last complete `src=g1` line.
- A key missing from a kept line is 0, which matters for `NonZero` heartbeat lines. A log with no complete line has `-` in every tripwire column, not 0: no line is not the same as zero counts.
- Value counts are not validated against key widths. The contract does not require it, and a check could only turn data into `-`.

**D3. Columns** (exact set: Q3). New `summary.tsv` columns are appended after the existing 22, so positional readers (`scripts/agent/brief.sh` reads `$2` and `$3`) keep working, and the whole line goes last.

- `ipc_avg_us` and `ipc_iters`
- `reentry_lock`, `reentry_ctx` and `reentry_holder_irqs`
- `ev_ph`, `ev_self`, `ev_self_irq` (`kind=self` with `ctx=irq` or `irq-exit`) and `ev_stuck`
- `tw_src`, `tw_cpu`, `tw_t` and `tw_ncpu`
- one `tw_<key>` column per key, in `Key::ALL` order, holding the comma list as printed (a missing key is `0`)
- `g1_elrmm` (the `src=g1` line's `elrmm`, for H1 candidate 1)
- `tw_line`, the whole last complete line

`tw_t` beside `last_tick` shows a stale line, one whose CPU 0 stopped long before the end. The key list comes from `shared::tripwire::Key::ALL` (Q2). A test fails when the catalogue and the parsed `GOLDEN_FULL` disagree in order or names.

**D4. Report layout.**

- **Single run** (`summary.md`, unchanged except as follows):
  - the class table lists the 9 classes;
  - a "Gate 1 IPC" line gives the mean of `ipc_avg_us` over CLEAN boots;
  - a "Tripwire counters by class" table follows (B1's format: rows are `key[index name]` with a non-zero value in some boot, columns are the classes present, cells are "boots non-zero / sum", or "/ max" for gauges);
  - the per-boot table gains a `tw_src` column.
  - Index names come from `shared` (`WakeSource`, `LockClass`, `SchedulerClass`, `N2Kind`, `BadchanSite`; CPU n for per-CPU keys).
- **Interleaved run** (top-level `summary.md`):
  1. **Settings:** design, rotation, progress, QEMU version and sha256, firmware and sha256, host, load at start and end, and the harness's own commit.
  2. **Arms:** label, checkout, commit (`-dirty`), channel, rustc and kernel ELF sha256.
  3. **Classes per arm:** the 9 classes, Total, and the CLEAN rate with its Wilson interval over conclusive boots.
  4. **Load:** per-arm load1 mean and max. The 25% verdict reads "redo the pair" when it fails.
  5. **Pair tests,** one block per pair (earlier arm = previous, later arm = new):
     - the regression guard: CLEAN counts, one-sided p toward the new arm being lower, verdict `fails` or `passes`;
     - one row per class, plus each `--combine` group: counts, one-sided p (fewer in new), one-sided p (more in new), two-sided p, and "removed" (0 in new and one-sided p < 0.05);
     - raw differences beside the p values;
     - a Bonferroni note when there is more than one pair.
  6. **Gate 1 IPC:** per arm, the mean of `ipc_avg_us` over CLEAN boots, with n, and the `G1PASS` count.
  7. **Tripwire per arm:** as above, with arms as columns. The count of non-CLEAN boots with neither a complete tripwire line nor a fatal report is R16's check.
  8. **Non-CLEAN boots:** round, arm, class, last tick, first fatal line or detail, `tw_src` and log.

  Fisher runs on conclusive boots only, as `soak-matrix.sh` does.
- **Step 1b's combined test.** It needs WEDGE-STUCK + PANIC-LOCK compared two-sided. `main` has no lock detector, so its PANIC-LOCK count is 0 and the combined count equals its WEDGE-STUCK. So a generic `--combine CLASS+CLASS` row computes exactly the ADR's test: `--combine WEDGE-STUCK+PANIC-LOCK`. No step-specific code.

**D5. Interleave CLI and layout** (Q1, Q8).

- `aios soak --arm DIR --arm DIR [--arm DIR [--arm DIR]] [options]`, with an `arm=DIR` alias for `just soak`.
- Each DIR is a git checkout (a worktree is fine). Labels are A–D in the order given; the first is "previous" in every pair.
- The same DIR twice is an A/A control, built once.
- Without `--arm` nothing changes. A single `--arm` is a usage error.
- Per arm (skipped with `--no-build` except for the ESP checks):
  - `rustup toolchain install --no-self-update` in DIR (rustup reads DIR's `rust-toolchain.toml`, as `soak-matrix.sh` does);
  - `just disk` in DIR, with `RUSTUP_TOOLCHAIN` and `CARGO_TARGET_DIR` unset;
  - `just --evaluate disk_img`, `kernel_elf` and `edk2_fw` read in DIR;
  - the ESP snapshot to `.scratch.*/esp-X.img`, and the kernel ELF sha256 from that snapshot;
  - the git rev, with `-dirty`;
  - a refusal if DIR lacks 7167d40 (#196; strict-NX firmware faults every older kernel).
- One harness, the running `aios`, boots and classifies every arm with its own QEMU arguments. An arm's justfile `run` recipe is not used, the same as single mode today.
- Rounds rotate the arm order by one each round (A B, B A, A B, …).
- Each boot gets a fresh data disk.
- Output, default `target/soak/<timestamp>-<mode>-arms`: under `target/soak/`, so `/merge-and-cleanup` copies it and `/justin:brief` sees it.
  - `arm-X/run-NN.log`, `summary.tsv`, `summary.md` and `build.log` per arm: each is a normal single-run directory, so every single-run reader works on it;
  - top-level `summary.md` (the pair report), `boots.tsv` (round, position, arm, then the `summary.tsv` columns) and `arms.tsv`.
- Signals, exit statuses and the first-boot setup check behave as in single mode. The first-boot check applies to each arm's first boot.

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

**D7. `scripts/soak-matrix.sh` and its workflow** (Q1). Recommended: the interleave mode replaces the script, which is deleted in T10, under the owner's no-legacy rule.

- Two interleave implementations would be the duplicate that rule forbids.
- The script cannot serve step 1b's A/B anyway: it classifies each arm with that arm's own harness, and both arms predate 1a, so the subclasses and columns would be missing. That is why the 2026-10-06 note classifies by hand.
- `.github/workflows/soak-matrix.yml` keeps its name, inputs and trust model: full 40-hex SHAs only, this repository's PR heads fetched, no `rust-cache`, the computed step limit.
- Its soak step changes:
  - each arm becomes `git worktree add --detach "$RUNNER_TEMP/arm-$L" "$sha"`;
  - the harness is built from the dispatching ref (`just tools`);
  - the step runs `just soak --arm … runs= secs= mode= report_only=1 out="$MATRIX_OUT"`.
- Every arm is then classified by the dispatching ref's classifier: the intended change, and the summary records the harness commit.
- `soak-matrix.sh` features that go:
  - pre-R4 arms booted by `soak-qemu.sh`, and the `timeout` probe with them. Every arm is now booted by one harness, and any arm with a 7167d40-or-later ESP boots;
  - ref resolution inside the tool. Local users pass checkouts, and CI creates the worktrees in YAML.

**D8. Oracle parity tests after the split** (Q4). Recommended:

- Keep the classifier differential (synthetic corpus and real logs) against the oracle through `base_line()`. It is the acceptance's mechanism, and it proves 1a changed nothing but the intended refinements.
- Re-bless `classify.golden` and the CLI and harness goldens from aios (`AIOS_BLESS_GOLDENS=1`), reviewing each diff.
- Delete the oracle recorders and the CLI and harness differentials (`record_*_from_oracle`, `cli_differential_against_oracle`, `harness_differential_against_oracle`, `run_oracle`, and the ambient-`timeout` plumbing in `soak_fake.rs`). Their outputs now differ by design.
- CI's Tools job keeps `fetch-depth: 0`, since the classify oracle and docs-check's `check.py` oracle still read history.

**D9. The catalogue comes from `shared`** (Q2). Recommended: `aios-tools` depends on `shared` (a path dependency) and reads `Key::ALL`, `name()`, `width()` and `is_gauge()`, and the index enums' names, from it.

- One source of truth: when the kernel adds a key, the tools build picks it up, and the `GOLDEN_FULL` test catches an order change.
- The cost is `sha2` (no default features, MIT/Apache, already in `Cargo.lock`) as a transitive dependency of the host tool. That needs rule 01's approved-dependency sentence amended. Hence the owner question.

-----

## Tasks

One commit each, `Crash fix step 1a: <desc>`, pushed after its checks pass (rule 03). Every code task runs the A3 gates. "Bless" means `AIOS_BLESS_GOLDENS=1 cargo test -p aios-tools --test <file>`, then reading the golden diff before committing.

- [x] **T1. Working plan.** This file.
  - Check: `cargo run -q -p aios-tools -- docs-check`: no new drift except knowledge-hygiene's non-empty `plans/`.
- [ ] **T2. Runner refactor: `Arm` and `boot_once`.** No change in behaviour.
  - Files: `runner.rs`. An `Arm` holds the checkout root, firmware, the ESP snapshot path, the kernel sha line and the git rev. `boot_once(arm, cfg, n, …) -> BootOutcome` runs one QEMU boot and writes the log and footer. The loop in `build_and_boot` calls it.
  - Tests: the existing unit and harness tests.
  - Check: `cargo test -p aios-tools` with `git status --short tools/tests/golden` empty, so no golden changed.
- [ ] **T3. Class enum, WEDGE split, PANIC-LOCK.**
  - Files: `classify.rs` (`Class`, refinement, `base_line()`, `reentry` parsing), `report.rs` (9-class table and counts), `mod.rs` (`USAGE` class list), `scripts/agent/brief.sh` (its `summary.tsv` class counts gain the subclasses, so WEDGE is not read as 0), `justfile` comment line 137, developer-guide §5.6 "Classes" table, `tools/tests/fixtures/soak/synthetic.txt` (new cases: each WEDGE-STUCK branch, each WEDGE-ALIVE branch, gpu markers missing, a two-line `lock re-entry:` panic, a `lock re-entry:` after an earlier EXCEPTION, which stays EXCEPTION), the `soak_classify.rs` fold differential plus the A1 real-log check, and the goldens re-blessed (D8, after Q4).
  - Tests: one unit test per mapping. The corpus check "every class has at least 5 cases" now covers the 9 classes.
  - Check: A1 run on `target/soak/167` prints exactly the expected matrix. The golden diffs contain only class tokens and class-table rows (`git diff --word-diff tools/tests/golden`).
- [ ] **T4. DEGRADED and the IPC line.**
  - Files: `classify.rs` (IPC parse; DEGRADED where CLEAN was returned), `report.rs` (`ipc_avg_us`, `ipc_iters` columns), `mod.rs` `USAGE`, developer-guide §5.6 heartbeat rule 3 and the classes table, corpus cases (0 iters with G1DONE, which is step 1b's Gate 1 FAIL; 9999; 10000; an unreadable count per Q5; a fatal boot with 0 iters, which stays fatal, as `pr209-fix-198` shows).
  - Tests: unit tests; a drift test that reads `kernel/src/bench.rs` and asserts `IPC_ITERATIONS: usize = 10_000`.
  - Check: A1 still shows CLEAN → DEGRADED 0. The A2 classes hold.
- [ ] **T5. Tripwire parser and event counts.**
  - Files: new `tripwire.rs` (`parse_line`, `LastLines { last, g1 }`, `EventCounts`), wired into the `Scan` in `classify.rs`. `tools/Cargo.toml` gains the `shared` path dependency (Q2) and rule 01's tools sentence is amended in the same commit.
  - Tests:
    - `GOLDEN_FULL` extracted from `shared/src/tripwire.rs` parses complete (`n=66`), with its keys in `Key::ALL` order;
    - `GOLDEN_NONZERO` parses, and its missing keys read 0;
    - an incomplete last line falls back to the previous complete one;
    - a line that starts mid-line is accepted;
    - `[tripwire-ev]` is not a tripwire line;
    - `v=2` is kept raw;
    - `src=panic` and `src=exc` are accepted;
    - a log with no line gives `-`;
    - event lines count by kind, and by ctx for `self`.
  - Check: unit tests pass. A scratch run over B1's text logs shows `tw_src` `panic` for runs 05, 07 and 11.
- [ ] **T6. Tripwire columns, `--classify --out`, per-class table.**
  - Files: `report.rs` (D3 columns, the "Tripwire counters by class" table, Gate 1 IPC line), `mod.rs` (`--classify --out DIR` writes `summary.tsv` and `summary.md` for the given logs, with timing from each footer; `USAGE`), `runner.rs` (rows carry the new columns), developer-guide §5.6 output table, observability §6.5 "Parser contract" (names `aios soak` as the consumer and lists the columns), goldens re-blessed.
  - Tests: `tsv_row` field count equals the header count (22 + new); unit tests of the table on synthetic classifications; CLI goldens for `--classify --out`.
  - Check: A2 in full, meaning the projection diff is empty for 30 of 30 rows and the per-class table matches the note's.
- [ ] **T7. Statistics module.**
  - Files: new `stats.rs` (`fisher_low`, `fisher_high`, `fisher_two`, `wilson` moved from `report.rs`).
  - Tests: as in D6.
  - Check: `cargo test -p aios-tools stats`.
- [ ] **T8. Interleave mode.**
  - Files: new `interleave.rs`, plus `mod.rs` (`--arm`, `arm=`, `--allow-mixed-toolchains`; `USAGE`), `runner.rs` (shared setup), `host.rs` (sha256 of the resolved QEMU binary, `rust-toolchain.toml` channel, `rustc --version` in a directory, `merge-base --is-ancestor`), `tools/tests/common/soak_fake.rs` (fake arm checkouts with their own fake `just` and `rustc`; QEMU that differs per arm).
  - Tests (fake-QEMU scenarios):
    - two arms with the boot order ABBAAB… recorded in `boots.tsv`;
    - A/A with the same dir twice;
    - refusals, each with exit 2 before any boot: channel mismatch, rustc mismatch, firmware mismatch, an arm without 7167d40, one `--arm`, five `--arm`;
    - `--allow-mixed-toolchains` marks the summary;
    - a QEMU sha change mid-soak stops the soak, and that boot is not counted;
    - SIGINT mid-round leaves per-arm `run-NN.log` intact and no summary;
    - per-arm directories hold valid single-run files.
  - Check: the scenario goldens pass. In `cargo run -q -p aios-tools -- soak --help`, the `--arm` text names the checks.
- [ ] **T9. Pair report and load rule.**
  - Files: new `pair.rs` (D4 interleaved report from per-arm rows; `--combine`), `interleave.rs` (load1 > CPU count at the start refuses unless overridden, per Q6; the per-boot load is recorded as today), `mod.rs` (`--combine`, the override flag; `USAGE`).
  - Tests:
    - the report on synthetic arms: regression guard pass and fail at p = 0.05 boundaries, "removed" 5→0 yes and 4→0 no, a combined row, INCONCLUSIVE excluded from the denominators;
    - the 25% verdict at 24% and 26%;
    - the Gate 1 mean over CLEAN only;
    - the R16 count;
    - a golden of a fake two-arm run's `summary.md`.
  - Check: goldens and units pass. A dry run of the step-1b A/B command line (`--help` shows `--combine`) is documented in the PR.
- [ ] **T10. CI: `runs` input, `soak-matrix.yml` on aios, delete `scripts/soak-matrix.sh`.**
  - Files: `.github/workflows/ci.yml` (`workflow_dispatch.inputs.runs`, default 5, passed as `RUNS` through `env`; a plan step computes the step limit as runs × (90 + 15) s + 5 min, and the job limit rises to cover the maximum, per Q7), `.github/workflows/soak-matrix.yml` (D7), `scripts/soak-matrix.sh` deleted, the developer-guide §5.6 interleave paragraph rewritten, `docs/project/agent-loop.md` (its `target/soak-matrix/` sentence), `.claude/CLAUDE.md` layout (`scripts/` line, tools soak line).
  - Check:
    - `actionlint` if installed, else `python3 -I -c 'import yaml…'` over both workflows;
    - `rg -n 'soak-matrix\.sh' -g '!docs/knowledge/**' -g '!target' .` returns nothing;
    - docs-check `repo-paths` has no new finding.
  - The CI behaviour itself is checked after push: a `workflow_dispatch` of CI on the branch with `runs=2`, and a `soak-matrix.yml` dispatch with two arms (`runs=2`), both read from the job summary. These runs are on CI, not local QEMU.
- [ ] **T11. Docs sweep.**
  - Files:
    - the ADR, a review note "Step 1a delivered, <date>": what landed, the R10 evidence, that the 2026-10-06 hand-classification note and the `soak-matrix.sh` paths in "Soak protocol" are superseded (in-place pointer only), and the step-1b A/B command line;
    - developer-guide §5.6 consistency pass (classes, columns, interleave, statistics, CI input);
    - observability §6.5 (if T6 left anything);
    - `README.md` (`just soak` row: classes and `--arm`);
    - `.claude/CLAUDE.md` (layout: `tools/` soak modules);
    - rule 01 (if not done in T5);
    - `docs/knowledge/discussions/2026-09-22-jl-rust-agent-tools.md` only if it states current behaviour that changed.
  - Check: docs-check no new drift (plan finding aside); `/audit-loop` docs scope.
- [ ] **T12. Final gate and distill.**
  - A1 and A2 re-run on the final head, with output pasted into the PR.
  - An attended local smoke soak, with the owner's go-ahead because it is host-exclusive and about 5 minutes: `just soak --arm <af59149 worktree> --arm . runs=2 secs=75 report_only=1`. It must show 4 boots, rotation ABBA, the pair report and per-arm directories. Only this task's own QEMU PID may be stopped.
  - `/verify-phase`-equivalent gates (A3), then `/audit-loop` until a clean round.
  - Distill this plan into lessons and decisions, then delete it.
  - Push, open the PR, run `/review-pr-comments`, and hand off for `/merge-and-cleanup`.

After the merge (not step 1a's PR): step 1b's A/B soak runs with `just soak --arm <af59149> --arm <e211d6d> runs=20 secs=75 --combine WEDGE-STUCK+PANIC-LOCK`, then `mode=gpu runs=10`.

-----

## Docs this makes stale

| Doc | Change | Task |
|---|---|---|
| `docs/project/developer-guide.md` §5.6 (and the `just soak` row at :1559) | Classes table, heartbeat rule 3 (DEGRADED), output files, the `--classify --out`, interleave, statistics and CI `runs` paragraphs; the `soak-matrix.sh` paragraph is replaced | T3, T4, T6, T10, T11 |
| `docs/kernel/observability.md` §6.5 | "Parser contract" names its consumer and the columns | T6 |
| Crash-fix ADR | A "Step 1a delivered" review note; pointers where "Soak protocol" names `soak-matrix.sh` and the hand classification | T11 |
| `.claude/CLAUDE.md` | Layout: the `scripts/` line loses `soak-matrix.sh`; the `tools/` line names the soak modules | T10, T11 |
| `.claude/rules/01-code-conventions.md` | The tools dependency sentence, if Q2 = A | T5 |
| `justfile` | The `soak` comment's class list (line 137) | T3 |
| `README.md` | The `just soak` row | T11 |
| `docs/project/agent-loop.md` | The `scripts/soak-matrix.sh` / `target/soak-matrix/` sentence (:63) | T10 |
| `scripts/agent/brief.sh` | Class counts read from `summary.tsv` | T3 |
| `tools/src/cmd/soak/mod.rs` `USAGE` | Every new option and class (in the commit that adds it) | T3–T9 |

The `.claude/` edits go through a permission prompt (rule 08), so they are batched where possible.

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
  - the owner's answers to Q1–Q8;
  - R4's soak port on `main` (present);
  - the run-167 and B1 logs on this Mac (present, read-only).
- **Golden churn hides an unintended change.** T3, T4 and T6 re-bless goldens. Mitigation: the fold differential runs on every commit; each re-bless is its own commit with a word-diff review; T2's refactor must change no golden.
- **Losing the CLI and harness oracle** (Q4 A) leaves those formats guarded only by aios-blessed goldens. They were ported byte for byte and verified in R4; from 1a on they evolve on purpose.
- **One harness boots old arms.** The harness's QEMU arguments, not an arm's justfile, are used. They are identical for af59149 and e211d6d. A future arm whose `run` recipe changes devices would be booted differently from its `just run`. `arms.tsv` records the arguments, and the developer guide says so.
- **Tripwire lines torn by unlocked UART output.** The `n` check rejects them, and the parser falls back to an older complete line. `tw_t` against `last_tick` shows the staleness. Under the contract a stale line is still correct data.
- **Schema drift.** A key added in `shared` changes the column set with no tools edit (D9). That is wanted, but it changes `summary.tsv`'s width; readers must go by header name (`brief.sh` uses positions 2 and 3, which never move).
- **Load-rule refusal blocks a soak on a busy Mac or runner.** The override flag exists, and the refusal message names it.
- **Fisher precision.** The log-factorial sum is fine to n in the thousands; the exact u128 oracle covers up to 60.
- **`.claude/` permission prompts** (rule 08) for `CLAUDE.md` and rule 01. They are batched into T5, T10 and T11, so an unattended run can stall there.
- **Scope creep** (the audit-loop lesson): extras such as re-reporting an interleaved run from its logs, an `[smp]` parser or H1 verdict code stay out. Audit findings that ask for them go to the owner as descoping questions.
- **docs-check `repo-paths`.** It fails if a current-state doc still names `scripts/soak-matrix.sh` after T10. T10 updates those docs in the same commit.
- **The T12 smoke soak is host-exclusive.** It is attended and needs the owner's go-ahead. It stops only its own QEMU PID, never `pkill`.

-----

## Issues Encountered

(to be filled during implementation)

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

(to be filled during implementation)

## Lessons Learned

(to be filled during implementation)
