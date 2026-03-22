# AIOS Privacy & Recovery Design

Part of: [identity.md](../identity.md) — Identity & Relationships
**Related:** [core.md](./core.md) — Key hierarchy & Crypto Core, [relationships.md](./relationships.md) — Trust model & did:peer, [credentials.md](./credentials.md) — Credential isolation, [cross-device.md](./cross-device.md) — Multi-device key escrow

**Cross-references:** [data-protection](../../platform/multi-device/data-protection.md) — DLP & encryption zones, [privacy](../../security/privacy.md) — System-wide privacy architecture

-----

## 13. Privacy

### 13.1 Local-First Identity

- No central identity server. No "identity provider."
- Identity is created on-device and stays on-device.
- Relationships are stored locally, not in a cloud database.
- Identity information is shared only with explicit consent.
- DID documents ([relationships.md §5.2](./relationships.md)) are peer-exchanged, never published to a ledger.

### 13.2 Minimal Disclosure

When interacting with peers or services, the user controls what identity information is shared:

```rust
pub struct IdentityDisclosureConfig {
    /// What to share with peers by default
    pub peer_default: PeerDisclosure,
    /// Per-relationship overrides (keyed by did:peer)
    pub relationship_overrides: HashMap<DidPeer, PeerDisclosure>,
    /// What to share with services by default
    pub service_default: ServiceDisclosure,
}

pub struct PeerDisclosure {
    pub share_display_name: bool,
    pub share_avatar: bool,
    pub share_device_count: bool,
    pub share_relationship_count: bool,
}

/// What identity information is disclosed to system services.
pub struct ServiceDisclosure {
    /// Share a stable pseudonymous ID (not the real IdentityId)
    pub share_pseudonym: bool,
    /// Share display name with the service
    pub share_display_name: bool,
}
```

### 13.3 Anonymous Mode

For interactions where identity should not be revealed:

```rust
impl IdentityService {
    pub fn create_anonymous_session(&self) -> AnonymousSession {
        // Generate ephemeral key pair (not linked to primary identity)
        let ephemeral = crypto_core::generate_ed25519();

        AnonymousSession {
            ephemeral_key: ephemeral.public,
            key_id: ephemeral.id,
            // No link to primary identity — separate did:peer
            ephemeral_did: DidPeer::from_key(&ephemeral.public),
            linked_identity: None,
            expires: SystemTime::now() + Duration::from_secs(24 * 3600),
        }
    }
}
```

Anonymous sessions use ephemeral keys that are not linked to the primary identity. The peer sees a valid Ed25519 public key (and corresponding did:peer) but cannot connect it to the user's real identity.

### 13.4 Selective Disclosure

Beyond binary share/don't-share, AIOS supports cryptographic selective disclosure — proving properties about identity without revealing the underlying data.

#### 13.4.1 Schnorr Proofs (Free with Ed25519)

Ed25519 signatures are Schnorr signatures over the edwards25519 curve. This provides zero-knowledge proof of key possession without revealing the key itself:

```rust
pub struct IdentityProof {
    /// Proves ownership of the identity key without revealing it
    pub proof_type: ProofType,
    /// Challenge-response nonce (prevents replay)
    pub challenge: [u8; 32],
    /// Schnorr proof bytes
    pub proof: SchnorrProof,
}

pub enum ProofType {
    /// "I own this did:peer" — proves key possession
    KeyOwnership,
    /// "I have a relationship with X at trust level >= Y"
    TrustThreshold { min_level: f32 },
    /// "I am a member of group G" — without revealing which member
    GroupMembership { group_id: GroupId },
}
```

#### 13.4.2 Bulletproofs for Range Proofs

For privacy-preserving trust verification — proving a trust score exceeds a threshold without revealing the exact score:

```rust
pub struct TrustRangeProof {
    /// Bulletproof: proves trust_score >= threshold without revealing trust_score
    pub proof: BulletproofBytes,  // ~700 bytes
    /// The threshold being proven against (public)
    pub threshold: f32,
    /// Verification time: ~3ms on Cortex-A72
    pub verifier_cost: Duration,
}
```

