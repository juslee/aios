# AIOS Relationships & Trust Model

Part of: [identity.md](../identity.md) — Identity & Relationships
**Related:** [core.md](./core.md) — Identity data model & key management, [sharing.md](./sharing.md) — Space sharing configuration, [cross-device.md](./cross-device.md) — Peer protocol & device sync, [privacy.md](./privacy.md) — Selective disclosure & ZKP

**Cross-references:** [decentralisation.md](../../security/decentralisation.md) — Pillar 1 (did:peer, SD-JWT), [behavioral-monitor](../../intelligence/behavioral-monitor.md) — Trust anomaly feeds

-----

## 5. Relationships

### 5.1 Data Model

The relationship model uses `did:peer` (numalgo 2) as the identity representation, replacing raw Ed25519 public keys. This provides standards-compliant identity with built-in support for key rotation, multiple keys per identity, and service endpoint encoding — all without servers or blockchain.

```rust
pub struct Relationship {
    /// Peer's DID (did:peer numalgo 2)
    pub peer_did: DidPeer,
    /// Resolved DID Document (cached, updated on key rotation)
    pub peer_did_document: DidDocument,
    /// Kind of relationship (user-declared)
    pub kind: RelationshipKind,
    /// Multi-dimensional trust (replaces flat TrustLevel)
    pub trust: TrustRelation,
    /// Spaces shared with this identity
    pub shared_spaces: Vec<SharedSpaceConfig>,
    /// When this relationship was established
    pub established: Timestamp,
    /// How this relationship was established
    pub establishment: EstablishmentMethod,
    /// Verifiable credentials issued to/by this peer
    pub credentials: Vec<SdJwtCredential>,
    /// Key event history for this peer (append-only)
    pub key_events: Vec<SignedKeyEvent>,
}

pub enum RelationshipKind {
    /// Close personal relationship — highest default trust
    Family,
    /// Personal relationship — high trust
    Friend,
    /// Professional relationship — moderate trust
    Colleague,
    /// Met once, low interaction — low trust
    Acquaintance,
    /// Automated service (API, bot, company) — minimal trust
    Service,
    /// No established relationship — no trust
    Unknown,
}

pub enum EstablishmentMethod {
    /// Mutual introduction via existing relationship
    MutualIntroduction { introducer: DidPeer },
    /// Direct peer discovery on local network (TOFU)
    PeerDiscovery,
    /// In-person QR code exchange (verified)
    QrExchange,
    /// NFC tap exchange (verified)
    NfcExchange,
    /// SPAKE2+ password-authenticated pairing (TOFU)
    Spake2Pairing,
    /// Imported from external service
    ServiceImport { service: ServiceId },
}
```

### 5.2 did:peer Identity Representation

AIOS uses `did:peer` numalgo 2 for all identity representation. Each DID encodes multiple keys and service endpoints directly in the DID string — no resolution infrastructure needed.

```rust
/// did:peer numalgo 2: keys and services encoded in the DID string
pub struct DidPeer {
    /// The full DID string (e.g., "did:peer:2.Ez6Mk...Vz6Mk...SeyJ...")
    pub did: String,
    /// Decoded keys from the DID
    pub keys: DidPeerKeys,
}

pub struct DidPeerKeys {
    /// Identity verification key (Ed25519 or hybrid Ed25519+ML-DSA-44)
    pub verification: PublicKeyBundle,
    /// Key agreement key (X25519 or hybrid X25519+ML-KEM-768)
    pub key_agreement: KeyAgreementBundle,
    /// Optional device-specific keys
    pub device_keys: Vec<DeviceKeyEntry>,
}

/// DID Document resolved from did:peer — cached locally, updated on rotation
pub struct DidDocument {
    pub id: String,
    pub verification_methods: Vec<VerificationMethod>,
    pub key_agreement: Vec<KeyAgreement>,
    pub service: Vec<ServiceEndpoint>,
    /// Last update timestamp (for staleness detection)
    pub updated: Timestamp,
}
```

**Why did:peer over raw public keys:**

| Aspect | Raw Ed25519 key | did:peer numalgo 2 |
|---|---|---|
| Key rotation | New key = new identity | Signed delta documents update in-place |
| Multiple keys | Single purpose | Identity + device + encryption in one DID |
| Service discovery | Out-of-band | Endpoints encoded in DID string |
| Standards compliance | Custom | W3C DID + DIF specification |
| Interoperability | AIOS-only | Any DID-compatible system |

### 5.3 Establishing Relationships

Relationships are established through verified exchange, never through unilateral declaration. The establishment method determines initial trust level via the TOFU upgrade pattern (§6.4).

