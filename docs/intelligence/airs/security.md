# AIOS AIRS Resource Orchestration Security

Part of: [airs.md](../airs.md) — AI Runtime Service
**Related:** [intelligence-services.md](./intelligence-services.md) — Intelligence services including security layers, [../../security/model.md](../../security/model.md) — Security model

-----

AIRS serves as the central resource orchestrator — directing memory pool boundaries, prefetching space objects, scheduling compression, and processing agent hints about anticipated needs. This responsibility is bounded by the security model. Full details in [model.md §9](../../security/model.md).

**Encryption boundary:** Prefetch and compression directives operate through the normal Space Storage read/write path ([spaces.md §4.3.1](../../storage/spaces.md)). Space Storage handles both per-space decryption ([spaces.md §6](../../storage/spaces.md)) and device-level decryption ([spaces.md §4.10](../../storage/spaces.md)) transparently. AIRS never holds space keys or device keys, and never sees plaintext of encrypted spaces unless it has been granted `ReadSpace` capability for that space. This is a structural guarantee enforced by the IPC boundary — AIRS cannot bypass Space Storage to access raw blocks.

## 10. Resource Orchestration Security

### 10.1 Security Path Isolation

AIRS performs two functions: security verification (Layers 1, 3, 5) and resource orchestration. These operate on separate code paths with a strict priority fence:

```rust
pub enum AirsInternalPath {
    /// Intent verification, behavioral analysis, injection detection.
    /// Highest priority. Never delayed by resource operations.
    /// Dedicated IPC channel from kernel.
    Security,
    /// Pool directives, prefetch, compression scheduling.
    /// Lower priority. Yields to security. Droppable under load.
    /// Falls back to kernel static heuristics if unavailable.
    Resource,
}
```

The security path and resource path share no mutable state. A resource decision never influences an intent verification, and vice versa. If the resource path is under load, the security path still meets its SLA (< 10 ms for synchronous intent checks).

#### 10.1.1 Internal Crash Containment

AIRS is a single process, but a panic in one subsystem must not crash all of AIRS. Each intelligence service runs within a `catch_unwind` boundary:

```rust
/// Each AIRS subsystem runs behind a panic boundary.
/// A panic in the Space Indexer does not crash intent verification.
pub struct SubsystemRunner {
    name: &'static str,
    state: SubsystemState,
    consecutive_panics: u32,
    last_panic: Option<Timestamp>,
}

pub enum SubsystemState {
    Running,
    /// Subsystem panicked, restarting
    Restarting,
    /// Subsystem panicked too many times, disabled until manual intervention
    Disabled { reason: String },
}

impl SubsystemRunner {
    /// Run a subsystem task within a panic boundary.
    /// On panic: log, increment counter, restart subsystem.
    /// On repeated panic (3x in 60s): disable subsystem, notify user.
    /// Panics more than 60s apart reset the consecutive counter.
    pub fn run<F, R>(&mut self, f: F) -> Option<R>
    where F: FnOnce() -> R + std::panic::UnwindSafe
    {
        match std::panic::catch_unwind(f) {
            Ok(result) => Some(result),
            Err(panic_info) => {
                log_panic(self.name, &panic_info);
                let current = now();
                // Reset counter if last panic was more than 60s ago
                if let Some(last) = self.last_panic {
                    if current.saturating_sub(last) > Duration::from_secs(60) {
                        self.consecutive_panics = 0;
                    }
                }
                self.consecutive_panics += 1;
                self.last_panic = Some(current);
                if self.consecutive_panics >= 3 {
                    self.state = SubsystemState::Disabled {
                        reason: format!("{} panicked {} times in 60s", self.name, self.consecutive_panics),
                    };
                    notify_user(self.name);
                } else {
                    self.state = SubsystemState::Restarting;
                }
                None
            }
        }
    }
}
```

**Containment boundaries:**

| Subsystem | On Panic | System Impact |
|---|---|---|
| Space Indexer | Restart indexer; semantic search degrades to text-only | Low — search still works via full-text index |
| Context Engine | Fall back to rule-based heuristics | Low — context inference is less nuanced |
| Attention Manager | Notifications pass through unfiltered | Medium — user sees more notifications |
| Intent Verifier | Apply fallback policy (skip / block-all / read-only per agent) | Medium — security degrades to capability-only |
| Behavioral Monitor | Rate limits still enforced by kernel | Medium — anomaly detection paused |
| Adversarial Defense | Kernel structural checks still active | Medium — injection detection paused |
| Inference Engine | All AIRS services degrade to non-AI fallbacks | High — equivalent to AIRS-down state |
| Conversation Manager | Conversation bar unavailable; agents still functional | Low — agents work independently |

If the Inference Engine itself panics, the entire AIRS process restarts via the Service Manager's reconnection protocol (ipc.md §5.5). Recovery target: < 500 ms.

### 10.2 Agent Hint Processing

Agents may submit resource hints — lightweight signals about anticipated needs. These are untrusted input from potentially malicious agents:

```rust
pub struct ResourceHint {
    agent: AgentId,
    hint_type: HintType,
    timestamp: Timestamp,
}

pub enum HintType {
    /// Agent anticipates needing more memory soon
    MemoryAnticipation { estimated_bytes: usize },
    /// Agent anticipates accessing these space objects
    AccessAnticipation { objects: Vec<ObjectId> },
    /// Agent anticipates needing inference
    InferenceAnticipation { task_type: TaskType },
}
```

**Screening pipeline (before AIRS considers any hint):**

1. **Rate limit** — max 10 hints/minute per agent. Exceeding triggers throttle.
2. **Blast radius check** — requested resources must be within agent's `BlastRadiusPolicy` limits.
3. **Capability check** — hinted objects must be in spaces the agent holds capabilities for.
4. **Behavioral consistency** — hint volume and pattern compared to agent's baseline.
5. **No feedback** — hints are fire-and-forget. AIRS never confirms or denies acting on a hint. This prevents agents from using hints as a probe.

Hints that fail screening are silently dropped and logged as security events.

### 10.3 Kernel Oversight

The kernel monitors AIRS resource directive patterns using simple statistical checks (no AI, no LLM). If AIRS directive behavior becomes anomalous (rate > 3σ above baseline, or hard limits exceeded), the kernel transitions to **fallback mode**:

- Resource orchestration disabled (no prefetch, no pool resize, no compression scheduling)
- Security functions (intent verification, behavioral monitoring, adversarial defense) remain active
- Static heuristics replace AI-driven decisions (plain LRU, fixed pools, age-based compression)
- System is slower but equally secure

Recovery: AIRS exits fallback when directive rates return to within 2σ for 10 consecutive minutes.

### 10.4 Resource Allocation Opacity

Agents cannot observe AIRS resource decisions. Each agent sees only its own virtual address space (TTBR0 page table isolation). Pool boundary changes, prefetch activity for other agents, and AIRS directive rates are kernel-internal operations invisible to userspace. This prevents resource allocation side-channel attacks. See [model.md §9.4](../../security/model.md) for full analysis.

### 10.5 Provenance

All AIRS resource directives are logged in the provenance chain (Merkle chain, kernel-signed, append-only). Directive types: `ResourcePrefetch`, `ResourcePoolResize`, `ResourceCompress`, `ResourceFallbackTransition`, `ResourceHintReceived`. These follow standard audit retention (7 days full, 90 days summarized, hash-only after). See [model.md §2.7.1](../../security/model.md).
