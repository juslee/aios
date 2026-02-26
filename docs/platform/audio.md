# AIOS Audio Subsystem

## Deep Technical Architecture

**Parent document:** [architecture.md](../project/architecture.md)
**Related:** [subsystem-framework.md](./subsystem-framework.md) — Universal hardware abstraction (capability gate, sessions, data channels, audit, power, POSIX bridge), [scheduler.md](../kernel/scheduler.md) — RT scheduling class and audio deadline (§5.2), [hal.md](../kernel/hal.md) — `PlatformAudio` and `PlatformPwm` extension traits (§12), [compositor.md](./compositor.md) — A/V sync and presentation timestamps, [networking.md](./networking.md) — Companion subsystem implementation

**Note:** The audio subsystem implements the subsystem framework. Its capability gate, session model, audit logging, power management, and POSIX bridge follow the universal patterns defined in the framework document. This document covers the audio-specific design decisions and architecture.

-----

## 1. Overview

Audio is a system subsystem following the universal Subsystem Framework pattern. It manages the complete audio path — from agents producing or consuming PCM samples, through a software mixing engine, down to hardware-specific drivers that emit or capture sound.

Audio has a unique constraint among subsystems: **time sensitivity is absolute.** A network packet delayed by 50ms is invisible to the user. A video frame delayed by 50ms is a noticeable stutter. An audio sample delayed by 5ms is an audible glitch. Audio is the most timing-critical data path in the entire OS, and the subsystem is designed around this reality.

The audio subsystem provides:

1. **PCM mixing engine** — a software mixer that combines streams from multiple agents into a single output, applying per-stream volume, format conversion, and sample-rate conversion.
2. **Platform-specific drivers** — VirtIO-Sound for QEMU development, I2S for HiFiBerry-style DACs on Raspberry Pi, PWM audio for the Pi headphone jack, HDMI audio for monitors and TVs, and hardware codecs on Apple Silicon.
3. **Capability-gated sessions** — agents request `AudioPlayback` or `AudioCapture` capabilities; the kernel capability gate enforces access before any audio flows.
4. **RT scheduling integration** — audio mixing callbacks run in the scheduler's Real-Time class with EDF (Earliest Deadline First), guaranteeing that audio never starves (see [scheduler.md](../kernel/scheduler.md) §5.2).
5. **A/V synchronization** — a shared timeline with the compositor for synchronized audio-video presentation.

**Design principle:** Audio is infrastructure that agents use, not an application. An agent that wants to play a notification sound, stream music, run a voice call, or perform text-to-speech uses the same session API. The subsystem handles mixing, routing, format negotiation, device switching, and latency management transparently.

-----

## 2. Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                        Agent Layer                                │
│                                                                   │
│  Media Player   Conversation Bar   Notification   Game Agent     │
│  (music stream) (TTS + STT)        (alert sounds) (spatial audio)│
│                                                                   │
│  Each agent holds an AudioPlayback and/or AudioCapture capability │
└───────────────────────┬──────────────────────────────────────────┘
                        │ IPC: open_session, write_samples, close
                        ▼
┌──────────────────────────────────────────────────────────────────┐
│                    Audio Subsystem Service                         │
│                      (userspace process)                           │
│                                                                   │
│  ┌──────────────┐  ┌───────────────┐  ┌────────────────────┐    │
│  │  Session      │  │   PCM Mixing  │  │   Format           │    │
│  │  Manager      │  │   Engine      │  │   Negotiation      │    │
│  │              │  │               │  │                    │    │
│  │  open/close  │  │  mix N → 1   │  │  sample rate       │    │
│  │  capability  │  │  per-stream   │  │  channel layout    │    │
│  │  gate check  │  │  volume/pan   │  │  bit depth         │    │
│  │  intent      │  │  SRC          │  │  format conversion │    │
│  └──────────────┘  └───────────────┘  └────────────────────┘    │
│                                                                   │
│  ┌──────────────┐  ┌───────────────┐  ┌────────────────────┐    │
│  │  Route        │  │   Capture     │  │   A/V Sync         │    │
│  │  Manager      │  │   Engine      │  │   Controller       │    │
│  │              │  │               │  │                    │    │
│  │  device      │  │  mic → agents │  │  shared clock      │    │
│  │  selection   │  │  echo cancel  │  │  presentation ts   │    │
│  │  hotplug     │  │  gain ctrl    │  │  compositor link   │    │
│  │  fallback    │  │  multiplex    │  │  lip-sync offset   │    │
│  └──────────────┘  └───────────────┘  └────────────────────┘    │
│                                                                   │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │                    POSIX Bridge                              │ │
│  │  /dev/audio0 (playback)  /dev/audioin0 (capture)            │ │
│  │  /dev/mixer0 (volume)    /dev/dsp (OSS compat)              │ │
│  └─────────────────────────────────────────────────────────────┘ │
└───────────────────────┬──────────────────────────────────────────┘
                        │
                        ▼
┌──────────────────────────────────────────────────────────────────┐
│                    Device Abstraction Layer                        │
│                                                                   │
│  trait AudioDevice: DeviceClass                                   │
│  ├── supported_formats() → Vec<AudioFormat>                       │
│  ├── open_output(format) → PcmOutputStream                       │
│  ├── open_input(format) → PcmInputStream                         │
│  ├── set_volume(f32)                                              │
│  └── properties() → &DeviceProperties                            │
└───────────────────────┬──────────────────────────────────────────┘
                        │
                        ▼
┌──────────────────────────────────────────────────────────────────┐
│                     Hardware Drivers                               │
│                                                                   │
│  VirtIO-Sound     │  I2S (HiFiBerry)  │  PWM Audio  │  HDMI     │
│  (QEMU)           │  (Pi 4/5 DAC)     │  (Pi 3.5mm) │  (Pi/Mac) │
│                   │                   │             │           │
│  Apple CoreAudio  │  USB Audio Class  │  Bluetooth  │           │
│  (M1-M4 codec)    │  (USB headsets)   │  A2DP/HFP   │           │
└──────────────────────────────────────────────────────────────────┘
        ↕                       ↕
   Capability Gate           Audit Space
   (kernel-enforced)         (system/audit/audio/)
```

-----

## 3. Audio Subsystem Implementation

The audio subsystem implements the framework's `Subsystem` trait with audio-specific types.

### 3.1 Subsystem Registration

```rust
pub struct AudioSubsystem {
    /// All known audio devices (output + input)
    devices: Vec<Box<dyn AudioDevice>>,

    /// The software PCM mixer (see §4)
    mixer: PcmMixer,

    /// The capture multiplexer (see §4.5)
    capture_mux: CaptureMux,

    /// Active sessions (playback + capture)
    sessions: HashMap<SessionId, AudioSession>,

    /// Route table: which device handles which role
    routes: RouteTable,

    /// A/V sync controller (see §7)
    av_sync: AvSyncController,

    /// Power state tracker per device
    power: HashMap<DeviceId, PowerState>,
}

impl Subsystem for AudioSubsystem {
    const ID: SubsystemId = "audio";
    type Capability = AudioCapability;
    type Device = Box<dyn AudioDevice>;
    type Session = AudioSession;
    type AuditEvent = AudioAuditEvent;

    fn init(&mut self, registry: &DeviceRegistry) -> Result<()> {
        // Discover audio devices via HAL
        // On QEMU: VirtIO-Sound device
        // On Pi: HDMI audio, I2S (if DAC connected), PWM
        // On Apple Silicon: built-in codec
        let devices = hal::platform().init_audio(&device_tree)?;
        for dev in devices {
            let id = registry.register(dev.descriptor())?;
            self.devices.push(dev);
            self.routes.add_default(id);
        }

        // Initialize the PCM mixer with the default output
        let default_out = self.routes.default_output()?;
        let format = default_out.preferred_format();
        self.mixer.init(format)?;

        Ok(())
    }

