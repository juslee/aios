//! `aios loop`, the PR loop.
//!
//! R2a is `aios loop review`, read-only: every gh call goes through
//! `crate::gh::GhCli`, which runs `gh::is_read`, no stage gets a write tool,
//! and nothing is committed or pushed. R2b adds the write side.

pub mod claude;
pub mod config;
pub mod gate;
pub mod git;
pub mod github;
pub mod schema;
pub mod settings;
pub mod state;
