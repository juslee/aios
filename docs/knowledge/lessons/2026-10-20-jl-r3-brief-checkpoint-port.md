---
author: jl + claude
date: 2026-10-20
tags: [tooling]
status: final
---

# Lesson: Porting brief.sh and checkpoint.sh to Rust, and guarding a compiled git writer

## What happened

PR #257 (Tools R3) replaced `scripts/agent/brief.sh` (jq, awk, sed and `gh` over git) and `scripts/agent/checkpoint.sh` (the WIP commit, secret scan and push of a `claude/*` branch) with `aios brief` and `aios checkpoint`. It also added the shared `tools/src/gh.rs` (a read-only gh runner), `tools/src/git.rs` and `tools/src/merge_ready.rs` (the merge-ready verdict). A second branch (R2) builds on those modules.

The method was the one from the soak port ([2026-09-29-jl-soak-port-awk-and-process-parity.md](2026-09-29-jl-soak-port-awk-and-process-parity.md)): the oracle is the deleted script's blob in git history, goldens are recorded from it, and differential tests run it beside `aios` in twin repositories with a fake `gh`. The scripts were deleted in the same PR.

The audit ran 11 rounds and fixed 39 findings; gate issue #271 recorded the owner rulings. Follow-ups: #275 (consolidate `git.rs`; `hardened(cwd)`), #276 (the guard asks with the QEMU reason on direct shim calls), #277 (run `.claude/hooks/tests` in CI), #278 (checkpoint hardening: the `git add -A` status, multi-commit sequencer detection, replace refs).

## Why it happened

- **jq is not Rust.** The brief's jq filters carry semantics a straight translation loses.
  - `//` falls through only on `null` and `false`, so a thread count of 0 stays 0; a Rust `unwrap_or` that treats zero as missing would replace it.
  - `max_by` keeps the last maximum, so a tie among main's CI runs picks the later one.
  - `tonumber` goes through a double, so a number above 2^53 loses precision in jq. The port parses it exactly as `u64` and records that as an accepted divergence (D4).
  - `[[:cntrl:]]` maps to `?`, and `scan("#([0-9]+)")|tonumber|unique` sorts numerically. Each has a unit-table row.
