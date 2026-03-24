# Media Kit

**Layer:** Platform | **Crate:** `aios_media` | **Architecture:** [`docs/platform/media-pipeline.md`](../../platform/media-pipeline.md)

## 1. Overview

Media Kit is the orchestration layer for all media workloads: video playback, audio decoding,
adaptive streaming, video recording, and real-time communication (WebRTC). It does not replace
Audio Kit, Camera Kit, or the Compositor -- it coordinates them. Playing a video requires
network data to arrive through the networking subsystem, pass through a container demuxer,
feed into parallel video and audio decoders, route decoded video frames to the compositor
for display and decoded audio samples to Audio Kit for playback -- all synchronized to
sub-40ms precision. Media Kit owns this coordination graph.

The pipeline is built around a directed acyclic graph (DAG) of processing elements connected
by typed pads, inspired by GStreamer's element-pad model and PipeWire's unified media graph.
Agents compose pipelines declaratively from registered elements (sources, demuxers, decoders,
filters, encoders, muxers, sinks). The subsystem handles codec selection, format negotiation
between pads, clock synchronization, adaptive quality, and DRM enforcement transparently.
Hardware codec selection prefers dedicated decode engines (V4L2 stateless on Raspberry Pi,
media engines on Apple Silicon, VirtIO-Video on QEMU) and falls back to software
implementations (dav1d for AV1, openh264 for H.264) without agent intervention.

Use Media Kit when your agent needs to play audio or video files, stream content from a
network source, record video with audio, or participate in a WebRTC call. Do not use it for
raw audio playback without decoding (use [Audio Kit](./audio.md) directly) or for raw camera
capture without encoding (use [Camera Kit](./camera.md) directly). Media Kit consumes Audio
Kit for PCM output, Camera Kit for capture input, [Network Kit](./network.md) for streaming
transport, and [Compute Kit](../kernel/compute.md) for hardware-accelerated decode/encode.

## 2. Core Traits

