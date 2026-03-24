# Camera Kit

**Layer:** Platform | **Crate:** `aios_camera` | **Architecture:** [`docs/platform/camera.md`](../../platform/camera.md)

## 1. Overview

Camera Kit provides session-based camera access with hardware-enforced privacy, a composable
ISP pipeline, and zero-copy frame delivery from sensor to GPU. Every camera operation starts
with a `CaptureSession` that declares the agent's intent (viewfinder, photo, video, or scan).
Unlike audio or storage, camera access always requires explicit user approval through a
compositor-rendered prompt dialog -- there are no auto-grant rules. When a new session is
requested while another agent is already streaming, the system uses the `Prompt` conflict
policy to ask the user which agent should have access, rather than silently sharing or
preempting.

Privacy is the defining constraint of Camera Kit's design. The kernel controls the camera
indicator LED via direct MMIO -- agents cannot suppress it. Before any frame is delivered,
an anti-silent-capture check validates that the privacy indicator is active and the user's
consent is current. On hardware without a dedicated LED (VirtIO-Camera, some USB webcams),
the compositor renders an unfakeable software indicator at the highest z-order. Every frame
delivery is logged in the audit trail, creating a complete forensic record of what saw through
the camera and when.

Use Camera Kit when your agent needs to capture photos, stream video for calls, scan documents
or QR codes, or perform AR overlay processing. Do not use it for pre-recorded video playback
(use [Media Kit](./media.md)) or for accessing screen content (use the compositor's screen
capture API, which has its own separate capability). Camera Kit coordinates with
[Audio Kit](./audio.md) for A/V synchronization in video recording, and with
[Media Kit](./media.md) for encoding captured frames into container formats.

## 2. Core Traits

