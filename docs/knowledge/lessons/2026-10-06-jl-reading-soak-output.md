---
author: jl + claude
date: 2026-10-06
tags: [tooling, boot]
status: final
---

# Lesson: Read soak output from the checkout that ran it, and read it carefully

## What happened

Docs that quote soak results (the crash-fix ADR, the developer guide) were audited against the wrong place, and counts from soak logs were repeated without checking how the logs are laid out. An early version of this note said soak output lives only in the main checkout. That is not true.

## Why it happened

`aios soak` (and `just soak`) write under `target/soak/` of the checkout that ran them: by default `target/soak/<timestamp>-<mode>`, or the `out=` directory. Any worktree that ran the harness has its own `target/soak/`. The main checkout also holds a copy of each PR's runs, because `/merge-and-cleanup` copies a worktree's `target/soak/` entries to `<main>/target/soak/pr<number>-<run>` before it merges (the merge deletes the worktree and the ignored `target/` with it). So a run is in the worktree that took it until that PR merges, and under `pr<number>-<run>` in the main checkout afterwards.

## What we learned

Layout of one run directory (`aios soak --help` has the full text):

- One directory per run, holding `run-NN.log`, `summary.tsv`, `summary.md` and `build.log`. Run 167 has one directory per arm, mode and round (`main-gpu-r1`, `pr161-text-r4`). `scripts/soak-matrix.sh` writes `target/soak-matrix/<timestamp>-<mode>` in the main checkout by default, with `summary.md`, `arms.tsv` and an `arm-X/rNN` directory per boot; `/merge-and-cleanup` copies only `target/soak/`, so keep a matrix run's `--out` outside a PR worktree.
- `summary.tsv` is tab-separated with a header line. Column 3 is the class, 4 the last heartbeat tick, 9 the host's load1, 17 the detail and 18 the first fatal line. `summary.md` records the commit (with `-dirty` when tracked files were modified), the kernel ELF sha256 prefix, the QEMU version, the firmware path and the host load.
- Logs hold ESC and other non-text bytes from the firmware's output. Some grep builds then print `Binary file matches` or nothing for a pattern that is in the file. Use `grep -a` (or `rg -a`). On 2026-10-10 the system grep and ugrep both matched without `-a` on a current log, so check your own grep instead of assuming either way.
- A boot can hold several fatal reports. `first_fatal` holds only the first one, and the class comes from the earliest report, so a count of reports across the logs can differ from a count of boots.
- UART lines from several CPUs can interleave and split a report. The classifier matches substrings and register fields for that reason; an anchored `^\[bench\] ...` grep can miss a line that another CPU's output split.

Claims worth re-checking against the logs when a doc quotes them: per-class counts, the last bench line of heartbeat-alive WEDGE boots (they do not all stop at the same line), and heartbeat ticks after a PANIC.

## How to avoid next time

- To audit a doc that quotes soak numbers, find the run under `target/soak/` in the worktree that took it, or under `pr<number>-<run>` in the main checkout, and read `summary.tsv` and `summary.md` first, then the logs with `-a`.
- Compare the commit and ELF prefix in `summary.md` with the commit the doc names.
- Use the `[soak] meta` line at the end of a log to find boots by shape: `g1done=-1` with `hb_count` above 1 is a heartbeat-alive boot whose bench never completed.
- Re-classify a saved log with `aios soak --classify LOG...`.
