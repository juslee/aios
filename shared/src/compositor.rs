//! Compositor protocol types shared between kernel and user-space.
//!
//! Per docs/platform/compositor/protocol.md §3.1–3.4. Phase 7 M24 ships the
//! Layer 1 subset: surface lifecycle, flat z-ordered surfaces, opaque blitting,
//! rect/full damage. Fences (§3.3), subsurfaces (§3.1), full hint set (§4),
//! and scene-graph diffs (rendering.md §5.1) are deferred.

use crate::input::InputEvent;
use crate::ipc::{ChannelId, SharedMemoryId, MAX_MESSAGE_SIZE};
use crate::sched::ProcessId;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of surfaces tracked by the compositor at once.
/// Layer 1 desktop never has more than ~10 windows; 32 leaves headroom.
pub const MAX_SURFACES: usize = 32;

/// Maximum bytes for a UTF-8 surface title before truncation.
pub const SURFACE_TITLE_MAX: usize = 64;

// ---------------------------------------------------------------------------
// Surface identity
// ---------------------------------------------------------------------------

/// Unique identifier for a compositor surface.
///
/// Assigned by the compositor on `CreateSurface`. Monotonically increasing,
/// never reused — even at 1M surfaces/sec the 64-bit space takes >500k years
/// to wrap, so reasoning about surface identity is straightforward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SurfaceId(pub u64);

impl SurfaceId {
    /// Sentinel value representing "no surface" — used in fields that may be absent.
    pub const NONE: Self = Self(0);

    /// First valid surface id (allocator starts here).
    pub const FIRST: Self = Self(1);

    /// Returns true if this id is the NONE sentinel.
    pub const fn is_none(self) -> bool {
        self.0 == 0
    }
}

// ---------------------------------------------------------------------------
// Surface state machine
// ---------------------------------------------------------------------------

/// Surface lifecycle state.
///
/// Surfaces progress linearly: Created → Configured → Active. `Suspended`
/// is a reversible detour from `Active`. `Destroyed` is terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SurfaceState {
    /// Surface allocated by the compositor; client has not received Configure yet.
    Created = 0,
    /// Compositor has sent Configure; client knows dimensions but no buffer attached.
    Configured = 1,
    /// Buffer attached and being composited each frame.
    Active = 2,
    /// Surface hidden (minimized / agent backgrounded). Resources retained, not composited.
    Suspended = 3,
    /// Surface torn down. Slot is released but the SurfaceId is never reused.
    Destroyed = 4,
}

impl SurfaceState {
    /// Returns true if no further transitions out of this state are possible.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Destroyed)
    }

    /// Returns true if the state machine permits transitioning from `self` to `next`.
    ///
    /// Per protocol.md §3.1 state diagram. `Destroyed` from any non-terminal state
    /// is always allowed (cleanup path).
    pub const fn can_transition_to(self, next: Self) -> bool {
        match (self, next) {
            // Forward path.
            (Self::Created, Self::Configured) => true,
            (Self::Configured, Self::Active) => true,
            // Idempotent self-transition for Active (subsequent AttachBuffer calls).
            (Self::Active, Self::Active) => true,
            // Reversible suspend.
            (Self::Active, Self::Suspended) => true,
            (Self::Suspended, Self::Active) => true,
            // Cleanup from any non-terminal state.
            (Self::Created, Self::Destroyed) => true,
            (Self::Configured, Self::Destroyed) => true,
            (Self::Active, Self::Destroyed) => true,
            (Self::Suspended, Self::Destroyed) => true,
            // Anything else is invalid (skipping Configured, regressing Destroyed, etc.).
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Z-order layers
// ---------------------------------------------------------------------------

/// Compositing layer — controls z-order across surfaces.
///
/// Surfaces are sorted first by layer (Background to Panel), then by insertion
/// order within a layer. Numeric values are stable: higher = closer to viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum SurfaceLayer {
    /// Behind everything else — wallpaper, workspace background.
    Background = 0,
    /// Application windows. Default for client surfaces.
    Normal = 1,
    /// Foreground utility windows (always-on-top dialogs).
    TopLevel = 2,
    /// Floating overlays — notifications, menus, popovers.
    Overlay = 3,
    /// System chrome — taskbar, status strip. Locked to screen edges.
    Panel = 4,
}

// ---------------------------------------------------------------------------
// Surface metadata
// ---------------------------------------------------------------------------

/// UTF-8 surface title, fixed-capacity for IPC marshalling.
///
/// Always truncates at a UTF-8 character boundary so the stored bytes are
/// guaranteed to be valid UTF-8 (provided the input was valid UTF-8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct SurfaceTitle {
    /// Title bytes (zero-padded after `len`).
    pub bytes: [u8; SURFACE_TITLE_MAX],
    /// Number of valid bytes in `bytes`.
    pub len: u8,
}

