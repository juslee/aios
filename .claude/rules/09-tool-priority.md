# Tool Priority

## LSP First

When working with Rust code, **always prefer LSP** over manual searching:

| Task | Use | Not |
|---|---|---|
| Find definition | `LSP goToDefinition` | Grep/Glob for symbol name |
| Find usages | `LSP findReferences` | Grep across codebase |
| Get type info | `LSP hover` | Reading source to infer types |
| List symbols in file | `LSP documentSymbol` | Reading entire file |
| Search symbols workspace-wide | `LSP workspaceSymbol` | Glob + Grep combination |
| Find trait implementations | `LSP goToImplementation` | Manual search |
| Trace call chains | `LSP incomingCalls/outgoingCalls` | Manual code tracing |

## General Tool Priority

From most preferred to least preferred:

1. **LSP** — semantic code intelligence (definitions, references, types, call hierarchy)
2. **Grep** — content search (patterns, strings, regex)
3. **Glob** — file discovery (by name pattern)
4. **Read** — file contents (when you need to read implementation details)
5. **Bash** — system commands (only when dedicated tools cannot accomplish the task)

## When NOT to Use LSP

- File doesn't exist yet (use Read/Grep to find similar patterns)
- Searching for string literals or comments (use Grep)
- Finding files by name (use Glob)
- Non-Rust files (LSP only configured for `.rs`)
