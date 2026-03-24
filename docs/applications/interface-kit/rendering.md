# AIOS Interface Kit — Render Pipeline

Part of: [interface-kit.md](../interface-kit.md) — Interface Kit Architecture
**Related:** [gpu.md](../../platform/gpu/rendering.md) — GPU rendering, [compositor.md](../../platform/compositor/rendering.md) — Frame composition, [performance.md](./performance.md) — Frame budget

-----

## 8. Render Pipeline

### 8.1 From Widgets to Pixels

```text
Widget Tree (declarative, returned by view())
  │
  ▼
Layout Tree (positioned nodes with absolute coordinates)
  │
  ▼
Diff (compare with previous frame's tree)
  │    Only process changed subtrees
  ▼
Display List (flat list of draw commands)
  │    Primitives: quad, text, image, clip, transform
  ▼
Damage Tracking (regions that changed since last frame)
  │    Only re-render damaged rectangles
  ▼
GPU Submission (wgpu render pass)
  │    Batched draw calls, texture atlas
  ▼
Present (swap chain present / compositor submit)
```

### 8.2 Display List Primitives

```rust
pub enum Primitive {
    /// Filled rectangle with optional rounded corners
    Quad {
        bounds: Rectangle,
        background: Background,
        border: Border,
        shadow: Shadow,
    },
    /// Rendered text run
    Text {
        content: String,
        bounds: Rectangle,
        color: Color,
        size: f32,
        font: Font,
        alignment: Alignment,
    },
    /// Image from texture atlas or standalone
    Image {
        handle: ImageHandle,
        bounds: Rectangle,
        filter: ImageFilter,
    },
    /// Clip all children to a rectangle
    Clip {
        bounds: Rectangle,
        children: Vec<Primitive>,
    },
    /// Affine transform
    Transform {
        transform: Transform2D,
        children: Vec<Primitive>,
    },
    /// Custom shader (for canvas widget)
    Shader {
        bounds: Rectangle,
        program: ShaderProgram,
    },
}
```

### 8.3 Damage Tracking

Only redraw regions that changed. This is critical for battery life and GPU efficiency:

```rust
pub struct DamageTracker {
    previous_display_list: Vec<Primitive>,
    damaged_regions: Vec<Rectangle>,
}

impl DamageTracker {
    pub fn compute_damage(
        &mut self,
        current: &[Primitive],
    ) -> &[Rectangle] {
        self.damaged_regions.clear();

        for (prev, curr) in self.previous_display_list.iter().zip(current.iter()) {
            if prev != curr {
                // Mark both old and new bounds as damaged
                self.damaged_regions.push(prev.bounds());
                self.damaged_regions.push(curr.bounds());
            }
        }

        // Handle list length changes
        if current.len() > self.previous_display_list.len() {
            for prim in &current[self.previous_display_list.len()..] {
                self.damaged_regions.push(prim.bounds());
            }
        }

        self.previous_display_list = current.to_vec();
        self.merge_overlapping(&mut self.damaged_regions);
        &self.damaged_regions
    }
}
```

### 8.4 Frame Pacing

Target 60fps (16.6ms per frame). Budget breakdown:

| Phase | Budget |
|-------|--------|
| Event handling + `update()` | 1ms |
| `view()` — build widget tree | 2ms |
| Layout | 2ms |
| Diff + damage | 1ms |
| Display list generation | 2ms |
| GPU submission | 2ms |
| GPU rendering | 4ms |
| Present + vsync | 2.6ms |
| **Total** | **16.6ms** |

If layout or rendering exceeds budget, the toolkit skips frames rather than dropping interactivity. Input events are always processed — the view may lag but never the response.

-----

### 8.5 Animation System

Interface Kit provides a declarative animation system inspired by SwiftUI's springs and Jetpack Compose's animate*AsState. Animations are defined as state transitions, not imperative frame-by-frame updates.

#### 8.5.1 Spring Animations

