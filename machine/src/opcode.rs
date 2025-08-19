use std::fmt::{Debug, Display};

#[repr(u8)]
#[derive(Default, Copy, Clone)]
pub enum Instruction {
    ADD = 0,
    SUB,
    MUL,
    DIV,
    INC,
    DEC,
    ADDF,
    SUBF,
    MULF,
    DIVF,
    EQ = 30,
    LE,
    GT,
    JMP = 50,
    JMPT,
    JMPF,
    CALL,
    PRINT = 100,
    RETURN,
    PUSH = 200,
    POP,
    DUP,
    LOAD,
    STORE,
    MOVE,
    CONST,
    STRING,
    SUSP = 251,
    NOOP = 252,
    DATA = 253,
    #[default]
    HALT = 254,
}

impl From<u8> for Instruction {
    fn from(value: u8) -> Self {
        match value {
            0 => Instruction::ADD,
            1 => Instruction::SUB,
            2 => Instruction::MUL,
            3 => Instruction::DIV,
            4 => Instruction::ADDF,
            5 => Instruction::SUBF,
            6 => Instruction::MULF,
            7 => Instruction::DIVF,
            30 => Instruction::EQ,
            31 => Instruction::LE,
            32 => Instruction::GT,
            50 => Instruction::JMP,
            51 => Instruction::JMPT,
            52 => Instruction::JMPF,
            53 => Instruction::CALL,
            100 => Instruction::PRINT,
            101 => Instruction::RETURN,
            200 => Instruction::PUSH,
            201 => Instruction::POP,
            202 => Instruction::LOAD,
            203 => Instruction::STORE,
            204 => Instruction::CONST,
            251 => Instruction::SUSP,
            252 => Instruction::NOOP,
            254 => Instruction::HALT,
            _ => Instruction::HALT,
        }
    }
}

impl Into<u8> for Instruction {
    fn into(self) -> u8 {
        self as u8
    }
}

#[derive(Default, Clone, Copy)]
pub struct Byte<V> {
    bytecode: Instruction,
    operands: [u8; 2],
    value: V,
}

impl<V: Default + Copy> Byte<V> {
    pub fn new(bytecode: Instruction, operands: [u8; 2]) -> Self {
        Self {
            bytecode,
            operands,
            value: V::default(),
        }
    }

    pub fn new_with(bytecode: Instruction, operands: [u8; 2], value: V) -> Self {
        Self {
            bytecode,
            operands,
            value,
        }
    }

    pub fn bytecode(&self) -> Instruction {
        self.bytecode
    }

    pub fn operand(&self, idx: usize) -> u8 {
        self.operands[idx]
    }

    pub fn constant(&self) -> V {
        self.value
    }
}

#[cfg(debug_assertions)]
impl<V: Display> Debug for Byte<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({:?}) - {}", self.bytecode, self.operands, self.value)
    }
}

#[cfg(debug_assertions)]
impl Display for Instruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", *self as u8)
    }
}
