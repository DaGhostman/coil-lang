use common::{ArchivedByte as Byte, ArchivedInstruction as Instruction, Value};
use criterion::{Criterion, SamplingMode, criterion_group, criterion_main};
use machine::Machine;
use std::hint::black_box;

fn fib(n: u16) -> () {
    let fib = [
        Byte::new(Instruction::CONST).with_value(Value::from(n as i64)),
        Byte::new(Instruction::CALL).with_operand_u32(1),
        Byte::new(Instruction::JMP).with_operand_u32(4),
        Byte::new(Instruction::HALT),
        //
        Byte::new(Instruction::LOAD).with_operand_u32(0), // Load argument n
        Byte::new(Instruction::CONST).with_value(Value::from(2)), // Load 2
        Byte::new(Instruction::LE),                       // Compare n < 2
        Byte::new(Instruction::JMPF).with_operand_u32(10), // Jump if false
        Byte::new(Instruction::LOAD).with_operand_u32(0),
        Byte::new(Instruction::RETURN), // Return n
        // -- FIB
        Byte::new(Instruction::LOAD).with_operand_u32(0), // Load n
        Byte::new(Instruction::CONST).with_value(Value::from(1)), // Load 1
        Byte::new(Instruction::SUB),                      // n - 1
        Byte::new(Instruction::CALL).with_operand_u32(1), // Call FIB(n - 1)
        Byte::new(Instruction::JMP).with_operand_u32(4),
        Byte::new(Instruction::LOAD).with_operand_u32(0), // Store result
        Byte::new(Instruction::CONST).with_value(Value::from(2)), // Load 2
        Byte::new(Instruction::SUB),                      // n - 2
        Byte::new(Instruction::CALL).with_operand_u32(1), // Call FIB(n - 2)
        Byte::new(Instruction::JMP).with_operand_u32(4),
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
