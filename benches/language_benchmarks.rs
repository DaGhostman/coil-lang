use criterion::{Criterion, black_box, criterion_group, criterion_main};

use common::memory::table::Table;

pub fn insertion_benchmark(c: &mut Criterion) {
    c.bench_function("custom table insert", |b| {
        let mut table = Table::new();

        b.iter(|| {
            table.insert("key", 12);
        });
    });
    c.bench_function("rustc table insert", |b| {
        let mut table = rustc_hash::FxHashMap::default();

        b.iter(|| {
            table.insert("key", 12);
        });
    });
}

pub fn retrieval_benchmark(c: &mut Criterion) {
    c.bench_function("custom table fetch", |b| {
        let mut table = Table::new();
        table.insert("key", 12);

        b.iter(|| {
            assert!(Some(12) == table.get("key").copied());
        });
    });
    c.bench_function("rustc table fetch", |b| {
        let mut table = rustc_hash::FxHashMap::default();
        table.insert("key", 12);

        b.iter(|| {
            assert!(Some(12) == table.get("key").copied());
        });
    });
}

pub fn stack_storage(c: &mut Criterion) {
    c.bench_function("Native stack", |b| {
        let mut stack = [0; 1024];
        stack[420] = 69;

        b.iter(black_box(|| {
            assert!(69 == stack[420]);
        }));
    });

    c.bench_function("Vec stack", |b| {
        let mut stack = vec![0; 1024];
        stack[420] = 69;

        b.iter(black_box(|| {
            assert!(69 == stack[420]);
        }));
    });
}

criterion_group!(
    benches,
    insertion_benchmark,
    retrieval_benchmark,
    stack_storage
);
criterion_main!(benches);
