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
| Scope | All of it: docs-check, the push guard, brief, checkpoint, soak, and the new PR loop, which is written in Rust from the start |
| Build model | A prebuilt binary called through a checking shim that fails closed |
| Dependencies | An ergonomic set: clap, anyhow, serde + serde_json, regex, time |
| Order | Infrastructure via the docs-check port first (R1), then the soak port (R4) so the crash fix can start, then the loop (R2), the scripts (R3), and the guard last in shadow mode (R5, R5b). The owner moved the soak port to second on 2026-09-22 |
| Location | `tools/` at the repository root, with the guard and loop sources protected by permission ask rules. On 2026-09-28 the owner made R2's changed-paths gate the guarantee, because the ask rules cover `Edit` and `Write` only and an allowed Bash command can bypass them (§1 Protection) |
| Soak vs crash fix | The soak port (R4) lands before crash-fix step 1a, so the harness is not changed twice |

**Note:** precompact is out of scope; the remember plugin and its PreCompact hook were retired by the two-team harness PR.

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
  - `signal-hook` (R4; owner-approved 2026-09-29: catching SIGINT, SIGTERM and SIGHUP needs it under `forbid(unsafe_code)`)
  - `time` (still unused: R4 runs `date`)

**Subcommands, and what each replaces:**

| Subcommand | Replaces |
| --- | --- |
| `aios docs-check [--all\|--json\|--markdown\|--list-checks\|--check X\|--update-baseline]` | `scripts/docs/check.py` |
| `aios loop review\|merge\|sweep\|eval …`, `aios retro` | the planned Python `pr_loop` / `retro` |
| `aios brief [--no-fetch]` | `scripts/agent/brief.sh` |
| `aios checkpoint [--handoff saved] [--allow PATH]…` | `scripts/agent/checkpoint.sh` |
| `aios soak …`, `aios soak --classify LOG…` | `scripts/soak-qemu.sh` |
| `aios guard` (PreToolUse hook: JSON on stdin) | `.claude/hooks/git-push-guard.py` |
| `aios hook` (repeat-error, path-guard, route-shadow, route-outcome) | added by #220 (model routing); `path-guard --agent-type` by the two-team harness PR |

**Source layout:**

