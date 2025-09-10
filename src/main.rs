// use common::{Byte, Instruction, Value};
use compiler::{Pipeline};
use machine::{Machine};


fn main() {
    let argc = std::env::args().collect::<Vec<_>>();
    let filename = argc[1].clone();

    let pipeline = Pipeline::new();
    // let fib = [
    //     Byte::new_with_value(Instruction::CONST, Value::from(32 as i64)),
    //     Byte::new_with_operands(Instruction::CALL, [3, 1]),
    //     Byte::new(Instruction::HALT),
    //     //
    //     Byte::new_with_operands(Instruction::LOAD, [0, 0]), // Load argument n
    //     Byte::new_with_value(Instruction::CONST, Value::from(2)), // Load 2
    //     Byte::new(Instruction::LE),                         // Compare n < 2
    //     Byte::new_with_operands(Instruction::JMPF, [9, 0]), // Jump if false
    //     Byte::new_with_operands(Instruction::LOAD, [0, 0]),
    //     Byte::new(Instruction::RETURN), // Return n
    //     // -- FIB
    //     Byte::new_with_operands(Instruction::LOAD, [0, 0]), // Load n
    //     Byte::new_with_value(Instruction::CONST, Value::from(1)), // Load 1
    //     Byte::new(Instruction::SUB),                        // n - 1
    //     Byte::new_with_operands(Instruction::CALL, [3, 1]), // Call FIB(n - 1)
    //     Byte::new_with_operands(Instruction::LOAD, [0, 0]), // Store result
    //     Byte::new_with_value(Instruction::CONST, Value::from(2)), // Load 2
    //     Byte::new(Instruction::SUB),                        // n - 2
    //     Byte::new_with_operands(Instruction::CALL, [3, 1]), // Call FIB(n - 2)
    //     // Opcode::new(Bytecode::STORE, [2, 0, 0]),  // Store result
    //     Byte::new(Instruction::ADD),    // Add results
    //     Byte::new(Instruction::RETURN), // Return result;
    // ];
    // Machine::<64>::default().run(&fib);

    if let Ok(bytecode) = pipeline.run(filename) {
        Machine::<256>::default().run(&bytecode);
    }
}
