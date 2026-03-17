# AIOS Intent Verifier — Specification

Part of: [intent-verifier.md](../intent-verifier.md) — Intent Verifier Architecture
**Related:** [pipeline.md](./pipeline.md) — Verification Pipeline, [security.md](./security.md) — Adversarial Resistance

---

## §3 Intent Specification

Intent specification is the contract between an agent and the verification system. It declares *what the agent intends to do* in a form that enables both algorithmic pre-checking and semantic LLM verification. This section defines two levels of specification: the current `DeclaredIntent` (sufficient for initial implementation) and the research-informed `StructuredIntent` (enabling machine-checkable verification without LLM inference for ~80% of actions).

---

### §3.1 DeclaredIntent (Current Design)

The baseline intent structure from AIRS intelligence services (§5.4) provides the minimal contract for intent verification:

```rust
pub struct DeclaredIntent {
    task: TaskId,
    agent: AgentId,
    description: String,            // "Research papers about transformers"
    expected_spaces: Vec<SpaceId>,
    expected_capabilities: Vec<Capability>,
}
```

**Field roles in verification:**

- **`description`** — Free-text natural language describing the agent's purpose for this task. This field requires LLM inference for every verification: the Intent Verifier must semantically compare each observed action against this description to determine alignment. This is the most expressive field but also the most expensive to verify.

- **`expected_spaces`** — Enumerates the specific spaces the agent expects to access during this task. This is machine-checkable: any access to a space not in this list is immediately flagged without LLM involvement. However, the list is exact SpaceIds, not patterns — the agent must know specific space identifiers at task registration time.

- **`expected_capabilities`** — Lists the capability types the agent expects to exercise. Like `expected_spaces`, this enables O(1) pre-filtering: an action requiring an undeclared capability type triggers immediate suspicion.

**Limitations of DeclaredIntent:**

The free-text `description` field dominates verification cost. Every action that passes the `expected_spaces` and `expected_capabilities` pre-filters must still be sent to the LLM for semantic comparison against the description. For an agent performing 100 actions per minute, this means up to 100 LLM inference calls per minute — unsustainable on resource-constrained devices.

Additionally, `DeclaredIntent` lacks structure for expressing:

- *Purpose categories* — why the agent accesses data (retrieval vs. transformation vs. exfiltration)
- *Temporal constraints* — ordering and timing requirements on actions
- *Data flow paths* — which data moves where, through what transformations
- *Resource bounds* — expected volume and rate of operations

These gaps motivate the `StructuredIntent` extension.

---

### §3.2 StructuredIntent (Research-Informed Enhancement)

`StructuredIntent` extends `DeclaredIntent` with machine-checkable fields that enable algorithmic pre-filtering for the majority of verification decisions. The design draws from four research areas:

- **Apple privacy manifests** — structured purpose declarations that categorize *why* an app accesses sensitive data, enabling automated review without human inspection of source code
- **NeMo Guardrails execution rails** — programmatic action constraints that define allowed/disallowed action sequences, enabling runtime enforcement without per-action LLM calls
- **Metric Temporal Logic (MTL)** — time-bounded behavioral formulas that express constraints like "every write must be preceded by a read within 30 seconds," enabling formal verification of action sequences
- **Decentralized Information Flow Control (DIFC)** — taint labels on data that restrict flow paths, enabling compile-time or runtime detection of unauthorized data movement

```rust
pub struct StructuredIntent {
    /// Base intent (backward compatible)
    base: DeclaredIntent,

    /// Machine-checkable purpose categories
    purposes: Vec<IntentPurpose>,

    /// Expected action patterns as temporal logic formulas
    expected_behavior: Vec<TemporalSpec>,

    /// Allowed data flow paths (source → sink with required transforms)
    allowed_flows: Vec<DataFlowSpec>,

    /// Maximum resource bounds
    resource_bounds: ResourceBounds,
}
```

Each field contributes to algorithmic pre-checking in the verification pipeline (§4.2 in pipeline.md):

- **`purposes`** — categorizes the intent into machine-checkable purpose types, enabling O(1) lookup of whether an observed action falls within a declared purpose category
- **`expected_behavior`** — temporal logic formulas that the runtime monitor evaluates against the action stream without LLM inference
- **`allowed_flows`** — data flow specifications that the IPC taint tracking system (§5 in information-flow.md) verifies at every data movement
- **`resource_bounds`** — hard numeric limits enforced by simple counter comparison

#### Purpose Categories

