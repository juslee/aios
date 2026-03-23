# Wireless Kit

**Layer:** Platform | **Crate:** `aios_wireless` | **Architecture:** [`docs/platform/wireless.md`](../../platform/wireless.md)

## 1. Overview

Wireless Kit provides WiFi and Bluetooth connectivity through unified trait abstractions
that hide the complexity of radio hardware, firmware management, and protocol stacks. It
exposes station management for WiFi (scan, connect, roam) and adapter management for
Bluetooth (classic profiles and BLE GATT), while handling WPA2/WPA3 authentication,
firmware blob loading, and radio coexistence internally.

Application developers use Wireless Kit when they need to interact with wireless
connectivity beyond what [Network Kit](./network.md) provides. Network Kit handles
TCP/IP-level networking and is the right choice for HTTP requests, socket connections, and
DNS resolution. Wireless Kit is for lower-level wireless concerns: scanning for WiFi
networks, managing Bluetooth pairings, communicating with BLE peripherals (fitness
trackers, IoT sensors, smart home devices), or checking signal strength. If your agent
only needs internet access, use Network Kit instead.

Wireless Kit enforces capability-gated access at every layer. Scanning for WiFi networks
requires `WifiScan`, connecting requires `WifiConnect`, and BLE GATT operations require
`BluetoothAccess`. Firmware blobs are loaded through a sandboxed `FirmwareLoader` that
validates signatures and enforces regulatory domain restrictions -- an agent cannot
instruct the radio to operate on frequencies prohibited in the device's configured region.
All radio state changes are logged to the audit trail.

## 2. Core Traits

```rust
use aios_capability::CapabilityHandle;
use aios_network::NetworkInterface;

/// WiFi station management for scanning, connecting, and roaming.
///
/// Represents the device's WiFi radio in station (client) mode.
/// Only one station connection is active at a time.
pub trait WifiStation {
    /// Scan for available access points. Returns a list sorted by signal strength.
    fn scan(&self, cap: &CapabilityHandle) -> Result<Vec<AccessPoint>, WirelessError>;

    /// Connect to a network by SSID. Credentials are resolved from the
    /// credential vault or prompted from the user.
    fn connect(
        &mut self,
        ssid: &Ssid,
        credentials: Option<WifiCredentials>,
        cap: &CapabilityHandle,
    ) -> Result<ConnectionHandle, WirelessError>;

    /// Disconnect from the current network.
    fn disconnect(&mut self, cap: &CapabilityHandle) -> Result<(), WirelessError>;

    /// Return the current connection status and signal metrics.
    fn status(&self) -> ConnectionStatus;

    /// Subscribe to connection state change events.
    fn on_state_change(&self, handler: Box<dyn Fn(ConnectionStatus) + Send>) -> SubscriptionId;

    /// Return the underlying network interface for use with Network Kit.
    fn network_interface(&self) -> Result<&dyn NetworkInterface, WirelessError>;
}

/// Bluetooth adapter supporting classic and BLE modes.
///
/// Manages discovery, pairing, and profile connections for classic Bluetooth,
/// and provides access to the BLE GATT client through `ble_gatt()`.
pub trait BluetoothAdapter {
    /// Start Bluetooth device discovery. Results arrive through the callback.
    fn start_discovery(
        &self,
        filter: Option<DiscoveryFilter>,
        cap: &CapabilityHandle,
    ) -> Result<DiscoverySession, WirelessError>;

    /// Pair with a discovered Bluetooth device.
    fn pair(
        &mut self,
        address: BtAddress,
        cap: &CapabilityHandle,
    ) -> Result<PairedDevice, WirelessError>;

    /// List all paired devices.
    fn paired_devices(&self) -> Result<Vec<PairedDevice>, WirelessError>;

    /// Remove a pairing.
    fn unpair(
        &mut self,
        address: BtAddress,
        cap: &CapabilityHandle,
    ) -> Result<(), WirelessError>;

    /// Access the BLE GATT client for peripheral communication.
    fn ble_gatt(&self) -> &dyn BleGatt;

    /// Return the adapter's current power state and address.
    fn info(&self) -> AdapterInfo;
}

/// BLE GATT client for communicating with Bluetooth Low Energy peripherals.
///
/// Provides service discovery, characteristic read/write, and notification
/// subscription for BLE devices.
pub trait BleGatt {
    /// Connect to a BLE peripheral by address.
    fn connect(
        &self,
        address: BtAddress,
        cap: &CapabilityHandle,
    ) -> Result<GattConnection, WirelessError>;

    /// Discover all services on a connected peripheral.
    fn discover_services(
        &self,
        conn: &GattConnection,
    ) -> Result<Vec<GattService>, WirelessError>;

    /// Read a characteristic value.
    fn read_characteristic(
        &self,
        conn: &GattConnection,
        characteristic: &GattCharacteristic,
        cap: &CapabilityHandle,
    ) -> Result<Vec<u8>, WirelessError>;

    /// Write a characteristic value.
    fn write_characteristic(
        &self,
        conn: &GattConnection,
        characteristic: &GattCharacteristic,
        value: &[u8],
        cap: &CapabilityHandle,
    ) -> Result<(), WirelessError>;

    /// Subscribe to characteristic notifications.
    fn subscribe_notifications(
        &self,
        conn: &GattConnection,
        characteristic: &GattCharacteristic,
        handler: Box<dyn Fn(&[u8]) + Send>,
        cap: &CapabilityHandle,
    ) -> Result<SubscriptionId, WirelessError>;
}

/// WPA2/WPA3 supplicant managing authentication and key exchange.
///
/// Used internally by WifiStation. Exposed for advanced use cases
/// like custom EAP methods in enterprise environments.
pub trait WpaSupplicant {
    /// The authentication method in use (WPA2-PSK, WPA3-SAE, EAP, etc.).
    fn auth_method(&self) -> AuthMethod;

    /// Whether the current connection uses WPA3-SAE (preferred).
    fn is_wpa3(&self) -> bool;

    /// The credential source for the current connection.
    fn credential_source(&self) -> CredentialSource;
}

/// Firmware loader for wireless chipset firmware blobs.
///
/// Validates firmware signatures and enforces regulatory domain
/// restrictions. Not typically called by application code.
pub trait FirmwareLoader {
    /// Load firmware for the specified chipset and regulatory domain.
    fn load(
        &self,
        chipset: &ChipsetId,
        domain: RegulatoryDomain,
    ) -> Result<FirmwareImage, WirelessError>;

    /// Return the currently loaded firmware version, if any.
    fn current_version(&self) -> Option<FirmwareVersion>;

    /// Check whether a firmware update is available.
    fn update_available(&self) -> Result<Option<FirmwareVersion>, WirelessError>;
}
```

