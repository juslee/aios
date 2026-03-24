# AIOS Space Storage — Encryption

Part of: [spaces.md](../spaces.md) — Space Storage System
**Related:** [block-engine.md](./block-engine.md) — Block Engine (§4.10 Device Encryption), [data-structures.md](./data-structures.md) — Core Data Structures, [sync.md](./sync.md) — Space Sync

-----

## 6. Encryption

> **Implementation status:** Phase 18a (not active in Phase 4a-4l). This section documents the design for per-space encryption. Phase 4 uses device-level encryption only (§4.10). Per-space encryption for Personal/Collaborative/Untrusted zones will be added in Phase 18a, providing cross-zone isolation within the running system.
>
> **Security note for Phases 4-17:** During Phases 4-17, all spaces rely solely on device-level encryption (§4.10). This means an attacker who obtains the device key (e.g., via physical access after boot, when the TPM/TrustZone-sealed key is loaded into memory) can read plaintext from ALL spaces — Personal, Collaborative, and Untrusted zones are not individually encrypted. The 8-layer security model's Layer 6 (Cryptographic Enforcement) operates at device granularity only until Phase 18a adds per-space keys. The other 7 layers (capability checks, intent verification, behavioral monitoring, etc.) still provide defense-in-depth during this period.

### 6.1 Key Management

```rust
/// The master storage key, derived from the user's identity passphrase.
/// Independent of the device key (§4.10) — different derivation salt.
pub struct MasterKey {
    /// 256-bit key material. Stored on a pinned kernel page
    /// (VmFlags::PINNED | VmFlags::NO_DUMP). Zeroized on drop.
    key_bytes: ZeroizeBox<[u8; 32]>,
    /// How this key was derived.
    derivation: KeyDerivationMethod,
}

pub enum KeyDerivationMethod {
    Argon2id {
        salt: [u8; 32],
        params: Argon2Params,
    },
}

pub struct Argon2Params {
    /// Memory cost in KiB (default: 65536 = 64 MB)
    m_cost: u32,
    /// Time cost / iterations (default: 3)
    t_cost: u32,
    /// Parallelism (default: 4)
    parallelism: u32,
}

pub struct SpaceKeyManager {
    /// Master key derived from user's identity passphrase
    master_key: MasterKey,
    /// Per-space keys (encrypted with master key)
    space_keys: HashMap<SpaceId, EncryptedSpaceKey>,
}

pub struct EncryptedSpaceKey {
    space: SpaceId,
    algorithm: EncryptionAlgorithm,
    encrypted_key: Vec<u8>,             // encrypted with master key
    key_version: u32,
    created_at: Timestamp,
}

pub enum EncryptionAlgorithm {
    Aes256Gcm,                          // default
    ChaCha20Poly1305,                   // alternative
}
```

**Key derivation flow:**
```text
1. User authenticates (password, biometric, hardware key)
2. Identity keys unlocked (Ed25519 keypair)
3. Master storage key derived: Argon2id(password, device_salt)
4. Per-space keys decrypted with master key
5. Spaces become accessible
```

**Key rotation:** Space keys can be rotated without re-encrypting all data. New writes use the new key. Old data is re-encrypted in the background. The rotation is tracked by a `KeyRotationManifest` in the WAL — if the system crashes during rotation, recovery resumes re-encryption from the last checkpointed block. Both old and new keys are retained until re-encryption completes, ensuring all blocks are always decryptable. During re-encryption, each block gets a fresh nonce from the `NonceGenerator` (§6.1.1) — the counter increments for every encryption operation, including re-encryption, so nonce reuse never occurs.

### 6.1.1 Nonce Management

AES-256-GCM requires a unique nonce (initialization vector) for every encryption operation under the same key. Reusing a nonce under the same key is catastrophic — it breaks GCM authentication and enables plaintext recovery via ciphertext XOR.

