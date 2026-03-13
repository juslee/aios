//! Gate 1 benchmark: IPC round-trip, context switch, and shared memory throughput.
//!
//! Runs all benchmarks sequentially from a single thread, producing structured
//! output with avg/p99 latencies. Gate 1 criteria:
//! - IPC round-trip < 10 us
//! - Context switch < 20 us

use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use spin::Mutex;

use crate::arch::aarch64::timer;
use crate::arch::aarch64::uart::UartWriter;
use crate::cap;
use crate::ipc::{self, ChannelId};
use crate::sched;
use crate::task::process::{KernelResourceLimits, ProcessControl, ProcessId, PROCESS_TABLE};
use crate::task::{CpuSet, SchedulerClass, Thread, ThreadId};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Number of IPC round-trip iterations.
const IPC_ITERATIONS: usize = 10_000;

/// Number of context switch iterations.
const CTX_SWITCH_ITERATIONS: usize = 1_000;

/// Shared memory throughput test size (256 KiB).
const SHM_SIZE: usize = 256 * 1024;

/// Maximum samples for p99 computation.
const MAX_SAMPLES: usize = 10_000;

// ---------------------------------------------------------------------------
// Bench result (samples stored in BSS — stack is only 32KB)
// ---------------------------------------------------------------------------

/// Static sample buffer in BSS (80 KiB). Benchmarks run sequentially,
/// so a single shared buffer is safe.
static SAMPLE_BUF: Mutex<[u64; MAX_SAMPLES]> = Mutex::new([0u64; MAX_SAMPLES]);

struct BenchResult {
    name: &'static str,
    iterations: usize,
    min_ns: u64,
    max_ns: u64,
    total_ns: u64,
    sample_count: usize,
}

impl BenchResult {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            iterations: 0,
            min_ns: u64::MAX,
            max_ns: 0,
            total_ns: 0,
            sample_count: 0,
        }
    }

    fn record(&mut self, ns: u64) {
        self.iterations += 1;
        self.total_ns += ns;
        if ns < self.min_ns {
            self.min_ns = ns;
        }
        if ns > self.max_ns {
            self.max_ns = ns;
        }
        if self.sample_count < MAX_SAMPLES {
            let mut buf = SAMPLE_BUF.lock();
            buf[self.sample_count] = ns;
            self.sample_count += 1;
        }
    }

    fn avg_ns(&self) -> u64 {
        if self.iterations == 0 {
            0
        } else {
            self.total_ns / self.iterations as u64
        }
    }

    fn avg_us(&self) -> u64 {
        self.avg_ns() / 1000
    }

    /// Compute p99 latency using in-place insertion sort on the static buffer.
    fn p99_ns(&self) -> u64 {
        if self.sample_count == 0 {
            return 0;
        }
        let mut buf = SAMPLE_BUF.lock();
        // Insertion sort in place (only the valid range).
        for i in 1..self.sample_count {
            let key = buf[i];
            let mut j = i;
            while j > 0 && buf[j - 1] > key {
                buf[j] = buf[j - 1];
                j -= 1;
            }
            buf[j] = key;
        }
        let idx = self.sample_count * 99 / 100;
        buf[idx]
    }

    fn p99_us(&self) -> u64 {
        self.p99_ns() / 1000
    }

    /// Reset the static sample buffer for the next benchmark.
    fn reset_samples(&self) {
        let mut buf = SAMPLE_BUF.lock();
        for v in buf[..self.sample_count].iter_mut() {
            *v = 0;
        }
    }
}

// ---------------------------------------------------------------------------
// Time conversion
// ---------------------------------------------------------------------------

/// Convert counter ticks to nanoseconds.
/// Delegates to the shared crate's portable implementation.
#[inline(always)]
fn ticks_to_ns(ticks: u64) -> u64 {
    shared::ticks_to_ns(ticks, timer::read_cntfrq())
}

// ---------------------------------------------------------------------------
// Synchronization
// ---------------------------------------------------------------------------

/// Signal that the bench server is ready.
static BENCH_SERVER_READY: AtomicBool = AtomicBool::new(false);

/// Channel for bench IPC tests.
static BENCH_CHANNEL: AtomicU32 = AtomicU32::new(u32::MAX);

/// Signal to bench server to exit.
static BENCH_SERVER_EXIT: AtomicBool = AtomicBool::new(false);

/// Yield-partner ready for context switch benchmark.
static BENCH_YIELD_PARTNER_READY: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Bench server thread
// ---------------------------------------------------------------------------

