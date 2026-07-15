//! VM instruction set and encoded bytecode words (`Byte`).
//!
//! Append new `Instruction` variants only — `#[repr(u8)]` discriminants
//! must stay stable for archived bytecode.

use rkyv::{Archive, Deserialize, Serialize};

use crate::Value;

#[repr(u8)]
#[derive(Default, Copy, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[cfg_attr(debug_assertions, derive(Debug))]
#[rkyv(compare(PartialEq), derive(Clone), derive(Copy))]
pub enum Instruction {
    // Special
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

    // Built-ins
    PRINT,
    FORMAT,
    STRINGIFY,
    NATIVE,
    INIT,
    SET,

    // Sum types — append-only beyond this point.
    //
    // MakeEnum:     [31:16] tag, [15:0] arity
    // JumpIfMatch:  [31:16] tag, [15:0] pool index → 32-bit target
    // Unpack:       [31:0] arity
    // LoadField:    [15:0] field_index
    MakeEnum,
    JumpIfMatch,
    Unpack,
    LoadField,

    // StorePop: [31:0] slot_index — pop stack and write slot (let bindings).
    // STORE is a no-op used by match-arm binding codegen.
    StorePop,

    // UnpackAt: [31:16] arity, [15:0] slot offset — unpack enum at slot in place.
    UnpackAt,

    // FFI — stack bottom→top unless noted.
    FfiLoad,
    // FfiInvoke: lib, fn_id, args_tuple
    FfiInvoke,
    // DeclareFFI: [15:0] arg arity; stack: lib, name, args_tuple, ret_tag
    DeclareFFI,

    // Aggregates — MakeTuple/MakeArray/MakeDict: [15:0] arity; Index: no operand
    MakeTuple,
    MakeArray,
    Index,

    // Records — MakeDict: [15:0] field count; GetField/SetField: no operand
    MakeDict,
    GetField,
    SetField,

    // HostInvoke: [15:0] tuple arity; stack: fn_id, args_tuple
    HostInvoke,

    // Fused superinstructions — underlying op in [31:24] where applicable.
    //
    // LoadReturnSlot: [31:0] slot
    // ConstReturnImm: [31:0] inline i32
    // BinSlotImm:     [31:24] op, [23:16] slot, [15:0] i16 imm
    // CmpJmpf:        [31:24] op, [15:0] false-branch target
    // BinReturn:      [31:24] op
    // BinSlotSlot:    [31:24] op, [23:16] slot a, [15:8] slot b
    LoadReturnSlot,
    ConstReturnImm,
    BinSlotImm,
    CmpJmpf,
    BinReturn,
    BinSlotSlot,
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

impl From<u8> for ArchivedInstruction {
    fn from(value: u8) -> Self {
        unsafe { std::mem::transmute(value) }
    }
}

#[repr(C)]
#[derive(Default, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq))]
pub struct Byte {
    bytecode: Instruction,
    _pad: [u8; 3],
    operands: u32,
}

impl Byte {
    /// High bit on a `CONST` operand marks a constant-pool index.
    pub const POOL_FLAG: u32 = 1 << 31;

    pub fn new(bytecode: Instruction) -> Self {
        Self {
            bytecode,
            _pad: [0; 3],
            operands: Default::default(),
        }
    }

    pub fn with_operand_u32(mut self, operand: u32) -> Self {
        self.operands = operand;
        self
    }

    /// CALL: [31:24] arity, [23:0] target (24 bits).
    pub fn with_call_packed(mut self, arity: u32, target: u32) -> Self {
        debug_assert!(target <= 0xFFFFFF, "CALL target exceeds 24-bit encoding");
        self.operands = (arity << 24) | (target & 0xFFFFFF);
        self
    }

    pub fn call_parts(&self) -> (usize, usize) {
        (
            (self.operands >> 24) as usize,
            (self.operands & 0xFFFFFF) as usize,
        )
    }

    pub fn with_value_u32(mut self, v: u32) -> Self {
        if matches!(self.bytecode, Instruction::CALL) {
            let arity = self.operands;
            return self.with_call_packed(arity, v);
        }
        self.operands = (self.operands & 0xFFFF_0000) | (v & 0xFFFF);
        self
    }

