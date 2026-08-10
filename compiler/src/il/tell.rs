//! Shared operand/local cursor (`tell`) analysis over final bytecode.
//!
//! [`super::sp`] tracks operand *height*, which is not the same quantity: locals
//! and the operand stack share one buffer, and `STORE` raises the cursor to
//! `slot + 1` regardless of height. Passes that delete a store change the cursor
//! and can therefore move a callee frame over slots that are still live — see
//! `docs/internals/limitations.md`.
//!
//! The bytecode path is checked against the VM directly, while the symbolic-IL
//! path feeds cursor-safe pre-lower optimizations. `tell_cursor_model_matches_vm`
//! in `compiler/tests/cursor_model.rs` diffs every bytecode prediction against
//! the real cursor recorded by `machine::cursor_trace`.
//!
//! **The cursor is not a per-PC constant.** A loop whose body stores to a higher
//! slot each pass reaches its header with a different cursor on the back edge
//! than on first entry, so [`Tell::Unknown`] at a join is often the correct
//! answer rather than a gap — see
//! `loop_header_cursor_is_unknown_when_the_body_stores_higher`. A pass asking
//! "is it safe to delete this store?" should therefore compare the cursor
//! *with and without* the store, since that difference propagates identically
//! along a path even where the absolute value is unknown. This module supplies
//! the validated per-op rules that such a relative analysis needs.

use common::{Byte, Instruction};

use super::op::{EntryKind, IlJumpKind, IlOp};

/// Frame-relative cursor before an instruction.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Tell {
    Known(u32),
    Unknown,
}

impl Tell {
    pub fn known(self) -> Option<u32> {
        match self {
            Tell::Known(v) => Some(v),
            Tell::Unknown => None,
        }
    }
}

/// How one instruction moves the cursor.
#[derive(Copy, Clone, Debug)]
enum Effect {
    /// Net push/pop, as for operand height.
    Delta(i32),
    /// Net push/pop, then raise to at least `floor` (`STORE`, `BinSlot*Store`).
    DeltaThenFloor(i32, u32),
    /// Absolute set (`Seek`).
    Set(u32),
    /// Control leaves this frame; no fall-through cursor.
    Terminator,
    /// Effect not modelled — poisons the rest of the path.
    Unknown,
}

/// Per-instruction cursor-in, indexed by PC.
#[derive(Clone, Debug)]
pub struct TellInfo {
    pub tell_in: Vec<Tell>,
}

impl TellInfo {
    pub fn tell_before(&self, pc: usize) -> Tell {
        self.tell_in.get(pc).copied().unwrap_or(Tell::Unknown)
    }

    /// Share of instructions the analysis resolved (diagnostics / tests).
    pub fn coverage(&self) -> f64 {
        if self.tell_in.is_empty() {
            return 1.0;
        }
        let known = self.tell_in.iter().filter(|t| t.known().is_some()).count();
        known as f64 / self.tell_in.len() as f64
    }

    /// True when deleting a one-value producer followed by `STORE slot` keeps
    /// the shared cursor unchanged at the pair's continuation.
    pub fn can_remove_one_value_store(&self, producer_idx: usize, slot: u32) -> bool {
        let Some(before) = self.tell_before(producer_idx).known() else {
            return false;
        };
        let after_store = apply(
            Tell::Known(before.saturating_add(1)),
            Effect::DeltaThenFloor(-1, slot.saturating_add(1)),
        );
        after_store == Some(Tell::Known(before))
    }
}

