# AIOS Accessibility AI Enhancement

Part of: [accessibility.md](../accessibility.md) — Accessibility Engine
**Related:** [assistive-technology.md](./assistive-technology.md) — Core assistive technology (no-AIRS baseline), [intelligence.md](./intelligence.md) — AI-native intelligence, [intelligence-services.md](../../intelligence/airs/intelligence-services.md) — AIRS Intelligence Services

-----

## 10. AIRS Enhancement

When AIRS is available, every accessibility feature improves. But every feature works without AIRS first — AIRS makes it better, not possible.

### 10.1 Enhancement Matrix

| Feature | Without AIRS | With AIRS |
|---|---|---|
| Text-to-speech | eSpeak-NG (robotic, fast) | Neural TTS (natural voice, configurable) |
| Image descriptions | Developer-provided alt text only | Auto-generated descriptions for unlabeled images |
| Voice control | ~50 fixed commands, keyword spotting | Full natural language, context-aware |
| UI navigation | Standard focus order | Smart navigation: skip repetitive elements, predict next action |
| Content summaries | None (full content read) | "This page has 42 messages. 3 are unread. The most recent is from Alice about the project deadline." |
| Error messages | Literal error text | "The file couldn't be saved because the disk is full. You have 3 large files in Downloads that could be deleted." |
| Form assistance | Field labels and types | "This form asks for your shipping address. There are 6 fields. You've filled in 2." |
| Context adaptation | Static accessibility settings | Simplify UI when user is struggling (detected from interaction patterns) |

### 10.2 Neural TTS

When AIRS loads, the screen reader can upgrade from eSpeak-NG to a neural TTS model. The upgrade is seamless — the screen reader engine swaps its output backend without interrupting the speech queue.

```rust
pub struct NeuralTtsEngine {
    /// AIRS inference session for TTS
    session: AirsInferenceSession,

    /// TTS model (loaded as part of AIRS model registry)
    model: ModelRef,

    /// Voice configuration
    voice: NeuralVoice,

    /// Streaming output (starts speaking before full synthesis completes)
    streaming: bool,
}

impl NeuralTtsEngine {
    /// Synthesize speech from text.
    /// Returns a stream of PCM audio chunks for immediate playback.
    pub async fn synthesize(&self, text: &str) -> AudioStream {
        let tokens = self.session.tokenize(text).await;
        let audio = self.session.infer_streaming(
            &self.model,
            &tokens,
            InferenceHints {
                voice: self.voice.clone(),
                streaming: self.streaming,
            },
        ).await;
        audio
    }
}

impl ScreenReaderEngine {
    /// Upgrade from eSpeak-NG to neural TTS.
    /// Fallback remains available if neural TTS fails.
    pub fn upgrade_to_neural_tts(&mut self) {
        if let Some(ref airs_layer) = self.neural_engine {
            // Neural TTS becomes primary, eSpeak-NG becomes fallback.
            // If neural TTS latency exceeds 200ms, automatically
            // fall back to eSpeak-NG without interrupting the
            // current speech queue.
            self.tts_backend = TtsBackend::Neural { fallback: TtsBackend::EspeakNg };
        }
    }
}
```

The user is never forced to use neural TTS. Some users prefer eSpeak-NG's robotic voice because it's faster, more predictable, and they've developed muscle memory for its cadence. The preference is respected.

### 10.3 AI Image Description

When an agent's accessibility tree contains an image without alt text, and AIRS is available, the Accessibility Manager can request an AI-generated description:

```rust
impl AirsAccessibilityLayer {
    /// Generate a description for an image that lacks alt text.
    pub async fn describe_image(
        &self,
        image_data: &[u8],
        context: &ImageContext,
    ) -> Result<String> {
        let prompt = format!(
            "Describe this image concisely for a screen reader user. \
             Context: this image appears in {} with the heading '{}'. \
             Focus on information content, not visual aesthetics.",
            context.surface_title,
            context.nearest_heading.as_deref().unwrap_or("unknown"),
        );

        let description = self.airs.infer(
            ModelTask::ImageDescription,
            &prompt,
            Some(image_data),
        ).await?;

        Ok(description)
    }
}
```

AI-generated descriptions are cached in the accessibility tree so they're not regenerated on every focus change. They're also marked as AI-generated so the user can request re-description or dismiss them.

### 10.4 Context-Aware UI Adaptation

AIRS can observe interaction patterns and adapt the UI for accessibility users:

- **Repeated failed interactions** (e.g., clicking the wrong button multiple times) trigger increased target sizes and spacing
- **Slow navigation** through complex menus triggers simplified menu presentation
- **Frequent use of "repeat"** suggests speech is too fast or content is too complex — adjusts automatically
- **Time-of-day patterns** adjust contrast and brightness automatically (brighter during day, dimmer at night)

All adaptations are transparent to the user and can be disabled. The system tells the user what it changed and why: "I noticed you're having trouble with the small buttons. I've made them larger. You can undo this by saying 'reset button size.'"

-----

## 11. No-AIRS Fallback

Every accessibility feature works without AIRS. This section documents the specific fallback behavior for each AIRS-enhanced feature.

### 11.1 Fallback Specifications

```text
Fallback behavior when AIRS is unavailable.
These are not degraded-mode afterthoughts — they are complete,
usable implementations that happen to be less sophisticated.

AirsFallback::TextToSpeech
    engine:     EspeakNg
    quality:    Robotic but clear, <10ms latency
    coverage:   100+ languages

AirsFallback::ImageDescription
    behavior:   Only developer-provided alt text shown
    missing:    Screen reader says 'image' with no description

AirsFallback::VoiceControl
    vocabulary:  ~50 fixed commands
    recognition: Phoneme pattern matching, no language model
    wake_word:   Required — 'Computer' by default

AirsFallback::ContentSummary
    behavior:    Screen reader reads all content linearly
    workaround:  User can use heading navigation to skip sections

AirsFallback::SmartNavigation
    behavior:    Tab order follows widget hierarchy
    workaround:  User uses landmark navigation (jump to heading, list, form)

AirsFallback::ContextAdaptation
    behavior:    Accessibility settings remain fixed
    workaround:  User manually adjusts via Conversation Bar or preferences
```

### 11.2 Graceful Degradation at Runtime

If AIRS goes down while the user is actively using AI-enhanced accessibility features, the Accessibility Manager falls back without interruption:

```rust
impl AccessibilityManager {
    /// Handle AIRS disconnection at runtime.
    /// Switch all features to non-AIRS mode immediately.
    pub fn airs_disconnected(&mut self) {
        // Switch TTS back to eSpeak-NG mid-sentence if needed
        if self.screen_reader.primary_engine == TtsEngine::Neural {
            self.screen_reader.primary_engine = TtsEngine::EspeakNg;
            self.screen_reader.speak(
                "AI voice temporarily unavailable. Using standard voice.",
                SpeechPriority::Next,
            );
        }

        // Voice control reverts to keyword spotter
        if let Some(ref mut voice) = self.voice_control {
            voice.mode = VoiceControlMode::KeywordOnly;
            self.screen_reader.speak(
                "Voice control: basic commands only.",
                SpeechPriority::Queued,
            );
        }

        // Clear cached AI descriptions
        self.airs_layer = None;

        // No disruption to: screen reader, Braille, switch scanning,
        // high contrast, magnification, keyboard navigation, reduced motion
    }
}
```

The key invariant: **no accessibility feature becomes unavailable when AIRS disconnects.** Quality may decrease (robotic voice instead of natural, no image descriptions), but functionality remains complete.