```rust
use aios_capability::CapabilityHandle;
use core::time::Duration;

/// Unique identifier for a compressed format (codec).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodecId {
    // Video codecs
    H264,
    H265,
    VP8,
    VP9,
    AV1,
    // Audio codecs
    AAC,
    Opus,
    Vorbis,
    Flac,
    Pcm,
    Mp3,
}

/// Core abstraction for all encoders and decoders.
///
/// Hardware and software codecs both implement this trait. The codec
/// registry selects the best available implementation based on hardware
/// capabilities and power budget.
pub trait MediaCodec: Send + Sync {
    /// The codec this implementation handles.
    fn codec_id(&self) -> CodecId;

    /// Whether this is a video, audio, or subtitle codec.
    fn codec_type(&self) -> CodecType;

    /// Static capability description (max resolution, profiles, HW accel).
    fn capabilities(&self) -> CodecCapabilities;

    /// Configure the codec before first use. Must be called exactly once.
    fn configure(&mut self, params: &CodecParams) -> Result<(), MediaError>;

    /// Submit an encoded packet, receive zero or more decoded frames.
    fn decode(&mut self, packet: &MediaPacket) -> Result<Vec<MediaFrame>, MediaError>;

    /// Submit a decoded frame, receive zero or more encoded packets.
    fn encode(&mut self, frame: &MediaFrame) -> Result<Vec<MediaPacket>, MediaError>;

    /// Signal end-of-stream and drain buffered frames.
    fn flush(&mut self) -> Result<Vec<MediaFrame>, MediaError>;

    /// Reset to post-configure state (for seek operations).
    fn reset(&mut self);
}

/// Container format demuxer and muxer.
///
/// Parses container formats (MP4, WebM, MKV, MPEG-TS) into individual
/// codec streams, or combines streams into a container for recording.
pub trait ContainerEngine {
    /// Open a container for reading from a data source.
    fn open_demuxer(&mut self, source: DataSource) -> Result<ContainerInfo, MediaError>;

    /// Read the next packet from the container.
    fn next_packet(&mut self) -> Result<Option<MediaPacket>, MediaError>;

    /// Seek to a position in the container (duration from start).
    fn seek(&mut self, position: Duration) -> Result<(), MediaError>;

    /// Create a muxer for writing a container to a data sink.
    fn create_muxer(
        &mut self,
        format: ContainerFormat,
        sink: DataSink,
        streams: &[StreamConfig],
    ) -> Result<MuxerHandle, MediaError>;

    /// Write an encoded packet to the muxer.
    fn write_packet(&mut self, muxer: &MuxerHandle, packet: &MediaPacket)
        -> Result<(), MediaError>;

    /// Finalize and close the muxer (writes index, flushes buffers).
    fn finalize_muxer(&mut self, muxer: MuxerHandle) -> Result<(), MediaError>;
}

/// A composable media processing pipeline.
///
/// Pipelines are DAGs of processing elements (source, demuxer, decoder,
/// filter, encoder, muxer, sink) connected by typed pads. The pipeline
/// manages buffer flow, clock synchronization, and state transitions.
/// Audio is always the clock master -- video frames are dropped or
/// duplicated to maintain sync, but audio is never interrupted.
pub trait PlaybackPipeline {
    /// Current pipeline state.
    fn state(&self) -> PipelineState;

    /// Start or resume playback.
    fn play(&mut self) -> Result<(), MediaError>;

    /// Pause playback (buffers are preserved).
    fn pause(&mut self) -> Result<(), MediaError>;

    /// Stop playback and release resources.
    fn stop(&mut self) -> Result<(), MediaError>;

    /// Seek to a position.
    fn seek(&mut self, position: Duration) -> Result<(), MediaError>;

    /// Get the current playback position.
    fn position(&self) -> Duration;

    /// Get the total duration (if known).
    fn duration(&self) -> Option<Duration>;

    /// Set playback speed (1.0 = normal, 2.0 = double speed).
    fn set_rate(&mut self, rate: f64) -> Result<(), MediaError>;

    /// Select an audio track (for multi-audio content).
    fn select_audio_track(&mut self, track_id: TrackId) -> Result<(), MediaError>;

    /// Select a subtitle track (None disables subtitles).
    fn select_subtitle_track(&mut self, track_id: Option<TrackId>) -> Result<(), MediaError>;

    /// Get the audio format being output to Audio Kit.
    fn audio_format(&self) -> Option<AudioFormat>;

    /// Get the video format being output to the compositor.
    fn video_format(&self) -> Option<VideoFormat>;
}

/// Media session with transport controls and Now Playing metadata.
///
/// Sessions are the user-facing identity of a media playback. They appear
/// in the system's Now Playing UI, respond to media key events, and
/// participate in audio session arbitration via Audio Kit.
pub trait MediaSession {
    /// The session's unique identifier.
    fn id(&self) -> MediaSessionId;

    /// Current playback state.
    fn playback_state(&self) -> PlaybackState;

    /// Set the Now Playing metadata (title, artist, album, artwork).
    fn set_metadata(&mut self, metadata: &MediaMetadata) -> Result<(), MediaError>;

    /// Register a callback for transport control events (play, pause, next, prev).
    fn on_transport_control(&mut self, handler: TransportControlHandler);

    /// Set the playback position for scrubbing UI.
    fn set_position_info(&mut self, position: Duration, duration: Duration);
}

/// Adaptive bitrate streaming engine.
///
/// Handles HLS, DASH, and MoQ streaming protocols with automatic
/// quality adaptation based on network conditions.
pub trait StreamingEngine {
    /// Open a streaming source by URL.
    fn open(&mut self, url: &str) -> Result<StreamInfo, MediaError>;

    /// Get the available quality variants.
    fn variants(&self) -> &[StreamVariant];

    /// Force a specific quality variant (disables adaptive selection).
    fn set_variant(&mut self, variant_index: usize) -> Result<(), MediaError>;

    /// Re-enable adaptive bitrate selection.
    fn set_adaptive(&mut self) -> Result<(), MediaError>;

    /// Get current bandwidth estimate.
    fn bandwidth_estimate(&self) -> u64;

    /// Get current buffer level.
    fn buffer_level(&self) -> Duration;
}

/// WebRTC real-time communication session.
///
/// Provides ICE/DTLS/RTP negotiation, low-latency encode/decode,
/// and congestion control for video calls and screen sharing.
pub trait RtcSession {
    /// The session's unique identifier.
    fn id(&self) -> RtcSessionId;

    /// Current connection state.
    fn state(&self) -> RtcConnectionState;

    /// Create an SDP offer for initiating a call.
    fn create_offer(&mut self) -> Result<String, MediaError>;

    /// Set the local SDP offer or answer.
    fn set_local_description(&mut self, sdp: &str) -> Result<(), MediaError>;

    /// Set the remote SDP offer or answer.
    fn set_remote_description(&mut self, sdp: &str) -> Result<(), MediaError>;

    /// Add an ICE candidate.
    fn add_ice_candidate(&mut self, candidate: &str) -> Result<(), MediaError>;

    /// Add a local video track from Camera Kit.
    fn add_video_track(&mut self, camera_session: CameraSessionHandle)
        -> Result<TrackId, MediaError>;

    /// Add a local audio track from Audio Kit.
    fn add_audio_track(&mut self, audio_session: AudioSessionHandle)
        -> Result<TrackId, MediaError>;

    /// Enable screen sharing as a video track.
    fn add_screen_share(&mut self, display: DisplayId) -> Result<TrackId, MediaError>;

    /// Register a callback for incoming remote tracks.
    fn on_remote_track(&mut self, handler: RemoteTrackHandler);

    /// Close the RTC session.
    fn close(self) -> Result<(), MediaError>;
}

/// Container format identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerFormat {
    Mp4,
    WebM,
    Mkv,
    MpegTs,
    Ogg,
    Wav,
    Flac,
}

/// Pipeline state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineState {
    /// Pipeline constructed but not started.
    Ready,
    /// Actively processing media.
    Playing,
    /// Paused with buffers preserved.
    Paused,
    /// Stopped; resources released.
    Stopped,
    /// An unrecoverable error occurred.
    Error,
}
```

