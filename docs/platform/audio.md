# AIOS Audio Subsystem

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [subsystem-framework.md](./subsystem-framework.md) — Universal hardware abstraction (capability gate, sessions, data channels, audit, power, POSIX bridge), [scheduler.md](../kernel/scheduler.md) — RT scheduling class and audio deadline (§5.2), [hal.md](../kernel/hal.md) — `PlatformAudio` and `PlatformPwm` extension traits (§12), [compositor.md](./compositor.md) — Render pipeline and presentation timing, [networking.md](./networking.md) — Companion subsystem implementation, [wireless.md](./wireless.md) — Bluetooth audio integration (A2DP, HFP, LE Audio) (§7.3)

**Note:** The audio subsystem implements the subsystem framework. Its capability gate, session model, audit logging, power management, and POSIX bridge follow the universal patterns defined in the framework document. This document covers the audio-specific design decisions and architecture.

-----

## Document Map

This document was split for navigability. Each sub-document preserves the original section numbers for cross-reference stability.

| Document | Sections | Content |
|---|---|---|
| **This file** | §1, §14–§17 | Overview, implementation order, design principles, future directions, AI-native audio |
| [subsystem.md](./audio/subsystem.md) | §2, §3 | Architecture diagram, subsystem implementation (sessions, capability gate, device abstraction, route manager, AIRS integration hooks) |
| [mixing.md](./audio/mixing.md) | §4, §10 | PCM mixing engine, AI-enhanced capture pipeline, DSP filter graph extensibility, format negotiation |
| [drivers.md](./audio/drivers.md) | §5 | Hardware drivers (VirtIO-Sound, I2S, PWM, HDMI, Apple CoreAudio, USB Audio), privacy-first hardware support |
| [scheduling.md](./audio/scheduling.md) | §6, §7 | RT scheduling integration (EDF deadlines, predictive buffer management) and A/V sync with compositor |
| [integration.md](./audio/integration.md) | §8, §9, §11, §12, §13 | HDMI audio routing, power management, audit/observability (with visual mic indicator), POSIX bridge, boot chime |

-----

## 1. Overview

Audio is a system subsystem following the universal Subsystem Framework pattern. It manages the complete audio path — from agents producing or consuming PCM samples, through a software mixing engine, down to hardware-specific drivers that emit or capture sound.

Audio has a unique constraint among subsystems: **time sensitivity is absolute.** A network packet delayed by 50ms is invisible to the user. A video frame delayed by 50ms is a noticeable stutter. An audio sample delayed by 5ms is an audible glitch. Audio is the most timing-critical data path in the entire OS, and the subsystem is designed around this reality.

The audio subsystem provides:

1. **PCM mixing engine** — a software mixer that combines streams from multiple agents into a single output, applying per-stream volume, format conversion, and sample-rate conversion. An extensible DSP filter graph allows insertion of processing nodes (EQ, spatial audio, AI effects) at any point in the pipeline.
2. **Platform-specific drivers** — VirtIO-Sound for QEMU development, I2S for HiFiBerry-style DACs on Raspberry Pi, PWM audio for the Pi headphone jack, HDMI audio for monitors and TVs, and hardware codecs on Apple Silicon. All drivers support privacy-first hardware controls.
3. **Capability-gated sessions** — agents request `AudioPlayback` or `AudioCapture` capabilities; the kernel capability gate enforces access before any audio flows.
4. **RT scheduling integration** — audio mixing callbacks run in the scheduler's Real-Time class with EDF (Earliest Deadline First), guaranteeing that audio never starves (see [scheduler.md](../kernel/scheduler.md) §5.2). AIRS provides predictive buffer hints to preemptively adjust for load changes.
5. **AI-enhanced capture pipeline** — the capture path includes noise suppression (frozen RNNoise-class model), voice activity detection for power management, and echo cancellation as first-class processing stages, not optional add-ons.
6. **A/V synchronization** — a shared timeline with the compositor for synchronized audio-video presentation.
7. **AIRS integration** — context-aware routing, adaptive model selection for capture processing, predictive buffer management, and neural quality assessment via the AI runtime.

**Design principle:** Audio is infrastructure that agents use, not an application. An agent that wants to play a notification sound, stream music, run a voice call, or perform text-to-speech uses the same session API. The subsystem handles mixing, routing, format negotiation, device switching, and latency management transparently.

