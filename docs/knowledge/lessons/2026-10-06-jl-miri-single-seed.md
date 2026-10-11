---
author: jl + claude
date: 2026-10-06
tags: [tooling, sched]
status: final
---

# Lesson: CI runs Miri with one seed, so size threaded tests until mutations fail on it

## What happened

Reviewing crash-fix step 1b's lock type in `shared/` (2026-09-24), a threaded test of an atomics protocol passed on Miri's default seed with a load-bearing ordering weakened. The test ran `four_threads_*` with `#[cfg(miri)] ITERS = 25`. It still passed with the snapshot `fence(Acquire)` removed, with `restamp` as a Relaxed store, or with the holder-field store Relaxed. Only a run with `-Zmiri-many-seeds` caught them. At `ITERS = 400` all six ordering mutations failed on the default seed and on every explicit seed from 0 to 9 (the field-store mutation on 8 of the 10); the test took about 5 seconds under Miri. At `ITERS = 100` the reviewer still saw misses on seeds 1 to 3. That lock type is not on `main` at 368d99a; the method applies to any atomics-protocol type added to `shared/`.

## Why it happened

CI's Miri gate is `just miri`, which is `cargo miri test -p shared --target-dir target/miri-tests` (the `miri` job in `.github/workflows/ci.yml`). It passes no seed flags, so every test runs once on Miri's default seed. A weak-memory race that needs a particular interleaving is found by that seed or not at all. The ADR's reliance on Miri to catch ordering regressions in `shared/` types holds only if a test fails on that one seed.

## What we learned

- Passing under `-Zmiri-many-seeds` proves little about CI. Failing on the default seed after mutating each ordering is the bar.
- The iteration count under `cfg(miri)` is the knob. Choose it by mutation testing, not by run time alone.
- Re-checked on 2026-10-10: `MIRIFLAGS="-Zmiri-many-seeds=0..3" cargo miri test -p shared --target-dir target/miri-tests <filter>` runs the filtered tests once per seed, in parallel, and the per-seed output interleaves (`okok`).

## How to avoid next time

For new atomics-protocol code in `shared/`, mutation-test each ordering the design relies on:

1. Copy the module into a scratch crate (give its `Cargo.toml` a `[workspace]` table and copy `rust-toolchain.toml`).
2. Weaken one ordering (for example with `sed`).
3. Run `cargo miri test --lib <filter>` on the default seed, then with `MIRIFLAGS=-Zmiri-seed=N` for several N.
4. Raise the `cfg(miri)` iteration count until every mutation fails on the default seed.

A local sweep over many seeds is a convenience check, not the gate: `MIRIFLAGS="-Zmiri-many-seeds=0..16" cargo miri test -p shared --target-dir target/miri-tests <filter>`. Judge it by the exit code or by grepping `FAILED|Undefined|race`, because the output of parallel seeds interleaves. `std::thread::scope` and `spin_loop()` spin-waits work under Miri. If only a many-seed run fails, ask for more Miri iterations rather than a multi-seed CI step.