## 3. Usage Patterns

**Minimal -- play a local audio file:**

```rust
use aios_media::MediaKit;

// MediaKit creates a complete pipeline: file source -> demuxer -> decoder -> Audio Kit
let mut pipeline = MediaKit::open("space://user/home/music/song.flac")?;
pipeline.play()?;

// Wait for playback to complete
while pipeline.state() != PipelineState::Stopped {
    aios_runtime::yield_now().await;
}
```

**Realistic -- video playback with transport controls:**

```rust
use aios_media::{MediaKit, MediaMetadata, PipelineState};
use core::time::Duration;

// Open a video file -- creates video + audio decode pipelines automatically
let mut pipeline = MediaKit::open("space://user/home/videos/movie.mkv")?;

// Create a media session for system integration (Now Playing, media keys)
let mut session = MediaKit::create_session()?;
session.set_metadata(&MediaMetadata {
    title: "My Movie".into(),
    artist: None,
    album: None,
    artwork: None,
    duration: pipeline.duration(),
})?;

// Handle transport controls from media keys and system UI
session.on_transport_control(|control| {
    match control {
        TransportControl::Play => pipeline.play().ok(),
        TransportControl::Pause => pipeline.pause().ok(),
        TransportControl::SeekForward => {
            let pos = pipeline.position() + Duration::from_secs(10);
            pipeline.seek(pos).ok()
        }
        TransportControl::SeekBackward => {
            let pos = pipeline.position().saturating_sub(Duration::from_secs(10));
            pipeline.seek(pos).ok()
        }
        _ => None,
    };
});

pipeline.play()?;
```

