//! SIGINT, SIGTERM and SIGHUP while a soak runs. The script trapped INT and TERM
//! (`cleanup; exit 130` / `exit 143`); the harness now supervises QEMU itself,
//! so it also catches HUP (exit 129), which would otherwise leave QEMU running
//! with nothing left to stop it at the time limit.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};

/// The signals caught during a soak, and the exit status each one leads to.
const CAUGHT: [(i32, usize); 3] = [(SIGINT, 130), (SIGTERM, 143), (SIGHUP, 129)];

/// Records the last of SIGINT, SIGTERM or SIGHUP to arrive, instead of letting
/// it end the process, so the caller can stop QEMU and clean up first.
pub struct Interrupts {
    code: Arc<AtomicUsize>,
}

impl Interrupts {
    /// Replace the default action of the three signals for the rest of the process.
    pub fn install() -> Result<Interrupts> {
        let code = Arc::new(AtomicUsize::new(0));
        for (signal, exit) in CAUGHT {
            signal_hook::flag::register_usize(signal, Arc::clone(&code), exit)
                .with_context(|| format!("cannot install a handler for signal {signal}"))?;
        }
        Ok(Interrupts { code })
    }

    /// The exit status the harness should end with, once a signal has arrived.
    pub fn pending(&self) -> Option<u8> {
        match self.code.load(Ordering::SeqCst) {
            0 => None,
            code => Some(u8::try_from(code).expect("an exit status from CAUGHT")),
        }
    }
}
