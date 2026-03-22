# AIOS Agent Identity

Part of: [identity.md](../identity.md) — Identity & Relationships
**Related:** [core.md](./core.md) — Key management and Crypto Core, [credentials.md](./credentials.md) — Credential isolation & service identities, [privacy.md](./privacy.md) — Provenance and selective disclosure

**Cross-references:** [agents](../../applications/agents.md) — Agent lifecycle and manifest format, [adversarial-defense](../../security/adversarial-defense.md) — Supply chain threats, [intent-verifier](../../intelligence/intent-verifier.md) — Intent verification for delegated actions

-----

## 10. Agent Identity

Agents are first-class identity participants in AIOS. Every agent has a cryptographic identity derived from its developer's signing key, and every action an agent takes is attributable through an unforgeable delegation chain. This section covers the identity aspects of agents — how they prove who made them, what they're authorized to do, and how their actions are recorded.

### 10.1 Agent Manifest Signing

Every agent ships with a signed manifest that establishes its identity and integrity:

```rust
pub struct AgentManifest {
    /// Unique agent identifier (developer namespace + agent name)
    pub agent_id: AgentId,
    /// Developer identity (did:peer of the developer)
    pub developer_did: DidPeer,
    /// Developer's signing key (from manifest signature)
    pub developer_key: Ed25519PublicKey,
    /// Agent name (human-readable)
    pub agent_name: String,
    /// Agent version (semver)
    pub version: Version,
    /// Content hash of the agent binary/bundle
    pub binary_hash: ContentHash,
    /// Declared capabilities (what the agent requests access to)
    pub declared_capabilities: Vec<DeclaredCapability>,
    /// Declared intent (what the agent does, verified at install)
    pub declared_intent: DeclaredIntent,
    /// Minimum AIOS version required
    pub min_aios_version: Version,
    /// Build reproducibility metadata
    pub build_info: Option<BuildInfo>,
}

pub struct BuildInfo {
    /// Build system identifier (e.g., "cargo 1.80.0")
    pub build_system: String,
    /// Source repository URL
    pub source_url: Option<String>,
    /// Source commit hash (for reproducible builds)
    pub source_commit: Option<ContentHash>,
    /// Build timestamp
    pub build_timestamp: Timestamp,
}

pub struct SignedManifest {
    /// The manifest contents
    pub manifest: AgentManifest,
    /// Developer's signature over the serialized manifest
    pub developer_signature: Signature,
    /// Optional: countersignature from a transparency log
    pub transparency_receipt: Option<TransparencyReceipt>,
}
```

#### 10.1.1 Manifest Verification

The Identity Service verifies agent manifests at install time:

```text
Install flow:
1. Parse SignedManifest from agent package
2. Verify developer_signature against developer_key
3. Verify binary_hash matches actual agent binary
4. Check developer_did against known/trusted developer identities
5. Compare declared_capabilities against user's trust policy
6. If transparency_receipt present: verify inclusion proof
7. Store verified manifest in Identity Service agent registry
```

Agents without valid manifest signatures are rejected. The user can override for development/sideloaded agents, but these receive the lowest trust level and a visible warning indicator.

#### 10.1.2 Supply Chain Verification

AIOS verifies the supply chain from developer to installed agent:

```rust
pub struct SupplyChainVerification {
    /// Was the manifest signature valid?
    pub signature_valid: bool,
    /// Does the binary hash match?
    pub binary_integrity: bool,
    /// Is the developer identity known to this user?
    pub developer_known: bool,
    /// Developer's trust level (from relationship graph)
    pub developer_trust: TrustLevel,
    /// Was the agent seen in a transparency log?
    pub transparency_verified: bool,
    /// Has the agent been seen by other trusted peers?
    pub peer_attestation_count: u32,
    /// Overall supply chain confidence
    pub confidence: SupplyChainConfidence,
}

pub enum SupplyChainConfidence {
    /// Developer is in user's trust graph + transparency log + peer attestations
    High,
    /// Developer signed + binary verified, but no transparency/peer data
    Medium,
    /// Sideloaded or unverified developer
    Low,
    /// Failed verification — should not run
    Failed,
}
```

**Key principle:** Supply chain verification uses the same identity and trust infrastructure as peer relationships. A developer is an identity in the user's relationship graph. An agent from a Family-trust developer gets higher supply chain confidence than one from an Unknown developer.

### 10.2 Agent Signing Keys

Each installed agent receives a scoped signing key derived from the user's identity:

```rust
pub struct AgentIdentity {
    /// The verified manifest (established at install)
    pub manifest: SignedManifest,
    /// Agent-scoped signing key (derived from user's device key)
    /// Used for signing actions on behalf of the user
    pub agent_signing_key: DerivedKey,
    /// Key derivation path (SLIP-0010 HD):
    /// m/0x41494F53'/agents'/agent_id_hash'/0'
    pub derivation_path: DerivationPath,
    /// When this agent identity was established
    pub installed: Timestamp,
    /// Last time the agent's manifest was re-verified
    pub last_verified: Timestamp,
}
```

Agent signing keys are HD-derived (§4, [core.md](./core.md)) so they can be revoked independently without affecting other agents or the user's identity key.

### 10.3 Delegation Chains

When an agent acts on behalf of the user, the action includes a delegation chain that proves authorization:

```rust
pub struct DelegatedAction {
    /// What action was performed
    pub action: ActionDescription,
    /// The user's identity (delegator)
    pub user_identity: IdentityId,
    /// The agent that performed the action (delegate)
    pub agent_identity: AgentIdentity,
    /// User's signature authorizing the agent (capability grant)
    pub delegation_proof: SpaceCapabilityToken,
    /// Agent's signature over the action
    pub agent_signature: Signature,
    /// Timestamp
    pub timestamp: Timestamp,
}
```

This creates an unforgeable record: the user authorized the agent (delegation_proof), and the agent performed the specific action (agent_signature). Both signatures are verifiable by any third party.

#### 10.3.1 Delegation Chain Limits

Multi-agent delegation (agent A delegates to agent B) is bounded to prevent confused deputy attacks and unbounded trust propagation:

```rust
pub const MAX_DELEGATION_DEPTH: u8 = 3;

pub struct DelegationChain {
    /// The original delegator (always the user)
    pub root: IdentityId,
    /// Ordered chain of delegates
    pub chain: Vec<DelegationLink>,
}

pub struct DelegationLink {
    /// Who delegated
    pub delegator: DelegatorId,
    /// Who received delegation
    pub delegate: AgentId,
    /// What capabilities were delegated (must be subset of delegator's)
    pub delegated_capabilities: Vec<Capability>,
    /// Proof of delegation (signature by delegator)
    pub proof: Signature,
    /// Expiry (delegation is time-bounded)
    pub expires: Timestamp,
}

pub enum DelegatorId {
    User(IdentityId),
    Agent(AgentId),
}
```

**Delegation invariants:**

- **Depth limit:** `chain.len() <= MAX_DELEGATION_DEPTH` (user → agent₁ → agent₂ → agent₃). Deeper chains are rejected by the capability system.
- **Monotonic attenuation:** Each link can only narrow capabilities, never widen. Agent₂ cannot have more capabilities than agent₁ granted it.
- **Time bounding:** Every delegation link has an expiry. Sub-delegations cannot outlive their parent delegation.
- **Revocation cascades:** Revoking any link invalidates all downstream links. Revoking the user's delegation to agent₁ immediately invalidates agent₂ and agent₃'s delegations.
- **Audit trail:** Every delegation link is recorded in the provenance system.

### 10.4 Provenance Integration

Every space object records who created or modified it. Provenance is identity-linked and Merkle-chained for tamper evidence:

```rust
pub struct Provenance {
    /// Who created this object
    pub creator: ProvenanceActor,
    /// When
    pub created: Timestamp,
    /// Modification history
    pub modifications: Vec<ProvenanceEntry>,
    /// Merkle-linked to parent (tamper-evident)
    pub parent_hash: Option<MerkleHash>,
    /// Signature over this provenance entry
    pub signature: Signature,
}

pub enum ProvenanceActor {
    /// Created directly by the user
    User { identity: IdentityId },
    /// Created by an agent on behalf of the user
    Agent {
        identity: IdentityId,
        agent: AgentIdentity,
        delegation: SpaceCapabilityToken,
    },
    /// Created by AI inference
    AiGenerated {
        identity: IdentityId,
        model: ModelId,
        prompt_hash: ContentHash,
        generation_params: GenerationParams,
    },
    /// Imported from external source
    Imported {
        identity: IdentityId,
        source_url: String,
        import_agent: AgentIdentity,
    },
}
```

#### 10.4.1 AI-Generated Content Provenance

AI-generated content receives enhanced provenance metadata so users can always distinguish human-created from AI-generated content:

```rust
pub struct GenerationParams {
    /// Model identifier (e.g., "airs:llama-3.1-8b-q4")
    pub model_id: ModelId,
    /// Model version hash (for reproducibility)
    pub model_hash: ContentHash,
    /// Quantization level used
    pub quantization: Option<String>,
    /// Temperature and sampling parameters
    pub sampling: SamplingParams,
    /// Whether the output was edited by the user after generation
    pub human_edited: bool,
    /// If edited: hash of the original AI output (before edits)
    pub original_output_hash: Option<ContentHash>,
}

pub struct SamplingParams {
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: Option<u32>,
    pub seed: Option<u64>,
}
```

**Provenance guarantees for AI content:**

- Every AI-generated object is tagged with `ProvenanceActor::AiGenerated`
- The `prompt_hash` is a SHA-256 hash of the prompt/context (the prompt itself is not stored in provenance — it may contain sensitive data)
- If the user edits AI-generated content, `human_edited` is set to `true` and `original_output_hash` preserves a link to the unedited version
- The `model_hash` allows verification that the same model version was used (important for reproducibility and audit)
- Provenance is signed by the user's identity key (AI cannot self-attest — the user's device witnessed the generation)

#### 10.4.2 Multi-Agent Provenance

When multiple agents collaborate on content, provenance records the full chain:

```rust
pub struct ProvenanceEntry {
    /// Who modified
    pub actor: ProvenanceActor,
    /// What changed (high-level description)
    pub action: ProvenanceAction,
    /// When
    pub timestamp: Timestamp,
    /// Hash of the object state after this modification
    pub state_hash: ContentHash,
    /// Signature over this entry
    pub signature: Signature,
    /// Link to previous entry (Merkle chain)
    pub previous: MerkleHash,
}

pub enum ProvenanceAction {
    Created,
    Modified { description: String },
    AiGenerated { generation_params: GenerationParams },
    AiEdited { original_hash: ContentHash },
    Imported { source: String },
    Delegated { from_agent: AgentId, to_agent: AgentId },
    Merged { sources: Vec<ContentHash> },
}
```

The Merkle chain ensures that provenance history cannot be modified without detection. Any observer can verify the complete history of who did what, when, and whether AI was involved at any step.