    pub fn value_u32(&self) -> u32 {
        if matches!(self.bytecode, Instruction::CALL) {
            self.operands & 0xFFFFFF
        } else {
            self.operands & 0xFFFF
        }
    }

    pub fn with_operands_u16(mut self, operands: [u16; 2]) -> Self {
        let mut operand: u32 = 0;
        operand ^= operands[0] as u32;
        operand <<= 16;
        operand ^= operands[1] as u32;
        self.operands = operand;
        self
    }

    pub fn with_const_inline(mut self, value: i32) -> Self {
        debug_assert!(self.bytecode as u8 == Instruction::CONST as u8);
        self.operands = value as u32;
        self
    }

    pub fn with_const_pool(mut self, pool_index: u32) -> Self {
        debug_assert!(self.bytecode as u8 == Instruction::CONST as u8);
        self.operands = Self::POOL_FLAG | pool_index;
        self
    }

    pub fn new_with_value(bytecode: Instruction, value: u64) -> Self {
        let v = value as i64;
        if v >= i32::MIN as i64 && v <= i32::MAX as i64 {
            Self::new(bytecode).with_const_inline(v as i32)
        } else {
            Self::new(bytecode).with_const_pool(0)
        }
    }

    pub fn bytecode(&self) -> &Instruction {
        &self.bytecode
    }

    pub fn operand_u32(&self) -> u32 {
        self.operands
    }

    pub fn operand_u16(&self, index: usize) -> u16 {
        match index {
            0 => (self.operands >> 16) as u16,
            1 => ((self.operands << 16) >> 16) as u16,
            _ => unreachable!("Unable to use larger index when using u32 operands"),
        }
    }

    pub fn constant(&self, pool: &[u64]) -> u64 {
        if self.operands & Self::POOL_FLAG != 0 {
            pool[(self.operands & !Self::POOL_FLAG) as usize]
        } else {
            self.operands as i32 as i64 as u64
        }
    }

    /// JumpIfMatch target from pool (index in lower 16 bits of `operands`).
    pub fn jump_if_match_target(&self, pool: &[u64]) -> usize {
        pool[(self.operands & 0xFFFF) as usize] as usize
    }

    pub fn with_bin_slot_imm(mut self, op: u8, slot: u8, imm: i16) -> Self {
        self.operands = ((op as u32) << 24) | ((slot as u32) << 16) | (imm as u16 as u32);
        self
    }

    pub fn bin_slot_imm_parts(&self) -> (u8, usize, i64) {
        let o = self.operands;
        (
            (o >> 24) as u8,
            ((o >> 16) & 0xFF) as usize,
            (o & 0xFFFF) as u16 as i16 as i64,
        )
    }

    pub fn with_cmp_jmpf(mut self, op: u8, target: u16) -> Self {
        self.operands = ((op as u32) << 24) | (target as u32);
        self
    }

    pub fn cmp_jmpf_parts(&self) -> (u8, usize) {
        (
            (self.operands >> 24) as u8,
            (self.operands & 0xFFFF) as usize,
        )
    }

    pub fn with_bin_return(mut self, op: u8) -> Self {
        self.operands = (op as u32) << 24;
        self
    }

    pub fn bin_return_op(&self) -> u8 {
        (self.operands >> 24) as u8
    }

    pub fn with_bin_slot_slot(mut self, op: u8, a: u8, b: u8) -> Self {
        self.operands = ((op as u32) << 24) | ((a as u32) << 16) | ((b as u32) << 8);
        self
    }

    pub fn bin_slot_slot_parts(&self) -> (u8, usize, usize) {
        let o = self.operands;
        (
            (o >> 24) as u8,
            ((o >> 16) & 0xFF) as usize,
            ((o >> 8) & 0xFF) as usize,
        )
    }
}

impl ArchivedByte {
    pub const POOL_FLAG: u32 = Byte::POOL_FLAG;

    pub fn new(bytecode: ArchivedInstruction) -> Self {
        Self {
            bytecode,
            _pad: [0; 3],
            operands: Default::default(),
        }
    }

