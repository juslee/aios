---
author: claude
date: 2026-03-25
tags: [gpu, drivers, virtio, dma]
status: final
---

# VirtIO-GPU 2D Driver — Lessons Learned (Phase 6 M19)

## QEMU virtio-gpu-device uses legacy v1 by default

The risk that QEMU's `virtio-gpu-device` might default to VirtIO modern (v2) transport was unfounded. On macOS QEMU 10.x, `-device virtio-gpu-device` uses legacy MMIO v1 (version=1), same as `virtio-blk-device`. The version-mismatch warning log was added as a diagnostic but never triggered.

## VirtIO-GPU appears at MMIO slot 30

On QEMU virt with `-device virtio-blk-pci,drive=disk0 -device virtio-blk-device,drive=data0 -device virtio-gpu-device`, the GPU is at slot 30 (phys 0x0A003C00). DTB-based probe misses it (DTB only lists the first few slots); brute-force MMIO scan finds it. This confirms the two-strategy probe approach is essential.

## 1280x800 is the default QEMU VirtIO-GPU resolution

GET_DISPLAY_INFO returns 1280x800 on QEMU (not 1024x768 as I assumed). This is exactly 1000 pages (4,096,000 bytes), fitting within the order-10 (4MB = 4,194,304 bytes) maximum. Any resolution producing >4MB would need the MAX_FRAMEBUFFER_BYTES clamp, but the common QEMU defaults happen to fit.

## VirtIO common extraction was worthwhile

Extracting `mmio_read32`/`mmio_write32`, virtqueue layout helpers, and constants to `virtio_common.rs` before writing the GPU driver prevented ~70 lines of duplication. The `order_for_pages` function was already in `shared::memory` — the block driver had a local duplicate that was removed.

## Command submission pattern: cmd+response in single DMA page

Using a single 4K DMA page split at offset 2048 (cmd at 0, response at 2048) works cleanly for all VirtIO-GPU commands. The largest response (VirtioGpuRespDisplayInfo) is 408 bytes, well within the 2K half. The 3-descriptor chain variant (`submit_command_with_extra`) handles RESOURCE_ATTACH_BACKING by placing the extra data (mem_entry array) immediately after the command header in the same page.