/// `BinSlotImmStore` hides its destination slot in the high half of the pool
/// entry, so the pool is needed to resolve the cursor floor.
fn effect(byte: &Byte, pool: &[u64]) -> Effect {
    let insn = *byte.bytecode();
    match insn {
        Instruction::Seek => Effect::Set(byte.operand_u32()),
        Instruction::STORE | Instruction::StorePop => {
            let n = byte.load_store_count();
            let mut max_slot = 0u32;
            for i in 0..n {
                max_slot = max_slot.max(byte.load_store_slot_at(i));
            }
            Effect::DeltaThenFloor(-(n as i32), max_slot + 1)
        }
        Instruction::BinSlotSlotStore => {
            let (_, _, _, dest) = byte.bin_slot_slot_store_parts();
            Effect::DeltaThenFloor(0, dest as u32 + 1)
        }
        Instruction::BinSlotImmStore => {
            let (_, _, pool_idx) = byte.bin_slot_imm_store_parts();
            match pool.get(pool_idx) {
                Some(packed) => Effect::DeltaThenFloor(0, (*packed >> 32) as u32 + 1),
                None => Effect::Unknown,
            }
        }
        // Pops the scrutinee, pushes the payload. The VM uses the runtime
        // payload length and ignores the operand, but they agree for well-typed
        // code — the same assumption `sp` makes.
        Instruction::Unpack => Effect::Delta(byte.operand_u32() as i32 - 1),
        // A frame that returns or is replaced leaves no fall-through cursor.
        Instruction::RETURN
        | Instruction::HALT
        | Instruction::LoadReturnSlot
        | Instruction::ConstReturnImm
        | Instruction::BinReturn
        | Instruction::ReturnPair
        | Instruction::TailCall => Effect::Terminator,
        other => match super::sp::byte_stack_delta(other, byte) {
            Some(d) => Effect::Delta(d),
            None => Effect::Unknown,
        },
    }
}

/// Cursor effect on the fall-through and branch-taken edges.
///
/// They differ for `JumpIfMatch`, which only pops the scrutinee (and pushes the
/// payload) when the tag matches. The payload arity is not encoded in the byte —
/// the VM reads it from the runtime enum — so the taken edge is unmodelled here.
/// An IL-level model can do better: `IlJumpKind::JumpIfMatch` carries `arity`.
fn edge_effects(byte: &Byte, pool: &[u64]) -> (Effect, Effect) {
    match *byte.bytecode() {
        Instruction::JumpIfMatch => (Effect::Delta(0), Effect::Unknown),
        Instruction::PairJumpIfTag => (Effect::Delta(0), Effect::Delta(-1)),
        _ => {
            let e = effect(byte, pool);
            (e, e)
        }
    }
}

/// True when `byte`'s cursor effect is modelled on both edges. A pass can check
/// this to refuse a region up front instead of walking into `Unknown`.
pub fn is_modelled(byte: &Byte, pool: &[u64]) -> bool {
    let (fall, branch) = edge_effects(byte, pool);
    !matches!(fall, Effect::Unknown) && !matches!(branch, Effect::Unknown)
}

fn apply(before: Tell, eff: Effect) -> Option<Tell> {
    let cur = match before {
        Tell::Known(v) => v,
        Tell::Unknown => {
            return match eff {
                Effect::Set(v) => Some(Tell::Known(v)),
                Effect::Terminator => None,
                _ => Some(Tell::Unknown),
            };
        }
    };
    match eff {
        Effect::Delta(d) => Some(Tell::Known(shift(cur, d))),
        Effect::DeltaThenFloor(d, floor) => Some(Tell::Known(shift(cur, d).max(floor))),
        Effect::Set(v) => Some(Tell::Known(v)),
        Effect::Terminator => None,
        Effect::Unknown => Some(Tell::Unknown),
    }
}

fn shift(cur: u32, delta: i32) -> u32 {
    if delta >= 0 {
        cur.saturating_add(delta as u32)
    } else {
        cur.saturating_sub(delta.unsigned_abs())
    }
}