```rust
pub enum IntentPurpose {
    /// Reading/searching existing information
    InformationRetrieval {
        sources: Vec<SpacePattern>,
    },

    /// Creating new content (documents, notes, code)
    ContentCreation {
        targets: Vec<SpacePattern>,
    },

    /// Sending messages, emails, notifications
    Communication {
        endpoints: Vec<NetworkPattern>,
    },

    /// Transforming data from one format/location to another
    DataTransformation {
        input: SpacePattern,
        output: SpacePattern,
        transform_type: TransformType,
    },

    /// System configuration, file management, maintenance
    SystemManagement {
        scope: ManagementScope,
    },
}

pub enum ManagementScope {
    /// Only the agent's own space
    OwnSpace,
    /// User-designated spaces
    DesignatedSpaces(Vec<SpacePattern>),
    /// System-wide (requires elevated trust)
    SystemWide,
}
```

Purpose categories enable a critical optimization: if an agent declares only `InformationRetrieval` purposes, any write action is immediately flagged as suspicious without consulting the LLM. The category narrows the semantic space the LLM must evaluate when it is consulted.

#### Temporal Specifications

```rust
pub struct TemporalSpec {
    /// Compact MTL formula
    /// Examples:
    ///   "always (space_write(X) -> previously(space_read(X), 30s))"
    ///   "never (network_send AND NOT declared_endpoint)"
    ///   "eventually (task_complete, 3600s)"
    formula: String,

    /// Human-readable description
    description: String,
}
```

Temporal specs are evaluated by the runtime temporal logic monitor (§9 in behavioral.md). The monitor maintains a sliding window of recent actions and evaluates each formula incrementally as new actions arrive. This catches ordering violations — such as an agent attempting to delete data it never read — without LLM inference.

#### Data Flow Specifications

```rust
pub struct DataFlowSpec {
    source: DataFlowEndpoint,
    sink: DataFlowEndpoint,
    /// Required transformation before data reaches sink
    required_transform: Option<TransformType>,
}

pub enum DataFlowEndpoint {
    Space(SpacePattern),
    Network(NetworkPattern),
    Inference(ModelPattern),
    User,
}

pub enum TransformType {
    Summarize,
    Anonymize,
    Aggregate,
    Encrypt,
    Format { target_format: String },
}
```

Data flow specs integrate with the IPC taint label system (§5 in information-flow.md). When data moves from a source to a sink, the verification system checks whether a matching `DataFlowSpec` exists and whether the `required_transform` was applied. An agent that reads email content and sends it to the network without the declared `Summarize` transform triggers a violation — the raw data flow was not authorized.

#### Resource Bounds

```rust
pub struct ResourceBounds {
    /// Max objects read per hour
    max_read_rate: u32,
    /// Max objects written per hour
    max_write_rate: u32,
    /// Max network bytes per hour
    max_network_bytes: u64,
    /// Max inference tokens per hour
    max_inference_tokens: u64,
    /// Max concurrent space access
    max_concurrent_spaces: u8,
}
```

Resource bounds are the simplest form of machine-checkable constraint. The verification system maintains per-agent counters and compares against these limits. Exceeding a bound triggers immediate throttling without LLM consultation. This provides a hard ceiling on damage even if all other verification layers are bypassed.

#### Pattern Types

```rust
pub struct SpacePattern {
    /// Glob-style pattern: "research/*", "user/notes/**"
    pattern: String,
}

pub struct NetworkPattern {
    /// Domain pattern: "*.arxiv.org", "api.openai.com"
    domain: String,
    /// Allowed ports (empty = all)
    ports: Vec<u16>,
    /// Allowed protocols
    protocols: Vec<Protocol>,
}

pub struct ModelPattern {
    /// Model name pattern: "summarizer-*", "embedding-*"
    pattern: String,
}

pub enum Protocol {
    Https,
    Wss,
    Smtp,
    Custom(String),
}
```

Patterns use glob-style matching for spaces and domain matching for network endpoints. The `**` pattern matches any depth of nesting (`"research/**"` matches `"research/papers/2024/transformers"`), while `*` matches a single path segment.

---

### §3.3 Intent in Agent Manifests

`StructuredIntent` integrates with the `AgentManifest` (agents.md §2.4) to declare an agent's overall intent at install time. The manifest's intent section declares the *maximum envelope* of behavior the agent may exhibit across all tasks. Individual task intents (registered at runtime via §3.4) must be subsets of the manifest intent.

```toml
[agent]
name = "Research Assistant"
bundle_id = "com.example.research-assistant"

[agent.intent]
description = "Search arxiv for papers about transformers and summarize findings"

[[agent.intent.purposes]]
type = "information_retrieval"
sources = ["arxiv/*", "scholar/*"]

[[agent.intent.purposes]]
type = "content_creation"
targets = ["research/notes/*"]

[[agent.intent.allowed_flows]]
source = { space = "arxiv/*" }
sink = { space = "research/notes/*" }
required_transform = "summarize"

[[agent.intent.allowed_flows]]
source = { space = "research/notes/*" }
sink = { network = "*.example.com" }

[agent.intent.resource_bounds]
max_read_rate = 100
max_write_rate = 50
max_network_bytes = 10485760  # 10 MB
max_inference_tokens = 50000

[[agent.intent.temporal_rules]]
formula = "always (space_delete -> previously(user_confirm, 300s))"
description = "Deletions require user confirmation within 5 minutes"
```

