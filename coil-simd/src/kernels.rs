//! Portable numeric kernels with runtime SIMD dispatch.

use crate::level::{SimdLevel, detect};
use crate::scalar;

/// Dot product of equal-prefix slices (`min(len)`).
#[inline]
pub fn dot_f64(a: &[f64], b: &[f64]) -> f64 {
    // Tiny vectors: scalar avoids probe + call overhead.
    if a.len().min(b.len()) < 8 {
        return scalar::dot_f64(a, b);
    }
    match detect() {
        #[cfg(target_arch = "x86_64")]
        SimdLevel::Avx2 => unsafe { crate::x86_64::avx2::dot_f64(a, b) },
        #[cfg(target_arch = "x86_64")]
        SimdLevel::Sse2 => unsafe { crate::x86_64::sse2::dot_f64(a, b) },
        #[cfg(target_arch = "aarch64")]
        SimdLevel::Neon => unsafe { crate::aarch64::dot_f64(a, b) },
        _ => scalar::dot_f64(a, b),
    }
}

/// Wrapping integer dot product.
#[inline]
pub fn dot_i64(a: &[i64], b: &[i64]) -> i64 {
    // i64 multiply is scalar on current ISAs; keep one path.
    scalar::dot_i64(a, b)
}

#[inline]
pub fn zip_add_f64(a: &[f64], b: &[f64], out: &mut [f64]) {
    dispatch_zip_f64(a, b, out, scalar::zip_add_f64, zip_add_f64_simd)
}

#[inline]
pub fn zip_sub_f64(a: &[f64], b: &[f64], out: &mut [f64]) {
    dispatch_zip_f64(a, b, out, scalar::zip_sub_f64, zip_sub_f64_simd)
}

#[inline]
pub fn zip_neg_f64(a: &[f64], out: &mut [f64]) {
    if a.len().min(out.len()) < 8 {
        return scalar::zip_neg_f64(a, out);
    }
    match detect() {
        #[cfg(target_arch = "x86_64")]
        SimdLevel::Avx2 => unsafe { crate::x86_64::avx2::zip_neg_f64(a, out) },
        #[cfg(target_arch = "x86_64")]
        SimdLevel::Sse2 => unsafe { crate::x86_64::sse2::zip_neg_f64(a, out) },
        #[cfg(target_arch = "aarch64")]
        SimdLevel::Neon => unsafe { crate::aarch64::zip_neg_f64(a, out) },
        _ => scalar::zip_neg_f64(a, out),
    }
}

#[inline]
pub fn zip_add_i64(a: &[i64], b: &[i64], out: &mut [i64]) {
    dispatch_zip_i64(a, b, out, scalar::zip_add_i64, zip_add_i64_simd)
}

#[inline]
pub fn zip_sub_i64(a: &[i64], b: &[i64], out: &mut [i64]) {
    dispatch_zip_i64(a, b, out, scalar::zip_sub_i64, zip_sub_i64_simd)
}

#[inline]
pub fn zip_neg_i64(a: &[i64], out: &mut [i64]) {
    if a.len().min(out.len()) < 8 {
        return scalar::zip_neg_i64(a, out);
    }
    match detect() {
        #[cfg(target_arch = "x86_64")]
        SimdLevel::Avx2 => unsafe { crate::x86_64::avx2::zip_neg_i64(a, out) },
        #[cfg(target_arch = "x86_64")]
        SimdLevel::Sse2 => unsafe { crate::x86_64::sse2::zip_neg_i64(a, out) },
        #[cfg(target_arch = "aarch64")]
        SimdLevel::Neon => unsafe { crate::aarch64::zip_neg_i64(a, out) },
        _ => scalar::zip_neg_i64(a, out),
    }
}

