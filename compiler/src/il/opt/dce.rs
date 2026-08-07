//! IL optimization — dce passes.

use crate::il::op::IlOp;
use common::Instruction;

/// Side-effect-free single-value producer: dropping it with its `Pop` is a no-op.
fn is_droppable_producer(op: &IlOp) -> bool {
    matches!(
        op,
        IlOp::Const { .. } | IlOp::ConstPool { .. } | IlOp::String { .. } | IlOp::Load { .. }
    )
}

/// Run [`stack_dce_once`] to a fixpoint: removing a pair can expose a new one
/// (`Load a; Const c; Pop; Pop` → `Load a; Pop` → empty).
pub(super) fn stack_dce(ops: &mut Vec<IlOp>) {
    loop {
        let before = ops.len();
        stack_dce_once(ops);
        if ops.len() == before {
            return;
        }
    }
}

fn stack_dce_once(ops: &mut Vec<IlOp>) {
    let mut out = Vec::with_capacity(ops.len());
    let mut i = 0;
    while i < ops.len() {
        if i + 1 < ops.len()
            && matches!(&ops[i], IlOp::Dup { .. })
            && matches!(&ops[i + 1], IlOp::Pop { .. })
        {
            i += 2;
            continue;
        }
        // Pure producer discarded immediately (statement-position literals,
        // inlined unit returns like `Vec::push`'s `CONST 0`).
        if i + 1 < ops.len()
            && is_droppable_producer(&ops[i])
            && matches!(&ops[i + 1], IlOp::Pop { .. })
        {
            i += 2;
            continue;
        }
        if i + 1 < ops.len()
            && let (IlOp::Load { slot: s0, .. }, IlOp::StorePop { slot: s1, .. }) =
                (&ops[i], &ops[i + 1])
            && s0 == s1
        {
            i += 2;
            continue;
        }
        // Residual Byte fallback (pre-absorb fragments / tests).
        if i + 1 < ops.len()
            && let (Some(b0), Some(b1)) = (ops[i].as_encode_byte(), ops[i + 1].as_encode_byte())
            && *b0.bytecode() == Instruction::DUPLICATE
            && *b1.bytecode() == Instruction::POP
            && matches!(&ops[i], IlOp::Byte { .. })
            && matches!(&ops[i + 1], IlOp::Byte { .. })
        {
            i += 2;
            continue;
        }
        if i + 1 < ops.len()
            && let (Some(b0), Some(b1)) = (ops[i].as_encode_byte(), ops[i + 1].as_encode_byte())
            && *b0.bytecode() == Instruction::LOAD
            && (*b1.bytecode() == Instruction::STORE || *b1.bytecode() == Instruction::StorePop)
            && b0.load_store_single_slot().is_some()
            && b0.load_store_single_slot() == b1.load_store_single_slot()
            && matches!(&ops[i], IlOp::Byte { .. })
            && matches!(&ops[i + 1], IlOp::Byte { .. })
        {
            i += 2;
            continue;
        }
        out.push(ops[i].clone());
        i += 1;
    }
    *ops = out;
}

/// `StorePop s; Load s` → `Dup; StorePop s` when the value stays on stack after
/// store. Refused when SP-in `h <= s + 1`: the store extends `tell` to `s + 1`,
/// so a remaining Dup copy is no longer TOS and later pops (e.g. `CONST; CmpJmpf`)
/// eat the local — classic shared-stack hazard after nested CALL returns
/// (`tell == frame_base + 1`, store to a higher slot).
pub(super) fn mem_fwd(ops: &mut Vec<IlOp>, entry_sp: i32) {
    let sp = crate::il::sp::analyze_at(ops, entry_sp);
    let mut i = 0;
    while i + 1 < ops.len() {
        let slot_loc = {
            match (&ops[i], &ops[i + 1]) {
                (IlOp::StorePop { slot: s0, loc }, IlOp::Load { slot: s1, .. }) if s0 == s1 => {
                    Some((*s0, *loc))
                }
                _ => None,
            }
        };
        if let Some((slot, loc)) = slot_loc {
            let refuse = match sp.sp_before(i) {
                crate::il::sp::Sp::Known(h) => h <= slot as i32 + 1,
                crate::il::sp::Sp::Unknown => true,
            } || mem_fwd_load_feeds_index(ops, i + 1);
            if !refuse {
                ops[i] = IlOp::Dup { loc };
                ops[i + 1] = IlOp::StorePop { slot, loc };
                i += 2;
                continue;
            }
        }
        i += 1;
    }
}

fn slot_used_by(op: &IlOp, slot: u32) -> bool {
    match op {
        IlOp::Load { slot: s, .. } | IlOp::LoadReturnSlot { slot: s, .. } => *s == slot,
        IlOp::BinSlotImm { slot: s, .. } => *s as u32 == slot,
        IlOp::BinSlotSlot { a, b, .. } => *a as u32 == slot || *b as u32 == slot,
        _ => false,
    }
}

fn is_store_barrier(op: &IlOp) -> bool {
    matches!(
        op,
        IlOp::HostInvoke { .. }
            | IlOp::Print { .. }
            | IlOp::Entry { .. }
            | IlOp::SetField { .. }
            | IlOp::Jump { .. }
            | IlOp::Label(_)
            | IlOp::Return { .. }
            | IlOp::Halt { .. }
            | IlOp::LoadReturnSlot { .. }
            | IlOp::ConstReturnImm { .. }
            | IlOp::BinReturn { .. }
    )
}

/// True when `Load` at `load_idx` is the tuple-destructure reload (`Const; Index`).
pub(super) fn mem_fwd_load_feeds_index(ops: &[IlOp], load_idx: usize) -> bool {
    matches!(ops.get(load_idx + 1), Some(IlOp::Const { .. }))
        && matches!(ops.get(load_idx + 2), Some(IlOp::Index { .. }))
}

/// Drop `StorePop s` (and a preceding dead producer / Dup) when `s` is unused
/// before the next store to `s` or a control/effect barrier. Straight-line only.
pub(super) fn dead_store(ops: &mut Vec<IlOp>) {
    let mut remove: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut i = 0;
    while i < ops.len() {
        let IlOp::StorePop { slot, .. } = &ops[i] else {
            i += 1;
            continue;
        };
        let slot = *slot;
        let mut used = false;
        let mut j = i + 1;
        while j < ops.len() {
            if is_store_barrier(&ops[j]) {
                // Jumps/labels may reach a later Load on a back-edge (loop-carried).
                if !matches!(&ops[j], IlOp::Return { .. } | IlOp::Halt { .. }) {
                    used = true;
                }
                break;
            }
            if matches!(&ops[j], IlOp::StorePop { slot: s, .. } if *s == slot) {
                break;
            }
            if slot_used_by(&ops[j], slot) {
                used = true;
                break;
            }
            j += 1;
        }
        if !used {
            // Only when the stored value is otherwise dead: Dup;StorePop or
            // Const/Load/ConstPool immediately before.
            if i > 0 {
                match &ops[i - 1] {
                    IlOp::Dup { .. }
                    | IlOp::Const { .. }
                    | IlOp::ConstPool { .. }
                    | IlOp::Load { .. } => {
                        remove.insert(i - 1);
                        remove.insert(i);
                    }
                    _ => {}
                }
            }
        }
        i += 1;
    }
    if remove.is_empty() {
        return;
    }
    let mut out = Vec::with_capacity(ops.len());
    for (idx, op) in ops.iter().enumerate() {
        if !remove.contains(&idx) {
            out.push(op.clone());
        }
    }
    *ops = out;
}
