# AIOS Interface Kit — Performance

Part of: [interface-kit.md](../interface-kit.md) — Interface Kit Architecture
**Related:** [rendering.md](./rendering.md) — Render pipeline, [compositor.md](../../platform/compositor/rendering.md) — Frame scheduling

-----

## 13. Performance

### 13.1 Frame Budget Enforcement

```rust
pub struct FrameProfiler {
    budget_ms: f64,
    phase_timings: HashMap<Phase, f64>,
}

impl FrameProfiler {
    pub fn begin_phase(&mut self, phase: Phase) {
        self.phase_timings.insert(phase, Instant::now().as_millis_f64());
    }

    pub fn end_phase(&mut self, phase: Phase) -> f64 {
        let start = self.phase_timings[&phase];
        let elapsed = Instant::now().as_millis_f64() - start;
        elapsed
    }

    pub fn should_skip_frame(&self) -> bool {
        let total: f64 = self.phase_timings.values().sum();
        total > self.budget_ms * 1.5 // allow 50% overshoot before skipping
    }
}
```

### 13.2 Texture Atlas

All images, glyphs, and icons share a single GPU texture atlas to minimize draw calls and texture binding switches:

```rust
pub struct TextureAtlas {
    texture: wgpu::Texture,
    allocator: AtlasAllocator,
    size: u32, // 4096x4096 default
}

impl TextureAtlas {
    pub fn allocate(&mut self, width: u32, height: u32) -> AtlasRegion {
        match self.allocator.allocate(width, height) {
            Some(region) => region,
            None => {
                // Atlas full — evict least recently used entries
                self.evict_lru();
                self.allocator.allocate(width, height)
                    .expect("atlas eviction failed to free space")
            }
        }
    }
}
```

### 13.3 Agent UI Performance Guidelines

- Keep `view()` under 2ms — avoid allocations, use `lazy()` for expensive subtrees
- Use `lazy()` to skip unchanged subtrees during diff
- Avoid unbounded lists — use `virtual_list()` for scrollable content over 100 items
- Profile with the AIOS Inspector's frame timing panel
