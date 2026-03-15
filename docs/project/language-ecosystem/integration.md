# AIOS Language Ecosystem: Integration & Build Plan

Part of: [language-ecosystem.md](../language-ecosystem.md) — Language Ecosystem
**Related:** [runtimes.md](./runtimes.md) — Runtime deep dives, [operations.md](./operations.md) — Operations & security, [ai.md](./ai.md) — AI-driven optimization

---

## 6. How It All Fits Together

### When Each Language Arrives

| Language | Introduced | Tooling Complete | Self-Hosting on AIOS |
|---|---|---|---|
| Rust | Phase 0 (kernel) | Phase 16 (SDK) | Phase 22+ (needs rustc + LLVM) |
| Python | Phase 16 | Phase 16 | Phase 16 (RustPython ships with OS) |
| TypeScript | Phase 16 | Phase 16 | Phase 16 (QuickJS-ng ships with OS) |
| WASM | Phase 16 (agents) | Phase 16 + 30 (browser) | N/A (compile on host, deploy .wasm) |
| C/C++ | Phase 22 | Phase 22f | Phase 22f (clang builds on AIOS) |
| Linux binaries | Phase 35 | Phase 35 | Whatever runs on Linux |

### The Dependency Chain

```mermaid
flowchart TD
    P03["`Phases 0-3: Kernel boots, IPC, capabilities
*Rust kernel code compiles on HOST, runs on AIOS*`"]
    P47["`Phases 4-7: Storage, GPU, networking
*Foundation for all language runtimes*`"]
    P813["`Phases 8-13: AIRS, agents framework
*AI inference available to all languages*`"]
    P16["`Phase 16: SDK + Developer Experience
*Python RustPython, TypeScript QuickJS-ng, WASM wasmtime ON AIOS
Rust SDK published -- develop on HOST*`"]
    P21["`Phase 21: Performance optimization
*All runtimes tuned for production*`"]
    P22["`Phase 22: POSIX + BSD Userland
*C/C++ clang ON AIOS -- FIRST COMPILED LANGUAGE
CPython + Node.js available*`"]
    P22P["`Phase 22+: Cross-compile rustc
*Rust development ON AIOS -- RUST SELF-HOSTING*`"]
    P23P["`Phase 23+: Native rustc compiles rustc
*Full self-hosting -- AIOS COMPILES ITSELF*`"]
    P35["`Phase 35: Linux binary compatibility
*ANY Linux program runs -- UNIVERSAL COMPATIBILITY*`"]

    P03 --> P47 --> P813 --> P16
    P16 --> P21 --> P22 --> P22P --> P23P
    P23P --> P35
```

### What Each Phase Unlocks for Developers

| Phase | What You Can Do | Where You Do It |
|---|---|---|
| 16 | Write Python/TS/WASM agents for AIOS | On host OR on AIOS |
| 16 | Write Rust agents for AIOS | On host only (cross-compile) |
| 22 | Write C programs on AIOS | On AIOS natively |
| 22 | Use CPython with C extensions on AIOS | On AIOS natively |
| 22+ | Write Rust programs on AIOS | On AIOS natively |
| 23+ | Compile AIOS kernel on AIOS | On AIOS natively |
| 35 | Run any Linux binary on AIOS | On AIOS natively |

### Runtime Comparison

| Dimension | Rust | Python | TypeScript | WASM |
|---|---|---|---|---|
| Runtime | None (native) | RustPython | QuickJS-ng | wasmtime (AOT) |
| Startup | < 1 ms | ~50 ms | < 5 ms | < 1 ms (pre-compiled) |
| Performance | Baseline | 10-50x slower | 10-50x slower | ~1.2-3x slower |
| Memory overhead | None | ~10 MB interpreter | < 1 MB engine | ~5 MB runtime |
| Binary size | ~1-10 MB | ~20 MB (interpreter) | ~700 KB (engine) | ~15 MB (wasmtime) |
| C extension support | Via FFI | No (RustPython) | No | No |
| Trust level | Trusted | Semi-trusted | Semi-trusted | Untrusted OK |
| Available on AIOS | Phase 16 (SDK) | Phase 16 | Phase 16 | Phase 16 |
| Self-hosting on AIOS | Phase 22+ | Phase 16 | Phase 16 | Host-compiled |

---

## 7. What Needs to Be Built

### Per-Language Implementation Work

**Rust SDK (Phase 13-16):**

- [ ] `aios-sdk` crate with `AgentContext` trait
- [ ] `#[agent]` proc macro for entry point generation
- [ ] Syscall wrappers for all 31 AIOS syscalls
- [ ] IPC message builders for Space, Network, AIRS services
- [ ] Hot-reload support (< 2s incremental builds)
- [ ] `aios agent new/dev/test/publish` CLI workflow

**Python Runtime (Phase 16):**

- [ ] Embed RustPython into agent process
- [ ] RustPython embedding bindings for `AgentContext`
- [ ] `aios-sdk` pip package
- [ ] Restricted stdlib implementation (remove dangerous modules)
- [ ] `open()` / `os.path` redirection to Space API
- [ ] Dependency resolution and hash-pinning at install time (no pip at runtime)
- [ ] Async support (`asyncio` event loop integration)