```rust
/// Counter-based nonce generation. Each space key tracks a monotonically
/// increasing counter. The nonce is constructed from the counter + a random
/// component to prevent nonce reuse across crash/recovery boundaries.
pub struct NonceGenerator {
    /// Monotonic counter, persisted to disk with the space key metadata.
    /// Incremented on every block write. On crash recovery, the counter
    /// is advanced by a safety margin (1000) to ensure no reuse.
    counter: AtomicU64,
    /// Random prefix (32 bits), generated at key creation time.
    /// Combined with the 64-bit counter to fill the 96-bit nonce.
    random_prefix: u32,
    /// Key identifier for the space key this generator is bound to.
    /// Used in NonceExhausted errors to identify which key needs rotation.
    key_id: KeyId,
}

/// Overflow safety threshold. When the counter reaches this value,
/// the space key MUST be rotated before any further encryption.
/// Set to u64::MAX - 2^20 (~1 million operations of safety margin)
/// to ensure no accidental wraparound. At 1 TB/month write rate with
/// 4 KB blocks, this counter lasts ~2.3 billion years — but key rotation
/// after device migration, crash recovery advances, or bulk re-encryption
/// could consume counter space faster. The guard is cheap insurance.
const NONCE_COUNTER_LIMIT: u64 = u64::MAX - (1 << 20);

impl NonceGenerator {
    /// Generate the next nonce. MUST be called exactly once per encryption.
    /// Returns Err if the counter has reached the overflow safety threshold,
    /// requiring a space key rotation before further encryption.
    pub fn next_nonce(&self) -> Result<[u8; 12], NonceExhausted> {
        let count = self.counter.fetch_add(1, Ordering::SeqCst);
        if count >= NONCE_COUNTER_LIMIT {
            return Err(NonceExhausted { space_key_id: self.key_id });
        }
        let mut nonce = [0u8; 12];
        nonce[..4].copy_from_slice(&self.random_prefix.to_le_bytes());
        nonce[4..].copy_from_slice(&count.to_le_bytes());
        Ok(nonce)
    }

    /// On crash recovery: advance counter by safety margin to guarantee
    /// no nonce reuse, even if some writes were lost.
    pub fn recover(&self, last_persisted: u64) {
        self.counter.store(last_persisted + 1000, Ordering::SeqCst);
    }
}

/// Error returned when the nonce counter approaches u64::MAX.
/// The space key must be rotated (§6.1) before further encryption.
pub struct NonceExhausted { pub space_key_id: KeyId }
```

**Why counter-based, not random?** Random 96-bit nonces have a birthday collision probability of ~2^-32 after 2^32 encryptions. For a space with millions of blocks across years of edits, this is uncomfortably close. Counter-based nonces guarantee uniqueness as long as the counter never repeats — which the monotonic counter + crash recovery margin ensures.

### 6.1.2 Key Zeroization and Memory Protection

Decrypted space keys are security-critical material. AIOS ensures they cannot leak to swap, remain in memory longer than needed, or be observable via side channels:

```rust
/// A decrypted space key in memory. Automatically zeroized on drop.
pub struct DecryptedSpaceKey {
    /// Key material — allocated on a dedicated kernel page that is:
    /// 1. mlock'd (pinned, never paged to swap or zram)
    /// 2. mprotect'd PROT_READ only (writes go through dedicated API)
    /// 3. Excluded from core dumps
    key_bytes: ZeroizeBox<[u8; 32]>,
    /// Space this key belongs to
    space_id: SpaceId,
    /// Key version (for rotation tracking)
    version: u32,
}

impl Drop for DecryptedSpaceKey {
    fn drop(&mut self) {
        // Zeroize key material before deallocation.
        // Uses volatile writes to prevent compiler optimization.
        self.key_bytes.zeroize();
    }
}
```

**Key lifetime policy:**
- Decrypted keys are loaded when the user authenticates and a space is accessed
- Keys are zeroized when the user locks the screen, logs out, or the space is unmounted
- Keys are stored on pinned kernel pages — never eligible for zram compression or swap
- The kernel page holding key material is mapped with `VmFlags::PINNED | VmFlags::NO_DUMP`

### 6.1.3 Cross-Zone Deduplication Boundaries

Content-addressed storage deduplicates identical blocks — but deduplication across security zones creates a side channel. An agent with access to the Untrusted zone could write known content and check whether the refcount is >1, leaking whether that content exists in an encrypted Personal zone.

