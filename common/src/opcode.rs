use std::fmt::{Debug};

use crate::promise;

#[repr(u8)]
#[derive(Debug, Default, Copy, Clone)]
pub enum Instruction {
    // -- Special
    #[default]
    HALT,
    NOOP,
    POP,
    CONST,
    STORE,
    LOAD,
    CALL,
    RETURN,
    JMP,
    JMPT,
    JMPF,
    STRING,
    DATA,
    ACQUIRE,
    RELEASE,

    // Arithmetic
    ADD,
    SUB,
    MUL,
    DIV,
    MOD,
    // INC,
    // DEC,
    ADDF,
    SUBF,
    MULF,
    DIVF,
    MODF,
    NOT,
    NEG,
    AND,
    OR,
    SHL,
    SHR,
    XOR,
    EQ,
    NEQ,
    LE,
    LEF,
    GT,
    GTF,
    // -- Keyword
    PRINT,
    FORMAT,
    STRINGIFY,
    NATIVE,
    RESUME,
    SUSP,
    FREE,
}

impl From<u8> for Instruction {
    fn from(value: u8) -> Self {
        unsafe { std::mem::transmute(value) }
    }
}

impl From<Instruction> for u8 {
    fn from(val: Instruction) -> Self {
        val as u8
    }
}

type OPERAND = usize;
const OPERAND_COUNT: usize = 2;
#[derive(Default, Clone, Copy)]
pub struct Byte<V> {
    bytecode: Instruction,
    operands: [OPERAND; OPERAND_COUNT],
    value: V,
}

impl<V: Default + Copy> Byte<V> {
    pub fn new(bytecode: Instruction) -> Self {
        Self {
            bytecode,
            operands: Default::default(),
            value: V::default(),
        }
    }

    pub fn new_with_operands(bytecode: Instruction, operands: [OPERAND; OPERAND_COUNT]) -> Self {
        Self {
            bytecode,
            operands,
            value: V::default(),
        }
    }

    pub fn new_with_value(bytecode: Instruction, value: V) -> Self {
        Self {
            bytecode,
            operands: [0, 0],
            value,
        }
    }

    pub fn new_with_operands_and_value(
        bytecode: Instruction,
        operands: [usize; 2],
        value: V,
    ) -> Self {
        Self {
            bytecode,
            operands,
            value,
        }
    }

    pub fn bytecode(&self) -> &Instruction {
        &self.bytecode
    }

    pub fn operand(&self, idx: usize) -> OPERAND {
        promise!(idx < OPERAND_COUNT);

        self.operands[idx]
    }

    pub fn constant(&self) -> V {
        self.value
    }
}

#[cfg(debug_assertions)]
impl<V: std::fmt::Display> Debug for Byte<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?}({:?}) - {}",
            self.bytecode, self.operands, self.value
        )
    }
}

#[cfg(debug_assertions)]
impl std::fmt::Display for Instruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", *self as u8)
    }
}