```rust
use aios_capability::CapabilityHandle;
use core::time::Duration;

/// A camera device driver abstraction covering UVC, CSI/MIPI, VirtIO-Camera,
/// and platform-specific cameras (Pi Camera, Apple ISP).
///
/// Drivers register with the camera subsystem at boot or hotplug time.
/// Agents never interact with CameraDevice directly -- they use CaptureSession.
pub trait CameraDevice {
    /// The device's unique identifier.
    fn id(&self) -> CameraId;

    /// Human-readable device name (e.g., "Logitech C920", "Pi Camera Module 3").
    fn name(&self) -> &str;

    /// The device type (USB/UVC, CSI/MIPI, VirtIO, platform-specific).
    fn device_type(&self) -> CameraDeviceType;

    /// List of supported capture modes (resolution, frame rate, format).
    fn supported_modes(&self) -> &[CaptureMode];

    /// Whether this device has a hardware-controlled privacy LED.
    fn has_hardware_led(&self) -> bool;

    /// Open the device for streaming at the given mode.
    fn open(&mut self, mode: &CaptureMode) -> Result<(), CameraError>;

    /// Start frame delivery.
    fn start_streaming(&mut self) -> Result<(), CameraError>;

    /// Stop frame delivery.
    fn stop_streaming(&mut self) -> Result<(), CameraError>;

    /// Close the device and release hardware resources.
    fn close(&mut self) -> Result<(), CameraError>;
}

/// A capture session with declared intent and user-approved access.
///
/// Sessions are the only way agents interact with cameras. Creating a session
/// triggers a user-facing consent prompt. The session holds the privacy indicator
/// active for its entire lifetime.
pub trait CaptureSession {
    /// The session's unique identifier.
    fn id(&self) -> SessionId;

    /// The declared intent for this session.
    fn intent(&self) -> &SessionIntent;

    /// The negotiated capture format.
    fn format(&self) -> &CaptureFormat;

    /// Current session state (configuring, streaming, paused, closing).
    fn state(&self) -> CaptureSessionState;

    /// Start capturing frames. Privacy indicator activates.
    fn start_capture(&mut self) -> Result<(), CameraError>;

    /// Pause capture. Privacy indicator remains active but no frames are delivered.
    fn pause(&mut self) -> Result<(), CameraError>;

    /// Resume a paused capture.
    fn resume(&mut self) -> Result<(), CameraError>;

    /// Read the next captured frame. Returns None if no frame is ready.
    fn next_frame(&mut self) -> Result<Option<CaptureFrame>, CameraError>;

    /// Reconfigure the session (e.g., change resolution mid-stream).
    fn reconfigure(&mut self, format: &CaptureFormat) -> Result<(), CameraError>;

    /// End the session, release hardware, deactivate privacy indicator.
    fn close(self) -> Result<(), CameraError>;
}

/// Declared purpose for camera access. Used for conflict resolution
/// and behavioral monitoring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionIntent {
    /// Live viewfinder preview (low latency, medium resolution).
    Viewfinder,
    /// Still photo capture (full resolution, single shot or burst).
    Photo,
    /// Video recording or video call (sustained streaming).
    Video,
    /// Document or QR code scanning (lower resolution, focused processing).
    Scan,
    /// Augmented reality overlay (low latency, GPU-bound processing).
    AugmentedReality,
}

/// Image signal processing pipeline for raw sensor data.
///
/// The ISP pipeline transforms raw Bayer sensor data into processed RGB/YUV
/// frames. On platforms with hardware ISP (Apple ISP, Pi Camera), the pipeline
/// maps to hardware blocks. On other platforms, software implementations are used.
pub trait IspPipeline {
    /// Configure the ISP for a given sensor mode and output format.
    fn configure(
        &mut self,
        input: &SensorMode,
        output: &CaptureFormat,
    ) -> Result<(), CameraError>;

    /// Enable or disable auto-exposure.
    fn set_auto_exposure(&mut self, enabled: bool) -> Result<(), CameraError>;

    /// Enable or disable auto-white-balance.
    fn set_auto_white_balance(&mut self, enabled: bool) -> Result<(), CameraError>;

    /// Enable or disable auto-focus (if hardware supports it).
    fn set_auto_focus(&mut self, enabled: bool) -> Result<(), CameraError>;

    /// Set exposure compensation (-2.0 to +2.0 EV).
    fn set_exposure_compensation(&mut self, ev: f32) -> Result<(), CameraError>;

    /// Get current 3A (auto-exposure, auto-white-balance, auto-focus) state.
    fn three_a_state(&self) -> ThreeAState;

    /// Enable noise reduction (strength 0.0 = off, 1.0 = maximum).
    fn set_noise_reduction(&mut self, strength: f32) -> Result<(), CameraError>;
}

/// Hardware-enforced privacy indicator state.
///
/// The privacy indicator is controlled by the kernel, not by agents.
/// This trait is used by the camera subsystem internally and by the
/// compositor for rendering the software indicator.
pub trait PrivacyIndicator {
    /// Whether the hardware LED is currently active.
    fn led_active(&self) -> bool;

    /// Whether the software (compositor) indicator is active.
    fn software_indicator_active(&self) -> bool;

    /// The agent that currently holds camera access (for display in indicator).
    fn active_agent(&self) -> Option<AgentId>;

    /// The declared intent of the active session (for display in indicator).
    fn active_intent(&self) -> Option<&SessionIntent>;
}

/// Capture format descriptor.
#[derive(Debug, Clone)]
pub struct CaptureFormat {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub pixel_format: PixelFormat,
}

/// A single captured frame with metadata.
#[derive(Debug)]
pub struct CaptureFrame {
    /// Presentation timestamp (monotonic clock).
    pub timestamp: u64,
    /// Frame sequence number within the session.
    pub sequence: u64,
    /// Pixel data (zero-copy DMA buffer when possible).
    pub data: FrameBuffer,
    /// Frame format.
    pub format: CaptureFormat,
    /// ISP metadata (exposure, white balance, focus distance).
    pub metadata: FrameMetadata,
}

/// Device type classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CameraDeviceType {
    /// USB Video Class device (webcams, document cameras).
    UsbUvc,
    /// CSI/MIPI-connected sensor (Pi Camera, mobile cameras).
    CsiMipi,
    /// Virtual camera for testing and QEMU.
    VirtioCamera,
    /// Platform-specific (Apple ISP).
    Platform(String),
}
```

