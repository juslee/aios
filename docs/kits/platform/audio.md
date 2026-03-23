# Audio Kit

**Layer:** Platform | **Crate:** `aios_audio` | **Architecture:** [`docs/platform/audio.md`](../../platform/audio.md)

## 1. Overview

Audio Kit provides session-based audio playback, capture, mixing, and DSP processing with
hardware-agnostic driver abstraction. Every audio operation happens within an `AudioSession`
that declares its role (playback, capture, communication, or system sounds), allowing the
system to arbitrate between competing consumers. When a communication session starts during
music playback, Audio Kit automatically ducks the music volume and restores it when the call
ends -- all without the music agent needing to know about the call.

The audio pipeline is built around three layers: sessions for policy and routing, the mixer
for combining multiple audio streams with sample-rate conversion, and the DSP filter graph
for real-time processing (equalization, noise suppression, acoustic echo cancellation,
spatialization). Privacy is a first-class concern: microphone capture sessions require the
`AudioCapture` capability and always activate a visible microphone indicator in the
compositor, ensuring no agent can silently record audio.

Use Audio Kit when your agent needs to play sounds, capture microphone input, process audio
in real time, or participate in communication sessions. Do not use it for media file decoding
(use [Media Kit](./media.md), which builds on Audio Kit) or for notification sounds (use
[Notification Kit](../application/notification.md), which routes through Audio Kit internally).

## 2. Core Traits

