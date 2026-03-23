# Intent Kit

**Layer:** Intelligence | **Crate:** `aios_intent` | **Architecture:** [`docs/intelligence/intent-verifier.md`](../../intelligence/intent-verifier.md) + 6 sub-docs

## 1. Overview

The Intent Kit enforces that agent actions match their declared intent. Traditional permission
systems answer "is this agent allowed to perform this action?" at each individual syscall.
They do not answer "should data that originated from Space X reach network output Y?" across a
chain of actions spanning multiple agents. The Intent Kit closes this gap by implementing
decentralized information flow control (DIFC) through taint labels on data, a data flow graph
that tracks propagation across IPC boundaries, and a verification pipeline that checks runtime
actions against declared intent specifications.

Agents declare their intent at task start using `DeclaredIntent` (free-text description plus
expected Spaces and capabilities) or the more structured `StructuredIntent` (machine-checkable
purpose categories, temporal logic formulas, data flow specifications, and resource bounds).
The Intent Verifier, running inside the [AIRS Kit](airs.md) system service, evaluates each
observed action against the declared intent. Structured fields enable algorithmic pre-checking
for ~80% of actions without LLM inference. Ambiguous cases fall through to semantic LLM
verification. Violations trigger escalating enforcement: log, warn, throttle, block, or
terminate.

Use the Intent Kit when your agent needs to declare its intended behavior to the system,
verify incoming data provenance, or participate in information flow control. Do not use it for
simple permission checks (use the [Capability Kit](../kernel/capability.md)) or for runtime
resource management (use the [Compute Kit](../kernel/compute.md)).

## 2. Core Traits

