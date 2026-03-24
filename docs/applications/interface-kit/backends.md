# AIOS Interface Kit — Platform Backends

Part of: [interface-kit.md](../interface-kit.md) — Interface Kit Architecture
**Related:** [compositor.md](../../platform/compositor.md) — Compositor protocol, [aios-features.md](./aios-features.md) — AIOS-specific features

-----

## 9. Platform Backends

### 9.1 AIOS Backend

The AIOS backend communicates with the compositor via IPC instead of creating its own window:

```rust
pub struct AiosBackend {
    compositor_channel: IpcChannel,
    surface_buffers: HashMap<SurfaceId, GpuBuffer>,
    flow_channel: Option<IpcChannel>,
    capability_set: CapabilitySet,
    context_subscription: Option<IpcChannel>,
}

impl InterfaceBackend for AiosBackend {
    fn create_surface(&mut self, hints: SurfaceHints) -> SurfaceId {
        // Send semantic hints to compositor (content type, resize behavior)
        let msg = CompositorMsg::CreateSurface {
            hints: hints.into(),
            buffer_format: BufferFormat::Bgra8,
        };
        self.compositor_channel.send(&msg);

        let response: CompositorResponse = self.compositor_channel.recv();
        let surface_id = response.surface_id;

        // Allocate GPU buffer for this surface
        let buffer = GpuBuffer::allocate(hints.initial_size);
        self.surface_buffers.insert(surface_id, buffer);

        surface_id
    }

    fn submit(&mut self, surface: SurfaceId, display_list: &DisplayList) {
        let buffer = &mut self.surface_buffers[&surface];

        // Render display list to GPU buffer
        let encoder = buffer.begin_render_pass();
        render_display_list(&display_list, &mut encoder);
        encoder.finish();

        // Share buffer handle with compositor (zero-copy)
        self.compositor_channel.send(&CompositorMsg::SubmitBuffer {
            surface,
            buffer_handle: buffer.share_handle(),
            damage: display_list.damage_regions(),
        });
    }

    fn capabilities(&self) -> InterfaceCapabilities {
        InterfaceCapabilities {
            semantic_hints: true,
            flow_integration: true,
            space_backed_data: true,
            capability_aware_ui: true,
            gpu_rendering: true,
            high_dpi: true,
            touch_input: true,
        }
    }
}
```

**Unique AIOS capabilities:**

- **GPU buffer sharing.** The agent renders to a GPU buffer and shares the handle with the compositor via IPC. No pixel copying — the compositor composites directly from the agent's buffer.
- **Semantic window hints.** The agent tells the compositor what kind of content it displays (text editor, media player, terminal). The compositor uses this for intelligent layout, animations, and context transitions.
- **Flow integration.** Drag events route through the Flow system, preserving data types and provenance. Dropping an image from a browser into a notes agent carries the source URL and content type.
- **Capability-aware rendering.** Widgets can query the agent's capability set and disable or hide elements the agent cannot use.

### 9.2 Linux Backend

Standard iced bridge behavior — wgpu + winit on Wayland or X11:

```rust
pub struct LinuxBackend {
    window: winit::window::Window,
    surface: wgpu::Surface,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl InterfaceBackend for LinuxBackend {
    fn create_surface(&mut self, hints: SurfaceHints) -> SurfaceId {
        // Ignore AIOS-specific hints, create standard window
        self.window.set_title(&hints.title);
        self.window.set_inner_size(hints.initial_size.into());
        SurfaceId(0) // single window
    }

    fn submit(&mut self, _surface: SurfaceId, display_list: &DisplayList) {
        let frame = self.surface.get_current_texture().unwrap();
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());

        render_display_list_wgpu(display_list, &view, &mut encoder);

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
    }

    fn capabilities(&self) -> InterfaceCapabilities {
        InterfaceCapabilities {
            semantic_hints: false,
            flow_integration: false,
            space_backed_data: false,
            capability_aware_ui: false,
            // LinuxBackend is only constructed after successful wgpu surface creation
            // and GPU initialization; headless / no-GPU paths use SoftwareBackend instead.
            gpu_rendering: true,
            high_dpi: true,
            touch_input: false,
        }
    }
}
```

### 9.3 macOS Backend

Identical to Linux backend but uses Metal via wgpu. winit handles Cocoa window management.

### 9.4 Web Backend

WASM target using Canvas API for rendering and DOM events for input:

