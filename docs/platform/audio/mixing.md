# AIOS Audio Subsystem — PCM Mixing & Format Negotiation

Part of: [audio.md](../audio.md) — Audio Subsystem
**Related:** [subsystem.md](./subsystem.md) — Architecture and sessions, [drivers.md](./drivers.md) — Hardware drivers, [scheduling.md](./scheduling.md) — RT scheduling and A/V sync

-----

## 4. PCM Mixing Engine

The mixer is the heart of the audio subsystem. It combines PCM streams from multiple agents into a single output stream sent to the hardware device. The mixer runs as an RT-class thread with hard deadlines.

### 4.1 Mixer Architecture

```text
Agent A ──→ [Stream A: 44100 Hz, stereo, f32] ──→ ┐
                                                    │  Format
Agent B ──→ [Stream B: 48000 Hz, mono,   i16] ──→ ├──→ Conversion ──→ ┐
                                                    │  (SRC + channel   │
Agent C ──→ [Stream C: 48000 Hz, stereo, f32] ──→ ┘   mapping)        │
                                                                        │
                                    ┌───────────────────────────────────┘
                                    ▼
                              ┌───────────┐
                              │   Mixer   │
                              │           │
                              │  Sum all  │   Output format:
                              │  streams  │   48000 Hz, stereo, f32
                              │  at f32   │──→ [Mixed PCM] ──→ Hardware
                              │           │
                              │  Apply:   │
                              │  - volume │
                              │  - pan    │
                              │  - clip   │
                              └───────────┘
```

### 4.2 Mixer Implementation

```rust
pub struct PcmMixer {
    /// All active playback streams feeding into the mixer
    streams: Vec<MixerStream>,

    /// Output format (matches hardware device)
    output_format: AudioFormat,

    /// Mix buffer (pre-allocated, reused each callback)
    mix_buffer: Vec<f32>,

    /// Per-stream temporary buffer for format conversion
    convert_buffer: Vec<f32>,

    /// System master volume (0.0 - 1.0)
    master_volume: AtomicF32,

    /// RT thread handle for the mix callback
    rt_thread: Option<ThreadId>,

    /// Ring buffer shared with hardware driver
    hardware_ring: RingBuffer<f32>,

    /// Buffer size in frames (determines latency)
    buffer_frames: u32,

    /// Number of mix periods (double-buffering: 2, triple: 3)
    periods: u32,
}

/// A single agent's audio stream feeding the mixer
struct MixerStream {
    id: StreamId,
    agent: AgentId,
    session: SessionId,

    /// Ring buffer: agent writes here, mixer reads
    ring: RingBuffer<f32>,

    /// Source format (may differ from output format)
    source_format: AudioFormat,

    /// Sample rate converter (if source rate != output rate)
    src: Option<SampleRateConverter>,

    /// Channel mapper (if source channels != output channels)
    channel_map: Option<ChannelMapper>,

    /// Per-stream volume (0.0 - 1.0)
    volume: AtomicF32,

    /// Per-stream pan (-1.0 = full left, 0.0 = center, 1.0 = full right)
    pan: AtomicF32,

    /// Muted flag
    muted: AtomicBool,

    /// Priority (RT streams get preference if mixer must drop frames)
    priority: Priority,

    /// Underrun counter (incremented when agent fails to supply samples)
    underrun_count: u64,
}
```

### 4.3 Mix Callback (RT Thread)

The mix callback runs in the scheduler's Real-Time class. It is invoked at a fixed period (default: 5ms / 200 Hz) and must complete within its WCET budget (0.5ms). See [scheduler.md](../../kernel/scheduler.md) §5.2 for the RT admission parameters.

**INVARIANT: No blocking in RT callbacks.** The mix and capture callbacks must NEVER:
- Acquire locks (Mutex, RwLock, spinlock)
- Allocate memory (no Vec::push, no Box::new, no alloc)
- Perform I/O (no IPC send, no UART write, no disk access)
- Call any function with unbounded execution time

All buffers are pre-allocated. All stream state is accessed via atomics (Relaxed ordering
is sufficient — the mixer is the sole writer for mix_buffer, and per-stream volumes are
eventually-consistent). Violations introduce unbounded latency and cause audible glitches.

