use common::Value;
use criterion::{Criterion, SamplingMode, criterion_group, criterion_main};
use machine::{Byte, Instruction, Machine};
use std::hint::black_box;

fn fib(n: u16) -> () {
    let fib = [
        Byte::new_with(Instruction::CONST, [0, 0], Value::from(n as i64)),
        Byte::new(Instruction::CALL, [3, 1]),
        Byte::new(Instruction::HALT, [0, 0]),
        //
        Byte::new(Instruction::LOAD, [0, 0]), // Load argument n
        Byte::new_with(Instruction::CONST, [0, 0], Value::from(2)), // Load 2
        Byte::new(Instruction::LE, [0, 0]),   // Compare n < 2
        Byte::new(Instruction::JMPF, [9, 0]), // Jump if false
        Byte::new(Instruction::LOAD, [0, 0]),
        Byte::new(Instruction::RETURN, [0, 0]), // Return n
        // -- FIB
        Byte::new(Instruction::LOAD, [0, 0]), // Load n
        Byte::new_with(Instruction::CONST, [0, 0], Value::from(1)), // Load 1
        Byte::new(Instruction::SUB, [0, 0]),  // n - 1
        Byte::new(Instruction::CALL, [3, 1]), // Call FIB(n - 1)
        Byte::new(Instruction::LOAD, [0, 0]), // Store result
        Byte::new_with(Instruction::CONST, [0, 0], Value::from(2)), // Load 2
        Byte::new(Instruction::SUB, [0, 0]),  // n - 2
        Byte::new(Instruction::CALL, [3, 1]), // Call FIB(n - 2)
        // Opcode::new(Bytecode::STORE, [2, 0, 0]),  // Store result
        Byte::new(Instruction::ADD, [0, 0]),    // Add results
        Byte::new(Instruction::RETURN, [0, 0]), // Return result;
    ];

    Machine::<512>::default().run(fib.as_slice())
}

fn fib_benchmark(c: &mut Criterion) {
    for n in 13..=32 {
        c.bench_function(&format!("fib #{}", n), |b| b.iter(|| fib(black_box(n))));
    }
}

criterion_group!(benches, fib_benchmark);
criterion_main!(benches);
