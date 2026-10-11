//! Kernel synchronisation primitives.
//!
//! - [`irq_spin_lock`]: [`IrqSpinLock`], the detect-only spinlock of the 9
//!   statics that the timer IRQ path shares with thread code (crash-fix step
//!   1b), with [`held_by_stream`] and [`note_restore`], its run-time checks,
//!   and [`held_by_own_stream`] for the heartbeat scans' skip counters.
//! - `selftest` (feature `tripwire-selftest` only): a thread that holds
//!   THREAD_TABLE with IRQs on, to prove the PANIC-LOCK path end to end.

pub mod irq_spin_lock;
#[cfg(feature = "tripwire-selftest")]
pub mod selftest;

pub use irq_spin_lock::{held_by_own_stream, held_by_stream, note_restore, IrqSpinLock};
