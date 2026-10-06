---
author: jl + claude
date: 2026-10-06
tags: [tooling]
status: final
---

# Lesson: Spec-correct code can still carry the bypass: path-guard

## What happened

`aios hook path-guard` (`tools/src/cmd/hook/path_guard.rs`) is a `PreToolUse` hook. It keeps the Sonnet `worker` agent from editing `kernel/`, `uefi-stub/` and `shared/` (see [the model routing ADR](../decisions/2026-10-06-jl-model-routing-hooks.md)). Its working plan was reviewed, implemented and reviewed in several rounds. Each round was clean against the plan. The code still had four ways around the guard, and each one let the worker edit kernel code with no decision:

1. **Nested worktrees.** The first version took the repository root from the input `cwd`, but every AIOS worktree lives inside the main checkout under `.claude/worktrees/<name>/`. With `cwd` at the main checkout, an `Edit` of `<main>/.claude/worktrees/x/kernel/src/main.rs` became the relative path `.claude/worktrees/x/kernel/src/main.rs`, which matches no `kernel/` prefix. A sibling worktree outside the `cwd` repository got no decision either. This was reproduced with the built binary on 2026-10-05.
2. **`..` folded before symlinks.** The resolver folded `..` lexically and then followed symlinks. With `docs/linked -> kernel/src`, the lexical fold turned `docs/linked/../x.rs` into `docs/x.rs`, so the hook made no decision, while the OS writes `kernel/x.rs`.
3. **A gitfile planted at depth.** Only a root-level `.git` was denied. With a deny prefix of more than one component, such as `kernel/arch/`, `Write` could create `kernel/.git`. Git then reported `kernel/` as the root, `kernel/arch/boot.S` became `arch/boot.S`, and nothing matched.
4. **`GIT_CEILING_DIRECTORIES` faking "no repository".** An inherited ceiling that names the repository, or any directory between the target and the root, makes git print exactly the "not a git repository (or any of the parent directories)" text. The hook gives that answer no decision, so a real repository's `kernel/` edit went through.

All four are fixed:

- The root comes from the target.
- Paths resolve physically, so each symlink is spliced in before the next component and `..` applies to the real directory.
- A `.git` component at any depth is denied.
- The location and discovery variables are removed from git's environment.
- Git's root must be the nearest ancestor that holds a `.git` entry.
- A "no repository" answer is cross-checked against the filesystem.

## Why it happened

- **Each hole was in the spec, not just the code.** The first plan took the root from `cwd` and normalised `.` and `..` lexically before resolving symlinks. Its first revision denied only a `.git` as the first path component, and gave no decision whenever git said "not a git repository". Code that followed each version exactly carried its holes.
- **Verifiers judged the code against the plan.** Adversarial verifiers reproduced some of these bypasses and then rejected their own findings, as "matches the plan" or as "design creep" (asking for more than the spec). A reproduced bypass was filed as a non-finding because the spec allowed it.
- **One platform answer was second-hand and wrong.** A helper agent answered a Claude Code docs question for the hook contract, and the plan took its answer. It said `SubagentStop` input carries a `stop_reason`; the hooks reference has that name only as a `-p` process-exit field. It also said `SubagentStop` fires for background subagents, which the docs do not say. No code had used either claim yet. Only reviewers who read the live docs themselves caught it, in review round 5.

## What we learned

- **A guard is correct when it stops the threat, not when it matches its spec.** The spec is a guess at how to stop the threat, and it can carry the hole. Review a guard by attacking it, not by comparing it with the plan.
- **A reproduced bypass is a finding, whatever the spec says.** "Matches the plan" means the plan is wrong too.
- **Path guards need the OS's view of a path.** That means physical resolution, symlinks before `..`, and the repository root taken from the target. A path that is relative to something the caller supplies can be moved.
- **Inherited environment is attacker input for a guard.** Any variable that changes how a helper tool answers (`GIT_DIR`, `GIT_CEILING_DIRECTORIES`, and the rest) must be cleared or pinned, and the helper's answer cross-checked against the filesystem.
- **A helper agent's summary of platform docs is a lead, not a fact.** Read the docs page itself.

## How to avoid next time

- **The lead re-checks every rejected finding a verifier reproduced.** When a verifier shows a bypass and then rejects it as "matches the plan" or "design creep", the lead re-runs the reproduction and decides it, and changes the spec if the bypass is real.
- **For a guard, test the threat model.** Write the attacker's moves down and give each one a test: symlinks, `..` behind a link, nested and sibling worktrees, planted metadata, and inherited environment variables. A green suite that only tests the spec text says nothing about the threat.
- **Resolve paths physically.** Walk each component the way the OS does, and deny what cannot be resolved (a `..` after a missing component, too many links).
- **Take repository roots from the target, and check them.** Run discovery from the target's directory with the location and discovery variables removed. Then require the reported root to be the nearest ancestor that holds a `.git` entry.
- **Verify platform docs first-hand.** Record each contract fact with its URL and read date, and mark anything not on the page "not documented". A plan must not rely on a fact that only a helper agent supplied.
- **State what the guard does not stop.** For `path-guard`, that is Bash writes, an existing nested repository or submodule under a denied prefix, and a timed-out hook (which fails open). Nobody should mistake it for a sandbox.
