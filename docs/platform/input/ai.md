# AIOS AI-Native Input Intelligence

Part of: [input.md](../input.md) — Input Subsystem
**Related:** [events.md](./events.md) — Event pipeline stages, [gestures.md](./gestures.md) — Gesture recognition ML, [devices.md](./devices.md) — BadUSB defense, [integration.md](./integration.md) — Capability model for AI features

-----

## 10. AI-Native Input Intelligence

The input subsystem embeds two tiers of machine intelligence: **kernel-internal models** that run at sub-millisecond latency with no external dependencies, and **AIRS-dependent services** that leverage the AI Runtime's language models and context engine for semantic understanding.

This split is architecturally load-bearing. Kernel-internal models handle security (BadUSB), accessibility (tremor, debounce), and comfort (acceleration, palm rejection) — features that must work from the moment the device is connected, even if AIRS is offline or starting up. AIRS-dependent services add intelligence (prediction, context awareness, behavioral analysis) that enhances the experience but is never required.

```text
Input Intelligence Architecture

┌────────────────────────────────────────────────────────────────┐
│                   Kernel-Internal ML (~1.8MB)                   │
│                   Always available, <1ms latency                │
│                                                                 │
│  Security          Accessibility       Comfort                  │
│  ─────────         ─────────────       ───────                  │
│  BadUSB HID clf    Tremor Kalman       Pointer accel sigmoid    │
│  Timing entropy    Tremor CNN          Key repeat HMM           │
│  Traffic CNN       Debounce HMM        Palm rejection CNN       │
│  Injection detect  Click assist LSTM   Touch trajectory Kalman  │
│  KS anomaly SVM   Fitts' Law adapt    Spatial touch Gaussian   │
│  Mouse dyn SVM     Scan frequency      $P+ geometric matcher   │
│                                        TCN gesture backbone     │
│                                        N-gram predictor         │
└────────────────────────────┬───────────────────────────────────┘
                             │ IPC (InputPrediction,
                             │      BiometricFeature,
                             │      ContextRead)
┌────────────────────────────┴───────────────────────────────────┐
│                   AIRS-Dependent Services                        │
│                   Available when AIRS is online                  │
│                                                                 │
│  Inference Engine        Context Engine       Behavioral Monitor │
│  ─────────────────       ──────────────       ────────────────── │
│  Transformer predict     Gesture interpret    Multimodal anomaly │
│  Neural reranker         Shortcut suggest     Continuous auth    │
│  Sentence completion     Workflow predict     Behavioral profile │
│  Eye gaze intent         Adaptive scanning                      │
└────────────────────────────────────────────────────────────────┘
```

-----

### 10.1 Predictive Input

#### Tier 1: N-Gram Predictor (Kernel-Internal)

A frozen trigram/4-gram language model provides basic next-word prediction with no AIRS dependency.

| Property | Value |
|---|---|
| Model type | Trigram/4-gram with Kneser-Ney smoothing |
| Size | ~500KB (compressed frequency tables) |
| Vocabulary | ~30K common words |
| Inference latency | <1ms on Cortex-A72 |
| Update strategy | Frozen at build time; no online learning |
| Privacy | No user data stored or transmitted |

The n-gram predictor provides ~3 candidate words after each keystroke. It handles common phrases and frequent word completions. For more sophisticated prediction, AIRS Tier 2 takes over.

#### Tier 2: Transformer Predictor (AIRS-Dependent)

When AIRS is online, a distilled transformer model provides multi-word and sentence-level completion with context awareness.

| Property | Value |
|---|---|
| Model type | 6-layer transformer decoder (distilled) |
| Parameters | 5–15M (INT8 quantized) |
| Size | 5–15MB |
| Inference latency | 5–10ms on Cortex-A72 (NEON SIMD via GGML Runtime) |
| Vocabulary | 50K subword tokens (BPE) |
| Context window | Last 256 tokens + application context from Context Engine |
| Privacy | Federated learning (gradient updates only, never raw text) |

The transformer predictor is invoked via IPC from the input subsystem to AIRS:

```text
Input Subsystem                          AIRS
─────────────────                        ────
                    IPC: PredictionRequest
partial_text ──────────────────────────► Inference Engine
app_context                              + Context Engine
content_type                             │
                                         ▼
                    IPC: PredictionResponse
candidates ◄────────────────────────────  [("world", 0.85),
confidence                                 ("wide", 0.12),
                                           ("war", 0.03)]
```

**Capability chain:**
- Input subsystem holds `InputData` (processes raw keystrokes)
- AIRS holds `InputPrediction` (receives partial text, returns candidates)
- AIRS does NOT hold `InputData` — it never sees raw keystroke timing or individual key events
- The input subsystem sends only the partial word/sentence for prediction

#### Autocorrect

Two-stage autocorrect pipeline:

1. **Spatial touch model (kernel-internal):** Per-key Gaussian distribution (~10KB per keyboard layout) models where the user actually touches vs the key center. Updated online with simple mean/covariance update. Generates correction candidates based on spatial probability.

2. **Neural reranker (AIRS-dependent):** Small transformer (~500K params) reranks correction candidates using surrounding sentence context. Inference <5ms. Falls back to frequency-based ranking when AIRS is offline.

Content type hints affect autocorrect behavior:

| Content Type | Autocorrect | Prediction | IME |
|---|---|---|---|
| `Text` | Enabled | Enabled | Enabled |
| `Email` | Enabled | Email-specific | Enabled |
| `Url` | Disabled | URL completion | Disabled |
| `Password` | Disabled | Disabled | Disabled |
| `Code` | Disabled | Code completion (AIRS) | Disabled |
| `Number` | Disabled | Disabled | Disabled |

-----

### 10.2 Adaptive Parameters

All adaptive parameters are kernel-internal ML — no AIRS dependency.

#### Pointer Acceleration

The pointer acceleration curve is a parameterized sigmoid function personalized per user:

```text
output_speed = max_speed × sigmoid(gain × (input_speed - offset))

Parameters: { max_speed, gain, offset }
Size: ~20 bytes per user
Update: Online Bayesian optimization every ~100 pointer events
Convergence: ~2-3 minutes of normal use
```

The system observes Fitts' Law task performance (target acquisition time and endpoint error) and iteratively adjusts the sigmoid parameters:

1. User moves pointer toward a target (click or keyboard shortcut)
2. System records: input velocity, endpoint accuracy, overshoot
3. Bayesian update: posterior on {max_speed, gain, offset} given observations
4. Apply MAP estimate to acceleration curve

Users can select acceleration profiles:

| Profile | Behavior | Use Case |
|---|---|---|
| Flat | No acceleration (1:1 mapping) | Gaming, precision work |
| Adaptive | Bayesian-tuned sigmoid (default) | General use |
| High | Amplified acceleration | Large/multi-monitor setups |
| Custom | User-specified curve | Accessibility, preferences |

#### Key Repeat Rate

Adaptive key repeat uses a 2-state Hidden Markov Model:

- **State 1:** Intentional hold (user wants repeat) — short delay, fast repeat
- **State 2:** Accidental hold (user meant single press) — long delay

The HMM learns per-user hold duration distributions (~200 bytes of state, updated every 1000 keystrokes). This eliminates unwanted repeats for slow typists while preserving rapid repeat for experienced users.

Default parameters:

| Parameter | Default | Range |
|---|---|---|
| Initial delay | 400ms | 150–1000ms |
| Repeat rate | 30 Hz | 2–50 Hz |
| HMM adaptation | Enabled | On/Off |

#### Touch Sensitivity

Capacitive touchscreens lose accuracy with moisture, gloves, or screen protectors. The input subsystem adapts:

- **Amplitude histogram:** Track touch signal amplitude distribution over a sliding window. Detect environmental changes (wet screen, gloves) and adjust sensitivity threshold.
- **Size:** ~1KB state per touchscreen
- **Update:** Every 100 touch events
- **No AIRS dependency:** Pure signal processing

#### Adaptive State Persistence

All adaptive parameters are stored in the user's input profile space:

```text
user/input/
├── acceleration.profile     # pointer acceleration sigmoid params
├── key_repeat.profile       # key repeat HMM state
├── touch_sensitivity.dat    # touchscreen amplitude histogram
├── gesture_prototypes.dat   # custom gesture embeddings
├── debounce.profile         # accessibility debounce HMM
└── tremor.profile           # tremor filter Kalman state
```

State is saved on user session close and restored on login. Driver restarts reload from this persistent state — no cold-start penalty.

-----

### 10.3 Gesture Learning

The three-layer gesture system (see [gestures.md](./gestures.md) §5.5) includes ML at two levels:

#### Layer 1: $P+ Geometric Matcher (No ML)

Deterministic template matching. Users add custom gestures with 1 example. Templates stored in `user/input/gesture_prototypes.dat`. ~2KB per gesture template.

#### Layer 2: TCN Backbone (Kernel-Internal ML)

A frozen Temporal Convolutional Network backbone extracts features from continuous touch/sensor streams:

| Property | Value |
|---|---|
| Model type | Single-stage dilated causal TCN |
| Parameters | ~200K (frozen, pre-trained on public gesture datasets) |
| Size | ~800KB (INT8 quantized) |
| Input | 500ms window at 100Hz = 50 samples per channel |
| Inference | <1ms on Cortex-A72 |
| Output | 128-dim embedding per gesture window |

Custom gesture recognition uses few-shot prototypical classification:

1. User demonstrates a custom gesture 3–5 times during enrollment
2. TCN extracts embedding for each example
3. Prototype = mean embedding (~512 bytes per gesture class)
4. At runtime: TCN extracts embedding → nearest-prototype classification

**Capability model:**
- `GestureDefine` capability required to enroll new gestures
- `GestureRecognize` capability to receive gesture events
- Third-party apps get `GestureRecognize` but not `GestureDefine` (unless explicitly granted)

#### Layer 3: AIRS Context Interpretation (AIRS-Dependent)

AIRS provides context-aware gesture interpretation:

- Same gesture ID → different action depending on active application
- Context Engine provides: active application, document type, selected tool, recent actions
- Gesture → action mapping stored per-application in the user's gesture space

Example:

| Gesture | Drawing App | Web Browser | File Manager |
|---|---|---|---|
| Circle stroke | Select region | Refresh page | Select file |
| Swipe left | Undo brush | Back | Navigate up |
| Two-finger pinch | Zoom canvas | Zoom page | Icon size |

-----

### 10.4 Anomaly Detection

Input anomaly detection operates at two levels: device-level (BadUSB defense) and behavioral-level (continuous authentication, injection detection).

#### Device-Level: BadUSB Defense (Kernel-Internal)

Three models run before the `InputDevice` capability is granted to a new USB device:

**Model 1: HID Descriptor Classifier**

| Property | Value |
|---|---|
| Model type | Decision tree |
| Size | ~2KB |
| Inference | <0.01ms |
| Input | HID descriptor features (class combinations, usage pages, field counts) |
| Purpose | Detect anomalous class combinations (keyboard + mass storage) |
| Accuracy | >99% detection of known BadUSB toolkits |

**Model 2: Timing Entropy Analyzer**

| Property | Value |
|---|---|
| Model type | Statistical threshold (no ML) |
| Size | ~100 bytes |
| Input | Inter-event intervals for first 100 HID reports |
| Purpose | Human typing has characteristic entropy (not periodic, not random). Scripted injection has near-zero entropy. |
| Detection | Entropy < threshold → scripted injection alert |

**Model 3: Traffic Fingerprint CNN**

| Property | Value |
|---|---|
| Model type | Small 1D CNN |
| Parameters | ~20K |
| Size | ~80KB (INT8) |
| Input | First 100 USB packet timing patterns (~1 second) |
| Inference | <1ms |
| Purpose | Distinguish legitimate HID devices from attack devices by traffic patterns |
| Accuracy | >98% on legitimate devices, <0.1% false positive |

**Integration with capability system:**

