---
author: jl + claude
date: 2026-10-06
tags: [tooling, boot, sched]
status: final
---

# Lesson: Compare boot behaviour only with interleaved arms, and keep the evidence

## What happened

Several reviews attributed a boot failure to the branch under test, or accepted an implementer's boot evidence, without a comparison that could show it. The recurring cases: a PC=0 instruction abort (ESR 0x86000006) that a branch's soak showed and `main` showed too; a `free_pages` BUG panic or low-FAR data abort blamed on new code; a timing cost read off two soak batches taken hours apart; an output directory deleted before its one log of a new failure signature had been read; a "soak" that ran from a dirty tree.

## Why it happened

The boot failure rate on the local host depends on host load and on TCG timing, and both drift. Two soaks run one after the other measure the drift as much as the change. The original PC=0 case is the clearest: the M26 branch crashed in every boot of a short text-mode run, and `main` before M26 showed the same PC=0 abort in an interleaved run on the same host. In text mode M26 ran the same kernel code as `main` (the compositor, shell and test app are skipped), so the excess was layout and timing sensitivity, not M26 logic.

## What we learned

- A pair of soaks counts only when its arms are interleaved on one host, one QEMU binary and one firmware image. `scripts/soak-matrix.sh REF REF [REF [REF]]` does that: it builds each revision in its own worktree, then boots one arm per round with the arm order rotated every round. It refuses arms that boot different firmware or pin different toolchains, stops if QEMU or the firmware changes mid-soak, and prints pairwise Fisher exact tests of CLEAN against not CLEAN. Giving it the same ref twice is an A/A control that shows how far two identical arms drift by chance.
- A single clean `just run` is not evidence a race is gone, and a single failed boot is not evidence a change broke something. Expect `main` itself to fail a 75-second boot in a good share of runs under host load (see `2026-10-06-jl-soak-noise-base-rates.md`).
- To split where a timing cost comes from, build each variant as its own tree and interleave short boots, rather than comparing batches taken at different loads. Compare the bench `min=` first: load inflates `avg` and `p99` but barely moves the best round trip. Only the boots that reach the bench line give a figure, so budget rounds for that fraction.
- A constant fake value must never look current to the code it feeds. A generation of 0 matched a switch counter before the first dispatch and produced a false re-entry panic in one scratch variant; use a value the real path can never produce.

## How to avoid next time

**Before attributing a failure to a branch**, run `main` (or the branch's merge base) through the same interleaved soak, and read the Fisher p-values. Re-verify old counts if the scheduler or IRQ entry code (`exceptions.rs` `irq_el1_entry`, `context_switch.S`) changed since they were taken.

**For a scratch variant** (not a commit), make an arm outside the repository:

```bash
git archive <rev> | tar -x -C <scratch>/arm-<name>
# patch the variant there, then build it with a clean environment
env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS -u CARGO_TARGET_DIR -u RUSTUP_TOOLCHAIN just disk
```

Check that the unpatched arm's `.text` equals the worktree's ELF with `llvm-objcopy --only-section=.text` on both (the debug-info paths change the ELF's sha, not its code). Boot each arm with its own harness for a short run (`--no-build --runs 1 --secs 30 --stall-secs 10 --report-only`, each into a new `--out`). An archived tree has no `.git`, so `summary.md` shows the commit as `unknown`. Since Tools R4 `just soak` also builds the arm's own `aios` first, and that build stamps itself from git state; this was not re-run on an archived tree, so confirm it works there or use a detached worktree (`git worktree add --detach`, which is what the matrix script does). Prefer the matrix script's worktree arms for anything that will be quoted.

**Never delete measurement logs** before every non-CLEAN log has been checked for a new signature. Give each attempt a fresh output directory. A discarded attempt's directory once held the only log of a new failure (a PC equal to a lock word).

**When reviewing someone's boot evidence**:

- `summary.md` showing `<rev>-dirty` means the soak ran from a modified tree. Pull the ELF out of the image (`mcopy -n -i aios.img ::/EFI/AIOS/aios.elf <out>`) and compare it with a fresh build of the commit. The harness records a 16-character sha256 prefix of that ELF; when the prefixes match, the soak tested the commit. When they differ, compare the bytes of `.text`, `.rodata` and `.data` (`llvm-objdump -s -j <section>` piped to a hash), because `.llvm.<hash>` suffixes in the string table change between builds.
- The harness warns when the kernel in the ESP differs from the one under `target/`: it boots the image's kernel, not the build tree's.
- Read the line order of a gpu `frame.rs:51` PANIC boot before accepting "the bench never started" (see the noise lesson).
- Correlate UART lines by their `[secs]` timestamp, not by their position. Once the log rings are ready, `kinfo!` lines are buffered and printed at the next `drain_logs`, while direct `println!` lines (such as `[smp]` and `[bench]`) print at once, so a direct line can sit well above a ring line logged before it. Do not accept "right after line X" wording in a doc as a log-order claim.
- To exercise a path the stock boot never reaches, patch a scratch arm (a longer settle wait, a yield until a later tick) and look for the forced line.
- A diff of each function's disassembly with addresses stripped shows a task's real codegen delta better than a size comparison.