/// Bench IPC server: sits in ipc_recv loop, replies immediately.
/// IRQs masked during benchmark to avoid timer preemption skewing results.
fn bench_server_entry() -> ! {
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    // Wait for channel to be set (yield to let main thread run).
    let ch = loop {
        let val = BENCH_CHANNEL.load(Ordering::Acquire);
        if val != u32::MAX {
            break ChannelId(val);
        }
        sched::thread_yield();
    };

    // Signal ready.
    BENCH_SERVER_READY.store(true, Ordering::Release);

    // Mask IRQs — the bench main thread uses direct-switch IPC which
    // doesn't need timer interrupts. This prevents preemption during
    // measurement from skewing results.
    // SAFETY: DAIFSet #0x2 sets the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFSet, #0x2") };

    let mut recv_buf = [0u8; ipc::MAX_MESSAGE_SIZE];

    loop {
        if BENCH_SERVER_EXIT.load(Ordering::Acquire) {
            break;
        }

        match ipc::ipc_recv(ch, &mut recv_buf, 100) {
            Ok((len, _sender)) => {
                // Echo back the same data.
                let _ = ipc::ipc_reply(ch, &recv_buf[..len]);
            }
            Err(_) => {
                // Timeout or error — continue.
            }
        }
    }

    loop {
        // SAFETY: wfe is a hint instruction, safe at EL1.
        unsafe { core::arch::asm!("wfe") };
    }
}

/// Yield partner for context switch benchmark.
fn bench_yield_entry() -> ! {
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    BENCH_YIELD_PARTNER_READY.store(true, Ordering::Release);

    // Just keep yielding — the bench main thread measures the switch time.
    loop {
        sched::thread_yield();
    }
}

// ---------------------------------------------------------------------------
// Benchmark implementations
// ---------------------------------------------------------------------------

/// Benchmark IPC round-trip (same core).
fn bench_ipc_same_core(ch: ChannelId) -> BenchResult {
    let mut result = BenchResult::new("IPC round-trip (same core)");
    let send_buf = [0xABu8; 8];
    let mut recv_buf = [0u8; ipc::MAX_MESSAGE_SIZE];

    // Warm up (IRQs still enabled for scheduler).
    for _ in 0..100 {
        let _ = ipc::ipc_call(ch, &send_buf, &mut recv_buf, 1000);
    }

    // Mask IRQs during measurement to prevent timer preemption from
    // skewing results. IPC direct-switch path is synchronous and doesn't
    // need timer interrupts.
    // SAFETY: DAIFSet/DAIFClr #0x2 mask/unmask IRQs. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFSet, #0x2") };

    for _i in 0..IPC_ITERATIONS {
        let start = timer::read_counter();
        let r = ipc::ipc_call(ch, &send_buf, &mut recv_buf, 1000);
        let end = timer::read_counter();
        if r >= 0 {
            let ns = ticks_to_ns(end.wrapping_sub(start));
            result.record(ns);
        }
    }

    // SAFETY: Restore IRQs after measurement.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    result
}

/// Benchmark context switch via yield.
fn bench_context_switch() -> BenchResult {
    let mut result = BenchResult::new("Context switch");

    // Wait for yield partner to be ready.
    while !BENCH_YIELD_PARTNER_READY.load(Ordering::Acquire) {
        sched::thread_yield();
    }

    // Warm up.
    for _ in 0..50 {
        sched::thread_yield();
    }

    for _ in 0..CTX_SWITCH_ITERATIONS {
        let start = timer::read_counter();
        sched::thread_yield();
        let end = timer::read_counter();
        let ns = ticks_to_ns(end.wrapping_sub(start));
        result.record(ns);
    }
    result
}

