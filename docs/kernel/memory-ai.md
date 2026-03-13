# AIOS Memory Management — Model Memory (AIRS)

**Part of:** [memory.md](./memory.md) — Memory Management Hub
**Related:** [memory-physical.md](./memory-physical.md) — Physical memory and pools, [memory-reclamation.md](./memory-reclamation.md) — MGLRU and DAMON, [airs.md](../intelligence/airs.md) — AI Runtime

-----

## 6. Model Memory (AIRS)

### 6.1 The Problem

On target hardware, AI model memory dominates everything else:

```text
Memory budget on a 4 GB Raspberry Pi 5:

Total RAM:                              4096 MB
  - Kernel:                              256 MB
  - Reserved (firmware tables, MMIO):    128 MB
  - DMA pool:                            128 MB
  - User pool (OS services, agents,     1536 MB
    heap, browser, headroom):
  ─────────────────────────────────────────────
  Available for model:              2048 MB (2 GB)

Llama 3.1 8B at Q4_K_M:               ~4500 MB  ← does not fit
Llama 3.1 8B at Q3_K_S:               ~3200 MB  ← does not fit
Phi-3 Mini 3.8B at Q4_K_M:            ~2300 MB  ← does not fit
Phi-3 Mini 3.8B at Q4_K_M + KV cache: ~2700 MB  ← does not fit
TinyLlama 1.1B at Q4_K_M:             ~700 MB   ← fits
Phi-2 2.7B at Q4_K_M:                 ~1800 MB  ← fits

On a 2 GB device (model pool is 0 — see [§2.4](./memory-physical.md)):
  Available for model:                  0 MB (cloud inference only)
  All 1.75 GB (after kernel/DMA/reserved) is user pool
```

The model IS the memory problem. Traditional OS memory management — where everything is fungible and swappable — does not work here. Model weights must stay in RAM. Swapping 3 GB of model data to an SD card would take tens of seconds and make inference unusable.

### 6.2 Model Memory Region

Model weights are loaded into the model pool — a dedicated region of physical memory that is pinned (never paged out), uses 2 MB huge pages (to reduce TLB pressure), and is mapped read-only into the AIRS process.

```rust
/// A loaded model's memory region
pub struct ModelMemoryRegion {
    /// Physical frames backing this model (2 MB huge pages)
    frames: Vec<PhysicalFrame>,
    /// Total size in bytes
    size: usize,
    /// Reference count (multiple sessions can share weights)
    refcount: AtomicUsize,
    /// Model identity
    model_id: ModelId,
    /// Virtual address mapped in AIRS process
    vaddr: VirtualAddress,
}

/// Mapping configuration for model memory
pub struct ModelMapping {
    /// Physical base
    phys_base: PhysicalAddress,
    /// Virtual base in AIRS address space
    virt_base: VirtualAddress,
    /// Size (2 MB aligned)
    size: usize,
    /// Flags: read-only, shared, pinned, huge pages
    flags: VmFlags,
}

impl ModelMapping {
    pub fn new(region: &ModelMemoryRegion) -> Self {
        Self {
            phys_base: region.frames[0].address(),
            virt_base: region.vaddr,
            size: region.size,
            flags: VmFlags::READ | VmFlags::SHARED | VmFlags::PINNED | VmFlags::HUGE,
        }
    }
}
```

**Why huge pages for models:** A 4 GB model mapped with 4 KB pages requires 1,048,576 page table entries and the same number of TLB entries. The TLB on a Cortex-A76 (Pi 5) has ~1280 entries — hopeless. With 2 MB huge pages, the same model needs only 2048 TLB entries. Still more than the TLB can hold, but the miss rate is dramatically lower because each entry covers 512x more memory.

**Why pinned:** Model weights are read-only after loading. They are never written, so they are never dirty, so there is nothing to write back to disk. Evicting them from RAM saves nothing — they would just need to be reloaded from storage. Pinning prevents the page reclamation system from touching model memory.

**Reference counting:** When multiple inference sessions (conversation bar, space indexer, intent verifier) all use the same model, they share the same physical memory region. The refcount tracks how many sessions hold a reference. The model is evicted only when the refcount drops to zero AND memory pressure requires it.

#### 6.2.1 Model Page Pinning and Inference Safety

Model pages are pinned for their **entire resident lifetime**, not just during active inference:

