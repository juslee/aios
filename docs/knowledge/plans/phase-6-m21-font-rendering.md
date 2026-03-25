---
author: claude
date: 2026-03-25
tags: [gpu, display, font, rendering, phase-6]
status: final
phase: 6
milestone: M21
---

# Plan: Phase 6 M21 — Font Rendering & Text Display

## Context

M19-M20 delivered the VirtIO-GPU 2D driver and GPU Service with double-buffered display. The kernel currently shows an AIOS blue screen on `just run-gpu` but all diagnostic output goes to UART only. M21 adds bitmap font rendering so boot log text appears on the GPU framebuffer — the first visual diagnostic output beyond UART.

## Approach

**Scope**: Steps 13-15 from the phase doc. Simple bitmap font rendering using `spleen-font` crate (16x32 monospace glyphs). No TTF parsing, no glyph atlas, no complex text layout — that's Tier 2/3 for later phases.

### Key Design Decisions

1. **Direct framebuffer writes, not IPC**: Text rendering writes pixels directly to the back buffer's `fb_virt` address. The back buffer is WB Cacheable (MAIR Attr3), so regular writes work — no `write_volatile` needed for pixels. VirtIO `transfer_to_host` handles the DMA sync.

2. **Boot log capture via `BootLogBuffer`**: **CRITICAL TIMING ISSUE** — `drain_logs()` is called 11 times in `kernel_main` BEFORE the scheduler starts. By the time the GPU Service thread runs, ring buffers are empty. Solution: Add a static `BootLogBuffer` that captures formatted log lines alongside UART output during `drain_logs()`. The GPU text renderer reads from this buffer.

3. **Rendering inside GPU Service init**: Boot log rendering happens in `gpu_service_entry()` after `init_double_buffering()` and before the IPC recv loop. The service thread has direct access to the back buffer and can call `swap_buffers` internally.

4. **Module structure**: New `kernel/src/gpu/text.rs` with pure rendering functions. Called from `gpu/service.rs` during init.

5. **Raw pointer params, not `GpuBufferHandle`**: `blit_glyph`/`draw_text`/`draw_boot_log` take `fb_virt: *mut u32, stride, width, height` for decoupling. Caller extracts from handle.

### spleen-font Crate (Verified)

- **Version**: 0.2, **License**: MIT (compatible with BSD-2-Clause)
- **Feature**: `s16x32` (upgraded from 8x16 for readability at 1280x800)
- **Cargo.toml**: `spleen-font = { version = "0.2", default-features = false, features = ["s16x32"] }`
- **API**:
  ```rust
  use spleen_font::{PSF2Font, FONT_16X32};
  let font = PSF2Font::new(FONT_16X32).unwrap();
  let glyph = font.glyph_for_utf8("A".as_bytes());
  for row in glyph {
      for pixel in row {
          // pixel: bool — true = foreground, false = background
      }
  }
  ```
- **Properties**: Zero allocations, no_std, no alloc needed, ~40 KiB binary for 16x32

### Screen Layout (1280×800 at 16×32)

- Characters per line: `1280 / 16 = 80`
- Total lines: `800 / 32 = 25`
- Header: 1 line ("AIOS Boot Log") + 1 blank line = 2 lines
- Content lines: `25 - 2 = 23` visible log entries
- `BootLogBuffer` size: 256 lines × 160 chars = 40960 bytes (fits in BSS, ring buffer captures full boot)

### BootLogBuffer Design

```rust
const MAX_LOG_LINES: usize = 256;
const MAX_LINE_LEN: usize = 160;

struct BootLogBuffer {
    lines: [[u8; MAX_LINE_LEN]; MAX_LOG_LINES],
    line_lens: [u8; MAX_LOG_LINES],
    count: usize,  // total lines written (newest at count-1)
}

static BOOT_LOG: Mutex<BootLogBuffer> = Mutex::new(BootLogBuffer::new());
static BOOT_LOG_CAPTURE: AtomicBool = AtomicBool::new(true);  // starts ENABLED — captures from first drain_logs()
```

