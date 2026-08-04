//! Runtime SIMD capability probe.

use std::sync::OnceLock;

/// Best SIMD ISA the current process may use.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SimdLevel {
    /// Portable scalar loops (always correct).
    Scalar,
    /// x86_64 SSE2 (baseline on this target) — 2×`f64` / 2×`i64`.
    Sse2,
    /// x86_64 AVX2 (+FMA when available) — 4×`f64` / 4×`i64`.
    Avx2,
    /// aarch64 NEON — 2×`f64` / 2×`i64`.
    Neon,
}

static LEVEL: OnceLock<SimdLevel> = OnceLock::new();

/// Return the cached [`SimdLevel`] for this process.
pub fn detect() -> SimdLevel {
    *LEVEL.get_or_init(probe)
}

fn probe() -> SimdLevel {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            return SimdLevel::Avx2;
        }
        // SSE2 is part of the x86_64 ABI / stdlib baseline.
        return SimdLevel::Sse2;
    }
    #[cfg(target_arch = "aarch64")]
    {
        return SimdLevel::Neon;
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        SimdLevel::Scalar
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_is_stable() {
        let a = detect();
        let b = detect();
        assert_eq!(a, b);
        #[cfg(target_arch = "x86_64")]
        {
            assert!(matches!(a, SimdLevel::Sse2 | SimdLevel::Avx2));
        }
        #[cfg(target_arch = "aarch64")]
        {
            assert_eq!(a, SimdLevel::Neon);
        }
    }
}
