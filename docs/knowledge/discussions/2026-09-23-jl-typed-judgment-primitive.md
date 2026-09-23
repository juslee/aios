---
author: jl + claude
date: 2026-09-23
tags: [intelligence, airs, security, memory, typesafe]
status: draft
---

# Discussion: Typed judgments in AIRS

## Context

AIOS's intelligence services keep asking a model small, closed questions. How urgent is this notification? Is this agent action consistent with its declared task? Which setting does this sentence mean? The AIRS spec answers them by generating text and parsing it, or with hand-written heuristics.

A **typed judgment** answers them directly. The model reads a state (text or JSON) and one or more questions, and returns a probability distribution for each question from one shared prefill of the state. It generates no tokens. There are three question types, named as in TypeSafe's System One API and the open Decider models:

- **Noul:** the probability that a yes/no condition holds. 0.5 means yes and no are equally likely, not "medium".
- **Choice:** a distribution over a known, unordered set of options.
- **Score:** a probability-weighted position on ordered levels, each described in words.

Questions over one state run as parallel branches and cannot see each other's answers. Code keeps the policy: thresholds, weights, precedence and arithmetic.

**Why decide this before Phase 11.** The Phase 11 doc (AIRS Inference Engine, `development-plan.md:428`) will freeze four things: the session state machine, the metering model, the result type and the AIRS Kit trait. A typed judgment touches all four. The spec already has most of the parts:

- `forward` returns logits (`airs/inference.md:1586-1592`), and sampling computes `Softmax → probs` before it draws a token (`:720-721`). The probabilities exist and are thrown away.
- `OutputConstraint::Choice` "precomputes token prefixes for each option" (`airs/inference.md:907-911`), but it returns only the sampled string.
- No API exposes a probability. Every confidence in the AIRS spec is either written by the model inside its generated output or computed by hand. For example, the Intent Verifier parses the model's verdict and falls back to `Suspicious` at 0.5 when parsing fails (`intent-verifier/pipeline.md:300`).
- Consumers call AIRS methods that are defined nowhere: `batch_assess_urgency` (`intelligence/attention.md:1165`), `analyze_urgency` (`intelligence/attention.md:281`) and `interpret_preference` (`preferences/resolution.md:207`). `InferenceResult` is used (`kits/intelligence/airs.md:44`, `:212`) but never defined.

**Citations.** A cited path that starts with a top-level `docs/` directory (`intelligence/`, `kernel/`, `kits/`, `storage/`, `security/`, `experience/`, `applications/`) is relative to `docs/`, so `intelligence/airs.md` is the AIRS overview and `kits/intelligence/airs.md` the AIRS Kit. A shorter path, such as `airs/inference.md` or `development-plan.md`, names the only file under `docs/` that ends that way. A path marked "the repo's" is relative to the repository root, not `docs/`. A bare `:line` refers to the last file named before it.

**Evidence.** On 2026-09-23 a multi-agent survey searched `docs/` and the repo's `kernel/src` and `shared/src` for heuristics standing in for a semantic judgment: keyword lists, hand-tuned weights, magic thresholds, rule trees, self-reported confidences and prompt-then-parse steps. It found 634 hits, which merged to 389 distinct sites. Verifiers checked each quote at its cited line and rejected any site they were unsure of. 129 survived: 15 strong and 114 plausible. The full site list is not kept in the repo. The [Jev eval](../research/2026-09-22-jl-jev-review-comment-triage-eval.md) supplies the main lesson: a typed judgment only works when the evidence it needs is in the state it is given.

## Key Ideas

### 1. A judgment session in the AIRS engine (Phase 11)

The engine prefills the state once. For each question, it forks the session and runs the question through `forward`. It then reads the logits at the answer position, restricted to the option tokens. Nothing is sampled.

The mechanism works on any causal language model. A model trained for judgments improves calibration but is not a precondition. For example, decider-2b's base model, Qwen3.5-2B-Base, has an in-task expected calibration error (ECE) of 0.121 zero-shot, against 0.037 after Decider's training.