    fn open_session(
        &self,
        agent: AgentId,
        cap: &AudioCapability,
        intent: &SessionIntent,
    ) -> Result<AudioSession> {
        // 1. Kernel capability gate (cannot be bypassed)
        gate_check(agent, Self::ID, cap, intent)?;

        // 2. Select device
        let device = match &cap.target {
            AudioTarget::Default => self.routes.default_for(intent.direction)?,
            AudioTarget::Specific(id) => self.devices.get(id)?,
            AudioTarget::Role(role) => self.routes.device_for_role(role)?,
        };

        // 3. Conflict resolution
        let active = self.sessions_for_device(device.id());
        match self.conflict_policy(intent.direction).resolve(&active, intent) {
            ConflictResolution::Share => { /* proceed — mixer handles it */ }
            ConflictResolution::Deny => return Err(AudioError::DeviceBusy),
            ConflictResolution::Prompt(msg) => {
                user::prompt_blocking(msg)?;
            }
            _ => {}
        }

        // 4. Negotiate format
        let format = negotiate_audio_format(
            &cap.preferred_format,
            device.supported_formats(),
        )?;

        // 5. Create data channel
        let channel = match intent.direction {
            DataDirection::Consume => {
                // Playback: agent writes samples → mixer → device
                self.mixer.add_playback_stream(format, intent.priority)?
            }
            DataDirection::Produce => {
                // Capture: device → capture mux → agent reads samples
                self.capture_mux.add_capture_stream(device, format, agent)?
            }
            DataDirection::Both => {
                // Full-duplex (voice calls)
                AudioChannel::Duplex {
                    playback: self.mixer.add_playback_stream(format, intent.priority)?,
                    capture: self.capture_mux.add_capture_stream(
                        self.routes.default_input()?, format, agent,
                    )?,
                }
            }
        };

        let session = AudioSession {
            id: new_session_id(),
            agent,
            device_id: device.id(),
            channel,
            format,
            intent: intent.clone(),
            started_at: now(),
            samples_processed: 0,
            peak_level: 0.0,
        };

        // 6. Audit
        self.audit(AudioAuditEvent::SessionOpened {
            session_id: session.id,
            agent,
            device: device.id(),
            direction: intent.direction,
            format,
            purpose: intent.purpose.clone(),
        });

        Ok(session)
    }

    fn device_added(&mut self, desc: HardwareDescriptor) -> Result<Self::Device> {
        let device = match desc.bus {
            Bus::USB => usb_audio::create_device(desc)?,
            Bus::Bluetooth => bluetooth_audio::create_device(desc)?,
            Bus::Virtual => virtio_sound::create_device(desc)?,
            Bus::Platform => platform_audio::create_device(desc)?,
            _ => return Err(AudioError::UnsupportedBus(desc.bus)),
        };

        // Auto-route: if this is the first device of its kind, make it default
        if self.routes.default_output().is_err() && device.supports_output() {
            self.routes.set_default_output(device.id());
        }

        Ok(device)
    }

    fn device_removed(&mut self, device_id: DeviceId) -> Result<()> {
        // Close all sessions using this device
        let affected: Vec<SessionId> = self.sessions.values()
            .filter(|s| s.device_id == device_id)
            .map(|s| s.id)
            .collect();

        for session_id in affected {
            self.close_session_forced(session_id, Reason::DeviceRemoved)?;
        }

        // Reroute to fallback device if this was the default
        if self.routes.is_default(device_id) {
            if let Some(fallback) = self.routes.next_available_output() {
                self.routes.set_default_output(fallback);
                self.mixer.reroute(fallback)?;
            }
        }

        Ok(())
    }

    fn posix_bridge(&self) -> &dyn PosixBridge {
        &self.posix  // maps /dev/audio*, /dev/audioin*, /dev/mixer*, /dev/dsp
    }

    fn audit_space(&self) -> SpacePath {
        SpacePath::system("audit/audio")
    }
}
```

### 3.2 Audio Capabilities

```rust
/// Capability token for audio access.
/// Agents declare required audio capabilities in their manifest.
/// The capability gate verifies these before any session opens.
pub enum AudioCapability {
    /// Permission to play audio through an output device
    Playback {
        /// Which device (default, specific, or by role)
        target: AudioTarget,
        /// Maximum number of concurrent streams
        max_streams: u8,
        /// Maximum volume (0.0 - 1.0). System agents get 1.0,
        /// background agents may be capped at 0.5
        max_volume: f32,
        /// Whether exclusive (bypass mixer) access is permitted
        exclusive: bool,
    },

    /// Permission to capture audio from an input device
    Capture {
        /// Which device (default mic, specific device)
        target: AudioTarget,
        /// Maximum sample rate (prevents high-fidelity eavesdropping
        /// for agents that only need voice-quality audio)
        max_sample_rate: u32,
        /// Maximum capture duration per session
        max_duration: Option<Duration>,
    },

    /// Permission for both playback and capture (voice calls, echo cancellation)
    Duplex {
        playback: Box<AudioCapability>,
        capture: Box<AudioCapability>,
    },
}

pub enum AudioTarget {
    /// System chooses the best available device
    Default,
    /// A specific device by ID
    Specific(DeviceId),
    /// A device filling a specific role
    Role(AudioRole),
}

pub enum AudioRole {
    /// Primary speaker/headphone output
    SystemOutput,
    /// Notification sounds (may be a different device)
    Notification,
    /// Voice communication (headset preferred)
    Communication,
    /// Primary microphone
    SystemInput,
    /// Dedicated monitor speakers (HDMI)
    Monitor,
}
```

### 3.3 Audio Sessions

```rust
pub struct AudioSession {
    pub id: SessionId,
    pub agent: AgentId,
    pub device_id: DeviceId,
    pub channel: AudioChannel,
    pub format: AudioFormat,
    pub intent: SessionIntent,
    pub capability: AudioCapability,
    pub started_at: Timestamp,
    pub samples_processed: u64,
    pub peak_level: f32,
}

impl DeviceSession for AudioSession {
    fn agent(&self) -> AgentId { self.agent }
    fn capability(&self) -> &dyn Capability { &self.capability }
    fn started_at(&self) -> Timestamp { self.started_at }
    fn intent(&self) -> &SessionIntent { &self.intent }

    fn channel(&self) -> &dyn DataChannel {
        match &self.channel {
            AudioChannel::Playback(ch) => ch,
            AudioChannel::Capture(ch) => ch,
            AudioChannel::Duplex { playback, .. } => playback,
        }
    }

    fn close(self) -> Result<SessionSummary> {
        let duration = now() - self.started_at;
        let summary = SessionSummary {
            session_id: self.id,
            duration,
            data_transferred: self.samples_processed * self.format.bytes_per_sample() as u64,
        };

        // Audit the close
        audit(AudioAuditEvent::SessionClosed {
            session_id: self.id,
            agent: self.agent,
            duration,
            samples_processed: self.samples_processed,
            peak_level: self.peak_level,
        });

        Ok(summary)
    }
}
```

### 3.4 Conflict Resolution

Audio output uses a **Share** policy — multiple agents can play simultaneously through the mixer. Audio input (microphone) uses a **Prompt** policy — the user must consent to microphone access, and concurrent capture requires explicit approval.

```rust
impl AudioSubsystem {
    fn conflict_policy(&self, direction: DataDirection) -> Box<dyn ConflictPolicy> {
        match direction {
            // Playback: always share via mixer
            DataDirection::Consume => Box::new(AlwaysSharePolicy),

            // Capture: prompt user if another agent is already capturing
            DataDirection::Produce => Box::new(PromptOnConflictPolicy {
                message: "An agent is requesting microphone access while \
                          another agent is already listening. Allow?",
            }),

            // Duplex: prompt for capture portion
            DataDirection::Both => Box::new(PromptOnConflictPolicy {
                message: "An agent is requesting microphone access for \
                          a voice session. Allow?",
            }),
        }
    }
}
```

-----

## 4. PCM Mixing Engine

The mixer is the heart of the audio subsystem. It combines PCM streams from multiple agents into a single output stream sent to the hardware device. The mixer runs as an RT-class thread with hard deadlines.

### 4.1 Mixer Architecture

```
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
}
```

### 4.3 Mix Callback (RT Thread)

The mix callback runs in the scheduler's Real-Time class. It is invoked at a fixed period (default: 5ms / 200 Hz) and must complete within its WCET budget (0.5ms). See [scheduler.md](../kernel/scheduler.md) §5.2 for the RT admission parameters.

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

Audio capture (microphone input) is the reverse path. The hardware driver fills a ring buffer with PCM samples; the capture engine distributes those samples to all agents that hold active capture sessions.

```rust
pub struct CaptureMux {
    /// Active capture streams (one per agent session)
    streams: Vec<CaptureStream>,
    /// Hardware input ring buffer
    hardware_ring: RingBuffer<f32>,
    /// Echo canceller (if playback is also active)
    echo_canceller: Option<EchoCanceller>,
    /// Automatic gain control
    agc: AutoGainControl,
}

