# Identity Kit

**Layer:** Application | **Crate:** `aios_identity` | **Architecture:** [`docs/experience/identity.md`](../../experience/identity.md)

## 1. Overview

Identity Kit provides user identity, authentication, device pairing, and credential
management for AIOS applications. It is the foundation on which personalization, capability
delegation, and secure cross-device continuity are built. Every capability grant in AIOS
traces back to an identity: a user identity verified through biometrics or passkeys, a
device identity established through hardware attestation, or a service identity bound to
an agent's signed manifest.

The Kit is built around decentralized identifiers (DIDs). Each user has a self-sovereign
`did:aios` identity anchored to a post-quantum cryptographic (PQC) key hierarchy stored in
hardware-backed secure storage. Device pairing uses SPAKE2+ password-authenticated key
exchange, producing mutual trust without a central authority. Trust relationships between
identities are expressed as `TrustRelation` objects that carry delegation chains, enabling
one identity to grant specific capabilities to another with cryptographic verifiability.

Identity Kit also manages credentials -- WebAuthn passkeys, OAuth tokens, service
passwords -- in an isolated credential store. Each credential is encrypted at rest, scoped
to the identity and origin that created it, and accessible only through explicit capability
grants. Applications never see raw credential material; they call `authenticate()` and
Identity Kit handles the protocol negotiation, biometric verification, and secure token
injection.

## 2. Core Traits

```rust
use aios_capability::{Capability, CapabilityHandle};
use aios_storage::Space;

/// Represents a user, device, or service identity.
///
/// Identities are DID-based and backed by a PQC key hierarchy.
/// The identity is the root of all capability delegation in AIOS.
pub trait IdentityProvider {
    /// Return the DID for this identity (e.g., `did:aios:user:abc123`).
    fn did(&self) -> &Did;

    /// Return the identity's display name.
    fn display_name(&self) -> &str;

    /// Return the identity type (User, Device, Service, or Agent).
    fn identity_type(&self) -> IdentityType;

    /// Verify that this identity is authentic using its key chain.
    fn verify(&self) -> Result<VerificationResult, IdentityError>;

    /// Return the public keys associated with this identity.
    fn public_keys(&self) -> &[PublicKey];

    /// Return all trust relationships this identity participates in.
    fn trust_relations(&self) -> Result<Vec<TrustRelation>, IdentityError>;

    /// Export a portable identity representation for cross-device sync.
    fn export_portable(&self) -> Result<PortableIdentity, IdentityError>;
}

/// A stored credential (passkey, OAuth token, password) tied to an identity.
///
/// Credentials are encrypted at rest and never exposed as raw material
/// to application code. Use `authenticate()` to trigger protocol-specific
/// flows that inject credentials securely.
pub trait Credential {
    /// The credential's unique identifier.
    fn id(&self) -> CredentialId;

    /// The origin or service this credential is bound to.
    fn origin(&self) -> &Origin;

    /// The credential type (WebAuthn, OAuth, Password, Certificate).
    fn credential_type(&self) -> CredentialType;

    /// When this credential was last used.
    fn last_used(&self) -> Option<Timestamp>;

    /// Whether the credential has expired.
    fn is_expired(&self) -> bool;

    /// Delete this credential from the store.
    fn delete(self) -> Result<(), IdentityError>;
}

/// A trust relationship between two identities with delegation semantics.
///
/// Trust relations carry capability delegation chains: identity A trusts
/// identity B to exercise specific capabilities on A's behalf. The chain
/// is cryptographically signed at each link.
pub trait TrustRelation {
    /// The identity that grants trust.
    fn grantor(&self) -> &Did;

    /// The identity that receives trust.
    fn grantee(&self) -> &Did;

    /// The capabilities delegated through this relation.
    fn delegated_capabilities(&self) -> &[Capability];

    /// The trust level (Explicit, Verified, TOFU, Organizational).
    fn trust_level(&self) -> TrustLevel;

    /// Verify the cryptographic chain of this trust relation.
    fn verify_chain(&self) -> Result<ChainVerification, IdentityError>;

    /// Revoke this trust relation, cascading revocation to derived grants.
    fn revoke(self) -> Result<(), IdentityError>;
}

/// SPAKE2+ device pairing for establishing mutual trust between devices.
///
/// Pairing binds two devices to the same user identity without a central
/// server. The pairing PIN is entered on both devices simultaneously.
pub trait DevicePairing {
    /// Begin a pairing session on this device (displays a pairing code).
    fn initiate(&mut self) -> Result<PairingSession, IdentityError>;

    /// Join a pairing session using a code from the initiating device.
    fn join(&mut self, code: &PairingCode) -> Result<PairingSession, IdentityError>;

    /// Complete the pairing, establishing a TrustRelation between devices.
    fn complete(&mut self, session: PairingSession) -> Result<TrustRelation, IdentityError>;

    /// List all paired devices for the current user identity.
    fn paired_devices(&self) -> Result<Vec<PairedDevice>, IdentityError>;

    /// Revoke a device pairing, removing its trust relation and synced keys.
    fn revoke_device(&mut self, device: &DeviceId) -> Result<(), IdentityError>;
}

/// WebAuthn platform authenticator for passkey-based authentication.
///
/// Identity Kit acts as an AIOS-native FIDO2 authenticator. Passkeys
/// are stored in hardware-backed secure storage and bound to the user's
/// PQC key hierarchy.
pub trait WebAuthnAuthenticator {
    /// Register a new passkey for an origin.
    fn register(
        &mut self,
        origin: &Origin,
        options: RegistrationOptions,
    ) -> Result<RegistrationResult, IdentityError>;

    /// Authenticate with a stored passkey for an origin.
    fn authenticate(
        &mut self,
        origin: &Origin,
        challenge: &Challenge,
    ) -> Result<AuthenticationResult, IdentityError>;

    /// List registered passkeys, optionally filtered by origin.
    fn list_passkeys(&self, origin: Option<&Origin>) -> Result<Vec<PasskeyInfo>, IdentityError>;

    /// Delete a passkey.
    fn delete_passkey(&mut self, id: &PasskeyId) -> Result<(), IdentityError>;
}
```

