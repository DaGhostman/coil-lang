//! IL optimization — cfg passes.

use crate::il::op::{IlJumpKind, IlOp, Label};
use common::Instruction;

pub(super) fn label_targets(ops: &[IlOp]) -> std::collections::HashMap<u32, usize> {
    let mut map = std::collections::HashMap::new();
    for (i, op) in ops.iter().enumerate() {
        if let IlOp::Label(Label(id)) = op {
            map.insert(*id, i);
        }
    }
    map
}

pub(super) fn jump_thread(ops: &mut Vec<IlOp>) {
    let targets = label_targets(ops);
    for i in 0..ops.len() {
        let IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            target,
            loc,
        } = ops[i]
        else {
            continue;
        };
        let Some(&idx) = targets.get(&target.0) else {
            continue;
        };
        let mut j = idx;
        while j < ops.len() {
            match &ops[j] {
                IlOp::Label(_) => j += 1,
                IlOp::Jump {
                    kind: IlJumpKind::Unconditional,
                    target: t2,
                    ..
                } => {
                    ops[i] = IlOp::Jump {
                        kind: IlJumpKind::Unconditional,
                        target: *t2,
                        loc,
                    };
                    break;
                }
                _ => break,
            }
        }
    }
}

/// True when `op` would fuse into a `*Jmpf` superinstruction with a following
/// `JumpIfFalse` (mirrors the `try_fuse_*_jmpf` arms in [`crate::il::lower`]).
fn fuses_with_jmpf(op: Option<&IlOp>) -> bool {
    let Some(op) = op else {
        return false;
    };
    if matches!(op, IlOp::BinSlotImm { op: o, .. } | IlOp::BinSlotSlot { op: o, .. }
        if crate::il::lower::is_jmpf_cond_op(Instruction::from(*o)))
    {
        return true;
    }
    if matches!(op, IlOp::Bin { op: o, .. } if crate::il::lower::is_jmpf_cond_op(*o)) {
        return true;
    }
    match op.as_encode_byte() {
        Some(b) => match *b.bytecode() {
            Instruction::LogNot => true,
            Instruction::BinSlotImm => {
                crate::il::lower::is_jmpf_cond_op(Instruction::from(b.bin_slot_imm_parts().0))
            }
            Instruction::BinSlotSlot => {
                crate::il::lower::is_jmpf_cond_op(Instruction::from(b.bin_slot_slot_parts().0))
            }
            other => crate::il::lower::is_jmpf_cond_op(other),
        },
        None => false,
    }
}

/// `JMPF A; JMP B; A:` → `JMPT B`, dropping the trailing unconditional jump.
///
/// This is the shape every `if cond { break / return / continue }` guard emits.
/// Refused when the condition producer would fuse into a `*Jmpf` superinstruction
/// — there is no `*Jmpt` counterpart, so inverting would cost more than it saves.
pub(crate) fn invert_branch_over_jump(ops: &mut Vec<IlOp>) {
    let mut remove: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut i = 0;
    while i + 2 < ops.len() {
        let (
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: skip,
                loc,
            },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: far,
                ..
            },
        ) = (&ops[i], &ops[i + 1])
        else {
            i += 1;
            continue;
        };
        let (skip, far, loc) = (*skip, *far, *loc);
        let prev = i.checked_sub(1).and_then(|p| ops.get(p));
        if fuses_with_jmpf(prev) || !labels_bind_at(ops, i + 2, skip) {
            i += 1;
            continue;
        }
        ops[i] = IlOp::Jump {
            kind: IlJumpKind::JumpIfTrue,
            target: far,
            loc,
        };
        remove.insert(i + 1);
        i += 2;
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

/// True when `target` is bound by the run of labels starting at `from`, i.e. the
/// JMPF's false path is exactly the next instruction.
fn labels_bind_at(ops: &[IlOp], from: usize, target: Label) -> bool {
    for op in &ops[from..] {
        match op {
            IlOp::Label(l) if *l == target => return true,
            IlOp::Label(_) => continue,
            _ => return false,
        }
    }
    false
}

pub(super) fn is_unconditional_jmp(op: &IlOp) -> bool {
    matches!(
        op,
        IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            ..
        }
    )
}

pub(super) fn is_return_terminator(op: &IlOp) -> bool {
    matches!(
        op,
        IlOp::Return { .. }
            | IlOp::Halt { .. }
            | IlOp::LoadReturnSlot { .. }
            | IlOp::ConstReturnImm { .. }
            | IlOp::BinReturn { .. }
    ) || matches!(
        op.as_encode_byte(),
        Some(b) if matches!(
            *b.bytecode(),
            Instruction::RETURN
                | Instruction::HALT
                | Instruction::LoadReturnSlot
                | Instruction::ConstReturnImm
                | Instruction::BinReturn
        )
    )
}

pub(super) fn eliminate_dead_blocks(ops: &mut Vec<IlOp>) {
    let mut out = Vec::with_capacity(ops.len());
    let mut reachable = true;
    for op in ops.drain(..) {
        if matches!(op, IlOp::Label(_)) {
            reachable = true;
            out.push(op);
            continue;
        }
        if !reachable {
            continue;
        }
        // Sweep after JMP and RETURN/HALT/*Return. Entry labels + CALL-0
        // continuations must be labeled so live code is not treated as
        // fall-through-after-terminator.
        let term = is_unconditional_jmp(&op) || is_return_terminator(&op);
        out.push(op);
        if term {
            reachable = false;
        }
    }
    *ops = out;
}

