# AIOS Terminal Testing & Performance

Part of: [terminal.md](../terminal.md) — Terminal Emulator Architecture
**Related:** [emulation.md](./emulation.md) — VT parser under test, [rendering.md](./rendering.md) — Performance targets, [sessions.md](./sessions.md) — PTY integration tests, [multiplexer.md](./multiplexer.md) — Session lifecycle tests

-----

## 13. Testing Strategy

The terminal emulator requires rigorous testing at multiple levels: the VT parser must correctly handle the full xterm escape sequence vocabulary, the rendering pipeline must meet latency targets, the session lifecycle must be robust against crashes, and the capability gate must resist escalation attempts.

### 13.1 VT Parser Conformance

The VT parser is tested against two industry-standard conformance suites:

#### 13.1.1 vttest (Thomas Dickey)

vttest is the canonical VT100/VT220/xterm conformance test suite. It exercises:

- Cursor positioning (CUP, CUU, CUD, CUF, CUB)
- Scrolling regions (DECSTBM)
- Character sets (G0/G1/G2/G3, DEC Special Graphics)
- Screen modes (132-column, reverse video, origin mode)
- Line drawing, double-width/double-height characters
- VT52 compatibility mode

The terminal runs vttest in QEMU integration tests. Each test screen is captured as a cell grid snapshot and compared against reference grids. Mismatches are reported with diff highlighting.

```text
Test matrix:
  vttest section 1 (cursor movement)     → 12 subtests
  vttest section 2 (screen features)     → 8 subtests
  vttest section 3 (character sets)      → 6 subtests
  vttest section 4 (double-size chars)   → 4 subtests
  vttest section 5 (keyboard)            → interactive (manual)
  vttest section 6 (mouse)               → 3 subtests
  vttest section 8 (VT52)                → 4 subtests
  vttest section 11 (xterm extensions)   → 14 subtests
```

#### 13.1.2 esctest (iTerm2)

esctest provides ~200 individual escape sequence test cases covering CSI, OSC, DCS, and mode interactions. Each test sends a specific sequence, reads back terminal state via device status reports (DSR), and verifies the expected effect.

Test coverage includes:

- Every CSI sequence in §3.7 (sequence reference table)
- OSC 0–112 set/query operations
- DCS requests (DECRQM, XTGETTCAP)
- Mode interactions (DECCKM + cursor keys, DECAWM + line wrapping, LNM + Enter key)
- Edge cases: empty parameters, maximum parameter values, malformed sequences

### 13.2 Property-Based Testing

The VT parser state machine is tested with property-based testing (proptest/quickcheck) to verify structural invariants that must hold for all possible input byte sequences:

| Property | Description |
|---|---|
| Ground convergence | Any byte sequence of length N eventually returns the parser to Ground state within N + max_sequence_length bytes |
| Bounded buffers | ParamBuffer never exceeds 32 parameters, IntermediateBuffer never exceeds 4 bytes, OscBuffer never exceeds 4096 bytes |
| No panics | Parser never panics on any input byte sequence (including invalid UTF-8, random binary data, null bytes) |
| Deterministic | Same input byte sequence always produces the same parser state and grid output |
| CAN/SUB recovery | After any CAN (0x18) or SUB (0x1A) byte, the parser is in Ground state |
| ESC preemption | An ESC (0x1B) byte from any state transitions to Escape state (the "anywhere" rule) |
| Grid bounds | Cursor position never exceeds grid dimensions (row < rows, col < cols) |
| Wrap invariant | Auto-wrap at right margin produces a cursor at column 0 of the next row (or scrolls if at bottom) |

```rust
/// Example proptest: parser always returns to Ground after CAN.
#[proptest]
fn can_always_resets(bytes: Vec<u8>) {
    let mut parser = VtParser::new();
    for &b in &bytes {
        parser.advance(b);
    }
    parser.advance(0x18); // CAN
    assert_eq!(parser.state(), ParserState::Ground);
}
```

### 13.3 Fuzz Testing

The VT parser is a primary fuzz target — it processes untrusted byte streams from arbitrary programs running in the shell.

#### 13.3.1 Fuzz Targets

| Target | Input | Success Criteria |
|---|---|---|
| `fuzz_vt_parser` | Random byte sequences | No panics, no OOB, no infinite loops |
| `fuzz_vt_grid` | Escape sequences → grid operations | Grid dimensions remain valid, no cursor escape |
| `fuzz_osc_handler` | OSC strings (titles, colors, clipboard) | Buffer bounds respected, no capability escalation |
| `fuzz_dcs_handler` | DCS payloads (Sixel, XTGETTCAP) | Payload processing terminates, bounded memory |
| `fuzz_utf8_parser` | Mixed valid/invalid UTF-8 | Correct replacement character insertion, no state corruption |

