//! Kernel Address Space Layout Randomization (KASLR).
//!
//! Computes a random slide for the kernel virtual base address.
//! The slide is a multiple of 2 MiB within a configurable range.
//!
//! Entropy source priority:
//! 1. BootInfo RNG seed (from UEFI RNG protocol, if available)
//! 2. CNTPCT_EL0 (ARM generic timer counter — weak but non-deterministic)
//!
//! The computed slide is printed at boot only. No runtime API exposes
//! the slide to prevent information leaks.
//!
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

/// Compute a KASLR slide from an entropy source.
///
/// If `rng_seed` is non-zero (from UEFI RNG), uses its first 8 bytes.
/// Otherwise falls back to `CNTPCT_EL0` (timer counter).
///
/// Returns a `KaslrConfig` with the computed slide. The slide is currently
/// computed but **not applied** — full KASLR address transition requires
/// careful TTBR1 rebuild with both old and new mappings active.
/// The slide value is logged for verification.
pub fn compute_slide(rng_seed: &[u8; 32]) -> KaslrConfig {
    let mut config = KaslrConfig::default_config();

    // Extract entropy
    let entropy: u64 = if rng_seed.iter().any(|&b| b != 0) {
        // Use first 8 bytes of RNG seed
        u64::from_le_bytes([
            rng_seed[0],
            rng_seed[1],
            rng_seed[2],
            rng_seed[3],
            rng_seed[4],
            rng_seed[5],
            rng_seed[6],
            rng_seed[7],
        ])
    } else {
        // Fallback: read timer counter (weak entropy)
        let cnt: u64;
        // SAFETY: CNTPCT_EL0 is readable at EL1. It's a monotonic counter
        // incremented by the system timer — non-deterministic at boot.
        unsafe {
            core::arch::asm!(
                "mrs {}, CNTPCT_EL0",
                out(reg) cnt,
                options(nomem, nostack, preserves_flags),
            );
        }
        cnt
    };

    // Compute slide: steps = range / alignment, slide = (entropy % steps) * alignment
    let steps = config.slide_range / config.alignment; // 64 for 128 MiB / 2 MiB
    let slide = ((entropy as usize) % steps) * config.alignment;
    config.slide = slide;

    crate::kinfo!(
        Mm,
        "KASLR base: {:#x} (slide: {:#x}, {} MiB)",
        config.base.wrapping_add(slide),
        slide,
        slide / (1024 * 1024)
    );

    config
}
