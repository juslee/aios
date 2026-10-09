//! Terminal signals while a soak runs. The script trapped INT and TERM
//! (`cleanup; exit 130` / `exit 143`); the harness now supervises QEMU itself,
//! so it also catches HUP (exit 129) and QUIT (exit 131, Ctrl-\), the other
//! terminal signals that end a process. Either would otherwise end the harness
//! and leave QEMU, in its own process group, running with nothing left to stop
//! it at the time limit. SIGKILL cannot be caught: a harness killed by it leaves
//! the current boot's QEMU running until it is killed by hand. A signal that
//! was ignored when the harness started is caught too (unlike bash, which keeps
//! an ignored signal ignored): reading the old disposition needs `sigaction`,
//! which `forbid(unsafe_code)` rules out, so `nohup` does not protect a soak,
//! as the help text says.
//!
//! SIGTSTP (Ctrl-Z) suspends the harness as usual, but while a boot's QEMU runs
//! the harness first stops QEMU's group too, which the terminal's SIGTSTP never
//! reaches: a harness stopped on its own would leave QEMU running past its time
//! limit (the script's `timeout` ran in its own group and kept counting). The
//! uncatchable SIGSTOP, and SIGTTIN or SIGTTOU stopping a background harness,
//! stop only the harness, so QEMU runs on unbounded until it is continued.
//! SIGTTOU is not caught because a caught one does not stop a background write
//! to the terminal: the write restarts and raises it again, forever.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use signal_hook::consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM, SIGTSTP};

/// The signals caught during a soak, and the exit status each one leads to.
const CAUGHT: [(i32, usize); 4] = [(SIGINT, 130), (SIGTERM, 143), (SIGHUP, 129), (SIGQUIT, 131)];

/// Records the last of SIGINT, SIGTERM, SIGHUP or SIGQUIT to arrive, instead of
/// letting it end the process, so the caller can stop QEMU and clean up first;
/// and handles SIGTSTP (see [`Interrupts::defer_suspend`]).
pub struct Interrupts {
    code: Arc<AtomicUsize>,
    /// SIGTSTP arrived and has not been taken by [`Interrupts::take_suspend`].
    suspend: Arc<AtomicBool>,
    /// SIGTSTP stops the process at once, from the signal handler, as its
    /// default action does. Cleared while a boot's QEMU runs.
    stop_at_once: Arc<AtomicBool>,
}

impl Interrupts {
    /// Replace the default action of the four signals for the rest of the
    /// process, and handle SIGTSTP, which still stops the process at once
    /// outside [`Interrupts::defer_suspend`].
    pub fn install() -> Result<Interrupts> {
        let code = Arc::new(AtomicUsize::new(0));
        for (signal, exit) in CAUGHT {
            signal_hook::flag::register_usize(signal, Arc::clone(&code), exit)
                .with_context(|| format!("cannot install a handler for signal {signal}"))?;
        }
        let suspend = Arc::new(AtomicBool::new(false));
        let stop_at_once = Arc::new(AtomicBool::new(true));
        // The stop goes first: while it applies, the flag is set only once the
        // process has been continued, and defer_suspend clears it.
        signal_hook::flag::register_conditional_default(SIGTSTP, Arc::clone(&stop_at_once))
            .and_then(|_| signal_hook::flag::register(SIGTSTP, Arc::clone(&suspend)))
            .with_context(|| format!("cannot install a handler for signal {SIGTSTP}"))?;
        Ok(Interrupts {
            code,
            suspend,
            stop_at_once,
        })
    }

    /// The exit status the harness should end with, once a signal has arrived.
    pub fn pending(&self) -> Option<u8> {
        match self.code.load(Ordering::SeqCst) {
            0 => None,
            code => Some(u8::try_from(code).expect("an exit status from CAUGHT")),
        }
    }

    /// Until the returned guard is dropped, SIGTSTP only records itself for
    /// [`Interrupts::take_suspend`], so the caller can stop QEMU before the
    /// harness stops with [`Interrupts::stop_self`]. Hold it only while no child
    /// runs in the terminal's foreground group: such a child would stop on the
    /// SIGTSTP while the harness waits for it. Dropping the guard stops the
    /// harness if a SIGTSTP is still untaken, so none is lost.
    pub fn defer_suspend(&self) -> DeferSuspend<'_> {
        self.suspend.store(false, Ordering::SeqCst);
        self.stop_at_once.store(false, Ordering::SeqCst);
        DeferSuspend { interrupts: self }
    }

    /// Whether a SIGTSTP arrived since the last call (or since
    /// [`Interrupts::defer_suspend`]).
    pub fn take_suspend(&self) -> bool {
        self.suspend.swap(false, Ordering::SeqCst)
    }

    /// Stop the harness, as SIGTSTP's default action does, until SIGCONT.
    pub fn stop_self(&self) {
        // Only an unknown signal number fails, and SIGTSTP is known.
        let _ = signal_hook::low_level::emulate_default_handler(SIGTSTP);
    }
}

/// While alive, SIGTSTP is deferred; see [`Interrupts::defer_suspend`].
pub struct DeferSuspend<'a> {
    interrupts: &'a Interrupts,
}

impl Drop for DeferSuspend<'_> {
    fn drop(&mut self) {
        self.interrupts.stop_at_once.store(true, Ordering::SeqCst);
        if self.interrupts.take_suspend() {
            self.interrupts.stop_self();
        }
    }
}