```rust
impl IdentityService {
    /// Initiate pairing via SPAKE2+ (TOFU — Colleague-tier trust)
    pub fn initiate_spake2_pairing(
        &self,
        passphrase: &str,
    ) -> Result<Spake2Session, Error> {
        let session = spake2::start_a(passphrase, self.identity.did_peer());
        Ok(session)
    }

    /// Complete relationship after SPAKE2+ or QR/NFC exchange
    pub fn establish_relationship(
        &mut self,
        peer_did: DidPeer,
        peer_document: DidDocument,
        method: EstablishmentMethod,
        kind: RelationshipKind,
    ) -> Result<Relationship, Error> {
        // 1. Verify peer's DID Document signature
        let valid = self.verify_did_document(&peer_did, &peer_document)?;
        if !valid {
            return Err(Error::InvalidDidDocument);
        }

        // 2. Determine initial verification level from method
        let verification_level = match &method {
            EstablishmentMethod::QrExchange
            | EstablishmentMethod::NfcExchange => VerificationLevel::Verified,
            EstablishmentMethod::Spake2Pairing
            | EstablishmentMethod::PeerDiscovery => VerificationLevel::Tofu,
            EstablishmentMethod::MutualIntroduction { .. } => {
                VerificationLevel::IntroducedTofu
            }
            EstablishmentMethod::ServiceImport { .. } => {
                VerificationLevel::Unverified
            }
        };

        // 3. Compute initial trust from kind + verification
        let trust = TrustRelation::initial(
            &kind,
            verification_level,
        );

        // 4. Issue SD-JWT credential for the relationship
        let credential = self.issue_relationship_credential(
            &peer_did,
            &kind,
            &method,
        )?;

        let relationship = Relationship {
            peer_did,
            peer_did_document: peer_document,
            kind,
            trust,
            shared_spaces: Vec::new(),
            established: Timestamp::now(),
            establishment: method,
            credentials: vec![credential],
            key_events: Vec::new(),
        };

        // 5. Store in system space
        space::write(
            &format!(
                "system/identity/relationships/{}",
                relationship.peer_did.short_id()
            ),
            &relationship,
        );

        Ok(relationship)
    }
}
```

### 5.4 Mutual Introduction

A trusted intermediary can introduce two identities. The introduction carries the introducer's trust as a signal (attenuated by the TOFU discount):

```rust
pub struct Introduction {
    /// Who is being introduced (their did:peer)
    pub subject_did: DidPeer,
    pub subject_document: DidDocument,
    /// Who is introducing them
    pub introducer_did: DidPeer,
    /// Suggested relationship kind
    pub suggested_kind: RelationshipKind,
    /// SD-JWT credential signed by introducer vouching for subject
    pub vouching_credential: SdJwtCredential,
    /// Introducer's signature over the introduction
    pub signature: VersionedSignature,
}
```

When Alice introduces Bob to Carol, both Bob and Carol receive an `Introduction` signed by Alice and backed by an SD-JWT credential. They can verify Alice's signature (they both trust Alice) and establish a relationship with initial trust derived from Alice's trust level, attenuated by the transitive trust factor (§6.2).

### 5.5 Verifiable Credentials (SD-JWT)

AIOS uses **SD-JWT (Selective Disclosure JWT)** for verifiable credentials. SD-JWT is Ed25519-compatible, requires no pairing cryptography, and allows holders to selectively disclose individual claims.

```rust
/// SD-JWT credential — standard JWT with individually disclosable claims
pub struct SdJwtCredential {
    /// The signed JWT (header.payload.signature)
    pub jwt: SignedJwt,
    /// Individual disclosures (salt + claim pairs)
    pub disclosures: Vec<Disclosure>,
    /// Credential type
    pub credential_type: CredentialType,
    /// Expiry (optional)
    pub expires: Option<Timestamp>,
}

pub struct Disclosure {
    /// Random salt (prevents correlation)
    pub salt: [u8; 16],
    /// Claim name
    pub claim_name: String,
    /// Claim value
    pub claim_value: CredentialValue,
}

pub enum CredentialType {
    /// "This device runs genuine AIOS version X"
    DeviceAttestation,
    /// "I authorize this device to act on my behalf for N hours"
    TrustDelegation { duration_hours: u32 },
    /// "I am a designated recovery guardian for user X"
    RecoveryAuthorization,
    /// "I grant read/write access to Space Y"
    CapabilityTransfer,
    /// "I vouch for this identity's relationship kind"
    RelationshipVouching,
}
```

**Selective disclosure flow:**

