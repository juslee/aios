# Capability Kit

**Layer:** Kernel | **Crate:** `aios_capability` | **Architecture:** [`docs/security/model.md`](../../security/model.md), [`docs/security/model/capabilities.md`](../../security/model/capabilities.md)

## 1. Overview

Capability Kit is the security foundation of AIOS. Every resource access — reading a Space,
opening an IPC channel, using a camera, running inference — requires presenting a capability
token to the kernel. Without one, the operation is denied. There are no ambient permissions,
no root user, no privilege escalation paths. If an agent doesn't hold a capability for a
resource, it cannot access that resource, period.

Application developers interact with Capability Kit when they need to declare required
permissions in their agent manifest, check whether optional capabilities were granted,
attenuate tokens before delegating to child agents, or handle graceful degradation when
a capability is denied or revoked. Most day-to-day capability enforcement happens
transparently — when you call `storage.read()`, Storage Kit checks the capability internally.
But when you need fine-grained control (delegating a read-only subset to a helper agent,
revoking a temporary grant, or querying your own capability table), you use Capability Kit
directly.

Kit authors use Capability Kit to gate access to their Kit's resources. Every Kit that
manages a shared resource (Storage, Network, Camera, Audio, etc.) validates capabilities
on every operation. Capability Kit provides the enforcement API that makes this possible.

## 2. Core Traits

```rust
use aios_capability::{
    CapabilityToken, CapabilityHandle, CapabilityTable,
    AttenuationSpec, Capability, TokenId,
};

/// An unforgeable token representing permission to access a resource.
///
/// Tokens are created by the kernel during agent installation and placed
/// in the agent's capability table. The agent references them by handle
/// (an index), never by the token value itself — handles are forgery-proof
/// because the kernel validates them on every syscall.
pub struct CapabilityToken {
    /// Globally unique identifier.
    pub id: TokenId,
    /// What this token grants access to.
    pub capability: Capability,
    /// The agent that holds this token.
    pub holder: AgentId,
    /// Who approved this grant (user identity or system).
    pub granted_by: IdentityId,
    /// When the token was created.
    pub created_at: Timestamp,
    /// Mandatory expiry — the kernel rejects tokens without one.
    /// Maximum TTL varies by trust level:
    ///   System/Native: 365 days (renewed at boot)
    ///   Third-party:   90 days  (re-requested from Service Manager)
    ///   Web content:   24 hours (re-requested per session)
    pub expires: Timestamp,
    /// Whether this token can be delegated to other agents.
    pub delegatable: bool,
    /// Restrictions applied via attenuation (monotonically increasing).
    pub attenuations: Vec<Attenuation>,
    /// Revocation flag — checked on every use.
    pub revoked: bool,
    /// Parent token (if this was delegated or attenuated from another).
    pub parent_token: Option<TokenId>,
    /// Usage tracking for audit and rate limiting.
    pub usage_count: u64,
    pub last_used: Timestamp,
}

/// The set of resource types that capabilities can grant access to.
pub enum Capability {
    /// Read objects in a Space path.
    ReadSpace(SpaceId),
    /// Write objects in a Space path.
    WriteSpace(SpaceId),
    /// Create IPC channels.
    ChannelCreate,
    /// Access an existing IPC channel.
    ChannelAccess(ChannelId),
    /// Access camera hardware.
    CameraCapture { device: Option<CameraDeviceId> },
    /// Access audio input.
    AudioCapture,
    /// Access audio output.
    AudioPlayback,
    /// Access network resources.
    NetworkAccess { protocol: NetworkProtocol },
    /// Run inference on compute hardware.
    ComputeAccess { tier: ComputeTier },
    /// Access USB devices.
    UsbAccess { class: UsbDeviceClass },
    /// Post notifications to the user.
    NotificationPost { channel: ChannelId },
    /// Custom capability defined by a Kit or agent.
    Custom { namespace: String, name: String },
}

/// Per-agent capability table with O(1) handle lookup.
///
/// Each agent gets a table with up to 256 slots. The kernel validates
/// every handle on every syscall — there is no caching or bypassing.
pub struct CapabilityTable {
    /// Fixed-size array. Handle is the index.
    tokens: [Option<CapabilityToken>; 256],
    /// Next free slot for O(1) insertion.
    next_free: u32,
    /// Delegation records for cascade revocation.
    delegated: Vec<DelegationRecord>,
}

impl CapabilityTable {
    /// Look up a token by handle. Returns an error if the handle is
    /// invalid, the slot is empty, or the token has been revoked.
    /// Every failed lookup is audit-logged.
    pub fn get(&self, handle: CapabilityHandle) -> Result<&CapabilityToken, CapError>;

    /// Insert a new token into the next free slot.
    pub fn insert(&mut self, token: CapabilityToken) -> Result<CapabilityHandle, CapError>;

    /// Revoke a token and all its delegated children (cascade).
    /// Also invalidates any IPC channels created with this token.
    pub fn revoke(&mut self, token_id: TokenId);

    /// List all non-revoked, non-expired tokens in this table.
    pub fn list_active(&self) -> impl Iterator<Item = (CapabilityHandle, &CapabilityToken)>;
}

/// Specification for creating a more restricted version of a token.
///
/// Attenuation is one-way: permissions can only be narrowed, never expanded.
/// The kernel enforces monotonic reduction.
pub struct AttenuationSpec {
    /// Narrow a Space path (must be a sub-path of the original).
    pub narrow_path: Option<String>,
    /// Reduce the expiry (must be earlier than the original).
    pub reduce_expiry: Option<Timestamp>,
    /// Remove write permission (cannot add if original is read-only).
    pub remove_write: bool,
    /// Add a rate limit (cannot increase if original has one).
    pub add_rate_limit: Option<RateLimit>,
    /// Restrict to specific operations.
    pub restrict_operations: Option<Vec<OperationType>>,
}
```

