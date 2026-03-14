# AIOS Audio Subsystem — RT Scheduling & A/V Sync

Part of: [audio.md](../audio.md) — Audio Subsystem
**Related:** [subsystem.md](./subsystem.md) — Architecture and sessions, [mixing.md](./mixing.md) — PCM mixer and capture pipeline, [drivers.md](./drivers.md) — Hardware drivers, [integration.md](./integration.md) — Power management and HDMI routing

-----

## 6. RT Scheduling Integration

Audio is the most timing-sensitive subsystem. A missed deadline causes an audible glitch — an underrun (silence) or overrun (repeated samples). The audio subsystem depends heavily on the kernel scheduler's Real-Time class. See [scheduler.md](../../kernel/scheduler.md) §3.1 and §5.2.

### 6.1 RT Task Registration

The audio subsystem registers its mixing and capture callbacks as RT tasks during initialization.

```rust
impl AudioSubsystem {
    fn register_rt_tasks(&self) -> Result<()> {
        // Playback mixer: 200 Hz (5ms period), 0.5ms WCET
        let mixer_rt = RtTask {
            period: Duration::from_micros(5000),
            wcet: Duration::from_micros(500),
            relative_deadline: Duration::from_micros(5000),
            affinity: CpuSet::single(CpuId(0)), // pinned to core 0
            overrun: RtOverrunState::default(),
            deferred: false,
        };

        // Capture callback: 200 Hz (5ms period), 0.3ms WCET
        let capture_rt = RtTask {
            period: Duration::from_micros(5000),
            wcet: Duration::from_micros(300),
            relative_deadline: Duration::from_micros(5000),
            affinity: CpuSet::single(CpuId(0)),
            overrun: RtOverrunState::default(),
            deferred: false,
        };

        // Admission control: scheduler verifies total RT utilization < 70%
        // Mixer:   0.5ms / 5ms   = 10% utilization
        // Capture: 0.3ms / 5ms   =  6% utilization
        // Total audio RT load:     16% utilization
        // Compositor: 4ms / 16.6ms = 24% utilization
        // Grand total:              40% — well under 70% ceiling
        scheduler::admit_rt(mixer_rt)?;
        scheduler::admit_rt(capture_rt)?;

        Ok(())
    }
}
```

### 6.2 Latency Budget

The total round-trip audio latency budget is the sum of all stages in the audio path.

```text
Latency budget breakdown (target: < 10ms round-trip):

Input (capture) path:
  Hardware → DMA buffer fill         1.0 ms  (half of 2ms DMA period)
  Capture callback processing        0.3 ms  (SRC + AEC + AGC)
  Agent ring buffer read latency     0.5 ms  (IPC notification delay)
  ─────────────────────────────────────────
  Total input latency:               1.8 ms

Output (playback) path:
  Agent ring buffer write latency    0.5 ms  (IPC notification delay)
  Mix callback processing            0.5 ms  (SRC + mix + clip)
  Hardware ring buffer → DMA         2.5 ms  (one period of the DMA buffer)
  DAC/codec output delay             0.2 ms  (hardware fixed delay)
  ─────────────────────────────────────────
  Total output latency:              3.7 ms

Round-trip (mic → processing → speakers):
  Input + processing + output        5.5 ms (nominal)
                                     9.0 ms (worst case with scheduling jitter)
```

### Predictive Buffer Management (AIRS Integration)

AIRS observes system load patterns from the scheduler and observability subsystems. When it predicts a load spike (compilation starting, large model inference beginning, many agents launching simultaneously), it sets a `BufferHint` on the audio subsystem to preemptively adjust buffer sizes.