```rust
impl ModelMemoryRegion {
    /// Model frames are pinned from the moment they are loaded
    /// until the model is explicitly evicted. They are NEVER on
    /// the free list. Page reclamation cannot touch them.
    ///
    /// Pinning invariants:
    /// 1. All frames have VmFlags::PINNED set at allocation time
    /// 2. Pinned frames are excluded from the page reclaimer's scan
    /// 3. The model pool pressure calculation ignores pinned pages
    ///    (model pool pressure = 0 when all pages are model weights)
    /// 4. Dynamic pool resizing ([§12.2](./memory-reclamation.md)) cannot reclaim pinned frames
    ///    — it only moves the pool boundary using FREE pages
    /// 5. Eviction requires refcount == 0 (no active sessions)
    ///    AND an explicit eviction decision by AIRS or the kernel
    ///
    /// During active inference, model pages are accessed read-only.
    /// Since they are pinned, mapped read-only, and shared:
    /// - No page fault can occur (pages are always resident)
    /// - No eviction can occur (pinned frames are never reclaimed)
    /// - No corruption can occur (read-only mapping, W^X enforced)
    /// - No concurrent modification is possible (no writer exists)
    pub fn is_safe_for_inference(&self) -> bool {
        self.frames.iter().all(|f| f.flags().contains(VmFlags::PINNED))
            && self.refcount.load(Ordering::Acquire) > 0
    }
}
```

**Inference-critical invariant:** An LLM inference session that begins with a loaded model will never observe model page eviction, corruption, or stale data, regardless of concurrent memory pressure on the user pool. This is guaranteed by three properties:
1. Model pages are pinned (never reclaimable by the page reclaimer)
2. Model pages are read-only (no writer can modify weights during inference)
3. Model eviction requires refcount == 0 (impossible while any session is active)

**Interaction with dynamic pool resizing ([§12.2](./memory-reclamation.md)):** When AIRS resource orchestration resizes the model pool / user pool boundary, only FREE pages participate. Model weight pages have nonzero refcounts and the `PINNED` flag — they are never on the free list and cannot be moved by pool boundary adjustment. The `security_floor` invariant ([§12.2](./memory-reclamation.md)) additionally prevents the pool from shrinking below the primary model's footprint while security services are active.

### 6.3 KV Cache Management

KV caches are the per-session cost of maintaining conversation context. Unlike model weights (which are static and shared), KV caches are dynamic, per-session, and can grow large:

```text
KV cache size ≈ 2 × num_layers × head_dim × num_kv_heads × context_length × sizeof(f16)

Llama 3.1 8B:
  32 layers × 128 head_dim × 8 kv_heads × 8192 context = ~1 GB at f16
  With Q8 quantization: ~512 MB
  With Q4 quantization: ~256 MB
```

AIOS uses **PagedAttention** — a technique pioneered by vLLM that manages KV caches as non-contiguous fixed-size blocks mapped through a block table, analogous to virtual memory page tables. Traditional KV cache allocation pre-reserves contiguous memory for the maximum context length, wasting 60-80% of model pool memory on empty slots. PagedAttention allocates blocks on demand as tokens are generated, reducing waste to under 4%.