/// Absolute jump target for the branch forms that carry one.
///
/// The fused compares keep a 16-bit field that is either a direct PC or a pool
/// index; `BinSlot*Jmpf` always packs the target in the pool entry's high half
/// because the immediate/slot uses the low bits.
fn jump_target(byte: &Byte, pool: &[u64]) -> Option<usize> {
    match *byte.bytecode() {
        Instruction::JMP | Instruction::JMPF | Instruction::JMPT => {
            Some(byte.operand_u32() as usize)
        }
        Instruction::CmpJmpf => {
            let (_, t) = byte.cmp_jmpf_parts();
            if byte.cmp_jmpf_is_pool() {
                Some(*pool.get(t)? as usize)
            } else {
                Some(t)
            }
        }
        Instruction::LogNotJmpf => {
            let t = byte.log_not_jmpf_target();
            if byte.log_not_jmpf_is_pool() {
                Some(*pool.get(t)? as usize)
            } else {
                Some(t)
            }
        }
        Instruction::BinSlotImmJmpf => {
            let (_, _, pool_idx) = byte.bin_slot_imm_jmpf_parts();
            Some((*pool.get(pool_idx)? >> 32) as usize)
        }
        Instruction::BinSlotSlotJmpf => {
            let (_, _, pool_idx) = byte.bin_slot_slot_jmpf_parts();
            Some((*pool.get(pool_idx)? >> 32) as usize)
        }
        Instruction::BinSlotSlotConstJmpf => {
            let (_, _, pool_idx) = byte.bin_slot_slot_const_jmpf_parts();
            let packed = *pool.get(pool_idx)?;
            Some((packed >> 32) as usize)
        }
        Instruction::JumpIfMatch | Instruction::PairJumpIfTag => {
            Some(byte.jump_if_match_target(pool))
        }
        _ => None,
    }
}

/// True when control cannot fall through to the next instruction.
fn is_unconditional_transfer(byte: &Byte) -> bool {
    matches!(
        *byte.bytecode(),
        Instruction::JMP
            | Instruction::RETURN
            | Instruction::ReturnPair
            | Instruction::HALT
            | Instruction::LoadReturnSlot
            | Instruction::ConstReturnImm
            | Instruction::BinReturn
            | Instruction::TailCall
    )
}

/// `JumpIfMatch` taken edge: pop the scrutinee, push `arity` payloads.
fn signed_arity_delta(arity: u32) -> i32 {
    arity.min(i32::MAX as u32) as i32 - 1
}

/// `CALL` / `MakeCoro`: pop `arity` args, push one result. The callee frame base
/// is `tell - arity` and the matching return seeks back to it before pushing.
fn call_arity_delta(arity: u32) -> i32 {
    1 - arity.min(i32::MAX as u32) as i32
}

fn effect_il(op: &IlOp, pool: &[u64]) -> Effect {
    match op {
        IlOp::StorePop { slot, .. } => Effect::DeltaThenFloor(-1, slot.saturating_add(1)),
        IlOp::Entry { kind, arity, .. } => match kind {
            EntryKind::Call | EntryKind::MakeCoro => Effect::Delta(call_arity_delta(*arity)),
            EntryKind::TailCall => Effect::Terminator,
            EntryKind::CodePtr | EntryKind::MakePolyFn => Effect::Delta(1),
        },
        IlOp::PrologueJmp { .. } => Effect::Terminator,
        IlOp::Byte { byte, .. } => effect(byte, pool),
        IlOp::Jump { .. } => Effect::Unknown,
        _ => match super::sp::stack_delta(op) {
            Some(delta) => Effect::Delta(delta),
            None => Effect::Unknown,
        },
    }
}

fn edge_effects_il(op: &IlOp, pool: &[u64]) -> (Effect, Effect) {
    if let IlOp::Jump { kind, .. } = op {
        return match kind {
            IlJumpKind::Unconditional => (Effect::Delta(0), Effect::Delta(0)),
            IlJumpKind::JumpIfFalse | IlJumpKind::JumpIfTrue => {
                (Effect::Delta(-1), Effect::Delta(-1))
            }
            IlJumpKind::JumpIfMatch { arity, .. } => {
                (Effect::Delta(0), Effect::Delta(signed_arity_delta(*arity)))
            }
        };
    }
    let effect = effect_il(op, pool);
    (effect, effect)
}

