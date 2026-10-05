---
author: jl + claude
date: 2026-09-22
tags: [tooling, agent-loop, security, ci]
status: active
---

# Discussion: Rust agent tooling (`aios` host binary)

## Context

When this was written (2026-09-22, main at `33c6b3d`), the agent-loop tooling was Python and bash:

| Tool | Lines |
| --- | --- |
| `scripts/docs/check.py` (docs-check) | 1,670 |
| `.claude/hooks/git-push-guard.py` | 1,817, plus 709 lines of tests |
| `scripts/agent/brief.sh` | 508 |
| `scripts/agent/checkpoint.sh` | 285 |
| `scripts/soak-qemu.sh` | 853 |
| `.claude/hooks/precompact-save.sh` | 152 |

**Note (2026-09-24):** R1 replaced `scripts/docs/check.py` with `aios docs-check` and deleted it. The other line counts are as of `33c6b3d`.

The approved PR-loop design (`docs/knowledge/discussions/2026-09-22-jl-justin-review-merge-loop.md`, on branch `claude/justin-review-merge-loop` until R2 merges) planned a Python orchestrator.

The owner decided to move all of it to Rust. The main reasons:

- one language across the repository;
- types and `cargo test` for a long-lived state machine and a security parser;
- no dependency on `/usr/bin/python3` or on GNU `timeout`. The GNU `timeout` requirement blocked PR #169's Ubuntu 26.04 runner, which ships uutils.
  - **Note (2026-09-24, before #196):** since #192 (`aa1f128`), `soak-qemu.sh` accepts any `timeout` that behaves correctly, uutils included, so #169 no longer waited on R4. Rebased onto #192 (head `5285b16`), #169's soak job completed on ubuntu-26.04 with uutils `timeout`, but none of its boots was CLEAN: all 5 stopped at a synchronous exception at the kernel load address (`0x4008_0000`) before any boot marker, while main's soak, then on ubuntu-24.04, reached EL1 and failed later.
  - **Update (2026-09-28):** #196 (`7167d40`) fixed that fault. Ubuntu 26.04's strict-NX edk2 maps EfiLoaderData execute-never, and the stub now loads `PF_X` segments as `LOADER_CODE`. #169's final head (`8ad88aa`, on `7167d40`) reached EL1 in all 5 soak boots (1 CLEAN, 3 EXCEPTION at kernel virtual addresses, 1 WEDGE). #169 merged on 2026-09-27 as `96b569a`, so main's soak now runs on ubuntu-26.04.

Owner decisions, 2026-09-22:

| Topic | Decision |
| --- | --- |
| Scope | All of it: docs-check, the push guard, brief, checkpoint, precompact, soak, and the new PR loop, which is written in Rust from the start |
| Build model | A prebuilt binary called through a checking shim that fails closed |
| Dependencies | An ergonomic set: clap, anyhow, serde + serde_json, regex, time |
| Order | Infrastructure via the docs-check port first (R1), then the soak port (R4) so the crash fix can start, then the loop (R2), the scripts (R3), and the guard last in shadow mode (R5, R5b). The owner moved the soak port to second on 2026-09-22 |
| Location | `tools/` at the repository root, with the guard and loop sources protected by permission ask rules. On 2026-09-28 the owner made R2's changed-paths gate the guarantee, because the ask rules cover `Edit` and `Write` only and an allowed Bash command can bypass them (§1 Protection) |
| Soak vs crash fix | The soak port (R4) lands before crash-fix step 1a, so the harness is not changed twice |

## Design

### 1. Crate and layout

**The crate.** `tools/` is a Cargo workspace member.

- Package `aios-tools`, one binary `aios`, edition 2021, license BSD-2-Clause.
- It is in `members` but not in `default-members`, so a plain `cargo build` still builds only the kernel crates.
- It always builds for the host and uses the pinned nightly.
- Dependencies (MIT/Apache, audited by the existing Security CI job):
  - `clap` (derive)
  - `anyhow`
  - `serde`, `serde_json`
  - `regex`
  - `time`

**Subcommands, and what each replaces:**

| Subcommand | Replaces |
| --- | --- |
| `aios docs-check [--all\|--json\|--markdown\|--list-checks\|--check X\|--update-baseline]` | `scripts/docs/check.py` |
| `aios loop review\|merge\|sweep\|eval …`, `aios retro` | the planned Python `pr_loop` / `retro` |
| `aios brief [--no-fetch]` | `scripts/agent/brief.sh` |
| `aios checkpoint [--handoff saved] [--allow PATH]…` | `scripts/agent/checkpoint.sh` |
| `aios precompact` (hook: JSON on stdin) | `.claude/hooks/precompact-save.sh` |
| `aios soak …`, `aios soak --classify LOG…` | `scripts/soak-qemu.sh` |
| `aios guard` (PreToolUse hook: JSON on stdin) | `.claude/hooks/git-push-guard.py` |

**Source layout:**

- Shared modules in `tools/src/`: `gh`, `git`, `proc` (subprocess, timeout, process groups), `json`, `config`, `paths`, `pystr` (Python `str` semantics for ports: whitespace, `splitlines`, `isdigit`/`int`, text decoding, `unquote`; added in R1).
- One directory per subcommand in `tools/src/cmd/<name>/`.
- Tests in `tools/tests/`.
- Golden files in `tools/tests/golden/`.
- Fake `gh` and `claude` executables for tests, built as test-only binaries.

**Protection.** `.claude/settings.json` gets ask rules for `Edit` and `Write` on `tools/src/cmd/guard/**` and `tools/src/cmd/loop/**`.

- Interactive `Edit` and `Write` calls on those paths prompt the owner.
- The loop's headless fix stage gets a refusal for those calls instead.
- The rest of `tools/` is freely editable.
- R1 verified the rule syntax with headless probes (see Open Questions). The rules use the `**/` form (`Edit` and `Write` on `**/tools/src/cmd/guard/**` and `**/tools/src/cmd/loop/**`), because a leading `/` anchors at the project root of the session's checkout and does not match the same paths under `.claude/worktrees/*/`, where rule 03 puts all work.
- The ask rules cover the `Edit` and `Write` tools only. An allowed Bash command (`sed`, `awk`, `cp`, `echo` are in the allow list) can still rewrite those sources without a prompt, even headless; an R1 probe confirmed a `sed -i` rewrite. So the ask rules are a speed bump, not the guarantee. **The guarantee is R2's changed-paths gate:** the loop sends any PR whose diff touches `tools/src/cmd/guard/**` or `tools/src/cmd/loop/**` to needs-human instead of merging it, whichever tool made the change (owner decision, 2026-09-28). Those two globs are the minimum. The guard and loop binary is also built from the shared modules in `tools/src/`, `tools/src/main.rs`, `Cargo.toml`, `Cargo.lock` and `rust-toolchain.toml`, so a change there can alter guard or loop behaviour without touching either glob. The gate's full path set is an R2 decision (see Open Questions).
- These rules and the gate protect the sources. The built binary that hooks run is protected separately, by a provenance stamp and permission rules on `target/tools/**` (§2 Install and provenance, #203).

**What stays out of the crate.** Retro-editable material stays in `scripts/agent/`: prompts, `loop-config.json` and eval cases.

### 2. Build, invocation, fail-closed

**Build.** `just tools` runs:

```sh
cargo build --release -p aios-tools --target-dir target/tools
```

The separate target directory means a tools build never waits on a kernel build's lock. The recipe then installs the binary and its provenance stamp (next paragraphs, #203).

**Invocation.** Every caller goes through the POSIX `sh` shim `.claude/hooks/aios <subcommand> …`: hooks, skills, `just` recipes and `claude -p` stages.

The shim resolves the binary from the **main checkout**: the parent of `git rev-parse --path-format=absolute --git-common-dir`, plus `/target/tools/installed/aios`. This holds even when the caller is in a PR worktree. So by default the guard and the loop run the code merged on main rather than a PR's, which is the same "runs from main" rule as the loop spec.

**Install and provenance (#203).** `just tools` copies cargo's `target/tools/release/aios` to a temporary file and renames it to `target/tools/installed/aios`, a path cargo never writes, so a caller never finds that binary missing or half-written. Only after the binary is in place does it rename the provenance stamp `installed/aios.stamp` in beside it. The stamp holds:

- HEAD's tree entries (`git ls-tree HEAD`) for the build inputs: `tools/` (the tree hash `git rev-parse HEAD:tools` prints), `Cargo.lock`, `Cargo.toml`, `rust-toolchain.toml`, a legacy `rust-toolchain` file and `.cargo/`. The repository has no `rust-toolchain`, but rustup prefers it to `rust-toolchain.toml` when both exist, so it is an input: an untracked one makes the build dirty;
- the git hash of the installed binary;
- `source dirty` if `git status` showed uncommitted changes to those inputs after the build, otherwise `source clean`.

The shim recomputes the entries and the hash from the main checkout on every call and treats a missing or mismatched stamp as stale (Freshness, below). A binary built from another tree, or replaced after its build, no longer passes as fresh, whatever its mtime.

The stamp is provenance, not tamper-proofing. A session that can write `<main>/target/tools` can write a matching stamp beside its own binary, or poison cargo's intermediate artifacts there (`release/deps/`) so that the next `just tools` installs them with a valid stamp. The build environment is outside the stamp too: `RUSTC_WRAPPER`, `RUSTFLAGS`, a cargo config above the checkout or in `$CARGO_HOME`, a rustup directory override (`rustup override set`, kept in `~/.rustup/settings.toml`), `RUSTUP_TOOLCHAIN`, or a `just` option set through its environment variable (`JUST_JUSTFILE`, `JUST_DOTENV_COMMAND` and the like). Keeping sessions out of `target/tools` is the job of permission rules in `.claude/settings.json`:

- a deny for `Edit` and `Write` on `**/target/tools/**`;
- asks for the common Bash write commands (`cp`, `mv`, `ln`, `install`, `tee`, `dd`, `rm`, `touch`, `sed -i` and a `>` redirection) when the command line names `target/tools`. The `sed` rules need `-i` or `--in-place` as written, so an in-place flag inside a cluster such as `-Ei` gets through;
- an ask for every `just` call that starts with an option. Options such as `-f`/`--justfile`, `-d`/`--working-directory`, `-E`/`--dotenv-path`, `-F`/`--dotenv-filename` and `--dotenv-command` run another justfile or load an environment file, so a different `tools` recipe, or the real one in an injected environment, would write a valid stamp. `just` takes options only before the first recipe name or override, so one prefix rule covers every option, in any order and in short-flag clusters. It also asks for read-only calls such as `just --list`.

The Bash rules are a speed bump, as they are for the guard sources (§1 Protection); a sandbox or filesystem write-deny would be the guarantee.

- `AIOS_TOOLS_BIN` overrides the binary path, so a PR's own build can be tested explicitly. An override has no stamp and no freshness test: when it is a runnable file, `aios guard` runs it as a child as it would a fresh binary, with no ask.
- CI runs `cargo test` on every PR.

**Freshness.** The binary is fresh when both checks pass:

- It is newer than every file under the main checkout's `tools/` and than its `Cargo.lock`, `Cargo.toml`, `rust-toolchain.toml`, `rust-toolchain` (if there is one) and `.cargo/`, so a pull that only bumps the pinned nightly still rebuilds it. The shim checks this with a `find -newer` test. `just tools` gives the binary its build's start time, so an edit made during a build still counts as newer.
- Its stamp matches main's HEAD and the binary itself, and says `source clean`. A binary that matches but says `source dirty` is dirty: built from uncommitted input changes.

The stamp does not replace the mtime test, because HEAD's tree entries do not see uncommitted edits. On macOS, with a 3.7 MB binary, the checks take a guard call from about 33 ms (R1's shim) to about 68 ms, mostly for hashing the binary.

| State | `aios guard` (every shell command) | Other subcommands |
| --- | --- | --- |
| Fresh | Runs the binary as a child. Its own exit 0 (decision on stdout) and 2 (block) pass through; any other exit, its own included, becomes an ask | runs |
| Dirty | **Fails closed**: asks, and starts no build (a rebuild of uncommitted changes cannot fix it) | runs |
| Stale (e.g. just after a pull, or a replaced binary) | **Fails closed**: asks, and starts one background `just tools` (lock directory `target/tools/.building`, created with `mkdir`) | Rebuilds in the foreground (incremental, seconds), then runs; if the rebuild fails, prints a warning naming `just tools` to stderr and runs the stale binary |
| Missing (not a non-empty regular executable file) | **Fails closed**: prints a PreToolUse `permissionDecision: "ask"` with the reason "aios tools not built; run just tools", exits 0 | Builds in the foreground (a full release build), then runs; exits 3 naming `just tools` if the build fails |

Every `aios guard` path that runs neither a fresh binary nor a runnable `AIOS_TOOLS_BIN` override (which gets no stamp or freshness test) prints an ask and exits 0: the states above, an `AIOS_TOOLS_BIN` that is not a runnable file, and a shim that cannot find its own directory or the main checkout. Each ask names its cause in `permissionDecisionReason`.

**R1 deviation (Missing, other subcommands).** The original design exited non-zero, naming `just tools`, without a build. R1's shim builds instead, which the final review accepted because it is friendlier. The cost: a caller such as `scripts/agent/brief.sh` blocks on a full release build on a fresh checkout. SessionStart's `aios --prebuild` (below) is the mitigation.

**Other build and CI hooks:**

- **Session start:** `.claude/hooks/setup-dev-env.sh` starts `just tools` in the background when the binary is missing or stale.
- **CI:** a new job, "Tools (host)", runs:
  - `cargo fmt --check -p aios-tools`
  - `cargo clippy -p aios-tools --all-targets -- -D warnings` (widened to `--all-targets` in R1 so test code is linted; `just clippy` runs the same command)
  - `cargo test -p aios-tools`, including the parity and golden tests

  It becomes a required check on `main` right after R1 merges (owner decision, 2026-09-24), not when R5b switches the guard, because once `check.py` is deleted, `goldens_match_aios` in this job is the only parity gate.
- **`just check`:** gains the host clippy step for this crate.

### 3. Parity and switch-over

Each port proves parity, records the old tool's output as golden files, switches the call sites, and deletes the old script, all in the same PR. The goldens keep the parity tests alive after the old tool is gone.

| PR | Port | Parity proof | Switch-over |
| --- | --- | --- | --- |
| R1 | docs-check | Byte-identical stdout, exit code and written `baseline.json` against `check.py` in every mode. Test inputs: (a) the real repository, (b) a fixture repository with one injected drift per check (15), (c) a pure line-shift case | `just docs-check` calls `aios`; `check.py` is deleted; the baseline format is unchanged |
| R2 | PR loop | New code: the loop spec's tests with fake `gh`/`claude`, plus the eval suites | — |
| R3 | brief, checkpoint, precompact | The 15 checkpoint scenarios in temporary repos with a bare remote; a golden brief from recorded `gh` responses (the exact line format the skills parse); precompact's no-op and plugin-resolution cases | Skills call `aios`; the scripts are deleted |
| R4 | soak | Classifier parity on committed fixtures: the 63 synthetic cases plus a curated set of real logs. Checked fields: class, markers, first fatal line, and the `summary.tsv`/`summary.md` formats. Process handling (timeout, kill-after, process group) is tested with a fake QEMU, then one real 2-boot soak | `soak-qemu.sh` is deleted; there is no external `timeout` dependency, and the CI baseline is re-measured on the new image |
| R5 | guard | The 55 unit tests ported as table tests. A committed adversarial corpus whose decisions must equal the Python guard's. The 3,502-command history replay is local-only, because raw transcript commands can contain secrets | **Shadow mode:** Python decides, Rust runs in parallel, and disagreements go to `.git/aios-agent/guard-shadow.jsonl` |
| R5b | guard switch | 1,000 real calls with 0 disagreements | The Rust guard decides; the Python guard and its tests are deleted |

**Porting inputs.** The fixtures and corpora are preserved outside the repository in `$(git rev-parse --git-common-dir)/aios-agent/port-inputs/`:

- the synthetic and real soak logs;
- the CI soak artifacts;
- the guard's round-2 and round-3 corpora;
- the replay tool.

Each port PR commits the curated subset it needs.

### 4. Sequence, related changes, interactions

**Order:** R1 → R4 → R2 → R3 → R5 → R5b, one PR each. The owner moved the soak port to second on 2026-09-22. Crash-fix step 1a (ADR #174) waits for R4, so its harness changes are made once, in Rust, and the crash fix can start after two tooling PRs.

**Loop spec amendments** (made in R2's first commit):

- `scripts/agent/pr_loop.py` becomes `tools/src/cmd/loop/`, invoked as `.claude/hooks/aios loop …`.
- The Python 3.9 standard-library constraint becomes the crate constraints above.
- The fake-executable tests become test binaries.
- The merge stage sends any PR whose diff touches `tools/src/cmd/guard/**` or `tools/src/cmd/loop/**` to needs-human instead of merging it (§1 Protection; owner decision, 2026-09-28). This narrows the loop spec's auto-merge scope, which is "Everything".

Apart from that gate, all loop behaviour, prompts, config and evals are unchanged.

**Documentation and rules, updated in the PR that changes them:**

- **`CLAUDE.md` workspace layout:** add `tools/`. R1 also fixes the stale `scripts/setup-dev-env.sh` entry, which now lives in `.claude/hooks/`.
- **Build-command tables:** README and the developer guide.
- **`docs/project/agent-loop.md`.**
- **Rule 05 (file placement):** add `tools/`.
- **Rule 01:** a note that "all dependencies must be `no_std`" applies to kernel and shared crates, not to host tools.

## Open Questions

- The exact permission-rule syntax that anchors `Edit(...)`/`Write(...)` ask rules to `tools/src/cmd/guard/**` in the project settings, and whether headless `claude -p` turns those asks into refusals. To be verified in R1 with a one-call check.
  - **R1 answer (`**/` form):** headless `claude -p` turns these ask rules into refusals, checked with probes on Claude Code 2.1.280.
    - The anchored `Edit(/tools/src/cmd/guard/**)` and `Write(/tools/src/cmd/guard/**)` match at the root of the session's checkout, the directory that holds `.claude/`. They do not match the same paths under `.claude/worktrees/*/`: a nested edit was applied with no denial.
    - `Edit(**/tools/src/cmd/guard/**)` and `Write(**/tools/src/cmd/guard/**)` match both places. The probe got denials for a root `Edit`, a nested worktree `Edit` and a nested `Write`, and an unguarded control edit was applied.
    - `.claude/settings.json` has the `**/` rules for `guard` and `loop`.
    - The rules do not cover Bash. An allowed `sed -i` rewrote a guarded file. A `Bash(*tools/src/cmd/guard*)` ask rule does override that allow, but the owner chose not to add Bash patterns. R2's changed-paths gate is the guarantee (§1 Protection).
- Whether `default-members` exclusion keeps `cargo build --target aarch64-unknown-none` and the kernel CI jobs from trying to build the host crate for the bare-metal target. To be verified in R1.
  - **R1 answer:** yes. With `default-members = ["kernel", "shared"]`, `cargo build --target aarch64-unknown-none -v` compiles nothing from `aios-tools`; the kernel CI jobs call recipes that build the default members or name a package with `-p`; `just test`, the only `--workspace` recipe, excludes `aios-tools`.
- Which paths R2's changed-paths gate covers (§1 Protection). The owner made the gate the guarantee for the guard and loop sources, but their behaviour also depends on every input of the `aios` binary: the shared modules, `main.rs`, the manifests and the pinned toolchain. There are two options, for an owner decision in R2:
  - Option A widens the gate to all of those inputs. Every tools PR would then need human review, R2-R5 included.
  - Option B keeps the two globs and narrows the claim from "the guarantee" to "a check on direct edits".
- How R5 protects the main checkout's binary before the guard runs through the shim (§2 Invocation). Nothing stops a session from replacing `<main>/target/tools/release/aios`, and the shim's freshness test compares mtimes only. Candidates, for an owner decision in R5 alongside the guard branch's fail-open exits (next question): a sandbox or filesystem write-deny on `target/tools/**` (Bash-pattern ask rules are easy to get around with `CARGO_TARGET_DIR`, `ln`, `mv` or `install`), or a provenance stamp (`just tools` records `git rev-parse HEAD:tools` and a `Cargo.lock` hash beside the binary, and the shim treats a mismatch as stale or missing).
  - **#203 answer (2026-09-29; owner decision: both measures):**
    - The provenance stamp (§2 Install and provenance). `just tools` records HEAD's tree entries for every build input: `tools/`, `Cargo.lock`, `Cargo.toml`, `rust-toolchain.toml`, a legacy `rust-toolchain` and `.cargo/`, the same list the mtime test covers (#203 added `.cargo/` and `rust-toolchain` to that test). The entry for `tools/` is the `git rev-parse HEAD:tools` tree hash. It also records the installed binary's git hash and whether the inputs had uncommitted changes. The shim treats a missing or mismatched stamp as stale, which for `guard` means an ask and a background rebuild. The binary's own hash is in the stamp because without it a binary replaced after a real build would still match. The mtime test stays, because HEAD's entries do not see uncommitted edits.
    - Permission rules on `target/tools/**` in `.claude/settings.json`: a deny for `Edit` and `Write`, asks for the common Bash write commands that name `target/tools`, and an ask for every `just` call that starts with an option (`-f`, `-d`, `-E`, `-F`, `--dotenv-command` and the rest), since those could run another justfile or load an environment file, so a different `tools` recipe would write a valid stamp (§2 Install and provenance). Like the Bash side of §1 Protection, these are a speed bump. A command that reaches the path indirectly (through a variable, a `cd`, a glob, a script, or a write-capable tool that is not listed) still gets through, and so do a `sed` in-place flag inside a cluster (`sed -Ei`) and a `just` option set through its environment variable.
    - Still open: the stamp cannot tell a session's own stamp from one `just tools` wrote, and it trusts cargo's intermediate artifacts in `target/tools/release/deps/` and the build environment. Only a sandbox or a filesystem write-deny closes these.
- How R5 closes the shim's `guard` exits that fail open, before `aios guard` is wired as the PreToolUse hook. Claude Code treats a PreToolUse exit other than 0 or 2 as a non-blocking error and runs the tool, so each of these breaks §2's promise that `aios guard` fails closed:
  - **Directory case.** `[ -x ]` is true for a directory, so `AIOS_TOOLS_BIN=<dir> aios guard` passes the test and `exec` exits 126 with empty stdout. The fix is `[ -f ] && [ -x ]`.
  - **Rebuild window.** The stale branch tests `[ -x "$bin" ]`, starts a background `just tools`, and only then runs `exec "$bin"`. Cargo's uplift removes the old `target/tools/release/aios` before it links (Linux) or copies (macOS) the new one. On macOS it does this on every build, a no-op build included, so the file is re-created (a new inode) each time. On Linux the file is a hard link to the deps artifact, which cargo leaves in place when every unit is fresh, so there the window opens only on builds that relink. That includes the build after one that recompiled the `aios_tools` lib, because `just tools`'s `touch -r` also moves the shared inode's mtime back before the lib's rlib. On Linux the link is atomic, so the window only leaves the file missing, never half-written. A guard call in that window finds no binary, or (macOS) a half-written one, at `exec`, and exits with empty stdout: 126 or 127, and under macOS `/bin/sh` (bash 3.2) also 1 (bash's "Undefined error: 0" path) or 137 (SIGKILL from AppleSystemPolicy on a half-written file). On macOS, every guard call made while the shim's own background build runs can hit it; on Linux, only calls made during a build that relinks can. The other subcommands share the window when callers run at the same time; for `docs-check`, an exit 1 with empty stdout reads as new drift.
  - **Pre-dispatch `exit 3`.** The shim exits 3 before it looks at the subcommand when the shell cannot enter the hook's directory, or, when git cannot name the common dir, its parent's parent. This is close to unreachable, but it applies to `guard` too.

  Candidates: dispatch on `guard` before those exits and print the ask there; run the binary instead of `exec`ing it and map its failure to the ask, where mapping (or retrying on) 126, 127 or a binary that has vanished does not close exits 1 and 137, because by then the binary exists again, and only treating every exit other than 0 or 2 with empty stdout as the ask does; or have `just tools` install the binary with an atomic rename (a copy to a temporary name, then `mv -f`) to a path cargo never writes, such as `target/tools/bin/aios`, and have the shim run that path, so it never goes missing or half-written. Renaming into `target/tools/release/aios` keeps the window, because cargo re-creates that file on every build.
  - **#203 answer (2026-09-29):** all three are closed, each with a shim test in `tools/tests/shim.rs`.
    - Directory case. The shim runs a binary, `AIOS_TOOLS_BIN` or the main checkout's, only if it is a regular, executable, non-empty file (`[ -f ] && [ -x ] && [ -s ]`). It must be non-empty because an empty executable file runs as an empty script and exits 0, which Claude Code reads as no objection.
    - Rebuild window. Both fixes are in:
      - `just tools` installs by rename at `target/tools/installed/aios`, so that file is never missing or half-written. The name is `installed/`, not `bin/`, because `cargo install --root target/tools` writes `target/tools/bin/`.
      - The guard branch runs the binary as a child instead of `exec`. It passes through only the binary's own exit 0 and 2, and turns every other exit into an ask, whatever the stdout. That includes the binary's own other exits, such as 1 for an error or 101 for a panic: the guard did not decide, and passing those exits on would run the tool. A usage error from clap exits 2, so it blocks the tool, which also fails closed.
    - Pre-dispatch exits. They go through one helper: it prints the ask for `guard`, exits 0 for `--prebuild`, and exits 3 for any other subcommand.
- Whether R2's `aios loop` subcommands (merge, review) must exit 3 when a stale binary's foreground rebuild fails, instead of running the stale binary (§2 Freshness, Stale row). For `docs-check` the fallback costs only a report from an earlier checker; for the loop it would run an earlier main's merge logic. If yes, R2 changes the shim's stale branch for those subcommands, updates the shim header, and adds a shim test.

## References

- `docs/knowledge/discussions/2026-09-22-jl-justin-review-merge-loop.md` (branch `claude/justin-review-merge-loop`): the approved PR-loop design this tooling implements.
- `docs/knowledge/decisions/2026-09-22-jl-crash-fix-preemption-and-fp.md` (PR #174): the crash-fix steps that use the soak harness.
- PR #169: the Ubuntu 26.04 runner. Its soak failed on uutils `timeout` before #192 and at the kernel load address before #196. It merged on 2026-09-27 (`96b569a`), so main's soak runs on ubuntu-26.04 and R4 does not gate it (see the notes under Context).

## Outcome

_Fill in when graduated or archived:_

- Graduated to: `docs/project/agent-loop.md` and the developer guide when R5b lands.