```rust
use aios_intent::{
    DeclaredIntent, StructuredIntent, IntentVerifier,
    TaintLabel, LabelSet, DataFlowGraph,
    IntentPurpose, TemporalSpec, DataFlowSpec,
};
use aios_capability::CapabilityHandle;

/// Declare an agent's intended behavior for a task.
///
/// Every agent task begins by declaring intent. The declaration is
/// registered with the Intent Verifier and used to assess all subsequent
/// actions. Without a declaration, all actions are logged as "undeclared"
/// and evaluated with heightened suspicion.
pub struct DeclaredIntent {
    /// The task this intent covers.
    pub task: TaskId,
    /// The declaring agent.
    pub agent: AgentId,
    /// Free-text description of what the agent intends to do.
    /// Used by the LLM verifier for semantic comparison.
    pub description: String,
    /// Spaces the agent expects to access during this task.
    /// Any access to an unlisted Space is flagged immediately.
    pub expected_spaces: Vec<SpaceId>,
    /// Capability types the agent expects to exercise.
    /// Any undeclared capability usage triggers suspicion.
    pub expected_capabilities: Vec<Capability>,
}

/// Extended intent with machine-checkable fields.
///
/// StructuredIntent enables algorithmic pre-filtering for the majority
/// of verification decisions, reducing LLM inference calls by ~80%.
/// Agents that provide StructuredIntent get faster verification and
/// lower overhead than those using DeclaredIntent alone.
pub struct StructuredIntent {
    /// Base intent (backward compatible with DeclaredIntent).
    pub base: DeclaredIntent,
    /// Machine-checkable purpose categories (why data is accessed).
    pub purposes: Vec<IntentPurpose>,
    /// Expected action patterns as temporal logic formulas.
    pub expected_behavior: Vec<TemporalSpec>,
    /// Allowed data flow paths (source Space -> sink, with transforms).
    pub allowed_flows: Vec<DataFlowSpec>,
    /// Maximum resource bounds for the task.
    pub resource_bounds: ResourceBounds,
}

/// Why an agent accesses data. Machine-checkable purpose categories
/// inspired by Apple privacy manifests.
pub enum IntentPurpose {
    /// Read data to display to the user.
    Display,
    /// Read data to transform and write back.
    Transform,
    /// Read data to generate a summary or analysis.
    Analyze,
    /// Read data to send over the network.
    NetworkTransmit { destinations: Vec<String> },
    /// Read data to share with another agent via IPC.
    AgentShare { target_agents: Vec<AgentId> },
    /// Read data for indexing or search.
    Index,
    /// Read data for inference input.
    InferenceInput,
}

/// A temporal logic formula constraining action sequences.
///
/// Based on Metric Temporal Logic (MTL). Enables formal verification of
/// action ordering without LLM inference.
pub struct TemporalSpec {
    /// Human-readable description of the constraint.
    pub description: String,
    /// The formula in the temporal specification language.
    pub formula: TemporalFormula,
}

/// Allowed data flow path from source to sink.
pub struct DataFlowSpec {
    /// Source Space or data origin.
    pub source: DataFlowEndpoint,
    /// Destination Space, network, or agent.
    pub sink: DataFlowEndpoint,
    /// Required transforms the data must pass through.
    pub required_transforms: Vec<String>,
    /// Whether the flow requires user approval at runtime.
    pub requires_consent: bool,
}

/// Register intent and query verification results.
///
/// The IntentVerifier is a system service. Agents interact with it to
/// register intent at task start and to query verification status.
/// The actual verification happens automatically on every syscall.
pub trait IntentVerifier {
    /// Register intent for a new task. Must be called before the agent
    /// performs any actions for the task.
    fn declare(
        &self,
        intent: DeclaredIntent,
        cap: &CapabilityHandle,
    ) -> Result<IntentHandle, IntentError>;

    /// Register structured intent for enhanced algorithmic pre-checking.
    fn declare_structured(
        &self,
        intent: StructuredIntent,
        cap: &CapabilityHandle,
    ) -> Result<IntentHandle, IntentError>;

    /// Query the current verification status for a task.
    fn status(
        &self,
        handle: &IntentHandle,
    ) -> Result<VerificationStatus, IntentError>;

    /// End an intent declaration (task complete). The verifier logs the
    /// final compliance report and releases resources.
    fn complete(
        &self,
        handle: IntentHandle,
    ) -> Result<ComplianceReport, IntentError>;
}

/// Verification status for a declared task.
pub struct VerificationStatus {
    /// Number of actions verified so far.
    pub actions_verified: u64,
    /// Number of actions that passed verification.
    pub actions_aligned: u64,
    /// Number of suspicious actions (logged but allowed).
    pub actions_suspicious: u64,
    /// Number of actions blocked as violations.
    pub actions_blocked: u64,
    /// Current enforcement level for this task.
    pub enforcement: EnforcementLevel,
}

/// Enforcement levels (escalating).
pub enum EnforcementLevel {
    /// Normal operation. Actions are logged.
    Normal,
    /// Elevated scrutiny. All actions require verification.
    Heightened,
    /// Actions are throttled (rate-limited).
    Throttled,
    /// New actions are blocked pending review.
    Blocked,
}

/// Immutable label attached to data describing its security provenance.
///
/// Labels propagate automatically through reads, writes, and IPC
/// transfers. The kernel enforces flow constraints at IPC delivery time.
pub struct TaintLabel {
    /// Security zone of data origin (highest sensitivity encountered).
    pub zone: SecurityZone,
    /// Spaces the data originated from (max 4 for compact storage).
    pub source_spaces: Vec<SpaceId>,
    /// Whether data has passed through an approved declassification gate.
    pub declassified: bool,
    /// Integrity level (1 = untrusted external, 5 = kernel-verified).
    pub integrity: u8,
}

/// Query and inspect the data flow graph.
///
/// The DataFlowGraph tracks how data moves across IPC boundaries.
/// Agents can query it to check data provenance before processing.
pub trait DataFlowGraph {
    /// Query the provenance labels attached to received data.
    fn labels_for(
        &self,
        message: &MessageRef,
    ) -> Result<LabelSet, IntentError>;

    /// Check if data with the given labels can safely flow to a
    /// destination (e.g., network, another agent).
    fn check_flow(
        &self,
        labels: &LabelSet,
        destination: &DataFlowEndpoint,
    ) -> Result<FlowDecision, IntentError>;

    /// Query the full flow path for a piece of data (audit trail).
    fn trace_provenance(
        &self,
        labels: &LabelSet,
    ) -> Result<Vec<FlowHop>, IntentError>;
}

/// Result of a flow check.
pub enum FlowDecision {
    /// The flow is allowed by the declared intent.
    Allowed,
    /// The flow requires user consent (declared but consent-gated).
    RequiresConsent { reason: String },
    /// The flow is blocked (not declared or violates taint labels).
    Blocked { reason: String },
}
```

## 3. Usage Patterns

### Declaring basic intent at task start

```rust
use aios_intent::{IntentVerifier, DeclaredIntent};

fn start_research_task(
    verifier: &dyn IntentVerifier,
    cap: &CapabilityHandle,
    research_space: SpaceId,
) -> Result<IntentHandle, IntentError> {
    let intent = DeclaredIntent {
        task: TaskId::new(),
        agent: AgentId::current(),
        description: "Research papers about transformers and save notes".into(),
        expected_spaces: vec![research_space],
        expected_capabilities: vec![
            Capability::SpaceRead,
            Capability::SpaceWrite,
            Capability::NetworkRead, // Fetch papers from URLs
        ],
    };

    verifier.declare(intent, cap)
}
```

### Declaring structured intent for faster verification

