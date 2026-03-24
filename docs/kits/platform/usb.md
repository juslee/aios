# USB Kit

**Layer:** Platform | **Crate:** `aios_usb` | **Architecture:** [`docs/platform/usb.md`](../../platform/usb.md)

## 1. Overview

USB Kit provides a unified abstraction over USB host controllers, device enumeration, class
drivers, and hotplug lifecycle management. It hides the differences between xHCI and DWC2
controller hardware behind the `UsbHostController` trait, while exposing capability-gated
access to attached peripherals through typed device handles. USB Kit is primarily a
*plumbing layer* -- most application developers will never import it directly. Instead,
upper-layer Kits ([Input Kit](./input.md), [Audio Kit](./audio.md), [Storage Kit](./storage.md),
[Camera Kit](./camera.md)) consume the class driver interfaces that USB Kit provides.

The main reason an application developer would interact with USB Kit directly is to
subscribe to hotplug events -- for example, to detect when a specific USB accessory is
connected and present UI accordingly. USB Kit's `HotplugManager` delivers typed
attach/detach events through a subscription channel, with each event carrying the device's
descriptor tree and the capabilities required to open it. All device access is gated by
the capability system: an agent cannot communicate with a USB device unless its manifest
declares the appropriate `UsbDeviceAccess` capability.

USB Kit also manages per-device power states, enabling selective suspend for idle
peripherals and coordinating with [Power Kit](./power.md) to prevent devices from being
suspended while transfers are in flight. IOMMU integration ensures that DMA buffers
allocated by class drivers are isolated per agent, preventing one agent's USB device from
reading another agent's memory.

## 2. Core Traits

```rust
use aios_capability::CapabilityHandle;
use aios_memory::{DmaBuffer, DmaPool};

/// A USB host controller abstraction covering xHCI and DWC2 hardware.
///
/// Application code never calls this directly -- it is used internally
/// by USB Kit to manage the controller lifecycle. Exposed here for
/// driver authors writing new host controller implementations.
pub trait UsbHostController {
    /// Initialize the controller hardware and begin port monitoring.
    fn init(&mut self) -> Result<(), UsbError>;

    /// Return the number of root hub ports on this controller.
    fn port_count(&self) -> u8;

    /// Query the current connection status of a root hub port.
    fn port_status(&self, port: u8) -> Result<PortStatus, UsbError>;

    /// Reset a port, triggering device enumeration on the attached device.
    fn port_reset(&mut self, port: u8) -> Result<(), UsbError>;

    /// Submit a transfer request to the controller's schedule.
    fn submit_transfer(&mut self, transfer: TransferRequest) -> Result<TransferId, UsbError>;

    /// Cancel a pending transfer.
    fn cancel_transfer(&mut self, id: TransferId) -> Result<(), UsbError>;

    /// Allocate a DMA buffer suitable for this controller's alignment requirements.
    fn alloc_dma_buffer(&self, size: usize) -> Result<DmaBuffer, UsbError>;
}

/// An enumerated USB device with cached descriptors.
///
/// Obtained from hotplug events or device enumeration. Opening the device
/// for I/O requires the `UsbDeviceAccess` capability.
pub trait UsbDevice {
    /// The device's unique bus address (bus, port, device number).
    fn address(&self) -> UsbAddress;

    /// The device descriptor (vendor ID, product ID, class, etc.).
    fn descriptor(&self) -> &DeviceDescriptor;

    /// All configuration descriptors for this device.
    fn configurations(&self) -> &[ConfigurationDescriptor];

    /// The currently active configuration, if any.
    fn active_configuration(&self) -> Option<u8>;

    /// Select a configuration, activating its interfaces.
    fn set_configuration(&mut self, config: u8, cap: &CapabilityHandle) -> Result<(), UsbError>;

    /// Perform a control transfer on endpoint 0.
    fn control_transfer(
        &self,
        request: ControlRequest,
        cap: &CapabilityHandle,
    ) -> Result<Vec<u8>, UsbError>;
}

/// USB HID class driver that feeds normalized events into Input Kit.
///
/// Most developers interact with HID devices through Input Kit's event
/// stream rather than this trait directly.
pub trait HidDriver {
    /// The HID report descriptor parsed from the device.
    fn report_descriptor(&self) -> &HidReportDescriptor;

    /// Subscribe to parsed HID input reports.
    fn on_report(&self, handler: Box<dyn Fn(&HidReport) + Send>) -> SubscriptionId;

    /// Send an output report to the device (e.g., keyboard LEDs).
    fn send_output_report(&self, report: &[u8], cap: &CapabilityHandle) -> Result<(), UsbError>;
}

/// USB Mass Storage class driver exposing block device access.
///
/// Storage Kit consumes this interface to present USB drives as Spaces.
pub trait StorageDriver {
    /// The storage capacity in bytes.
    fn capacity(&self) -> u64;

    /// The logical block size in bytes (typically 512).
    fn block_size(&self) -> u32;

    /// Read blocks into a DMA buffer.
    fn read_blocks(
        &self,
        lba: u64,
        count: u32,
        buffer: &mut DmaBuffer,
        cap: &CapabilityHandle,
    ) -> Result<(), UsbError>;

    /// Write blocks from a DMA buffer.
    fn write_blocks(
        &self,
        lba: u64,
        count: u32,
        buffer: &DmaBuffer,
        cap: &CapabilityHandle,
    ) -> Result<(), UsbError>;
}

/// Hotplug lifecycle manager for USB device attach/detach events.
pub trait HotplugManager {
    /// Subscribe to all hotplug events matching an optional filter.
    fn subscribe(
        &self,
        filter: Option<HotplugFilter>,
    ) -> Result<HotplugSubscription, UsbError>;

    /// List all currently attached USB devices.
    fn attached_devices(&self) -> Result<Vec<Box<dyn UsbDevice>>, UsbError>;

    /// Request safe removal of a device (flush pending transfers, notify class drivers).
    fn request_removal(&self, address: UsbAddress) -> Result<(), UsbError>;
}
```