**Advanced -- adaptive streaming with quality monitoring:**

```rust
use aios_media::{MediaKit, StreamingEngine, PipelineState};
use core::time::Duration;

// Open an HLS stream with adaptive bitrate
let mut stream = MediaKit::open_stream("https://example.com/live/stream.m3u8")?;

// Inspect available quality variants
let variants = stream.variants();
for (i, v) in variants.iter().enumerate() {
    log::info!("Variant {}: {}x{} @ {} kbps", i, v.width, v.height, v.bitrate / 1000);
}

// Let the engine choose quality automatically based on bandwidth
stream.set_adaptive()?;

let mut pipeline = stream.into_pipeline()?;
pipeline.play()?;

// Periodically log streaming health
loop {
    aios_runtime::sleep(Duration::from_secs(5)).await;
    let bw = stream.bandwidth_estimate();
    let buf = stream.buffer_level();
    log::info!("Bandwidth: {} kbps, Buffer: {:?}", bw / 1000, buf);

    if pipeline.state() == PipelineState::Stopped {
        break;
    }
}
```

> **Common Mistakes**
>
> - **Not handling `PipelineState::Error`.** Pipeline errors (corrupt file, network failure,
>   unsupported codec) transition the pipeline to `Error` state. Always check for this in
>   your event loop.
> - **Seeking during buffering.** Seeking while the pipeline is still buffering may cause a
>   stall. Check `buffer_level()` for streaming sources before seeking.
> - **Creating pipelines you never play.** Pipeline construction allocates codec instances
>   and buffers. Create pipelines only when you intend to use them.
> - **Ignoring A/V sync.** Media Kit handles A/V sync automatically. If you bypass the
>   pipeline and feed Audio Kit and the compositor separately, you are responsible for
>   synchronization -- prefer the pipeline approach.
> - **Hardcoding codec preferences.** The codec registry selects the best implementation
>   for the platform. Let Media Kit choose unless you have a specific reason to override.

## 4. Integration Examples

**Media Kit + Audio Kit -- decoded audio with custom DSP:**

```rust
use aios_media::MediaKit;
use aios_audio::{AudioKit, AudioRole, DspFilter};

// Open a media file and extract the audio pipeline
let mut pipeline = MediaKit::open("space://user/home/music/podcast.mp3")?;

// Get the Audio Kit session that Media Kit created internally
let audio_session = pipeline.audio_session()?;

// Add custom DSP filters to the audio output
let mut stream = audio_session.mixer().streams()[0].clone();
stream.set_filters(&[
    DspFilter::Equalizer(vec![
        EqBand { frequency: 100.0, gain_db: 3.0, q: 1.0 },  // boost bass
        EqBand { frequency: 3000.0, gain_db: 2.0, q: 1.0 }, // boost voice clarity
    ]),
    DspFilter::Compressor {
        threshold: -20.0,
        ratio: 4.0,
        attack_ms: 5.0,
        release_ms: 100.0,
    },
])?;

pipeline.play()?;
```

**Media Kit + Camera Kit -- record camera to file:**

```rust
use aios_media::{MediaKit, ContainerFormat, CodecId};
use aios_camera::{CameraKit, SessionIntent, CaptureFormat, PixelFormat};

// Open camera
let mut camera = CameraKit::open_session(
    CameraKit::default_camera()?,
    SessionIntent::Video,
    CaptureFormat { width: 1920, height: 1080, fps: 30, pixel_format: PixelFormat::Nv12 },
)?;

// Create a recording pipeline: camera frames -> H.264 encoder -> MP4 muxer
let mut recorder = MediaKit::create_recording(
    "space://user/home/videos/clip.mp4",
    ContainerFormat::Mp4,
)?;
recorder.add_video_input(camera.format(), CodecId::H264)?;

camera.start_capture()?;
while recording_active {
    if let Some(frame) = camera.next_frame()? {
        recorder.write_video_frame(&frame)?;
    }
}
recorder.finalize()?;
camera.close()?;
```