- **Capture lifecycle**:
  1. Starts `true` (enabled) — captures ALL boot log entries from the very first `drain_logs()` call
  2. Boot-time `drain_logs()` calls fill buffer (IRQs masked, no contention)
  3. GPU Service's `take_boot_log()` sets `false` BEFORE locking Mutex → disables capture
  4. After step 3, timer tick drain_logs sees false → skips buffer entirely
  5. **No-GPU path**: Capture stays `true` but nobody reads. Timer tick overhead: try_lock + format per entry (~2μs/tick) — negligible. Buffer in BSS (7.5 KiB) — negligible.

- **Deadlock prevention** (belt-and-suspenders):
  - `drain_logs()` uses `try_lock()` (not `lock()`) for BOOT_LOG — if Mutex held, skip capture
  - `take_boot_log()` sets `BOOT_LOG_CAPTURE = false` (Release) BEFORE locking Mutex
  - This prevents: GPU Service holds lock on CPU 0 → timer tick IRQ preempts → drain_logs tries lock → deadlock
  - Even if the atomic flag race occurs, `try_lock()` returns None → safe skip

- **No-GPU path**: Capture stays `true` but nobody reads. Timer tick overhead: one `try_lock` + format per entry — negligible. Buffer in BSS (~40 KiB) — negligible.
- **Overflow**: When `count >= MAX_LOG_LINES`, oldest lines are overwritten (ring buffer). GPU Service reads the last N lines that fit on screen.

### Implementation Details (Gap Closures)

1. **stride is BYTES, not pixels**: `GpuBufferHandle.stride = width * 4` for B8G8R8A8. Pixel offset into `*mut u32` buffer = `py * (stride / 4) + px`. Must divide stride by 4 for u32 pointer arithmetic.

2. **PSF2Font caching**: `PSF2Font::new(FONT_16X32)` parses the PSF2 header — lightweight but shouldn't be called per-glyph. `draw_text()` creates it once and passes `&PSF2Font` to `blit_glyph()`. `blit_glyph` signature takes `&PSF2Font` as first param.

3. **char → UTF-8 for glyph lookup**: `glyph_for_utf8` takes `&[u8]`. For ASCII: `&[ch as u8]`. For multi-byte: use `ch.encode_utf8(&mut [0u8; 4])` → pass the resulting slice. Fallback for unknown glyphs: render `'?'`.

4. **DMA memory is WB Cacheable** (MAIR Attr3, confirmed in `kmap.rs:290`): Regular writes to `fb_virt` are safe. No `write_volatile` needed. Use `core::ptr::write` or slice assignment for framebuffer pixels.

5. **Timer tick safety**: Timer ticks start AFTER `sched::start()` → `DAIFClr` IRQ unmask. All boot-time `drain_logs()` calls happen BEFORE IRQ unmask — no contention. After scheduler: (a) capture flag starts false for no-GPU path; (b) GPU Service sets false before locking; (c) drain_logs uses `try_lock()` as final safety net. Three-layer defense against IRQ-context deadlock.

6. **Full-screen fill optimization**: Use `core::slice::from_raw_parts_mut(fb, pixel_count).fill(color)` for filling background — much faster than per-pixel writes.

7. **Font load failure**: If `PSF2Font::new(FONT_16X32)` returns `Err`, skip text rendering entirely (keep AIOS blue screen). Log warning to UART.

### Files Modified

| File | Change |
|------|--------|
| `kernel/Cargo.toml` | Add `spleen-font` dependency |
| `kernel/src/gpu/mod.rs` | Add `pub mod text;` |
| `kernel/src/gpu/text.rs` | **NEW** — `blit_glyph`, `draw_text`, `draw_boot_log` |
| `kernel/src/gpu/service.rs` | Call `text::draw_boot_log()` after `init_double_buffering()`, swap buffers |
| `kernel/src/observability/mod.rs` | Add `BootLogBuffer`, modify `drain_logs()` to also write to buffer, add `boot_log_buffer()` accessor |
| `shared/src/gpu.rs` | Add text color constants (`BOOT_LOG_BG`, `BOOT_LOG_FG`, `BOOT_LOG_HEADER`) |
| `CLAUDE.md` | Workspace Layout, Key Technical Facts, deps |
| Phase doc | Fix feature name, uncheck prematurely checked boxes, check off completed steps |

