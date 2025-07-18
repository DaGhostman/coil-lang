use machine::{Bytecode, Machine, Opcode};
use std::hint::black_box;
use criterion::{criterion_group, criterion_main, Criterion, SamplingMode};

fn fib(n: u16) -> () {
    let bits = n.to_be_bytes();
    let fib = [
        // -- MAIN
        Opcode::new(Bytecode::CONST, [0, bits[0], bits[1]]),
        Opcode::new(Bytecode::CALL, [4, 1, 0]),
        Opcode::new(Bytecode::STORE, [0, 0, 0]),
        // Opcode::new(Bytecode::PRINT, [0, 0, 0]),
        Opcode::new(Bytecode::HALT, [0, 0, 0]),
        Opcode::new(Bytecode::STORE, [0, 0, 0]), // Argument `n`
        Opcode::new(Bytecode::CONST, [0, 0, 2]),
        Opcode::new(Bytecode::STORE, [1, 0, 0]),
        Opcode::new(Bytecode::LE, [0, 1, 2]),
        Opcode::new(Bytecode::JMPF, [10, 2, 0]),
        Opcode::new(Bytecode::RETURN, [0, 0, 0]),
        Opcode::new(Bytecode::CONST, [0, 0, 1]),
        Opcode::new(Bytecode::STORE, [1, 0, 0]),
        Opcode::new(Bytecode::SUB, [0, 1, 1]),
        Opcode::new(Bytecode::LOAD, [1, 0, 0]),
        Opcode::new(Bytecode::CALL, [4, 1, 0]),
        Opcode::new(Bytecode::STORE, [2, 0, 0]),
        Opcode::new(Bytecode::CONST, [0, 0, 2]),
        Opcode::new(Bytecode::STORE, [1, 0, 0]),
        Opcode::new(Bytecode::SUB, [0, 1, 1]),
        Opcode::new(Bytecode::LOAD, [1, 0, 0]),
        Opcode::new(Bytecode::CALL, [4, 1, 0]),
        Opcode::new(Bytecode::STORE, [3, 0, 0]),
        Opcode::new(Bytecode::ADD, [2, 3, 0]),
        Opcode::new(Bytecode::RETURN, [0, 0, 0]),
    ];


    Machine::<u64>::default().run(fib.as_slice())
}

fn fib_benchmark(c: &mut Criterion) {
    for n in 13..=32 {
        c.bench_function(&format!("fib #{}", n), |b| b.iter(|| fib(black_box(n))));
    }
}

criterion_group!(benches, fib_benchmark);
criterion_main!(benches);
