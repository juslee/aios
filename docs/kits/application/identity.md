# Identity Kit

**Layer:** Application | **Architecture:** `docs/experience/identity.md`

## Purpose

Identity Kit provides user identity, authentication, device pairing, and multi-device trust. It is the foundation on which personalization, capability delegation, and secure cross-device continuity are built. Identity Kit ensures that the right person on the right device has access to the right capabilities.

## Key APIs

| Trait / API | Description |
|---|---|
| `Identity` | Represents a user identity with associated credentials and trust metadata |
| `DevicePairing` | SPAKE2+ based pairing flow for establishing trust between personal devices |
| `TrustChain` | Attestation chain linking device hardware identity to user identity |
| `BiometricAuth` | Local biometric verification (Face ID, fingerprint) tied to capability unlock |

## Orchestrates

- **Capability Kit** — identity verification gates capability grants and delegations
- **Storage Kit** — identity credentials and pairing records are stored in encrypted Spaces
- **Network Kit** — remote identity verification and cross-device sync use authenticated channels

## Implementation Phase

Phase 3+ (basic identity). Full identity Phase 15+