```rust
use aios_intent::{IntentVerifier, StructuredIntent, DeclaredIntent, IntentPurpose, DataFlowSpec};

fn start_export_task(
    verifier: &dyn IntentVerifier,
    cap: &CapabilityHandle,
    source_space: SpaceId,
) -> Result<IntentHandle, IntentError> {
    let intent = StructuredIntent {
        base: DeclaredIntent {
            task: TaskId::new(),
            agent: AgentId::current(),
            description: "Export documents as PDF to the user's download folder".into(),
            expected_spaces: vec![source_space],
            expected_capabilities: vec![Capability::SpaceRead],
        },
        purposes: vec![
            IntentPurpose::Display,
            IntentPurpose::Transform,
        ],
        expected_behavior: vec![],
        allowed_flows: vec![
            DataFlowSpec {
                source: DataFlowEndpoint::Space(source_space),
                sink: DataFlowEndpoint::LocalFile("/downloads/".into()),
                required_transforms: vec!["to-pdf".into()],
                requires_consent: false,
            },
        ],
        resource_bounds: ResourceBounds {
            max_reads: Some(100),
            max_writes: Some(100),
            max_network_bytes: Some(0), // No network access
            max_duration: Some(core::time::Duration::from_secs(300)),
        },
    };

    verifier.declare_structured(intent, cap)
}
```

### Checking data provenance before processing

```rust
use aios_intent::{DataFlowGraph, FlowDecision, DataFlowEndpoint};

fn safe_to_send_to_network(
    graph: &dyn DataFlowGraph,
    message: &MessageRef,
    destination: &str,
) -> Result<bool, IntentError> {
    let labels = graph.labels_for(message)?;

    match graph.check_flow(
        &labels,
        &DataFlowEndpoint::Network(destination.into()),
    )? {
        FlowDecision::Allowed => Ok(true),
        FlowDecision::RequiresConsent { reason } => {
            // Prompt user for approval before sending
            request_user_consent(&reason);
            Ok(false) // Pending approval
        }
        FlowDecision::Blocked { reason } => {
            // Data cannot leave the device -- taint labels prevent it
            log_blocked_flow(&reason);
            Ok(false)
        }
    }
}
```

## 4. Integration Examples

### Intent Kit + Capability Kit: layered security

```rust
use aios_intent::{IntentVerifier, DeclaredIntent};
use aios_capability::CapabilityHandle;

fn secure_data_access(
    verifier: &dyn IntentVerifier,
    cap: &CapabilityHandle,
    space: SpaceId,
) -> Result<(), Box<dyn core::error::Error>> {
    // Layer 1: Capability check (handled by the kernel)
    // The agent must hold SpaceRead for this Space.

    // Layer 2: Intent verification
    // The agent must have declared this Space in its intent.
    let handle = verifier.declare(DeclaredIntent {
        task: TaskId::new(),
        agent: AgentId::current(),
        description: "Read user documents for display".into(),
        expected_spaces: vec![space],
        expected_capabilities: vec![Capability::SpaceRead],
    }, cap)?;

    // Layer 3: Taint label enforcement
    // Data read from this Space carries taint labels. If the agent
    // later tries to send this data over the network without declaring
    // NetworkTransmit purpose, the flow is blocked.

    // ... perform work ...

    let report = verifier.complete(handle)?;
    assert_eq!(report.violations, 0);

    Ok(())
}
```

### Intent Kit + IPC Kit: labeled message passing

```rust
use aios_intent::DataFlowGraph;

fn process_ipc_message(
    graph: &dyn DataFlowGraph,
    message: &MessageRef,
) -> Result<(), IntentError> {
    // Every IPC message carries taint labels automatically.
    // Check the labels before processing.
    let labels = graph.labels_for(message)?;

    if labels.zone == SecurityZone::Personal && labels.integrity < 3 {
        // Personal data with low integrity -- treat with caution.
        // Do not forward to untrusted agents.
        log::warn!("Processing low-integrity personal data");
    }

    // The agent's declared intent must cover this data flow.
    // If the agent sends this data onward, the taint labels propagate
    // to the outgoing message, and the next recipient's intent is
    // verified in turn.

    Ok(())
}
```

### Intent Kit + AIRS Kit: semantic verification fallback

```rust
use aios_intent::IntentVerifier;

// Internal to the Intent Verifier, shown for illustration.
// Agents do not call AIRS for intent verification directly.
fn verify_action(
    action: &ActionObservation,
    intent: &StructuredIntent,
    airs_available: bool,
) -> VerificationResult {
    // Step 1: Algorithmic pre-check (no AIRS needed)
    // - Check expected_spaces against accessed Space
    // - Check expected_capabilities against used capability
    // - Evaluate purpose categories
    // - Check temporal formulas
    // - Check data flow specs
    if let Some(result) = algorithmic_precheck(action, intent) {
        return result; // ~80% of actions are resolved here
    }

    // Step 2: LLM semantic verification (AIRS required)
    if airs_available {
        return airs_semantic_verify(action, &intent.base.description);
    }

    // Step 3: Fallback (no AIRS) -- allow with elevated logging
    VerificationResult::Suspicious {
        reason: "Ambiguous action, AIRS unavailable for semantic check".into(),
    }
}
```

