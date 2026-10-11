---
author: jl + claude
date: 2026-10-06
tags: [tooling]
status: final
---

# Lesson: `just check` never lints the shared test modules, and host clippy on them already fails

## What happened

A task that added host tests to `shared/` ran clippy on them and got a wall of errors from other modules' tests. Left unexplained, those look like the task's own regressions, and tempt a fix of code outside its scope.

Still true on 2026-10-10 at 368d99a. `cargo clippy -p shared --all-targets` ends with `could not compile shared (lib test) due to 1 previous error; 24 warnings emitted`:

| File | Findings |
| --- | --- |
| `shared/src/ipc.rs` | 11: nine `assertions_on_constants`, the one error: a comparison with a type's minimum or maximum (`absurd_extreme_comparisons`, line 355), plus an operation with no effect (line 362) |
| `shared/src/storage.rs` | 6, all `assertions_on_constants` |
| `shared/src/memory.rs` | 4: `needless_range_loop` (lines 609, 651, 656) and `manual_range_contains` (line 667) |
| `shared/src/compositor.rs` | 2: `too_many_arguments` (8 of 7, line 1467) and an operation with no effect (line 1626) |
| `shared/src/gpu.rs` | 2, `assertions_on_constants` |

## Why it happened

`just check` runs clippy only for `aarch64-unknown-none`, `uefi-stub` and the host tools crate (`clippy` recipe in the `justfile`), always with `-D warnings`. The `#[cfg(test)]` modules of `shared/` compile only for the host, so nothing lints them, and the findings accumulated. `just test` builds them but does not run clippy.

## What we learned

- A clippy failure in `shared/` tests is not evidence about the current change.
- The list changes as people touch those files, so the counts above are a dated snapshot. What stays true is that the host target is not linted.

## How to avoid next time

- When you add host tests to `shared/`, run `cargo clippy -p shared --all-targets` (add `--message-format short`) and filter its output to your own file with `grep '^shared/src/<file>.rs'`. Do not expect exit 0.
- Do not fix other modules' findings unless the task asks for it.
- Re-run the command before relying on this lesson: someone may have cleaned the findings up, and then `just clippy` could lint the host target as well.