**TypeScript Runtime (Phase 16):**

- [ ] Embed QuickJS-ng into agent process
- [ ] napi-like bridge for `AgentContext`
- [ ] `@aios/sdk` npm package
- [ ] TypeScript → JavaScript transpilation at install time
- [ ] `fetch()` redirection through Network Translation Module
- [ ] Promise/async integration with AIOS IPC

**WASM Runtime (Phase 16):**

- [ ] Integrate wasmtime into agent process
- [ ] WASI-to-AIOS syscall bridge (WASI 0.2.0 baseline)
- [ ] AOT compilation pipeline (install-time .wasm → native)
- [ ] Memory limits and fuel metering
- [ ] WASI Component Model support for capability passing
- [ ] WIT interface definitions for AIOS agent APIs

**C/C++ Toolchain (Phase 22):**

- [ ] musl libc port (syscall dispatch → AIOS IPC)
- [ ] POSIX translation layer (FD table, path resolver, process lifecycle)
- [ ] LLVM/clang cross-compiled for AIOS
- [ ] Self-hosting: clang compiles clang on AIOS

**Rust Self-Hosting (Phase 22+):**

- [ ] Cross-compile rustc + cargo for AIOS aarch64
- [ ] Verify rustc works through POSIX layer
- [ ] Native Rust compilation on AIOS
- [ ] rustc compiles rustc on AIOS (full self-hosting)

---

## 8. Key Architectural Decisions

### Why These Four Languages?

From the architecture docs, the selection criteria were:

1. **Rust** — AIOS is written in Rust. Native performance. Systems programming.
2. **Python** — Largest AI/ML ecosystem. Most agent developers know Python.
3. **TypeScript** — Largest web developer population. Type safety over JavaScript.
4. **WASM** — Language-agnostic sandbox for untrusted code. Future-proof.

These four cover ~90% of the developer population that would build AIOS agents.

### Why Embedded Interpreters Instead of System Runtimes?

The key insight: embedded interpreters (RustPython, QuickJS-ng) are available at **Phase 16**,
while system runtimes (CPython, Node.js) require the POSIX layer at **Phase 22**. By embedding
the interpreters directly into the agent process, AIOS gets multi-language support 3 phases
earlier — before the POSIX layer even exists.

The tradeoff is performance (embedded interpreters are slower) and compatibility (no C extensions,
no Node.js stdlib). For agent workloads that are I/O-bound (waiting on AI inference, space
queries, network requests), this tradeoff is acceptable.

### Why QuickJS-ng Over Boa?

Both QuickJS-ng and Boa are viable JavaScript engines for AIOS. The decision factors:

| Factor | QuickJS-ng (chosen) | Boa (future candidate) |
|---|---|---|
| Performance | Baseline | ~3-5x slower |
| Language | C (minimal deps) | Rust (pure, zero C deps) |
| ECMAScript conformance | ~85% test262 | >90% test262 |
| AIOS alignment | Good (embeds easily) | Excellent (Rust-native) |

QuickJS-ng is chosen for Phase 16 because agent workloads need adequate performance now.
Boa's pure-Rust nature makes it the preferred long-term choice once its performance reaches
parity — eliminating the only C dependency in the agent runtime stack.

### Security Equivalence Across Runtimes

All four runtimes enforce identical capability semantics. The `RuntimeAdapter` trait provides
the abstraction:

```rust
pub trait RuntimeAdapter: Send + Sync {
    /// Initialize the runtime (load interpreter, JIT, etc.)
    fn init(&mut self, manifest: &AgentManifest) -> Result<()>;
    /// Load the agent's code
    fn load(&mut self, code: &[u8]) -> Result<()>;
    /// Create an AgentContext bridge for this runtime
    fn create_context(&self, channels: &ChannelSet) -> Box<dyn AgentContext>;
    /// Start the agent's event loop
    fn run(&mut self, ctx: Box<dyn AgentContext>) -> Result<AgentResult>;
    /// Signal shutdown
    fn shutdown(&mut self, deadline: Timestamp);
    /// Runtime type identifier
    fn runtime_type(&self) -> RuntimeType;
}

// Four implementations:
pub struct NativeRuntime;      // Rust — direct execution
pub struct PythonRuntime;      // RustPython or CPython
pub struct TypeScriptRuntime;  // QuickJS-ng or V8
pub struct WasmRuntime;        // wasmtime — AOT-compiled WASM
```

A Python agent with `[spaces.read, ai.complete]` capabilities can do exactly what a Rust agent
with the same capabilities can do — nothing more, nothing less. The runtime cannot grant
capabilities the manifest doesn't declare.

Each runtime gets a pre-audited capability profile at Layer 10 of the composable capability
system (see [capabilities.md](../../security/model/capabilities.md) §3.7):
`runtime.native.v1`, `runtime.python.v1`, `runtime.typescript.v1`, `runtime.wasm.v1`.
These profiles grant the minimum capabilities each runtime needs to function (interpreter
memory, temp space, IPC channels) without granting anything beyond what the agent manifest
declares.
