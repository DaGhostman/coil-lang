//! Criterion benches: scalar reference vs runtime-dispatched SIMD kernels.
//!
//! Run: `cargo bench -p coil-simd --bench simd_vs_scalar`

use std::hint::black_box;
use std::time::Instant;

use coil_simd::{self, SimdLevel, detect, scalar};
use criterion::{Criterion, criterion_group, criterion_main};

fn fill_f64(n: usize, seed: f64) -> Vec<f64> {
    (0..n).map(|i| (i as f64) * seed + 0.1).collect()
}

fn fill_i64(n: usize, seed: i64) -> Vec<i64> {
    (0..n).map(|i| (i as i64).wrapping_mul(seed).wrapping_sub(3)).collect()
}

fn bench_dot_f64(c: &mut Criterion) {
    let mut group = c.benchmark_group("dot_f64");
    for &n in &[64usize, 256, 1024, 4096, 16384] {
        let a = fill_f64(n, 0.5);
        let b = fill_f64(n, 0.3);
        group.bench_function(format!("scalar/{n}"), |ben| {
            ben.iter(|| black_box(scalar::dot_f64(black_box(&a), black_box(&b))))
        });
        group.bench_function(format!("simd/{n}"), |ben| {
            ben.iter(|| black_box(coil_simd::dot_f64(black_box(&a), black_box(&b))))
        });
    }
    group.finish();
}

fn bench_dot_i64(c: &mut Criterion) {
    let mut group = c.benchmark_group("dot_i64");
    for &n in &[64usize, 256, 1024, 4096, 16384] {
        let a = fill_i64(n, 7);
        let b = fill_i64(n, 3);
        group.bench_function(format!("scalar/{n}"), |ben| {
            ben.iter(|| black_box(scalar::dot_i64(black_box(&a), black_box(&b))))
        });
        group.bench_function(format!("simd/{n}"), |ben| {
            ben.iter(|| black_box(coil_simd::dot_i64(black_box(&a), black_box(&b))))
        });
    }
    group.finish();
}

fn bench_zip_add_f64(c: &mut Criterion) {
    let mut group = c.benchmark_group("zip_add_f64");
    for &n in &[64usize, 256, 1024, 4096, 16384] {
        let a = fill_f64(n, 0.5);
        let b = fill_f64(n, 0.3);
        let mut out_s = vec![0.0; n];
        let mut out_v = vec![0.0; n];
        group.bench_function(format!("scalar/{n}"), |ben| {
            ben.iter(|| {
                scalar::zip_add_f64(black_box(&a), black_box(&b), black_box(&mut out_s));
                black_box(&out_s);
            })
        });
        group.bench_function(format!("simd/{n}"), |ben| {
            ben.iter(|| {
                coil_simd::zip_add_f64(black_box(&a), black_box(&b), black_box(&mut out_v));
                black_box(&out_v);
            })
        });
    }
    group.finish();
}

fn bench_zip_add_i64(c: &mut Criterion) {
    let mut group = c.benchmark_group("zip_add_i64");
    for &n in &[64usize, 256, 1024, 4096, 16384] {
        let a = fill_i64(n, 7);
        let b = fill_i64(n, 3);
        let mut out_s = vec![0; n];
        let mut out_v = vec![0; n];
        group.bench_function(format!("scalar/{n}"), |ben| {
            ben.iter(|| {
                scalar::zip_add_i64(black_box(&a), black_box(&b), black_box(&mut out_s));
                black_box(&out_s);
            })
        });
        group.bench_function(format!("simd/{n}"), |ben| {
            ben.iter(|| {
                coil_simd::zip_add_i64(black_box(&a), black_box(&b), black_box(&mut out_v));
                black_box(&out_v);
            })
        });
    }
    group.finish();
}

fn bench_zip_mul(c: &mut Criterion) {
    let mut group = c.benchmark_group("zip_mul");
    for &n in &[256usize, 4096] {
        let af = fill_f64(n, 0.5);
        let bf = fill_f64(n, 0.3);
        let ai = fill_i64(n, 7);
        let bi = fill_i64(n, 3);
        let mut of_s = vec![0.0; n];
        let mut of_v = vec![0.0; n];
        let mut oi_s = vec![0; n];
        let mut oi_v = vec![0; n];
        group.bench_function(format!("f64/scalar/{n}"), |ben| {
            ben.iter(|| {
                scalar::zip_mul_f64(black_box(&af), black_box(&bf), black_box(&mut of_s));
                black_box(&of_s);
            })
        });
        group.bench_function(format!("f64/simd/{n}"), |ben| {
            ben.iter(|| {
                coil_simd::zip_mul_f64(black_box(&af), black_box(&bf), black_box(&mut of_v));
                black_box(&of_v);
            })
        });
        group.bench_function(format!("i64/scalar/{n}"), |ben| {
            ben.iter(|| {
                scalar::zip_mul_i64(black_box(&ai), black_box(&bi), black_box(&mut oi_s));
                black_box(&oi_s);
            })
        });
        group.bench_function(format!("i64/simd/{n}"), |ben| {
            ben.iter(|| {
                coil_simd::zip_mul_i64(black_box(&ai), black_box(&bi), black_box(&mut oi_v));
                black_box(&oi_v);
            })
        });
    }
    group.finish();
}