/// Benchmark shared memory throughput.
fn bench_shmem_throughput() -> (u64, u64) {
    // Allocate a shared memory region.
    let pid = ProcessId(8);
    let flags = crate::mm::pgtable::VmFlags::READ | crate::mm::pgtable::VmFlags::WRITE;

    let region = match crate::ipc::shmem::shared_memory_create(pid, SHM_SIZE, flags) {
        Ok(id) => id,
        Err(_) => return (0, 0),
    };

    // Map the region (registers the mapping in the table).
    if crate::ipc::shmem::shared_memory_map(pid, region, flags).is_err() {
        let _ = crate::ipc::shmem::shared_memory_unmap(pid, region);
        return (0, 0);
    }

    // Phase 3: kernel threads access shared memory via the direct map.
    let dmap_va = match crate::ipc::shmem::region_dmap_addr(region) {
        Some(va) => va,
        None => {
            let _ = crate::ipc::shmem::shared_memory_unmap(pid, region);
            return (0, 0);
        }
    };

    // Write benchmark.
    let start = timer::read_counter();
    // SAFETY: dmap_va points to the shared region's backing pages via the
    // kernel direct map. The region is at least SHM_SIZE bytes.
    unsafe {
        let ptr = dmap_va as *mut u8;
        for i in 0..SHM_SIZE {
            core::ptr::write_volatile(ptr.add(i), 0xAA);
        }
    }
    let end = timer::read_counter();
    let write_ns = ticks_to_ns(end.wrapping_sub(start));

    // Read benchmark.
    let start = timer::read_counter();
    // SAFETY: Same direct-map VA, just reading.
    unsafe {
        let ptr = dmap_va as *const u8;
        let mut sum: u64 = 0;
        for i in 0..SHM_SIZE {
            sum += core::ptr::read_volatile(ptr.add(i)) as u64;
        }
        // Prevent optimization.
        core::hint::black_box(sum);
    }
    let end = timer::read_counter();
    let read_ns = ticks_to_ns(end.wrapping_sub(start));

    // Clean up.
    let _ = crate::ipc::shmem::shared_memory_unmap(pid, region);

    // Calculate MB/s: size_bytes / time_ns * 1000 = MB/s
    let write_mbs = (SHM_SIZE as u64 * 1000).checked_div(write_ns).unwrap_or(0);
    let read_mbs = (SHM_SIZE as u64 * 1000).checked_div(read_ns).unwrap_or(0);

    (write_mbs, read_mbs)
}

// ---------------------------------------------------------------------------
// Main entry
// ---------------------------------------------------------------------------