| Layer | Change | Today |
| --- | --- | --- |
| `InferenceRuntime` | No signature change when each option is one token. Options longer than one token need teacher-forced `forward` calls on a forked cache. New requirement: fork session state that is not a KV block (Open Question 2). | `forward` returns next-position logits (`airs/inference.md:1586-1592`). The KV cache is PagedAttention blocks with copy-on-write prefix sharing (`kernel/memory/ai.md:142`, `:185-196`). |
| Engine session | Add a readout mode to `SessionConfig`, in which `max_tokens` is 0. Add a `Ready → Completed` transition. Reuse the `Choice` prefix tables and read `probs` without sampling. | `SessionConfig` is at `airs/inference.md:58-69`. `Ready` leads only to `Generating` (`:1143-1152`). |
| Prefix sharing | Key shared prefixes on the state and the question set, not only the system prompt. Share only within one trust domain. | Keyed on "hashing the system prompt tokens" (`airs/inference.md:656`). |
| Metering | Check prefill tokens before a session starts, not only predicted completion. Prompt tokens are counted after the fact. The admission check charges only a predicted completion length, and prompt length is just one input to that prediction, so a judgment's prefill is never checked against the budget before it runs. | `TokenUsage` counts "Cumulative prompt tokens processed" (`airs/inference.md:1001-1003`), and `TokenBudget` covers "prompt + completion" (`:1013-1014`). But the pre-session check charges an estimated completion length (`:1243-1246`), predicted from prompt length and other features (`:1104`, `:1118`), and usage updates "On each token" (`:1093`). The kernel compute budget (`:1248-1249`) tracks device time and power, not tokens (`kernel/compute/budget.md:17-20`), and covers only work on a "non-CPU device" (`kernel/compute/budget.md:10`). `InferenceBudget.max_tokens` is "Maximum tokens to generate" (`kits/kernel/compute.md:142-143`). |
| AIRS Kit | Add `judge` and `judge_batch` beside `infer` and `embed`, gated on `InferenceAccess`. Define `InferenceResult`. Replace the free-text classifier example. | The trait is at `kits/intelligence/airs.md:42-57` and the capability table at `:338-342`. The example prompts "Classify activity" and reads back a string (`:306-313`). |
| Principles | Add an explicit exception to "Streaming always… No blocking calls". A judgment has nothing to stream. | `intelligence/airs.md:142` |
| Model registry | Add a judgment task type. Allow a second resident model only on devices with 8 GB or more (nominal RAM; the implemented sizing differs, see Key Idea 5). | `TaskType` is at `airs/model-registry.md:71-90`. Specialists fit alongside the primary model at 8 GB (`:213`) and at 16 GB and above (`:269-272`). |
| Gate 2 and benchmarks | Add criteria for judgment latency and calibration. | Gate 2 measures throughput, first-token latency and memory (`development-plan.md:253-257`). Its "If NO" branch already says "Focus on embedding/classification" (`:259`). |

**Ownership.** Phase 11 owns the runtime, the engine session and the Kit method; its row in the plan names "AIRS Kit (inference)" (`development-plan.md:428`). Phase 12, "AIRS Kit (services)" (`development-plan.md:429`), defines the service calls built on it. Each consumer phase owns its questions, thresholds and labelled calibration set.

**Placements considered and not chosen:**

- **Compute Kit Tier 3.** It arrives in Phase 23 (`development-plan.md:440`), and its interface takes tokens and returns generated output (`kits/kernel/compute.md:117-138`), with no readout.
- **A separate judgment service.** It would need a second copy of the weights and a second IPC hop.
- **`OutputConstraint::Choice` alone.** It samples a string and returns no distribution.

### 2. Results carry distributions, not a confidence

Each answer returns its full distribution:

- Noul: P(yes).
- Choice: the probability of each option.
- Score: the probability of each level, plus the weighted position.

