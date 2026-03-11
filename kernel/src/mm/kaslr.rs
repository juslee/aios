//! Kernel Address Space Layout Randomization (KASLR).
//!
//! Wraps the pure computation from shared::kaslr with hardware entropy sources.
//! Per memory.md §3.3.

// Re-export from shared crate.
pub use shared::{compute_slide_from_entropy, KaslrConfig};

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

    let config = compute_slide_from_entropy(entropy);

    crate::kinfo!(
        Mm,
        "KASLR base: {:#x} (slide: {:#x}, {} MiB)",
        config.base.wrapping_add(config.slide),
        config.slide,
        config.slide / (1024 * 1024)
    );

    config
}