impl SurfaceTitle {
    /// Empty title.
    pub const EMPTY: Self = Self {
        bytes: [0; SURFACE_TITLE_MAX],
        len: 0,
    };

    /// Construct a `SurfaceTitle` from a byte slice, truncating at a UTF-8
    /// character boundary if it exceeds `SURFACE_TITLE_MAX`.
    ///
    /// If `input` is not valid UTF-8 or contains an unfinished codepoint at the
    /// truncation point, the longest valid UTF-8 prefix that fits is stored.
    pub fn from_bytes(input: &[u8]) -> Self {
        let mut bytes = [0u8; SURFACE_TITLE_MAX];
        let mut cut = core::cmp::min(input.len(), SURFACE_TITLE_MAX);
        // Walk back to a UTF-8 char boundary. Continuation bytes have the
        // top two bits == 10; lead bytes do not. We never cross over a lead
        // byte's continuations.
        while cut > 0 && (input[cut - 1] & 0xC0) == 0x80 && {
            // Walk past contiguous continuation bytes to find the lead.
            let mut probe = cut - 1;
            while probe > 0 && (input[probe] & 0xC0) == 0x80 {
                probe -= 1;
            }
            // Decide whether the lead at `probe` plus its continuations fit
            // entirely within [0, cut). If not, we must truncate before `probe`.
            let lead = input[probe];
            let expected_len = if lead & 0x80 == 0 {
                1
            } else if lead & 0xE0 == 0xC0 {
                2
            } else if lead & 0xF0 == 0xE0 {
                3
            } else if lead & 0xF8 == 0xF0 {
                4
            } else {
                1 // invalid lead — treat as 1-byte to make progress
            };
            cut < probe + expected_len
        } {
            cut -= 1;
        }
        // Also handle the case where the byte at cut-1 is itself a lead with
        // outstanding continuations beyond `cut`.
        if cut > 0 {
            let lead = input[cut - 1];
            let expected_len = if lead & 0x80 == 0 {
                1
            } else if lead & 0xE0 == 0xC0 {
                2
            } else if lead & 0xF0 == 0xE0 {
                3
            } else if lead & 0xF8 == 0xF0 {
                4
            } else {
                1
            };
            if expected_len > 1 && cut < (cut - 1) + expected_len {
                cut -= 1;
            }
        }
        bytes[..cut].copy_from_slice(&input[..cut]);
        Self {
            bytes,
            len: cut as u8,
        }
    }

    /// View the title as a byte slice of length `self.len`.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }
}

/// Coarse content classification used by the compositor for layout hints.
///
/// Phase 7 M24: stored on each surface but not yet acted on. Layer 2 (smart
/// desktop) consumes this to drive context-aware layout in later phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SurfaceContentType {
    /// Text-heavy reading/editing surface.
    Document = 0,
    /// Monospace terminal emulator.
    Terminal = 1,
    /// Web content renderer.
    Browser = 2,
    /// Real-time interactive content (low-latency, fullscreen-friendly).
    Game = 3,
    /// System or application preferences.
    Settings = 4,
    /// System chrome (taskbar, status strip, launcher).
    SystemUI = 5,
    /// Unspecified / generic.
    Generic = 6,
}

impl SurfaceContentType {
    /// Convert from a raw u8 discriminant. Returns `None` on unknown values.
    pub const fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Self::Document),
            1 => Some(Self::Terminal),
            2 => Some(Self::Browser),
            3 => Some(Self::Game),
            4 => Some(Self::Settings),
            5 => Some(Self::SystemUI),
            6 => Some(Self::Generic),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Damage tracking