```rust
use aios_capability::CapabilityHandle;
use core::time::Duration;

/// A capability-gated audio session with role-based routing.
///
/// Sessions are the entry point for all audio operations. The session's
/// role determines its priority in the mixing pipeline and how it interacts
/// with other active sessions.
pub trait AudioSession {
    /// The session's unique identifier.
    fn id(&self) -> SessionId;

    /// The session's declared role.
    fn role(&self) -> AudioRole;

    /// The current session state (active, suspended, ducked).
    fn state(&self) -> SessionState;

    /// The audio format negotiated for this session.
    fn format(&self) -> &AudioFormat;

    /// Set the session volume (0.0 = silent, 1.0 = full).
    fn set_volume(&mut self, volume: f32) -> Result<(), AudioError>;

    /// Get the current volume level.
    fn volume(&self) -> f32;

    /// Pause the session (playback stops, capture pauses).
    fn pause(&mut self) -> Result<(), AudioError>;

    /// Resume a paused session.
    fn resume(&mut self) -> Result<(), AudioError>;

    /// End the session and release all resources.
    fn close(self) -> Result<(), AudioError>;
}

/// Audio session roles that determine mixing priority and arbitration.
pub enum AudioRole {
    /// Background music or ambient audio. Lowest priority; auto-ducked.
    Playback,
    /// Voice/video communication. Highest priority; ducks other sessions.
    Communication,
    /// System sounds (notifications, alerts). Medium priority.
    System,
    /// Audio capture only (recording, voice input).
    Capture,
    /// Game audio with spatialization support.
    Game,
}

/// Multi-source audio mixer with per-stream volume and SRC.
///
/// The mixer combines multiple audio streams into a single output,
/// handling sample-rate conversion automatically when stream formats
/// differ from the hardware output format.
pub trait AudioMixer {
    /// Add a playback stream to the mixer, returning a writable handle.
    fn add_stream(&mut self, format: AudioFormat) -> Result<PlaybackStream, AudioError>;

    /// Remove a stream from the mixer.
    fn remove_stream(&mut self, stream_id: &StreamId) -> Result<(), AudioError>;

    /// Set the master output volume.
    fn set_master_volume(&mut self, volume: f32) -> Result<(), AudioError>;

    /// Get the current master volume.
    fn master_volume(&self) -> f32;

    /// Get peak level meters for monitoring.
    fn peak_levels(&self) -> PeakLevels;
}

/// A writable audio playback stream.
pub trait PlaybackStream {
    /// The stream's identifier.
    fn id(&self) -> &StreamId;

    /// Write audio samples to the stream buffer.
    fn write(&mut self, samples: &[u8]) -> Result<usize, AudioError>;

    /// Query the available buffer space in bytes.
    fn available(&self) -> usize;

    /// Set the per-stream volume.
    fn set_volume(&mut self, volume: f32) -> Result<(), AudioError>;

    /// Apply a DSP filter chain to this stream.
    fn set_filters(&mut self, filters: &[DspFilter]) -> Result<(), AudioError>;
}

/// Microphone capture with privacy indicator enforcement.
///
/// Opening a CaptureStream automatically activates the compositor's
/// microphone indicator. The indicator cannot be suppressed by agents.
pub trait CaptureStream {
    /// The stream's identifier.
    fn id(&self) -> &StreamId;

    /// Read captured audio samples from the buffer.
    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, AudioError>;

    /// Query the number of bytes available for reading.
    fn available(&self) -> usize;

    /// The capture format (sample rate, channels, bit depth).
    fn format(&self) -> &AudioFormat;

    /// Apply DSP filters to the capture pipeline (e.g., noise suppression).
    fn set_filters(&mut self, filters: &[DspFilter]) -> Result<(), AudioError>;
}

/// Composable DSP filter chain for real-time audio processing.
pub trait DspFilterGraph {
    /// Add a filter to the processing chain.
    fn add_filter(&mut self, filter: DspFilter) -> Result<FilterId, AudioError>;

    /// Remove a filter from the chain.
    fn remove_filter(&mut self, id: &FilterId) -> Result<(), AudioError>;

    /// Reorder the filter chain.
    fn set_order(&mut self, order: &[FilterId]) -> Result<(), AudioError>;

    /// Process a buffer of samples through the filter chain.
    fn process(&self, input: &[u8], output: &mut [u8]) -> Result<usize, AudioError>;
}

/// Available DSP filters.
pub enum DspFilter {
    /// Parametric equalizer with configurable bands.
    Equalizer(Vec<EqBand>),
    /// Acoustic echo cancellation for communication sessions.
    EchoCancellation,
    /// Noise suppression (ML-enhanced when AIRS available).
    NoiseSuppression { strength: f32 },
    /// 3D audio spatialization.
    Spatializer { position: SpatialPosition },
    /// Dynamic range compressor.
    Compressor { threshold: f32, ratio: f32, attack_ms: f32, release_ms: f32 },
    /// Low-pass, high-pass, or band-pass filter.
    BiquadFilter { filter_type: BiquadType, frequency: f32, q: f32 },
}

/// Audio format descriptor.
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub bit_depth: u16,
    pub encoding: AudioEncoding,
}

/// Hardware audio device driver trait.
pub trait AudioDevice {
    /// The device's human-readable name.
    fn name(&self) -> &str;

    /// The device type (output, input, or both).
    fn device_type(&self) -> AudioDeviceType;

    /// Supported audio formats.
    fn supported_formats(&self) -> &[AudioFormat];

    /// The currently active format.
    fn active_format(&self) -> &AudioFormat;

    /// Open the device for playback or capture.
    fn open(&mut self, format: &AudioFormat) -> Result<(), AudioError>;

    /// Close the device.
    fn close(&mut self) -> Result<(), AudioError>;
}
```

## 3. Usage Patterns

**Minimal -- play a short audio clip:**

```rust
use aios_audio::{AudioKit, AudioRole, AudioFormat, AudioEncoding};

// Create a playback session
let mut session = AudioKit::create_session(AudioRole::Playback)?;

// Add a stream to the mixer with the desired format
let mut stream = session.mixer().add_stream(AudioFormat {
    sample_rate: 44100,
    channels: 2,
    bit_depth: 16,
    encoding: AudioEncoding::PcmS16Le,
})?;

// Write PCM samples to the stream
let pcm_data = load_audio_file("notification.wav")?;
stream.write(&pcm_data)?;
```

**Realistic -- voice capture with noise suppression:**

