//! Post-codegen peephole fusion for common stack sequences.
//!
//! Recognised convoys collapse into operator-parameterized superinstructions
//! (`BinSlotImm`, `BinSlotSlot`, `CmpJmpf`, `BinReturn`, etc.). The packed
//! operator byte selects int/float arithmetic or comparison; the VM reuses
//! the shared `binary!` macro. Fusion shrinks the stream, so jump/call targets
//! past fused windows are relocated via [`adjust_target`] / [`patch_targets`].
//!
//! | Fused opcode     | Source convoy               |
//! |------------------|-----------------------------|
//! | `BinSlotImm`     | `LOAD s; CONST k; <op>`     |
//! | `BinSlotSlot`    | `LOAD a; LOAD b; <op>`      |
//! | `CmpJmpf`        | `<cmp>; JMPF t`             |
//! | `BinReturn`      | `<binop>; RETURN`           |
//! | `LoadReturnSlot` | `LOAD s; RETURN`            |
//! | `ConstReturnImm` | `CONST k; RETURN`           |
//!
//! Also folds `CONST a; CONST b; <ADD|SUB|MUL>` when both are inline and the
//! result is a non-negative `i32`. Fusion bails when operands won't fit packed
//! fields (slot > 255, imm outside `i16`, target > `u16::MAX`, pool-backed const).

use common::{Byte, Instruction};

/// A window that was collapsed into one fused opcode.
#[derive(Clone, Copy)]
pub struct FusionSite {
    /// Index of the window's first instruction in the ORIGINAL
    /// (pre-fusion) bytecode.
    orig: usize,
    /// Bytes removed by the fusion (`window_len - 1`).
    removed: usize,
}

/// Fuse recognised convoys in place and return the sites that were
/// collapsed (used to relocate function entry offsets and the
/// program start offset). `pool` is the constant pool; the pass
/// rewrites the `JUMP_IF_MATCH` targets it stores.
pub fn fuse_bytecode(bytecode: &mut Vec<Byte>, pool: &mut Vec<u64>) -> Vec<FusionSite> {
    let sites = fuse_bytecode_pass(bytecode, pool);
    for byte in &mut *bytecode {
        patch_targets(byte, &sites, pool);
    }
    sites
}

fn fuse_bytecode_pass(bytecode: &mut Vec<Byte>, pool: &mut Vec<u64>) -> Vec<FusionSite> {
    let mut out = Vec::with_capacity(bytecode.len());
    let mut fusion_sites: Vec<FusionSite> = Vec::new();
    let mut i = 0;
    while i < bytecode.len() {
        if let Some((fused, window)) = try_fuse(&bytecode[i..], pool) {
            fusion_sites.push(FusionSite {
                orig: i,
                removed: window - 1,
            });
            out.push(fused);
            i += window;
            continue;
        }
        out.push(bytecode[i]);
        i += 1;
    }

    *bytecode = out;
    fusion_sites
}

/// Shift a target offset down by the bytes removed by every fusion
/// whose window ends at or before it.
pub fn adjust_target(target: usize, fusion_sites: &[FusionSite]) -> usize {
    let delta: usize = fusion_sites
        .iter()
        .filter(|s| s.orig + s.removed < target)
        .map(|s| s.removed)
        .sum();
    target.saturating_sub(delta)
}

