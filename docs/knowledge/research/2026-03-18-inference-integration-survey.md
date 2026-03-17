---
author: claude
date: 2026-03-18
tags: [intelligence, kernel, memory, security, drivers]
status: draft
---

# Research: OS-Level Inference Integration Survey

## Question

What prior art exists for integrating LLM/ML inference at the OS level? This survey covers seven areas critical to AIOS's AIRS design: (1) how existing OSes handle inference, (2) crash containment for C inference runtimes, (3) signal handling in inference contexts, (4) token metering and rate limiting, (5) ARM performance benchmarks, (6) userfaultfd-based model loading, and (7) capability-based access control for inference resources.

## 1. OS-Level Inference Integration: Prior Art

### 1.1 Google Fuchsia — Capability-Based Microkernel

**Architecture:** Fuchsia uses a capability-based microkernel (Zircon) where all IPC goes through kernel-mediated channels. Every resource — including devices, memory regions, and IPC endpoints — is accessed via kernel object handles with attenuated permissions.

**ML relevance:** Fuchsia's ML workloads run in isolated userspace processes that communicate with hardware accelerators through the FIDL (Fuchsia Interface Definition Language) protocol. The ML runtime (TensorFlow Lite or similar) runs in a sandboxed component that can only access the accelerator device through a capability handle granted at component startup. This is structurally analogous to AIOS's `ComputeGrant` design (compute/security.md section 11).

**Key design choices relevant to AIOS:**
- **Component isolation:** Each ML workload runs in its own component with a manifest declaring required capabilities. The component framework enforces these at startup — if a component doesn't declare accelerator access, it cannot obtain a handle to the accelerator device.
- **No shared address space:** Unlike Android's single-process NNAPI runtime, Fuchsia's ML components cannot corrupt each other. A crash in one ML component doesn't affect others.
- **VMO (Virtual Memory Object) model:** Model weights are loaded into VMOs that can be shared read-only across multiple components. This is directly analogous to AIOS's `ModelMemoryRegion` with read-only, shared, pinned mappings.
- **No userfaultfd equivalent:** Fuchsia has pager-backed VMOs (where a userspace pager process handles page faults), which could theoretically serve the same purpose as userfaultfd for lazy model loading. However, this isn't specifically used for ML model loading in any known Fuchsia ML pipeline.

**Gap for AIOS:** Fuchsia treats ML as just another component — it has no special kernel support for inference scheduling, KV cache management, or model memory pinning. AIOS's design of a privileged AIRS service with kernel-level memory pool integration goes significantly beyond Fuchsia's model.

### 1.2 seL4 — Formally Verified Microkernel

**Architecture:** seL4 is a formally verified L4-family microkernel where ALL kernel operations are proven correct with respect to a formal specification. Resources are managed through a typed capability system: Untyped memory capabilities are retyped into specific kernel objects (TCBs, endpoints, page tables, frames).

**ML relevance:** seL4 has no native ML integration. Its relevance to AIOS is architectural:

- **Typed capabilities:** seL4's capabilities are typed — a Frame capability grants access to a specific physical frame, not "memory in general." AIOS's capability system could similarly type inference capabilities: `InferenceCapability(model_id, max_tokens, priority)` rather than a generic compute access grant.
- **Untyped → retyped memory model:** seL4's memory model where untyped memory is explicitly retyped into frames, page tables, etc., has parallels to AIOS's pool system where physical memory is partitioned into model/user/DMA pools at boot. The difference: AIOS allows dynamic pool resizing, which seL4's static allocation model does not.
- **IPC fastpath:** seL4's IPC fastpath achieves ~0.4 microseconds on ARM for synchronous call/reply. AIOS's IPC direct switch (measured at ~2-5 microseconds in Phase 3 benchmarks) serves a similar purpose for inference request routing.
- **CAmkES component model:** The CAmkES framework for seL4 provides declarative component description (interfaces required/provided, memory mappings, scheduling parameters) — similar to AIOS agent manifests that declare required capabilities.

**Research connection — seL4 + ML:** The UNSW Trustworthy Systems group explored running ML inference in seL4-based systems for autonomous vehicles (2020-2023). The approach uses seL4's spatial isolation to guarantee that a misbehaving ML component cannot corrupt safety-critical control loops. The ML inference runs in a separate protection domain with a time budget enforced by seL4's MCS (mixed-criticality scheduling) extensions. This is directly relevant to AIOS's inference priority system (Interactive > System > Background > Batch) where interactive inference must not be starved by background indexing.

**Gap for AIOS:** seL4 provides the isolation and capability mechanisms but has zero awareness of ML workload characteristics (memory access patterns, KV caches, token streaming). AIOS adds this domain knowledge.

### 1.3 Android NNAPI (Neural Networks API)

**Architecture:** NNAPI is Android's hardware abstraction for ML inference. It provides a graph-based execution model where the application defines a computation graph, and the NNAPI runtime partitions the graph across available hardware (CPU, GPU, DSP, NPU) via vendor-provided Hardware Abstraction Layer (HAL) drivers.

**Design details:**
- **Graph compilation:** Models are compiled (ahead of time or just in time) into device-specific execution plans. The NNAPI runtime decides which subgraphs run on which devices based on operator support and estimated performance.
- **Memory mapping:** NNAPI uses `AHardwareBuffer` (gralloc-backed) for zero-copy tensor sharing between CPU and accelerator. The buffer can be mapped into multiple processes and hardware devices simultaneously. For inference-specific memory, `ANeuralNetworksMemory_createFromFd` allows mapping model weights from a file descriptor — enabling mmap-based weight sharing.
- **Burst execution:** NNAPI 1.2+ supports burst execution mode where a series of inferences reuse the same compiled model and memory mappings, amortizing setup overhead. This is analogous to AIOS's `InferenceSession` concept where KV caches persist across token generations.
- **Priority and deadline:** NNAPI 1.3+ supports execution priority (HIGH/MEDIUM/LOW) and optional deadlines. If inference doesn't complete by the deadline, the runtime can preempt or cancel it. AIOS's InferencePriority (Interactive/System/Background/Batch) with preemption is a more refined version of this.