## 3. Usage Patterns

**Minimal -- check the current user identity:**

```rust
use aios_identity::IdentityKit;

let identity = IdentityKit::current_user()?;
println!("Logged in as: {} ({})", identity.display_name(), identity.did());

let verification = identity.verify()?;
assert!(verification.is_valid());
```

**Realistic -- authenticate with a web service using passkeys:**

```rust
use aios_identity::{IdentityKit, WebAuthnAuthenticator};

let origin = Origin::new("https://example.com");

// Check if we have a passkey for this origin
let passkeys = IdentityKit::authenticator().list_passkeys(Some(&origin))?;

if passkeys.is_empty() {
    // Register a new passkey (triggers biometric prompt)
    let result = IdentityKit::authenticator().register(
        &origin,
        RegistrationOptions {
            user_name: "alice@example.com",
            user_display_name: "Alice",
            ..Default::default()
        },
    )?;
    // Send result.attestation to the server for registration
} else {
    // Authenticate with existing passkey (triggers biometric prompt)
    let challenge = fetch_challenge_from_server(&origin)?;
    let result = IdentityKit::authenticator().authenticate(&origin, &challenge)?;
    // Send result.assertion to the server for verification
}
```

**Advanced -- pair a new device and delegate capabilities:**

```rust
use aios_identity::{IdentityKit, DevicePairing};

// On the existing device: initiate pairing
let mut pairing = IdentityKit::device_pairing();
let session = pairing.initiate()?;
println!("Enter this code on your new device: {}", session.display_code());

// On the new device: join with the code
let mut pairing = IdentityKit::device_pairing();
let session = pairing.join(&PairingCode::from_str("123-456")?)?;

// Complete pairing on both devices (SPAKE2+ exchange)
let trust = pairing.complete(session)?;

// The trust relation automatically delegates Space sync capabilities,
// enabling the new device to pull the user's encrypted Spaces.
assert_eq!(trust.trust_level(), TrustLevel::Explicit);
```

> **Common Mistakes**
>
> - **Accessing raw credential material.** Identity Kit never exposes passwords or tokens
>   to application code. Call `authenticate()` and let the Kit inject credentials into
>   the appropriate protocol channel (TLS, HTTP header, etc.).
> - **Caching identity verification results.** Verification is time-sensitive. Always call
>   `identity.verify()` before security-critical operations rather than caching a previous
>   `VerificationResult`.
> - **Ignoring trust level.** A `TOFU` (trust-on-first-use) relation is weaker than an
>   `Explicit` one. Gate sensitive operations on the appropriate `TrustLevel`.
> - **Pairing over untrusted networks.** SPAKE2+ is resistant to eavesdropping, but the
>   pairing code must be exchanged out-of-band. Do not transmit it over the network.

## 4. Integration Examples

**Identity Kit + Capability Kit -- identity-gated capability grants:**

```rust
use aios_identity::IdentityKit;
use aios_capability::CapabilityKit;

// Verify the caller's identity before granting a sensitive capability
let identity = IdentityKit::current_user()?;
let verification = identity.verify()?;

if verification.is_valid() && verification.trust_level() >= TrustLevel::Verified {
    // Grant the capability with the identity as the delegation root
    let handle = CapabilityKit::grant(
        Capability::StorageWrite { space: "finances".into() },
        GrantOptions {
            delegated_by: identity.did().clone(),
            expires: Duration::from_secs(60 * 60),
            ..Default::default()
        },
    )?;
}
```

**Identity Kit + Storage Kit -- encrypted credential storage:**

```rust
use aios_identity::IdentityKit;
use aios_storage::StorageKit;

// Credentials are stored in a dedicated encrypted Space
// managed by Identity Kit. Applications access them indirectly.
let credentials = IdentityKit::credential_store();

// Store an OAuth token (encrypted at rest, scoped to origin)
credentials.store(CredentialBuilder::oauth()
    .origin("https://api.example.com")
    .access_token(&token)
    .refresh_token(&refresh)
    .expires_at(expiry)
    .build()?
)?;

// Retrieve credentials for an origin (requires CredentialAccess capability)
let creds = credentials.for_origin(&Origin::new("https://api.example.com"))?;
// creds is a handle, not raw material -- use it via authenticate()
```

