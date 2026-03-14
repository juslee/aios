# AIOS Compositor Security and Accessibility

Part of: [compositor.md](../compositor.md) — Compositor and Display Architecture
**Related:** [model.md](../../security/model.md) — Security model and threat model, [capabilities.md](../../security/model/capabilities.md) — Capability system internals, [accessibility.md](../../experience/accessibility.md) — System accessibility architecture, [input.md](./input.md) — Secure input routing

-----

## 10. Security Model

The compositor is a TL1 system service with direct access to display hardware, GPU command submission, and user input streams. It is the sole mediator between agents and the screen — no agent can draw pixels, read the framebuffer, or receive input without passing through compositor capability checks. This section defines the security properties the compositor enforces.

-----

### 10.1 Capability-Gated Surface Operations

Every compositor operation requires a valid `DisplayCapability` token. The token is checked on every `CompositorRequest` before the compositor processes it. There is no fast path that bypasses capability validation.

```rust
/// Display capability token attached to each agent's compositor session.
///
/// Controls what display operations the agent can perform. Issued by the
/// Service Manager at session open, based on the agent's trust level and
/// manifest declarations.
pub struct DisplayCapability {
    /// Maximum surface dimensions (pixels).
    max_width: u32,
    max_height: u32,
    /// Maximum GPU memory this agent can allocate (bytes).
    max_memory: usize,
    /// Can request fullscreen mode.
    fullscreen: bool,
    /// Can create overlay/panel-layer surfaces.
    overlay: bool,
    /// Can submit GPU compute shaders (WebGPU, GPGPU).
    gpu_compute: bool,
    /// Can capture screen content (screenshots, recording).
    capture: bool,
    /// Can receive secure input events (password fields, biometric prompts).
    secure_input: bool,
    /// Can inject synthetic input events (accessibility tools, automation).
    synthetic_input: bool,
}
```

**Capability hierarchy.** Capabilities form a strict hierarchy. Each level includes all permissions from the levels below:

| Level | Permissions | Typical holder |
|---|---|---|
| Basic | Create surface, receive input, attach buffers | All agents |
| Fullscreen | Request exclusive fullscreen mode | TL2 trusted agents |
| Overlay | Create overlay/panel-layer surfaces | TL1 system services |
| GPU compute | Submit compute shaders, allocate GPU memory | TL2 with manifest declaration |
| Capture | Take screenshots, record screen regions | TL1 system services |
| Synthetic input | Inject keyboard/mouse/touch events | TL1 accessibility services |

**Per-operation validation.** The compositor validates capabilities on every request, not just at session creation. If a capability is revoked mid-session (see [capabilities.md](../../security/model/capabilities.md) §3.1 for cascade revocation), subsequent requests using that capability are rejected immediately.

