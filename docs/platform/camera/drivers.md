# AIOS Camera Drivers

Part of: [camera.md](../camera.md) — Camera Subsystem
**Related:** [devices.md](./devices.md) — Device taxonomy and discovery, [pipeline.md](./pipeline.md) — ISP pipeline and frame delivery, [usb/device-classes.md](../usb/device-classes.md) — UVC protocol details (§4.4), [device-model/discovery.md](../../kernel/device-model/discovery.md) — Driver trait and matching

-----

## §7 Camera Drivers

### §7.1 UVC Driver

The UVC (USB Video Class) driver handles USB webcams and document cameras. The core protocol implementation is in the USB subsystem (see [usb/device-classes.md](../usb/device-classes.md) §4.4). The camera subsystem's `UvcCameraDriver` wraps the USB-level `UvcDriver` with camera-specific session management, ISP integration, and privacy enforcement.

```rust
/// UVC camera driver — wraps the USB-level UVC driver.
pub struct UvcCameraDriver {
    /// Underlying USB UVC driver instance.
    usb_driver: UvcDriver,
    /// Camera device identifier in the camera subsystem.
    camera_id: CameraId,
    /// Parsed UVC capabilities.
    capabilities: CameraCapabilitiesDescriptor,
    /// Current streaming state.
    state: DriverState,
    /// Frame assembler for multi-packet frame reconstruction.
    frame_assembler: UvcFrameAssembler,
    /// Buffer pool for incoming frames.
    buffer_pool: CameraBufferPool,
    /// Privacy indicator state.
    indicator_active: bool,
}
```

#### UVC-Specific Features

**Format support**: The UVC driver supports MJPEG (most common), YUY2 (uncompressed), NV12 (semi-planar), and H.264 (UVC 1.5 devices). MJPEG requires CPU decoding before ISP processing or GPU display; the driver performs JPEG decode using a software decoder (no external library dependency — minimal Huffman + IDCT implementation).

**Camera controls**: The driver translates `CameraControl` commands to UVC control requests:

| CameraControl | UVC Control | Interface |
|---|---|---|
| Exposure | CT_EXPOSURE_TIME_ABSOLUTE | Camera Terminal |
| AutoExposure | CT_AE_MODE | Camera Terminal |
| Focus | CT_FOCUS_ABSOLUTE | Camera Terminal |
| AutoFocus | CT_FOCUS_AUTO | Camera Terminal |
| Zoom | CT_ZOOM_ABSOLUTE | Camera Terminal |
| Pan/Tilt | CT_PANTILT_ABSOLUTE | Camera Terminal |
| Brightness | PU_BRIGHTNESS | Processing Unit |
| Contrast | PU_CONTRAST | Processing Unit |
| WhiteBalance | PU_WHITE_BALANCE_TEMPERATURE | Processing Unit |
| AutoWhiteBalance | PU_WHITE_BALANCE_TEMPERATURE_AUTO | Processing Unit |
| Gain | PU_GAIN | Processing Unit |

**Hotplug**: When a USB camera is disconnected during an active session, the driver:

1. Sends `CameraEvent::DeviceRemoved` to all active sessions
2. Sessions receive an error on the next frame delivery attempt
3. The privacy indicator is deactivated
4. An audit entry records the unexpected disconnection
5. The camera subsystem removes the device from the registry

### §7.2 CSI/MIPI Driver

The CSI driver handles MIPI CSI-2 connected cameras, primarily Raspberry Pi Camera Modules. It manages the CSI-2 receiver hardware, the I2C/CCI sensor interface, and DMA frame transfers.

```rust
/// CSI/MIPI camera driver.
pub struct CsiCameraDriver {
    /// Camera identifier.
    camera_id: CameraId,
    /// CSI-2 receiver registers (MMIO base).
    csi_receiver: CsiReceiverRegs,
    /// Image sensor control (I2C/CCI).
    sensor: Box<dyn SensorDriver>,
    /// DMA engine for frame transfers.
    dma: DmaChannel,
    /// Active sensor mode.
    current_mode: Option<SensorMode>,
    /// Buffer pool for DMA frame reception.
    buffer_pool: CameraBufferPool,
    /// Frame completion interrupt handler state.
    irq_state: CsiIrqState,
    /// 3A algorithm state (AE, AWB, AF).
    auto_control: AutoControlState,
    /// Privacy indicator GPIO (if available).
    indicator_gpio: Option<GpioPin>,
}
```

#### Sensor Subdevice Interface

Each image sensor is controlled via a sensor-specific driver that implements the `SensorDriver` trait:

