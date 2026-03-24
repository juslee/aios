# AIOS Interface Kit — AI-Native Intelligence

Part of: [interface-kit.md](../interface-kit.md) — Interface Kit Architecture
**Related:** [airs.md](../../intelligence/airs.md) — AI Runtime Service, [context-engine.md](../../intelligence/context-engine.md) — Context signals, [behavioral-monitor.md](../../intelligence/behavioral-monitor.md) — Behavioral analysis

-----

## 17. AIRS-Dependent UI Intelligence

These features require the AI Runtime Service (AIRS) for semantic understanding. When AIRS is offline, the UI falls back to standard behavior — no degraded experience, just fewer intelligent enhancements.

### 17.1 Predictive UI Prefetching

AIRS observes user interaction sequences and predicts which screens or data the agent will need next. The Interface Kit runtime can pre-render offscreen content before the user navigates to it.

```rust
/// Hint from AIRS: predicted next user action.
pub struct PrefetchHint {
    /// Predicted message the user is likely to trigger.
    pub predicted_message: MessageFingerprint,
    /// Confidence (0.0–1.0). Only act above threshold (default: 0.7).
    pub confidence: f32,
    /// Suggested prefetch: pre-compute view() for predicted state.
    pub prefetch_view: bool,
    /// Suggested prefetch: pre-load data from Spaces.
    pub prefetch_data: Option<SpaceQuery>,
}
```

The runtime receives `PrefetchHint` via a subscription. If confidence exceeds the threshold, it speculatively calls `update()` with the predicted message on a shadow copy of the state, runs `view()`, and caches the resulting layout tree. If the user does trigger the predicted action, the cached tree is used immediately — eliminating the view/layout latency for that frame.

**Fallback:** Without AIRS, no prefetch hints are generated. The UI renders reactively as normal.

### 17.2 Adaptive Layout

AIRS learns per-user layout preferences from interaction patterns:

- **Information density:** Users who scroll quickly through content may prefer compact spacing; users who linger prefer relaxed layouts.
- **Widget sizing:** Frequently tapped buttons can be enlarged; rarely used controls can shrink.
- **Content priority:** Sections the user reads first get more visual prominence (larger type, higher position).

```rust
/// AIRS-generated layout adaptation for an agent.
pub struct LayoutAdaptation {
    /// Suggested spacing multiplier (1.0 = theme default).
    pub spacing_factor: f32,
    /// Suggested font size multiplier (1.0 = theme default).
    pub font_scale: f32,
    /// Widgets to promote (show earlier/larger).
    pub promoted: Vec<ViewId>,
    /// Widgets to demote (show later/smaller).
    pub demoted: Vec<ViewId>,
}
```

Agents opt into adaptive layout via `subscription()`:

```rust
fn subscription(&self) -> Subscription<Message> {
    airs::layout_adaptation("com.example.notes")
        .map(Message::LayoutAdapted)
}
```

**Fallback:** Without AIRS, theme defaults apply uniformly.

### 17.3 Smart Widget Visibility

AIRS tracks which widgets each user actually interacts with and recommends progressive disclosure:

- Toolbar buttons used less than once per 100 sessions can be collapsed into an overflow menu.
- Settings sections the user never opens can be collapsed by default.
- Advanced options can be shown when AIRS infers the user's expertise level.

This is informed by Microsoft's adaptive menus research, which showed that frequency-based reordering reduced task completion time by 15-22% for expert users.

```rust
/// AIRS recommendation for widget visibility.
pub enum VisibilityHint {
    /// Always visible (frequently used).
    Prominent,
    /// Visible but available (occasionally used).
    Normal,
    /// Collapsed into overflow (rarely used).
    Collapsed,
    /// Hidden unless explicitly searched for (never used by this user).
    Hidden,
}
```

**Fallback:** All widgets visible at their default visibility level.

### 17.4 Accessibility ML

AIRS enhances accessibility beyond what static analysis can provide:

- **Auto alt-text generation.** Images displayed via the `image()` widget can be sent to AIRS for captioning. The generated alt-text is injected into the accessibility tree's `AccessibilityNode.description` field. Inspired by the AltGen pipeline (auto-generated alt-text for EPUB) and IconDesc (fine-tuned LLM for icon descriptions).
- **Reading order inference.** For complex layouts (dashboards, multi-column designs), AIRS infers the logical reading order from visual structure and injects `AccessibilityNode` ordering hints.
- **Label generation.** Unlabeled interactive controls (buttons with only icons) receive generated labels based on visual context and surrounding text.

```rust
/// AIRS-generated accessibility enhancement.
pub struct AccessibilityEnhancement {
    /// Target node in the accessibility tree.
    pub node_id: NodeId,
    /// Enhancement type.
    pub enhancement: AccessEnhancementKind,
}

pub enum AccessEnhancementKind {
    /// Generated image description.
    AltText(String),
    /// Inferred reading order position.
    ReadingOrder(u32),
    /// Generated label for unlabeled control.
    GeneratedLabel(String),
}
```

**Fallback:** Standard accessibility tree from widget structure. Images without alt-text show "Image" to screen readers. Reading order follows widget tree order.

### 17.5 Context-Aware UI Adaptation

AIRS integrates with the Context Engine to adapt UI beyond simple theme switching:

- **Activity-aware layout.** When AIRS detects a presentation context (external display, audience), it can suggest hiding notification badges and simplifying the UI.
- **Task-aware suggestions.** If AIRS understands the user's current task (e.g., writing a report), it can promote relevant tools (text formatting, citation insertion) and demote irrelevant ones.
- **Ambient intelligence.** Time-of-day, location, and device posture influence UI choices — larger touch targets when the device is held one-handed, compact layouts at a desk with keyboard.

**Fallback:** Fixed theme-based context switching (work/leisure/focus/gaming) as described in [theme.md §6.2](./theme.md).

-----

## 18. Kernel-Internal ML

These features use small, frozen statistical models (decision trees, lightweight neural networks) that run in-kernel or in the Interface Kit runtime without AIRS dependency. They operate purely on numerical signals — no semantic understanding.

### 18.1 Frame Jank Classification

A small decision tree classifies the cause of dropped frames, inspired by Android's Perfetto FrameTimeline:

```text
Inputs:
  - layout_time_ms    (time spent in layout pass)
  - diff_time_ms      (time spent in view tree diff)
  - render_time_ms    (time spent in GPU submission)
  - present_time_ms   (time from submit to display)
  - widget_count      (number of widgets in tree)
  - dirty_count       (number of dirty widgets)

Classification:
  IF layout_time_ms > budget * 0.4  → LayoutBound
  IF render_time_ms > budget * 0.4  → RenderBound
  IF present_time_ms > budget * 0.3 → CompositorBound
  IF diff_time_ms > budget * 0.3    → DiffBound (too many widgets changing)
  ELSE                              → Mixed
```

The classification is reported to the Inspector (see [inspector.md](../inspector.md)) for developer debugging and to the observability subsystem for aggregate analysis. On repeated jank events of the same class, the runtime can take automatic action:

- **LayoutBound:** Enable aggressive constraint caching for the offending subtree.
- **RenderBound:** Reduce texture atlas resolution or disable shadows.
- **DiffBound:** Suggest the developer use `lazy()` for unchanged subtrees.

### 18.2 Gesture Prediction

A lightweight model predicts touch/cursor targets based on trajectory:

- **Touch target prediction.** Given the last 3-5 touch/cursor positions and velocities, predict which widget the user is targeting. Pre-highlight the predicted target to reduce perceived latency.
- **Scroll prediction.** Predict scroll direction and velocity to prefetch content in virtual lists. An Echo State Network or simple linear extrapolation from recent scroll events.
- **Keyboard anticipation.** When a text input is focused, predict likely keystrokes based on input field type (email → expect '@', number → expect digits) to pre-render autocomplete candidates.

