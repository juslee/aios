# AIOS Credential Isolation & Service Identities

Part of: [identity.md](../identity.md) — Identity & Relationships
**Related:** [core.md](./core.md) — Kernel Crypto Core and key hierarchy, [agents.md](./agents.md) — Agent delegation chains, [privacy.md](./privacy.md) — Privacy controls and selective disclosure

**Cross-references:** [networking/security](../../platform/networking/security.md) — Credential vault integration, [adversarial-defense](../../security/adversarial-defense.md) — Credential theft prevention

-----

## 11. Credential Isolation

### 11.1 The Problem

Traditional systems give agents (applications) direct access to credentials. A browser stores cookies. An email client stores IMAP passwords. A GitHub CLI stores OAuth tokens. Any application with read access to `~/.config/` can steal every credential on the system.

### 11.2 AIOS Solution

Agents never possess credentials. The Identity Service holds all credentials. Agents request credential **use** — the Identity Service applies the credential on their behalf:

```rust
impl IdentityService {
    /// Agent requests to use a credential
    pub fn use_credential(
        &self,
        agent: &AgentId,
        service: &ServiceId,
        request: CredentialUseRequest,
    ) -> Result<CredentialUseResponse, Error> {
        // 1. Verify agent has capability to use this service's credentials
        if !self.capability_manager.check_credential_use(agent, service) {
            return Err(Error::CapabilityDenied);
        }

        // 2. Retrieve credential from vault
        let credential = self.vault.get(service)?;

        // 3. Apply credential to the request (agent never sees the credential)
        let response = match &request {
            CredentialUseRequest::HttpRequest { method, url, headers } => {
                // Identity Service performs the HTTP request internally,
                // injecting the credential. Agent receives only the response.
                let authed_response = self.http_client.execute_with_credential(
                    &credential, method, url, headers,
                )?;
                CredentialUseResponse::HttpResponse {
                    status: authed_response.status,
                    headers: authed_response.headers,
                    body: authed_response.body,
                }
            }
            CredentialUseRequest::Sign { data } => {
                CredentialUseResponse::Signed {
                    signature: credential.sign(data),
                }
            }
        };

        // 4. Audit log
        self.audit_log.record(CredentialUseAudit {
            agent: *agent,
            service: service.clone(),
            request_type: request.type_name(),
            timestamp: SystemTime::now(),
        });

        Ok(response)
    }
}
```

The agent receives the effect of the credential (an HTTP header value, a signed payload) but never the credential itself. Even if the agent is compromised, the attacker cannot extract credentials.

### 11.3 Credential Storage

```rust
pub struct CredentialVault {
    /// Credentials encrypted at rest with identity key
    credentials: HashMap<ServiceId, EncryptedCredential>,
    /// Decryption requires kernel Crypto Core
    encryption_key_id: KeyId,
}

pub struct EncryptedCredential {
    pub service: ServiceId,
    pub credential_type: CredentialType,
    pub encrypted_data: Vec<u8>,    // AES-256-GCM encrypted
    pub nonce: [u8; 12],
    pub added: Timestamp,
    pub last_used: Timestamp,
    pub rotation_policy: Option<RotationPolicy>,
}

pub enum CredentialType {
    OAuthToken { provider: String, scopes: Vec<String> },
    ApiKey { service: String },
    Password { service: String, username: String },
    Certificate { subject: String },
    Passkey { relying_party: String, credential_id: Vec<u8> },
}
```

### 11.4 Credential Rotation

Credentials have configurable rotation policies that the Identity Service enforces automatically:

```rust
pub struct RotationPolicy {
    /// Maximum age before rotation is required
    pub max_age: Duration,
    /// Warn the user this many days before expiry
    pub warning_period: Duration,
    /// Auto-rotate if the service supports it (OAuth refresh)
    pub auto_rotate: bool,
    /// Rotation strategy
    pub strategy: RotationStrategy,
}

pub enum RotationStrategy {
    /// Use OAuth refresh token to obtain new access token
    OAuthRefresh,
    /// Prompt the user to re-authenticate
    UserReauth,
    /// Generate new API key via service API (if supported)
    ApiRotation { endpoint: String },
    /// No automatic rotation — warn only
    ManualOnly,
}
```

For OAuth tokens with refresh tokens, the Identity Service automatically refreshes access tokens before expiry. The agent never knows the token changed — it continues to request credential use, and the Identity Service transparently provides the fresh token.

-----

## 12. AIOS as Platform Authenticator

AIOS's Kernel Crypto Core acts as a FIDO2/WebAuthn platform authenticator. Users log into websites and services using their AIOS identity key as a passkey — no passwords, no external hardware tokens required.

### 12.1 CTAP2 Authenticator

The Kernel Crypto Core implements the CTAP2 (Client to Authenticator Protocol) internal authenticator interface:

```rust
pub struct PlatformAuthenticator {
    /// Authenticator AAGUID (identifies AIOS as the authenticator type)
    pub aaguid: [u8; 16],
    /// Supported algorithms (preference order)
    pub algorithms: Vec<CoseAlgorithm>,
    /// User presence verification method
    pub user_verification: UserVerificationMethod,
}

pub enum CoseAlgorithm {
    /// Ed25519 (preferred — native to AIOS identity)
    EdDSA,
    /// ECDSA with P-256 (required for WebAuthn compatibility)
    ES256,
    /// Post-quantum hybrid (future — see core.md §4)
    MlDsa44Hybrid,
}

pub enum UserVerificationMethod {
    /// Passphrase entry
    Passphrase,
    /// Session confidence above threshold (§17.1.2, continuous auth)
    SessionConfidence { min_confidence: f32 },
    /// Hardware key touch (optional FIDO2 backup)
    HardwareKey,
}
```