```text
USB device connected
    │
    ├── Parse HID descriptor
    │   └── Model 1: descriptor classifier
    │       ├── PASS → continue
    │       └── FAIL → block + alert
    │
    ├── Buffer first 100 reports (do NOT grant capability yet)
    │   └── Model 2: timing entropy
    │       ├── PASS → continue
    │       └── FAIL → block + alert
    │
    ├── Analyze buffered traffic
    │   └── Model 3: traffic fingerprint
    │       ├── PASS → grant InputDevice capability
    │       └── FAIL → block + alert
    │
    └── Continuous monitoring (post-grant)
        └── Ongoing entropy check on typing patterns
            ├── Normal → no action
            └── Anomaly → revoke InputDevice capability + alert
```

#### Behavioral-Level: Continuous Authentication (Split)

**Kernel-internal features (extracted in input subsystem):**

| Feature Set | Model | Size | Purpose |
|---|---|---|---|
| Keystroke dynamics | One-class SVM | ~10KB/user | Hold time, flight time, digraph latency distributions |
| Mouse dynamics | Isolation Forest | ~5KB/user | Speed distribution, acceleration, curvature, click duration |
| Combined anomaly score | Weighted average | ~100 bytes | Fuse keystroke + mouse scores |

These models extract features from input events and compute anomaly scores. They never see content — only timing and kinematic features.

**AIRS-dependent behavioral fusion:**

The AIRS Behavioral Monitor (see [airs.md](../../intelligence/airs.md)) fuses input anomaly scores with other signals:

- Cross-modal correlation: keystroke + mouse + application usage patterns
- GNN on behavioral graph: model transitions between input modalities
- Continuous authentication confidence: if confidence drops below threshold → trigger re-authentication

**Capability chain:**
- Input subsystem computes timing features (kernel-internal, no content)
- Input subsystem sends `BiometricFeature` (feature vectors, NOT raw events) to AIRS
- AIRS Behavioral Monitor holds `BiometricFeature` capability
- AIRS does NOT hold `InputData` — structurally cannot access raw keystrokes

**Privacy:**
- Keystroke dynamics are biometric data — stored encrypted in user's identity space
- `BiometricTemplate` capability required to read enrolled templates
- Feature extraction is one-way: timing features cannot reconstruct what was typed

#### Software Injection Detection (Kernel-Internal)

Even agents with `InputInject` capability have their injection patterns monitored:

| Detection | Method | Purpose |
|---|---|---|
| Rate limiting | Events/second threshold per capability | Prevent input flooding |
| Timing analysis | Inter-event entropy vs expected distribution | Detect scripted patterns |
| Target analysis | Which surfaces receive injected events | Detect privilege escalation attempts |
| Flagging | All injected events carry `INJECTED` flag | Policy can reject for sensitive ops |

-----

### 10.5 Context-Aware Shortcuts (AIRS-Dependent)

When AIRS is online, the Context Engine provides intelligent shortcut suggestions:

#### Adaptive Shortcut Suggestion

- Track which menu items / commands the user accesses most frequently
- Suggest keyboard shortcuts for frequently used actions
- Features: active application, recent commands, time of day, document type

#### Workflow Prediction

- Markov chain on command sequences per workflow context
- Predict next command and pre-populate (e.g., `git status` → `git add .` → `git commit`)
- Requires `ContextRead` + `TaskRead` capabilities from AIRS

#### Command Palette Ranking

- Rank command palette entries by predicted relevance
- Small gradient-boosted tree or MLP (~50K params) on AIRS side
- Features: current application, recent commands, document type, time of day
- Latency: <10ms per query

All context-aware features require explicit AIRS capabilities and degrade to frequency-based ordering when AIRS is offline.

-----

### 10.6 Accessibility Adaptation

#### Tremor Compensation (Kernel-Internal)

Two-tier tremor filtering:

**Tier 1: Kalman Filter (default)**

| Property | Value |
|---|---|
| Algorithm | Kalman filter separating intentional (low-freq) from tremor (3–12Hz) |
| Size | ~1KB state |
| Latency | <0.01ms per sample |
| Effectiveness | 60–80% reduction in tremor-induced pointing error |
| Detection | Auto-detected via FFT on 1-second pointer movement window |
| Activation | Automatic when tremor frequency detected — no user configuration needed |

