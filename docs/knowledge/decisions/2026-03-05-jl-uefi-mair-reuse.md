---
author: Justin Lee
date: 2026-03-05
tags: [kernel, boot, mmu]
status: final
---

# ADR: Reuse edk2 MAIR/TCR instead of custom configuration

## Context

After UEFI ExitBootServices, the MMU is still ON with edk2's MAIR and TCR settings.
We need to set up kernel page tables. Should we reconfigure MAIR/TCR to our own
values, or reuse what edk2 left?

## Options Considered

### Option A: Custom MAIR/TCR

- Pros: Full control over memory attributes, cleaner abstraction
- Cons: Requires disabling MMU to change (risky), or modifying while MMU on
  (CONSTRAINED UNPREDICTABLE per ARM ARM — may corrupt TLB state)

### Option B: Reuse edk2 MAIR/TCR

- Pros: No need to touch MAIR/TCR while MMU is on, safe TTBR0-only swap,
  edk2 attributes are well-tested and correct
- Cons: Locked into edk2's attribute indices (Attr0=Device, Attr1=NC, Attr2=WT, Attr3=WB)

## Decision

Reuse edk2 MAIR/TCR (Option B). The risk of CONSTRAINED UNPREDICTABLE behavior
from modifying MAIR/TCR while MMU is on far outweighs the flexibility of custom
attributes.

edk2 MAIR = 0xffbb4400:
- Attr0 = 0x00 (Device-nGnRnE) — for MMIO
- Attr1 = 0x44 (Normal Non-Cacheable) — Phase 1 identity map
- Attr2 = 0xBB (Normal Write-Through) — not currently used
- Attr3 = 0xFF (Normal Write-Back) — Phase 2+ RAM upgrade

TCR T0SZ = 20 (44-bit VA space), reused as-is.

## Consequences

- Page table entries must use edk2's MAIR indices (not custom ones)
- Phase 1 identity map uses Attr1 (NC) — spinlocks don't work (see lesson on NC memory)
- Phase 2 upgrades to Attr3 (WB) — enables full atomic RMW and spinlocks
- This decision is permanent for the kernel's lifetime on edk2-booted systems