**Key types:**

```rust
/// A discovered WiFi access point.
pub struct AccessPoint {
    pub ssid: Ssid,
    pub bssid: [u8; 6],
    pub signal_dbm: i8,
    pub frequency_mhz: u32,
    pub security: SecurityType,
    pub channel_width: ChannelWidth,
}

/// WiFi security types.
pub enum SecurityType {
    Open,
    Wpa2Psk,
    Wpa3Sae,
    Wpa2Enterprise,
    Wpa3Enterprise,
}

/// BLE GATT service with its characteristics.
pub struct GattService {
    pub uuid: Uuid,
    pub characteristics: Vec<GattCharacteristic>,
    pub is_primary: bool,
}
```

## 3. Usage Patterns

**Scan and connect to a WiFi network:**

```rust
use aios_wireless::{WifiStation, SecurityType};
use aios_capability::CapabilityKit;

let wifi = aios_wireless::wifi_station();
let cap = CapabilityKit::request("WifiScan")?;

// Scan for networks
let networks = wifi.scan(&cap)?;
for ap in &networks {
    println!(
        "  {} ({} dBm, {:?})",
        ap.ssid, ap.signal_dbm, ap.security
    );
}

// Connect to a known network (credentials from vault or prompt)
let connect_cap = CapabilityKit::request("WifiConnect")?;
let handle = wifi.connect(&networks[0].ssid, None, &connect_cap)?;
println!("Connected: {:?}", wifi.status());
```

**Communicate with a BLE heart rate monitor:**

```rust
use aios_wireless::{BluetoothAdapter, BleGatt, DiscoveryFilter};
use aios_capability::CapabilityKit;

let bt = aios_wireless::bluetooth_adapter();
let cap = CapabilityKit::request("BluetoothAccess")?;

// Discover heart rate monitors (BLE service UUID 0x180D)
let session = bt.start_discovery(
    Some(DiscoveryFilter {
        service_uuids: vec![Uuid::from_u16(0x180D)],
        ..Default::default()
    }),
    &cap,
)?;

let device = session.next().await?;
let gatt = bt.ble_gatt();
let conn = gatt.connect(device.address, &cap)?;

// Find heart rate measurement characteristic (0x2A37)
let services = gatt.discover_services(&conn)?;
let hr_char = services.iter()
    .flat_map(|s| &s.characteristics)
    .find(|c| c.uuid == Uuid::from_u16(0x2A37))
    .ok_or(WirelessError::ServiceNotFound)?;

// Subscribe to heart rate notifications
gatt.subscribe_notifications(&conn, hr_char, Box::new(|data| {
    let bpm = data[1]; // Simplified; real parsing handles flags byte
    println!("Heart rate: {} BPM", bpm);
}), &cap)?;
```

