---
author: claude
date: 2026-03-26
tags: [drivers, input, virtio]
status: final
---

# VirtIO-Input Eventq Is Device-to-Driver

The VirtIO-input eventq works in the reverse direction from blk/gpu drivers. The driver pre-fills the available ring with empty 8-byte VirtioInputEvent buffers (VIRTQ_DESC_F_WRITE flag), and the device writes events into them. This is the opposite of blk/gpu where the driver submits commands and the device writes responses.

Key implications:
- Each descriptor is standalone (no chaining) — unlike blk/gpu which chain command + response descriptors
- Pre-fill ALL queue_size slots at init, not just one at a time
- Buffer recycling is critical: after reading from the used ring, immediately re-add the descriptor to the available ring, or the device runs out of buffers and stops sending events (silently)
- Notify after recycling is necessary to tell the device new buffers are available

The config space also uses a unique select/subsel protocol (VirtIO spec §5.8.2) where you write select+subsel as a packed u32 to offset 0x100, then read size and data from offset 0x108. This is different from blk/gpu which have simple register reads at fixed offsets.
