//! IL → `Vec<Byte>` lowering with label-safe fusion select.

use std::collections::HashMap;

use common::{Byte, DebugLoc, Instruction};

use super::op::{EntryKind, IlJumpKind, IlOp, Label};
use super::opt;

/// Result of lowering an IL module.
pub struct Lowered {
    pub bytecode: Vec<Byte>,
    pub debug_locs: Vec<DebugLoc>,
    /// Final PC for each bound label (last bind wins).
    pub label_pcs: HashMap<u32, usize>,
    /// Pre-fusion emitting index → post-fusion PC (for remapping `functions` etc.).
    pub pre_to_post: HashMap<usize, usize>,
    /// Post-fusion bytecode length.
    pub code_len: usize,
}

/// Intermediate slot before PC assignment. Jump targets stay symbolic.
#[derive(Clone)]
enum Slot {
    Byte(Byte, DebugLoc),
    Jump(IlJumpKind, Label, DebugLoc),
    Entry(EntryKind, u32, Label, DebugLoc),
    PrologueJmp(DebugLoc),
    CmpJmpf(u8, Label, DebugLoc),
    LogNotJmpf(Label, DebugLoc),
    BinSlotImmJmpf {
        op: u8,
        slot: u8,
        imm: i16,
        target: Label,
        loc: DebugLoc,
    },
}

impl Slot {
    fn loc(&self) -> DebugLoc {
        match self {
            Slot::Byte(_, l)
            | Slot::Jump(_, _, l)
            | Slot::Entry(_, _, _, l)
            | Slot::PrologueJmp(l)
            | Slot::CmpJmpf(_, _, l)
            | Slot::LogNotJmpf(_, l)
            | Slot::BinSlotImmJmpf { loc: l, .. } => *l,
        }
    }
}

/// True if `op` is a plain `IlOp::Byte` JMP/JMPF/JMPT carrying a real absolute
/// PC (not the prologue sentinel `u32::MAX`). Symbolic jumps use [`IlOp::Jump`].
pub fn is_residual_abs_jump(op: &IlOp) -> bool {
    let IlOp::Byte { byte, .. } = op else {
        return false;
    };
    matches!(
        *byte.bytecode(),
        Instruction::JMP | Instruction::JMPF | Instruction::JMPT
    ) && byte.operand_u32() != u32::MAX
}

/// Debug/test inventory: panics if any residual absolute control-flow jump
/// remains as `IlOp::Byte`. Allow [`IlOp::PrologueJmp`] / `u32::MAX` only.
pub fn assert_no_residual_abs_jumps(ops: &[IlOp]) {
    for (i, op) in ops.iter().enumerate() {
        debug_assert!(
            !is_residual_abs_jump(op),
            "residual abs JMP/JMPF/JMPT as IlOp::Byte at op index {i}; labelize before opts/fuse"
        );
        if is_residual_abs_jump(op) {
            panic!(
                "residual abs JMP/JMPF/JMPT as IlOp::Byte at op index {i}; labelize before opts/fuse"
            );
        }
    }
}