## 5. Capability Requirements

| Capability | What It Gates | Default Grant |
| --- | --- | --- |
| `IntentDeclare` | Registering intent for a task | Granted to all agents |
| `IntentStatus` | Querying verification status for own tasks | Granted to all agents |
| `IntentComplete` | Ending an intent declaration | Granted to declaring agent |
| `FlowGraphRead` | Querying taint labels and data provenance | Granted to all agents |
| `FlowGraphTrace` | Full provenance trace (audit trail) | System agents only |
| `IntentAdmin` | Modifying verification policy, viewing all tasks | System agents only |
| `Declassify` | Removing taint labels from data (declassification gate) | Requires explicit user approval |

## 6. Error Handling

```rust
/// Errors returned by the Intent Kit.
pub enum IntentError {
    /// The agent lacks the required intent capability.
    /// Recovery: declare the capability in the agent manifest.
    CapabilityDenied(String),

    /// The intent declaration references Spaces or capabilities the agent
    /// does not hold. You cannot declare intent to access Spaces you
    /// cannot reach.
    /// Recovery: request the underlying capabilities first.
    InvalidDeclaration {
        reason: String,
        missing_capabilities: Vec<Capability>,
    },

    /// The intent handle has expired or been completed.
    /// Recovery: declare a new intent for a new task.
    HandleExpired(IntentHandle),

    /// A data flow was blocked by taint label enforcement.
    /// Recovery: add the flow to `allowed_flows` in StructuredIntent,
    /// or request user consent for the specific flow.
    FlowBlocked {
        source_zone: SecurityZone,
        destination: String,
        reason: String,
    },

    /// The temporal specification formula is malformed.
    /// Recovery: fix the formula syntax and re-declare.
    InvalidTemporalSpec(String),

    /// The Intent Verifier is not available (early boot or AIRS down).
    /// During unavailability, all actions are logged but not blocked.
    /// Recovery: actions proceed with elevated logging; re-check later.
    VerifierUnavailable,

    /// The data flow graph does not contain labels for the referenced
    /// message (message may have been garbage-collected).
    LabelsNotFound(MessageRef),

    /// The compliance report could not be generated (internal error).
    ReportError(String),
}
```

## 7. Platform & AI Availability

The Intent Kit separates algorithmic verification from AI-powered semantic analysis:

**Always available (no AIRS dependency):**

- Intent declaration (DeclaredIntent and StructuredIntent).
- Algorithmic pre-checking of structured fields:
  - Space access list verification (O(1) per action).
  - Capability type verification (O(1) per action).
  - Purpose category matching.
  - Temporal formula evaluation (MTL model-checking).
  - Data flow path checking against declared allowed_flows.
  - Resource bound enforcement (read/write/network counters).
- Taint label propagation through IPC messages (kernel-enforced).
- Data flow graph queries and provenance tracing.
- Compliance reporting at task completion.

**Available when AIRS is loaded:**

- Semantic LLM verification for ambiguous actions (~20% of actions).
- Natural language intent description comparison against observed behavior.
- Behavioral anomaly detection (learned agent behavior profiles).
- Adversarial intent evasion detection (ML-based).
- Rich compliance report generation with natural language explanations.

**Feature detection:**

```rust
use aios_intent::IntentVerifier;

fn verification_mode(verifier: &dyn IntentVerifier) -> &str {
    // When AIRS is unavailable, the verifier operates in
    // "algorithmic-only" mode. StructuredIntent fields are still
    // checked; only the LLM semantic fallback is disabled.
    if verifier.airs_available() {
        "full (algorithmic + semantic)"
    } else {
        "algorithmic only"
    }
}
```

**Verification overhead:**

| Verification Path | Latency | AIRS Required |
| --- | --- | --- |
| Structured pre-check (cache hit) | < 1 us | No |
| Structured pre-check (full evaluation) | < 100 us | No |
| LLM semantic verification | 10-50 ms | Yes |
| Taint label propagation (per IPC message) | < 1 us | No |
| Data flow graph query | < 10 us | No |

Agents that provide `StructuredIntent` with complete purpose categories and data flow
specs experience near-zero verification overhead. Agents using only `DeclaredIntent`
with free-text descriptions incur LLM inference cost on ambiguous actions.
