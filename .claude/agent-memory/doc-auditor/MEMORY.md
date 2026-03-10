# Doc Auditor Memory

## Audit Patterns
- Always verify section references (e.g. "see security.md Section 3.7") actually exist in target
- Struct/enum names must be identical across all docs that reference them
- CLAUDE.md Architecture Document Map must list all docs with correct paths and section numbers
- New docs must be added to architecture.md Related documents list
- When a doc shows a struct, verify field names match the canonical definition in the owning doc

## Common Issues Found
- security.md §1.1 = "What We Defend Against", §1.2 = "Trust Boundaries" (people confuse these)
- security.md §6.2 = "Incident Types and Responses", §6.3 = "Escalation Policy" (4 levels are in §6.3)
- agents.md: `CapabilityRequest` is a struct with {capability, justification, required}, NOT an enum
- agents.md: field is `bundle_id` not `id`, type is `RuntimeType` not `Runtime`
- agents.md/security.md: `ProfileReference.version_req` not `version`
- security.md §3.7: profile IDs use format "os.base.v1", "runtime.native.v1" etc.
- security.md §3.7.7: user overrides stored at `user/preferences/capability-overrides/`
- airs.md §5.5 = "Behavioral Monitor", §5.6 = "Adversarial Defense" (easy to mix up)

## Key Section Map (security.md)
- §1.1 What We Defend Against | §1.2 Trust Boundaries | §1.3 What We Don't Defend Against
- §3.1 Capability Token Lifecycle | §3.2 Kernel Capability Table | §3.3 Attenuation
- §3.7 Composable Capability Profiles | §3.7.7 Storage
- §6.2 Incident Types | §6.3 Escalation Policy (4 levels)
- §7.1 Inspector | §7.2 Conversation Bar

## Key Section Map (airs.md)
- §5.5 Behavioral Monitor | §5.6 Adversarial Defense | §5.9 Agent Capability Intelligence
- §10.1 Security Path Isolation | §10.3 Kernel Oversight

## Key Section Map (agents.md)
- §2.4 The AgentManifest | §3.1 Installation

## Key Section Map (architecture.md)
- §2.6 Attention Management | §2.8 Preference System
- §6.3 Agent Sandbox and Execution Model | §6.5 Multi-Identity and Shared Devices

## Key Section Map (attention.md)
- §3 The Attention Item | §15.2 Pre-AIRS Triage (Rule-Based Mode)

## Key Section Map (context-engine.md)
- §8 Fallback (Without AIRS) | §8.1 Rule-Based Fallback

## Key Section Map (observability.md)

- §1.1 Relationship to Security Observability | §1.2 Scope Exclusions
- §2.1 Problem | §2.2 Log Levels | §2.3 Subsystem Tags | §2.4 Log Entry Format
- §2.5 Per-Core Ring Buffer | §2.6 Logging Macros | §2.7 UART Drain | §2.8 Early Boot Fallback
- §3.2 Counter | §3.3 Gauge | §3.4 Histogram | §3.5 Kernel Metrics Registry | §3.6 Feature Gating
- §4.2 Trace Events | §4.3 Trace Record | §4.4 Per-Core Trace Ring | §4.5 Trace Point Macro | §4.6 Trace Export
- §5.2 Health Level | §5.3 Health Registry | §5.4 Per-Subsystem Thresholds | §5.5 Relationship to MemoryPressure
- §6.2 UART Drain (Phase 3) | §6.3 Kernel Info Page | §6.4 AuditRead Extension (Phase 13)
- §7.2 Storage Metrics | §7.3 Integration Points | §7.4 Health Signal Integration

## Key Section Map (spaces.md)

- §4 Block Engine | §4.2 Write Path | §4.3 Read Path | §4.4 Crash Recovery | §4.8 Write Amplification Tracking

## Naming Conventions (verified)
- `BehavioralBaseline` (not `BehaviorBaseline`) — canonical in security.md and agents.md
- `BehavioralMonitor`, `BehavioralPolicy` — security.md canonical
- `BehavioralRule`, `BehavioralCondition`, `BehavioralAction` — airs.md (fixed from `Behavior*`)