```rust
use aios_audio::{AudioKit, AudioRole, DspFilter, AudioFormat, AudioEncoding};

// Create a capture session (activates microphone indicator automatically)
let mut session = AudioKit::create_session(AudioRole::Capture)?;

// Open a capture stream with noise suppression
let mut capture = session.open_capture(AudioFormat {
    sample_rate: 16000,
    channels: 1,
    bit_depth: 16,
    encoding: AudioEncoding::PcmS16Le,
})?;

capture.set_filters(&[
    DspFilter::NoiseSuppression { strength: 0.8 },
])?;

// Read captured audio in a loop
let mut buffer = vec![0u8; 1600]; // 50ms at 16kHz mono 16-bit
loop {
    let bytes_read = capture.read(&mut buffer)?;
    if bytes_read > 0 {
        process_voice_input(&buffer[..bytes_read]);
    }
}
```

**Advanced -- communication session with echo cancellation:**

```rust
use aios_audio::{AudioKit, AudioRole, DspFilter};

// Communication sessions have highest priority and auto-duck other audio
let mut session = AudioKit::create_session(AudioRole::Communication)?;

// Set up capture with echo cancellation and noise suppression
let mut capture = session.open_capture(AudioFormat::voice_default())?;
capture.set_filters(&[
    DspFilter::EchoCancellation,
    DspFilter::NoiseSuppression { strength: 0.9 },
])?;

// Set up playback for remote party's audio
let mut playback = session.mixer().add_stream(AudioFormat::voice_default())?;

// Audio routing loop
loop {
    // Read local mic input (echo-cancelled)
    let local_audio = capture.read(&mut mic_buffer)?;
    send_to_remote(&mic_buffer[..local_audio]);

    // Play remote party's audio
    let remote_audio = receive_from_remote()?;
    playback.write(&remote_audio)?;
}
```

> **Common Mistakes**
>
> - **Not closing sessions.** Leaked sessions hold hardware resources and keep the
>   microphone indicator active. Always close sessions when done, or use RAII wrappers.
> - **Ignoring session state changes.** Your playback session may be ducked or suspended
>   when a higher-priority session starts. Handle `SessionState::Ducked` and
>   `SessionState::Suspended` events.
> - **Writing samples faster than the hardware consumes them.** Check `available()` before
>   writing to avoid buffer overruns. Use the returned byte count from `write()`.
> - **Requesting `AudioCapture` without justification.** Microphone access is a sensitive
>   capability. Agents that request it without clear user benefit may be flagged by the
>   behavioral monitor.

## 4. Integration Examples

**Audio Kit + Media Kit -- decoded audio playback:**

```rust
use aios_audio::{AudioKit, AudioRole};
use aios_media::{MediaKit, PlaybackPipeline};

// Media Kit handles decoding; Audio Kit handles output
let pipeline = MediaKit::open("space://user/home/music/song.flac")?;
let mut session = AudioKit::create_session(AudioRole::Playback)?;
let mut stream = session.mixer().add_stream(pipeline.audio_format())?;

// Media Kit decodes frames; Audio Kit plays them
while let Some(frame) = pipeline.next_audio_frame()? {
    stream.write(&frame.samples)?;
}
```

**Audio Kit + Notification Kit -- notification sounds:**

```rust
use aios_audio::{AudioKit, AudioRole};
use aios_notification::SoundRef;

// Notification Kit delegates sound playback to Audio Kit internally.
// If you need custom notification sounds, register them:
AudioKit::register_system_sound(
    "com.example.app.alert",
    include_bytes!("sounds/alert.wav"),
)?;

// Then reference it in notifications
NotificationKit::builder()
    .title("Task complete")
    .sound(SoundRef::Custom("com.example.app.alert"))
    .channel(&channel_id)
    .post()?;
```

**Audio Kit + Conversation Kit -- voice input for AI assistant:**

```rust
use aios_audio::{AudioKit, AudioRole, DspFilter};
use aios_conversation::ConversationKit;

let mut session = AudioKit::create_session(AudioRole::Capture)?;
let mut capture = session.open_capture(AudioFormat::voice_default())?;
capture.set_filters(&[
    DspFilter::NoiseSuppression { strength: 0.85 },
])?;

// Stream captured audio to the conversation engine for speech-to-text
let conversation = ConversationKit::current_session()?;
while let Ok(bytes_read) = capture.read(&mut buffer) {
    if bytes_read > 0 {
        conversation.stream_audio(&buffer[..bytes_read])?;
    }
}
```