## Progress

- [ ] Step 13: spleen-font integration and glyph renderer
  - [ ] 13a: Add `spleen-font = { version = "0.2", default-features = false, features = ["s16x32"] }` to `kernel/Cargo.toml`
  - [ ] 13b: Verify compiles: `just check` (confirms no_std compatibility on aarch64-unknown-none)
  - [ ] 13c: Create `kernel/src/gpu/text.rs`, add `pub mod text;` to `kernel/src/gpu/mod.rs`
  - [ ] 13d: Implement `blit_glyph()`:
    - Signature: `pub fn blit_glyph(font: &PSF2Font, fb: *mut u32, stride_px: u32, fb_w: u32, fb_h: u32, ch: char, x: i32, y: i32, fg: u32, bg: u32)`
    - `stride_px = handle.stride / 4` (convert bytes to pixels — caller computes once)
    - Convert char to UTF-8 bytes: `let mut buf = [0u8; 4]; let bytes = ch.encode_utf8(&mut buf);`
    - Call `font.glyph_for_utf8(bytes.as_bytes())` — if returns empty/no glyph, try `'?'` as fallback
    - Iterate rows (32) × pixels (16) via glyph row/pixel iterators, check each `bool`
    - For each pixel: compute `px = x + col`, `py = y + row`
    - Boundary clip: skip if `px < 0 || px >= fb_w as i32 || py < 0 || py >= fb_h as i32`
    - Write color: `fb.add((py as u32 * stride_px + px as u32) as usize).write(color)` — regular write (WB Cacheable memory)
    - SAFETY comment: caller ensures fb points to valid framebuffer of stride_px×fb_h u32 elements
  - [ ] 13e: Add text color constants to `shared/src/gpu.rs`:
    - `BOOT_LOG_BG: u32 = 0xFF1A1A2E` (dark blue-grey, B8G8R8A8)
    - `BOOT_LOG_FG: u32 = 0xFFE0E0E0` (light grey)
    - `BOOT_LOG_HEADER: u32 = 0xFF5B8CFF` (AIOS blue)
  - [ ] 13f: Wire a test character into GPU Service init (temporary) to verify glyph visible on QEMU
  - [ ] 13g: Verify: `just check` zero warnings, QEMU display shows character
  - Commit: `Phase 6 M21: Step 13 — spleen-font integration and glyph renderer`