-----

## 14. Implementation Order

The audio subsystem is implemented across multiple development phases, building complexity incrementally.

```text
Phase 8:   HAL PlatformAudio trait + VirtIO-Sound driver
           ├── VirtIO-Sound device discovery and initialization
           ├── Raw PCM output (write_samples via HAL)
           ├── Boot chime (synthesized, direct HAL write)
           └── Test: audible tone in QEMU

Phase 13:  Audio subsystem service (basic)
           ├── Subsystem registration with framework
           ├── Session open/close with capability gate
           ├── Single-stream playback (no mixing)
           ├── AudioPlayback capability type
           └── Audit logging (session opened/closed)

Phase 16:  PCM mixing engine
           ├── Multi-stream software mixer
           ├── Per-stream volume and pan
           ├── Format negotiation
           ├── Sample rate conversion (linear)
           ├── RT task registration (5ms period, 0.5ms WCET)
           └── Ring buffer shared memory between agent and mixer

Phase 21:  Raspberry Pi audio drivers
           ├── HDMI audio (VC4/VC7)
           ├── PWM audio (3.5mm headphone jack)
           ├── I2S driver (HiFiBerry DAC support)
           ├── DMA-driven output (ping-pong buffers)
           └── Device detection from device tree

Phase 23:  Advanced mixing and capture
           ├── Audio capture (microphone input)
           ├── Capture multiplexing to multiple agents
           ├── AI-enhanced capture pipeline (noise suppression, VAD)
           ├── Echo cancellation
           ├── Automatic gain control
           ├── High-quality SRC (polyphase sinc)
           ├── DSP filter graph (per-stream and post-mix insertion points)
           └── Full-duplex (voice calls)

Phase 25:  USB and Bluetooth audio
           ├── USB Audio Class driver
           ├── Isochronous USB transfers
           ├── Bluetooth A2DP (audio streaming)
           ├── Bluetooth HFP (hands-free voice)
           ├── Hotplug: automatic route switching
           ├── Hardware microphone kill switch support
           └── Crossfade on device change

Phase 27:  A/V sync and HDMI advanced
           ├── Shared media timeline with compositor
           ├── Presentation timestamps
           ├── HDMI EDID audio capability parsing
           ├── HDMI CEC volume/mute control
           ├── Audio Return Channel (ARC)
           ├── Predictive buffer management (AIRS integration)
           └── Multi-channel audio (5.1/7.1 for HDMI)

Phase 31:  Apple Silicon audio + power management
           ├── Apple codec drivers (CS42L83, TAS5770L)
           ├── Hardware DSP integration
           ├── Lid close/open mute behavior
           ├── Thermal throttling (SRC quality reduction)
           ├── On-demand subsystem activation
           ├── AIRS route advisor (context-aware device selection)
           └── Suspend/resume with state preservation

Phase 36:  POSIX bridge + compatibility + spatial audio
           ├── /dev/audio*, /dev/dsp, /dev/mixer* nodes
           ├── OSS-compatible ioctl interface
           ├── ALSA compatibility shim (if needed)
           ├── Spatial audio DSP filter (HRTF binaural rendering)
           ├── Visual microphone activity indicator (compositor integration)
           └── PulseAudio/PipeWire socket compatibility (stretch)
```

-----

## 15. Design Principles

1. **Audio is the clock master.** In any synchronized media pipeline, audio timing is authoritative. Video adjusts to match audio, never the reverse. Audio glitches are more perceptible than dropped video frames.

2. **RT or nothing.** The mix callback runs in the scheduler's Real-Time class with hard EDF deadlines. There is no "best effort" audio path. If the RT admission controller rejects the audio task (utilization ceiling exceeded), the system has a configuration problem, not a graceful degradation.

3. **Never block in the callback.** The mix and capture callbacks must never acquire locks, allocate memory, or perform I/O. All buffers are pre-allocated, all state is accessed via atomics, all slow paths are deferred to non-RT threads. This invariant is non-negotiable — violating it introduces unbounded latency. (Validated by AAudio/Oboe strict callback discipline.)

4. **Shared by default, exclusive by capability.** Multiple agents share the mixer. Only agents with explicit `exclusive: true` capability can bypass the mixer for direct hardware access. This keeps the common case simple and the rare case possible.

