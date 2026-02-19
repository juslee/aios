# AIOS Task Manager

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [agents.md](../applications/agents.md) — Agent framework and task agents, [airs.md](./airs.md) — AI Runtime Service (inference, intent verification), [context-engine.md](./context-engine.md) — Context-aware prioritization, [boot-lifecycle.md](../kernel/boot-lifecycle.md) — Boot phases and service startup

-----

## 1. Overview

Traditional operating systems have no concept of user intent. The user opens programs, moves files, switches windows. The OS manages processes and memory — it has no idea why. When the user says "organize my photos from the trip," they must manually open an image editor, a file browser, and perhaps a metadata tool, then perform dozens of individual operations that together accomplish the goal. The OS sees thirty file renames and ten program launches. It does not see "organize photos."

AIOS introduces the Task Manager — a system service that bridges the gap between user intent and agent execution. When a user expresses an intent (through the Conversation Bar, a keyboard shortcut, or a long-press context action), the Task Manager decomposes that intent into a structured graph of subtasks, spawns task agents to execute each subtask, orchestrates their coordination, monitors their progress, and reports completion back to the user.

**The Task Manager is not a process list.** It does not display running PIDs or memory usage (that is the Inspector's job). It manages *goals* — what the user wants to accomplish — and coordinates the agents that accomplish them.

**The Task Manager is not AIRS.** AIRS provides the inference engine — it understands natural language, generates embeddings, and verifies intent. The Task Manager *uses* AIRS to decompose intents, but it owns the task lifecycle: creation, scheduling, monitoring, retry, and completion.

**The Task Manager is not the Agent Runtime.** The Agent Runtime manages process isolation, capability enforcement, and sandbox security. The Task Manager tells the Agent Runtime *which* agents to spawn and *what* capabilities they need, then monitors the task-level outcome.

```
User: "Summarize all the research papers I read this week and
       email the summary to my team"

Without Task Manager:
  User must: open paper space, search by date, open each paper,
  copy text, open AI chat, paste text, request summary, copy
  summary, open email, compose message, paste, send. ~15 minutes.

With Task Manager:
  1. User speaks/types the intent
  2. Task Manager decomposes into subtasks:
     a. Query space "research" for papers accessed this week
     b. For each paper: extract key findings (inference)
     c. Synthesize findings into a unified summary (inference)
     d. Compose email with summary to "team" contact group
     e. Send via email connector (requires user confirmation)
  3. Task agents execute steps a-d autonomously
  4. Step e pauses for user confirmation (send action)
  5. User reviews summary, confirms send
  6. Task complete. ~30 seconds of user time.
```

-----

## 2. Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                       Task Manager                                │
│                   (privileged userspace service)                   │
│                                                                   │
│  ┌───────────────────────────────────────────────────────────┐   │
│  │              Intent Decomposer                             │   │
│  │                                                           │   │
│  │  Intent Parser       Task Graph Builder    Capability Planner│  │
│  │  (NL → structured)   (subtask DAG)         (min caps per     │  │
│  │                                             subtask)         │  │
│  └───────────────────────────────────────────────────────────┘   │
│                                                                   │
│  ┌───────────────────────────────────────────────────────────┐   │
│  │              Task Scheduler                                │   │
│  │                                                           │   │
│  │  DAG Executor        Concurrency Mgr     Priority Router  │   │
│  │  (topological        (parallel where      (context-aware   │  │
│  │   order exec)         safe, serial         scheduling)      │  │
│  │                       where required)                       │  │
│  └───────────────────────────────────────────────────────────┘   │
│                                                                   │
│  ┌───────────────────────────────────────────────────────────┐   │
│  │              Agent Orchestrator                             │   │
│  │                                                           │   │
│  │  Agent Selector      Spawn Manager       Result Collector │   │
│  │  (task → agent       (capability          (aggregate       │  │
│  │   matching)           scoping, spawn)      subtask results) │  │
│  └───────────────────────────────────────────────────────────┘   │
│                                                                   │
│  ┌───────────────────────────────────────────────────────────┐   │
│  │              Task State Store                              │   │
│  │                                                           │   │
│  │  In-Flight State     Progress Tracker    Audit Logger     │   │
│  │  (volatile, per-     (% complete,         (provenance      │  │
│  │   task session)       subtask status)      chain entries)   │  │
│  └───────────────────────────────────────────────────────────┘   │
│                                                                   │
└─────────────┬──────────────┬────────────────┬─────────────────────┘
              │              │                │
              ▼              ▼                ▼
         AIRS (IPC)     Agent Runtime    Space Storage
         (intent        (spawn agents,   (task state,
          decompose,     enforce caps,    object access,
          inference)     lifecycle)       audit log)
```

### 2.1 Relationship to Other Services

The Task Manager sits between the user's intent and the system's execution machinery. It does not duplicate functionality — it coordinates it.

```
┌──────────────────────────────────────────────────────────┐
│  User                                                      │
│  "Summarize my research papers and email the summary"     │
└──────────────┬─────────────────────────────────────────────┘
               │ natural language intent
               ▼
┌──────────────────────────────────────────────────────────┐
│  Conversation Bar / Conversation Manager                   │
│  Recognizes this as a task request (not a question)       │
│  Routes to Task Manager via IPC                           │
└──────────────┬─────────────────────────────────────────────┘
               │ structured intent
               ▼
┌──────────────────────────────────────────────────────────┐
│  Task Manager                                              │
│  1. Asks AIRS to decompose intent → task graph             │
│  2. Plans capabilities needed per subtask                  │
│  3. Asks Agent Runtime to spawn task agents                │
│  4. Monitors execution, handles failures                   │
│  5. Collects results, reports to user                      │
└──────────────┬──────────┬───────────────┬──────────────────┘
               │          │               │
          ┌────┘    ┌─────┘         ┌─────┘
          ▼         ▼               ▼
      ┌────────┐ ┌───────────┐ ┌──────────────┐
      │  AIRS  │ │Agent      │ │Space Storage │
      │        │ │Runtime    │ │              │
      │ decomp,│ │ spawn,    │ │ read/write   │
      │ infer, │ │ sandbox,  │ │ objects,     │
      │ verify │ │ lifecycle │ │ task state   │
      └────────┘ └───────────┘ └──────────────┘
```

**AIRS** provides:
- Intent decomposition: parsing natural language into a structured task graph
- Inference: subtasks that require LLM reasoning (summarization, classification, generation)
- Intent verification: ensuring spawned agents act within the declared task scope

**Agent Runtime** provides:
- Process isolation: each task agent runs in its own address space (TTBR0)
- Capability enforcement: task agents receive only the capabilities their subtask requires
- Lifecycle management: spawn, pause, suspend, terminate

**Space Storage** provides:
- Object access: task agents read from and write to spaces
- Task state: the Task struct is stored as a space object in `system/tasks/`
- Audit: all task actions are logged to the provenance chain

**Context Engine** provides:
- Priority hints: deep work context may boost task priority; leisure may deprioritize background tasks
- Resource scheduling: context-aware compute allocation for task agent inference requests

-----

## 3. Intent Decomposition

### 3.1 From Natural Language to Task Graph

When the user expresses an intent, the Task Manager asks AIRS to decompose it into a structured task graph. This is the core intelligence step — transforming a fuzzy human goal into a concrete execution plan.

```rust
pub struct Intent {
    /// Raw natural language from the user
    raw: String,
    /// Structured representation (produced by AIRS)
    parsed: Option<ParsedIntent>,
    /// Source of the intent
    source: IntentSource,
    /// Timestamp
    created_at: Timestamp,
}

pub enum IntentSource {
    /// User typed or spoke into the Conversation Bar
    ConversationBar,
    /// User invoked a context action (long-press, right-click)
    ContextAction { object: ObjectId, action: String },
    /// User triggered a keyboard shortcut mapped to a task
    KeyboardShortcut { shortcut: String },
    /// Another agent requested task creation (requires user approval)
    AgentRequest { agent: AgentId },
}

pub struct ParsedIntent {
    /// What the user wants to achieve (high-level summary)
    goal: String,
    /// What spaces are involved
    target_spaces: Vec<SpaceId>,
    /// What objects are involved (if specific)
    target_objects: Vec<ObjectId>,
    /// What actions are required
    actions: Vec<IntentAction>,
    /// Estimated complexity (affects decomposition strategy)
    complexity: IntentComplexity,
    /// Confidence score from AIRS parsing (0.0-1.0)
    confidence: f32,
}

pub enum IntentAction {
    Query,          // search/find objects
    Read,           // read object content
    Summarize,      // generate summary (inference)
    Transform,      // modify content (inference)
    Create,         // create new object
    Send,           // communicate externally (email, message)
    Organize,       // move, tag, relate objects
    Delete,         // remove objects (always requires confirmation)
}

pub enum IntentComplexity {
    /// Single action, single agent — "rename this file"
    Trivial,
    /// Few steps, one agent — "summarize this document"
    Simple,
    /// Multiple steps, possibly multiple agents — "organize my photos"
    Moderate,
    /// Many steps, multiple agents, external services — "research and report"
    Complex,
}
```

### 3.2 Task Graph Construction

AIRS decomposes the parsed intent into a directed acyclic graph (DAG) of subtasks. Edges represent data dependencies — a subtask cannot execute until all its predecessors have completed and their outputs are available.

```rust
pub struct TaskGraph {
    /// Root task (the user's intent)
    root: TaskId,
    /// All subtasks in the graph
    subtasks: HashMap<TaskId, SubTask>,
    /// Dependency edges: (predecessor, successor)
    edges: Vec<(TaskId, TaskId)>,
}

pub struct SubTask {
    id: TaskId,
    /// What this subtask does
    description: String,
    /// What type of work
    action: SubTaskAction,
    /// What capabilities the executing agent needs
    required_capabilities: Vec<Capability>,
    /// Input data (output from predecessor subtasks, or space objects)
    inputs: Vec<SubTaskInput>,
    /// Expected output type
    output_type: OutputType,
    /// Whether this subtask needs user confirmation before executing
    requires_confirmation: bool,
    /// Estimated duration (from AIRS, used for progress reporting)
    estimated_duration: Option<Duration>,
    /// Current state
    state: SubTaskState,
}

pub enum SubTaskAction {
    /// Query a space for matching objects
    SpaceQuery { space: SpaceId, query: String },
    /// Read object content
    SpaceRead { space: SpaceId, object: ObjectId },
    /// Write or create an object
    SpaceWrite { space: SpaceId, content_type: ContentType },
    /// Run AIRS inference (summarize, generate, classify)
    Inference { task_type: InferenceTaskType, prompt_template: String },
    /// Call a registered tool on another agent
    ToolCall { tool_name: String, params: Value },
    /// Send data through a connector (email, chat, etc.)
    ConnectorSend { connector: String, destination: String },
    /// User confirmation gate (execution pauses here)
    UserConfirmation { prompt: String },
}

pub enum SubTaskInput {
    /// Output from a predecessor subtask
    FromSubTask(TaskId),
    /// Object from a space
    FromSpace { space: SpaceId, object: ObjectId },
    /// Literal value (e.g., user-provided text)
    Literal(Value),
}

pub enum OutputType {
    /// Text content (summary, email body, etc.)
    Text,
    /// Structured data (JSON, list, table)
    Structured,
    /// Space object reference (created or modified object)
    ObjectRef,
    /// Boolean (confirmation gate result)
    Boolean,
    /// No output (side-effect only)
    Void,
}
```

### 3.3 Decomposition Example

```
User intent: "Summarize all the research papers I read this week
              and email the summary to my team"

AIRS decomposition:

  ┌─────────────────────────┐
  │  T1: SpaceQuery         │
  │  space: "research"      │
  │  query: "accessed this  │
  │    week by user"        │
  │  caps: ReadSpace        │
  └───────────┬─────────────┘
              │ outputs: Vec<ObjectId>
              ▼
  ┌─────────────────────────┐
  │  T2: Inference          │  (runs once per paper, parallelized)
  │  type: Summarize        │
  │  input: T1 objects      │
  │  prompt: "Extract key   │
  │    findings from this   │
  │    research paper"      │
  │  caps: ReadSpace,       │
  │        InferenceCpu     │
  └───────────┬─────────────┘
              │ outputs: Vec<String> (per-paper summaries)
              ▼
  ┌─────────────────────────┐
  │  T3: Inference          │
  │  type: Synthesize       │
  │  input: T2 summaries    │
  │  prompt: "Combine these │
  │    paper summaries into │
  │    a unified briefing"  │
  │  caps: InferenceCpu     │
  └───────────┬─────────────┘
              │ output: String (unified summary)
              ▼
  ┌─────────────────────────┐
  │  T4: Inference          │
  │  type: Generate         │
  │  input: T3 summary      │
  │  prompt: "Compose a     │
  │    professional email   │
  │    with this summary"   │
  │  caps: InferenceCpu     │
  └───────────┬─────────────┘
              │ output: String (email body)
              ▼
  ┌─────────────────────────┐
  │  T5: UserConfirmation   │
  │  prompt: "Send this     │
  │    summary email to     │
  │    your team?"          │
  │  [shows email preview]  │
  └───────────┬─────────────┘
              │ output: Boolean (approved/rejected)
              ▼
  ┌─────────────────────────┐
  │  T6: ConnectorSend      │
  │  connector: "email"     │
  │  destination: "team"    │
  │  input: T4 email body   │
  │  caps: Network("smtp")  │
  └─────────────────────────┘
```

### 3.4 Decomposition Without AIRS

If AIRS is unavailable (model not loaded, inference engine busy), the Task Manager cannot decompose complex intents. It falls back to:

1. **Direct action intents:** Simple intents that map to a single agent action ("open this document," "play this song") are handled by the Agent Runtime directly, without decomposition. The Task Manager is not involved.

2. **Known task templates:** The Task Manager ships with a library of pre-built task templates for common intents (summarize document, organize photos by date, export space to archive). These templates are static task graphs — no AIRS decomposition needed. They cover the most common use cases with fixed structure.

3. **Deferred execution:** For complex intents that require AIRS decomposition, the Task Manager queues the intent and notifies the user: "I'll start this task when the AI model is ready." When AIRS becomes available, the queued intent is decomposed and executed.

```rust
pub struct TaskTemplateLibrary {
    templates: HashMap<String, TaskTemplate>,
}

pub struct TaskTemplate {
    /// Pattern to match against parsed intent
    pattern: IntentPattern,
    /// Pre-built task graph (no AIRS decomposition needed)
    graph: TaskGraph,
    /// Required capabilities
    capabilities: Vec<Capability>,
}

pub enum IntentPattern {
    /// Exact action on a specific content type
    Exact { action: IntentAction, content_type: ContentType },
    /// Keyword-based matching
    Keywords { keywords: Vec<String>, action: IntentAction },
}
```

-----

## 4. Task Lifecycle

### 4.1 Task States

```
                          ┌───────────┐
                          │  Created  │
                          └─────┬─────┘
                                │ decomposition complete
                                ▼
                          ┌───────────┐
                    ┌─────│  Pending  │
                    │     └─────┬─────┘
                    │           │ agent(s) spawned
              user  │           ▼
            cancels │     ┌───────────┐
                    │     │  Running  │◄──────────────┐
                    │     └──┬──┬──┬──┘               │
                    │        │  │  │                   │
                    │        │  │  ├── subtask fails   │
                    │        │  │  │   (retryable)     │
                    │        │  │  │        │          │
                    │        │  │  │        ▼          │
                    │        │  │  │  ┌──────────┐    │
                    │        │  │  │  │ Retrying │────┘
                    │        │  │  │  └──────────┘
                    │        │  │  │
                    │        │  │  └── waiting for
                    │        │  │      user confirmation
                    │        │  │           │
                    │        │  │           ▼
                    │        │  │     ┌───────────┐
                    │        │  │     │  Paused   │───────┘
                    │        │  │     └───────────┘  (user confirms
                    │        │  │                     or rejects)
                    │        │  │
                    │        │  └── all subtasks complete
                    │        │           │
                    │        │           ▼
                    │        │     ┌───────────┐
                    │        │     │ Completed │
                    │        │     └───────────┘
                    │        │
                    │        └── unrecoverable failure
                    │                 │
                    ▼                 ▼
              ┌───────────┐   ┌───────────┐
              │ Cancelled │   │  Failed   │
              └───────────┘   └───────────┘
```

```rust
pub struct Task {
    /// Unique task identifier
    id: TaskId,
    /// The user's original intent
    intent: Intent,
    /// Decomposed task graph
    graph: TaskGraph,
    /// Current state
    state: TaskState,
    /// Task agents currently executing subtasks
    agents: Vec<AgentId>,
    /// Capability set allocated to this task
    capabilities: CapabilitySet,
    /// Activity log (user-visible progress)
    activity_log: Vec<ActivityEntry>,
    /// Child tasks (if this task spawned sub-tasks)
    children: Vec<TaskId>,
    /// Persistence mode
    persistence: Persistence,
    /// Link to context at creation time
    context: ContextLink,
    /// When the task was created
    created_at: Timestamp,
    /// When the task last changed state
    updated_at: Timestamp,
    /// Priority (derived from context and user interaction)
    priority: TaskPriority,
}

pub enum TaskState {
    /// Intent received, decomposition in progress
    Created,
    /// Decomposed, waiting for agent availability
    Pending,
    /// Subtasks executing
    Running { progress: TaskProgress },
    /// Waiting for user confirmation on a subtask
    Paused { waiting_on: TaskId, prompt: String },
    /// Retrying a failed subtask
    Retrying { subtask: TaskId, attempt: u32 },
    /// All subtasks complete
    Completed { result: TaskResult },
    /// User cancelled
    Cancelled,
    /// Unrecoverable failure
    Failed { error: TaskError },
}

pub enum SubTaskState {
    /// Not yet started (predecessors incomplete)
    Blocked,
    /// Predecessors complete, ready to execute
    Ready,
    /// Agent spawned, executing
    Running { agent: AgentId },
    /// Completed successfully
    Completed { output: Value },
    /// Failed (may be retried)
    Failed { error: SubTaskError, retries: u32 },
    /// Skipped (predecessor failed, this subtask is unreachable)
    Skipped,
}

pub struct TaskProgress {
    /// Total subtasks in the graph
    total: u32,
    /// Subtasks completed
    completed: u32,
    /// Subtasks currently running
    running: u32,
    /// Subtasks blocked (waiting on predecessors)
    blocked: u32,
    /// Estimated time remaining (if AIRS provided duration estimates)
    estimated_remaining: Option<Duration>,
}

pub enum TaskPriority {
    /// User is actively waiting for this result
    Interactive,
    /// User initiated but is doing other things
    Background,
    /// System-initiated task (e.g., scheduled maintenance)
    System,
}

pub enum Persistence {
    /// Task agent state is volatile — lost on crash or reboot
    Ephemeral,
    /// Task metadata (intent, progress) is persisted to space
    /// but in-flight agent state is volatile
    MetadataPersisted,
}

pub struct TaskResult {
    /// Summary of what was accomplished
    summary: String,
    /// Objects created or modified
    affected_objects: Vec<ObjectId>,
    /// Total execution time
    duration: Duration,
}

pub struct TaskError {
    /// Which subtask failed
    subtask: TaskId,
    /// What went wrong
    error: SubTaskError,
    /// How many retries were attempted
    retries_attempted: u32,
    /// What was accomplished before failure
    partial_result: Option<TaskResult>,
}

pub enum SubTaskError {
    /// AIRS inference failed (model unavailable, timeout)
    InferenceFailed(String),
    /// Space operation failed (object not found, permission denied)
    SpaceError(String),
    /// Agent crashed during execution
    AgentCrashed(AgentId),
    /// Tool call failed (target agent unavailable)
    ToolCallFailed { tool: String, reason: String },
    /// Connector failed (network error, auth error)
    ConnectorFailed { connector: String, reason: String },
    /// User rejected at confirmation gate
    UserRejected,
    /// Timeout (subtask exceeded estimated duration by 5x)
    Timeout,
}
```

### 4.2 Task Lifecycle Flow

```
1. Intent arrives (Conversation Bar, context action, keyboard shortcut)
     │
     ▼
2. Task Manager creates Task in Created state
   Stores to system/tasks/ space
     │
     ▼
3. Task Manager sends intent to AIRS for decomposition
   AIRS returns: ParsedIntent + TaskGraph
     │
     ├── AIRS unavailable → check template library → queue if no match
     │
     └── AIRS returns graph
           │
           ▼
4. Capability planning
   For each subtask, determine minimum capability set
   Verify user has approved these capabilities
   If new capabilities needed → prompt user
     │
     ▼
5. Task enters Pending state
   Task Manager schedules execution based on TaskPriority
     │
     ▼
6. DAG Executor walks the graph in topological order
   For each Ready subtask:
     a. Agent Selector finds or spawns an appropriate task agent
     b. Spawn Manager requests Agent Runtime to create the agent
        with the subtask's minimum capability set
     c. Task agent receives subtask description via IPC
     d. Task agent executes and reports result
     │
     ├── SubTask completes → mark successor subtasks as Ready
     │                         continue DAG walk
     │
     ├── SubTask fails (retryable) → retry up to 3 times
     │                                 exponential backoff
     │
     ├── SubTask fails (unrecoverable) → mark task Failed
     │                                     report to user
     │
     ├── SubTask is UserConfirmation → pause task
     │                                   present to user
     │                                   wait for approval/rejection
     │
     └── All subtasks complete → task enters Completed state
                                  report result to user
                                  terminate task agents
```

-----

## 5. Agent Orchestration

### 5.1 Task Agents

Task agents are ephemeral agents spawned specifically to execute one or more subtasks. Their lifetime is the task's lifetime. They differ from persistent agents in several ways:

| Property | Persistent Agent | Task Agent |
|---|---|---|
| Lifetime | Indefinite, survives reboots | Task duration only |
| State persistence | Saves to spaces, restores on reboot | Volatile — lost on crash/reboot |
| Capabilities | Full set from manifest and user approval | Minimal set per subtask only |
| User installation | Requires manifest review, user approval | Spawned transparently by Task Manager |
| Visibility | Listed in Agent Runtime, visible in Inspector | Shown in task progress UI |
| Reboot behavior | Relaunched by Agent Runtime | NOT relaunched — task is lost |

```rust
pub struct TaskAgentConfig {
    /// Which subtask(s) this agent will execute
    subtasks: Vec<TaskId>,
    /// Minimum capabilities for these subtasks
    capabilities: Vec<Capability>,
    /// Memory limit (conservative — task agents are short-lived)
    memory_limit: usize,
    /// CPU priority (derived from task priority)
    cpu_priority: SchedulingPriority,
    /// Parent task (for audit linkage)
    task: TaskId,
    /// Maximum lifetime (hard timeout — prevents runaway agents)
    max_lifetime: Duration,
}
```

### 5.2 Agent Selection

When a subtask is ready to execute, the Task Manager must decide which agent runs it. Three strategies, tried in order:

**1. Reuse an existing task agent.** If a task agent from the same task is idle and already holds the necessary capabilities, reuse it. This avoids spawn overhead and keeps the agent count low.

**2. Spawn a new task agent from a known manifest.** For common subtask types (space query, inference, file transform), the Task Manager has built-in agent manifests optimized for single-purpose execution. These are lightweight agents (~2 MB memory, <20ms startup) that execute one subtask and exit.

**3. Delegate to a registered tool.** If a third-party agent has registered a tool that matches the subtask (e.g., a PDF parser agent registered `pdf-extract`), the Task Manager delegates via the Tool Manager. The third-party agent runs the subtask in its own process; the Task Manager collects the result.

```rust
pub struct AgentSelector {
    /// Built-in agent manifests for common subtask types
    builtin_manifests: HashMap<SubTaskAction, AgentManifest>,
    /// Tool registry (from AIRS Tool Manager)
    tool_registry: ToolRegistry,
}

impl AgentSelector {
    pub fn select(
        &self,
        subtask: &SubTask,
        active_agents: &[AgentId],
    ) -> AgentSelection {
        // Strategy 1: reuse existing idle agent with matching capabilities
        for agent in active_agents {
            if self.agent_can_handle(agent, subtask) {
                return AgentSelection::Reuse(*agent);
            }
        }

        // Strategy 2: check if a registered tool matches
        if let Some(tool) = self.tool_registry.find_for_action(&subtask.action) {
            return AgentSelection::ToolCall {
                tool_name: tool.name.clone(),
                provider: tool.provider,
            };
        }

        // Strategy 3: spawn a built-in task agent
        let manifest = self.builtin_manifests
            .get(&subtask.action)
            .expect("built-in manifest exists for every SubTaskAction variant");
        AgentSelection::Spawn {
            manifest: manifest.clone(),
            capabilities: subtask.required_capabilities.clone(),
        }
    }
}

pub enum AgentSelection {
    /// Reuse an existing task agent
    Reuse(AgentId),
    /// Delegate to a tool on another agent
    ToolCall { tool_name: String, provider: AgentId },
    /// Spawn a new task agent
    Spawn { manifest: AgentManifest, capabilities: Vec<Capability> },
}
```

### 5.3 Capability Scoping

The Task Manager enforces the principle of least privilege for task agents. Each task agent receives only the capabilities needed for its specific subtask — not the full set the user has approved for the task.

```
Task: "Summarize papers and email the summary"

User-approved capabilities for this task:
  ReadSpace("research"), InferenceCpu, Network("smtp")

Subtask T1 (SpaceQuery) agent receives:
  ReadSpace("research")
  — no inference, no network

Subtask T2 (Inference) agent receives:
  ReadSpace("research"), InferenceCpu
  — no network

Subtask T6 (ConnectorSend) agent receives:
  Network("smtp")
  — no space read, no inference

Each agent has the minimum capability surface for attack.
A compromised T2 agent cannot send email.
A compromised T6 agent cannot read research papers.
```

```rust
pub struct CapabilityPlanner {
    /// Maps subtask actions to required capability types
    action_caps: HashMap<SubTaskAction, Vec<CapabilityType>>,
}

impl CapabilityPlanner {
    /// Compute the minimum capability set for a subtask
    pub fn plan_capabilities(
        &self,
        subtask: &SubTask,
        task_caps: &CapabilitySet,
    ) -> Result<Vec<Capability>> {
        let required_types = self.action_caps.get(&subtask.action)
            .ok_or(TaskError::UnknownAction)?;

        let mut caps = Vec::new();
        for cap_type in required_types {
            // Find matching capability from the task's approved set
            let cap = task_caps.find_by_type(cap_type)
                .ok_or(TaskError::MissingCapability(cap_type.clone()))?;
            // Attenuate: restrict to only the resources this subtask needs
            let scoped = cap.attenuate(&subtask.scope())?;
            caps.push(scoped);
        }
        Ok(caps)
    }
}
```

-----

## 6. Task Scheduling

### 6.1 DAG Execution

The Task Scheduler executes subtasks in topological order. Subtasks with no unmet dependencies execute concurrently when the DAG structure permits.

```rust
pub struct DagExecutor {
    graph: TaskGraph,
    /// Subtasks with all predecessors complete
    ready_queue: VecDeque<TaskId>,
    /// Currently executing subtasks
    in_flight: HashMap<TaskId, AgentId>,
    /// Maximum concurrent subtasks (based on available resources)
    max_concurrency: usize,
}

impl DagExecutor {
    /// Advance execution: start ready subtasks, collect results
    pub async fn step(&mut self, orchestrator: &mut AgentOrchestrator) -> StepResult {
        // Collect completed subtasks
        let completed = orchestrator.poll_completions().await;
        for (subtask_id, result) in completed {
            self.in_flight.remove(&subtask_id);
            self.mark_completed(subtask_id, result);
            // Check if successors are now ready
            for successor in self.graph.successors(subtask_id) {
                if self.all_predecessors_complete(successor) {
                    self.ready_queue.push_back(successor);
                }
            }
        }

        // Start new subtasks up to concurrency limit
        while self.in_flight.len() < self.max_concurrency {
            match self.ready_queue.pop_front() {
                Some(subtask_id) => {
                    let agent = orchestrator.execute(subtask_id).await?;
                    self.in_flight.insert(subtask_id, agent);
                }
                None => break,
            }
        }

        // Check termination
        if self.in_flight.is_empty() && self.ready_queue.is_empty() {
            if self.all_subtasks_complete() {
                StepResult::TaskComplete
            } else {
                StepResult::TaskFailed
            }
        } else {
            StepResult::InProgress
        }
    }
}
```

### 6.2 Concurrency Limits

The Task Manager limits concurrent subtask execution based on system resources and context:

```rust
pub struct ConcurrencyPolicy {
    /// Maximum concurrent task agents across all tasks
    global_max: usize,                  // default: 4
    /// Maximum concurrent task agents per task
    per_task_max: usize,                // default: 2
    /// Context-aware adjustment
    context_adjustment: ContextAdjustment,
}

pub struct ContextAdjustment {
    /// During deep work (work_engagement > 0.8): reduce task concurrency
    /// to avoid competing with the user's foreground agents
    deep_work_limit: usize,            // default: 1
    /// During idle (no user input for 5 min): increase task concurrency
    idle_limit: usize,                 // default: 4
    /// During leisure: reduce to minimum
    leisure_limit: usize,              // default: 1
}
```

### 6.3 Priority and Preemption

Interactive tasks (user is waiting) take priority over background tasks. When an interactive task arrives while background tasks are running:

1. Background task agents are **not** terminated — they are deprioritized in the scheduler.
2. Interactive task agents get `Interactive` scheduling priority.
3. Inference requests from interactive tasks preempt background inference (AIRS scheduling).
4. If memory pressure is critical, background task agents may be suspended (state lost — task will need to restart later).

-----

## 7. Error Handling and Retry

### 7.1 Subtask Failure

When a subtask fails, the Task Manager applies a retry policy before marking the task as failed:

```rust
pub struct RetryPolicy {
    /// Maximum retry attempts per subtask
    max_retries: u32,                   // default: 3
    /// Backoff between retries
    backoff: BackoffStrategy,
    /// Which errors are retryable
    retryable: Vec<SubTaskErrorType>,
}

pub enum BackoffStrategy {
    /// Fixed delay between retries
    Fixed(Duration),
    /// Exponential: delay * 2^attempt (capped at max_delay)
    Exponential { initial: Duration, max: Duration },
}

impl RetryPolicy {
    pub fn should_retry(&self, error: &SubTaskError, attempt: u32) -> bool {
        if attempt >= self.max_retries {
            return false;
        }
        match error {
            // Retryable: transient failures
            SubTaskError::InferenceFailed(_) => true,   // model may be loading
            SubTaskError::AgentCrashed(_) => true,      // respawn agent
            SubTaskError::ConnectorFailed { .. } => true, // network may recover
            SubTaskError::Timeout => true,              // may succeed with more time

            // Not retryable: permanent failures
            SubTaskError::SpaceError(_) => false,       // object not found, perm denied
            SubTaskError::UserRejected => false,        // user said no
            SubTaskError::ToolCallFailed { .. } => false, // tool agent unavailable
        }
    }

    pub fn retry_delay(&self, attempt: u32) -> Duration {
        match &self.backoff {
            BackoffStrategy::Fixed(d) => *d,
            BackoffStrategy::Exponential { initial, max } => {
                let delay = *initial * 2u32.pow(attempt);
                delay.min(*max)
            }
        }
    }
}
```

### 7.2 Partial Results

When a task fails, the Task Manager preserves partial results. If subtasks T1-T3 completed but T4 failed, the outputs from T1-T3 are still available. The user sees:

```
Task: "Summarize papers and email summary"
Status: Failed at step 4 of 6 (compose email)
Completed:
  ✓ Found 8 papers from this week
  ✓ Extracted key findings from each paper
  ✓ Generated unified summary
Failed:
  ✗ Compose email — inference timeout (AIRS busy)

[Retry] [View partial summary] [Cancel]
```

The user can retry (Task Manager restarts from the failed subtask, using cached outputs from completed subtasks), view what was accomplished, or cancel.

### 7.3 Task Agent Crashes

Task agents are ephemeral processes. If one crashes:

1. The Agent Runtime reports the crash to the Task Manager via IPC.
2. The Task Manager marks the subtask as `Failed(AgentCrashed)`.
3. The retry policy spawns a new agent and re-executes the subtask.
4. The new agent starts from scratch — there is no in-flight state to recover. This is acceptable because subtasks are designed to be idempotent: re-running a summarization produces the same result.

```rust
impl TaskManager {
    async fn handle_agent_crash(&mut self, agent_id: AgentId) {
        // Find which subtask this agent was running
        if let Some(subtask_id) = self.find_subtask_for_agent(agent_id) {
            let subtask = self.graph.subtask_mut(subtask_id);
            let attempt = subtask.retries();

            if self.retry_policy.should_retry(
                &SubTaskError::AgentCrashed(agent_id),
                attempt,
            ) {
                // Retry: spawn new agent, re-execute subtask
                subtask.set_state(SubTaskState::Failed {
                    error: SubTaskError::AgentCrashed(agent_id),
                    retries: attempt + 1,
                });
                let delay = self.retry_policy.retry_delay(attempt);
                self.schedule_retry(subtask_id, delay).await;
            } else {
                // Max retries exceeded — fail the task
                self.fail_task(subtask_id, SubTaskError::AgentCrashed(agent_id)).await;
            }
        }
    }
}
```

-----

## 8. Task State Persistence

### 8.1 What Is Persisted

Task metadata — the intent, the decomposed graph, subtask states, and progress — is persisted to `system/tasks/` as a space object. This allows the Workspace to display active tasks, the Inspector to show task history, and the user to review what happened.

```rust
pub struct PersistedTaskState {
    /// Task identity and intent
    task_id: TaskId,
    intent: Intent,
    /// Task graph (subtask descriptions and edges)
    graph: TaskGraph,
    /// Current state of each subtask
    subtask_states: HashMap<TaskId, SubTaskState>,
    /// Activity log
    activity_log: Vec<ActivityEntry>,
    /// Creation and completion timestamps
    created_at: Timestamp,
    completed_at: Option<Timestamp>,
    /// Final result (if completed)
    result: Option<TaskResult>,
    /// Final error (if failed)
    error: Option<TaskError>,
}
```

### 8.2 What Is NOT Persisted

Task agent in-flight state is **not** persisted. If the system crashes or reboots while a task is running:

- Task agents are terminated (they are ephemeral — see [agents.md](../applications/agents.md) Section 2.2 and 3.5).
- The task metadata in `system/tasks/` shows the last known subtask states.
- Completed subtask outputs that were written to spaces survive (spaces are persistent).
- In-flight subtask state (partial inference, buffered data) is lost.

**On next boot:** The Task Manager reads `system/tasks/` and finds tasks that were `Running` when the system went down. It marks them as `Failed` with error `SystemReboot`. The user sees:

```
Task: "Summarize papers and email summary"
Status: Interrupted by system reboot
Completed before reboot:
  ✓ Found 8 papers from this week
  ✓ Extracted findings from 5 of 8 papers
Interrupted:
  ✗ Findings extraction for 3 remaining papers (in progress at reboot)

[Restart task] [Resume from last checkpoint] [Dismiss]
```

"Resume from last checkpoint" re-runs only the incomplete subtasks, using cached outputs from subtasks that completed before the reboot. "Restart task" re-runs everything from scratch.

### 8.3 Design Rationale: Why Task Agents Are Ephemeral

Task agents are deliberately not persisted across reboots for three reasons:

1. **Correctness.** A half-completed inference has no meaningful state to restore. LLM generation is non-deterministic — restoring a partial token sequence would produce incoherent output. It is better to restart the subtask cleanly.

2. **Simplicity.** Persisting agent process state requires checkpointing address spaces, KV caches, and IPC channel state. This is complex, error-prone, and expensive. For short-lived task agents (typical lifetime: seconds to minutes), the cost exceeds the benefit.

3. **Security.** Ephemeral agents leave no residual state. After a task completes, the agent's address space is freed, its capabilities are revoked, and its memory is zeroed. There is no stale process holding capabilities it no longer needs.

-----

## 9. Boot Phase and Dependencies

### 9.1 Boot Phase

The Task Manager starts during **Phase 5 (Experience)** of the boot sequence. It depends on services from Phase 3 (AI Services) and Phase 4 (User Services) being available.

```
Phase 3: AI Services (non-critical path)
  ├── airs_core ──→ space_indexer
  │       │
  │       └──→ context_engine
  │
Phase 4: User Services (critical path continues)
  ├── identity_service
  │       │
  │       ▼
  │   preference_service
  │       │
  │       ├──→ attention_manager
  │       │
  │       └──→ agent_runtime
  │
Phase 5: Experience (starts when Phase 4 completes)
  ├── workspace ──→ conversation_bar
  │                      │
  │                      └──→ autostart_agents
  │
  └── task_manager  ←── NEW: starts here
         │
         ├── depends on: airs_core (for intent decomposition)
         ├── depends on: agent_runtime (for spawning task agents)
         ├── depends on: space_storage (for task state in system/tasks/)
         └── depends on: context_engine (for priority and scheduling)
```

### 9.2 Service Dependencies

| Dependency | Required? | What It Provides | Degraded Behavior Without |
|---|---|---|---|
| AIRS | Soft | Intent decomposition, inference for subtasks | Template-only decomposition; complex intents queued |
| Agent Runtime | Hard | Spawning and managing task agents | Task Manager cannot execute any tasks |
| Space Storage | Hard | Task state persistence, object access for subtasks | Task Manager cannot start |
| Context Engine | Soft | Priority hints, resource scheduling | All tasks run at default priority |
| Attention Manager | Soft | Task completion notifications | Task results shown only in Workspace |
| Conversation Manager | Soft | Receiving intents from Conversation Bar | Only context-action and shortcut intents work |

### 9.3 Startup Sequence

```rust
pub struct TaskManagerService {
    state_store: TaskStateStore,
    decomposer: IntentDecomposer,
    scheduler: TaskScheduler,
    orchestrator: AgentOrchestrator,
    template_library: TaskTemplateLibrary,
}

impl TaskManagerService {
    /// Called during Phase 5 boot
    pub async fn init(&mut self) -> Result<()> {
        // 1. Connect to Space Storage — load persisted task state
        self.state_store.connect("system/tasks/").await?;

        // 2. Load task template library from system/config/task-templates/
        self.template_library.load().await?;

        // 3. Connect to Agent Runtime — required for spawning task agents
        self.orchestrator.connect_agent_runtime().await?;

        // 4. Connect to AIRS — optional (may not be ready yet)
        match self.decomposer.connect_airs().await {
            Ok(_) => log::info!("Task Manager: AIRS connected, full decomposition available"),
            Err(_) => log::info!("Task Manager: AIRS not ready, using templates only"),
        }

        // 5. Connect to Context Engine — optional
        self.scheduler.connect_context_engine().await.ok();

        // 6. Recover interrupted tasks from previous session
        self.recover_interrupted_tasks().await;

        // 7. Register IPC endpoint at sys.tasks
        self.register_ipc("sys.tasks").await?;

        // 8. Ready to accept intents
        log::info!("Task Manager: ready");
        Ok(())
    }

    async fn recover_interrupted_tasks(&mut self) {
        let interrupted = self.state_store.find_running_tasks().await;
        for task in interrupted {
            // Mark as interrupted — user can restart or dismiss
            self.state_store.mark_interrupted(task.id).await;
            log::info!("Task {}: interrupted by reboot, marked for user review", task.id);
        }
    }
}
```

### 9.4 Late AIRS Connection

AIRS may not be ready when the Task Manager starts (Phase 5 does not wait for Phase 3). The Task Manager handles this gracefully:

1. On startup, attempt to connect to AIRS. If AIRS is not ready, proceed without it.
2. Register for AIRS health notifications. When AIRS becomes healthy, connect.
3. Any intents that arrived before AIRS was ready and could not be decomposed from templates are re-processed.

```rust
impl IntentDecomposer {
    /// Called when AIRS health status changes
    pub async fn on_airs_available(&mut self) {
        // Drain the deferred intent queue
        while let Some(intent) = self.deferred_queue.pop() {
            match self.decompose_with_airs(&intent).await {
                Ok(graph) => self.emit_task_ready(intent, graph),
                Err(e) => log::warn!("Deferred intent decomposition failed: {}", e),
            }
        }
    }
}
```

-----

## 10. SDK API for Agents

Agents interact with the Task Manager through the SDK's `AgentContext` trait. Three categories of operations are available.

### 10.1 Reading Task State

Agents with the `TaskRead` capability can query active tasks and their progress:

```rust
// Agent reads active tasks (e.g., Workspace displaying task list)
let tasks = ctx.tasks().list_active().await?;

for task in &tasks {
    println!("Task: {} — {}", task.intent.raw, task.state.summary());
    println!("  Progress: {}/{} subtasks", task.progress.completed, task.progress.total);
}

// Subscribe to task state changes (real-time updates)
let mut task_stream = ctx.tasks().subscribe().await?;
while let Some(update) = task_stream.next().await {
    match update {
        TaskUpdate::Progress { task_id, progress } => {
            ui.update_task_progress(task_id, progress);
        }
        TaskUpdate::Completed { task_id, result } => {
            ui.show_task_complete(task_id, result);
        }
        TaskUpdate::Failed { task_id, error } => {
            ui.show_task_failed(task_id, error);
        }
    }
}
```

### 10.2 Creating Tasks

Agents with the `TaskCreate` capability can request task creation. Agent-created tasks always require user approval before execution begins:

```rust
// An agent can request task creation on behalf of the user
// (e.g., a scheduling agent creating a "prepare meeting notes" task)
let task_id = ctx.tasks().create(TaskRequest {
    intent: "Prepare meeting notes for tomorrow's standup",
    source: IntentSource::AgentRequest { agent: ctx.agent_id() },
    priority: TaskPriority::Background,
}).await?;

// The task is Created but Pending until the user approves
// User sees: "Scheduling Agent wants to create a task:
//            'Prepare meeting notes for tomorrow's standup'
//            [Approve] [Deny]"
```

### 10.3 Task Agent SDK

Task agents (spawned by the Task Manager) receive their subtask description through a specialized event:

```rust
// Task agent entry point
#[agent(
    name = "Task Worker",
    capabilities = [],  // capabilities are granted dynamically per subtask
)]
async fn task_worker(ctx: AgentContext) -> Result<()> {
    // Receive subtask assignment from Task Manager
    let assignment = ctx.receive_task_assignment().await?;

    match &assignment.action {
        SubTaskAction::SpaceQuery { space, query } => {
            let results = ctx.spaces().query(space, query).await?;
            ctx.report_task_result(Value::from(results)).await?;
        }
        SubTaskAction::Inference { task_type, prompt_template } => {
            let input = ctx.receive_task_input().await?;
            let result = ctx.infer()
                .with_context(&input)
                .prompt(prompt_template)
                .await?;
            ctx.report_task_result(Value::from(result)).await?;
        }
        // ... other subtask types
        _ => {
            ctx.report_task_error("Unknown subtask action").await?;
        }
    }

    Ok(())
}
```

### 10.4 Capability Requirements

| Operation | Required Capability | Notes |
|---|---|---|
| List active tasks | `TaskRead` | Read-only, Workspace and Inspector use this |
| Subscribe to updates | `TaskRead` | Streaming variant of list |
| Create a task | `TaskCreate` | Requires user approval before execution |
| Cancel a task | `TaskWrite` | User can always cancel; agents need capability |
| Execute subtask (task agent) | Dynamic per subtask | Granted by Task Manager, minimum set |
| Read task result | `TaskRead` | After completion, results are in spaces |

-----

## 11. Diagnostics

The Task Manager exposes its internal state through the Inspector, following the same pattern as other AIOS services.

### 11.1 Inspector View

```
┌───────────────────────────────────────────────────────────────┐
│  Task Manager — Inspector                                      │
│                                                                │
│  Active Tasks: 2                                              │
│                                                                │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │  Task: "Summarize research papers and email summary"      │ │
│  │  State: Running (4/6 subtasks complete)                   │ │
│  │  Priority: Interactive                                    │ │
│  │  Created: 14:32:05 (2 min ago)                           │ │
│  │                                                           │ │
│  │  Subtask Graph:                                          │ │
│  │    ✓ T1: Query research papers (0.3s)                    │ │
│  │    ✓ T2: Extract findings from 8 papers (12.1s)          │ │
│  │    ✓ T3: Synthesize unified summary (3.2s)               │ │
│  │    ▶ T4: Compose email (running, 1.1s elapsed)           │ │
│  │    ○ T5: User confirmation (blocked on T4)               │ │
│  │    ○ T6: Send email (blocked on T5)                      │ │
│  │                                                           │ │
│  │  Agents: 1 active (task-worker-a7f3)                     │ │
│  │  Memory: 18 MB (agent) + 2 MB (Task Manager overhead)    │ │
│  │  Inference: 3 requests completed, 1 in flight            │ │
│  └──────────────────────────────────────────────────────────┘ │
│                                                                │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │  Task: "Organize photos by location"                      │ │
│  │  State: Running (12/47 subtasks complete)                 │ │
│  │  Priority: Background                                     │ │
│  │  Created: 14:15:22 (19 min ago)                          │ │
│  │  Progress: ████████░░░░░░░░░░░░  25%                     │ │
│  └──────────────────────────────────────────────────────────┘ │
│                                                                │
│  Recent Completed: 14 tasks today                             │
│  Recent Failed: 1 task today (network timeout)                │
│                                                                │
│  Service Health:                                              │
│    AIRS connection:     Connected (full decomposition)        │
│    Agent Runtime:       Connected                             │
│    Context Engine:      Connected (priority hints active)     │
│    Template library:    24 templates loaded                   │
│    Deferred queue:      0 intents waiting                     │
└───────────────────────────────────────────────────────────────┘
```

### 11.2 Diagnostic API

```rust
pub enum TaskDiagnostic {
    /// All active tasks with their current state
    ActiveTasks {
        tasks: Vec<TaskSummary>,
    },
    /// Detailed view of a specific task
    TaskDetail {
        task: Task,
        agents: Vec<AgentId>,
        resource_usage: TaskResourceUsage,
    },
    /// Service connection health
    ServiceHealth {
        airs: ConnectionStatus,
        agent_runtime: ConnectionStatus,
        context_engine: ConnectionStatus,
        space_storage: ConnectionStatus,
    },
    /// Historical statistics
    Statistics {
        tasks_today: u64,
        tasks_completed: u64,
        tasks_failed: u64,
        avg_task_duration: Duration,
        avg_subtasks_per_task: f32,
        most_common_intents: Vec<(String, u64)>,
    },
}

