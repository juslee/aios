# AIOS AI-Native Browser Intelligence

Part of: [browser.md](../browser.md) — Browser Kit Architecture
**Related:** [security.md](./security.md) — Security Architecture, [engine-integration.md](./engine-integration.md) — Engine Integration

-----

## 13. AI-Native Browser Intelligence

Browser Kit sits at the boundary between web content and OS services. This position gives it unique leverage for intelligence: the Kit sees every navigation, every tab lifecycle event, and every capability request — without needing to understand DOM internals. Intelligence splits cleanly into three tiers based on where the knowledge and computation live, plus a fourth cross-application dimension that no traditional browser can replicate.

```text
Intelligence Tier          Dependency        Latency    Availability
───────────────────────    ──────────────    ────────   ────────────
13.1 AIRS-Dependent        AIRS running      10-500ms   When model loaded
13.2 Kernel-Internal ML    Frozen trees      <1ms       Always
13.3 Browser-Internal      Engine runtime    Varies     Always
13.4 Cross-Application     AIRS + Spaces     10-500ms   When model loaded
```

-----

### 13.1 AIRS-Dependent Intelligence

These features require AIRS inference and degrade gracefully when AIRS is unavailable. Each operates through Browser Kit's IPC channel to the AIRS service — the browser engine never talks to AIRS directly.

#### 13.1.1 Page Content Summarization

AIRS summarizes page content for tab overview, turning opaque URLs into human-readable descriptions. The browser engine extracts text content through the `ContentExtractor` Kit trait and sends it to AIRS via the standard inference channel.

```rust
/// Browser Kit trait for content extraction (implemented by engine)
pub trait ContentExtractor {
    /// Extract main text content from the current page.
    /// Engine strips nav, ads, boilerplate — returns article body.
    fn extract_text(&self, tab_id: TabId) -> Result<ExtractedContent, BrowserError>;
}

/// Summary produced by AIRS, stored in tab metadata
pub struct TabSummary {
    /// Human-readable summary (1-2 sentences)
    pub summary: heapless::String<256>,
    /// Detected content category
    pub category: ContentCategory,
    /// Confidence score (0-100)
    pub confidence: u8,
    /// Timestamp of summarization
    pub generated_at: Timestamp,
}

pub enum ContentCategory {
    Article,
    Documentation,
    Shopping,
    Social,
    Video,
    Email,
    Finance,
    Reference,
    Unknown,
}
```

The user sees "Meeting notes from Q3 planning" in the tab switcher instead of `https://docs.example.com/d/1a2b3c4d/edit`. Summaries are cached in the tab's Space object and invalidated on navigation.

#### 13.1.2 Smart Tab Grouping

AIRS groups tabs by semantic topic using embedding similarity. When the user has 30 tabs open, AIRS clusters them: "5 tabs about React performance", "3 tabs about flight prices", "8 tabs from work email." Grouping runs asynchronously — the user never waits for it.

```rust
pub struct TabGroup {
    /// Auto-generated group label
    pub label: heapless::String<64>,
    /// Tab IDs in this group
    pub tabs: heapless::Vec<TabId, 32>,
    /// Semantic centroid (embedding vector, for re-clustering)
    pub centroid_hash: ContentHash,
}
```

The grouping algorithm:

1. Each tab's `TabSummary` embedding is computed by AIRS (reusing Space Indexer infrastructure).
2. Hierarchical agglomerative clustering groups tabs with cosine similarity > 0.7.
3. AIRS generates a human-readable label for each cluster from its member summaries.
4. Groups update incrementally as tabs open/close — full re-clustering only on explicit request.

#### 13.1.3 Phishing Detection

AIRS analyzes page content and visual appearance to detect phishing attempts with higher confidence than pattern matching alone. The detection pipeline combines three signals:

| Signal | Source | Detection Method |
|---|---|---|
| Domain similarity | URL bar | Levenshtein distance to known brands (e.g., `paypa1.com` vs `paypal.com`) |
| Visual similarity | Page screenshot | AIRS embedding comparison against known login page templates |
| Content analysis | Extracted text | AIRS classifies urgency language, credential requests, suspicious forms |