## 3. Usage Patterns

### Checking capabilities at runtime

```rust
use aios_capability::{CapabilityHandle, CapError};
use aios_app::AgentContext;

fn export_notes(ctx: &AgentContext) -> Result<(), AppError> {
    // Check if we have the optional flow_publish capability
    match ctx.capability(Capability::FlowPublish) {
        Ok(handle) => {
            // We have sharing permission — export via Flow Kit
            let channel = aios_flow::open_channel("text/markdown", handle)?;
            channel.post(selected_notes)?;
            Ok(())
        }
        Err(CapError::NotGranted) => {
            // Graceful degradation: save to local Space instead
            aios_storage::write(&ctx.space("user/notes/exports/"), &selected_notes)?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}
```

### Attenuating before delegation

```rust
use aios_capability::AttenuationSpec;

fn delegate_to_spell_checker(ctx: &AgentContext, note_id: ObjectId) -> Result<(), AppError> {
    // We hold ReadSpace("user/notes/") — attenuate to a single note, read-only, 1 hour
    let restricted = ctx.attenuate(
        ctx.capability(Capability::ReadSpace(SpaceId::new("user/notes/")))?,
        AttenuationSpec {
            narrow_path: Some(format!("user/notes/{}", note_id)),
            reduce_expiry: Some(Timestamp::now() + Duration::hours(1)),
            remove_write: true,
            add_rate_limit: None,
            restrict_operations: None,
        },
    )?;

    // Delegate the attenuated token to the spell-checker agent
    ctx.delegate(restricted, spell_checker_agent)?;
    Ok(())
}
```

### Handling revocation

```rust
use aios_capability::RevocationEvent;
use aios_app::{Application, Event};

impl Application for MyApp {
    fn on_event(&mut self, event: Event) {
        match event {
            Event::CapabilityRevoked(RevocationEvent { handle, capability }) => {
                // A capability was revoked (user changed permissions, token expired,
                // or parent agent revoked delegation). Update UI accordingly.
                match capability {
                    Capability::CameraCapture { .. } => {
                        self.camera_preview.disable();
                        self.show_toast("Camera access was revoked");
                    }
                    Capability::ReadSpace(space) => {
                        self.invalidate_cached_data_for(&space);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}
```