**Monitor WiFi connection state:**

```rust
use aios_wireless::{WifiStation, ConnectionStatus};

let wifi = aios_wireless::wifi_station();
wifi.on_state_change(Box::new(|status| {
    match status {
        ConnectionStatus::Connected { ssid, signal_dbm, .. } => {
            println!("WiFi connected: {} ({} dBm)", ssid, signal_dbm);
        }
        ConnectionStatus::Disconnected { reason } => {
            println!("WiFi disconnected: {:?}", reason);
        }
        ConnectionStatus::Connecting { ssid } => {
            println!("Connecting to {}...", ssid);
        }
    }
}));
```

> **Common Mistakes**
>
> - **Scanning too frequently.** WiFi scans temporarily interrupt data transfer on the
>   current connection. Limit scans to user-initiated actions or at most once per 30
>   seconds. AIRS can advise optimal scan intervals.
> - **Not handling BLE disconnects.** BLE peripherals disconnect frequently (out of range,
>   battery, interference). Always register a disconnect handler and implement reconnection.
> - **Storing WiFi credentials directly.** Use the credential vault integration
>   (`WifiCredentials::FromVault`) instead of hardcoding passwords in your agent.

## 4. Integration Examples

**Wireless Kit + Network Kit -- WiFi as a network interface:**

```rust
use aios_wireless::WifiStation;
use aios_network::{NetworkKit, HttpClient};

let wifi = aios_wireless::wifi_station();

// WiFi connection provides a NetworkInterface to Network Kit
if let ConnectionStatus::Connected { .. } = wifi.status() {
    let iface = wifi.network_interface()?;

    // Network Kit uses the WiFi interface for HTTP requests
    let client = NetworkKit::http_client();
    let response = client.get("https://api.example.com/data").await?;
    println!("Response: {}", response.status());
}
```

**Wireless Kit + Audio Kit -- Bluetooth A2DP streaming:**

```rust
use aios_wireless::{BluetoothAdapter, ClassicProfile};
use aios_audio::{AudioKit, AudioRoute};

let bt = aios_wireless::bluetooth_adapter();
let cap = aios_capability::CapabilityKit::request("BluetoothAccess")?;

// Pair with a Bluetooth speaker
let speaker = bt.pair(speaker_address, &cap)?;

// Audio Kit automatically discovers A2DP-capable paired devices
// and makes them available as audio output routes
let routes = AudioKit::available_routes();
for route in routes {
    if route.device_type() == AudioDeviceType::BluetoothA2dp {
        AudioKit::set_output_route(&route)?;
        println!("Audio output set to: {}", route.name());
        break;
    }
}
```

**Wireless Kit + Input Kit -- Bluetooth HID gamepad:**

```rust
use aios_wireless::{BluetoothAdapter, DiscoveryFilter};
use aios_input::{InputKit, InputEvent, GamepadButton};

// Bluetooth HID devices are automatically routed to Input Kit after pairing.
// The app just listens for input events.

let bt = aios_wireless::bluetooth_adapter();
let cap = aios_capability::CapabilityKit::request("BluetoothAccess")?;

// Pair the gamepad (one-time setup)
let gamepad = bt.pair(gamepad_address, &cap)?;

// Receive gamepad events through Input Kit
let input = InputKit::event_stream();
while let Some(event) = input.next().await {
    if let InputEvent::GamepadButton { button, pressed, .. } = event {
        println!("Gamepad {:?}: {}", button, if pressed { "pressed" } else { "released" });
    }
}
```

## 5. Capability Requirements

| Method | Required Capability | Default Grant |
| --- | --- | --- |
| `WifiStation::scan` | `WifiScan` | Granted to all agents |
| `WifiStation::connect` | `WifiConnect` | Prompt user |
| `WifiStation::disconnect` | `WifiConnect` | Prompt user |
| `WifiStation::status` | None | Always available |
| `BluetoothAdapter::start_discovery` | `BluetoothAccess` | Prompt user |
| `BluetoothAdapter::pair` | `BluetoothAccess` | Prompt user |
| `BluetoothAdapter::paired_devices` | `BluetoothAccess` | Prompt user |
| `BleGatt::connect` | `BluetoothAccess` | Prompt user |
| `BleGatt::read_characteristic` | `BluetoothAccess` | Prompt user |
| `BleGatt::write_characteristic` | `BluetoothAccess` | Prompt user |

