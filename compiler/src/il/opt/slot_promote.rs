//! IL optimization — join-safe slot promotion.
//!
//! Locals and operands share one buffer, so `STORE t` executed with the cursor at
//! `t + 1` writes TOS back to the address it already occupies, and the store's own
//! floor puts the cursor back where it was: a bit-exact no-op. The mirror shape is
//! a run of `LOAD`s that re-pushes the top of the stack right before a terminator
//! that pops exactly those values.
//!
//! Joins come for free: [`crate::il::tell`] poisons a program point whose
//! predecessors disagree, so a `Tell::Known` cursor is one every path agrees on.
//!
//! Dropping the store hides the fact that the *push* is what really defines the
//! slot, so a store only goes when every surviving reference to its slot goes with
//! it — the slot leaves the frame instead of keeping an invisible def. Ops with a
//! slot operand this pass cannot resolve before lowering (pool-packed `BinSlot*`
//! fused forms, `Seek`, `UnpackAt`) refuse the whole body.

use std::collections::{HashMap, HashSet};

use common::Instruction;

use crate::il::op::{EntryKind, IlOp};
use crate::il::tell::TellInfo;

/// Frame-relative slots `op` names as an operand.
///
/// Returns `false` when `op` addresses a slot this pass cannot resolve on
/// symbolic IL; the caller must then refuse the body. The default arm is safe
/// because every slot-addressing VM handler is enumerated here.
fn visit_named_slots(op: &IlOp, mut visit: impl FnMut(u32)) -> bool {
    match op {
        IlOp::Load { slot, .. }
        | IlOp::StorePop { slot, .. }
        | IlOp::LoadReturnSlot { slot, .. } => visit(*slot),
        IlOp::BinSlotImm { slot, .. } => visit(u32::from(*slot)),
        IlOp::BinSlotSlot { a, b, .. } => {
            visit(u32::from(*a));
            visit(u32::from(*b));
        }
        IlOp::Byte { byte, .. } => match *byte.bytecode() {
            Instruction::LOAD | Instruction::STORE | Instruction::StorePop => {
                for i in 0..byte.load_store_count() {
                    visit(byte.load_store_slot_at(i));
                }
            }
            Instruction::BinSlotImm => visit(byte.bin_slot_imm_parts().1 as u32),
            Instruction::BinSlotSlot => {
                let (_, a, b) = byte.bin_slot_slot_parts();
                visit(a as u32);
                visit(b as u32);
            }
            Instruction::INC | Instruction::DEC => visit(byte.inc_dec_parts().0 as u32),
            Instruction::LoadReturnSlot => visit(byte.operand_u32()),
            // Pool-packed destinations, absolute cursor moves and in-place
            // unpacks name slots this pass cannot read from the IL alone.
            Instruction::BinSlotImmJmpf
            | Instruction::BinSlotSlotJmpf
            | Instruction::BinSlotImmStore
            | Instruction::BinSlotSlotStore
            | Instruction::Seek
            | Instruction::UnpackAt => return false,
            _ => {}
        },
        _ => {}
    }
    true
}

/// Single slot written by a `STORE`-class op, or `None` for packed / other ops.
fn stored_slot(op: &IlOp) -> Option<u32> {
    match op {
        IlOp::StorePop { slot, .. } => Some(*slot),
        IlOp::Byte { byte, .. }
            if matches!(
                *byte.bytecode(),
                Instruction::STORE | Instruction::StorePop
            ) =>
        {
            byte.load_store_single_slot()
        }
        _ => None,
    }
}

/// Slots a `LOAD`-class op pushes, in push order.
fn loaded_slots(op: &IlOp) -> Option<Vec<u32>> {
    match op {
        IlOp::Load { slot, .. } => Some(vec![*slot]),
        IlOp::Byte { byte, .. } if *byte.bytecode() == Instruction::LOAD => {
            Some((0..byte.load_store_count()).map(|i| byte.load_store_slot_at(i)).collect())
        }
        _ => None,
    }
}

