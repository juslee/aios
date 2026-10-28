//! `aios guard`: the Rust port of `.claude/hooks/git-push-guard.py` (R5).
//!
//! The port is exact (plan D1): every decision, exit code and reason byte is the
//! Python guard's, the gaps of issue #241 included, until the R5b cutover. Each
//! module names the Python lines it ports.
//!
//! - [`shell`]: the shell model (`ShellParser`, `brace_expand`, `parse_commands`).
//! - [`shlex`]: Python's `shlex.quote` and `shlex.split`.

pub mod shell;
pub mod shlex;
