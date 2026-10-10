---
name: audit-loop
description: >
  Runs rule 02's audit on one claude/* branch before its PR is marked ready.
  Each round runs the committed audit-loop workflow (read-only doc and code
  lenses plus skeptic votes). Confirmed in-diff findings go to a writer
  spawned from the branch tip, and the lead fast-forwards and pushes the fix. Rounds repeat until a
  complete round confirms nothing, or the stop rule ends the audit. At most
  one audit per team at a time, and no new round during the other team's
  quiet window.
---

# /audit-loop <worktree>

You run in the main checkout and never edit the branch yourself (rule 11).

## Inputs

| Name | How to get it |
| --- | --- |
| `W` | the absolute worktree path |
| branch | `git -C <W> branch --show-current` |
| base | `git -C <W> merge-base origin/main HEAD` |
| context | the plan path, the issue numbers, and any owner decisions that limit scope |
| gates | the latest writer report's gate output; its sha must equal the head |

If the gate output is missing or was taken on another sha, first spawn the branch's writer with the task "run the gates and report" (no commit), as `/justin:team` describes.

### Mode

Classify `git -C <W> diff --name-only <base>..HEAD`, ignoring `*.md` files:

- **kernel**: any path under `kernel/`, `shared/` or `uefi-stub/`, or `rust-toolchain.toml`, `Cargo.toml` or `Cargo.lock`.
- **tools**: any path under `tools/`, `scripts/`, `.github/` or `.claude/`, or `justfile` or `.gitattributes`.
- **both**: kernel and tools paths both present; run both lens sets.
- **docs**: no path left after ignoring `*.md`.
- A non-`.md` path that matches none of the lists above counts as **tools**.

The docs lens runs in every mode. Re-detect the mode after every fix round.

## One round

1. Record `head=$(git -C <W> rev-parse HEAD)`. If the other team holds or has requested a quiet window (`qemu-lock.sh status` shows `mode=quiet`, or a `QUIET-REQ` you acknowledged is open), wait for `QUIET-END` before starting the round.
2. Run the `audit-loop` workflow with the Workflow tool, passing these args:
   - `worktree`, `branch`, `base`, `head`, `mode`, `context`, `gates`;
   - the ledger from earlier rounds: `fixed` and `refuted` findings (never re-reported), and `uncertain`, the previous round's `uncertain` findings (re-checked, and re-reported if still present).
3. The workflow returns:
   - `complete`: false when any lens failed (`lens_failures` is not empty) or a finding stayed unverified (`uncertain` is not empty);
   - `in_diff`: confirmed findings in this branch's diff;
   - `pre_existing`: confirmed findings that were already there;
   - `refuted` and `uncertain`.
4. Write the result to `<git-common-dir>/aios-agent/audit/<branch>/<head>.json`. `<git-common-dir>` comes from `git rev-parse --path-format=absolute --git-common-dir`, and `<branch>` has its `/` replaced by `-`. A fresh session resumes from this file after a usage-limit stop.
5. If `in_diff` is not empty, fix it:
   - Spawn the writer for the findings' area (kernel-dev, worker or doc-writer), as `/justin:team` describes, with the findings in its prompt and the commit message `Audit round <N>: fix <summary>`. Split by area when findings span areas.
   - When it reports gates passing, run `/justin:team`'s Placement on its range (a kernel-dev range gets the lead's Fable review before the fast-forward, rule 11), which ends with `git -C <W> push -u origin claude/<branch>`. Then start the next round with its gate output.
6. Pass this round's `uncertain` findings as the next round's `uncertain` arg for re-verification.

## Stop

- **Converged:** a round with `complete: true` and an empty `in_diff`.
- **Not converged:** after 4 rounds, or after 2 incomplete rounds in a row.
  - Leave the PR as a draft.
  - Open a `needs-human` issue titled `Gate: PR #<n> audit did not converge`, listing the open findings. The brief holds the PR until the owner rules.

A round that returns nothing because agents failed is incomplete, not clean.

## Pre-existing findings

Never fix pre-existing findings on this branch. For each confirmed one:

1. Search for an existing issue: `gh issue list --search "<file> <summary>"`.
2. If none exists, file one issue per defect, labelled `agent`, citing the branch, the round and the skeptic's evidence.