struct CaptureStream {
    id: StreamId,
    agent: AgentId,
    session: SessionId,
    /// Ring buffer: capture engine writes, agent reads
    ring: RingBuffer<f32>,
    /// Target format for this agent's session
    target_format: AudioFormat,
    /// Sample rate converter (if hardware rate != agent rate)
    src: Option<SampleRateConverter>,
}

/// Called by the RT scheduler at the capture period.
/// Registered as RT task: period = 5ms, wcet = 0.3ms, deadline = 5ms
fn capture_callback(capture: &mut CaptureMux) {
    let frames = capture.hardware_ring.available();
    let mut samples = vec![0.0f32; frames]; // pre-allocated in practice
    capture.hardware_ring.read(&mut samples);

    // Apply echo cancellation (removes speaker bleed from mic)
    if let Some(aec) = &mut capture.echo_canceller {
        aec.process(&mut samples);
    }

    // Apply automatic gain control
    capture.agc.process(&mut samples);

    // Distribute to all active capture streams
    for stream in &mut capture.streams {
        // Format convert if needed
        let converted = if let Some(src) = &mut stream.src {
            src.process_capture(&samples)
        } else {
            &samples
        };

        // Write to agent's ring buffer
        stream.ring.write(converted);
    }
}
```

-----

## 5. Hardware Drivers

Each platform has different audio hardware. The HAL provides the `PlatformAudio` extension trait (see [hal.md](../kernel/hal.md) §12) that initializes the hardware at boot. The audio subsystem builds higher-level drivers on top.

### 5.1 QEMU: VirtIO-Sound

The primary development platform. VirtIO-Sound is a paravirtualized audio device defined by the VIRTIO specification. It provides virtual PCM streams without needing real audio hardware.

```rust
pub struct VirtioSoundDevice {
    /// VirtIO transport (MMIO-based on QEMU virt machine)
    transport: VirtioTransport,

    /// Control virtqueue (device configuration)
    control_vq: VirtQueue,
    /// Event virtqueue (device notifications)
    event_vq: VirtQueue,
    /// TX virtqueue (playback: host → device)
    tx_vq: VirtQueue,
    /// RX virtqueue (capture: device → host)
    rx_vq: VirtQueue,

    /// Negotiated features
    features: VirtioSoundFeatures,

    /// Available PCM streams (queried from device)
    streams: Vec<VirtioPcmStream>,

    /// Jacks (physical connectors, if emulated)
    jacks: Vec<VirtioJack>,
}

impl VirtioSoundDevice {
    /// Initialize the VirtIO-Sound device.
    /// Called during Phase 2 boot via HAL PlatformAudio trait.
    pub fn init(transport: VirtioTransport) -> Result<Self> {
        // 1. Negotiate features
        let features = transport.negotiate(
            VIRTIO_SND_F_PCM_OUTPUT | VIRTIO_SND_F_PCM_INPUT
        )?;

        // 2. Set up virtqueues
        let control_vq = transport.setup_queue(0)?;
        let event_vq = transport.setup_queue(1)?;
        let tx_vq = transport.setup_queue(2)?;
        let rx_vq = transport.setup_queue(3)?;

        // 3. Query available streams
        let streams = Self::query_pcm_streams(&control_vq)?;
        let jacks = Self::query_jacks(&control_vq)?;

        Ok(Self {
            transport, control_vq, event_vq, tx_vq, rx_vq,
            features, streams, jacks,
        })
    }

    /// Write mixed PCM samples to the device.
    /// Called from the mixer's hardware_ring flush.
    pub fn write_pcm(&self, stream_id: u32, buffer: &[u8]) -> Result<usize> {
        let desc = VirtioSndPcmXfer {
            stream_id,
            // buffer is appended as a separate descriptor in the chain
        };
        self.tx_vq.add_chain(&[
            VirtqDesc::readable(&desc),
            VirtqDesc::readable(buffer),
            VirtqDesc::writable(&status_buf), // device writes status here
        ])?;
        self.tx_vq.notify();
        Ok(buffer.len())
    }
}

impl AudioDevice for VirtioSoundDevice {
    fn name(&self) -> &str { "VirtIO Sound Device" }

    fn supported_formats(&self) -> Vec<AudioFormat> {
        self.streams.iter().map(|s| AudioFormat {
            sample_rate: s.rate,
            channels: s.channels,
            format: virtio_to_sample_format(s.format),
        }).collect()
    }

    fn open_output(&self, format: AudioFormat) -> Result<PcmOutputStream> {
        let stream_id = self.find_output_stream(&format)?;
        self.set_stream_params(stream_id, &format)?;
        self.prepare_stream(stream_id)?;
        self.start_stream(stream_id)?;
        Ok(PcmOutputStream::new(stream_id, format))
    }

    fn open_input(&self, format: AudioFormat) -> Result<PcmInputStream> {
        let stream_id = self.find_input_stream(&format)?;
        self.set_stream_params(stream_id, &format)?;
        self.prepare_stream(stream_id)?;
        self.start_stream(stream_id)?;
        Ok(PcmInputStream::new(stream_id, format))
    }

    fn set_volume(&self, volume: f32) -> Result<()> {
        // VirtIO-Sound supports per-stream volume control
        for stream in &self.streams {
            self.control_vq.send(VirtioSndCtrl::SetVolume {
                stream_id: stream.id,
                volume: (volume * 32767.0) as i16,
            })?;
        }
        Ok(())
    }
}
```

### 5.2 Raspberry Pi: I2S (HiFiBerry-Style DACs)

I2S (Inter-IC Sound) is a serial bus protocol for connecting digital audio devices. On Raspberry Pi 4 and 5, the I2S interface connects to external DAC boards (HiFiBerry DAC+, IQaudIO, etc.) for high-quality audio output.

```rust
pub struct I2sAudioDevice {
    /// I2S peripheral base address (BCM2711: 0xFE203000)
    base: *mut u8,

    /// PCM/I2S control/status register
    cs: VolatileReg<u32>,
    /// PCM/I2S FIFO data register
    fifo: VolatileReg<u32>,
    /// PCM/I2S mode register
    mode: VolatileReg<u32>,
    /// PCM/I2S receive/transmit config
    rxc: VolatileReg<u32>,
    txc: VolatileReg<u32>,

    /// DMA channel for zero-copy FIFO transfers
    dma_channel: DmaChannel,
    /// DMA control blocks (ping-pong buffer)
    dma_cb: [DmaControlBlock; 2],

    /// GPIO pins configured for I2S function
    /// Pi 4: GPIO 18 (BCLK), 19 (LRCLK), 20 (DIN), 21 (DOUT)
    pins: I2sPins,

    /// Clock source (from clock manager)
    clock: PcmClock,

    /// Active format
    format: AudioFormat,
}

impl I2sAudioDevice {
    /// Initialize I2S peripheral from device tree.
    /// The device tree node specifies:
    ///   - Base address
    ///   - Clock parent (PLLD or oscillator)
    ///   - GPIO pin assignments
    ///   - DAC type (for codec-specific initialization)
    pub fn init(dt_node: &DeviceTreeNode) -> Result<Self> {
        let base = dt_node.reg_address()?;

        // 1. Configure GPIO pins for I2S alt function
        let pins = I2sPins::configure(dt_node)?;

        // 2. Set up PCM clock
        //    Target: BCLK = sample_rate * bits_per_sample * channels
        //    For 48000 Hz, 32-bit, stereo: BCLK = 3.072 MHz
        let clock = PcmClock::init(dt_node.clock_parent()?, 3_072_000)?;

        // 3. Configure I2S registers
        let mut dev = Self { base, pins, clock, /* ... */ };
        dev.configure_mode(I2sMode::Master, 32, 2)?;
        dev.configure_dma()?;

        Ok(dev)
    }

