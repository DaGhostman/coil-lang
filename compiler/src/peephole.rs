//! Post-codegen peephole fusion for common stack sequences.

use common::{Byte, Instruction};

/// Fuse sequences and return original indices where 4-byte windows
/// were replaced (used to fix function entry offsets).
pub fn fuse_bytecode(bytecode: &mut Vec<Byte>) -> Vec<usize> {
    let mut out = Vec::with_capacity(bytecode.len());
    let mut fusion_sites: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < bytecode.len() {
        if let Some(fused) = try_fuse_jmpf_leq(&bytecode[i..]) {
            fusion_sites.push(i);
            out.push(fused);
            i += 4;
            continue;
        }
        if let Some(fused) = try_fuse_sub_call(&bytecode[i..]) {
            fusion_sites.push(i);
            out.push(fused);
            i += 4;
            continue;
        }
        out.push(bytecode[i]);
        i += 1;
    }

    for byte in &mut out {
        patch_targets(byte, &fusion_sites);
    }

    *bytecode = out;
    fusion_sites
}

pub fn adjust_target(target: usize, fusion_sites: &[usize]) -> usize {
    let delta = fusion_sites
        .iter()
        .filter(|&&site| site + 3 < target)
        .count()
        * 3;
    target.saturating_sub(delta)
}

fn patch_targets(byte: &mut Byte, fusion_sites: &[usize]) {
    match byte.bytecode() {
        Instruction::JMP | Instruction::JMPT | Instruction::JMPF => {
            let t = adjust_target(byte.operand_u32() as usize, fusion_sites);
            *byte = Byte::new(*byte.bytecode()).with_operand_u32(t as u32);
        }
        Instruction::CALL => {
            let (arity, target) = byte.call_parts();
            let t = adjust_target(target, fusion_sites);
            *byte = Byte::new(Instruction::CALL).with_call_packed(arity as u32, t as u32);
        }
        Instruction::JmpfLeqSlotImm => {
            let (slot, imm, target) = byte.jmpf_leq_slot_imm_parts();
            let t = adjust_target(target, fusion_sites);
            if t <= u16::MAX as usize {
                *byte = Byte::new(Instruction::JmpfLeqSlotImm)
                    .with_jmpf_leq_slot_imm(slot as u8, imm, t as u16);
            }
        }
        Instruction::SubCallSlotImm => {
            let (slot, imm, target) = byte.sub_call_slot_imm_parts();
            let t = adjust_target(target, fusion_sites);
            if t <= u16::MAX as usize {
                *byte = Byte::new(Instruction::SubCallSlotImm)
                    .with_sub_call_slot_imm(slot as u8, imm, t as u16);
            }
        }
        _ => {}
    }
}

fn const_inline_imm(byte: &Byte) -> Option<u8> {
    if *byte.bytecode() != Instruction::CONST {
        return None;
    }
    let op = byte.operand_u32();
    if op & Byte::POOL_FLAG != 0 {
        return None;
    }
    let v = op as i32;
    if !(0..=255).contains(&v) {
        return None;
    }
    Some(v as u8)
}

fn load_slot(byte: &Byte) -> Option<u8> {
    if *byte.bytecode() != Instruction::LOAD {
        return None;
    }
    let slot = byte.operand_u32();
    if slot > 255 {
        return None;
    }
    Some(slot as u8)
}

fn try_fuse_jmpf_leq(window: &[Byte]) -> Option<Byte> {
    if window.len() < 4 {
        return None;
    }
    let slot = load_slot(&window[0])?;
    let imm = const_inline_imm(&window[1])?;
    if *window[2].bytecode() != Instruction::LEQ {
        return None;
    }
    if *window[3].bytecode() != Instruction::JMPF {
        return None;
    }
    let target = window[3].operand_u32();
    if target > u16::MAX as u32 {
        return None;
    }
    Some(
        Byte::new(Instruction::JmpfLeqSlotImm)
            .with_jmpf_leq_slot_imm(slot, imm, target as u16),
    )
}

fn try_fuse_sub_call(window: &[Byte]) -> Option<Byte> {
    if window.len() < 4 {
        return None;
    }
    let slot = load_slot(&window[0])?;
    let imm = const_inline_imm(&window[1])?;
    if *window[2].bytecode() != Instruction::SUB {
        return None;
    }
    if *window[3].bytecode() != Instruction::CALL {
        return None;
    }
    let (arity, target) = window[3].call_parts();
    if arity != 1 || target > u16::MAX as usize {
        return None;
    }
    Some(
        Byte::new(Instruction::SubCallSlotImm)
            .with_sub_call_slot_imm(slot, imm, target as u16),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuse_jmpf_leq_sequence() {
        let mut bc = vec![
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::CONST).with_const_inline(2),
            Byte::new(Instruction::LEQ),
            Byte::new(Instruction::JMPF).with_operand_u32(8),
            Byte::new(Instruction::HALT),
        ];
        fuse_bytecode(&mut bc);
        assert_eq!(bc.len(), 2);
        assert_eq!(*bc[0].bytecode(), Instruction::JmpfLeqSlotImm);
        assert_eq!(bc[0].jmpf_leq_slot_imm_parts(), (0, 2, 5));
    }

    #[test]
    fn fuse_sub_call_sequence() {
        let mut bc = vec![
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::CONST).with_const_inline(1),
            Byte::new(Instruction::SUB),
            Byte::new(Instruction::CALL).with_call_packed(1, 3),
            Byte::new(Instruction::HALT),
        ];
        fuse_bytecode(&mut bc);
        assert_eq!(bc.len(), 2);
        assert_eq!(*bc[0].bytecode(), Instruction::SubCallSlotImm);
        assert_eq!(bc[0].sub_call_slot_imm_parts(), (0, 1, 3));
    }

    #[test]
    fn skip_fusion_when_const_uses_pool() {
        let mut bc = vec![
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::CONST).with_const_pool(0),
            Byte::new(Instruction::LEQ),
            Byte::new(Instruction::JMPF).with_operand_u32(8),
        ];
        fuse_bytecode(&mut bc);
        assert_eq!(bc.len(), 4);
    }

    #[test]
    fn skip_fusion_when_call_arity_not_one() {
        let mut bc = vec![
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::CONST).with_const_inline(1),
            Byte::new(Instruction::SUB),
            Byte::new(Instruction::CALL).with_call_packed(2, 3),
        ];
        fuse_bytecode(&mut bc);
        assert_eq!(bc.len(), 4);
    }

    #[test]
    fn adjusts_jump_targets_after_fusion() {
        let mut bc = vec![
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::CONST).with_const_inline(2),
            Byte::new(Instruction::LEQ),
            Byte::new(Instruction::JMPF).with_operand_u32(10),
            Byte::new(Instruction::CONST).with_const_inline(1),
            Byte::new(Instruction::RETURN),
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::CONST).with_const_inline(1),
            Byte::new(Instruction::SUB),
            Byte::new(Instruction::CALL).with_call_packed(1, 3),
            Byte::new(Instruction::JMP).with_operand_u32(20),
        ];
        fuse_bytecode(&mut bc);
        let jmpf = bc
            .iter()
            .find(|b| *b.bytecode() == Instruction::JmpfLeqSlotImm)
            .expect("jmpf fusion");
        // Two fusion sites (0 and 6) both precede original target 10 → 10 - 6 = 4.
        assert_eq!(jmpf.jmpf_leq_slot_imm_parts().2, 4);
        let jmp = bc
            .iter()
            .find(|b| *b.bytecode() == Instruction::JMP)
            .expect("jmp");
        assert_eq!(jmp.operand_u32(), 14);
    }
}