fn patch_targets(byte: &mut Byte, fusion_sites: &[FusionSite], pool: &mut [u64]) {
    match byte.bytecode() {
        Instruction::JMP | Instruction::JMPT | Instruction::JMPF => {
            // `u32::MAX` is the prologue JMP placeholder; the pipeline
            // patches it later to the (separately adjusted)
            // `program_start_offset`, so leave the sentinel alone.
            if byte.operand_u32() != u32::MAX {
                let t = adjust_target(byte.operand_u32() as usize, fusion_sites);
                *byte = Byte::new(*byte.bytecode()).with_operand_u32(t as u32);
            }
        }
        Instruction::CALL | Instruction::MakeCoro => {
            let (arity, target) = byte.call_parts();
            let t = adjust_target(target, fusion_sites);
            *byte = Byte::new(*byte.bytecode()).with_call_packed(arity as u32, t as u32);
        }
        Instruction::CmpJmpf => {
            let (op, target) = byte.cmp_jmpf_parts();
            let t = adjust_target(target, fusion_sites);
            if t <= u16::MAX as usize {
                *byte = Byte::new(Instruction::CmpJmpf).with_cmp_jmpf(op, t as u16);
            }
        }
        Instruction::BinSlotImmJmpf => {
            let (op, slot, pool_idx) = byte.bin_slot_imm_jmpf_parts();
            if pool_idx < pool.len() {
                let packed = pool[pool_idx];
                let imm = packed as u32;
                let target = adjust_target((packed >> 32) as usize, fusion_sites);
                pool[pool_idx] = ((target as u64) << 32) | (imm as u64);
            }
            let _ = (op, slot);
        }
        Instruction::LogNotJmpf => {
            let t = adjust_target(byte.log_not_jmpf_target(), fusion_sites);
            if t <= u16::MAX as usize {
                *byte = Byte::new(Instruction::LogNotJmpf).with_log_not_jmpf(t as u16);
            }
        }
        // `JUMP_IF_MATCH` keeps its absolute target in the constant
        // pool (lower 16 operand bits index it), so relocate the pool
        // entry, not the inline operand. Each `JUMP_IF_MATCH` owns a
        // distinct pool slot, so this adjusts every target exactly once.
        Instruction::JumpIfMatch => {
            let idx = (byte.operand_u32() & 0xFFFF) as usize;
            if idx < pool.len() {
                pool[idx] = adjust_target(pool[idx] as usize, fusion_sites) as u64;
            }
        }
        // Self-identifying absolute code pointers (dictionary slots,
        // CallIndirect targets, escaped generic fn values).
        Instruction::CodePtr | Instruction::MakePolyFn => {
            let t = adjust_target(byte.operand_u32() as usize, fusion_sites);
            *byte = Byte::new(*byte.bytecode()).with_operand_u32(t as u32);
        }
        _ => {}
    }
}

/// Shift absolute jump/call targets at or after `threshold` forward by `delta`.
///
/// Used when bytecode is inserted (e.g. static initializer prologue) so
/// `CALL`/`JMP` operands emitted before the splice still reach the right
/// instruction.
pub fn bump_targets_at_or_after(
    bytecode: &mut [Byte],
    pool: &mut [u64],
    threshold: usize,
    delta: usize,
) {
    if delta == 0 {
        return;
    }
    for byte in bytecode.iter_mut() {
        bump_target_in_byte(byte, pool, threshold, delta);
    }
}

/// Like [`bump_targets_at_or_after`], but skips the instruction at `skip_index`
/// (used when a freshly inserted `JMP` already has a pre-adjusted target).
pub fn bump_targets_at_or_after_skip_byte(
    bytecode: &mut [Byte],
    pool: &mut [u64],
    skip_index: usize,
    threshold: usize,
    delta: usize,
) {
    if delta == 0 {
        return;
    }
    for (i, byte) in bytecode.iter_mut().enumerate() {
        if i == skip_index {
            continue;
        }
        bump_target_in_byte(byte, pool, threshold, delta);
    }
}