**Sub-surface attenuation.** When an agent creates a sub-surface for embedded content (e.g., a browser tab embedding an iframe's rendering surface), the sub-surface receives an attenuated copy of the parent's capability. The attenuated copy removes `fullscreen`, `overlay`, `capture`, and `synthetic_input` permissions. The sub-surface inherits `max_memory` as a fraction of the parent's remaining GPU memory budget.

**Subsystem integration.** The compositor implements the `Subsystem` trait from the subsystem framework (see [subsystem-framework.md](../subsystem-framework.md)):

```rust
impl Subsystem for DisplaySubsystem {
    const ID: SubsystemId = "display";
    type Capability = DisplayCapability;
    type Device = DisplayOutput;
    type Session = RenderSession;
    type AuditEvent = DisplayAuditEvent;

    fn gate_check(
        &self,
        agent: AgentId,
        cap: &DisplayCapability,
        intent: &SessionIntent,
    ) -> Result<(), GateError> {
        // Validate trust level permits requested capabilities.
        // Validate GPU memory budget is within system limits.
        // Validate surface dimensions do not exceed output resolution.
        // Log gate check result to audit ring.
    }

    fn open_session(
        &self,
        agent: AgentId,
        cap: &DisplayCapability,
        intent: &SessionIntent,
    ) -> Result<RenderSession, SessionError> {
        self.gate_check(agent, cap, intent)?;

        let surface = self.compositor.create_surface(
            cap.max_width,
            cap.max_height,
            agent,
        )?;

        Ok(RenderSession {
            surface,
            agent,
            capability: cap.clone(),
            buffers: SharedBufferPool::new(cap.max_memory),
        })
    }
}
```

-----

### 10.2 GPU Isolation and Process Sandboxing

Agents never submit GPU commands directly. All GPU work is mediated by the compositor, which acts as a GPU command proxy — similar to Chromium's GPU process architecture.

**Per-process GPU context.** Each agent session receives a logically separate GPU context. The compositor maintains per-agent command queues and memory pools. One agent's GPU state (shaders, textures, render targets) is invisible to other agents. Context isolation is enforced at the compositor level (separate wgpu device instances or separate command encoders within a shared device, depending on GPU driver capabilities).

**GPU command validation.** Agents submit high-level render commands (draw surface, apply shader effect) via IPC. The compositor translates these into GPU-native commands after validation. Validation checks include: buffer bounds (no out-of-bounds texture reads), shader complexity limits (maximum instruction count, maximum loop iterations), and memory allocation caps (per-agent GPU memory budget from `DisplayCapability.max_memory`).

**IOMMU enforcement.** On hardware with ARM SMMU (System Memory Management Unit), such as Raspberry Pi 4/5 with the VideoCore GPU, the compositor configures IOMMU stream IDs so that GPU DMA is restricted to the compositor's own allocated memory regions. An agent cannot trick the GPU into reading arbitrary physical memory because the IOMMU rejects DMA to addresses outside the permitted range. On QEMU (VirtIO-GPU), isolation relies on the hypervisor's device emulation boundaries.

**GPU fault handling.** If an agent's GPU context faults — shader timeout (exceeding the per-dispatch time limit), invalid memory access, or resource exhaustion — the compositor destroys that agent's GPU context and all associated surfaces. Other agents are unaffected. The compositor logs a `DisplayAuditEvent::GpuFault` with the faulting agent's identity, fault type, and the command that triggered it. The agent receives a `CompositorEvent::SessionTerminated` with an error code indicating the fault reason.

**Browser WebGPU sandboxing.** Browser tabs requesting WebGPU access receive a further-restricted GPU context. Shader validation is stricter: no unbounded loops, no shared memory atomics (to prevent GPU-based timing side channels), and a lower per-tab GPU memory budget. WebGPU shaders are validated against the WGSL specification before compilation. The browser agent (TL2) delegates WebGPU surfaces to individual tab sub-surfaces, each with attenuated capabilities.

**Chromium-style GPU process model.** GPU command execution runs in the compositor's own address space, not in agents' address spaces. Agents submit render command buffers via shared memory IPC. The compositor deserializes, validates, and batches these commands before submitting them to the GPU driver. This ensures that even a fully compromised agent cannot issue raw GPU commands.

-----

### 10.3 Screen Capture Protection

Screen capture is a privileged operation. By default, agents cannot read back any pixel data from the compositor's framebuffer or from other agents' surfaces.

**Capture capability.** Only agents holding a `DisplayCapability` with `capture: true` can request screenshots or screen recordings. The capability is typically granted only to TL1 system services (e.g., the screenshot tool, the Inspector dashboard) and explicitly never to TL3 or TL4 agents.

**Per-surface capture policy.** Each surface declares a capture policy via its `SurfaceHints`:

```rust
/// Controls whether this surface's content can appear in screen captures.
pub enum CapturePolicy {
    /// Surface appears black/transparent in all captures.
    None,
    /// Only TL1 and TL2 agents with capture capability can capture this surface.
    AllowTrusted,
    /// Any agent with capture capability can capture this surface.
    AllowAll,
}
```

Surfaces displaying sensitive content (password managers, banking agents, DRM-protected media) set `CapturePolicy::None`. The compositor renders these surfaces normally to the display but substitutes a black rectangle when compositing a capture buffer.

**Watermarking.** When a trusted agent captures a screen region, the compositor embeds an invisible watermark in the captured image. The watermark encodes: the capturing agent's `AgentId`, the timestamp (millisecond precision), and a HMAC signature using the system's device key. This creates an audit trail — if a captured image is leaked, the watermark identifies the source. Watermark embedding uses LSB steganography in the spatial domain, imperceptible to human vision.

**Screen recording.** Continuous capture (screen recording, screen sharing) requires the `ScreenRecord` permission, which is a separate capability from single-frame `capture`. The compositor displays a persistent recording indicator (a colored dot in the status bar) whenever any agent is actively recording. Recording sessions are logged with start/stop timestamps, target region, and the recording agent's identity.

**Audit logging.** Every capture operation — single frame or recording start/stop — generates a `DisplayAuditEvent::ScreenCapture` entry containing: captor `AgentId`, list of captured `SurfaceId` values, capture dimensions, timestamp, and whether any surfaces were redacted due to `CapturePolicy::None`.

-----

### 10.4 Secure Clipboard

Clipboard operations in AIOS route through the Flow subsystem (see [flow.md](../../storage/flow.md) §6 for compositor integration). The compositor's role is to enforce security policy at the display boundary — the point where data crosses between agents via copy/paste or drag/drop.

**Content type validation.** When an agent places content on the clipboard via Flow, the compositor validates that the declared MIME type matches the actual content structure. A payload declared as `text/plain` that contains embedded HTML tags is rejected. This prevents content injection attacks where an agent disguises executable content as plain text to exploit a paste target's parser.

**Paste confirmation for trust boundary crossings.** When clipboard content flows from an untrusted agent (TL3 or TL4) to a trusted agent (TL1 or TL2), the compositor interposes a confirmation dialog. The dialog displays: the source agent's name and trust level, the content type and size, and a preview of the content (truncated for large payloads). The user must explicitly approve the paste. This prevents untrusted agents from injecting malicious content into trusted contexts via the clipboard.

**Clipboard isolation.** Agents can only read clipboard content that was explicitly shared with them through Flow's capability-gated transfer mechanism. There is no global clipboard buffer that all agents can read. An agent must hold a `FlowCapability` with read access to the relevant Flow channel to receive paste data. The compositor mediates this by forwarding paste requests to the Flow service, which enforces capability checks.

**Clipboard content limits.** Maximum clipboard payload size is bounded per trust level: TL1/TL2 agents can place up to 64 MiB on the clipboard, TL3 agents up to 4 MiB, and TL4 agents (web content) up to 1 MiB. Payloads exceeding the limit are rejected with an error.

**Audit trail.** All copy and paste operations are logged through the Flow subsystem's audit channel. Each log entry records: source agent, target agent, content MIME type, content size in bytes, timestamp, and whether user confirmation was required and granted.

**Clear-on-lock.** When the screen locks (user-initiated or idle timeout), the compositor sends a `ClearClipboard` command to the Flow service. All pending clipboard content is zeroed. This prevents an attacker with physical access to a locked device from pasting previously copied sensitive data after unlock.

-----

### 10.5 Trust Level Enforcement

The compositor maps each agent's trust level to a fixed set of display capabilities. This mapping is the compositor's policy — it cannot be overridden by the agent's manifest. Trust level escalation requires explicit user approval through the identity verification flow (see [identity.md](../../experience/identity.md)).

| Trust Level | Display Capabilities |
|---|---|
| TL0 (kernel) | No surfaces, no display access. The kernel does not participate in the compositor protocol. |
| TL1 (system services) | System overlays (status bar, lock screen, notification center). Synthetic input injection (for accessibility services). Screen capture and recording. Secure input bypass (for system auth dialogs). Unlimited surface dimensions. |
| TL2 (trusted agents) | Standard surface creation. Fullscreen mode. Hotkey registration (agent-specific shortcuts). GPU compute (with manifest declaration). Surface dimensions up to output resolution. |
| TL3 (untrusted agents) | Restricted surface size (max 2048x2048 or 75% of output resolution, whichever is smaller). No overlay or panel-layer surfaces. No screen capture. No focus stealing (surface cannot programmatically request focus). No hotkey registration. GPU memory limited to 32 MiB. |
| TL4 (web content) | Most restricted. No direct compositor access — web content is rendered by the browser agent (TL2) into sub-surfaces. Sandboxed WebGPU only (validated shaders, no raw GPU). Cannot set surface title or icon (browser agent controls chrome). GPU memory limited to 64 MiB per tab. |

**Focus stealing prevention.** TL3 and TL4 agents cannot programmatically request focus. They can set `SurfaceUrgency::High` to display a visual indicator (badge, subtle glow), but the compositor does not transfer keyboard focus without user interaction. TL1 and TL2 agents can request focus for system-critical surfaces (e.g., incoming call dialog, security alert), but such requests are rate-limited to prevent abuse.

**Trust escalation.** An agent can request elevated display capabilities by initiating a trust escalation flow. The compositor forwards the request to the Service Manager, which presents a user consent dialog via a TL1 overlay surface. The user sees the agent's identity, the requested capabilities, and can approve or deny. Approved escalations are time-bounded (maximum 1 hour for TL3 agents) and logged.

-----

## 11. Accessibility

The compositor provides the display-side infrastructure for AIOS accessibility. This includes the accessibility tree (the structured representation of all on-screen content), screen reader event routing, visual transforms (magnification, high contrast), and keyboard navigation management. The full accessibility system architecture — including the Accessibility Manager service, screen reader (eSpeak-NG TTS), braille output, switch scanning, and AI-enhanced accessibility — is defined in [accessibility.md](../../experience/accessibility.md).

-----

### 11.1 Accessibility Tree

The compositor maintains a system-wide accessibility tree that represents every interactive element on screen. Agents contribute their subtrees; the compositor assembles them into a unified tree rooted at the display output.

```rust
/// System-wide accessibility tree maintained by the compositor.
pub struct AccessibilityTree {
    /// Root node (represents the entire display output).
    root: AccessNodeId,
    /// Flat storage for all nodes, indexed by AccessNodeId.
    nodes: Vec<AccessNode>,
    /// Map from surface to its root node in the tree.
    surface_map: HashMap<SurfaceId, AccessNodeId>,
}

/// A single node in the accessibility tree.
pub struct AccessNode {
    id: AccessNodeId,
    /// WAI-ARIA aligned role.
    role: AccessRole,
    /// Human-readable name (button label, window title, etc.).
    name: String,
    /// Extended description (tooltip text, help text).
    description: Option<String>,
    /// Current state flags.
    state: AccessState,
    /// Child node identifiers (ordered).
    children: Vec<AccessNodeId>,
    /// Actions this node supports.
    actions: Vec<AccessAction>,
    /// Bounding rectangle in compositor coordinates.
    bounds: Rect,
    /// Live region configuration (for dynamic content announcements).
    live_region: Option<LiveRegion>,
}
```

**AccessRole enum.** Roles are aligned with WAI-ARIA to ensure compatibility with assistive technology conventions:

```rust
pub enum AccessRole {
    Window, Button, TextInput, Label, List, ListItem,
    Menu, MenuItem, ScrollArea, Slider, Checkbox,
    RadioButton, Tab, TabPanel, Toolbar, StatusBar,
    Dialog, Alert, Image, Link, Table, Row, Cell,
    Heading, Separator, ProgressBar, Tooltip, Tree,
    TreeItem, Grid, GridCell,
}
```

**AccessAction and AccessState:**

```rust
pub enum AccessAction {
    Click, Focus, Expand, Collapse, ScrollTo,
    SetValue(String), Increment, Decrement, ShowMenu,
}

bitflags! {
    pub struct AccessState: u32 {
        const FOCUSED         = 1 << 0;
        const SELECTED        = 1 << 1;
        const EXPANDED        = 1 << 2;
        const DISABLED        = 1 << 3;
        const REQUIRED        = 1 << 4;
        const READ_ONLY       = 1 << 5;
        const MULTI_SELECTABLE = 1 << 6;
        const CHECKED         = 1 << 7;
        const PRESSED         = 1 << 8;
    }
}
```

**Incremental updates.** Agents do not send full accessibility trees on every frame. Instead, they send incremental diffs via IPC:

```rust
pub enum AccessTreeUpdate {
    AddNode { parent: AccessNodeId, node: AccessNode, index: usize },
    RemoveNode { id: AccessNodeId },
    UpdateProperty { id: AccessNodeId, property: AccessProperty },
}

pub enum AccessProperty {
    Name(String),
    Description(Option<String>),
    State(AccessState),
    Bounds(Rect),
    LiveRegion(Option<LiveRegion>),
    Value(String),
}
```

The compositor applies diffs atomically per-surface. A full tree rebuild is only required when a surface is first created or after a surface recovery from a crash.

-----

### 11.2 Screen Reader Integration

The compositor generates accessibility events and routes them to the Accessibility Manager service (see [accessibility.md](../../experience/accessibility.md) §2) via a dedicated IPC channel. The Accessibility Manager then forwards relevant events to the active screen reader for speech output.

```rust
pub enum AccessibilityEvent {
    /// Keyboard focus moved to a new node.
    FocusChanged { node: AccessNodeId },
    /// A node's value changed (text input, slider position, etc.).
    ValueChanged { node: AccessNodeId, value: String },
    /// An alert was raised (error message, confirmation dialog).
    Alert { message: String },
    /// A new surface (window) was created.
    WindowCreated { surface: SurfaceId },
    /// A surface (window) was destroyed.
    WindowDestroyed { surface: SurfaceId },
    /// A live region's content changed (chat message, status update).
    LiveRegionChanged { node: AccessNodeId, text: String },
    /// Selection changed within a list, table, or tree.
    SelectionChanged { node: AccessNodeId },
    /// A node's state changed (expanded/collapsed, checked/unchecked).
    StateChanged { node: AccessNodeId, state: AccessState },
}
```

**Live region announcements.** Nodes with a `LiveRegion` configuration generate `LiveRegionChanged` events when their content changes. The `LiveRegion` struct specifies the announcement priority:

```rust
pub struct LiveRegion {
    /// How urgently changes should be announced.
    priority: LiveRegionPriority,
    /// Whether to announce the full region content or just the diff.
    atomic: bool,
}

pub enum LiveRegionPriority {
    /// Announce at the next natural pause in speech.
    Polite,
    /// Interrupt current speech to announce immediately.
    Assertive,
}
```

**Event priority.** The compositor orders accessibility events by priority to prevent announcement queue flooding. Priority order (highest first): `Alert` > `LiveRegionChanged` (Assertive) > `FocusChanged` > `LiveRegionChanged` (Polite) > `ValueChanged` > `StateChanged` > `SelectionChanged`. Lower-priority events are coalesced if a higher-priority event for the same node arrives within 100ms.

**WAI-ARIA compliance.** All standard ARIA roles, states, and properties defined in the WAI-ARIA 1.2 specification are supported. The compositor's `AccessRole` enum maps directly to ARIA roles. Custom roles are not supported — agents must use the closest standard role and supplement with `description` for additional context.

-----

### 11.3 Magnification

Magnification is a compositor-level operation. The compositor applies zoom at the composition stage, after all surfaces have been composited but before presentation to the display. Individual surfaces do not need to know about magnification — they render at their native resolution.

```rust
pub struct MagnificationConfig {
    /// Whether magnification is active.
    enabled: bool,
    /// Zoom scale factor. Range: 1.0 (no zoom) to 16.0 (maximum zoom).
    scale: f32,
    /// Viewport follows keyboard focus changes.
    follow_focus: bool,
    /// Viewport follows mouse pointer movement.
    follow_pointer: bool,
    /// Circular lens mode instead of full-screen zoom.
    lens_mode: bool,
}
```

**Smooth zoom animation.** Zoom level changes (triggered by keyboard shortcut or gesture) are animated over 200ms using `EaseInOut` easing. The compositor interpolates the scale factor per-frame to avoid jarring visual transitions. If reduced motion is enabled (see §11.4), zoom changes are instant.

**Follow-focus tracking.** When `follow_focus` is enabled, the magnification viewport automatically pans to center on the currently focused element in the accessibility tree. The compositor reads the focused node's `bounds` from the accessibility tree and smoothly scrolls the viewport to center that rectangle. If the focused element is larger than the viewport at the current zoom level, the viewport aligns to the top-left corner of the element.

**Follow-pointer mode.** When `follow_pointer` is enabled, the magnification viewport tracks the mouse pointer. As the pointer approaches the viewport edge (within 10% of the viewport dimension), the viewport scrolls in that direction. Scroll speed is proportional to how close the pointer is to the edge — faster at the very edge, slower near the threshold boundary.

**Lens mode.** Instead of magnifying the entire screen, lens mode displays a circular magnification region (default diameter: 400 logical pixels) centered on the mouse pointer. Content outside the lens is rendered at normal scale. The lens tracks pointer movement in real time. Lens diameter is configurable from 200 to 800 logical pixels.

-----

### 11.4 High Contrast and Reduced Motion

The compositor applies visual accessibility transforms as post-processing shaders during the composition stage. These transforms affect all surfaces uniformly — agents do not need to implement their own high-contrast or reduced-motion support.

**High contrast modes:**

```rust
pub enum HighContrastMode {
    /// No color adjustment.
    Off,
    /// Invert all colors (light becomes dark, dark becomes light).
    InvertColors,
    /// Increase contrast by expanding the luminance range.
    IncreasedContrast,
    /// Force dark background with light text across all surfaces.
    ForceDarkBackground,
    /// Force light background with dark text across all surfaces.
    ForceLightBackground,
}
```

High contrast is implemented as a fragment shader applied to the final composited framebuffer. `InvertColors` applies a per-pixel `1.0 - color` transform in linear color space (gamma-correct inversion). `IncreasedContrast` applies a sigmoid curve to luminance, pushing mid-tones toward black or white. The forced background modes remap the luminance channel while preserving relative hue, ensuring that color-coded information (charts, syntax highlighting) remains distinguishable.

**Reduced motion.** When reduced motion is enabled, the compositor disables all animation interpolation. Window open/close transitions, layout rearrangements, notification slide-ins, and zoom animations all become instant (zero-duration). The `AnimationSystem` (see [rendering.md](./rendering.md) §5.5) checks the reduced-motion flag before scheduling any animation and substitutes a single-frame transition. Agents are notified of the reduced-motion preference via `CompositorEvent::PreferenceChanged` so they can disable their own internal animations.

**Color filters.** The compositor provides color blindness compensation via per-output color matrix transforms:

| Filter | Purpose |
|---|---|
| Protanopia simulation | Simulates red-weak color blindness for testing |
| Protanopia compensation | Shifts reds toward distinguishable hues |
| Deuteranopia simulation | Simulates green-weak color blindness for testing |
| Deuteranopia compensation | Shifts greens toward distinguishable hues |
| Tritanopia simulation | Simulates blue-weak color blindness for testing |
| Tritanopia compensation | Shifts blues toward distinguishable hues |

Color matrices are applied as a 3x3 linear transform in the composition shader. Simulation and compensation filters are mutually exclusive — simulation is for developers testing their agents' color accessibility; compensation is for users with color vision deficiencies.

**Grayscale mode.** A per-output desaturation filter reduces visual complexity by removing color information entirely. Implemented as a weighted luminance conversion: `L = 0.2126R + 0.7152G + 0.0722B` (ITU-R BT.709). Grayscale mode can be combined with high contrast modes.

-----

### 11.5 Keyboard Navigation

The compositor manages system-wide keyboard navigation. It maintains tab order, renders focus indicators, and provides spatial navigation — all independent of individual agents' input handling.

**Tab order management.** The compositor computes a tab order for all interactive elements across all visible surfaces. The default order follows spatial position: left-to-right, top-to-bottom (adjusted for RTL locales). Agents can override the default order by setting explicit `tabindex` values on their accessibility tree nodes. Negative `tabindex` values remove elements from the tab sequence (they can still receive programmatic focus). Zero `tabindex` inserts the element at its natural spatial position. Positive `tabindex` values are ordered before spatially-positioned elements, in ascending numeric order.

**Focus ring rendering.** The compositor draws a visible focus indicator around the currently focused interactive element. The focus ring is a compositor-level overlay — it is drawn on top of all surface content, not by the owning agent. The default focus ring is a 2px solid outline in a high-contrast color (system accent color or white on dark backgrounds, black on light backgrounds). The focus ring respects the focused element's `bounds` from the accessibility tree and applies 2px of padding to avoid obscuring element borders.

**Spatial navigation.** When the user presses arrow keys with the spatial navigation modifier (configurable, default: no modifier in accessibility mode), focus moves to the nearest interactive element in the arrow direction. The compositor performs a geometric search over all interactive nodes in the accessibility tree: it projects a beam from the currently focused element's center in the arrow direction and selects the nearest intersecting element. Ties are broken by perpendicular distance from the beam center line.

**Skip links.** The compositor provides built-in keyboard shortcuts to jump to major landmarks in the accessibility tree:

| Shortcut | Target |
|---|---|
| Ctrl+Alt+M | Main content area (first node with `role: Window` that is not a dialog or panel) |
| Ctrl+Alt+N | Navigation area (first node with `role: Toolbar` or `role: Menu`) |
| Ctrl+Alt+S | Status bar (first node with `role: StatusBar`) |
| Ctrl+Alt+H | Next heading (cycles through nodes with `role: Heading`) |

These shortcuts are handled by the compositor before input routing and cannot be overridden by agents.

**Focus trap for modal dialogs.** When a surface contains a node with `role: Dialog` and `state: FOCUSED`, the compositor traps Tab/Shift+Tab cycling within the dialog's descendant nodes. Focus cannot escape the dialog via keyboard navigation until the dialog is dismissed. This prevents users from accidentally interacting with content behind a modal dialog, which would be confusing for screen reader users who cannot see the visual overlay.
