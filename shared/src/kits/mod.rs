//! Kit trait hierarchies for AIOS subsystems.
//!
//! Each Kit defines a collection of Rust traits that formalize the contract a
//! subsystem exposes. Trait definitions live here in `shared/` so they are
//! testable on the host (`just test`). Kernel-side `impl` blocks live in
//! `kernel/`.
//!
//! See `docs/kits/README.md` for the full Kit architecture.

pub mod capability;
pub mod ipc;
pub mod memory;
pub mod storage;