- [ ] Step 14: Text rendering and boot log display
  - [ ] 14a: Implement `draw_text()` in `kernel/src/gpu/text.rs`:
    - Signature: `pub fn draw_text(font: &PSF2Font, fb: *mut u32, stride_px: u32, fb_w: u32, fb_h: u32, text: &str, start_x: i32, start_y: i32, fg: u32, bg: u32)`
    - `stride_px` = handle.stride / 4 (pixels, not bytes)
    - Create font ONCE in caller (`draw_boot_log`), pass `&PSF2Font` through
    - Iterate chars, call `blit_glyph` for each, advance cursor X by 16
    - `\n`: advance Y by 32, reset X to `start_x`
    - Line wrap: when `cursor_x + 16 > fb_w as i32`, advance Y by 32, reset X
    - Stop rendering if `cursor_y + 32 > fb_h as i32` (off bottom)
  - [ ] 14b: Implement `fill_rect()` helper in `kernel/src/gpu/text.rs`:
    - Fill rectangular region with solid color (for background)
    - Used to clear framebuffer before drawing text
  - [ ] 14c: Add `BootLogBuffer` to `kernel/src/observability/mod.rs`:
    - Static `BOOT_LOG: Mutex<BootLogBuffer>` with BSS-allocated buffer (~40 KiB)
    - Static `BOOT_LOG_CAPTURE: AtomicBool = true` — checked before locking Mutex
    - `BootLogBuffer::push_line(line: &[u8])` — appends formatted line; if `count >= MAX_LOG_LINES`, overwrite oldest (ring)
    - `pub fn take_boot_log(out_lines: &mut [[u8; MAX_LINE_LEN]], out_lens: &mut [u8]) -> usize`:
      - FIRST: `BOOT_LOG_CAPTURE.store(false, Release)` — disables capture BEFORE locking
      - THEN: `BOOT_LOG.lock()` — safe now; timer tick will see false flag and skip
      - Copies lines to caller's buffers, returns line count
      - Returns min(count, out capacity) of the MOST RECENT lines
    - No `enable_boot_log_capture()` needed — capture starts `true` at static init
  - [ ] 14d: Modify `drain_logs()` AND `early_boot_log()` to capture to `BootLogBuffer`:
    - **drain_logs** (processes ring buffer entries after LogRingsReady):
      - Check `BOOT_LOG_CAPTURE.load(Relaxed)` — skip buffer write entirely if false
      - If capture enabled: `try_lock()` BOOT_LOG — if None, skip buffer write (UART still gets it)
      - Format each entry ONCE into a stack `[u8; 160]` via `ArrayWriter`
      - Write formatted bytes to UART (existing path, always)
      - Push formatted bytes to `BOOT_LOG` via `push_line()`
    - **early_boot_log** (direct UART before LogRingsReady, ~8 boot phase messages):
      - Same pattern: check capture flag → try_lock → format to ArrayWriter → push_line
      - Captures the FULL boot sequence from first klog! call (ExceptionVectors through HeapReady)
      - BootLogBuffer is BSS static with spin::Mutex — safe before heap init
    - `ArrayWriter`: struct with `buf: &mut [u8]`, `pos: usize`, implements `fmt::Write`:
      - `write_str`: copies bytes, advances pos, silently truncates at buf.len() (returns `Ok(())` always — never Err)
      - This ensures formatting doesn't abort mid-line if message exceeds 160 chars
    - Same format for both paths: `[secs.micros] [core] LEVEL Subsys msg`
  - (No 14d2 needed — capture starts `true` at static init, captures from first drain_logs call)
  - [ ] 14e: Implement `draw_boot_log()` in `kernel/src/gpu/text.rs`:
    - Signature: `pub fn draw_boot_log(fb: *mut u32, stride_px: u32, fb_w: u32, fb_h: u32)`
    - Create `PSF2Font::new(FONT_16X32)` — if Err, log warning and return (keep AIOS blue)
    - Fill entire framebuffer with `BOOT_LOG_BG` via `slice::fill()`
    - Draw header "AIOS Boot Log" at (8, 0) in `BOOT_LOG_HEADER` color via `draw_text()`
    - Calculate visible lines: `max_lines = (fb_h - 32) / 16` (32px header area)
    - Stack-allocate line buffers: `let mut lines = [[0u8; 160]; 48]; let mut lens = [0u8; 48];`
    - Call `observability::take_boot_log(&mut lines, &mut lens)` — gets most recent entries, disables capture
    - Render each line via `draw_text()` in `BOOT_LOG_FG` color, starting at Y=32
    - Only render `min(count, max_lines)` lines
  - [ ] 14f: Wire into GPU Service entry (`kernel/src/gpu/service.rs`):
    - In `gpu_service_loop()`, after `init_double_buffering(&mut state)` (line ~127)
    - Guard with `if let Some(back) = state.back_buffer.as_ref()` — do NOT unwrap (OOM-safe)
    - Call `crate::gpu::text::draw_boot_log(back.fb_virt as *mut u32, back.stride / 4, back.width, back.height)`
    - Call `swap_buffers(&mut state).ok();` to present (best-effort)
    - If back_buffer is None (init_double_buffering failed): skip text rendering, keep AIOS blue
    - Remove the test character from Step 13f
  - [ ] 14g: Verify: `just check` zero warnings, `just run-gpu` displays boot log text on QEMU display, `just run` still boots normally
  - Commit: `Phase 6 M21: Step 14 — text rendering and boot log display`

