# AIOS Flow SDK API

Part of: [flow.md](./flow.md) — Flow System
**Related:** [flow-data-model.md](./flow-data-model.md) — TypedContent and FlowEntry types, [flow-transforms.md](./flow-transforms.md) — TransformHandler trait, [flow-security.md](./flow-security.md) — Capability requirements, [agents.md](../applications/agents.md) — Agent SDK

-----

## 12. SDK API

### 12.1 Rust API

The agent SDK exposes Flow through the `FlowClient` trait on the `AgentContext`:

```rust
/// The Flow client interface, accessed via ctx.flow()
#[async_trait]
pub trait FlowClient: Send + Sync {
    /// Push content into Flow.
    /// Requires FlowWrite capability.
    async fn push(
        &self,
        content: TypedContent,
        options: FlowOptions,
    ) -> Result<TransferId>;

    /// Pull the most recent matching content from Flow.
    /// Requires FlowRead capability.
    /// Blocks until content is available or timeout expires.
    async fn pull(
        &self,
        filter: FlowFilter,
    ) -> Result<FlowEntry>;

    /// Pull without blocking. Returns None if no matching content is available.
    async fn try_pull(
        &self,
        filter: FlowFilter,
    ) -> Result<Option<FlowEntry>>;

    /// Browse Flow history.
    async fn history(
        &self,
        query: FlowQuery,
    ) -> Result<Vec<FlowEntry>>;

    /// Search Flow history semantically (requires AIRS).
    async fn search(
        &self,
        natural_language_query: &str,
        limit: u32,
    ) -> Result<Vec<FlowEntry>>;

    /// Create a Flow pipe for streaming data.
    async fn create_pipe(
        &self,
        direction: FlowPipeDirection,
        target: FlowTarget,
    ) -> Result<FlowPipe>;

    /// Register an action handler for SemanticType::Action content (§15.8 in flow-extensions.md).
    /// When action content is pasted, the Flow Service dispatches to the registered handler.
    async fn register_action(
        &self,
        action_id: &str,
        handler: Box<dyn ActionHandler>,
    ) -> Result<()>;

    /// Register a transform with the Flow Service.
    /// The handler implements the transform logic (see TransformHandler trait below).
    async fn register_transform(
        &self,
        transform: Transform,
        handler: Box<dyn TransformHandler>,
    ) -> Result<TransformId>;

    /// Subscribe to Flow events (new transfers, deliveries).
    /// Future: per-subscriber queues (§16.11 in flow-extensions.md) add
    /// SubscriptionConfig with configurable depth and overflow policy.
    async fn subscribe(
        &self,
        filter: FlowFilter,
    ) -> Result<FlowSubscription>;
}

/// Trait implemented by agents that provide content transforms.
/// Registered via FlowClient::register_transform().
/// Async because transforms may call AIRS (speech-to-text, summarization)
/// or perform I/O (network fetch, file conversion).
#[async_trait]
pub trait TransformHandler: Send + Sync {
    /// Transform input content into the declared output type.
    async fn transform(&self, input: ContentPayload) -> Result<ContentPayload, FlowError>;
}

pub struct FlowOptions {
    /// Transfer intent
    pub intent: TransferIntent,
    /// Target (Any, specific agent, specific surface)
    pub target: FlowTarget,
    /// Whether to purge content after delivery
    pub ephemeral: bool,
    /// Optional expiration time
    pub expires_in: Option<Duration>,
}

pub struct FlowFilter {
    /// Filter by content type (MIME pattern, e.g., "text/*")
    pub content_type: Option<String>,
    /// Filter by semantic type
    pub semantic_type: Option<SemanticType>,
    /// Filter by source agent
    pub source_agent: Option<AgentId>,
    /// Filter by intent
    pub intent: Option<TransferIntent>,
    /// Timeout for blocking pull
    pub timeout: Option<Duration>,
}

pub struct FlowQuery {
    /// Time range
    pub since: Option<Timestamp>,
    pub until: Option<Timestamp>,
    /// Content type filter
    pub content_type: Option<String>,
    /// Semantic type filter
    pub semantic_type: Option<SemanticType>,
    /// Source or destination agent
    pub agent: Option<AgentId>,
    /// Maximum results
    pub limit: u32,
    /// Offset for pagination
    pub offset: u32,
}

pub struct FlowSubscription {
    /// Channel that receives FlowEvent notifications
    channel: ChannelId,
}

impl FlowSubscription {
    /// Wait for the next Flow event matching the subscription filter.
    /// Blocks until an event is available.
    pub async fn recv(&self) -> Result<FlowEvent, FlowError>;
}

pub enum FlowEvent {
    /// New content available matching the subscription filter
    ContentAvailable { transfer_id: TransferId },
    /// A transfer was completed (delivered)
    TransferCompleted { entry: FlowEntry },
    /// A transfer was cancelled
    TransferCancelled { transfer_id: TransferId },
}
```

**Usage examples:**