```rust
/// KV cache for a single inference session, using PagedAttention.
/// Blocks are allocated on demand — no pre-reservation of max context.
pub struct KvCache {
    /// Session owning this cache
    pub session: SessionId,
    /// Block table: logical block index → physical block.
    /// Analogous to a page table mapping virtual → physical pages.
    /// Grows dynamically as context length increases.
    pub block_table: Vec<Option<KvBlockId>>,
    /// Current context length (tokens stored)
    pub context_length: u32,
    /// Maximum context length (model limit)
    pub max_context: u32,
    /// Total bytes currently allocated (not reserved)
    pub allocated_bytes: usize,
    /// Last time this cache was used
    pub last_used: Timestamp,
    /// Priority for eviction ordering
    pub priority: CachePriority,
    /// Prefix sharing: if this cache shares a prefix with another session,
    /// the shared blocks are COW (copy-on-write) — only divergent blocks
    /// are independently allocated.
    pub shared_prefix: Option<SharedPrefix>,
}

/// Fixed-size block in the KV cache.
/// Each block holds KV data for a fixed number of token positions.
pub struct KvCacheBlock {
    /// Unique block ID in the model pool
    id: KvBlockId,
    /// Physical frame(s) backing this block (may use medium 64 KB pages)
    frames: [PhysicalFrame; FRAMES_PER_KV_BLOCK],
    /// Number of token positions stored in this block
    tokens_stored: u32,
    /// Capacity: token positions per block
    tokens_capacity: u32,
    /// Reference count: >1 when shared via prefix caching
    refcount: AtomicU32,
}

/// Prefix sharing between sessions with common system prompts or context.
/// When two sessions share the first N tokens (e.g., same system prompt),
/// their KV blocks for those tokens are shared via COW.
pub struct SharedPrefix {
    /// Source session whose blocks we share
    source: SessionId,
    /// Number of shared blocks (from block 0 to shared_blocks-1)
    shared_blocks: u32,
    /// Shared blocks are read-only. If this session modifies a shared
    /// block (e.g., due to positional encoding differences), it COWs:
    /// allocate a new block, copy data, update block_table entry.
}

pub enum CachePriority {
    /// User actively waiting (conversation bar)
    Interactive,
    /// System service (intent verification, context engine)
    System,
    /// Background work (space indexing)
    Background,
}

/// Block sizing: 16 token positions per block at the default.
/// For Llama 3.1 8B (32 layers, 8 KV heads, 128 head_dim, Q8):
///   Per-token KV size = 2 × 32 × 8 × 128 × 1 byte = 64 KB
///   Block size = 16 tokens × 64 KB = 1 MB per block
/// Backed by 64 KB medium THP pages for efficient TLB usage.
const KV_TOKENS_PER_BLOCK: u32 = 16;
const KV_BLOCK_SIZE: usize = 1 * MB; // 1 MB blocks (model-dependent)
const KV_MEDIUM_PAGE_SIZE: usize = 64 * KB; // 64 KB medium THP
const FRAMES_PER_KV_BLOCK: usize = KV_BLOCK_SIZE / KV_MEDIUM_PAGE_SIZE; // 16 medium pages per block
```

**PagedAttention memory savings:**

```text
Scenario: 4 concurrent sessions, 8K max context, 8B model (Q8 KV)

Traditional (contiguous pre-allocation):
  Per session: 8192 tokens × 64 KB/token = 512 MB reserved
  4 sessions: 2048 MB reserved
  Actual usage (avg 2K tokens used): 512 MB
  Waste: 1536 MB (75%)

PagedAttention (on-demand blocks):
  Per session: only allocated blocks for actual tokens
  4 sessions at avg 2K tokens: 4 × 128 MB = 512 MB allocated
  Waste: < 20 MB (partially-filled last blocks)
  Savings: 1516 MB freed for other use

On an 8 GB device with 4 GB model pool:
  Traditional: 2 GB KV + 2.5 GB model weights = exceeds pool
  PagedAttention: 512 MB KV + 2.5 GB model weights = 3 GB, fits with 1 GB headroom
```

**Prefix caching — cross-session KV sharing:**

When multiple sessions use the same system prompt (common: conversation bar, intent verifier, and behavioral monitor all share AIOS system prompts), their KV cache blocks for those tokens are identical. PagedAttention enables sharing:

```text
Session A (conversation bar):  [system prompt KV | user context A KV]
Session B (intent verifier):   [system prompt KV | user context B KV]
Session C (behavioral monitor):[system prompt KV | user context C KV]

Without prefix sharing:  3 × 200 tokens × 64 KB = 38.4 MB for system prompts
With prefix sharing:     1 × 200 tokens × 64 KB = 12.8 MB (shared via COW)
Savings: 25.6 MB — significant when model pool is 2-4 GB
```

**KV cache eviction** follows priority ordering when the model pool is under pressure. **Important:** KV cache "eviction" means **deallocation back to the model pool free list** — not MGLRU-based page reclamation. KV cache blocks live in the pinned model pool, which is excluded from MGLRU tracking entirely. MGLRU governs user pool pages (agent heaps, page cache, shared memory). The KV cache eviction policy below is a separate, AIRS-driven mechanism:

```text
Eviction order (first evicted → last evicted):
1. Background session KV caches (space indexing, metadata generation)
2. System session KV caches (intent verifier, behavioral monitor)
3. Idle interactive session KV caches (conversation bar idle > 5 min)
4. Active interactive session KV caches (never evicted — inference fails instead)

Within a priority level, partially-filled blocks are evicted first (least
tokens stored = least re-computation cost to reconstruct).
```