// ---------------------------------------------------------------------------

/// Region of a surface buffer that changed since the last submission.
///
/// Phase 7 supports a single rectangle, full-surface, and empty. Multi-rect
/// damage lists are a Phase 25 optimization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageRegion {
    /// A rectangular subregion changed (surface-local pixel coordinates).
    Rect {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
    /// The entire surface changed (full repaint).
    FullSurface,
    /// Nothing changed since the last frame; compositor skips this surface.
    Empty,
}

impl DamageRegion {
    /// Returns true if this region encompasses any pixels.
    pub const fn has_damage(&self) -> bool {
        match self {
            Self::Empty => false,
            Self::Rect { width, height, .. } => *width > 0 && *height > 0,
            Self::FullSurface => true,
        }
    }
}

/// Screen-space damage accumulator used by the compositor during one frame.
///
/// Layer 1: a single bounding rect per output. Multi-rect dirty lists are
/// deferred. The accumulator unions every reported rect into one bounding box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DamageTracker {
    /// Inclusive damage bounds; `None` means no damage this frame.
    bounds: Option<DamageRect>,
    /// Whether a full-output redraw is pending (suppresses the bounding rect).
    full: bool,
}

/// Inclusive rectangle used by the screen-space damage accumulator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DamageRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl DamageTracker {
    /// Empty tracker — nothing damaged.
    pub const fn new() -> Self {
        Self {
            bounds: None,
            full: false,
        }
    }

    /// Returns true if any damage is recorded for this frame.
    pub const fn has_damage(&self) -> bool {
        self.full || self.bounds.is_some()
    }

    /// Returns true if a full-output redraw is pending.
    pub const fn is_full(&self) -> bool {
        self.full
    }

    /// Mark the entire output as damaged.
    pub fn mark_full(&mut self) {
        self.full = true;
        self.bounds = None;
    }

    /// Add a rectangular damage region (in output coordinates).
    ///
    /// Unioned with any previously added regions. No-op when `mark_full` was
    /// called earlier in the same frame.
    pub fn union(&mut self, rect: DamageRect) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        if self.full {
            return;
        }
        self.bounds = Some(match self.bounds {
            None => rect,
            Some(existing) => union_rect(existing, rect),
        });
    }

    /// Current bounding rect, if any. Returns `None` when `is_full()`.
    pub const fn bounds(&self) -> Option<DamageRect> {
        if self.full {
            None
        } else {
            self.bounds
        }
    }

    /// Reset to the empty state at the start of a new frame.
    pub fn clear(&mut self) {
        self.bounds = None;
        self.full = false;
    }
}

impl Default for DamageTracker {
    fn default() -> Self {
        Self::new()
    }
}

const fn union_rect(a: DamageRect, b: DamageRect) -> DamageRect {
    let x0 = if a.x < b.x { a.x } else { b.x };
    let y0 = if a.y < b.y { a.y } else { b.y };
    let a_x1 = a.x + a.width;
    let a_y1 = a.y + a.height;
    let b_x1 = b.x + b.width;
    let b_y1 = b.y + b.height;
    let x1 = if a_x1 > b_x1 { a_x1 } else { b_x1 };
    let y1 = if a_y1 > b_y1 { a_y1 } else { b_y1 };
    DamageRect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    }
}

// ---------------------------------------------------------------------------
// IPC protocol — request/response/event wire format
// ---------------------------------------------------------------------------

/// Compositor command discriminants for IPC requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum CompositorCommand {
    /// Allocate a new surface; compositor returns SurfaceId in the response.
    CreateSurface = 1,
    /// Attach a shared-memory buffer to an existing surface.
    AttachBuffer = 2,
    /// Tear down a surface and release any associated tracking.
    DestroySurface = 3,
    /// Request a new size — compositor responds with a Configure event.
    Resize = 4,
    /// Move a surface to a different z-order layer.
    SetLayer = 5,
}

impl CompositorCommand {
    /// Convert from a raw u32 discriminant.
    pub const fn from_u32(val: u32) -> Option<Self> {
        match val {
            1 => Some(Self::CreateSurface),
            2 => Some(Self::AttachBuffer),
            3 => Some(Self::DestroySurface),
            4 => Some(Self::Resize),
            5 => Some(Self::SetLayer),
            _ => None,
        }
    }
}