5. **The mixer is always f32 internally.** All format conversion happens at the edges — when samples enter the mixer (from agents) and when they leave (to hardware). Internal mixing is always 32-bit float to preserve dynamic range.

6. **Silence is better than garbage.** On underrun, the mixer inserts silence. Repeating stale samples or interpolating is worse than a brief silence in nearly all cases. The exception (music playback) can opt into repeat-last behavior via session intent.

7. **Microphone access is always audited, prompted, and visible.** No agent can capture audio without user consent. The capability gate enforces this, every microphone access is logged to the audit space, and a persistent visual indicator shows active capture sessions. There are no backdoors. Hardware kill switches produce silence, not errors.

8. **Hardware details stop at the device abstraction.** The mixer does not know whether it is writing to VirtIO, I2S, PWM, HDMI, or USB. The `AudioDevice` trait is the boundary. Adding a new audio output type requires only implementing that trait.

9. **Latency is configurable per session.** Voice calls use 2.5ms buffers. Music playback uses 20ms buffers. The agent declares intent, and the subsystem chooses the appropriate latency mode. One size does not fit all.

10. **Power management is aggressive.** Audio hardware is powered down within 10 seconds of the last session closing. The 100ms wake-up penalty for the first sound after idle is acceptable — users do not perceive it. VAD-based power gating further reduces capture device power when no speech is detected.

11. **The boot chime proves the hardware works.** A synthesized tone at Phase 2 completion confirms that the audio path from CPU to DAC is functional. No audio files, no filesystem, no subsystem required — just the HAL and arithmetic.

12. **AI-enhanced capture is first-class, not optional.** Noise suppression, echo cancellation, and voice activity detection are core pipeline stages in the capture engine. They run as frozen kernel-internal models (RNNoise-class, <100KB) with no AIRS dependency. AIRS can swap in more capable models when available, but the baseline always works.

13. **The pipeline is extensible via DSP filter graph.** Processing nodes (EQ, spatial audio, AI effects) can be inserted at per-stream, post-mix, or per-device points. Each node declares its WCET budget; the RT admission controller validates the total. This makes the fixed mixer topology the default while enabling arbitrary processing when needed.

-----

## 16. Future Directions

Research-informed capabilities planned for later phases or hardware generations.

### 16.1 Neural Sample Rate Conversion

Current polyphase sinc interpolation provides high-quality SRC for music playback. Neural SRC models (NU-Wave 2, DAFx24 sample-rate-independent RNNs) show promise for higher quality at arbitrary rate ratios but are not yet real-time-ready for OS-level deployment. When inference cost drops sufficiently, neural SRC could replace polyphase sinc as an AIRS-managed DSP filter node for premium audio sessions.

### 16.2 Spatial Audio and HRTF Rendering