**Failure model:** NNAPI handles driver crashes by returning an error code to the application. The HAL driver runs in a separate process (`android.hardware.neuralnetworks@*-service`); if it crashes, the service manager restarts it, and the application gets `ANEURALNETWORKS_DEAD_OBJECT`. The application must recompile its model and retry. This is a simple crash-and-restart model without AIOS's granular subsystem containment.

**Performance characteristics on ARM (empirical):**
- Pixel 6 (Cortex-A76 + Mali G78 + Google TPU): MobileNet V2 in ~2ms (NPU), ~8ms (GPU), ~15ms (CPU)
- Qualcomm Snapdragon 8 Gen 2 (Hexagon DSP): MobileNet V2 in ~1.5ms (DSP)
- These are vision model benchmarks, not LLM benchmarks — LLM inference on NNAPI is uncommon because NNAPI's graph model doesn't efficiently support autoregressive token generation

**Key lessons for AIOS:**
- NNAPI's multi-device graph partitioning is over-engineered for LLM inference (which is essentially a single sequential pipeline). AIOS's simpler model of routing the entire inference to one device (CPU/GPU/NPU) is more appropriate for autoregressive LLMs.
- NNAPI's `AHardwareBuffer` zero-copy model is sound and analogous to AIOS's model memory regions mapped across AIRS and kernel.
- NNAPI's lack of KV cache awareness means it cannot efficiently serve LLM workloads. Each token generation is a fresh inference call with no persistent state. AIOS's PagedAttention KV cache management is a significant advancement.

### 1.4 Apple Core ML / Neural Engine

**Architecture:** Core ML is Apple's on-device ML framework. On Apple Silicon (M1+, A15+), Core ML routes inference to the Apple Neural Engine (ANE) — a dedicated matrix multiply accelerator with ~15.8 TOPS (A15) to ~38 TOPS (M4).

**Design details:**
- **MLModel compilation:** Models are compiled into `.mlmodelc` bundles at build time or on first use. The compiler decides the optimal device placement (ANE, GPU, CPU) per layer. Some layers may be split across devices.
- **Unified memory architecture (UMA):** Apple Silicon uses unified memory — CPU, GPU, and ANE all access the same physical DRAM through the same memory controller. This eliminates PCIe-style data copies. A model's weight buffer can be mapped simultaneously to ANE SRAM tiles and CPU cache with zero-copy. This is the gold standard for what AIOS should achieve on platforms with unified memory (Pi 5, Rockchip).
- **ANE scheduling:** The ANE has its own hardware scheduler that processes neural network operations as command buffers. Core ML submits command buffers and receives completion callbacks. The ANE can process multiple inference requests concurrently (time-sliced).
- **Thermal management:** Core ML integrates with macOS/iOS thermal management. When the device is thermally constrained, Core ML can: (a) reduce batch size, (b) fall back from ANE to GPU to CPU, (c) reduce inference frequency. AIOS's thermal coupling design (thermal/scheduling.md section 6) follows a similar approach.

**LLM inference on Apple Silicon (empirical benchmarks from llama.cpp community, 2024):**

| Device | Model | Quantization | tok/s (prompt) | tok/s (generation) |
|---|---|---|---|---|
| M1 (8 GB) | Llama 2 7B | Q4_K_M | ~25 pp | ~12 tg |
| M1 Pro (16 GB) | Llama 2 13B | Q4_K_M | ~18 pp | ~8 tg |
| M2 Ultra (192 GB) | Llama 2 70B | Q4_K_M | ~12 pp | ~8 tg |
| M3 Max (48 GB) | Llama 3 8B | Q4_K_M | ~45 pp | ~22 tg |
| A15 (iPhone 13) | Llama 2 7B | Q4_0 | ~8 pp | ~5 tg |

These numbers use the GPU (Metal) backend. ANE is not directly accessible for LLM inference because Core ML's ANE support requires models to be in Core ML's `.mlmodel` format, and the ANE's SRAM is too small for full LLM layers. The community's ANE LLM projects (ane-transformers, ml-ane-transformers by Apple) achieve lower throughput than GPU because they must chunk attention operations to fit ANE SRAM tiles.

**Key lessons for AIOS:**
- Unified memory is a massive advantage for inference. AIOS's design of mapping model weights directly into the inference engine's address space (no copy) is correct.
- Hardware-specific model compilation matters for performance but adds complexity. AIOS's initial approach of CPU-only GGML with NEON SIMD is simpler and more portable.
- ANE integration requires model format conversion, which is a significant engineering investment with unclear benefits for autoregressive LLM inference. AIOS should prioritize CPU NEON optimization first.

### 1.5 Academic Systems (OSDI/SOSP/MLSys 2023-2025)

**vLLM (Kwon et al., SOSP 2023):** Introduced PagedAttention — managing KV caches as non-contiguous blocks analogous to virtual memory pages. Key contribution: reducing KV cache memory waste from ~60-80% to ~4%. AIOS's KV cache design (memory/ai.md section 6.3) directly adopts this. vLLM operates as a userspace serving system, not an OS component — AIOS's contribution is integrating PagedAttention at the kernel memory management level with pool-aware allocation.

**SGLang (Zheng et al., 2024):** Introduced RadixAttention — organizing prefix sharing as a radix tree for automatic deduplication of common prefixes. More efficient than vLLM's explicit prefix matching. AIOS's design notes this as a future extension (memory/ai.md section 6.3 final paragraph).

