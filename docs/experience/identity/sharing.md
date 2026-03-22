# AIOS Space Sharing

Part of: [identity.md](../identity.md) — Identity & Relationships
**Related:** [relationships.md](./relationships.md) — Trust model & relationship graph, [cross-device.md](./cross-device.md) — Space Mesh sync across devices

**Cross-references:** [spaces.md](../../storage/spaces.md) — Space storage architecture, [model/capabilities.md](../../security/model/capabilities.md) — Kernel capability system

-----

## 7. Space Sharing

### 7.1 Shared Space Configuration

Spaces are shared with specific identities at specific access levels:

```rust
pub struct SharedSpaceConfig {
    /// The space being shared
    pub space_id: SpaceId,
    /// The identity it's shared with
    pub shared_with: IdentityId,
    /// Access level
    pub access: AccessLevel,
    /// Capability token (cryptographically bound to identity)
    pub capability_token: SpaceCapabilityToken,
    /// When sharing was granted
    pub granted: SystemTime,
    /// Optional expiry
    pub expires: Option<SystemTime>,
}

pub enum AccessLevel {
    /// No access
    None,
    /// Can read objects in the space
    ReadOnly,
    /// Can read and write objects
    ReadWrite,
    /// Can read, write, and manage sharing for this space
    Admin,
}
```

### 7.2 Sharing Flow

```rust
impl IdentityService {
    pub fn share_space(
        &mut self,
        space_id: &SpaceId,
        with: &IdentityId,
        access: AccessLevel,
    ) -> Result<SharedSpaceConfig, Error> {
        // 1. Verify we have admin access to this space
        if !self.has_admin_access(space_id) {
            return Err(Error::InsufficientAccess);
        }

        // 2. Verify the target identity exists in our relationships
        let relationship = self.get_relationship(with)
            .ok_or(Error::UnknownIdentity)?;

        // 3. Create capability token bound to their identity
        let token = self.capability_manager.create_space_token(
            space_id,
            with,
            access,
        );

        // 4. Create sharing config
        let config = SharedSpaceConfig {
            space_id: space_id.clone(),
            shared_with: *with,
            access,
            capability_token: token,
            granted: SystemTime::now(),
            expires: None,
        };

        // 5. Store sharing config
        space::write(
            &format!("system/identity/sharing/{}/{}", space_id, with.short()),
            &config,
        );

        // 6. If peer is online, notify them via AIOS Peer Protocol
        if let Some(peer) = self.network.find_peer(with) {
            peer.send(PeerMessage::SpaceShared {
                space_id: space_id.clone(),
                access,
                token: token.clone(),
            });
        }

        Ok(config)
    }

    pub fn revoke_space_sharing(
        &mut self,
        space_id: &SpaceId,
        from: &IdentityId,
    ) -> Result<(), Error> {
        // 1. Revoke the capability token
        self.capability_manager.revoke_space_token(space_id, from);

        // 2. Remove sharing config
        space::delete(
            &format!("system/identity/sharing/{}/{}", space_id, from.short()),
        );

        // 3. Notify peer if online
        if let Some(peer) = self.network.find_peer(from) {
            peer.send(PeerMessage::SpaceRevoked {
                space_id: space_id.clone(),
            });
        }

        Ok(())
    }
}
```

### 7.3 Capability Binding

Capability tokens are cryptographically bound to identity:

```rust
/// Space-sharing capability token. Distinct from the kernel-level
/// CapabilityToken (model.md §3) — this is a higher-level
/// identity-bound access grant for cross-space sharing.
pub struct SpaceCapabilityToken {
    /// What this token grants access to
    pub space_id: SpaceId,
    /// Who this token is for (identity-bound, non-transferable)
    pub bound_to: IdentityId,
    /// What access level
    pub access: AccessLevel,
    /// When this token was issued
    pub issued: SystemTime,
    /// Signed by the space owner's identity key
    pub owner_signature: Signature,
    /// Token ID for revocation
    pub token_id: TokenId,
}

impl SpaceCapabilityToken {
    pub fn verify(&self, owner_public_key: &Ed25519PublicKey) -> bool {
        let data = self.signable_bytes();
        crypto_core::verify(owner_public_key, &data, &self.owner_signature)
    }
}
```

A capability token cannot be transferred to another identity. If Alice shares a space with Bob, Bob cannot re-share that token with Carol. Bob would need Alice's permission (Admin access) to share further.