**Properties:**

| Property | Value |
|---|---|
| Proof size | ~700 bytes (logarithmic in range) |
| Verification time | ~3ms on Cortex-A72 |
| Prover time | ~10ms |
| Trusted setup | None (transparent) |
| Batching | Supported (amortized verification) |

**Use cases:**

- "Prove I'm trusted enough to join this shared space" — without revealing exact trust score
- "Prove I've had this identity for >1 year" — without revealing creation date
- "Prove my device count is ≥2" — without revealing exact count (for recovery eligibility)

#### 13.4.3 Ring Signatures for Anonymous Group Actions

For actions that should be attributable to a group but not to a specific member:

```rust
pub struct RingSignature {
    /// Signs a message on behalf of a group without revealing which member signed
    pub signature: RingSignatureBytes,
    /// The ring of public keys (group members)
    pub ring: Vec<PublicKey>,
    /// Message being signed
    pub message_hash: [u8; 32],
}

impl IdentityService {
    /// Sign a message anonymously within a group (e.g., anonymous feedback,
    /// whistleblowing, anonymous voting within a team space).
    pub fn ring_sign(
        &self,
        message: &[u8],
        group_members: &[PublicKey],
    ) -> Result<RingSignature, Error> {
        // Signer must be in the ring
        if !group_members.contains(&self.identity.public_key) {
            return Err(Error::NotInRing);
        }
        // Linkable ring signature (Borromean) — same signer produces
        // same key image, preventing double-signing without deanonymization
        crypto_core::ring_sign(self.primary_key_id, message, group_members)
    }
}
```

#### 13.4.4 ZKP Architecture Split

| Technique | Runtime | Use case |
|---|---|---|
| Schnorr proofs | Kernel Crypto Core | Key ownership, identity proof |
| Bulletproofs | Kernel Crypto Core | Range proofs, trust thresholds |
| Ring signatures | Kernel Crypto Core | Anonymous group actions |
| zk-SNARKs | AIRS (too heavy for kernel) | Arbitrary predicate proofs, complex attestations |

Schnorr proofs, Bulletproofs, and ring signatures run in the kernel Crypto Core — they are small, constant-time, and have no trusted setup. zk-SNARKs are deferred to AIRS for complex predicates that exceed kernel complexity budgets.

-----

## 14. Recovery Design: Prevention First, Safety Net Second

AIOS takes a **prevention-first** approach to recovery. The primary design goal is to prevent lockout — not to recover from it. Recovery mechanisms exist as a safety net, not as a primary workflow.

**Design philosophy:** Every recovery mechanism is an attack surface. AIOS minimizes this surface by making recovery rare (aggressive session persistence), graduated (different ceremony levels for different assets), and time-delayed (cancel windows prevent rapid theft).

### 14.1 Prevention-Based Foundation

Before any recovery mechanism activates, AIOS prevents lockout through aggressive session persistence and low-friction passphrase management. This is the **primary** defense — recovery is secondary.

**1. Aggressive session persistence.** Once authenticated, the session stays alive as long as possible. The master key is sealed to the current boot session using the device's TPM or Secure Enclave (when available). The user only re-enters their passphrase after a cold reboot — not on lid close, not on sleep, not on screen lock.

```rust
pub enum SessionPersistence {
    /// Master key sealed to TPM/Secure Enclave for current boot session.
    /// Key survives sleep/wake cycles. Destroyed only on shutdown/reboot.
    /// This is the default on devices with a secure element.
    HardwareSealed {
        sealed_blob: Vec<u8>,
    },
    /// Master key kept in pinned kernel memory across sleep/wake.
    /// Less secure than hardware sealing (vulnerable to cold boot attacks)
    /// but available on all devices. Destroyed on shutdown/reboot.
    KernelPinned,
}
```

**2. Passphrase change while authenticated.** While the session is live (master key in memory), the user can change their passphrase at any time. This is the "recovery" path — it happens *before* the user forgets, not after.