## 5. Capability Requirements

| Method | Required Capability | Notes |
| --- | --- | --- |
| `AudioKit::create_session(Playback)` | `AudioPlayback` | Granted by default to most agents |
| `AudioKit::create_session(Communication)` | `AudioPlayback` + `AudioCapture` | Ducks other sessions |
| `AudioKit::create_session(Capture)` | `AudioCapture` | Activates microphone indicator |
| `AudioKit::create_session(System)` | `AudioSystem` | Reserved for system services |
| `CaptureStream::read` | `AudioCapture` | Checked on session creation |
| `DspFilterGraph::add_filter` | `AudioPlayback` or `AudioCapture` | Depends on session role |
| `AudioMixer::set_master_volume` | `AudioSystem` | System-level volume control |

```toml
# Agent manifest example
[capabilities.required]
AudioPlayback = { reason = "Play audio clips and music" }

[capabilities.optional]
AudioCapture = { reason = "Voice input for dictation and commands" }
```

## 6. Error Handling

```rust
/// Errors returned by Audio Kit operations.
#[derive(Debug)]
pub enum AudioError {
    /// The required capability was not granted.
    CapabilityDenied(Capability),

    /// No audio output device is available.
    NoOutputDevice,

    /// No audio input device is available.
    NoInputDevice,

    /// The requested audio format is not supported by the hardware.
    UnsupportedFormat(AudioFormat),

    /// The session was suspended by a higher-priority session.
    SessionSuspended,

    /// The audio buffer overflowed (writing faster than hardware drains).
    BufferOverrun,

    /// The audio buffer underflowed (hardware drained faster than writes).
    BufferUnderrun,

    /// The DSP filter configuration is invalid.
    InvalidFilterConfig(String),

    /// The session has already been closed.
    SessionClosed,

    /// The maximum number of concurrent sessions has been reached.
    TooManySessions { max: u32 },

    /// A hardware driver error occurred.
    DeviceError(String),
}
```

**Fallback cascade:**

| Failure | Degradation |
| --- | --- |
| No hardware audio device | Software mixer with silent output; capture unavailable |
| HDMI audio unavailable | Fall back to I2S or USB audio output |
| DSP filter not supported | Filter skipped with warning; audio passes through unprocessed |
| AIRS unavailable | ML-enhanced noise suppression falls back to static DSP filter |
| Session suspended by call | Playback auto-resumes when communication session ends |
| Buffer underrun | Silent gap inserted; no crash; warning logged |

## 7. Platform & AI Availability

**AIRS-enhanced features (require AIRS Kit):**

| Feature | What AIRS provides | Without AIRS |
| --- | --- | --- |
| ML noise suppression | Neural network denoising adapted to environment | Static spectral subtraction |
| Adaptive latency | Learns optimal buffer size per use pattern | Fixed buffer size |
| Echo cancellation | ML-enhanced AEC for complex acoustic environments | Linear adaptive AEC |
| Voice activity detection | Neural VAD with speaker diarization | Energy-threshold VAD |
| Audio scene classification | Identifies environment (office, outdoor, car) | No classification |

**Platform availability:**

| Platform | Playback | Capture | DSP Filters | Spatial Audio | Notes |
| --- | --- | --- | --- | --- | --- |
| QEMU virt | VirtIO-Sound | VirtIO-Sound | Software DSP | Software only | Testing only |
| Raspberry Pi 4 | HDMI/I2S/USB | USB audio | Software DSP | Software only | No onboard mic |
| Raspberry Pi 5 | HDMI/I2S/USB | USB audio | Software DSP | Software only | No onboard mic |
| Apple Silicon | System audio | Built-in mic | Hardware + SW | Hardware spatial | Full experience |

**Implementation phase:** Phase 8+ (session management, mixer, VirtIO-Sound driver, DSP
pipeline). Hardware-specific drivers arrive with their respective BSP phases.

---

*See also: [Media Kit](./media.md) | [Notification Kit](../application/notification.md) | [Conversation Kit](../intelligence/conversation.md) | [Capability Kit](../kernel/capability.md) | [Input Kit](./input.md)*