**Tier 2: CNN Predictor (upgrade)**

| Property | Value |
|---|---|
| Model | 1D CNN, ~30K params |
| Size | ~120KB (INT8) |
| Latency | <0.2ms per sample at 100Hz |
| Effectiveness | Better than Kalman for irregular tremor patterns |
| Activation | AIRS recommends upgrade when Kalman residual exceeds threshold |

**Progressive enhancement:** The system detects tremor automatically (no user self-identification required) and enables compensation transparently. This is privacy-preserving — the user benefits without disclosing a medical condition.

#### Adaptive Switch Scanning

For users operating a single switch:

**Frequency-based reordering (kernel-internal):**
- Track selection frequency per UI element per screen
- Reorder scanning sequence: most-selected elements first
- Reduces average selection time by 40–60% vs linear scan
- Model: categorical distribution (~1KB per application screen)

**Context-aware prediction (AIRS-dependent):**
- AIRS Context Engine predicts intended targets based on:
  - Time of day (morning routine: email → calendar → messages)
  - Recent notifications (new message → likely wants Messages app)
  - Application state (open dialog → likely wants OK or Cancel)
- Scan order adapts to predicted intent
- Requires `ContextRead` + `AttentionRead` capabilities

#### Click Assistance (Kernel-Internal)

Small LSTM (~10K params, ~40KB) distinguishes intentional from involuntary mouse clicks:

| Property | Value |
|---|---|
| Input | Click timing features (hold duration, inter-click interval, approach velocity) |
| Output | Probability of intentional click |
| Training | Transfer learning + per-user fine-tuning during accessibility setup |
| Latency | <0.1ms per click event |

#### Fitts' Law Adaptive Target Sizing (Kernel-Internal)

Dynamically resize touch targets based on predicted pointing accuracy:

- Per-user Fitts' Law parameters (a, b coefficients) — ~16 bytes, updated online
- Predict error probability for each target given its size and distance
- Enlarge targets where error probability > threshold
- Compositor provides target geometry; input subsystem returns adjusted hit regions

#### Built-In Keyboard Accessibility Transforms

Three classic transforms implemented as pipeline stages:

| Transform | Behavior | State Size |
|---|---|---|
| **StickyKeys** | Modifier keys (Shift, Ctrl, Alt) stay active after release until next non-modifier key | ~4 bytes (active modifiers) |
| **FilterKeys** | Ignore brief keystrokes (configurable minimum hold duration) | ~4 bytes (threshold) |
| **BounceKeys** | Ignore rapid re-presses of the same key (configurable interval) | ~8 bytes (last key + timestamp) |

These transforms run in the kernel-internal pipeline (Stage 6: Accessibility Transforms, see [events.md](./events.md) §4.2) and are activated via the accessibility agent's `Configure` capability.

-----

### 10.7 Kernel-Internal Model Budget

Total model footprint for the full kernel-internal ML suite:

| Category | Models | Total Size |
|---|---|---|
| **Security** | BadUSB HID classifier (2KB) + timing entropy (100B) + traffic CNN (80KB) + injection detector (1KB) + keystroke SVM (10KB) + mouse SVM (5KB) | ~98KB |
| **Accessibility** | Tremor Kalman (1KB) + tremor CNN (120KB) + debounce HMM (100B) + click LSTM (40KB) + Fitts' adapter (16B) + scan frequency (1KB) | ~162KB |
| **Comfort** | Pointer accel (20B) + key repeat HMM (100B) + palm rejection CNN (200KB) + touch Kalman (1KB) + spatial touch Gaussian (10KB) | ~211KB |
| **Gestures** | $P+ templates (2KB/gesture) + TCN backbone (800KB) + prototypes (512B/class) | ~803KB |
| **Prediction** | N-gram predictor (500KB) | ~500KB |
| **Total frozen models** | | **~1.77MB** |
| **Per-user adaptive state** | All adaptive parameters combined | **~50KB** |

All models are loaded once at input subsystem startup. Per-user adaptive state is loaded on user login and saved on session close.