```rust
pub struct PhishingVerdict {
    /// Overall risk score (0-100)
    pub risk_score: u8,
    /// Individual signal scores
    pub domain_score: u8,
    pub visual_score: u8,
    pub content_score: u8,
    /// Human-readable explanation
    pub explanation: heapless::String<256>,
    /// Suggested action
    pub action: PhishingAction,
}

pub enum PhishingAction {
    /// No risk detected
    Allow,
    /// Show warning banner, allow proceed
    Warn,
    /// Block page, require explicit override
    Block,
}
```

When AIRS detects a phishing page, Browser Kit inserts a capability-enforced warning surface between the engine and the compositor. The engine cannot suppress or modify this warning — it is rendered by the OS, not the browser.

#### 13.1.4 Accessibility Enhancement

AIRS provides on-demand accessibility improvements for web content:

- **Alt text generation**: When an `<img>` element lacks alt text, AIRS generates a description from the image content. The engine requests this through the `AccessibilityBridge` Kit trait.
- **Reading mode**: AIRS identifies the main content region and generates a simplified view, stripping clutter. Works alongside the engine's native reader mode.
- **Content simplification**: For users who request it, AIRS rewrites complex text at a lower reading level. The original text remains available.

All accessibility enhancements are optional and user-controlled through the accessibility preferences (see [accessibility.md](../../experience/accessibility.md) §10).

#### 13.1.5 Predictive Loading

AIRS understands user intent from context signals (time of day, recent navigation, active Space) and prefetches likely content. When the Context Engine reports "user is researching flight prices," AIRS can predict that search result links are likely to be clicked and instruct Browser Kit to prefetch them.

```rust
pub struct PrefetchHint {
    /// URL to prefetch
    pub url: heapless::String<512>,
    /// Confidence that user will navigate here (0-100)
    pub confidence: u8,
    /// Prefetch depth: dns-only, connect, full-page
    pub depth: PrefetchDepth,
}

pub enum PrefetchDepth {
    /// DNS resolution only (<1 KiB network)
    DnsOnly,
    /// TCP/TLS connection establishment
    Connect,
    /// Full page download (only at >90% confidence)
    FullPage,
}
```

Prefetch respects capability constraints — Browser Kit only prefetches from origins the current tab agent has network access to. No speculative cross-origin connections without explicit capability.

#### 13.1.6 Search Enhancement

AIRS provides semantic search across ALL browser history stored in Spaces. The user asks "find that article about Rust lifetimes I read last month" and AIRS searches across the `web-history` Space using embedding similarity, not just keyword matching.

This search operates through the Space Indexer (see [space-indexer.md](../../intelligence/space-indexer.md) §8), which already indexes all Space content. Browser history is just another set of objects in a Space — no browser-specific search infrastructure needed.

When multiple browser engines store history in the same Space (Firefox in `web-history/firefox/`, Chrome in `web-history/chrome/`), the search covers both transparently.

-----

### 13.2 Kernel-Internal ML

These features use frozen decision trees trained offline and deployed as static data in the kernel. They run in constant time with no AIRS dependency, no inference latency, and no model loading. All models are under 16 KiB each.

#### 13.2.1 Navigation Prediction

A decision tree predicts the top-3 most likely URLs the user will navigate to, enabling DNS pre-resolution and connection warming. The model uses features available without page content analysis:

```rust
pub struct NavigationPredictor {
    /// Frozen decision tree (~12 KiB)
    tree: &'static [DecisionNode],
    /// Per-tab navigation history (ring buffer)
    history: heapless::Vec<NavigationRecord, 16>,
    /// Predictions (top-3)
    predictions: [NavigationPrediction; 3],
}

pub struct NavigationPrediction {
    /// Predicted URL hash (matched against history)
    pub url_hash: u64,
    /// Confidence (0-100)
    pub confidence: u8,
    /// Pre-staging action
    pub action: PrefetchDepth,
}
```

| Feature | Description | Type |
|---|---|---|
| `hour_of_day` | Current hour (0-23) | Numeric |
| `day_of_week` | Current day (0-6) | Categorical |
| `last_3_urls` | Hash of previous 3 navigations | Categorical |
| `time_on_page` | Seconds spent on current page | Numeric |
| `tab_count` | Number of open tabs | Numeric |
| `referrer_domain` | Domain of referring page | Categorical |

