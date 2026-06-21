use std::fmt::Debug;

use rkyv::{Archive, Deserialize, Serialize};

use crate::Value;

#[repr(u8)]
#[derive(Debug, Default, Copy, Clone, Archive, Serialize, Deserialize)]
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

    // ---- Phase 15C: sum types and pattern matching ----
    //
    // CRITICAL: these are APPENDED (not inserted) to keep the
    // `#[repr(u8)]` discriminant values of every prior opcode
    // stable. Inserting a new variant before `SET` would shift
    // the numeric value of `SET` (and every later opcode) and
    // silently corrupt every `.0s` archive ever compiled.
    //
    // Operand layout (Phase 15C):
    // - `MAKE_ENUM`:    upper 16 bits = tag, lower 16 bits = arity.
    // - `JUMP_IF_MATCH`: upper 16 bits = tag, lower 16 bits = target
    //   offset (in bytecode positions). The payload arity is read
    //   from the runtime enum object (`ObjEnum::payload.len()`) so
    //   no separate arity field is needed.
    // - `UNPACK`:       full u32 = arity (redundant with
    //   `ObjEnum::payload.len()` but kept for symmetry with the
    //   spec; the VM reads it from the enum at runtime).
    //
    // **KNOWN LIMITATION (Phase 15D, MEDIUM #1)**: the
    // `JUMP_IF_MATCH` target offset is a 16-bit unsigned
    // value, so the largest jump target is 65,535 bytes
    // (0xFFFF). A program whose bytecode exceeds this
    // would have its `JUMP_IF_MATCH` target silently
    // truncated by the `with_operands_u16` constructor.
    // In practice the 15C codegen always patches the
    // `JUMP_IF_MATCH` placeholder with an absolute offset
    // computed from `arm_body_offsets[i] as u16`, so any
    // match arm body past 65,535 bytes is unreachable.
    // Programs with large arm bodies (e.g., very deep
    // expression trees in a single arm) would silently
    // fail to dispatch. The fix is to widen the operand
    // layout to a full `u32` (matching the regular
    // `JMP`) and use a separate scratch word for the
    // tag. That change is deferred to a future phase
    // because no current test program approaches the
    // 65,535-byte limit.
    MakeEnum,
    JumpIfMatch,
    Unpack,
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

#[derive(Default, Clone, Copy, Archive, Serialize, Deserialize)]
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
