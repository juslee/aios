---
author: jl + claude
date: 2026-10-06
tags: [tooling]
status: final
---

# Lesson: Run the docs-check, hook and objdump gates the way a worktree needs

## What happened

Reviewers working in `.claude/worktrees/*` reported false drift, a failing objdump gate, and misleading docs-check results on harness and `tools/` branches. Three separate causes, all seen between 2026-09-22 and 2026-10-09.

## Why it happened

- **docs-check runs main's binary.** `just docs-check` is `.claude/hooks/aios docs-check`. The shim runs the **main checkout's** `target/tools/installed/aios`, whichever worktree calls it. A branch that changes `tools/` (a new check, a changed baseline rule) therefore shows false drift until it merges. On 2026-10-06, main's binary reported 17 false "stale IrqSpinLock" lock-order findings that the branch's own build did not.
- **`cargo objdump` fails under asdf.** In a worktree, `cargo objdump -- -h` stops with "No version is set for command cargo-objdump", because the worktree's `.tool-versions` does not pin it. The gate is not failing; the shim is.
- **The Bash tool runs zsh and keeps no state.** See `2026-10-06-jl-testing-harness-shell-snippets.md`.

## What we learned

Re-checked on 2026-10-10 at 368d99a:

- `just tools` in a worktree installs that worktree's own `target/tools/installed/aios`. `AIOS_TOOLS_BIN=target/tools/installed/aios just docs-check` then runs it. That exact form is allow-listed in `.claude/settings.json`. The override is taken relative to the current directory and has no stamp or freshness test (header of `.claude/hooks/aios`). The earlier path `target/tools/release/aios` is gone: the shim uses a copy cargo never writes (#203, #229).
- `aios docs-check --baseline <file> --update-baseline` rewrites any baseline file, counts included, and keeps `reason` on accepted false positives. The sysroot `llvm-objdump` works: on the kernel built at 368d99a, `-h` shows `.text.boot` at VMA 0xffff000000080000 and LMA 0x40080000, `.text.vectors` at 0x...80800, and the 128 KiB `.stack` after `.bss`.
- The hook tests run as `/usr/bin/python3 -m unittest discover -s .claude/hooks/tests` (55 tests, all pass, about a minute; also allow-listed).
- Since #228 the tools tests also run a `check.py` oracle materialised from git history, plus patches in `tools/tests/fixtures/docs-check/`. A branch-only change to the oracle's behaviour needs its own patch there, because branch commits vanish on a squash merge.

## How to avoid next time

**docs-check on a branch**

```bash
just tools
AIOS_TOOLS_BIN=target/tools/installed/aios just docs-check
```

To prove `scripts/docs/baseline.json` is exactly what regeneration would write, with no stale count or entry:

```bash
cp scripts/docs/baseline.json <scratch>/baseline.json
AIOS_TOOLS_BIN=target/tools/installed/aios .claude/hooks/aios docs-check --baseline <scratch>/baseline.json --update-baseline
diff scripts/docs/baseline.json <scratch>/baseline.json
```

Expect a diff only for findings the committed baseline deliberately leaves out (a working plan in `docs/knowledge/plans/` shows up as `plans-not-empty`).

**Extra checks for `tools/` changes**

- `RUSTDOCFLAGS="-D warnings" cargo doc -p aios-tools --no-deps --document-private-items` catches broken intra-doc links and unclosed HTML tags in doc comments that clippy and the tests miss. It is not a CI job, and at 368d99a it already fails on an unclosed `<path>` tag in `tools/src/cmd/hook/path_guard.rs`.
- `tools/tests/common/mod.rs` has `#![allow(dead_code)]`, so unused test helpers never warn; grep their uses by hand.
- Oracle proof: `AIOS_REQUIRE_CHECK_PY=1 cargo test -q -p aios-tools --test docs_check_parity differential_against_check_py -- --nocapture` prints how many cases and live modes were compared and how many differ; plain `cargo test` hides that on success. Never run the ignored `record_goldens_from_check_py` in a report-only review: it rewrites the goldens (so does `AIOS_BLESS_GOLDENS=1`).
- A branch-only oracle patch is right when `git diff <sha>^ <sha> -- scripts/docs/check.py` equals the fixture patch except for its `index` line (the fixture applies on top of the migration patch, so the blob ids differ).

**The objdump gate**

```bash
cargo build --target aarch64-unknown-none
"$(rustc --print sysroot)"/lib/rustlib/*/bin/llvm-objdump -h target/aarch64-unknown-none/debug/kernel
```

Check `.text.boot` at VMA 0xffff000000080000 and LMA 0x40080000. For per-symbol V-register counts and the traps in them, see `2026-10-06-jl-irq-path-codegen-hazards.md`. To place an exception's ELR in a function, find the symbol with `llvm-nm -C --defined-only <elf>` (the demangled prefix is `kernel::`) and disassemble around it with `--start-address` and `--stop-address` in literal hex. An `awk '/<sym>:/'` range over the whole `-d` output misses because of the `.llvm.<hash>` suffix.

**Reviewing a merge commit or a drifted PR**

- Isolate the hand edits in a merge: `T=$(git merge-tree --write-tree <ours> <theirs> | head -1)`, then `git diff $T <merge-sha>`. Only conflict resolutions and extra edits remain; auto-merged files drop out. It writes an object and touches no worktree or index.
- Before auditing cross-references in a PR, run `git merge-tree --write-tree --name-only origin/main HEAD` in its worktree. On 2026-10-06 `main` moved the root `CLAUDE.md` to `.claude/CLAUDE.md` (#218) under an open PR, and the PR conflicted in `docs/project/agent-loop.md`. Read cross-referenced files from the merged tree (`git show <tree>:<path>`), not only from the branch.
- Replay a deleted script from history with `git show <sha>:<path>` into a scratch directory. A Python 3.14 module that defines dataclasses needs `sys.modules[name] = module` before `exec_module`.
