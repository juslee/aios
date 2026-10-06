---
author: jl + claude
date: 2026-09-29
tags: [tooling, agent-loop]
status: in-progress
---

# Tools R4: `aios soak` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `scripts/soak-qemu.sh` with `aios soak` in the host crate `tools/`: the same classifier output, reports, console text and exit statuses, with QEMU supervised by the harness itself instead of an external `timeout`. Then switch `just soak` and CI over and delete the script.

**Architecture:**
- The classifier is a line-by-line port of the script's awk program. It works on bytes, with awk's number semantics.
- The report writers, the boot loop and the command line are ports of the matching bash functions.
- A `Supervisor` in `tools/src/proc.rs` runs each QEMU in its own process group. It sends SIGTERM at the time limit and SIGKILL 10 s later, and reports `timeout(1)`'s exit statuses.
- Parity is gated in three layers:
  - goldens recorded from the old script;
  - differential tests that run the old script, read from git history at `212df62`, beside `aios`;
  - a fake QEMU for the process handling.

**Tech Stack:** Rust (std, the `aios-tools` crate), clap, `regex::bytes`, `signal-hook` 0.4.4 (new), and POSIX utilities run as subprocesses: `kill`, `date`, `uname`, `getconf`/`sysctl`, `sha256sum`/`shasum`, `git`, `just`, `mcopy` and `qemu-system-aarch64`.

**Spec:** [docs/knowledge/discussions/2026-09-22-jl-rust-agent-tools.md](../discussions/2026-09-22-jl-rust-agent-tools.md) §3, R4 row. **Lesson:** [docs/knowledge/lessons/2026-09-24-jl-rust-port-parity-gotchas.md](../lessons/2026-09-24-jl-rust-port-parity-gotchas.md).

-----

## Decisions

**Owner (2026-09-29):**

- Add `signal-hook` 0.4.4 (MIT OR Apache-2.0). It is the only safe way to catch SIGINT, SIGTERM and SIGHUP under `#![forbid(unsafe_code)]`.
- Commit **synthetic logs only**. Real soak logs are used in an uncommitted verification run (Task 8) and never committed.

**Owner (2026-10-06):**

