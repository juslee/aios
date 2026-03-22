# Wireless Kit

**Layer:** Platform | **Architecture:** `docs/platform/wireless.md` + 6 sub-docs

## Purpose

WiFi and Bluetooth stack abstraction with firmware management, WPA3 security, and radio coexistence. Exposes station management and BLE GATT interfaces to upper layers while hiding hardware and firmware differences behind unified traits.

## Key APIs

| Trait / API | Description |
|---|---|
| `WifiStation` | Station management: scan, connect, roam, WPA2/WPA3-SAE authentication |
| `BluetoothAdapter` | Classic and BLE adapter; HCI transport over USB or SDIO |
| `BleGatt` | BLE GATT client/server for peripheral integration (HID, audio, sensors) |
| `WpaSupplicant` | WPA2/WPA3 supplicant with rogue AP detection and credential vault integration |
| `FirmwareLoader` | Versioned firmware blob loading with regulatory domain enforcement |

## Dependencies

Memory Kit, Capability Kit, Network Kit

## Consumers

Network Kit, Audio Kit (Bluetooth audio), Input Kit (Bluetooth HID)

## Implementation Phase

Phase 8+
