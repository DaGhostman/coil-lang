use common::Value;
use criterion::{Criterion, SamplingMode, criterion_group, criterion_main};
use machine::{Byte, Instruction, Machine};
use std::hint::black_box;

fn fib(n: u16) -> () {
    let fib = [
        Byte::new_with_value(Instruction::CONST, Value::from(n as i64)),
        Byte::new_with_operands(Instruction::CALL, [3, 1]),
        Byte::new(Instruction::HALT),
        //
        Byte::new_with_operands(Instruction::LOAD, [0, 0]), // Load argument n
        Byte::new_with_value(Instruction::CONST, Value::from(2)), // Load 2
        Byte::new(Instruction::LE),                         // Compare n < 2
        Byte::new_with_operands(Instruction::JMPF, [9, 0]), // Jump if false
        Byte::new_with_operands(Instruction::LOAD, [0, 0]),
        Byte::new(Instruction::RETURN), // Return n
        // -- FIB
        Byte::new_with_operands(Instruction::LOAD, [0, 0]), // Load n
        Byte::new_with_value(Instruction::CONST, Value::from(1)), // Load 1
        Byte::new(Instruction::SUB),                        // n - 1
        Byte::new_with_operands(Instruction::CALL, [3, 1]), // Call FIB(n - 1)
        Byte::new_with_operands(Instruction::LOAD, [0, 0]), // Store result
        Byte::new_with_value(Instruction::CONST, Value::from(2)), // Load 2
        Byte::new(Instruction::SUB),                        // n - 2
        Byte::new_with_operands(Instruction::CALL, [3, 1]), // Call FIB(n - 2)
        // Opcode::new(Bytecode::STORE, [2, 0, 0]),  // Store result
        Byte::new(Instruction::ADD),    // Add results
        Byte::new(Instruction::RETURN), // Return result;
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