fn bench_matmul_f64(c: &mut Criterion) {
    let mut group = c.benchmark_group("matmul_f64");
    for &dim in &[16usize, 32, 64, 128] {
        let m = dim;
        let k = dim;
        let n = dim;
        let a = fill_f64(m * k, 0.25);
        let b = fill_f64(k * n, 0.11);
        let mut c_s = vec![0.0; m * n];
        let mut c_v = vec![0.0; m * n];
        group.bench_function(format!("scalar/{dim}x{dim}"), |ben| {
            ben.iter(|| {
                scalar::matmul_f64(
                    black_box(&a),
                    black_box(&b),
                    black_box(&mut c_s),
                    m,
                    k,
                    n,
                );
                black_box(&c_s);
            })
        });
        group.bench_function(format!("simd/{dim}x{dim}"), |ben| {
            ben.iter(|| {
                coil_simd::matmul_f64(
                    black_box(&a),
                    black_box(&b),
                    black_box(&mut c_v),
                    m,
                    k,
                    n,
                );
                black_box(&c_v);
            })
        });
    }
    group.finish();
}

fn bench_matmul_i64(c: &mut Criterion) {
    let mut group = c.benchmark_group("matmul_i64");
    for &dim in &[16usize, 32, 64, 128] {
        let m = dim;
        let k = dim;
        let n = dim;
        let a = fill_i64(m * k, 5);
        let b = fill_i64(k * n, 3);
        let mut c_s = vec![0; m * n];
        let mut c_v = vec![0; m * n];
        group.bench_function(format!("scalar/{dim}x{dim}"), |ben| {
            ben.iter(|| {
                scalar::matmul_i64(
                    black_box(&a),
                    black_box(&b),
                    black_box(&mut c_s),
                    m,
                    k,
                    n,
                );
                black_box(&c_s);
            })
        });
        group.bench_function(format!("simd/{dim}x{dim}"), |ben| {
            ben.iter(|| {
                coil_simd::matmul_i64(
                    black_box(&a),
                    black_box(&b),
                    black_box(&mut c_v),
                    m,
                    k,
                    n,
                );
                black_box(&c_v);
            })
        });
    }
    group.finish();
}

fn bench_bytes_eq(c: &mut Criterion) {
    let mut group = c.benchmark_group("bytes_eq");
    for &n in &[64usize, 256, 1024, 4096, 65536] {
        let a = vec![0xA5u8; n];
        let mut b = a.clone();
        // Keep equal for the steady path; mismatch would exit early.
        b[n - 1] = 0xA5;
        group.bench_function(format!("slice_eq/{n}"), |ben| {
            ben.iter(|| black_box(black_box(&a) == black_box(&b)))
        });
        group.bench_function(format!("simd/{n}"), |ben| {
            ben.iter(|| black_box(coil_simd::bytes::eq(black_box(&a), black_box(&b))))
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_dot_f64,
    bench_dot_i64,
    bench_zip_add_f64,
    bench_zip_add_i64,
    bench_zip_mul,
    bench_matmul_f64,
    bench_matmul_i64,
    bench_bytes_eq
);
criterion_main!(benches);

/// Quick wall-clock summary printed when running the harness binary helpers.
#[allow(dead_code)]
fn print_manual_summary() {
    let level = detect();
    eprintln!("SimdLevel = {level:?}");
    let n = 4096usize;
    let a = fill_f64(n, 0.5);
    let b = fill_f64(n, 0.3);
    let t0 = Instant::now();
    for _ in 0..10_000 {
        black_box(scalar::dot_f64(&a, &b));
    }
    let scalar_ns = t0.elapsed();
    let t1 = Instant::now();
    for _ in 0..10_000 {
        black_box(coil_simd::dot_f64(&a, &b));
    }
    let simd_ns = t1.elapsed();
    eprintln!(
        "dot_f64/{n}: scalar={scalar_ns:?} simd={simd_ns:?} speedup={:.2}x",
        scalar_ns.as_secs_f64() / simd_ns.as_secs_f64()
    );
    let _ = SimdLevel::Scalar;
}
