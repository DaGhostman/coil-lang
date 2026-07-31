//! Sink for emitting `Byte`s into either a `Vec` or [`CodeBuf`].

use common::{Byte, Instruction};

use super::CodeBuf;

/// Minimal push API shared by local `Vec<Byte>` fragments and [`CodeBuf`].
pub trait EmitBuf {
    fn push_byte(&mut self, b: Byte);
    fn push(&mut self, b: Byte) {
        self.push_byte(b);
    }
    fn extend_from_slice_bytes(&mut self, bytes: &[Byte]) {
        for &b in bytes {
            self.push_byte(b);
        }
    }

    /// Hot-path `LOAD` (typed on [`CodeBuf`], packed `Byte` on `Vec`).
    fn push_load(&mut self, slot: u32) {
        self.push_byte(Byte::new(Instruction::LOAD).with_operand_u32(slot));
    }

    /// Hot-path `StorePop`.
    fn push_store_pop(&mut self, slot: u32) {
        self.push_byte(Byte::new(Instruction::StorePop).with_operand_u32(slot));
    }

    /// Inline `CONST` (pool-backed consts stay on [`Self::push_byte`]).
    fn push_const(&mut self, imm: i32) {
        self.push_byte(Byte::new(Instruction::CONST).with_const_inline(imm));
    }

    /// Hot-path `RETURN`.
    fn push_return(&mut self) {
        self.push_byte(Byte::new(Instruction::RETURN));
    }
}

impl EmitBuf for Vec<Byte> {
    fn push_byte(&mut self, b: Byte) {
        self.push(b);
    }
}

impl EmitBuf for CodeBuf {
    fn push_byte(&mut self, b: Byte) {
        self.push(b);
    }

    fn push_load(&mut self, slot: u32) {
        CodeBuf::push_load(self, slot);
    }

    fn push_store_pop(&mut self, slot: u32) {
        CodeBuf::push_store_pop(self, slot);
    }

    fn push_const(&mut self, imm: i32) {
        CodeBuf::push_const(self, imm);
    }

    fn push_return(&mut self) {
        CodeBuf::push_return(self);
    }
}