This invariant is validated by the AAudio/Oboe strict callback discipline used in Android's
low-latency audio path, and by CoreAudio's Audio Unit render callbacks on macOS/iOS.

```rust
/// Called by the RT scheduler every `period` (5ms default).
/// Must complete within WCET (0.5ms).
/// Registered as an RT task:
///   period = 5ms, wcet = 0.5ms, deadline = 5ms
fn mix_callback(mixer: &mut PcmMixer) {
    let frames = mixer.buffer_frames;
    let channels = mixer.output_format.channels as usize;
    let mix = &mut mixer.mix_buffer[..frames as usize * channels];

    // 1. Zero the mix buffer
    mix.fill(0.0);

    // 2. Read and mix each active stream
    for stream in &mut mixer.streams {
        if stream.muted.load(Relaxed) {
            // Skip muted streams (still drain their ring buffer)
            stream.ring.advance(frames);
            continue;
        }

        // Read samples from the agent's ring buffer
        let read = stream.ring.read(&mut mixer.convert_buffer[..frames as usize * channels]);
        if read == 0 {
            // Underrun: agent didn't supply samples in time
            // Silence is mixed (zeros). Increment underrun counter.
            stream.underrun_count += 1;
            continue;
        }

        // Sample rate conversion (if needed)
        if let Some(src) = &mut stream.src {
            src.process(&mut mixer.convert_buffer[..read]);
        }

        // Channel mapping (if needed)
        if let Some(mapper) = &stream.channel_map {
            mapper.map(&mut mixer.convert_buffer[..read]);
        }

        // Apply per-stream volume and pan
        let vol = stream.volume.load(Relaxed);
        let pan = stream.pan.load(Relaxed);
        apply_volume_pan(&mut mixer.convert_buffer[..read], vol, pan, channels);

        // Accumulate into mix buffer (additive mixing)
        for (dst, src) in mix.iter_mut().zip(mixer.convert_buffer[..read].iter()) {
            *dst += *src;
        }
    }

    // 3. Apply master volume
    let master = mixer.master_volume.load(Relaxed);
    for sample in mix.iter_mut() {
        *sample *= master;
    }

    // 4. Clip to [-1.0, 1.0] (prevent distortion from multiple loud streams)
    for sample in mix.iter_mut() {
        *sample = sample.clamp(-1.0, 1.0);
    }

    // 5. Write to hardware ring buffer
    mixer.hardware_ring.write(mix);
}
```

### 4.4 Sample Rate Conversion

When an agent's stream format differs from the hardware output format, the mixer performs sample rate conversion (SRC). AIOS uses a polyphase sinc interpolator for high quality and a linear interpolator for low-latency scenarios.

```rust
pub struct SampleRateConverter {
    /// Source sample rate (e.g., 44100)
    source_rate: u32,
    /// Target sample rate (e.g., 48000)
    target_rate: u32,
    /// Conversion ratio
    ratio: f64,
    /// Filter coefficients (precomputed for this ratio)
    coefficients: Vec<f32>,
    /// History buffer for the polyphase filter
    history: Vec<f32>,
    /// Quality setting
    quality: SrcQuality,
}

pub enum SrcQuality {
    /// Linear interpolation — lowest latency, acceptable for notifications
    Linear,
    /// Polyphase sinc — high quality, used for music playback
    Sinc { filter_len: usize },
}
```

### 4.5 Capture Engine

Audio capture (microphone input) is the reverse path. The hardware driver fills a ring buffer with PCM samples; the capture engine processes those samples through a pipeline of stages before distributing to agents.

The capture engine processes audio from hardware through a pipeline of processing stages before distributing to agents.

Pipeline: `hardware → AGC → AEC → NoiseSuppressor → VAD → distribution`

Each stage implements the `CaptureStage` trait. Stages can be swapped at runtime (e.g., AIRS upgrading noise suppression model).