pub struct TaskSummary {
    id: TaskId,
    intent_summary: String,
    state: TaskState,
    progress: TaskProgress,
    priority: TaskPriority,
    created_at: Timestamp,
    duration: Duration,
}

pub struct TaskResourceUsage {
    /// Total memory used by task agents
    agent_memory: usize,
    /// Total CPU time consumed
    cpu_time: Duration,
    /// Total inference requests
    inference_requests: u64,
    /// Total inference tokens generated
    inference_tokens: u64,
    /// Space operations
    space_reads: u64,
    space_writes: u64,
}
```

-----

## 12. Implementation Order

The Task Manager is built incrementally across several development phases. Each phase delivers independently testable functionality.

```
Phase 10: Basic Task Model
  ├── Task struct and TaskState enum
  ├── Task state persistence in system/tasks/ space
  ├── IPC endpoint (sys.tasks) with basic operations
  ├── Workspace integration (display active tasks)
  └── Manual task creation from Conversation Bar (single-step tasks)

Phase 11: Task Decomposition and Execution
  ├── Intent decomposition via AIRS (NL → task graph)
  ├── Task template library (common intents without AIRS)
  ├── DAG executor (topological order, sequential execution)
  ├── Task agent spawning via Agent Runtime
  ├── Capability planner (minimum capability per subtask)
  ├── User confirmation gates
  └── Result collection and task completion