```rust
// Simple copy/paste between agents
ctx.flow().push(
    TypedContent::plain_text("Hello from Agent A"),
    FlowOptions {
        intent: TransferIntent::Copy,
        target: FlowTarget::Any,
        ephemeral: false,
        expires_in: None,
    },
).await?;

// Pull from Flow (blocking, waits up to 5 seconds)
let entry = ctx.flow().pull(FlowFilter {
    content_type: Some("text/*".into()),
    timeout: Some(Duration::from_secs(5)),
    ..Default::default()
}).await?;

// Quote with attribution
ctx.flow().push(
    TypedContent::rich_text(selected_html, source_url),
    FlowOptions {
        intent: TransferIntent::Quote,
        target: FlowTarget::Agent(research_agent_id),
        ephemeral: false,
        expires_in: None,
    },
).await?;

// Browse history: last hour, code only
let history = ctx.flow().history(FlowQuery {
    since: Some(Timestamp::now() - Duration::from_secs(3600)),
    content_type: Some("text/x-*".into()),
    semantic_type: Some(SemanticType::Code { language: "rust".into() }),
    limit: 50,
    offset: 0,
    ..Default::default()
}).await?;

// Semantic search
let results = ctx.flow().search(
    "that architecture diagram from the meeting",
    10,
).await?;

// Ephemeral transfer (password)
ctx.flow().push(
    TypedContent::plain_text(password),
    FlowOptions {
        intent: TransferIntent::Copy,
        target: FlowTarget::Agent(browser_tab_id),
        ephemeral: true,
        expires_in: Some(Duration::from_secs(30)),
    },
).await?;

// Subscribe to incoming transfers
let subscription = ctx.flow().subscribe(FlowFilter {
    content_type: Some("application/pdf".into()),
    ..Default::default()
}).await?;

// In the agent's event loop:
loop {
    let event = subscription.recv().await?;
    match event {
        FlowEvent::ContentAvailable { transfer_id } => {
            let entry = ctx.flow().pull(FlowFilter::default()).await?;
            // Process the PDF
        }
        _ => {}
    }
}
```

### 12.2 Python API

The Python SDK wraps the same IPC protocol:

```python
import aios

@aios.agent(
    name="Research Assistant",
    capabilities=["FlowRead", "FlowWrite", "ReadSpace('research')"],
)
async def research_assistant(ctx: aios.AgentContext):
    # Push content to Flow
    await ctx.flow.push(
        content=aios.TypedContent.plain_text("Key finding: ..."),
        intent=aios.TransferIntent.QUOTE,
        target=aios.FlowTarget.ANY,
    )

    # Pull from Flow (blocking)
    entry = await ctx.flow.pull(
        content_type="text/*",
        timeout=5.0,
    )
    print(f"Received: {entry.content.as_text()}")

    # Browse history
    history = await ctx.flow.history(
        since=aios.Timestamp.hours_ago(1),
        content_type="application/pdf",
        limit=20,
    )
    for entry in history:
        print(f"{entry.source_agent}: {entry.content.metadata.title}")

    # Semantic search
    results = await ctx.flow.search("transformer paper from arxiv")
    for entry in results:
        print(f"{entry.content.metadata.title} ({entry.initiated_at})")

    # Streaming: subscribe to incoming content
    async for event in ctx.flow.subscribe(content_type="image/*"):
        if isinstance(event, aios.FlowEvent.ContentAvailable):
            entry = await ctx.flow.pull()
            await process_image(entry.content)
```

### 12.3 TypeScript API

The TypeScript SDK provides the same interface for agents written in TypeScript and for PWAs using the `aios.flow()` web API:

```typescript
import { agent, AgentContext, TransferIntent, FlowTarget } from '@aios/sdk';

export default agent({
  name: 'Research Assistant',
  capabilities: ['FlowRead', 'FlowWrite'],
}, async (ctx: AgentContext) => {

  // Push content to Flow
  await ctx.flow.push(
    TypedContent.plainText('Key finding: ...'),
    {
      intent: TransferIntent.Quote,
      target: FlowTarget.Any,
    }
  );

  // Pull from Flow
  const entry = await ctx.flow.pull({
    contentType: 'text/*',
    timeout: 5000,
  });
  console.log(`Received: ${entry.content.asText()}`);

  // Browse history
  const history = await ctx.flow.history({
    since: Timestamp.hoursAgo(1),
    contentType: 'application/pdf',
    limit: 20,
  });

  // Semantic search
  const results = await ctx.flow.search('transformer paper from arxiv');

  // Subscribe to incoming transfers
  for await (const event of ctx.flow.subscribe({ contentType: 'image/*' })) {
    if (event.type === 'content_available') {
      const entry = await ctx.flow.pull();
      await processImage(entry.content);
    }
  }
});
```

**PWA Web API (browser-specific):**

```javascript
// In a PWA running in a Tab Agent — AIOS-specific Web API extension
// Only available on AIOS, feature-detect with: if (window.aios?.flow)

// Push selected text to Flow
const selection = window.getSelection().toString();
await aios.flow.push({
  content: selection,
  mimeType: 'text/plain',
  intent: 'quote',
  metadata: {
    title: document.title,
    sourceDescription: `Selected from ${window.location.href}`,
  },
});

// Pull from Flow
const entry = await aios.flow.pull({ contentType: 'text/*' });
document.getElementById('input').value = entry.content;
```