The default animation model uses critically-damped springs (from the theme's `SpringConfig`). Springs produce natural-feeling motion that is interruptible — redirecting a spring mid-flight preserves velocity for smooth transitions.

```rust
/// A spring-animated value.
pub struct Animated<T: Interpolatable> {
    /// Current value (updated each frame).
    current: T,
    /// Target value.
    target: T,
    /// Current velocity.
    velocity: T,
    /// Spring configuration (from theme).
    spring: SpringConfig,
}

impl<T: Interpolatable> Animated<T> {
    /// Set a new target. The spring redirects smoothly from
    /// the current position and velocity — no discontinuity.
    pub fn animate_to(&mut self, target: T) {
        self.target = target;
        // velocity is preserved — this is what makes springs interruptible
    }

    /// Advance the spring by dt seconds.
    pub fn step(&mut self, dt: f32) {
        // Critically-damped spring differential equation:
        // acceleration = stiffness * (target - current) - damping * velocity
        let displacement = self.target.sub(&self.current);
        let spring_force = displacement.scale(self.spring.stiffness);
        let damping_force = self.velocity.scale(self.spring.damping * 2.0
            * (self.spring.stiffness * self.spring.mass).sqrt());
        let acceleration = spring_force.sub(&damping_force).scale(1.0 / self.spring.mass);
        self.velocity = self.velocity.add(&acceleration.scale(dt));
        self.current = self.current.add(&self.velocity.scale(dt));
    }

    /// Whether the animation has settled (within epsilon of target, near-zero velocity).
    pub fn is_settled(&self) -> bool {
        self.current.distance(&self.target) < 0.5
            && self.velocity.magnitude() < 0.1
    }
}
```

#### 8.5.2 Transition System

State changes in widgets trigger automatic transitions:

```rust
fn view(&self) -> Element<Message> {
    let panel = container(self.content())
        .width(if self.expanded { Length::Fill } else { Length::Fixed(48.0) })
        // width change is automatically animated with theme spring
        .animate(AnimationProperty::Width);

    let opacity = if self.visible { 1.0 } else { 0.0 };
    container(panel)
        .opacity(opacity)
        // opacity change is animated with a 200ms ease-out
        .animate_with(AnimationProperty::Opacity, Animation::ease_out(Duration::from_millis(200)))
        .into()
}
```

The runtime detects when an animated property changes between frames and creates/updates an `Animated<T>` internally. The widget does not manage animation state — it declares the desired end state, and the runtime interpolates.

#### 8.5.3 Enter/Exit Animations

Widgets entering or leaving the tree can specify animations:

```rust
fn view(&self) -> Element<Message> {
    column(
        self.items.iter().enumerate().map(|(i, item)| {
            container(text(&item.title))
                .enter(Animation::slide_in(Direction::Left, Duration::from_millis(150)))
                .exit(Animation::fade_out(Duration::from_millis(100)))
                .into()
        }).collect()
    )
    .into()
}
```

When an item is removed from the list, the runtime keeps its widget alive for the exit animation duration before destroying it.

#### 8.5.4 Gesture-Driven Animation

Touch gestures can drive animations directly, creating fluid interactions:

```rust
/// A gesture-driven animation tracks a user's finger/cursor
/// and hands off to a spring when released.
pub struct GestureAnimation<T: Interpolatable> {
    /// Current value driven by gesture.
    current: T,
    /// The value to spring back to if gesture is cancelled.
    rest_value: T,
    /// The value to spring to if gesture completes.
    commit_value: T,
    /// Spring for release animation.
    spring: SpringConfig,
    /// Whether the gesture is currently active.
    tracking: bool,
}
```

Examples: pull-to-refresh (spring back on release), swipe-to-delete (commit or cancel threshold), draggable panels (snap to grid positions).

#### 8.5.5 Reduced Motion

When `theme.animation.reduce_motion` is true (see [theme.md](./theme.md)):
- All `Animated<T>` values snap instantly to their target.
- Enter/exit animations are skipped.
- Gesture animations still track the finger but snap on release instead of springing.

This is automatic — widgets never check the flag.