The result also names the model id and version, the origin (local or remote) and what the model was calibrated for.

There is no `confidence` field. Decider's `confidence` is the top probability, while TypeSafe computes (n·peak − 1)/(n − 1), so a threshold tuned on one backend is wrong on the other. Code derives any summary it needs. It also pins each threshold to the model version the threshold was tuned on, as the Jev eval advises (`2026-09-22-jl-jev-review-comment-triage-eval.md:146`).

### 3. Code keeps the policy

- **Thresholds and bands** on probabilities, calibrated per consumer.
- **Deterministic precedence and caps.** The Intent Verifier's pre-check `ClearViolation` wins over the model (`intent-verifier/pipeline.md:311`). Screening is "not a trust boundary on its own" (`adversarial-defense/screening.md:20`). Regex and Luhn detectors and kernel capability checks stay in code.
- **Rule-based fallbacks** for when AIRS is down: "Every AIRS feature has a non-AI fallback" (`intelligence/airs.md:143`).
- **Arithmetic, counting, dates and aggregation.**
- **Question text as code constants.** Untrusted text goes only into a delimited state field.

Rules do not go in the question either. The Decider authors report that, on one probe, a paragraph of rules scores 0.24 against 0.67 for a one-sentence question. So the Intent Verifier's rule-paragraph prompt (`intent-verifier/pipeline.md:262-280`) cannot be ported as it stands.

### 4. Adoption order

Adopt first where the evidence is in the state and the call can be asynchronous. Adopt last at security gates, and there only as a signal under deterministic caps. The table lists the 16 highest-ranked sites, ranked by five criteria in order:

1. The evidence is in the state.
2. The call fits CPU latency.
3. A wrong answer does little harm.
4. It replaces a mechanism that is undefined or self-reported.
5. Its phase comes earlier.

| # | Consumer | Site | Today | Judgment | Phase |
| --- | --- | --- | --- | --- | --- |
| 1 | Behavioral Monitor Tier 2 | `behavioral-monitor/intelligence.md:206-225` | JSON-mode verdict with a self-reported confidence | Noul: is this flagged burst explained by what the user asked the agent to do? Asynchronous, 1–5 calls an hour (`behavioral-monitor/intelligence.md:237-239`). Not a gate, since nothing waits on it, but still a signal only: a `FalsePositive` verdict can suppress enforcement (`:203`, `:215-216`), so Tier 1 hard limits stay in code. | 46 (arch doc: stale 41); see Open Question 10 |
| 2 | Sensitivity labels | `security/privacy/data-lifecycle.md:62` | Keyword detection and pattern matching | One Noul per `ClassificationLabel` in place of the keywords; the PII pattern matchers stay in code | 13 |
| 3 | Alt-text quality | `experience/accessibility/testing.md:621-622` | Presence is checked; quality "requires manual review" | Noul: is this a real description, not a filename or placeholder? | 38 |
| 4 | Flow content screening | `storage/flow/security.md:65-66` | An unspecified `AirsClassifier(String)` | One Noul per label the source names (passwords, credentials, PII) | 16 |
| 5 | Attention content urgency | `intelligence/attention.md:203`, `:281`, `:341` | An urgency-marker list and a rule tree | Score (how soon), plus Nouls, in place of the marker list; the rule tree's precedence stays in code | 17, batch path only (`intelligence/attention.md` §14.2) |
| 6 | Preference NLU | `preferences/resolution.md:207` | `interpret_preference`, which is undefined | Choices: request type, setting, direction | 15 |
| 7 | Privacy query routing | `security/privacy/intelligence.md:270` | An unspecified classifier | Choice over query types | 18 |
| 8 | Inspector NL queries | `applications/inspector/intelligence.md:30-31` | A confidence score of unstated origin, gated at 0.8 (`applications/inspector/intelligence.md:54`) | Choices: intent, scope | none in §8 |
| 9 | Entity typing | `space-indexer/pipeline.md:183` | A confidence reported by the extraction model | Choice over entity kind | 13 |
| 10 | Answer relevance | `conversation-manager/streaming.md:337` | Embedding cosine | Noul: did the response answer the question? | 18 |
| 11 | Semantic search re-rank | `storage/spaces/query-engine.md:35` | A cosine threshold | Noul per result, asynchronous or on a small top-N only; not inside the < 500 ms query (`storage/spaces/query-engine.md:152`) | 13 |
| 12 | Intent Verifier LLM path | `intent-verifier/pipeline.md:262-280` | A rule-paragraph prompt and a parsed verdict | Noul: would this task need this action? Asynchronous only | 20 |
| 13 | Manifest description check | `intent-verifier/specification.md:275` | "LLM-checked" | One Noul per declared purpose | 20 |
| 14 | Browser phishing content | `applications/browser/intelligence.md:99` | "AIRS classifies urgency language, credential requests, suspicious forms" | Nouls feeding `content_score`; an asynchronous warning only | 35 |
| 15 | Adversarial screening Tier 2 | `security/adversarial-defense/intelligence.md:315-336` | A prompt returning `injection_probability` and `confidence` | Noul, on non-destructive reads only, which Tier 2 may defer (`adversarial-defense/screening.md:213-214`) | none in §8 |
| 16 | Tool ranking | `tool-manager/intelligence.md:57` | `semantic_match` from embedding cosine | Choice over the candidate tools, feeding `semantic_match` | none in §8 |