**Media Kit + Network Kit -- WebRTC video call:**

```rust
use aios_media::MediaKit;
use aios_camera::{CameraKit, SessionIntent, CaptureFormat, PixelFormat};
use aios_audio::{AudioKit, AudioRole};

// Create an RTC session
let mut rtc = MediaKit::create_rtc_session()?;

// Add local camera and microphone tracks
let camera = CameraKit::open_session(
    CameraKit::default_camera()?,
    SessionIntent::Video,
    CaptureFormat { width: 1280, height: 720, fps: 30, pixel_format: PixelFormat::Nv12 },
)?;
let audio = AudioKit::create_session(AudioRole::Communication)?;

rtc.add_video_track(camera.handle())?;
rtc.add_audio_track(audio.handle())?;

// Exchange SDP with the remote peer (signaling is agent-specific)
let offer = rtc.create_offer()?;
let answer = signaling_channel.exchange_sdp(&offer).await?;
rtc.set_remote_description(&answer)?;

// Handle incoming remote tracks
rtc.on_remote_track(|track| {
    match track.kind() {
        TrackKind::Video => compositor_surface.attach_video_track(track),
        TrackKind::Audio => audio_output.attach_audio_track(track),
    }
});
```

## 5. Capability Requirements

| Method | Required Capability | Notes |
| --- | --- | --- |
| `MediaKit::open` (local file) | `MediaPlayback` | Read access to the Space object |
| `MediaKit::open_stream` | `MediaPlayback` + `NetworkAccess` | Streaming requires network |
| `MediaKit::create_recording` | `MediaRecord` | Write access to the target Space |
| `MediaKit::create_rtc_session` | `MediaRtc` + `NetworkAccess` | Real-time communication |
| `RtcSession::add_video_track` | `CameraCapture` | Delegates to Camera Kit |
| `RtcSession::add_audio_track` | `AudioCapture` | Delegates to Audio Kit |
| `RtcSession::add_screen_share` | `ScreenCapture` | Compositor screen capture |
| Pipeline with DRM content | `MediaDrm` | Required for CENC-encrypted streams |

```toml
# Agent manifest example -- media player
[capabilities.required]
MediaPlayback = { reason = "Play audio and video files" }
AudioPlayback = { reason = "Audio output for media playback" }

[capabilities.optional]
NetworkAccess = { reason = "Stream media from the internet" }
MediaDrm = { reason = "Play DRM-protected content" }
```

```toml
# Agent manifest example -- video call app
[capabilities.required]
MediaRtc = { reason = "Real-time video and audio communication" }
NetworkAccess = { reason = "Connect to call servers" }
CameraCapture = { reason = "Send local video in calls" }
AudioCapture = { reason = "Send local audio in calls" }
AudioPlayback = { reason = "Play remote participant audio" }

[capabilities.optional]
ScreenCapture = { reason = "Share screen during calls" }
```

## 6. Error Handling

```rust
/// Errors returned by Media Kit operations.
#[derive(Debug)]
pub enum MediaError {
    /// The required capability was not granted.
    CapabilityDenied(Capability),

    /// The media file or stream could not be opened.
    SourceNotFound(String),

    /// The container format is not recognized.
    UnsupportedContainer(String),

    /// No codec is available for the requested format (HW and SW both missing).
    UnsupportedCodec(CodecId),

    /// The codec failed to decode a packet (corrupt data).
    DecodeError { codec: CodecId, detail: String },

    /// The codec failed to encode a frame.
    EncodeError { codec: CodecId, detail: String },

    /// A/V synchronization was lost beyond recovery threshold.
    SyncLost { audio_pts: i64, video_pts: i64 },

    /// The network stream stalled (buffer underrun).
    BufferUnderrun,

    /// DRM license acquisition failed.
    DrmLicenseError(String),

    /// DRM content decryption failed.
    DrmDecryptionError,

    /// The pipeline entered an unrecoverable error state.
    PipelineError(String),

    /// The RTC connection failed (ICE, DTLS, or network error).
    RtcConnectionFailed(String),

    /// The recording destination is full or inaccessible.
    RecordingFailed(String),

    /// The maximum number of concurrent pipelines has been reached.
    TooManyPipelines { max: u32 },
}
```