/// Argument count of a `TailCall`, whose operands may stay where they are.
///
/// `TailCall` copies `arity` values from `tell - arity` down to the frame base and
/// then leaves the frame, so a lower `tell` just moves the source range without a
/// successor to disturb.
///
/// `CALL` is deliberately absent: it takes its frame base from `tell - arity`, so
/// dropping its operand loads moves the callee frame down over caller slots — that
/// needs slot liveness, not just the cursor. `RETURN` is excluded for a different
/// reason: `LOAD t; RETURN` is the suffix the whole-buffer return convoys sink into
/// a join, and eliding the `LOAD` in one predecessor loses that sink for more than
/// it saves.
fn tail_call_arity(op: &IlOp) -> Option<u32> {
    match op {
        IlOp::Entry {
            kind: EntryKind::TailCall,
            arity,
            ..
        } => Some(*arity),
        IlOp::Byte { byte, .. } if *byte.bytecode() == Instruction::TailCall => {
            Some(byte.call_parts().0 as u32)
        }
        _ => None,
    }
}

/// `STORE t` whose value already sits at frame-relative position `t`.
fn is_self_store(op: &IlOp, cursor: &TellInfo, idx: usize) -> Option<u32> {
    let slot = stored_slot(op)?;
    let before = cursor.tell_before(idx).known()?;
    (before == slot.saturating_add(1)).then_some(slot)
}

/// Indices of `LOAD` words that re-push arguments a following `TailCall` already
/// finds on the stack.
///
/// The run must end at the call and cover exactly its arguments: slots
/// `H - n ..= H - 1` for a cursor of `H` at the run start. A label at the run
/// start is fine — slot `s` and stack position `s` are the same address, so both
/// spellings read the same memory on every path that agrees on `H`.
fn collect_retained_loads(ops: &[IlOp], cursor: &TellInfo) -> HashSet<usize> {
    let mut retained = HashSet::new();
    for (k, op) in ops.iter().enumerate() {
        let Some(n) = tail_call_arity(op) else {
            continue;
        };
        if n == 0 {
            continue;
        }
        let mut run: Vec<(usize, Vec<u32>)> = Vec::new();
        let mut pushed = 0u32;
        let mut j = k;
        while j > 0 && pushed < n {
            let Some(slots) = loaded_slots(&ops[j - 1]) else {
                break;
            };
            pushed += slots.len() as u32;
            run.push((j - 1, slots));
            j -= 1;
        }
        if pushed != n {
            continue;
        }
        run.reverse();
        let Some(height) = cursor.tell_before(j).known() else {
            continue;
        };
        if height < n {
            continue;
        }
        let ordered: Vec<u32> = run.iter().flat_map(|(_, slots)| slots.iter().copied()).collect();
        if ordered
            .iter()
            .enumerate()
            .all(|(i, slot)| *slot == height - n + i as u32)
        {
            retained.extend(run.iter().map(|(idx, _)| *idx));
        }
    }
    retained
}

