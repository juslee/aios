# AIOS Audio Subsystem — Architecture & Implementation

Part of: [audio.md](../audio.md) — Audio Subsystem
**Related:** [mixing.md](./mixing.md) — PCM mixer and capture pipeline, [drivers.md](./drivers.md) — Hardware drivers, [scheduling.md](./scheduling.md) — RT scheduling and A/V sync, [integration.md](./integration.md) — HDMI, power, audit, POSIX

-----

## 2. Architecture

```text
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

    /// AIRS provides context-aware device selection hints
    airs_route_advisor: Option<AirsRouteAdvisor>,

    /// AIRS signals predicted load changes
    buffer_hint: Option<BufferHint>,

    /// Exposes underrun rate, SNR, peak level to AIRS
    quality_metrics: AudioQualityMetrics,
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

        // 2b. Consult AIRS route advisor (tiebreaker only)
        // If the agent requested Default target and AIRS has a suggestion
        // with sufficient confidence, prefer the AIRS-suggested device.
        // Never overrides AudioTarget::Specific or explicit user choice.
        if matches!(&cap.target, AudioTarget::Default) {
            if let Some(advisor) = &self.airs_route_advisor {
                if advisor.confidence > 0.7 {
                    if let Some(suggested) = advisor.suggested_output {
                        device = self.devices.get(suggested)?;
                    }
                }
            }
        }

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

```rust
/// AIRS integration hook for context-aware audio routing.
/// AIRS observes time-of-day patterns, active agents, user location,
/// and session intent to advise the RouteManager on device selection.
///
/// The advisor provides hints as a tiebreaker — it never overrides
/// explicit user device choices.
pub struct AirsRouteAdvisor {
    /// Suggested output device for the next session
    pub suggested_output: Option<DeviceId>,
    /// Suggested input device for the next session
    pub suggested_input: Option<DeviceId>,
    /// Confidence in the suggestion (0.0 - 1.0)
    pub confidence: f32,
    /// Reason for the suggestion (for audit logging)
    pub reason: String,
}

/// Buffer size hint from AIRS predictive load management.
/// The mixer checks this each period and smoothly adjusts buffer
/// size within the current latency mode bounds.
pub struct BufferHint {
    /// Suggested buffer size adjustment (positive = increase, negative = decrease)
    pub adjustment_frames: i32,
    /// Predicted load event (e.g., "compilation starting", "model inference")
    pub predicted_event: String,
    /// Time until predicted load change
    pub eta: Duration,
    /// Confidence in the prediction (0.0 - 1.0)
    pub confidence: f32,
}

/// Audio quality metrics exposed to AIRS for monitoring and assessment.
pub struct AudioQualityMetrics {
    /// Underrun count in the last measurement window
    pub underrun_count: u32,
    /// Signal-to-noise ratio estimate (dB)
    pub snr_db: f32,
    /// Peak output level in the last window (0.0 - 1.0)
    pub peak_level: f32,
    /// Average latency of the output path
    pub avg_latency: Duration,
    /// Measurement window duration
    pub window: Duration,
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
