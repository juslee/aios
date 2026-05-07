//! Window compositor — system service that owns the display.
//!
//! The compositor is a kernel-side system service following the GPU Service
//! pattern (`crate::gpu::service`). It receives surface lifecycle requests
//! from clients via IPC, composes their shared-memory buffers into a
//! double-buffered DMA composition buffer, and presents through the
//! VirtIO-GPU driver.
//!
//! M24 established the service skeleton, capability grants, IPC channel,
//! and the global flag that signals the GPU Service to release the display.
//! M25 layers in the window manager, decoration rendering, hit-testing and
//! cursor (Steps 17–18), focus management (Step 19), the input pipeline
//! and IPC dispatch (Steps 20, 21), and system hotkeys (Step 22).
//!
//! Per docs/platform/compositor.md and docs/platform/compositor/protocol.md.

use core::sync::atomic::AtomicBool;

pub mod cursor;
pub mod focus;
pub mod hotkey;
pub mod input_route;
pub mod render;
pub mod service;
pub mod shell;
pub mod surface;
pub mod text;
pub mod window;

// ---------------------------------------------------------------------------
// Display ownership flag
// ---------------------------------------------------------------------------

/// `true` once the compositor has taken control of the display from the
/// GPU Service. Read by the GPU Service to gate scanout-changing operations
/// after handoff. Stored as an `AtomicBool` so cross-core reads need no lock.
///
/// Set during display handoff (Step 11); never cleared (M24 has no
/// compositor-shutdown path — that's Phase 18 territory).
#[allow(dead_code)] // Wired up by Step 11; read by GPU Service in Step 11.
pub static COMPOSITOR_ACTIVE: AtomicBool = AtomicBool::new(false);
