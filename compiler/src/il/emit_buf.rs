//! Sink for emitting `Byte`s into either a `Vec` or [`CodeBuf`].

use common::{Byte, Instruction};

use super::CodeBuf;

/// Minimal push API shared by local `Vec<Byte>` fragments and [`CodeBuf`].
pub trait EmitBuf {
    fn push_byte(&mut self, b: Byte);
    fn push(&mut self, b: Byte) {
        self.push_byte(b);
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
    ///
    /// Kept for `Vec` / `&mut impl EmitBuf`; `CodeBuf` callers usually hit the
    /// inherent method (which shadows this in method resolution).
    #[allow(dead_code)]
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

    #[allow(dead_code)]
    fn push_return(&mut self) {
        CodeBuf::push_return(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::il::IlOp;
    use common::Instruction;

    #[test]
    fn vec_emit_buf_packs_hot_path_bytes() {
        let mut buf: Vec<Byte> = Vec::new();
        EmitBuf::push_load(&mut buf, 3);
        EmitBuf::push_store_pop(&mut buf, 4);
        EmitBuf::push_const(&mut buf, 7);
        EmitBuf::push_return(&mut buf);

        assert_eq!(*buf[0].bytecode(), Instruction::LOAD);
        assert_eq!(buf[0].operand_u32(), 3);
        assert_eq!(*buf[1].bytecode(), Instruction::StorePop);
        assert_eq!(buf[1].operand_u32(), 4);
        assert_eq!(*buf[2].bytecode(), Instruction::CONST);
        assert_eq!(buf[2].operand_u32(), 7);
        assert_eq!(*buf[3].bytecode(), Instruction::RETURN);
    }

    #[test]
    fn codebuf_emit_buf_trait_lifts_hot_path_ops() {
        // Codegen uses `&mut impl EmitBuf`; trait overrides must lift, not pack Bytes.
        let mut buf = CodeBuf::new();
        EmitBuf::push_load(&mut buf, 1);
        EmitBuf::push_store_pop(&mut buf, 2);
        EmitBuf::push_const(&mut buf, 0);
        EmitBuf::push_return(&mut buf);
        let ops = buf.ops();
        assert!(matches!(ops[0], IlOp::Load { slot: 1, .. }));
        assert!(matches!(ops[1], IlOp::StorePop { slot: 2, .. }));
        assert!(matches!(ops[2], IlOp::Const { imm: 0, .. }));
        assert!(matches!(ops[3], IlOp::Return { .. }));
    }
}
