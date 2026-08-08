use common::{ArchivedByte as Byte, Instruction};

use crate::{I64BinaryOp, I64Function, I64Instr};

/// Translate a small pure integer bytecode body into the JIT IR.
///
/// The adapter intentionally recognizes both fused and unfused compiler output
/// and returns `None` for calls, effects, unknown constants, or control flow.
pub fn translate_i64_bytecode(code: &[Byte], entry: usize) -> Option<I64Function> {
    if let Some(op) = fused_slot_slot(code, entry) {
        return Some(I64Function::binary(op));
    }
    if let Some((op, imm)) = fused_slot_imm(code, entry) {
        return Some(I64Function::binary_imm(op, imm));
    }
    if array_len_body(code, entry) {
        return Some(I64Function::array_len());
    }
    if let Some(index) = array_index_body(code, entry) {
        return Some(I64Function::array_index_const(index));
    }
    if let Some(op) = stack_binary_body(code, entry) {
        return Some(op);
    }
    None
}

fn fused_slot_slot(code: &[Byte], entry: usize) -> Option<I64BinaryOp> {
    let op = code.get(entry)?;
    let ret = code.get(entry + 1)?;
    if *op.bytecode() != Instruction::BinSlotSlot || *ret.bytecode() != Instruction::RETURN {
        return None;
    }
    let (raw_op, lhs, rhs) = op.bin_slot_slot_parts();
    if lhs != 0 || rhs != 1 {
        return None;
    }
    integer_op(raw_op)
}

fn fused_slot_imm(code: &[Byte], entry: usize) -> Option<(I64BinaryOp, i64)> {
    let op = code.get(entry)?;
    let ret = code.get(entry + 1)?;
    if *op.bytecode() != Instruction::BinSlotImm || *ret.bytecode() != Instruction::RETURN {
        return None;
    }
    let (raw_op, slot, imm) = op.bin_slot_imm_parts();
    if slot != 0 {
        return None;
    }
    Some((integer_op(raw_op)?, imm as i64))
}

fn array_len_body(code: &[Byte], entry: usize) -> bool {
    matches!(
        (code.get(entry), code.get(entry + 1), code.get(entry + 2)),
        (Some(load), Some(len), Some(ret))
            if load_slot(load) == Some(0)
                && *len.bytecode() == Instruction::ArrayLen
                && *ret.bytecode() == Instruction::RETURN
    )
}

fn array_index_body(code: &[Byte], entry: usize) -> Option<i64> {
    let load = code.get(entry)?;
    let constant = code.get(entry + 1)?;
    let index = code.get(entry + 2)?;
    let ret = code.get(entry + 3)?;
    if load_slot(load) != Some(0)
        || *index.bytecode() != Instruction::Index
        || *ret.bytecode() != Instruction::RETURN
    {
        return None;
    }
    inline_const(constant)
}

fn stack_binary_body(code: &[Byte], entry: usize) -> Option<I64Function> {
    let lhs = code.get(entry)?;
    let rhs = code.get(entry + 1)?;
    let binary = code.get(entry + 2)?;
    let ret = code.get(entry + 3)?;
    if load_slot(lhs) != Some(0)
        || load_slot(rhs) != Some(1)
        || *ret.bytecode() != Instruction::RETURN
    {
        return None;
    }
    let op = integer_op(*binary.bytecode() as u8)?;
    Some(I64Function::new(
        2,
        vec![
            I64Instr::LoadParam { dst: 0, param: 0 },
            I64Instr::LoadParam { dst: 1, param: 1 },
            I64Instr::Binary {
                dst: 2,
                lhs: 0,
                rhs: 1,
                op,
            },
            I64Instr::Return { value: 2 },
        ],
    ))
}

fn load_slot(byte: &Byte) -> Option<usize> {
    if *byte.bytecode() == Instruction::LOAD {
        byte.load_store_single_slot().map(|slot| slot as usize)
    } else {
        None
    }
}

fn inline_const(byte: &Byte) -> Option<i64> {
    if *byte.bytecode() != Instruction::CONST {
        return None;
    }
    let raw = byte.operand_u32();
    (raw & (1 << 31) == 0).then_some(raw as i32 as i64)
}

fn integer_op(raw: u8) -> Option<I64BinaryOp> {
    match Instruction::from(raw) {
        Instruction::ADD => Some(I64BinaryOp::Add),
        Instruction::SUB => Some(I64BinaryOp::Sub),
        Instruction::MUL => Some(I64BinaryOp::Mul),
        Instruction::DIV => Some(I64BinaryOp::Div),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::ArchivedInstruction;

    fn fused_add() -> Vec<Byte> {
        vec![
            Byte::new(ArchivedInstruction::BinSlotSlot).with_bin_slot_slot(
                Instruction::ADD as u8,
                0,
                1,
            ),
            Byte::new(ArchivedInstruction::RETURN),
        ]
    }

    #[test]
    fn translates_fused_integer_binary() {
        let function = translate_i64_bytecode(&fused_add(), 0).expect("fused add");
        assert_eq!(function.params(), 2);
    }

    #[test]
    fn translates_unfused_integer_binary() {
        let code = vec![
            Byte::new(ArchivedInstruction::LOAD).with_load_store_slot(0),
            Byte::new(ArchivedInstruction::LOAD).with_load_store_slot(1),
            Byte::new(ArchivedInstruction::ADD),
            Byte::new(ArchivedInstruction::RETURN),
        ];
        let function = translate_i64_bytecode(&code, 0).expect("unfused add");
        assert_eq!(function.params(), 2);
    }

    #[test]
    fn translates_array_and_immediate_shapes() {
        let len = vec![
            Byte::new(ArchivedInstruction::LOAD).with_load_store_slot(0),
            Byte::new(ArchivedInstruction::ArrayLen),
            Byte::new(ArchivedInstruction::RETURN),
        ];
        assert!(translate_i64_bytecode(&len, 0).is_some());

        let index = vec![
            Byte::new(ArchivedInstruction::LOAD).with_load_store_slot(0),
            Byte::new(ArchivedInstruction::CONST).with_const_inline(1),
            Byte::new(ArchivedInstruction::Index),
            Byte::new(ArchivedInstruction::RETURN),
        ];
        assert!(translate_i64_bytecode(&index, 0).is_some());
    }

    #[test]
    fn refuses_effectful_body() {
        let code = vec![
            Byte::new(ArchivedInstruction::CALL),
            Byte::new(ArchivedInstruction::RETURN),
        ];
        assert!(translate_i64_bytecode(&code, 0).is_none());
    }
}
