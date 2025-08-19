use common::Value;
// use common::{Registers, Types};
use machine::{Byte, Instruction, Machine};

fn main() {
    let fib = [
        // Byte::new_with(Instruction::CONST, [0, 0], Value::new(1, 32u64)),
        // Byte::new(Instruction::STRING, [0, 1]),
        // Byte::new(Instruction::DATA, [97, 0]),
        // Byte::new(Instruction::DATA, [0, 0]),
        // Byte::new(Instruction::PRINT, [0, 0]),
        // Byte::new(Instruction::HALT, [0, 0]),
        Byte::new_with(Instruction::CONST, [0, 0], Value::int(32)),
        Byte::new(Instruction::CALL, [4, 1]),
        Byte::new(Instruction::PRINT, [0, 0]),
        Byte::new(Instruction::HALT, [0, 0]),
        //
        Byte::new(Instruction::LOAD, [0, 0]), // Load argument n
        Byte::new_with(Instruction::CONST, [0, 0], Value::int(2)), // Load 2
        Byte::new(Instruction::LE, [0, 0]),   // Compare n < 2
        Byte::new(Instruction::JMPF, [10, 0]), // Jump if false
        Byte::new(Instruction::LOAD, [0, 0]),
        Byte::new(Instruction::RETURN, [0, 0]), // Return n
        // -- FIB
        Byte::new(Instruction::LOAD, [0, 0]), // Load n
        Byte::new_with(Instruction::CONST, [0, 0], Value::int(1)), // Load 1
        Byte::new(Instruction::SUB, [0, 0]),  // n - 1
        Byte::new(Instruction::CALL, [4, 1]), // Call FIB(n - 1)
        Byte::new(Instruction::LOAD, [0, 0]), // Store result
        Byte::new_with(Instruction::CONST, [0, 0], Value::int(2)), // Load 2
        Byte::new(Instruction::SUB, [0, 0]),  // n - 2
        Byte::new(Instruction::CALL, [4, 1]), // Call FIB(n - 2)
        // Opcode::new(Bytecode::STORE, [2, 0, 0]),  // Store result
        Byte::new(Instruction::ADD, [1, 0]), // Add results
        Byte::new(Instruction::RETURN, [0, 0]), // Return result;
                                             //
    ];

    let mut vm = Machine::default();
    vm.run(fib.as_slice());
}
