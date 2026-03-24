# AIOS Interface Kit — Theme System

Part of: [interface-kit.md](../interface-kit.md) — Interface Kit Architecture
**Related:** [context-engine.md](../../intelligence/context-engine.md) — Context-aware adaptation, [accessibility.md](./accessibility.md) — High contrast themes

-----

## 6. Theme System

### 6.1 Theme Tokens

The theme system uses a token-based approach. Every visual property references a token, not a hardcoded value:

```rust
pub struct Theme {
    pub palette: Palette,
    pub typography: Typography,
    pub spacing: Spacing,
    pub radius: Radius,
    pub animation: AnimationConfig,
}

pub struct Palette {
    pub background: Color,
    pub surface: Color,
    pub primary: Color,
    pub secondary: Color,
    pub accent: Color,
    pub text: Color,
    pub text_secondary: Color,
    pub text_disabled: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub border: Color,
    pub shadow: Color,
}

pub struct Typography {
    pub heading_large: TextStyle,   // 28px, bold
    pub heading_medium: TextStyle,  // 22px, semibold
    pub heading_small: TextStyle,   // 18px, semibold
    pub body: TextStyle,            // 15px, regular
    pub body_small: TextStyle,      // 13px, regular
    pub caption: TextStyle,         // 11px, regular
    pub monospace: TextStyle,       // 14px, monospace
}

pub struct Spacing {
    pub xs: f32,    // 4
    pub sm: f32,    // 8
    pub md: f32,    // 16
    pub lg: f32,    // 24
    pub xl: f32,    // 32
    pub xxl: f32,   // 48
}
```

### 6.2 Context-Aware Themes

On AIOS, the theme adapts to the Context Engine's inferred state:

```rust
pub fn theme_for_context(base: &Theme, context: &ContextState) -> Theme {
    let mut theme = base.clone();

    match context.mode {
        ContextMode::Work => {
            // Higher information density
            theme.spacing = Spacing::compact();
            // Neutral, focused palette
            theme.palette.background = Color::from_rgb(0.97, 0.97, 0.98);
        }
        ContextMode::Leisure => {
            // Lower density, more breathing room
            theme.spacing = Spacing::relaxed();
            // Warmer tones
            theme.palette.background = Color::from_rgb(0.98, 0.97, 0.95);
        }
        ContextMode::Focus => {
            // Minimal chrome, maximum content area
            theme.spacing = Spacing::minimal();
            // Reduced contrast for non-focused elements
            theme.palette.text_secondary = theme.palette.text_disabled;
        }
        ContextMode::Gaming => {
            // Dark theme, high contrast
            theme.palette = Palette::dark();
        }
    }

    // Time-of-day adjustment
    if context.time_of_day.hour() >= 20 || context.time_of_day.hour() < 6 {
        theme.palette = theme.palette.warm_shift(0.05);
    }

    theme
}
```

On non-AIOS platforms, `theme_for_context` is never called. The application uses the base theme directly, or the system light/dark mode preference.

### 6.3 Agent Theming

Agents can customize their theme within bounds set by the system:

```rust
pub struct AgentThemeOverride {
    /// Accent color (agent branding)
    pub accent: Option<Color>,
    /// Whether to use a custom icon set
    pub icon_set: Option<IconSet>,
    /// Font override (must be available on system)
    pub font: Option<Font>,
}

impl AgentThemeOverride {
    /// Apply agent overrides to system theme, clamping to accessibility bounds
    pub fn apply(&self, system_theme: &Theme) -> Theme {
        let mut theme = system_theme.clone();
        if let Some(accent) = self.accent {
            // Ensure sufficient contrast ratio (WCAG AA: 4.5:1)
            if contrast_ratio(accent, theme.palette.background) >= 4.5 {
                theme.palette.accent = accent;
            }
        }
        theme
    }
}
```

### 6.4 Motion Tokens

Animation timing and easing are part of the theme system, not hardcoded in widgets:

```rust
pub struct AnimationConfig {
    /// Default transition duration for state changes (hover, press, focus)
    pub transition_duration: Duration,
    /// Easing curve for enter transitions
    pub ease_in: EasingCurve,
    /// Easing curve for exit transitions
    pub ease_out: EasingCurve,
    /// Spring parameters for physics-based animations
    pub spring: SpringConfig,
    /// Whether to reduce motion for accessibility (respects user preference)
    pub reduce_motion: bool,
}

pub struct SpringConfig {
    /// Spring stiffness (higher = snappier)
    pub stiffness: f32,   // default: 300.0
    /// Damping ratio (1.0 = critically damped, <1.0 = bouncy)
    pub damping: f32,     // default: 0.85
    /// Mass (higher = more inertia)
    pub mass: f32,        // default: 1.0
}

pub enum EasingCurve {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    CubicBezier(f32, f32, f32, f32),
    Spring(SpringConfig),
}
```

When `reduce_motion` is true (set by system accessibility preference or user override), all animations collapse to instant state changes. Widgets never check this flag directly — the runtime respects it automatically.

### 6.5 Elevation System

Surfaces have elevation levels that affect shadow depth and layering:

```rust
pub struct Elevation {
    /// Elevation level (0 = flat, higher = more shadow)
    pub level: u8,       // 0-5
    /// Shadow color (derived from palette.shadow)
    pub shadow_color: Color,
    /// Shadow blur radius (computed from level)
    pub blur_radius: f32,
    /// Shadow offset (computed from level)
    pub offset_y: f32,
}

impl Elevation {
    pub fn flat() -> Self { Self { level: 0, .. } }
    pub fn card() -> Self { Self { level: 1, .. } }     // subtle
    pub fn dropdown() -> Self { Self { level: 2, .. } }  // floating
    pub fn dialog() -> Self { Self { level: 3, .. } }    // prominent
    pub fn tooltip() -> Self { Self { level: 4, .. } }   // overlay
}
```
