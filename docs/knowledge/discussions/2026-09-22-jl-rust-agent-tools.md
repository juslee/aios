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
- no dependency on `/usr/bin/python3` or on GNU `timeout`. The GNU `timeout` requirement is what blocks PR #169's Ubuntu 26.04 runner, which ships uutils.
  - **Note (2026-09-24):** since #192 (`aa1f128`), `soak-qemu.sh` accepts any `timeout` that behaves correctly, uutils included, so #169 no longer waits on R4. #169 was rebased onto #192 (head `5285b16`), and its soak job now completes on ubuntu-26.04 with uutils `timeout`. The job is report-only and none of its boots is CLEAN: all 5 stop at a synchronous exception at the kernel load address (`0x4008_0000`) before any boot marker, unlike main's ubuntu-24.04 soak, whose boots reach EL1 and fail later.

Owner decisions, 2026-09-22:

| Topic | Decision |
| --- | --- |
| Scope | All of it: docs-check, the push guard, brief, checkpoint, precompact, soak, and the new PR loop, which is written in Rust from the start |
| Build model | A prebuilt binary called through a checking shim that fails closed |
| Dependencies | An ergonomic set: clap, anyhow, serde + serde_json, regex, time |
| Order | Infrastructure via the docs-check port first (R1), then the soak port (R4) so the crash fix can start, then the loop (R2), the scripts (R3), and the guard last in shadow mode (R5, R5b). The owner moved the soak port to second on 2026-09-22 |
| Location | `tools/` at the repository root, with the guard and loop sources protected by permission ask rules |
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

- Shared modules in `tools/src/`: `gh`, `git`, `proc` (subprocess, timeout, process groups), `json`, `config`, `paths`.
- One directory per subcommand in `tools/src/cmd/<name>/`.
- Tests in `tools/tests/`.
- Golden files in `tools/tests/golden/`.
- Fake `gh` and `claude` executables for tests, built as test-only binaries.

**Protection.** `.claude/settings.json` gets ask rules for `Edit` and `Write` on `tools/src/cmd/guard/**` and `tools/src/cmd/loop/**`.

- Interactive edits of those paths prompt the owner.
- The loop's headless fix stage gets a refusal instead, so it cannot silently change its own guard or merge logic, and such a PR ends at needs-human.
- The rest of `tools/` is freely editable.
- R1 must verify the exact rule syntax: path anchoring relative to the settings file.

**What stays out of the crate.** Retro-editable material stays in `scripts/agent/`: prompts, `loop-config.json` and eval cases.

### 2. Build, invocation, fail-closed

**Build.** `just tools` runs:

```sh
cargo build --release -p aios-tools --target-dir target/tools
```

The separate target directory means a tools build never waits on a kernel build's lock.

**Invocation.** Every caller goes through the POSIX `sh` shim `.claude/hooks/aios <subcommand> …`: hooks, skills, `just` recipes and `claude -p` stages.

The shim resolves the binary from the **main checkout**: the parent of `git rev-parse --path-format=absolute --git-common-dir`, plus `/target/tools/release/aios`. This holds even when the caller is in a PR worktree. So by default the guard and the loop run the code merged on main rather than a PR's, which is the same "runs from main" rule as the loop spec. That binary is an unprotected build artifact, not a verified one: a session that writes `<main>/target/tools` (a `cargo build --target-dir`, `cp` or `touch`, all auto-allowed today) can replace it with a build of unreviewed code, and because the freshness test below compares mtimes only, a future-dated replacement survives later pulls. How R5 closes this is an open question (below).

- `AIOS_TOOLS_BIN` overrides the binary path, so a PR's own build can be tested explicitly.
- CI runs `cargo test` on every PR.

**Freshness.** The binary must be newer than every file under the main checkout's `tools/` and than `Cargo.lock`. The shim checks this with a `find -newer` test, which takes a few milliseconds.

| State | `aios guard` (every shell command) | Other subcommands |
| --- | --- | --- |
| Fresh | runs | runs |
| Stale (e.g. just after a pull) | Runs the stale binary, built from an earlier main (unless a session replaced it, see above), and starts one background `just tools` (lock directory `target/tools/.building`, created with `mkdir`) | Rebuilds in the foreground (incremental, seconds), then runs |
| Missing | **Fails closed**: prints a PreToolUse `permissionDecision: "ask"` with the reason "aios tools not built; run just tools", exits 0 | Exits non-zero, naming `just tools` |

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

All loop behaviour, prompts, config and evals are unchanged.

**Documentation and rules, updated in the PR that changes them:**

- **`CLAUDE.md` workspace layout:** add `tools/`. R1 also fixes the stale `scripts/setup-dev-env.sh` entry, which now lives in `.claude/hooks/`.
- **Build-command tables:** README and the developer guide.
- **`docs/project/agent-loop.md`.**
- **Rule 05 (file placement):** add `tools/`.
- **Rule 01:** a note that "all dependencies must be `no_std`" applies to kernel and shared crates, not to host tools.

## Open Questions

- The exact permission-rule syntax that anchors `Edit(...)`/`Write(...)` ask rules to `tools/src/cmd/guard/**` in the project settings, and whether headless `claude -p` turns those asks into refusals. To be verified in R1 with a one-call check.
  - **R1 answer (anchored form):** `Edit(/tools/src/cmd/guard/**)` and `Write(/tools/src/cmd/guard/**)` match, a leading `/` being relative to the project root (the directory that holds `.claude/`), and headless `claude -p` turns the ask into a refusal (probe: guarded file unchanged, one denial each for `Edit` and `Write`, an unguarded control edit applied). `.claude/settings.json` has these rules for `guard` and `loop`.
- Whether `default-members` exclusion keeps `cargo build --target aarch64-unknown-none` and the kernel CI jobs from trying to build the host crate for the bare-metal target. To be verified in R1.
  - **R1 answer:** yes. With `default-members = ["kernel", "shared"]`, `cargo build --target aarch64-unknown-none -v` compiles nothing from `aios-tools`; the kernel CI jobs call recipes that build the default members or name a package with `-p`; `just test`, the only `--workspace` recipe, excludes `aios-tools`.
- How R5 protects the main checkout's binary before the guard runs through the shim (§2 Invocation). Nothing stops a session from replacing `<main>/target/tools/release/aios`, and the shim's freshness test compares mtimes only. Candidates, for an owner decision in R5 alongside the shim's `[ -x ]` directory case: a sandbox or filesystem write-deny on `target/tools/**` (Bash-pattern ask rules are easy to get around with `CARGO_TARGET_DIR`, `ln`, `mv` or `install`), or a provenance stamp (`just tools` records `git rev-parse HEAD:tools` and a `Cargo.lock` hash beside the binary, and the shim treats a mismatch as stale or missing).

## References

- `docs/knowledge/discussions/2026-09-22-jl-justin-review-merge-loop.md` (branch `claude/justin-review-merge-loop`): the approved PR-loop design this tooling implements.
- `docs/knowledge/decisions/2026-09-22-jl-crash-fix-preemption-and-fp.md` (PR #174): the crash-fix steps that use the soak harness.
- PR #169: the Ubuntu 26.04 runner. Its soak failed on uutils `timeout` before #192; since the rebase onto #192 the soak job completes on 26.04, so it does not wait on R4 (its boots are not CLEAN; see the note under Context).

## Outcome

_Fill in when graduated or archived:_

- Graduated to: `docs/project/agent-loop.md` and the developer guide when R5b lands.
