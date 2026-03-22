# Media Kit

**Layer:** Platform | **Architecture:** `docs/platform/media-pipeline.md` + 6 sub-docs

## Purpose

Full media processing stack: codec framework, container demux/mux, A/V synchronized playback, adaptive streaming, WebRTC real-time communication, and DRM content protection. Hardware codec selection and software fallback are transparent to consumers.

## Key APIs

| Trait / API | Description |
|---|---|
| `MediaCodec` | Codec framework trait with hardware/software selection and capability registry |
| `ContainerEngine` | Demuxer/muxer for MP4, WebM, MKV, MPEG-TS |
| `PlaybackPipeline` | Graph-model pipeline with A/V sync, clock recovery, buffering, and subtitle rendering |
| `MediaSession` | Session lifecycle, transport controls, and Now Playing metadata |
| `StreamingEngine` | HLS/DASH/MoQ adaptive bitrate with jitter buffer and network resilience |
| `RtcSession` | WebRTC session: ICE, DTLS, RTP, SDP negotiation, simulcast/SVC |

## Dependencies

Memory Kit, Capability Kit, Audio Kit, Compute Kit, Network Kit

## Consumers

Browser Kit, applications, Conversation Kit

## Implementation Phase

Phase 9+