/// Optimize and lower `ops` into VM bytecode.
pub fn lower(ops: &[IlOp], pool: &mut Vec<u64>) -> Lowered {
    let mut ops = ops.to_vec();
    opt::optimize(&mut ops, &opt::OptimizeOptions::default());
    assert_no_residual_abs_jumps(&ops);

    let mut pre_slots: Vec<Slot> = Vec::with_capacity(ops.len());
    // For each pre-fusion emitting index, labels that bind to it.
    let mut binds_at: HashMap<usize, Vec<u32>> = HashMap::new();
    let mut pending: Vec<u32> = Vec::new();

    for op in &ops {
        match op {
            IlOp::Label(Label(id)) => pending.push(*id),
            IlOp::Byte { byte, loc } => {
                let idx = pre_slots.len();
                if !pending.is_empty() {
                    binds_at.insert(idx, std::mem::take(&mut pending));
                }
                pre_slots.push(Slot::Byte(*byte, *loc));
            }
            IlOp::Jump { kind, target, loc } => {
                let idx = pre_slots.len();
                if !pending.is_empty() {
                    binds_at.insert(idx, std::mem::take(&mut pending));
                }
                pre_slots.push(Slot::Jump(*kind, *target, *loc));
            }
            IlOp::Entry {
                kind,
                arity,
                target,
                loc,
            } => {
                let idx = pre_slots.len();
                if !pending.is_empty() {
                    binds_at.insert(idx, std::mem::take(&mut pending));
                }
                pre_slots.push(Slot::Entry(*kind, *arity, *target, *loc));
            }
            IlOp::PrologueJmp { loc } => {
                let idx = pre_slots.len();
                if !pending.is_empty() {
                    binds_at.insert(idx, std::mem::take(&mut pending));
                }
                pre_slots.push(Slot::PrologueJmp(*loc));
            }
        }
    }
    let end_labels = pending;

    let pre_len = pre_slots.len();
    let (slots, pre_to_post) = fuse_slots_with_origins(pre_slots, pool, &binds_at);

    // Assign in pre-slot order so a rebound label keeps the *last* bind
    // (HashMap iteration order would be nondeterministic).
    let mut label_pcs: HashMap<u32, usize> = HashMap::new();
    for pre in 0..pre_len {
        if let Some(ids) = binds_at.get(&pre) {
            let pc = pre_to_post.get(&pre).copied().unwrap_or(slots.len());
            for id in ids {
                label_pcs.insert(*id, pc);
            }
        }
    }
    for id in end_labels {
        label_pcs.insert(id, slots.len());
    }

    let mut bytecode = Vec::with_capacity(slots.len());
    let mut debug_locs = Vec::with_capacity(slots.len());
    for slot in &slots {
        bytecode.push(encode_slot(slot, &label_pcs, pool));
        debug_locs.push(slot.loc());
    }

    // Symbolic jumps are label-resolved at encode. Residual abs JMP Bytes
    // are forbidden ([`assert_no_residual_abs_jumps`]); this hook is a no-op.
    remap_absolute_targets(&mut bytecode, pool, &pre_to_post, slots.len());

    let code_len = bytecode.len();
    Lowered {
        bytecode,
        debug_locs,
        label_pcs,
        pre_to_post,
        code_len,
    }
}

fn fuse_slots_with_origins(
    slots: Vec<Slot>,
    pool: &mut Vec<u64>,
    binds_at: &HashMap<usize, Vec<u32>>,
) -> (Vec<Slot>, HashMap<usize, usize>) {
    let abs_jump_targets = absolute_jump_targets(&slots);
    let mut out = Vec::with_capacity(slots.len());
    let mut origins: HashMap<usize, usize> = HashMap::new();
    let mut i = 0;
    while i < slots.len() {
        // Do not fuse a window that would pull an op with an incoming
        // label / absolute jump into a fused superinstruction with a
        // preceding op (match joins, attr-inlined absolute JMP→RETURN).
        // *Return fusions also refuse a join on window[0]: JMP-to-join
        // lands on ConstReturnImm/LoadReturnSlot/BinReturn and ignores
        // the stacked arm value (label bind or absolute JMP target).
        let mut fused = None;
        if let Some((f, window)) = try_fuse_slots(&slots[i..], pool) {
            let crosses_label = (1..window).any(|k| binds_at.contains_key(&(i + k)));
            let crosses_abs = (1..window).any(|k| abs_jump_targets.contains(&(i + k)));
            let return_at_join = slot_is_return_fusion(&f)
                && (binds_at.contains_key(&i) || abs_jump_targets.contains(&i));
            if !crosses_label && !crosses_abs && !return_at_join {
                fused = Some((f, window));
            }
        }
        if let Some((fused, window)) = fused {
            let post = out.len();
            for k in 0..window {
                origins.insert(i + k, post);
            }
            out.push(fused);
            i += window;
        } else {
            origins.insert(i, out.len());
            out.push(slots[i].clone());
            i += 1;
        }
    }
    (out, origins)
}

/// Pre-fusion indices targeted by absolute `JMP`/`JMPF`/`JMPT` bytes.
fn absolute_jump_targets(slots: &[Slot]) -> std::collections::HashSet<usize> {
    let mut set = std::collections::HashSet::new();
    for s in slots {
        let Slot::Byte(b, _) = s else {
            continue;
        };
        match *b.bytecode() {
            Instruction::JMP | Instruction::JMPF | Instruction::JMPT => {
                let t = b.operand_u32();
                if t != u32::MAX {
                    set.insert(t as usize);
                }
            }
            _ => {}
        }
    }
    set
}

