//! AIOS host tooling: the library behind the `aios` binary.
//!
//! R1 provides `docs-check`, a byte-for-byte port of `scripts/docs/check.py`;
//! later PRs add the agent-loop commands. The crate is host-only (`std`) and is
//! a workspace member but never a default member, so `cargo build --target
//! aarch64-unknown-none` never selects it.
#![forbid(unsafe_code)]

pub mod cmd;
pub mod paths;
pub mod proc;
pub mod pystr;