**Key types:**

```rust
/// Filter criteria for hotplug subscriptions.
pub struct HotplugFilter {
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub device_class: Option<UsbClass>,
}

/// A hotplug event delivered through a subscription.
pub enum HotplugEvent {
    /// A new device was attached and enumerated.
    Attached { device: Box<dyn UsbDevice> },
    /// A device was physically removed.
    Detached { address: UsbAddress },
    /// A device entered an error state.
    Error { address: UsbAddress, error: UsbError },
}
```

## 3. Usage Patterns

**Subscribe to hotplug events (the most common app-level pattern):**

```rust
use aios_usb::{HotplugManager, HotplugFilter, HotplugEvent, UsbClass};

let hotplug = aios_usb::hotplug_manager();

// Watch for USB storage devices only
let subscription = hotplug.subscribe(Some(HotplugFilter {
    vendor_id: None,
    product_id: None,
    device_class: Some(UsbClass::MassStorage),
}))?;

loop {
    match subscription.recv().await {
        HotplugEvent::Attached { device } => {
            let desc = device.descriptor();
            println!(
                "USB drive connected: {} ({}:{:04x}:{:04x})",
                desc.product_string.as_deref().unwrap_or("Unknown"),
                desc.manufacturer_string.as_deref().unwrap_or(""),
                desc.vendor_id,
                desc.product_id,
            );
        }
        HotplugEvent::Detached { address } => {
            println!("USB device removed: {:?}", address);
        }
        HotplugEvent::Error { address, error } => {
            eprintln!("USB error on {:?}: {}", address, error);
        }
    }
}
```

**List currently attached devices:**

```rust
use aios_usb::HotplugManager;

let hotplug = aios_usb::hotplug_manager();
for device in hotplug.attached_devices()? {
    let desc = device.descriptor();
    println!(
        "  Bus {:03}:{:03} — {:04x}:{:04x} {}",
        device.address().bus,
        device.address().device,
        desc.vendor_id,
        desc.product_id,
        desc.product_string.as_deref().unwrap_or("(unknown)"),
    );
}
```

**Send an output report to a HID device (e.g., set keyboard LEDs):**

```rust
use aios_usb::HidDriver;
use aios_capability::CapabilityHandle;

fn set_keyboard_leds(hid: &dyn HidDriver, caps_lock: bool, cap: &CapabilityHandle) {
    let mut report = [0u8; 1];
    if caps_lock {
        report[0] |= 0x02; // Caps Lock LED
    }
    hid.send_output_report(&report, cap).ok();
}
```

> **Common Mistakes**
>
> - **Polling for devices instead of subscribing.** Use `HotplugManager::subscribe()` with
>   a filter rather than repeatedly calling `attached_devices()`. The subscription channel
>   delivers events immediately and uses no CPU while idle.
> - **Ignoring the `Detached` event.** Always handle detach to clean up UI or file handles.
>   A detached device's methods will return `UsbError::DeviceDisconnected`.
> - **Holding DMA buffers too long.** DMA buffers are allocated from the DMA pool, which
>   has a fixed capacity. Release them promptly after the transfer completes.

## 4. Integration Examples

**USB Kit + Input Kit -- HID device to input events:**

```rust
use aios_usb::{HotplugManager, HotplugEvent, UsbClass, HotplugFilter};
use aios_input::{InputKit, InputEvent};

// USB Kit automatically routes HID devices to Input Kit during enumeration.
// As an app developer, you receive input events from Input Kit, not USB Kit.

let input = InputKit::event_stream();
while let Some(event) = input.next().await {
    match event {
        InputEvent::KeyPress { key, device, .. } => {
            // `device` identifies the source -- including USB HID devices
            println!("Key {:?} from device {:?}", key, device);
        }
        _ => {}
    }
}
```

**USB Kit + Storage Kit -- USB drive as a Space:**

```rust
use aios_usb::{HotplugManager, HotplugEvent, UsbClass, HotplugFilter};
use aios_storage::SpaceKit;

// When a USB mass storage device is attached, Storage Kit automatically
// mounts it as an external Space. Apps discover it through Space Kit.

let spaces = SpaceKit::list_spaces()?;
for space in spaces {
    if space.storage_tier() == StorageTier::External {
        println!("External drive: {} ({} free)", space.name(), space.free_bytes());
    }
}
```