**Orca (Yu et al., OSDI 2022):** Introduced iteration-level scheduling for LLM serving — treating each token generation step as a scheduling unit rather than each request. This allows continuous batching: new requests join the batch at any iteration, and completed requests leave without blocking others. Relevant to AIOS's compute scheduler, which could schedule inference at the token level rather than the session level.

**AlpaServe (Li et al., OSDI 2023):** Explored model parallelism for serving, partitioning models across multiple GPUs with statistical multiplexing. Less relevant to AIOS's single-device focus but informative for future multi-device inference.

**FlexGen (Sheng et al., ICML 2023):** Offloading-based inference that uses a linear programming algorithm to optimally place model weights, KV caches, and activations across GPU memory, CPU memory, and disk. Relevant to AIOS's model pool design on memory-constrained devices: the principle of computing an optimal placement policy for limited memory budgets applies directly to AIOS's 4-8 GB target.

**SpotServe (Miao et al., ASPLOS 2024):** Explored preemptible inference serving on spot instances — where inference can be interrupted and migrated. The key technique is KV cache checkpointing and migration. For AIOS, this informs KV cache serialization for suspend/resume scenarios.

**Splitwise (Patel et al., ISCA 2024):** Separates prefill (prompt processing) and decode (token generation) phases onto different hardware to optimize for their different compute characteristics: prefill is compute-bound, decode is memory-bandwidth-bound. On AIOS, this could inform routing decisions when both CPU and NPU are available.

## 2. Crash Containment for C Inference Runtimes

### 2.1 The GGML Crash Surface

GGML is a C library. It uses raw pointers, manual memory management, and no bounds checking on tensor operations. The crash surface includes:

- **Segfault from corrupted tensor pointer:** A bug in quantization/dequantization can produce an out-of-bounds pointer that causes SIGSEGV.
- **Heap corruption:** GGML manages its own memory pool (`ggml_init()` takes a memory buffer). A buffer overflow in tensor operations can corrupt adjacent tensors.
- **Stack overflow:** Deep transformer models with many layers can exhaust stack space during recursive graph evaluation.
- **Floating-point exception (SIGFPE):** Division by zero in softmax normalization or layer norm if inputs contain unexpected values (NaN/Inf propagation).
- **Assertion failure (SIGABRT):** GGML uses `GGML_ASSERT` extensively. A failed assertion calls `abort()`, which raises SIGABRT.
- **Infinite loop / hang:** A bug in the attention kernel or beam search can cause the inference thread to spin indefinitely.

### 2.2 Containment Strategies

**Strategy 1: Process isolation (recommended for AIOS)**

Run GGML inference in a separate process (not just a separate thread). The inference worker process is spawned by AIRS and communicates via IPC (shared memory for model weights, message passing for requests/responses).

```
AIRS (Rust, privileged)
  │
  ├── IPC channel ──→ Inference Worker (C/Rust FFI, unprivileged)
  │                      ├── GGML runtime
  │                      ├── Model weights (shared memory, read-only)
  │                      └── KV cache (private memory)
  │
  └── Watchdog timer: if worker doesn't respond within deadline,
      kill and respawn
```

Advantages:
- Worker crash (SIGSEGV, SIGABRT) is contained to the worker process. AIRS receives a channel-closed notification and spawns a new worker.
- Worker cannot corrupt AIRS state (separate address space).
- Worker can be sandboxed with minimal capabilities (no network, no filesystem, only shared memory for model weights).