1. Issuer (user) creates SD-JWT with multiple claims (trust level, device count, relationship kind, etc.)
2. Each claim is individually salted and hashed; hashes are in the JWT
3. Holder presents JWT + only the disclosures they choose to reveal
4. Verifier checks JWT signature + disclosed claim hashes

**Why SD-JWT over BBS+ or AnonCreds:**

| System | Ed25519 compatible | Complexity | Proof size | AIOS fit |
|---|---|---|---|---|
| **SD-JWT** | Yes | Low | ~200B base + ~100B/claim | Best — simple, standard, Ed25519 native |
| BBS+ | No (needs BLS12-381) | Medium | ~400B + ~32B/claim | Second curve required |
| AnonCreds | No (CL signatures) | High | ~1-2 KB | Too complex for local-first OS |

-----

## 6. Trust Model

### 6.1 TrustRelation Structure

Trust is not a single `f32` — it is a multi-dimensional structure that captures direct assessment, verification strength, context-specific trust, and transitive graph effects:

```rust
/// Multi-dimensional trust — replaces flat f32
pub struct TrustRelation {
    /// Direct trust from personal interactions [0.0, 1.0]
    pub direct_trust: f32,
    /// How this identity was verified
    pub verification_level: VerificationLevel,
    /// Context-specific trust scores [0.0, 1.0] each
    pub context_trust: ContextTrust,
    /// Trust propagated through 1-2 hops (EigenTrust)
    pub transitive_trust: f32,
    /// How much data backs this score (0.0 = no data, 1.0 = extensive history)
    pub confidence: f32,
    /// Interaction statistics (feeds trust computation)
    pub interaction_history: InteractionStats,
    /// Timestamp of last trust recalculation
    pub last_computed: Timestamp,
}

pub enum VerificationLevel {
    /// In-person key verification (QR fingerprint match, NFC tap)
    Verified,
    /// Trust-on-first-use (SPAKE2+ pairing, network discovery)
    Tofu,
    /// Introduced by a trusted intermediary (TOFU via introducer)
    IntroducedTofu,
    /// No verification performed
    Unverified,
}

/// Context-specific trust — different domains have independent scores
pub struct ContextTrust {
    /// Trust for recovery operations (guardian role)
    pub recovery: f32,
    /// Trust for data sharing (space access)
    pub data_sharing: f32,
    /// Trust for device management (device addition/revocation)
    pub device_management: f32,
    /// Trust for communication (attention priority, Flow auto-accept)
    pub communication: f32,
}

pub struct InteractionStats {
    /// Total interactions observed
    pub total_interactions: u32,
    /// Successful interactions (completed shares, valid messages)
    pub successful_interactions: u32,
    /// Failed or suspicious interactions
    pub failed_interactions: u32,
    /// First interaction timestamp
    pub first_seen: Timestamp,
    /// Most recent interaction timestamp
    pub last_seen: Timestamp,
    /// Running mean of interaction interval (seconds)
    pub mean_interval: f32,
}
```

### 6.2 EigenTrust 2-Hop Propagation

AIOS computes transitive trust via bounded EigenTrust propagation. Trust flows through the relationship graph but attenuates rapidly — preventing gaming while enabling useful "friend-of-friend" trust:

```rust
impl TrustEngine {
    /// Compute combined trust score for a peer.
    /// Direct trust (0.7 weight) + 1-hop transitive (0.25) + 2-hop (0.05).
    pub fn compute_trust(&self, peer: &DidPeer) -> TrustScore {
        let direct = self.direct_trust(peer);

        // 1-hop: trust of peers who trust this peer
        let transitive_1hop = self.transitive_trust(peer, 1);

        // 2-hop: trust through peers-of-peers
        let transitive_2hop = self.transitive_trust(peer, 2);

        let combined = 0.70 * direct
            + 0.25 * transitive_1hop
            + 0.05 * transitive_2hop;

        TrustScore {
            direct,
            transitive: 0.25 * transitive_1hop + 0.05 * transitive_2hop,
            combined: combined.clamp(0.0, 1.0),
            confidence: self.confidence(peer),
        }
    }

    /// Compute N-hop transitive trust.
    /// Pre-trusted set = user's own devices + Family-tier contacts.
    fn transitive_trust(&self, target: &DidPeer, hops: u8) -> f32 {
        let mut score = 0.0_f32;
        let attenuation = match hops {
            1 => 0.5,
            2 => 0.25,
            _ => return 0.0,
        };

        for (intermediary, rel) in &self.relationships {
            if intermediary == target {
                continue;
            }
            let intermediary_trust = rel.trust.direct_trust;
            if intermediary_trust < 0.1 {
                continue; // Skip untrusted intermediaries
            }

            if hops == 1 {
                // Does intermediary trust target?
                if let Some(intermediary_opinion) =
                    self.peer_trust_for(intermediary, target)
                {
                    score += intermediary_trust
                        * intermediary_opinion
                        * attenuation;
                }
            } else {
                // 2-hop: recurse through intermediary's peers
                let hop1 = self.transitive_trust_via(
                    intermediary, target, attenuation,
                );
                score += intermediary_trust * hop1 * attenuation;
            }
        }

        score.clamp(0.0, 1.0)
    }
}
```