## 3. Usage Patterns

**Minimal -- take a single photo:**

```rust
use aios_camera::{CameraKit, SessionIntent, CaptureFormat, PixelFormat};

// List available cameras
let cameras = CameraKit::enumerate_devices()?;
let camera_id = cameras[0].id();

// Open a photo session (triggers user consent prompt)
let mut session = CameraKit::open_session(
    camera_id,
    SessionIntent::Photo,
    CaptureFormat {
        width: 1920,
        height: 1080,
        fps: 30,
        pixel_format: PixelFormat::Jpeg,
    },
)?;

// Capture a single frame
session.start_capture()?;
let frame = session.next_frame()?.expect("frame available");
save_to_space("space://user/home/photos/capture.jpg", &frame.data)?;
session.close()?;
```

**Realistic -- video call viewfinder with preview:**

```rust
use aios_camera::{CameraKit, SessionIntent, CaptureFormat, PixelFormat};

// Open a video session for a call
let mut session = CameraKit::open_session(
    CameraKit::default_camera()?,
    SessionIntent::Video,
    CaptureFormat {
        width: 1280,
        height: 720,
        fps: 30,
        pixel_format: PixelFormat::Nv12,
    },
)?;

session.start_capture()?;

// Stream frames to the video call encoder
loop {
    match session.next_frame()? {
        Some(frame) => {
            // Send frame to compositor for local preview
            compositor_surface.submit_frame(&frame)?;
            // Send frame to Media Kit encoder for network transmission
            video_encoder.encode_frame(&frame)?;
        }
        None => {
            // No frame ready yet; yield and retry
            aios_runtime::yield_now().await;
        }
    }
}
```

**Advanced -- document scanning with ISP tuning:**

```rust
use aios_camera::{CameraKit, SessionIntent, CaptureFormat, PixelFormat};

let mut session = CameraKit::open_session(
    CameraKit::default_camera()?,
    SessionIntent::Scan,
    CaptureFormat {
        width: 3264,
        height: 2448,
        fps: 15,
        pixel_format: PixelFormat::Rgb24,
    },
)?;

// Tune ISP for document scanning: disable auto-focus hunting, boost sharpness
let isp = session.isp_pipeline()?;
isp.set_auto_focus(false)?;
isp.set_exposure_compensation(0.5)?; // slightly brighter for paper
isp.set_noise_reduction(0.3)?;       // light NR to preserve text edges

session.start_capture()?;
let frame = session.next_frame()?.expect("frame available");

// Process the document frame (OCR, perspective correction, etc.)
let document = process_document_scan(&frame)?;
session.close()?;
```

> **Common Mistakes**
>
> - **Not closing sessions.** Open sessions hold the camera hardware exclusively and keep
>   the privacy indicator active. Always close sessions when done. Use RAII wrappers or
>   `defer!` patterns.
> - **Ignoring the consent prompt.** `open_session()` blocks until the user approves or
>   denies. It returns `CameraError::ConsentDenied` if the user declines. Do not retry
>   immediately -- respect the user's decision.
> - **Requesting higher resolution than needed.** Higher resolution consumes more memory,
>   bandwidth, and power. A video call rarely needs more than 720p. A QR scanner works
>   well at 640x480.
> - **Assuming frame availability.** `next_frame()` returns `None` when no frame is ready.
>   Use async polling or the frame-ready callback, not a tight spin loop.
> - **Accessing ISP on devices without one.** USB webcams handle ISP internally. Check
>   `session.has_isp_pipeline()` before calling `isp_pipeline()`.