- [ ] Step 15: Shared crate refactoring and docs update for M21
  - [ ] 15a: Verify text color constants are in `shared/src/gpu.rs` (done in 13e)
  - [ ] 15b: Add host-side tests for color constants in `shared/src/gpu.rs` (value assertions)
  - [ ] 15c: Update `CLAUDE.md`:
    - Workspace Layout: add `kernel/src/gpu/text.rs`
    - Key Technical Facts: spleen-font 0.2 (s16x32 feature), glyph 16×32, BootLogBuffer 256 lines × 160 chars
    - kernel/Cargo.toml deps: add spleen-font
  - [ ] 15d: Update `README.md` project structure if applicable
  - [ ] 15e: Update phase doc: check off Steps 13-15, update Status (feature name and version already reconciled)
  - [ ] 15f: Dead code cleanup: grep for `#[allow(dead_code)]`
  - [ ] 15g: Run `/audit-loop` — triple audit until 0 issues
  - [ ] 15h: Verify: `just check` + `just test` pass
  - Commit: `Phase 6 M21: Step 15 — shared crate refactoring and docs update`

## Dependencies & Risks

- **Depends on**: M20 complete (GPU Service, double buffering, VirtIO-GPU driver) ✓
- **Risk: spleen-font `PSF2Font::new` panics on aarch64**: The `unwrap()` could panic if font data is invalid. Mitigation: use `unwrap()` since FONT_16X32 is compiled-in constant data; if it fails, it's a crate bug.
- **Risk: spleen-font API differs from research**: Crate is version 0.2. Mitigation: read source code during Step 13b if API doesn't match; fall back to `font8x8` crate (MIT, simpler API, 8×8 only) as backup.
- **Risk: BootLogBuffer deadlock in IRQ context**: drain_logs() is called from timer tick on CPU 0. If GPU Service holds BOOT_LOG lock on same CPU and timer preempts → deadlock. Mitigation: (1) drain_logs uses `try_lock()` — never blocks; (2) `BOOT_LOG_CAPTURE` flag set false by GPU Service BEFORE locking. Two-layer defense.
- **Risk: Character encoding**: spleen-font handles ASCII well but log messages should be ASCII-only. Non-ASCII → render '?' via fallback.

## Phase Doc Reconciliation

1. **Feature name**: Phase doc updated to `features = ["s16x32"]` (was `font-8x16`)
2. **Version**: Phase doc updated to `version = "0.2"` (upgraded from 0.1 for 16x32 support)
3. **Step 14 checkboxes**: `draw_text` and `draw_boot_log` signatures are marked `[x]` but don't exist in code — uncheck them
4. **Function signature deviation**: Phase doc says `draw_boot_log(fb: &GpuBufferHandle)` — implementing as `draw_boot_log(fb: *mut u32, stride, width, height)` for decoupling. Document in plan.
5. **drain_logs integration**: Phase doc says "drains the last N log entries from the kernel log ring" — implementing via BootLogBuffer capture since ring buffers are empty by GPU Service start time. Functionally equivalent.

## Verification

```bash
# Build gate
just check          # zero warnings

# Test gate
just test           # all 431+ tests pass

# QEMU gate (GPU)
just run-gpu        # Boot log text visible on QEMU display window
                    # Header "AIOS Boot Log" in blue
                    # Log lines in light grey on dark background
                    # Text legible and properly positioned

# QEMU gate (no GPU)
just run            # Boots normally, GOP framebuffer fallback, no crash
```

## Issues Encountered

(to be filled during implementation)

## Decisions Made

(to be filled during implementation)

## Lessons Learned

(to be filled during implementation)