fn il_jump_target(op: &IlOp, labels: &std::collections::HashMap<u32, usize>) -> Option<usize> {
    match op {
        IlOp::Jump { target, .. } => labels.get(&target.0).copied(),
        _ => None,
    }
}

fn il_is_unconditional_transfer(op: &IlOp) -> bool {
    matches!(
        op,
        IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            ..
        } | IlOp::Entry {
            kind: EntryKind::TailCall,
            ..
        } | IlOp::PrologueJmp { .. }
    ) || matches!(
        op,
        IlOp::Return { .. }
            | IlOp::Halt { .. }
            | IlOp::LoadReturnSlot { .. }
            | IlOp::ConstReturnImm { .. }
            | IlOp::BinReturn { .. }
    ) || matches!(
        op,
        IlOp::Byte { byte, .. }
            if is_unconditional_transfer(byte)
    )
}

fn analyze_cfg(
    n: usize,
    entry: usize,
    entry_tell: u32,
    mut edge_effects: impl FnMut(usize) -> (Effect, Effect),
    mut jump_target: impl FnMut(usize) -> Option<usize>,
    mut is_unconditional_transfer: impl FnMut(usize) -> bool,
) -> TellInfo {
    let mut tell_in: Vec<Option<Tell>> = vec![None; n];
    if entry < n {
        tell_in[entry] = Some(Tell::Known(entry_tell));
    }

    // Disagreeing predecessors poison rather than pick a side.
    fn meet(slot: &mut Option<Tell>, incoming: Tell) -> bool {
        let next = match *slot {
            None => incoming,
            Some(Tell::Unknown) => Tell::Unknown,
            Some(Tell::Known(a)) => match incoming {
                Tell::Known(b) if a == b => Tell::Known(a),
                _ => Tell::Unknown,
            },
        };
        if *slot != Some(next) {
            *slot = Some(next);
            true
        } else {
            false
        }
    }

    for _ in 0..n.saturating_mul(2).max(8) {
        let mut changed = false;
        for pc in 0..n {
            let Some(before) = tell_in[pc] else {
                continue;
            };
            let (fall_eff, branch_eff) = edge_effects(pc);

            if let Some(target) = jump_target(pc)
                && target < n
                && let Some(edge) = apply(before, branch_eff)
            {
                changed |= meet(&mut tell_in[target], edge);
            }
            if !is_unconditional_transfer(pc)
                && pc + 1 < n
                && let Some(edge) = apply(before, fall_eff)
            {
                changed |= meet(&mut tell_in[pc + 1], edge);
            }
        }
        if !changed {
            break;
        }
    }

    TellInfo {
        tell_in: tell_in
            .into_iter()
            .map(|tell| tell.unwrap_or(Tell::Unknown))
            .collect(),
    }
}

/// Compute cursor-in per PC for one function body, seeded at `entry_tell`.
///
/// A function entry has its arguments already in slots `0..arity`, so the
/// caller passes `arity` (`CALL` sets the callee frame base at `tell - arity`).
pub fn analyze_at(code: &[Byte], pool: &[u64], entry: usize, entry_tell: u32) -> TellInfo {
    analyze_cfg(
        code.len(),
        entry,
        entry_tell,
        |pc| edge_effects(&code[pc], pool),
        |pc| jump_target(&code[pc], pool),
        |pc| is_unconditional_transfer(&code[pc]),
    )
}