/// Compositor request — fixed-size, repr(C), serialized into RawMessage.data.
///
/// All fields are always present; unused fields are zeroed. Discriminate via
/// `command` and read only the fields relevant to that command.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct CompositorRequest {
    /// Command discriminant (matches `CompositorCommand`).
    pub command: u32,
    /// SurfaceId.0 (for AttachBuffer, DestroySurface, Resize, SetLayer; 0 on CreateSurface).
    pub surface_id: u64,
    /// Surface width (CreateSurface, Resize).
    pub width: u32,
    /// Surface height (CreateSurface, Resize).
    pub height: u32,
    /// Initial layer (CreateSurface, SetLayer) — `SurfaceLayer as u8`.
    pub layer: u8,
    /// Initial content type (CreateSurface) — `SurfaceContentType as u8`.
    pub content_type: u8,
    /// Damage region tag (AttachBuffer): 0 = Empty, 1 = FullSurface, 2 = Rect.
    pub damage_tag: u8,
    /// Padding to align `title` to a 1-byte boundary (no-op; documents intent).
    pub _pad: u8,
    /// Title bytes (CreateSurface). Length carried in `title_len`.
    pub title: [u8; SURFACE_TITLE_MAX],
    /// Title byte count.
    pub title_len: u8,
    /// Padding to keep total size predictable.
    pub _pad2: [u8; 7],
    /// Shared memory region id (AttachBuffer).
    pub shmem_id: u32,
    /// Damage rect x (AttachBuffer when damage_tag=2).
    pub damage_x: u32,
    /// Damage rect y (AttachBuffer when damage_tag=2).
    pub damage_y: u32,
    /// Damage rect width (AttachBuffer when damage_tag=2).
    pub damage_w: u32,
    /// Damage rect height (AttachBuffer when damage_tag=2).
    pub damage_h: u32,
}

impl CompositorRequest {
    /// All-zero request (used as scratch buffer or builder base).
    pub const fn zeroed() -> Self {
        Self {
            command: 0,
            surface_id: 0,
            width: 0,
            height: 0,
            layer: 0,
            content_type: 0,
            damage_tag: 0,
            _pad: 0,
            title: [0; SURFACE_TITLE_MAX],
            title_len: 0,
            _pad2: [0; 7],
            shmem_id: 0,
            damage_x: 0,
            damage_y: 0,
            damage_w: 0,
            damage_h: 0,
        }
    }

    /// Decode the damage region carried by this request (only meaningful for AttachBuffer).
    pub fn decode_damage(&self) -> DamageRegion {
        match self.damage_tag {
            0 => DamageRegion::Empty,
            1 => DamageRegion::FullSurface,
            2 => DamageRegion::Rect {
                x: self.damage_x,
                y: self.damage_y,
                width: self.damage_w,
                height: self.damage_h,
            },
            _ => DamageRegion::Empty,
        }
    }
}

/// Event tag for compositor → client messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum CompositorEventTag {
    Configure = 1,
    FocusChanged = 2,
    CloseRequested = 3,
    BufferReleased = 4,
    FramePresented = 5,
    Input = 6,
}

impl CompositorEventTag {
    /// Convert from a raw u32 discriminant.
    pub const fn from_u32(val: u32) -> Option<Self> {
        match val {
            1 => Some(Self::Configure),
            2 => Some(Self::FocusChanged),
            3 => Some(Self::CloseRequested),
            4 => Some(Self::BufferReleased),
            5 => Some(Self::FramePresented),
            6 => Some(Self::Input),
            _ => None,
        }
    }
}

/// Compositor event — fixed-size, repr(C), sent from compositor to client via IPC.
///
/// The variant is identified by `tag` (a `CompositorEventTag`); only the
/// fields relevant to that variant are populated.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct CompositorEvent {
    /// Event variant tag.
    pub tag: u32,
    /// Surface id this event refers to (0 if not surface-bound).
    pub surface_id: u64,
    /// New surface width (Configure).
    pub width: u32,
    /// New surface height (Configure).
    pub height: u32,
    /// Display scale × 100 (Configure). 100 = 1.0×, 200 = 2.0×.
    pub scale_x100: u32,
    /// Focus state (FocusChanged): 0 = lost, 1 = gained.
    pub focused: u8,
    /// Padding for explicit alignment.
    pub _pad: [u8; 3],
    /// Released shared memory region id (BufferReleased).
    pub shmem_id: u32,
    /// Frame presentation timestamp in 1 kHz timer ticks (FramePresented).
    pub timestamp_ticks: u64,
    /// Embedded input event (Input). Other variants leave this zeroed.
    pub input: InputEventBytes,
}