    /// Start DMA-driven I2S output.
    /// Uses ping-pong DMA: while one buffer plays, the mixer fills the other.
    fn start_dma_output(&mut self) -> Result<()> {
        // Configure two DMA control blocks in a cycle:
        //   CB0: transfer buffer_a to I2S FIFO → next = CB1
        //   CB1: transfer buffer_b to I2S FIFO → next = CB0
        // DMA generates an interrupt when each CB completes,
        // signaling the mixer to fill the completed buffer.

        self.dma_cb[0] = DmaControlBlock {
            transfer_info: DMA_TI_SRC_INC | DMA_TI_DEST_DREQ | DMA_TI_PERMAP_PCM_TX,
            src_addr: self.buffer_a.physical_addr(),
            dst_addr: self.fifo.physical_addr(),
            transfer_len: self.buffer_size_bytes(),
            next_cb: &self.dma_cb[1] as *const _ as u32,
            ..Default::default()
        };

        self.dma_cb[1] = DmaControlBlock {
            transfer_info: DMA_TI_SRC_INC | DMA_TI_DEST_DREQ | DMA_TI_PERMAP_PCM_TX,
            src_addr: self.buffer_b.physical_addr(),
            dst_addr: self.fifo.physical_addr(),
            transfer_len: self.buffer_size_bytes(),
            next_cb: &self.dma_cb[0] as *const _ as u32,
            ..Default::default()
        };

        // Enable I2S transmit + DMA
        self.cs.write(CS_EN | CS_TXON | CS_DMAEN);
        self.dma_channel.start(&self.dma_cb[0])?;

        Ok(())
    }
}
```

### 5.3 Raspberry Pi: PWM Audio

PWM (Pulse Width Modulation) audio on the Raspberry Pi drives the 3.5mm headphone jack. It is lower quality than I2S but requires no external hardware. See [hal.md](../kernel/hal.md) §12 for the `PlatformPwm` trait.

```rust
pub struct PwmAudioDevice {
    /// PWM peripheral base (BCM2711: 0xFE20C000)
    base: *mut u8,

    /// PWM channel 0 (left) and channel 1 (right)
    channels: [PwmChannel; 2],

    /// DMA channel for continuous sample output
    dma_channel: DmaChannel,

    /// Clock divider (determines effective sample rate)
    /// PWM clock = 500 MHz / divider. For 48 kHz stereo at 10-bit:
    ///   divider = 500_000_000 / (48000 * 1024) ≈ 10
    clock_divider: u32,

    /// PWM range (bit depth). 1024 gives ~10-bit effective resolution.
    pwm_range: u32,
}

impl PwmAudioDevice {
    /// Convert f32 PCM samples to PWM duty cycle values.
    fn pcm_to_pwm(&self, samples: &[f32], output: &mut [u32]) {
        let half_range = self.pwm_range as f32 / 2.0;
        for (i, sample) in samples.iter().enumerate() {
            // Map [-1.0, 1.0] to [0, pwm_range]
            output[i] = ((sample + 1.0) * half_range) as u32;
        }
    }
}

impl AudioDevice for PwmAudioDevice {
    fn name(&self) -> &str { "Analog Audio (3.5mm)" }

    fn supported_formats(&self) -> Vec<AudioFormat> {
        vec![
            AudioFormat { sample_rate: 48000, channels: 2, format: SampleFormat::I16 },
            AudioFormat { sample_rate: 44100, channels: 2, format: SampleFormat::I16 },
        ]
    }

    fn properties(&self) -> &DeviceProperties {
        &DeviceProperties::Audio(AudioDeviceProperties {
            output_type: OutputType::AnalogHeadphone,
            max_sample_rate: 48000,
            max_channels: 2,
            effective_bit_depth: 10, // PWM limitation
            latency_ms: 10,         // higher than I2S due to PWM conversion
        })
    }
}
```

### 5.4 HDMI Audio

HDMI audio is available on all platforms with HDMI output. Audio samples are embedded into the HDMI data stream alongside video frames, using Audio InfoFrames and IEC 60958 encoding. HDMI audio routing is covered in detail in §8.

```rust
pub struct HdmiAudioDevice {
    /// HDMI controller base address
    /// Pi 4: VC4 HDMI (0xFE902000)
    /// Pi 5: VC7 HDMI
    hdmi_base: *mut u8,

    /// Audio clock regeneration unit
    audio_clock: HdmiAudioClock,

    /// EDID-reported audio capabilities of the connected display
    sink_caps: HdmiAudioCaps,

    /// Active audio infoframe
    infoframe: AudioInfoFrame,

    /// Audio sample packet buffer
    /// HDMI carries audio in 192-sample blocks (IEC 60958 frames)
    packet_buffer: Vec<Iec60958Frame>,

    /// DMA channel for audio packet insertion
    dma_channel: DmaChannel,

    /// CEC controller for volume/mute pass-through
    cec: Option<CecController>,
}

impl HdmiAudioDevice {
    /// Initialize HDMI audio from EDID and device tree.
    pub fn init(hdmi: &HdmiController, edid: &Edid) -> Result<Self> {
        // 1. Parse EDID Short Audio Descriptors
        let sink_caps = HdmiAudioCaps::from_edid(edid)?;

        // 2. Select best common format
        //    Prefer: 48 kHz stereo LPCM (universally supported)
        let format = sink_caps.best_lpcm_format()?;

        // 3. Configure audio clock regeneration (N/CTS values)
        //    N and CTS must satisfy: 128 * sample_rate = pixel_clock * N / CTS
        let audio_clock = HdmiAudioClock::configure(
            hdmi.pixel_clock(),
            format.sample_rate,
        )?;

        // 4. Set up Audio InfoFrame
        let infoframe = AudioInfoFrame {
            coding_type: AudioCoding::Lpcm,
            channel_count: format.channels,
            sample_rate: format.sample_rate,
            sample_size: format.bits_per_sample(),
            channel_allocation: ChannelAllocation::stereo(),
        };

        Ok(Self {
            hdmi_base: hdmi.base(),
            audio_clock,
            sink_caps,
            infoframe,
            packet_buffer: Vec::with_capacity(192),
            dma_channel: hdmi.audio_dma_channel()?,
            cec: CecController::init(hdmi).ok(),
        })
    }
}
```

### 5.5 Apple Silicon: Hardware Codecs

Apple Silicon Macs (M1-M4) have integrated audio codecs with hardware DSP, accessed through device tree bindings. The driver communicates with the codec via a platform-specific mailbox protocol.

```rust
pub struct AppleAudioDevice {
    /// Codec mailbox base address (from device tree)
    mailbox_base: *mut u8,

    /// Codec type (determined by SoC variant)
    codec: AppleCodecType,

    /// Hardware DSP for effects (EQ, dynamic range compression)
    dsp: Option<AppleAudioDsp>,

    /// Speaker topology (laptop: 4-6 speakers, desktop: 2)
    topology: SpeakerTopology,

    /// Device tree node for this codec
    dt_node: DeviceTreePath,
}

pub enum AppleCodecType {
    /// MacBook built-in speakers + headphone jack
    CS42L83,
    /// Mac Mini / Mac Studio line-out
    TAS5770L,
    /// Studio Display speakers
    SSM3515,
}

impl AudioDevice for AppleAudioDevice {
    fn name(&self) -> &str {
        match self.codec {
            AppleCodecType::CS42L83 => "Built-in Speakers",
            AppleCodecType::TAS5770L => "Line Out",
            AppleCodecType::SSM3515 => "Display Speakers",
        }
    }

    fn supported_formats(&self) -> Vec<AudioFormat> {
        // Apple codecs support 44.1, 48, 96, 192 kHz
        // Channel count depends on speaker topology
        vec![
            AudioFormat { sample_rate: 48000, channels: self.topology.channels(), format: SampleFormat::F32 },
            AudioFormat { sample_rate: 96000, channels: self.topology.channels(), format: SampleFormat::F32 },
            AudioFormat { sample_rate: 44100, channels: 2, format: SampleFormat::F32 },
        ]
    }
}
```

### 5.6 USB Audio Class

USB headsets and microphones implement the USB Audio Class standard. The USB meta-subsystem (see [subsystem-framework.md](./subsystem-framework.md) §12) detects USB Audio Class devices and routes them to the audio subsystem.

```rust
pub struct UsbAudioDevice {
    /// USB device handle (from USB subsystem)
    usb: UsbDeviceHandle,