## 4. Integration Examples

### With Storage Kit — capability-gated Space access

```rust
use aios_storage::{Space, Object};
use aios_capability::Capability;

fn read_user_document(ctx: &AgentContext, path: &str) -> Result<Object, AppError> {
    // Storage Kit checks ReadSpace capability internally.
    // If the agent doesn't hold it, this returns CapError::NotGranted.
    let space = ctx.space("user/documents/");
    let object = space.read(path)?;
    Ok(object)
}
```

### With IPC Kit — capability transfer over channels

```rust
use aios_ipc::Channel;
use aios_capability::Capability;

fn share_access_with_child(ctx: &AgentContext, child_channel: &Channel) -> Result<(), AppError> {
    // Attenuate our storage capability to read-only, 10-minute expiry
    let read_only = ctx.attenuate(
        ctx.capability(Capability::ReadSpace(SpaceId::new("user/photos/")))?,
        AttenuationSpec {
            remove_write: true,
            reduce_expiry: Some(Timestamp::now() + Duration::minutes(10)),
            ..Default::default()
        },
    )?;

    // Transfer the attenuated capability to the child agent over IPC
    child_channel.transfer_capability(read_only)?;
    Ok(())
}
```

### With Security Kit — Inspector capability view

```rust
use aios_security::Inspector;

// The Inspector agent uses Capability Kit to display all capabilities
// held by any agent, their delegation chains, and expiry status.
// Users can revoke any capability through the Inspector UI.
fn show_agent_capabilities(inspector: &Inspector, agent: AgentId) {
    let caps = inspector.list_capabilities(agent);
    for (handle, token) in caps {
        inspector.display_capability(handle, token);
        // Show delegation tree if this token was delegated
        if let Some(parent) = token.parent_token {
            inspector.display_delegation_chain(parent);
        }
    }
}
```

## 5. Capability Requirements

| Capability | What It Gates | Default Grant |
|---|---|---|
| `ChannelCreate` | Creating new IPC channels | Granted to all agents |
| `ChannelAccess(id)` | Sending/receiving on a specific channel | Per-channel, on creation |
| `ReadSpace(space)` | Reading objects in a Space | Prompt user on first access |
| `WriteSpace(space)` | Writing objects in a Space | Prompt user on first access |
| `CameraCapture` | Accessing camera hardware | Prompt user, LED enforced |
| `AudioCapture` | Accessing microphone | Prompt user, indicator enforced |
| `NetworkAccess` | Making network requests | Prompt user per protocol |
| `ComputeAccess` | Using GPU/NPU for compute | Granted based on trust level |

### Agent manifest example

```toml
[agent]
name = "com.example.secure-notes"
version = "1.0.0"

[capabilities.required]
# Agent cannot function without these — installation fails if user declines
storage_read = { spaces = ["user/notes/"] }
storage_write = { spaces = ["user/notes/"] }

[capabilities.optional]
# Agent works without these but with reduced functionality
flow_clipboard = true     # Copy/paste support
network_https = true      # Cloud sync
camera_capture = true     # Photo attachment
```

## 6. Error Handling

```rust
/// Errors returned by Capability Kit operations.
pub enum CapError {
    /// The agent does not hold the required capability.
    /// Recovery: check optional capabilities, degrade gracefully.
    NotGranted,

    /// The capability token has been revoked (by user, expiry, or cascade).
    /// Recovery: re-request from Service Manager if appropriate.
    Revoked { token_id: TokenId },

    /// The capability token has expired.
    /// Recovery: request renewal from Service Manager.
    Expired { token_id: TokenId, expired_at: Timestamp },

    /// The capability table is full (256 slots).
    /// Recovery: release unused capabilities, then retry.
    TableFull,

    /// The attenuation spec is invalid (e.g., trying to widen permissions).
    /// Recovery: fix the spec — permissions can only narrow, never widen.
    InvalidAttenuation { reason: &'static str },

    /// The handle is out of range or points to an empty slot.
    /// Recovery: use list_active() to find valid handles.
    InvalidHandle { handle: CapabilityHandle },

    /// The token is not delegatable.
    /// Recovery: the original granter must create a delegatable token.
    NotDelegatable { token_id: TokenId },
}
```

