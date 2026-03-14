# AIOS Input Future Directions

Part of: [input.md](../input.md) — Input Subsystem
**Related:** [ai.md](./ai.md) — Current AI-native features, [devices.md](./devices.md) — Current device classes, [gestures.md](./gestures.md) — Current gesture recognition

-----

## 11. Future Directions

This section describes research directions and architectural extensions beyond the current input subsystem design. These are aspirational capabilities that may inform future phases.

### 11.1 Spatial Input (VR/AR Controllers)

As AIOS targets additional form factors, spatial input becomes relevant:

#### 6DOF Controllers

VR/AR controllers report position (X, Y, Z) and orientation (pitch, yaw, roll) — 6 degrees of freedom. The input subsystem extends to:

- **New event types:** `SpatialEvent { position: Vec3, orientation: Quat, buttons, trigger, grip }`
- **Tracking coordinate systems:** Controller-relative, headset-relative, world-space
- **Prediction:** Extend touch trajectory Kalman to 6DOF prediction (critical at 90Hz+ VR refresh rates)
- **Gesture recognition:** $P+ extends to 3D strokes; TCN backbone processes 6DOF temporal sequences

#### Hand Tracking

Camera-based hand tracking produces skeleton data (21 joints per hand):

- **Joint events:** Position + rotation per joint, confidence per joint
- **Pinch detection:** Thumb-to-finger distance as analog trigger
- **Gesture recognition:** Hand pose classification via small CNN on joint positions
- **Privacy:** Hand tracking data is processed locally; only recognized gestures leave the input subsystem

#### Integration with Compositor

Spatial input requires the compositor to support 3D scene graphs:

- Hit-testing in 3D (ray casting from controller/gaze through scene)
- Virtual cursor in world space
- Spatial UI panels (2D surfaces placed in 3D space)

### 11.2 Voice-as-Input Integration

Voice input bridges the audio and input subsystems:

#### Architecture

```text
Audio Subsystem          Input Subsystem
──────────────           ───────────────
Microphone capture       Voice-to-text pipeline:
PCM stream ────────────► VAD (voice activity detect)
                         → ASR (speech recognition)
                         → TextEvent { Commit }

                         Voice commands:
                         → Intent recognition
                         → InputEvent::Command { action }
```

#### Design Decisions

- **Voice Activity Detection (VAD):** Lightweight kernel-internal model (~100KB) for always-on wake word detection. Only activates the full ASR pipeline when speech is detected.
- **ASR pipeline:** AIRS-dependent (requires inference engine with audio encoder model). Produces `TextEvent::Commit` events indistinguishable from keyboard input.
- **Voice commands:** AIRS Context Engine interprets spoken commands as semantic `InputEvent::Command` events (e.g., "undo" → Command::Undo).
- **Privacy:** Wake word detection runs locally. Full ASR can run locally (on-device model) or via network (user choice). Capability model: `AudioCapture` + `InputInject` for the voice agent.

#### Multimodal Fusion

Combining voice with gesture:

- "Move **this** [gesture: point at object] **there** [gesture: point at destination]"
- Requires temporal alignment of audio and gesture events
- AIRS fuses deictic references ("this", "there") with concurrent pointing gestures
- Produces a single semantic command: `Move(object_id, destination)`

### 11.3 Neural Input (BCI / EMG)

Brain-computer interfaces (BCI) and electromyography (EMG) represent the frontier of input:

#### BCI Interfaces

- **EEG-based BCI:** Non-invasive, low bandwidth (~5-20 bits/min for P300 speller). Suitable for binary decisions and slow text entry.
- **Invasive BCI:** High bandwidth (Stanford 2023: 62 WPM typing from neural signals via LLM decoder). Not consumer-ready but architecturally relevant.
- **Input subsystem integration:** BCI devices produce either:
  - Binary events (P300: "yes"/"no") → `SwitchInputEvent`
  - Decoded text (neural typing) → `TextEvent::Commit`
  - Cursor control (motor imagery) → `MotionEvent`

#### EMG Interfaces

- **Wristband EMG:** Detect finger/hand gestures from muscle signals (Meta/CTRL-labs research)
- **Sub-threshold gestures:** Detect intended finger movements without visible motion
- **Input mapping:** EMG → discrete gestures (pinch, spread, fist) → `GestureEvent`

#### Privacy Considerations

Neural and EMG data are among the most sensitive biometric data:

- Raw neural/EMG signals never leave the device
- Only recognized gestures or decoded commands traverse the input pipeline
- `NeuralInput` capability is a distinct, highly restricted capability class
- Audit logging records device usage, never signal content

### 11.4 Haptic Feedback Subsystem

Haptic feedback is the output counterpart to input — closing the input-output loop:

#### Haptic Device Types

