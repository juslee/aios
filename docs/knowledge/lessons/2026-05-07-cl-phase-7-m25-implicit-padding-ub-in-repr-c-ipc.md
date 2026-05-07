---
author: claude
date: 2026-05-07
tags: [ipc, compositor, unsafe, repr-c, padding]
status: final
---

# Implicit padding in `repr(C)` structs is UB when serialized to `&[u8]`

## What I hit

`CompositorEvent` and `CompositorRequest` were `repr(C)` Copy structs with
`u32` fields followed by `u64` fields. The compiler inserts implicit
padding to align the `u64` (4 bytes after `command: u32` before
`surface_id: u64`, again 4 bytes after `shmem_id: u32` before
`timestamp_ticks: u64`).

The IPC delivery path serializes these structs to bytes via:

```rust
let bytes: &[u8] = unsafe {
    core::slice::from_raw_parts(
        (event as *const CompositorEvent) as *const u8,
        core::mem::size_of::<CompositorEvent>(),
    )
};
ipc_send(channel, bytes);
```

Padding bytes between named fields are formally **uninitialized memory**
under Rust's freeze/init rules — even if the struct was constructed via
a `const fn zeroed()` literal, the language does not guarantee the
padding is zeroed. Reading those padding bytes is UB. In our case
`ipc_send` does `copy_from_slice` over the slice, which is a read.

It worked in practice on aarch64 + nightly because const-fn struct
literal evaluation tends to zero-init the underlying memory. But
that's a toolchain accident; future Rust versions or different
backends could change behaviour.

## Fix

Add explicit `_pad_*: [u8; N]` fields wherever the compiler would
insert implicit padding, and zero them in every constructor:

```rust
#[repr(C)]
pub struct CompositorEvent {
    pub tag: u32,
    pub _pad_tag: [u8; 4],     // explicit pad before u64-aligned field
    pub surface_id: u64,
    // ...
    pub shmem_id: u32,
    pub _pad_shmem: [u8; 4],   // explicit pad before next u64
    pub timestamp_ticks: u64,
    // ...
}

impl CompositorEvent {
    pub const fn zeroed() -> Self {
        Self {
            tag: 0,
            _pad_tag: [0; 4],   // initialized
            surface_id: 0,
            // ...
            shmem_id: 0,
            _pad_shmem: [0; 4], // initialized
            timestamp_ticks: 0,
            // ...
        }
    }
}
```

After this, every byte in the struct is named and gets a defined
initial value. `from_raw_parts` over the struct's bytes is now sound.

## Where to look

Any `repr(C)` struct that crosses an IPC, FFI, or hardware boundary
with a `slice::from_raw_parts((s as *const _) as *const u8, …)` or
equivalent. Check fields with mismatched alignment. The pattern:

| Field i | Field i+1 | Padding |
|---|---|---|
| `u8 / u16 / u32` | `u64 / u128` | yes — to natural alignment |
| `u32` | `[u8; N]` where N is small | no |
| `u8` | `u8` | no |

`#[repr(C, packed)]` removes padding but introduces alignment hazards
and is a different choice. For IPC where layout stability matters
across versions, prefer named explicit padding.

## Detection

Possible with `#[deny(unsafe_op_in_unsafe_fn)]` plus careful review.
Crates like `bytemuck` (`Zeroable + NoUninit`) provide compile-time
checks but require additional dependencies. For a `no_std` kernel
crate, the explicit-padding pattern is the lowest-friction option.

The audit caught this on Phase 7 M25 PR; it's been latent since
M24 introduced the IPC types. Going forward, every new `repr(C)`
struct that gets serialized to `&[u8]` should have its layout
inspected for implicit padding before review.