- Execution is **subagent-driven** (superpowers:subagent-driven-development): a fresh implementer and a task review per task, then a whole-branch review and the audit loop.
- The seven controller rulings below are **accepted** as written. For the `.gitattributes` that Task 1 shares with #206, whichever of R4 and #205/#206 merges second drops the duplicate.
- origin/main (c6f5511: #210, #211, #218) was merged into this branch before Task 1, not before Task 7; the prototype patch still applies cleanly to the merged tree.

**Controller rulings (recorded here for review):**

1. **No `time` crate.** The default output directory's timestamp comes from `date +%Y%m%d-%H%M%S`, as in the script.
2. **The repository is the git checkout containing the working directory.** The shim runs main's binary, so the binary's own location says nothing about which checkout to boot. The `just` recipe stays `[no-cd]`.
3. **The oracle is the script at `212df62abcc4cbc024ea4011a408f1cd20a6494e`, read with `git cat-file`.** This is the #206 pattern. The differential tests keep running after the script is deleted, because CI's `Tools (host)` job checks out full history.
4. **Goldens are recorded on macOS** (awk version 20200816). On Ubuntu 26.04, mawk, gawk and original-awk print the same classifier line for all 108 cases, checked in Docker on 2026-09-29. So the goldens and differentials hold on CI's Linux too.
5. **Stderr messages start `soak: error:` / `soak: warning:`** (previously `soak-qemu:`). The golden recorder applies this one rename.
6. **SIGHUP is caught** like SIGINT and SIGTERM (exit 129). The harness now owns QEMU's time limit, so a harness killed by HUP would otherwise leave QEMU running forever.
7. **Accepted divergences,** listed in the module docs:
   - a step that `set -e` ended with the failing command's status (`cp`, `sha256sum`, `just --evaluate` of the image variables, an unreadable log) now ends with `soak: error:` and status 2;
   - numeric options with leading zeros are decimal everywhere (bash read them as octal in `$((...))`);
   - elapsed times come from a monotonic clock, so a reading can differ from bash's `$SECONDS` by 1 s;
   - bash's job-control notice for a SIGKILLed job is gone.

-----

## Global Constraints

- **Crate.** `tools/` (package `aios-tools`, binary `aios`), edition 2021, `#![forbid(unsafe_code)]`, host-only. It is not a default workspace member.
- **Dependencies.** Existing: clap 4.6.7, anyhow 1.0.104, regex 1.13.1, serde 1.0.229, serde_json 1.0.151. Add `signal-hook = { version = "0.4.4", default-features = false }` in Task 3, which first uses it. Its lock entries are `signal-hook` 0.4.4, `signal-hook-registry` 1.4.8 and `errno` 0.3.14, all MIT OR Apache-2.0, with `libc` unchanged at 0.2.186. Do not add `time`, `libc`, `nix` or `sha2`.
- **Bytes, not strings.** Log-derived fields stay `Vec<u8>` from input to `summary.tsv`, `summary.md` and stdout.
  - `clip()` cuts at a byte count and can split a UTF-8 character, as the script did.
  - Never use `String::from_utf8_lossy`, `str::lines`, `trim` or `split_whitespace` on log data.
- **awk semantics.** These live in `tools/src/cmd/soak/awk.rs`; use them everywhere:
  - string to number is C `atof` (`to_num`);
  - number to string is `%.30g` for integral values and `%.6g` otherwise (`num_str`);
  - `trim`, `clip` and `fields` are the awk program's.
- **The oracle** is the blob `212df62abcc4cbc024ea4011a408f1cd20a6494e:scripts/soak-qemu.sh`, read by `tests/common/soak.rs`.
- **Public repository.** Committed files must not contain `/Users/`, `/home/` or real soak logs. Test goldens hold only normalised paths (`<ROOT>`).
- **Porting inputs** live in `$(git rev-parse --git-common-dir)/aios-agent/port-inputs/r4/`: `synthetic.txt`, `r4-prototype.patch`, `bwk.out` and `SHA256SUMS`. Check them before Task 1:
  ```bash
  R=$(git rev-parse --git-common-dir)/aios-agent/port-inputs/r4
  (cd "$R" && shasum -a 256 -c SHA256SUMS)
  ```
  The expected sums are `synthetic.txt` 9dffb725…9886, `r4-prototype.patch` c382751a…2475 and `bwk.out` 1af70d3c…aac.
- **Per-task gate** (all must pass before the commit):
  - `cargo fmt --check -p aios-tools`
  - `cargo clippy -p aios-tools --all-targets -- -D warnings`
  - `cargo test -p aios-tools`
- **Commits.** Subject `Tools R4 Task N: <what>`, ending with the session's `Co-Authored-By` trailer. Push after each task with `git push -u origin claude/tools-r4-soak`. Never push to `main` or merge; the owner merges via `/merge-and-cleanup`.
- **Subagent rules:**
  - no `git stash` (the stack is shared across worktrees and sessions);
  - no working around a refused command with another command; report it instead;
  - no touching credentials (`ssh-add`, keychains, tokens);
  - no edits to `.claude/settings.json`;
  - work only inside `.claude/worktrees/tools-r4`.

-----

## Review Focus

The five input classes most likely to bite someone using `aios soak`. Each has a pinning test in the task that owns the code.

1. **A QEMU that outlives the harness.** QEMU and anything it forked must be gone when `aios soak` exits, on every path: the time limit, a QEMU that ignores SIGTERM, Ctrl-C, `kill`, SIGHUP, a setup error and a panic. An orphaned QEMU on this Mac would skew the crash-fix rate soaks for hours.
   - Task 3: `the_whole_process_group_is_stopped`, `a_child_that_ignores_sigterm_gets_sigkill_and_137`, `dropping_a_supervisor_stops_its_child`.
   - Task 6: the `interrupt-int` and `interrupt-term` goldens, `sighup_stops_qemu_and_cleans_up`, and the `still running: -` line in every harness golden.
2. **Log bytes that are not clean UTF-8.** NUL, CR, ANSI escapes and a field clipped inside a character must pass through to stdout, `summary.tsv` and `summary.md` byte for byte, with no panic and no lossy replacement.
   - Task 1: `clip-multibyte`, `nul-cr-bytes` and `ansi-escapes` in `classify.golden`, and `the_first_fatal_line_is_clipped_at_200_bytes_even_inside_a_character`.
   - Task 6: `panic-exit`, whose boot prints a `|` and an `é` that the goldens carry into all three outputs.
3. **Running from somewhere other than the repository root.** `just soak` is `[no-cd]`, so relative `out=` and `--classify` paths resolve against the invocation directory. The repository is found from a subdirectory. `--classify` works outside any checkout.
   - Task 5: `classify_works_outside_a_git_checkout`.
   - Task 6: the `subdir` and `default-out` goldens.
4. **An interrupted soak.** The output must keep `summary.tsv` with the finished boots' rows and have no `summary.md`, because `scripts/agent/brief.sh` reads "tsv without md" as a run in progress. The scratch directory must be removed, and a rerun into the same `out=` must be refused.
   - Task 6: `interrupt-int` (lists `run-01.log run-02.log summary.tsv`) and `out-not-empty`.
5. **A host missing a tool.** No QEMU on PATH, no `mcopy`, a firmware path that does not exist, or a failing build: each gets the script's message and exit status, or its documented fallback line.
   - Task 6: the `no-qemu`, `no-mcopy` and `build-fails` goldens.
   - Task 4: `sha256_16_is_the_digest_prefix` (the error path).

-----

## Prototype and evidence

The code in this plan exists and has been run. It lives in `r4-prototype.patch` (67 files against `212df62`), built and tested in a scratch detached worktree that has since been removed. The tasks apply it in six slices, whose staging was dry-run: every intermediate state passes the per-task gate, and the six slices together equal the patch byte for byte.

| Check | Result |
|---|---|
| Classifier vs the oracle's `CLASSIFY_AWK`, 108 synthetic cases (70 new, 38 from #168's review corpora) | identical |
| Classifier vs the oracle on 354 local `.log` files (real soak logs from `port-inputs/` and `target/soak/`) | identical (not committed) |
| `format_result`, the `--classify` block and `md_row` vs the script's bash functions, 384 lines | identical |
| `wilson` and the class share, k ≤ n ≤ 25 (351 pairs), and the load-average awk | identical |
| `to_num`/`num_str` vs awk (37 strings), `fmt_g` vs C `printf` (30 values × 2 precisions) | identical |
| `aios soak` vs `bash soak-qemu.sh`, 30 command lines (`--classify` and usage errors) | identical after the prefix rename |
| Harness vs the script, 16 fake-QEMU scenarios (timeout 124, kill-after 137, interrupts, build failure, `--out` errors, gpu, subdir, default out) | identical after normalisation |
| `kill -TERM -- -PGID` stops a group on macOS (BSD kill) and Ubuntu 26.04 (procps) | yes; exit 1 for a group that is gone |
| Mutations: clip 200→199; the footer's `hb_count` renamed | caught by the goldens and the differential |
| Full `cargo test -p aios-tools` with R1's suites | 143 unit tests plus all integration binaries pass, about 50 s |