| Device | Mechanism | Feedback Types |
|---|---|---|
| Gamepad rumble | Eccentric rotating mass (ERM) | Low-freq (left), high-freq (right) |
| Adaptive triggers | Linear resonant actuator (LRA) | Trigger resistance curves |
| Touchscreen haptics | Piezoelectric actuator | Texture simulation, click feedback |
| VR controllers | LRA + voice coil | Spatial haptics, collision feedback |

#### Haptic Effect Language

A declarative haptic effect language for cross-device feedback:

```rust
pub enum HapticEffect {
    /// Simple vibration: intensity (0.0–1.0), duration (ms)
    Vibrate { intensity: f32, duration_ms: u32 },
    /// Periodic vibration: waveform, frequency, duration
    Periodic { waveform: Waveform, frequency_hz: f32, duration_ms: u32 },
    /// Ramp: start intensity → end intensity over duration
    Ramp { start: f32, end: f32, duration_ms: u32 },
    /// Texture: simulate surface texture under finger movement
    Texture { roughness: f32, pattern: TexturePattern },
    /// Click: sharp tactile click at a point
    Click { intensity: f32 },
}
```

#### AIRS Integration

- **Context-aware haptics:** AIRS adjusts haptic intensity based on environment (silent mode, accessibility preferences)
- **Predictive haptics:** Pre-render haptic effects based on predicted interaction (touch approaching a button → prepare click feedback)
- **Accessibility haptics:** Substitute visual feedback with haptic feedback for visually impaired users

### 11.5 Cross-Device Input

Using one device as an input source for another:

#### Phone as Trackpad

- Phone's touchscreen provides multi-touch input to the AIOS desktop
- Connection: Bluetooth HID or AIOS Peer Protocol
- Phone runs a thin client that translates touch events to HID reports
- Desktop receives events through standard Bluetooth HID pipeline

#### Tablet as Drawing Surface

- Tablet's touchscreen + stylus provide high-precision drawing input
- Pressure, tilt, and position data transmitted via AIOS Peer Protocol
- Lower latency than Bluetooth HID (QUIC-based protocol)
- Desktop treats tablet as a remote touchscreen device

#### Universal Clipboard

- Copy on one device, paste on another
- Clipboard content transferred via Flow subsystem (see [flow.md](../../storage/flow.md))
- Input subsystem detects cross-device paste intent (Ctrl+V when clipboard source is remote)
- Capability: `CrossDeviceInput` for authorized device pairs

#### Architecture

```text
Device A (phone/tablet)          Device B (desktop)
─────────────────────            ──────────────────
Touch/stylus events              Bluetooth HID or
        │                        AIOS Peer Protocol
        ▼                               │
  Local input subsystem                  ▼
  (optional local UI)            Input subsystem
        │                        (treats as remote
        ▼                         input device)
  HID report encoding                   │
        │                               ▼
        └──────────────────────► Standard input pipeline
                                 (same as local devices)
```

### 11.6 Formal Verification of Input Pipeline

The input pipeline handles security-critical data (credentials, financial input, authentication tokens typed via keyboard). Formal verification can provide mathematical guarantees:

#### Verification Targets

| Component | Property | Technique |
|---|---|---|
| Focus routing | Events only reach focused surface | Model checking (TLA+ or Alloy) |
| Capability enforcement | No capability escalation in input chain | Type-level proofs (Rust's type system) |
| Secure input session | No observer receives events during secure session | Model checking |
| BadUSB defense | All USB devices screened before capability grant | Control flow analysis |
| Key repeat state machine | No stuck-key possible, repeat always terminable | State machine verification |
| Gesture state machine | All states reachable, no deadlock, all gestures terminable | Model checking |
| Transform pipeline ordering | Security transforms always run before user transforms | Static analysis |

#### Approach

1. **Rust type system:** Encode capability requirements as type parameters. A function that sends input events requires `InputReceive` as a type-level witness — compilation fails without it.

2. **TLA+ models:** Specify focus routing, capability flow, and gesture state machines in TLA+. Model check against the properties above. The model is separate from the implementation but validated against the same test cases.

3. **Property-based testing:** Use proptest or similar to generate random input event sequences and verify:
   - Events never leak across focus boundaries
   - Capability revocation immediately stops event delivery
   - Secure input sessions exclude all non-privileged observers
   - Transform pipeline maintains event ordering (timestamps monotonically increase)

4. **Fuzz testing:** Fuzz the HID report descriptor parser, the XKB layout engine, and the gesture recognition state machine with malformed inputs. Target: zero crashes, zero undefined behavior.

#### Research References

- seL4's verified IPC mechanism provides a model for formally verified event delivery
- CertiKOS (Yale) demonstrated verified interrupt handling — applicable to input IRQ processing
- The $P+ gesture recognizer is deterministic and formally verifiable by construction
