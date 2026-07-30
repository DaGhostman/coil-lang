//! Stack-height (SP) analysis over IL ops.
//!
//! Used by return-join convoys to refuse sinks when predecessor heights disagree
//! or any op in the region has an unknown stack effect.

use common::Instruction;

use super::op::{EntryKind, IlJumpKind, IlOp, Label};

/// Stack height relative to analysis entry (usually 0 at `ops[0]`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Sp {
    Known(i32),
    Unknown,
}

impl Sp {
    pub fn is_known(self) -> bool {
        matches!(self, Sp::Known(_))
    }

    pub fn known(self) -> Option<i32> {
        match self {
            Sp::Known(v) => Some(v),
            Sp::Unknown => None,
        }
    }

    fn apply(self, delta: Option<i32>) -> Sp {
        match (self, delta) {
            (Sp::Known(h), Some(d)) => Sp::Known(h + d),
            _ => Sp::Unknown,
        }
    }
}

/// Per-op SP-in (height before the op at that index).
#[derive(Clone, Debug)]
pub struct SpInfo {
    pub sp_in: Vec<Sp>,
}

impl SpInfo {
    pub fn sp_before(&self, idx: usize) -> Sp {
        self.sp_in.get(idx).copied().unwrap_or(Sp::Unknown)
    }
}

/// Net stack delta for `op`, or `None` if the effect is unknown / fail-closed.
pub fn stack_delta(op: &IlOp) -> Option<i32> {
    match op {
        IlOp::Label(_) => Some(0),
        IlOp::Load { .. } | IlOp::Const { .. } | IlOp::Dup { .. } => Some(1),
        IlOp::StorePop { .. } | IlOp::Pop { .. } => Some(-1),
        IlOp::Bin { .. } => Some(-1),
        // Slot forms push a computed value without consuming eval-stack args.
        IlOp::BinSlotImm { .. } | IlOp::BinSlotSlot { .. } => Some(1),
        // Terminators: treat as consuming the returned value for fall-through SP.
        IlOp::Return { .. } | IlOp::LoadReturnSlot { .. } | IlOp::ConstReturnImm { .. } => {
            Some(-1)
        }
        IlOp::BinReturn { .. } => Some(-2),
        IlOp::Halt { .. } => Some(0),
        IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            ..
        } => Some(0),
        IlOp::Jump {
            kind: IlJumpKind::JumpIfFalse | IlJumpKind::JumpIfTrue,
            ..
        } => Some(-1),
        IlOp::Jump {
            kind: IlJumpKind::JumpIfMatch { arity, .. },
            ..
        } => {
            // Scrutinee consumed; arity payloads left in slots — eval stack −1.
            let _ = arity;
            Some(-1)
        }
        IlOp::Entry {
            kind: EntryKind::Call | EntryKind::MakeCoro,
            arity,
            ..
        } => Some(1 - *arity as i32),
        IlOp::Entry {
            kind: EntryKind::TailCall,
            ..
        } => None,
        IlOp::Entry {
            kind: EntryKind::CodePtr | EntryKind::MakePolyFn,
            ..
        } => Some(1),
        IlOp::PrologueJmp { .. } => Some(0),
        IlOp::Byte { byte, .. } => byte_stack_delta(*byte.bytecode(), byte),
    }
}

fn byte_stack_delta(insn: Instruction, byte: &common::Byte) -> Option<i32> {
    match insn {
        Instruction::LOAD | Instruction::CONST | Instruction::DUPLICATE | Instruction::STRING
        | Instruction::CodePtr | Instruction::MakePolyFn => Some(1),
        Instruction::POP | Instruction::StorePop | Instruction::STORE => Some(-1),
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
        | Instruction::ADDF
        | Instruction::SUBF
        | Instruction::MULF
        | Instruction::DIVF
        | Instruction::MODF
        | Instruction::LEF
        | Instruction::LEQF
        | Instruction::GTF
        | Instruction::GEQF
        | Instruction::PowF
        | Instruction::SHL
        | Instruction::SHR
        | Instruction::XOR
        | Instruction::AND
        | Instruction::OR => Some(-1),
        Instruction::NOT | Instruction::NEG => Some(0),
        Instruction::BinSlotImm | Instruction::BinSlotSlot => Some(1),
        Instruction::RETURN | Instruction::LoadReturnSlot | Instruction::ConstReturnImm => {
            Some(-1)
        }
        Instruction::BinReturn => Some(-2),
        Instruction::HALT | Instruction::NOOP => Some(0),
        Instruction::JMP => Some(0),
        Instruction::JMPF | Instruction::JMPT => Some(-1),
        Instruction::CALL | Instruction::MakeCoro => {
            let (arity, _) = byte.call_parts();
            Some(1 - arity as i32)
        }
        Instruction::TailCall => None,
        // Fail closed for the long tail (PRINT, FORMAT, MakeEnum, HostInvoke, …).
        _ => None,
    }
}