### 12.2 WebAuthn Registration Flow

When a user registers with a website using AIOS as authenticator:

```text
Registration:
1. Relying party (website) sends challenge + relying_party_id
2. Browser agent forwards to Identity Service via IPC
3. Identity Service verifies user presence (passphrase or session confidence)
4. Kernel Crypto Core generates credential key pair:
   - Derivation path: m/0x41494F53'/passkeys'/rp_id_hash'/credential_index'
   - Algorithm: Ed25519 (preferred) or P-256 (if RP requires)
5. Credential stored in CredentialVault as CredentialType::Passkey
6. Public key + attestation returned to relying party
7. Audit log records: registration event, RP identity, timestamp
```

The credential private key never leaves the Kernel Crypto Core. The browser agent (or any other agent) receives only the public key and attestation object.

### 12.3 WebAuthn Authentication Flow

```text
Authentication:
1. Relying party sends challenge + credential_id + relying_party_id
2. Browser agent forwards to Identity Service
3. Identity Service looks up credential by (rp_id, credential_id)
4. User presence verified (per-session cache: require once, cache for session)
5. Kernel Crypto Core signs challenge with credential private key
6. Signed assertion returned to browser agent → relying party
7. Audit log records: authentication event, RP identity, timestamp
```

### 12.4 Hardware Key as Optional Backup

Hardware FIDO2 keys (YubiKey, etc.) are supported as **optional backup authenticators**, not the primary path:

```rust
pub enum AuthenticatorSource {
    /// AIOS Kernel Crypto Core (default, always available)
    Platform,
    /// External FIDO2 hardware key (optional backup)
    /// User concern: supply chain risks, side-channel attacks
    HardwareKey { transport: HidTransport },
}

pub enum HidTransport {
    Usb,
    Nfc,
    Ble,
}
```

**Design rationale:** Hardware keys are a separate physical device that can be lost, stolen, or compromised via supply chain attacks. AIOS's Kernel Crypto Core provides equivalent security guarantees — private keys never leave kernel address space — without requiring an external device. Hardware keys are available for users who want a second factor or physical backup, but they are not required or recommended as the primary authenticator.

### 12.5 Algorithm Negotiation

When a relying party requests authentication, the platform authenticator negotiates the best algorithm:

```text
Preference order (COSE algorithm identifiers):
1. EdDSA (Ed25519)         — COSE alg: -8, native to AIOS, fastest, smallest signatures
2. ES256 (ECDSA P-256)     — COSE alg: -7, required by many RPs, WebAuthn mandatory-to-implement
3. RS256 (RSASSA-PKCS1-v1_5 RSA-2048) — COSE alg: -257, legacy fallback only
4. ML-DSA-44 hybrid        — COSE alg: TBD, post-quantum (future, requires RP support)
```

Most relying parties support ES256 at minimum. AIOS prefers Ed25519 when the RP supports it (e.g., via the `public-key` credential type with EdDSA). For RPs that only support P-256, AIOS generates and manages P-256 keys through the same Crypto Core infrastructure.

-----

### 12.6 Service Identities

External services are modeled as identities with their own relationship entries:

```rust
pub struct ServiceIdentity {
    /// Service identifier
    pub service_id: ServiceId,
    /// Service name (e.g., "GitHub", "Slack")
    pub name: String,
    /// Service's public key (if applicable, for verification)
    pub service_key: Option<Ed25519PublicKey>,
    /// Service's DID (if the service supports did:web or did:peer)
    pub service_did: Option<String>,
    /// Connector agent that interfaces with this service
    pub connector_agent: AgentId,
    /// Credentials for this service
    pub credentials: Vec<CredentialType>,
    /// What data this service has access to
    pub data_sharing: DataSharingConfig,
    /// Trust level for this service
    pub trust_level: TrustLevel,
}

pub struct DataSharingConfig {
    /// What spaces this service can access
    pub space_access: Vec<(SpaceId, AccessLevel)>,
    /// What identity information is shared with this service
    pub identity_disclosure: IdentityDisclosure,
}

pub enum IdentityDisclosure {
    /// Full identity shared (display name, avatar)
    Full,
    /// Minimal (pseudonymous handle)
    Minimal,
    /// Anonymous (no identity information)
    Anonymous,
}
```

When a connector agent (e.g., slack-connector) accesses Slack's API, it goes through the Identity Service for credential use. The connector agent never directly holds the OAuth token.

### 12.7 OAuth Flow for Connector Agents

Connector agents that need to authenticate with external services follow a mediated OAuth flow:

```text
OAuth Authorization Code flow (mediated):
1. Connector agent requests OAuth authorization for service
2. Identity Service generates authorization URL with PKCE
3. Browser agent opens authorization URL (user sees consent screen)
4. User authorizes → callback URL receives authorization code
5. Identity Service exchanges code for tokens (agent never sees code)
6. Tokens stored in CredentialVault, encrypted at rest
7. Connector agent uses tokens via use_credential() — never directly

Token refresh:
1. Identity Service detects token near expiry (rotation_policy)
2. Uses refresh token to obtain new access token
3. Updates CredentialVault — connector agent is unaware
4. Next use_credential() call returns fresh token transparently
```

**Security properties:** The connector agent never sees the OAuth client secret, authorization code, access token, or refresh token. All it can do is request the Identity Service to apply credentials on its behalf. If the connector agent is compromised, the attacker gains the ability to make authenticated requests (which are audited and capability-bounded) but cannot exfiltrate the tokens themselves.
