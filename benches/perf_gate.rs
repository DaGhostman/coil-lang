//! Phase 1 perf gate (see AGENTS.md): a Criterion benchmark that exercises
//! the full pipeline — compile `examples/fib.0s` from source, then run the
//! resulting bytecode on the VM. Fails the benchmark (with a non-zero
//! exit code from `cargo bench`) if fib(32) drifts past the budget.
//!
//! Run with `cargo bench --bench perf_gate`. Override the budget locally
//! with `PERF_GATE_BUDGET_MS=50 cargo bench --bench perf_gate`.

use std::time::Instant;

use criterion::{Criterion, criterion_group, criterion_main};
use machine::Machine;
use compiler::Pipeline;
use common::Byte;

const DEFAULT_BUDGET_MS: u128 = 80;

fn budget_us() -> u128 {
    std::env::var("PERF_GATE_BUDGET_MS")
        .ok()
        .and_then(|s| s.parse::<u128>().ok())
        .unwrap_or(DEFAULT_BUDGET_MS)
        * 1000
}

/// Read fib.0s and compile via Pipeline::compile_src.
fn compile_fib() -> Vec<Byte> {
    let src = std::fs::read_to_string("examples/fib.0s")
        .expect("examples/fib.0s must exist");
    let mut pipeline = Pipeline::new();
    pipeline
        .compile_src(&src)
        .expect("fib.0s must compile cleanly for the perf gate")
}

fn time_fib32_run() -> u128 {
    let code = compile_fib();
    // Warm up: first call pays for rkyv setup + initial GC.
    for _ in 0..3 {
        let mut vm = Machine::<512>::default();
        let _ = vm.run_raw(&code);
    }
    let n_iters: usize = 5;
    let start = Instant::now();
    for _ in 0..n_iters {
        let mut vm = Machine::<512>::default();
        vm.run_raw(&code);
    }
    start.elapsed().as_micros() / n_iters as u128
}

fn perf_gate(c: &mut Criterion) {
    let _ = time_fib32_run();
    c.bench_function("fib32_compile_and_run", |b| {
        b.iter(|| {
            let us = time_fib32_run();
            std::hint::black_box(us);
        });
    });
}

fn assert_gate(_c: &mut Criterion) {
    let us = time_fib32_run();
    let budget = budget_us();
    if us > budget {
        eprintln!(
            "\n\x1b[31mPERF GATE FAILED:\x1b[0m fib(32) took {} µs ({:.2} ms), \
             budget {} µs ({:.2} ms).\n\
             Run `cargo bench --bench perf_gate` for details.",
            us, us as f64 / 1000.0, budget, budget as f64 / 1000.0
        );
        panic!("fib(32) regressed past the performance gate");
    } else {
        eprintln!(
            "\x1b[32mPERF GATE OK:\x1b[0m fib(32) = {} µs ({:.2} ms), \
             budget {} µs ({:.2} ms)",
            us, us as f64 / 1000.0, budget, budget as f64 / 1000.0
        );
    }
}

criterion_group! {
    name = gates;
    config = Criterion::default().sample_size(10);
    targets = perf_gate, assert_gate
}
criterion_main!(gates);
