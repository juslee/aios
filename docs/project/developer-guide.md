# AIOS Kernel Developer Guide

**Audience**: Experienced Rust developer new to OS kernel development, OR experienced C kernel developer new to Rust.

**Scope**: Kernel-side development only. For application/agent development, see [architecture.md SS4](./architecture.md#4-developer-experience).

**Related documents**:
- [CONTRIBUTING.md](../../CONTRIBUTING.md) -- PR process, commit style, branching
- [CLAUDE.md](../../CLAUDE.md) -- Code conventions, quality gates, technical facts
- [hal.md](../kernel/hal.md) -- Hardware Abstraction Layer and platform porting (SS7)
- [deadlock-prevention.md](../kernel/deadlock-prevention.md) -- Lock ordering rules (SS12)
- [memory.md](../kernel/memory.md) -- Memory management architecture and APIs (SS4)

> **How to read this guide:** Sections 1--2 cover the Rust patterns and kernel idioms
> you need before touching the codebase. Sections 3--4 cover style rules and known
> pitfalls (with exact do/don't code). Sections 5--6 cover the build and debug
> workflow. Section 8 provides cross-references to architecture documents for
> deeper topics.

---

## Table of Contents

1. [Rust Competency Model](#1-rust-competency-model)
2. [AIOS Kernel Patterns](#2-aios-kernel-patterns)
3. [Code Style and Organization](#3-code-style-and-organization)
4. [Common Pitfalls](#4-common-pitfalls)
5. [Build, Test, and Verification Workflow](#5-build-test-and-verification-workflow)
6. [Debugging Techniques](#6-debugging-techniques)
7. [Contributing a New Subsystem](#7-contributing-a-new-subsystem)
8. [Cross-Reference Index](#8-cross-reference-index)
9. [Planned Expansions](#9-planned-expansions)
10. [Appendix: AIOS Glossary](#10-appendix-aios-glossary)

---

## 1. Rust Competency Model

This section is organized into three tiers. Tier 1 is prerequisite knowledge for any contribution. Tier 2 enables effective work across the kernel. Tier 3 covers deep subsystem internals that you can learn as needed.

### Tier 1 -- Must Know (prerequisites for any contribution)

**`#![no_std]` / `#![no_main]`**

The kernel (`kernel/`) uses both `#![no_std]` and `#![no_main]` because it provides its own entry point (`_start` in `boot.S`). The shared crate (`shared/`) uses `#![no_std]` but *not* `#![no_main]` -- it is a library, not an entry point. There is no standard library and no runtime. You work with `core::` types only -- `core::fmt`, `core::sync::atomic`, `core::ptr`, `core::mem`, `core::arch::asm!`. The `alloc` crate (for `Vec`, `Box`, `String`) becomes available after heap bootstrap: initially via a 128 KiB static bump allocator (`mm/bump.rs`), then via the full slab allocator once `mm::enable_slab_allocator()` runs. Most kernel code avoids heap allocation on hot paths.

**`unsafe` blocks and the SAFETY contract**

AIOS requires a three-line SAFETY comment before every `unsafe` block. This is enforced during code review and audits. The format:

```rust
// SAFETY: <what invariant makes this safe>
// <who maintains the invariant>
// <what happens if violated>
unsafe { ... }
```

Real example from `kernel/src/arch/aarch64/uart.rs`:

```rust
// SAFETY: UART base is a valid PL011 MMIO address (set by init or default).
// init_pl011() sets the base from DTB; QEMU maps this unconditionally.
// Writing to an invalid address would cause a synchronous data abort.
unsafe {
    while mmio_read32(base + UARTFR) & FR_TXFF != 0 {}
    mmio_write32(base + UARTDR, byte as u32);
}
```

**`extern crate alloc`**

The heap becomes available after `mm::enable_slab_allocator()` is called in `kernel_main`. Before that point, only the 128 KiB static bump allocator (`mm/bump.rs`) backs `alloc::`. After the switch, the full slab allocator handles all allocations. The bump allocator never frees -- early boot allocations are intentionally leaked.

**`core::ptr::read_volatile` / `write_volatile`**

ALL hardware register access uses volatile operations. Regular pointer dereferences are optimized by LLVM and will miss register side effects. A read from a UART status register, for instance, must happen every iteration of a polling loop -- the optimizer would otherwise hoist it out. AIOS wraps these in `mmio_read32` / `mmio_write32` inline helpers (see SS2.1).

**`core::sync::atomic` ordering semantics**

The kernel uses atomics extensively. The orderings you will encounter:

| Ordering | Guarantee | AIOS usage |
|---|---|---|
| `Relaxed` | No ordering. Compiler may reorder freely. | Statistics counters (`TICK_COUNT`), single-writer statics (`BOOT_MAIR`) |
| `Acquire` | Subsequent reads/writes cannot be reordered before this load. | Consumer side of SPSC rings, gate checks (`SLAB_READY.load`) |
| `Release` | Prior reads/writes cannot be reordered after this store. | Producer side of SPSC rings, gate transitions (`SLAB_READY.store`) |
| `SeqCst` | Full sequential consistency across all threads. | Rarely needed -- prefer Acquire/Release |

**`core::arch::asm!()`**

Inline assembly for system register access, barriers, and instructions not expressible in Rust. Key operand syntax:

```rust
// Read a system register into a Rust variable
let val: u64;
unsafe { core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) val) };

// Write a Rust variable to a system register
unsafe { core::arch::asm!("msr CNTP_TVAL_EL0, {}", in(reg) tick_interval) };

// Pure register read -- optimizer-friendly hints
unsafe {
    core::arch::asm!("mrs {}, MPIDR_EL1", out(reg) mpidr,
                      options(nomem, nostack, preserves_flags))
};
```

Key operand types:
- `in(reg) val` -- pass a Rust value into a general-purpose register
- `out(reg) val` -- capture a register value into a Rust variable
- `inout(reg) val` -- both input and output (same register)
- `options(nomem, nostack, preserves_flags)` -- for pure register reads with no side effects

**`#[repr(C)]` for FFI structs**

Any struct shared with assembly code must use `#[repr(C)]` to guarantee field layout matches assembly offsets. The Rust compiler is free to reorder fields in the default representation. Critical structs:

| Struct | Size | Where | Purpose |
|---|---|---|---|
| `TrapFrame` | 272 bytes | `trap.rs` | Saved on exception from EL0 |
| `ThreadContext` | 296 bytes | `task/mod.rs` | Saved during voluntary context switch |
| `FpContext` | 528 bytes | `task/mod.rs` | Floating-point register state |
| `RawPageTable` | 4096 bytes | `mmu.rs` | ARM page table (512 x 8-byte entries) |

Compile-time size assertions verify correctness:

```rust
const _: () = assert!(core::mem::size_of::<TrapFrame>() == 272);
```

### Tier 2 -- Should Know (effective contribution)

**Trait objects for hardware abstraction**

The `Platform` trait (`kernel/src/platform/mod.rs`) uses `&'static dyn Platform` -- a static lifetime because platform detection happens before the heap exists and the platform reference must live for the entire kernel lifetime.

```rust
pub trait Platform: Send + Sync {
    fn name(&self) -> &'static str;
    fn init_uart(&self, dt: &DeviceTree) -> Uart;
    fn init_interrupts(&self, dt: &DeviceTree) -> InterruptController;
    fn init_timer(&self, dt: &DeviceTree, ic: &InterruptController) -> Timer;
}

pub fn detect_platform(dt: &DeviceTree) -> &'static dyn Platform {
    let compat = dt.root_compatible_str();
    if compat.contains("virt") || compat.contains("qemu") {
        static QEMU: qemu::QemuPlatform = qemu::QemuPlatform;
        return &QEMU;
    }
    panic!("Unknown platform: {}", compat);
}
```

The `static QEMU` inside the function body is a function-scoped static -- it has `'static` lifetime but is only accessible through the returned reference. This pattern avoids global state while satisfying the lifetime requirement.

**`GlobalAlloc` switchable implementation**

The kernel allocator (`mm/mod.rs`) switches from bump to slab at runtime via an `AtomicBool` gate:

```rust
static SLAB_READY: AtomicBool = AtomicBool::new(false);

unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if SLAB_READY.load(Ordering::Acquire) {
            slab::alloc(layout)
        } else {
            bump::alloc(layout)
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if SLAB_READY.load(Ordering::Acquire) {
            slab::dealloc(ptr, layout);
        }
        // Bump allocator never frees -- early boot allocations are leaked.
    }
}

pub fn enable_slab_allocator() {
    SLAB_READY.store(true, Ordering::Release);
}
```

The `Release`/`Acquire` pairing guarantees that all slab initialization writes are visible to any thread that sees `SLAB_READY == true`. This is a one-way gate -- once set, it never reverts.

**`UnsafeCell` + manual `Sync` impl for write-once statics**

Used when a static is written once during single-core boot, then becomes read-only:

```rust
#[repr(C, align(4096))]
struct RawPageTable {
    entries: UnsafeCell<[u64; 512]>,
}

// SAFETY: Written once during single-core boot, then read-only by MMU hardware.
// No concurrent access during init (only boot CPU runs).
unsafe impl Sync for RawPageTable {}
```

Why not `Mutex`? Two reasons: (1) during early boot, spinlocks may not work (NC memory issue -- see SS4.1), and (2) the MMU hardware reads these tables directly -- it does not acquire locks.

**`spin::Mutex`**

Uses exclusive load/store pairs (`ldaxr`/`stlxr`) internally, which require the global exclusive monitor. The global exclusive monitor only functions with Inner Shareable + Cacheable memory attributes. **Current status (post-Phase 2 M8):** the TTBR0 identity map now uses WB cacheable (MAIR Attr3), so `spin::Mutex` works correctly on identity-mapped RAM. The NC constraint only applies if you explicitly map memory as Non-Cacheable (e.g., device MMIO regions, or the framebuffer). See SS4.1 for the full history.

**Linker script symbols as `extern "C"` statics**

Symbols defined in `linker.ld` (such as `__kernel_virt_base`, `__bss_start`, `__bss_end`) are accessed in Rust via:

```rust
extern "C" {
    static __bss_start: u8;
    static __bss_end: u8;
}

// To get the address (not the value):
let bss_start = unsafe { &__bss_start as *const u8 as usize };
```

These symbols have no storage -- the linker assigns them addresses. You take the address of the symbol, never read its value.

**`const fn` constructors**

Static initialization in Rust requires `const fn`. Any global `static` that is not zero-initialized needs a `const` constructor. Examples from the codebase:

```rust
// Array of const-initialized items (observability/mod.rs)
static LOG_RINGS: [LogRing; MAX_CORES] = [const { LogRing::INIT }; MAX_CORES];

// Const constructor for LogRing
impl LogRing {
    const INIT: Self = Self {
        entries: UnsafeCell::new([LogEntry::ZERO; LOG_RING_SIZE]),
        head: AtomicU32::new(0),
        tail: AtomicU32::new(0),
    };
}

// Const constructor for MessageRing (ipc/mod.rs)
impl MessageRing {
    const fn new() -> Self {
        Self {
            entries: [const { RawMessage::EMPTY }; RING_CAPACITY],
            head: 0,
            tail: 0,
            len: 0,
        }
    }
}
```

The `[const { T::INIT }; N]` syntax creates an array where each element is independently const-evaluated. This works even when `T` does not implement `Copy`.

### Tier 3 -- Nice to Have (deep kernel work)

These topics are documented in the architecture docs. Study them when you work on the relevant subsystem.

- **4-level page table mechanics**: PGD, PUD, PMD, PTE levels; descriptor bit fields; table vs. block descriptors. See `mm/pgtable.rs` and [memory.md SS3](../kernel/memory.md).

- **ARM exception model**: Exception levels EL0--EL3; ESR_EL1 syndrome register and fault status codes; FAR_EL1 faulting address; vector table layout with 16 entries (4 exception types x 4 source ELs). See `arch/aarch64/exceptions.rs` and `trap.rs`.

- **GICv3 architecture**: Distributor (global SPI routing), Redistributor (per-core PPI/SGI enable), CPU interface (IAR/EOIR/PMR for acknowledge/complete/priority masking). See `arch/aarch64/gic.rs` and [hal.md SS4.1](../kernel/hal.md).

- **PSCI for SMP**: `CPU_ON` function ID `0xC400_0003`, HVC conduit on QEMU, SMC on real hardware, SMCCC calling convention. See `arch/aarch64/psci.rs` and `smp.rs`.

- **ASID management and TLB invalidation**: 16-bit ASIDs, generation tracking with full flush on wrap, per-page vs. per-ASID vs. global invalidation. See `mm/asid.rs` and `mm/tlb.rs`.

- **VirtIO MMIO transport**: Legacy (v1) register layout, virtqueue descriptor chains, DMA page allocation, polled completion. See `drivers/virtio_blk.rs` and [spaces-block-engine.md §4.1](../storage/spaces-block-engine.md).

- **Block Engine internals**: Content-addressed storage with CRC-32C integrity, SHA-256 hashing, write-ahead log, LSM-tree MemTable. See `storage/block_engine.rs`, `storage/wal.rs`, `storage/lsm.rs` and [spaces-block-engine.md](../storage/spaces-block-engine.md).

### Rust Concepts NOT Used in AIOS

Understanding what AIOS does *not* use helps set expectations:

- **`std::` anything** -- No filesystem, no networking, no threads library, no `println!`. The kernel provides these services; it cannot depend on them. (This restriction applies to *kernel code only*; application developers will have full `std` access via AIOS runtimes in later phases.)
- **`async`/`await`** -- The kernel scheduler is cooperative/preemptive at the thread level, not at the Rust async task level. There is no executor.
- **Dynamic dispatch (mostly)** -- Outside the `Platform` trait, AIOS uses monomorphization (generics) rather than trait objects. This avoids vtable indirection on hot paths.
- **`String` and `Vec` on hot paths** -- Heap allocation in interrupt handlers or the scheduler is forbidden. Fixed-size arrays and stack buffers are used instead (e.g., `FixedQueue<T, N>`, `MsgBuf` with a 48-byte stack buffer).
- **`#[derive(Debug)]` on kernel structs** -- Debug formatting pulls in formatting machinery that increases binary size. Kernel structs implement display manually where needed.

### Recommended Reading

| Book | What It Covers | When You Need It |
|---|---|---|
| *Rust for Rustaceans* (Jon Gjengset) | Advanced Rust patterns, unsafe, FFI | New to Rust, experienced programmer |
| *Rust Atomics and Locks* (Mara Bos) | Memory ordering, atomics, lock-free structures | Concurrency in kernel code |
| *The Rustonomicon* (online) | Unsafe Rust, raw pointers, variance | Writing `unsafe` kernel code |
| *Operating Systems: Three Easy Pieces* (online) | VM, scheduling, concurrency concepts | New to OS development |
| *ARM Architecture Reference Manual* (ARM DDI 0487) | AArch64 system registers, exceptions | Deep hardware interaction |

---

## 2. AIOS Kernel Patterns

This section documents the recurring patterns in AIOS kernel code with real examples. Each pattern includes the code, an explanation of why it is written that way, and do/don't guidance.

### 2.1 Unsafe Pattern: MMIO Access

All hardware register access in AIOS uses volatile read/write helpers. The pattern separates the volatile access into `#[inline(always)]` functions, then wraps usage sites in SAFETY-documented `unsafe` blocks.

**Helper functions** (from `kernel/src/arch/aarch64/uart.rs`):

```rust
#[inline(always)]
unsafe fn mmio_read32(addr: usize) -> u32 {
    core::ptr::read_volatile(addr as *const u32)
}

#[inline(always)]
unsafe fn mmio_write32(addr: usize, val: u32) {
    core::ptr::write_volatile(addr as *mut u32, val);
}
```

**Usage with SAFETY comment** (from `uart.rs`):

```rust
pub fn putc(byte: u8) {
    let base = UART_BASE_ADDR.load(Ordering::Relaxed);
    // SAFETY: UART base is a valid PL011 MMIO address (set by init or default).
    unsafe {
        while mmio_read32(base + UARTFR) & FR_TXFF != 0 {}
        mmio_write32(base + UARTDR, byte as u32);
    }
}
```

**Register offsets** are defined as module-level constants with architecture doc references:

```rust
// PL011 register offsets (hal.md SS4.3)
const UARTDR: usize = 0x000;
const UARTFR: usize = 0x018;
const UARTIBRD: usize = 0x024;
const UARTFBRD: usize = 0x028;
const UARTLCR_H: usize = 0x02C;
const UARTCR: usize = 0x030;
```

**Do / Don't:**

```rust
// DO: Wrap volatile access in inline helpers, document the device
// SAFETY: GIC distributor at 0x0800_0000, valid on QEMU virt.
unsafe { mmio_write32(gicd_base + GICD_CTLR, 0x3); }

// DON'T: Scatter raw volatile calls without safety comments
unsafe { core::ptr::write_volatile(0x0800_0000 as *mut u32, 0x3); }

// DON'T: Use non-volatile writes to MMIO (optimizer will remove them)
*(0x0900_0000 as *mut u32) = byte as u32;  // WRONG: may be optimized away
```

### 2.2 Unsafe Pattern: Page Table Manipulation

Page tables use `UnsafeCell` for interior mutability of write-once statics:

```rust
#[repr(C, align(4096))]
struct RawPageTable {
    entries: UnsafeCell<[u64; 512]>,
}

// SAFETY: Page tables are written once during single-core boot, then read-only
// by the MMU hardware. No concurrent access during init.
unsafe impl Sync for RawPageTable {}

#[no_mangle]
static TTBR0_L0: RawPageTable = RawPageTable {
    entries: UnsafeCell::new([0; 512]),
};
```

**Why `UnsafeCell`?** Rust's `static` items are immutable by default. Page tables must be written during boot, then become read-only. `UnsafeCell` provides interior mutability; the `unsafe impl Sync` asserts thread safety (guaranteed by single-core boot -- secondaries are parked in `wfe` at this point).

**Why `#[repr(C, align(4096))]`?** ARM MMU requires page tables to be page-aligned (4 KiB). `repr(C)` ensures predictable layout for hardware traversal -- the MMU table walker reads entries at fixed 8-byte offsets.

**Why `#[no_mangle]`?** Assembly code in `boot.S` references these tables by symbol name. Without `#[no_mangle]`, the Rust compiler mangles the symbol and the linker cannot resolve it.

**Descriptor bit fields** are defined as constants (from `mmu.rs`):

```rust
const PTE_VALID: u64 = 1 << 0;
const PTE_TABLE: u64 = 1 << 1;       // Table descriptor (not block)
const PTE_AF: u64 = 1 << 10;         // Access flag
const PTE_SH_INNER: u64 = 0b11 << 8; // Inner shareable
const PTE_PXN: u64 = 1 << 53;        // Privileged execute-never
const PTE_UXN: u64 = 1 << 54;        // Unprivileged execute-never
```

**Cross-references:** Full 4-level table implementation in `mm/pgtable.rs`; kernel address space construction in `mm/kmap.rs`; W^X enforcement details in [memory.md SS3](../kernel/memory.md).

### 2.3 Unsafe Pattern: Lock-free SPSC Rings

Per-core logging uses a Single-Producer Single-Consumer (SPSC) ring buffer with no locking:

```rust
pub struct LogRing {
    entries: UnsafeCell<[LogEntry; LOG_RING_SIZE]>,
    head: AtomicU32,
    tail: AtomicU32,
}

impl LogRing {
    fn push(&self, entry: LogEntry) {
        let head = self.head.load(Ordering::Relaxed);
        let next_head = head.wrapping_add(1);

        // If the ring is full, advance tail to discard the oldest entry.
        let tail = self.tail.load(Ordering::Relaxed);
        if next_head.wrapping_sub(tail) > LOG_RING_SIZE as u32 {
            self.tail.store(tail.wrapping_add(1), Ordering::Relaxed);
        }

        let idx = (head & LOG_RING_MASK) as usize;

        // SAFETY: Single producer (owning core). UnsafeCell provides interior
        // mutability. No concurrent writes to this index because head is only
        // advanced by the owning core.
        unsafe {
            let slot = (*self.entries.get()).as_mut_ptr().add(idx);
            core::ptr::write(slot, entry);
        }

        self.head.store(next_head, Ordering::Release);
    }
}

// SAFETY: LogRing is accessed per-core (producer) and by drain (consumer).
// The SPSC protocol ensures no data races.
unsafe impl Sync for LogRing {}
```

**Key design decisions:**

- **Per-core ownership**: Each CPU has its own `LogRing` in the `LOG_RINGS` array. The producer (logging code on the owning core) never races with other producers -- there is exactly one writer per ring.

- **Overwrite-on-full**: When the ring fills, the oldest entry is discarded (tail advanced). This prevents logging from blocking kernel execution. Losing old log entries is acceptable; blocking the scheduler is not.

- **Release/Acquire pairing**: `head.store(Release)` in `push` pairs with `head.load(Acquire)` in `pop`. This guarantees the entry data written before the head advance is visible to the consumer when it reads the new head value.

- **No lock needed**: The SPSC invariant (one producer, one consumer) eliminates the need for a mutex. Contrast this with `MessageRing` in `ipc/mod.rs`, which uses `spin::Mutex` because multiple threads may send to the same channel.

### 2.4 Unsafe Pattern: System Register Access

ARM system registers are accessed via `mrs` (read) and `msr` (write) instructions wrapped in inline assembly.

**Reading a system register** (from `timer.rs`):

```rust
#[inline(always)]
fn read_cntfrq() -> u64 {
    let val: u64;
    // SAFETY: CNTFRQ_EL0 is always readable at EL1.
    unsafe { core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) val) };
    val
}
```

**Reading with optimizer hints** (from `observability/mod.rs`):

```rust
pub fn current_core_id() -> usize {
    let mpidr: u64;
    // SAFETY: MPIDR_EL1 is always readable at EL1.
    unsafe {
        core::arch::asm!("mrs {}, MPIDR_EL1", out(reg) mpidr,
                          options(nomem, nostack, preserves_flags))
    };
    (mpidr & 0xFF) as usize
}
```

**Writing a system register with barrier** (from `timer.rs`):

```rust
// SAFETY: System register writes for timer configuration at EL1.
unsafe {
    core::arch::asm!("msr CNTP_TVAL_EL0, {}", in(reg) tick_interval);
    core::arch::asm!("msr CNTP_CTL_EL0, {}", in(reg) 1u64);
    core::arch::asm!("isb");
}
```

**When to use each `options()` flag:**

| Flag | Meaning | Use when |
|---|---|---|
| `nomem` | Instruction does not access memory | Pure register reads/writes (`mrs`, `msr`) |
| `nostack` | Instruction does not use the stack | Most single instructions |
| `preserves_flags` | Instruction does not change condition flags (NZCV) | `mrs`, `msr`, `nop` |
| (none -- no options) | Instruction may have arbitrary side effects | Memory barriers (`dsb`, `tlbi`), `eret`, `wfe` |

**Critical rule:** Always add `isb` (Instruction Synchronization Barrier) after `msr` writes to registers that affect instruction fetch or translation:

- `VBAR_EL1` -- exception vector base address
- `SCTLR_EL1` -- MMU enable/disable
- `TTBR0_EL1`, `TTBR1_EL1` -- translation table base
- `TCR_EL1` -- translation control
- `CPACR_EL1` -- FPU/NEON enable

Without `isb`, the processor pipeline may execute subsequent instructions using stale register values.

### 2.5 Boot Sequence Pattern

The kernel boot sequence (`kernel_main` in `main.rs`) follows a strict phase progression. Each phase is tracked by the `EarlyBootPhase` enum (18 variants, from `EntryPoint` to `Complete`):

```rust
// From kernel/src/boot_phase.rs
pub fn advance_boot_phase(phase: EarlyBootPhase) {
    CURRENT_PHASE.store(phase as u64, Ordering::Relaxed);
    if phase >= EarlyBootPhase::UartReady {
        crate::kinfo!(Boot, "{:?} -- {}ms", phase, boot_elapsed_ms());
    }
}
```

The boot sequence in `kernel_main` is a linear chain of initialization steps. Each step advances the boot phase and may depend on earlier phases being complete:

```rust
// Simplified kernel_main structure (from main.rs)
pub extern "C" fn kernel_main(boot_info_ptr: u64) -> ! {
    // 1. Validate BootInfo
    let boot_info = validate_boot_info(boot_info_ptr);

    // 2. Install exception vectors
    exceptions::install_vector_table();
    advance_boot_phase(EarlyBootPhase::ExceptionVectors);

    // 3. Parse DTB, detect platform
    let dt = dtb::DeviceTree::parse(boot_info.device_tree);
    let platform = platform::detect_platform(&dt);
    advance_boot_phase(EarlyBootPhase::DeviceTreeParsed);

    // 4. Initialize UART (full PL011 init with baud rate)
    platform.init_uart(&dt);
    advance_boot_phase(EarlyBootPhase::UartReady);

    // 5. GIC + Timer
    let ic = platform.init_interrupts(&dt);
    let timer = platform.init_timer(&dt, &ic);
    advance_boot_phase(EarlyBootPhase::TimerReady);

    // 6. MMU (swap TTBR0 identity map)
    unsafe { mmu::init_mmu(); }
    advance_boot_phase(EarlyBootPhase::MmuEnabled);

    // 7. Physical memory (buddy allocator + pools)
    mm::init::init_memory(boot_info);
    advance_boot_phase(EarlyBootPhase::PageAllocatorReady);

    // 8. Slab allocator + heap
    mm::enable_slab_allocator();
    advance_boot_phase(EarlyBootPhase::HeapReady);

    // 9. Full kernel address space (TTBR1)
    mm::kmap::init_kernel_address_space();
    advance_boot_phase(EarlyBootPhase::KmapReady);

    // 10. Log rings ready (switch from synchronous to ring-buffered logging)
    advance_boot_phase(EarlyBootPhase::LogRingsReady);

    // 11. SMP bringup (secondary cores)
    smp::bring_up_secondaries(&dt, &ic);
    advance_boot_phase(EarlyBootPhase::SmpReady);

    // ... scheduler init, IPC init, etc.
    advance_boot_phase(EarlyBootPhase::Complete);
    // Enter scheduler idle loop
}
```

**Key ordering constraints:**

1. FPU must be enabled before any Rust code (done in `boot.S`, before `kernel_main`).
2. Exception vectors must be installed before interrupts are unmasked.
3. UART must be initialized before meaningful logging.
4. MMU swap must happen before buddy allocator init (buddy needs the identity map).
5. Buddy allocator must be initialized before slab allocator (slab allocates pages from buddy).
6. Slab allocator must be ready before any `alloc::` usage (Vec, Box, String).
7. Full TTBR1 (kmap) must be built before MMIO virtual addresses are used.
8. Log rings must be marked ready before timer tick handler starts draining them.

**Adding a new boot phase:** If you need to add initialization between existing phases, add a new variant to `EarlyBootPhase` in `shared/src/boot.rs` (renumbering subsequent variants) and insert the corresponding `advance_boot_phase()` call at the correct point in `kernel_main`.

### 2.6 Error Handling Patterns

AIOS uses three distinct error handling patterns, each suited to a different situation.

**Pattern 1: Syscall results** -- `Result<T, i64>` with numeric error code

```rust
pub fn channel_create(creator: ThreadId) -> Result<ChannelId, i64> {
    let mut table = CHANNEL_TABLE.lock();
    // Find free slot...
    match free_slot {
        Some(id) => Ok(ChannelId(id as u32)),
        None => Err(IpcError::Enomem as i64),
    }
}
```

The error code is returned to userspace in register `x0` via the `TrapFrame`. Error values are defined in `shared/src/syscall.rs` as `IpcError` enum variants with numeric discriminants.

**Pattern 2: Unrecoverable panic** -- UART output then halt

```rust
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let mut w = crate::arch::aarch64::uart::UartWriter;
    let _ = writeln!(&mut w, "PANIC: {}", info);
    halt()
}

fn halt() -> ! {
    loop {
        // SAFETY: wfe is a hint instruction, safe at any EL.
        unsafe { core::arch::asm!("wfe") }
    }
}
```

Why `wfe` and not `loop {}`? `wfe` (Wait For Event) puts the core in a low-power state. A bare `loop {}` burns full CPU cycles doing nothing. On real hardware, this matters for power consumption and thermal management.

**Pattern 3: Boot validation** -- structured log then explicit halt

```rust
let boot_info = if boot_info_ptr != 0 {
    let bi = unsafe { &*(boot_info_ptr as *const BootInfo) };
    if bi.magic == BOOTINFO_MAGIC {
        kinfo!(Boot, "BootInfo at {:#x}, magic OK", boot_info_ptr);
    } else {
        kerror!(Boot, "BootInfo at {:#x}, BAD magic {:#x}", boot_info_ptr, bi.magic);
        halt();
    }
    bi
} else {
    kerror!(Boot, "No BootInfo (Phase 0 mode)");
    halt();
};
```

This pattern uses the structured logging system (`kinfo!`/`kerror!`) for diagnostic output before halting, rather than `panic!`. The advantage is control over the output format and the ability to log multiple diagnostic values before stopping.

**When to use which:**

| Situation | Pattern | Example |
|---|---|---|
| Syscall fails (recoverable by caller) | `Result<T, i64>` | Channel full, permission denied |
| Invariant violated (kernel bug) | `panic!()` | Null pointer, impossible enum match |
| Boot validation failure | `kerror!` + `halt()` | Bad magic, missing BootInfo |

### 2.7 Atomic Patterns

**Pattern 1: NC-memory-safe atomics** -- load/store only, no read-modify-write

```rust
/// Serializes secondary core printing. Core N waits for PRINT_TURN == N
/// before printing, then stores N+1. Uses only load(Acquire)/store(Release)
/// -- no exclusive load/store pairs -- which is safe on NC memory.
static PRINT_TURN: AtomicUsize = AtomicUsize::new(1);
```

Usage: Core N spin-waits on `PRINT_TURN.load(Acquire) == N`, prints its output, then executes `PRINT_TURN.store(N + 1, Release)`. This turn-based protocol compiles to `ldar`/`stlr` instructions (plain load-acquire and store-release), which work on Non-Cacheable memory because they do not use exclusive load/store pairs.

**Pattern 2: AtomicBool gate** -- one-time phase transition

```rust
static SLAB_READY: AtomicBool = AtomicBool::new(false);

// Writer (called exactly once during boot):
pub fn enable_slab_allocator() {
    SLAB_READY.store(true, Ordering::Release);
}

// Reader (called on every allocation):
if SLAB_READY.load(Ordering::Acquire) {
    slab::alloc(layout)
} else {
    bump::alloc(layout)
}
```

The `Release`/`Acquire` pair ensures all slab initialization writes are visible before any allocation goes through the slab path. This is a one-way gate -- it transitions from `false` to `true` exactly once and never reverts.

**Pattern 3: Write-once statics** -- boot CPU writes, secondaries read

```rust
#[no_mangle]
static BOOT_MAIR: AtomicU64 = AtomicU64::new(0);
#[no_mangle]
static BOOT_TCR: AtomicU64 = AtomicU64::new(0);
#[no_mangle]
static BOOT_SCTLR: AtomicU64 = AtomicU64::new(0);
```

The boot CPU writes these with `store(Relaxed)` after reading the system registers during `init_mmu()`. Secondary cores read them with `load(Relaxed)` during `_secondary_entry` in `boot.S` to configure their own MMU with identical settings. `Relaxed` is sufficient because a `dsb sy` barrier between the write and the `sev` that wakes secondary cores guarantees visibility.

**Pattern 4: Per-CPU guard** -- preventing re-entrancy

```rust
// From sched/mod.rs
static IN_SCHEDULER: [AtomicBool; MAX_CORES] = [const { AtomicBool::new(false) }; MAX_CORES];

pub fn schedule() {
    let core = current_core_id();
    if IN_SCHEDULER[core].swap(true, Ordering::Acquire) {
        return; // Already in schedule() on this core (re-entrant timer tick)
    }
    // ... do scheduling work ...
    IN_SCHEDULER[core].store(false, Ordering::Release);
}
```

This prevents the timer tick handler from calling `schedule()` while `schedule()` is already executing on the same core. The `swap` atomically sets the flag and returns the previous value.

### 2.8 Macro Patterns

**Structured logging macros** (from `observability/mod.rs`):

```rust
#[macro_export]
macro_rules! klog {
    ($level:ident, $subsys:ident, $($arg:tt)*) => {{
        const _LEVEL: $crate::observability::LogLevel = $crate::observability::LogLevel::$level;
        if _LEVEL >= $crate::observability::MIN_LOG_LEVEL {
            $crate::observability::log_impl(
                _LEVEL,
                $crate::observability::Subsystem::$subsys,
                format_args!($($arg)*),
            );
        }
    }};
}

// Convenience wrappers
#[macro_export]
macro_rules! kinfo {
    ($subsys:ident, $($arg:tt)*) => { $crate::klog!(Info, $subsys, $($arg)*) };
}
```

Key design decisions:

- The `const _LEVEL` binding makes the level comparison a compile-time constant. When `MIN_LOG_LEVEL` is `Info` and the call is `kdebug!(...)`, the entire macro expands to nothing -- zero runtime cost.
- `format_args!()` is used instead of `format!()` because it does not allocate. The formatting happens into a fixed 48-byte stack buffer inside `log_impl()`.
- `#[macro_export]` places the macro at the crate root, so it is invoked as `crate::kinfo!()` from within the kernel crate.

**Feature-gated trace macro** (from `observability/trace.rs`):

```rust
#[macro_export]
macro_rules! trace_point {
    ($event:expr) => {
        #[cfg(feature = "kernel-tracing")]
        {
            $crate::observability::trace::record($event);
        }
    };
}
```

When `kernel-tracing` is not enabled (the default), this expands to an empty block that the compiler eliminates entirely. Adding trace points to hot paths has zero overhead in the default build.

**Print macros** (from `uart.rs`):

```rust
#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => {
        $crate::arch::aarch64::uart::_print(
            format_args!("{}\n", format_args!($($arg)*))
        )
    };
}
```

These mirror the standard library's `print!`/`println!` interface but route output through the PL011 UART driver. Available from anywhere in the kernel crate via `crate::println!()`.

### 2.9 Unsafe Pattern: VirtIO MMIO Device Probe

The VirtIO-blk driver (`drivers/virtio_blk.rs`) demonstrates a two-stage device discovery pattern used for MMIO-attached VirtIO devices on QEMU virt:

**Stage 1 -- DTB-first probe** (preferred):

```rust
fn probe(dt: &DeviceTree) -> Option<usize> {
    // Check DTB-provided VirtIO MMIO base addresses first.
    for i in 0..dt.virtio_mmio_count {
        let phys = dt.virtio_mmio_bases[i] as usize;
        if probe_slot(phys) {
            return Some(phys);
        }
    }
    // Stage 2: brute-force scan if DTB is incomplete.
    for slot in 0..VIRTIO_MMIO_SLOT_COUNT {
        let phys = VIRTIO_MMIO_REGION_BASE as usize + slot * VIRTIO_MMIO_REGION_STRIDE as usize;
        if probe_slot(phys) {
            return Some(phys);
        }
    }
    None
}
```

**Stage 2 -- Slot validation** (three-check chain):

```rust
fn probe_slot(phys: usize) -> bool {
    let virt = MMIO_BASE + phys;
    // SAFETY: MMIO read from the VirtIO MMIO region, mapped as device memory.
    let magic = unsafe { mmio_read32(virt + VIRTIO_MMIO_MAGIC_VALUE) };
    if magic != VIRTIO_MMIO_MAGIC {          // 0x74726976 ("virt" in LE)
        return false;
    }
    let version = unsafe { mmio_read32(virt + VIRTIO_MMIO_VERSION) };
    if version != 1 { return false; }        // Legacy MMIO only
    let device_id = unsafe { mmio_read32(virt + VIRTIO_MMIO_DEVICE_ID) };
    device_id == VIRTIO_DEVICE_ID_BLK        // 2 = block device
}
```

**Key design decisions:**

- **DTB-first, brute-force fallback**: DTB is authoritative but may be incomplete on some platforms. The brute-force scan covers all `VIRTIO_MMIO_SLOT_COUNT` (32) MMIO slots at 512-byte stride starting at `0x0A00_0000`.
- **MMIO via TTBR1 mapping**: All device registers are accessed through the kernel's MMIO virtual mapping (`MMIO_BASE + phys`), not via the identity map.
- **VirtIO spec compliance**: The three-check chain (magic → version → device ID) follows VirtIO spec §3.1 initialization sequence exactly.

**VirtIO initialization sequence** (after probe succeeds):

```text
1. Reset device          (write 0 to Status register)
2. Set ACKNOWLEDGE        (Status |= 1)
3. Set DRIVER             (Status |= 2)
4. Negotiate features     (read DEVICE_FEATURES, write DRIVER_FEATURES)
5. Set GUEST_PAGE_SIZE    (legacy v1 requirement: write 4096)
6. Configure virtqueue    (set QUEUE_SEL, read QUEUE_NUM_MAX, set QUEUE_NUM, QUEUE_ALIGN)
7. Allocate DMA pages     (buddy allocator, DMA pool)
8. Set QUEUE_PFN          (physical page frame number of descriptor table)
9. Set DRIVER_OK          (Status |= 4)
```

**DMA allocation pattern** (from `init_device`):

```rust
// Compute virtqueue memory layout using const helper functions.
let total = virtqueue_size(queue_size as usize);
let total_pages = total.div_ceil(4096);
let order = order_for_pages(total_pages);  // local helper: smallest order where 2^order >= pages

// Allocate from the DMA pool (ensures cache-coherent physical memory).
let vq_phys = crate::mm::frame::alloc_dma_pages(order)?;
let vq_virt = DIRECT_MAP_BASE + vq_phys;
```

### 2.10 Pattern: Content-Addressed Block I/O

The Block Engine (`storage/block_engine.rs`) implements a content-addressed storage layer with crash-safe writes:

**CRC-32C integrity verification** (inline, table-driven):

```rust
/// CRC-32C lookup table using Castagnoli polynomial 0x1EDC6F41.
const CRC32C_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let poly: u32 = 0x82F6_3B78; // bit-reversed 0x1EDC6F41
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 { crc = (crc >> 1) ^ poly; } else { crc >>= 1; }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc = CRC32C_TABLE[((crc ^ byte as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}
```

This pattern uses a **const-evaluated lookup table** -- the 256-entry table is computed entirely at compile time. The `const` block with `while` loops (not `for` -- `for` is not allowed in `const` contexts) generates the same table that would be produced by a runtime initializer, with zero startup cost.

**SHA-256 content hashing** (via `sha2` crate, `no_std`):

```rust
use sha2::{Digest, Sha256};

fn compute_hash(data: &[u8]) -> ContentHash {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    ContentHash(result.into())
}
```

**WAL append-then-commit pattern** (crash-safe write path):

```text
1. Compute SHA-256 hash of data
2. Check MemTable for existing hash → deduplicate if found (bump refcount)
3. CRC-32C the data, compute sectors needed
4. WAL append (uncommitted entry) → returns (sequence, index) for O(1) commit
5. Write data blocks to disk (header + data + padding per sector)
6. WAL commit at stored index → toggles committed=1, recomputes entry CRC
7. MemTable insert (new entry with refcount=1)
```

If the kernel crashes between steps 4 and 6, recovery replays only committed WAL entries. Uncommitted entries with valid data CRC-32C are salvaged; those with invalid CRC are discarded.

**On-disk data layout per block:**

```text
First sector:   [crc32c:u32 | data_len:u32 | data[..min(504, len)] | padding]
Remaining:      [data[rest...] | padding to sector boundary]
```

### 2.11 Pattern: Defensive Indexing

A lesson from the Redox OS project: in kernel code, always prefer fallible access over panicking indexing.

**Do:**

```rust
// Defensive: returns None on out-of-bounds, caller handles gracefully
if let Some(entry) = table.get(index) {
    process(entry);
}
```

**Don't:**

```rust
// Panicking: kernel crash on out-of-bounds (unacceptable in a kernel)
let entry = table[index];  // panics if index >= table.len()
```

AIOS uses direct indexing (`foo[n]`) only when the index is provably in-bounds (e.g., `core_id & 0x3` for a 4-element array, or iterating with `enumerate()`). For indices from external input (syscall arguments, device registers), always use `.get()` or explicit bounds checking.

### 2.12 Unsafe Pattern: Read-Unaligned Struct Recovery

The WAL (`storage/wal.rs`) reads on-disk structures from sector buffers where alignment is not guaranteed:

```rust
fn read_entry(&self, logical_index: u64) -> Result<WalEntry, StorageError> {
    let sector_index = logical_index / WAL_ENTRIES_PER_SECTOR as u64;
    let entry_offset = (logical_index % WAL_ENTRIES_PER_SECTOR as u64) as usize;
    let byte_offset = entry_offset * WAL_ENTRY_SIZE;

    let mut sector_buf = [0u8; SECTOR_SIZE];
    virtio_blk::read_sector(absolute_sector, &mut sector_buf)?;

    // SAFETY: WalEntry is repr(C) and 64 bytes. The sector buffer is valid for reads.
    // read_unaligned handles any alignment mismatch between the buffer and the struct.
    let entry = unsafe {
        core::ptr::read_unaligned(
            sector_buf[byte_offset..].as_ptr() as *const WalEntry
        )
    };
    Ok(entry)
}
```

**Why `read_unaligned`?** Sector buffers are `[u8; 512]` with alignment 1. `WalEntry` may have alignment requirements > 1. Using `core::ptr::read` on a misaligned pointer is undefined behavior. `read_unaligned` performs a byte-by-byte copy that is always safe regardless of the source pointer's alignment.

**The inverse -- writing structs to buffers:**

```rust
// SAFETY: WalEntry is repr(C), 64 bytes. Copies struct bytes into the sector buffer.
unsafe {
    core::ptr::copy_nonoverlapping(
        &entry as *const WalEntry as *const u8,
        sector_buf[byte_offset..].as_mut_ptr(),
        WAL_ENTRY_SIZE,
    );
}
```

**When to use which:**

| Operation | Function | When |
|---|---|---|
| Read struct from byte buffer | `ptr::read_unaligned` | On-disk structs, network packets, DMA buffers |
| Write struct to byte buffer | `ptr::copy_nonoverlapping` | Serializing structs to sector buffers |
| Read struct from aligned memory | `ptr::read` | Stack/heap structs with known alignment |
| Read hardware register | `ptr::read_volatile` | MMIO registers (prevents optimizer elimination) |

---

## 3. Code Style and Organization

### 3.1 File Size Norms

AIOS kernel files follow standard Rust community size expectations, adjusted for kernel overhead:

| Range | Interpretation | Examples |
|---|---|---|
| < 100 lines | Small, focused utility | `bump.rs` (44), `heap.rs` (68), `boot_phase.rs` (68) |
| 100--300 lines | Typical module | `uart.rs` (157), `timer.rs` (211), `cap/mod.rs` (236), `smp.rs` (218), `wal.rs` (248) |
| 300--500 lines | Larger subsystem | `pgtable.rs` (436), `slab.rs` (493), `service/mod.rs` (403), `sched/scheduler.rs` (432), `virtio_blk.rs` (489) |
| 500--800 lines | Complex module; consider splitting | `buddy.rs` (680), `syscall/mod.rs` (668), `shmem.rs` (642), `block_engine.rs` (614), `bench.rs` (549) |
| > 800 lines | Must split into submodules | (none currently; `ipc/` and `sched/` were split) |

**Guidelines:**

- Aim for 200--500 lines per file.
- Consider splitting at approximately 600 lines. Split by extracting a logical sub-concern into its own file within the same directory.
- Kernel code runs approximately 50% larger than application code due to `// SAFETY:` comments and MMIO boilerplate. A 600-line kernel file is roughly equivalent to a 400-line application file in terms of logic density.

**IPC as a split example:** The original 2035-line `ipc/mod.rs` was split by concern into focused submodules:

```text
ipc/
  mod.rs      (215)  # Channel struct, CHANNEL_TABLE, create/destroy, re-exports
  channel.rs  (507)  # ipc_call, ipc_recv, ipc_reply, ipc_send, ipc_cancel
  timeout.rs  (185)  # Timeout queue, sleep helpers, wakeup error delivery
  direct.rs          # Direct switch fast path, priority inheritance, reply switch
  tests.rs    (668)  # Test initialization, thread entries, test-only helpers
  notify.rs          # Notification objects (signal/wait)
  select.rs          # IPC select (multi-wait)
  shmem.rs           # Shared memory regions
```

**Scheduler as a split example:** The 840-line `sched/mod.rs` was split into:

```text
sched/
  mod.rs         (154)  # RunQueue, globals, thread allocation helpers, re-exports
  scheduler.rs   (432)  # schedule(), enter_scheduler(), timer_tick(), block/unblock
  init.rs        (277)  # Scheduler init, idle/test threads, load balancer
```

### 3.2 Module Structure Patterns

**Pattern 1: Simple re-export** -- for arch-specific modules

```rust
// From kernel/src/arch/aarch64/mod.rs
pub mod exceptions;
pub mod gic;
pub mod mmu;
pub mod psci;
pub mod timer;
pub mod trap;
pub mod uart;
```

No `use` statements. Submodules are accessed via their full path: `crate::arch::aarch64::uart::putc()`. This keeps the module boundary explicit and avoids ambiguity about where a symbol originates.

**Pattern 2: Shared/kernel split** -- types in `shared/`, logic in `kernel/`

Types that cross the kernel/UEFI-stub boundary live in the `shared` crate. The kernel re-exports them:

```rust
// From kernel/src/observability/mod.rs
pub use shared::{LogEntry, LogLevel, Subsystem};

// From kernel/src/ipc/mod.rs
pub use shared::{
    ChannelId, EndpointState, RawMessage, DEFAULT_TIMEOUT_TICKS,
    MAX_CHANNELS, MAX_MESSAGE_SIZE, RING_CAPACITY,
};
```

The `shared` crate is `no_std` and has zero kernel dependencies. This ensures the UEFI stub can use the same type definitions without pulling in kernel internals.

**Pattern 3: Feature-gated modules**

```toml
# From kernel/Cargo.toml
[features]
default = ["kernel-metrics"]
kernel-metrics = []
kernel-tracing = []
```

```rust
// From kernel/src/observability/trace.rs
#[macro_export]
macro_rules! trace_point {
    ($event:expr) => {
        #[cfg(feature = "kernel-tracing")]
        {
            $crate::observability::trace::record($event);
        }
    };
}
```

When `kernel-tracing` is disabled (the default), `trace_point!()` compiles to nothing -- zero runtime cost, zero binary size impact. Metrics (`kernel-metrics`) are enabled by default because they have low overhead and are needed for scheduler tuning.

**Pattern 4: Driver module** -- for device drivers

```text
kernel/src/drivers/
  mod.rs          (3)    # pub mod declarations only
  virtio_blk.rs   (489)  # Full VirtIO-blk MMIO driver
```

Driver modules follow a flat structure: `mod.rs` contains only `pub mod` re-exports, and each driver is a standalone file. The driver file owns a global `Mutex<Option<Device>>` static and exposes a public API (`init()`, `read_sector()`, `write_sector()`). No trait abstraction yet -- that comes when a second driver is added.

**Pattern 5: Storage subsystem** -- layered with clear dependency direction

```text
kernel/src/storage/
  mod.rs            (204)  # init(), run_self_tests(), re-exports
  block_engine.rs   (614)  # BlockEngine, Superblock, CRC-32C, SHA-256
  wal.rs            (248)  # WalEntry, circular buffer, append/commit
  lsm.rs            (114)  # MemTable, sorted Vec with binary search
```

Dependency direction is strictly downward: `mod.rs` → `block_engine.rs` → `wal.rs` + `lsm.rs`. The block engine calls into the VirtIO driver (`crate::drivers::virtio_blk`) for disk I/O. Shared types (`ContentHash`, `BlockLocation`, `StorageError`) live in `shared/src/storage.rs`.

### 3.3 Naming Conventions

AIOS follows standard Rust naming with strict consistency:

| Category | Convention | Examples |
|---|---|---|
| Functions | `snake_case` | `init_gicv3()`, `enable_slab_allocator()`, `advance_boot_phase()` |
| Types | `CamelCase` | `InterruptController`, `CapabilityToken`, `ThreadContext` |
| Constants | `SCREAMING_SNAKE` | `UART_PHYS`, `MAX_CORES`, `BOOTINFO_MAGIC` |
| Statics | `SCREAMING_SNAKE` | `SLAB_READY`, `PRINT_TURN`, `CHANNEL_TABLE` |
| Modules | `snake_case` | `boot_phase`, `observability`, `context_switch` |
| Enum variants | `CamelCase` | `LogLevel::Info`, `ThreadState::Blocked`, `Syscall::IpcCall` |
| Feature flags | `kebab-case` | `kernel-metrics`, `kernel-tracing` |

**Module-level doc comments** are required for every new file (except crate roots like `main.rs` and `lib.rs`, which use crate-level attributes instead). They describe the module's purpose, its role in the system, and reference the relevant architecture doc:

```rust
//! PL011 UART driver.
//!
//! Supports two modes:
//! 1. Early boot: hardcoded base 0x0900_0000 (QEMU pre-initialized).
//! 2. Post-DTB: full PL011 initialization from DTB-sourced base address
//!    with baud rate programming (required on real hardware).
```

```rust
//! Kernel observability: structured logging, metric counters, trace points.
//!
//! Replaces raw `println!()` with per-core ring-buffered structured logging.
//! Per observability.md SS2-4.
```

### 3.4 Documentation Standards

**SAFETY comments** follow the three-line format described in SS1 Tier 1. Here is the full example from `uart.rs` showing the pattern in context:

```rust
pub fn init_pl011(base: usize) -> Uart {
    // SAFETY: All writes are to PL011 MMIO registers at the DTB-provided
    // base address. The initialization sequence follows the PL011 TRM.
    // Writing to an invalid base would cause a synchronous data abort.
    unsafe {
        mmio_write32(base + UARTCR, 0);          // Disable UART
        // ... (wait for TX complete, flush FIFO) ...
        mmio_write32(base + UARTIBRD, 13);        // Baud rate integer divisor
        mmio_write32(base + UARTFBRD, 1);         // Baud rate fractional divisor
        mmio_write32(base + UARTLCR_H, 0x70);    // 8N1, FIFO enabled
        mmio_write32(base + UARTCR, 0x301);       // Enable UART + TX + RX
    }
    UART_BASE_ADDR.store(base, Ordering::Relaxed);
    Uart { base }
}
```

**Architecture doc references** appear in code comments to connect implementation details to the specification:

```rust
// PL011 register offsets (hal.md SS4.3)
const UARTDR: usize = 0x000;
```

```rust
//! Per observability.md SS2-4.
```

```rust
// Kernel virtual address space layout (memory.md SS3.1)
pub const KERNEL_BASE: usize = 0xFFFF_0000_0000_0000;
```

**Compile-time assertions** verify struct sizes and alignment:

```rust
const _: () = assert!(core::mem::size_of::<TrapFrame>() == 272);
const _: () = assert!(core::mem::size_of::<LogEntry>() == 64);
```

These catch layout regressions at compile time rather than producing silent data corruption at runtime.

**`#[allow(dead_code)]`** is used for code wired for future phases:

```rust
#[allow(dead_code)]
pub mod pgtable;  // Full API used in Phase 2+
```

This annotation is acceptable only for modules/functions that will be used in a subsequent phase. It must not be used to silence warnings about genuinely unused code.

---

## 4. Common Pitfalls

Each pitfall follows the format: Symptom, Root Cause, Do, Don't. These are real issues encountered during AIOS development.

### 4.1 NC Memory Atomics

**Symptom:** Core hangs during SMP bringup or on first `spin::Mutex::lock()` call. No exception output -- the core is stuck in an infinite loop.

**Root cause:** The Phase 1 identity map originally used Non-Cacheable Normal memory (edk2 MAIR Attr1=0x44). Atomic read-modify-write operations (`fetch_add`, `compare_exchange`, `swap`) compile to exclusive load/store pairs (`ldaxr`/`stlxr`). These instructions require the global exclusive monitor, which only works with Inner Shareable + Cacheable memory attributes. On NC memory, the exclusive store (`stlxr`) always reports failure, causing an infinite retry loop.

> **Current status (post-Phase 2 M8):** The TTBR0 identity map was upgraded to WB cacheable (MAIR Attr3) in Phase 2 M8. `spin::Mutex` and atomic RMW operations now work correctly on identity-mapped RAM. This pitfall remains relevant if you explicitly map memory as Non-Cacheable (device MMIO regions, framebuffer, etc.).

`spin::Mutex` internally uses `compare_exchange` to acquire the lock. Under contention (or even on first lock if the implementation uses a CAS loop), it hangs on NC memory.

**Do:**

```rust
// Safe on NC memory: plain load/store (compiled to ldar/stlr)
static FLAG: AtomicBool = AtomicBool::new(false);
FLAG.store(true, Ordering::Release);     // Compiles to: stlr
while !FLAG.load(Ordering::Acquire) {    // Compiles to: ldar
    core::hint::spin_loop();
}
```

**Don't:**

```rust
// HANGS on NC memory: exclusive load/store pairs
static COUNTER: AtomicUsize = AtomicUsize::new(0);
COUNTER.fetch_add(1, Ordering::Relaxed);  // Uses ldaxr/stlxr -- hangs

// HANGS on NC memory: spin::Mutex uses compare_exchange internally
static LOCK: spin::Mutex<u32> = spin::Mutex::new(0);
let guard = LOCK.lock();  // Hangs on first contended access
```

**Resolution:** After Phase 2 M8 upgrades the TTBR0 RAM blocks from NC (Attr1) to Write-Back cacheable (Attr3), all atomic operations and spinlocks work correctly. Until then, use only `load`/`store` with `Acquire`/`Release` ordering for inter-core synchronization.

### 4.2 TTBR Swap with MMU On

**Symptom:** CONSTRAINED UNPREDICTABLE behavior. Possible symptoms include cache corruption, data aborts at seemingly random addresses, or silent data corruption.

**Root cause:** Changing `MAIR_EL1` or `TCR_EL1` while the MMU is enabled violates ARM Architecture Reference Manual constraints. The CPU may use a mix of old and new attribute values for in-flight translations, causing cache corruption or aliasing.

**Do:**

```rust
// Swap TTBR0 only (keeps MAIR/TCR unchanged -- safe while MMU is on)
core::arch::asm!(
    "msr TTBR0_EL1, {ttbr0}",
    "isb",
    ttbr0 = in(reg) new_ttbr0,
);
```

**Don't:**

```rust
// UNDEFINED BEHAVIOR: changing MAIR while MMU is on
core::arch::asm!("msr MAIR_EL1, {}", in(reg) new_mair);

// UNDEFINED BEHAVIOR: changing TCR while MMU is on
core::arch::asm!("msr TCR_EL1, {}", in(reg) new_tcr);
```

**AIOS strategy:** Reuse edk2's MAIR and TCR configuration from Phase 1 onward. Build page tables with edk2-compatible attribute indices (Attr0=Device, Attr3=WB). Only swap TTBR0/TTBR1 (safe while MMU is on). Writing MAIR/TCR is only safe when the MMU is off -- which is the case during secondary core bringup (before `SCTLR_EL1` write).

### 4.3 TLBI Broadcast with Parked Cores

**Symptom:** System hangs after executing a TLB invalidation instruction. No exception output.

**Root cause:** Broadcast TLB invalidation instructions (any `tlbi` with the `IS` suffix, or `tlbi alle1`) combined with `dsb sy` or `dsb ish` wait for the invalidation to complete on ALL processing elements in the shareability domain. Secondary cores parked in a `wfe` loop never process the TLBI maintenance broadcast, so the `dsb` waits forever.

**Do:**

```rust
// During boot (secondary cores parked): local-only TLBI + non-shareable barrier
core::arch::asm!(
    "tlbi vmalle1",    // VM-local, non-broadcast
    "dsb nsh",         // Non-shareable barrier (this core only)
    "isb",
);
```

**Don't:**

```rust
// HANGS during boot: broadcast TLBI + shareable barrier
core::arch::asm!(
    "tlbi alle1",      // Broadcasts to all PEs in the inner shareable domain
    "dsb sy",          // Waits for ALL PEs to complete -- parked cores never do
    "isb",
);
```

**When broadcast is safe:** After all secondary cores are online and running on Write-Back cacheable memory (post-Phase 2 M8). The standard TLBI sequence for runtime use is:

```rust
// After all cores are online: broadcast is safe
core::arch::asm!(
    "tlbi vmalle1is",  // Inner-shareable broadcast
    "dsb ish",         // Wait for all cores in inner-shareable domain
    "isb",
);
```

### 4.4 Missing ISB After MSR

**Symptom:** Stale system register values are used. Exceptions may jump to the old vector table. MMU enable/disable may not take effect for several instructions.

**Root cause:** ARM processors are pipelined. `msr` writes a system register, but subsequent instructions may already be decoded and executing with the old value. `isb` (Instruction Synchronization Barrier) flushes the pipeline, ensuring all subsequent instructions see the new register value.

**Do:**

```rust
// ISB after registers that affect instruction fetch or translation
core::arch::asm!("msr VBAR_EL1, {}", in(reg) vbar);
core::arch::asm!("isb");  // Pipeline flush -- new VBAR takes effect immediately

core::arch::asm!("msr SCTLR_EL1, {}", in(reg) sctlr);
core::arch::asm!("isb");  // Pipeline flush -- MMU enable/disable takes effect
```

**Don't:**

```rust
// WRONG: no ISB after VBAR write
core::arch::asm!("msr VBAR_EL1, {}", in(reg) vbar);
// If an interrupt fires HERE, it may jump to the OLD vector table address!
```

**Registers requiring ISB after write:**

| Register | Controls | ISB required? |
|---|---|---|
| `VBAR_EL1` | Exception vector base | Yes |
| `SCTLR_EL1` | MMU enable, alignment checks | Yes |
| `TCR_EL1` | Translation control | Yes |
| `TTBR0_EL1` | User translation table base | Yes |
| `TTBR1_EL1` | Kernel translation table base | Yes |
| `CPACR_EL1` | FPU/NEON enable | Yes |
| `CNTP_TVAL_EL0` | Timer compare value | Yes (if followed by timer-dependent code) |
| `CNTP_CTL_EL0` | Timer enable/mask | Yes (if timing is critical) |

### 4.5 W^X Violations

**Symptom:** Security policy violation during code review or audit. Page is both writable and executable.

**Root cause:** A page table entry has both write permission and execute permission bits set. AIOS enforces Write XOR Execute (W^X) -- every page is either writable OR executable, never both. This policy prevents code injection attacks where an attacker writes shellcode to a writable page and then executes it.

**Do:**

```rust
// Correct: text segment is RX (read + execute, not writable)
// PTE bits: AP=RO, PXN=0, UXN=1
map_text(vaddr, paddr, pages);

// Correct: data segment is RW (read + write, not executable)
// PTE bits: AP=RW, PXN=1, UXN=1
map_data(vaddr, paddr, pages);
```

**Don't:**

```rust
// WRONG: page is both writable and executable
entry |= PTE_AP_RW;  // Writable
// Missing PXN/UXN bits -- page is also executable = W^X violation
```

**Reference:** `mm/kmap.rs` demonstrates correct permission separation for the kernel address space: `.text` = RX, `.rodata` = RO, `.data`/`.bss` = RW+XN, direct map = RW+XN, MMIO = Device+XN.

### 4.6 Forgetting DSB Before SEV

**Symptom:** Secondary core wakes from `wfe` but reads stale data from shared memory. May see zeroes instead of the expected stack pointer or entry point address.

**Root cause:** `sev` (Send Event) wakes cores from `wfe`, but does not guarantee that prior memory stores are visible to the woken core. ARM's relaxed memory model means stores may still be in the store buffer or write-combine buffer.

**Do:**

```rust
// Ensure all stores are committed to the coherency point before waking cores
core::arch::asm!("dsb sy");  // All stores visible to all observers
core::arch::asm!("sev");     // Wake all cores -- they will see the stores
```

**Don't:**

```rust
// WRONG: sev without dsb -- woken core may read stale data
SHARED_DATA.store(42, Ordering::Relaxed);
core::arch::asm!("sev");  // Core wakes but may see 0 instead of 42
```

### 4.7 Stack Alignment

**Symptom:** Alignment fault (`EC=0x25` in ESR_EL1) on the first function call after setting up the stack pointer.

**Root cause:** The AArch64 Procedure Call Standard (AAPCS64) requires 16-byte stack alignment at every public function boundary. If SP is not 16-byte aligned when a function is called, `stp`/`ldp` instructions that assume alignment will fault.

**Do:**

```asm
// Correct: ensure 16-byte alignment before any function call
mov sp, x1
and sp, sp, #~0xf    // Force 16-byte alignment
bl some_function      // Safe -- SP is aligned
```

**Don't:**

```asm
// WRONG: odd number of 8-byte pushes leaves SP misaligned
stp x0, x1, [sp, #-16]!   // SP aligned (pushed 16 bytes)
str x2, [sp, #-8]!         // SP now 8-byte aligned (not 16!)
bl some_function            // FAULT: misaligned stack
```

**In practice:** Always push/pop registers in pairs using `stp`/`ldp`. If you need to save an odd number of registers, pair the last one with `xzr` (zero register) to maintain 16-byte alignment.

### 4.8 DMA Memory Coherence

**Symptom:** VirtIO operations succeed on QEMU but fail silently on real hardware (Raspberry Pi 4/5). Reads return stale data; writes appear to be lost.

**Root cause:** QEMU's memory model is cache-coherent for DMA -- the virtual CPU and virtual devices share the same memory view. Real ARM hardware requires explicit cache maintenance for DMA buffers, or the buffers must be allocated from non-cacheable DMA memory.

**Do:**

```rust
// Allocate from the DMA pool (guaranteed cache-coherent on all platforms)
let frame = crate::mm::frame::alloc_dma_pages(order)?;

// Use DSB SY before device notification (ensures writes are visible to device)
core::arch::asm!("dsb sy");
unsafe { mmio_write32(base + VIRTIO_MMIO_QUEUE_NOTIFY, 0); }
```

**Don't:**

```rust
// WRONG on real hardware: kernel pool memory may be WB-cacheable
let frame = crate::mm::frame::alloc_pages(Pool::Kernel, order)?;
// Device sees stale cache lines, not the CPU's latest writes
```

**AIOS strategy:** The DMA pool (64 MB on QEMU 2G) is reserved for all device-facing buffers. The VirtIO driver allocates virtqueue descriptors and request buffers exclusively from DMA pages via `alloc_dma_pages()`. On QEMU this is functionally identical to kernel pool allocation, but the separation ensures correctness when porting to real hardware.

**Cross-reference:** Linux kernel DMA API documentation emphasizes that "coherent DMA memory does not preclude the usage of proper memory barriers" -- even with DMA-coherent memory, CPU store reordering requires `dsb sy` (equivalent to Linux's `dma_wmb()`) before the device reads the data.

### 4.9 VirtIO Ring Barrier Ordering

**Symptom:** VirtIO device intermittently misses requests, returns stale descriptors, or reports I/O errors. Works most of the time but fails under load.

**Root cause:** The CPU may reorder stores to the virtqueue available ring and the device doorbell (QUEUE_NOTIFY) register. If the device reads the available ring before the CPU's descriptor writes are visible, it processes stale or incomplete descriptors.

**Do:**

```rust
// Update available ring index
unsafe {
    let avail_idx_ptr = (avail_virt + 2) as *mut u16;
    core::ptr::write_volatile(avail_idx_ptr, new_idx);
}

// Memory barrier BEFORE doorbell notification (VirtIO spec §2.7.13.1)
core::arch::asm!("dsb sy");

// Notify device (doorbell write)
unsafe { mmio_write32(base + VIRTIO_MMIO_QUEUE_NOTIFY, 0); }
```

**Don't:**

```rust
// WRONG: no barrier between ring update and doorbell
core::ptr::write_volatile(avail_idx_ptr, new_idx);
mmio_write32(base + VIRTIO_MMIO_QUEUE_NOTIFY, 0);  // Device may see old ring index
```

The VirtIO specification (§2.7.13.1) requires: "The driver MUST perform a suitable memory barrier before the idx update, to ensure the device sees the most up-to-date copy."

---

## 5. Build, Test, and Verification Workflow

### 5.1 Just Commands

AIOS uses [just](https://just.systems/) as its build system wrapper. All recipes are defined in the `justfile` at the repository root:

| Command | What it does |
|---|---|
| `just build` | Debug kernel build (`cargo build --target aarch64-unknown-none`) |
| `just build-release` | Release kernel build (`cargo build --release`) |
| `just build-stub` | Compile the UEFI stub (`cargo build -p uefi-stub --target aarch64-unknown-uefi`) |
| `just disk` | Create the 64 MiB FAT32 ESP disk image with UEFI stub + kernel ELF |
| `just create-data-disk` | Create 256 MiB raw data disk image for VirtIO-blk storage (if not exists) |
| `just run` | Build everything and launch QEMU with edk2 UEFI firmware |
| `just run-display` | Same as `run` but with a graphical window (for framebuffer testing) |
| `just run-direct` | Phase 0 mode: direct `-kernel` boot, no UEFI (quick debugging) |
| `just debug` | Launch QEMU paused with GDB server on `tcp::1234` |
| `just test` | Run host-side unit tests (shared crate) |
| `just clippy` | Run clippy on kernel and stub targets with `-D warnings` |
| `just fmt` | Format code with `cargo fmt` |
| `just fmt-check` | Check formatting without modifying files (CI mode) |
| `just check` | **CI gate**: `fmt-check` + `clippy` + `build` + `build-stub` |
| `just audit` | Audit dependencies for known vulnerabilities (RustSec) |
| `just deny` | Check dependency policy (licenses, bans, advisories) |
| `just miri` | Run Miri on the shared crate (detects undefined behavior in unsafe code) |
| `just security-check` | `audit` + `deny` + `miri` |
| `just clean` | Remove build artifacts and disk image |

**Note:** `just run`, `just run-display`, and `just debug` depend on `create-data-disk` and include VirtIO data disk QEMU flags (`-drive file=data.img,if=none,format=raw,id=disk0 -device virtio-blk-device,drive=disk0`).

**Daily workflow:**

```bash
# Edit code, then verify it compiles
just build

# Full CI check before committing
just check

# Run unit tests
just test

# Test with QEMU (boots the full kernel)
just run
# Exit QEMU with: Ctrl-A X
```

### 5.2 Quality Gates

Every milestone must pass these gates before it can be considered complete:

| Gate | Command | Passes when |
|---|---|---|
| **Compile** | `cargo build --target aarch64-unknown-none` | Zero warnings |
| **Check** | `just check` | Zero warnings, zero errors |
| **Test** | `just test` | All 275+ host-side tests pass |
| **QEMU** | `just run` | UART output matches phase acceptance criteria |
| **CI** | Push to GitHub | All CI jobs pass |
| **Objdump** | `cargo objdump -- -h` | Sections at expected VMA/LMA addresses |

**Common failures and fixes:**

| Failure | Cause | Fix |
|---|---|---|
| `unused variable` warning | Clippy strict mode (`-D warnings`) | Prefix with `_` or remove the variable |
| `unsafe` without SAFETY comment | Code review / clippy custom lint | Add the three-line SAFETY comment |
| Test timeout in CI | QEMU boot regression | Check recent changes to `boot.S` or `kernel_main` init sequence |
| CI format check failure | Uncommitted formatting changes | Run `just fmt` then commit the changes |
| Clippy `dead_code` warning | Function added for future phase | Add `#[allow(dead_code)]` with a comment explaining which phase uses it |

### 5.3 QEMU Verification

After `just run`, QEMU boots with edk2 UEFI firmware and launches the kernel. The kernel outputs diagnostic information over the serial console (UART).

**Controls:**

| Shortcut | Action |
|---|---|
| `Ctrl-A X` | Exit QEMU immediately |
| `Ctrl-A C` | Switch to QEMU monitor (type `quit` to exit, or `info mtree` for memory map) |
| `Ctrl-A H` | Show QEMU multiplexer help |

**Matching acceptance criteria:** Phase docs specify exact UART strings to verify. Example acceptance criteria:

```text
Acceptance: `just run` shows:
  AIOS kernel booting...
  BootInfo at 0x..., magic OK
  EL: 1
```

Match the string patterns (literal text), not exact hex addresses (which may vary between boots or QEMU versions). The `0x...` notation indicates a variable hex value.

**Timeout:** If the kernel does not produce the expected output within approximately 10 seconds, something is wrong. QEMU boots in under 2 seconds on modern hardware.

### 5.4 Host-Side Tests

The kernel crate is `no_std` and cannot run host tests directly. All testable logic lives in the `shared` crate, which compiles for both the kernel target and the host:

```bash
# Run all shared crate tests
just test

# Equivalent manual command:
cargo test --workspace --exclude kernel --exclude uefi-stub --target-dir target/host-tests
```

Currently 275 tests across: `boot`, `cap`, `collections`, `ipc`, `kaslr`, `memory`, `observability`, `sched`, `storage`, `syscall`.

**Adding a new test:**

```rust
// In shared/src/collections.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_queue_push_pop() {
        let mut q = FixedQueue::<u32, 4>::new();
        assert!(q.push(42).is_ok());
        assert_eq!(q.pop(), Some(42));
    }

    #[test]
    fn fixed_queue_overflow() {
        let mut q = FixedQueue::<u32, 2>::new();
        assert!(q.push(1).is_ok());
        assert!(q.push(2).is_ok());
        assert!(q.push(3).is_err());  // Queue full
    }
}
```

Test modules are placed inside the `shared` crate source files alongside the code they test. The `kernel` crate does not contain `#[cfg(test)]` modules -- kernel logic is tested via QEMU boot verification and through the shared crate's unit tests for any extractable logic.

### 5.5 Extracting Kernel Logic for Host Testing

A key testing strategy in AIOS is extracting pure logic from `kernel/` into `shared/` so it can be tested on the host. This was used extensively in Phase 3 to add 83 tests for capabilities, IPC types, memory utilities, and service types.

**What can be extracted:** Any function or data structure that does not depend on kernel-specific state (hardware registers, global statics, inline assembly, interrupt context). Good candidates include:

- Data structure methods (e.g., `CapabilityTable::grant/revoke/attenuate`)
- Pure computation (e.g., `order_for_pages()`, `ticks_to_ns()`)
- Validation logic (e.g., `validate_user_va()`, `Capability::permits()`)
- Type definitions with associated constants (e.g., `SelectEntry`, `ServiceName`)

**What cannot be extracted:** Functions that depend on kernel globals (`CHANNEL_TABLE`, `PROCESS_TABLE`), call assembly intrinsics, or interact with hardware.

**Dependency injection pattern:** When a function is mostly pure but has one kernel dependency, inject it as a parameter:

```rust
// BEFORE (in kernel): calls kernel-only new_token_id()
pub fn attenuate(&mut self, handle: CapabilityHandle, ...) -> Result<...> {
    let new_id = crate::cap::new_token_id();  // AtomicU64 in kernel
    // ...
}

// AFTER (in shared): caller provides the ID
pub fn attenuate(&mut self, handle: CapabilityHandle, ..., new_id: CapabilityTokenId) -> Result<...> {
    // Pure logic — no kernel dependencies
}
```

The kernel call site provides the injected value:

```rust
// kernel/src/syscall/mod.rs
let new_id = crate::cap::new_token_id();
proc.cap_table.attenuate(handle, new_cap, new_expiry, pid, new_id)
```

**Test helper pattern:** Create helpers at the top of the test module for common setup:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn make_token(id: u64, cap: Capability) -> CapabilityToken {
        CapabilityToken {
            id: CapabilityTokenId(id),
            capability: cap,
            holder: ProcessId(1),
            // ... sensible defaults
        }
    }

    #[test]
    fn grant_and_retrieve() {
        let mut table = CapabilityTable::new();
        let token = make_token(1, Capability::ChannelCreate);
        let handle = table.grant(token).unwrap();
        assert!(table.get(handle).is_some());
    }
}
```

**`no_std` test constraints:** The `shared` crate is `no_std`, so tests cannot use `Vec`, `String`, or heap allocation. Use fixed-size arrays and stack-based data structures. The `#[cfg(test)]` module inherits the parent's `no_std` setting but `cargo test` links the standard library, so `assert_eq!` and `#[should_panic]` work normally.

**Current test distribution (275 tests):**

| Module | Tests | Coverage |
|---|---|---|
| `cap` | 51 | Capability permissions, token lifecycle, table grant/revoke/cascade/attenuate/list |
| `ipc` | 48 | Channel IDs, message validation, select entries, service names, user VA checks |
| `memory` | 41 | Buddy math, pool config, order_for_pages, ticks_to_ns, BenchStats |
| `storage` | 33 | Content types, block locations, VirtIO constants, struct sizes, WAL capacity |
| `boot` | 22 | BootInfo validation, EarlyBootPhase ordering, memory descriptors |
| `collections` | 18 | FixedQueue, RingBuffer edge cases |
| `observability` | 18 | Log level ordering, subsystem tags |
| `sched` | 18 | Thread state, scheduler class, CpuSet, resource limits, priority |
| `syscall` | 15 | Syscall numbering, IpcError codes |
| `kaslr` | 11 | KASLR slide computation, alignment, bounds |

---

## 6. Debugging Techniques

### 6.1 UART Printf Debugging

The primary debugging tool in AIOS is structured logging via the `klog!` macro family:

```rust
kinfo!(Boot, "BootInfo at {:#x}, magic OK", boot_info_ptr);
kwarn!(Mm, "Pool {} has low free pages: {}", pool_name, free_count);
kerror!(Ipc, "Channel {} not found", channel_id);
kdebug!(Sched, "Context switch: {} -> {}", from_tid, to_tid);
```

**Output format:** `[timestamp] [core] LEVEL Subsystem message`

```text
[   0.001234] [0] INFO  Boot  AIOS kernel booting...
[   0.005678] [0] INFO  Mm    Buddy init: 508123 free pages across 4 pools
[   0.010000] [2] INFO  Boot  Secondary core 2 online
```

**Available subsystem tags:** `Boot`, `Mm`, `Sched`, `Ipc`, `Cap`, `Irq`, `Timer`, `Uart`, `Gic`, `Mmu`, `Smp`, `Storage`, `Audit`.

**Available log levels:** `Trace`, `Debug`, `Info`, `Warn`, `Error`. In debug builds, all levels from `Debug` up are emitted. In release builds, only `Info` and above.

**Early boot behavior:** Before the `LogRingsReady` boot phase is reached, `klog!` writes directly to the UART (synchronous, immediate output). After `LogRingsReady`, it writes to per-core ring buffers that are drained by the timer tick handler every 1 ms. This means early boot messages appear immediately, while later messages may be slightly delayed.

**Exception handler note:** Exception vector stubs use direct `putc()` calls, not `klog!`. This prevents recursive faults when TTBR0 is switched away from the identity map (which would make the logging format string inaccessible).

### 6.2 QEMU GDB

For low-level debugging where UART output is insufficient, attach GDB to QEMU:

```bash
# Terminal 1: start QEMU paused at first instruction
just debug

# Terminal 2: connect GDB
aarch64-none-elf-gdb target/aarch64-unknown-none/debug/kernel
(gdb) target remote :1234
(gdb) break kernel_main
(gdb) continue
```

**Useful GDB commands for kernel debugging:**

| Command | Purpose |
|---|---|
| `info registers` | Show all general-purpose and system registers |
| `x/16gx $sp` | Examine 16 quad-words (64-bit) at the stack pointer |
| `x/10i $pc` | Disassemble 10 instructions at the program counter |
| `break kernel_main` | Break at kernel entry point |
| `break *0xFFFF000000080000` | Break at a specific virtual address |
| `stepi` | Single-step one machine instruction |
| `nexti` | Step over one instruction (skip function calls) |
| `bt` | Show backtrace (if frame pointers are preserved) |
| `monitor info mtree` | Show QEMU's view of the memory map |
| `monitor info registers` | QEMU-level register dump (more complete than GDB's) |

**Reading system registers:** GDB for AArch64 provides access to system registers through `info registers`. You can also use `monitor` commands to query QEMU directly.

**Tip:** The `just debug` recipe starts QEMU with `-S` (paused). The CPU is stopped at the reset vector. Use `continue` to let it run to your breakpoint.

### 6.3 Objdump Section Verification

Verify that the linked binary has sections at the expected addresses:

```bash
cargo objdump --target aarch64-unknown-none -- -h
```

Expected output (Phase 2+, with virtual linking):

```text
Idx Name           Size     VMA              LMA              Type
  0 .text          ...      ffff000000080000 0000000040080000 TEXT
  1 .rodata        ...      ffff0000000xxxxx 00000000400xxxxx DATA
  2 .data          ...      ffff0000000xxxxx 00000000400xxxxx DATA
  3 .bss           ...      ffff0000000xxxxx 00000000400xxxxx BSS
```

**Key checks:**

- **VMA** (Virtual Memory Address) should be in the `0xFFFF_0000_...` range -- these are kernel virtual addresses.
- **LMA** (Load Memory Address) should be in the `0x4008_0000` range -- this is the physical address where UEFI loads the kernel ELF.
- `.text` should be the first section (code pages are RX under W^X policy).
- `.text.vectors` and `.text.rvectors` should appear before `.text` with correct alignment (2048 bytes for vector tables).

### 6.4 Common Debug Scenarios

**"Kernel hangs at boot" (no UART output at all):**

1. Is the UART initialized? Phase 0 uses QEMU's pre-initialized UART. Phase 1+ requires `init_pl011()` from DTB. Check the UART base address.
2. Is the exception vector table installed? A missing `VBAR_EL1` write means the first exception crashes silently -- the CPU jumps to address 0x0.
3. Is the stack pointer aligned to 16 bytes? A misaligned SP faults before the first `println!` call.
4. Is the FPU enabled? Rust generates NEON/FP instructions by default (hard-float ABI). A disabled FPU faults on the first such instruction.
5. Check `boot.S` execution order: FPU enable must come before any Rust code runs.

**"Data abort at address X" (ESR_EL1 in exception output):**

1. Read the ESR_EL1 value from the exception handler output.
2. Extract the Data Fault Status Code (DFSC) from bits [5:0]:

| DFSC | Meaning |
|---|---|
| `0b000100` (4) | Translation fault, level 0 -- L0 page table entry invalid |
| `0b000101` (5) | Translation fault, level 1 -- L1 entry missing |
| `0b000110` (6) | Translation fault, level 2 -- L2 entry missing |
| `0b000111` (7) | Translation fault, level 3 -- L3 (PTE) entry missing |
| `0b001001` (9) | Access flag fault -- PTE exists but AF bit not set |
| `0b001101` (13) | Permission fault -- write to read-only or execute of XN page |

3. Read FAR_EL1 (Faulting Address Register) to see which address triggered the fault.
4. Cross-reference the faulting address against your page table mappings. Is the address in the kernel range (`0xFFFF...`)? The direct map range? MMIO range? Unmapped?

**"Timer interrupts not firing":**

1. Is `CNTP_CTL_EL0.ENABLE` set? (bit 0 = 1)
2. Is `CNTP_CTL_EL0.IMASK` clear? (bit 1 = 0 means interrupts are not masked at the timer level)
3. Is the GIC PPI enabled? Check that `gic.enable_irq(30)` was called (INTID 30 is the EL1 physical timer).
4. Is `PSTATE.I` clear? The CPU-level interrupt mask must be cleared: `msr DAIFClr, #0x2`.
5. Is the GIC CPU interface priority mask set correctly? `ICC_PMR_EL1` must be higher than the interrupt priority.

**"IPC call blocks forever":**

1. Check that the channel exists in `CHANNEL_TABLE` and has not been destroyed.
2. Check the receiver thread's state -- is it actually blocked in `IpcRecv` waiting on this channel?
3. Check the timeout value -- `u64::MAX` means no timeout (intentional for some use cases, but verify).
4. Check capability enforcement -- does the calling thread's process have a `ChannelAccess` capability for this channel?
5. Check lock ordering -- if both `PROCESS_TABLE` and `CHANNEL_TABLE` locks are needed, `PROCESS_TABLE` must be acquired first (see [deadlock-prevention.md](../kernel/deadlock-prevention.md)).

**"VirtIO device not found" (storage init fails):**

1. Does `data.img` exist? Run `just create-data-disk` to create the 256 MiB data disk.
2. Check QEMU command line for the VirtIO data disk flags: `-drive if=none,id=data0,file=data.img,format=raw` and `-device virtio-blk-device,drive=data0`.
3. Verify the VirtIO MMIO probe range: the driver scans `0x0A00_0000` to `0x0A00_3E00` at 512-byte stride. If the data disk is attached as `virtio-blk-pci` instead of `virtio-blk-device`, it will not appear in the MMIO scan range.
4. Check the DTB for VirtIO MMIO slots: `dt.virtio_mmio_bases` should contain the device base address.

**"Block Engine CRC mismatch on read":**

1. The block was likely corrupted during a previous write. Check the WAL for the entry -- is it marked `committed=1`?
2. If the entry is committed but data CRC fails, the disk image may be corrupted. Delete `data.img` and re-run to trigger a fresh format.
3. On QEMU with `cache=none` or `cache=writeback`, I/O ordering guarantees differ. The default (`cache=none` on raw) should work correctly with the DSB SY barriers in the VirtIO driver.

### 6.5 Kernel Address Space Map

Understanding the virtual address layout is essential for interpreting faulting addresses and page table entries:

```text
Virtual Address Space (48-bit, T1SZ=16)
========================================

TTBR1 (kernel) — upper half: 0xFFFF_0000_0000_0000 and above
─────────────────────────────────────────────────────────────

0xFFFF_0010_0000_0000  ┌─────────────────────┐
                       │  MMIO mappings       │  Device memory (Attr0)
                       │  UART, GIC, etc.     │  2 MB blocks, RW+XN
                       └─────────────────────┘
          ...          │  (unmapped gap)       │
0xFFFF_0001_0000_0000  ┌─────────────────────┐
                       │  Direct map           │  All physical RAM
                       │  (DIRECT_MAP_BASE)    │  2 MB blocks, RW+XN
                       │  phys 0x4000_0000+    │  WB cacheable (Attr3)
                       └─────────────────────┘
          ...          │  (unmapped gap)       │
0xFFFF_0000_000x_xxxx  ┌─────────────────────┐
                       │  .bss / .data         │  RW+XN (4 KB pages)
                       ├─────────────────────┤
                       │  .rodata              │  RO+XN (4 KB pages)
                       ├─────────────────────┤
                       │  .text                │  RX (4 KB pages)
0xFFFF_0000_0008_0000  └─────────────────────┘  ← KERNEL_VIRT_BASE


TTBR0 (user / identity) — lower half: 0x0000_0000_0000_0000
─────────────────────────────────────────────────────────────

0x0000_0000_8000_0000  ┌─────────────────────┐
                       │  RAM block 2          │  1 GB block, WB (Attr3)
0x0000_0000_4000_0000  ├─────────────────────┤
                       │  RAM block 1          │  1 GB block, WB (Attr3)
                       │  (kernel load @       │  Contains kernel at
                       │   0x4008_0000)        │  physical load address
0x0000_0000_0000_0000  ├─────────────────────┤
                       │  Device block         │  1 GB block, Device (Attr0)
                       │  UART @ 0x0900_0000   │  MMIO peripherals
                       └─────────────────────┘
```

**Address translation quick reference:**

| Address range | What it is | How to interpret |
|---|---|---|
| `0xFFFF_0000_0008_xxxx` | Kernel code/data | Subtract `VIRT_PHYS_OFFSET` to get physical |
| `0xFFFF_0001_xxxx_xxxx` | Direct map | Subtract `DIRECT_MAP_BASE`, add `0x4000_0000` for physical |
| `0xFFFF_0010_xxxx_xxxx` | MMIO | Subtract `MMIO_BASE` to get MMIO physical |
| `0x0000_0000_4xxx_xxxx` | Identity-mapped RAM | Address IS the physical address |
| `0x0000_0000_09xx_xxxx` | Identity-mapped MMIO | Address IS the physical address |

**Converting between virtual and physical:**

```rust
// Kernel virtual to physical (for addresses in kernel image)
let phys = virt.wrapping_sub(VIRT_PHYS_OFFSET);
// VIRT_PHYS_OFFSET = 0xFFFE_FFFF_C000_0000

// Direct map virtual to physical
let phys = (virt - DIRECT_MAP_BASE) + 0x4000_0000;
// DIRECT_MAP_BASE = 0xFFFF_0001_0000_0000, RAM starts at 0x4000_0000
```

### 6.6 Common QEMU Failure Modes

These are failure patterns encountered during AIOS development (Phases 0--3), with root causes and resolutions. They are specific to QEMU `virt` with `-cpu cortex-a72 -smp 4`.

**Silent hang (no output, QEMU unresponsive to Ctrl-C):**

| Cause | Diagnosis | Fix |
|---|---|---|
| `dsb sy` with parked secondary cores on NC memory | Hang occurs during SMP init or first TLBI broadcast | Use `dsb nsh` (local only) until cores are fully online with WB memory |
| `spin::Mutex` on NC memory | Hang on first contended lock acquire | Replace with `load(Acquire)`/`store(Release)` protocol; see SS4.1 |
| Infinite loop in exception handler | Core took exception but handler loops forever | Add UART putc calls at top of exception vector stubs |
| PSCI CPU_ON with virtual entry address | Secondary core jumps to virtual address with MMU off | Convert `_secondary_entry` to physical before PSCI call (`smp.rs`) |
| VirtIO polling timeout | Hang during storage init or block I/O | Device not responding; verify `data.img` exists and QEMU flags include `virtio-blk-device` |

**Garbage UART output (random characters instead of text):**

| Cause | Diagnosis | Fix |
|---|---|---|
| Wrong baud rate divisors | Phase 1+ after PL011 full init | IBRD=13, FBRD=1 for 115200 baud at 24 MHz APB clock |
| Writing before UART FIFO ready | Characters dropped or corrupted | Check TXFF (bit 5 of FR) before each write |
| MMIO base address wrong | All output garbage from start | Verify UART base 0x0900_0000 on QEMU virt |

**Immediate reboot (QEMU restarts the kernel):**

| Cause | Diagnosis | Fix |
|---|---|---|
| Missing VBAR_EL1 setup | First exception causes jump to address 0x0 | Set VBAR_EL1 in boot.S before any Rust code runs |
| Stack pointer misaligned | SP not 16-byte aligned causes fault | Ensure `.balign 16` on stack symbols in linker script |
| FPU not enabled | First NEON instruction faults | Enable FPU in boot.S: `orr x1, x1, #(3 << 20); msr CPACR_EL1, x1; isb` |

**QEMU prints exception info then halts:**

| Cause | Diagnosis | Fix |
|---|---|---|
| Data abort from unmapped VA | FAR_EL1 shows the unmapped address | Check page table mappings cover the accessed range |
| W^X violation | Permission fault on write to RX page | Ensure page is mapped RW (not RX) for data sections |
| Address confusion (virt vs phys) | FAR_EL1 shows physical address while MMU expects virtual | After MMU enable, all pointers must be virtual; convert via `VIRT_PHYS_OFFSET` |

**IPC/scheduler specific failures:**

| Symptom | Cause | Fix |
|---|---|---|
| Thread blocks and never wakes | Notification waiter not registered before signal | Ensure register-then-block ordering with table lock held across both |
| IPC timeout never fires | `NOTIFY_DEADLINES` lock contention in timer IRQ | Use `try_lock()` in IRQ context, never blocking lock |
| Double context switch corruption | `IN_SCHEDULER` guard not set | Set per-CPU `IN_SCHEDULER` AtomicBool before schedule(), clear after |
| Direct switch to wrong thread | CURRENT_THREAD stale after save_context return | Re-read core ID from hardware (`MPIDR_EL1`) after save_context |

---

## 7. Contributing a New Subsystem

This section describes the process for adding a new kernel subsystem, using the IPC subsystem as a concrete example of the pattern.

### 7.1 Directory and Module Setup

Create a directory under `kernel/src/` for the subsystem:

```text
kernel/src/mysubsys/
  mod.rs          # Public API, core data structures, module docs
```

Add it to `kernel/src/main.rs`:

```rust
mod mysubsys;
```

If the subsystem has types shared with the UEFI stub or used in host tests, add them to the `shared` crate:

```rust
// shared/src/mysubsys.rs
#![cfg_attr(not(test), no_std)]

pub const MAX_WIDGETS: usize = 64;

#[repr(C)]
pub struct WidgetId(pub u32);
```

And re-export from `shared/src/lib.rs`:

```rust
pub mod mysubsys;
```

### 7.2 Static Global State

Kernel subsystems typically use global statics protected by `spin::Mutex`:

```rust
// kernel/src/mysubsys/mod.rs
use spin::Mutex;

struct WidgetTable {
    widgets: [Option<Widget>; MAX_WIDGETS],
}

static WIDGET_TABLE: Mutex<WidgetTable> = Mutex::new(WidgetTable {
    widgets: [const { None }; MAX_WIDGETS],
});
```

**Rules for global statics:**

1. Use `spin::Mutex` for mutable shared state (safe only after Phase 2 M8 WB memory upgrade).
2. Use `AtomicBool`/`AtomicU64` for simple flags and counters.
3. Use `UnsafeCell` + `unsafe impl Sync` only for write-once-during-boot statics.
4. Document the lock ordering if your subsystem acquires multiple locks. See [deadlock-prevention.md](../kernel/deadlock-prevention.md).

### 7.3 Initialization Function

Follow the boot sequence pattern -- provide an `init()` function called from `kernel_main`:

```rust
pub fn init() {
    // Initialize subsystem state
    crate::kinfo!(Boot, "Widget subsystem initialized ({} max)", MAX_WIDGETS);
}
```

Add the call at the appropriate point in `kernel_main`, respecting ordering dependencies.

### 7.4 Adding Host Tests

Extract testable logic into the `shared` crate and add tests there:

```rust
// shared/src/mysubsys.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widget_id_zero_is_valid() {
        let id = WidgetId(0);
        assert_eq!(id.0, 0);
    }
}
```

Run with `just test` to verify.

### 7.5 Adding Observability

Use the existing logging and metrics infrastructure:

```rust
use crate::{kinfo, kwarn, kerror};
use crate::observability::metrics::METRICS;

pub fn create_widget() -> Result<WidgetId, i64> {
    let mut table = WIDGET_TABLE.lock();
    // ... find free slot ...
    kinfo!(Boot, "Widget {} created", id.0);
    METRICS.ipc_messages_sent.increment();  // Reuse existing metrics or add new ones
    Ok(id)
}
```

If you need a new subsystem tag for logging, add a variant to `Subsystem` in `shared/src/observability.rs`.

### 7.6 Checklist

Before submitting a new subsystem:

- [ ] Module doc comment (`//!`) at the top of every file
- [ ] Architecture doc reference in the module doc (e.g., "Per mysubsys.md SS2")
- [ ] All `unsafe` blocks have three-line SAFETY comments
- [ ] `#[repr(C)]` on any struct shared with assembly
- [ ] Compile-time size assertions for critical structs
- [ ] Lock ordering documented if multiple locks are acquired
- [ ] Unit tests in `shared/` for extractable logic
- [ ] `just check` passes (zero warnings)
- [ ] `just test` passes (all tests green)

---

## 8. Cross-Reference Index

This guide covers Rust patterns and development workflow. For deeper topics on specific subsystems, consult these architecture documents:

| Topic | Document | Relevant Sections |
|---|---|---|
| **Platform porting** | [hal.md](../kernel/hal.md) | SS7 (step-by-step porting guide) |
| **Lock ordering rules** | [deadlock-prevention.md](../kernel/deadlock-prevention.md) | SS3 (ascending CPU ID), SS12 (developer rules) |
| **Kernel allocation API** | [memory.md](../kernel/memory.md) | SS4 (kalloc/kfree, slab API) |
| **Page table format** | [memory.md](../kernel/memory.md) | SS3 (4-level tables, PTE bits) |
| **Physical memory management** | [memory.md](../kernel/memory.md) | SS2.2 (buddy allocator, pools) |
| **IPC protocol** | [ipc.md](../kernel/ipc.md) | SS3-4 (channel API, synchronous call/reply) |
| **Scheduler classes** | [scheduler.md](../kernel/scheduler.md) | SS3.1 (RT/Interactive/Normal/Idle) |
| **Security model** | [security.md](../security/security.md) | All (eight-layer model) |
| **Capability system** | [ipc.md](../kernel/ipc.md) | SS8 (per-process tables, enforcement) |
| **Observability** | [observability.md](../kernel/observability.md) | SS2-4 (logging, metrics, tracing) |
| **Boot sequence** | [boot.md](../kernel/boot.md) | SS3.3 (step-by-step boot) |
| **Boot lifecycle** | [boot-lifecycle.md](../kernel/boot-lifecycle.md) | All (18-phase boot progression) |
| **Storage architecture** | [spaces.md](../storage/spaces.md) | SS1-2 (core insight, architecture) |
| **Block Engine** | [spaces-block-engine.md](../storage/spaces-block-engine.md) | SS4.1-4.10 (on-disk layout, WAL, LSM) |
| **Flow (unified clipboard)** | [flow.md](../storage/flow.md) | SS1-2 (overview, architecture) |
| **System architecture** | [architecture.md](./architecture.md) | All (system overview) |
| **Development plan** | [development-plan.md](./development-plan.md) | SS8 (phase table) |
| **PR process** | [CONTRIBUTING.md](../../CONTRIBUTING.md) | All (branch naming, commit style, review) |
| **Code conventions** | [CLAUDE.md](../../CLAUDE.md) | Code Conventions, Unsafe Documentation Standard |

---

## 9. Planned Expansions

This guide is designed to grow alongside the kernel. The following areas are intentionally left as stubs or brief overviews, with expansion planned as the corresponding phases land.

### Section 5: Build, Test, and Verification

| Topic | Expand When | Content to Add |
|---|---|---|
| `just` recipe deep-dive | Phase 4+ | Annotated walkthroughs of each recipe, common flag combinations, customization |
| CI pipeline explanation | Phase 4+ | GitHub Actions workflow breakdown, what each job catches, how to read CI failures |
| Integration test patterns | Phase 4+ (user-space) | How to write kernel integration tests, QEMU-based acceptance test harness |
| `shared` crate test patterns | **Done (SS5.5)** | Extraction workflow, dependency injection, test helper patterns, no_std constraints, test distribution |
| Storage debugging patterns | Phase 5+ | Block Engine diagnosis, WAL recovery analysis, CRC verification procedures |
| Cross-compilation troubleshooting | Phase 7+ (multi-platform) | Common linker errors, target-specific build issues, conditional compilation patterns |

### Section 6: Debugging Techniques

| Topic | Expand When | Content to Add |
|---|---|---|
| QEMU GDB deep-dive | Ongoing | Breakpoints in exception handlers, `x/8gx` for page table inspection, system register reads |
| Memory debugging | Phase 4+ | Buddy poison patterns (`0xDEAD_DEAD`), slab red zone detection, diagnosing use-after-free |
| Common QEMU failure modes | **Done (SS6.6)** | Silent hang, garbage UART, immediate reboot, IPC/scheduler failures |
| Per-phase debugging recipes | Each phase | E.g., "Phase 4 storage debugging", "Phase 5 compositor debugging" |
| Multi-core debugging | Phase 4+ | IPC deadlock detection, priority inversion diagnosis, per-core log correlation |

### Section 7: Contributing a New Subsystem

| Topic | Expand When | Content to Add |
|---|---|---|
| Real subsystem walkthrough | Phase 4+ | Step-by-step example of how a subsystem (e.g., networking) was added end to end |
| Architecture doc template | Phase 4+ | What to write before coding: scope, API surface, resource budget, security model |
| Extending `Subsystem` enum | Ongoing | How to add a new observability subsystem tag and wire it through logging/metrics/tracing |

### New Sections (future)

| Section | Expand When | Content |
|---|---|---|
| Performance Profiling | Phase 5+ | Measurement methodology, timer-based profiling, IPC latency benchmarks |
| Security Review Checklist | Phase 4+ | Expanding beyond W^X: capability audit patterns, trust level verification, syscall boundary checks |
| Multi-Platform Porting | Phase 7+ (RISC-V) | Platform abstraction patterns, arch-conditional compilation, HAL trait implementation guide |
| Application Developer Guide | Phase 12+ | `std`-based development, runtime APIs, agent SDK, IPC from user-space |

---

## 10. Appendix: AIOS Glossary

Terms that may be unfamiliar or have AIOS-specific meanings.

| Term | Definition |
|---|---|
| **ASID** | Address Space Identifier. 16-bit tag in TTBR0 that allows the TLB to cache translations for multiple address spaces simultaneously without flushing on every context switch. Managed by `mm/asid.rs`. |
| **BootInfo** | Structure passed from the UEFI stub to the kernel at boot. Contains the memory map, DTB physical address, framebuffer info, RNG seed, and a magic value (`0x41494F53_424F4F54` = "AIOSBOOT"). Defined in `shared/src/boot.rs`. |
| **Block Engine** | Content-addressed storage layer providing crash-safe writes via WAL + CRC-32C integrity + SHA-256 hashing. Manages superblock, data region, and MemTable index. Implemented in `storage/block_engine.rs`. |
| **Buddy allocator** | Physical page allocator that manages free pages in power-of-two blocks (orders 0--10, covering 4 KiB to 4 MiB). Uses bitmap coalescing to merge adjacent free blocks. Implemented in `mm/buddy.rs`. |
| **ContentHash** | SHA-256 hash of a data block, used as the primary identifier for content-addressed storage. Wrapper type `[u8; 32]` with custom `Ord` for sorted MemTable lookups. Defined in `shared/src/storage.rs`. |
| **CRC-32C** | Castagnoli variant of CRC-32 using polynomial 0x1EDC6F41. Used for both superblock and data block integrity verification. Computed via a 256-entry const-initialized lookup table in `storage/block_engine.rs`. |
| **Direct map** | A 1:1 virtual-to-physical mapping of all RAM at `DIRECT_MAP_BASE` (`0xFFFF_0001_0000_0000`). Allows the kernel to access any physical address via a fixed offset calculation. Built in `mm/kmap.rs`. |
| **Direct switch** | IPC fast path that bypasses the scheduler. When thread A calls thread B and B is already waiting in `IpcRecv`, the kernel context-switches directly from A to B without touching the run queue. Approximately 0.2 microseconds. Implemented in `ipc/direct.rs`. |
| **DMA pool** | Physical memory pool (64 MB on QEMU 2G) reserved for device-facing buffers. Required because DMA-capable devices need cache-coherent memory. VirtIO virtqueues and request buffers are allocated from this pool. |
| **DSB** | Data Synchronization Barrier. ARM instruction that ensures all prior memory accesses are complete before subsequent instructions execute. Variants: `dsb sy` (full system), `dsb ish` (inner shareable), `dsb nsh` (non-shareable / local only). |
| **EL** | Exception Level. ARM privilege levels: EL0 (user), EL1 (kernel), EL2 (hypervisor), EL3 (secure monitor). AIOS kernel runs at EL1. QEMU boots directly to EL1. |
| **ESR** | Exception Syndrome Register (`ESR_EL1`). Contains the exception class (EC, bits [31:26]) and instruction-specific syndrome (ISS, bits [24:0]). Used by the trap handler to determine the cause of a synchronous exception. |
| **FAR** | Faulting Address Register (`FAR_EL1`). Contains the virtual address that caused a data or instruction abort. |
| **GICv3** | Generic Interrupt Controller version 3. ARM's standard interrupt controller. Components: Distributor (SPI routing), Redistributor (per-core PPI/SGI), CPU Interface (acknowledge/complete). |
| **ISB** | Instruction Synchronization Barrier. ARM instruction that flushes the processor pipeline, ensuring all subsequent instructions are fetched and decoded using the current system register state. |
| **Magazine** | Per-CPU object cache in the slab allocator. Provides a two-chance fast path: check current magazine, then previous magazine, before falling back to the slab. 32 objects per magazine round. |
| **MemTable** | In-memory sorted index mapping `ContentHash` → `BlockLocation` with refcount for deduplication. Uses `Vec::binary_search_by()` for O(log n) lookup. Capacity 65536 entries. Implemented in `storage/lsm.rs`. |
| **MPIDR** | Multiprocessor Affinity Register (`MPIDR_EL1`). Each core has a unique value. AIOS uses bits [7:0] as the core ID (valid for up to 256 cores on QEMU virt). |
| **NC memory** | Non-Cacheable Normal memory. Phase 1 identity map uses NC attributes (edk2 MAIR Attr1=0x44). Atomic RMW operations hang on NC memory because the exclusive monitor requires cacheability. See SS4.1. |
| **Pool** | A partition of the physical memory managed by separate buddy allocator instances. AIOS defines four pools: `Kernel` (128 MB), `User` (remainder), `Model` (0 MB, reserved for future AI model memory), `DMA` (64 MB). |
| **PSCI** | Power State Coordination Interface. ARM firmware standard for CPU power management. AIOS uses `CPU_ON` (function ID `0xC400_0003`) to bring secondary cores online. Invoked via HVC on QEMU, SMC on real hardware. |
| **Slab allocator** | Kernel object allocator with 5 size classes (64, 128, 256, 512, 4096 bytes). Backed by the buddy allocator's kernel pool. Features magazine caching and red zone corruption detection. Implemented in `mm/slab.rs`. |
| **SPSC** | Single-Producer Single-Consumer. Lock-free ring buffer pattern used for per-core logging. Only the owning core writes (producer); only the drain function reads (consumer). See SS2.3. |
| **StorageError** | Enum covering all storage failure modes (11 variants): `BlockNotFound`, `ChecksumFailed`, `DecryptionFailed`, `IoError`, `QuotaExceeded`, `DeviceFull`, `WalFull`, `SuperblockCorrupt`, `DeviceNotFound`, `VirtioError`, `MemTableFull`. All variants are `Copy` (no `String` fields) for `no_std` compatibility. |
| **Superblock** | On-disk metadata block (4096 bytes, sectors 0--7) containing storage layout parameters: WAL region location, data region start, append pointer, and CRC-32C checksum. Magic value `0x41494F53_50414345` ("AIOSPACE"). |
| **TrapFrame** | 272-byte structure saved on exception entry from EL0. Contains all 31 general-purpose registers + SP_EL0 + ELR_EL1 + SPSR_EL1. `#[repr(C)]` layout matches assembly save/restore offsets in `exceptions.rs`. |
| **ThreadContext** | 296-byte structure saved during voluntary context switch between kernel threads. Contains 31 GP regs + SP + PC + PSTATE + TTBR0 + timer state. Used by `save_context`/`restore_context` in `context_switch.S`. |
| **TLBI** | TLB Invalidate instruction family. Variants include: `tlbi vmalle1` (local, all entries), `tlbi vmalle1is` (inner-shareable broadcast), `tlbi vae1is` (single virtual address, broadcast), `tlbi aside1is` (single ASID, broadcast). |
| **Trust level** | Security classification for processes: 0=kernel, 1=system service, 2=privileged agent, 3=normal agent, 4=sandboxed. Determines default resource limits and the scope of capabilities that can be granted. |
| **TTBR** | Translation Table Base Register. `TTBR0_EL1` holds the user-space page table root (bits [47:0] = physical address, bits [63:48] = ASID). `TTBR1_EL1` holds the kernel page table root. |
| **VirtIO** | Virtual I/O specification for device emulation. AIOS uses VirtIO MMIO legacy (v1) transport with polled I/O for the block device driver. The driver probes MMIO slots at `0x0A00_0000` with 512-byte stride. |
| **W^X** | Write XOR Execute policy. Every memory page is either writable or executable, never both. Enforced at the page table level via PXN (Privileged Execute-Never) and UXN (Unprivileged Execute-Never) bits. |
| **WAL** | Write-Ahead Log. Circular buffer of 64-byte entries on disk (sectors 8--131079, 64 MiB) providing crash safety for the Block Engine. Entries are appended uncommitted, then committed after data is written. Recovery replays committed entries and salvages uncommitted entries with valid CRC. Implemented in `storage/wal.rs`. |
| **WFE** | Wait For Event instruction. Puts the core in a low-power state until an event (from `sev` or an interrupt) occurs. Used for secondary core parking and the halt loop. Preferred over `wfi` because `sev` wakes all `wfe`-parked cores simultaneously. |
