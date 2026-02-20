use std::fmt::Debug;

use rkyv::{Archive, Deserialize, Serialize};

use crate::Value;

#[repr(u8)]
#[derive(Debug, Default, Copy, Clone, Archive, Serialize, Deserialize, PartialEq, Eq)]
#[rkyv(compare(PartialEq), derive(Debug), derive(Clone), derive(Copy))]
pub enum Instruction {
    // -- Special
    #[default]
    HALT,
    NOOP,
    DUPLICATE,
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
    INC,
    DEC,

    // Arithmetic
    ADD,
    SUB,
    MUL,
    DIV,
    MOD,
    ADDF,
    SUBF,
    MULF,
    DIVF,
    MODF,
    NOT,
    FLP,
    NEG,
    AND,
    OR,
    SHL,
    SHR,
    XOR,
    EQ,
    NEQ,
    LE,
    LEQ,
    LEF,
    LEQF,
    GT,
    GEQ,
    GTF,
    GEQF,
    // -- Keyword
    PRINT,
    FORMAT,
    STRINGIFY,
    NATIVE,
    INIT,
    SET,

    // -- Variants (sum types)
    VARIANT_SET,
    MATCH_BRANCH,
    MATCH_DEFAULT,
    VARIANT_POP,
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

#[derive(Default, Clone, Copy, Archive, Serialize, Deserialize, PartialEq, Eq)]
#[rkyv(compare(PartialEq))]
pub struct Byte {
    bytecode: Instruction,
    operands: u32,
    value: u64,
}

impl Byte {
    pub fn new(bytecode: Instruction) -> Self {
        Self {
            bytecode,
            operands: Default::default(),
            value: 0,
        }
    }

    pub fn with_operand_u32(mut self, operand: u32) -> Self {
        self.operands = operand;

        self
    }

    pub fn with_operands_u16(mut self, operands: [u16; 2]) -> Self {
        let mut operand: u32 = 0;
        operand ^= operands[0] as u32;
        operand <<= 16;
        operand ^= operands[1] as u32;

        self.operands = operand;

        self
    }

    pub fn new_with_value(bytecode: Instruction, value: u64) -> Self {
        Self {
            bytecode,
            operands: 0,
            value,
        }
    }

    pub fn bytecode(&self) -> &Instruction {
        &self.bytecode
    }

    pub fn operand_u32(&self) -> u32 {
        self.operands
    }

    ///
    ///```
    /// use common::{Instruction, Byte};
    ///
    /// let mut value: Byte = Byte::new(Instruction::default());
    /// value = value.with_operands_u16([1, 2,]);
    /// assert_eq!(1, value.operand_u16(0));
    /// assert_eq!(2, value.operand_u16(1));
    /// ```
    ///
    pub fn operand_u16(&self, index: usize) -> u16 {
        match index {
            0 => (self.operands >> 16) as u16,
            1 => ((self.operands << 16) >> 16) as u16,
            _ => unreachable!("Unable to use larger index when using u32 operands"),
        }
    }

    pub fn constant(&self) -> u64 {
        self.value
    }
}

impl ArchivedByte {
    pub fn new(bytecode: ArchivedInstruction) -> Self {
        Self {
            bytecode,
            operands: Default::default(),
            value: 0.into(),
        }
    }

    pub fn with_operand_u32(mut self, operand: u32) -> Self {
        self.operands = operand.into();

        self
    }

    pub fn with_value(mut self, value: Value) -> Self {
        self.value = (value.raw() as u64).into();

        self
    }

    pub fn with_operands_u16(mut self, operands: [u16; 2]) -> Self {
        let mut operand: u32 = 0;
        operand ^= operands[0] as u32;
        operand <<= 16;
        operand ^= operands[1] as u32;

        self.operands = operand.into();

        self
    }

    pub fn bytecode(&self) -> &ArchivedInstruction {
        &self.bytecode
    }

    pub fn operand_u32(&self) -> u32 {
        self.operands.into()
    }

    ///
    ///```
    /// use common::{Instruction, Byte};
    ///
    /// let mut value: Byte = Byte::new(Instruction::default());
    /// value = value.with_operands_u16([1, 2,]);
    /// assert_eq!(1, value.operand_u16(0));
    /// assert_eq!(2, value.operand_u16(1));
    /// ```
    ///
    pub fn operand_u16(&self, index: usize) -> u16 {
        match index {
            0 => (self.operands >> 16) as u16,
            1 => ((self.operands << 16) >> 16) as u16,
            _ => unreachable!("Unable to use larger index when using u32 operands"),
        }
    }

    pub fn constant(&self) -> u64 {
        self.value.into()
    }
}

#[cfg(debug_assertions)]
impl Debug for Byte {
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
#[cfg(debug_assertions)]
impl Debug for ArchivedByte {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?}({:?}) - {}",
            self.bytecode, self.operands, self.value
        )
    }
}

#[cfg(debug_assertions)]
impl std::fmt::Display for ArchivedInstruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", *self as u8)
    }
}