**EigenTrust properties on AIOS's bounded graph:**

| Property | Value |
|---|---|
| Typical peer count | 5–100 |
| Trust matrix size | ~10 KB (100×100 × 4 bytes) |
| Computation time | <1μs on <100 peers |
| Pre-trusted set | User's own devices + Family-tier contacts |
| Propagation depth | 2 hops maximum |
| Update frequency | On-event (interaction, verification, key change) |
| Sybil resistance | Creating fake nodes without Family-tier connections has no effect |

### 6.3 Trust Signals

| Signal | Source | Direct trust effect | Context affected |
|---|---|---|---|
| Relationship kind (user-declared) | User declaration | Family=0.9, Friend=0.7, Colleague=0.5, Acquaintance=0.3 | All contexts |
| Verification level | Establishment method | Verified=+0.1, TOFU=0, IntroducedTOFU=-0.05 | All contexts |
| Interaction frequency | Interaction stats | Frequent & successful → +0.1 | Communication |
| Successful space sharing | Space sharing history | Clean sharing history → +0.1 | Data sharing |
| Recovery participation | Recovery events | Successful share refresh → +0.15 | Recovery |
| Device attestation | SD-JWT credential | Valid attestation → +0.05 | Device management |
| Mutual connections | Trust graph | Pre-trusted shared peers → transitive boost | All (via EigenTrust) |
| Time since last interaction | System clock | >6 months → exponential decay | All contexts |
| Key change without ceremony | Key transparency log | Unexpected key change → -0.3, auto-downgrade | All contexts |
| Negative behavioral signal | Behavioral monitor | Suspicious pattern → -0.2 | Affected context |

### 6.4 TOFU + Verification Upgrade Pattern

Relationships start at TOFU trust and can be upgraded through verification ceremonies. Key changes trigger SSH-style warnings with automatic trust downgrade:

```text
TOFU Pairing (Colleague-tier)
    │
    ├─── QR/NFC Verification ──→ Verified (Family-tier eligible)
    │
    ├─── Successful Interactions ──→ Trust score increases (within tier)
    │
    └─── Key Change Detected ──→ WARNING + automatic downgrade to TOFU
              │
              └─── Re-verification ──→ Trust restored
```

```rust
impl IdentityService {
    /// Handle key rotation notification from a peer.
    /// SSH-style: warn on unexpected key change, auto-downgrade trust.
    pub fn handle_peer_key_rotation(
        &mut self,
        peer_did: &DidPeer,
        signed_update: &SignedKeyRotation,
    ) -> Result<KeyRotationAction, Error> {
        let relationship = self.find_relationship_mut(peer_did)?;

        // Verify the rotation is signed by the OLD key
        let old_key = &relationship.peer_did_document
            .verification_methods[0].public_key;
        let valid = crypto_core::verify_versioned(
            old_key,
            &signed_update.rotation_data,
            &signed_update.signature,
        )?;

        if !valid {
            // Unsigned key change — potential MITM or compromise
            relationship.trust.verification_level =
                VerificationLevel::Unverified;
            relationship.trust.direct_trust *= 0.3; // Severe penalty
            return Ok(KeyRotationAction::Rejected {
                reason: "Signature verification failed",
            });
        }

        // Valid rotation — update DID Document, log event
        relationship.peer_did_document =
            signed_update.new_document.clone();
        relationship.key_events.push(signed_update.to_key_event());

        // Downgrade from Verified to TOFU (key changed, fingerprint no longer matched)
        if relationship.trust.verification_level == VerificationLevel::Verified {
            relationship.trust.verification_level = VerificationLevel::Tofu;
            return Ok(KeyRotationAction::AcceptedWithDowngrade {
                message: "Key rotated. Re-verify via QR/NFC to restore Verified status.",
            });
        }

        Ok(KeyRotationAction::Accepted)
    }
}
```

### 6.5 Trust Decay

Trust decays exponentially without interaction, with interaction events resetting the decay clock. The decay rate varies by relationship kind — Family relationships decay slowly, Service relationships decay quickly:

