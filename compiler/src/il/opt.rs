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
}

impl Default for OptimizeOptions {
    fn default() -> Self {
        Self {
            jump_thread: true,
            // JMP + RETURN/HALT/*Return sweep; entry labels + CALL-0
            // continuations labeled.
            dead_block: true,
            stack_dce: true,
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
}
