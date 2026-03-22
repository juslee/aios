# Camera Kit

**Layer:** Platform | **Architecture:** `docs/platform/camera.md` + 7 sub-docs

## Purpose

Camera device abstraction, capture pipeline, ISP processing, and session-based access with hardware-enforced privacy. Supports USB UVC, CSI/MIPI, and VirtIO-Camera sources. A hardware LED indicator and consent model ensure no silent capture is possible.

## Key APIs

| Trait / API | Description |
|---|---|
| `CameraDevice` | Driver trait covering UVC, CSI/MIPI, VirtIO-Camera, and platform cameras |
| `CaptureSession` | Session lifecycle with `SessionIntent` (viewfinder, photo, video, scan) and conflict resolution |
| `IspPipeline` | Image signal processor stages: demosaic, 3A algorithms, noise reduction, HDR |
| `SessionIntent` | Declared capture intent used by conflict resolution and the Prompt policy |
| `PrivacyIndicator` | Hardware LED enforcement and on-screen recording consent overlay |

## Dependencies

Memory Kit, Capability Kit, Compute Kit

## Consumers

Conversation Kit, Browser Kit, applications

## Implementation Phase

Phase 9+