    /// Audio streaming interfaces (one per endpoint direction)
    streaming_interfaces: Vec<UsbAudioStreamInterface>,

    /// Audio control interface (volume, mute, etc.)
    control_interface: UsbAudioControlInterface,

    /// Isochronous endpoint for real-time audio data
    iso_endpoint: UsbIsoEndpoint,

    /// Feedback endpoint (adaptive rate: device tells host its actual clock)
    feedback_endpoint: Option<UsbIsoEndpoint>,

    /// Descriptor-reported formats
    formats: Vec<AudioFormat>,
}

impl UsbAudioDevice {
    /// USB isochronous transfers run on 1ms USB frames.
    /// At 48 kHz stereo 16-bit: 48 samples * 2 ch * 2 bytes = 192 bytes per frame.
    fn configure_iso_transfer(&self, format: &AudioFormat) -> Result<()> {
        let bytes_per_frame = format.sample_rate / 1000
            * format.channels as u32
            * format.bytes_per_sample();
        self.iso_endpoint.configure(bytes_per_frame, 2)?; // double-buffered
        Ok(())
    }
}
```

-----

## 6. RT Scheduling Integration

Audio is the most timing-sensitive subsystem. A missed deadline causes an audible glitch — an underrun (silence) or overrun (repeated samples). The audio subsystem depends heavily on the kernel scheduler's Real-Time class. See [scheduler.md](../kernel/scheduler.md) §3.1 and §5.2.

### 6.1 RT Task Registration

The audio subsystem registers its mixing and capture callbacks as RT tasks during initialization.

```rust
impl AudioSubsystem {
    fn register_rt_tasks(&self) -> Result<()> {
        // Playback mixer: 200 Hz (5ms period), 0.5ms WCET
        let mixer_rt = RtTask {
            period: Duration::from_micros(5000),
            wcet: Duration::from_micros(500),
            relative_deadline: Duration::from_micros(5000),
            affinity: CpuSet::single(CpuId(0)), // pinned to core 0
            overrun: RtOverrunState::default(),
            deferred: false,
        };

        // Capture callback: 200 Hz (5ms period), 0.3ms WCET
        let capture_rt = RtTask {
            period: Duration::from_micros(5000),
            wcet: Duration::from_micros(300),
            relative_deadline: Duration::from_micros(5000),
            affinity: CpuSet::single(CpuId(0)),
            overrun: RtOverrunState::default(),
            deferred: false,
        };

        // Admission control: scheduler verifies total RT utilization < 70%
        // Mixer:   0.5ms / 5ms   = 10% utilization
        // Capture: 0.3ms / 5ms   =  6% utilization
        // Total audio RT load:     16% utilization
        // Compositor: 4ms / 16.6ms = 24% utilization
        // Grand total:              40% — well under 70% ceiling
        scheduler::admit_rt(mixer_rt)?;
        scheduler::admit_rt(capture_rt)?;

        Ok(())
    }
}
```

### 6.2 Latency Budget

The total round-trip audio latency budget is the sum of all stages in the audio path.

```
Latency budget breakdown (target: < 10ms round-trip):

Input (capture) path:
  Hardware → DMA buffer fill         1.0 ms  (half of 2ms DMA period)
  Capture callback processing        0.3 ms  (SRC + AEC + AGC)
  Agent ring buffer read latency     0.5 ms  (IPC notification delay)
  ─────────────────────────────────────────
  Total input latency:               1.8 ms

Output (playback) path:
  Agent ring buffer write latency    0.5 ms  (IPC notification delay)
  Mix callback processing            0.5 ms  (SRC + mix + clip)
  Hardware ring buffer → DMA         2.5 ms  (one period of the DMA buffer)
  DAC/codec output delay             0.2 ms  (hardware fixed delay)
  ─────────────────────────────────────────
  Total output latency:              3.7 ms

Round-trip (mic → processing → speakers):
  Input + processing + output        5.5 ms (nominal)
                                     9.0 ms (worst case with scheduling jitter)
```

### 6.3 Buffer Sizing

Buffer size directly trades latency for robustness. Smaller buffers reduce latency but increase the probability of underruns.

```rust
pub enum AudioLatencyMode {
    /// Lowest latency: 2.5ms buffer (120 frames @ 48kHz)
    /// For: voice calls, real-time monitoring, musical instruments
    /// Risk: underruns on loaded systems
    RealTime,

    /// Balanced: 5ms buffer (240 frames @ 48kHz)
    /// For: games, interactive audio, TTS playback
    /// Default mode for most sessions
    Interactive,

    /// Robust: 20ms buffer (960 frames @ 48kHz)
    /// For: music playback, background audio, notifications
    /// Virtually eliminates underruns even under heavy load
    Relaxed,
}

impl AudioLatencyMode {
    pub fn buffer_frames(&self, sample_rate: u32) -> u32 {
        let ms = match self {
            Self::RealTime => 2.5,
            Self::Interactive => 5.0,
            Self::Relaxed => 20.0,
        };
        (sample_rate as f64 * ms / 1000.0) as u32
    }

    /// Scheduler period for this latency mode
    pub fn rt_period(&self) -> Duration {
        match self {
            Self::RealTime => Duration::from_micros(2500),
            Self::Interactive => Duration::from_micros(5000),
            Self::Relaxed => Duration::from_micros(10000),
        }
    }
}
```

### 6.4 Underrun Handling

When the mixer reads from an agent's ring buffer and finds it empty, an underrun has occurred. The response depends on severity.

```rust
pub struct UnderrunPolicy {
    /// Number of consecutive underruns before taking action
    tolerance: u32,
    /// Action on repeated underruns
    action: UnderrunAction,
}

pub enum UnderrunAction {
    /// Insert silence and continue (default — minimizes audible impact)
    Silence,
    /// Repeat the last buffer (less noticeable for music)
    RepeatLast,
    /// Increase buffer size automatically (trades latency for stability)
    AutoResize { max_buffer_ms: f32 },
    /// Close the session (agent is not supplying audio in time)
    CloseSession,
}
```

-----

## 7. A/V Sync with Compositor

When video and audio play simultaneously (media player, video call, game), the audio and video streams must be synchronized. A lip-sync error greater than ±40ms is perceptible; greater than ±80ms is distracting. The audio subsystem and compositor share a timeline to maintain synchronization. See [compositor.md](./compositor.md) §6 for the render pipeline.

### 7.1 Shared Timeline

```rust
/// System-wide media clock.
/// The audio subsystem is the clock master — video follows audio.
/// Rationale: audio glitches are more noticeable than dropped video frames.
pub struct MediaTimeline {
    /// Monotonic reference clock (ARM Generic Timer, ~54 MHz)
    base_clock: ArmGenericTimer,

    /// Current playback position in media time (microseconds)
    media_position: AtomicU64,

    /// Playback rate (1.0 = normal, 0.5 = half speed, 2.0 = double)
    playback_rate: AtomicF32,

    /// Audio samples played since timeline start
    /// (ground truth — audio hardware is the clock master)
    samples_played: AtomicU64,

    /// Sample rate of the output device
    sample_rate: u32,
}

impl MediaTimeline {
    /// Get the current media time, derived from audio samples played.
    /// This is the authoritative time source — video adjusts to match.
    pub fn current_media_time(&self) -> Duration {
        let samples = self.samples_played.load(Relaxed);
        let rate = self.playback_rate.load(Relaxed);
        Duration::from_micros(
            (samples as f64 / self.sample_rate as f64 * 1_000_000.0 / rate as f64) as u64
        )
    }
}
```

### 7.2 Presentation Timestamps

Agents that produce synchronized audio and video attach presentation timestamps (PTS) to both streams. The audio subsystem and compositor use these to schedule delivery.

```rust
/// A timestamped audio buffer from an agent
pub struct TimestampedAudioBuffer {
    /// PCM sample data
    samples: Vec<f32>,
    /// Presentation timestamp: when these samples should be heard
    pts: Duration,
    /// Duration of audio in this buffer
    duration: Duration,
}

