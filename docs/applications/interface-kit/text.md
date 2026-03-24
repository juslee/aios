# AIOS Interface Kit — Text Rendering

Part of: [interface-kit.md](../interface-kit.md) — Interface Kit Architecture
**Related:** [rendering.md](./rendering.md) — Render pipeline, [terminal.md](../terminal/rendering.md) — Terminal font engine, [accessibility.md](./accessibility.md) — Screen reader text

-----

## 7. Text Rendering

Text is the hardest part of any UI toolkit. AIOS is a text-forward OS — summaries, search results, conversation, code — so text rendering must be excellent.

### 7.1 Text Pipeline

```text
Input string (Unicode)
  │
  ▼
Itemization (split into runs by script, direction, font)
  │
  ▼
Font fallback (select font for each run from fallback chain)
  │
  ▼
Shaping (Unicode codepoints → positioned glyphs)
  │    Uses: swash (pure Rust) or harfbuzz (C, via harfbuzz-rs)
  │    Handles: ligatures, kerning, contextual alternates
  ▼
Line breaking (Unicode UAX #14 line break algorithm)
  │    Handles: word wrap, hyphenation, CJK break rules
  ▼
Bidi reordering (Unicode UAX #9 bidirectional algorithm)
  │    Handles: mixed LTR/RTL text (English + Arabic)
  ▼
Layout (position lines, compute baselines, alignment)
  │
  ▼
Rasterization (glyphs → GPU texture atlas)
  │    Subpixel positioning for crisp rendering
  ▼
Rendering (textured quads to GPU)
```

### 7.2 Font Fallback

```rust
pub struct FontFallbackChain {
    fonts: Vec<FontDescriptor>,
}

impl FontFallbackChain {
    pub fn system_default() -> Self {
        FontFallbackChain {
            fonts: vec![
                FontDescriptor::new("Inter", Weight::NORMAL),       // Latin
                FontDescriptor::new("Noto Sans CJK", Weight::NORMAL), // CJK
                FontDescriptor::new("Noto Sans Arabic", Weight::NORMAL), // Arabic
                FontDescriptor::new("Noto Color Emoji", Weight::NORMAL), // Emoji
                FontDescriptor::new("Symbols Nerd Font", Weight::NORMAL), // Icons
            ],
        }
    }

    /// Find the first font that contains the given character
    pub fn font_for_char(&self, ch: char) -> &FontDescriptor {
        for font in &self.fonts {
            if font.contains_glyph(ch) {
                return font;
            }
        }
        &self.fonts[0] // fallback to primary
    }
}
```

### 7.3 Glyph Cache

Glyphs are rasterized once and cached in a GPU texture atlas. The cache is keyed by `(font_id, glyph_id, size, subpixel_offset)`:

```rust
pub struct GlyphCache {
    atlas: TextureAtlas,
    entries: HashMap<GlyphKey, AtlasEntry>,
    lru: LruIndex,
}

pub struct GlyphKey {
    font_id: FontId,
    glyph_id: u16,
    size: OrderedFloat<f32>,
    subpixel_x: u8,  // 0-3 for quarter-pixel positioning
    subpixel_y: u8,
}

impl GlyphCache {
    pub fn get_or_rasterize(
        &mut self,
        key: GlyphKey,
        rasterizer: &mut Rasterizer,
    ) -> &AtlasEntry {
        if !self.entries.contains_key(&key) {
            let image = rasterizer.rasterize(key.font_id, key.glyph_id, key.size);
            let entry = self.atlas.allocate(image.width, image.height);
            self.atlas.upload(entry.region, &image.data);
            self.entries.insert(key, entry);
        }
        self.lru.touch(&key);
        &self.entries[&key]
    }
}
```

-----

### 7.4 Internationalization (ICU4X)

Interface Kit uses ICU4X — a Rust-native Unicode library from the Unicode Consortium — for all internationalization needs. ICU4X is `no_std + alloc` compatible with zero-copy data loading, making it suitable for both kernel-adjacent code and user-space agents.

**Key integrations:**

- **Line breaking** (`icu_segmenter`): UAX #14 line break algorithm. Replaces ad-hoc line breaking logic. Handles CJK character breaks, Thai word breaks, and soft hyphenation.
- **Grapheme clusters** (`icu_segmenter`): UAX #29 grapheme cluster boundaries. Essential for cursor positioning — a single "character" like a family emoji is 7 Unicode code points but 1 grapheme cluster.
- **Bidirectional text** (`icu_properties` + custom bidi): UAX #9 bidirectional algorithm for mixed LTR/RTL text (English + Arabic/Hebrew).
- **Collation** (`icu_collator`): Locale-aware string sorting for list widgets, file browsers, and search results.
- **Number formatting** (`icu_decimal`): Locale-correct number display (1,234.56 vs 1.234,56).
- **Date/time formatting** (`icu_datetime`): Calendar-aware formatting for 400+ locales.

```rust
/// AIOS provides locale data via a custom DataProvider
/// that reads ICU4X data blobs from Spaces storage.
pub struct SpaceDataProvider {
    /// Space path containing ICU4X data files.
    space: SpaceId,
    /// Cached data blobs (zero-copy mmap from Space).
    cache: HashMap<DataKey, DataPayload>,
}

impl DataProvider<LocaleFallbackLikelySubtagsV1Marker> for SpaceDataProvider {
    fn load(&self, req: DataRequest) -> Result<DataResponse<...>, DataError> {
        // Load locale data from Spaces storage, zero-copy via mmap
        // ...
    }
}
```

**On non-AIOS platforms**, ICU4X loads data from bundled blobs compiled into the application binary via `icu_datagen`.

-----

### 7.5 RTL Layout Mirroring

When the primary text direction is RTL (Arabic, Hebrew), Interface Kit automatically mirrors the layout:

- `row![]` reverses child order (rightmost child first).
- `text_input()` aligns text to the right.
- Scrollbars move to the left side.
- Icons that imply direction (arrows, chevrons) are mirrored.

This is handled at the layout engine level, not per-widget. Widgets declare whether they are direction-sensitive:

```rust
pub trait Widget<M, R: Renderer> {
    // ... existing methods ...

    /// Whether this widget should be mirrored in RTL layouts.
    /// Default: true (most widgets mirror). Icons with inherent
    /// directionality (e.g., a "reply" arrow) return false.
    fn is_direction_sensitive(&self) -> bool { true }
}
```

-----

### 7.6 Variable Fonts

Interface Kit supports OpenType variable fonts, which encode multiple weights, widths, and optical sizes in a single font file:

```rust
pub struct VariableFontAxes {
    /// Weight axis (100=Thin, 400=Regular, 700=Bold, 900=Black).
    pub weight: f32,
    /// Width axis (75=Condensed, 100=Normal, 125=Expanded).
    pub width: f32,
    /// Optical size axis (auto-adjusted based on rendered size).
    pub optical_size: f32,
    /// Italic axis (0.0=Upright, 1.0=Italic).
    pub italic: f32,
}
```

Benefits for AIOS:
- **Single font file** per family reduces Space storage and memory usage.
- **Smooth weight transitions** for animation (e.g., bold-on-hover without font swapping).
- **Optical size** ensures text looks sharp at both caption (11px) and heading (28px) sizes — the font adjusts its design automatically.

The system font (Inter Variable or equivalent) ships as a variable font. The Theme system references weight/width values, not discrete font files.
