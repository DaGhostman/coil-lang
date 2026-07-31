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

    /// Inline `CONST` (pool-backed: [`Self::push_const_pool`]).
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

    fn push_pop(&mut self) {
        self.push_byte(Byte::new(Instruction::POP));
    }

    fn push_index(&mut self) {
        self.push_byte(Byte::new(Instruction::Index));
    }

    fn push_make_tuple(&mut self, arity: u32) {
        self.push_byte(Byte::new(Instruction::MakeTuple).with_operand_u32(arity));
    }

    fn push_make_array(&mut self, arity: u32) {
        self.push_byte(Byte::new(Instruction::MakeArray).with_operand_u32(arity));
    }

    fn push_make_enum(&mut self, tag: u16, arity: u16) {
        self.push_byte(Byte::new(Instruction::MakeEnum).with_operands_u16([tag, arity]));
    }

    fn push_box_value(&mut self, tag: u32) {
        self.push_byte(Byte::new(Instruction::BoxValue).with_operand_u32(tag));
    }

    fn push_unbox_value(&mut self, tag: u32) {
        self.push_byte(Byte::new(Instruction::UnboxValue).with_operand_u32(tag));
    }

    fn push_load_field(&mut self, index: u32) {
        self.push_byte(Byte::new(Instruction::LoadField).with_operand_u32(index));
    }

    fn push_get_field(&mut self) {
        self.push_byte(Byte::new(Instruction::GetField));
    }

    fn push_set_field(&mut self) {
        self.push_byte(Byte::new(Instruction::SetField));
    }

    fn push_host_invoke(&mut self, arity: u32) {
        self.push_byte(Byte::new(Instruction::HostInvoke).with_operand_u32(arity));
    }

    #[allow(dead_code)]
    fn push_print(&mut self) {
        self.push_byte(Byte::new(Instruction::PRINT));
    }

    fn push_const_pool(&mut self, idx: u32) {
        self.push_byte(Byte::new(Instruction::CONST).with_const_pool(idx));
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

    fn push_pop(&mut self) {
        CodeBuf::push_pop(self);
    }

    fn push_index(&mut self) {
        CodeBuf::push_index(self);
    }

    fn push_make_tuple(&mut self, arity: u32) {
        CodeBuf::push_make_tuple(self, arity);
    }

    fn push_make_array(&mut self, arity: u32) {
        CodeBuf::push_make_array(self, arity);
    }

    fn push_make_enum(&mut self, tag: u16, arity: u16) {
        CodeBuf::push_make_enum(self, tag, arity);
    }

    fn push_box_value(&mut self, tag: u32) {
        CodeBuf::push_box_value(self, tag);
    }

    fn push_unbox_value(&mut self, tag: u32) {
        CodeBuf::push_unbox_value(self, tag);
    }

    fn push_load_field(&mut self, index: u32) {
        CodeBuf::push_load_field(self, index);
    }

    fn push_get_field(&mut self) {
        CodeBuf::push_get_field(self);
    }

    fn push_set_field(&mut self) {
        CodeBuf::push_set_field(self);
    }

    fn push_host_invoke(&mut self, arity: u32) {
        CodeBuf::push_host_invoke(self, arity);
    }

    fn push_print(&mut self) {
        CodeBuf::push_print(self);
    }

    fn push_const_pool(&mut self, idx: u32) {
        CodeBuf::push_const_pool(self, idx);
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

    #[test]
    fn codebuf_lifts_longtail_aggregate_ops() {
        let mut buf = CodeBuf::new();
        EmitBuf::push_index(&mut buf);
        EmitBuf::push_make_tuple(&mut buf, 2);
        EmitBuf::push_make_array(&mut buf, 3);
        EmitBuf::push_make_enum(&mut buf, 7, 1);
        EmitBuf::push_pop(&mut buf);
        let ops = buf.ops();
        assert!(matches!(ops[0], IlOp::Index { .. }));
        assert!(matches!(ops[1], IlOp::MakeTuple { arity: 2, .. }));
        assert!(matches!(ops[2], IlOp::MakeArray { arity: 3, .. }));
        assert!(matches!(ops[3], IlOp::MakeEnum { tag: 7, arity: 1, .. }));
        assert!(matches!(ops[4], IlOp::Pop { .. }));
    }

    #[test]
    fn vec_emit_buf_packs_box_unbox_load_field() {
        let mut buf: Vec<Byte> = Vec::new();
        EmitBuf::push_box_value(&mut buf, 3);
        EmitBuf::push_unbox_value(&mut buf, 4);
        EmitBuf::push_load_field(&mut buf, 2);
        assert_eq!(*buf[0].bytecode(), Instruction::BoxValue);
        assert_eq!(buf[0].operand_u32(), 3);
        assert_eq!(*buf[1].bytecode(), Instruction::UnboxValue);
        assert_eq!(buf[1].operand_u32(), 4);
        assert_eq!(*buf[2].bytecode(), Instruction::LoadField);
        assert_eq!(buf[2].operand_u32(), 2);
    }

    #[test]
    fn codebuf_emit_buf_trait_lifts_residual_typed() {
        let mut buf = CodeBuf::new();
        EmitBuf::push_const_pool(&mut buf, 4);
        EmitBuf::push_get_field(&mut buf);
        EmitBuf::push_set_field(&mut buf);
        EmitBuf::push_host_invoke(&mut buf, 2);
        EmitBuf::push_print(&mut buf);
        let ops = buf.ops();
        assert!(matches!(ops[0], IlOp::ConstPool { idx: 4, .. }));
        assert!(matches!(ops[1], IlOp::GetField { .. }));
        assert!(matches!(ops[2], IlOp::SetField { .. }));
        assert!(matches!(ops[3], IlOp::HostInvoke { arity: 2, .. }));
        assert!(matches!(ops[4], IlOp::Print { .. }));
    }

    #[test]
    fn codebuf_push_byte_absorbs_residual_typed() {
        let mut buf = CodeBuf::new();
        buf.push(Byte::new(Instruction::CONST).with_const_pool(1));
        buf.push(Byte::new(Instruction::GetField));
        buf.push(Byte::new(Instruction::PRINT));
        buf.push(Byte::new(Instruction::HostInvoke).with_operand_u32(0));
        let ops = buf.ops();
        assert!(matches!(ops[0], IlOp::ConstPool { idx: 1, .. }));
        assert!(matches!(ops[1], IlOp::GetField { .. }));
        assert!(matches!(ops[2], IlOp::Print { .. }));
        assert!(matches!(ops[3], IlOp::HostInvoke { arity: 0, .. }));
    }
}
