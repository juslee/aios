# Unsafe Documentation Standard

Every `unsafe` block in `kernel/` requires a preceding comment with three parts:

```rust
// SAFETY: <invariant that makes this safe>
// <who maintains the invariant>
// <what happens if violated>
unsafe { ... }
```

Example:

```rust
// SAFETY: UART base address 0x0900_0000 is valid MMIO on QEMU virt.
// QEMU maps this region unconditionally. Writing to unmapped memory
// on a different machine would cause a synchronous abort.
unsafe { core::ptr::write_volatile(uart_base as *mut u32, byte as u32) };
```