**DAMON integration for KV caches:** While MGLRU does not track model pool pages, DAMON ([§10.9](./memory-reclamation.md)) can still monitor access patterns on KV cache memory regions. DAMON detects when a KV cache transitions from active (inference in progress) to idle (session waiting), enabling AIRS to make proactive eviction decisions. DAMON reports access frequency; AIRS decides whether to evict; the kernel executes the deallocation. The feedback path is: DAMON → AIRS resource orchestration → kernel KV cache eviction → model pool free list.

When a KV cache is evicted, the session's conversation history is still in a space object. The cache can be reconstructed by re-processing the conversation — slower than keeping it in RAM, but not data-losing. With prefix caching, reconstruction is faster: only the session-specific suffix needs recomputation; the shared prefix blocks may still be resident from another session.

**Alternative: RadixAttention (SGLang, 2024).** SGLang's RadixAttention organizes prefix sharing as a radix tree rather than flat block tables. Each edge represents a sequence of tokens; shared prefixes naturally coalesce as tree paths. This enables LRU-based prefix eviction at the tree level and automatic deduplication of arbitrary-length common prefixes, not just fixed-size block boundaries. AIOS's PagedAttention block table design can be extended to use radix tree indexing for the prefix layer while retaining per-session block allocation for session-specific suffixes — see [§13](./memory-hardening.md) (Future Directions).

### 6.4 Model Loading and Eviction

Models are loaded from space storage into the model pool. AIOS uses **userfaultfd-based lazy loading** — a technique that enables inference to begin before the entire model is resident in RAM. Instead of blocking until all model pages are faulted in, the kernel registers a userfaultfd handler that loads pages on demand with intelligent prefetch, allowing the first inference to start within seconds even for multi-GB models on SD card storage.

```text
Model loading flow (userfaultfd lazy loading):

1. AIRS requests model load: model_id = "phi-3-mini-q4"
     ↓
2. Kernel allocates virtual address range in model pool (2 MB huge page aligned)
   — physical pages NOT yet allocated (lazy)
     ↓
3. Register userfaultfd handler for the model region:
   - Handler knows the GGUF file layout (tensor offsets)
   - Handler reads from space storage on page fault
     ↓
4. AIRS maps the region read-only into its address space
     ↓
5. Inference can start IMMEDIATELY:
   - First token access faults in the embedding layer weights (~50 MB)
   - Subsequent layers are faulted in as inference progresses
   - Prefetch thread reads ahead: if layer N is accessed, prefetch layers N+1, N+2
     ↓
6. Background prefetch continues loading remaining layers:
   - Prioritizes layers in inference order (embedding → attention → FFN)
   - Uses low-priority I/O to avoid blocking active inference faults
     ↓
7. After full warmup (all pages resident), inference runs at full speed
   - userfaultfd handler is detached (no further overhead)
   - Pages are pinned with VmFlags::PINNED
```

```rust
/// Lazy model loader using userfaultfd
pub struct LazyModelLoader {
    /// userfaultfd file descriptor for this model region
    uffd: UserfaultFd,
    /// Model region virtual address range
    region: VirtualRange,
    /// GGUF file handle in space storage
    gguf_file: SpaceObjectHandle,
    /// Tensor layout: maps virtual offset → GGUF file offset
    tensor_map: Vec<TensorMapping>,
    /// Prefetch state
    prefetch: PrefetchState,
    /// Pages loaded so far
    pages_loaded: AtomicUsize,
    /// Total pages needed
    pages_total: usize,
}

pub struct TensorMapping {
    /// Virtual offset within model region
    vaddr_offset: usize,
    /// Offset within GGUF file
    file_offset: usize,
    /// Size in bytes
    size: usize,
    /// Layer index (for prefetch ordering)
    layer: u32,
}

pub struct PrefetchState {
    /// Last layer accessed by inference
    last_accessed_layer: AtomicU32,
    /// Prefetch window: how many layers ahead to read
    window: u32,           // default: 2 layers ahead
    /// Prefetch thread handle
    thread: Option<JoinHandle<()>>,
}

impl LazyModelLoader {
    /// Handle a page fault in the model region
    fn handle_fault(&self, addr: VirtualAddress) -> Result<(), FaultError> {
        let offset = addr.0 - self.region.start.0;
        let tensor = self.tensor_map.iter()
            .find(|t| offset >= t.vaddr_offset && offset < t.vaddr_offset + t.size)
            .ok_or(FaultError::UnmappedRegion)?;

        // Read the faulted page from space storage
        let page_offset = offset & !(PAGE_SIZE_2MB - 1); // 2 MB aligned
        let file_offset = tensor.file_offset + (page_offset - tensor.vaddr_offset);
        let data = self.gguf_file.read_at(file_offset, PAGE_SIZE_2MB)?;

        // Install the page via userfaultfd UFFDIO_COPY
        self.uffd.copy(addr, &data)?;
        self.pages_loaded.fetch_add(1, Ordering::Relaxed);

        // Signal prefetch thread: advance if needed
        self.prefetch.last_accessed_layer.store(tensor.layer, Ordering::Relaxed);

        Ok(())
    }
}
```