```rust
/// The capture engine processes audio from hardware through a pipeline
/// of processing stages before distributing to agents.
///
/// Pipeline: hardware → AGC → AEC → NoiseSuppressor → VAD → distribution
///
/// Each stage implements the `CaptureStage` trait. Stages can be
/// swapped at runtime (e.g., AIRS upgrading noise suppression model).
pub trait CaptureStage: Send {
    /// Process audio samples in-place.
    /// Must complete within its declared WCET budget.
    fn process(&mut self, samples: &mut [f32]) -> CaptureStageResult;

    /// Declared worst-case execution time for RT admission.
    fn wcet(&self) -> Duration;

    /// Human-readable name for audit logging.
    fn name(&self) -> &str;
}

pub enum CaptureStageResult {
    /// Samples processed, continue pipeline
    Continue,
    /// VAD detected silence — signal power manager, still deliver silence to agents
    Silence,
    /// Stage wants to be bypassed (e.g., AEC when no playback is active)
    Bypass,
}

pub struct CaptureMux {
    /// Active capture streams (one per agent session)
    streams: Vec<CaptureStream>,
    /// Hardware input ring buffer
    hardware_ring: RingBuffer<f32>,
    /// Processing pipeline stages (executed in order)
    pipeline: Vec<Box<dyn CaptureStage>>,
    /// Quality metrics exposed to AIRS
    metrics: CaptureQualityMetrics,
    /// Pre-allocated scratch buffer (sized to max period frames × channels)
    pre_allocated_buffer: Vec<f32>,
}
```

The default pipeline is constructed at subsystem init. All stages are kernel-internal ML models — no AIRS dependency.

```rust
impl CaptureMux {
    /// Build the default capture processing pipeline.
    /// All stages are kernel-internal ML models — no AIRS dependency.
    fn default_pipeline() -> Vec<Box<dyn CaptureStage>> {
        vec![
            Box::new(AutoGainControl::new()),        // normalize input level
            Box::new(EchoCanceller::new()),           // remove speaker bleed
            Box::new(NoiseSuppressor::rnnoise()),     // frozen RNNoise model (<100KB)
            Box::new(VoiceActivityDetector::new()),   // gates capture device power
        ]
    }
}
```

`NoiseSuppressor` uses a GRU-based model predicting per-band gains. `VoiceActivityDetector` combines energy thresholding with a small neural classifier and gates the capture device when no speech is detected.

```rust
/// RNNoise-class noise suppression.
/// GRU-based model predicting per-band gains across 22 frequency bands.
/// Model: <100KB weights, ~5% CPU per active capture stream.
/// Compiled into the audio subsystem binary — no runtime loading.
pub struct NoiseSuppressor {
    /// Model weights (frozen, compiled-in)
    weights: &'static [f32],
    /// GRU hidden state (per-stream, reset on session start)
    hidden_state: Vec<f32>,
    /// 22-band gain output
    band_gains: [f32; 22],
    /// FFT workspace (pre-allocated)
    fft_buffer: Vec<f32>,
}

impl NoiseSuppressor {
    /// Create with the built-in RNNoise model.
    pub fn rnnoise() -> Self { /* ... */ }

    /// AIRS can swap in a more capable model (e.g., DTLN for voice calls).
    /// The new model must implement CaptureStage with compatible WCET.
    pub fn upgrade_model(&mut self, weights: &'static [f32], config: ModelConfig) { /* ... */ }
}

impl CaptureStage for NoiseSuppressor {
    fn process(&mut self, samples: &mut [f32]) -> CaptureStageResult {
        // 1. FFT → frequency domain
        // 2. Run GRU inference → 22-band gains
        // 3. Apply gains per band
        // 4. IFFT → time domain
        CaptureStageResult::Continue
    }
    fn wcet(&self) -> Duration { Duration::from_micros(150) }
    fn name(&self) -> &str { "noise_suppressor" }
}

/// Voice Activity Detection.
/// Combines energy thresholding with a small neural classifier (~10KB).
/// Gates capture device power when no speech detected.
pub struct VoiceActivityDetector {
    /// Energy threshold (adaptive, tracks noise floor)
    energy_threshold: f32,
    /// Neural classifier weights (frozen, compiled-in)
    classifier_weights: &'static [f32],
    /// Consecutive silence frames before signaling power manager
    silence_frames: u32,
    /// Configurable silence duration before power gate (default: 3 seconds)
    silence_timeout_frames: u32,
    /// Whether speech is currently detected
    speech_active: bool,
}

impl CaptureStage for VoiceActivityDetector {
    fn process(&mut self, samples: &mut [f32]) -> CaptureStageResult {
        let energy = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
        let is_speech = energy > self.energy_threshold
            || self.neural_classify(samples);

        if is_speech {
            self.silence_frames = 0;
            self.speech_active = true;
            CaptureStageResult::Continue
        } else {
            self.silence_frames += 1;
            if self.silence_frames > self.silence_timeout_frames {
                self.speech_active = false;
                CaptureStageResult::Silence // signal power manager
            } else {
                CaptureStageResult::Continue
            }
        }
    }
    fn wcet(&self) -> Duration { Duration::from_micros(50) }
    fn name(&self) -> &str { "vad" }
}
```