## 4. Integration Examples

**Camera Kit + Media Kit -- record video with audio:**

```rust
use aios_camera::{CameraKit, SessionIntent, CaptureFormat, PixelFormat};
use aios_audio::{AudioKit, AudioRole};
use aios_media::{MediaKit, RecordingPipeline, ContainerFormat};

// Open camera and microphone
let mut camera = CameraKit::open_session(
    CameraKit::default_camera()?,
    SessionIntent::Video,
    CaptureFormat { width: 1920, height: 1080, fps: 30, pixel_format: PixelFormat::Nv12 },
)?;
let mut audio_session = AudioKit::create_session(AudioRole::Capture)?;
let mut mic = audio_session.open_capture(AudioFormat::voice_default())?;

// Create a recording pipeline that muxes video + audio into MP4
let mut recorder = MediaKit::create_recording(
    "space://user/home/videos/recording.mp4",
    ContainerFormat::Mp4,
)?;

camera.start_capture()?;

while recording_active {
    if let Some(frame) = camera.next_frame()? {
        recorder.write_video_frame(&frame)?;
    }
    let audio_bytes = mic.read(&mut audio_buffer)?;
    if audio_bytes > 0 {
        recorder.write_audio_samples(&audio_buffer[..audio_bytes])?;
    }
}

recorder.finalize()?;
camera.close()?;
audio_session.close()?;
```

**Camera Kit + Conversation Kit -- visual context for AI:**

```rust
use aios_camera::{CameraKit, SessionIntent, CaptureFormat, PixelFormat};
use aios_conversation::ConversationKit;

let mut session = CameraKit::open_session(
    CameraKit::default_camera()?,
    SessionIntent::Scan,
    CaptureFormat { width: 1280, height: 720, fps: 5, pixel_format: PixelFormat::Jpeg },
)?;

session.start_capture()?;
let frame = session.next_frame()?.expect("frame");

// Send the captured frame to the conversation engine for visual understanding
let conversation = ConversationKit::current_session()?;
conversation.attach_image(&frame.data, "What do you see in this image?")?;
let response = conversation.get_response().await?;

session.close()?;
```

**Camera Kit + Flow Kit -- stream frames to clipboard:**

```rust
use aios_camera::{CameraKit, SessionIntent};
use aios_flow::FlowKit;

let mut session = CameraKit::open_session(
    CameraKit::default_camera()?,
    SessionIntent::Photo,
    CaptureFormat { width: 1920, height: 1080, fps: 30, pixel_format: PixelFormat::Jpeg },
)?;

session.start_capture()?;
let frame = session.next_frame()?.expect("frame");

// Push the captured photo to the Flow clipboard channel
FlowKit::clipboard().push_image(frame.data.as_bytes(), "image/jpeg")?;

session.close()?;
```

## 5. Capability Requirements

| Method | Required Capability | Notes |
| --- | --- | --- |
| `CameraKit::enumerate_devices` | `CameraEnumerate` | Lists cameras without accessing them |
| `CameraKit::open_session(Viewfinder)` | `CameraCapture` | Always prompts user for consent |
| `CameraKit::open_session(Photo)` | `CameraCapture` | Always prompts user for consent |
| `CameraKit::open_session(Video)` | `CameraCapture` + `CameraRecord` | Video recording requires additional cap |
| `CameraKit::open_session(Scan)` | `CameraCapture` | Lower-privilege scan mode |
| `CaptureSession::next_frame` | (checked at session creation) | No per-frame capability check |
| `IspPipeline::configure` | `CameraCapture` | Part of active session |

```toml
# Agent manifest example
[capabilities.required]
CameraCapture = { reason = "Capture photos for document scanning" }

[capabilities.optional]
CameraRecord = { reason = "Record video clips for user review" }
```

## 6. Error Handling