The predictor achieves ~65% accuracy for the top-1 prediction during routine browsing (checking email, reading news). Pre-staging actions are bounded: DNS-only for <70% confidence, TCP connect for 70-90%, no full prefetch from kernel-internal ML (that requires AIRS confirmation).

#### 13.2.2 Tab Memory Pressure Prediction

A decision tree predicts which tabs will exceed their memory budget, enabling proactive eviction before the system hits memory pressure. This runs in the kernel's memory reclamation path.

```rust
pub struct TabMemoryPredictor {
    /// Frozen decision tree (~8 KiB)
    tree: &'static [DecisionNode],
}

pub struct TabMemoryFeatures {
    /// Current RSS in pages
    pub rss_pages: u32,
    /// RSS growth rate (pages/second, averaged over 10s)
    pub growth_rate: i32,
    /// Content type classification
    pub content_type: TabContentType,
    /// Seconds since last user interaction
    pub idle_seconds: u32,
    /// Whether tab is visible
    pub visible: bool,
    /// Number of iframes
    pub iframe_count: u8,
}

pub enum TabContentType {
    StaticPage,
    WebApp,
    VideoPlayer,
    SocialFeed,
    DocumentEditor,
    Unknown,
}
```

Tabs predicted to exceed budget within 30 seconds are candidates for proactive suspension. The kernel suspends tab agents (freezing their address space) rather than killing them — suspended tabs resume instantly when the user switches back.

#### 13.2.3 Network Bandwidth Allocation

Per-tab QoS decisions based on content type classification. A decision tree classifies each tab's network usage pattern and assigns scheduling priority in the Network Kit's bandwidth scheduler.

| Content Type | Priority | Rationale |
|---|---|---|
| Video streaming | High | Jitter-sensitive, user-visible stalls |
| Interactive web app | High | Latency-sensitive (keystrokes, clicks) |
| Background sync | Low | Deferrable without user impact |
| Static page load | Medium | Burst then idle |
| Bulk download | Low | Throughput-tolerant |

The classifier runs on packet metadata (flow size, inter-arrival time, port numbers) — it never inspects payload content. Classification updates every 5 seconds per tab.

#### 13.2.4 Cryptojacking Detection

A decision tree detects sustained high CPU usage from web content without corresponding user interaction — the hallmark of in-browser cryptocurrency mining.

```rust
pub struct CryptojackingDetector {
    /// Frozen decision tree (~4 KiB)
    tree: &'static [DecisionNode],
    /// Per-tab CPU usage history (10-second windows)
    cpu_history: heapless::Vec<CpuSample, 30>,
}

pub struct CpuSample {
    /// CPU time consumed in this window (microseconds)
    pub cpu_us: u32,
    /// User interaction events in this window
    pub interaction_count: u8,
    /// WebAssembly execution time (microseconds)
    pub wasm_us: u32,
}
```

Detection features: sustained CPU > 80% for > 30 seconds, near-zero user interaction, high WASM execution ratio. When detected, Browser Kit throttles the tab agent's CPU quota and surfaces a notification to the user. False positive rate is kept below 1% by requiring all three signals simultaneously.

#### 13.2.5 Tab Priority Scoring

A composite score determines scheduler priority for tab agents. The score combines multiple signals into a single u8 value used by the kernel scheduler.

```rust
pub fn compute_tab_priority(tab: &TabState) -> u8 {
    let mut score: u16 = 0;

    // Visibility is the strongest signal
    if tab.visible { score += 40; }

    // Recency of user interaction (exponential decay)
    let recency = 30u16.saturating_sub(tab.idle_seconds.min(30) as u16);
    score += recency;

    // Audio playback keeps priority high
    if tab.playing_audio { score += 20; }

    // Background tabs with active timers get a small boost
    if tab.has_active_timers && !tab.visible { score += 10; }

    score.min(100) as u8
}
```

This scoring function is deterministic and runs in <100ns. It feeds directly into the scheduler's per-agent priority without AIRS involvement.

