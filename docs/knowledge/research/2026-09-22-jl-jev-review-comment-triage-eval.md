---
author: jl + claude
date: 2026-09-22
tags: [tooling, research, typesafe, code-review]
status: final
---

# Research: Can Jev triage PR review comments?

## Question

Could TypeSafe's Jev model (a classifier that returns calibrated probabilities, not text) replace the per-comment Claude pass in [`/review-pr-comments`](../../../.claude/skills/review-pr-comments/SKILL.md)? That pass decides, for each Copilot comment, whether to fix the code or push back. A good pre-filter could also serve the review-merge loop's verify stage.

**Answer: no.** From the comment and the code around it, Jev cannot tell which comments need a fix. A same-input Claude Sonnet baseline could not either. Jev is parked for comment triage. The only place left to test it is the loop's verify stage (see [Implications](#implications-for-aios)).

## Method

**Data.** Every Copilot review thread in juslee/aios up to 2026-09-22, fetched with the GitHub GraphQL `reviewThreads` API: 859 threads, 784 of them with an author reply. 79% are on markdown docs (675), 20% on Rust or assembly (170), and 14 on tooling. The 75 unreplied threads were left out.

**Ground truth.** Claude agents read each author reply and labelled the outcome: `fixed`, `reviewer_wrong`, `by_design`, `deferred`, `already_fixed`, `answered` or `other`. They also recorded whether the PR changed in response (`changed_in_pr`) and whether the comment was right (`comment_valid`). An independent second labeller relabelled 139 items: 55 of the 56 replies that don't open with a fix-style word ("Fixed", "Addressed", "Updated" and similar; one id was mistyped), plus a random 84 that do. They agreed on 98.6% of outcomes (Cohen's κ 0.97), 100% of `changed_in_pr` and 95.7% of `comment_valid`. An adjudicator settled the 8 disagreements: 2 on outcome and 6 on `comment_valid`.

**What Jev saw.** The same inputs a triager has before acting: PR title, file path, the diff hunk the comment was made on (the last 80 lines; 426 of 859 hunks hit that cap), and the comment. The author's reply was never sent. The model was pinned to `jev-1.13.0`. Six questions went in one request:

| Question | Type | Asks |
| --- | --- | --- |
| `needs_change` | Noul | Should the author edit the change in response to the comment? |
| `comment_correct` | Noul | Is the problem the comment points out real? |
| `claim_support` | Choice | Does the hunk support, contradict, or say too little about the comment's claim? (the shape of TypeSafe's citation-check cookbook) |
| `kind` | Choice | Code suggestion, nit, question, or FYI (step 4 of `/review-pr-comments`) |
| `category` | Choice | The review-merge loop's 9 finding categories |
| `severity` | Score | Assuming the comment is right, how much harm would ignoring it cause? (4 levels) |

**Baseline.** Claude Sonnet answered the same six questions from the same inputs, 40 items per prompt, with no repository access.

**Extra arms.** These ran on a 151-item subset: all 51 comments that led to no change, plus 100 random ones that did.

- **File arm:** adds the file as it was at the commented commit, capped at 90,000 characters (20 of 151 files hit the cap).
- **Policy arm:** adds the file (capped at 60,000 characters to leave room; 29 files hit the cap) and the PR description, which carries phase scope and deferrals. It also splits `needs_change` into narrow questions: `claim_accurate`, `already_handled`, `intentional`, `out_of_scope`.

The subset holds 15 of the 16 reviewer-wrong comments. The sixteenth led to a change, so it was not in the no-change sample.

**Checks.**

- A blind check with repository access: agents judged 15 of Jev's lowest-scored "fixed" comments and 15 random "fixed" controls without knowing which group each came from.
- A critic agent re-derived the metric code and tested the conclusion.
- Confidence intervals come from a bootstrap that resamples whole PRs, because comments within one PR are correlated.

## Findings

### Outcomes

Of the 783 labelled comments that have answers from both models, 732 (93.5%) led to a change:

| Outcome | Count |
| --- | --- |
| fixed | 726 |
| by_design | 16 |
| reviewer_wrong | 13 |
| deferred | 13 |
| already_fixed | 8 |
| other | 7 |