## 7. Platform & AI Availability

Capability Kit is a kernel primitive — it runs on all AIOS platforms with identical
behavior. There is no degradation path: capabilities are either enforced or the system
is not AIOS.

When AIRS is online, it enhances the capability system with:

- **Smart grant suggestions**: AIRS analyzes agent behavior patterns to recommend
  which optional capabilities to grant, and warns when a capability request seems
  inconsistent with the agent's declared purpose (Intent Verifier integration).
- **Anomaly detection**: the Behavioral Monitor flags agents that suddenly use
  capabilities they have held but never exercised — a potential indicator of
  compromise.
- **Capability intelligence**: AIRS suggests composable capability profiles for
  common patterns (e.g., "photo editor" = camera + storage + compute) to reduce
  user prompt fatigue.

Without AIRS, all capabilities work identically — the user simply sees more manual
prompts and doesn't get intelligent grant suggestions.

## For Kit Authors

This section is for developers building new Kits or subsystems that need to gate
access to their resources.

### Defining a new capability type

```rust
use aios_capability::{Capability, CapabilityHandle, CapError};

/// Register your capability type with the kernel's capability registry.
/// This is done once during Kit initialization.
pub fn register_capability_type() {
    aios_capability::register_custom(
        "com.aios.mykit",           // namespace
        "MyResource",               // capability name
        CapabilityPolicy {
            default_grant: GrantPolicy::PromptUser,
            max_ttl_by_trust: [
                (TrustLevel::System, Duration::days(365)),
                (TrustLevel::Native, Duration::days(365)),
                (TrustLevel::ThirdParty, Duration::days(90)),
                (TrustLevel::WebContent, Duration::hours(24)),
            ],
            delegatable_default: true,
            audit_all_uses: true,
        },
    );
}
```

### Enforcing capabilities in your Kit

```rust
/// Every public API in your Kit should validate capabilities.
/// The pattern: accept a CapabilityHandle, validate it, then proceed.
pub fn my_kit_operation(
    handle: CapabilityHandle,
    args: &OperationArgs,
) -> Result<OperationResult, MyKitError> {
    // 1. Validate the capability (checks revocation, expiry, permissions)
    let token = aios_capability::validate(handle)?;

    // 2. Check that the capability grants the specific permission needed
    match &token.capability {
        Capability::Custom { namespace, name }
            if namespace == "com.aios.mykit" && name == "MyResource" => {}
        _ => return Err(MyKitError::WrongCapability),
    }

    // 3. Check attenuations (rate limits, path restrictions, etc.)
    aios_capability::check_attenuations(&token, args)?;

    // 4. Record usage for audit
    aios_capability::record_usage(handle);

    // 5. Perform the operation
    do_the_actual_work(args)
}
```

### Cascade revocation integration

```rust
/// When your Kit caches state based on capabilities (e.g., open sessions,
/// cached file handles), register a revocation callback so the kernel can
/// clean up when a capability is revoked.
pub fn register_revocation_handler() {
    aios_capability::on_revoke("com.aios.mykit", |token_id| {
        // Clean up any sessions or cached state associated with this token
        MY_KIT_SESSION_CACHE.invalidate_for_token(token_id);
    });
}
```

## Cross-References

- [Security Model — Capability System Internals](../../security/model/capabilities.md) — kernel implementation details
- [IPC Kit](./ipc.md) — capability transfer over channels
- [App Kit](../application/app.md) — agent manifest capability declarations
- [Security Kit](../application/security.md) — Inspector capability view
- [Intent Kit](../intelligence/intent.md) — capability verification against declared intent