fn try_fuse_slots(window: &[Slot], pool: &mut Vec<u64>) -> Option<(Slot, usize)> {
    // Peephole order (see `peephole::try_fuse`); JMPF targets stay symbolic.
    if let Some(s) = try_fuse_load_const_cmp_jmpf_slot(window) {
        return Some((s, 4));
    }
    if window.len() >= 3
        && let (Some(a), Some(b), Some(c)) = (
            slot_as_byte(&window[0]),
            slot_as_byte(&window[1]),
            slot_as_byte(&window[2]),
        )
    {
        let w = [a, b, c];
        if let Some(fused) = try_fold_const_bin_local(&w, pool) {
            return Some((Slot::Byte(fused, window[0].loc()), 3));
        }
        if let Some(fused) = try_fuse_bin_slot_imm_local(&w) {
            return Some((Slot::Byte(fused, window[0].loc()), 3));
        }
        if let Some(fused) = try_fuse_bin_slot_slot_local(&w) {
            return Some((Slot::Byte(fused, window[0].loc()), 3));
        }
    }
    if window.len() >= 2
        && let (Some(b0), Slot::Jump(IlJumpKind::JumpIfFalse, tgt, _)) =
            (slot_as_byte(&window[0]), &window[1])
    {
        if *b0.bytecode() == Instruction::BinSlotImm {
            let (op, slot, imm) = b0.bin_slot_imm_parts();
            if is_cmp_op(Instruction::from(op)) {
                return Some((
                    Slot::BinSlotImmJmpf {
                        op,
                        slot: slot as u8,
                        imm: imm as i16,
                        target: *tgt,
                        loc: window[0].loc(),
                    },
                    2,
                ));
            }
        }
        if *b0.bytecode() == Instruction::LogNot {
            return Some((Slot::LogNotJmpf(*tgt, window[0].loc()), 2));
        }
        if is_cmp_op(*b0.bytecode()) {
            return Some((
                Slot::CmpJmpf(*b0.bytecode() as u8, *tgt, window[0].loc()),
                2,
            ));
        }
    }
    if window.len() >= 2
        && let (Some(a), Some(b)) = (slot_as_byte(&window[0]), slot_as_byte(&window[1]))
    {
        let w = [a, b];
        if let Some(fused) = try_fuse_load_return_local(&w) {
            return Some((Slot::Byte(fused, window[0].loc()), 2));
        }
        if let Some(fused) = try_fuse_const_return_local(&w) {
            return Some((Slot::Byte(fused, window[0].loc()), 2));
        }
        if let Some(fused) = try_fuse_bin_return_local(&w) {
            return Some((Slot::Byte(fused, window[0].loc()), 2));
        }
    }
    None
}

fn try_fuse_load_const_cmp_jmpf_slot(window: &[Slot]) -> Option<Slot> {
    if window.len() < 4 {
        return None;
    }
    let b0 = slot_as_byte(&window[0])?;
    let b1 = slot_as_byte(&window[1])?;
    let b2 = slot_as_byte(&window[2])?;
    let Slot::Jump(IlJumpKind::JumpIfFalse, tgt, _) = &window[3] else {
        return None;
    };
    let slot = load_slot(&b0)?;
    let imm = i16::try_from(const_inline_value(&b1)?).ok()?;
    if !is_cmp_op(*b2.bytecode()) {
        return None;
    }
    Some(Slot::BinSlotImmJmpf {
        op: *b2.bytecode() as u8,
        slot,
        imm,
        target: *tgt,
        loc: window[0].loc(),
    })
}

fn slot_as_byte(s: &Slot) -> Option<Byte> {
    match s {
        Slot::Byte(b, _) => Some(*b),
        _ => None,
    }
}

fn slot_is_return_fusion(s: &Slot) -> bool {
    match s {
        Slot::Byte(b, _) => matches!(
            *b.bytecode(),
            Instruction::LoadReturnSlot
                | Instruction::ConstReturnImm
                | Instruction::BinReturn
        ),
        _ => false,
    }
}