fn bump_target_in_byte(byte: &mut Byte, pool: &mut [u64], threshold: usize, delta: usize) {
    let bump = |t: usize| -> usize {
        if t >= threshold {
            t + delta
        } else {
            t
        }
    };
    match byte.bytecode() {
        Instruction::JMP | Instruction::JMPT | Instruction::JMPF => {
            if byte.operand_u32() != u32::MAX {
                let t = bump(byte.operand_u32() as usize);
                *byte = Byte::new(*byte.bytecode()).with_operand_u32(t as u32);
            }
        }
        Instruction::CALL | Instruction::MakeCoro => {
            let (arity, target) = byte.call_parts();
            let t = bump(target);
            *byte = Byte::new(*byte.bytecode()).with_call_packed(arity as u32, t as u32);
        }
        Instruction::CmpJmpf => {
            let (op, target) = byte.cmp_jmpf_parts();
            let t = bump(target);
            if t <= u16::MAX as usize {
                *byte = Byte::new(Instruction::CmpJmpf).with_cmp_jmpf(op, t as u16);
            }
        }
        Instruction::BinSlotImmJmpf => {
            let (op, slot, pool_idx) = byte.bin_slot_imm_jmpf_parts();
            if pool_idx < pool.len() {
                let packed = pool[pool_idx];
                let imm = packed as u32;
                let target = bump((packed >> 32) as usize);
                pool[pool_idx] = ((target as u64) << 32) | (imm as u64);
            }
            let _ = (op, slot);
        }
        Instruction::LogNotJmpf => {
            let t = bump(byte.log_not_jmpf_target());
            if t <= u16::MAX as usize {
                *byte = Byte::new(Instruction::LogNotJmpf).with_log_not_jmpf(t as u16);
            }
        }
        Instruction::JumpIfMatch => {
            let idx = (byte.operand_u32() & 0xFFFF) as usize;
            if idx < pool.len() {
                pool[idx] = bump(pool[idx] as usize) as u64;
            }
        }
        Instruction::CodePtr | Instruction::MakePolyFn => {
            let t = bump(byte.operand_u32() as usize);
            *byte = Byte::new(*byte.bytecode()).with_operand_u32(t as u32);
        }
        _ => {}
    }
}

/// True for integer binary ops that `BinSlotImm` can carry
/// (arithmetic + comparisons on a local and an inline immediate).
fn is_int_bin_op(i: Instruction) -> bool {
    matches!(
        i,
        Instruction::ADD
            | Instruction::SUB
            | Instruction::MUL
            | Instruction::DIV
            | Instruction::MOD
            | Instruction::LE
            | Instruction::LEQ
            | Instruction::GT
            | Instruction::GEQ
            | Instruction::EQ
            | Instruction::NEQ
            | Instruction::Pow
            | Instruction::BITAND
            | Instruction::BITOR
    )
}

/// True for comparison ops that `CmpJmpf` can carry (int or float).
fn is_cmp_op(i: Instruction) -> bool {
    matches!(
        i,
        Instruction::LE
            | Instruction::LEQ
            | Instruction::GT
            | Instruction::GEQ
            | Instruction::EQ
            | Instruction::NEQ
            | Instruction::LEF
            | Instruction::LEQF
            | Instruction::GTF
            | Instruction::GEQF
    )
}

/// True for any binary op that `BinReturn` can carry (int or float
/// arithmetic and comparisons).
fn is_bin_op(i: Instruction) -> bool {
    is_int_bin_op(i)
        || matches!(
            i,
            Instruction::ADDF
                | Instruction::SUBF
                | Instruction::MULF
                | Instruction::DIVF
                | Instruction::MODF
                | Instruction::LEF
                | Instruction::LEQF
                | Instruction::GTF
                | Instruction::GEQF
                | Instruction::Pow
                | Instruction::BITAND
                | Instruction::BITOR
        )
}

/// Try every fusion rule against the window, returning the fused byte
/// and the number of original instructions it replaces.
fn try_fuse(window: &[Byte], pool: &mut Vec<u64>) -> Option<(Byte, usize)> {
    // 4-instruction convoys first.
    if let Some(b) = try_fuse_load_const_cmp_jmpf(window, pool) {
        return Some((b, 4));
    }
    // 3-instruction convoys (their prefix overlaps the shorter rules).
    if let Some(b) = try_fold_const_bin(window) {
        return Some((b, 3));
    }
    if let Some(b) = try_fuse_bin_slot_imm(window) {
        return Some((b, 3));
    }
    if let Some(b) = try_fuse_bin_slot_slot(window) {
        return Some((b, 3));
    }
    // 2-instruction convoys.
    if let Some(b) = try_fuse_bin_slot_imm_jmpf(window, pool) {
        return Some((b, 2));
    }
    if let Some(b) = try_fuse_log_not_jmpf(window) {
        return Some((b, 2));
    }
    if let Some(b) = try_fuse_cmp_jmpf(window) {
        return Some((b, 2));
    }
    if let Some(b) = try_fuse_load_return(window) {
        return Some((b, 2));
    }
    if let Some(b) = try_fuse_const_return(window) {
        return Some((b, 2));
    }
    if let Some(b) = try_fuse_bin_return(window) {
        return Some((b, 2));
    }
    None
}

