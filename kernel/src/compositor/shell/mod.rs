//! Desktop shell — compositor-internal Panel/Normal layer surfaces.
//!
//! M26 introduces three shell surfaces (Status Strip, Taskbar, Workspace)
//! that are owned by the compositor process (`ProcessId(10)`) rather than
//! external clients. They are registered in `SURFACE_TABLE` like any other
//! surface but never have IPC events delivered to them — `service::is_self_channel`
//! suppresses self-deliveries to the compositor's well-known channel.
//!
//! Step 24 ships the Status Strip; Step 25 adds the Taskbar; Step 26
//! adds the Workspace home view plus bare-Super edge-detected toggle;
//! Step 27 layers shell input integration on the same scaffolding.
//!
//! Per docs/experience/experience.md §2 (Five Surfaces), §3
//! (Workspace), §6 (Status Strip), and the M26 working plan in
//! `docs/knowledge/plans/phase-7-m26-desktop-shell.md`.

pub mod status_strip;
pub mod taskbar;
pub mod workspace;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur while initializing or driving the desktop shell.
///
/// These mirror the underlying surface / shmem / capability error paths but
/// are surfaced as a single enum so the caller (the compositor service
/// `compositor_loop`) can log a single warning and continue running headless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellError {
    /// `surface_create` failed (table full, invalid size).
    SurfaceCreate,
    /// `shared_memory_create` failed (capability denied, OOM, table full).
    ShmemCreate,
    /// `surface_attach_buffer` failed (state machine, ownership).
    AttachBuffer,
    /// The display has zero width or height; nothing to render onto.
    NoDisplay,
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// Allocate the shell surfaces and seed their backing buffers.
///
/// Called once by `compositor::service::compositor_loop` after a successful
/// `display_handoff`. On error the shell is left uninitialized — the
/// compositor still composes client surfaces; the user simply sees no
/// shell chrome until the next boot. The caller must NOT hold any
/// compositor mutex at the time of the call (this function acquires
/// `SHARED_REGION_TABLE` and `SURFACE_TABLE` in the documented order).
pub fn init_shell_surfaces(display_width: u32, display_height: u32) -> Result<(), ShellError> {
    if display_width == 0 || display_height == 0 {
        return Err(ShellError::NoDisplay);
    }
    status_strip::init(display_width)?;
    taskbar::init(display_width, display_height)?;
    // Workspace failure is non-fatal — the user keeps a usable desktop
    // (Status Strip + Taskbar) even if storage's space_list is broken
    // or the display is too narrow for the home view. Log and continue.
    if let Err(e) = workspace::init(display_width, display_height) {
        crate::kwarn!(
            Compositor,
            "shell: workspace init failed ({:?}); home view disabled",
            e
        );
    }
    Ok(())
}

/// Per-loop tick called by the compositor service main loop.
///
/// `now_ms` is the current value of `arch::aarch64::timer::TICK_COUNT`
/// (1 kHz monotonic ticks since boot). The shell decides internally
/// whether each sub-surface needs to redraw — typical cadence is once
/// per second. No-op when the shell was never initialized (e.g., the
/// compositor is running headless).
pub fn tick(now_ms: u64) {
    status_strip::tick(now_ms);
    taskbar::tick(now_ms);
    workspace::tick(now_ms);
}

// ---------------------------------------------------------------------------
// Pointer dispatch (M26 Step 27)
// ---------------------------------------------------------------------------

/// Route a pointer event that resolved to a shell surface.
///
/// Called by the input router (`input_route::deliver_to_surface`)
/// whenever a pointer event's target is a shell surface (per
/// `surface::is_shell_id`). Dispatches based on which shell the id
/// matches:
///   * Status Strip — drop (non-interactive in M26 per phase doc).
///   * Taskbar — `taskbar::handle_pointer` resolves a workspace-button
///     or entry click to a focus / toggle action.
///   * Workspace — `workspace::handle_pointer`, a silent no-op in M26
///     (Layer 1 home view has no interactive cells).
///   * Unknown shell-classified id — drop.
///
/// Returns silently in all cases — pointer events targeted at shells
/// are never forwarded to client surfaces or to the decoration
/// machinery.
pub fn route_pointer(id: shared::compositor::SurfaceId, event: &shared::input::InputEvent) {
    if Some(id) == taskbar::surface_id() {
        taskbar::handle_pointer(event);
    } else if Some(id) == workspace::surface_id() {
        workspace::handle_pointer(event);
    } else if Some(id) == status_strip::surface_id() {
        // Drop — Status Strip is non-interactive in M26.
    }
    // Unknown shell-classified id (shouldn't happen): drop silently.
}