fn encode_slot(slot: &Slot, labels: &HashMap<u32, usize>, pool: &mut Vec<u64>) -> Byte {
    match slot {
        Slot::Byte(b, _) => *b,
        Slot::PrologueJmp(_) => Byte::new(Instruction::JMP).with_operand_u32(u32::MAX),
        Slot::Jump(kind, target, _) => {
            let pc = resolve(labels, *target);
            match kind {
                IlJumpKind::Unconditional => Byte::new(Instruction::JMP).with_operand_u32(pc),
                IlJumpKind::JumpIfFalse => Byte::new(Instruction::JMPF).with_operand_u32(pc),
                IlJumpKind::JumpIfTrue => Byte::new(Instruction::JMPT).with_operand_u32(pc),
                IlJumpKind::JumpIfMatch { tag, .. } => {
                    let idx = pool.len() as u16;
                    pool.push(pc as u64);
                    Byte::new(Instruction::JumpIfMatch).with_operands_u16([*tag as u16, idx])
                }
            }
        }
        Slot::Entry(kind, arity, target, _) => {
            let pc = resolve(labels, *target);
            match kind {
                EntryKind::Call => Byte::new(Instruction::CALL).with_call_packed(*arity, pc),
                EntryKind::TailCall => {
                    Byte::new(Instruction::TailCall).with_call_packed(*arity, pc)
                }
                EntryKind::MakeCoro => {
                    Byte::new(Instruction::MakeCoro).with_call_packed(*arity, pc)
                }
                EntryKind::CodePtr => Byte::new(Instruction::CodePtr).with_operand_u32(pc),
                EntryKind::MakePolyFn => Byte::new(Instruction::MakePolyFn).with_operand_u32(pc),
            }
        }
        Slot::CmpJmpf(op, target, _) => {
            let pc = resolve(labels, *target);
            if pc <= u16::MAX as u32 {
                Byte::new(Instruction::CmpJmpf).with_cmp_jmpf(*op, pc as u16)
            } else {
                Byte::new(Instruction::JMPF).with_operand_u32(pc)
            }
        }
        Slot::LogNotJmpf(target, _) => {
            let pc = resolve(labels, *target);
            if pc <= u16::MAX as u32 {
                Byte::new(Instruction::LogNotJmpf).with_log_not_jmpf(pc as u16)
            } else {
                Byte::new(Instruction::JMPF).with_operand_u32(pc)
            }
        }
        Slot::BinSlotImmJmpf {
            op,
            slot,
            imm,
            target,
            ..
        } => {
            let pc = resolve(labels, *target);
            let idx = pool.len();
            pool.push(((pc as u64) << 32) | (*imm as u16 as u32 as u64));
            Byte::new(Instruction::BinSlotImmJmpf).with_bin_slot_imm_jmpf(*op, *slot, idx as u16)
        }
    }
}

fn resolve(labels: &HashMap<u32, usize>, target: Label) -> u32 {
    *labels.get(&target.0).unwrap_or(&0) as u32
}

/// Remap residual absolute jump targets that still use pre-fusion indices.
/// Symbolic [`IlOp::Jump`] / fused jmp forms are already label-resolved at
/// encode — do not touch them (double-remap breaks loop exits under fusion).
/// Leftover CALL/CodePtr Bytes (defer frame-setup target 0, missing fn
/// CodePtr 0) are not fusion-sensitive and are left as-is.
fn remap_absolute_targets(
    bytecode: &mut [Byte],
    pool: &mut [u64],
    pre_to_post: &HashMap<usize, usize>,
    len: usize,
) {
    let _ = (bytecode, pool, pre_to_post, len);
    // Production emit has no residual abs JMP/JMPF/JMPT as `IlOp::Byte`
    // (see [`assert_no_residual_abs_jumps`]). Prologue uses `u32::MAX`.
}

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

fn try_fuse_bin_slot_imm_local(window: &[Byte; 3]) -> Option<Byte> {
    let slot = load_slot(&window[0])?;
    let imm = i16::try_from(const_inline_value(&window[1])?).ok()?;
    let op = *window[2].bytecode();
    if !is_int_bin_op(op) {
        return None;
    }
    Some(Byte::new(Instruction::BinSlotImm).with_bin_slot_imm(op as u8, slot, imm))
}

fn try_fuse_bin_slot_slot_local(window: &[Byte; 3]) -> Option<Byte> {
    let a = load_slot(&window[0])?;
    let b = load_slot(&window[1])?;
    let op = *window[2].bytecode();
    if !is_bin_op(op) {
        return None;
    }
    Some(Byte::new(Instruction::BinSlotSlot).with_bin_slot_slot(op as u8, a, b))
}

fn try_fold_const_bin_local(window: &[Byte; 3], pool: &mut Vec<u64>) -> Option<Byte> {
    let a = const_inline_value(&window[0])? as i64;
    let b = const_inline_value(&window[1])? as i64;
    let result = match *window[2].bytecode() {
        Instruction::ADD => a + b,
        Instruction::SUB => a - b,
        Instruction::MUL => a * b,
        Instruction::DIV if b != 0 => a / b,
        Instruction::MOD if b != 0 => a % b,
        _ => return None,
    };
    if (0..=i32::MAX as i64).contains(&result) {
        return Some(Byte::new(Instruction::CONST).with_operand_u32(result as u32));
    }
    let bits = common::Value::from(result).raw() as u64;
    let idx = pool.len();
    pool.push(bits);
    Some(Byte::new(Instruction::CONST).with_const_pool(idx as u32))
}

fn try_fuse_load_return_local(window: &[Byte; 2]) -> Option<Byte> {
    let slot = load_slot(&window[0])?;
    if *window[1].bytecode() != Instruction::RETURN {
        return None;
    }
    Some(Byte::new(Instruction::LoadReturnSlot).with_operand_u32(slot as u32))
}