```rust
impl IdentityService {
    /// Change the identity passphrase while the session is active.
    /// The master key is already in memory — re-derive it under the new passphrase.
    pub fn change_passphrase(
        &mut self,
        current_passphrase: &str,
        new_passphrase: &str,
    ) -> Result<(), Error> {
        // 1. Verify current passphrase (defense against unattended terminal)
        let verify_key = derive_master_key(current_passphrase, &self.identity_salt);
        if verify_key != self.master_key {
            return Err(Error::InvalidPassphrase);
        }

        // 2. Generate new salt
        let new_salt = crypto_core::random_bytes::<32>();

        // 3. Derive new master key from new passphrase
        let new_master = derive_master_key(new_passphrase, &new_salt);

        // 4. Re-encrypt all space keys under new master key
        for (space_id, encrypted_key) in &mut self.space_keys {
            let decrypted = decrypt_space_key(&self.master_key, encrypted_key);
            *encrypted_key = encrypt_space_key(&new_master, &decrypted);
        }

        // 5. Update stored salt and master key
        self.identity_salt = new_salt;
        self.master_key = new_master;

        // 6. Persist updated key material
        space::write("system/identity/key_metadata", &self.key_metadata());

        Ok(())
    }
}
```

**3. Clear warning at setup.** During first boot:

```text
Your passphrase protects all data on this device.

You can change your passphrase at any time while logged in.

For maximum protection, pair a second AIOS device — this enables
device-to-device recovery if you ever forget your passphrase.
```

### 14.2 Graduated Recovery Tiers

Different assets require different recovery guarantees. A single recovery mechanism for everything is either too strict (session tokens) or too weak (master identity key).

```text
Tier 1 — Session Recovery (low ceremony)
  What:       Session tokens, cached credentials, temporary auth state
  Threat:     Device reboot, app crash
  Mechanism:  TPM-sealed session + device passphrase
  Time:       Seconds (automatic on boot)
  Failure:    Re-authenticate; mild inconvenience

Tier 2 — Device Key Recovery (medium ceremony)
  What:       Per-device encryption keys, local storage keys
  Threat:     Device loss, hardware failure
  Mechanism:  2-of-N Feldman VSS shares from paired devices + passphrase
  Time:       Minutes (requires one other AIOS device)
  Failure:    Lose device-local data (identity survives)

Tier 3 — Identity Recovery (high ceremony)
  What:       Ed25519 identity key (Level 1 in key hierarchy)
  Threat:     Loss of ALL devices, catastrophic failure
  Mechanism:  3-of-5 Feldman VSS + dead man's switch + 48-72h delay
  Time:       Hours to days (intentional delay)
  Failure:    Permanent identity loss if recovery fails
```

**Key invariant:** Each tier's recovery mechanism is independent. Compromising Tier 1 recovery does not aid a Tier 3 attack. This is enforced by separate key hierarchies:

```text
Identity Key (Ed25519, Level 1)           ← Tier 3 recovery
  └─ Device Key (Ed25519, Level 2)        ← Tier 2 recovery
       └─ Session Key (symmetric)         ← Tier 1 recovery
```

**Recovery paths are additive, never mutually exclusive.** Apple's mistake: enabling a Recovery Key disables Recovery Contact. AIOS never removes a recovery path when another is configured.

### 14.3 Tier 1 — Session Recovery

Session recovery is automatic and invisible. It is the prevention-first philosophy in action.

```rust
pub struct SessionRecovery {
    /// TPM-sealed master key (survives sleep/reboot if TPM available)
    pub sealed_state: SessionPersistence,
    /// Keystroke timing model confidence (§17.1.1)
    /// If confidence drops below threshold, require passphrase
    pub session_confidence: f32,
    /// Time since last explicit authentication
    pub last_auth: SystemTime,
    /// Continuous authentication via keystroke biometrics
    pub biometric_active: bool,
}

impl SessionRecovery {
    /// Called on every boot. If TPM-sealed state exists and
    /// the user's passphrase unlocks it, session resumes silently.
    pub fn attempt_resume(&self) -> Result<MasterKey, SessionExpired> {
        match &self.sealed_state {
            SessionPersistence::HardwareSealed { sealed_blob } => {
                // TPM unseal — requires same boot state (PCR values)
                crypto_core::tpm_unseal(sealed_blob)
                    .map_err(|_| SessionExpired)
            }
            SessionPersistence::KernelPinned => {
                // Kernel-pinned keys don't survive reboot
                Err(SessionExpired)
            }
        }
    }
}
```