- Shared modules in `tools/src/`: `gh`, `git`, `proc` (subprocess, timeout, process groups), `json`, `config`, `paths`, `pystr` (Python `str` semantics for ports: whitespace, `splitlines`, `isdigit`/`int`, text decoding, `unquote`; added in R1), `pyre` (compiles a Python `str` pattern so that `\d` and `\s` match exactly what CPython 3.14's `re` matches, Unicode 16.0; added in #205).
  - **Exact classes (#205, owner decision 2026-09-29).** R1 first ported `\d` as `[0-9]` and used the `regex` crate's plain `\s`, which lacks U+001C..U+001F; those two choices produced about half of R1's listed divergences. Every docs-check regex now compiles through `pyre::compile` and is check.py's pattern with these changes, none of which alters what a `\d` or `\s` matches: the five patterns that use lookaround (L171, L518, L534, L882, L951) are rewritten as code, and the `\s` in L171's lookahead is `pystr::is_space`, Python's class; three more (L302 `[^\w\- ]` in `gh_slug`, L791 `#\[test\]` and L1171 `[^a-z0-9 ]` in `norm_section`) are plain character or substring code with no regex; the seven path patterns (L94, L467, L1126, L1135, L1139, L1147 and L1278, the last two ported as the one `AGENT_FILE_RE`) end `\n?$` where check.py ends `$`, because Python's non-MULTILINE `$` also matches before a final newline; `re.match` and `re.fullmatch` become `^` and `$` anchors, and `re.M`, `re.I` and `re.S` arguments become inline flags; and some literal characters are escaped differently (`regex::escape` for `re.escape`, `"` for Python's `\"`, and `\^`, `\{`, `\}` and `\-` inside three classes, L166, L317 and L1344). The crate's Unicode `\d` is `\p{Nd}`, Python's `\d`, and `\s`/`\S` become `[\s\x1c-\x1f]`/`[^\s\x1c-\x1f]`. Both match CPython 3.14 (Unicode 16.0); CPython 3.12 and 3.13 (Unicode 15.x) lack 80 of the 760 `\d` code points, so the differential oracle (§3) accepts only a 3.14 interpreter. `pystr`'s digit helpers take any Unicode decimal digit, as `int()` does. R2-R5 compile their ported patterns the same way. The class and integer divergences that remain are `\w`/`\b` word classes, `(?i)` folding of `ı`/`İ`, `str.isdigit()` on non-decimal digits such as `²`, integers beyond `u64`, and the 4300-digit `int()` limit; the module docs list them along with R1's other accepted divergences.
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
- These rules and the gate protect the sources. The built binary that hooks run is protected separately, by a provenance stamp and permission rules on `target/tools/**` (§2 Install and provenance, #203). Those rules are a speed bump too, and their `Edit`/`Write` deny does not reach the main checkout's `target/tools` from a session started in a worktree.

**What stays out of the crate.** Retro-editable material stays in `scripts/agent/`: prompts, `loop-config.json` and eval cases.

### 2. Build, invocation, fail-closed

**Build.** `just tools` runs:

```sh
cargo build --release -p aios-tools --target-dir target/tools
```

The separate target directory means a tools build never waits on a kernel build's lock. The recipe then installs the binary and its provenance stamp (next paragraphs, #203).

**Invocation.** Every caller goes through the POSIX `sh` shim `.claude/hooks/aios <subcommand> …`: hooks, skills, `just` recipes and `claude -p` stages.

The shim resolves the binary from the **main checkout**: the parent of `git rev-parse --path-format=absolute --git-common-dir`, plus `/target/tools/installed/aios`. This holds even when the caller is in a PR worktree. So by default the guard and the loop run the main checkout's build rather than a PR's, and the provenance stamp (below) passes that build as fresh only when its inputs are on `origin/main`, merged through a PR. This is the same "runs from main" rule as the loop spec. The pin is only as firm as the git metadata git resolves the common dir from, which a session can rewrite (see the stamp's limits below).

**Install and provenance (#203).** `just tools` copies cargo's `target/tools/release/aios` to a temporary file and renames it to `target/tools/installed/aios`, a path cargo never writes, so a caller never finds that binary missing or half-written. Recipes run one at a time, from the cargo build through both renames, under a kernel file lock (`flock(2)`, taken through `flock(1)` on Linux or `lockf(1)` on macOS and the BSDs) on `target/tools/.install.lock`, so one recipe never copies `release/aios` while another's build removes and rewrites it. The kernel releases the lock when its recipe exits, however it dies, so no recipe has to judge a lock dead and take it over (a takeover cannot be made atomic in `sh`: two waiters can both take over one dead lock and run at once). cargo runs without the lock's descriptor, so a daemon it starts cannot keep the lock; a build orphaned by a killed recipe still holds cargo's own build-directory lock, which the next recipe's cargo waits for. Only after the binary is in place does it rename the provenance stamp `installed/aios.stamp` in beside it. The stamp holds:

- HEAD's tree entries (`git ls-tree HEAD`) for the build inputs: `tools/` (the tree hash `git rev-parse HEAD:tools` prints), `Cargo.lock`, `Cargo.toml`, `rust-toolchain.toml`, a legacy `rust-toolchain` file, `.cargo/` and the `justfile`, whose `tools` recipe builds the binary and writes the stamp. The repository has no `rust-toolchain`, but rustup prefers it to `rust-toolchain.toml` when both exist, so it is an input: an untracked one makes the build dirty, and so does a gitignored one;
- the git hash of the installed binary;
- `source dirty` if `git status --ignored` showed uncommitted changes to those inputs before or after the build (untracked and gitignored files count: cargo runs an ignored `tools/build.rs` and reads an ignored `.cargo/config`; exclude pathspecs leave out the OS and editor files no build reads, `.DS_Store`, `*.swp`, `*.swo`, `*~` and `*.rs.bk`, which Finder or an editor leaves in the main checkout), if the index marks an input file assume-unchanged or skip-worktree, which `git status` skips (`git ls-files -v` tags such a file in lowercase or `S`, every other file `H`), again before or after the build, if HEAD has committed input changes that `origin/main` (`refs/remotes/origin/main`) lacks (counted from their merge base, so a main checkout behind `origin/main` is clean), or if there is no `origin/main`; otherwise `source clean`. Without the `origin/main` test, an allowed `git commit` in the main checkout followed by `just tools` would stamp unreviewed code clean. This repository squash-merges, so such a commit never reaches `origin/main` itself, even after its change merges through a PR: the main checkout stays dirty until it is reset onto `origin/main` (`git reset --keep origin/main`). Both `git status` calls, the recipe's and the shim's, run with `--no-optional-locks`, so a background build never holds the main checkout's `index.lock` while other git commands run there. They also pin the work tree to the checkout (`--work-tree`) and turn off the config that lets `git status` skip reading files (`core.fsmonitor`, `core.untrackedCache`, `core.checkStat`, `core.trustctime`), since a session can set `core.worktree` or a file system monitor that reports no change in the main checkout's `.git/config` and so hide edited inputs from a plain `git status`. The test runs before the build as well as after it, so a file the build read and then removed (an ignored `tools/build.rs` that deletes itself, say) still stamps it dirty; a file that appears and goes again only while the build runs is not seen. Every git call in the recipe and the shim runs with replace refs and the grafts file off (`GIT_NO_REPLACE_OBJECTS=1`, `GIT_GRAFT_FILE` set to a path under `/dev/null` that cannot exist), since a plain file write under `.git/refs/replace/` or to `.git/info/grafts` could otherwise make an unmerged input commit look merged.

The shim recomputes the entries and the hash from the main checkout on every call and treats a missing or mismatched stamp as stale (Freshness, below). A binary built from another tree, or replaced after its build, no longer passes as fresh, whatever its mtime. `aios guard` checks and runs the same file: a hard link to the binary, made for the call in a fresh directory under `target/tools`, so a `just tools` rename between the check and the run cannot swap in a binary that was never checked (a link that cannot be made is an ask). The stamp records only that a build was dirty, not why, so on a dirty stamp the shim repeats the recipe's dirty test and names the cause it finds now.

The `justfile` is an input so that an edited `tools` recipe, uncommitted or not on `origin/main`, stamps the build dirty. That holds only while the edited recipe keeps the stamp code: the recipe runs before it writes the stamp, so a recipe edited to write its own `source clean` stamp is stamp forgery (next paragraph). The shim's background rebuild runs whatever recipe the main checkout's `justfile` holds, from the hook, with no prompt; a session can run that recipe itself with an allowed `just tools` anyway.

The stamp is provenance, not tamper-proofing. A session that can write `<main>/target/tools` can write a matching stamp beside its own binary, or poison cargo's intermediate artifacts there (`release/deps/`) so that the next `just tools` installs them with a valid stamp. The build environment is outside the stamp too: `RUSTC_WRAPPER`, `RUSTFLAGS`, a cargo config above the checkout or in `$CARGO_HOME`, a rustup directory override (`rustup override set`, kept in `~/.rustup/settings.toml`), `RUSTUP_TOOLCHAIN`, or a `just` option set through its environment variable (`JUST_JUSTFILE`, `JUST_DOTENV_COMMAND` and the like). `origin/main` is a local ref: a session that can move it (`git update-ref`, or a fetch with an explicit destination) can make unmerged commits look merged. Which checkout is the main checkout is git metadata as well: the shim takes the parent of `git rev-parse --git-common-dir`, which git resolves from files a session can rewrite with allowed commands (`echo`, `sed`): the main checkout's `.git/commondir`, and a linked worktree's `.git` gitdir file and its `.git/worktrees/<name>/commondir`. Pointed at a repository the session controls (a `cp -R` copy, say), they make the shim run that repository's binary, stamped `source clean` by its own `just tools` against its own `origin/main`, with no ask. The dirty test trusts the main checkout's `.git/config` and index the same way: it pins the work tree, turns off the config above and reads the index flags, but a clean filter (a `filter.<driver>.clean` or `.process` command in the config, given to input files by an attributes file) still makes `git status` see the committed content in place of an edit, so a background rebuild stamps it `source clean`. No permission rule covers those files. Keeping sessions out of `target/tools` and off `origin/main` is the job of permission rules in `.claude/settings.json`:

- a deny for `Edit` and `Write` on `**/target/tools/**`;
- asks for the common Bash write commands (`cp`, `mv`, `ln`, `install`, `tee`, `dd`, `rm`, `touch`, `sed -i` and a `>` redirection) when the command line names `target/tools`. The `sed` rules need `-i` or `--in-place` as written, so an in-place flag inside a cluster such as `-Ei` gets through;
- an ask for every `just` call that starts with an option. Options such as `-f`/`--justfile`, `-d`/`--working-directory`, `-E`/`--dotenv-path`, `-F`/`--dotenv-filename` and `--dotenv-command` run another justfile or load an environment file, so a different `tools` recipe, or the real one in an injected environment, would write a valid stamp. `just` takes options only before the first recipe name or override, so one prefix rule covers every option, in any order and in short-flag clusters. It also asks for read-only calls such as `just --list`. It does not cover a recipe named with a path (`just <dir>/tools` or `just <dir>/ tools`), which runs that directory's justfile with no option: an ask on every `just` call that contains a `/` would also catch redirections such as `2>/dev/null`, which headless stages would then have refused;
- asks for `git fetch` and `git pull` with a destination refspec under `remotes/` (`git fetch origin <branch>:refs/remotes/origin/main`), which would move `origin/main` without a merge. `git update-ref` and a push into the local repository (`git push . <commit>:refs/remotes/origin/main`) are not in the allow list, so they prompt anyway.

The Bash rules are a speed bump, as they are for the guard sources (§1 Protection); a sandbox or filesystem write-deny would be the guarantee. Ways around them found since (push security review, 2026-10-09), none of them new in #203: on a case-insensitive filesystem such as macOS APFS, a case variant of a path (`TARGET/TOOLS/installed/aios`, or `refs/Remotes/origin/main` as a fetch destination) names the same file but matches no ask, and so does a `//` or `/./` spelling; a `sed` script's `w` command writes a file without `-i`; allowed tools that run commands (`awk 'BEGIN{system(...)}'`, `find -exec`) reach any path; a `just` override with shell text (`just 'var=x; cmd' recipe`) runs `cmd` through any recipe that interpolates `var`; and the `origin/main` ref file can be written directly (`echo <sha> > .git/refs/remotes/origin/main`). Whether the `>` redirection rule matches at all depends on whether Claude Code matches rules against the command with its redirections removed, which is not probed. Closing these needs the guard itself (R5) or a sandbox, not more patterns. The `Edit`/`Write` deny has a gap too: like every project path rule, it applies under the session's own checkout (Open Questions, R1 answer), so a session started in a worktree, where rule 03 puts all work, is not denied writes to the main checkout's `target/tools`, which holds the binary the shim runs. Unlike R1's guard-source rules, the #203 rules were not probed headless.

- `AIOS_TOOLS_BIN` overrides the binary path, so a PR's own build can be tested explicitly. An override has no stamp and no freshness test: when it is a runnable file, `aios guard` runs it as a child as it would a fresh binary, with no ask.
- CI runs `cargo test` on every PR.

**Freshness.** The binary is fresh when both checks pass:

- It is newer than every file under the main checkout's `tools/` and than its `Cargo.lock`, `Cargo.toml`, `rust-toolchain.toml`, `rust-toolchain` (if there is one), `.cargo/` and `justfile`, so a pull that only bumps the pinned nightly still rebuilds it. The shim checks this with a `find -newer` test. `just tools` gives the binary its build's start time, so an edit made during a build still counts as newer. The test skips directories and the OS and editor files the dirty test leaves out: Finder or an editor rewrites them (vim a swap file every few seconds), and creating one moves its directory's mtime, so either would start rebuild after rebuild. A file removed since the build moves no mtime the test reads. Uncommitted, the removal leaves a clean build valid, because the recipe's dirty test also runs before the build, so a clean stamp means the build started without the file; committed, it changes HEAD's entries in the stamp.
- Its stamp matches the main checkout's HEAD and the binary itself, and says `source clean`. A binary that matches but says `source dirty` is dirty when the shim's repeat of the dirty test still fails: uncommitted, untracked or gitignored input files, input files that index flags hide from `git status`, committed input changes that `origin/main` lacks, or no `origin/main` to compare against. When the test now passes (a fetch of `origin/main` rewrites no input file, and a reset of the main checkout onto `origin/main` once its input commits have merged need not, so neither the mtime test nor HEAD's entries need change), the binary is stale instead, and a rebuild stamps it clean.

The stamp does not replace the mtime test, because HEAD's tree entries do not see uncommitted edits. On macOS, with a 3.7 MB binary, the checks take a guard call from about 33 ms (R1's shim) to about 68 ms, mostly for hashing the binary.

| State | `aios guard` (every shell command) | Other subcommands | `aios hook` (every Bash, edit and Agent call) |
| --- | --- | --- | --- |
| Fresh | Runs the binary as a child. Its own exit 0 (decision on stdout) and 2 (block) pass through; any other exit, its own included, becomes an ask | runs | runs a hard link to the binary as a child; its stdout only on exit 0, else the fallback |
| Dirty | **Fails closed**: asks, and starts no build (a rebuild would stamp it dirty again: the input files must be reverted or removed, any that are needed merged through a PR first; the main checkout must be reset onto `origin/main` once its input commits have merged, or if they are not wanted, since a squash merge never puts those commits themselves on `origin/main`; with no `origin/main`, it must be fetched; the ask names the cause the shim finds now). Once the cause is gone the binary is stale (next row) | runs | the fallback; no build |
| Stale (e.g. just after a pull, or a replaced binary) | **Fails closed**: asks, and starts one background `just tools` (lock directory `target/tools/.building`, created with `mkdir`; a lock left by a killed build is taken over after a minute or two, `find -mmin +1`, when no recipe holds `target/tools/.install.lock`). The reason says whether the build started, one was already running, or none could start (no `just` on `PATH`, neither `flock(1)` nor `lockf(1)`, which the recipe needs, or no lock directory) | Rebuilds in the foreground (incremental, seconds), then runs; if the rebuild fails, prints a warning naming `just tools` to stderr and runs the stale binary | the fallback, and one background `just tools` |
| Missing (not a non-empty regular executable file) | **Fails closed**: prints a PreToolUse `permissionDecision: "ask"` with the reason "aios tools not built; run just tools", exits 0 | Builds in the foreground (a full release build), then runs; exits 3 naming `just tools` if the build fails | the fallback, and one background `just tools` |

The `hook` fallback prints nothing, except `path-guard`'s PreToolUse deny for the agent types its `--agent-type` names (every caller when it names none or a value is empty) and for a payload with an `agent_id` but no `agent_type`.

Every `aios guard` path that runs neither a fresh binary nor a runnable `AIOS_TOOLS_BIN` override (which gets no stamp or freshness test) prints an ask and exits 0: the states above, an `AIOS_TOOLS_BIN` that is not a runnable file, and a shim that cannot find its own directory or the main checkout. Each of these paths has its own `permissionDecisionReason`. The stale state has one reason for each outcome of the background build (started, already running, could not start), each covering every cause of a stale binary (a newer input, a missing or mismatched stamp, git failing), the dirty state has one reason for each cause (uncommitted, untracked or gitignored input files; input files that assume-unchanged or skip-worktree flags hide from `git status`; input commits `origin/main` lacks; no `origin/main`), and a guard binary that exits other than 0 or 2 gets one reason for the main checkout's build and another for an `AIOS_TOOLS_BIN` override.

**R1 deviation (Missing, other subcommands).** The original design exited non-zero, naming `just tools`, without a build. R1's shim builds instead, which the final review accepted because it is friendlier. The cost: a caller such as `scripts/agent/brief.sh` blocks on a full release build on a fresh checkout. SessionStart's `aios --prebuild` (below) is the mitigation.

**R4 deviation (`just soak`).** `just soak` does not go through the shim: it depends on `just tools` and runs the checkout's own `target/tools/installed/aios` (the copy `just tools` installs, never cargo's `release/aios`, which a concurrent build can be rewriting), so from a PR worktree the harness that classifies the boots comes from the same commit as the kernel, and the commit in `summary.md` names both. A failed tools build stops the soak. Every other recipe, hook and skill still goes through the shim.

**Other build and CI hooks:**

- **Session start:** `.claude/hooks/setup-dev-env.sh` starts `just tools` in the background when the binary is missing or stale.
- **CI:** a new job, "Tools (host)", runs:
  - `cargo fmt --check -p aios-tools`
  - `cargo clippy -p aios-tools --all-targets -- -D warnings` (widened to `--all-targets` in R1 so test code is linted; `just clippy` runs the same command)
  - `cargo test -p aios-tools`, including the parity and golden tests, with `AIOS_REQUIRE_CHECK_PY=1` (#206, below)

  It becomes a required check on `main` right after R1 merges (owner decision, 2026-09-24), not when R5b switches the guard, because once `check.py` is deleted, this job's `goldens_match_aios` and the check.py differential are the parity gates.
- **`just check`:** gains the host clippy step for this crate.

### 3. Parity and switch-over

Each port proves parity, records the old tool's output as golden files, switches the call sites, and deletes the old script, all in the same PR. The goldens keep the parity tests alive after the old tool is gone.

**The old tool as an oracle after deletion (#206).** R1's differential test stopped comparing once `check.py` was deleted. It now materialises `check.py` from git history (`git cat-file blob 33c6b3d:scripts/docs/check.py`, the last version on main and the one the goldens were first recorded against) and runs it next to `aios` on every golden case and on the live checkout, wherever `python3` is CPython 3.14. The ignored golden recorder uses the same copy. #218 later moved the project memory to `.claude/CLAUDE.md` and made `aios` read only that path, so the oracle makes the same move (owner decision, 2026-10-09): the extracted `check.py` is patched with a small committed patch (`tools/tests/fixtures/docs-check/check-py-claude-md.patch`, applied with `git apply` beside `snapshot-claude-md.patch`, failing the test if it does not apply) that changes its `CLAUDE.md` paths and widens `BEFORE_CLAUDE_RE` exactly as #218 changed `aios`, and no check logic. The patched oracle reproduces the migrated goldens byte for byte. The oracle accepts only an interpreter whose `unicodedata.unidata_version` is 16.0.0, the version `pyre`'s classes match: CPython 3.12 and 3.13 (Unicode 15.x) lack 80 of those digits, so a test using one would fail against it although `aios` matches 3.14. The test skips, printing the reason, without such a `python3` or without that commit (a shallow clone). CI's Tools (host) job has both (`fetch-depth: 0`, and CPython 3.14 from `actions/setup-python`, whatever the runner image's own `python3` is) and sets `AIOS_REQUIRE_CHECK_PY=1`, which turns the skip into a failure there. R2-R5 can keep their own differential the same way after deleting their script. `.gitattributes` marks `tools/tests/fixtures/**` and `tools/tests/golden/**` `-text`, so a checkout with `core.autocrlf=true` cannot rewrite these byte-compared files.

| PR | Port | Parity proof | Switch-over |
| --- | --- | --- | --- |
| R1 | docs-check | Byte-identical stdout, exit code and written `baseline.json` against `check.py` in every mode. Test inputs: (a) the real repository, (b) a fixture repository with one injected drift per check (15), (c) a pure line-shift case | `just docs-check` calls `aios`; `check.py` is deleted; the baseline format is unchanged |
| R2 | PR loop | New code: the loop spec's tests with fake `gh`/`claude`, plus the eval suites | — |
| R3 | brief, checkpoint | The 15 checkpoint scenarios in temporary repos with a bare remote; a golden brief from recorded `gh` responses (the exact line format the skills parse) | Skills call `aios`; the scripts are deleted |
| R4 | soak | Classifier parity on committed fixtures: the 63 synthetic cases plus a curated set of real logs. Checked fields: class, markers, first fatal line, and the `summary.tsv`/`summary.md` formats. Process handling (timeout, kill-after, process group) is tested with a fake QEMU, then one real 2-boot soak | `soak-qemu.sh` is deleted; there is no external `timeout` dependency, and the CI baseline is re-measured on the new image |
| R5 | guard | The 55 unit tests ported as table tests. A committed adversarial corpus whose decisions must equal the Python guard's. The 3,502-command history replay is local-only, because raw transcript commands can contain secrets. The guard's rule 11 rules (QEMU starts through the lock wrapper, pattern kills and toolchain changes, spawn shape, agent placement: no EnterWorktree from agents or leads, `git reset --hard` only in an agent's own temporary worktree, no agent commits in the main checkout) are ported with their unit tests | **Shadow mode:** Python decides, Rust runs in parallel, and disagreements go to `.git/aios-agent/guard-shadow.jsonl` |
| R5b | guard switch | 1,000 real calls with 0 disagreements | The Rust guard decides; the Python guard and its tests are deleted |

R4 as built: 108 synthetic cases (real logs are verified locally, not committed); the oracle is the script's blob at `212df62`, read from git history, so the differentials outlive the deletion.

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

- **`.claude/CLAUDE.md` workspace layout:** add `tools/`. R1 also fixes the stale `scripts/setup-dev-env.sh` entry, which now lives in `.claude/hooks/`.
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
    - The provenance stamp (§2 Install and provenance). `just tools` records HEAD's tree entries for every build input: `tools/`, `Cargo.lock`, `Cargo.toml`, `rust-toolchain.toml`, a legacy `rust-toolchain`, `.cargo/` and the `justfile` (its `tools` recipe builds the binary and writes the stamp), the same list the mtime test covers (#203 added `.cargo/`, `rust-toolchain` and `justfile` to that test). The entry for `tools/` is the `git rev-parse HEAD:tools` tree hash. It also records the installed binary's git hash and whether the inputs were dirty: uncommitted changes (untracked and gitignored files included), input files that assume-unchanged or skip-worktree flags hide from `git status`, committed changes that `origin/main` lacks, or no `origin/main`. On a dirty stamp the shim repeats that test, names the cause it finds, and treats the binary as stale once the cause is gone. The shim treats a missing or mismatched stamp as stale, which for `guard` means an ask and a background rebuild. The binary's own hash is in the stamp because without it a binary replaced after a real build would still match. The mtime test stays, because HEAD's entries do not see uncommitted edits.
    - Permission rules on `target/tools/**` in `.claude/settings.json`: a deny for `Edit` and `Write`, asks for the common Bash write commands that name `target/tools`, and an ask for every `just` call that starts with an option (`-f`, `-d`, `-E`, `-F`, `--dotenv-command` and the rest), since those could run another justfile or load an environment file, so a different `tools` recipe would write a valid stamp (§2 Install and provenance). Like the Bash side of §1 Protection, these are a speed bump. A command that reaches the path indirectly (through a variable, a `cd`, a glob, a script, or a write-capable tool that is not listed) still gets through, and so do a `sed` in-place flag inside a cluster (`sed -Ei`), a `just` option set through its environment variable, and a `just` recipe named with a path (`just <dir>/tools`), which runs another justfile with no option. The `Edit`/`Write` deny, like any project path rule, applies under the session's own checkout: a session started in a worktree is not denied writes to the main checkout's `target/tools`. Asks also cover a `git fetch` or `git pull` whose destination refspec is under `remotes/` as spelled (a case variant such as `refs/Remotes/` gets through on macOS), since `just tools` stamps inputs clean only when `origin/main` holds them. The #203 rules were not probed headless.
    - Still open: the stamp cannot tell a session's own stamp from one `just tools` wrote (or from one a `tools` recipe edited to stamp itself clean wrote), and it trusts cargo's intermediate artifacts in `target/tools/release/deps/`, the build environment, the local `refs/remotes/origin/main` (a session that moves it makes unmerged commits look merged), the git metadata the shim's main-checkout resolution reads (`.git/commondir`, a linked worktree's `.git` gitdir file and `.git/worktrees/<name>/commondir`; a session that rewrites them points the shim at a repository it controls), the main checkout's `.git/config` and index that the dirty test reads (it pins the work tree, turns off `core.fsmonitor`, `core.untrackedCache` and the stat shortcuts, and counts assume-unchanged and skip-worktree flags as dirty, but a clean filter driver in the config still hides an edit from `git status`), and the object store (it turns off replace refs and the grafts file, but a rewritten object file or commit-graph still changes the commits git sees). Only a sandbox or a filesystem write-deny closes these.
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
- Whether R2's `aios loop` subcommands (merge, review) must exit 3 when a stale binary's foreground rebuild fails, instead of running the stale binary (§2 Freshness, Stale row). Since #203, "stale" also covers a binary that failed provenance (a missing or mismatched stamp: replaced after its build, or built from another tree), so the fallback runs an unverified binary, not only an earlier main's build. For `docs-check` that costs at worst a wrong report; for the loop it would run unverified merge logic. If yes, R2 changes the shim's stale branch for those subcommands, updates the shim header, and adds a shim test.

## References

- `docs/knowledge/discussions/2026-09-22-jl-justin-review-merge-loop.md` (branch `claude/justin-review-merge-loop`): the approved PR-loop design this tooling implements.
- `docs/knowledge/decisions/2026-09-22-jl-crash-fix-preemption-and-fp.md` (PR #174): the crash-fix steps that use the soak harness.
- PR #169: the Ubuntu 26.04 runner. Its soak failed on uutils `timeout` before #192 and at the kernel load address before #196. It merged on 2026-09-27 (`96b569a`), so main's soak runs on ubuntu-26.04 and R4 does not gate it (see the notes under Context).

## Outcome

_Fill in when graduated or archived:_

- Graduated to: `docs/project/agent-loop.md` and the developer guide when R5b lands.
