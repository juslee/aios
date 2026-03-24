---
tags: [platform, agents, security, storage]
type: architecture
---

# AIOS Agent Package Format

**Parent document:** [README.md](../README.md) — Kit Architecture
**ADR:** [BeOS Lessons — Lesson 3](../../knowledge/decisions/2026-03-23-jl-beos-haiku-redox-lessons.md)
**Related:** [agents/lifecycle.md](../../applications/agents/lifecycle.md) — Agent installation and startup, [secure-boot/updates.md](../../security/secure-boot/updates.md) — A/B update scheme, [spaces.md](../../storage/spaces.md) — Package stored as Space object

-----

## 1. Overview

Agent packages are immutable sealed containers inspired by Haiku's `hpkg` format. A package is never extracted to disk — its contents are accessed read-only through the Space Service at runtime. Activation mounts the package; deactivation unmounts it and all traces vanish. This gives the system instant rollback (swap the active package version), atomic updates (replace the package file and activate the new version), and zero-residue uninstall (deactivate removes all filesystem footprint).

Packages are stored as Space objects with `ContentType::AgentPackage`. They are not a new Block Engine primitive — they use the existing content-addressed, encrypted, signed object model. The Version Store's Merkle DAG tracks package history, giving every installed agent a full version timeline from which any previous version can be restored without data loss.

Agent data is strictly separated from agent code. When a package version is rolled back, only the code reverts. User data under `/spaces/user/agents/{bundle_id}/data/` is never touched by package operations. This separation is enforced by the Agent Runtime — no package manifest field can override it.

-----

## 2. Package Structure

Agent packages are stored as Space objects. There is no new Block Engine type.

```rust
/// A content-addressed agent package stored in Space Storage.
pub struct AgentPackage {
    /// Space Storage object identifier for this package version.
    pub object_id: ObjectId,

    /// The signed, validated manifest extracted from the package.
    pub manifest: AgentManifest,

    /// SHA-256 hash of the complete package contents.
    /// Identical content across different agents shares storage blocks
    /// via the Block Engine's content-addressed deduplication.
    pub content_hash: ContentHash,

    /// Version Store node for this package version.
    /// Links to parent versions via the Merkle DAG, enabling
    /// instant rollback by activating a previous node.
    pub version: Version,

    /// Current activation state.
    pub activation_state: PackageActivationState,
}
```

The internal layout of a `.aios-agent` archive follows a fixed directory structure:

```text
my-agent.aios-agent/
├── manifest.toml          # AgentManifest (Ed25519-signed)
├── bin/
│   └── agent              # Native ELF (aarch64) or WASM module
├── src/
│   └── main.py            # Script entry point (Python/TypeScript agents)
├── resources/
│   ├── icons/
│   │   ├── icon-16.png
│   │   └── icon-256.png
│   └── data/
├── config/
│   └── defaults.toml      # Default configuration (copied on first install)
└── migration/
    └── v2_to_v3.toml      # Data migration descriptor for version upgrades
```

The `manifest.toml` declares the agent's identity, capabilities, content types, and Scriptable Protocol suites. All fields that touch other AIOS subsystems (`content_types`, `scriptable`, `data_migration`) are validated at activation time.

-----

## 3. Package Lifecycle

```rust
/// Activation state of an installed agent package.
pub enum PackageActivationState {
    /// Package stored but not active. No process, no registered suites,
    /// no content type claims. Package data retained for reactivation.
    Available,

    /// Package active. Agent process may or may not be running
    /// (depends on ActivationMode: eager starts immediately, lazy defers).
    Active,

    /// Package suspended by user, Agent Runtime (memory pressure), or policy.
    /// State preserved in Spaces; agent can resume without reinstall.
    Suspended,

    /// Package deactivated. Suites and content type registrations removed.
    /// Package object remains in Space Storage for potential reactivation.
    Deactivated,
}
```

**Activation sequence** (transitions `Available` → `Active`):

```text
1. Validate content_hash against stored SHA-256
   ✗ → reject (integrity failure)

2. Verify Ed25519 package signature against publisher key
   ✗ → reject (signature invalid)

3. Set activation_state = Active

4. Register Scriptable suites with Tool Manager

5. Register content types with Content Type Registry

6. Create Version Store snapshot (captures pre-activation state for rollback)

7. Service Manager records agent as startable
   → eager: start process immediately
   → lazy: defer until first use trigger
```

**Deactivation** reverses steps 5 → 4 → 3 in order, then stops the agent process via the graceful shutdown protocol. No package data is deleted — only registrations and the process are removed.

-----

## 4. State Rollback

The Version Store tracks package versions as a Merkle DAG. Each activation creates a snapshot node with a reference to the parent version. Rollback activates a previous node.