16 comments count as "reviewer wrong" (outcome `reviewer_wrong`, or `comment_valid` of no). The 51 no-change comments cluster in 19 PRs; PR 39 alone has 10.

### Jev cannot decide fix vs push back

| Measure | Jev | Sonnet |
| --- | --- | --- |
| `needs_change` AUC, all | 0.72 (95% CI 0.65–0.80) | 0.58 |
| `needs_change` AUC, compared within file kind | 0.66 | 0.66 |
| Brier score (always guessing the base rate scores 0.061) | 0.060 | 0.127 |
| Items with `needs_change` at or above 0.8 | 752 of 783 | 211 of 783 |
| Reviewer-wrong AUC, `1 − comment_correct` | 0.67 | 0.60 |
| Reviewer-wrong AUC, `claim_support = contradicts` | 0.50 | 0.50 |

- **Jev ranks slightly better than chance, but its probabilities carry no information.** Nearly every comment scores above 0.8, and its Brier score matches always guessing the 93.5% base rate. When the evidence doesn't separate the classes, that is what a calibrated model does.
- **Jev's lead over Sonnet comes from the mix of file kinds.** Doc comments were changed 96% of the time and Rust comments 81%, so knowing the file kind alone gives AUC 0.69. Compared within file kind, Jev and Sonnet tie.
- **Jev almost never says a comment is wrong.** It chose `contradicts` for 6 of 858 comments. The replies that pushed back cite evidence from other files ("Verified `kernel/src/arch/aarch64/linker.ld` line 73", "Verified `kernel/src/smp.rs` lines 208–214") or from phase plans ("deferred to Phase 4"). None of that is in the hunk.

### More context does not help

AUC on the 151-item subset (51 no-change, 15 reviewer-wrong). The `needs_change` rows show 95% PR-bootstrap intervals:

| Arm | Predicts no change | Predicts reviewer wrong |
| --- | --- | --- |
| Hunk only, `needs_change` | 0.71 (0.63–0.80) | 0.65 (0.51–0.85) |
| + file, `needs_change` | 0.70 (0.61–0.78) | 0.63 (0.48–0.75) |
| + file and PR description, `needs_change` | 0.65 (0.57–0.74) | 0.59 (0.46–0.74) |
| Narrow questions: `claim_accurate` / `already_handled` / `intentional` / `out_of_scope` | 0.58 / 0.46 / 0.47 / 0.54 | 0.58 / 0.50 / 0.53 / 0.43 |
| Logistic combination of the arm's questions, 5-fold CV grouped by PR: hunk / file / policy | 0.70 / 0.66 / 0.56 | 0.53 / 0.40 / 0.43 |

The narrow questions carry no signal, and combining questions does no better than `needs_change` alone. The likely reason is that a PR description seldom addresses the specific concern a reviewer later raises, and the counter-evidence sits in other files.

### Low Jev scores do not flag unnecessary fixes

The labels record what the authoring agent did, so a wrong comment the agent "fixed" anyway would look like a Jev error. The blind check tested for this:

| Group | Change warranted: yes / marginal / no | Comment correct: yes / partly / no |
| --- | --- | --- |
| Jev's 15 lowest "fixed" (0.12–0.76) | 7 / 7 / 1 | 10 / 3 / 2 |
| 15 random "fixed" (mean 0.89) | 8 / 6 / 1 | 14 / 0 / 1 |

Jev's lowest-scored comments are slightly less often fully correct. The difference is not significant (Fisher p ≈ 0.17), and the fixes were warranted about equally often in both groups.

### The signal that exists

- **Routing.** Sending Jev's lowest-scored 30% of comments for a closer look catches 32 of 51 no-change and 11 of 16 wrong comments. Random routing would catch 15 and 5. That is real but modest, and in `/review-pr-comments` there is nothing to save by skipping the rest. Deciding whether to fix takes the same code reading as making the fix.
- **Severity predicts deferral.** Mean `severity` was 2.68 for deferred comments and 2.49 for by-design ones, against 1.92 for fixed ones. Serious concerns tend to get postponed rather than fixed.
- **Category is consistent across models.** Jev and Sonnet agree on the 9-way category 84.5% of the time (κ 0.69). They also agree on `kind` 85% of the time, but κ is only 0.29, because Jev put 824 of 858 comments in `code_suggestion`. Neither has a reference label.