OS-level spatial audio (binaural HRTF rendering, Ambisonics decoding) is a Phase 36+ DSP filter. Key design constraint: spatial processing must be an optional filter node in the DSP graph, not a mandatory system transform — otherwise it conflicts with exclusive-mode access (the WASAPI lesson). HRTF personalization (matching the user's head/ear geometry) is an AIRS-dependent feature requiring calibration data.

Formats to support:

- **Object-based** — individual sound sources with 3D position metadata (most flexible, requires application support)
- **Scene-based (Ambisonics)** — speaker-independent 3D soundfield (flexible, requires decoding)
- **Channel-based** — traditional surround 5.1/7.1 (most compatible)

### 16.3 Formal WCET Verification

seL4's methodology for formal worst-case execution time analysis can be applied to the audio mix and capture callbacks. This would provide mathematical proof that the callbacks complete within their declared WCET budgets (0.5ms mixer, 0.3ms capture), eliminating underruns by construction rather than by testing. This is a hardening step for production-grade RT guarantees.

### 16.4 Hardware Voice Activity Detection

Dedicated VAD chips (AON1100 M3 at <260 microwatts, NASP NeuroVoice at microwatt-level with microsecond latency) enable always-on voice detection without powering the main audio subsystem. The HAL `PlatformAudio` trait should be extended with a `hw_vad() -> Option<&dyn HardwareVad>` method. When present, the hardware VAD replaces the software VAD in the capture pipeline, reducing power consumption by orders of magnitude for always-listening scenarios.

### 16.5 Composable Processing Graph (PipeWire Model)

The current DSP filter graph (§4.6) provides per-stream and post-mix insertion points. A future evolution is a fully composable processing graph where nodes are connected by ports and links (PipeWire's architecture). This enables arbitrary topologies:

- Side-chain compression (one stream's level controls another's gain)
- Multi-band splitting and recombination
- Loopback routing (system audio capture for streaming)
- Virtual audio devices (pure software sources/sinks)

The graph topology would be managed by a policy engine — either static configuration or AIRS-driven dynamic rewiring based on active agents and session intents.

-----

## 17. AI-Native Audio

Audio intelligence capabilities that leverage AIRS for semantic understanding and adaptive behavior. These features distinguish AIOS from traditional operating systems by making audio routing, processing, and quality management context-aware and self-optimizing.

### 17.1 AIRS-Dependent Capabilities

These require the AI Runtime Service (AIRS) for semantic understanding and context awareness:

**Context-aware audio routing.** AIRS observes time-of-day patterns, active agents, user location, and session intent to advise the `RouteManager` on device selection. The `AirsRouteAdvisor` (see [subsystem.md](./audio/subsystem.md) §3.1) provides hints as a tiebreaker when multiple devices are equally valid — it never overrides explicit user choices. Examples:

- Morning routine detected → prefer kitchen speaker over headphones
- Video call agent active → route to headset if available
- Meeting room acoustics detected via microphone → suggest speakerphone mode

**Adaptive noise suppression model selection.** The capture pipeline (see [mixing.md](./audio/mixing.md) §4.5) runs a frozen RNNoise-class model by default. AIRS can upgrade to a more capable model (DTLN, ~33ms/second on M1) when it classifies the session as a voice call requiring higher quality. Model selection is transparent to the agent — the capture pipeline swaps the `NoiseSuppressor` trait object.

**Predictive buffer management.** AIRS observes system load patterns (from scheduler and observability subsystems) and sets `BufferHint` values (see [scheduling.md](./audio/scheduling.md) §6.2) to preemptively adjust buffer sizes before predicted load spikes (compilation starting, model inference beginning). The mixer treats hints as advisory — it can accept or ignore based on current underrun statistics.

**Neural audio quality assessment.** AIRS runs non-intrusive quality models (Quality-Net, ARECHO) on sampled audio data to estimate MOS scores without a clean reference signal. When quality drops below threshold (codec artifacts, network jitter, hardware degradation), AIRS can trigger corrective action: suggest codec change, reroute to different device, or alert the user.

**Content-type-aware automatic EQ.** AIRS classifies audio content (speech, music, action/gaming, notification) and applies appropriate frequency profiles via DSP filter graph nodes. Classification runs on a lightweight model analyzing spectral features, not full audio content understanding.

### 17.2 Kernel-Internal ML Capabilities

These are purely statistical and run as frozen models within the audio subsystem, with no AIRS dependency:

**RNNoise-class noise suppression.** A GRU-based model predicting per-band gains across 22 frequency bands. Model size: <100KB weights. CPU cost: ~5% per active capture stream. Runs in the capture pipeline's `NoiseSuppressor` stage. The model is compiled into the audio subsystem binary — no runtime loading, no filesystem dependency.

**Energy-based VAD with neural classifier.** Voice activity detection combines energy thresholding with a small neural classifier (~10KB model). When no speech is detected for a configurable duration (default: 3 seconds), the VAD signals the power manager to suspend the capture device. Speech onset triggers immediate wake. CPU cost: <1% continuous.

**Neural AGC envelope follower.** Replaces heuristic automatic gain control with a small neural model that predicts optimal gain based on recent signal history. Smoother gain transitions, better handling of speech pauses, and reduced pumping artifacts compared to traditional AGC. Frozen weights, deterministic inference.

**Adaptive echo path estimation.** Enhances the traditional adaptive filter (NLMS/RLS) in the echo canceller with a neural post-filter that suppresses residual echo. The adaptive filter handles the linear echo path; the neural post-filter handles non-linear distortion from loudspeaker saturation. Combined CPU cost: ~8% during full-duplex sessions.

-----

## Cross-Reference Index

| Reference | Location | Topic |
|---|---|---|
| §2 | [subsystem.md](./audio/subsystem.md) | System architecture diagram |
| §3.1 Subsystem Registration | [subsystem.md](./audio/subsystem.md) | AudioSubsystem struct, AIRS hooks |
| §3.2 Audio Capabilities | [subsystem.md](./audio/subsystem.md) | Capability tokens and target types |
| §3.3 Audio Sessions | [subsystem.md](./audio/subsystem.md) | Session lifecycle |
| §3.4 Conflict Resolution | [subsystem.md](./audio/subsystem.md) | Share/prompt policies |
| §4.1 Mixer Architecture | [mixing.md](./audio/mixing.md) | N-to-1 sample mixing diagram |
| §4.2 Mixer Implementation | [mixing.md](./audio/mixing.md) | PcmMixer struct |
| §4.3 Mix Callback | [mixing.md](./audio/mixing.md) | RT thread, no-blocking invariant |
| §4.4 Sample Rate Conversion | [mixing.md](./audio/mixing.md) | Polyphase sinc / linear SRC |
| §4.5 Capture Engine | [mixing.md](./audio/mixing.md) | AI-enhanced capture pipeline |
| §4.6 DSP Filter Graph | [mixing.md](./audio/mixing.md) | Extensible processing node system |
| §5.1 VirtIO-Sound | [drivers.md](./audio/drivers.md) | QEMU audio driver |
| §5.2 I2S | [drivers.md](./audio/drivers.md) | HiFiBerry DAC driver |
| §5.3 PWM Audio | [drivers.md](./audio/drivers.md) | Pi 3.5mm headphone jack |
| §5.4 HDMI Audio | [drivers.md](./audio/drivers.md) | HDMI output driver |
| §5.5 Apple Silicon | [drivers.md](./audio/drivers.md) | Apple codec drivers |
| §5.6 USB Audio Class | [drivers.md](./audio/drivers.md) | USB headset driver |
| §5.7 Privacy Controls | [drivers.md](./audio/drivers.md) | Hardware mic kill switch |
| §6.1 RT Task Registration | [scheduling.md](./audio/scheduling.md) | EDF deadline scheduling |
| §6.2 Latency Budget | [scheduling.md](./audio/scheduling.md) | End-to-end latency breakdown |
| §6.3 Buffer Sizing | [scheduling.md](./audio/scheduling.md) | Latency mode configuration |
| §6.4 Underrun Handling | [scheduling.md](./audio/scheduling.md) | Underrun policy and recovery |
| §7.1 Shared Timeline | [scheduling.md](./audio/scheduling.md) | Media clock (audio master) |
| §7.2 Presentation Timestamps | [scheduling.md](./audio/scheduling.md) | PTS for A/V sync |
| §7.3 Synchronization Protocol | [scheduling.md](./audio/scheduling.md) | Drift detection and correction |
| §7.4 Audio-Compositor IPC | [scheduling.md](./audio/scheduling.md) | Timeline update messages |
| §8.1 EDID Parsing | [integration.md](./audio/integration.md) | HDMI audio capability discovery |
| §8.2 CEC Control | [integration.md](./audio/integration.md) | HDMI volume/mute pass-through |
| §8.3 Multi-Output Routing | [integration.md](./audio/integration.md) | Device priority and fallback |
| §9.1 Power States | [integration.md](./audio/integration.md) | Audio power management |
| §9.2 Power Events | [integration.md](./audio/integration.md) | Lid close, suspend, thermal |
| §9.3 On-Demand Activation | [integration.md](./audio/integration.md) | Lazy subsystem startup |
| §10.1 Format Types | [mixing.md](./audio/mixing.md) | AudioFormat, SampleFormat |
| §10.2 Format Negotiation | [mixing.md](./audio/mixing.md) | Agent-device format matching |
| §11.1 Audit Events | [integration.md](./audio/integration.md) | Audio audit logging |
| §11.2 Audit Space | [integration.md](./audio/integration.md) | Audit directory structure |
| §11.3 AIRS Integration | [integration.md](./audio/integration.md) | Audio-related AIRS queries |
| §11.4 Visual Mic Indicator | [integration.md](./audio/integration.md) | Persistent capture indicator |
| §12 POSIX Bridge | [integration.md](./audio/integration.md) | /dev/audio0, /dev/dsp |
| §13 Boot Chime | [integration.md](./audio/integration.md) | Early audio startup |
