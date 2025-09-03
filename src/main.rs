// mod language;

use common::Value;

use compiler::Compiler;
// use common::{Registers, Types};
use machine::{Byte, Instruction, Machine};
use parser::ParserBuilder;

// use crate::language::Language;

fn main() {
    let sub = [
        Byte::new_with_value(Instruction::CONST, Value::from(6)),
        Byte::new_with_value(Instruction::CONST, Value::from(2)),
        Byte::new(Instruction::SUB),
        Byte::new(Instruction::PRINTI),
        Byte::new(Instruction::HALT),
    ];
    let mul = [
        Byte::new_with_value(Instruction::CONST, Value::from(6.0)),
        Byte::new_with_value(Instruction::CONST, Value::from(2.0)),
        Byte::new(Instruction::MULF),
        Byte::new(Instruction::PRINTF),
        Byte::new(Instruction::HALT),
    ];
    let async_counters = [
        Byte::new_with_operands(Instruction::JMP, [29, 0]),
        Byte::new_with_operands(Instruction::LOAD, [1, 0]), // Body + 1
        Byte::new(Instruction::LOAD),
        Byte::new(Instruction::LE),
        Byte::new_with_operands(Instruction::JMPF, [13, 0]),
        Byte::new_with_operands(Instruction::LOAD, [1, 0]),
        Byte::new_with_value(Instruction::CONST, Value::from(1)),
        Byte::new(Instruction::ADD),
        Byte::new_with_operands(Instruction::STORE, [1, 0]),
        Byte::new_with_operands(Instruction::LOAD, [1, 0]),
        Byte::new(Instruction::PRINTI),
        Byte::new(Instruction::SUSP),
        Byte::new_with_operands(Instruction::JMP, [1, 0]),
        Byte::new_with_operands(Instruction::LOAD, [1, 0]),
        Byte::new(Instruction::RETURN),
        Byte::new_with_operands(Instruction::LOAD, [1, 0]), // Body + 4
        Byte::new_with_operands(Instruction::LOAD, [0, 0]),
        Byte::new(Instruction::LE),
        Byte::new_with_operands(Instruction::JMPF, [27, 0]),
        Byte::new_with_operands(Instruction::LOAD, [1, 0]),
        Byte::new_with_value(Instruction::CONST, Value::from(4)),
        Byte::new(Instruction::MUL),
        Byte::new_with_operands(Instruction::STORE, [1, 0]),
        Byte::new_with_operands(Instruction::LOAD, [1, 0]),
        Byte::new(Instruction::PRINTI),
        Byte::new(Instruction::SUSP),
        Byte::new_with_operands(Instruction::JMP, [15, 0]),
        Byte::new_with_operands(Instruction::LOAD, [1, 0]),
        Byte::new(Instruction::RETURN),
        Byte::new_with_value(Instruction::CONST, Value::from(0)),
        Byte::new_with_value(Instruction::CONST, Value::from(10)),
        Byte::new_with_operands(Instruction::CALL, [1, 2]), // Calls
        Byte::new(Instruction::PRINTI),
        Byte::new_with_value(Instruction::CONST, Value::from(1)),
        Byte::new_with_value(Instruction::CONST, Value::from(10)),
        Byte::new_with_operands(Instruction::CALL, [15, 2]), // Calls
        Byte::new(Instruction::RESUME),
        Byte::new(Instruction::RESUME),
        Byte::new(Instruction::PRINTI),
        Byte::new(Instruction::HALT),
    ];

    let fib = [
        // Byte::new_with(Instruction::CONST, [0, 0], Value::new(1, 32u64)),
        // Byte::new(Instruction::STRING, [0, 1]),
        // Byte::new(Instruction::DATA, [97, 0]),
        // Byte::new(Instruction::DATA, [0, 0]),
        // Byte::new(Instruction::PRINT, [0, 0]),
        // Byte::new(Instruction::HALT, [0, 0]),
        Byte::new_with_value(Instruction::CONST, Value::from(32)),
        Byte::new_with_operands(Instruction::CALL, [4, 1]),
        Byte::new(Instruction::PRINTI),
        Byte::new(Instruction::HALT),
        //
        Byte::new(Instruction::LOAD), // Load argument n
        Byte::new_with_value(Instruction::CONST, Value::from(2)), // Load 2
        Byte::new(Instruction::LE),   // Compare n < 2
        Byte::new_with_operands(Instruction::JMPF, [10, 0]), // Jump if false
        Byte::new_with_operands(Instruction::LOAD, [0, 0]),
        Byte::new(Instruction::RETURN), // Return n
        // -- FIB
        Byte::new_with_operands(Instruction::LOAD, [0, 0]), // Load n
        Byte::new_with_value(Instruction::CONST, Value::from(1)), // Load 1
        Byte::new(Instruction::SUB),                        // n - 1
        Byte::new_with_operands(Instruction::CALL, [4, 1]), // Call FIB(n - 1)
        Byte::new_with_operands(Instruction::LOAD, [0, 0]), // Store result
        Byte::new_with_value(Instruction::CONST, Value::from(2)), // Load 2
        Byte::new(Instruction::SUB),                        // n - 2
        Byte::new_with_operands(Instruction::CALL, [4, 1]), // Call FIB(n - 2)
        // Opcode::new(Bytecode::STORE, [2, 0, 0]),  // Store result
        Byte::new(Instruction::ADD),    // Add results
        Byte::new(Instruction::RETURN), // Return result;
    ];

    // let mut language = Language::default();
    // language.run_bytecode(&fib);

    let argc = std::env::args().collect::<Vec<_>>();
    let src = std::fs::read_to_string(argc[1].as_str()).unwrap();

    let mut compiler = Compiler::default();
    match ParserBuilder::new().parse(argc[1].clone(), src.as_str()) {
        Ok(ast) => {
            // dbg!(&ast, &compiler.compile(&ast));
            Machine::<256>::default().run(&compiler.compile(&ast));
        }
        Err(err) => (),
    };

    // vm.run(fib.as_slice());
}