### Side findings on the review process

- **Copilot is rarely wrong here, but many fixes are cosmetic.** Of the 15 random "fixed" comments, 8 were worth fixing, 6 were marginal (a link label, a duplicated helper, a test name, an en dash, a missing semicolon in checklist prose, a wording nuance) and 1 was wrong. The wrong one said `Duration::from_hours` does not exist. The blind check found it has been stable since Rust 1.91, and the code was changed anyway.
- **Truncated hunks hurt both models.** Jev's AUC is 0.77 on hunks under the 80-line cap and 0.67 on truncated ones; Sonnet drops too, from 0.61 to 0.57. This does not change the conclusion.

### Operations

| Run | Result |
| --- | --- |
| 859 hunk-only requests | 1.79M input tokens (2,087 mean), $0.075 in total; latency median 0.94 s, p95 3.84 s; no retries or failures |
| 151 file-arm requests | 11,190 tokens mean (up to 29,141), $0.071; median 1.37 s |
| Pricing (TypeSafe docs, 2026-09-22) | $0.042 per million input tokens; output tokens free; limit 1,200 requests per minute |

Cost and latency are not the obstacle.

### Limitations

- **The ground truth is what the author did.** Replies were posted under the owner's account, and most read as agent-written by `/review-pr-comments` (`Fixed in <sha>. …`). They are not an independent judgment of correctness. The blind check covers only 30 items.
- **Few minority cases.** There are only 16 wrong and 51 no-change comments, clustered in a few PRs. Results for the wrong-comment task have wide intervals.
- **The Sonnet baseline was batched and had no repository access.** It is not the real per-comment pass, which reads the code; that pass produced the labels.
- **Narrow scope.** One repository, one reviewer (Copilot), and 79% documentation comments.
- **The harness was not kept.** It lived in a session scratchpad. A rerun needs the data fetch, the labelling prompts and the question set described in [Method](#method).

## References

- TypeSafe docs: [HTTP API](https://docs.typesafe.ai/api.md), [Models, pricing and limits](https://docs.typesafe.ai/models.md), [Noul](https://docs.typesafe.ai/primitives/noul.md), [Citation-check cookbook](https://docs.typesafe.ai/cookbooks/citation_check.md), [Jev with coding agents](https://docs.typesafe.ai/introduction/coding-agents.md)
- [`/review-pr-comments` skill](../../../.claude/skills/review-pr-comments/SKILL.md): step 4 categorizes comments, and steps 5–6 fix or reply. The review-merge loop spec plans to replace this skill.
- The review-merge loop spec, `docs/knowledge/discussions/2026-09-22-jl-justin-review-merge-loop.md` on branch `claude/justin-review-merge-loop`: verify stage, finding categories, seeded-bug evals
- [PR #172](https://github.com/juslee/aios/pull/172): re-enabled the TypeSafe plugin in project settings

## Implications for AIOS

1. **Do not add Jev to `/review-pr-comments`.** The decision needs evidence from other files and phase plans, and the pass that makes the decision also makes the fix, so a filter saves nothing.
2. **The review-merge loop's verify stage is the only place left to test.** Each finding there carries its own claim and evidence excerpt, which is the citation-check shape Jev is built for. Verify is one `claude -p` run per round that tries to refute every finding. A Jev pre-check could shrink the set that run has to refute, or let a round skip verify when Jev confirms every finding with high confidence. Test it against the loop's seeded-bug and clean eval cases once they exist, and pin the model version the thresholds are tuned on.
3. **Loop design: consider a severity floor for convergence.** The spec converges on zero confirmed findings of any severity. Given how many review comments are cosmetic, that rule may keep the loop cycling on nits.
4. **Loop design: consider a deferral path.** In the spec, a confirmed in-scope finding is either fixed, or declined and then re-judged and tie-broken. Its issue filing covers only pre-existing problems on lines the PR does not touch. Here, though, serious in-scope concerns were the ones most often deferred to a later phase. A "defer to a tracked issue" outcome would let the loop do the same without going to needs-human.