-----

### 13.3 Browser-Internal Intelligence

Some intelligence capabilities belong inside the browser engine, not in Browser Kit. The Kit provides hooks for engines to query AIRS if they choose, but the core logic stays engine-side.

#### 13.3.1 Chrome `ai.*` APIs

Chrome's emerging `ai.*` APIs (Prompt API, Summarizer, Writer/Rewriter) are browser-engine-level APIs. On AIOS, these can route to AIRS for inference through a thin Bridge:

```text
Chrome ai.summarizer.create()
  → Chrome internal API
    → Browser Kit AirsInferenceBridge trait
      → IPC to AIRS service
        → GGML inference
      → Result back to Chrome
    → Chrome resolves Promise
```

The DOM access, Promise lifecycle, and API surface stay inside Chrome. Browser Kit only provides the inference transport. Engines that do not implement Chrome's `ai.*` APIs are unaffected.

#### 13.3.2 Layout and Paint Optimization

Layout tree construction, style resolution, paint order optimization, and layer compositing are engine-internal concerns. Browser Kit has no visibility into these structures and does not attempt to optimize them. The engine's own intelligence (e.g., Blink's layout caching, Gecko's incremental layout) operates independently.

#### 13.3.3 Tracking Prevention

Safari's Intelligent Tracking Prevention (ITP) and Firefox's Enhanced Tracking Protection (ETP) classify trackers using engine-internal heuristics. AIOS can enhance this at the capability level — Browser Kit can deny network capabilities to known tracking domains — but the classification logic itself stays in the engine.

Browser Kit provides a `TrackingClassification` hook that engines can optionally call to supplement their internal lists with AIOS-maintained tracking data:

```rust
/// Optional hook for engines to query OS-level tracking database
pub trait TrackingOracle {
    /// Check if a domain is a known tracker.
    /// Returns None if the oracle has no opinion.
    fn classify_domain(&self, domain: &str) -> Option<TrackingClassification>;
}

pub enum TrackingClassification {
    /// Known tracker — block or restrict
    Tracker { category: TrackerCategory },
    /// Known safe — do not restrict
    Safe,
}
```

**Key principle**: Browser-internal intelligence is NOT part of Browser Kit. The Kit provides hooks; engines use them or not.

-----

### 13.4 Cross-Application AIRS Advantage

The structural advantage of AIOS: one AIRS instance serves ALL browsers simultaneously. On traditional operating systems, each browser is a silo with its own history, its own tracking database, its own intelligence. On AIOS, browsers share intelligence infrastructure through Spaces.

#### 13.4.1 Unified History Search

Firefox stores history in `web-history/firefox/`, Chrome in `web-history/chrome/`. Both are Spaces. The Space Indexer indexes both. When the user searches "find that Rust article," AIRS searches across both browser histories in a single query.

```text
Traditional OS:
  Firefox history → Firefox search → results from Firefox only
  Chrome history  → Chrome search  → results from Chrome only

AIOS:
  Firefox history ─┐
                   ├→ Space Indexer → AIRS search → unified results
  Chrome history  ─┘
```

No browser modification required. History enters Spaces through the standard `WebHistoryStore` Kit trait. Indexing happens automatically.

#### 13.4.2 Consistent Ad Blocking

One tracking/ad database serves all browsers. Updates propagate once and apply everywhere. The `TrackingOracle` (§13.3.3) queries the same database regardless of which engine calls it.

This eliminates the common user frustration of configuring ad blocking separately in each browser, maintaining different filter lists, and getting inconsistent results.

#### 13.4.3 Consistent Privacy Policy

Privacy settings applied uniformly across all browsers. If the user sets "block third-party cookies," this is enforced at the capability level by Browser Kit — not by each engine's internal cookie jar. An engine cannot bypass this restriction because the capability system operates below the engine.

```text
User sets: "Block third-party cookies"
  → Preference stored in user Space
    → Browser Kit reads preference
      → Capability gate denies cross-origin cookie storage
        → Applies to Firefox, Chrome, and any other engine equally
```

#### 13.4.4 Cross-Browser Tab Deduplication

