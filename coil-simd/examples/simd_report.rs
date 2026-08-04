//! Print SIMD vs scalar wall-clock speedups for coil-simd kernels.
//!
//! ```text
//! cargo run -p coil-simd --release --example simd_report
//! ```

use std::hint::black_box;
use std::time::Instant;

use coil_simd::{self, detect, scalar};

fn fill_f64(n: usize, seed: f64) -> Vec<f64> {
    (0..n).map(|i| (i as f64) * seed + 0.1).collect()
}

fn fill_i64(n: usize, seed: i64) -> Vec<i64> {
    (0..n)
        .map(|i| (i as i64).wrapping_mul(seed).wrapping_sub(3))
        .collect()
}

fn time_ns(iters: u32, mut body: impl FnMut()) -> f64 {
    // Warmup
    for _ in 0..iters.min(32) {
        body();
    }
    let t0 = Instant::now();
    for _ in 0..iters {
        body();
    }
    t0.elapsed().as_secs_f64() * 1e9 / f64::from(iters)
}

fn row(name: &str, scalar_ns: f64, simd_ns: f64) {
    let speedup = scalar_ns / simd_ns;
    println!(
        "| `{name}` | {scalar_ns:.1} | {simd_ns:.1} | **{speedup:.2}×** |"
    );
}

fn main() {
    let level = detect();
    println!("# coil-simd: scalar vs SIMD");
    println!();
    println!("- host `SimdLevel`: `{level:?}`");
    println!("- build: release");
    println!("- times: median-ish mean ns/iter over fixed iteration counts");
    println!();
    println!("| Kernel | Scalar ns/iter | SIMD ns/iter | Speedup |");
    println!("|--------|---------------:|-------------:|--------:|");

    for &n in &[64usize, 256, 1024, 4096, 16384] {
        let a = fill_f64(n, 0.5);
        let b = fill_f64(n, 0.3);
        let iters = if n <= 256 {
            200_000
        } else if n <= 4096 {
            50_000
        } else {
            10_000
        };
        let s = time_ns(iters, || {
            black_box(scalar::dot_f64(black_box(&a), black_box(&b)));
        });
        let v = time_ns(iters, || {
            black_box(coil_simd::dot_f64(black_box(&a), black_box(&b)));
        });
        row(&format!("dot_f64/{n}"), s, v);
    }

    for &n in &[64usize, 256, 1024, 4096, 16384] {
        let a = fill_i64(n, 7);
        let b = fill_i64(n, 3);
        let iters = if n <= 256 {
            200_000
        } else if n <= 4096 {
            50_000
        } else {
            10_000
        };
        let s = time_ns(iters, || {
            black_box(scalar::dot_i64(black_box(&a), black_box(&b)));
        });
        let v = time_ns(iters, || {
            black_box(coil_simd::dot_i64(black_box(&a), black_box(&b)));
        });
        row(&format!("dot_i64/{n}"), s, v);
    }

    for &n in &[64usize, 256, 1024, 4096, 16384] {
        let a = fill_f64(n, 0.5);
        let b = fill_f64(n, 0.3);
        let mut out_s = vec![0.0; n];
        let mut out_v = vec![0.0; n];
        let iters = if n <= 256 {
            100_000
        } else if n <= 4096 {
            30_000
        } else {
            8_000
        };
        let s = time_ns(iters, || {
            scalar::zip_add_f64(black_box(&a), black_box(&b), black_box(&mut out_s));
            black_box(&out_s);
        });
        let v = time_ns(iters, || {
            coil_simd::zip_add_f64(black_box(&a), black_box(&b), black_box(&mut out_v));
            black_box(&out_v);
        });
        row(&format!("zip_add_f64/{n}"), s, v);
    }

    for &dim in &[16usize, 32, 64, 128] {
        let m = dim;
        let k = dim;
        let n = dim;
        let a = fill_f64(m * k, 0.25);
        let b = fill_f64(k * n, 0.11);
        let mut c_s = vec![0.0; m * n];
        let mut c_v = vec![0.0; m * n];
        let iters = match dim {
            16 => 50_000,
            32 => 10_000,
            64 => 2_000,
            _ => 400,
        };
        let s = time_ns(iters, || {
            scalar::matmul_f64(
                black_box(&a),
                black_box(&b),
                black_box(&mut c_s),
                m,
                k,
                n,
            );
            black_box(&c_s);
        });
        let v = time_ns(iters, || {
            coil_simd::matmul_f64(
                black_box(&a),
                black_box(&b),
                black_box(&mut c_v),
                m,
                k,
                n,
            );
            black_box(&c_v);
        });
        row(&format!("matmul_f64/{dim}x{dim}"), s, v);
    }

    for &dim in &[16usize, 32, 64, 128] {
        let m = dim;
        let k = dim;
        let n = dim;
        let a = fill_i64(m * k, 5);
        let b = fill_i64(k * n, 3);
        let mut c_s = vec![0; m * n];
        let mut c_v = vec![0; m * n];
        let iters = match dim {
            16 => 50_000,
            32 => 10_000,
            64 => 2_000,
            _ => 400,
        };
        let s = time_ns(iters, || {
            scalar::matmul_i64(
                black_box(&a),
                black_box(&b),
                black_box(&mut c_s),
                m,
                k,
                n,
            );
            black_box(&c_s);
        });
        let v = time_ns(iters, || {
            coil_simd::matmul_i64(
                black_box(&a),
                black_box(&b),
                black_box(&mut c_v),
                m,
                k,
                n,
            );
            black_box(&c_v);
        });
        row(&format!("matmul_i64/{dim}x{dim}"), s, v);
    }

    for &n in &[64usize, 1024, 4096, 65536] {
        let a = vec![0xA5u8; n];
        let b = a.clone();
        let iters = if n <= 1024 { 200_000 } else { 40_000 };
        let s = time_ns(iters, || {
            black_box(black_box(a.as_slice()) == black_box(b.as_slice()));
        });
        let v = time_ns(iters, || {
            black_box(coil_simd::bytes::eq(black_box(&a), black_box(&b)));
        });
        row(&format!("bytes_eq/{n}"), s, v);
    }

    println!();
    println!("Notes:");
    println!("- `simd/*` is the public runtime-dispatched API (AVX-512 on this host when available).");
    println!("- `scalar/*` calls `coil_simd::scalar` directly (no ISA intrinsics).");
    println!("- Tiny inputs may intentionally use scalar inside the public API (overhead guard).");
    println!("- `bytes_eq` compares against Rust slice `==` (already heavily optimized / libc).");
}