/// Wire-format byte buffer carrying a serialized `InputEvent`.
///
/// Decoded via `InputEventBytes::decode()`. Using a fixed byte buffer keeps
/// `CompositorEvent` `repr(C)` and avoids exposing `InputEvent`'s Rust enum
/// layout in the IPC protocol.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct InputEventBytes {
    pub tag: u32,
    pub key_or_buttons: u32,
    pub state_or_modifiers: u32,
    pub x: u32,
    pub y: u32,
    pub button_state: u32,
    pub _reserved: [u32; 2],
}

impl InputEventBytes {
    /// All-zero buffer.
    pub const fn zeroed() -> Self {
        Self {
            tag: 0,
            key_or_buttons: 0,
            state_or_modifiers: 0,
            x: 0,
            y: 0,
            button_state: 0,
            _reserved: [0; 2],
        }
    }

    /// Encode an `InputEvent` for IPC transport.
    ///
    /// Keyboard events store the evdev keycode (via `KeyCode::to_evdev`) so the
    /// wire format is a stable u16 rather than the Rust enum's discriminant.
    pub fn encode(event: &InputEvent) -> Self {
        let mut out = Self::zeroed();
        match event {
            InputEvent::Keyboard {
                key,
                state,
                modifiers,
            } => {
                out.tag = 1;
                out.key_or_buttons = key.to_evdev() as u32;
                out.state_or_modifiers = ((*state as u32) & 0xFF) | ((modifiers.0 as u32) << 8);
            }
            InputEvent::Pointer {
                x,
                y,
                button,
                state,
            } => {
                out.tag = 2;
                out.x = *x;
                out.y = *y;
                // Encode Option as 0 = None, button + 1 = Some(button).
                out.key_or_buttons = button.map(|b| b as u32 + 1).unwrap_or(0);
                out.button_state = state.map(|s| s as u32 + 1).unwrap_or(0);
            }
        }
        out
    }
}

impl CompositorEvent {
    /// All-zero event.
    pub const fn zeroed() -> Self {
        Self {
            tag: 0,
            surface_id: 0,
            width: 0,
            height: 0,
            scale_x100: 0,
            focused: 0,
            _pad: [0; 3],
            shmem_id: 0,
            timestamp_ticks: 0,
            input: InputEventBytes::zeroed(),
        }
    }

    /// Build a Configure event.
    pub const fn configure(surface: SurfaceId, width: u32, height: u32, scale_x100: u32) -> Self {
        let mut e = Self::zeroed();
        e.tag = CompositorEventTag::Configure as u32;
        e.surface_id = surface.0;
        e.width = width;
        e.height = height;
        e.scale_x100 = scale_x100;
        e
    }

    /// Build a FocusChanged event.
    pub const fn focus_changed(surface: SurfaceId, focused: bool) -> Self {
        let mut e = Self::zeroed();
        e.tag = CompositorEventTag::FocusChanged as u32;
        e.surface_id = surface.0;
        e.focused = if focused { 1 } else { 0 };
        e
    }

    /// Build a CloseRequested event.
    pub const fn close_requested(surface: SurfaceId) -> Self {
        let mut e = Self::zeroed();
        e.tag = CompositorEventTag::CloseRequested as u32;
        e.surface_id = surface.0;
        e
    }

    /// Build a BufferReleased event.
    pub const fn buffer_released(surface: SurfaceId, shmem_id: SharedMemoryId) -> Self {
        let mut e = Self::zeroed();
        e.tag = CompositorEventTag::BufferReleased as u32;
        e.surface_id = surface.0;
        e.shmem_id = shmem_id.0;
        e
    }