**Manifest validation at install time:**

AIRS performs `SecurityAnalysis` (agents.md §3.1) on the manifest during agent installation. The analysis has three stages:

1. **Structured field validation** — machine-checked without LLM. Verifies that declared purposes are consistent with declared capabilities (e.g., an agent declaring only `InformationRetrieval` purposes but requesting `WriteSpace` capabilities triggers a flag). Checks that allowed flows reference only declared source/sink patterns. Validates resource bounds are within platform limits.

2. **Description analysis** — LLM-checked. AIRS parses the free-text `description` field and compares it against the structured fields. If the description mentions "sending emails" but no `Communication` purpose is declared, AIRS raises a `SecurityConcern`. If the description says "read-only research" but `ContentCreation` targets are declared, that mismatch is flagged.

3. **Code analysis** — LLM-checked against the agent's code bundle. AIRS verifies that the code's actual behavior matches both the structured fields and the description. Capabilities used in code but not declared in the manifest are flagged as `capabilities_undeclared`. Capabilities declared but not used in code are flagged as `capabilities_unused` — a sign of over-privileged intent.

A mismatch between any two of {structured fields, description, code behavior} raises a `SecurityConcern` that is surfaced to the user during the install approval flow.

---

### §3.4 Intent Registration at Task Start

When a user initiates a task, the system constructs a `StructuredIntent` that combines the user's instruction with the agent's manifest capabilities. This registered intent governs all subsequent verification for the task's lifetime.

**Registration flow:**

```text
1. User provides natural language instruction
   → "summarize my unread emails"

2. AIRS parses instruction into StructuredIntent:
   a. Extract purposes:
      - InformationRetrieval { sources: ["email/inbox/*"] }
      - ContentCreation { targets: ["email/summaries/*"] }
   b. Infer allowed flows:
      - email/inbox/* → email/summaries/* (transform: Summarize)
   c. Set resource bounds based on task scope:
      - max_read_rate: 200 (inbox may be large)
      - max_write_rate: 10 (one summary per batch)
      - max_network_bytes: 0 (no network needed)
   d. Generate temporal specs:
      - "never (space_delete)" — summarization never deletes
      - "eventually (task_complete, 600s)" — expect completion in 10 min

3. Intersect with agent manifest capabilities:
   - Task intent MUST be a subset of manifest intent
   - If task requires purposes not in manifest → reject
   - If task needs spaces not covered by manifest patterns → reject

4. Register in IntentVerifier.active_tasks:
   - Key: TaskId (unique per task invocation)
   - Value: StructuredIntent (immutable for task lifetime)

5. Verification begins for all subsequent actions
```

**Intersection semantics:**

The task intent is the *intersection* of user instruction and manifest envelope, not the *union*. An agent whose manifest declares access to `email/*` and `calendar/*` receives a task intent scoped only to `email/*` when the user says "summarize my emails." The agent cannot access `calendar/*` during this task, even though its manifest permits it. This principle of least privilege per task limits the blast radius of a compromised agent.

```rust
pub fn register_task(
    &mut self,
    task: TaskId,
    agent: AgentId,
    user_instruction: &str,
    manifest: &AgentManifest,
) -> Result<StructuredIntent, IntentError> {
    // Step 1: Parse user instruction into candidate intent
    let candidate = self.parse_instruction(user_instruction)?;

    // Step 2: Validate candidate is subset of manifest
    let validated = self.intersect_with_manifest(candidate, manifest)?;

    // Step 3: Register for ongoing verification
    self.active_tasks.insert(task, validated.clone());

    Ok(validated)
}

pub enum IntentError {
    /// User instruction requests actions outside manifest envelope
    ExceedsManifest {
        requested: Vec<IntentPurpose>,
        allowed: Vec<IntentPurpose>,
    },
    /// AIRS unavailable and instruction is ambiguous
    AmbiguousWithoutAirs {
        instruction: String,
        suggestion: String,
    },
    /// Agent has no manifest intent section
    NoManifestIntent,
}
```

**Without AIRS:** When AIRS is unavailable, the system cannot parse free-text instructions into structured intents. In this case, registration falls back to the `DeclaredIntent` path: the agent's manifest `expected_spaces` and `expected_capabilities` are used directly, and the `description` field is stored but not semantically analyzed. Algorithmic pre-checks (space membership, capability type matching, resource bounds) continue to operate. Only the LLM-dependent semantic comparison is skipped, reducing verification coverage from ~95% to ~60% of actions.
