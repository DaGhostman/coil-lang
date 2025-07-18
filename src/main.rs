use machine::{Bytecode, Machine, Opcode};

fn main() {
    // let fib = [
    //     // -- MAIN
    //     Opcode::new(Bytecode::CONST, (13 as u16).to_be_bytes()),
    //     Opcode::new(Bytecode::CALL, [6, 1]),
    //     Opcode::new(Bytecode::STORE, [0, 0]),
    //     Opcode::new(Bytecode::LOAD, [0, 0]),
    //     Opcode::new(Bytecode::PRINT, [0, 0]),
    //     Opcode::new(Bytecode::HALT, [0, 0]),
    //     // -- FIB
    //     // --- if n < 2: return n
    //     Opcode::new(Bytecode::STORE, [0, 0]),
    //     Opcode::new(Bytecode::LOAD, [0, 0]),
    //     Opcode::new(Bytecode::CONST, (2 as u16).to_be_bytes()),
    //     Opcode::new(Bytecode::LE, [0, 0]),
    //     Opcode::new(Bytecode::JMPF, [13, 0]),
    //     Opcode::new(Bytecode::LOAD, [0, 0]),
    //     Opcode::new(Bytecode::RETURN, [0, 0]),
    //
    //     // --- FIB(n - 1)
    //     Opcode::new(Bytecode::CONST, (1 as u16).to_be_bytes()),
    //     Opcode::new(Bytecode::STORE, [1, 0]),
    //     Opcode::new(Bytecode::SUB, [0, 1]),
    //     Opcode::new(Bytecode::CALL, [6, 1]),
    //     Opcode::new(Bytecode::STORE, [1, 0]),
    //
    //
    //     // --- FIB(n - 2)
    //     Opcode::new(Bytecode::CONST, (2 as u16).to_be_bytes()),
    //     Opcode::new(Bytecode::STORE, [2, 0]),
    //     Opcode::new(Bytecode::SUB, [0, 2]),
    //     Opcode::new(Bytecode::CALL, [6, 1]),
    //     Opcode::new(Bytecode::STORE, [2, 0]),
    //
    //     // --- FIB(n - 1) + FIB(n - 2)
    //     Opcode::new(Bytecode::ADD, [1, 2]),
    //     Opcode::new(Bytecode::RETURN, [0, 0]),
    // ];
    let fib = [
        // -- MAIN
        Opcode::new(Bytecode::CONST, [0, 0, 32]),
        Opcode::new(Bytecode::CALL, [5, 1, 0]),
        Opcode::new(Bytecode::STORE, [0, 0, 0]),
        // Opcode::new(Bytecode::LOAD, [0, 0]),
        Opcode::new(Bytecode::PRINT, [0, 0, 0]),
        Opcode::new(Bytecode::HALT, [0, 0, 0]),
        // -- FIB
        Opcode::new(Bytecode::STORE, [0, 0, 0]), // Argument `n`
        // Opcode::new(Bytecode::PRINT, [0, 0, 0]),
        // --- if n < 2: return n
        Opcode::new(Bytecode::CONST, [0, 0, 2]),
        Opcode::new(Bytecode::STORE, [1, 0, 0]),
        Opcode::new(Bytecode::LE, [0, 1, 2]),
        Opcode::new(Bytecode::JMPF, [11, 2, 0]),
        Opcode::new(Bytecode::RETURN, [0, 0, 0]),

        // --- FIB(n - 1)
        Opcode::new(Bytecode::CONST, [0, 0, 1]),
        Opcode::new(Bytecode::STORE, [1, 0, 0]),
        Opcode::new(Bytecode::SUB, [0, 1, 1]),
        // Opcode::new(Bytecode::PRINT, [1, 0, 0]),
        Opcode::new(Bytecode::LOAD, [1, 0, 0]),
        Opcode::new(Bytecode::CALL, [5, 1, 0]),
        Opcode::new(Bytecode::STORE, [2, 0, 0]),


        // --- FIB(n - 2)
        Opcode::new(Bytecode::CONST, [0, 0, 2]),
        Opcode::new(Bytecode::STORE, [1, 0, 0]),
        Opcode::new(Bytecode::SUB, [0, 1, 1]),
        Opcode::new(Bytecode::LOAD, [1, 0, 0]),
        Opcode::new(Bytecode::CALL, [5, 1, 0]),
        Opcode::new(Bytecode::STORE, [3, 0, 0]),

        // --- FIB(n - 1) + FIB(n - 2)
        Opcode::new(Bytecode::ADD, [2, 3, 0]),
        // Opcode::new(Bytecode::PRINT, [2, 0, 0]),
        // Opcode::new(Bytecode::PRINT, [3, 0, 0]),
        // Opcode::new(Bytecode::PRINT, [0, 0, 0]),
        
        Opcode::new(Bytecode::RETURN, [0, 0, 0]),
    ];


    // let code = [
    //     Opcode::new(Bytecode::CALL,  [26, 0]),
    //     Opcode::new(Bytecode::STORE, [10, 0]),
    //     // Opcode::new(Bytecode::PRINT, [10, 0]),
    //     Opcode::new(Bytecode::CONST, (0 as u16).to_be_bytes()),
    //     Opcode::new(Bytecode::STORE, [0, 0]),
    //     Opcode::new(Bytecode::CONST, VALUE.to_be_bytes()),
    //     Opcode::new(Bytecode::STORE, [1, 0]),
    //     Opcode::new(Bytecode::CONST, VALUE.to_be_bytes()),
    //     Opcode::new(Bytecode::STORE, [2, 0]),
    //     Opcode::new(Bytecode::CONST, VALUE.to_be_bytes()),
    //     Opcode::new(Bytecode::STORE, [3, 0]),
    //     Opcode::new(Bytecode::CONST, VALUE.to_be_bytes()),
    //     Opcode::new(Bytecode::STORE, [4, 0]),
    //     Opcode::new(Bytecode::ADD,   [1, 2]),
    //     Opcode::new(Bytecode::STORE, [1, 0]),
    //     Opcode::new(Bytecode::ADD,   [1, 3]),
    //     Opcode::new(Bytecode::STORE, [1, 0]),
    //     Opcode::new(Bytecode::ADD,   [1, 4]),
    //     Opcode::new(Bytecode::STORE, [1, 0]),
    //     Opcode::new(Bytecode::INC,   [0, 0]),
    //     Opcode::new(Bytecode::CONST, (1000 as u16).to_be_bytes()),
    //     Opcode::new(Bytecode::LOAD,  [0, 0]),
    //     Opcode::new(Bytecode::LE,    [0, 0]),
    //     Opcode::new(Bytecode::JMPF,  [6, 0]),
    //     Opcode::new(Bytecode::LOAD,  [1, 0]),
    //     Opcode::new(Bytecode::PRINT, [1, 0]),
    //     // --
    //     Opcode::new(Bytecode::HALT, [0, 0]),
    //     // --
    //     Opcode::new(Bytecode::CONST, (42 as u16).to_be_bytes()),
    //     // Opcode::new(Bytecode::STORE, [11, 0]),
    //     // Opcode::new(Bytecode::PRINT, [11, 0]),
    //     Opcode::new(Bytecode::CONST, (69 as u16).to_be_bytes()),
    //     Opcode::new(Bytecode::RETURN, [0, 0]),
    //     // --
    //     Opcode::new(Bytecode::HALT, [0, 0]),
    // ];

    Machine::<u64>::default().run(fib.as_slice())
}
