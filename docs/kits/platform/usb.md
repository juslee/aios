# USB Kit

**Layer:** Platform | **Architecture:** `docs/platform/usb.md` + 4 sub-docs

## Purpose

USB host controller abstraction, device class drivers, capability-gated hotplug, and power management. Provides a unified device model for all USB-attached peripherals; upper-layer Kits (Input, Audio, Storage, Camera) consume device class interfaces without knowing the underlying USB transport.

## Key APIs

| Trait / API | Description |
|---|---|
| `UsbHostController` | Host controller trait covering xHCI and DWC2; DMA, interrupt, and transfer management |
| `UsbDevice` | Enumerated device handle with descriptor cache and configuration management |
| `HidDriver` | USB HID class driver; feeds normalized events into Input Kit |
| `StorageDriver` | USB Mass Storage / UAS class driver; exposes block device to Storage Kit |
| `HotplugManager` | State machine for attach/detach events; capability-gated device access on arrival |

## Dependencies

Memory Kit, Capability Kit

## Consumers

Input Kit, Audio Kit, Storage Kit, Camera Kit

## Implementation Phase

Phase 8+