```rust
pub struct WebBackend {
    canvas: web_sys::HtmlCanvasElement,
    context: web_sys::CanvasRenderingContext2d,
    event_queue: VecDeque<PlatformEvent>,
}

impl InterfaceBackend for WebBackend {
    fn submit(&mut self, _surface: SurfaceId, display_list: &DisplayList) {
        self.context.clear_rect(
            0.0, 0.0,
            self.canvas.width() as f64,
            self.canvas.height() as f64,
        );

        for primitive in display_list.primitives() {
            match primitive {
                Primitive::Quad { bounds, background, border, .. } => {
                    self.context.set_fill_style(&background.to_css().into());
                    self.context.fill_rect(
                        bounds.x as f64, bounds.y as f64,
                        bounds.width as f64, bounds.height as f64,
                    );
                }
                Primitive::Text { content, bounds, color, size, .. } => {
                    self.context.set_fill_style(&color.to_css().into());
                    self.context.set_font(&format!("{}px Inter", size));
                    self.context.fill_text(
                        content, bounds.x as f64, bounds.y as f64,
                    ).ok();
                }
                _ => { /* ... */ }
            }
        }
    }

    fn capabilities(&self) -> InterfaceCapabilities {
        InterfaceCapabilities {
            semantic_hints: false,
            flow_integration: false,
            space_backed_data: false,
            capability_aware_ui: false,
            gpu_rendering: false, // Canvas 2D, not WebGPU
            high_dpi: true,
            touch_input: true,
        }
    }
}
```

### 9.5 Bridge Trait

Bridges translate external toolkit APIs to Interface Kit primitives. A bridge is a layer between an external toolkit (Flutter, Qt, GTK, Electron) and the `InterfaceBackend`:

```rust
/// A bridge translates an external toolkit's widget model
/// to Interface Kit primitives.
pub trait InterfaceBridge {
    /// The external toolkit's widget type.
    type ExternalWidget;
    /// The external toolkit's event type.
    type ExternalEvent;

    /// Translate an external widget tree to Interface Kit Elements.
    fn translate_widget(&self, widget: &Self::ExternalWidget) -> Element<()>;

    /// Translate an Interface Kit platform event to the external format.
    fn translate_event(&self, event: PlatformEvent) -> Option<Self::ExternalEvent>;

    /// Declare which InterfaceCapabilities this bridge supports.
    /// Bridges that cannot express AIOS features (e.g., GTK cannot
    /// express semantic window hints) return false for those capabilities.
    fn supported_capabilities(&self) -> InterfaceCapabilities;

    /// Bridge name for diagnostics.
    fn name(&self) -> &str;
}
```

**Bridge capability matrix:**

| Capability | iced bridge | Flutter bridge | Qt bridge | GTK bridge | Web bridge |
|---|---|---|---|---|---|
| Semantic window hints | ✓ (AIOS) | ✓ (AIOS) | ✗ | ✗ | ✗ |
| Flow integration | ✓ (AIOS) | Partial | Partial (MIME) | Partial (MIME) | ✗ |
| Space-backed data | ✓ (AIOS) | ✗ | ✗ | ✗ | ✗ |
| Capability-aware UI | ✓ (AIOS) | ✗ | ✗ | ✗ | ✗ |
| GPU rendering | ✓ (wgpu) | ✓ (Impeller) | ✓ (RHI) | ✓ (GSK) | ✗ (Canvas 2D) |
| High DPI | ✓ | ✓ | ✓ | ✓ | ✓ |
| Touch input | ✓ | ✓ | ✓ | ✓ | ✓ |
| Accessibility tree | ✓ (AccessKit) | ✓ (Semantics) | ✓ (AT-SPI2) | ✓ (ATK) | ✓ (ARIA) |

The iced bridge is the only bridge that exposes all AIOS capabilities. Other bridges provide varying levels of AIOS feature support — but all support the core UI features (rendering, input, accessibility).

### 9.6 Backend Selection

The backend is selected at startup based on the target platform and available resources:

```rust
pub fn select_backend() -> Box<dyn InterfaceBackend> {
    #[cfg(target_os = "aios")]
    { Box::new(AiosBackend::new()) }

    #[cfg(target_os = "linux")]
    { Box::new(LinuxBackend::new()) }  // iced bridge, wgpu + winit

    #[cfg(target_os = "macos")]
    { Box::new(MacosBackend::new()) }  // iced bridge, wgpu + winit (Metal)

    #[cfg(target_arch = "wasm32")]
    { Box::new(WebBackend::new()) }    // iced bridge, Canvas + DOM

    #[cfg(not(any(target_os = "aios", target_os = "linux", target_os = "macos", target_arch = "wasm32")))]
    { compile_error!("unsupported platform") }
}
```
