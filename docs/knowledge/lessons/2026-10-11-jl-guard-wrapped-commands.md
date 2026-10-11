---
author: jl + claude
date: 2026-10-11
tags: [tooling, security]
status: final
---

# Lesson: A guard must analyse what a trusted wrapper runs, as the argument vector it runs

## What happened

Guard rule 1 (`.claude/hooks/git-push-guard.py`) lets the verifier and an owner or solo main thread start QEMU only through the host lock wrapper, `scripts/agent/qemu-lock.sh run ... -- <command>`. Until PR #234's T17 security review (2026-10-10), the guard trusted the genuine wrapper without reading its body, on the reasoning that running the command after `--` is the wrapper's job. It never analysed that command. The settings allowed `Bash(bash scripts/agent/qemu-lock.sh *)`. So `bash scripts/agent/qemu-lock.sh run --team solo --mode boot --label x --eta-min 5 -- git push origin HEAD:main` passed both the permission layer and the guard, for the main thread and for the verifier. So did `gh pr merge`, pattern kills, toolchain changes and rule 4 placement changes wrapped the same way.

The fix took three rounds. Each was written test first: the guard's tests went from 126 to 133, 137 and 140, and the tests that pin each hole failed on the head before its fix. Each fix but the last was broken by the attack round that followed it:

1. **First fix (19a120c).** For every `run`, the guard analyses the words after the `--` that ends the wrapper's options, read the way the wrapper reads them, as a nested command at the call's cwd. A QEMU start in that command gets no rule 1 decision of its own. The settings allow narrows to `status` and `clear-stale`, an owner edit of `.claude/settings.json` (rule 11, Config).
2. **The attack round on the first fix broke it three ways.**
   - The fix re-parsed `shell_join` of the wrapped words, and `shlex.quote` turned every word from an unresolved expansion into a literal. The checks keyed on a word's expansion flag therefore never fired: `-- "$SHELL" -c 'git push origin HEAD:main'`, `-- $GIT push ...` and `-- sh -c "$CMD"` were allowed when wrapped and asked unwrapped.
   - The nested analysis never saw the words xargs appends, so `printf 'HEAD:main\n' | xargs qemu-lock.sh run ... -- git push origin` was allowed.
   - A parse error in the re-parsed text (`-- sh -c 'echo "x'; git push origin HEAD:main`) aborted the whole analysis, so the later push deny became an ask.

   The second fix (d3c40e0) dropped the re-parse. The wrapper runs its command as `"$@"`, so the guard hands the words to the command analyser as an argument vector. The words keep the expansion flags they had on the call, and the analyser also gets the call's text and a copy of its state. An xargs, find or parallel that runs the call is put in front of the wrapped command. A parse error inside the wrapped command asks, and the outer analysis goes on.
3. **The attack round on the second fix broke it three more ways.**
   - The guard read "no literal `--`" as "the wrapper refuses". But an option word from an expansion (`run $LOCKOPTS git push origin HEAD:main`) can split at run time into words that include `--`, so the wrapper ran words nobody analysed (none; deny unwrapped).
   - A subcommand from an expansion (`qemu-lock.sh $SUB ...`) decided nothing.
   - The guard checked the lock at the payload cwd, while the wrapper takes the lock of its own repository.

   The third fix (f294d1d) changed three things. The parser's per-word flag now tells a bare expansion from one only inside double quotes. The guard asks for an option value or option stop that may split or be `--`, and analyses a subcommand from an expansion as a `run` and asks. It checks the genuine wrapper's `run` against its own repository's lock.
4. **The attack round on the third fix (2026-10-11) approved it.** The bypass review and the regression review both completed. The regression review compared the guard's decisions before and after over 18948 command evaluations: 1120 differed, all intended. No deny or ask reason was lost, and nothing outside the wrapper changed. What remained is out of model and went to #241 for R5.

## Why it happened

- **The trust was in the design.** "Running the command after `--` is its job" is true of the lock and false of the command, because the caller chooses the command. The spec carried the hole, as in [the path-guard lesson](2026-10-06-jl-path-guard-spec-correct-bypasses.md), and an allow rule ending in `*` let the same payload through the permission layer.
- **The first fix threw away what the parser knew.** Turning parsed words back into shell text and parsing it again lost the per-word state the rules depend on: whether a word came from an expansion, and which tool appends arguments. It also widened a local parse failure to the whole command.
- **The guard read structure from the literal words.** The shell decides where the options stop and which subcommand runs only after word splitting.

## What we learned

- A wrapper that runs a command its caller supplies (`-- <cmd>`, `"$@"`) is a way around every rule, unless the guard analyses that command under the same rules. Trusting the wrapper's path says nothing about its argument.
- Hand a nested command to the analyser as the argument vector the wrapper will run. Keep each word's expansion state and the outer call's context: cwd, variables, text, and any argument-appending tool in front. Never turn the words back into shell text and re-parse them.
- Keep a parse failure local. An unparseable nested string asks, and the rest of the command is still analysed, so a later deny stays a deny.
- Quoting decides what an expansion can become. A bare unresolved expansion can split into any words, `--` included. One inside double quotes stays one word, except for `"$@"` and array forms, which the guard does not model (#241). When an expansion fills a word that decides structure (where options stop, which subcommand runs) and could change that structure, the guard must ask.
- Check the state the wrapped tool uses (the lock of the wrapper's own repository), not the state at the caller's cwd.
- Every guard fix gets its own attack round and a regression comparison of decisions before and after over a large command set. Account for every difference, and require zero lost reasons and zero differences outside the changed rule.
- A stated threat model ends the loop. The leftovers of the last round need deliberate same-command constructions, or tools the stateless text guard cannot see, so they were listed in #241 and not patched. Compare T7b, where each parser patch added new false allows and denies.

## How to avoid next time

- When a rule trusts a tool by path or name, write down what that tool runs for its caller, and analyse that under the same rules.
- Review every `Bash(<tool> *)` allow for a subcommand or option that runs something, and narrow it to subcommands that run nothing (here `status` and `clear-stale`).
- For R5 (the Rust guard, #241): keep the per-word expansion state (bare or quoted), pass nested commands as argument vectors, keep parse errors local, and port the T17 tests in `.claude/hooks/tests/test_git_push_guard.py` into the table.