    pub fn with_operand_u32(mut self, operand: u32) -> Self {
        self.operands = operand.into();
        self
    }

    pub fn with_call_packed(mut self, arity: u32, target: u32) -> Self {
        self.operands = ((arity << 24) | (target & 0xFFFFFF)).into();
        self
    }

    pub fn with_const_inline(mut self, value: i32) -> Self {
        self.operands = (value as u32).into();
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

    pub fn with_value(mut self, value: Value) -> Self {
        let raw = value.raw() as u64;
        let v = raw as i64;
        if v >= i32::MIN as i64 && v <= i32::MAX as i64 {
            self.operands = (v as i32 as u32).into();
        } else {
            self.operands = Self::POOL_FLAG.into();
        }
        self
    }

    pub fn with_value_u32(mut self, v: u32) -> Self {
        self.operands = v.into();
        self
    }

    pub fn value_u32(&self) -> u32 {
        u32::from(self.operands) & 0xFFFFFF
    }

    pub fn bytecode(&self) -> &ArchivedInstruction {
        &self.bytecode
    }

    pub fn operand_u32(&self) -> u32 {
        self.operands.into()
    }

    pub fn operand_u16(&self, index: usize) -> u16 {
        match index {
            0 => (self.operands >> 16) as u16,
            1 => ((self.operands << 16) >> 16) as u16,
            _ => unreachable!("Unable to use larger index when using u32 operands"),
        }
    }

    pub fn constant(&self, pool: &[u64]) -> u64 {
        let op: u32 = self.operands.into();
        if op & Self::POOL_FLAG != 0 {
            pool[(op & !Self::POOL_FLAG) as usize]
        } else {
            op as i32 as i64 as u64
        }
    }

    pub fn call_parts(&self) -> (usize, usize) {
        let op: u32 = self.operands.into();
        ((op >> 24) as usize, (op & 0xFFFFFF) as usize)
    }

    pub fn jump_if_match_target(&self, pool: &[u64]) -> usize {
        let op: u32 = self.operands.into();
        pool[(op & 0xFFFF) as usize] as usize
    }

    pub fn bin_slot_imm_parts(&self) -> (u8, usize, i64) {
        let o: u32 = self.operands.into();
        (
            (o >> 24) as u8,
            ((o >> 16) & 0xFF) as usize,
            (o & 0xFFFF) as u16 as i16 as i64,
        )
    }

    pub fn with_bin_slot_imm(mut self, op: u8, slot: u8, imm: i16) -> Self {
        let packed = ((op as u32) << 24) | ((slot as u32) << 16) | (imm as u16 as u32);
        self.operands = packed.into();
        self
    }

    pub fn cmp_jmpf_parts(&self) -> (u8, usize) {
        let o: u32 = self.operands.into();
        ((o >> 24) as u8, (o & 0xFFFF) as usize)
    }

    pub fn with_cmp_jmpf(mut self, op: u8, target: u16) -> Self {
        self.operands = (((op as u32) << 24) | (target as u32)).into();
        self
    }

    pub fn bin_return_op(&self) -> u8 {
        (u32::from(self.operands) >> 24) as u8
    }

    pub fn with_bin_return(mut self, op: u8) -> Self {
        self.operands = ((op as u32) << 24).into();
        self
    }

    pub fn bin_slot_slot_parts(&self) -> (u8, usize, usize) {
        let o: u32 = self.operands.into();
        (
            (o >> 24) as u8,
            ((o >> 16) & 0xFF) as usize,
            ((o >> 8) & 0xFF) as usize,
        )
    }

    pub fn with_bin_slot_slot(mut self, op: u8, a: u8, b: u8) -> Self {
        self.operands = (((op as u32) << 24) | ((a as u32) << 16) | ((b as u32) << 8)).into();
        self
    }
}

#[cfg(debug_assertions)]
use std::fmt::Debug;

#[cfg(debug_assertions)]
impl Debug for Byte {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}({:?})", self.bytecode, self.operands)
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
        write!(f, "{}({:?})", self.bytecode as u8, self.operands)
    }
}

#[cfg(debug_assertions)]
impl std::fmt::Display for ArchivedInstruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", *self as u8)
    }
}