```rust
/// Interface to a specific image sensor.
pub trait SensorDriver: Send + Sync {
    /// Sensor identification.
    fn sensor_id(&self) -> &str;

    /// Available sensor modes (resolution × format × fps combinations).
    fn modes(&self) -> &[SensorMode];

    /// Apply a sensor mode (resolution, format, lanes, link frequency).
    fn set_mode(&mut self, mode: &SensorMode) -> Result<(), DriverError>;

    /// Start streaming from the sensor.
    fn stream_on(&mut self) -> Result<(), DriverError>;

    /// Stop streaming.
    fn stream_off(&mut self) -> Result<(), DriverError>;

    /// Set exposure time in microseconds.
    fn set_exposure(&mut self, exposure_us: u32) -> Result<(), DriverError>;

    /// Set analog gain (1.0 = unity).
    fn set_analog_gain(&mut self, gain: f32) -> Result<(), DriverError>;

    /// Set digital gain (1.0 = unity).
    fn set_digital_gain(&mut self, gain: f32) -> Result<(), DriverError>;

    /// Trigger auto-focus (if supported). Returns focus position.
    fn auto_focus(&mut self) -> Result<Option<u32>, DriverError>;

    /// Set manual focus position.
    fn set_focus(&mut self, position: u32) -> Result<(), DriverError>;

    /// Read sensor temperature (for black level compensation).
    fn temperature(&self) -> Result<Option<i32>, DriverError>;

    /// Sensor-specific calibration data (black level, dead pixels, lens shading).
    fn calibration(&self) -> &SensorCalibration;
}
```

#### Supported Sensors

Initial sensor driver implementations:

| Sensor | Driver | Key Features |
|---|---|---|
| IMX219 | `Imx219Driver` | 8MP, 2-lane CSI-2, fixed focus |
| IMX477 | `Imx477Driver` | 12.3MP, 2-lane CSI-2, C/CS-mount lens |
| IMX708 | `Imx708Driver` | 11.9MP, 2-lane CSI-2, PDAF autofocus, HDR |

Each sensor driver implements I2C register access using the platform's I2C controller. Register maps are derived from sensor datasheets and existing open-source drivers (Linux kernel `drivers/media/i2c/` as reference).

#### CSI-2 Receiver

The CSI-2 receiver deserializes MIPI D-PHY signals and writes raw frame data to memory:

```rust
/// CSI-2 receiver hardware registers.
pub struct CsiReceiverRegs {
    /// MMIO base address.
    base: usize,
}

impl CsiReceiverRegs {
    /// Configure the receiver for the given sensor mode.
    pub fn configure(&mut self, mode: &SensorMode) -> Result<(), DriverError> {
        // Set number of active data lanes
        // Set expected data type (RAW8/10/12)
        // Configure DMA target address and stride
        // Enable frame-end interrupt
        // Start receiver
        todo!()
    }

    /// Read receiver status (error flags, frame count).
    pub fn status(&self) -> CsiStatus {
        todo!()
    }
}
```

#### DMA Frame Transfer

The CSI receiver writes frame data to memory via DMA:

1. **Pre-allocated buffers** — the driver allocates DMA buffers from the DMA page pool at session start
2. **Ping-pong** — two buffers alternate: while the DMA writes to buffer A, the ISP processes buffer B
3. **Frame-end interrupt** — signals that a complete frame is in the buffer, triggering ISP processing
4. **Error handling** — CRC errors, short frames, and line count mismatches are detected via receiver status registers; corrupted frames are dropped

### §7.3 VirtIO-Camera Driver

The VirtIO-Camera driver provides a virtual camera for QEMU-based development and testing. It generates synthetic frames without real camera hardware.

```rust
/// VirtIO-Camera virtual camera driver.
pub struct VirtioCameraDriver {
    /// Camera identifier.
    camera_id: CameraId,
    /// VirtIO MMIO transport.
    transport: VirtioMmioTransport,
    /// Command/response virtqueue.
    cmd_queue: Virtqueue,
    /// Frame data virtqueue.
    frame_queue: Virtqueue,
    /// Device configuration.
    config: VirtioCameraConfig,
    /// Current test pattern generator.
    pattern: TestPattern,
    /// Frame sequence counter.
    sequence: u64,
    /// Buffer pool.
    buffer_pool: CameraBufferPool,
}
```

#### Test Pattern Generation

The VirtIO-Camera generates configurable test patterns for development:

```rust
pub enum TestPattern {
    /// SMPTE color bars (standard broadcast test pattern).
    ColorBars,
    /// Horizontal gradient (black to white).
    Gradient,
    /// 8×8 checkerboard (useful for resolution testing).
    Checkerboard,
    /// Moving diagonal lines (useful for motion/timing testing).
    MovingLines { speed: u32 },
    /// Solid color (useful for white balance and exposure testing).
    SolidColor { r: u8, g: u8, b: u8 },
    /// Frame counter overlay (displays frame number for timing verification).
    FrameCounter,
    /// Random noise (useful for ISP denoising testing).
    Noise { intensity: u8 },
}
```

The VirtIO host (QEMU) can also inject frames from a file or webcam passthrough, enabling testing with real-world imagery without requiring a physical camera on the target device.

#### Simulated Features

The VirtIO-Camera simulates:

- **Format negotiation** — accepts any format within configured capabilities
- **Camera controls** — brightness, contrast, exposure affect the generated pattern
- **Privacy indicator** — virtual LED state queryable via VirtIO config space
- **Hotplug** — can be dynamically added/removed via QEMU monitor
- **Multi-camera** — configurable number of virtual cameras (each with independent patterns)
- **Error injection** — configurable frame drop rate, corruption, and latency for robustness testing

