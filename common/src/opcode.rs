use std::fmt::{Debug, Display};

#[repr(u8)]
#[derive(Debug, Default, Copy, Clone)]
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
    EQF,
    LE,
    LEF,
    GT,
    GTF,
    JMP = 50,
    JLE,
    JGT,
    JEQ,
    JMPT,
    JMPF,
    GOTO,
    CALL,
    PRINTI = 100,
    PRINTF,
    PRINTB,
    PRINTS,
    RETURN,
    PUSH = 200,
    POP,
    DUP,
    LOAD,
    STORE,
    MOVE,
    CONST,
    STRING,
    RESUME = 250,
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
            100 => Instruction::PRINTI,
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

impl From<Instruction> for u8 {
    fn from(val: Instruction) -> Self {
        val as u8
    }
}

#[derive(Default, Clone, Copy)]
pub struct Byte<V> {
    bytecode: Instruction,
    operands: [usize; 2],
    value: V,
}

impl<V: Default + Copy> Byte<V> {
    pub fn new(bytecode: Instruction) -> Self {
        Self {
            bytecode,
            operands: [0, 0],
            value: V::default(),
        }
    }

    pub fn new_with_operands(bytecode: Instruction, operands: [usize; 2]) -> Self {
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

    pub fn bytecode(&self) -> Instruction {
        self.bytecode
    }

    pub fn operand(&self, idx: usize) -> usize {
        self.operands[idx]
    }

    pub fn constant(&self) -> V {
        self.value
    }
}

#[cfg(debug_assertions)]
impl<V: Display> Debug for Byte<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?}({:?}) - {}",
            self.bytecode, self.operands, self.value
        )
    }
}

#[cfg(debug_assertions)]
impl Display for Instruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", *self as u8)
    }
}