- **A tool list that agrees with a blind extractor.** The differential runs each script under a PATH jail with no `/usr/bin` and no `/bin`, and the per-script tool lists are derived from the scripts by a lexer, checked by a test. An extractor that skips quoted text misses `"$(… | paste …)"`: checkpoint runs `paste`, `head`, `dirname` and `basename` only inside double-quoted command substitutions, and brief runs `date` only in `"$(date …)"`. Commands also follow `if` and `elif`. A list that matched a blind extractor would have left `paste` out of the jail, and the oracle would have failed early and identically on both sides.
- **No git identity in a hermetic environment.** The harness keeps `GIT_CONFIG_GLOBAL=/dev/null`. The setup commits pin an identity, but checkpoint's own `wip` commit did not: it failed on the CI runner ("unable to auto-detect email address"), and on the Mac it recorded the host user's identity and machine-dependent shas. The runs now set a pinned identity and pinned dates after the environment scrub, and a scenario checks both.
- **Twin repositories per run.** Each scenario materialises its repository fresh for the oracle and for `aios`, with the same commands and the same pinned dates, then compares stdout, exit status, side effects, and the fake-gh and docs-check call logs.
- **Both scripts `cd "$ROOT"` first, but brief reads some things from the main worktree.** Phase docs, knowledge notes, pathspecs and the docs-check child use the toplevel, so both ports resolve the toplevel from their cwd and run every child from it (`run_from_subdirectory`). brief.sh's `MAIN_WT` is the first porcelain worktree: handoffs come from `<main>/.remember`, `sh qemu-lock.sh status` runs in the main worktree, and `rel_path` labels (`- checkout: (main checkout)`) are relative to it. `run_from_linked_worktree` pins that split.
- **Test repositories sit inside the checkout.** `CARGO_TARGET_TMPDIR` is `<repo>/target/tmp`, and the shared `isolated()` helper removes `GIT_CEILING_DIRECTORIES`. An "outside a repository" scenario built there resolves the real aios checkout, where checkpoint runs `git add -A` and could commit. Such a scenario runs under `std::env::temp_dir()` with `GIT_CEILING_DIRECTORIES` set after the scrub, and every scenario asserts that its twin's toplevel is the twin root.
- **Oracle output depends on the host's tools.** On a missing `boots.tsv`, BSD awk stops where gawk goes on to run its END block, so the oracle's output differs by platform. Goldens are recorded on macOS, `PLATFORM_SCENARIOS` lists each such scenario with the tool and the oracle line, and the port follows the macOS golden. The golden check runs everywhere; the oracle comparison runs on macOS only.
- **The guard cannot see what a compiled binary does.** The guard read `checkpoint.sh` and asked once per pause, because it met a push refspec it could not resolve. `aios checkpoint` commits and pushes inside a binary, and `Bash(just *)` and `Bash(cargo *)` are allowed, so the ask would vanish. The binary has its own bounds: the refspec is built only from a `claude/?*` branch, the push is never forced, the secret scan fails closed on a git error (exit 1, index restored), and the WIP is held when a ready PR exists. The control is owner settings asks, one per invocation spelling (`just`, the `--justfile` fallback, the shim, the installed binary, `cargo run`).
- **The permission probe found holes the plan had not.** The headless probe covered 24 spellings, including a background Bash call, a Monitor and a background agent, and all were refused in the end. The first counted run found three that ran unasked: `sh .claude/hooks/aios checkpoint`, `cargo r -q -p aios-tools -- checkpoint` and `just 'checkpoint'` (written inline in the prompt, the quoted form also read as English and was inconclusive until the command went into a fenced block). Five extra asks fixed them. Audit round 1 then found `bash .claude/hooks/aios checkpoint` and `target/release/aios checkpoint` unasked, and one more rule (`Bash(*/aios checkpoint*)`) closed them. Settings patterns match the command text, so a shell quoting such as `just che'ck'point` and an indirect spelling such as `sh -c 'just checkpoint'` stay accepted gaps.
- **The docs gate fails while a plan is committed.** `docs-check` reports every file under `docs/knowledge/plans/` (except the README and `_template.md`) as `knowledge-hygiene plans-not-empty`, unbaselined. While the working plan and the shared contract copy were on the branch, `just docs-check` exited 1, so every "no new drift" check had to expect those two findings. The ship pass deletes the plans, which clears them.
- **The non-UTF-8 class took a sweep.** Audit round 4 found that a branch name must reach `gh pr list --head` as bytes; round 6 found the same fault in the rebase/bisect gate (the git-path test) and in several path renderings. Git paths, branch names, phase-doc names and soak specs are bytes on Unix. A `String` or a lossy conversion changes the value (a non-UTF-8 branch printed as `(detached)`), and a differential that compares lossy renderings agrees with the bug. The fix was one sweep of the whole class (bytes through, `GhCli::run_os`, `paths::normpath_bytes`, raw-byte differentials), not one finding at a time.
- **`current_exe()` can name a deleted file.** brief spawns docs-check as a child of its own binary. On Linux `current_exe()` reads `/proc/self/exe`, which says `<path> (deleted)` once `just tools` has renamed a new binary over the running one. The child is spawned through `/proc/self/exe` on Linux and `current_exe()` elsewhere.

## What we learned

- **The recipe path and the direct path differ.** The shim exports `GIT_NO_REPLACE_OBJECTS=1` and `GIT_GRAFT_FILE=/dev/null/no-grafts`, so on `just checkpoint` the secret scan reads original objects, while the parity tests run the binary directly and do not see that. Say in the module docs which mode a behaviour belongs to, so a reviewer does not "fix" it.
- **List what the port keeps for parity, in one place.** The `git add -A` status unchecked, a multi-commit sequencer not detected, evil merges and binary files not scanned, replace refs honoured outside the shim, the push attempted after a commit failure. File the follow-up (#278) before the ship pass.
- **A fail-closed rule needs its clean-tree scenario.** With nothing dirty and nothing unpushed the script prints `Safe to /clear` and exits 0, and a failed secret scan must never reach that line. An unpushed commit already suppresses the line for another reason, so it cannot pin the rule.
- **A frozen API needs an amendment path.** R2 vendors `gh.rs`, `git.rs`, `merge_ready.rs` and two test helpers from a pinned sha. The non-UTF-8 fix changed a frozen signature (`gh::run_program` became generic over its argument type), which took an owner ruling and a new freeze sha.

## How to avoid next time

- Port jq filters by their semantics, with one unit-table row per quirk, not operator by operator.
- Derive a jail's tool list from the script with a lexer that reads command substitutions inside double quotes, and test the lexer on the lines that need it.
- Give every spawned git a pinned identity and date after the environment scrub, and make each scenario assert the toplevel it ran in.
- Build "outside a repository" tests under `std::env::temp_dir()` with a `GIT_CEILING_DIRECTORIES` ceiling, never under `target/`.
- Keep git paths, branch names and spec strings as bytes from input to output, and compare differentials as bytes.
- Spawn yourself through `/proc/self/exe` on Linux.
- When the guard cannot see what a binary does, bound the binary, add owner asks per spelling, probe them headless (`-p`, background Bash, Monitor, background agent) before the code exists, and re-probe after any settings change. State which spellings stay open.
- Run `just docs-check` expecting the `plans-not-empty` findings while a plan is committed, and delete the plan in the ship pass.
