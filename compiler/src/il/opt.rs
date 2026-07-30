//! IL optimization passes unlocked by symbolic labels.

use common::Instruction;

use super::op::{IlJumpKind, IlOp, Label};

/// Options for [`optimize`].
#[derive(Clone)]
pub struct OptimizeOptions {
    /// Collapse `JMP L` where `L` begins with `JMP L2` into `JMP L2`.
    pub jump_thread: bool,
    /// Remove unreachable ops after unconditional JMP / RETURN until a label.
    pub dead_block: bool,
    /// Drop redundant `DUPLICATE; POP` and `LOAD s; StorePop s`.
    pub stack_dce: bool,
    /// Sink identical `LOAD`/`CONST` producers into a join `RETURN` and fuse.
    pub return_convoy: bool,
}

impl Default for OptimizeOptions {
    fn default() -> Self {
        Self {
            jump_thread: true,
            // JMP + RETURN/HALT/*Return sweep; entry labels + CALL-0
            // continuations labeled.
            dead_block: true,
            stack_dce: true,
            return_convoy: true,
        }
    }
}

/// Run IL opts in place. Safe to call before [`super::lower`].
pub fn optimize(ops: &mut Vec<IlOp>, opts: &OptimizeOptions) {
    if opts.jump_thread {
        jump_thread(ops);
    }
    if opts.dead_block {
        eliminate_dead_blocks(ops);
    }
    if opts.stack_dce {
        stack_dce(ops);
    }
    if opts.return_convoy {
        return_convoy(ops);
    }
}

fn label_targets(ops: &[IlOp]) -> std::collections::HashMap<u32, usize> {
    let mut map = std::collections::HashMap::new();
    for (i, op) in ops.iter().enumerate() {
        if let IlOp::Label(Label(id)) = op {
            map.insert(*id, i);
        }
    }
    map
}

fn jump_thread(ops: &mut Vec<IlOp>) {
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

fn is_unconditional_jmp(op: &IlOp) -> bool {
    matches!(
        op,
        IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            ..
        }
    )
}

fn is_return_terminator(op: &IlOp) -> bool {
    let IlOp::Byte { byte, .. } = op else {
        return false;
    };
    matches!(
        *byte.bytecode(),
        Instruction::RETURN
            | Instruction::HALT
            | Instruction::LoadReturnSlot
            | Instruction::ConstReturnImm
            | Instruction::BinReturn
    )
}

