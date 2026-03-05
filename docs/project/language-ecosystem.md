# AIOS Language Ecosystem: Technical Deep Dive

**Parent document:** [architecture.md](./architecture.md), [development-plan.md](./development-plan.md)
**Related:** [agents.md](../applications/agents.md) — Agent SDK and runtime adapters, [posix.md](../platform/posix.md) — C/C++ toolchain and POSIX layer, [browser.md](../applications/browser.md) — SpiderMonkey/WASM in browser
**Scope:** Rust, Python, TypeScript, WASM — how each runs on AIOS, what's needed, when it arrives

---

## 1. Overview

AIOS supports four first-class development languages through a unified capability model. Every
language runtime enforces the same security boundaries — the language is an implementation detail,
the capability set is the security boundary.

```
┌─────────────────────────────────────────────────────────────┐
│                    Agent Manifest (manifest.toml)            │
│            Declares: runtime, capabilities, schedule         │
├─────────────────────────────────────────────────────────────┤
│                    RuntimeAdapter trait                       │
│         NativeRuntime  PythonRuntime  TSRuntime  WasmRuntime │
├──────┬──────────┬────────────┬───────────────────────────────┤
│ Rust │  Python  │ TypeScript │           WASM                │
│native│RustPython│  QuickJS   │         wasmtime              │
│ ELF  │embedded  │ embedded   │      AOT-compiled             │
├──────┴──────────┴────────────┴───────────────────────────────┤
│              AIOS Capability System (8 layers)               │
│         Intent → Capability → Behavior → Zone → ...         │
├─────────────────────────────────────────────────────────────┤
│              AIOS Kernel (31 syscalls, IPC)                   │
└─────────────────────────────────────────────────────────────┘
```

### When Each Language Arrives

| Language | Introduced | Tooling Complete | Self-Hosting on AIOS |
|---|---|---|---|
| Rust | Phase 0 (kernel) | Phase 12 (SDK) | Phase 15+ (needs rustc + LLVM) |
| Python | Phase 12 | Phase 12 | Phase 12 (RustPython ships with OS) |
| TypeScript | Phase 12 | Phase 12 | Phase 12 (QuickJS ships with OS) |
| WASM | Phase 12 (agents) | Phase 12 + 21 (browser) | N/A (compile on host, deploy .wasm) |
| C/C++ | Phase 15 | Phase 15f | Phase 15f (clang builds on AIOS) |
| Linux binaries | Phase 25 | Phase 25 | Whatever runs on Linux |

---

## 2. Rust — Native Performance, Zero Overhead

### How It Works

Rust agents compile to native aarch64 ELF binaries. The `aios-sdk` crate provides direct
syscall wrappers and IPC message builders. No interpreter, no VM, no runtime overhead.

```rust
use aios_sdk::prelude::*;

#[agent]
async fn my_agent(ctx: AgentContext) -> Result<()> {
    // Direct IPC to Space Service — compiles to syscall instructions
    let results = ctx.spaces().query("project notes").await?;

    // Direct IPC to AIRS — compiles to syscall instructions
    let summary = ctx.ai().complete("Summarize these notes", &results).await?;

    Ok(())
}
```

### What's Needed to Run on AIOS

| Component | Source | Phase |
|---|---|---|
| `aios-sdk` crate | Built with AIOS | Phase 10 |
| `#[agent]` proc macro | Generates entry point + manifest parsing | Phase 10 |
| Rust compiler (cross) | rustc on host, target aarch64-unknown-none | Phase 0+ |
| Rust compiler (native) | rustc running ON AIOS | Phase 15+ |

### Self-Hosting: When Can You Write Rust ON AIOS?

This is the hardest self-hosting problem because `rustc` depends on LLVM (C++):

```
Phase 15a:  musl libc compiled on AIOS (C library available)
Phase 15f:  LLVM/clang compiled on AIOS (C++ compiler available)
            → Now LLVM libraries exist natively
Phase 15+:  Cross-compile rustc for AIOS from host
            → Ship as pre-built binary initially
Phase 16+:  Native rustc compiles rustc on AIOS
            → Full Rust self-hosting achieved
```