**Agent manifest example:**

```toml
[capabilities.required]
WifiScan = "Scan for available WiFi networks"

[capabilities.optional]
WifiConnect = "Connect to WiFi networks"
BluetoothAccess = "Discover and communicate with Bluetooth devices"
```

## 6. Error Handling

```rust
/// Errors returned by Wireless Kit operations.
#[derive(Debug)]
pub enum WirelessError {
    /// The wireless radio is disabled (airplane mode or hardware kill switch).
    RadioDisabled,

    /// WiFi authentication failed (wrong password, rejected by AP).
    AuthenticationFailed { ssid: Ssid, reason: AuthFailReason },

    /// WiFi association failed (AP refused the connection).
    AssociationFailed { ssid: Ssid, reason: String },

    /// The requested network was not found in the last scan results.
    NetworkNotFound(Ssid),

    /// The BLE peripheral disconnected unexpectedly.
    BleDisconnected(BtAddress),

    /// A BLE GATT operation failed.
    GattError { uuid: Uuid, reason: GattErrorCode },

    /// The requested BLE service was not found on the peripheral.
    ServiceNotFound,

    /// The required capability was not granted.
    CapabilityDenied(String),

    /// Firmware for the wireless chipset could not be loaded.
    FirmwareLoadFailed { chipset: String, reason: String },

    /// The regulatory domain prohibits the requested operation.
    RegulatoryViolation { domain: RegulatoryDomain, reason: String },

    /// The Bluetooth adapter is not present on this device.
    AdapterNotPresent,

    /// Pairing was rejected by the remote device or cancelled by the user.
    PairingFailed(BtAddress),

    /// The operation timed out.
    Timeout { operation: String, timeout_ms: u32 },
}
```

**Recovery guidance:**

| Error | Recovery |
| --- | --- |
| `RadioDisabled` | Prompt user to disable airplane mode; listen for radio state changes |
| `AuthenticationFailed` | Prompt for new credentials; check credential vault |
| `BleDisconnected` | Attempt reconnection with exponential backoff |
| `ServiceNotFound` | Verify peripheral supports the expected service; check firmware version |
| `FirmwareLoadFailed` | Check for firmware updates; fall back to limited functionality |
| `PairingFailed` | Retry with user confirmation; ensure device is in pairing mode |

## 7. Platform & AI Availability

**Platform support:**

| Platform | WiFi | Bluetooth Classic | BLE | Notes |
| --- | --- | --- | --- | --- |
| QEMU virt | No | No | No | No wireless hardware emulation |
| Raspberry Pi 4 | BCM43455 | Yes | Yes (4.2) | Broadcom firmware required |
| Raspberry Pi 5 | BCM43455 | Yes | Yes (5.0) | Improved BLE range |
| Apple Silicon | BCM4387 | Yes | Yes (5.3) | Full feature set |

**AIRS-enhanced features:**

| Feature | What AIRS provides | Without AIRS |
| --- | --- | --- |
| Roaming decisions | Predicts optimal AP for handoff based on movement patterns | Signal-threshold roaming only |
| Rogue AP detection | ML-based behavioral anomaly detection for evil-twin attacks | Static BSSID allowlist |
| BLE reconnection | Learns peripheral availability patterns for optimal retry timing | Fixed exponential backoff |
| Power optimization | Adapts scan intervals and radio duty cycle to usage patterns | Fixed intervals |
| Coexistence tuning | Optimizes WiFi/BT radio sharing for concurrent workloads | Static time-division |

**Feature detection:**

```rust
use aios_wireless::WirelessKit;

if WirelessKit::wifi_available() {
    println!("WiFi radio present");
} else {
    println!("No WiFi hardware (QEMU or unsupported platform)");
}

if WirelessKit::bluetooth_available() {
    let bt = aios_wireless::bluetooth_adapter();
    println!("Bluetooth: {}", bt.info().version);
}
```

**Implementation phase:** Phase 8+. Wireless Kit depends on [Memory Kit](../kernel/memory.md),
[Capability Kit](../kernel/capability.md), and [Network Kit](./network.md) (for WiFi-backed
network interfaces).

---

*See also: [Network Kit](./network.md) | [Audio Kit](./audio.md) | [Input Kit](./input.md) | [Power Kit](./power.md) | [USB Kit](./usb.md)*
