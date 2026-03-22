# Conversation Kit

**Layer:** Application | **Architecture:** `docs/intelligence/conversation-manager.md` + sub-docs

## Purpose

Conversation Kit manages conversation sessions, context windows, and tool orchestration for interactions with AIRS. It handles streaming token delivery with backpressure, context compression across turns, and the Conversation Bar UI. It is the primary way users interact with AIRS and the primary way agents invoke tools.

## Key APIs

| Trait / API | Description |
|---|---|
| `ConversationSession` | Persistent session with history, forking, and cross-device continuity |
| `ContextWindow` | Assembles token budget from history, RAG results, and tool outputs |
| `ToolOrchestrator` | Discovers, invokes, and chains tools on behalf of AIRS during a turn |
| `ConversationBar` | Compositor-integrated UI surface for text input and streaming output |
| `StreamingDelivery` | Token-by-token delivery pipeline with backpressure and cancellation |

## Orchestrates

- **AIRS Kit** — inference engine that generates tokens and selects tools
- **Context Kit** — supplies ambient context signals to shape model behavior
- **Search Kit** — RAG retrieval to augment context windows with relevant content
- **Flow Kit** — content attachments, clipboard paste, and output sharing
- **Capability Kit** — gates which tools and data sources a session may access

## Implementation Phase

Phase 14+