### 14.4 Tier 2 — Device Key Recovery (Feldman VSS)

When a device is lost or fails, its encryption keys can be recovered from any `t` of the user's other AIOS devices. This uses Feldman Verifiable Secret Sharing over Ed25519 scalars.

#### 14.4.1 Feldman VSS Overview

Feldman VSS (1987) splits a secret into `n` shares such that any `t` shares can reconstruct it, while fewer than `t` shares reveal nothing. Unlike basic Shamir SSS, Feldman adds **verifiable commitments** — each shareholder can verify their share is valid without learning the secret.

```rust
/// A VSS share — one per paired device.
pub struct VssShare {
    /// Share index (1-based, unique per shareholder)
    pub index: u32,
    /// Share value — Ed25519 scalar (32 bytes)
    pub value: Scalar,
    /// Which key this share protects
    pub key_tier: RecoveryTier,
    /// Epoch (incremented on each refresh)
    pub epoch: u64,
}

/// Public commitments — verifiable by any shareholder.
pub struct VssCommitments {
    /// Commitments: C_i = g^{a_i} for each polynomial coefficient
    /// (t elements, each 32 bytes for Ed25519/Ristretto)
    pub commitments: Vec<RistrettoPoint>,
    /// Threshold required for reconstruction
    pub threshold: u32,
    /// Total number of shares dealt
    pub total_shares: u32,
}

impl VssShare {
    /// Verify this share against public commitments.
    /// g^{share_value} should equal product(C_i^{index^i}).
    pub fn verify(&self, commitments: &VssCommitments) -> bool {
        let g = RISTRETTO_BASEPOINT_POINT;
        let lhs = g * self.value;

        let mut rhs = commitments.commitments[0];
        let mut power = Scalar::from(self.index);
        for c in &commitments.commitments[1..] {
            rhs += c * power;
            power *= Scalar::from(self.index);
        }

        lhs == rhs
    }
}
```

#### 14.4.2 Share Distribution

Shares are distributed during device pairing — the same ceremony that establishes the multi-device relationship ([cross-device.md §8](./cross-device.md)).

```rust
impl IdentityService {
    /// Deal VSS shares for device key recovery.
    /// Called during device pairing when a new device joins the constellation.
    pub fn deal_device_key_shares(
        &self,
        device_key: &Scalar,
        paired_devices: &[DeviceId],
    ) -> Result<VssDealResult, Error> {
        let n = paired_devices.len() as u32;
        let t = recovery_threshold(n); // 2-of-N for Tier 2

        // Generate random polynomial: f(x) = device_key + a_1*x + ... + a_{t-1}*x^{t-1}
        let coefficients = generate_polynomial(device_key, t);

        // Compute commitments: C_i = g^{a_i}
        let commitments = coefficients.iter()
            .map(|a| RISTRETTO_BASEPOINT_POINT * a)
            .collect();

        // Evaluate polynomial at each device's index
        let shares: Vec<VssShare> = (1..=n)
            .map(|i| VssShare {
                index: i,
                value: evaluate_polynomial(&coefficients, Scalar::from(i)),
                key_tier: RecoveryTier::Device,
                epoch: 0,
            })
            .collect();

        // Each share is encrypted to its recipient's device key
        // and sent via the peer protocol
        Ok(VssDealResult { shares, commitments: VssCommitments {
            commitments,
            threshold: t,
            total_shares: n,
        }})
    }
}

/// Threshold selection: Tier 2 uses 2-of-N (low ceremony).
fn recovery_threshold(n: u32) -> u32 {
    match n {
        0..=1 => 1,  // Single device — passphrase only
        2..=4 => 2,  // 2-of-N
        _ => 2,      // Still 2-of-N; Tier 2 is intentionally low-friction
    }
}
```