The capture callback runs the pipeline in sequence, then distributes processed samples to all active agent streams.

```rust
/// Called by the RT scheduler at the capture period.
/// Registered as RT task: period = 5ms, wcet = sum of all pipeline stage WCETs
fn capture_callback(capture: &mut CaptureMux) {
    let frames = capture.hardware_ring.available();
    let mut samples = &mut capture.pre_allocated_buffer[..frames];
    capture.hardware_ring.read(samples);

    // Run processing pipeline
    let mut power_gate = false;
    for stage in &mut capture.pipeline {
        match stage.process(samples) {
            CaptureStageResult::Continue => {}
            CaptureStageResult::Silence => { power_gate = true; }
            CaptureStageResult::Bypass => { continue; }
        }
    }

    // Signal power manager if VAD detected extended silence
    if power_gate {
        capture.signal_power_manager(PowerHint::CaptureIdle);
    }

    // Distribute to all active capture streams
    for stream in &mut capture.streams {
        let converted = if let Some(src) = &mut stream.src {
            src.process_capture(samples)
        } else {
            samples
        };
        stream.ring.write(converted);
    }

    // Update quality metrics for AIRS
    capture.metrics.update(samples);
}
```

### 4.6 DSP Filter Graph

The mixer supports an extensible DSP filter graph for inserting processing nodes at three points in the audio pipeline. This is inspired by PipeWire's filter-chain architecture but uses a simpler linear insertion model suitable for RT execution with admission control.

```rust
/// A processing node that can be inserted into the audio pipeline.
/// Each node declares its WCET budget; the RT admission controller
/// validates that total pipeline WCET fits within the mix period.
pub trait DspNode: Send {
    /// Process audio samples in-place.
    fn process(&mut self, samples: &mut [f32], format: &AudioFormat);

    /// Declared worst-case execution time.
    fn wcet(&self) -> Duration;

    /// Human-readable name for audit and diagnostics.
    fn name(&self) -> &str;

    /// Whether this node is currently active (inactive nodes are skipped).
    fn is_active(&self) -> bool { true }
}

/// Where in the pipeline a DSP node is inserted.
pub enum DspInsertionPoint {
    /// Per-stream: applied to a single agent's stream before mixing.
    /// Use case: per-agent EQ, noise gate, compressor.
    PerStream(StreamId),

    /// Post-mix: applied to the mixed output before hardware delivery.
    /// Use case: system-wide EQ, spatial audio rendering, master limiter.
    PostMix,

    /// Per-device: applied to the hardware output after format conversion.
    /// Use case: device-specific correction (speaker EQ, DAC calibration).
    PerDevice(DeviceId),
}

/// Registry of active DSP nodes. The mixer consults this during each
/// mix callback to apply per-stream and post-mix processing.
pub struct DspFilterGraph {
    /// Per-stream nodes, keyed by stream ID
    per_stream: HashMap<StreamId, Vec<Box<dyn DspNode>>>,
    /// Post-mix nodes (applied in order)
    post_mix: Vec<Box<dyn DspNode>>,
    /// Per-device nodes, keyed by device ID
    per_device: HashMap<DeviceId, Vec<Box<dyn DspNode>>>,
    /// Total WCET budget consumed by active DSP nodes
    total_wcet: Duration,
    /// Maximum allowed WCET for DSP processing (from RT admission)
    max_wcet: Duration,
}

impl DspFilterGraph {
    /// Insert a DSP node. Fails if total WCET would exceed budget.
    pub fn insert(&mut self, point: DspInsertionPoint, node: Box<dyn DspNode>) -> Result<()> {
        let new_total = self.total_wcet + node.wcet();
        if new_total > self.max_wcet {
            return Err(AudioError::DspBudgetExceeded {
                requested: node.wcet(),
                available: self.max_wcet - self.total_wcet,
            });
        }

        match point {
            DspInsertionPoint::PerStream(id) => {
                self.per_stream.entry(id).or_default().push(node);
            }
            DspInsertionPoint::PostMix => {
                self.post_mix.push(node);
            }
            DspInsertionPoint::PerDevice(id) => {
                self.per_device.entry(id).or_default().push(node);
            }
        }

        self.total_wcet = new_total;
        Ok(())
    }
}
```