**The blocker isn't Rust — it's LLVM.** Rust's compiler uses LLVM as its code generation backend.
Until LLVM runs natively on AIOS (Phase 15f), `rustc` can't run natively either. The practical
path: cross-compile `rustc` on the host and ship it as a pre-built AIOS binary, then later
achieve true self-hosting.

### Development Workflow (Phase 12+)

```bash
# On host (Mac/Linux) — primary development path
aios agent new my-agent --lang rust
aios agent dev                    # Hot-reload, < 2s incremental builds
aios agent test                   # Run tests against mock AIOS services
aios agent publish                # Package and deploy to AIOS

# On AIOS (Phase 15f+) — once rustc is available natively
cargo build --release             # Compile directly on AIOS
aios agent install ./target/      # Install from local build
```

---

## 3. Python — RustPython Embedded Interpreter

### How It Works

Python agents run inside an embedded **RustPython** interpreter (pure Rust, no C dependencies).
The interpreter lives inside the agent process sandbox. PyO3 bindings expose the `AgentContext`
to Python code.

```python
from aios_sdk import agent, spaces, ai

@agent
async def my_agent(ctx):
    # Same capability-gated API as Rust
    results = await ctx.spaces.query("project notes")
    summary = await ctx.ai.complete("Summarize these notes", results)
    return summary
```

### What's Needed to Run on AIOS

| Component | Source | License | Phase |
|---|---|---|---|
| RustPython interpreter | github.com/RustPython/RustPython | MIT | Phase 12 |
| PyO3 bindings | github.com/PyO3/pyo3 | Apache-2.0/MIT | Phase 12 |
| `aios-sdk` pip package | Built with AIOS | BSD-2-Clause | Phase 12 |
| Agent-local `site-packages/` | Declared in manifest, installed at install time | — | Phase 12 |

### Security: Restricted Standard Library

Python's stdlib is powerful and dangerous. AIOS surgically restricts it:

**Removed entirely** (bypass the sandbox):
- `os.system()`, `subprocess` — arbitrary command execution
- `socket` — raw network access (must use SDK's capability-gated `fetch()`)
- `ctypes`, `cffi` — FFI to native code (escape the sandbox)
- `multiprocessing` — process spawning (must use agent spawning API)

**Redirected through Space API**:
- `open()` → reads/writes through the Space Service, capability-checked
- `os.path`, `os.getcwd` → operates on the agent's space view
- `importlib` → restricted to agent-local packages only

**Unchanged**: `json`, `re`, `datetime`, `collections`, `itertools`, `math`, `hashlib`,
`base64`, `urllib.parse`, `dataclasses`, `typing`, `asyncio` — safe pure-Python modules.

### Why RustPython, Not CPython?

| | RustPython | CPython |
|---|---|---|
| Language | Pure Rust | C |
| Dependencies | Zero C deps | Needs libc, libm, pthreads |
| Sandbox integration | Compiles into agent binary | Requires POSIX layer (Phase 15) |
| Available at | Phase 12 | Phase 15 (needs POSIX) |
| Performance | Slower (~2-10x vs CPython) | Baseline |
| Compatibility | ~95% of pure Python | 100% |
| C extensions | No | Yes |

RustPython is available **3 phases earlier** than CPython because it doesn't need the POSIX layer.
For agents (which use the AIOS SDK, not C extensions), this tradeoff is worth it.

After Phase 15, CPython becomes available through the POSIX layer for workloads that need C
extension compatibility (numpy, etc.).

### Self-Hosting: When Can You Write Python ON AIOS?

**Phase 12.** RustPython ships with the OS. You can write and run Python agents directly on AIOS
from Phase 12 onward. No cross-compilation needed — Python is interpreted.

```bash
# On AIOS (Phase 12+)
aios agent new my-agent --lang python
# Edit .py files directly on AIOS
aios agent dev                    # Runs immediately via RustPython
```

This makes Python the **first self-hosting development language** on AIOS (alongside TypeScript),
arriving 3 phases before C/C++ (Phase 15) and ~4 phases before Rust (Phase 16+).

---

## 4. TypeScript — QuickJS Embedded Runtime

### How It Works

TypeScript agents run inside an embedded **QuickJS** JavaScript engine (small, embeddable, C).
TypeScript is transpiled to JavaScript at install time. A napi-like bridge exposes `AgentContext`.

```typescript
import { agent, AgentContext } from '@aios/sdk';

export default agent(async (ctx: AgentContext) => {
    // Same capability-gated API as Rust and Python
    const results = await ctx.spaces.query("project notes");
    const summary = await ctx.ai.complete("Summarize these notes", results);
    return summary;
});
```

### What's Needed to Run on AIOS

| Component | Source | License | Phase |
|---|---|---|---|
| QuickJS engine | bellard.org/quickjs | MIT | Phase 12 |
| napi-like bridge | Custom, built with AIOS | BSD-2-Clause | Phase 12 |
| `@aios/sdk` npm package | Built with AIOS | BSD-2-Clause | Phase 12 |
| TypeScript transpiler | Bundled (runs at install time) | Apache-2.0 | Phase 12 |

### Security: No Node.js Standard Library

TypeScript agents have **no access to Node.js APIs**. No `fs`, `net`, `child_process`, `http`,
`crypto` (Node's), `os`, `path`, `stream`, `buffer`, `worker_threads`.

All I/O goes through the AIOS SDK:
- `ctx.spaces.query()` instead of `fs.readFile()`
- `ctx.network.fetch()` instead of `http.request()` — capability-gated
- `ctx.ai.complete()` instead of calling an external API

`fetch()` is available but redirected through the Network Translation Module, which enforces
capability gates on which domains the agent can contact.

### Why QuickJS, Not V8?

| | QuickJS | V8 |
|---|---|---|
| Binary size | ~700 KB | ~30+ MB |
| Startup time | < 5 ms | ~50-100 ms |
| JIT compilation | No (interpreter only) | Yes |
| Peak performance | 10-50x slower than V8 | Baseline |
| Memory usage | < 1 MB base | ~10+ MB base |
| Dependencies | Minimal C | Large C++ codebase |
| AIOS integration | Embeds easily | Requires POSIX layer |
| Available at | Phase 12 | Phase 21 (via Servo/SpiderMonkey) |

QuickJS is chosen for the same reason as RustPython: it's **small, embeddable, and available
before the POSIX layer.** Agent workloads are I/O-bound (waiting on AIRS, Space queries, network),
so QuickJS's slower execution speed rarely matters.

For compute-heavy JavaScript (browser workloads), SpiderMonkey arrives in Phase 21 via Servo.

### Self-Hosting: When Can You Write TypeScript ON AIOS?

**Phase 12.** QuickJS ships with the OS. TypeScript transpilation happens at install time.
You can write and run TypeScript agents directly on AIOS from Phase 12 onward.

---

## 5. WebAssembly — Universal Sandbox

### How It Works

WASM agents run in **wasmtime** (Rust-based WASM runtime). Modules are AOT-compiled to native
aarch64 at install time — no JIT at startup. Only WASI imports are available; no direct syscall
access and no shared memory.

```rust
// Any language that compiles to WASM works
// Rust example:
#[no_mangle]
pub fn agent_main() {
    let query = aios_wasi::spaces_query("project notes");
    let summary = aios_wasi::ai_complete("Summarize", &query);
    aios_wasi::output(summary);
}
```

### What's Needed to Run on AIOS

| Component | Source | License | Phase |
|---|---|---|---|
| wasmtime runtime | github.com/bytecodealliance/wasmtime | Apache-2.0/MIT | Phase 12 |
| WASI-to-AIOS bridge | Custom — maps WASI imports to AIOS IPC | BSD-2-Clause | Phase 12 |
| AOT compiler | wasmtime's Cranelift (compiles .wasm → native at install) | Apache-2.0 | Phase 12 |

### Two WASM Paths

**Agent WASM (Phase 12):** WASM modules run in wasmtime inside the agent sandbox. Double-sandboxed:
WASM's linear memory sandbox inside AIOS's capability sandbox.

```
┌──────────────────────────────────┐
│ AIOS Agent Sandbox (capabilities)│
│  ┌────────────────────────────┐  │
│  │ WASM Sandbox (wasmtime)    │  │
│  │  Linear memory only        │  │
│  │  WASI imports only         │  │
│  │  No shared memory          │  │
│  │  No direct syscalls        │  │
│  └────────────────────────────┘  │
└──────────────────────────────────┘
```

**Browser WASM (Phase 21):** WASM runs inside SpiderMonkey (via Servo) within Tab Agents.
Web API imports are capability-checked at the OS level — more secure than traditional browser
WASM because enforcement is hardware-backed (MMU), not just browser-logic.

### Why WASM Matters for AIOS

WASM is the **untrusted code** runtime. For agents from unknown authors or third-party plugins:

| Property | WASM | Native (Rust) | Interpreted (Python/TS) |
|---|---|---|---|
| Memory safety | Guaranteed (linear memory) | Developer's responsibility | Runtime-enforced |
| Syscall access | None (WASI only) | Direct | SDK-mediated |
| Language support | Any (Rust, C, Go, Zig, etc.) | Rust only | Python or TS only |
| Performance | Near-native (AOT compiled) | Native | 10-50x slower |
| Trust level | Untrusted OK | Trusted only | Semi-trusted |
| Binary portability | Universal | aarch64 only | Source-portable |

### Self-Hosting: WASM Development on AIOS

WASM modules are compiled on the host and deployed as `.wasm` files. The AOT compilation
(`.wasm` → native aarch64) happens at install time on AIOS via wasmtime's Cranelift backend.

To compile WASM **on** AIOS, you'd need a compiler targeting WASM running natively:
- **Rust → WASM**: Needs `rustc` with `wasm32-wasi` target (Phase 16+)
- **C → WASM**: Needs clang with `wasm32-wasi` target (Phase 15f)
- **AssemblyScript → WASM**: Needs Node.js or QuickJS-compatible tooling (Phase 12+)

---

## 6. How It All Fits Together

### The Dependency Chain

```
Phase 0-3:   Kernel boots, IPC works, capabilities enforced
             → Rust kernel code compiles on HOST, runs on AIOS

Phase 4-7:   Storage, GPU, networking
             → Foundation for all language runtimes

Phase 8-11:  AIRS, agents framework
             → AI inference available to all languages

Phase 12:    SDK + Developer Experience
             → Python (RustPython) available ON AIOS    ← FIRST INTERPRETED LANGUAGES
             → TypeScript (QuickJS) available ON AIOS
             → WASM (wasmtime) available ON AIOS
             → Rust SDK published (develop on HOST)

Phase 14:    Performance optimization
             → All runtimes tuned for production

Phase 15:    POSIX + BSD Userland
             → C/C++ (clang) available ON AIOS          ← FIRST COMPILED LANGUAGE ON AIOS
             → CPython available (C extension compat)
             → Node.js available (V8, full compat)

Phase 15+:   Cross-compile rustc for AIOS
             → Rust development ON AIOS                  ← RUST SELF-HOSTING

Phase 16+:   Native rustc compiles rustc
             → Full self-hosting                         ← AIOS COMPILES ITSELF

Phase 25:    Linux binary compatibility
             → ANY Linux program runs                    ← UNIVERSAL COMPATIBILITY
```

### What Each Phase Unlocks for Developers

| Phase | What You Can Do | Where You Do It |
|---|---|---|
| 12 | Write Python/TS/WASM agents for AIOS | On host OR on AIOS |
| 12 | Write Rust agents for AIOS | On host only (cross-compile) |
| 15 | Write C programs on AIOS | On AIOS natively |
| 15 | Use CPython with C extensions on AIOS | On AIOS natively |
| 15+ | Write Rust programs on AIOS | On AIOS natively |
| 16+ | Compile AIOS kernel on AIOS | On AIOS natively |
| 25 | Run any Linux binary on AIOS | On AIOS natively |

### Runtime Comparison

| Dimension | Rust | Python | TypeScript | WASM |
|---|---|---|---|---|
| Runtime | None (native) | RustPython | QuickJS | wasmtime (AOT) |
| Startup | < 1 ms | ~50 ms | < 5 ms | < 1 ms (pre-compiled) |
| Performance | Baseline | 10-50x slower | 10-50x slower | ~1.2-2x slower |
| Memory overhead | None | ~10 MB interpreter | < 1 MB engine | ~5 MB runtime |
| Binary size | ~1-10 MB | ~20 MB (interpreter) | ~700 KB (engine) | ~15 MB (wasmtime) |
| C extension support | Via FFI | No (RustPython) | No | No |
| Trust level | Trusted | Semi-trusted | Semi-trusted | Untrusted OK |
| Available on AIOS | Phase 12 (SDK) | Phase 12 | Phase 12 | Phase 12 |
| Self-hosting on AIOS | Phase 15+ | Phase 12 | Phase 12 | Host-compiled |

---

## 7. What Needs to Be Built

### Per-Language Implementation Work

**Rust SDK (Phase 10-12):**
- [ ] `aios-sdk` crate with `AgentContext` trait
- [ ] `#[agent]` proc macro for entry point generation
- [ ] Syscall wrappers for all 31 AIOS syscalls
- [ ] IPC message builders for Space, Network, AIRS services
- [ ] Hot-reload support (< 2s incremental builds)
- [ ] `aios agent new/dev/test/publish` CLI workflow

**Python Runtime (Phase 12):**
- [ ] Embed RustPython into agent process
- [ ] PyO3 bindings for `AgentContext`
- [ ] `aios-sdk` pip package
- [ ] Restricted stdlib implementation (remove dangerous modules)
- [ ] `open()` / `os.path` redirection to Space API
- [ ] Dependency resolution at install time (no pip at runtime)
- [ ] Async support (`asyncio` event loop integration)

**TypeScript Runtime (Phase 12):**
- [ ] Embed QuickJS into agent process
- [ ] napi-like bridge for `AgentContext`
- [ ] `@aios/sdk` npm package
- [ ] TypeScript → JavaScript transpilation at install time
- [ ] `fetch()` redirection through Network Translation Module
- [ ] Promise/async integration with AIOS IPC

**WASM Runtime (Phase 12):**
- [ ] Integrate wasmtime into agent process
- [ ] WASI-to-AIOS syscall bridge
- [ ] AOT compilation pipeline (install-time .wasm → native)
- [ ] Memory limits and fuel metering
- [ ] WASI preview 2 support for capability passing

**C/C++ Toolchain (Phase 15):**
- [ ] musl libc port (syscall dispatch → AIOS IPC)
- [ ] POSIX translation layer (FD table, path resolver, process lifecycle)
- [ ] LLVM/clang cross-compiled for AIOS
- [ ] Self-hosting: clang compiles clang on AIOS

**Rust Self-Hosting (Phase 15+):**
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

The key insight: embedded interpreters (RustPython, QuickJS) are available at **Phase 12**,
while system runtimes (CPython, Node.js) require the POSIX layer at **Phase 15**. By embedding
the interpreters directly into the agent process, AIOS gets multi-language support 3 phases
earlier — before the POSIX layer even exists.

The tradeoff is performance (embedded interpreters are slower) and compatibility (no C extensions,
no Node.js stdlib). For agent workloads that are I/O-bound (waiting on AI inference, space
queries, network requests), this tradeoff is acceptable.

### Security Equivalence

All four runtimes enforce identical capability semantics. The `RuntimeAdapter` trait provides
the abstraction:

```rust
trait RuntimeAdapter {
    fn initialize(&mut self, manifest: &AgentManifest) -> Result<()>;
    fn execute(&mut self, ctx: AgentContext) -> Result<AgentOutput>;
    fn capabilities(&self) -> &CapabilitySet;  // Same type for all runtimes
}

// Four implementations:
struct NativeRuntime;      // Rust — direct execution
struct PythonRuntime;      // RustPython — embedded interpreter
struct TypeScriptRuntime;  // QuickJS — embedded engine
struct WasmRuntime;        // wasmtime — AOT-compiled WASM
```

A Python agent with `[spaces.read, ai.complete]` capabilities can do exactly what a Rust agent
with the same capabilities can do — nothing more, nothing less. The runtime cannot grant
capabilities the manifest doesn't declare.