/// The inline `i32` value of a `CONST`, or `None` if it is pool-backed.
fn const_inline_value(byte: &Byte) -> Option<i32> {
    if *byte.bytecode() != Instruction::CONST {
        return None;
    }
    let op = byte.operand_u32();
    if op & Byte::POOL_FLAG != 0 {
        return None;
    }
    Some(op as i32)
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

/// `LOAD s; CONST k; <int binop>` → `BinSlotImm`.
fn try_fuse_bin_slot_imm(window: &[Byte]) -> Option<Byte> {
    if window.len() < 3 {
        return None;
    }
    let slot = load_slot(&window[0])?;
    let imm = i16::try_from(const_inline_value(&window[1])?).ok()?;
    let op = *window[2].bytecode();
    if !is_int_bin_op(op) {
        return None;
    }
    Some(Byte::new(Instruction::BinSlotImm).with_bin_slot_imm(op as u8, slot, imm))
}

/// `LOAD a; LOAD b; <binop>` → `BinSlotSlot` (int or float).
fn try_fuse_bin_slot_slot(window: &[Byte]) -> Option<Byte> {
    if window.len() < 3 {
        return None;
    }
    let a = load_slot(&window[0])?;
    let b = load_slot(&window[1])?;
    let op = *window[2].bytecode();
    if !is_bin_op(op) {
        return None;
    }
    Some(Byte::new(Instruction::BinSlotSlot).with_bin_slot_slot(op as u8, a, b))
}

/// `CONST a; CONST b; <ADD|SUB|MUL>` → `CONST (a op b)`, when both are
/// inline and the result is a non-negative `i32` (inline `CONST`
/// reserves the high bit for the pool flag, so negatives can't fold).
fn try_fold_const_bin(window: &[Byte]) -> Option<Byte> {
    if window.len() < 3 {
        return None;
    }
    let a = const_inline_value(&window[0])? as i64;
    let b = const_inline_value(&window[1])? as i64;
    let result = match *window[2].bytecode() {
        Instruction::ADD => a + b,
        Instruction::SUB => a - b,
        Instruction::MUL => a * b,
        _ => return None,
    };
    if result < 0 || result > i32::MAX as i64 {
        return None;
    }
    Some(Byte::new(Instruction::CONST).with_operand_u32(result as u32))
}

/// `LOAD s; CONST k; <cmp>; JMPF t` → `BinSlotImmJmpf` (pool packs imm + target).
fn try_fuse_load_const_cmp_jmpf(window: &[Byte], pool: &mut Vec<u64>) -> Option<Byte> {
    if window.len() < 4 {
        return None;
    }
    let slot = load_slot(&window[0])?;
    let imm = i16::try_from(const_inline_value(&window[1])?).ok()?;
    let op = *window[2].bytecode();
    if !is_cmp_op(op) {
        return None;
    }
    if *window[3].bytecode() != Instruction::JMPF {
        return None;
    }
    let target = window[3].operand_u32() as u64;
    let idx = pool.len();
    pool.push((target << 32) | (imm as u16 as u32 as u64));
    Some(Byte::new(Instruction::BinSlotImmJmpf).with_bin_slot_imm_jmpf(op as u8, slot, idx as u16))
}

/// `BinSlotImm; JMPF t` → `BinSlotImmJmpf` (second peephole pass).
fn try_fuse_bin_slot_imm_jmpf(window: &[Byte], pool: &mut Vec<u64>) -> Option<Byte> {
    if window.len() < 2 {
        return None;
    }
    if *window[0].bytecode() != Instruction::BinSlotImm {
        return None;
    }
    if *window[1].bytecode() != Instruction::JMPF {
        return None;
    }
    let (op, slot, imm) = window[0].bin_slot_imm_parts();
    if !is_cmp_op(Instruction::from(op)) {
        return None;
    }
    let target = window[1].operand_u32() as u64;
    let idx = pool.len();
    pool.push((target << 32) | (imm as u32 as u64));
    Some(Byte::new(Instruction::BinSlotImmJmpf).with_bin_slot_imm_jmpf(op, slot as u8, idx as u16))
}

/// `LogNot; JMPF t` → `LogNotJmpf`.
fn try_fuse_log_not_jmpf(window: &[Byte]) -> Option<Byte> {
    if window.len() < 2 {
        return None;
    }
    if *window[0].bytecode() != Instruction::LogNot {
        return None;
    }
    if *window[1].bytecode() != Instruction::JMPF {
        return None;
    }
    let target = window[1].operand_u32();
    if target > u16::MAX as u32 {
        return None;
    }
    Some(Byte::new(Instruction::LogNotJmpf).with_log_not_jmpf(target as u16))
}

/// `<cmp>; JMPF t` → `CmpJmpf`.
fn try_fuse_cmp_jmpf(window: &[Byte]) -> Option<Byte> {
    if window.len() < 2 {
        return None;
    }
    let op = *window[0].bytecode();
    if !is_cmp_op(op) {
        return None;
    }
    if *window[1].bytecode() != Instruction::JMPF {
        return None;
    }
    let target = window[1].operand_u32();
    if target > u16::MAX as u32 {
        return None;
    }
    Some(Byte::new(Instruction::CmpJmpf).with_cmp_jmpf(op as u8, target as u16))
}

/// `LOAD s; RETURN` → `LoadReturnSlot`.
fn try_fuse_load_return(window: &[Byte]) -> Option<Byte> {
    if window.len() < 2 {
        return None;
    }
    let slot = load_slot(&window[0])?;
    if *window[1].bytecode() != Instruction::RETURN {
        return None;
    }
    Some(Byte::new(Instruction::LoadReturnSlot).with_operand_u32(slot as u32))
}

/// `CONST k; RETURN` → `ConstReturnImm`.
fn try_fuse_const_return(window: &[Byte]) -> Option<Byte> {
    if window.len() < 2 {
        return None;
    }
    let value = const_inline_value(&window[0])?;
    if *window[1].bytecode() != Instruction::RETURN {
        return None;
    }
    Some(Byte::new(Instruction::ConstReturnImm).with_operand_u32(value as u32))
}

/// `<binop>; RETURN` → `BinReturn`.
fn try_fuse_bin_return(window: &[Byte]) -> Option<Byte> {
    if window.len() < 2 {
        return None;
    }
    let op = *window[0].bytecode();
    if !is_bin_op(op) {
        return None;
    }
    if *window[1].bytecode() != Instruction::RETURN {
        return None;
    }
    Some(Byte::new(Instruction::BinReturn).with_bin_return(op as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fuse(bc: &mut Vec<Byte>) -> Vec<FusionSite> {
        let mut pool = Vec::new();
        fuse_bytecode(bc, &mut pool)
    }

    #[test]
    fn fuse_bin_slot_imm_arith() {
        let mut bc = vec![
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::CONST).with_const_inline(1),
            Byte::new(Instruction::SUB),
            Byte::new(Instruction::HALT),
        ];
        fuse(&mut bc);
        assert_eq!(bc.len(), 2);
        assert_eq!(*bc[0].bytecode(), Instruction::BinSlotImm);
        assert_eq!(bc[0].bin_slot_imm_parts(), (Instruction::SUB as u8, 0, 1));
    }

    #[test]
    fn fuse_bin_slot_imm_comparison() {
        let mut bc = vec![
            Byte::new(Instruction::LOAD).with_operand_u32(2),
            Byte::new(Instruction::CONST).with_const_inline(10),
            Byte::new(Instruction::LEQ),
            Byte::new(Instruction::HALT),
        ];
        fuse(&mut bc);
        assert_eq!(*bc[0].bytecode(), Instruction::BinSlotImm);
        assert_eq!(bc[0].bin_slot_imm_parts(), (Instruction::LEQ as u8, 2, 10));
    }

    #[test]
    fn skip_bin_slot_imm_when_immediate_too_wide() {
        let mut bc = vec![
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::CONST).with_const_inline(40_000),
            Byte::new(Instruction::ADD),
        ];
        fuse(&mut bc);
        assert_eq!(bc.len(), 3);
        assert_eq!(*bc[0].bytecode(), Instruction::LOAD);
    }

    #[test]
    fn fuse_bin_slot_slot_arith() {
        let mut bc = vec![
            Byte::new(Instruction::LOAD).with_operand_u32(1),
            Byte::new(Instruction::LOAD).with_operand_u32(2),
            Byte::new(Instruction::MUL),
            Byte::new(Instruction::HALT),
        ];
        fuse(&mut bc);
        assert_eq!(bc.len(), 2);
        assert_eq!(*bc[0].bytecode(), Instruction::BinSlotSlot);
        assert_eq!(bc[0].bin_slot_slot_parts(), (Instruction::MUL as u8, 1, 2));
    }

    #[test]
    fn fuse_bin_slot_slot_supports_float_ops() {
        let mut bc = vec![
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::LOAD).with_operand_u32(1),
            Byte::new(Instruction::ADDF),
        ];
        fuse(&mut bc);
        assert_eq!(*bc[0].bytecode(), Instruction::BinSlotSlot);
        assert_eq!(bc[0].bin_slot_slot_parts(), (Instruction::ADDF as u8, 0, 1));
    }

    #[test]
    fn load_load_not_followed_by_binop_falls_back() {
        // `LOAD a; LOAD b; RETURN` must not become BinSlotSlot; the
        // trailing `LOAD b; RETURN` fuses to LoadReturnSlot instead.
        let mut bc = vec![
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::LOAD).with_operand_u32(1),
            Byte::new(Instruction::RETURN),
        ];
        fuse(&mut bc);
        assert_eq!(*bc[0].bytecode(), Instruction::LOAD);
        assert_eq!(*bc[1].bytecode(), Instruction::LoadReturnSlot);
    }

    #[test]
    fn const_fold_add() {
        let mut bc = vec![
            Byte::new(Instruction::CONST).with_const_inline(2),
            Byte::new(Instruction::CONST).with_const_inline(3),
            Byte::new(Instruction::ADD),
            Byte::new(Instruction::HALT),
        ];
        fuse(&mut bc);
        assert_eq!(bc.len(), 2);
        assert_eq!(*bc[0].bytecode(), Instruction::CONST);
        assert_eq!(bc[0].operand_u32() as i32, 5);
    }

    #[test]
    fn const_fold_skips_negative_result() {
        // 3 - 5 = -2 can't be an inline CONST (high bit = pool flag),
        // so the convoy is left untouched.
        let mut bc = vec![
            Byte::new(Instruction::CONST).with_const_inline(3),
            Byte::new(Instruction::CONST).with_const_inline(5),
            Byte::new(Instruction::SUB),
        ];
        fuse(&mut bc);
        assert_eq!(bc.len(), 3);
        assert_eq!(*bc[0].bytecode(), Instruction::CONST);
    }

    #[test]
    fn fuse_cmp_jmpf_sequence() {
        let mut bc = vec![
            Byte::new(Instruction::GT),
            Byte::new(Instruction::JMPF).with_operand_u32(9),
            Byte::new(Instruction::HALT),
        ];
        fuse(&mut bc);
        assert_eq!(bc.len(), 2);
        assert_eq!(*bc[0].bytecode(), Instruction::CmpJmpf);
        // The op is preserved; the target 9 is relocated down by the
        // 1 byte this fusion removed → 8.
        assert_eq!(bc[0].cmp_jmpf_parts(), (Instruction::GT as u8, 8));
    }

    #[test]
    fn fuse_load_return_sequence() {
        let mut bc = vec![
            Byte::new(Instruction::LOAD).with_operand_u32(2),
            Byte::new(Instruction::RETURN),
            Byte::new(Instruction::HALT),
        ];
        fuse(&mut bc);
        assert_eq!(bc.len(), 2);
        assert_eq!(*bc[0].bytecode(), Instruction::LoadReturnSlot);
        assert_eq!(bc[0].operand_u32(), 2);
    }

    #[test]
    fn fuse_const_return_sequence() {
        let mut bc = vec![
            Byte::new(Instruction::CONST).with_const_inline(7),
            Byte::new(Instruction::RETURN),
            Byte::new(Instruction::HALT),
        ];
        fuse(&mut bc);
        assert_eq!(bc.len(), 2);
        assert_eq!(*bc[0].bytecode(), Instruction::ConstReturnImm);
        assert_eq!(bc[0].operand_u32() as i32, 7);
    }

    #[test]
    fn fuse_bin_return_covers_many_ops() {
        for op in [
            Instruction::SUB,
            Instruction::MUL,
            Instruction::DIV,
            Instruction::EQ,
            Instruction::LEQ,
            Instruction::GT,
            Instruction::ADDF,
        ] {
            let mut bc = vec![
                Byte::new(op),
                Byte::new(Instruction::RETURN),
                Byte::new(Instruction::HALT),
            ];
            fuse(&mut bc);
            assert_eq!(bc.len(), 2, "op {op:?} should fuse");
            assert_eq!(*bc[0].bytecode(), Instruction::BinReturn);
            assert_eq!(bc[0].bin_return_op(), op as u8);
        }
    }

    #[test]
    fn skip_const_return_when_pool_backed() {
        let mut bc = vec![
            Byte::new(Instruction::CONST).with_const_pool(0),
            Byte::new(Instruction::RETURN),
        ];
        fuse(&mut bc);
        assert_eq!(bc.len(), 2);
        assert_eq!(*bc[0].bytecode(), Instruction::CONST);
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
        fuse(&mut bc);
        // `LOAD; CONST; LEQ; JMPF` fuses to `BinSlotImmJmpf` (−3 bytes).
        let binslot = bc
            .iter()
            .find(|b| *b.bytecode() == Instruction::BinSlotImmJmpf)
            .expect("bin_slot_imm_jmpf fusion");
        assert_eq!(binslot.bin_slot_imm_jmpf_parts().0, Instruction::LEQ as u8);
        let jmp = bc
            .iter()
            .find(|b| *b.bytecode() == Instruction::JMP)
            .expect("jmp");
        // Target 20 sits past windows at 0 (−3) and 4 (−1) and 6 (−2): 20 − 6 = 14.
        assert_eq!(jmp.operand_u32(), 14);
    }

    #[test]
    fn relocates_jump_if_match_pool_target() {
        // A `JUMP_IF_MATCH` whose pool target (10) points past a
        // fused window should have its pool entry shifted down.
        let mut bc = vec![
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::RETURN),
            Byte::new(Instruction::JumpIfMatch).with_operands_u16([0, 0]),
        ];
        let mut pool = vec![10u64];
        fuse_bytecode(&mut bc, &mut pool);
        // One window at 0 removed 1 byte; target 10 > 2 → 10 - 1 = 9.
        assert_eq!(pool[0], 9);
    }

    #[test]
    fn fuse_log_not_jmpf_sequence() {
        let mut bc = vec![
            Byte::new(Instruction::LogNot),
            Byte::new(Instruction::JMPF).with_operand_u32(7),
            Byte::new(Instruction::HALT),
        ];
        fuse(&mut bc);
        assert_eq!(bc.len(), 2);
        assert_eq!(*bc[0].bytecode(), Instruction::LogNotJmpf);
        assert_eq!(bc[0].log_not_jmpf_target(), 6);
    }

    #[test]
    fn fuse_load_const_cmp_jmpf_to_bin_slot_imm_jmpf() {
        let mut bc = vec![
            Byte::new(Instruction::LOAD).with_operand_u32(1),
            Byte::new(Instruction::CONST).with_const_inline(2000),
            Byte::new(Instruction::LEQ),
            Byte::new(Instruction::JMPF).with_operand_u32(20),
            Byte::new(Instruction::HALT),
        ];
        let mut pool = Vec::new();
        fuse_bytecode(&mut bc, &mut pool);
        assert_eq!(bc.len(), 2);
        assert_eq!(*bc[0].bytecode(), Instruction::BinSlotImmJmpf);
        assert_eq!(pool.len(), 1);
        assert_eq!(pool[0] >> 32, 17); // target 20 − 3 bytes removed
        assert_eq!(pool[0] as u32 as i32, 2000);
    }

    #[test]
    fn prologue_jmp_sentinel_is_not_adjusted() {
        let mut bc = vec![
            Byte::new(Instruction::JMP).with_operand_u32(u32::MAX),
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::RETURN),
        ];
        fuse(&mut bc);
        assert_eq!(*bc[0].bytecode(), Instruction::JMP);
        assert_eq!(bc[0].operand_u32(), u32::MAX);
    }

    #[test]
    fn code_ptr_is_relocated_by_patch_targets() {
        // Fuse a 2-byte window before a CodePtr so its absolute target shifts.
        let mut bc = vec![
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::RETURN),
            Byte::new(Instruction::CodePtr).with_operand_u32(10),
            Byte::new(Instruction::MakePolyFn).with_operand_u32(12),
        ];
        fuse(&mut bc);
        // One byte removed in the LOAD;RETURN window → targets 10→9, 12→11.
        let code_ptr = bc
            .iter()
            .find(|b| matches!(b.bytecode(), Instruction::CodePtr))
            .expect("CodePtr preserved");
        assert_eq!(code_ptr.operand_u32(), 9);
        let poly = bc
            .iter()
            .find(|b| matches!(b.bytecode(), Instruction::MakePolyFn))
            .expect("MakePolyFn preserved");
        assert_eq!(poly.operand_u32(), 11);
    }

    #[test]
    fn code_ptr_wide_target_survives_fusion() {
        let wide = 100_000u32;
        let mut bc = vec![
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::RETURN),
            Byte::new(Instruction::CodePtr).with_operand_u32(wide),
        ];
        fuse(&mut bc);
        let code_ptr = bc
            .iter()
            .find(|b| matches!(b.bytecode(), Instruction::CodePtr))
            .expect("CodePtr");
        // Window removed 1 byte; wide target must not truncate to u16.
        assert_eq!(code_ptr.operand_u32(), wide - 1);
        assert!(code_ptr.operand_u32() > u16::MAX as u32);
    }

    #[test]
    fn code_ptr_is_never_folded_as_const_data() {
        // CONST a; CONST b; ADD folds — CodePtr must not participate.
        let mut bc = vec![
            Byte::new(Instruction::CodePtr).with_operand_u32(1),
            Byte::new(Instruction::CONST).with_const_inline(2),
            Byte::new(Instruction::ADD),
        ];
        fuse(&mut bc);
        assert_eq!(bc.len(), 3, "CodePtr; CONST; ADD must not fold");
        assert_eq!(*bc[0].bytecode(), Instruction::CodePtr);
        assert_eq!(bc[0].operand_u32(), 1);
    }

    #[test]
    fn bump_targets_at_or_after_shifts_call_operand() {
        let mut bc = vec![
            Byte::new(Instruction::CONST).with_const_inline(0),
            Byte::new(Instruction::CALL).with_call_packed(1, 5),
            Byte::new(Instruction::HALT),
        ];
        let mut pool = Vec::<u64>::new();
        bump_targets_at_or_after(&mut bc, &mut pool, 3, 2);
        assert_eq!(bc[1].call_parts().1, 7);
    }

    #[test]
    fn code_ptr_return_does_not_fuse_to_const_return() {
        let mut bc = vec![
            Byte::new(Instruction::CodePtr).with_operand_u32(7),
            Byte::new(Instruction::RETURN),
        ];
        fuse(&mut bc);
        assert_eq!(bc.len(), 2);
        assert_eq!(*bc[0].bytecode(), Instruction::CodePtr);
        assert_eq!(*bc[1].bytecode(), Instruction::RETURN);
    }
}