**Fallback cascade:**

| Failure | Degradation |
| --- | --- |
| Hardware codec unavailable | Software codec used (higher CPU, higher latency) |
| Hardware codec init fails | Software fallback attempted automatically |
| A/V sync drift | Video frames dropped or duplicated to re-sync; audio never skipped |
| Network bandwidth drops | ABR engine selects lower quality variant; buffer absorbs transient dips |
| DRM license unavailable | `DrmLicenseError`; non-DRM content unaffected |
| AIRS unavailable | ML-based ABR falls back to buffer-level heuristic; no quality prediction |
| Stream segment missing | Resilience engine retries; skips segment after timeout |
| Corrupt packet | Packet discarded; decoder self-recovers at next keyframe |

## 7. Platform & AI Availability

**AIRS-enhanced features (require AIRS Kit):**

| Feature | What AIRS provides | Without AIRS |
| --- | --- | --- |
| RL-based ABR | Reinforcement learning selects optimal bitrate | Buffer-level heuristic (BBA) |
| Predictive buffering | Prefetches segments based on viewing patterns | Sequential prefetch only |
| Content-aware encoding | Adapts encoding parameters to scene complexity | Fixed encoding profile |
| Audio enhancement | Neural audio upscaling for low-bitrate streams | Pass-through only |
| Smart thumbnails | Generates keyframe thumbnails with scene understanding | I-frame extraction only |

**Codec availability by platform:**

| Codec | QEMU virt | Raspberry Pi 4 | Raspberry Pi 5 | Apple Silicon |
| --- | --- | --- | --- | --- |
| H.264 decode | Software (openh264) | Hardware (V4L2) | Hardware (V4L2) | Hardware (VT) |
| H.265 decode | Software | Hardware (V4L2) | Hardware (V4L2) | Hardware (VT) |
| VP9 decode | Software (libvpx) | Hardware (V4L2) | Hardware (V4L2) | Hardware (VT) |
| AV1 decode | Software (dav1d) | Software (dav1d) | Hardware (V4L2) | Hardware (VT) |
| Opus decode | Software (opus) | Software (opus) | Software (opus) | Software (opus) |
| AAC decode | Software | Software | Software | Hardware (AudioToolbox) |
| H.264 encode | Software (openh264) | Hardware (V4L2) | Hardware (V4L2) | Hardware (VT) |

**DRM support:**

| DRM System | Status | Notes |
| --- | --- | --- |
| Widevine L3 | Software CDM | No TEE required; SD quality only |
| Widevine L1 | Platform-dependent | Requires TrustZone on ARM; HD/4K quality |
| PlayReady SL150 | Software CDM | Basic content protection |
| PlayReady SL3000 | Platform-dependent | Requires hardware security module |
| FairPlay | Apple Silicon only | Native integration via Secure Enclave |

**Implementation phase:** Phase 10+ (codec framework, container engine, playback pipeline,
A/V sync). Streaming protocols arrive in Phase 11+. WebRTC arrives in Phase 12+. DRM
integration requires Phase 13+ (security infrastructure). AI features require Phase 15+
(AIRS integration).

---

*See also: [Audio Kit](./audio.md) | [Camera Kit](./camera.md) | [Network Kit](./network.md) | [Capability Kit](../kernel/capability.md) | [Compute Kit](../kernel/compute.md) | [Storage Kit](./storage.md)*
