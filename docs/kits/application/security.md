# Security Kit

**Layer:** Application | **Architecture:** `docs/security/model.md` + sub-docs

## Purpose

Security Kit is the user-facing layer over AIOS's capability and audit systems. It surfaces the Inspector UI, permission prompts, and the security dashboard — translating kernel-level capability tables and audit rings into forms a user can understand, review, and act on.

## Key APIs

| Trait / API | Description |
|---|---|
| `SecurityInspector` | Live view of all active capability grants across agents and applications |
| `PermissionPrompt` | Synchronous capability request UI presented to the user for explicit approval |
| `AuditViewer` | Browsable presentation of the kernel audit ring with filtering and search |
| `TrustDashboard` | Aggregate trust posture view — anomalies, revocations, and active sessions |

## Orchestrates

- **Capability Kit** — reads capability tables and performs revocations on user instruction
- **Intent Kit** — surfaces intent verification results and flags for user review
- **Identity Kit** — associates capability grants with verified user and device identities
- **Interface Kit** — renders all Security Kit UI surfaces within the compositor

## Implementation Phase

Phase 17+