#### 13.3.2 Corpus and Coverage

- **Seed corpus:** Captured output from real terminal sessions (bash, vim, tmux, htop, compilation output)
- **Dictionary:** VT escape sequence prefixes (ESC, CSI, OSC, DCS, APC terminators)
- **Coverage target:** 100% of parser state transitions, >90% of handler branches
- **Fuzzer:** cargo-fuzz (libFuzzer) for continuous fuzzing; AFL++ for diversity

#### 13.3.3 Crash Analysis

Fuzzer-discovered crashes are triaged:

```text
Severity classification:
  Critical  — memory corruption, capability escalation, data exfiltration
  High      — parser hang (infinite loop), unbounded memory growth
  Medium    — incorrect rendering, mode state corruption
  Low       — cosmetic rendering artifacts
```

All Critical and High findings block release. Medium findings must be fixed within the milestone. Low findings are tracked but don't block.

### 13.4 Integration Testing (QEMU)

Full-stack integration tests run in QEMU, exercising the complete terminal pipeline:

```text
Test setup:
  1. Boot AIOS in QEMU with terminal agent
  2. Terminal agent creates a PTY session with /bin/sh
  3. Test harness sends input via Scriptable protocol (§8.9)
  4. Test harness reads terminal grid state via Scriptable protocol
  5. Compare grid state against expected output

Test scenarios:
  - Simple echo:       send "echo hello\n" → grid contains "hello"
  - Color output:      send "printf '\e[31mred\e[0m'\n" → cell attributes = fg red
  - Cursor movement:   send CSI sequences → cursor at expected position
  - Alt screen:        send DECSET 1049 → alt buffer active
  - Scrollback:        send 100 lines → first lines in scrollback, last 24 in grid
  - Window resize:     compositor resize → PTY notified → shell redraws
  - Tab completion:    shell TAB → partial completion rendered
```

### 13.5 Accessibility Testing

| Test Category | Methodology |
|---|---|
| Screen reader output | Capture accessibility announcements for known terminal operations (new line, cursor move, mode change). Compare against expected announcement text. |
| WCAG contrast | For each color in default and high-contrast profiles, verify foreground/background contrast ratio ≥ 4.5:1 (AA) or ≥ 7:1 (AAA for high-contrast). |
| Keyboard navigation | Verify all multiplexer operations (tab switch, pane focus, session attach) are reachable via keyboard without mouse. |
| Focus indicators | Verify focused pane has visible border distinction at all supported font sizes. |
| Reduced motion | Verify cursor blink, smooth scroll, and animations are disabled when reduced motion preference is active. |

### 13.6 Multiplexer Testing

Session lifecycle testing covers the full state space of the session broker:

```text
State machine tests:
  Create → Active → Detach → Detached → Reattach → Active → Destroy
  Create → Active → Shell Exit → Exited → Destroy
  Create → Active → Pane Split → Two Panes → Close Pane → Single Pane
  Create → Active → Compositor Crash → Headless → Compositor Reconnect → Active

Concurrency tests:
  - Two agents reattach to same session simultaneously → one succeeds, one gets error
  - Session detach during active I/O → no data loss, output buffered
  - Rapid create/destroy cycles → no resource leaks (channels, shared memory, notifications)

Persistence tests:
  - Kill terminal agent → restart → sessions recovered from space
  - Corrupt session state file → graceful degradation (new session, not crash)
```

### 13.7 Security Testing