    /// Build a FramePresented event.
    pub const fn frame_presented(surface: SurfaceId, timestamp_ticks: u64) -> Self {
        let mut e = Self::zeroed();
        e.tag = CompositorEventTag::FramePresented as u32;
        e.surface_id = surface.0;
        e.timestamp_ticks = timestamp_ticks;
        e
    }

    /// Build an Input event for delivery to a focused surface.
    pub fn input(surface: SurfaceId, event: &InputEvent) -> Self {
        let mut e = Self::zeroed();
        e.tag = CompositorEventTag::Input as u32;
        e.surface_id = surface.0;
        e.input = InputEventBytes::encode(event);
        e
    }
}

// ---------------------------------------------------------------------------
// Compositor-internal addressing
// ---------------------------------------------------------------------------

/// Owner reference attached to each surface — kernel side only.
///
/// Carries the originating process and the channel that delivers events
/// back to that process. Pure data so it lives in shared for host testing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceOwner {
    pub pid: ProcessId,
    pub channel: ChannelId,
}

// ---------------------------------------------------------------------------
// Compile-time invariants
// ---------------------------------------------------------------------------

const _: () = assert!(core::mem::size_of::<CompositorRequest>() <= MAX_MESSAGE_SIZE);
const _: () = assert!(core::mem::size_of::<CompositorEvent>() <= MAX_MESSAGE_SIZE);

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{ButtonState, KeyCode, KeyState, Modifiers, MouseButton};

    #[test]
    fn request_fits_in_message() {
        assert!(core::mem::size_of::<CompositorRequest>() <= MAX_MESSAGE_SIZE);
    }

    #[test]
    fn event_fits_in_message() {
        assert!(core::mem::size_of::<CompositorEvent>() <= MAX_MESSAGE_SIZE);
    }

    #[test]
    fn surface_state_terminal() {
        assert!(SurfaceState::Destroyed.is_terminal());
        assert!(!SurfaceState::Active.is_terminal());
        assert!(!SurfaceState::Created.is_terminal());
    }

    #[test]
    fn surface_state_transitions_forward_path() {
        assert!(SurfaceState::Created.can_transition_to(SurfaceState::Configured));
        assert!(SurfaceState::Configured.can_transition_to(SurfaceState::Active));
        assert!(SurfaceState::Active.can_transition_to(SurfaceState::Active));
    }

    #[test]
    fn surface_state_transitions_suspend() {
        assert!(SurfaceState::Active.can_transition_to(SurfaceState::Suspended));
        assert!(SurfaceState::Suspended.can_transition_to(SurfaceState::Active));
    }

    #[test]
    fn surface_state_transitions_destroy_from_anywhere() {
        for s in [
            SurfaceState::Created,
            SurfaceState::Configured,
            SurfaceState::Active,
            SurfaceState::Suspended,
        ] {
            assert!(s.can_transition_to(SurfaceState::Destroyed));
        }
    }

    #[test]
    fn surface_state_transitions_invalid() {
        // Skipping Configured.
        assert!(!SurfaceState::Created.can_transition_to(SurfaceState::Active));
        // Regressing from Destroyed.
        assert!(!SurfaceState::Destroyed.can_transition_to(SurfaceState::Active));
        // Regressing from Active to Configured.
        assert!(!SurfaceState::Active.can_transition_to(SurfaceState::Configured));
    }

    #[test]
    fn surface_layer_ordering() {
        assert!(SurfaceLayer::Background < SurfaceLayer::Normal);
        assert!(SurfaceLayer::Normal < SurfaceLayer::TopLevel);
        assert!(SurfaceLayer::TopLevel < SurfaceLayer::Overlay);
        assert!(SurfaceLayer::Overlay < SurfaceLayer::Panel);
    }

    #[test]
    fn surface_title_short_input() {
        let t = SurfaceTitle::from_bytes(b"hello");
        assert_eq!(t.len, 5);
        assert_eq!(t.as_bytes(), b"hello");
    }

    #[test]
    fn surface_title_truncation_ascii() {
        let long = [b'a'; 100];
        let t = SurfaceTitle::from_bytes(&long);
        assert_eq!(t.len as usize, SURFACE_TITLE_MAX);
        assert_eq!(t.as_bytes().len(), SURFACE_TITLE_MAX);
    }

    #[test]
    fn surface_title_utf8_boundary_safety() {
        // Construct: 60 ASCII bytes followed by a 4-byte UTF-8 emoji ("🎯" U+1F3AF).
        // SURFACE_TITLE_MAX is 64; the emoji starts at byte 60, would extend to byte 63
        // inclusive — that fits exactly. Add another byte past the limit.
        let mut input = [0u8; 100];
        for byte in input.iter_mut().take(60) {
            *byte = b'a';
        }
        let emoji = "🎯".as_bytes();
        input[60..60 + emoji.len()].copy_from_slice(emoji);
        // Now add 'b' starting at 64; that byte is past the limit.
        input[64] = b'b';
        let t = SurfaceTitle::from_bytes(&input[..65]);
        // The truncation must not split the emoji.
        let stored = t.as_bytes();
        // Either the emoji is fully present (len = 64) or fully absent (len = 60).
        assert!(stored.len() == 64 || stored.len() == 60);
        // If present, it must be valid UTF-8 (no torn codepoint).
        assert!(core::str::from_utf8(stored).is_ok());
    }

    #[test]
    fn surface_title_truncation_utf8_mid_codepoint() {
        // 62 ASCII bytes + 4-byte emoji = 66 bytes total. Truncation at 64 must
        // back up before the emoji to 62.
        let mut input = [0u8; 100];
        for byte in input.iter_mut().take(62) {
            *byte = b'a';
        }
        let emoji = "🎯".as_bytes();
        input[62..62 + emoji.len()].copy_from_slice(emoji);
        let t = SurfaceTitle::from_bytes(&input[..66]);
        // The emoji starts at 62 and extends to 65. SURFACE_TITLE_MAX = 64, so it
        // does not fit and the truncation must back up to 62.
        assert_eq!(t.len, 62);
        assert!(core::str::from_utf8(t.as_bytes()).is_ok());
    }

    #[test]
    fn surface_id_none_sentinel() {
        assert!(SurfaceId::NONE.is_none());
        assert!(!SurfaceId::FIRST.is_none());
    }

    #[test]
    fn damage_region_empty_has_no_damage() {
        assert!(!DamageRegion::Empty.has_damage());
    }

    #[test]
    fn damage_region_zero_size_rect_has_no_damage() {
        assert!(!DamageRegion::Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 5
        }
        .has_damage());
    }

    #[test]
    fn damage_region_full_has_damage() {
        assert!(DamageRegion::FullSurface.has_damage());
    }

    #[test]
    fn damage_region_rect_with_size_has_damage() {
        assert!(DamageRegion::Rect {
            x: 5,
            y: 5,
            width: 10,
            height: 10
        }
        .has_damage());
    }

    #[test]
    fn damage_tracker_starts_empty() {
        let t = DamageTracker::new();
        assert!(!t.has_damage());
        assert!(!t.is_full());
        assert!(t.bounds().is_none());
    }

    #[test]
    fn damage_tracker_union_bounds() {
        let mut t = DamageTracker::new();
        t.union(DamageRect {
            x: 10,
            y: 10,
            width: 20,
            height: 20,
        });
        t.union(DamageRect {
            x: 50,
            y: 5,
            width: 10,
            height: 10,
        });
        let b = t.bounds().expect("bounds set");
        // Bounding rect spans (10..60, 5..30).
        assert_eq!(b.x, 10);
        assert_eq!(b.y, 5);
        assert_eq!(b.width, 50);
        assert_eq!(b.height, 25);
    }

    #[test]
    fn damage_tracker_full_suppresses_bounds() {
        let mut t = DamageTracker::new();
        t.union(DamageRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        });
        t.mark_full();
        assert!(t.is_full());
        assert!(t.bounds().is_none());
        // Subsequent union is a no-op.
        t.union(DamageRect {
            x: 100,
            y: 100,
            width: 5,
            height: 5,
        });
        assert!(t.bounds().is_none());
    }

    #[test]
    fn damage_tracker_clear_resets() {
        let mut t = DamageTracker::new();
        t.mark_full();
        t.clear();
        assert!(!t.has_damage());
    }

    #[test]
    fn compositor_command_round_trip() {
        for c in [
            CompositorCommand::CreateSurface,
            CompositorCommand::AttachBuffer,
            CompositorCommand::DestroySurface,
            CompositorCommand::Resize,
            CompositorCommand::SetLayer,
        ] {
            assert_eq!(CompositorCommand::from_u32(c as u32), Some(c));
        }
        assert_eq!(CompositorCommand::from_u32(0), None);
        assert_eq!(CompositorCommand::from_u32(99), None);
    }

    #[test]
    fn compositor_event_tag_round_trip() {
        for t in [
            CompositorEventTag::Configure,
            CompositorEventTag::FocusChanged,
            CompositorEventTag::CloseRequested,
            CompositorEventTag::BufferReleased,
            CompositorEventTag::FramePresented,
            CompositorEventTag::Input,
        ] {
            assert_eq!(CompositorEventTag::from_u32(t as u32), Some(t));
        }
        assert_eq!(CompositorEventTag::from_u32(0), None);
    }

    #[test]
    fn surface_content_type_round_trip() {
        for t in [
            SurfaceContentType::Document,
            SurfaceContentType::Terminal,
            SurfaceContentType::Browser,
            SurfaceContentType::Game,
            SurfaceContentType::Settings,
            SurfaceContentType::SystemUI,
            SurfaceContentType::Generic,
        ] {
            assert_eq!(SurfaceContentType::from_u8(t as u8), Some(t));
        }
        assert_eq!(SurfaceContentType::from_u8(99), None);
    }

    #[test]
    fn compositor_request_decode_damage() {
        let mut r = CompositorRequest::zeroed();
        r.damage_tag = 0;
        assert_eq!(r.decode_damage(), DamageRegion::Empty);
        r.damage_tag = 1;
        assert_eq!(r.decode_damage(), DamageRegion::FullSurface);
        r.damage_tag = 2;
        r.damage_x = 5;
        r.damage_y = 10;
        r.damage_w = 100;
        r.damage_h = 50;
        assert_eq!(
            r.decode_damage(),
            DamageRegion::Rect {
                x: 5,
                y: 10,
                width: 100,
                height: 50
            }
        );
    }

    #[test]
    fn compositor_event_configure_constructor() {
        let e = CompositorEvent::configure(SurfaceId(7), 800, 600, 100);
        assert_eq!(e.tag, CompositorEventTag::Configure as u32);
        assert_eq!(e.surface_id, 7);
        assert_eq!(e.width, 800);
        assert_eq!(e.height, 600);
        assert_eq!(e.scale_x100, 100);
    }

    #[test]
    fn compositor_event_input_keyboard_round_trip() {
        let original = InputEvent::Keyboard {
            key: KeyCode::A,
            state: KeyState::Pressed,
            modifiers: Modifiers(Modifiers::SHIFT),
        };
        let e = CompositorEvent::input(SurfaceId(3), &original);
        assert_eq!(e.tag, CompositorEventTag::Input as u32);
        assert_eq!(e.surface_id, 3);
        assert_eq!(e.input.tag, 1);
        // Wire format carries evdev keycode (stable u16), not the Rust enum
        // discriminant. KeyCode::A maps to evdev KEY_A (30).
        assert_eq!(e.input.key_or_buttons, KeyCode::A.to_evdev() as u32);
        // state in low byte, modifiers in next byte.
        assert_eq!(e.input.state_or_modifiers & 0xFF, KeyState::Pressed as u32);
        assert_eq!(
            (e.input.state_or_modifiers >> 8) & 0xFF,
            Modifiers::SHIFT as u32
        );
    }

    #[test]
    fn compositor_event_input_pointer_encoded() {
        let original = InputEvent::Pointer {
            x: 640,
            y: 400,
            button: Some(MouseButton::Left),
            state: Some(ButtonState::Pressed),
        };
        let e = CompositorEvent::input(SurfaceId(1), &original);
        assert_eq!(e.input.tag, 2);
        assert_eq!(e.input.x, 640);
        assert_eq!(e.input.y, 400);
        // Button is encoded as +1 to leave 0 for "no button".
        assert_eq!(e.input.key_or_buttons, MouseButton::Left as u32 + 1);
        assert_eq!(e.input.button_state, ButtonState::Pressed as u32 + 1);
    }
}
