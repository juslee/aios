//! Window compositor — system service that owns the display.
//!
//! The compositor is a kernel-side system service following the GPU Service
//! pattern (`crate::gpu::service`). It receives surface lifecycle requests
//! from clients via IPC, composes their shared-memory buffers into a
//! double-buffered DMA composition buffer, and presents through the
//! VirtIO-GPU driver.
//!
//! M24 establishes the service skeleton, capability grants, IPC channel,
//! and the global flag that signals the GPU Service to release the display.
//! Surface lifecycle, the actual render pipeline, and the composition loop
//! arrive in Steps 12-15.
//!
//! Per docs/platform/compositor.md and docs/platform/compositor/protocol.md.

use core::sync::atomic::AtomicBool;

pub mod render;
pub mod service;
pub mod surface;

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