```rust
/// Gesture prediction model output.
pub struct GesturePrediction {
    /// Predicted target widget.
    pub target: Option<ViewId>,
    /// Confidence (0.0–1.0).
    pub confidence: f32,
    /// Predicted action (tap, scroll, drag).
    pub action: PredictedAction,
}

pub enum PredictedAction {
    Tap,
    ScrollUp(f32),   // predicted velocity
    ScrollDown(f32),
    DragStart,
}
```

### 18.3 Interaction Latency Anomaly Detection

A per-widget z-score tracker flags abnormal event→render latency:

- Maintains a running mean and standard deviation of event-to-render time per widget type.
- Flags widgets where latency exceeds 2 standard deviations for 3+ consecutive frames.
- Reports anomalies to the observability subsystem for developer alerting.

This is a simple Welford online algorithm — no ML model needed, just statistical tracking:

```rust
pub struct LatencyTracker {
    count: u64,
    mean: f64,
    m2: f64,  // sum of squared deviations
}

impl LatencyTracker {
    pub fn update(&mut self, latency_ms: f64) {
        self.count += 1;
        let delta = latency_ms - self.mean;
        self.mean += delta / self.count as f64;
        let delta2 = latency_ms - self.mean;
        self.m2 += delta * delta2;
    }

    pub fn is_anomalous(&self, latency_ms: f64) -> bool {
        if self.count < 30 { return false; } // need baseline
        let stddev = (self.m2 / (self.count - 1) as f64).sqrt();
        (latency_ms - self.mean).abs() > 2.0 * stddev
    }
}
```

### 18.4 Layout Cost Prediction

A learned model predicts whether a subtree's layout recomputation will exceed the frame budget, enabling preemptive optimization:

- Input: `(widget_count, nesting_depth, fill_portion_count, last_layout_time_ms)`.
- Output: `estimated_layout_time_ms`.
- If predicted cost exceeds 30% of frame budget, the runtime promotes the subtree's parent to a relayout boundary (see [layout.md §5.4](./layout.md)).

This uses a shallow decision tree (~50 nodes) trained offline on frame profiling data.

-----

## 19. Future Directions

### 19.1 RL-Based UI Generation

Reinforcement learning agents that generate entire UI layouts from task descriptions. Research (Zhan et al. 2024) demonstrated hybrid VAE-GAN approaches achieving ~0.89 personalization accuracy with reconfiguration in ~1.2 seconds. Future AIOS agents could receive a task description and have AIRS generate an optimal UI layout automatically.

### 19.2 Federated Learning for Cross-Device Preferences

UI preferences (information density, preferred widget positions, interaction patterns) learned on one AIOS device could be shared across the user's device fleet via federated learning. Each device trains a local model on interaction data; aggregated updates (no raw data) are synced via Space Mesh. This preserves privacy while enabling consistent UI adaptation across desktop, tablet, and phone form factors.

### 19.3 Spatial and 3D UI

As display technology evolves toward spatial computing (AR/VR headsets, holographic displays), Interface Kit's declarative model extends naturally:

- `Element<M>` gains a `depth` dimension.
- Layout engine adds z-axis constraint propagation.
- The compositor renders surfaces in 3D space rather than 2D planes.
- Gesture recognition extends to 6DOF input (hand tracking, gaze).

### 19.4 Neural Interface Input

Brain-computer interfaces (BCIs) produce intent signals that could drive UI navigation:

- Focus signals map to widget selection.
- Confirmation signals map to button activation.
- The Event model treats BCI input as another `InputEvent` variant.

### 19.5 Compositional UI Reasoning

LLMs that understand Interface Kit's widget vocabulary could generate UI code from natural language descriptions, debug layout issues from screenshots, or suggest accessibility improvements by analyzing the widget tree structure.
