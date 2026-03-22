---
author: Justin Lee
date: 2026-03-16
updated: 2026-03-22
tags: [experience, compositor, intelligence]
status: final
---

# ADR: Three Coexisting Interaction Layers

## Context

How should AIOS present its user experience? Traditional desktop? AI-composed surfaces? The experience needs to work from Phase 6 (basic compositor) through Phase 30+ (full AIRS intelligence), supporting both Linux apps and native AIOS agents.

## Options Considered

### Option A: Traditional desktop only

- Pros: Familiar, works for all apps, no AI dependency
- Cons: Doesn't leverage what makes AIOS different, indistinguishable from existing OSes

### Option B: Intelligence Surface only

- Pros: Most differentiated, showcases AIRS capabilities
- Cons: Requires full AIRS stack to function, alienates users who want traditional workflows, Linux apps can't participate deeply

### Option C: Three coexisting layers with gradual transition

- Pros: Works at every phase of development, users choose their comfort level, Linux apps benefit from Layer 2 automatically, no forced migration
- Cons: Must design and maintain three interaction models simultaneously

## Decision

Three coexisting interaction layers (Option C):

**Layer 1 — Classic Desktop (Phase 6-7):** Traditional windows, taskbar, manual tiling. All software works (Linux apps, web apps, native agents). No AIRS required. Always available as fallback.

**Layer 2 — Smart Desktop (Phase 9-15):** Traditional windows with AIOS intelligence: information gravity (related windows cluster), context-aware layout, Flow between windows, attention-based dimming. Both native agents AND Linux apps benefit (compositor reads semantic hints).

**Layer 3 — Intelligence Surface (Phase 29-30+):** No fixed windows. AIRS composes information based on context and intent: generative UI, temporal screen, information gravity, context-morphing layout. Native AIOS agents only (deepest integration required).

Layers coexist on the same screen. Users naturally drift 1 -> 2 -> 3 as native apps improve. No forced migration.

**Intelligence Surface details (resolved 2026-03-22):**
- Mixed contexts handled via weighted blending (e.g., coding:0.6, reference:0.25, chat:0.15), not single-context selection
- MVP demo: "morning briefing" — single generated screen on login
- Testing: mock context replay + real rendering; synthetic scenario harness
- Compositor semantic hints: 3 levels (coarse=always on, medium=opt-in, fine=explicit capability grant)

## Consequences

- Must design compositor to support all three layers simultaneously
- Layer 1 is the minimum viable compositor (Phase 6-7)
- Layer 2 requires Context Engine and basic AIRS (Phase 9-15)
- Layer 3 requires full AIRS stack (Phase 29-30+)
- Linux apps participate in Layer 1 fully, Layer 2 partially (via compositor semantic hints)
- Interface Kit must support both traditional widget layout (Layer 1/2) and generative composition (Layer 3)