#### DSP Node Examples

**Parametric EQ** — adjusts frequency response per stream or system-wide. WCET: ~50μs for a 5-band parametric EQ at 48kHz. Controlled by user preferences or AIRS content-type detection.

**Spatial audio renderer** — HRTF binaural processing that converts stereo or multi-channel audio to headphone-optimized 3D sound. WCET: ~200μs. Inserted as a post-mix node when headphones are detected and spatial audio is enabled.

**Noise gate** — suppresses audio below a threshold. Useful for game voice chat where agents want to eliminate background noise from their stream. WCET: ~10μs. Inserted per-stream.

**AI effect nodes** — AIRS can insert DSP nodes that run frozen inference models. The WCET budget prevents AI processing from causing underruns. If the model is too expensive for real-time, AIRS must process asynchronously and feed results through a ring buffer.

-----

## 10. Audio Formats and Negotiation

### 10.1 Format Types

```rust
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub format: SampleFormat,
}

pub enum SampleFormat {
    /// 32-bit floating point [-1.0, 1.0] — internal mixer format
    F32,
    /// 16-bit signed integer [-32768, 32767] — CD quality, USB audio
    I16,
    /// 24-bit signed integer (packed) — professional audio, HDMI
    I24,
    /// 32-bit signed integer — high-resolution audio
    I32,
    /// 8-bit unsigned [0, 255] — legacy, low quality
    U8,
}

impl AudioFormat {
    pub fn bytes_per_sample(&self) -> u32 {
        match self.format {
            SampleFormat::F32 | SampleFormat::I32 => 4,
            SampleFormat::I24 => 3,
            SampleFormat::I16 => 2,
            SampleFormat::U8 => 1,
        }
    }

    pub fn bytes_per_frame(&self) -> u32 {
        self.bytes_per_sample() * self.channels as u32
    }

    pub fn bytes_per_second(&self) -> u32 {
        self.bytes_per_frame() * self.sample_rate
    }
}
```

### 10.2 Format Negotiation

When an agent opens a session with a requested format, the subsystem negotiates the actual format based on what the hardware supports.

```rust
/// Negotiate the best format between agent preference and device capability.
/// Returns the format that will be used for the session.
/// The mixer handles conversion between this format and the hardware format.
fn negotiate_audio_format(
    requested: &AudioFormat,
    supported: &[AudioFormat],
) -> Result<AudioFormat> {
    // 1. Exact match — no conversion needed
    if let Some(exact) = supported.iter().find(|f| **f == *requested) {
        return Ok(exact.clone());
    }

    // 2. Same sample rate, different format — cheap conversion
    if let Some(rate_match) = supported.iter()
        .find(|f| f.sample_rate == requested.sample_rate
                   && f.channels >= requested.channels)
    {
        return Ok(rate_match.clone());
    }

    // 3. Different sample rate — SRC needed
    //    Prefer 48000 (standard) or closest to requested
    let best = supported.iter()
        .min_by_key(|f| {
            let rate_diff = (f.sample_rate as i64 - requested.sample_rate as i64).abs();
            let ch_diff = (f.channels as i64 - requested.channels as i64).abs();
            rate_diff * 10 + ch_diff // weight sample rate more
        })
        .ok_or(AudioError::NoCompatibleFormat)?;

    Ok(best.clone())
}
```