**USB Kit + Capability Kit -- gated device access:**

```rust
use aios_usb::{HotplugManager, HotplugEvent};
use aios_capability::CapabilityKit;

let hotplug = aios_usb::hotplug_manager();
let sub = hotplug.subscribe(None)?;

if let HotplugEvent::Attached { device } = sub.recv().await {
    // Attempting to configure without capability fails
    let cap = CapabilityKit::request("UsbDeviceAccess")?;
    device.set_configuration(1, &cap)?;
}
```

## 5. Capability Requirements

| Method | Required Capability | Default Grant |
| --- | --- | --- |
| `HotplugManager::subscribe` | `UsbHotplugMonitor` | Granted to all agents |
| `HotplugManager::attached_devices` | `UsbHotplugMonitor` | Granted to all agents |
| `UsbDevice::set_configuration` | `UsbDeviceAccess` | Prompt user on first use |
| `UsbDevice::control_transfer` | `UsbDeviceAccess` | Prompt user on first use |
| `HidDriver::send_output_report` | `UsbDeviceAccess` | Prompt user on first use |
| `StorageDriver::read_blocks` | `UsbStorageRead` | Prompt user on first use |
| `StorageDriver::write_blocks` | `UsbStorageWrite` | Prompt user on first use |
| `HotplugManager::request_removal` | `UsbDeviceAccess` | Prompt user on first use |

**Agent manifest example:**

```toml
[capabilities.required]
UsbHotplugMonitor = "Monitor USB device connections"

[capabilities.optional]
UsbDeviceAccess = "Configure and communicate with USB accessories"
UsbStorageRead = "Read files from USB drives"
```

## 6. Error Handling

```rust
/// Errors returned by USB Kit operations.
#[derive(Debug)]
pub enum UsbError {
    /// The USB device was physically disconnected.
    DeviceDisconnected(UsbAddress),

    /// A transfer timed out waiting for the device to respond.
    TransferTimeout { endpoint: u8, timeout_ms: u32 },

    /// The device returned a STALL condition on the endpoint.
    Stalled { endpoint: u8 },

    /// The required capability was not granted.
    CapabilityDenied(String),

    /// The requested configuration or interface does not exist.
    InvalidConfiguration(u8),

    /// The DMA pool is exhausted; retry after freeing buffers.
    DmaPoolExhausted,

    /// The host controller encountered an unrecoverable error.
    ControllerError(String),

    /// The device descriptor could not be parsed.
    MalformedDescriptor(String),

    /// The device class driver failed to bind.
    DriverBindFailed { class: UsbClass, reason: String },

    /// IOMMU mapping failed for the device's DMA region.
    IommuError(String),
}
```

**Recovery guidance:**

| Error | Recovery |
| --- | --- |
| `DeviceDisconnected` | Clean up references; device is gone |
| `TransferTimeout` | Retry once, then assume device is hung; call `request_removal` |
| `Stalled` | Issue a clear-halt control transfer, then retry |
| `CapabilityDenied` | Request the capability or inform the user |
| `DmaPoolExhausted` | Free completed transfer buffers and retry |
| `ControllerError` | Controller reset in progress; wait for hotplug re-enumeration |
| `DriverBindFailed` | Device unsupported; inform user, no automatic recovery |

## 7. Platform & AI Availability

**Platform support:**

| Platform | xHCI | DWC2 | Hotplug | Notes |
| --- | --- | --- | --- | --- |
| QEMU virt | Emulated | No | Yes | Via QEMU USB passthrough |
| Raspberry Pi 4 | Via VL805 | Yes (OTG) | Yes | DWC2 on the OTG port |
| Raspberry Pi 5 | Native | No | Yes | Native xHCI controller |
| Apple Silicon | Native | No | Yes | Thunderbolt/USB4 via xHCI |

**AIRS-enhanced features:**

| Feature | What AIRS provides | Without AIRS |
| --- | --- | --- |
| Device identification | Identifies unknown devices by usage pattern | Descriptor-only identification |
| Anomaly detection | Detects BadUSB-style attacks from behavioral anomalies | Static allowlist only |
| Power optimization | Learns per-device idle patterns for optimal suspend timing | Fixed 30-second idle timeout |
| Driver selection | Recommends best class driver for multi-function devices | First-match binding |

**Feature detection:**

```rust
use aios_usb::UsbKit;

if UsbKit::is_available() {
    let hotplug = aios_usb::hotplug_manager();
    let count = hotplug.attached_devices()?.len();
    println!("{} USB devices connected", count);
} else {
    // No USB host controller present (e.g., minimal QEMU config)
    println!("USB not available on this platform");
}
```

**Implementation phase:** Phase 9+. USB Kit depends on [Memory Kit](../kernel/memory.md)
(DMA buffer allocation) and [Capability Kit](../kernel/capability.md) (device access gating).

---

*See also: [Input Kit](./input.md) | [Audio Kit](./audio.md) | [Storage Kit](./storage.md) | [Camera Kit](./camera.md) | [Power Kit](./power.md)*