/// A timestamped video frame from an agent
pub struct TimestampedVideoFrame {
    /// Surface buffer ID
    buffer: SharedBufferId,
    /// Presentation timestamp: when this frame should be displayed
    pts: Duration,
}
```

### 7.3 Synchronization Protocol

```
Audio-Video Synchronization Flow:

1. Agent decodes media, producing audio + video with PTS values
     Audio PTS: 1000ms, 1020ms, 1040ms, ...
     Video PTS: 1000ms, 1033ms, 1067ms, ... (30fps)

2. Audio subsystem plays samples at their PTS
   (audio is the master clock — never adjusted)

3. Compositor receives video frames with PTS
   Before presenting each frame, it queries the media timeline:
     audio_time = timeline.current_media_time()
     drift = video_pts - audio_time

   Three cases:
     drift < -40ms: Video is late.
                    Drop frame, present next immediately.
     drift > +40ms: Video is early.
                    Hold current frame, present new one at correct time.
     |drift| < 40ms: Acceptable sync.
                     Present frame on next VSync.

4. If drift accumulates beyond ±80ms for 5+ seconds:
   The agent is notified to resync (seek audio to video position
   or vice versa).
```

### 7.4 Communication Between Audio and Compositor

```rust
/// IPC message from audio subsystem to compositor
pub enum AudioToCompositor {
    /// Media timeline update (sent every mix period)
    TimelineUpdate {
        timeline_id: TimelineId,
        media_time: Duration,
        wall_clock: Timestamp,
    },

    /// Audio stream started/stopped (compositor may adjust VSync timing)
    StreamStateChanged {
        timeline_id: TimelineId,
        state: StreamState,
    },
}

/// IPC message from compositor to audio subsystem
pub enum CompositorToAudio {
    /// Video frame presented — allows audio to track actual display timing
    FramePresented {
        timeline_id: TimelineId,
        frame_pts: Duration,
        actual_present_time: Timestamp,
    },

    /// Compositor requesting audio to pause/resume (e.g., window minimized)
    PlaybackControl {
        timeline_id: TimelineId,
        action: PlaybackAction,
    },
}
```

-----

## 8. HDMI Audio Routing

HDMI audio requires coordination between the audio subsystem, the display driver, and the connected display device. The audio stream is embedded in the HDMI data stream, which means audio routing is tied to display routing.

### 8.1 EDID Parsing for Audio

When an HDMI display is connected, its EDID (Extended Display Identification Data) contains Short Audio Descriptors (SADs) that declare supported audio formats.

```rust
pub struct HdmiAudioCaps {
    /// Supported audio formats from EDID CEA-861 SADs
    formats: Vec<HdmiAudioFormat>,
    /// Whether the sink supports basic audio (2ch 48kHz LPCM — mandatory)
    basic_audio: bool,
    /// Speaker allocation (which physical speakers the display has)
    speaker_allocation: SpeakerAllocation,
}

pub struct HdmiAudioFormat {
    pub coding: AudioCoding,
    pub channels: u16,          // max channels for this format
    pub sample_rates: Vec<u32>, // supported sample rates
    pub bit_depths: Vec<u8>,    // for LPCM: 16, 20, 24
    pub max_bitrate: Option<u32>, // for compressed formats (AC3, DTS)
}

pub enum AudioCoding {
    Lpcm,       // Linear PCM (always supported)
    Ac3,        // Dolby Digital
    Aac,        // AAC-LC
    Dts,        // DTS
    DolbyTrueHd, // Dolby TrueHD (lossless)
    DtsHd,      // DTS-HD Master Audio (lossless)
    EAc3,       // Dolby Digital Plus
}

impl HdmiAudioCaps {
    pub fn from_edid(edid: &Edid) -> Result<Self> {
        let cea_block = edid.cea_extension()?;
        let sads = cea_block.short_audio_descriptors()?;
        let speaker_alloc = cea_block.speaker_allocation()?;

        Ok(Self {
            formats: sads.into_iter().map(|sad| HdmiAudioFormat {
                coding: AudioCoding::from_cea(sad.format_code),
                channels: sad.max_channels,
                sample_rates: sad.sample_rates(),
                bit_depths: sad.bit_depths(),
                max_bitrate: sad.max_bitrate(),
            }).collect(),
            basic_audio: cea_block.basic_audio_supported(),
            speaker_allocation: speaker_alloc,
        })
    }

    /// Find the best LPCM format that both source and sink support
    pub fn best_lpcm_format(&self) -> Result<AudioFormat> {
        let lpcm = self.formats.iter()
            .find(|f| f.coding == AudioCoding::Lpcm)
            .ok_or(AudioError::NoLpcmSupport)?;

        // Prefer 48kHz (standard), then 96kHz, then 44.1kHz
        let rate = if lpcm.sample_rates.contains(&48000) { 48000 }
            else if lpcm.sample_rates.contains(&96000) { 96000 }
            else { *lpcm.sample_rates.first().ok_or(AudioError::NoSampleRate)? };

        let depth = if lpcm.bit_depths.contains(&24) { 24 }
            else { 16 };

        Ok(AudioFormat {
            sample_rate: rate,
            channels: lpcm.channels.min(2), // start with stereo
            format: match depth {
                24 => SampleFormat::I24,
                _ => SampleFormat::I16,
            },
        })
    }
}
```

### 8.2 CEC (Consumer Electronics Control)

HDMI CEC allows the OS to control the TV/receiver's volume and mute via the HDMI cable. When the user adjusts system volume and the output is HDMI, the audio subsystem sends CEC commands to the sink device.

```rust
pub struct CecController {
    /// CEC register base address
    base: *mut u8,
    /// Logical address assigned to this device (typically "Playback Device 1")
    logical_addr: CecLogicalAddr,
    /// Physical address (determined by HDMI port topology)
    physical_addr: CecPhysicalAddr,
}

impl CecController {
    /// Send volume up/down/mute to the audio system (TV or receiver)
    pub fn set_volume(&self, volume: f32) -> Result<()> {
        if volume == 0.0 {
            self.send_message(CecMessage::UserControlPressed(CecOpcode::Mute))?;
        } else {
            // CEC volume is 0-100
            let cec_vol = (volume * 100.0) as u8;
            self.send_message(CecMessage::SetAudioVolume(cec_vol))?;
        }
        Ok(())
    }

    /// Request Audio Return Channel (ARC) activation.
    /// ARC sends audio FROM the TV back to the OS (e.g., TV tuner audio).
    pub fn activate_arc(&self) -> Result<()> {
        self.send_message(CecMessage::InitiateArc)?;
        Ok(())
    }

    /// Handle incoming CEC messages (TV remote volume buttons, etc.)
    pub fn handle_message(&self, msg: CecMessage) -> Option<AudioEvent> {
        match msg {
            CecMessage::UserControlPressed(CecOpcode::VolumeUp) => {
                Some(AudioEvent::VolumeChange(0.05)) // +5%
            }
            CecMessage::UserControlPressed(CecOpcode::VolumeDown) => {
                Some(AudioEvent::VolumeChange(-0.05)) // -5%
            }
            CecMessage::UserControlPressed(CecOpcode::Mute) => {
                Some(AudioEvent::MuteToggle)
            }
            _ => None,
        }
    }
}
```

### 8.3 Multi-Output Routing

When multiple audio outputs are available (HDMI + headphones + USB), the route manager selects the active output based on priority and user configuration.

```
Output priority (default, user-configurable):

1. USB headset      (exclusive — if plugged in, takes over)
2. Bluetooth A2DP   (exclusive — if connected and in audio profile)
3. 3.5mm headphone  (exclusive — if jack sense detects insertion)
4. HDMI audio       (shared with display — default for desktop use)
5. I2S DAC          (if connected via HAT)
6. PWM audio        (fallback — always available on Pi)