/// Row-major `C[m×n] = A[m×k] * B[k×n]` for `f64`.
#[inline]
pub fn matmul_f64(a: &[f64], b: &[f64], c: &mut [f64], m: usize, k: usize, n: usize) {
    let cells = m.saturating_mul(k).saturating_mul(n);
    if cells < 64 {
        return scalar::matmul_f64(a, b, c, m, k, n);
    }
    match detect() {
        #[cfg(target_arch = "x86_64")]
        SimdLevel::Avx2 => unsafe { crate::x86_64::avx2::matmul_f64(a, b, c, m, k, n) },
        #[cfg(target_arch = "x86_64")]
        SimdLevel::Sse2 => unsafe { crate::x86_64::sse2::matmul_f64(a, b, c, m, k, n) },
        #[cfg(target_arch = "aarch64")]
        SimdLevel::Neon => unsafe { crate::aarch64::matmul_f64(a, b, c, m, k, n) },
        _ => scalar::matmul_f64(a, b, c, m, k, n),
    }
}

/// Row-major wrapping `i64` matmul.
#[inline]
pub fn matmul_i64(a: &[i64], b: &[i64], c: &mut [i64], m: usize, k: usize, n: usize) {
    // Multiply stays scalar; still useful to share the saxpy-shaped loop.
    match detect() {
        #[cfg(target_arch = "x86_64")]
        SimdLevel::Avx2 => unsafe { crate::x86_64::avx2::matmul_i64(a, b, c, m, k, n) },
        #[cfg(target_arch = "x86_64")]
        SimdLevel::Sse2 => unsafe { crate::x86_64::sse2::matmul_i64(a, b, c, m, k, n) },
        #[cfg(target_arch = "aarch64")]
        SimdLevel::Neon => unsafe { crate::aarch64::matmul_i64(a, b, c, m, k, n) },
        _ => scalar::matmul_i64(a, b, c, m, k, n),
    }
}

#[inline]
fn dispatch_zip_f64(
    a: &[f64],
    b: &[f64],
    out: &mut [f64],
    scalar_fn: fn(&[f64], &[f64], &mut [f64]),
    simd_fn: unsafe fn(&[f64], &[f64], &mut [f64]),
) {
    if a.len().min(b.len()).min(out.len()) < 8 {
        return scalar_fn(a, b, out);
    }
    match detect() {
        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
        SimdLevel::Avx2 | SimdLevel::Sse2 | SimdLevel::Neon => unsafe { simd_fn(a, b, out) },
        _ => scalar_fn(a, b, out),
    }
}