-----

## Files

| File | Responsibility | Task |
|---|---|---|
| `tools/src/cmd/soak/awk.rs` | awk value semantics: `to_num`, `num_str`, `fmt_g`, `clip`, `trim`, `fields`, `find`/`contains` | 1 |
| `tools/src/cmd/soak/classify.rs` | `preprocess`, `classify` → `Classification` (11 byte fields), `CLASSES` | 1 |
| `tools/src/cmd/soak/report.rs` | `format_result`, `classify_details`, `md_cell`, `wilson`, `share`, `load_summary`, `tsv_row`, `md_row`, `summary_head`, `summary_table`, `TSV_HEADER` | 2 |
| `tools/src/proc.rs` | adds `Supervisor` and `signal_group` beside R1's `capture` | 3 |
| `tools/src/cmd/soak/signals.rs` | `Interrupts`: SIGINT/TERM/HUP → exit 130/143/129 | 3 |
| `tools/src/cmd/soak/host.rs` | `find_in_path`, `output_of`, `chomp`, `first_line`, `tail_lines`, `loadavg`, `load1`, `host_cpus`, `uname`, `sha256_16`, `git_rev`, `timestamp`, `just_evaluate`, `repo_root` | 4 |
| `tools/src/cmd/soak/runner.rs` | `Config`, `Progress` (`poll_log`), `footer`, `qemu_args`, `ScratchDir`, `run` (the boot loop), `KILL_AFTER` | 4 |
| `tools/src/cmd/soak/mod.rs` | `Args`, `USAGE`, `Request`, `parse`, `classify_files`, `run` | 1–4 (stages), 5 (final) |
| `tools/src/main.rs`, `tools/src/lib.rs`, `tools/src/cmd/mod.rs`, `tools/Cargo.toml` | the `soak` subcommand, crate docs, the dependency | 1, 3, 5 |
| `tools/tests/common/soak.rs` | the oracle, the corpus parser, golden helpers | 1 |
| `tools/tests/common/soak_fake.rs` | the fake QEMU, `just` and `mcopy`; scenarios; normalisation | 6 |
| `tools/tests/soak_classify.rs`, `soak_cli.rs`, `soak_harness.rs` | golden gates, recorders, differentials | 1, 5, 6 |
| `tools/tests/fixtures/soak/synthetic.txt` | 108-case corpus (`<NUL>`, `<CR>`, `<ESC>` placeholders) | 1 |
| `tools/tests/golden/soak/` | `classify.golden`, `cli/*.golden` (30), `harness/*.golden` (16) | 1, 5, 6 |
| `.gitattributes` | `-text` for fixtures and goldens (same content as #206) | 1 |
| `justfile`, `.github/workflows/ci.yml`, docs, rules, `scripts/agent/brief.sh`, `scripts/soak-qemu.sh` | switch-over and deletion | 7 |

**Interfaces the later tasks rely on:**

```rust
// awk.rs
pub fn to_num(s: &[u8]) -> f64;                 // `s + 0`, never -0
pub fn num_str(v: f64) -> String;               // awk number -> string
pub fn fmt_g(v: f64, precision: usize) -> String;
pub fn clip(s: &[u8], n: usize) -> Vec<u8>;
pub fn trim(s: &[u8]) -> Vec<u8>;
pub fn fields(line: &[u8]) -> impl Iterator<Item = &[u8]>;
pub fn contains(haystack: &[u8], needle: &[u8]) -> bool;
pub fn find(haystack: &[u8], needle: &[u8]) -> Option<usize>;
// classify.rs
pub const CLASSES: [&str; 6];                   // PCZERO PANIC EXCEPTION WEDGE INCONCLUSIVE CLEAN
pub struct Classification { pub class: &'static str, pub tick, hb, stall, markers: Vec<u8>,
                            pub lb: &'static str, pub detail, first: Vec<u8>, pub info: [Vec<u8>; 3] }
impl Classification { pub fn line(&self) -> Vec<u8>; }  // the 11 fields, tab-joined
pub fn preprocess(raw: &[u8]) -> Vec<u8>;
pub fn classify(raw: &[u8], limit_override: Option<u64>) -> Classification;
// proc.rs
pub struct Supervisor;  // spawn(Command, limit, kill_after) -> io::Result<Self>; id; service; exit_code; terminate; wait
pub fn signal_group(pgid: u32, signal: &str) -> anyhow::Result<()>;
// signals.rs
pub struct Interrupts;  // install() -> Result<Self>; pending() -> Option<u8>
// runner.rs
pub struct Config { runs_raw: String, runs: u64, secs_raw: String, secs: u64, stall_raw: String,
                    mode: String, out: Option<OsString>, build: bool, report_only: bool, fresh_data: bool }
pub fn run(cfg: &Config, cwd: &Path, out: &mut dyn Write, err: &mut dyn Write) -> anyhow::Result<u8>;
// mod.rs
pub fn run(args: &[OsString], cwd: &Path, out: &mut dyn Write, err: &mut dyn Write) -> anyhow::Result<u8>;
pub const USAGE: &str;
```

-----

## Task setup (every task)

All work happens in `.claude/worktrees/tools-r4` on `claude/tools-r4-soak`. Start each task with:

```bash
cd "$(git rev-parse --path-format=absolute --git-common-dir)/../.claude/worktrees/tools-r4"
R=$(git rev-parse --git-common-dir)/aios-agent/port-inputs/r4
PATCH=$R/r4-prototype.patch
inc() { local a=(); for p in "$@"; do a+=("--include=$p"); done; git apply "${a[@]}" "$PATCH"; }
```

`inc PATH...` applies the patch's changes to exactly those files. A file that a task edits by hand is never passed to `inc`.

Tasks 1–4 write a staged `tools/src/cmd/soak/mod.rs` that declares only the modules landed so far; Task 5 replaces it with the final file. The stage text is:

```rust
//! `aios soak`: the port of `scripts/soak-qemu.sh` (R4). Its modules land
//! ahead of the command line, which comes with this file's final version.

<the task's `pub mod` lines>
```

-----

### Task 1: The classifier, the corpus and its parity gates

**Files:**
- Create: `tools/src/cmd/soak/awk.rs`, `tools/src/cmd/soak/classify.rs`, `tools/src/cmd/soak/mod.rs` (stage 1)
- Create: `tools/tests/common/soak.rs`, `tools/tests/soak_classify.rs`, `tools/tests/fixtures/soak/synthetic.txt`, `tools/tests/golden/soak/classify.golden`
- Create: `.gitattributes`
- Modify: `tools/src/cmd/mod.rs` (`pub mod soak;`), `tools/tests/common/mod.rs` (`pub mod soak;`)

**Interfaces:** produces everything under `awk.rs` and `classify.rs` above, and the test helpers `oracle_script`, `oracle_awk`, `oracle_classify`, `run_oracle`, `rename_prefix`, `SynCase`, `parse_synthetic`, `synthetic_cases`, `write_cases`, `golden_dir`, `bless` and `check_golden`.

- [ ] **Step 1: Land the tests and the corpus first.**
  ```bash
  inc tools/tests/common/soak.rs tools/tests/soak_classify.rs tools/tests/fixtures/soak/synthetic.txt tools/tests/golden/soak/classify.golden
  sed -i '' 's/^pub mod fixture;$/pub mod fixture;\npub mod soak;/' tools/tests/common/mod.rs
  cmp tools/tests/fixtures/soak/synthetic.txt "$R/synthetic.txt"
  ```
- [ ] **Step 2: Run the tests to see them fail.**
  Run `cargo test -p aios-tools --test soak_classify`. Expected: a compile error, ``unresolved import `aios_tools::cmd::soak` ``.
- [ ] **Step 3: Land the implementation.**
  ```bash
  inc tools/src/cmd/mod.rs tools/src/cmd/soak/awk.rs tools/src/cmd/soak/classify.rs
  ```
  Write `tools/src/cmd/soak/mod.rs` as stage 1, with the module lines `pub mod awk;` and `pub mod classify;`.
- [ ] **Step 4: Run the tests to see them pass.**
  Run `cargo test -p aios-tools --lib soak::`, then `cargo test -p aios-tools --test soak_classify`. Expected: 17 unit tests (8 in `awk`, 9 in `classify`), then 3 integration tests pass with 2 ignored.
  - `classify_differential_against_oracle` has run the oracle from git history on all 108 cases.
  - `corpus_covers_every_class_and_stays_public_safe` checks that there are at least 5 cases per class and no local paths.
- [ ] **Step 5: Prove the recorder reproduces the golden.**
  ```bash
  cargo test -p aios-tools --test soak_classify -- --ignored record_classify_goldens_from_oracle
  git diff --exit-code -- tools/tests/golden/soak/classify.golden && cmp tools/tests/golden/soak/classify.golden "$R/bwk.out"
  ```
  Both must succeed. `bwk.out` is the independent pre-plan run through the script's own pipeline.
- [ ] **Step 6: Mutation check.** This is not committed.
  1. In `classify.rs`, change `clip(&trim(&line[start..]), 200)` to `199`.
  2. Run `cargo test -p aios-tools --test soak_classify`. Expected: `classify_goldens_match_aios` fails with "classify.golden differs", and the differential fails with "2 of 108 cases differ" (`clip-first-200`, `clip-multibyte`).
  3. Restore the file with `git checkout -- tools/src/cmd/soak/classify.rs`, then `git diff --stat` must show nothing for it.
- [ ] **Step 7: Add `.gitattributes`.**
  - If `git show origin/main:.gitattributes` succeeds (#206 merged first), leave the file alone; its rules already cover `tools/tests/`.
  - Otherwise create it byte-identical to #206's version, so whichever PR merges second merges cleanly:
    ```
    # The docs-check fixture bundles and goldens are compared byte for byte
    # (tools/tests/docs_check_parity.rs): no end-of-line conversion, whatever
    # core.autocrlf or text=auto says.
    tools/tests/fixtures/** -text
    tools/tests/golden/** -text
    ```
- [ ] **Step 8: Gate, commit, push.**
  Run the per-task gate. Then:
  ```bash
  git add .gitattributes tools/src/cmd/mod.rs tools/src/cmd/soak tools/tests/common tools/tests/soak_classify.rs tools/tests/fixtures/soak tools/tests/golden/soak
  git commit -m "Tools R4 Task 1: soak classifier, synthetic corpus, oracle goldens and differential"
  git push -u origin claude/tools-r4-soak
  ```

-----

### Task 2: Report writers

**Files:** Create `tools/src/cmd/soak/report.rs`. Modify `tools/src/cmd/soak/mod.rs` (stage 2).

**Interfaces:**
- Consumes: `awk::to_num`, `classify::{Classification, CLASSES}`.
- Produces: the report functions and types listed under Files, including `BootTiming` and `SummaryInfo<'a>`.

- [ ] **Step 1: Land the module; its tests are inline.**
  Run `inc tools/src/cmd/soak/report.rs`. Do not declare it yet.
- [ ] **Step 2: See that the tests are not built yet.**
  Run `cargo test -p aios-tools --lib soak::report`. Expected: 0 tests run; the module is not declared.
- [ ] **Step 3: Declare the module.**
  Write stage 2 of `mod.rs` with the module lines `pub mod awk;`, `pub mod classify;` and `pub mod report;`.
- [ ] **Step 4: Run the tests.**
  Run `cargo test -p aios-tools --lib soak::report`. Expected: 10 pass.
  - Every expected string in them was printed by the script's own `format_result`, `md_cell` and `wilson`, or by its awk one-liners.
  - They include the `%.0f` ties (`share(1, 8) == "12%"`, `share(3, 8) == "38%"`).
- [ ] **Step 5: Gate, commit, push.**
  Commit message: `Tools R4 Task 2: soak report writers (console line, summary.tsv, summary.md, Wilson interval)`.

-----

### Task 3: Process supervision and signals

**Files:**
- Modify: `tools/src/proc.rs` (adds `Supervisor` and `signal_group`), `tools/Cargo.toml` (the dependency), `Cargo.lock`.
- Create: `tools/src/cmd/soak/signals.rs`.
- Modify: `tools/src/cmd/soak/mod.rs` (stage 3).

**Interfaces:**
- Produces: `Supervisor::{spawn, id, service, exit_code, terminate, wait}` with `Drop`, `signal_group`, and `Interrupts::{install, pending}`.
- The exit-status contract: 124 when the limit stopped the child; 137 when SIGKILL was needed (or the child died of SIGKILL after the limit); otherwise the child's code, or 128 + its signal.

- [ ] **Step 1: Land the supervisor and its tests.**
  Run `inc tools/src/proc.rs`.
- [ ] **Step 2: Run the supervisor tests.**
  Run `cargo test -p aios-tools --lib proc::`. Expected: 11 pass in about 1–2 s: R1's 3, plus 8 supervisor tests covering exit codes, 124, 137, the process group, `terminate`, `Drop` and a group that is gone.
- [ ] **Step 3: Add the dependency and the signals module.**
  1. In `tools/Cargo.toml`, after the `serde_json` line, add:
     ```toml
     signal-hook = { version = "0.4.4", default-features = false }
     ```
  2. Run `inc tools/src/cmd/soak/signals.rs`.
  3. Write stage 3 of `mod.rs` with the module lines `pub mod awk;`, `pub mod classify;`, `pub mod report;` and `pub mod signals;`.
- [ ] **Step 4: Check the lock change.**
  Run `cargo build -p aios-tools`, then `git diff Cargo.lock`. Expected: exactly three new packages, `signal-hook` 0.4.4, `signal-hook-registry` 1.4.8 and `errno` 0.3.14, plus `"signal-hook"` in `aios-tools`' dependency list. `libc` stays at 0.2.186.
- [ ] **Step 5: Gate, commit, push.**
  The gate's clippy run also proves there is no `unused_dependencies` warning. Commit message: `Tools R4 Task 3: process-group supervisor (timeout semantics) and signal-hook interrupts`.

-----

### Task 4: Host probes and the boot loop

**Files:** Create `tools/src/cmd/soak/host.rs` and `tools/src/cmd/soak/runner.rs`. Modify `tools/src/cmd/soak/mod.rs` (stage 4).

**Interfaces:**
- Consumes: Tasks 1–3.
- Produces: `runner::{Config, Progress, footer, qemu_args, ScratchDir, run, KILL_AFTER}` and the `host` functions.
- `runner::run` is exercised end to end in Task 6. Here its pieces are unit-tested:
  - `Progress` against the script's `poll_log` arithmetic, including the `hb_max_gap` rule;
  - the footer bytes;
  - both QEMU argument vectors against the justfile recipes;
  - `ScratchDir`: private (0700), a `.scratch.XXXXXX` name, removed on drop.

- [ ] **Step 1: Land both modules.**
  Run `inc tools/src/cmd/soak/host.rs tools/src/cmd/soak/runner.rs`. Then write stage 4 of `mod.rs` with the module lines `pub mod awk;`, `pub mod classify;`, `pub mod host;`, `pub mod report;`, `pub mod runner;` and `pub mod signals;`.
- [ ] **Step 2: Run the tests.**
  Run `cargo test -p aios-tools --lib -- soak::host soak::runner`. Expected: 7 host and 4 runner tests pass.
  - `host_facts_have_the_script_shape` checks `loadavg`, `host_cpus`, `uname` and the timestamp on this host.
  - `git_rev_and_repo_root_in_a_scratch_repository` checks `-dirty`, `unknown` and a subdirectory.
- [ ] **Step 3: Gate, commit, push.**
  Commit message: `Tools R4 Task 4: soak host probes and boot loop (poll_log, footer, QEMU arguments, scratch directory)`.

-----

### Task 5: The `aios soak` command line and `--classify`

**Files:**
- Replace: `tools/src/cmd/soak/mod.rs` (final: `Args`, `USAGE`, `Request`, `parse`, `classify_files`, `run`, and the parser tests).
- Modify: `tools/src/main.rs` (a `Soak` subcommand that parses the raw `args_os()`), `tools/src/lib.rs`, and `tools/Cargo.toml` (the description).
- Create: `tools/tests/soak_cli.rs` and `tools/tests/golden/soak/cli/*.golden` (30 files).

**Interfaces:**
- Produces: `cmd::soak::{Args, USAGE, Request, parse, classify_files, run}`.
- `main` passes `std::env::args_os().skip(2)` because clap drops a leading `--`. The case `leading-dashdash-then-classify` pins this: `aios soak -- --classify x` must be "unexpected argument: --classify", as in bash.

- [ ] **Step 1: Land the CLI tests and goldens.**
  Run `inc tools/tests/soak_cli.rs 'tools/tests/golden/soak/cli/*'`.
- [ ] **Step 2: Run them to see them fail.**
  Run `cargo test -p aios-tools --test soak_cli`. Expected: a compile error, ``unresolved import `aios_tools::cmd::soak::USAGE` ``.
- [ ] **Step 3: Land the command line.**
  ```bash
  rm tools/src/cmd/soak/mod.rs
  inc tools/src/cmd/soak/mod.rs tools/src/main.rs tools/src/lib.rs
  sed -i '' 's/(docs-check; agent-loop commands in later PRs)/(docs-check, soak; agent-loop commands in later PRs)/' tools/Cargo.toml
  ```
- [ ] **Step 4: Run the tests.**
  Run `cargo test -p aios-tools --lib soak::`, then `cargo test -p aios-tools --test soak_cli --test cli`. Expected:
  - the 6 parser unit tests pass;
  - `soak_cli`: 4 pass (goldens, differential, help, outside a checkout) with 1 ignored;
  - R1's `cli` suite still passes, so `docs-check` is unchanged.
- [ ] **Step 5: The recorder reproduces the goldens.**
  ```bash
  cargo test -p aios-tools --test soak_cli -- --ignored record_cli_goldens_from_oracle
  git diff --exit-code -- tools/tests/golden/soak/cli
  ```
- [ ] **Step 6: Gate, commit, push.**
  Commit message: `Tools R4 Task 5: aios soak command line and --classify, with oracle goldens`.

-----

### Task 6: Harness scenarios with a fake QEMU

**Files:**
- Create: `tools/tests/common/soak_fake.rs`, `tools/tests/soak_harness.rs`, and `tools/tests/golden/soak/harness/*.golden` (16 files).
- Modify: `tools/tests/common/mod.rs` (`pub mod soak_fake;`).

**Interfaces:**
- Consumes: the `aios soak` binary; `common::soak::{oracle_script, rename_prefix, check_golden, golden_dir}`.
- The scenarios:
  - `panic-exit`, `clean-timeout` (124) and `kill-after` (137, about 12 s);
  - `stub-never-ran`, `gpu-inconclusive`, `build-fails` and `build-reuse-dirty`;
  - `no-mcopy`, `out-not-empty`, `out-is-file`, `out-is-root`;
  - `subdir`, `default-out`, `no-qemu`, `interrupt-int` and `interrupt-term`.
- Normalisation replaces:
  - paths with `<ROOT>` and scratch names with `.scratch.XXXXXX`;
  - footer times and `summary.tsv`'s stall, elapsed, load and harness-time columns;
  - the `stall=` console field, host and load rows, the commit id, and the default directory's timestamp;
  - bash's "Killed" job notice is dropped.
- `check_raw` then checks the real footer values the normalisation hides:
  - `clean-timeout`: `qemu_rc=124`, `elapsed` 3–4, `hb_last_advance` 1–2;
  - `kill-after`: `qemu_rc=137`, `elapsed` 12–13;
  - every scenario: no process left and no scratch directory.

- [ ] **Step 1: Land the helpers, tests and goldens.**
  ```bash
  inc tools/tests/common/soak_fake.rs tools/tests/soak_harness.rs 'tools/tests/golden/soak/harness/*'
  sed -i '' 's/^pub mod soak;$/pub mod soak;\npub mod soak_fake;/' tools/tests/common/mod.rs
  ```
- [ ] **Step 2: Run the harness tests.**
  Run `cargo test -p aios-tools --test soak_harness`. Expected: 2 pass (`harness_goldens_match_aios`, `sighup_stops_qemu_and_cleans_up`) with 2 ignored, in about 14 s. The fake scripts are POSIX `sh` (CI's `/bin/sh` is dash).
- [ ] **Step 3: The recorder reproduces the goldens.**
  This needs `timeout` or `gtimeout` on PATH (`brew install coreutils` on macOS).
  ```bash
  cargo test -p aios-tools --test soak_harness -- --ignored record_harness_goldens_from_oracle
  git diff --exit-code -- tools/tests/golden/soak/harness
  ```
- [ ] **Step 4: The differential.**
  Run `cargo test -p aios-tools --test soak_harness -- --ignored harness_differential_against_oracle`. Expected: pass, in about 30 s.
- [ ] **Step 5: Mutation check.** This is not committed.
  1. In `runner.rs` `footer()`, rename `hb_count=` to `hb_counts=`.
  2. Run `cargo test -p aios-tools --test soak_harness harness_goldens`. Expected: it fails with "harness/panic-exit.golden differs", among others.
  3. Restore the file with `git checkout -- tools/src/cmd/soak/runner.rs`.
- [ ] **Step 6: Gate, commit, push.**
  The whole `cargo test -p aios-tools` suite now takes about 50 s. Commit message: `Tools R4 Task 6: fake-QEMU harness scenarios, oracle goldens and differential`.

-----

### Task 7: Switch-over and deletion

**Files:**
- Modify: `justfile`, `.github/workflows/ci.yml`, `docs/project/developer-guide.md` (§5.6), `CLAUDE.md` (Workspace Layout), `.claude/rules/01-code-conventions.md` (host tool dependencies), `scripts/agent/brief.sh` (two comments), and `docs/knowledge/discussions/2026-09-22-jl-rust-agent-tools.md` (the dependencies list and the R4 row).
- Delete: `scripts/soak-qemu.sh`.

- [ ] **Step 1: Point the justfile recipe at the shim.**
  Keep `[no-cd]` and `[positional-arguments]`. Change the comment `key=value or --flags go to scripts/soak-qemu.sh` to `key=value or --flags go to aios soak`, and the body to:
  ```just
  soak *args:
      {{ quote(justfile_directory() / ".claude" / "hooks" / "aios") }} soak "$@"
  ```
  The shim finds the main checkout from its own location, so the call works from any directory. `aios soak` boots the checkout that contains the working directory (ruling 2).
- [ ] **Step 2: Build the tools in the CI soak job.**
  In the `qemu-soak` job of `.github/workflows/ci.yml`, add this step right before `Soak (5 boots x 90 s)`:
  ```yaml
        # just soak runs the aios binary (.claude/hooks/aios); build it first so
        # a build failure shows as its own step, not as a soak setup error.
        - name: Build the host tools
          run: just tools
  ```
  The step sits at the same indentation as the job's other `- name:` steps (six spaces).
- [ ] **Step 3: Update the docs.** Every sentence must match `tools/src/cmd/soak/mod.rs`'s `USAGE`.
  - `developer-guide.md` §5.6:
    - "(backed by `scripts/soak-qemu.sh`)" becomes "(backed by `aios soak` in the `tools/` crate)".
    - The command block's `scripts/soak-qemu.sh --help` becomes `.claude/hooks/aios soak --help`.
    - Replace the `timeout`/`gtimeout` paragraph with this text:

      > Each boot's QEMU runs in its own process group under the harness, which sends the group SIGTERM when `secs` run out and SIGKILL 10 s later if it is still running. QEMU's exit status is recorded as `timeout(1)` reported it: 124 when the time limit stopped it, 137 when SIGKILL was needed. No external `timeout` is needed. Ctrl-C (SIGINT), SIGTERM and SIGHUP stop the running QEMU and remove the scratch directory before the harness exits with 130, 143 or 129.

      The rest of that paragraph (fresh data disk, ESP snapshot, sha256, scratch directory, firmware) stays.
    - "The script warns when" becomes "The harness warns when".
    - "`scripts/soak-qemu.sh --classify LOG...`" becomes "`just soak --classify LOG...`".
    - In the CI bullet, "Ubuntu 24.04 QEMU and edk2" becomes "Ubuntu 26.04 QEMU and edk2". That is the current fact, since #169.
  - `CLAUDE.md` Workspace Layout:
    - The `scripts/` line becomes `agent/ (brief, checkpoint), docs/baseline.json (accepted docs drift)`.
    - The `tools/` line becomes `src/cmd/docs_check/ (docs drift checker), src/cmd/soak/ (boot soak harness, just soak), tests/ (goldens, fixtures)`.
  - `.claude/rules/01-code-conventions.md`: in "Its dependencies are the approved set clap, anyhow, serde, serde_json and regex", add "and signal-hook (R4)".
  - `scripts/agent/brief.sh`:
    - line 304 `# scripts/soak-qemu.sh writes summary.tsv…` becomes `# aios soak (just soak) writes summary.tsv…`;
    - line 379 `but soak-qemu.sh's out= can nest` becomes `but just soak's out= can nest`.
  - The spec, `2026-09-22-jl-rust-agent-tools.md`:
    - In §1's Dependencies list, add `signal-hook` with "(R4; owner-approved 2026-09-29: catching SIGINT, SIGTERM and SIGHUP needs it under `forbid(unsafe_code)`)", and note that `time` is still unused (R4 runs `date`).
    - Under §3's table, add one line: "R4 as built: 108 synthetic cases (real logs are verified locally, not committed); the oracle is the script's blob at `212df62`, read from git history, so the differentials outlive the deletion."
- [ ] **Step 4: Delete the script.**
  Run `git rm scripts/soak-qemu.sh`. Then run `git grep -n 'soak-qemu' -- ':!docs/knowledge/decisions' ':!docs/knowledge/lessons' ':!docs/knowledge/plans'`. Expected: only the spec's historical rows (the old line count, the #169 note, and the "Replaces" column).
- [ ] **Step 5: Check the result.**
  1. Build the PR's binary. The shim runs main's, which has no `soak` until this merges.
     ```bash
     cargo build --release -p aios-tools --target-dir target/tools
     export AIOS_TOOLS_BIN="$PWD/target/tools/release/aios"
     ```
  2. Run `just soak --help | head -1`. Expected: `Usage: aios soak [options] [key=value ...]`.
  3. Run `cd docs && just soak --report-only --classify ../tools/tests/fixtures/soak/synthetic.txt; cd ..`. Expected: exit 0 and one line (the bundle file classifies as one log).
  4. Run `just docs-check`. Expected: `0 new`. Fix any finding in the edited docs; do not update the baseline for it.
  5. Run `just check` and `just test`. Expected: zero warnings, and everything passes.
- [ ] **Step 6: Commit and push.**
  Commit message: `Tools R4 Task 7: just soak runs aios soak; CI builds the tools; docs; delete scripts/soak-qemu.sh`.

-----

### Task 8: Verification on real hardware and CI, lesson, PR

Nothing from Steps 1–3 is committed except the numbers quoted in the lesson and the PR body.

- [ ] **Step 1: Real-log differential (local only).**
  ```bash
  AIOS_SOAK_REAL_LOGS="$(git rev-parse --git-common-dir)/aios-agent/port-inputs:$(git rev-parse --path-format=absolute --git-common-dir)/../target/soak" \
    cargo test -p aios-tools --test soak_classify -- --ignored classify_differential_on_real_logs --nocapture
  ```
  Expected: pass, with the per-class counts printed. Before the plan the counts were 354 logs: CLEAN 68, EXCEPTION 21, INCONCLUSIVE 158, PANIC 20, PCZERO 31, WEDGE 56.
- [ ] **Step 2: Real soaks on this Mac.**
  1. **Preconditions:**
     - `pgrep -fl qemu-system-aarch64` prints nothing, so no other session's soak is running (crash-1b experiments);
     - `uptime`'s 1-minute load is noted.

     This is a functional check, not a rate soak, so high load does not block it. Record the load in the PR body.
  2. With `AIOS_TOOLS_BIN` set as in Task 7, run:
     - `just soak runs=2 out=target/soak/r4-text`. Expected: `just disk` builds, 2 boots are classified, both summaries are written, and the exit status is 0 or 1 by class;
     - `just soak --no-build mode=gpu runs=1 report_only=1 out=target/soak/r4-gpu`. Expected: a gpu-mode boot is classified, and exit 0.
  3. **Interrupt a real boot:**
     ```bash
     just soak --no-build runs=1 secs=60 out=target/soak/r4-int & sleep 15
     kill -INT $(pgrep -f 'aios soak' | head -1); wait
     ```
     Expected: `aios soak` exits 130 (just reports the recipe failed with that code), `pgrep -f qemu-system-aarch64` prints nothing, `target/soak/r4-int` holds `run-01.log` and `summary.tsv` only, and there is no `.scratch.*`.
  4. **Cross-check the new logs with the oracle:** rerun Step 1 with `AIOS_SOAK_REAL_LOGS="$PWD/target/soak"`. Expected: pass.
- [ ] **Step 3: CI soak on the new image.** The push run's `QEMU boot soak (report-only)` job is the re-measured CI baseline the spec asks for.
  1. Wait for it: `gh run list --branch claude/tools-r4-soak --limit 1`, then `gh run watch <id>`.
  2. Record its summary in the PR body: class counts, CLEAN rate and interval, QEMU version, and load.
  3. Compare it with the latest `main` soak on ubuntu-26.04 (`gh run list --branch main --workflow CI --limit 3`, then that job's summary).
  - A different rate is not a failure; the boot crash is still open.
  - A job that fails to run the harness is a failure.
- [ ] **Step 4: Lesson.** Write `docs/knowledge/lessons/2026-09-29-jl-soak-port-awk-and-process-parity.md`, with frontmatter `author: jl + claude`, `date`, `tags: [tooling]`, `status: final`. It covers:
  - awk's number rules (one-true-awk's `%.30g` for integral values; `atof` with hex and `inf`) and that four awks agree on the corpus;
  - byte fields;
  - clap swallowing a leading `--`;
  - process-group supervision with the `kill` utility under `forbid(unsafe_code)`, and why SIGHUP matters once the harness owns the time limit;
  - the git-blob oracle;
  - the fake-QEMU normalisation and what `check_raw` guards;
  - prototype-then-plan: the implementation was run against the oracle before the plan was written.
- [ ] **Step 5: Distil and delete the plan.**
  1. Move anything still useful from this plan into the lesson or the spec.
  2. Run `git rm docs/knowledge/plans/2026-09-29-jl-tools-r4-soak.md`.
  3. Commit with `Tools R4 Task 8: lesson; delete the working plan`.
- [ ] **Step 6: Final gates.**
  - `just check`, `just test`, `cargo test -p aios-tools` and `just build-release`.
  - The ignored R4 tests: the recorders must leave no diff, and the differentials must pass.
  - `/audit-loop` (rule 02) until a clean round, or until the owner stops it.
- [ ] **Step 7: PR.**
  1. Title: "Tools R4: aios soak (port of soak-qemu.sh), process supervision, switch-over". The body covers:
     - the summary;
     - the parity evidence (the table above, with Task 8's numbers);
     - the behaviour changes (rulings 5–7);
     - the settings: none changed;
     - follow-ups: crash-fix step 1a can start, since it waits for R4;
     - the test plan.
  2. Run `/review-pr-comments`.
  3. Hand off: report the PR URL and `gh pr checks`, and ask the owner to run `/merge-and-cleanup`.

-----

## Self-review notes

- **Spec coverage (R4 row):**
  - classifier parity on committed fixtures: Tasks 1 and 5;
  - fields checked: class, markers and first fatal line in `classify.golden`; the `summary.tsv`/`summary.md` formats in Task 2's tests and Task 6's goldens;
  - process handling with a fake QEMU: Task 6;
  - one real 2-boot soak: Task 8 Step 2;
  - the script is deleted and there is no `timeout` dependency: Task 7;
  - the CI baseline is re-measured: Task 8 Step 3.
  - The spec's "curated set of real logs" became a local-only differential, by owner decision.
- **Type consistency:** the interfaces above are copied from the prototype, which compiles as one crate.
- **Placeholders:** none. Every code change comes from `r4-prototype.patch` (checked by `SHA256SUMS`) or is spelled out in the step.
