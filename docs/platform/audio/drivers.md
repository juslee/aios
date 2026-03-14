# AIOS Audio Subsystem — Hardware Drivers

Part of: [audio.md](../audio.md) — Audio Subsystem
**Related:** [subsystem.md](./subsystem.md) — Architecture and sessions, [mixing.md](./mixing.md) — PCM mixer and capture pipeline, [scheduling.md](./scheduling.md) — RT scheduling

-----

## 5. Hardware Drivers

Each platform has different audio hardware. The HAL provides the `PlatformAudio` extension trait (see [hal.md](../../kernel/hal.md) §12) that initializes the hardware at boot. The audio subsystem builds higher-level drivers on top.

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

PWM (Pulse Width Modulation) audio on the Raspberry Pi drives the 3.5mm headphone jack. It is lower quality than I2S but requires no external hardware. See [hal.md](../../kernel/hal.md) §12 for the `PlatformPwm` trait.

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

USB headsets and microphones implement the USB Audio Class standard. The USB meta-subsystem (see [subsystem-framework.md](../subsystem-framework.md) §12) detects USB Audio Class devices and routes them to the audio subsystem.

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

### 5.7 Privacy-First Hardware Controls

Every audio driver that supports input (microphone capture) must implement hardware-level privacy controls. These are architectural requirements, not optional features.

#### Hardware Microphone Kill Switch

When the hardware provides a physical microphone kill switch (common on laptops and USB devices), the driver MUST:

1. **Report switch state** via `hw_mute_state()` — the audio subsystem polls this on each capture callback
2. **Return silence when muted** — the driver fills the capture buffer with zeros, NOT an error code
3. **Signal the compositor** — the visual microphone indicator shows the muted state (different icon/color)
4. **Log to audit space** — hardware mute events are recorded for the user's privacy dashboard

```rust
/// Privacy control extension for audio input drivers.
/// Every driver supporting capture MUST implement this trait.
pub trait AudioInputPrivacy {
    /// Check the hardware mute switch state.
    /// Called on each capture callback — must be fast (register read only).
    fn hw_mute_state(&self) -> MuteState;

    /// Whether this device has a hardware mute indicator LED.
    /// If true, the driver controls the LED directly.
    fn has_hw_mute_indicator(&self) -> bool { false }

    /// Signal the compositor that capture state changed.
    /// Used for the visual microphone activity indicator.
    fn capture_state_signal(&self) -> CaptureStateSignal;
}

pub enum MuteState {
    /// Hardware switch is in unmuted position — capture active
    Unmuted,
    /// Hardware switch is in muted position — driver returns silence
    HardwareMuted,
    /// No hardware switch present — software-only mute control
    NoSwitch,
}

pub enum CaptureStateSignal {
    /// Microphone is actively capturing audio
    Active,
    /// Microphone is hardware-muted (user pressed physical switch)
    HardwareMuted,
    /// Microphone is software-muted (agent or subsystem control)
    SoftwareMuted,
    /// No active capture session
    Inactive,
}
```

#### Per-Driver Privacy Implementation

```text
VirtIO-Sound:     Reports MuteState::NoSwitch (virtual device has no physical switch).
                  Privacy relies entirely on the capability gate and software controls.

I2S / HiFiBerry:  Depends on DAC board. Some boards have a hardware mute pin — the driver
                  reads the GPIO state. Others report MuteState::NoSwitch.

USB Audio Class:  USB Audio Class 2.0 defines a Mute Control feature unit. The driver reads
                  the mute state from the control interface. Many USB headsets have a physical
                  mute button that triggers this feature unit.

Apple Silicon:    Built-in microphones have a hardware disconnect controlled by the T2/M-series
                  security chip. When the lid is closed or the hardware privacy switch is
                  engaged, the mic is electrically disconnected. The driver detects this via
                  the codec mailbox and reports MuteState::HardwareMuted.
```

This design follows Android 12's pattern where apps receive silent audio (not errors) when the hardware kill switch is engaged, preventing apps from detecting that they are being muted.