**Storage per device:** 32 bytes (share) + 32 × t bytes (commitments) = ~96 bytes for 2-of-3. Negligible.

#### 14.4.3 Reconstruction

```rust
impl IdentityService {
    /// Reconstruct a device key from VSS shares (Lagrange interpolation).
    pub fn reconstruct_device_key(
        shares: &[VssShare],
        commitments: &VssCommitments,
    ) -> Result<Scalar, Error> {
        if shares.len() < commitments.threshold as usize {
            return Err(Error::InsufficientShares);
        }

        // Verify each share before using it
        for share in shares {
            if !share.verify(commitments) {
                return Err(Error::InvalidShare { index: share.index });
            }
        }

        // Lagrange interpolation at x=0 to recover the secret
        let mut secret = Scalar::ZERO;
        for (i, share_i) in shares.iter().enumerate() {
            let mut lagrange = Scalar::ONE;
            for (j, share_j) in shares.iter().enumerate() {
                if i != j {
                    let xi = Scalar::from(share_i.index);
                    let xj = Scalar::from(share_j.index);
                    lagrange *= xj * (xj - xi).invert();
                }
            }
            secret += share_i.value * lagrange;
        }

        Ok(secret)
    }
}
```

### 14.5 Tier 3 — Identity Recovery (VSS + Dead Man's Switch)

Identity recovery is the highest-ceremony operation in AIOS. It protects the Ed25519 identity key — the root of the user's entire digital existence.

#### 14.5.1 Identity Recovery Architecture

```text
                    ┌─────────────────────────────────────────┐
                    │         Identity Recovery Request        │
                    │                                         │
                    │  1. Collect 3-of-5 VSS shares           │
                    │  2. Verify share commitments             │
                    │  3. Verify passphrase (scrypt)           │
                    │  4. Publish recovery intent to all       │
                    │     devices (48-72h cancel window)       │
                    │  5. Any authenticated device can cancel  │
                    │  6. After delay: reconstruct identity    │
                    └─────────────────────────────────────────┘
```

#### 14.5.2 Share Configuration

```rust
pub struct IdentityRecoveryConfig {
    /// Threshold: 3-of-5 for identity recovery
    pub threshold: u32,        // default: 3
    pub total_shares: u32,     // default: 5
    /// Share distribution:
    /// - User's paired devices (weighted: primary=2, secondary=1)
    /// - Optional: trusted relationships (Family/Friend tier)
    pub shareholders: Vec<Shareholder>,
    /// Dead man's switch timeout
    pub heartbeat_timeout: Duration,   // default: 30 days
    /// Cancel window after recovery request
    pub cancel_window: Duration,       // default: 72 hours
}

pub struct Shareholder {
    pub holder_type: ShareholderType,
    pub device_or_peer: ShareholderId,
    /// Weight: primary device gets 2 sub-shares, others get 1
    pub weight: u32,
    /// Last confirmed reachability
    pub last_seen: SystemTime,
}

pub enum ShareholderType {
    /// User's own AIOS device
    OwnDevice { device_id: DeviceId },
    /// Trusted relationship (Family or Friend tier only)
    TrustedPeer { peer_did: DidPeer, min_trust: f32 },
}
```

#### 14.5.3 Weighted Shares

Primary devices receive more sub-shares than secondary devices, reflecting their higher security posture:

```text
Example: 3-of-5 with weights
  Primary laptop  (weight 2) → shares at indices 1, 2
  Phone           (weight 1) → share at index 3
  Tablet          (weight 1) → share at index 4
  Trusted friend  (weight 1) → share at index 5

Recovery scenarios:
  Primary + Phone     = indices {1,2,3} → meets threshold ✓
  Primary + Friend    = indices {1,2,5} → meets threshold ✓
  Phone + Tablet + Friend = indices {3,4,5} → meets threshold ✓
  Phone + Tablet      = indices {3,4}   → below threshold ✗
```

#### 14.5.4 Dead Man's Switch

The dead man's switch ensures recovery is possible even if the user becomes incapacitated, without enabling rapid theft:

```rust
pub struct DeadManSwitch {
    /// Heartbeat: user authenticates to ANY device
    pub last_heartbeat: SystemTime,
    /// Configurable timeout (default: 30 days)
    pub timeout: Duration,
    /// State machine
    pub state: SwitchState,
}

pub enum SwitchState {
    /// Normal operation — heartbeat received within timeout
    Active,
    /// Heartbeat missed — warning period (7 days)
    Warning {
        missed_since: SystemTime,
    },
    /// Timeout expired — VSS shares activated for recovery
    /// Recovery still requires threshold shares + cancel window
    Activated {
        activated_at: SystemTime,
    },
}

impl DeadManSwitch {
    /// Called on every user authentication event (passphrase, biometric, etc.)
    pub fn heartbeat(&mut self) {
        self.last_heartbeat = SystemTime::now();
        self.state = SwitchState::Active;
    }

    /// Periodic check (called by kernel timer, e.g., daily)
    pub fn check(&mut self) -> SwitchAction {
        let elapsed = SystemTime::now()
            .duration_since(self.last_heartbeat)
            .unwrap_or(Duration::from_secs(0));

        match &self.state {
            SwitchState::Active if elapsed > self.timeout - Duration::from_secs(7 * 24 * 3600) => {
                self.state = SwitchState::Warning {
                    missed_since: self.last_heartbeat,
                };
                SwitchAction::NotifyUser  // "Authenticate soon to keep recovery inactive"
            }
            SwitchState::Warning { .. } if elapsed > self.timeout => {
                self.state = SwitchState::Activated {
                    activated_at: SystemTime::now(),
                };
                SwitchAction::ActivateRecovery
            }
            _ => SwitchAction::None,
        }
    }
}
```

#### 14.5.5 Recovery Ceremony

```rust
impl IdentityService {
    /// Initiate identity recovery. This is a multi-step ceremony:
    /// 1. Collect threshold VSS shares from shareholders
    /// 2. Verify passphrase
    /// 3. Publish recovery intent (starts cancel window)
    /// 4. Wait for cancel window to expire
    /// 5. Reconstruct identity key
    pub fn initiate_identity_recovery(
        &self,
        shares: Vec<VssShare>,
        passphrase: &str,
        commitments: &VssCommitments,
    ) -> Result<RecoveryIntent, Error> {
        // Verify share count meets threshold
        if shares.len() < commitments.threshold as usize {
            return Err(Error::InsufficientShares);
        }

        // Verify each share against commitments
        for share in &shares {
            if !share.verify(commitments) {
                return Err(Error::InvalidShare { index: share.index });
            }
        }

        // Verify passphrase (scrypt-derived, not just SHA-256)
        if !self.verify_recovery_passphrase(passphrase) {
            return Err(Error::InvalidPassphrase);
        }

        // Publish recovery intent to ALL known devices
        let intent = RecoveryIntent {
            requester_device: self.device_id,
            timestamp: SystemTime::now(),
            cancel_deadline: SystemTime::now() + self.config.cancel_window,
            shares_collected: shares.len() as u32,
        };

        // Broadcast to all devices — any authenticated device can cancel
        for device in &self.known_devices {
            self.peer_protocol.send(device, PeerMessage::RecoveryIntent {
                intent: intent.clone(),
            });
        }

        Ok(intent)
    }

    /// Cancel a pending recovery from any authenticated device.
    pub fn cancel_recovery(&self, intent_id: &RecoveryIntentId) -> Result<(), Error> {
        // Broadcast cancellation to all devices
        for device in &self.known_devices {
            self.peer_protocol.send(device, PeerMessage::RecoveryCancelled {
                intent_id: *intent_id,
                cancelled_by: self.device_id,
            });
        }
        Ok(())
    }

    /// Complete recovery after cancel window expires without cancellation.
    pub fn complete_identity_recovery(
        shares: &[VssShare],
        commitments: &VssCommitments,
        intent: &RecoveryIntent,
    ) -> Result<IdentityKey, Error> {
        // Verify cancel window has expired
        if SystemTime::now() < intent.cancel_deadline {
            return Err(Error::CancelWindowActive);
        }

        // Reconstruct identity key via Lagrange interpolation
        let identity_scalar = Self::reconstruct_device_key(shares, commitments)?;

        // Derive identity key pair from recovered scalar
        let identity_key = IdentityKey::from_scalar(identity_scalar);

        Ok(identity_key)
    }
}
```

