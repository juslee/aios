//! GPU subsystem: service, text rendering, and Compute Kit implementation.
//!
//! The GPU Service wraps the VirtIO-GPU 2D driver in a capability-gated
//! IPC service with double-buffered display management.

pub mod service;
pub mod text;