### §7.4 Platform-Specific Drivers

#### Raspberry Pi Camera Platform

The Pi Camera platform driver ties together the CSI receiver, ISP, and sensor drivers into a complete camera pipeline specific to the Raspberry Pi hardware:

```rust
/// Raspberry Pi camera platform driver.
pub struct PiCameraDriver {
    /// CSI-2 receiver driver (bcm2835-unicam equivalent).
    csi: CsiCameraDriver,
    /// Hardware ISP interface (VideoCore VII on Pi 5).
    isp: Option<PiIspDriver>,
    /// Which CSI port (Pi 5 has two: CAM0, CAM1).
    port: CsiPort,
}

pub enum CsiPort {
    /// Primary CSI port (CAM0).
    Cam0,
    /// Secondary CSI port (CAM1, Pi 5 only).
    Cam1,
}
```

On Raspberry Pi 5, the hardware ISP is mandatory — raw sensor data passes through the VideoCore VII ISP before reaching the camera subsystem. The `PiIspDriver` configures ISP registers based on 3A algorithm output.

#### Future: Apple ISP

Apple Silicon devices (if targeted) use a proprietary ISP that handles the full sensor-to-output pipeline internally. The Apple ISP driver would expose the ISP as a black box, accepting high-level parameters (scene mode, HDR enable, face detection hints) rather than per-stage ISP controls.

### §7.5 CameraDevice Trait

All camera drivers implement the `CameraDevice` trait, which extends the subsystem framework's `DeviceClass`:

```rust
/// Camera device interface — all camera drivers implement this.
pub trait CameraDevice: DeviceClass + Send + Sync {
    /// Query camera capabilities (formats, resolutions, controls).
    fn capabilities(&self) -> &CameraCapabilitiesDescriptor;

    /// Configure the camera for streaming.
    fn configure(&mut self, config: &CameraConfig) -> Result<NegotiationResult, DriverError>;

    /// Start frame capture. Returns a receiver for completed frames.
    fn start_capture(&mut self) -> Result<FrameReceiver, DriverError>;

    /// Stop frame capture.
    fn stop_capture(&mut self) -> Result<(), DriverError>;

    /// Set a camera control value.
    fn set_control(&mut self, control: CameraControl, value: ControlValue)
        -> Result<(), DriverError>;

    /// Get a camera control's current value and range.
    fn get_control(&self, control: CameraControl)
        -> Result<(ControlValue, ControlRange<ControlValue>), DriverError>;

    /// Capture a single still image at full resolution.
    fn capture_still(&mut self, request: &StillCaptureRequest)
        -> Result<RawFrame, DriverError>;

    /// Whether this camera has a hardware privacy indicator LED.
    fn has_indicator_led(&self) -> bool;

    /// Control the hardware privacy indicator LED.
    fn set_indicator_led(&mut self, active: bool) -> Result<(), DriverError>;

    /// Whether a hardware privacy shutter is detected (e.g., closed).
    fn privacy_shutter_closed(&self) -> bool { false }

    /// Whether the device supports synchronized capture with other cameras.
    fn sync_capable(&self) -> bool { false }
}

/// Result of format negotiation.
pub enum NegotiationResult {
    /// Requested format accepted exactly.
    Accepted(CameraConfig),
    /// Format adjusted — actual parameters differ from requested.
    Adjusted {
        requested: CameraConfig,
        actual: CameraConfig,
        reason: &'static str,
    },
}

/// Channel for receiving completed frames from the driver.
pub struct FrameReceiver {
    /// Ring buffer of completed raw frames.
    ring: RingBuffer<RawFrame, 8>,
}

/// The depth device trait for depth/ToF sensors.
pub trait DepthDevice: CameraDevice {
    /// Maximum depth range in millimeters.
    fn max_range_mm(&self) -> u32;

    /// Depth accuracy at 1 meter in millimeters.
    fn accuracy_at_1m_mm(&self) -> u32;

    /// Whether this sensor provides confidence values per pixel.
    fn has_confidence(&self) -> bool;

    /// Start depth capture. Returns depth frames with per-pixel distance.
    fn start_depth_capture(&mut self) -> Result<FrameReceiver, DriverError>;
}
```

#### Driver Registration

Camera drivers register with the camera subsystem during device discovery:

```rust
impl CameraSubsystem {
    /// Called by device discovery when a camera device is found.
    pub fn device_added(&mut self, descriptor: HardwareDescriptor)
        -> Result<CameraId, CameraError>
    {
        // 1. Create appropriate driver instance based on descriptor
        // 2. Query capabilities
        // 3. Register in DeviceRegistry
        // 4. Update camera topology (groups)
        // 5. Log discovery to audit
        // 6. Return camera ID for future reference
        todo!()
    }

    /// Called when a camera device is removed (hotplug).
    pub fn device_removed(&mut self, camera_id: CameraId)
        -> Result<(), CameraError>
    {
        // 1. Terminate all active sessions on this camera
        // 2. Deactivate privacy indicator
        // 3. Release driver resources
        // 4. Remove from DeviceRegistry and topology
        // 5. Log removal to audit
        todo!()
    }
}
```