```rust
impl TrustRelation {
    /// Apply time-based trust decay.
    /// c(t) = direct_trust * exp(-λ * t)
    /// where t = time since last interaction, λ = decay rate.
    pub fn apply_decay(&mut self, now: Timestamp) {
        let t_seconds = (now - self.interaction_history.last_seen)
            .as_secs() as f64;

        // Decay rates per relationship kind (seconds^-1)
        // Family: half-life ~180 days; Service: half-life ~30 days
        let lambda = self.decay_rate();
        let factor = (-lambda * t_seconds).exp() as f32;

        self.direct_trust *= factor;
        self.transitive_trust *= factor;

        // Context trust decays independently
        self.context_trust.recovery *= factor;
        self.context_trust.data_sharing *= factor;
        self.context_trust.device_management *= factor;
        self.context_trust.communication *= factor;

        self.last_computed = now;
    }

    fn decay_rate(&self) -> f64 {
        // ln(2) / half_life_seconds
        let half_life_days = match self.verification_level {
            VerificationLevel::Verified => 365.0,
            VerificationLevel::Tofu => 180.0,
            VerificationLevel::IntroducedTofu => 90.0,
            VerificationLevel::Unverified => 30.0,
        };
        core::f64::consts::LN_2 / (half_life_days * 86400.0)
    }
}
```

### 6.6 Trust Effects

Trust level influences multiple system behaviors. The effective trust level is derived from the combined score (§6.2):

```rust
pub struct TrustEffects {
    pub attention_priority_boost: i8,
    pub space_sharing_default: AccessLevel,
    pub capability_sharing: bool,
    pub flow_auto_accept: bool,
    pub peer_sync_enabled: bool,
    pub recovery_guardian_eligible: bool,
}

impl TrustEffects {
    pub fn for_score(score: &TrustScore) -> Self {
        match score.combined {
            s if s >= 0.8 => TrustEffects {
                attention_priority_boost: 2,
                space_sharing_default: AccessLevel::ReadWrite,
                capability_sharing: true,
                flow_auto_accept: true,
                peer_sync_enabled: true,
                recovery_guardian_eligible: true,
            },
            s if s >= 0.5 => TrustEffects {
                attention_priority_boost: 1,
                space_sharing_default: AccessLevel::ReadOnly,
                capability_sharing: false,
                flow_auto_accept: false,
                peer_sync_enabled: true,
                recovery_guardian_eligible: false,
            },
            s if s >= 0.2 => TrustEffects {
                attention_priority_boost: 0,
                space_sharing_default: AccessLevel::None,
                capability_sharing: false,
                flow_auto_accept: false,
                peer_sync_enabled: false,
                recovery_guardian_eligible: false,
            },
            _ => TrustEffects {
                attention_priority_boost: -2,
                space_sharing_default: AccessLevel::None,
                capability_sharing: false,
                flow_auto_accept: false,
                peer_sync_enabled: false,
                recovery_guardian_eligible: false,
            },
        }
    }
}
```

### 6.7 Key Transparency Log

Each identity maintains an append-only, hash-chained log of key events. This provides an auditable history of identity changes and enables SSH-style "key changed" warnings with cryptographic evidence. See [core.md §4.9](./core.md) for the `KeyEvent` and `SignedKeyEvent` data structures.

```rust
impl IdentityService {
    /// Verify a peer's key transparency log for consistency.
    /// Detects gaps, forks, or tampered entries.
    pub fn verify_peer_key_log(
        &self,
        peer_did: &DidPeer,
        log: &[SignedKeyEvent],
    ) -> Result<LogVerification, Error> {
        let mut prev_hash: Option<[u8; 32]> = None;

        for event in log {
            // Verify hash chain continuity
            if event.previous_hash != prev_hash {
                return Ok(LogVerification::ChainBroken {
                    at_sequence: event.sequence,
                });
            }

            // Verify signature on each event
            let valid = crypto_core::verify_versioned(
                &event.signing_key,
                &event.event_data(),
                &event.signature,
            )?;
            if !valid {
                return Ok(LogVerification::InvalidSignature {
                    at_sequence: event.sequence,
                });
            }

            prev_hash = Some(event.compute_hash());
        }

        Ok(LogVerification::Valid {
            entries: log.len(),
            latest_sequence: log.last().map(|e| e.sequence).unwrap_or(0),
        })
    }
}
```

Peers exchange key transparency logs during relationship establishment and periodically during sync. Discrepancies (forked logs, missing entries) trigger trust downgrade and user notification — the cryptographic equivalent of SSH's `WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED`.
