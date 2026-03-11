//! Kernel Address Space Layout Randomization (KASLR) configuration.
//!
//! Pure data types and computation — no hardware register access.
//! Per memory.md §3.3.

/// KASLR configuration.
pub struct KaslrConfig {
    /// Base virtual address (before slide).
    pub base: usize,
    /// Alignment of the slide (must be power of 2).
    pub alignment: usize,
    /// Maximum slide range in bytes.
    pub slide_range: usize,
    /// Computed slide offset.
    pub slide: usize,
}

impl KaslrConfig {
    /// Default KASLR configuration for the kernel.
    ///
    /// - Base: KERNEL_BASE (0xFFFF_0000_0000_0000)
    /// - Alignment: 2 MiB (matches L2 block size for boot.S mapping)
    /// - Range: 128 MiB (64 possible positions)
    pub const fn default_config() -> Self {
        Self {
            base: 0xFFFF_0000_0000_0000,
            alignment: 2 * 1024 * 1024,     // 2 MiB
            slide_range: 128 * 1024 * 1024, // 128 MiB
            slide: 0,
        }
    }
}

/// Compute a KASLR slide from a raw entropy value.
///
/// This is the pure computation extracted from the kernel's `compute_slide()`,
/// which additionally reads hardware registers for entropy. By accepting
/// `entropy` as a parameter, this function is testable on the host.
///
/// Returns a `KaslrConfig` with the computed slide.
pub fn compute_slide_from_entropy(entropy: u64) -> KaslrConfig {
    let mut config = KaslrConfig::default_config();

    // steps = range / alignment, slide = (entropy % steps) * alignment
    let steps = config.slide_range / config.alignment; // 64 for 128 MiB / 2 MiB
    let slide = ((entropy as usize) % steps) * config.alignment;
    config.slide = slide;

    config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let cfg = KaslrConfig::default_config();
        assert_eq!(cfg.base, 0xFFFF_0000_0000_0000);
        assert_eq!(cfg.alignment, 2 * 1024 * 1024);
        assert_eq!(cfg.slide_range, 128 * 1024 * 1024);
        assert_eq!(cfg.slide, 0);
    }

    #[test]
    fn default_alignment_is_power_of_two() {
        let cfg = KaslrConfig::default_config();
        assert!(cfg.alignment.is_power_of_two());
    }

    #[test]
    fn slide_range_is_multiple_of_alignment() {
        let cfg = KaslrConfig::default_config();
        assert_eq!(cfg.slide_range % cfg.alignment, 0);
    }

    #[test]
    fn possible_positions_count() {
        let cfg = KaslrConfig::default_config();
        let positions = cfg.slide_range / cfg.alignment;
        assert_eq!(positions, 64);
    }

    #[test]
    fn slide_zero_entropy() {
        let cfg = compute_slide_from_entropy(0);
        assert_eq!(cfg.slide, 0);
    }

    #[test]
    fn slide_is_aligned() {
        for entropy in [0, 1, 42, 63, 64, 100, 1000, u64::MAX] {
            let cfg = compute_slide_from_entropy(entropy);
            assert_eq!(
                cfg.slide % cfg.alignment,
                0,
                "slide {:#x} not aligned for entropy {}",
                cfg.slide,
                entropy
            );
        }
    }

    #[test]
    fn slide_within_range() {
        for entropy in [0, 1, 42, 63, 64, 100, u64::MAX] {
            let cfg = compute_slide_from_entropy(entropy);
            assert!(
                cfg.slide < cfg.slide_range,
                "slide {:#x} >= range {:#x} for entropy {}",
                cfg.slide,
                cfg.slide_range,
                entropy
            );
        }
    }

    #[test]
    fn slide_wraps_at_steps() {
        // entropy=64 should give same slide as entropy=0 (64 positions).
        let cfg0 = compute_slide_from_entropy(0);
        let cfg64 = compute_slide_from_entropy(64);
        assert_eq!(cfg0.slide, cfg64.slide);
    }

    #[test]
    fn slide_max_value() {
        // Maximum slide = (steps-1) * alignment = 63 * 2MiB = 126 MiB.
        let cfg = compute_slide_from_entropy(63);
        assert_eq!(cfg.slide, 63 * 2 * 1024 * 1024);
    }

    #[test]
    fn slide_different_entropies_different_slides() {
        let cfg1 = compute_slide_from_entropy(1);
        let cfg2 = compute_slide_from_entropy(2);
        assert_ne!(cfg1.slide, cfg2.slide);
    }

    #[test]
    fn slide_deterministic() {
        let cfg_a = compute_slide_from_entropy(12345);
        let cfg_b = compute_slide_from_entropy(12345);
        assert_eq!(cfg_a.slide, cfg_b.slide);
    }
}
