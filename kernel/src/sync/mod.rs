//! Kernel synchronisation primitives.
//!
//! - [`irq_spin_lock`]: [`IrqSpinLock`], the detect-only spinlock of the 9
//!   statics that the timer IRQ path shares with thread code (crash-fix step
//!   1b), with [`held_by_stream`] and [`note_restore`], its run-time checks,
//!   and [`held_by_own_stream`] for the heartbeat scans' skip counters.

pub mod irq_spin_lock;

pub use irq_spin_lock::{held_by_own_stream, held_by_stream, note_restore, IrqSpinLock};