**AIOS deduplication is scoped per security zone:**

```text
Dedup scope          Blocks compared against     Side channel risk
──────────           ───────────────────────     ────────────────
Core ↔ Core          Yes (same zone)             None (system data, not sensitive)
Personal ↔ Personal  Yes (same zone)             Low (all user's own data)
Untrusted ↔ Untrusted Yes (same zone)            Low (all web-origin data)
Core ↔ Personal      NO (cross-zone)             Blocked
Untrusted ↔ Personal NO (cross-zone)             Blocked
Collaborative ↔ any  Per-space only              Blocked across spaces
```

Each security zone maintains its own content-hash → block mapping in the LSM-tree index. An `Untrusted` block write checks dedup only against other `Untrusted` blocks. This means the same content stored in both `Personal` and `Untrusted` zones is stored twice — intentional, because blocks encrypted with different keys have different ciphertexts and SHA-256 hashes, so cross-zone dedup is impossible for encrypted zones anyway. For unencrypted zones (Core, Ephemeral), cross-zone dedup is still disabled to avoid the refcount side channel. Typical overhead: 5% for users with mostly-distinct content per zone; up to 20-30% for users who intentionally duplicate large media across zones (e.g., a 50 MB photo in both `user/media/` and `web-storage/[origin]/cache-api/`).

### 6.2 Encryption Zones

This table extends the §4.10 encryption zone table with key source information. §4.10 documents what is encrypted; this section documents where keys come from.

| Zone | Space encryption (§6.1) | Device encryption (§4.10) | Key Source |
|---|---|---|---|
| Core (system/) | No | Yes | Device key (hardware-bound or boot passphrase) |
| Personal (user/) | Yes | Yes | Space: user identity master key. Device: device key. |
| Collaborative (shared/) | Yes | Yes | Space: shared key (capability exchange). Device: device key. |
| Untrusted (web-storage/) | Yes | Yes | Space: per-origin key. Device: device key. |
| Ephemeral (/tmp) | No | Yes | Device key only |

All zones are encrypted at the device level. The "Encrypted" column in prior versions of this table referred only to per-space encryption. With device-level transparent encryption (§4.10), nothing is stored as plaintext on the physical medium. Per-space encryption provides additional cross-zone isolation within the running system.

### 6.3 Key Recovery: Prevention-Based Design

AIOS does not implement key escrow or key recovery. There is no seed phrase, no recovery key file, no mnemonic backup. If the user forgets their passphrase and the device is powered off, encrypted data is permanently irrecoverable. This follows the same model as full-disk encryption (LUKS without escrow, VeraCrypt, FileVault without iCloud recovery).

**Why no recovery mechanism:** Every recovery mechanism is an attack surface. A 24-word mnemonic can be stolen, photographed, or socially engineered. A recovery file can be exfiltrated. Key escrow requires either a trusted server (contradicts local-first) or offline material that creates the same custodial burden recovery is supposed to eliminate. For a single-device, local-first, offline-capable system, the added complexity and failure modes outweigh the benefit.

**Prevention-based approach (see [identity.md §14](../../experience/identity.md)):**

| Mechanism | Purpose |
|---|---|
| Aggressive session persistence | Master key sealed to TPM/Secure Enclave across sleep/wake. User re-enters passphrase only after cold reboot. Minimizes forgetting. |
| Passphrase change while authenticated | While the session is live, the user can change their passphrase at any time. The "recovery" happens before the user forgets, not after. |
| Clear warning at setup | "If you forget your passphrase and your device is powered off, your data cannot be recovered. This is by design." |
| Multi-device key backup (Phase 13c+) | When multi-device support lands, Device A can hold an encrypted shard of Device B's master key. No seed phrases, no paper — just a second AIOS device. |

**Security properties:**
- No recovery key → no recovery key attack surface (theft, social engineering, phishing)
- No recovery key material on-device → no offline extraction target beyond the passphrase-derived master key
- No external infrastructure dependency → works fully offline, single-device, from day one
- Multi-device key backup (Phase 13c+) adds recovery without custodial burden — leverages Space Sync infrastructure already being built

-----