#[inline]
fn dispatch_zip_i64(
    a: &[i64],
    b: &[i64],
    out: &mut [i64],
    scalar_fn: fn(&[i64], &[i64], &mut [i64]),
    simd_fn: unsafe fn(&[i64], &[i64], &mut [i64]),
) {
    if a.len().min(b.len()).min(out.len()) < 8 {
        return scalar_fn(a, b, out);
    }
    match detect() {
        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
        SimdLevel::Avx2 | SimdLevel::Sse2 | SimdLevel::Neon => unsafe { simd_fn(a, b, out) },
        _ => scalar_fn(a, b, out),
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn zip_add_f64_simd(a: &[f64], b: &[f64], out: &mut [f64]) {
    match detect() {
        SimdLevel::Avx2 => crate::x86_64::avx2::zip_add_f64(a, b, out),
        _ => crate::x86_64::sse2::zip_add_f64(a, b, out),
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn zip_sub_f64_simd(a: &[f64], b: &[f64], out: &mut [f64]) {
    match detect() {
        SimdLevel::Avx2 => crate::x86_64::avx2::zip_sub_f64(a, b, out),
        _ => crate::x86_64::sse2::zip_sub_f64(a, b, out),
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn zip_add_i64_simd(a: &[i64], b: &[i64], out: &mut [i64]) {
    match detect() {
        SimdLevel::Avx2 => crate::x86_64::avx2::zip_add_i64(a, b, out),
        _ => crate::x86_64::sse2::zip_add_i64(a, b, out),
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn zip_sub_i64_simd(a: &[i64], b: &[i64], out: &mut [i64]) {
    match detect() {
        SimdLevel::Avx2 => crate::x86_64::avx2::zip_sub_i64(a, b, out),
        _ => crate::x86_64::sse2::zip_sub_i64(a, b, out),
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn zip_add_f64_simd(a: &[f64], b: &[f64], out: &mut [f64]) {
    crate::aarch64::zip_add_f64(a, b, out)
}

#[cfg(target_arch = "aarch64")]
unsafe fn zip_sub_f64_simd(a: &[f64], b: &[f64], out: &mut [f64]) {
    crate::aarch64::zip_sub_f64(a, b, out)
}

#[cfg(target_arch = "aarch64")]
unsafe fn zip_add_i64_simd(a: &[i64], b: &[i64], out: &mut [i64]) {
    crate::aarch64::zip_add_i64(a, b, out)
}

#[cfg(target_arch = "aarch64")]
unsafe fn zip_sub_i64_simd(a: &[i64], b: &[i64], out: &mut [i64]) {
    crate::aarch64::zip_sub_i64(a, b, out)
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
unsafe fn zip_add_f64_simd(a: &[f64], b: &[f64], out: &mut [f64]) {
    scalar::zip_add_f64(a, b, out)
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
unsafe fn zip_sub_f64_simd(a: &[f64], b: &[f64], out: &mut [f64]) {
    scalar::zip_sub_f64(a, b, out)
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
unsafe fn zip_add_i64_simd(a: &[i64], b: &[i64], out: &mut [i64]) {
    scalar::zip_add_i64(a, b, out)
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
unsafe fn zip_sub_i64_simd(a: &[i64], b: &[i64], out: &mut [i64]) {
    scalar::zip_sub_i64(a, b, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scalar;

    #[test]
    fn dot_f64_matches_scalar() {
        let a: Vec<f64> = (0..64).map(|i| i as f64 * 0.5).collect();
        let b: Vec<f64> = (0..64).map(|i| (i as f64).sin()).collect();
        let got = dot_f64(&a, &b);
        let expect = scalar::dot_f64(&a, &b);
        assert!((got - expect).abs() < 1e-9, "{got} vs {expect}");
    }

    #[test]
    fn matmul_f64_2x2() {
        let a = [1.0, 2.0, 3.0, 4.0];
        let b = [5.0, 6.0, 7.0, 8.0];
        let mut c = [0.0; 4];
        matmul_f64(&a, &b, &mut c, 2, 2, 2);
        assert_eq!(c, [19.0, 22.0, 43.0, 50.0]);
    }

    #[test]
    fn zip_and_neg_i64() {
        let a: Vec<i64> = (0..32).collect();
        let b: Vec<i64> = (0..32).map(|i| i * 2).collect();
        let mut add = vec![0; 32];
        let mut sub = vec![0; 32];
        let mut neg = vec![0; 32];
        zip_add_i64(&a, &b, &mut add);
        zip_sub_i64(&a, &b, &mut sub);
        zip_neg_i64(&a, &mut neg);
        for i in 0..32 {
            assert_eq!(add[i], a[i].wrapping_add(b[i]));
            assert_eq!(sub[i], a[i].wrapping_sub(b[i]));
            assert_eq!(neg[i], a[i].wrapping_neg());
        }
    }

    #[test]
    fn matmul_f64_matches_scalar_large() {
        let m = 8usize;
        let k = 8usize;
        let n = 8usize;
        let a: Vec<f64> = (0..m * k).map(|i| (i as f64) * 0.25 - 3.0).collect();
        let b: Vec<f64> = (0..k * n).map(|i| ((i % 5) as f64) - 2.0).collect();
        let mut c = vec![0.0; m * n];
        let mut expect = vec![0.0; m * n];
        matmul_f64(&a, &b, &mut c, m, k, n);
        scalar::matmul_f64(&a, &b, &mut expect, m, k, n);
        for i in 0..m * n {
            assert!(
                (c[i] - expect[i]).abs() < 1e-9,
                "mismatch at {i}: {} vs {}",
                c[i],
                expect[i]
            );
        }
    }

    #[test]
    fn matmul_i64_matches_scalar_large() {
        let m = 8usize;
        let k = 8usize;
        let n = 8usize;
        let a: Vec<i64> = (0..m * k).map(|i| (i as i64) - 10).collect();
        let b: Vec<i64> = (0..k * n).map(|i| (i as i64) % 7 - 3).collect();
        let mut c = vec![0; m * n];
        let mut expect = vec![0; m * n];
        matmul_i64(&a, &b, &mut c, m, k, n);
        scalar::matmul_i64(&a, &b, &mut expect, m, k, n);
        assert_eq!(c, expect);
    }
}