| Attack Vector | Test | Expected Behavior |
|---|---|---|
| Escape sequence injection | Send OSC 0 with control characters in title | Title sanitized, no escape sequence propagation |
| Clipboard exfiltration | Program sends OSC 52 read without `ClipboardRead` capability | Request denied, audit log entry |
| Capability escalation | Program sends OSC to request `ProcessCreate` capability | Ignored — capabilities are kernel-granted, not terminal-negotiated |
| Resource exhaustion | Program sends infinite OSC string (no ST terminator) | OscBuffer capped at 4096 bytes, excess discarded |
| Terminal escape | Program sends sequences designed to confuse multiplexer hotkey detection | Multiplexer prefix only recognized from compositor input, not PTY output |
| Pastejacking | Program writes ESC[200~ in output to break bracketed paste boundary | Terminal tracks bracket nesting, rejects unmatched markers |

-----

## 14. Performance Verification

### 14.1 Latency Benchmarks

Input-to-display latency is the terminal's most user-perceptible performance metric.

#### 14.1.1 Measurement Methodology

```text
Keystroke latency measurement:
  1. Test harness sends keystroke via compositor input injection
  2. Timestamp T₁ = input event timestamp
  3. Shell echoes character → PTY → VT parser → grid → surface
  4. Timestamp T₂ = compositor frame that includes the glyph
  5. Latency = T₂ - T₁

Measurement requirements:
  - Timestamps: nanosecond resolution (CNTPCT_EL0)
  - Warm state: discard first 10 keystrokes (cache warming)
  - Sample size: 1000 keystrokes minimum
  - Report: p50, p95, p99, max
```

#### 14.1.2 Latency Targets

| Scenario | p50 Target | p99 Target | Notes |
|---|---|---|---|
| Keystroke echo (idle terminal) | <4ms | <8ms | Single compositor frame at 120fps |
| Interactive output (line-by-line) | <8ms | <16ms | One frame at 60fps |
| Bulk output (compilation) | <16ms | <33ms | Throughput-optimized, frame skipping allowed |
| Cursor blink toggle | <2ms | <4ms | Damage region is single cell |
| Window resize | <16ms | <50ms | Full grid rebuild + surface resize |

### 14.2 Throughput Benchmarks

#### 14.2.1 VT Parser Throughput

```text
Benchmark: parse N bytes of realistic terminal output (mixed escape sequences + text)

Targets:
  Sustained throughput:  ≥100 MB/s (Alacritty-class, 10ms for 1MB of output)
  Burst throughput:      ≥500 MB/s (short bursts with pre-allocated buffers)
  Pure ASCII:            ≥1 GB/s (no escape sequences, straight Print action)
  Heavy CSI:             ≥50 MB/s (dense escape sequences, e.g., colored `ls -la`)
```

#### 14.2.2 Rendering Throughput

```text
Benchmark: render N dirty rows to compositor surface

Targets by terminal size:
  80×24 (standard):     <2ms full redraw, <0.5ms single row
  120×40 (medium):      <4ms full redraw, <0.5ms single row
  240×80 (large):       <8ms full redraw, <1ms single row

Frame rate under sustained output:
  Target: ≥60fps visible frame rate with frame skipping
  Method: debounce at 8ms, skip intermediate frames during burst
```

### 14.3 Memory Profiling

| Resource | Default Budget | Maximum | Measurement |
|---|---|---|---|
| Glyph atlas (GPU) | 4 MB | 16 MB | Atlas texture size, glyph count, eviction rate |
| Scrollback (memory tier) | 2 MB (~10K lines) | 20 MB (~100K lines) | Line count, bytes used, spill rate to space tier |
| Surface buffer (BGRA8888) | 1.9 MB (800×600) | 7.7 MB (1920×1080) | Allocated size, damage percentage per frame |
| PTY shared memory | 64 KB | 256 KB | Current size, peak size, resize frequency |
| Cell grid (in-memory) | 96 KB (80×24×50B/cell) | 960 KB (240×80×50B/cell) | Grid dimensions × cell size |
| VT parser state | 4.2 KB | 4.2 KB (fixed) | ParamBuffer + IntermediateBuffer + OscBuffer + state |
| Session state (per session) | ~8 KB | ~8 KB | Channels + notification + metadata |

Profiling is continuous: the terminal agent reports memory metrics to the observability subsystem every 10 seconds. Alerts trigger if any resource exceeds 80% of its maximum budget.

### 14.4 Stress Testing

| Stress Test | Input | Pass Criteria |
|---|---|---|
| Sustained output | `yes \| head -n 1000000` | Terminal remains responsive, memory stable, no dropped frames after output completes |
| Rapid resize | Resize terminal 100 times in 10 seconds | No crashes, grid dimensions always match surface, shell receives all SIGWINCH |
| Concurrent sessions | Open 16 sessions, all running `cat /dev/urandom \| xxd` | CPU usage proportional to active sessions, no session starvation |
| Long-running session | Single session active for 24 hours with periodic I/O | No memory leaks, scrollback within budget, space storage within quota |
| Unicode storm | Cat 100 MB of mixed CJK, emoji, combining characters, RTL text | Correct glyph rendering, no fallback font exhaustion, no parser state corruption |
| Malformed input flood | 100 MB of random bytes piped to terminal | Parser stays in bounds, no crashes, terminal recovers to normal operation after input ends |