fn eliminate_dead_blocks(ops: &mut Vec<IlOp>) {
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


fn stack_dce(ops: &mut Vec<IlOp>) {
    let mut out = Vec::with_capacity(ops.len());
    let mut i = 0;
    while i < ops.len() {
        if i + 1 < ops.len()
            && matches!(
                &ops[i],
                IlOp::Byte { byte, .. } if *byte.bytecode() == common::Instruction::DUPLICATE
            )
            && matches!(
                &ops[i + 1],
                IlOp::Byte { byte, .. } if *byte.bytecode() == common::Instruction::POP
            )
        {
            i += 2;
            continue;
        }
        if i + 1 < ops.len()
            && let (IlOp::Byte { byte: b0, .. }, IlOp::Byte { byte: b1, .. }) =
                (&ops[i], &ops[i + 1])
            && *b0.bytecode() == common::Instruction::LOAD
            && *b1.bytecode() == common::Instruction::StorePop
            && b0.operand_u32() == b1.operand_u32()
        {
            i += 2;
            continue;
        }
        out.push(ops[i].clone());
        i += 1;
    }
    *ops = out;
}

/// True if `byte` is a sinkable return producer (`LOAD s` or inline `CONST k`).
fn is_return_producer(byte: &common::Byte) -> bool {
    match *byte.bytecode() {
        Instruction::LOAD => byte.operand_u32() <= 255,
        Instruction::CONST => byte.operand_u32() & common::Byte::POOL_FLAG == 0,
        _ => false,
    }
}

fn fuse_producer_with_return(producer: common::Byte) -> common::Byte {
    match *producer.bytecode() {
        Instruction::LOAD => {
            common::Byte::new(Instruction::LoadReturnSlot).with_operand_u32(producer.operand_u32())
        }
        Instruction::CONST => {
            common::Byte::new(Instruction::ConstReturnImm).with_operand_u32(producer.operand_u32())
        }
        _ => unreachable!("is_return_producer gate"),
    }
}

/// Producer must sit immediately before `idx` (no intervening labels). Skipping
/// labels would attribute an outer join's CONST/LOAD to a later `Label; RETURN`
/// bind and fuse away a stacked arm value (Ord Lt diamond).
fn immediate_producer_before(ops: &[IlOp], idx: usize) -> Option<(usize, common::Byte)> {
    if idx == 0 {
        return None;
    }
    match &ops[idx - 1] {
        IlOp::Byte { byte, .. } if is_return_producer(byte) => Some((idx - 1, *byte)),
        _ => None,
    }
}

/// Sink identical `LOAD`/`CONST` producers into `Label(L); RETURN` and fuse to
/// `LoadReturnSlot` / `ConstReturnImm`, rebinding `L` onto the fused op.
fn return_convoy(ops: &mut Vec<IlOp>) {
    let mut joins: Vec<(usize, Label, common::Byte)> = Vec::new();
    let mut i = 0;
    while i + 1 < ops.len() {
        let IlOp::Label(join) = ops[i] else {
            i += 1;
            continue;
        };
        if !ops[i + 1].is_plain_return() {
            i += 1;
            continue;
        }
        let Some((_, fall_p)) = immediate_producer_before(ops, i) else {
            i += 1;
            continue;
        };

        let mut ok = true;
        let mut jump_preds = 0usize;
        for (j, op) in ops.iter().enumerate() {
            let IlOp::Jump {
                kind,
                target,
                ..
            } = op
            else {
                continue;
            };
            if *target != join {
                continue;
            }
            if *kind != IlJumpKind::Unconditional {
                ok = false;
                break;
            }
            let Some((_p_idx, p)) = immediate_producer_before(ops, j) else {
                ok = false;
                break;
            };
            if p != fall_p {
                ok = false;
                break;
            }
            jump_preds += 1;
        }
        if !ok || jump_preds == 0 {
            i += 1;
            continue;
        }

        joins.push((i, join, fall_p));
        i += 1;
    }

    if joins.is_empty() {
        return;
    }

    let mut remove_producer_at: std::collections::HashSet<usize> =
        std::collections::HashSet::new();
    let mut fuse_at_label: std::collections::HashMap<usize, common::Byte> =
        std::collections::HashMap::new();

    for (lab_idx, join, producer) in &joins {
        if let Some((fall_idx, p)) = immediate_producer_before(ops, *lab_idx)
            && p == *producer
        {
            remove_producer_at.insert(fall_idx);
        }
        for (j, op) in ops.iter().enumerate() {
            if let IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target,
                ..
            } = op
                && *target == *join
                && let Some((p_idx, p)) = immediate_producer_before(ops, j)
                && p == *producer
            {
                remove_producer_at.insert(p_idx);
            }
        }
        fuse_at_label.insert(*lab_idx, fuse_producer_with_return(*producer));
    }

    let mut out = Vec::with_capacity(ops.len());
    let mut idx = 0;
    while idx < ops.len() {
        if remove_producer_at.contains(&idx) {
            idx += 1;
            continue;
        }
        if let Some(fused) = fuse_at_label.get(&idx) {
            out.push(ops[idx].clone());
            out.push(IlOp::byte(*fused));
            idx += 2;
            continue;
        }
        out.push(ops[idx].clone());
        idx += 1;
    }
    *ops = out;
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{Byte, Instruction};

    #[test]
    fn jump_thread_collapses_goto_goto() {
        let mut ops = vec![
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Byte {
                byte: Byte::new(Instruction::CONST).with_const_inline(1),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Label(Label(0)),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(1),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Label(Label(1)),
            IlOp::Byte {
                byte: Byte::new(Instruction::HALT),
                loc: common::DebugLoc::unknown(),
            },
        ];
        jump_thread(&mut ops);
        match &ops[0] {
            IlOp::Jump {
                target: Label(1), ..
            } => {}
            _ => panic!("expected JMP L1 after jump threading"),
        }
    }

    #[test]
    fn stack_dce_removes_dup_pop() {
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::DUPLICATE)),
            IlOp::byte(Byte::new(Instruction::POP)),
            IlOp::byte(Byte::new(Instruction::HALT)),
        ];
        stack_dce(&mut ops);
        assert_eq!(ops.len(), 1);
    }

    #[test]
    fn dead_block_drops_after_unconditional_jmp() {
        let mut ops = vec![
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(1)),
            IlOp::Label(Label(0)),
            IlOp::byte(Byte::new(Instruction::HALT)),
        ];
        eliminate_dead_blocks(&mut ops);
        assert_eq!(ops.len(), 3);
        assert!(matches!(ops[1], IlOp::Label(Label(0))));
    }

    #[test]
    fn dead_block_drops_after_return_until_label() {
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::RETURN)),
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(1)),
            IlOp::Label(Label(0)),
            IlOp::byte(Byte::new(Instruction::HALT)),
        ];
        eliminate_dead_blocks(&mut ops);
        assert_eq!(ops.len(), 3);
        assert!(matches!(ops[0], IlOp::Byte { .. }));
        assert!(matches!(ops[1], IlOp::Label(Label(0))));
    }

    #[test]
    fn return_convoy_fuses_agreeing_const_join() {
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(0)),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(0)),
            IlOp::Label(Label(0)),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        return_convoy(&mut ops);
        assert!(
            ops.iter().any(|op| matches!(
                op,
                IlOp::Byte { byte, .. } if *byte.bytecode() == Instruction::ConstReturnImm
            )),
            "expected ConstReturnImm"
        );
        assert!(
            !ops.iter().any(|op| matches!(
                op,
                IlOp::Byte { byte, .. } if *byte.bytecode() == Instruction::CONST
            )),
            "producers should be stripped"
        );
        assert!(ops.iter().any(|op| matches!(op, IlOp::Label(Label(0)))));
        assert!(ops.iter().any(|op| matches!(
            op,
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                ..
            }
        )));
    }

    #[test]
    fn return_convoy_fuses_agreeing_load_join() {
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::LOAD).with_operand_u32(0)),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::LOAD).with_operand_u32(0)),
            IlOp::Label(Label(0)),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        return_convoy(&mut ops);
        assert!(ops.iter().any(|op| matches!(
            op,
            IlOp::Byte { byte, .. } if *byte.bytecode() == Instruction::LoadReturnSlot
                && byte.operand_u32() == 0
        )));
    }

    #[test]
    fn return_convoy_skips_disagreeing_consts() {
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(0)),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(1)),
            IlOp::Label(Label(0)),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        let before = ops.clone();
        return_convoy(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn return_convoy_skips_jump_without_producer() {
        // JMP to join with a value already on the stack (no LOAD/CONST before JMP).
        let mut ops = vec![
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(0)),
            IlOp::Label(Label(0)),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        let before = ops.clone();
        return_convoy(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn return_convoy_skips_producer_behind_intervening_label() {
        // Match/if join binds Label(54) on the stacked value; Label(48) is a
        // second bind before RETURN. Must not attribute CONST to Label(48).
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(0)),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(54),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(1)),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(54),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(0)),
            IlOp::Label(Label(54)),
            IlOp::Label(Label(48)),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        let before = ops.clone();
        return_convoy(&mut ops);
        assert!(ops == before);
    }
}
