//! IL optimization — dce passes.

use std::collections::HashMap;

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

#[derive(Clone)]
struct CopyBinding {
    producer: IlOp,
    dependencies: Vec<u32>,
}

/// Return the local slots read by a pure producer that can be cloned at a
/// later `Load`. Memory-dependent and stack-consuming producers are excluded.
fn copy_producer_dependencies(op: &IlOp) -> Option<Vec<u32>> {
    let mut dependencies = match op {
        IlOp::Const { .. } | IlOp::ConstPool { .. } | IlOp::String { .. } => Vec::new(),
        IlOp::Load { slot, .. } => vec![*slot],
        IlOp::BinSlotImm { slot, .. } => vec![*slot as u32],
        IlOp::BinSlotSlot { a, b, .. } => vec![*a as u32, *b as u32],
        _ => return None,
    };
    dependencies.sort_unstable();
    dependencies.dedup();
    Some(dependencies)
}

fn copy_prop_shape_sensitive_load(ops: &[IlOp], load_idx: usize) -> bool {
    let Some(next) = ops.get(load_idx + 1) else {
        return false;
    };
    if matches!(next, IlOp::GetField { .. }) {
        return true;
    }

    let mut idx = load_idx + 1;
    while let Some(op) = ops.get(idx) {
        if matches!(
            op,
            IlOp::MakeTuple { .. } | IlOp::MakeArray { .. } | IlOp::MakeEnum { .. }
        ) {
            return true;
        }
        if matches!(
            op,
            IlOp::Load { .. }
                | IlOp::Const { .. }
                | IlOp::ConstPool { .. }
                | IlOp::String { .. }
                | IlOp::Dup { .. }
                | IlOp::BinSlotImm { .. }
                | IlOp::BinSlotSlot { .. }
        ) {
            idx += 1;
            continue;
        }
        return false;
    }
    false
}

fn invalidate_copy_slot(bindings: &mut HashMap<u32, CopyBinding>, slot: u32) {
    bindings
        .retain(|bound_slot, binding| *bound_slot != slot && !binding.dependencies.contains(&slot));
}

fn copy_prop_barrier(op: &IlOp) -> bool {
    matches!(
        op,
        IlOp::Label(_)
            | IlOp::Jump { .. }
            | IlOp::Entry { .. }
            | IlOp::HostInvoke { .. }
            | IlOp::Print { .. }
            | IlOp::GetField { .. }
            | IlOp::SetField { .. }
            | IlOp::MakeTuple { .. }
            | IlOp::MakeArray { .. }
            | IlOp::MakeEnum { .. }
            | IlOp::BoxValue { .. }
            | IlOp::UnboxValue { .. }
            | IlOp::Return { .. }
            | IlOp::Halt { .. }
            | IlOp::LoadReturnSlot { .. }
            | IlOp::ConstReturnImm { .. }
            | IlOp::BinReturn { .. }
            | IlOp::Byte { .. }
    ) || matches!(
        op.as_encode_byte(),
        Some(byte)
            if matches!(
                *byte.bytecode(),
                Instruction::STORE
                    | Instruction::StorePop
                    | Instruction::HostInvoke
                    | Instruction::PRINT
                    | Instruction::CALL
                    | Instruction::TailCall
                    | Instruction::GetField
                    | Instruction::SetField
                    | Instruction::MakeTuple
                    | Instruction::MakeArray
                    | Instruction::MakeEnum
                    | Instruction::BoxValue
                    | Instruction::FfiInvoke
            )
    )
}

/// Forward pure producer copies through a straight-line IL region.
///
/// The pass deliberately stops at labels, control flow, calls, unknown bytes,
/// and memory operations. `dead_store_at` removes the now-unused original
/// producer/store pair only when the shared cursor proof allows it.
pub(super) fn copy_prop(ops: &mut Vec<IlOp>, entry_tell: u32) {
    let cursor = crate::il::tell::analyze_il_at(ops, entry_tell);
    let mut bindings: HashMap<u32, CopyBinding> = HashMap::new();
    let mut i = 0;

    while i < ops.len() {
        if let IlOp::Load { slot, .. } = ops[i]
            && cursor.tell_before(i).known().is_some()
            && !copy_prop_shape_sensitive_load(ops, i)
            && let Some(binding) = bindings.get(&slot).cloned()
        {
            let mut replacement = binding.producer;
            replacement.set_loc(ops[i].loc());
            ops[i] = replacement;
        }

        if i + 1 < ops.len()
            && let IlOp::StorePop { slot, .. } = &ops[i + 1]
            && let Some(dependencies) = copy_producer_dependencies(&ops[i])
            && !dependencies.contains(slot)
        {
            invalidate_copy_slot(&mut bindings, *slot);
            bindings.insert(
                *slot,
                CopyBinding {
                    producer: ops[i].clone(),
                    dependencies,
                },
            );
            i += 2;
            continue;
        }

        match &ops[i] {
            IlOp::StorePop { slot, .. } => invalidate_copy_slot(&mut bindings, *slot),
            op if copy_prop_barrier(op) => bindings.clear(),
            _ => {}
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
            | IlOp::Byte { .. }
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
#[cfg(test)]
pub(super) fn dead_store(ops: &mut Vec<IlOp>) {
    dead_store_at(ops, 0);
}

/// Cursor-seeded dead-store elimination for an IL function body.
pub(super) fn dead_store_at(ops: &mut Vec<IlOp>, entry_tell: u32) {
    let cursor = crate::il::tell::analyze_il_at(ops, entry_tell);
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
                        if cursor.can_remove_one_value_store(i - 1, slot) {
                            remove.insert(i - 1);
                            remove.insert(i);
                        }
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