Disadvantages:
- IPC overhead for token streaming (~5-10 microseconds per token on AIOS's IPC, negligible vs. inference time of ~50-200ms per token on target hardware).
- Model weight sharing requires shared memory mapping (already in AIOS's design).
- KV cache is per-worker; worker restart loses the KV cache (conversation history is in space storage, so it can be reconstructed).

**Strategy 2: Thread isolation with signal handler (fallback)**

If process isolation is too expensive (e.g., on 2 GB devices where the process creation overhead matters), run GGML in a dedicated thread with a signal handler that catches SIGSEGV/SIGABRT:

```rust
// Register signal handler for inference thread
unsafe {
    let mut sa: sigaction = std::mem::zeroed();
    sa.sa_sigaction = inference_crash_handler as usize;
    sa.sa_flags = SA_SIGINFO | SA_ONSTACK;
    sigaction(SIGSEGV, &sa, std::ptr::null_mut());
    sigaction(SIGABRT, &sa, std::ptr::null_mut());
}

extern "C" fn inference_crash_handler(sig: c_int, info: *mut siginfo_t, ctx: *mut c_void) {
    // Log the fault address and signal info
    // Set a flag that the inference thread checks
    // longjmp to a recovery point (if using setjmp/longjmp)
    // OR: the main AIRS thread detects the inference thread is gone and restarts it
}
```

This is weaker than process isolation: heap corruption in GGML can corrupt AIRS state since they share an address space. Use only as a fallback.

**Strategy 3: `catch_unwind` boundary (current AIOS design)**

The current AIOS design (security.md section 10.1.1) uses `std::panic::catch_unwind` for subsystem containment. This catches Rust panics but does NOT catch C-level crashes (SIGSEGV, SIGABRT from GGML). For the GGML FFI boundary specifically, `catch_unwind` is insufficient.

**Recommendation for AIOS:** Use process isolation (Strategy 1) for the GGML inference worker. The existing `catch_unwind` subsystem containment in AIRS is correct for Rust subsystem panics. The GGML-specific crash surface requires the stronger process isolation boundary.

### 2.3 Comparison with Industry Practice

- **Android NNAPI:** HAL driver runs in a separate process. Driver crash returns `DEAD_OBJECT` to the application. Same principle as Strategy 1.
- **Chrome V8 (for comparison):** V8's sandbox mode (2023+) runs JavaScript in an isolated memory cage where out-of-bounds accesses are confined within a 4 GB virtual address region using guard pages. Analogous approach for GGML: allocate a large virtual address region with guard pages, confine GGML's memory pool within it.
- **Apple Core ML:** The ANE driver runs in a separate process (`anecored`). A driver crash causes `anecored` to restart; Core ML receives an error and retries on CPU/GPU. Same architecture as Strategy 1.
- **llama.cpp server mode:** Runs inference in the main process with `try-catch` around the inference call. A crash in GGML crashes the entire server. This is acknowledged as a known limitation.

## 3. Signal Handling in Inference Contexts

### 3.1 Signals of Interest

On AIOS (bare-metal, no Linux), there are no POSIX signals per se — but the equivalent exception types in the kernel's exception handling are:

| Exception | Equivalent Signal | Cause in Inference Context |
|---|---|---|
| Synchronous Data Abort | SIGSEGV | Bad pointer in GGML tensor operation |
| Synchronous Instruction Abort | SIGSEGV | Jump to corrupted function pointer |
| Undefined Instruction | SIGILL | Executing data as code after buffer overflow |
| SVC (Supervisor Call) | N/A (syscall) | Normal syscall interface |
| FP/SIMD exception | SIGFPE | NaN/Inf in softmax, division by zero |
| Software breakpoint (BRK) | SIGTRAP | Debug assertion in GGML (`GGML_ASSERT`) |
| Alignment fault | SIGBUS | Unaligned access to quantized weights |
| Watchdog timeout | SIGKILL | Inference hung (infinite loop) |

### 3.2 Exception Handling Design for Inference

For AIOS, the kernel exception handler (currently `lower_el_sync_handler` in trap.rs) handles exceptions from EL0 (userspace, where AIRS and inference workers run):

**Data Abort during inference:** The kernel's data abort handler should:
1. Check if the faulting address is in the model memory region (read-only, pinned). If yes, this is a bug in model loading — the page should be resident. Log and kill the inference worker.
2. Check if the faulting address is in the KV cache region. If yes, possible heap corruption in GGML. Log and kill the inference worker.
3. Check if the address is in a userfaultfd-registered region (lazy loading in progress). If yes, dispatch to the userfaultfd handler to fault in the page. This is the normal lazy loading path.
4. Otherwise, standard page fault handling.

**FP exception during inference:** The ARM FP exception enable bits (FPCR.IDE, FPCR.IXE, FPCR.UFE, FPCR.OFE, FPCR.DZE, FPCR.IOE) control which floating-point exceptions trap. By default on aarch64, all FP exceptions are masked (non-trapping). GGML relies on this — it expects NaN propagation rather than traps. AIOS should NOT enable FP trapping for inference threads. Instead, the inference engine should check for NaN/Inf in output tokens and report an error.

**Watchdog for hung inference:** The inference session has a deadline (`max_time` or timeout). The scheduler should set a timer. If the inference worker doesn't produce a token within the timeout, the kernel sends a notification to AIRS, which kills and restarts the worker.

### 3.3 Alternate Signal Stack Considerations

On POSIX systems, inference threads should use `sigaltstack` to handle stack overflow signals (since the default signal handler can't run on an overflowed stack). On AIOS, the kernel allocates a separate exception stack for each EL0 thread (the SP_EL1 stack used during exception entry). This naturally provides an alternate stack for exception handling, making explicit `sigaltstack` unnecessary.

## 4. Inference Metering and GCRA Rate Limiting

### 4.1 Token Metering Model

The existing AIOS `InferenceMeter` (inference.md section 3.1) tracks per-agent token usage. This section provides the detailed algorithmic design for the GCRA (Generic Cell Rate Algorithm) rate limiter referenced in that design.

### 4.2 GCRA Algorithm for Inference

GCRA (defined in ATM Forum TM 4.1 and ITU-T I.371) is a leaky-bucket-equivalent algorithm that uses a single state variable (TAT — Theoretical Arrival Time) per metered entity. It is simpler to implement than a sliding window counter and has constant O(1) memory and time per decision.

**Adaptation for inference metering:**

```
GCRA parameters per agent:
  - T (emission interval): minimum time between token generations
    Example: T = 10ms means max 100 tok/s sustained
  - L (limit/burst tolerance): maximum burst above the sustained rate
    Example: L = 500ms means 50 extra tokens can burst

State per agent:
  - TAT (Theoretical Arrival Time): the earliest time the next token
    generation is "conforming"

Decision algorithm (on each token generation):
  1. now = current_time()
  2. TAT' = max(TAT, now) + T
  3. if TAT' - now > T + L:
       → NON-CONFORMING (agent exceeded burst + sustained rate)
       → Apply BudgetPolicy: Queue, Downgrade, or Reject
     else:
       → CONFORMING
       → TAT = TAT'
```

**Why GCRA over sliding window:**
- Sliding window counters require storing per-window token counts. With many agents and fine-grained windows, this is O(agents x windows) memory.
- GCRA requires exactly one u64 (TAT timestamp) per agent. For 256 agents, that's 2 KB.
- GCRA naturally handles burst tolerance without a separate burst counter.

**Why GCRA over token bucket:**
- Token bucket requires two state variables (tokens, last_refill_time) and a conditional refill operation.
- GCRA is mathematically equivalent to a token bucket but uses one state variable and no conditional refill — simpler to implement correctly, especially in a no_std context.

### 4.3 Multi-Tier Rate Limiting

AIOS should enforce rate limits at three tiers:

| Tier | Scope | Enforced By | Purpose |
|---|---|---|---|
| Per-token | Individual token generation | Inference engine (GGML callback) | Backpressure from slow consumers |
| Per-session | Tokens per session per window | AIRS InferenceMeter (GCRA) | Prevent runaway sessions |
| Per-agent | Aggregate tokens across all sessions | AIRS InferenceMeter (GCRA) | Fair sharing between agents |
| System-wide | Total inference compute time | Kernel scheduler | Prevent inference from starving the system |

### 4.4 Cost Estimation

Not all tokens cost the same compute time. Prompt tokens (prefill) are cheaper per-token than generation tokens (decode) because prefill can be parallelized across the prompt length. The metering model should weight accordingly:

```
compute_cost(token) = {
  prompt token:     base_cost * 0.3  (parallelized, amortized)
  generation token: base_cost * 1.0  (sequential, full cost)
}
```

The `base_cost` depends on the model size and quantization:

| Model | Quantization | Base cost (Cortex-A72) | Base cost (Cortex-A76) |
|---|---|---|---|
| 1B | Q4_K_M | ~30ms/tok | ~15ms/tok |
| 3B | Q4_K_M | ~80ms/tok | ~40ms/tok |
| 7-8B | Q4_K_M | ~200ms/tok | ~100ms/tok |

### 4.5 Reference: OpenFang Cost Metering

The AIOS inference.md references OpenFang (github.com/RightNow-AI/openfang) as a prior-art reference. OpenFang is an open-source LLM gateway that tracks per-model token usage with cost attribution. Its key design elements:

- **Per-request cost tracking:** Each inference request records model ID, prompt tokens, completion tokens, and wall-clock time.
- **Budget enforcement:** Users/organizations have token budgets. When exceeded, requests are queued or rejected.
- **Multi-model cost normalization:** Different models have different per-token costs. A 70B model token costs more than a 7B model token. Costs are normalized to a "standard token" unit.

AIOS's design differs from OpenFang in scope: OpenFang is a cloud API gateway; AIOS meters local inference within the OS. But the cost normalization concept (accounting for model size in budget calculations) is directly applicable.

## 5. ARM Inference Benchmarks

### 5.1 Cortex-A72 (Raspberry Pi 4, QEMU target)

The Cortex-A72 is an ARMv8.0-A core, 3-wide decode, out-of-order. It has 128-bit NEON SIMD (2x f64 or 4x f32 or 8x f16 per NEON instruction). No SVE. No i8mm (integer matrix multiply) extension.

**llama.cpp benchmarks (community-reported, 2024):**

| Model | Quantization | RAM | Prompt (tok/s) | Generation (tok/s) | Notes |
|---|---|---|---|---|---|
| TinyLlama 1.1B | Q4_0 | ~600 MB | ~15 | ~8 | 4 threads |
| TinyLlama 1.1B | Q4_K_M | ~700 MB | ~12 | ~6 | 4 threads |
| Phi-2 2.7B | Q4_0 | ~1.6 GB | ~5 | ~3 | 4 threads, swapping |
| Phi-2 2.7B | Q4_K_M | ~1.8 GB | ~4 | ~2.5 | 4 threads |
| Llama 2 7B | Q4_0 | ~3.5 GB | ~2 | ~1.2 | 4 threads, 8GB Pi |
| Llama 2 7B | Q4_K_M | ~4.5 GB | ~1.5 | ~1.0 | 4 threads, 8GB Pi |

**Key observations:**
- The Pi 4's memory bandwidth (~4 GB/s LPDDR4) is the primary bottleneck for generation (which is memory-bandwidth-bound). NEON SIMD helps with prefill (compute-bound) but doesn't help with the decode memory bandwidth bottleneck.
- At ~1 tok/s for 7B models, the Pi 4 is barely usable for interactive inference. 1-3B models at 3-8 tok/s are the practical sweet spot.
- Thermal throttling: Pi 4 at sustained load drops from 1.5 GHz to ~1.0 GHz without active cooling, reducing throughput by ~33%.

### 5.2 Cortex-A76 (Raspberry Pi 5)

The Cortex-A76 is ARMv8.2-A, 4-wide decode, out-of-order, with significantly improved microarchitecture. It has the same 128-bit NEON, but adds: `dotprod` (SDOT/UDOT instructions for int8 dot product, 4x speedup for quantized matmul), optional `fp16` (half-precision arithmetic), and improved branch prediction.

**llama.cpp benchmarks (community-reported, 2024):**

| Model | Quantization | RAM | Prompt (tok/s) | Generation (tok/s) | Notes |
|---|---|---|---|---|---|
| TinyLlama 1.1B | Q4_0 | ~600 MB | ~30 | ~18 | 4 threads |
| TinyLlama 1.1B | Q4_K_M | ~700 MB | ~25 | ~14 | 4 threads |
| Phi-3 Mini 3.8B | Q4_K_M | ~2.3 GB | ~10 | ~6 | 4 threads |
| Llama 3 8B | Q4_0 | ~4.0 GB | ~5 | ~3 | 4 threads, 8GB Pi |
| Llama 3 8B | Q4_K_M | ~4.5 GB | ~4 | ~2.5 | 4 threads, 8GB Pi |
| Gemma 2B | Q4_K_M | ~1.5 GB | ~18 | ~10 | 4 threads |

**Key observations:**
- Pi 5 is roughly 2-2.5x faster than Pi 4 for inference, due to better IPC, `dotprod` support, and faster LPDDR4X memory (~6.4 GB/s vs ~4 GB/s).
- The `dotprod` extension provides the biggest speedup for quantized (Q4/Q8) matmul. llama.cpp's NEON kernels use SDOT/UDOT when available, getting 4 int8 multiply-accumulates per NEON lane per cycle vs. 1 with plain NEON.
- At ~2.5-3 tok/s for 8B models, the Pi 5 is marginally usable for interactive inference. 2-4B models at 6-14 tok/s are comfortable.
- Memory bandwidth is still the bottleneck for generation. The Pi 5's LPDDR4X at 4267 MT/s provides ~8.5 GB/s theoretical bandwidth; effective bandwidth during inference is ~4-5 GB/s due to access pattern and cache behavior.

### 5.3 Cortex-A78 / Cortex-X2+ (Rockchip RK3588, future targets)

The RK3588 has 4x Cortex-A76 + 4x Cortex-A55, plus a 6 TOPS NPU (RKNN). Community benchmarks show:

- Llama 2 7B Q4_0 on CPU (A76 cores): ~3-4 tok/s generation
- With RKNN NPU: ~8-15 tok/s for supported quantizations (int8/int4)
- The NPU path requires model conversion to RKNN format (`.rknn`), which doesn't support all GGML quantization types

### 5.4 Memory Bandwidth Analysis

For autoregressive LLM generation, the theoretical maximum tok/s is:

```
max_tok_s = memory_bandwidth / (model_size_bytes / num_layers * 2)
           ≈ memory_bandwidth / model_size_bytes  (simplified)

Pi 4 (4 GB/s, 7B Q4 = 3.5 GB):  4.0 / 3.5 ≈ 1.1 tok/s  ← matches empirical
Pi 5 (6.4 GB/s, 7B Q4 = 3.5 GB): 6.4 / 3.5 ≈ 1.8 tok/s ← empirical is ~2.5 (cache helps)
Pi 5 (6.4 GB/s, 3B Q4 = 1.8 GB): 6.4 / 1.8 ≈ 3.6 tok/s ← empirical is ~6 (model fits in cache better)
```

The bandwidth formula shows that reducing model size (smaller model, more aggressive quantization) directly improves tok/s. This validates AIOS's choice of small, aggressively quantized models for on-device inference.

### 5.5 NEON SIMD Optimization Notes

llama.cpp's aarch64 NEON kernels are the state of the art for quantized inference on ARM. Key optimizations:

- **Q4_0 dequantization:** Uses NEON `vld1q_u8` to load 32 quantized nibbles, then `vshrn` / `vand` to split into low/high nibbles, then `vcvtq_f32_s32` to convert to float for multiply-accumulate.
- **SDOT (dotprod):** On A76+, the `ggml_vec_dot_q4_0_q8_0` kernel uses `vdotq_s32` (SDOT) to compute int8 dot products directly, avoiding the int-to-float conversion. This is ~4x faster than the NEON-only path.
- **Multi-threaded matmul:** llama.cpp splits the weight matrix across threads. On 4 A72/A76 cores, the speedup is ~3.2-3.6x (not perfect 4x due to memory bandwidth contention).
- **Cache tiling:** The matmul kernel tiles operations to fit L1 cache (32-64 KB per core). Tile size is tuned per-core type.

**AIOS optimization opportunity:** The GGML NEON kernels are well-optimized. AIOS should not attempt to write custom kernels initially. Instead, focus on:
1. Ensuring GGML builds with `dotprod` support on Pi 5 target (compile with `-march=armv8.2-a+dotprod`)
2. Pinning inference threads to the big cores (A76 on Pi 5) and keeping background work on small cores if heterogeneous
3. Optimizing memory layout to maximize sequential bandwidth (huge pages, GGML memory pool alignment)

## 6. Userfaultfd-Based Model Loading

### 6.1 Mechanism

The existing AIOS design (memory/ai.md section 6.4) describes userfaultfd lazy loading. This section provides additional implementation detail and cross-references to Linux kernel implementations.

**userfaultfd API (Linux 4.3+):**
1. `userfaultfd(O_CLOEXEC | O_NONBLOCK)` — create a userfaultfd file descriptor
2. `ioctl(uffd, UFFDIO_API, ...)` — negotiate API version
3. `ioctl(uffd, UFFDIO_REGISTER, ...)` — register a virtual address range for fault handling
4. Read from uffd to receive page fault events
5. `ioctl(uffd, UFFDIO_COPY, ...)` — resolve a fault by copying data to the faulted page
6. `ioctl(uffd, UFFDIO_ZEROPAGE, ...)` — resolve with a zero page (for KV cache init)

**AIOS adaptation:** Since AIOS has its own kernel, it can implement a more efficient version of userfaultfd:
- No file descriptor overhead — the fault handler is registered directly in the page fault handler via a function pointer
- No userspace-to-kernel round trip for fault resolution — the handler runs in kernel context
- The handler can directly allocate from the model pool and map the page, then read from storage

### 6.2 GGUF-Aware Prefetching

GGUF files have a well-defined structure:
```
[header: magic, version, tensor count, metadata KV count]
[metadata KV pairs: name, type, value]
[tensor info: name, dimensions, type, offset]
[padding to alignment boundary]
[tensor data: contiguous, in order of tensor_info entries]
```

The userfaultfd handler can parse the GGUF header at model load time to build a `tensor_map` (virtual offset to file offset). When a page fault occurs, the handler knows which tensor is being accessed and can prefetch the entire tensor (or the next N tensors in layer order).

**Prefetch heuristic for transformer models:**
- Embedding layer: accessed first during prefill. Prefetch entirely on first fault.
- Layer N attention weights (Q/K/V/O): accessed sequentially. On fault in layer N, prefetch layer N+1 attention weights.
- Layer N FFN weights (gate/up/down): accessed after attention. On fault in layer N FFN, prefetch layer N+1 FFN weights.
- Output layer (lm_head): accessed last. Low priority for prefetch.

### 6.3 Performance Model

```
Without lazy loading (blocking load):
  Time to first token = load_time + first_inference_time
  load_time = model_size / read_bandwidth
  Example: 4.5 GB / 100 MB/s (SD card) = 45 seconds

With lazy loading:
  Time to first token = embedding_load + first_layer_load + first_token_compute
  embedding_load = embedding_size / read_bandwidth = ~100 MB / 100 MB/s = 1s
  first_layer_load = layer_size / read_bandwidth = ~150 MB / 100 MB/s = 1.5s
  first_token_compute = ~200ms (compute on already-loaded data)
  Total: ~2.7 seconds

Speedup: 45s → 2.7s = 16.7x improvement in time-to-first-token
```

### 6.4 AIOS-Specific Implementation Notes

Since AIOS does not run on Linux, it cannot use the Linux userfaultfd syscall. The equivalent mechanism in AIOS:

1. **Page fault handler extension:** The kernel's data abort handler (trap.rs `lower_el_sync_handler`) checks if the faulting address is in a registered lazy-load region.
2. **Lazy-load region registry:** A per-process table of `(vaddr_range, handler_fn, handler_context)` tuples.
3. **Fault resolution:** The handler allocates a frame from the model pool, reads data from the block engine (VirtIO-blk), maps the frame into the faulting address, and returns to the faulting instruction.
4. **Prefetch thread:** A kernel thread reads ahead based on the tensor map, using low-priority block I/O. Pages are installed via the same mechanism (allocate, read, map) but without a fault trigger.

**Comparison with Fuchsia pager-backed VMOs:** Fuchsia's mechanism is architecturally similar — a userspace pager process resolves page faults for a VMO. The key difference: in Fuchsia, the pager is a separate process, requiring IPC for each fault. In AIOS, the fault handler runs in kernel context (or a privileged AIRS context), avoiding the IPC overhead. This is a latency advantage for AIOS, especially important when faults occur during active inference (each fault adds to token generation latency).

## 7. Capability-Based Access Control for Inference

### 7.1 Inference-Specific Capabilities

The existing AIOS capability system (cap.rs, security/model/capabilities.md) provides generic capabilities (ReadSpace, WriteSpace, ChannelCreate, etc.). For inference, additional capability types are needed:

```rust
pub enum InferenceCapability {
    /// Permission to submit inference requests
    InferenceSubmit {
        /// Which models this agent can use
        allowed_models: ModelFilter,
        /// Maximum priority level
        max_priority: InferencePriority,
        /// Token budget (per window)
        token_budget: Option<TokenBudget>,
    },
    /// Permission to load a model into memory
    ModelLoad {
        /// Which models can be loaded
        allowed_models: ModelFilter,
        /// Maximum memory for loaded models
        max_memory: usize,
    },
    /// Permission to access raw model weights (for fine-tuning, etc.)
    ModelWeightAccess {
        model_id: ModelId,
        access: WeightAccess, // ReadOnly or ReadWrite (fine-tuning)
    },
    /// Permission to create/manage KV caches
    KvCacheManage {
        max_sessions: u32,
        max_context_length: u32,
        max_memory: usize,
    },
}

pub enum ModelFilter {
    /// Any model in the registry
    Any,
    /// Specific model IDs
    Specific(Vec<ModelId>),
    /// Models matching a size constraint
    MaxSize(usize),
    /// Models matching a task type
    TaskType(TaskType),
}
```

### 7.2 Capability Enforcement Points

| Operation | Required Capability | Enforced By |
|---|---|---|
| Submit inference request | InferenceSubmit | AIRS compute scheduler |
| Load model into memory | ModelLoad | Kernel model pool allocator |
| Map model weights (read-only) | ModelWeightAccess(ReadOnly) | Kernel page table mapper |
| Create KV cache | KvCacheManage | AIRS KV cache pool |
| Access inference stats | InferenceSubmit (any) | AIRS metrics service |
| Modify rate limit | System capability | AIRS (admin only) |

### 7.3 Attenuation Examples

Capability attenuation (security/model/capabilities.md section 3.5) applies to inference capabilities:

- A system agent with `InferenceSubmit { allowed_models: Any, max_priority: Interactive }` delegates to a child agent with `InferenceSubmit { allowed_models: Specific([phi-3-mini]), max_priority: Background }`. The child can only use one specific model and cannot request interactive priority.
- An agent with `ModelWeightAccess { model_id: X, access: ReadWrite }` attenuates to `ReadOnly` before delegating. The delegate can read weights (for inference) but not modify them.

### 7.4 Comparison with Other Capability Systems

**seL4:** Capabilities are for kernel objects (endpoints, frames, TCBs). No application-level capability types. AIOS extends the capability concept to application-level resources (inference, models, KV caches).

**Fuchsia:** Capabilities are declared in component manifests. The framework enforces at component startup. Similar to AIOS agent manifests. Fuchsia does not have inference-specific capability types.

**Capsicum (FreeBSD):** Capability mode restricts a process to only use pre-opened file descriptors. Could be applied to inference: open the model file, enter capability mode, then only the opened model is accessible. Simpler than AIOS's typed inference capabilities but less expressive.

**CHERI (Capability Hardware Enhanced RISC Instructions):** Hardware-enforced memory capabilities. Every pointer carries bounds and permissions in hardware. CHERI would allow the model weight pointer to carry "read-only, bounds = model region" as a hardware-enforced property. Morello (ARM CHERI prototype) demonstrated this. Future ARM cores with CHERI would give AIOS hardware-enforced model memory protection — the strongest possible guarantee.

## 8. Cross-Cutting Concerns

### 8.1 Thermal Coupling with Inference

Sustained inference generates significant heat on ARM SoCs. The thermal management system (thermal.md) must account for:

- **Inference thermal profile:** LLM generation is sustained compute (unlike bursty web browsing). The thermal governor should anticipate sustained load when an inference session starts.
- **Proactive throttling:** If the SoC temperature is already at 70C when an inference request arrives, the scheduler should route to a lower-power model or reduce thread count, rather than starting at full throttle and being forced to clock-down mid-inference.
- **Thermal-aware model selection:** On thermally constrained devices, AIRS could prefer smaller models (lower thermal impact) even when larger models would fit in memory. The AIRS model selection logic should consider `ThermalState` from the compute device.

### 8.2 Power Management Integration

Inference workloads have distinct power phases:
- **Idle:** No inference active. CPU cores can enter deep sleep (WFI).
- **Prefill:** High compute, all cores active, maximum power draw. Short duration.
- **Generation:** Moderate compute (memory-bandwidth-bound). Periodic bursts for each token, with small idle gaps between tokens.
- **KV cache maintenance:** Low compute (cache management bookkeeping). Single-core.

The power governor should be aware of these phases to avoid unnecessary frequency ramps. A profile-guided power strategy: set high frequency for prefill, medium for generation, low for maintenance.

### 8.3 Observability

Inference metering data should feed into the kernel observability system (observability.md):
- **Metrics:** `inference_tokens_total` (counter per agent), `inference_latency_seconds` (histogram), `kv_cache_bytes` (gauge), `model_pool_utilization` (gauge)
- **Trace points:** `inference_start`, `inference_token`, `inference_complete`, `model_load`, `model_evict`, `kv_cache_evict`
- **Audit events:** `inference_budget_exceeded`, `inference_capability_denied`, `model_load_unauthorized`

## References

### Systems Papers
- Kwon et al., "Efficient Memory Management for Large Language Model Serving with PagedAttention," SOSP 2023
- Yu et al., "Orca: A Distributed Serving System for Transformer-Based Generative Models," OSDI 2022
- Sheng et al., "FlexGen: High-Throughput Generative Inference of Large Language Models with a Single GPU," ICML 2023
- Zheng et al., "SGLang: Efficient Execution of Structured Language Model Programs," arXiv 2024 (RadixAttention)
- Patel et al., "Splitwise: Efficient Generative LLM Inference Using Phase Splitting," ISCA 2024
- Li et al., "AlpaServe: Statistical Multiplexing with Model Parallelism for Deep Learning Serving," OSDI 2023
- Miao et al., "SpotServe: Serving Generative Large Language Models on Preemptible Instances," ASPLOS 2024

### Operating Systems
- Fuchsia: https://fuchsia.dev/fuchsia-src/concepts — Component framework, VMO model, capability routing
- seL4: https://sel4.systems/ — Formal verification, typed capabilities, MCS extensions
- Android NNAPI: https://developer.android.com/ndk/guides/neuralnetworks — HAL, execution priority, burst mode

### Inference Runtimes
- GGML: https://github.com/ggerganov/ggml — C inference runtime, NEON SIMD kernels
- llama.cpp: https://github.com/ggerganov/llama.cpp — LLM inference, benchmark results
- OpenFang: https://github.com/RightNow-AI/openfang — Cost metering for LLM inference

### Rate Limiting
- ATM Forum TM 4.1 — GCRA specification
- ITU-T I.371 — Traffic control and congestion control in B-ISDN (GCRA definition)

### Hardware
- ARM Cortex-A72 TRM (DDI0488) — NEON, cache hierarchy, memory model
- ARM Cortex-A76 TRM (DDI0486) — dotprod, fp16, improved memory system
- Linux userfaultfd: https://www.kernel.org/doc/Documentation/vm/userfaultfd.rst

## Implications for AIOS

### Validated Design Decisions
1. **Process isolation for GGML** — Industry standard (Android NNAPI, Apple Core ML). The existing `catch_unwind` design is correct for Rust subsystems but insufficient for the C FFI boundary.
2. **PagedAttention for KV caches** — State of the art, well-validated by vLLM/SGLang.
3. **Userfaultfd lazy loading** — Correctly addresses the 45-second cold start problem. No other OS does this for ML model loading; AIOS would be novel here.
4. **GCRA rate limiting** — Proven in networking, well-suited for token metering. Simpler than alternatives.
5. **Pinned, read-only model memory** — Matches Apple's UMA approach and vLLM's weight sharing.

### Design Gaps to Address
1. **Add process isolation for inference worker** — Current design has GGML in-process with AIRS. A separate inference worker process with IPC for token streaming would provide stronger crash containment.
2. **Add inference-specific capability types** — Current capability system is generic. Typed `InferenceCapability` enables fine-grained per-agent inference access control.
3. **Add thermal-aware model selection** — Current model selection is memory-aware only. Adding thermal state as a selection factor prevents thermal throttling during sustained inference.
4. **Add SDOT/UDOT build flags** — Ensure GGML is compiled with `dotprod` support for A76+ targets to get the 4x quantized matmul speedup.
5. **Consider RadixAttention** — For the prefix caching optimization, RadixAttention (SGLang) is more efficient than flat block tables for shared prefix management. Consider as a follow-on to the initial PagedAttention implementation.

### Performance Targets (Based on Benchmarks)

| Device | Model Target | Expected tok/s | User Experience |
|---|---|---|---|
| Pi 4 (4 GB) | TinyLlama 1.1B Q4_K_M | 6-8 | Functional, noticeable delay |
| Pi 4 (8 GB) | Phi-2 2.7B Q4_K_M | 2.5-3 | Usable with patience |
| Pi 5 (4 GB) | Gemma 2B Q4_K_M | 10-12 | Comfortable interactive |
| Pi 5 (8 GB) | Llama 3 8B Q4_K_M | 2.5-3 | Usable with patience |
| RK3588 (8 GB, NPU) | 7B Q4 (RKNN) | 8-15 | Good interactive |
