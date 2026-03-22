---
author: Justin Lee
date: 2026-03-22
tags: [platform, storage, flow]
status: final
---

# ADR: Translation Kit for Format Conversion

## Context

BeOS had a Translation Kit (BTranslatorRoster, BTranslator) for converting between data formats (PNG <-> JPEG <-> BMP). AIOS's Flow Kit handles typed content exchange with content transformation, but the actual format conversion logic needs a home.

Should format conversion live inside individual Kits (Media Kit for AV, Storage Kit for documents) or in a dedicated Translation Kit?

## Options Considered

### Option A: Embed in individual Kits

- Pros: No new Kit, conversion logic close to domain expertise
- Cons: Duplicated patterns, no unified conversion graph, each Kit reinvents converter registration

### Option B: Dedicated Translation Kit

- Pros: Unified converter registry, conversion graph (find path from format A to format B), single pattern for all format types, Flow Kit delegates to it cleanly, BeOS heritage
- Cons: One more Kit, some domain-specific converters still live in their domain Kit

## Decision

Translation Kit (Option B). A Platform Kit dedicated to format conversion.

Responsibilities:
- Converter registry (register translators for format pairs)
- Conversion graph (find optimal path between any two formats)
- Image formats (PNG, JPEG, WebP, BMP, SVG rasterization)
- Document formats (plain text, markdown, HTML, PDF extraction)
- Data formats (JSON, CSV, TOML, YAML)
- Used by Flow Kit for content transformation during drag-drop and clipboard operations

Domain-specific codecs (audio, video) remain in Media Kit — Translation Kit handles non-streaming formats.

## Consequences

- Flow Kit delegates content transformation to Translation Kit
- Extensible — agents can register new format converters
- Conversion graph enables automatic multi-step conversion (e.g., SVG -> PNG -> JPEG)
- Media Kit keeps audio/video codecs (streaming formats need different handling)
