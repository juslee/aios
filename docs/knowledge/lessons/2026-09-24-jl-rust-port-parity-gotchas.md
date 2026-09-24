---
author: jl + claude
date: 2026-09-24
tags: [tooling, agent-loop]
status: final
---

# Lesson: Porting a Python tool to Rust with byte-identical output

## What happened

PR R1 ported the docs drift checker (`scripts/docs/check.py`, 1,670 lines of Python) to `aios docs-check` in the host crate `tools/`. It proved parity with goldens recorded from the Python tool before deleting it.

The checks themselves took little effort. The effort went into three places:
- Python string semantics.
- Regex features that the Rust `regex` crate lacks or treats differently.
- The shim's freshness test.

Every task passed a per-task review. Even so, reviews of five of the fifteen tasks each found a divergence from `check.py` that was reachable in practice but not listed in the module docs, and a whole-branch review found three more.

## Why it happened

- **Line splitting.** Python `str.splitlines()` breaks at `\n`, `\r`, `\r\n`, `\x0b`, `\x0c`, `\x1c`, `\x1d`, `\x1e`, `\x85`, U+2028 and U+2029, and Python text mode turns `\r\n` and `\r` into `\n` before that. Rust `str::lines()` breaks only at `\n` and `\r\n`. On files that contain the other breaks, line numbers and even line contents differ.
- **Whitespace.** Python `str.strip()` and `str.split()` use `str.isspace()`, which also covers U+001C to U+001F. Rust `char::is_whitespace` does not, and neither does the `regex` crate's `\s`.
- **Lookaround.** Five `check.py` patterns used lookaround (`(?=...)`, `(?!...)`, `(?<!...)`). The `regex` crate has no lookaround and no backreferences.
- **Silent regex differences.** These look the same in both languages but are not:
  - Python's non-MULTILINE `$` also matches just before a final `\n`. The crate's `$` is `\z`.
  - Python's `re.IGNORECASE` folds `ı` (U+0131) and `İ` (U+0130) to `i`. The crate's `(?i)` does not.
  - Python's `\d` matches every Unicode decimal digit. The plan ported it as `[0-9]`.
- **Integer parsing.** Python `str.isdigit()` is true for non-decimal digits such as `²`, and `int()` then raises. CPython 3.11 and later also raises in `int()` past 4300 digits.
- **JSON numbers.** `serde_json` without `arbitrary_precision` rejects numbers beyond the f64 range, such as `1e400`, which Python's `json` accepts.
- **The shim's freshness test.** A `cargo build` with nothing to do leaves the binary's mtime unchanged. A `find -newer` freshness test then reports the binary as stale after every edit that does not change it, for example an edit under `tools/tests/`, and the shim rebuilds on every call.
- **Which binary the shim runs.** The shim runs the main checkout's binary on purpose, so by default a PR that adds or changes a subcommand gets main's build through the shim, not its own, until it merges. Set `AIOS_TOOLS_BIN` to run the PR's own build (see below).
- **Unused dependencies.** The pinned nightly's Cargo has a native `unused_dependencies` manifest lint. `clippy -- -D warnings` does not escalate it, so a dependency declared before its first use prints a warning in every `just check` without failing it.
- **Version-manager shims.** A version-manager shim for `python3` (asdf here) needs the real `HOME`. Under a test's isolated `HOME` it exits 126, which would have made the golden recorder fail and the differential test pass without comparing anything.

## What we learned

- Port the string primitives first (`tools/src/pystr.rs`, `tools/src/paths.rs`) and use them everywhere. Never mix in `str::lines()`, `trim()` or `split_whitespace()`.
- Rewrite each lookaround as a plain pattern plus a check on the text after the match, and write down why backtracking could not have produced a different match. Unit-test the cases of that argument.
- Audit divergences against the Python source, not the port. Enumerate every `re` pattern, `int()`/`isdigit()` call and `str` method in the Python being ported, and list each difference in the module doc with a pinning assertion. An audit that reads the port's own patterns misses exactly the classes it translated.
- End every `$`-anchored regex that is matched against a repository path with `\n?$`. Leave line-matched patterns alone, because a `splitlines()` line cannot contain `\n`.
- Record the goldens from the old tool before deleting it. Do it on two sources:
  - A pinned commit: `git clone --shared`, a detached checkout, and every ref deleted, so the history base is the pinned commit.
  - A fixture repository with exactly one injected drift per check.

  Keep a differential test that runs both tools while the old one exists, and prove it is not vacuous: capture the old tool's actual invocations, and check that a mutated port fails both the golden test and the differential.
- Resolve an external interpreter to its absolute path (`sys.executable`) outside the test's isolated environment, then run that path inside it.
- Settle permission-rule syntax with a one-call headless probe instead of reading it off the docs. Let the probe `Read` the file before it edits: `Edit` requires a prior read, and an edit that fails that precondition says nothing about whether the ask rule matched. The anchored `Edit(/tools/src/cmd/guard/**)` form works. The answer is recorded under Open Questions in `docs/knowledge/discussions/2026-09-22-jl-rust-agent-tools.md`.
- In subagent-driven execution, forbid `git stash` in every dispatch, because the stash stack is shared across worktrees and sessions. Also forbid working around a refused command with another command, and forbid touching credentials. One implementer did each of these.

## How to avoid next time

- `just tools` stamps `target/tools/release/aios` with the time the build started (`touch -r target/tools/.build-start`). A no-op build still makes the binary fresh, and a file edited during the build stays newer than the binary, so the next shim call rebuilds.
- Test a PR's own build with `AIOS_TOOLS_BIN="$PWD/target/tools/release/aios"`.
- Add a dependency in the PR that first uses it; `time` is approved but not yet declared.
- Before R2-R5 reuse `pystr` and `markdown`, reconsider the "`\d` is `[0-9]`" constraint and the use of the crate's plain `\s`. `\p{Nd}` and `[\s\x1c-\x1f]` match Python's `str`-pattern classes exactly, and these two choices produced about half of the listed divergences.
- Later ports with an old tool (R4, R3, R5) follow the same order:
  1. the fixture bundle and drift variants;
  2. an ignored recorder test;
  3. a differential test;
  4. the switch-over and the deletion, in the same PR.

  R5 is the exception to step 4: it switches to shadow mode, and the Python guard is deleted in R5b. R2 is new code: it gets the loop spec's tests with fake `gh`/`claude` and the eval suites, but no recorder, differential or deletion.