AIRS detects the same URL open in multiple browsers and can suggest consolidation. This is a natural consequence of the AIRS search infrastructure — URL hashes across all browser Spaces are trivially comparable.

```rust
pub struct DuplicateTabAlert {
    /// The duplicated URL
    pub url_hash: u64,
    /// Tabs with this URL, across all engines
    pub tabs: heapless::Vec<(EngineId, TabId), 8>,
    /// Suggested action
    pub suggestion: DedupSuggestion,
}

pub enum DedupSuggestion {
    /// Inform the user, let them decide
    Notify,
    /// Suggest closing duplicates (only if same content)
    SuggestClose,
}
```

This feature is informational only — AIOS never closes tabs without user consent.

-----

### 13.5 Graceful Degradation

When AIRS is unavailable (not loaded, insufficient memory, or explicitly disabled), intelligence features degrade predictably:

| Feature | With AIRS | Without AIRS |
|---|---|---|
| Page summarization | Semantic summary | URL + page title only |
| Tab grouping | Semantic clustering | Domain-based grouping |
| Phishing detection | Content + visual analysis | Domain pattern matching only |
| Accessibility enhancement | AI-generated alt text, simplification | Engine-native accessibility only |
| Predictive loading | Context-aware prefetch | History-frequency prefetch |
| History search | Semantic search | Keyword/exact match |
| Cross-browser dedup | Semantic URL matching | Exact URL matching |

Kernel-internal ML features (§13.2) are unaffected by AIRS availability — they run on frozen decision trees with no external dependency. Browser-internal intelligence (§13.3) is similarly independent.

The degradation is transparent to the user. Features that require AIRS simply produce less sophisticated results. No feature fails completely — every AIRS-dependent capability has a heuristic fallback. The browser remains fully functional for all core tasks (navigation, rendering, form submission, media playback) regardless of AIRS state.

```rust
/// Kit-level AIRS availability check
pub fn airs_available() -> AirsState {
    match airs_service_channel() {
        Some(ch) if ch.is_responsive() => AirsState::Available,
        Some(_) => AirsState::Overloaded,
        None => AirsState::Unavailable,
    }
}

pub enum AirsState {
    Available,
    /// AIRS running but response time > 1s — skip non-essential queries
    Overloaded,
    /// AIRS not loaded — use heuristic fallbacks for everything
    Unavailable,
}
```

-----

### 13.6 Future Directions

These capabilities are beyond the initial Browser Kit implementation but represent natural extensions of the architecture:

- **CHERI hardware capability integration**: When Arm Morello or successor hardware becomes available, Browser Kit can map web origins to hardware capabilities. Memory safety violations in one origin's renderer cannot corrupt another origin's memory — enforced by the CPU, not by software sandboxing. This is the ultimate realization of the capability-per-origin model.

- **Chrome `ai.*` API compatibility shim**: A formal compatibility layer that routes all Chrome `ai.*` API calls through AIRS, allowing web applications written for Chrome's AI features to work on any AIOS browser engine without modification. Requires stabilization of the Chrome AI API surface.

- **Formal verification of capability-origin mapping**: Machine-checked proofs that the origin-to-capability mapping preserves the same-origin policy invariants. Building on the static analysis framework (see [static-analysis.md](../../security/static-analysis.md)), the goal is to prove that no sequence of Browser Kit API calls can violate origin isolation.

- **Federated learning for phishing detection**: AIOS devices collaboratively train phishing detection models without sharing browsing data. Each device contributes gradient updates (not raw data) to improve the shared model. Requires the federated learning infrastructure from multi-device intelligence (see [multi-device/intelligence.md](../../platform/multi-device/intelligence.md) §14.3).

- **Personalized content ranking**: AIRS learns the user's reading preferences from browsing history and re-ranks search results, news feeds, and recommendations accordingly. Operates entirely on-device — no data leaves the user's Space.

- **Cross-device tab continuity**: Combined with multi-device Space Mesh sync, open tabs on one device appear as suggestions on another. "Continue reading on your tablet" with full semantic context, not just a URL. Builds on the handoff infrastructure in [multi-device/experience.md](../../platform/multi-device/experience.md) §4.1.
