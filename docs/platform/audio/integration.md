# AIOS Audio Subsystem — System Integration

Part of: [audio.md](../audio.md) — Audio Subsystem
**Related:** [subsystem.md](./subsystem.md) — Architecture and sessions, [mixing.md](./mixing.md) — PCM mixer and capture pipeline, [drivers.md](./drivers.md) — Hardware drivers, [scheduling.md](./scheduling.md) — RT scheduling and A/V sync

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

```text
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

The audio subsystem supports on-demand activation (see [intelligence.md](../../kernel/boot/intelligence.md) §17). It is not started at boot unless the boot chime is enabled. The first audio event (notification sound, media playback, voice interaction) triggers subsystem initialization, adding approximately 100ms to the first audio output.

```text
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

```text
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

### 11.4 Visual Microphone Activity Indicator

Every major production OS (Android 12+, macOS, Windows) now shows a persistent visual indicator when the microphone is active. AIOS requires this as an architectural constraint, not an optional feature.

**Requirement:** The compositor MUST display a persistent, non-dismissable indicator whenever any agent has an active audio capture session. The indicator MUST:

1. **Be visible at all times** — cannot be obscured by agent windows or system UI
2. **Show the capturing agent** — tapping/clicking the indicator reveals which agent(s) are using the microphone
3. **Distinguish hardware mute** — show a different state when the hardware kill switch is engaged (see [drivers.md](./drivers.md) §5.7)
4. **Be non-spoofable** — agents cannot draw over or hide the indicator; it is rendered by the compositor at the highest z-order

```rust
/// Message from audio subsystem to compositor for microphone indicator.
pub enum AudioCaptureIndicator {
    /// At least one capture session is active — show indicator
    Active {
        /// Agent(s) with active capture sessions
        agents: Vec<AgentId>,
        /// Whether any session has hardware mute engaged
        hw_muted: bool,
    },
    /// No active capture sessions — hide indicator
    Inactive,
}
```

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

The boot chime is a special audio path that operates before the audio subsystem is started. It uses the HAL's raw audio output to generate a synthesized tone. See [accessibility.md](../../kernel/boot/accessibility.md) §20.3.

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