**Identity Kit + Network Kit -- cross-device Space Mesh sync:**

```rust
use aios_identity::{IdentityKit, DevicePairing};
use aios_network::NetworkKit;

// After pairing, devices can sync Spaces through the Space Mesh.
// Identity Kit provides the authentication layer; Network Kit
// handles the transport.

let paired = IdentityKit::device_pairing().paired_devices()?;
for device in &paired {
    let peer = NetworkKit::connect_peer(device.network_address())?;

    // Authenticate the peer using the trust relation's key material
    let authenticated = peer.authenticate_with(device.trust_relation())?;

    // Space sync proceeds over the authenticated channel
    authenticated.sync_spaces(SyncPolicy::Incremental)?;
}
```

## 5. Capability Requirements

| Method | Required Capability | Notes |
| --- | --- | --- |
| `IdentityProvider::did` | `IdentityRead` | Reading own identity is always granted |
| `IdentityProvider::verify` | `IdentityRead` | Verification is read-only |
| `IdentityProvider::trust_relations` | `IdentityRead` | Lists relations for own identity |
| `TrustRelation::revoke` | `IdentityWrite` | Destructive; requires owner identity |
| `DevicePairing::initiate` | `IdentityWrite` + `NetworkAccess` | Creates pairing session |
| `DevicePairing::revoke_device` | `IdentityWrite` | Cascades to capability revocation |
| `WebAuthnAuthenticator::register` | `CredentialWrite` | Creates new passkey |
| `WebAuthnAuthenticator::authenticate` | `CredentialAccess` | Triggers biometric prompt |
| `Credential::delete` | `CredentialWrite` | Permanent deletion |
| `CredentialStore::for_origin` | `CredentialAccess` | Per-origin scoping enforced |

## 6. Error Handling & Degradation

```rust
/// Errors returned by Identity Kit operations.
#[derive(Debug)]
pub enum IdentityError {
    /// Biometric verification failed (wrong finger, face not recognized).
    BiometricFailed,

    /// The identity's key chain is invalid or has been tampered with.
    KeyChainInvalid { reason: String },

    /// The trust relation could not be verified (expired, revoked, or forged).
    TrustVerificationFailed(TrustFailureReason),

    /// Device pairing failed (wrong code, timeout, or protocol error).
    PairingFailed(PairingFailureReason),

    /// The credential was not found for the given origin.
    CredentialNotFound(Origin),

    /// The credential has expired and cannot be refreshed.
    CredentialExpired(CredentialId),

    /// The required capability was not granted.
    CapabilityDenied(Capability),

    /// The hardware security module is unavailable.
    HsmUnavailable,

    /// The PQC key derivation failed.
    KeyDerivationFailed(String),

    /// Cross-device sync failed for the identity store.
    SyncFailed(SyncError),

    /// Storage error in the credential store.
    StorageFailed(StorageError),
}
```

**Fallback cascade:**

| Failure | Degradation |
| --- | --- |
| HSM unavailable | Software-backed key storage with warning to user |
| Biometric sensor absent | Falls back to PIN/passphrase authentication |
| PQC algorithms unavailable | Classical key hierarchy (Ed25519/X25519) with upgrade path |
| Paired device unreachable | Sync deferred; local changes queued for next connection |
| WebAuthn registration fails | Offer password-based credential as fallback |
| Trust chain verification fails | Operation denied; user sees clear explanation |
| Credential store corrupted | Recovery from backup Space or re-authentication required |

## 7. Platform & AI Availability

**AIRS-enhanced features (require AIRS Kit):**

| Feature | What AIRS provides | Without AIRS |
| --- | --- | --- |
| Anomalous login detection | Behavioral monitor flags unusual authentication patterns | No anomaly detection |
| Smart credential selection | Context-aware choice of which credential to use for an origin | User manually selects |
| Trust scoring | EigenTrust-based reputation scores for trust relations | Binary trust (trusted/not) |
| Phishing resistance | ML analysis of authentication prompts for spoofing | Static origin matching only |
| Recovery recommendation | Guides user through identity recovery based on available factors | Static recovery wizard |
| Device risk assessment | Evaluates paired device health before sync | No device health checks |

**Platform availability:**

| Platform | HSM Support | Biometrics | PQC Keys | Notes |
| --- | --- | --- | --- | --- |
| QEMU virt | Software only | None | Software | Testing only |
| Raspberry Pi 4 | External TPM | None | Software | No built-in secure element |
| Raspberry Pi 5 | External TPM | None | Software | RP2350 secure boot chain |
| Apple Silicon | Secure Enclave | Face ID / Touch ID | Hardware-backed | Full hardware security |

**Implementation phase:** Phase 3+ (basic identity and credential store). Full identity with
device pairing and WebAuthn in Phase 15+. PQC key migration in Phase 35+.

---

*See also: [Capability Kit](../kernel/capability.md) | [Storage Kit](../platform/storage.md) | [Network Kit](../platform/network.md) | [Security Kit](security.md)*
