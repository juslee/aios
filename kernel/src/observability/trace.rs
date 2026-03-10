//! Kernel trace points: compile-time switchable binary event records.
//!
//! Feature-gated: `cfg(feature = "kernel-tracing")`, off by default.
//! Per observability.md §4.

#[cfg(feature = "kernel-tracing")]
mod enabled {
    use crate::smp::MAX_CORES;
    use core::sync::atomic::{AtomicU32, Ordering};

    // -----------------------------------------------------------------------
    // Trace events (observability.md §4.2)
    // -----------------------------------------------------------------------

    /// Trace event variants. Binary format — no string formatting on hot path.
    #[repr(u8)]
    #[derive(Clone, Copy)]
    pub enum TraceEvent {
        // Scheduler
        SchedSwitch {
            prev_tid: u32,
            next_tid: u32,
            prev_state: u8,
        } = 0,
        SchedWakeup {
            tid: u32,
            target_core: u8,
        } = 1,
        SchedMigrate {
            tid: u32,
            from_core: u8,
            to_core: u8,
        } = 2,
        SchedYield {
            tid: u32,
        } = 3,
        SchedBlock {
            tid: u32,
            reason: u8,
        } = 4,
        // IPC
        IpcSendBegin {
            channel: u32,
            len: u32,
        } = 5,
        IpcSendEnd {
            channel: u32,
        } = 6,
        IpcRecvBegin {
            channel: u32,
        } = 7,
        IpcRecvEnd {
            channel: u32,
            len: u32,
        } = 8,
        IpcDirectSwitch {
            from_tid: u32,
            to_tid: u32,
        } = 9,
        // Memory
        PageAlloc {
            pool: u8,
            order: u8,
            phys: u64,
        } = 10,
        PageFree {
            pool: u8,
            order: u8,
            phys: u64,
        } = 11,
        SlabAlloc {
            size_class: u8,
            ptr: u64,
        } = 12,
        SlabFree {
            size_class: u8,
            ptr: u64,
        } = 13,
        TlbFlush {
            scope: u8,
        } = 14,
        // Interrupts
        IrqEnter {
            irq_num: u16,
        } = 15,
        IrqExit {
            irq_num: u16,
        } = 16,
    }

    // -----------------------------------------------------------------------
    // Trace record (observability.md §4.3)
    // -----------------------------------------------------------------------

    /// Compact trace record: 32 bytes.
    /// Uses explicit tag + payload for stable binary layout.
    #[repr(C)]
    pub struct TraceRecord {
        pub timestamp: u64,
        pub core_id: u8,
        pub event_tag: u8,
        pub event_data: [u8; 14],
        pub _pad: [u8; 8],
    }

    const _: () = assert!(core::mem::size_of::<TraceRecord>() == 32);

    impl TraceRecord {
        const ZERO: Self = Self {
            timestamp: 0,
            core_id: 0,
            event_tag: 0,
            event_data: [0; 14],
            _pad: [0; 8],
        };
    }

    // -----------------------------------------------------------------------
    // Per-core trace ring (observability.md §4.4)
    // -----------------------------------------------------------------------

    const TRACE_RING_SIZE: usize = 4096;
    const TRACE_RING_MASK: u32 = (TRACE_RING_SIZE as u32) - 1;

    pub struct TraceRing {
        entries: [TraceRecord; TRACE_RING_SIZE],
        head: AtomicU32,
        tail: AtomicU32,
    }

    impl TraceRing {
        const INIT: Self = Self {
            entries: [TraceRecord::ZERO; TRACE_RING_SIZE],
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
        };

        fn push(&self, record: TraceRecord) {
            let head = self.head.load(Ordering::Relaxed);
            let idx = (head & TRACE_RING_MASK) as usize;
            // SAFETY: Single producer (owning core).
            unsafe {
                let slot = &self.entries[idx] as *const TraceRecord as *mut TraceRecord;
                core::ptr::write(slot, record);
            }
            self.head.store(head.wrapping_add(1), Ordering::Release);
        }
    }

    // SAFETY: TraceRing is SPSC per-core.
    unsafe impl Sync for TraceRing {}

    static TRACE_RINGS: [TraceRing; MAX_CORES] = [const { TraceRing::INIT }; MAX_CORES];