**Why userfaultfd instead of plain demand paging?** Standard demand paging (mmap + page fault) works but has no awareness of model structure. A page fault in the middle of a tensor triggers a single 4 KB/2 MB read. With userfaultfd, the fault handler knows the GGUF layout — it can prefetch entire tensors and prioritize layers that inference will access next. On SD card storage where sequential reads are 10x faster than random reads, this prefetch intelligence reduces model loading time by 40-60%.

**First-token latency improvement:**

```text
Loading a 4.5 GB model (Llama 3.1 8B Q4_K_M) from SD card:

Traditional (load all, then start):
  Load time: 45 seconds (100 MB/s sequential read)
  First token: 45 seconds

userfaultfd lazy loading:
  Embedding layer fault-in: ~2 seconds (first ~100 MB)
  First token: ~3 seconds (embedding + first attention layer)
  Full warmup (background): ~45 seconds

Time to first token: 3s vs 45s (15x faster)
```

This matters enormously for user experience. When the user opens the conversation bar, they expect a response in seconds, not a 45-second wait for model loading. Lazy loading with userfaultfd makes AIRS responsive immediately — the first few layers are enough to begin generating tokens. Inference quality is identical; only the first few tokens have slightly higher latency (page faults during layer traversal). By the time the user reads the first sentence, the full model is resident.

**Fault-around optimization (proposed):** Inspired by Linux's `fault_around_bytes` (default 64 KB), the target userfaultfd handler would map not just the faulted page but the surrounding 16 pages (64 KB) in the same fault handler invocation. This amortizes the trap overhead: instead of 16 separate page faults for a sequential model layer read, one fault resolves 16 pages. Since model weights are stored contiguously, the speculation hit rate is expected to be very high.

**Deterministic prefetching for transformer inference (proposed):** Transformer models have a highly predictable memory access pattern: layer 0 → layer 1 → ... → layer N, repeated for each token. Unlike general application memory access (where future pages are unknown), the userfaultfd handler can exploit this structure. When inference begins processing layer K, the prefetcher would issue asynchronous reads for layer K+1 (and optionally K+2) pages that aren't yet resident. This would convert most subsequent faults into TLB misses resolved against already-resident pages, eliminating I/O stalls after the first few layers warm up. See [§13.4](./memory-hardening.md) for related research directions.

```rust
/// Policy for model eviction when pool is full
pub struct ModelEvictionPolicy {
    /// Currently loaded models ordered by last use time
    loaded: Vec<LoadedModel>,
}

pub struct LoadedModel {
    pub model_id: ModelId,
    pub region: ModelMemoryRegion,
    pub last_used: Timestamp,
    pub active_sessions: usize,
}

impl ModelEvictionPolicy {
    /// Select a model to evict (returns None if no model can be evicted)
    pub fn select_victim(&self) -> Option<ModelId> {
        // Never evict a model with active interactive sessions
        // Prefer evicting models with zero sessions
        // Among those, evict least recently used
        self.loaded.iter()
            .filter(|m| m.active_sessions == 0)
            .min_by_key(|m| m.last_used)
            .map(|m| m.model_id)
    }
}
```

**On 2 GB devices:** No local model is loaded. The model pool is zero. All inference is routed to cloud endpoints via the NTM. This eliminates the memory pressure that model weights would cause on a 2 GB system.

**On 4 GB devices:** Only one small model (1-3B at Q4) fits at a time. Model switching requires full eviction and reload — an operation that takes several seconds from SD card storage. AIRS avoids unnecessary model switches by routing all task types to the single loaded model.

**On 8 GB devices:** A large model (8B Q4) and an embedding model can coexist simultaneously. Model switching is rare. The model pool has enough headroom for generous KV caches.
