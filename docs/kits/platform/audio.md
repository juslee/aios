# Audio Kit

**Layer:** Platform | **Architecture:** `docs/platform/audio.md` + 5 sub-docs

## Purpose

Audio session management, mixing, capture, and DSP pipeline with hardware-agnostic driver abstraction. Session-based routing arbitrates between competing consumers and integrates AIRS hints for adaptive latency and power management.

## Key APIs

| Trait / API | Description |
|---|---|
| `AudioSession` | Capability-gated session with role (playback, capture, communication) and routing policy |
| `AudioMixer` | Multi-source mixing with sample-rate conversion and per-stream volume control |
| `CaptureStream` | Microphone capture pipeline with privacy indicator enforcement |
| `DspFilterGraph` | Composable DSP filter chain: EQ, AEC, noise suppression, spatialization |
| `AudioDevice` | Hardware driver trait for VirtIO-Sound, I2S, HDMI, USB Audio |

## Dependencies

Memory Kit, Capability Kit, IPC Kit

## Consumers

Media Kit, Conversation Kit, Browser Kit, applications

## Implementation Phase

Phase 8+