```rust
/// Rollback an agent to a previous package version.
///
/// Deactivates the current version, activates the specified previous
/// version, and runs any inverse migration hooks if declared.
/// Agent data under /spaces/user/agents/{bundle_id}/data/ is untouched.
pub fn rollback_package(
    bundle_id: &BundleId,
    target_version: &Version,
) -> Result<(), PackageError>;
```

Rollback integrates with the A/B update scheme at the system level:

- **Agent packages** — individually rollback-able via the Version Store. Each agent maintains its own version history independent of the OS.
- **System components** — rolled back via A/B partitions on the ESP (see [secure-boot/updates.md](../../security/secure-boot/updates.md) §6).

The two schemes are complementary: A/B handles boot-critical OS components; the Version Store handles the per-agent package fleet.

-----

## 5. Data Separation

Agent code and agent data are stored in separate locations with separate rollback semantics.

```text
/spaces/system/agents/installed/{bundle_id}/    → package contents (read-only, sealed)
/spaces/user/agents/{bundle_id}/data/           → agent writable data   (NEVER rolled back)
/spaces/user/agents/{bundle_id}/config/         → agent configuration   (NEVER rolled back)
/spaces/user/agents/{bundle_id}/cache/          → agent cache           (reclaimable)
```

**Invariant.** The Agent Runtime enforces that package operations (activate, deactivate, rollback, update) never touch paths under `/spaces/user/agents/{bundle_id}/data/`. This directory belongs to the user. The only way to delete agent data is an explicit user action through the Settings surface or Inspector.

**Migration hooks.** When updating across versions with schema changes, the `data_migration` field in the manifest declares a migration descriptor:

```rust
/// Declares data migration rules between two package versions.
pub struct DataMigration {
    /// Package version this migration upgrades from.
    pub from_version: semver::Version,

    /// Package version this migration upgrades to.
    pub to_version: semver::Version,

    /// Path to the migration script relative to migration/ in the package.
    /// Runs in the agent's runtime (Python, TypeScript, WASM, or native).
    pub script: &'static str,
}
```

Migration scripts run after the new package activates but before the agent process starts. If a migration script fails, the activation is rolled back to the previous version automatically.

-----

## 6. Development Mode

For agent development, a dev-mode flag mounts from a writable directory instead of a sealed package. This is the only case where a package mount is writable.

```rust
/// Development activation — mounts from a source directory rather
/// than a sealed package object. Requires developer capability.
pub enum PackageSource {
    /// Production: sealed, content-addressed Space object.
    Sealed { object_id: ObjectId },

    /// Development: writable directory mount.
    /// Changes are reflected immediately without repackaging.
    DevDirectory { path: SpacePath },
}
```

Dev mode requires the `DeveloperMode` capability, which is not granted to agents in the standard trust tier. The Agent Runtime enforces this at activation time — attempting to activate a `DevDirectory` source without the capability returns `PackageError::CapabilityDenied`.

The dev directory layout must match the standard package layout (manifest.toml, bin/, src/, etc.). The Agent Runtime validates the manifest on each activation even in dev mode.

-----

## 7. Security

Package integrity is enforced at activation, not at install time. Storing a corrupted or tampered package is permitted — the integrity check gates execution.

**Signature verification.** Every package is signed with the publisher's Ed25519 key. The signature covers the full `content_hash` of the package. Verification uses the Identity subsystem's key resolution (see [identity/agents.md](../../experience/identity/agents.md) §10).

**Capability-constrained mount.** The read-only mount at `/spaces/system/agents/installed/{bundle_id}/` is served through Space Storage capability checks. An agent can only read its own package files — it cannot enumerate other agents' packages without an explicit `PackageInspect` capability.

**Content-addressed deduplication.** Two agents shipping identical resource files share the same underlying Block Engine blocks. The SHA-256 `content_hash` is the identity — blocks cannot be silently substituted without breaking verification.

**Manifest integrity.** The manifest is included in the `content_hash` calculation. A manifest cannot be altered after signing without invalidating the package signature.

-----

## 8. Design Principles

1. **Immutable packages.** Agent code is sealed in a content-addressed object. Contents are mounted read-only. Nothing is ever extracted to mutable storage.

2. **Data persistence.** User data survives all package operations — activation, deactivation, update, rollback. Data separation is an invariant enforced by the Agent Runtime, not a convention for agents to follow.

3. **Space objects.** Packages use the existing `ObjectId` + `ContentType::AgentPackage` model. No new Block Engine types. The Version Store provides history for free.

4. **Atomic updates.** Updating an agent replaces the package object in Space Storage and activates the new version. If activation fails, the previous version remains active.

5. **Clean uninstall.** Deactivating a package removes all filesystem footprint (registrations, process) without deleting user data. Full removal requires an explicit user action that deletes the data Space.