fn is_terminator(op: &IlOp) -> bool {
    matches!(
        op,
        IlOp::Return { .. }
            | IlOp::Halt { .. }
            | IlOp::LoadReturnSlot { .. }
            | IlOp::ConstReturnImm { .. }
            | IlOp::BinReturn { .. }
            | IlOp::Entry {
                kind: EntryKind::TailCall,
                ..
            }
    ) || matches!(
        op.as_encode_byte(),
        Some(b) if matches!(
            *b.bytecode(),
            Instruction::RETURN
                | Instruction::HALT
                | Instruction::LoadReturnSlot
                | Instruction::ConstReturnImm
                | Instruction::BinReturn
                | Instruction::TailCall
        )
    )
}

/// Compute SP-in for each op. Entry SP is 0 at index 0; unknown effects poison.
pub fn analyze(ops: &[IlOp]) -> SpInfo {
    let n = ops.len();
    let mut sp_in: Vec<Option<Sp>> = vec![None; n];
    if n == 0 {
        return SpInfo { sp_in: Vec::new() };
    }
    sp_in[0] = Some(Sp::Known(0));

    let mut label_at: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    for (i, op) in ops.iter().enumerate() {
        if let IlOp::Label(Label(id)) = op {
            label_at.insert(*id, i);
        }
    }

    fn meet_into(slot: &mut Option<Sp>, incoming: Sp) -> bool {
        let next = match *slot {
            None => incoming,
            Some(Sp::Unknown) => Sp::Unknown,
            Some(Sp::Known(a)) => match incoming {
                Sp::Known(b) if a == b => Sp::Known(a),
                _ => Sp::Unknown,
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
        // `None` = no fall-through edge into the next index.
        let mut fall_sp: Option<Sp> = Some(Sp::Known(0));
        for i in 0..n {
            if i > 0
                && let Some(edge) = fall_sp
            {
                changed |= meet_into(&mut sp_in[i], edge);
            }

            let op = &ops[i];
            let before = sp_in[i].unwrap_or(Sp::Unknown);
            let delta = stack_delta(op);
            let after = before.apply(delta);

            if let IlOp::Jump { kind, target, .. } = op {
                if let Some(&t) = label_at.get(&target.0) {
                    let edge_sp = match kind {
                        IlJumpKind::Unconditional => before.apply(Some(0)),
                        IlJumpKind::JumpIfFalse | IlJumpKind::JumpIfTrue => {
                            before.apply(Some(-1))
                        }
                        IlJumpKind::JumpIfMatch { .. } => before.apply(Some(-1)),
                    };
                    changed |= meet_into(&mut sp_in[t], edge_sp);
                }
                fall_sp = match kind {
                    IlJumpKind::Unconditional => None,
                    _ => Some(after),
                };
            } else if is_terminator(op) {
                fall_sp = None;
            } else if matches!(op, IlOp::Label(_)) {
                fall_sp = Some(before);
            } else {
                fall_sp = Some(after);
            }
        }
        if !changed {
            break;
        }
    }

    SpInfo {
        sp_in: sp_in
            .into_iter()
            .map(|s| s.unwrap_or(Sp::Unknown))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::DebugLoc;

    fn loc() -> DebugLoc {
        DebugLoc::unknown()
    }

    #[test]
    fn straight_line_known_heights() {
        let ops = vec![
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Const { imm: 2, loc: loc() },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        let info = analyze(&ops);
        assert_eq!(info.sp_before(0), Sp::Known(0));
        assert_eq!(info.sp_before(1), Sp::Known(1));
        assert_eq!(info.sp_before(2), Sp::Known(2));
        assert_eq!(info.sp_before(3), Sp::Known(1));
    }

    #[test]
    fn diamond_agreeing_heights() {
        // CONST 0; JMPF Lelse; CONST 1; JMP Ljoin; Label Lelse; CONST 2; Label Ljoin; RETURN
        let ops = vec![
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(1),
                loc: loc(),
            },
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(2),
                loc: loc(),
            },
            IlOp::Label(Label(1)),
            IlOp::Const { imm: 2, loc: loc() },
            IlOp::Label(Label(2)),
            IlOp::Return { loc: loc() },
        ];
        let info = analyze(&ops);
        // After JMPF, SP is 0 on both arms; each CONST → 1 at join.
        assert_eq!(info.sp_before(6), Sp::Known(1)); // Label join
        assert_eq!(info.sp_before(7), Sp::Known(1)); // RETURN
    }

    #[test]
    fn diamond_mismatched_heights_unknown_at_join() {
        // Then-arm pushes two consts; else pushes one — join SP disagrees.
        let ops = vec![
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(1),
                loc: loc(),
            },
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Const { imm: 3, loc: loc() },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(2),
                loc: loc(),
            },
            IlOp::Label(Label(1)),
            IlOp::Const { imm: 2, loc: loc() },
            IlOp::Label(Label(2)),
            IlOp::Return { loc: loc() },
        ];
        let info = analyze(&ops);
        assert_eq!(info.sp_before(7), Sp::Unknown);
    }

    #[test]
    fn unknown_byte_poisons() {
        let ops = vec![
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::byte(common::Byte::new(Instruction::PRINT)),
            IlOp::Return { loc: loc() },
        ];
        let info = analyze(&ops);
        assert_eq!(info.sp_before(0), Sp::Known(0));
        assert_eq!(info.sp_before(1), Sp::Known(1));
        assert_eq!(info.sp_before(2), Sp::Unknown);
    }
}