fn try_fuse_const_return_local(window: &[Byte; 2]) -> Option<Byte> {
    let value = const_inline_value(&window[0])?;
    if *window[1].bytecode() != Instruction::RETURN {
        return None;
    }
    Some(Byte::new(Instruction::ConstReturnImm).with_operand_u32(value as u32))
}

fn try_fuse_bin_return_local(window: &[Byte; 2]) -> Option<Byte> {
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
    use crate::il::IlBuilder;

    #[test]
    fn lower_resolves_forward_jmp() {
        let mut il = IlBuilder::new();
        let end = il.fresh_label();
        il.emit_jump(IlJumpKind::Unconditional, end);
        // Live code after JMP must be entered via a label (thunk style).
        il.bind_label(end);
        il.push_byte(Byte::new(Instruction::CONST).with_const_inline(1));
        il.push_byte(Byte::new(Instruction::HALT));

        let mut pool = Vec::new();
        let lowered = lower(il.ops(), &mut pool);
        assert_eq!(lowered.bytecode.len(), 3);
        assert_eq!(lowered.bytecode[0].operand_u32(), 1);
        assert!(matches!(
            *lowered.bytecode[2].bytecode(),
            Instruction::HALT
        ));
    }

    #[test]
    fn lower_fuses_bin_slot_slot() {
        let mut il = IlBuilder::new();
        il.push_byte(Byte::new(Instruction::LOAD).with_operand_u32(0));
        il.push_byte(Byte::new(Instruction::LOAD).with_operand_u32(1));
        il.push_byte(Byte::new(Instruction::ADD));
        il.push_byte(Byte::new(Instruction::RETURN));
        let mut pool = Vec::new();
        let lowered = lower(il.ops(), &mut pool);
        assert_eq!(lowered.bytecode.len(), 2);
        assert!(matches!(
            *lowered.bytecode[0].bytecode(),
            Instruction::BinSlotSlot
        ));
        assert!(matches!(
            *lowered.bytecode[1].bytecode(),
            Instruction::RETURN
        ));
    }

        #[test]
    fn lower_resolves_jmpf_with_label() {
        let mut il = IlBuilder::new();
        let exit = il.fresh_label();
        il.push_byte(Byte::new(Instruction::EQ));
        il.emit_jump(IlJumpKind::JumpIfFalse, exit);
        il.push_byte(Byte::new(Instruction::CONST).with_const_inline(1));
        il.bind_label(exit);
        il.push_byte(Byte::new(Instruction::HALT));

        let mut pool = Vec::new();
        let lowered = lower(il.ops(), &mut pool);
        // EQ; JMPF fuses to CmpJmpf (target still label-resolved).
        assert!(matches!(
            *lowered.bytecode[0].bytecode(),
            Instruction::CmpJmpf
        ));
        assert_eq!(lowered.bytecode[0].cmp_jmpf_parts().1, 2);
    }

    #[test]
    fn lower_resolves_entry_call() {
        let mut il = IlBuilder::new();
        let entry = il.fresh_label();
        il.emit_entry(EntryKind::Call, 1, entry);
        il.push_byte(Byte::new(Instruction::HALT));
        il.bind_label(entry);
        il.push_byte(Byte::new(Instruction::RETURN));

        let mut pool = Vec::new();
        let lowered = lower(il.ops(), &mut pool);
        assert!(matches!(
            *lowered.bytecode[0].bytecode(),
            Instruction::CALL
        ));
        assert_eq!(lowered.bytecode[0].call_parts(), (1, 2));
    }

    #[test]
    fn residual_abs_jmp_byte_is_detected() {
        let ops = vec![IlOp::byte(
            Byte::new(Instruction::JMP).with_operand_u32(42),
        )];
        assert!(is_residual_abs_jump(&ops[0]));
    }

    #[test]
    fn prologue_sentinel_jmp_is_not_residual() {
        let ops = vec![
            IlOp::PrologueJmp {
                loc: DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::JMP).with_operand_u32(u32::MAX)),
        ];
        assert!(!is_residual_abs_jump(&ops[0]));
        assert!(!is_residual_abs_jump(&ops[1]));
        assert_no_residual_abs_jumps(&ops);
    }

    #[test]
    #[should_panic(expected = "residual abs JMP")]
    fn lower_panics_on_residual_abs_jmp_byte() {
        let ops = vec![
            IlOp::byte(Byte::new(Instruction::JMP).with_operand_u32(1)),
            IlOp::byte(Byte::new(Instruction::HALT)),
        ];
        let mut pool = Vec::new();
        let _ = lower(&ops, &mut pool);
    }
}