/// Main benchmark entry point. Creates server threads, runs benchmarks,
/// prints Gate 1 results.
pub fn bench_main_entry() -> ! {
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    // Wait for system to settle (yield to let other threads run).
    {
        let start = timer::TICK_COUNT.load(Ordering::Relaxed);
        while timer::TICK_COUNT.load(Ordering::Relaxed) < start + 500 {
            sched::thread_yield();
        }
    }

    let mut w = UartWriter;
    let _ = writeln!(w, "\n[bench] === Gate 1 Benchmark ===");

    // --- Setup: Create bench channel and server ---
    let bench_ch = {
        let my_tid = crate::ipc::current_thread_id().unwrap_or(ThreadId(0));
        ipc::channel_create_unchecked(my_tid)
    };

    // Grant ChannelAccess to process 8 (bench process).
    let _ = cap::grant_to_process(
        ProcessId(8),
        shared::Capability::ChannelAccess(bench_ch),
        false,
    );

    // Set peer for the channel.
    let bench_server_tid_placeholder = ThreadId(0xB00);
    let _ = ipc::channel_set_peer(bench_ch, bench_server_tid_placeholder);

    BENCH_CHANNEL.store(bench_ch.0, Ordering::Release);

    crate::kinfo!(
        Ipc,
        "Bench main: channel {} created, waiting for server",
        bench_ch.0
    );

    // Wait for server ready (yield to let server thread run).
    while !BENCH_SERVER_READY.load(Ordering::Acquire) {
        sched::thread_yield();
    }

    crate::kinfo!(Ipc, "Bench main: server ready, starting IPC benchmark");

    // --- Benchmark 1: IPC round-trip (same core) ---
    let ipc_result = bench_ipc_same_core(bench_ch);
    let ipc_avg_us = ipc_result.avg_us();
    let ipc_p99_us = ipc_result.p99_us();
    let _ = writeln!(
        w,
        "[bench] {}: avg={} us, p99={} us, min={} ns, max={} ns ({} iters)",
        ipc_result.name,
        ipc_avg_us,
        ipc_p99_us,
        ipc_result.min_ns,
        ipc_result.max_ns,
        ipc_result.iterations
    );
    ipc_result.reset_samples();

    // --- Benchmark 2: Context switch ---
    let ctx_result = bench_context_switch();
    let ctx_avg_us = ctx_result.avg_us();
    let ctx_p99_us = ctx_result.p99_us();
    let _ = writeln!(
        w,
        "[bench] {}: avg={} us, p99={} us, min={} ns, max={} ns ({} iters)",
        ctx_result.name,
        ctx_avg_us,
        ctx_p99_us,
        ctx_result.min_ns,
        ctx_result.max_ns,
        ctx_result.iterations
    );
    ctx_result.reset_samples();

    // --- Benchmark 3: Shared memory throughput ---
    let (write_mbs, read_mbs) = bench_shmem_throughput();
    let _ = writeln!(
        w,
        "[bench] Shared memory throughput: write={} MB/s, read={} MB/s",
        write_mbs, read_mbs
    );

    // --- Gate 1 verdict ---
    let ipc_pass = ipc_avg_us < 10;
    let ctx_pass = ctx_avg_us < 20;

    let _ = writeln!(
        w,
        "[bench] Gate 1: IPC < 10 us:           {}",
        if ipc_pass { "PASS" } else { "FAIL" }
    );
    let _ = writeln!(
        w,
        "[bench] Gate 1: Context switch < 20 us: {}",
        if ctx_pass { "PASS" } else { "FAIL" }
    );
    let _ = writeln!(w, "[bench] === Gate 1 Complete ===");

    // Signal server to exit.
    BENCH_SERVER_EXIT.store(true, Ordering::Release);

    loop {
        // SAFETY: wfe is a hint instruction, safe at EL1.
        unsafe { core::arch::asm!("wfe") };
    }
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Create bench process, threads, and enqueue them.
pub fn init() {
    // Create Process 8: bench.
    {
        let mut procs = PROCESS_TABLE.lock();
        let mut name = [0u8; 32];
        name[..5].copy_from_slice(b"bench");
        procs[8] = Some(ProcessControl {
            pid: ProcessId(8),
            address_space: None,
            resource_limits: KernelResourceLimits::native(),
            cap_table: cap::CapabilityTable::new(),
            thread_ids: [None; 16],
            name,
        });
    }
    let _ = cap::grant_to_process(ProcessId(8), shared::Capability::ChannelCreate, true);
    let _ = cap::grant_to_process(ProcessId(8), shared::Capability::SharedMemoryCreate, true);
    let _ = cap::grant_to_process(ProcessId(8), shared::Capability::DebugPrint, false);

    // --- Bench server thread (same core as main for IPC direct switch) ---
    {
        let stack_phys = sched::alloc_kernel_stack();
        let stack_virt_top = sched::phys_to_virt(stack_phys) + sched::STACK_SIZE;

        let mut thread = Thread::new_kernel(
            ThreadId(0xB00),
            b"bench-server\0\0\0\0",
            bench_server_entry as *const () as usize,
            stack_phys,
        );
        thread.sched.class = SchedulerClass::Interactive;
        thread.sched.effective_class = SchedulerClass::Interactive;
        thread.sched.affinity = CpuSet::all();
        thread.context.sp = stack_virt_top as u64;
        thread.owner_pid = Some(ProcessId(8));

        let idx = sched::allocate_thread(thread).expect("thread table full for bench server");
        sched::enqueue_on_cpu(0, ThreadId(idx as u32), SchedulerClass::Interactive);
    }

    // --- Bench yield partner thread (for context switch benchmark) ---
    {
        let stack_phys = sched::alloc_kernel_stack();
        let stack_virt_top = sched::phys_to_virt(stack_phys) + sched::STACK_SIZE;

        let mut thread = Thread::new_kernel(
            ThreadId(0xB01),
            b"bench-yield\0\0\0\0\0",
            bench_yield_entry as *const () as usize,
            stack_phys,
        );
        thread.sched.class = SchedulerClass::Interactive;
        thread.sched.effective_class = SchedulerClass::Interactive;
        thread.sched.affinity = CpuSet::all();
        thread.context.sp = stack_virt_top as u64;
        thread.owner_pid = Some(ProcessId(8));

        let idx = sched::allocate_thread(thread).expect("thread table full for bench yield");
        sched::enqueue_on_cpu(0, ThreadId(idx as u32), SchedulerClass::Interactive);
    }

    // --- Bench main thread ---
    {
        let stack_phys = sched::alloc_kernel_stack();
        let stack_virt_top = sched::phys_to_virt(stack_phys) + sched::STACK_SIZE;

        let mut thread = Thread::new_kernel(
            ThreadId(0xB02),
            b"bench-main\0\0\0\0\0\0",
            bench_main_entry as *const () as usize,
            stack_phys,
        );
        thread.sched.class = SchedulerClass::Interactive;
        thread.sched.effective_class = SchedulerClass::Interactive;
        thread.sched.affinity = CpuSet::all();
        thread.context.sp = stack_virt_top as u64;
        thread.owner_pid = Some(ProcessId(8));

        let idx = sched::allocate_thread(thread).expect("thread table full for bench main");
        sched::enqueue_on_cpu(0, ThreadId(idx as u32), SchedulerClass::Interactive);
    }

    crate::kinfo!(Ipc, "Bench threads created");
}