    /// Encode a TraceEvent into a TraceRecord and push to per-core ring.
    pub fn record(event: TraceEvent) {
        let core = crate::observability::current_core_id().min(MAX_CORES - 1);
        let timestamp: u64;
        // SAFETY: CNTVCT_EL0 is always readable at EL1.
        unsafe { core::arch::asm!("mrs {}, CNTVCT_EL0", out(reg) timestamp) };

        let mut event_data = [0u8; 14];
        let event_tag = encode_event(&event, &mut event_data);

        let record = TraceRecord {
            timestamp,
            core_id: core as u8,
            event_tag,
            event_data,
            _pad: [0; 8],
        };

        TRACE_RINGS[core].push(record);
    }

    /// Encode event variant into tag + data bytes.
    fn encode_event(event: &TraceEvent, data: &mut [u8; 14]) -> u8 {
        match *event {
            TraceEvent::SchedSwitch {
                prev_tid,
                next_tid,
                prev_state,
            } => {
                data[0..4].copy_from_slice(&prev_tid.to_le_bytes());
                data[4..8].copy_from_slice(&next_tid.to_le_bytes());
                data[8] = prev_state;
                0
            }
            TraceEvent::SchedWakeup { tid, target_core } => {
                data[0..4].copy_from_slice(&tid.to_le_bytes());
                data[4] = target_core;
                1
            }
            TraceEvent::SchedMigrate {
                tid,
                from_core,
                to_core,
            } => {
                data[0..4].copy_from_slice(&tid.to_le_bytes());
                data[4] = from_core;
                data[5] = to_core;
                2
            }
            TraceEvent::SchedYield { tid } => {
                data[0..4].copy_from_slice(&tid.to_le_bytes());
                3
            }
            TraceEvent::SchedBlock { tid, reason } => {
                data[0..4].copy_from_slice(&tid.to_le_bytes());
                data[4] = reason;
                4
            }
            TraceEvent::IpcSendBegin { channel, len } => {
                data[0..4].copy_from_slice(&channel.to_le_bytes());
                data[4..8].copy_from_slice(&len.to_le_bytes());
                5
            }
            TraceEvent::IpcSendEnd { channel } => {
                data[0..4].copy_from_slice(&channel.to_le_bytes());
                6
            }
            TraceEvent::IpcRecvBegin { channel } => {
                data[0..4].copy_from_slice(&channel.to_le_bytes());
                7
            }
            TraceEvent::IpcRecvEnd { channel, len } => {
                data[0..4].copy_from_slice(&channel.to_le_bytes());
                data[4..8].copy_from_slice(&len.to_le_bytes());
                8
            }
            TraceEvent::IpcDirectSwitch { from_tid, to_tid } => {
                data[0..4].copy_from_slice(&from_tid.to_le_bytes());
                data[4..8].copy_from_slice(&to_tid.to_le_bytes());
                9
            }
            TraceEvent::PageAlloc { pool, order, phys } => {
                data[0] = pool;
                data[1] = order;
                data[2..10].copy_from_slice(&phys.to_le_bytes());
                10
            }
            TraceEvent::PageFree { pool, order, phys } => {
                data[0] = pool;
                data[1] = order;
                data[2..10].copy_from_slice(&phys.to_le_bytes());
                11
            }
            TraceEvent::SlabAlloc { size_class, ptr } => {
                data[0] = size_class;
                data[1..9].copy_from_slice(&ptr.to_le_bytes());
                12
            }
            TraceEvent::SlabFree { size_class, ptr } => {
                data[0] = size_class;
                data[1..9].copy_from_slice(&ptr.to_le_bytes());
                13
            }
            TraceEvent::TlbFlush { scope } => {
                data[0] = scope;
                14
            }
            TraceEvent::IrqEnter { irq_num } => {
                data[0..2].copy_from_slice(&irq_num.to_le_bytes());
                15
            }
            TraceEvent::IrqExit { irq_num } => {
                data[0..2].copy_from_slice(&irq_num.to_le_bytes());
                16
            }
        }
    }
}

// Re-exports for the feature-enabled case.
#[cfg(feature = "kernel-tracing")]
pub use enabled::{record, TraceEvent};

/// Emit a trace event. Compiles to nothing when `kernel-tracing` is disabled.
#[macro_export]
macro_rules! trace_point {
    ($event:expr) => {
        #[cfg(feature = "kernel-tracing")]
        {
            $crate::observability::trace::record($event);
        }
    };
}
