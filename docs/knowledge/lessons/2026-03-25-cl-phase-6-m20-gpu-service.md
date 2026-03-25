---
author: claude
date: 2026-03-25
tags: [gpu, ipc, capability, display, boot]
status: final
---

# GPU Service — Lessons Learned (Phase 6 M20)

## Direct driver calls before scheduler starts

The GPU Service thread is spawned during kernel_main, but the scheduler hasn't started yet — meaning the IPC loop won't execute until `sched::start()` runs. For the GOP→VirtIO-GPU transition (rendering AIOS blue to the VirtIO-GPU display), **direct driver calls** must be used instead of IPC. The GPU Service IPC path only becomes active once the scheduler delivers messages. This matches the echo service pattern where `ipc_recv()` blocks until the scheduler is running.

## NC memory and spin::Mutex in service threads

The GPU Service calls `VIRTIO_GPU.lock()` which is a `spin::Mutex`. This is safe because by the time GPU Service code runs (after scheduler start), the TTBR0 RAM blocks have been upgraded to Write-Back Cacheable (Attr3) in M8. The Phase 1 NC memory limitation (spinlocks hang on Non-Cacheable Normal memory) does not apply to any post-M8 code paths. But if ever moving GPU init earlier in boot, this hazard returns.

## ThreadId labels vs runtime indices

`ThreadId(0x900)` passed to `Thread::new_kernel()` is a **debug label**, not the runtime identity. The actual ThreadId used by the scheduler and IPC is the THREAD_TABLE index returned by `allocate_thread()`. The echo service and GPU Service both use high debug labels (0x700, 0x900) but run at whatever index the allocator provides. This distinction matters for `channel_create_unchecked(owner_tid)` — the `owner_tid` should be the debug label for identification, but IPC access control uses capabilities (ChannelAccess), not ownership.

## Double-buffer DMA budget

Two 1280x800x4 framebuffers need 2×4MB = 8MB from Pool::Dma (64MB). Plus the M19 test frame temporarily (~4MB before release). Total peak: ~12MB of 64MB (18.75%). Releasing the test frame before allocating double buffers would reduce peak to ~8MB, but the simpler "allocate then release" approach works fine within budget.

## Boot phase renumbering cascade

Inserting `GpuReady = 17` and bumping `Complete` to 18 required changes in 4 locations: the enum definition (shared/src/boot.rs), the transmute range guard (kernel/src/boot_phase.rs), EARLY_BOOT_PHASE_COUNT, and ~6 host-side tests that enumerate all variants or assert specific discriminant values. The contiguous-values test is the most fragile — it lists every variant in an array and checks sequential numbering.

## Audit loop caught real bugs

The audit loop found that `gpu_display_transition()` released the M19 test frame but didn't fill the new double buffers with AIOS blue, resulting in a black screen. The fix was adding `fill_rect` + `gpu_present_frame` inside `init_double_buffering()` after buffer allocation. This validates the audit-before-PR workflow — the black screen would have been the first thing a reviewer noticed.