```rust
/// Buffer hint from AIRS predictive load management.
/// Checked by the mixer each period. Applied smoothly over
/// multiple periods to avoid audible artifacts.
pub struct BufferHint {
    /// Suggested buffer size adjustment in frames
    /// Positive = increase buffer (trade latency for stability)
    /// Negative = decrease buffer (trade stability for latency)
    pub adjustment_frames: i32,
    /// Predicted event causing the load change
    pub predicted_event: String,
    /// Time until the predicted load change
    pub eta: Duration,
    /// Confidence in the prediction (0.0 - 1.0)
    pub confidence: f32,
}

impl PcmMixer {
    /// Check AIRS buffer hint and adjust if appropriate.
    /// Called at the start of each mix callback (before mixing).
    /// Requires `buffer_hint`, `min_buffer_frames`, and `max_buffer_frames`
    /// fields on PcmMixer (see mixing.md §4.2 for the full struct).
    fn check_buffer_hint(&mut self) {
        if let Some(hint) = self.pending_hint.take() {
            // Only act on high-confidence predictions
            if hint.confidence < 0.6 {
                return;
            }

            // Calculate target buffer size
            let current = self.buffer_frames as i32;
            let target = (current + hint.adjustment_frames)
                .clamp(self.min_buffer_frames as i32, self.max_buffer_frames as i32);

            // Smooth transition: adjust by at most 25% per period
            // to avoid audible artifacts from sudden buffer size changes
            let max_step = current / 4;
            let step = (target - current).clamp(-max_step, max_step);

            self.buffer_frames = (current + step) as u32;

            // Resize the mix buffer if needed
            if self.mix_buffer.len() < self.buffer_frames as usize * self.output_format.channels as usize {
                // Buffer was pre-allocated to max size — just update the slice bounds
            }
        }
    }
}
```

The mixer treats buffer hints as advisory. It can ignore hints when:

- The current underrun rate is zero (no need to increase buffers)
- The session is in RealTime latency mode (latency cannot be traded)
- The hint confidence is below 0.6

Buffer adjustments are logged to the audit space for AIRS to learn which predictions were accurate and improve future predictions.

### 6.3 Buffer Sizing

Buffer size directly trades latency for robustness. Smaller buffers reduce latency but increase the probability of underruns.

```rust
pub enum AudioLatencyMode {
    /// Lowest latency: 2.5ms buffer (120 frames @ 48kHz)
    /// For: voice calls, real-time monitoring, musical instruments
    /// Risk: underruns on loaded systems
    RealTime,

    /// Balanced: 5ms buffer (240 frames @ 48kHz)
    /// For: games, interactive audio, TTS playback
    /// Default mode for most sessions
    Interactive,

    /// Robust: 20ms buffer (960 frames @ 48kHz)
    /// For: music playback, background audio, notifications
    /// Virtually eliminates underruns even under heavy load
    Relaxed,
}

impl AudioLatencyMode {
    pub fn buffer_frames(&self, sample_rate: u32) -> u32 {
        let ms = match self {
            Self::RealTime => 2.5,
            Self::Interactive => 5.0,
            Self::Relaxed => 20.0,
        };
        (sample_rate as f64 * ms / 1000.0) as u32
    }

    /// Scheduler period for this latency mode
    pub fn rt_period(&self) -> Duration {
        match self {
            Self::RealTime => Duration::from_micros(2500),
            Self::Interactive => Duration::from_micros(5000),
            Self::Relaxed => Duration::from_micros(10000),
        }
    }
}
```

### 6.4 Underrun Handling

When the mixer reads from an agent's ring buffer and finds it empty, an underrun has occurred. The response depends on severity.

```rust
pub struct UnderrunPolicy {
    /// Number of consecutive underruns before taking action
    tolerance: u32,
    /// Action on repeated underruns
    action: UnderrunAction,
}

pub enum UnderrunAction {
    /// Insert silence and continue (default — minimizes audible impact)
    Silence,
    /// Repeat the last buffer (less noticeable for music)
    RepeatLast,
    /// Increase buffer size automatically (trades latency for stability)
    AutoResize { max_buffer_ms: f32 },
    /// Close the session (agent is not supplying audio in time)
    CloseSession,
}
```

-----

## 7. A/V Sync with Compositor

