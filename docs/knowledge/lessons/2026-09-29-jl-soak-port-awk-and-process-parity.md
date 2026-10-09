---
author: jl + claude
date: 2026-09-29
tags: [tooling]
status: final
---

# Lesson: Porting a bash + awk harness to Rust with byte-identical output

## What happened

PR R4 replaced `scripts/soak-qemu.sh` (the QEMU boot soak harness: an awk classifier, bash report writers and a `timeout --kill-after` boot loop) with `aios soak` in the host crate `tools/`. Parity was gated three ways:

- goldens recorded from the old script;
- differential tests that run the old script beside `aios`;
- a fake QEMU for the process handling.

The script was then deleted in the same PR.

Unlike R1 ([2026-09-24-jl-rust-port-parity-gotchas.md](2026-09-24-jl-rust-port-parity-gotchas.md)), the whole port was prototyped and run against the oracle before the plan was written. The plan's code tasks applied that prototype in slices. All seven task reviews were clean, with no parity divergence. The defects the run found were in the plan, and the controller's preflight scan caught them before any task ran: steps that could not run as written, and greps and claims that proved nothing.

On real logs the classifier matched the oracle on 354 local logs at planning time and on 417 at Task 8. CI's report-only soak ran the new harness end to end on the first push.

## Why it happened

- **awk numbers are not Rust numbers.** The classifier's awk did arithmetic on fields cut from serial logs. Two conversions matter.
  - **String to number** (`s + 0`) is C `atof` on the longest numeric prefix. That prefix can be `inf`, `nan` or hexadecimal (`0x1A`), and with no prefix the result is 0. `-0` becomes `0`.
  - **Number to string** prints integral values with `%.30g` (one-true-awk), not `%.6g`. A large tick count prints in full, not in exponent form.

  `str::parse::<f64>` and `format!("{}")` get every one of these wrong. On Ubuntu 26.04, mawk, gawk and original-awk printed the same classifier line as macOS's one-true-awk for all 108 corpus cases, so goldens recorded on macOS hold on CI's Linux.
- **Log fields are bytes.** Serial logs carry NUL, CR, ANSI escapes and UTF-8 that `clip()` cuts mid-character at 200 bytes, as awk's `substr` did. Any `String` on that path panics or replaces bytes. The script wrote those bytes unchanged to stdout, `summary.tsv` and `summary.md`.
- **clap swallows a leading `--`.** The script treated `--` as the end of its options. clap consumes it before the subcommand's parser sees it, so `aios soak` reparses its raw arguments after `soak` instead of taking clap's.
- **Process groups without `unsafe`.** `timeout --kill-after` killed QEMU and everything it forked. The crate is `#![forbid(unsafe_code)]`, so there is no `libc::kill(-pgid, …)`. The safe pieces are:
  - std's `CommandExt::process_group(0)`, which puts the child in its own group;
  - the `kill` utility (`kill -TERM -- -PGID`) to signal the group. BSD kill and procps both accept it, and both exit 1 for a group that is gone.
  - `signal-hook`'s flag registration, the one safe way to catch signals.
- **SIGHUP and SIGQUIT matter once the harness owns the time limit.** With `timeout` gone, nothing else bounds QEMU's run time. A terminal hangup or Ctrl-\ reaches only the foreground group, not QEMU's own group. A harness killed by either would leave QEMU running for good, skewing every later rate soak on the host. So the harness catches HUP and QUIT like INT and TERM (exit 129 and 131) and stops the group. SIGKILL cannot be caught, so a SIGKILLed harness still leaves its QEMU behind; the usage text says so.
- **The children still die of the signal the harness survives.** Once the handlers are installed, a Ctrl-C no longer ends `aios`, but `just create-data-disk` or `sha256sum` running in the terminal's foreground group dies of it and returns a failure. Reported as a setup error, that would exit 2 instead of 130. So any error after the handlers are installed yields to a pending signal's status, as the script's trap did.

## What we learned

- **Read the oracle from git, not the working tree.** The tests read `git cat-file blob <commit>:scripts/soak-qemu.sh` at a pinned commit, so the differentials keep running after the script is deleted. CI's `Tools (host)` job checks out full history, so the blob is always there.
- **Normalise only what must vary, and check what you normalise.** The fake-QEMU goldens replace these with placeholders:
  - paths (`<ROOT>`), timestamps and the scratch-directory suffix;
  - timings, load averages, the commit and the host line;
  - the timing columns of `summary.tsv`;
  - bash's job-control "Killed" notice.

  A placeholder hides a regression in exactly the value it replaces. `check_raw` therefore asserts ranges on the raw footers before normalisation: `qemu_rc` 124 or 137, and `elapsed` close to the limit. It also asserts that no process survives the scenario and no `.scratch.*` directory is left.
- **Prototype, then plan.** Port the whole tool once in scratch, and run it against the oracle on the corpus and on real logs. Then write the plan as slices of that verified patch, with a `SHA256SUMS` over the patch and the inputs. The implementers applied known-good code, so what remained to find was in the plan's prose. A preflight scan that checks every step against the tree and against the other tasks, before Task 1, caught these:
  - a restore step that used `git checkout --` on a file not yet tracked;
  - a clippy run cited as proof of a used dependency, though no default lint checks that;
  - an `export` expected to persist across agent shell calls;
  - a test whose name overstates what it compares.
- **List each input class that would hurt a user, with the test that pins it.** For a harness: a QEMU that outlives it; log bytes that are not clean UTF-8; running from a subdirectory or outside a checkout; an interrupted soak that must leave `summary.tsv` without `summary.md`; a host missing a tool. Reviewers checked each class against its named test.

## How to avoid next time

- For any awk or shell arithmetic being ported, port the value semantics first (`tools/src/cmd/soak/awk.rs`: `to_num`, `num_str`, `fmt_g`, `clip`, `trim`, `fields`). Use them everywhere, and test them against the real awk on a list of strings.
- Keep log-derived data as `Vec<u8>` from input to every output. Never use `from_utf8_lossy`, `str::lines`, `trim` or `split_whitespace` on it.
- Supervise any long-running child with `proc::Supervisor`: its own process group, SIGTERM at the limit, SIGKILL after the grace period, and a group stop on drop. Catch INT, TERM, HUP and QUIT with `signal-hook`, and let a pending signal decide the exit status of any error after that.
- Before writing a plan for a port, store the prototype patch and its inputs under `$(git rev-parse --git-common-dir)/aios-agent/port-inputs/<pr>/`, with a `SHA256SUMS`. The plan's code tasks apply slices of the patch with `git apply --include`.
- Leave settings edits out of subagent tasks. When a deleted script leaves a dead permission rule, the owner approves its removal as a separate commit (R4: `Bash(bash -n scripts/soak-qemu.sh)`).