When a higher-priority output becomes available:
  - Active playback sessions are rerouted to the new output
  - The mixer reconfigures for the new device's format
  - The transition is crossfaded (50ms linear fade) to avoid clicks
```

-----

## 9. Power Management

Audio devices have significant power impact. An active audio codec draws 10-50mW; an idle one can draw near zero in deep sleep. The audio subsystem implements the framework's `PowerManaged` trait with audio-specific policies.

### 9.1 Power States

```rust
impl PowerManaged for AudioSubsystem {
    fn idle_policy(&self) -> IdlePolicy {
        IdlePolicy {
            // Suspend audio hardware after 10 seconds of no active sessions
            idle_to_suspended: Duration::from_secs(10),
            // Full power-off after 2 minutes suspended
            suspended_to_off: Duration::from_secs(120),
            // Wake on any of these events
            wake_on: vec![
                WakeEvent::SessionRequested,  // agent wants to play audio
                WakeEvent::DeviceInterrupt,   // headphone jack insertion
            ],
        }
    }
}
```

### 9.2 Audio-Specific Power Events

```rust
pub enum AudioPowerEvent {
    /// Lid closed (laptop) — mute speakers, keep headphone output
    LidClosed,
    /// Lid opened — unmute speakers
    LidOpened,
    /// System entering sleep — stop all audio, power down codecs
    SystemSuspend,
    /// System waking — re-initialize codecs, restore sessions
    SystemResume,
    /// Thermal throttling — reduce audio processing (disable SRC, use linear)
    ThermalThrottle { level: ThermalLevel },
    /// External power disconnected — reduce codec power (lower sample rate)
    BatteryMode,
}

