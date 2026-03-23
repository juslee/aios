# Translation Kit

**Layer:** Platform | **Crate:** `aios_translation` | **Architecture:** [`docs/storage/flow/transforms.md`](../../storage/flow/transforms.md)

## 1. Overview

Translation Kit manages format conversion between content types for clipboard exchange,
drag-and-drop, file import/export, and inter-agent data sharing. It maintains a roster of
registered `Translator` implementations that form a directed conversion graph. When a
consumer needs data in a format different from what the producer offers, Translation Kit
finds the shortest conversion path through the graph and executes it transparently. This is
the content type negotiation system inspired by BeOS's translation architecture -- producers
declare what they output, consumers declare what they accept, and the Translation Kit
bridges the gap.

Application developers use Translation Kit when they need to export or import data in
formats their agent does not natively support. A drawing app that works with its own
vector format can register a translator to PNG, and every other agent in the system
immediately gains the ability to paste that drawing as an image. A text editor that
accepts Markdown can receive rich text from a browser through a chain of translators
(HTML to Markdown) without either agent knowing about the other's format. The key insight
is that translators compose: adding a single new translator to the roster can unlock
conversion paths between formats that previously had none.

Translation Kit is not the right tool for streaming media transcoding (use
[Media Kit](./media.md)) or for persistent file format conversion (use the
agent's own import/export logic). Translation Kit is designed for clipboard-sized data
transfers where low latency and format fidelity matter more than throughput.

## 2. Core Traits

```rust
use aios_memory::SharedBuffer;

/// Registry of available translators forming a directed conversion graph.
///
/// The roster resolves the shortest conversion path between any two formats
/// and chains translators together when no direct translation exists.
pub trait TranslationRoster {
    /// Register a new translator with the roster.
    fn register(&mut self, translator: Box<dyn Translator>) -> Result<TranslatorId, TranslationError>;

    /// Unregister a translator (e.g., when its agent is uninstalled).
    fn unregister(&mut self, id: TranslatorId) -> Result<(), TranslationError>;

    /// Check whether a conversion path exists between two formats.
    fn can_translate(&self, from: &FormatDescriptor, to: &FormatDescriptor) -> bool;

    /// Return all formats reachable from a given source format.
    fn reachable_formats(&self, from: &FormatDescriptor) -> Vec<FormatDescriptor>;

    /// Return the shortest conversion path between two formats.
    /// Each step in the path is a translator that handles one hop.
    fn find_path(
        &self,
        from: &FormatDescriptor,
        to: &FormatDescriptor,
    ) -> Result<TranslationPath, TranslationError>;

    /// Execute a full translation from source to target format.
    /// Chains translators if no single-hop translation exists.
    fn translate(
        &self,
        data: &[u8],
        from: &FormatDescriptor,
        to: &FormatDescriptor,
    ) -> Result<TranslatedData, TranslationError>;

    /// List all registered translators.
    fn translators(&self) -> Vec<TranslatorInfo>;

    /// List all known content formats (union of all translator inputs/outputs).
    fn known_formats(&self) -> Vec<FormatDescriptor>;
}

/// A single-hop content translator that converts between two specific formats.
///
/// Translators are the building blocks of the conversion graph. Each translator
/// declares which format(s) it can read and which format(s) it can produce.
/// The Translation Kit chains multiple translators to cover multi-hop conversions.
pub trait Translator: Send + Sync {
    /// Human-readable name of this translator.
    fn name(&self) -> &str;

    /// The source formats this translator can read.
    fn input_formats(&self) -> &[FormatDescriptor];

    /// The target formats this translator can produce.
    fn output_formats(&self) -> &[FormatDescriptor];

    /// The quality rating for a specific input-output pair (0.0 to 1.0).
    /// Used by the roster to prefer higher-quality paths when multiple exist.
    fn quality(&self, from: &FormatDescriptor, to: &FormatDescriptor) -> f32;

    /// Perform the translation.
    fn translate(
        &self,
        data: &[u8],
        from: &FormatDescriptor,
        to: &FormatDescriptor,
    ) -> Result<TranslatedData, TranslationError>;

    /// Estimate the output size for a given input size (for buffer pre-allocation).
    fn estimate_output_size(&self, input_size: usize, from: &FormatDescriptor, to: &FormatDescriptor) -> usize {
        input_size * 2 // Conservative default
    }
}

/// A typed format identity with capability metadata.
///
/// Formats are identified by MIME type, with optional AIOS-native type
/// identifiers for formats that have no standard MIME representation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FormatDescriptor {
    /// The MIME type (e.g., "image/png", "text/html", "application/pdf").
    pub mime_type: String,

    /// Optional AIOS-native content type for system-specific formats.
    pub aios_type: Option<AiosContentType>,

    /// Whether this format preserves the full fidelity of the source.
    pub lossless: bool,

    /// Optional metadata about the format (color space, encoding, etc.).
    pub metadata: FormatMetadata,
}

/// The result of a translation operation.
pub struct TranslatedData {
    /// The converted content bytes.
    pub data: Vec<u8>,

    /// The format of the output data.
    pub format: FormatDescriptor,

    /// The number of translator hops in the conversion chain.
    pub hop_count: u32,

    /// The cumulative quality score across all hops (product of per-hop scores).
    pub quality: f32,
}

/// A resolved conversion path through the translator graph.
pub struct TranslationPath {
    /// The ordered sequence of translators to execute.
    pub steps: Vec<TranslatorInfo>,

    /// The cumulative quality score for this path.
    pub quality: f32,

    /// The number of hops (translator invocations).
    pub hop_count: u32,
}
```

**Key types:**

```rust
/// AIOS-native content types for system-specific formats.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AiosContentType {
    /// A Space Object reference.
    SpaceObject,
    /// A Flow entry reference.
    FlowEntry,
    /// Structured agent message (Scriptable Protocol).
    AgentMessage,
    /// Rich text with semantic annotations.
    AnnotatedText,
    /// Custom content type registered by an agent.
    Custom(String),
}

/// Metadata about a content format.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct FormatMetadata {
    /// Character encoding for text formats.
    pub encoding: Option<String>,
    /// Color space for image formats.
    pub color_space: Option<String>,
    /// Sample rate for audio formats.
    pub sample_rate: Option<u32>,
}
```

## 3. Usage Patterns

**Register a custom translator for your agent's format:**

```rust
use aios_translation::{TranslationKit, Translator, FormatDescriptor, TranslatedData, TranslationError};

/// Translates our custom vector format to SVG.
struct VectorToSvgTranslator;

impl Translator for VectorToSvgTranslator {
    fn name(&self) -> &str { "MyApp Vector to SVG" }

    fn input_formats(&self) -> &[FormatDescriptor] {
        &[FormatDescriptor::mime("application/x-myapp-vector")]
    }

    fn output_formats(&self) -> &[FormatDescriptor] {
        &[FormatDescriptor::mime("image/svg+xml")]
    }

    fn quality(&self, _from: &FormatDescriptor, _to: &FormatDescriptor) -> f32 {
        1.0 // Lossless vector-to-vector conversion
    }

    fn translate(
        &self,
        data: &[u8],
        _from: &FormatDescriptor,
        _to: &FormatDescriptor,
    ) -> Result<TranslatedData, TranslationError> {
        let svg_bytes = convert_to_svg(data)?;
        Ok(TranslatedData {
            data: svg_bytes,
            format: FormatDescriptor::mime("image/svg+xml"),
            hop_count: 1,
            quality: 1.0,
        })
    }
}

// Register at agent startup -- now every agent can paste our vectors as SVG,
// and the SVG-to-PNG translator (built-in) means they also get PNG for free.
let roster = TranslationKit::roster();
roster.register(Box::new(VectorToSvgTranslator))?;
```

**Translate clipboard data to a format you can use:**

```rust
use aios_translation::TranslationKit;

let roster = TranslationKit::roster();

// Clipboard contains HTML; we want Markdown
let clipboard_data: &[u8] = get_clipboard_bytes();
let from = FormatDescriptor::mime("text/html");
let to = FormatDescriptor::mime("text/markdown");

if roster.can_translate(&from, &to) {
    let result = roster.translate(clipboard_data, &from, &to)?;
    println!(
        "Converted HTML to Markdown ({} hops, quality {:.0}%)",
        result.hop_count,
        result.quality * 100.0,
    );
    insert_text(&result.data);
} else {
    // Fall back to plain text
    let plain = FormatDescriptor::mime("text/plain");
    let result = roster.translate(clipboard_data, &from, &plain)?;
    insert_text(&result.data);
}
```

**Discover available conversions for drag-and-drop acceptance:**

```rust
use aios_translation::TranslationKit;

let roster = TranslationKit::roster();

// Source is dragging an image; what can we accept?
let source_format = FormatDescriptor::mime("image/webp");
let reachable = roster.reachable_formats(&source_format);

let we_accept = [
    FormatDescriptor::mime("image/png"),
    FormatDescriptor::mime("image/jpeg"),
];

let best_target = we_accept.iter()
    .filter(|f| reachable.contains(f))
    .max_by(|a, b| {
        let path_a = roster.find_path(&source_format, a).unwrap();
        let path_b = roster.find_path(&source_format, b).unwrap();
        path_a.quality.partial_cmp(&path_b.quality).unwrap()
    });

if let Some(target) = best_target {
    println!("Will accept drop as {}", target.mime_type);
    accept_drop(target);
} else {
    reject_drop();
}
```

> **Common Mistakes**
>
> - **Registering translators with quality 0.0.** The roster uses quality scores to rank
>   paths. A quality of 0.0 means the path will never be preferred over alternatives.
>   Use realistic scores (0.5 for lossy, 0.8 for good-quality lossy, 1.0 for lossless).
> - **Multi-hop chains that lose too much quality.** Quality scores multiply across hops.
>   A 3-hop chain at 0.8 quality per hop yields 0.51 overall. If quality drops below 0.3,
>   the roster will warn the consumer.
> - **Translating large files through the roster.** Translation Kit is optimized for
>   clipboard-sized data (up to a few megabytes). For large file conversions, use the
>   media pipeline or agent-specific import/export.
> - **Forgetting to unregister translators on agent shutdown.** Orphaned translators
>   that reference unloaded agent code will crash when invoked. Always unregister in
>   your agent's cleanup path.

## 4. Integration Examples

**Translation Kit + Flow Kit -- clipboard and drag-and-drop:**

```rust
use aios_translation::TranslationKit;
use aios_flow::{FlowKit, FlowEntry, TypedContent};

// Flow Kit uses Translation Kit internally for clipboard operations.
// When you copy a FlowEntry, Translation Kit converts it to the
// format requested by the paste target.

let entry = FlowKit::clipboard_entry()?;

// Flow entry might be rich text; target app wants plain text
let roster = TranslationKit::roster();
let plain = roster.translate(
    entry.content_bytes(),
    &entry.format(),
    &FormatDescriptor::mime("text/plain"),
)?;

println!("Pasted as plain text: {} bytes", plain.data.len());
```

**Translation Kit + Browser Kit -- web content adaptation:**

```rust
use aios_translation::TranslationKit;
use aios_browser::BrowserKit;

// Browser copies HTML content; a note-taking agent wants Markdown.
// Translation Kit bridges the gap without either knowing about the other.

let roster = TranslationKit::roster();
let html = BrowserKit::selected_html()?;

let markdown = roster.translate(
    html.as_bytes(),
    &FormatDescriptor::mime("text/html"),
    &FormatDescriptor::mime("text/markdown"),
)?;

// The user pastes web content as Markdown seamlessly
note_editor.insert(&String::from_utf8_lossy(&markdown.data));
```

**Translation Kit + Storage Kit -- format-aware import:**

```rust
use aios_translation::TranslationKit;
use aios_storage::SpaceKit;

// Import a file by finding a translator chain to our native format
let file_data = SpaceKit::read_object(object_id)?;
let file_format = FormatDescriptor::mime(&file_data.content_type);
let native_format = FormatDescriptor::mime("application/x-myapp-document");

let roster = TranslationKit::roster();
if let Ok(path) = roster.find_path(&file_format, &native_format) {
    println!(
        "Import path: {} hops (quality {:.0}%)",
        path.hop_count,
        path.quality * 100.0,
    );
    let converted = roster.translate(&file_data.bytes, &file_format, &native_format)?;
    open_document(&converted.data);
} else {
    println!("No import path for {}", file_format.mime_type);
}
```

## 5. Capability Requirements

| Method | Required Capability | Default Grant |
| --- | --- | --- |
| `TranslationRoster::register` | `TranslatorRegister` | Granted to all agents |
| `TranslationRoster::unregister` | `TranslatorRegister` | Own translators only |
| `TranslationRoster::translate` | None | Always available |
| `TranslationRoster::can_translate` | None | Always available |
| `TranslationRoster::find_path` | None | Always available |
| `TranslationRoster::known_formats` | None | Always available |
| `TranslationRoster::reachable_formats` | None | Always available |
| `TranslationRoster::translators` | None | Always available |

**Agent manifest example:**

```toml
[capabilities.required]
TranslatorRegister = "Register format translators for custom file format"
```

Consuming translations requires no capabilities -- any agent can convert between formats
using the roster. Only registering new translators requires a capability, ensuring that
the system can audit which agents contribute to the conversion graph.

## 6. Error Handling

```rust
/// Errors returned by Translation Kit operations.
#[derive(Debug)]
pub enum TranslationError {
    /// No conversion path exists between the source and target formats.
    NoPathFound {
        from: FormatDescriptor,
        to: FormatDescriptor,
    },

    /// A translator in the chain failed during conversion.
    TranslationFailed {
        translator: String,
        step: u32,
        reason: String,
    },

    /// The input data is malformed for the declared source format.
    InvalidInput {
        expected_format: FormatDescriptor,
        reason: String,
    },

    /// The translation output exceeds the maximum allowed size.
    OutputTooLarge { size: usize, max: usize },

    /// The required capability was not granted.
    CapabilityDenied(String),

    /// The translator ID was not found in the roster.
    TranslatorNotFound(TranslatorId),

    /// The conversion path quality is below the minimum threshold.
    QualityTooLow { quality: f32, minimum: f32 },

    /// The translation timed out (multi-hop chain too slow).
    Timeout { elapsed_ms: u32, limit_ms: u32 },
}
```

**Recovery guidance:**

| Error | Recovery |
| --- | --- |
| `NoPathFound` | Check `reachable_formats()` for alternatives; fall back to plain text |
| `TranslationFailed` | Retry with a different path if multiple exist; report to developer |
| `InvalidInput` | Verify the source format declaration matches actual content |
| `OutputTooLarge` | Split input into smaller chunks or use media pipeline for large data |
| `QualityTooLow` | Accept lower quality with explicit opt-in, or choose a closer format |
| `Timeout` | Reduce hop count by finding a more direct path |

## 7. Platform & AI Availability

**Built-in translators (available on all platforms):**

| From | To | Quality | Notes |
| --- | --- | --- | --- |
| `text/html` | `text/plain` | 0.7 | Strips tags, preserves structure |
| `text/html` | `text/markdown` | 0.85 | Preserves headings, links, lists |
| `text/markdown` | `text/html` | 0.95 | Full Markdown rendering |
| `text/markdown` | `text/plain` | 0.7 | Strips formatting |
| `image/svg+xml` | `image/png` | 0.8 | Rasterization at configurable DPI |
| `image/webp` | `image/png` | 0.95 | Near-lossless |
| `image/png` | `image/jpeg` | 0.75 | Lossy compression |
| `application/json` | `text/plain` | 0.6 | Pretty-printed JSON text |

**AIRS-enhanced features:**

| Feature | What AIRS provides | Without AIRS |
| --- | --- | --- |
| Path optimization | Learns which conversion paths produce the best results per content type | Shortest-hop selection only |
| Quality estimation | Predicts output quality from content analysis before translating | Static quality scores |
| Format detection | Identifies actual content format when MIME type is missing or wrong | MIME-based only |
| Smart chaining | Discovers non-obvious conversion chains by analyzing translator capabilities | Graph shortest path only |

**Platform availability:**

| Platform | Built-in Translators | Custom Translators | Notes |
| --- | --- | --- | --- |
| QEMU virt | All text/image | Yes | Full Translation Kit |
| Raspberry Pi 4 | All text/image | Yes | Full Translation Kit |
| Raspberry Pi 5 | All text/image | Yes | Full Translation Kit |
| Apple Silicon | All text/image | Yes | Full Translation Kit |

Translation Kit is platform-independent -- it operates entirely on in-memory data
with no hardware dependencies. Availability depends only on which translators are
registered, not on the underlying hardware.

**Implementation phase:** Phase 10+. Translation Kit depends on [Memory Kit](../kernel/memory.md)
for buffer management. It is consumed by [Flow Kit](../intelligence/flow.md) for clipboard and
drag-and-drop operations, and by any agent that imports or exports data in multiple formats.

---

*See also: [Flow Kit](../intelligence/flow.md) | [Storage Kit](./storage.md) | [Media Kit](./media.md)*