### 14.6 Proactive Share Refresh (Herzberg Protocol)

Shareholders periodically refresh their shares **without reconstructing the secret**. This prevents a mobile adversary who compromises devices one-by-one over time from accumulating enough shares to reconstruct the key.

```rust
impl VssShareHolder {
    /// Participate in a proactive share refresh round.
    /// Each shareholder generates a random polynomial with zero constant term,
    /// deals sub-shares to all other shareholders, and adds received sub-shares
    /// to their current share.
    pub fn refresh_round(
        &mut self,
        other_shareholders: &[ShareholderId],
        epoch: u64,
    ) -> Result<(), Error> {
        // 1. Generate random polynomial f_i(x) with f_i(0) = 0
        //    (zero constant term preserves the secret)
        let zero = Scalar::ZERO;
        let refresh_poly = generate_polynomial(&zero, self.threshold);

        // 2. Evaluate at each shareholder's index → sub-share
        let sub_shares: Vec<(u32, Scalar)> = other_shareholders.iter()
            .enumerate()
            .map(|(j, _)| {
                let idx = (j as u32) + 1;
                (idx, evaluate_polynomial(&refresh_poly, Scalar::from(idx)))
            })
            .collect();

        // 3. Send encrypted sub-shares to each shareholder
        for (idx, sub_share) in &sub_shares {
            self.send_sub_share(*idx, *sub_share, epoch);
        }

        // 4. Receive sub-shares from all other shareholders
        let received = self.collect_sub_shares(epoch)?;

        // 5. Add all received sub-shares to current share
        for sub_share in received {
            self.share.value += sub_share;
        }
        self.share.epoch = epoch;

        Ok(())
    }
}
```

**Properties:**

| Property | Value |
|---|---|
| Messages per round | O(n²) — each of n shareholders sends to n-1 others |
| Share size change | None — shares remain 32 bytes |
| Secret reconstruction | Secret unchanged; old shares become invalid |
| Refresh trigger | Configurable interval (default: 90 days) or on device add/remove |

### 14.7 Threshold Resharing (Desmedt-Jajodia)

When devices are added or removed, the threshold configuration changes without ever reconstructing the secret:

```rust
impl IdentityService {
    /// Reshare from (t, n) to (t', n') without reconstructing the secret.
    /// Used when: user adds a device (n increases), user loses a device
    /// (n decreases), or security policy changes threshold.
    pub fn reshare(
        &self,
        old_shareholders: &[VssShare],  // t shares from old config
        new_config: VssConfig,          // new (t', n')
    ) -> Result<VssDealResult, Error> {
        // Each old shareholder deals their share as a new (t', n') sharing
        // New shareholders combine received sub-shares
        // The secret is never reconstructed in any single location

        let mut new_shares = Vec::with_capacity(new_config.total as usize);

        for new_idx in 1..=new_config.total {
            let mut combined = Scalar::ZERO;
            for old_share in old_shareholders {
                // Old shareholder evaluates their sub-polynomial at new index
                let sub = old_share.value
                    * lagrange_coefficient(old_share.index, old_shareholders, new_idx);
                combined += sub;
            }
            new_shares.push(VssShare {
                index: new_idx,
                value: combined,
                key_tier: RecoveryTier::Identity,
                epoch: old_shareholders[0].epoch + 1,
            });
        }

        Ok(VssDealResult {
            shares: new_shares,
            commitments: compute_new_commitments(&new_config),
        })
    }
}
```

### 14.8 Recovery Safety Invariants