impl AudioSubsystem {
    fn handle_power_event(&mut self, event: AudioPowerEvent) -> Result<()> {
        match event {
            AudioPowerEvent::LidClosed => {
                // Mute built-in speakers only; external outputs continue
                for device in &mut self.devices {
                    if device.is_builtin_speaker() {
                        device.set_mute(true)?;
                    }
                }
            }
            AudioPowerEvent::SystemSuspend => {
                // Save mixer state, stop RT threads, power down all codecs
                self.saved_state = Some(self.mixer.save_state());
                scheduler::remove_rt(self.mixer_rt_id)?;
                scheduler::remove_rt(self.capture_rt_id)?;
                for device in &mut self.devices {
                    device.set_power(PowerState::Off)?;
                }
            }
            AudioPowerEvent::SystemResume => {
                // Restore codecs, restart RT threads, restore mixer state
                for device in &mut self.devices {
                    device.set_power(PowerState::Active)?;
                }
                self.register_rt_tasks()?;
                if let Some(state) = self.saved_state.take() {
                    self.mixer.restore_state(state);
                }
            }
            AudioPowerEvent::ThermalThrottle { level } => {
                match level {
                    ThermalLevel::Warm => {
                        // Switch SRC to linear interpolation (less CPU)
                        self.mixer.set_src_quality(SrcQuality::Linear);
                    }
                    ThermalLevel::Hot => {
                        // Additionally: reduce mix callback frequency
                        // from 200 Hz to 100 Hz (10ms period)
                        self.mixer.set_period(Duration::from_millis(10));
                    }
                    ThermalLevel::Critical => {
                        // Mute all non-essential audio, keep only
                        // system sounds and active voice calls
                        self.mixer.mute_non_essential();
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}
```

### 9.3 On-Demand Activation

The audio subsystem supports on-demand activation (see [boot-lifecycle.md](../kernel/boot-lifecycle.md) §17). It is not started at boot unless the boot chime is enabled. The first audio event (notification sound, media playback, voice interaction) triggers subsystem initialization, adding approximately 100ms to the first audio output.

```
Boot chime path (if enabled):
  Phase 2: HAL init_audio() initializes hardware
  Boot chime: raw PCM written directly via HAL (no subsystem needed)
  Audio subsystem: started on-demand when first agent requests a session

On-demand activation path:
  Agent requests AudioPlayback capability → kernel checks, approves
  Audio subsystem not yet running → service manager starts it (~50-100ms)
  Subsystem init: discover devices, configure mixer, register RT tasks
  Session opens → first audio plays
  Total first-audio latency: ~100-150ms (acceptable for non-real-time use)
```

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

-----

## 11. Audit and Observability

The audio subsystem logs all activity to the `system/audit/audio/` space, following the framework's audit model.

### 11.1 Audit Events

```rust
pub enum AudioAuditEvent {
    /// An agent opened an audio session
    SessionOpened {
        session_id: SessionId,
        agent: AgentId,
        device: DeviceId,
        direction: DataDirection,
        format: AudioFormat,
        purpose: String,
    },

    /// An audio session was closed
    SessionClosed {
        session_id: SessionId,
        agent: AgentId,
        duration: Duration,
        samples_processed: u64,
        peak_level: f32,
    },

    /// Microphone access was requested
    MicrophoneAccessRequested {
        agent: AgentId,
        granted: bool,
        reason: String,
    },

    /// Audio device connected or disconnected
    DeviceChanged {
        device: DeviceId,
        name: String,
        event: DeviceChangeEvent,
    },

    /// Audio route changed (e.g., headphones plugged in)
    RouteChanged {
        from: DeviceId,
        to: DeviceId,
        reason: String,
    },

    /// Mixer underrun (performance issue indicator)
    Underrun {
        stream: StreamId,
        agent: AgentId,
        consecutive_count: u32,
    },
}

impl AuditRecord for AudioAuditEvent {
    fn timestamp(&self) -> Timestamp { now() }
    fn agent(&self) -> AgentId { self.agent_id() }
    fn session(&self) -> SessionId { self.session_id() }
    fn task(&self) -> Option<TaskId> { None }
    fn event_type(&self) -> &str {
        match self {
            Self::SessionOpened { .. } => "audio.session.opened",
            Self::SessionClosed { .. } => "audio.session.closed",
            Self::MicrophoneAccessRequested { .. } => "audio.mic.requested",
            Self::DeviceChanged { .. } => "audio.device.changed",
            Self::RouteChanged { .. } => "audio.route.changed",
            Self::Underrun { .. } => "audio.underrun",
        }
    }
    fn summary(&self) -> String {
        match self {
            Self::SessionOpened { agent, direction, purpose, .. } =>
                format!("Agent {} opened {} session: {}", agent, direction, purpose),
            Self::MicrophoneAccessRequested { agent, granted, .. } =>
                format!("Agent {} mic access: {}", agent, if *granted { "granted" } else { "denied" }),
            _ => format!("{:?}", self),
        }
    }
}
```

### 11.2 Audit Space Structure

```
system/audit/audio/
  sessions/                     ← Who used audio, when, why
    {session_id}/
      agent: AgentId
      device: DeviceId
      direction: playback|capture
      started: Timestamp
      ended: Timestamp
      samples: u64
      peak_level: f32
  microphone/                   ← Microphone access log (privacy-critical)
    {request_id}/
      agent: AgentId
      granted: bool
      timestamp: Timestamp
  devices/                      ← Device connection history
    {device_id}/
      connected: Timestamp
      disconnected: Option<Timestamp>
      name: String
  underruns/                    ← Performance diagnostics
    daily_count: u32
    worst_agent: AgentId
```

### 11.3 AIRS Integration

Because the audit space follows the standard structure, AIRS can answer audio-related queries without audio-specific code:

- "What has accessed my microphone?" -> query `system/audit/audio/microphone/`
- "Is anything playing audio right now?" -> query active sessions
- "Which agent caused audio glitches?" -> query `system/audit/audio/underruns/`
- "How much audio did I record this week?" -> aggregate session durations

-----

## 12. POSIX Bridge

The audio subsystem exposes POSIX-compatible device nodes for BSD tools and legacy applications.

```rust
impl PosixBridge for AudioPosixBridge {
    fn dev_nodes(&self) -> Vec<DevNode> {
        vec![
            DevNode {
                path: "/dev/audio0".into(),
                device_class: "audio".into(),
                permissions: PosixPerms::rw(),
                device_id: self.default_output,
            },
            DevNode {
                path: "/dev/audioin0".into(),
                device_class: "audio".into(),
                permissions: PosixPerms::rw(),
                device_id: self.default_input,
            },
            DevNode {
                path: "/dev/mixer0".into(),
                device_class: "audio".into(),
                permissions: PosixPerms::rw(),
                device_id: self.mixer_control,
            },
            DevNode {
                path: "/dev/dsp".into(),
                device_class: "audio".into(),
                permissions: PosixPerms::rw(),
                device_id: self.default_output,
            },
        ]
    }

    fn ioctl(&self, fd: PosixFd, request: u64, arg: *mut u8) -> Result<i32> {
        match request {
            // OSS-compatible ioctls
            SNDCTL_DSP_SPEED => {
                let rate = unsafe { *(arg as *const i32) };
                self.set_sample_rate(fd, rate as u32)?;
                Ok(0)
            }
            SNDCTL_DSP_CHANNELS => {
                let ch = unsafe { *(arg as *const i32) };
                self.set_channels(fd, ch as u16)?;
                Ok(0)
            }
            SNDCTL_DSP_SETFMT => {
                let fmt = unsafe { *(arg as *const i32) };
                self.set_format(fd, oss_to_sample_format(fmt))?;
                Ok(0)
            }
            MIXER_READ_VOLUME => {
                let vol = self.get_volume()?;
                unsafe { *(arg as *mut i32) = (vol * 100.0) as i32; }
                Ok(0)
            }
            MIXER_WRITE_VOLUME => {
                let vol = unsafe { *(arg as *const i32) } as f32 / 100.0;
                self.set_volume(vol)?;
                Ok(0)
            }
            _ => Err(PosixError::Enotty),
        }
    }
}
```

-----

## 13. Boot Chime

The boot chime is a special audio path that operates before the audio subsystem is started. It uses the HAL's raw audio output to generate a synthesized tone. See [boot-lifecycle.md](../kernel/boot-lifecycle.md) §20.3.

```rust
/// Generate and play a boot chime directly via HAL.
/// No audio subsystem, no mixer, no sessions — raw hardware access.
pub fn play_boot_chime(hal_audio: &dyn PlatformAudio, event: BootChimeEvent) {
    let sample_rate = 48000u32;
    let duration_ms = match event {
        BootChimeEvent::Phase2Complete => 200,   // short tone: 440 Hz
        BootChimeEvent::Phase5Complete => 400,   // two ascending tones
        BootChimeEvent::Panic => 600,            // low descending tone
    };

    let samples = duration_ms * sample_rate / 1000;
    let mut buffer = vec![0i16; samples as usize];

    match event {
        BootChimeEvent::Phase2Complete => {
            // 440 Hz sine wave (A4), with 20ms fade-in/fade-out
            synthesize_sine(&mut buffer, 440.0, sample_rate, 20);
        }
        BootChimeEvent::Phase5Complete => {
            // Two ascending tones: C5 (523 Hz) then E5 (659 Hz)
            let half = samples as usize / 2;
            synthesize_sine(&mut buffer[..half], 523.0, sample_rate, 10);
            synthesize_sine(&mut buffer[half..], 659.0, sample_rate, 10);
        }
        BootChimeEvent::Panic => {
            // Descending tone: 220 Hz down to 110 Hz over 600ms
            synthesize_sweep(&mut buffer, 220.0, 110.0, sample_rate, 20);
        }
    }

    // Write directly to hardware — bypasses all subsystem infrastructure
    hal_audio.write_samples(&buffer).ok();
}
```

-----

## 14. Implementation Order

The audio subsystem is implemented across multiple development phases, building complexity incrementally.

```
Phase 8:   HAL PlatformAudio trait + VirtIO-Sound driver
           ├── VirtIO-Sound device discovery and initialization
           ├── Raw PCM output (write_samples via HAL)
           ├── Boot chime (synthesized, direct HAL write)
           └── Test: audible tone in QEMU

Phase 10:  Audio subsystem service (basic)
           ├── Subsystem registration with framework
           ├── Session open/close with capability gate
           ├── Single-stream playback (no mixing)
           ├── AudioPlayback capability type
           └── Audit logging (session opened/closed)

Phase 12:  PCM mixing engine
           ├── Multi-stream software mixer
           ├── Per-stream volume and pan
           ├── Format negotiation
           ├── Sample rate conversion (linear)
           ├── RT task registration (5ms period, 0.5ms WCET)
           └── Ring buffer shared memory between agent and mixer

Phase 14:  Raspberry Pi audio drivers
           ├── HDMI audio (VC4/VC7)
           ├── PWM audio (3.5mm headphone jack)
           ├── I2S driver (HiFiBerry DAC support)
           ├── DMA-driven output (ping-pong buffers)
           └── Device detection from device tree

Phase 16:  Advanced mixing and capture
           ├── Audio capture (microphone input)
           ├── Capture multiplexing to multiple agents
           ├── Echo cancellation
           ├── Automatic gain control
           ├── High-quality SRC (polyphase sinc)
           └── Full-duplex (voice calls)

Phase 18:  USB and Bluetooth audio
           ├── USB Audio Class driver
           ├── Isochronous USB transfers
           ├── Bluetooth A2DP (audio streaming)
           ├── Bluetooth HFP (hands-free voice)
           ├── Hotplug: automatic route switching
           └── Crossfade on device change

Phase 19:  A/V sync and HDMI advanced
           ├── Shared media timeline with compositor
           ├── Presentation timestamps
           ├── HDMI EDID audio capability parsing
           ├── HDMI CEC volume/mute control
           ├── Audio Return Channel (ARC)
           └── Multi-channel audio (5.1/7.1 for HDMI)

Phase 22:  Apple Silicon audio + power management
           ├── Apple codec drivers (CS42L83, TAS5770L)
           ├── Hardware DSP integration
           ├── Lid close/open mute behavior
           ├── Thermal throttling (SRC quality reduction)
           ├── On-demand subsystem activation
           └── Suspend/resume with state preservation

Phase 25:  POSIX bridge + compatibility
           ├── /dev/audio*, /dev/dsp, /dev/mixer* nodes
           ├── OSS-compatible ioctl interface
           ├── ALSA compatibility shim (if needed)
           └── PulseAudio/PipeWire socket compatibility (stretch)
```

-----

## 15. Design Principles

1. **Audio is the clock master.** In any synchronized media pipeline, audio timing is authoritative. Video adjusts to match audio, never the reverse. Audio glitches are more perceptible than dropped video frames.

2. **RT or nothing.** The mix callback runs in the scheduler's Real-Time class with hard EDF deadlines. There is no "best effort" audio path. If the RT admission controller rejects the audio task (utilization ceiling exceeded), the system has a configuration problem, not a graceful degradation.

3. **Shared by default, exclusive by capability.** Multiple agents share the mixer. Only agents with explicit `exclusive: true` capability can bypass the mixer for direct hardware access. This keeps the common case simple and the rare case possible.

4. **The mixer is always f32 internally.** All format conversion happens at the edges — when samples enter the mixer (from agents) and when they leave (to hardware). Internal mixing is always 32-bit float to preserve dynamic range.

5. **Silence is better than garbage.** On underrun, the mixer inserts silence. Repeating stale samples or interpolating is worse than a brief silence in nearly all cases. The exception (music playback) can opt into repeat-last behavior via session intent.

6. **Microphone access is always audited and prompted.** No agent can capture audio without user consent. The capability gate enforces this, and every microphone access is logged to the audit space. There are no backdoors.

7. **Hardware details stop at the device abstraction.** The mixer does not know whether it is writing to VirtIO, I2S, PWM, HDMI, or USB. The `AudioDevice` trait is the boundary. Adding a new audio output type requires only implementing that trait.

8. **Latency is configurable per session.** Voice calls use 2.5ms buffers. Music playback uses 20ms buffers. The agent declares intent, and the subsystem chooses the appropriate latency mode. One size does not fit all.

9. **Power management is aggressive.** Audio hardware is powered down within 10 seconds of the last session closing. The 100ms wake-up penalty for the first sound after idle is acceptable — users do not perceive it.

10. **The boot chime proves the hardware works.** A synthesized tone at Phase 2 completion confirms that the audio path from CPU to DAC is functional. No audio files, no filesystem, no subsystem required — just the HAL and arithmetic.
