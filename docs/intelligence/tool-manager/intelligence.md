# AIOS AI-Native Tool Intelligence

Part of: [tool-manager.md](../tool-manager.md) — Tool Manager
**Related:** [security.md](./security.md) — Audit & anomaly detection, [registry.md](./registry.md) — Tool discovery, [execution.md](./execution.md) — Execution pipeline

---

## 15. AI-Native Tool Selection

AIOS's Tool Manager is designed for AI-first operation. AIRS selects, invokes, and chains tools on behalf of agents and the user — treating tool descriptions as part of the LLM's reasoning context.

### 15.1 LLM-Powered Tool Selection

When AIRS processes a user request that requires tool invocation, it uses constrained decoding to generate structured tool call outputs:

1. **Context assembly:** AIRS collects available tool definitions (filtered by the active agent's capabilities) and injects them into the LLM prompt as a tool list
2. **Constrained decoding:** The inference engine uses a DFA-based grammar to constrain output to valid JSON tool call format: `{"function": "<tool_name>", "arguments": {<valid_params>}}`
3. **Tool dispatch:** The generated tool call is parsed and forwarded to the Tool Manager's execution pipeline (§5)
4. **Result integration:** The tool result is fed back into the LLM context for further reasoning or final response generation

```rust
/// Tool selection via constrained decoding
pub struct ToolSelector {
    /// Embedding index of tool descriptions
    tool_embeddings: EmbeddingIndex,
    /// Grammar for constrained decoding (DFA)
    tool_call_grammar: Grammar,
    /// Historical selection data (for learning)
    selection_history: SelectionHistory,
}

impl ToolSelector {
    /// Build the tool call grammar from available tools
    pub fn build_grammar(&self, available_tools: &[ToolInfo]) -> Grammar {
        // DFA states: ~100 per tool (name literal + param schema)
        // Total states: ~100 × N tools
        let mut grammar = Grammar::new();
        grammar.add_rule("tool_call", r#"{"function": <tool_name>, "arguments": <params>}"#);
        for tool in available_tools {
            grammar.add_literal("tool_name", &tool.name);
            grammar.add_schema("params", &tool.parameters);
        }
        grammar
    }
}
```

Cross-reference: [ai-native.md](../airs/ai-native.md) §14.3 for constrained decoding DFA states (~100 per tool call grammar).

### 15.2 Tool Ranking and Selection Heuristics

When multiple tools match a user request (e.g., two different PDF extractors), AIRS ranks candidates using a multi-signal scoring function:

```rust
pub struct ToolScore {
    /// Semantic similarity between request and tool description
    pub semantic_match: f32,      // 0.0–1.0, from embedding cosine similarity
    /// Provider trust level weight
    pub trust_weight: f32,         // TL3: 1.0, TL2: 0.8, TL1: 0.5, TL0: 0.2
    /// Historical success rate
    pub success_rate: f32,         // recent calls: successes / total
    /// Latency score (inverse of p50 latency, normalized)
    pub latency_score: f32,        // faster providers score higher
    /// User preference signal (explicit or inferred)
    pub preference_score: f32,     // from preference system
    /// Final composite score
    pub composite: f32,            // weighted sum
}

impl ToolScore {
    pub fn compute(weights: &SelectionWeights) -> f32 {
        weights.semantic * self.semantic_match
            + weights.trust * self.trust_weight
            + weights.reliability * self.success_rate
            + weights.speed * self.latency_score
            + weights.preference * self.preference_score
    }
}
```

**Semantic matching:** Tool descriptions are embedded using the Space Indexer's embedding model ([intelligence-services.md](../airs/intelligence-services.md) §5.1). When AIRS needs to select a tool, the user request is embedded and compared against tool description embeddings using cosine similarity. This enables natural-language tool discovery — "I need to convert this spreadsheet to a chart" matches a `data-visualize` tool without exact keyword match.

### 15.3 Tool Recommendation

AIRS proactively suggests tools based on user context, without the user explicitly requesting a tool:

**Context signals used for recommendation:**

| Signal | Source | Example |
|---|---|---|
| Active space content | Context Engine | User opened a PDF → suggest `pdf-extract` |
| Recent actions | Behavioral Monitor | User just wrote code → suggest `code-analyze` |
| Task description | Task Manager | Task includes "research" → suggest `web-search` |
| Conversation context | Conversation Manager | User asked about data → suggest `data-query` |
| Historical patterns | Selection History | User always uses `summarize` after `pdf-extract` |

Cross-reference: [context-engine.md](../context-engine.md) for context signal collection and inference.

**Recommendation display:** Tool recommendations are surfaced through the Attention Manager ([attention.md](../attention.md)) as low-urgency suggestions. The user can accept, dismiss, or ignore them. AIRS learns from accept/dismiss patterns to improve future recommendations.

### 15.4 Tool Chaining

Complex tasks often require multiple tool calls in sequence or parallel. AIRS builds tool chains as part of task decomposition:

**Sequential chaining:**
```text
User: "Summarize the research papers in my documents folder"
  1. list-files(space="documents", filter="*.pdf") → [file1.pdf, file2.pdf, ...]
  2. For each file: pdf-extract(object_id=file.id) → text
  3. For each text: summarize(text=extracted, max_words=200) → summary
  4. Combine summaries into final response
```

**Parallel chaining:**
```text
User: "Compare the pricing in these two documents"
  1. pdf-extract(object_id=doc1) → text1  }  parallel
     pdf-extract(object_id=doc2) → text2  }
  2. compare(text_a=text1, text_b=text2) → comparison
```

**DAG execution:** Tool chains are represented as directed acyclic graphs (DAGs) within the Task Manager's DAG execution system ([task-manager.md](../task-manager.md) §6.1). Independent branches execute in parallel; dependent steps wait for their inputs.

Cross-reference: [task-manager.md](../task-manager.md) §5.2 for `AgentSelector` integration with the Tool Registry.

---

## 16. Kernel-Internal ML

These features run as lightweight statistical models in kernel space — no AIRS dependency, no LLM inference. They provide optimization and security signals even when AIRS is unavailable.

### 16.1 Tool Call Anomaly Detection

A frozen decision tree model trained on historical tool call patterns detects anomalous behavior:

**Feature vector (per tool call):**

| Feature | Type | Description |
|---|---|---|
| `caller_id` | categorical | Which agent is calling |
| `tool_name` | categorical | Which tool is being called |
| `hour_of_day` | continuous | When the call occurs |
| `calls_last_minute` | continuous | Recent call rate from this caller |
| `novel_tool` | boolean | Has this caller ever called this tool before? |
| `param_size_bytes` | continuous | Parameter payload size |
| `caller_trust_level` | ordinal | TL0–TL3 |

**Model output:** Anomaly score (0.0–1.0). Scores above a configurable threshold (default: 0.8) trigger:
- **0.8–0.9:** Log warning, continue with call
- **0.9–0.95:** Add to Intent Verifier queue for async review
- **0.95+:** Block call, require synchronous Intent Verification

**Training:** The model is trained offline on historical tool call logs (collected during normal operation). The frozen decision tree is compiled to a lookup table — no floating-point inference in kernel space. Updated via system updates (A/B partition swap).

**Example anomalies detected:**
- An agent that normally calls `read-file` 5 times/day suddenly calls it 500 times in a minute
- An agent that has never called `send-email` starts calling it (novel tool access pattern)
- Tool calls at unusual hours from automated agents (potential compromise)

### 16.2 Latency Prediction

A simple linear regression model predicts tool call latency based on observable features:

**Input features:**

| Feature | Correlation |
|---|---|
| Provider CPU load (last 100ms) | Strong positive |
| Parameter payload size | Moderate positive |
| Provider's in-flight tool calls | Strong positive |
| Tool's historical p50 latency | Baseline |
| Provider's runtime type | Categorical offset (WASM ~20% slower than native Rust) |

**Output:** Predicted latency in microseconds.

**Use cases:**
- **Task Manager scheduling:** When deciding between two providers for the same tool, the Task Manager uses predicted latency to select the faster one
- **Timeout calibration:** The predicted latency informs the automatic timeout assignment (predicted × 3 = suggested timeout)
- **Preemptive warning:** If predicted latency exceeds the caller's deadline, the Tool Manager can return early with a `WouldTimeout` advisory

### 16.3 Tool Affinity Learning

A frequency-based model tracks which agents call which tools, enabling optimization:

```rust
pub struct ToolAffinityTable {
    /// (caller_agent, tool_id) → call count in current window
    affinity: HashMap<(AgentId, ToolId), u32>,
    /// Window duration (seconds)
    window_seconds: u32,
    /// Minimum calls to establish affinity
    threshold: u32,
}
```

**Optimization actions:**
- **IPC channel pre-warming:** If Agent A has strong affinity for Agent B's `pdf-extract` tool, pre-establish the IPC channel at Agent A startup (avoids first-call latency)
- **Provider keep-alive:** If a tool has active affinities, the service manager keeps the provider agent alive even during memory pressure (avoids restart-on-next-call)
- **Registry caching:** Frequently-queried tool entries are cached in a hot path (bypass HashMap lookup)

---

## 17. Future Directions

### 17.1 Tool Marketplace

A curated registry of third-party tools, installable through the agent marketplace:

- Browse, search, and install tool-providing agents
- Ratings, reviews, and download counts for community tools
- Automatic capability review during installation (see [agents.md](../../applications/agents.md) §3.1)
- Publisher verification and code signing for verified (TL2) tools

### 17.2 Tool Composition Language

A declarative DSL for defining tool pipelines without writing agent code:

```yaml
pipeline: research-summary
steps:
  - tool: web-search
    params: { query: "{{input.topic}}" }
    output: search_results
  - tool: pdf-extract
    foreach: search_results.documents
    params: { object_id: "{{item.id}}" }
    output: extracted_texts
  - tool: summarize
    params: { texts: "{{extracted_texts}}", max_words: 500 }
    output: summary
```

Pipelines would be compiled to Task Manager DAGs, inheriting all capability enforcement and sandboxing guarantees.

### 17.3 Federated Tool Discovery

Cross-device tool discovery via the multi-device sync protocol:

- Devices in a user's personal mesh advertise their registered tools
- A laptop agent can call a tool on the user's phone (e.g., camera capture)
- Tool calls cross device boundaries through the Space Sync protocol ([sync.md](../../storage/spaces/sync.md))
- Capability delegation spans devices (user explicitly grants cross-device tool access)

Cross-reference: [multi-device.md](../../platform/multi-device.md) for device pairing and trust.

### 17.4 Tool Versioning with Auto-Migration

Automatic parameter transformation between tool versions:

- When a tool bumps a major version with breaking schema changes, the provider can register a migration function
- The migration function transforms old-format parameters to new-format parameters
- Callers using the old version transparently get their calls migrated
- Migration functions are themselves tools (meta-tools), subject to the same validation

### 17.5 MCP Ecosystem Integration

Full bidirectional MCP support enabling AIOS to participate in the broader AI tool ecosystem:

- **MCP server mode:** AIOS exposes its registered tools as a standards-compliant MCP server, allowing Claude Desktop, VS Code, and other MCP clients to use AIOS tools
- **MCP client mode:** AIOS agents can connect to any MCP server and use its tools natively
- **MCP resource bridge:** MCP resources mapped to AIOS Space Storage objects
- **MCP prompt bridge:** MCP prompts mapped to AIOS Conversation Manager templates
- **MCP sampling bridge:** MCP sampling requests routed to AIRS inference engine

### 17.6 Speculative Tool Execution

Based on tool affinity patterns and user context prediction:

- AIRS predicts the next likely tool call based on current conversation context
- The predicted call is executed speculatively in a sandboxed environment
- If the prediction is correct, the result is returned instantly (zero-latency tool call)
- If incorrect, the speculative result is discarded (wasted compute, but no correctness impact)
- Speculative execution only applies to idempotent, read-only tools

### 17.7 Formal Verification of Tool Call Security

Apply formal methods to verify key security properties of the tool call pipeline:

- **Property 1:** No tool call can succeed without a valid capability chain (verified by capability system invariants)
- **Property 2:** Provider crash cannot corrupt caller state (verified by IPC isolation proofs)
- **Property 3:** Timeout enforcement is deadlock-free (verified by the deadlock prevention framework)

Cross-reference: [static-analysis.md](../../security/static-analysis.md) for formal verification approaches used in AIOS.