/// Drop `LOAD` / `STORE` words the shared cursor proves redundant.
///
/// Runs on one function body, seeded at `entry_tell` (`arity` at a function
/// entry). Safe to call on a bare buffer: an unresolvable slot operand or an
/// unknown cursor refuses instead of guessing.
pub(crate) fn slot_promote_at(ops: &mut Vec<IlOp>, entry_tell: u32) {
    if !ops.iter().all(|op| visit_named_slots(op, |_| {})) {
        return;
    }

    let cursor = crate::il::tell::analyze_il_at(ops, entry_tell);
    let mut drop = collect_retained_loads(ops, &cursor);

    let self_stores: Vec<(usize, u32)> = ops
        .iter()
        .enumerate()
        .filter_map(|(idx, op)| is_self_store(op, &cursor, idx).map(|slot| (idx, slot)))
        .collect();
    if !self_stores.is_empty() {
        let mut references: HashMap<u32, Vec<usize>> = HashMap::new();
        for (idx, op) in ops.iter().enumerate() {
            visit_named_slots(op, |slot| references.entry(slot).or_default().push(idx));
        }
        for (idx, slot) in &self_stores {
            let stores_of_slot: HashSet<usize> = self_stores
                .iter()
                .filter(|(_, s)| s == slot)
                .map(|(i, _)| *i)
                .collect();
            let refs = references.get(slot).map(Vec::as_slice).unwrap_or_default();
            // A store with no reader is dead code, not a promotion — leave it to
            // `dead_store`, whose cursor proof is the one that owns that call.
            let promotable = refs.iter().any(|r| drop.contains(r))
                && refs
                    .iter()
                    .all(|r| drop.contains(r) || stores_of_slot.contains(r));
            if promotable {
                drop.insert(*idx);
            }
        }
    }

    if drop.is_empty() {
        return;
    }
    let mut out = Vec::with_capacity(ops.len() - drop.len());
    for (idx, op) in ops.iter().enumerate() {
        if !drop.contains(&idx) {
            out.push(op.clone());
        }
    }
    *ops = out;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::il::op::Label;
    use common::{Byte, DebugLoc};

    fn loc() -> DebugLoc {
        DebugLoc::unknown()
    }

    fn tail_call(arity: u32) -> IlOp {
        IlOp::Entry {
            kind: EntryKind::TailCall,
            arity,
            target: Label(0),
            loc: loc(),
        }
    }

    fn counts(ops: &[IlOp]) -> (usize, usize) {
        (
            ops.iter().filter(|op| matches!(op, IlOp::Load { .. })).count(),
            ops.iter()
                .filter(|op| matches!(op, IlOp::StorePop { .. }))
                .count(),
        )
    }

    /// Argument materialization: each result already sits in the slot its store
    /// names, and the reload run feeds the `TailCall` that pops it.
    #[test]
    fn tail_call_argument_temps_leave_the_frame() {
        let mut ops = vec![
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::StorePop { slot: 3, loc: loc() },
            IlOp::Const { imm: 2, loc: loc() },
            IlOp::StorePop { slot: 4, loc: loc() },
            IlOp::Load { slot: 3, loc: loc() },
            IlOp::Load { slot: 4, loc: loc() },
            tail_call(2),
        ];
        slot_promote_at(&mut ops, 3);
        assert_eq!(counts(&ops), (0, 0), "{}", ops.len());
        assert_eq!(ops.len(), 3);
    }

    /// A reader the pass cannot remove keeps the store: dropping it would leave
    /// the slot defined only by a push no later pass can see.
    #[test]
    fn self_store_stays_when_a_reader_survives() {
        let mut ops = vec![
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::StorePop { slot: 2, loc: loc() },
            IlOp::Load { slot: 2, loc: loc() },
            IlOp::Pop { loc: loc() },
            IlOp::Return { loc: loc() },
        ];
        slot_promote_at(&mut ops, 2);
        assert_eq!(counts(&ops), (1, 1));
    }

    /// A store with no reader is dead code, not a promotion — `dead_store` owns it.
    #[test]
    fn store_to_a_slot_nobody_reads_is_left_alone() {
        let mut ops = vec![
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::StorePop { slot: 3, loc: loc() },
            IlOp::Return { loc: loc() },
        ];
        slot_promote_at(&mut ops, 3);
        assert_eq!(counts(&ops), (0, 1));
    }

    /// `STORE 5` with the cursor at 1 moves the value: only `slot + 1 == tell`
    /// makes the write land where the value already is.
    #[test]
    fn store_that_actually_moves_the_value_is_kept() {
        let mut ops = vec![
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::StorePop { slot: 5, loc: loc() },
            IlOp::Load { slot: 5, loc: loc() },
            IlOp::Load { slot: 5, loc: loc() },
            tail_call(2),
        ];
        slot_promote_at(&mut ops, 0);
        assert_eq!(counts(&ops), (2, 1));
    }

    /// An unknown cursor refuses instead of guessing (`FfiInvoke` is unmodelled).
    #[test]
    fn unknown_cursor_refuses_the_promotion() {
        let mut ops = vec![
            IlOp::byte(Byte::new(common::Instruction::FfiInvoke).with_operand_u32(0)),
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::StorePop { slot: 3, loc: loc() },
            IlOp::Load { slot: 3, loc: loc() },
            tail_call(1),
        ];
        slot_promote_at(&mut ops, 3);
        assert_eq!(counts(&ops), (1, 1));
    }

    /// The reload run must be exactly the top of the stack — arguments loaded
    /// from live locals are copies the `TailCall` cannot read in place.
    #[test]
    fn tail_call_run_off_the_stack_top_is_refused() {
        let mut ops = vec![
            IlOp::Load { slot: 0, loc: loc() },
            IlOp::Load { slot: 1, loc: loc() },
            tail_call(2),
        ];
        slot_promote_at(&mut ops, 4);
        assert_eq!(counts(&ops), (2, 0));
    }

    /// A pool-packed fused store hides its destination slot before lowering, so
    /// the whole body is refused rather than promoted against partial slot info.
    #[test]
    fn unresolvable_slot_operand_refuses_the_body() {
        let fused = Byte::new(common::Instruction::BinSlotImmStore)
            .with_bin_slot_imm_store(common::Instruction::ADD as u8, 0, 0);
        let mut ops = vec![
            IlOp::byte(fused),
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::StorePop { slot: 3, loc: loc() },
            IlOp::Load { slot: 3, loc: loc() },
            tail_call(1),
        ];
        slot_promote_at(&mut ops, 3);
        assert_eq!(counts(&ops), (1, 1));
    }

    /// Packed `LOAD` words carry the whole argument run in one op.
    #[test]
    fn packed_load_run_is_dropped_as_one_word() {
        let packed = Byte::new(common::Instruction::LOAD).with_load_store_packed(2, 3, 4, 0);
        let mut ops = vec![
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::StorePop { slot: 3, loc: loc() },
            IlOp::Const { imm: 2, loc: loc() },
            IlOp::StorePop { slot: 4, loc: loc() },
            IlOp::byte(packed),
            tail_call(2),
        ];
        slot_promote_at(&mut ops, 3);
        assert_eq!(ops.len(), 3);
        assert!(!ops.iter().any(|op| matches!(op, IlOp::Byte { .. })));
    }

    /// `CALL` takes its frame base from `tell - arity`; dropping operand loads
    /// would slide the callee frame over caller slots.
    #[test]
    fn call_reload_run_is_not_promoted() {
        let mut ops = vec![
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::StorePop { slot: 3, loc: loc() },
            IlOp::Const { imm: 2, loc: loc() },
            IlOp::StorePop { slot: 4, loc: loc() },
            IlOp::Load { slot: 3, loc: loc() },
            IlOp::Load { slot: 4, loc: loc() },
            IlOp::Entry {
                kind: EntryKind::Call,
                arity: 2,
                target: Label(0),
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        let before = ops.clone();
        slot_promote_at(&mut ops, 3);
        assert!(ops == before);
        assert_eq!(counts(&ops), (2, 2));
    }

    /// `LOAD t; RETURN` is the join sink for whole-buffer return convoys — do
    /// not treat it like a TailCall reload run.
    #[test]
    fn return_reload_is_not_promoted() {
        let mut ops = vec![
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::StorePop { slot: 3, loc: loc() },
            IlOp::Load { slot: 3, loc: loc() },
            IlOp::Return { loc: loc() },
        ];
        let before = ops.clone();
        slot_promote_at(&mut ops, 3);
        assert!(ops == before);
        assert_eq!(counts(&ops), (1, 1));
    }

    /// Absolute `Seek` names a cursor this pass cannot reason about from the
    /// IL alone, so the whole body is refused.
    #[test]
    fn seek_in_body_refuses_slot_promotion() {
        let mut ops = vec![
            IlOp::byte(Byte::new(common::Instruction::Seek).with_operand_u32(3)),
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::StorePop { slot: 3, loc: loc() },
            IlOp::Load { slot: 3, loc: loc() },
            tail_call(1),
        ];
        slot_promote_at(&mut ops, 3);
        assert_eq!(counts(&ops), (1, 1));
    }
}