When video and audio play simultaneously (media player, video call, game), the audio and video streams must be synchronized. A lip-sync error greater than ±40ms is perceptible; greater than ±80ms is distracting. The audio subsystem and compositor share a timeline to maintain synchronization. See [compositor.md](../compositor.md) §6 for the render pipeline.

### 7.1 Shared Timeline

```rust
/// System-wide media clock.
/// The audio subsystem is the clock master — video follows audio.
/// Rationale: audio glitches are more noticeable than dropped video frames.
pub struct MediaTimeline {
    /// Monotonic reference clock (ARM Generic Timer, 62.5 MHz on QEMU)
    base_clock: ArmGenericTimer,

    /// Current playback position in media time (microseconds)
    media_position: AtomicU64,

    /// Playback rate (1.0 = normal, 0.5 = half speed, 2.0 = double)
    playback_rate: AtomicF32,

    /// Audio samples played since timeline start
    /// (ground truth — audio hardware is the clock master)
    samples_played: AtomicU64,

    /// Sample rate of the output device
    sample_rate: u32,
}

impl MediaTimeline {
    /// Get the current media time, derived from audio samples played.
    /// This is the authoritative time source — video adjusts to match.
    pub fn current_media_time(&self) -> Duration {
        let samples = self.samples_played.load(Relaxed);
        let rate = self.playback_rate.load(Relaxed);
        Duration::from_micros(
            (samples as f64 / self.sample_rate as f64 * 1_000_000.0 / rate as f64) as u64
        )
    }
}
```

### 7.2 Presentation Timestamps

Agents that produce synchronized audio and video attach presentation timestamps (PTS) to both streams. The audio subsystem and compositor use these to schedule delivery.

```rust
/// A timestamped audio buffer from an agent
pub struct TimestampedAudioBuffer {
    /// PCM sample data
    samples: Vec<f32>,
    /// Presentation timestamp: when these samples should be heard
    pts: Duration,
    /// Duration of audio in this buffer
    duration: Duration,
}

/// A timestamped video frame from an agent
pub struct TimestampedVideoFrame {
    /// Surface buffer ID
    buffer: SharedBufferId,
    /// Presentation timestamp: when this frame should be displayed
    pts: Duration,
}
```

### 7.3 Synchronization Protocol

```text
Audio-Video Synchronization Flow:

1. Agent decodes media, producing audio + video with PTS values
     Audio PTS: 1000ms, 1020ms, 1040ms, ...
     Video PTS: 1000ms, 1033ms, 1067ms, ... (30fps)

2. Audio subsystem plays samples at their PTS
   (audio is the master clock — never adjusted)

3. Compositor receives video frames with PTS
   Before presenting each frame, it queries the media timeline:
     audio_time = timeline.current_media_time()
     drift = video_pts - audio_time

   Three cases:
     drift < -40ms: Video is late.
                    Drop frame, present next immediately.
     drift > +40ms: Video is early.
                    Hold current frame, present new one at correct time.
     |drift| < 40ms: Acceptable sync.
                     Present frame on next VSync.

4. If drift accumulates beyond ±80ms for 5+ seconds:
   The agent is notified to resync (seek audio to video position
   or vice versa).
```

### 7.4 Communication Between Audio and Compositor

```rust
/// IPC message from audio subsystem to compositor
pub enum AudioToCompositor {
    /// Media timeline update (sent every mix period)
    TimelineUpdate {
        timeline_id: TimelineId,
        media_time: Duration,
        wall_clock: Timestamp,
    },

    /// Audio stream started/stopped (compositor may adjust VSync timing)
    StreamStateChanged {
        timeline_id: TimelineId,
        state: StreamState,
    },
}

/// IPC message from compositor to audio subsystem
pub enum CompositorToAudio {
    /// Video frame presented — allows audio to track actual display timing
    FramePresented {
        timeline_id: TimelineId,
        frame_pts: Duration,
        actual_present_time: Timestamp,
    },

    /// Compositor requesting audio to pause/resume (e.g., window minimized)
    PlaybackControl {
        timeline_id: TimelineId,
        action: PlaybackAction,
    },
}
```
