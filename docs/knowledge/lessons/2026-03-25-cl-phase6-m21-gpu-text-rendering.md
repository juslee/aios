---
author: claude
date: 2026-03-25
tags: [gpu, drivers, boot, platform]
status: final
---

# GPU Text Rendering Lessons (Phase 6 M21)

## Git Worktree Build Isolation

When working in a git worktree, `cargo build` from the wrong directory builds the **main repo** binary, not the worktree's. The `just run-gpu` recipe uses the worktree's `target/` directory, so if you build from the main repo, QEMU runs the stale binary. Always verify the compilation path includes the worktree path (e.g., `.claude/worktrees/phase-6-m21/kernel`).

**Detection**: If `cargo build` says "Finished in 0.03s" without "Compiling", no recompilation happened — check your working directory.

## Font Size vs Display Resolution

At 1280x800 (VirtIO-GPU default on QEMU), an 8x16 bitmap font produces 160x50 characters — legible mathematically but appears as noise in the QEMU window due to macOS display scaling. The 16x32 font (80x25 chars, classic VGA text proportions) is clearly readable. Choose font size based on the **display window size**, not just the framebuffer resolution.

## Boot Log Ring Buffer Sizing

With `MAX_LOG_LINES=48`, the ring buffer wraps during boot and only preserves the last 48 entries (scheduler/IPC test noise at 6.0s+). Increasing to 256 captures the full boot sequence (~165 entries). The first boot messages (memory init, UART, GIC) are the most diagnostically valuable — show from the beginning, not the tail.

## spleen-font PSF2 API

The spleen-font 0.2 crate API: `PSF2Font::new(FONT_16X32)` → `font.glyph_for_utf8(bytes)` → `Glyph` iterator (rows) → `GlyphRow` iterator (bool pixels). ASCII fast path: single-byte ASCII maps directly to glyph index. Unicode em-dash (—) renders as `?` fallback since the font lacks that glyph — cosmetic only.
