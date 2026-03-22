# Translation Kit

**Layer:** Platform | **Architecture:** `docs/storage/flow/transforms.md`

## Purpose

Format conversion between content types for clipboard exchange, drag-and-drop, and data import/export. A roster of registered Translators forms a directed conversion graph; consumers request the best path from a source format to a target format without knowing which translator handles it.

## Key APIs

| Trait / API | Description |
|---|---|
| `TranslationRoster` | Registry of available translators; resolves shortest conversion path between formats |
| `Translator` | Single-hop conversion trait: declares supported source/target formats, performs the transform |
| `FormatDescriptor` | Typed format identity (MIME type, UTI, or AIOS-native content type) with capability metadata |

## Dependencies

Memory Kit

## Consumers

Flow Kit, Browser Kit, applications

## Implementation Phase

Phase 10+