/// Compute cursor-in per symbolic IL op, before lowering assigns PCs.
///
/// This is the optimizer-facing sibling of [`analyze_at`]. Symbolic labels
/// make the CFG exact, while residual `Byte` ops reuse the bytecode rules.
pub fn analyze_il_at(ops: &[IlOp], entry_tell: u32) -> TellInfo {
    let labels: std::collections::HashMap<u32, usize> = ops
        .iter()
        .enumerate()
        .filter_map(|(idx, op)| match op {
            IlOp::Label(label) => Some((label.0, idx)),
            _ => None,
        })
        .collect();
    analyze_cfg(
        ops.len(),
        0,
        entry_tell,
        |idx| edge_effects_il(&ops[idx], &[]),
        |idx| il_jump_target(&ops[idx], &labels),
        |idx| il_is_unconditional_transfer(&ops[idx]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::il::op::Label;

    fn store(slot: u32) -> Byte {
        Byte::new(Instruction::STORE).with_load_store_slot(slot)
    }

    fn cursors(code: &[Byte]) -> Vec<Option<u32>> {
        analyze_at(code, &[], 0, 0)
            .tell_in
            .iter()
            .map(|t| t.known())
            .collect()
    }

    #[test]
    fn store_raises_cursor_above_the_written_slot() {
        // CONST; STORE 5 → cursor 6, not 0: the store protects slots 0..=5.
        let code = vec![
            Byte::new(Instruction::CONST).with_const_inline(1),
            store(5),
            Byte::new(Instruction::NOOP),
        ];
        assert_eq!(cursors(&code), vec![Some(0), Some(1), Some(6)]);
    }

    #[test]
    fn store_to_a_low_slot_does_not_lower_the_cursor() {
        // Height returns to 3, but the floor of 1 must not pull the cursor down.
        let code = vec![
            Byte::new(Instruction::CONST).with_const_inline(1),
            Byte::new(Instruction::CONST).with_const_inline(2),
            Byte::new(Instruction::CONST).with_const_inline(3),
            Byte::new(Instruction::CONST).with_const_inline(4),
            store(0),
            Byte::new(Instruction::NOOP),
        ];
        assert_eq!(cursors(&code).last().copied().unwrap(), Some(3));
    }

    #[test]
    fn pop_lowers_the_cursor_below_a_previous_store_floor() {
        // Cursor is not monotone: POP moves it back below the store's floor.
        let code = vec![
            Byte::new(Instruction::CONST).with_const_inline(1),
            store(4),                                        // cursor -> 5
            Byte::new(Instruction::CONST).with_const_inline(2), // -> 6
            Byte::new(Instruction::POP),                     // -> 5
            Byte::new(Instruction::NOOP),
        ];
        assert_eq!(cursors(&code).last().copied().unwrap(), Some(5));
    }

    #[test]
    fn seek_sets_the_cursor_absolutely() {
        let code = vec![
            Byte::new(Instruction::CONST).with_const_inline(1),
            store(9),
            Byte::new(Instruction::Seek).with_operand_u32(2),
            Byte::new(Instruction::NOOP),
        ];
        assert_eq!(cursors(&code).last().copied().unwrap(), Some(2));
    }

    #[test]
    fn call_pops_args_and_pushes_one_result() {
        // Frame base is `tell - arity`; the return seeks back and pushes.
        let code = vec![
            Byte::new(Instruction::CONST).with_const_inline(1),
            Byte::new(Instruction::CONST).with_const_inline(2),
            Byte::new(Instruction::CALL).with_call_packed(2, 40),
            Byte::new(Instruction::NOOP),
        ];
        assert_eq!(cursors(&code).last().copied().unwrap(), Some(1));
    }

    #[test]
    fn entry_tell_seeds_the_argument_slots() {
        let code = vec![
            Byte::new(Instruction::LOAD).with_load_store_slot(0),
            Byte::new(Instruction::NOOP),
        ];
        let info = analyze_at(&code, &[], 0, 3);
        assert_eq!(info.tell_before(0).known(), Some(3));
        assert_eq!(info.tell_before(1).known(), Some(4));
    }

    #[test]
    fn symbolic_il_store_pair_proof_requires_existing_cursor_floor() {
        let loc = common::DebugLoc::unknown();
        let ops = vec![
            IlOp::Const { imm: 1, loc },
            IlOp::StorePop { slot: 5, loc },
            IlOp::Return { loc },
        ];

        let low = analyze_il_at(&ops, 0);
        assert!(!low.can_remove_one_value_store(0, 5));

        let high = analyze_il_at(&ops, 6);
        assert!(high.can_remove_one_value_store(0, 5));
    }

    /// Symbolic JMPF joins both edges with the same post-pop cursor.
    #[test]
    fn symbolic_il_jump_if_false_joins_both_edges() {
        let loc = common::DebugLoc::unknown();
        let ops = vec![
            IlOp::Const { imm: 1, loc },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(0),
                loc,
            },
            IlOp::Return { loc },
            IlOp::Label(Label(0)),
            IlOp::Return { loc },
        ];
        let info = analyze_il_at(&ops, 0);
        // Const → 1; JMPF pops → 0 on fall-through and taken.
        assert_eq!(info.tell_before(2).known(), Some(0));
        assert_eq!(info.tell_before(3).known(), Some(0));
    }

    /// `Entry{Call}` must agree with the bytecode `CALL` rule: `-arity + 1`.
    #[test]
    fn symbolic_il_call_pops_args_and_pushes_one_result() {
        let loc = common::DebugLoc::unknown();
        let ops = vec![
            IlOp::Load { slot: 0, loc },
            IlOp::Load { slot: 1, loc },
            IlOp::Load { slot: 2, loc },
            IlOp::Entry {
                kind: crate::il::op::EntryKind::Call,
                arity: 3,
                target: Label(0),
                loc,
            },
            IlOp::Return { loc },
        ];
        let info = analyze_il_at(&ops, 3);
        assert_eq!(info.tell_before(3).known(), Some(6));
        assert_eq!(info.tell_before(4).known(), Some(4));
    }

    /// Arity 0 is the sharpest anti-regression vs the old `arity - 1` delta (+0):
    /// a zero-arg call must still push one result (`delta = +1`).
    #[test]
    fn symbolic_il_call_arity_zero_pushes_one() {
        let loc = common::DebugLoc::unknown();
        let ops = vec![
            IlOp::Entry {
                kind: crate::il::op::EntryKind::Call,
                arity: 0,
                target: Label(0),
                loc,
            },
            IlOp::Return { loc },
        ];
        let info = analyze_il_at(&ops, 2);
        assert_eq!(info.tell_before(0).known(), Some(2));
        assert_eq!(info.tell_before(1).known(), Some(3));
    }

    /// `MakeCoro` shares `call_arity_delta` with `Call` — keep them locked together.
    #[test]
    fn symbolic_il_make_coro_matches_call_delta() {
        let loc = common::DebugLoc::unknown();
        for arity in [0u32, 1, 3] {
            let call = vec![
                IlOp::Entry {
                    kind: crate::il::op::EntryKind::Call,
                    arity,
                    target: Label(0),
                    loc,
                },
                IlOp::Return { loc },
            ];
            let coro = vec![
                IlOp::Entry {
                    kind: crate::il::op::EntryKind::MakeCoro,
                    arity,
                    target: Label(0),
                    loc,
                },
                IlOp::Return { loc },
            ];
            let entry = arity + 1;
            let after_call = analyze_il_at(&call, entry).tell_before(1).known();
            let after_coro = analyze_il_at(&coro, entry).tell_before(1).known();
            assert_eq!(
                after_call, after_coro,
                "MakeCoro arity {arity} must match Call"
            );
            assert_eq!(
                after_call,
                Some(entry + 1 - arity),
                "Call/MakeCoro arity {arity}"
            );
        }
    }

    /// Unlike bytecode JumpIfMatch, IL carries arity so the taken edge is modelled.
    #[test]
    fn symbolic_il_jump_if_match_models_taken_edge_with_arity() {
        let loc = common::DebugLoc::unknown();
        let ops = vec![
            IlOp::Jump {
                kind: IlJumpKind::JumpIfMatch { tag: 0, arity: 2 },
                target: Label(0),
                loc,
            },
            IlOp::Return { loc },
            IlOp::Label(Label(0)),
            IlOp::Return { loc },
        ];
        let info = analyze_il_at(&ops, 3);
        // Fall-through keeps cursor; taken edge applies arity-1 (= +1) → 4.
        assert_eq!(info.tell_before(1).known(), Some(3));
        assert_eq!(info.tell_before(2).known(), Some(4));
    }

    /// The cursor is genuinely not a per-PC constant: a loop that stores to a
    /// higher slot each pass reaches its header with a different cursor on the
    /// back edge than on first entry, so `Unknown` there is correct rather than a
    /// modelling gap. Consumers should therefore reason about the *change* a
    /// rewrite makes to the cursor, not its absolute value.
    #[test]
    fn loop_header_cursor_is_unknown_when_the_body_stores_higher() {
        let code = vec![
            Byte::new(Instruction::JMP).with_operand_u32(1), // preheader
            // header
            Byte::new(Instruction::CONST).with_const_inline(1),
            store(7), // body raises the cursor to 8
            Byte::new(Instruction::JMP).with_operand_u32(1),
        ];
        let info = analyze_at(&code, &[], 0, 0);
        // First entry arrives with 0, the back edge with 8.
        assert_eq!(info.tell_before(1), Tell::Unknown);
    }

    #[test]
    fn disagreeing_predecessors_poison() {
        // Fall-through arrives with 1, the back-branch with 0.
        let code = vec![
            Byte::new(Instruction::CONST).with_const_inline(1),
            Byte::new(Instruction::JMPF).with_operand_u32(3),
            Byte::new(Instruction::CONST).with_const_inline(2),
            Byte::new(Instruction::NOOP),
        ];
        let info = analyze_at(&code, &[], 0, 0);
        assert_eq!(info.tell_before(3), Tell::Unknown);
    }

    #[test]
    fn unmodelled_opcode_poisons_downstream() {
        let code = vec![
            Byte::new(Instruction::FfiInvoke).with_operand_u32(0),
            Byte::new(Instruction::NOOP),
        ];
        let info = analyze_at(&code, &[], 0, 0);
        assert_eq!(info.tell_before(0).known(), Some(0));
        assert_eq!(info.tell_before(1), Tell::Unknown);
    }

    /// Packed multi-slot STORE floors to `max(slot) + 1`, not the first slot.
    #[test]
    fn packed_store_floors_to_the_highest_written_slot() {
        let code = vec![
            Byte::new(Instruction::CONST).with_const_inline(1),
            Byte::new(Instruction::CONST).with_const_inline(2),
            Byte::new(Instruction::STORE).with_load_store_packed(2, 3, 7, 0),
            Byte::new(Instruction::NOOP),
        ];
        // Two consts → cursor 2; pop 2 then floor at 8.
        assert_eq!(cursors(&code).last().copied().unwrap(), Some(8));
    }

    /// Fused `BinSlotSlotStore` has no stack traffic but still protects `dest`.
    #[test]
    fn bin_slot_slot_store_raises_cursor_without_stack_delta() {
        let code = vec![
            Byte::new(Instruction::BinSlotSlotStore).with_bin_slot_slot_store(
                Instruction::BITAND as u8,
                0,
                1,
                5,
            ),
            Byte::new(Instruction::NOOP),
        ];
        assert_eq!(cursors(&code), vec![Some(0), Some(6)]);
    }

    /// Dest lives in the pool high half — a wrong decode silently under-floors.
    #[test]
    fn bin_slot_imm_store_floors_from_pool_dest() {
        let pool = vec![(5u64 << 32) | 1];
        let code = vec![
            Byte::new(Instruction::BinSlotImmStore).with_bin_slot_imm_store(
                Instruction::ADD as u8,
                0,
                0,
            ),
            Byte::new(Instruction::NOOP),
        ];
        let info = analyze_at(&code, &pool, 0, 0);
        assert_eq!(info.tell_before(1).known(), Some(6));
        assert!(is_modelled(&code[0], &pool));
    }

    #[test]
    fn bin_slot_imm_store_with_missing_pool_entry_is_unmodelled() {
        let code = vec![
            Byte::new(Instruction::BinSlotImmStore).with_bin_slot_imm_store(
                Instruction::ADD as u8,
                0,
                0,
            ),
            Byte::new(Instruction::NOOP),
        ];
        let info = analyze_at(&code, &[], 0, 0);
        assert!(!is_modelled(&code[0], &[]));
        assert_eq!(info.tell_before(1), Tell::Unknown);
    }

    #[test]
    fn unpack_pushes_payload_arity_minus_scrutinee() {
        let code = vec![
            Byte::new(Instruction::CONST).with_const_inline(1),
            Byte::new(Instruction::Unpack).with_operand_u32(2),
            Byte::new(Instruction::NOOP),
        ];
        // cursor 1 → pop 1 + push 2 → 2
        assert_eq!(cursors(&code).last().copied().unwrap(), Some(2));
    }

    /// Taken edge is intentionally unmodelled: payload arity is runtime-only.
    #[test]
    fn jump_if_match_fallthrough_keeps_cursor_taken_edge_unknown() {
        let pool = vec![2u64];
        let jim = Byte::new(Instruction::JumpIfMatch).with_operands_u16([0, 0]);
        assert!(!is_modelled(&jim, &pool));
        let code = vec![
            jim,
            Byte::new(Instruction::NOOP),
            Byte::new(Instruction::NOOP),
        ];
        let info = analyze_at(&code, &pool, 0, 3);
        assert_eq!(info.tell_before(1).known(), Some(3));
        assert_eq!(info.tell_before(2), Tell::Unknown);
    }

    #[test]
    fn return_terminator_blocks_fallthrough() {
        let code = vec![
            Byte::new(Instruction::CONST).with_const_inline(1),
            Byte::new(Instruction::RETURN),
            Byte::new(Instruction::NOOP),
        ];
        let info = analyze_at(&code, &[], 0, 0);
        assert_eq!(info.tell_before(2), Tell::Unknown);
    }

    /// Absolute `Seek` re-anchors a poisoned path so later stores stay usable.
    #[test]
    fn seek_recovers_known_cursor_after_unknown() {
        let code = vec![
            Byte::new(Instruction::FfiInvoke).with_operand_u32(0),
            Byte::new(Instruction::Seek).with_operand_u32(4),
            Byte::new(Instruction::NOOP),
        ];
        let info = analyze_at(&code, &[], 0, 0);
        assert_eq!(info.tell_before(1), Tell::Unknown);
        assert_eq!(info.tell_before(2).known(), Some(4));
    }

    /// Fused jmpf forms pack the target in the pool high half — must join both edges.
    #[test]
    fn bin_slot_imm_jmpf_joins_fallthrough_and_taken_with_same_cursor() {
        let pool = vec![2u64 << 32];
        let code = vec![
            Byte::new(Instruction::BinSlotImmJmpf).with_bin_slot_imm_jmpf(
                Instruction::LE as u8,
                0,
                0,
            ),
            Byte::new(Instruction::NOOP),
            Byte::new(Instruction::NOOP),
        ];
        let info = analyze_at(&code, &pool, 0, 1);
        assert!(is_modelled(&code[0], &pool));
        assert_eq!(info.tell_before(1).known(), Some(1));
        assert_eq!(info.tell_before(2).known(), Some(1));
    }

    #[test]
    fn bin_slot_slot_jmpf_joins_both_edges() {
        let pool = vec![2u64 << 32];
        let code = vec![
            Byte::new(Instruction::BinSlotSlotJmpf).with_bin_slot_slot_jmpf(
                Instruction::AND as u8,
                0,
                0,
            ),
            Byte::new(Instruction::NOOP),
            Byte::new(Instruction::NOOP),
        ];
        let info = analyze_at(&code, &pool, 0, 2);
        assert_eq!(info.tell_before(1).known(), Some(2));
        assert_eq!(info.tell_before(2).known(), Some(2));
    }
}
