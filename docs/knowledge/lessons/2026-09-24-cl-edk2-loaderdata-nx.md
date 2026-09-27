---
author: claude
date: 2026-09-24
tags: [boot, mmu, platform]
status: final
---

# Lesson: strict-NX edk2 maps EfiLoaderData execute-never, so the kernel text must be EfiLoaderCode

## What happened

On the Ubuntu 26.04 CI runners (QEMU 10.2.1, `qemu-efi-aarch64` 2025.11-3ubuntu7.2), every boot stopped right after the stub's last line:

```text
AIOS UEFI stub: ExitBootServices OK, jumping to kernel at 0x0000000040080000
Synchronous Exception at 0x0000000040080000
```

Local boots with Homebrew QEMU 11.1.1 and its bundled `edk2-aarch64-code.fd` worked. The same Ubuntu `QEMU_EFI.fd` also fails on the local QEMU 11.1.1, so the firmware causes it, not the QEMU version. `-d int` shows `ESR 0x21/0x8600000e` with FAR and ELR both at `0x40080000`: an instruction abort from EL1, a level-2 permission fault on the kernel's first instruction.

## Why it happened

The stub allocated the whole kernel image as `EfiLoaderData`. DxeCore applies `PcdDxeNxMemoryProtectionPolicy` when it allocates pages. It writes the permissions into the live TTBR0 tables and does not undo them at ExitBootServices. boot.S executes `_start` through that identity map until it branches to TTBR1.

- Upstream ArmVirt has set the policy to `0xC000000000007FD5` since edk2-stable202211 (commit 2997ae387397). Bit 2 of that value is EfiLoaderData, so LoaderData is mapped execute-never. Bit 1 (EfiLoaderCode) is clear under every ArmVirt policy value.
- Debian's edk2 package carried a patch reverting that commit from 2022.11-2 until 2025.02-5. Ubuntu 24.04's image (2024.02) still has it, uses `0x...7FD1`, and boots AIOS. Ubuntu 26.04's image (2025.11) does not, so it uses the upstream default.
- QEMU builds its bundled firmware with `0x...7FD1` (`nx.broken.shim.grub` in `roms/edk2-build.config`). Homebrew ships that build, so local boots never reach the strict path.

## What we learned

- The UEFI memory type of a loaded image sets its execute permission in the firmware map that the kernel inherits. Code the kernel runs before it switches page tables has to be `EfiLoaderCode`. Linux's arm64 EFI stub has done the same since v6.2 (commit 9cf42bca30e9).
- A local green boot says nothing about strict-NX firmware. The permissive policy in QEMU's and Homebrew's builds hides this whole class of bug.

## How to avoid next time

- `uefi-stub/src/elf.rs` gives each PT_LOAD segment its own allocation. PF_X segments are `LOADER_CODE` and all others `LOADER_DATA`. Segments must be page-aligned, and the entry point must be in a PF_X segment. Keep linker.ld's `ALIGN(4096)` boundaries between text, rodata and data.
- To test against strict NX locally, extract `usr/share/qemu-efi-aarch64/QEMU_EFI.fd` from `qemu-efi-aarch64_2025.11-3ubuntu7.2_all.deb` on archive.ubuntu.com and run `AIOS_EDK2_FW=<path> just run`.
- Known follow-ups:
  - The stub does no I/D cache maintenance after copying the kernel text. TCG does not model caches, but KVM and real Cortex-A cores need it.
  - linker.ld places the 128 KiB boot stack outside every PT_LOAD segment, so the firmware never reserves it.