```rust
/// Errors returned by Camera Kit operations.
#[derive(Debug)]
pub enum CameraError {
    /// The required capability was not granted.
    CapabilityDenied(Capability),

    /// The user denied camera access in the consent prompt.
    ConsentDenied,

    /// No camera device is available on this system.
    NoCameraDevice,

    /// The specified camera device was not found.
    DeviceNotFound(CameraId),

    /// The camera is already in use by another session.
    DeviceBusy { current_agent: AgentId },

    /// The requested capture format is not supported by the device.
    UnsupportedFormat(CaptureFormat),

    /// The privacy indicator could not be activated (hardware failure).
    /// Capture is blocked when the indicator cannot be verified.
    PrivacyIndicatorFailure,

    /// The session was preempted by a higher-priority session.
    SessionPreempted,

    /// A frame was dropped due to processing backpressure.
    FrameDropped { sequence: u64 },

    /// The ISP pipeline is not available on this device.
    NoIspPipeline,

    /// The session has already been closed.
    SessionClosed,

    /// A hardware driver error occurred.
    DeviceError(String),
}
```

**Fallback cascade:**

| Failure | Degradation |
| --- | --- |
| No hardware camera | `NoCameraDevice` error; no silent fallback |
| Hardware LED broken | Software indicator in compositor; capture still allowed |
| ISP hardware unavailable | Software ISP (slower, higher CPU usage) |
| AIRS unavailable | ML scene understanding disabled; basic capture works |
| USB camera disconnected mid-session | `DeviceError`; session transitions to `Closing` state |
| Frame processing too slow | Frames dropped with `FrameDropped` warning; stream continues |

## 7. Platform & AI Availability

**AIRS-enhanced features (require AIRS Kit):**

| Feature | What AIRS provides | Without AIRS |
| --- | --- | --- |
| Scene understanding | Neural scene classification (indoor, outdoor, portrait, document) | No classification |
| Smart framing | AI-driven auto-crop and reframing for video calls | Fixed center crop |
| Computational photography | Neural HDR, portrait mode with depth estimation | Basic multi-exposure HDR |
| Gesture recognition | Camera-based hand gesture interpretation | No gesture recognition |
| Anomaly detection | Detects mismatch between declared intent and actual capture behavior | Static policy checks only |

**Platform availability:**

| Platform | USB/UVC | CSI/MIPI | VirtIO-Camera | Hardware ISP | Hardware LED | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| QEMU virt | N/A | N/A | Software emulated | No | Software only | Testing only |
| Raspberry Pi 4 | Yes | Pi Camera v2 | N/A | No (software ISP) | Software only | USB webcams recommended |
| Raspberry Pi 5 | Yes | Pi Camera v3 | N/A | No (software ISP) | Software only | Improved CSI lanes |
| Apple Silicon | Yes | N/A | N/A | Apple ISP | Hardware LED | Full experience |

**Feature detection at runtime:**

```rust
use aios_camera::CameraKit;

// Check if any camera is available
if CameraKit::enumerate_devices()?.is_empty() {
    // No camera hardware -- disable camera features in UI
    return Ok(());
}

// Check for hardware ISP support
let camera = CameraKit::default_camera()?;
let has_isp = camera.device_type() != CameraDeviceType::UsbUvc;

// Check for AIRS-enhanced features
let has_scene_ai = aios_airs::is_available()
    && aios_airs::has_model("camera-scene-classifier");
```

**Implementation phase:** Phase 10+ (session lifecycle, capture pipeline, UVC driver, ISP
framework, privacy indicators). CSI/MIPI drivers arrive with BSP phases. AI features
require Phase 15+ (AIRS integration).

---

*See also: [Audio Kit](./audio.md) | [Media Kit](./media.md) | [Capability Kit](../kernel/capability.md) | [Memory Kit](../kernel/memory.md) | [Compute Kit](../kernel/compute.md)*