```text
INVARIANT 1: Recovery paths are additive, never mutually exclusive
  Configuring Tier 3 recovery never disables Tier 1 or Tier 2.
  Adding guardian recovery never disables device-based recovery.

INVARIANT 2: Identity key is never reconstructed on a single device during normal operation
  Only during the recovery ceremony (after cancel window) is the key
  materialized. During refresh/reshare, the secret stays distributed.

INVARIANT 3: Compromising fewer than t shareholders reveals nothing
  Information-theoretic security: t-1 shares provide zero information
  about the secret. This is a mathematical guarantee, not a computational one.

INVARIANT 4: Cancel window cannot be shortened
  The cancel_window duration is set during identity creation and cannot
  be reduced. It can only be extended. This prevents an attacker who
  compromises the config from reducing the delay to zero.

INVARIANT 5: Dead man's switch timeout ≥ cancel window
  heartbeat_timeout >= cancel_window + 7 days (warning period).
  This ensures the user always has time to cancel a fraudulent recovery
  before the dead man's switch activates.
```

### 14.9 Key Compromise Response

If the identity key is compromised (not just a lost device — a compromised key), emergency rekeying is required. This is distinct from recovery:

```rust
impl IdentityService {
    pub fn emergency_rekey(&mut self) -> Result<(), Error> {
        // 1. Generate new identity key (Level 1 in key hierarchy)
        let new_key = crypto_core::generate_ed25519();
        let new_did = DidPeer::from_key(&new_key.public);

        // 2. Sign migration certificate with OLD key (proves continuity)
        let migration = MigrationCertificate {
            old_did: self.identity.did.clone(),
            new_did: new_did.clone(),
            old_public: self.identity.public_key,
            new_public: new_key.public,
            reason: MigrationReason::KeyCompromise,
            timestamp: SystemTime::now(),
        };
        let signed = crypto_core::sign(self.primary_key_id, &migration.to_bytes());

        // 3. Notify all relationships via peer protocol
        for rel in &self.identity.relationships {
            if let Some(peer) = self.network.find_peer(&rel.peer_did) {
                peer.send(PeerMessage::IdentityMigration {
                    certificate: signed.clone(),
                });
            }
        }

        // 4. Update identity
        self.identity.did = new_did;
        self.identity.public_key = new_key.public;

        // 5. Re-sign all device certificates under new identity key
        self.resign_all_device_certs();

        // 6. Rotate all shared space tokens
        self.rotate_all_space_tokens();

        // 7. Re-deal VSS shares under new identity key
        self.redeal_all_recovery_shares();

        // 8. Log to key transparency log
        self.key_log.append(SignedKeyEvent {
            event: KeyEvent::EmergencyRekey {
                old_did: migration.old_did,
                new_did: migration.new_did.clone(),
            },
            signature: signed,
            timestamp: migration.timestamp,
        });

        Ok(())
    }
}
```

### 14.10 What We Evaluated

| Mechanism | Status | Rationale |
|---|---|---|
| BIP-39 seed phrase | Rejected | Users lose paper (38% within 6 months — Voskobojnikov 2021). Digital storage negates purpose. |
| Recovery key file | Rejected | Same custodial burden. Users store it next to the device or lose it entirely. |
| OPAQUE (aPAKE) | Rejected | Requires an online server. Contradicts local-first design. |
| Cloud escrow | Rejected | Central authority. Contradicts sovereignty design. |
| Feldman VSS + multi-device | **Adopted (Tier 2/3)** | Serverless, verifiable, 32-byte shares, O(n) dealing, Ed25519-native. |
| Dead man's switch | **Adopted (Tier 3)** | On-device timer, no server needed. Configurable timeout + cancel window. |
| Proactive share refresh | **Adopted** | Prevents mobile adversary. O(n²) messages/round, negligible for <10 devices. |
| Threshold resharing | **Adopted** | Secret never reconstructed during device add/remove. |
| Social recovery (guardian model) | **Adopted (optional Tier 3)** | Trusted peers as shareholders — but only Family/Friend tier, and combined with device shares. |
| TEE escrow | **Adopted (Tier 1)** | TPM/Secure Enclave sealing for session keys. Platform-abstracted via CryptoBackend. |