Phase 12: Agent Orchestration and Concurrency
  ├── Agent selector (reuse, tool delegation, spawn)
  ├── Concurrent subtask execution (parallel DAG branches)
  ├── Concurrency limits (global and per-task)
  ├── Context-aware priority adjustment
  └── Inspector integration (task diagnostics)

Phase 13: Error Handling and Resilience
  ├── Retry policy (exponential backoff, retryable errors)
  ├── Partial result preservation
  ├── Agent crash recovery (respawn and re-execute)
  ├── Interrupted task recovery on reboot
  └── Deferred intent queue (AIRS unavailable)

Phase 14: Optimization and Intelligence
  ├── Task history learning (common intents → cached decompositions)
  ├── Predictive task preparation (Context Engine signals)
  ├── Agent pool (pre-warmed task agents for fast startup)
  ├── Streaming progress to Workspace (real-time subtask updates)
  └── SDK: TaskRead, TaskCreate, TaskWrite capabilities for agents
```

**Critical dependencies:**

- Task Manager requires IPC (Phase 3) — all communication is IPC-based.
- Task Manager requires Agent Runtime (Phase 10) — cannot spawn task agents without it.
- Task Manager requires Space Storage (Phase 4) — task state persisted in `system/tasks/`.
- AIRS integration requires AIRS inference engine (Phase 8) — intent decomposition needs inference.
- Context-aware scheduling requires Context Engine (Phase 8) — priority hints from context state.
- Workspace display requires Compositor (Phase 6) — task progress shown in the Workspace UI.

-----

## 13. Design Principles

1. **Users think about goals, not programs.** The user says what they want. The Task Manager figures out which agents to run, in what order, with what data. The user never has to manually coordinate agents.

2. **Minimum capability per subtask.** Each task agent receives only the capabilities it needs for its specific subtask. A summarization agent cannot send email. An email agent cannot read research papers. This limits blast radius even within a single task.

3. **Ephemeral by default.** Task agents are born, execute, and die. They do not accumulate state, do not persist across reboots, and do not hold capabilities after completion. This is a security property: there is no residual attack surface from completed tasks.

4. **Graceful degradation.** Without AIRS, the Task Manager still handles common intents through templates. Without the Context Engine, tasks run at default priority. The system gets less intelligent, but it never stops working.

5. **Partial results are valuable.** A failed task that completed 4 of 6 steps still produced useful work. The user can see and use partial results. The system never discards work that was done correctly.

6. **Transparency.** Every task, every subtask, every agent spawn, every capability grant is visible in the Inspector. The user can always see what the system is doing on their behalf and why.

7. **User confirmation for irreversible actions.** Sending email, deleting objects, posting to external services — these actions always pause for user approval. The Task Manager automates the boring parts; the user approves the consequential parts.

8. **Idempotent subtasks.** Subtasks are designed so that re-executing them produces the same result. This makes retry safe: if an agent crashes during summarization, respawning and re-running the summarization produces the same summary. Side-effecting subtasks (send email) are gated behind user confirmation, which naturally prevents duplicate execution.