**Across all 129 sites:**

- **Role:** 114 feed a judgment into code policy as a signal, and 12 replace the heuristic outright. The other 3 cite rules that run only when AIRS is down; the judgment belongs in the AIRS branch beside them, and the rules stay.
- **Question type:** 57 Noul, 32 bundles of several questions, 25 Choice and 15 Score.
- **Latency:** 22 fit their stated budget, 49 are tight, and 58 state no budget.
- **Where:** the docs with the most sites are the Behavioral Monitor (9), Intent Verifier (8), Task Manager (8) and Space Indexer (7). No site in the repo's `kernel/src` or `shared/src` survived.

**Why 260 sites were rejected:**

- numeric control;
- AIRS-down fallback paths with no AIRS branch beside them;
- synchronous budgets under 10 ms with no deferred path;
- exact protocol lookups;
- evidence that lies outside the state;
- adversary-written state where the judgment would be the only gate;
- passing mentions.

The Context Engine was rejected outright. It is a feature-vector classifier, "not a full LLM, not generative" (`context-engine/inference.md:14`), and "It never sees content" (`context-engine.md:184`).

### 5. Degradation by device tier

The tiers below follow the architecture docs. The implemented pool sizing gives some boards a smaller model pool than the architecture docs do. The kernel passes `PoolConfig::from_total_ram` only the UEFI map's usable memory, leaving out firmware and reserved regions (the repo's `kernel/src/mm/init.rs:60-62`, `:109`). So a board of exactly 4, 8 or 16 GB falls just under a tier boundary: a 4 GB board gets no model pool (the repo's `shared/src/memory.rs:79-81`), and an 8 GB board gets a 2 GiB pool (`:82-83`). Separately, every board with under 4 GiB usable gets no model pool (`:80-81`), so a 2–3.9 GB board gets none where the model registry gives it 1 GB. A board with no model pool has no judgments unless the cloud question is settled (Open Question 8). Issue #184 tracks both conflicts (D22, D23).

- **AIRS down:** the rule-based fallbacks stay (`intelligence/airs.md:143`).
- **2 GB and under:** there is no local model (`kernel/memory/ai.md:33-35`, `:422`; `airs/model-registry.md:246-249`), so there are no judgments unless the cloud question is settled (Open Question 8). The two docs disagree about 2 GB devices: `airs/model-registry.md:251-252` gives 2–3.9 GB devices a 1 GB pool.
- **2–3.9 GB, per the model registry:** a 1 GB pool for a 1B Q4 model (`airs/model-registry.md:251-252`). No second model fits.
- **4 GB:** the model pool is 2 GB (`kernel/memory/ai.md:24`). "Only one small model (1-3B at Q4) fits at a time" (`kernel/memory/ai.md:424`). There is no second resident model, so judgments use the primary model's readout at its zero-shot calibration.
- **8 GB:** a 1–2B specialist of "~500 MB-1 GB" may stay loaded (`airs/model-registry.md:213`). decider-0.8b does not suit that slot as released:
  - It is 1.4–1.5 GB in bf16.
  - It is a v1 recipe, with held-out accuracy 0.707 and ECE 0.096.

  AIOS would need a quantized build and would have to measure it first.

## Open Questions

1. **Which runtime runs the judgment model?** candle main (commit `e68b659`, 2026-09-22) has no Qwen3.5 module. PRs #3396, #3461 and #3838 are still open. The options:
   - an out-of-tree port with a NEON Gated DeltaNet path;
   - waiting for upstream;
   - the GGML backend (`airs/inference.md` §3.9.2), since llama.cpp already runs the architecture;
   - a judgment model on an architecture candle supports (none is published yet).

   Either way, the risk register's "candle bridge integration is well-understood" (`development-plan.md:198`) needs updating.
2. **Forking state that is not KV.** In decider-2b (Qwen3.5-2B), 18 of 24 layers carry Gated DeltaNet recurrent state, which PagedAttention's copy-on-write blocks cannot share. `InferenceRuntime` needs a snapshot-and-fork contract that covers both kinds of state; the Kit's `InferenceSession::fork` (`kits/intelligence/airs.md:81`) would sit on top of it.
3. **Options longer than one token.** Either score them with teacher-forced passes, or require option labels that are a single token each.
4. **Question layout.** Decider scores each question in its own row by default. Packing the questions into one row changes up to 12% of answers when their order is reversed (decider-2b model card). Default to independent rows. Prompt order also matters. The design in Key Idea 1 prefills the state first and forks per question, as Decider's own server does for long states ("long state: run it once, fork the cache per question", `decider/serve.py`). The ARM figures in Open Question 5 come from a third-party build that caches the question prefix instead, so the state-first layout is unmeasured on ARM. Whatever order AIOS picks must match the order the model was trained on.
5. **Latency on target hardware.** The only ARM CPU figures are third-party, measured on a Neoverse-N1 with 4 threads, Q8_0 and decider-2b v8 weights:
   - about 0.55 s for a state under 64 tokens, with the question prefix cached;
   - about 4 s for a 200–400-token state;
   - 15–20 s for a cold question prefix;
   - about 3.2 GB resident.

   None of these meets the latency budgets the docs set for ranked consumers: < 50 ms for one attention item (`intelligence/attention.md:1139`) and < 500 ms for a batch of 50 (`:1747`), < 500 ms for a semantic query (`storage/spaces/query-engine.md:152`), < 10 ms for screening Tier 2, an asynchronous stage that can defer (`adversarial-defense/screening.md:279`), and the Intent Verifier's < 10 ms LLM target, which assumes an NPU and expects 50–100 ms on CPU-only hardware, with more actions routed to asynchronous verification (`intent-verifier/pipeline.md:456`). Row 5 in particular needs its batch budget revised. Measure on the Gate 2 target hardware, a Pi 4 (4GB) (`development-plan.md:255`), and on a Pi 5, before judgment criteria are added to Gate 2.
6. **Calibration.** No AIOS labels exist yet.
   - decider-2b v10 reports ECE 0.037 in-task and 0.084 held-out.
   - On the hard tier of JevBench, as reported by the Decider authors, ECE is 0.30 for decider-2b, 0.29 for decider-4b and 0.15 for decider-35b-a3b.
   - In our own eval, Jev scored a Brier of 0.060 against 0.061 for always guessing the base rate (`2026-09-22-jl-jev-review-comment-triage-eval.md:71`).

   Each consumer phase needs a labelled set before a threshold ships.
7. **Injection.** Attention items (`intelligence/attention.md:1434`, `:1436`), agent actions, manifests and web pages all put attacker-written text into the state. A typed answer cannot run instructions, but the text can still steer it. Where the judgment would be the only gate, it stays out.
8. **Cloud.** Still undecided, and the docs conflict:
   - `intelligence/airs.md:141` says there is no cloud dependency.
   - `development-plan.md:261` plans a hybrid of local and cloud.
   - `airs/model-registry.md:246-249` has a cloud-only mode below 2 GB.

   The request shape (a state plus typed questions, returning distributions) matches TypeSafe's hosted `/v1/systemone`. So it commits to nothing about where inference runs. Add no remote backend until this is decided.
9. **Provenance.** Decider's author publishes safetensors only. The GGUF conversions are third-party, and none is validated for reading the answers, which needs a logits shim. AIOS would convert, measure and sign its own build into a `ModelManifest` (`secure-boot/intelligence.md:31`).
10. **Behavioral Monitor Tier 2 phase.** The top-ranked consumer lands late. `behavioral-monitor.md:217-221` puts the core monitor with "AIRS Intelligence Services" (stale 10, now Phase 12) and Tier 2's semantic classifier (`behavioral-monitor/intelligence.md` §13.1) with "AIRS Capability Intelligence" (stale 41, now Phase 46). The plan's §8.2 Intent Kit row lists Phase 12 under "Also Touches" for the behavioral monitor (`development-plan.md:524`). Should the §13.1 classifier move up to Phase 12, alongside the core monitor, once Phase 11 has delivered the judgment session?

## References

Architecture docs:

- [AIRS inference engine](../../intelligence/airs/inference.md): §3.1 inference runtime, §3.3.4 prefix sharing, §3.4.5 structured output, §3.5 metering, §3.6 session lifecycle, §3.9 runtime bridges
- [AIRS Kit](../../kits/intelligence/airs.md), [Compute Kit](../../kits/kernel/compute.md)
- [AIRS overview](../../intelligence/airs.md) (design principles), [model registry](../../intelligence/airs/model-registry.md)
- [AI memory management](../../kernel/memory/ai.md): PagedAttention and device tiers
- [Development plan](../../project/development-plan.md): §8 phases, Gate 2, risk register
- The consumer docs cited in the adoption table

Knowledge docs:

- [Research: Can Jev triage PR review comments?](../research/2026-09-22-jl-jev-review-comment-triage-eval.md)
- [ADR: candle Replaces GGML for AIRS Inference](../decisions/2026-03-16-jl-candle-inference-runtime.md)

External sources, all read on 2026-09-23:

- TypeSafe docs: [primitives](https://docs.typesafe.ai/primitives.md), [HTTP API](https://docs.typesafe.ai/api.md), [confidence](https://docs.typesafe.ai/confidence.md)
- Decider: [github.com/Mapika/decider](https://github.com/Mapika/decider) at `7557fe0` (README, `decider/serve.py`, `decider/systemone.py`).
  - Model cards on Hugging Face: `Mapika/decider-0.8b`, `Mapika/decider-2b` (rev `fa996cea`), `Mapika/decider-4b` (rev `5e10009f`).
  - Third-party GGUF builds and the ARM CPU figures: `cosetoenor/decider-2b-GGUF` (rev `dcc6e553`).
- candle: [huggingface/candle](https://github.com/huggingface/candle), `candle-transformers/src/models` at `e68b659`; issue #3393; PRs #3396, #3461 and #3838.

## Outcome

_Fill in when graduated or archived:_

- Graduated to: `docs/intelligence/airs/inference.md` and `docs/kits/intelligence/airs.md` (expected, when the Phase 11 doc is written)
- Decisions extracted to: `decisions/YYYY-MM-DD-jl-...`
- Or: archived (no longer relevant because ...)
