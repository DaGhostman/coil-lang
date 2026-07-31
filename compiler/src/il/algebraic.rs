//! Algebraic / strength-reduction peeps on typed stack IL (Known-SP windows).

use common::Instruction;

use super::op::IlOp;
use super::sp;

/// Cheap identity / strength rewrites. Refuses when SP-in mid-window is Unknown.
pub fn algebraic_simplify(ops: &mut Vec<IlOp>) {
    if ops.len() < 2 {
        return;
    }
    let info = sp::analyze(ops);
    let mut out = Vec::with_capacity(ops.len());
    let mut i = 0;
    while i < ops.len() {
        if !info.sp_before(i).is_known() {
            out.push(ops[i].clone());
            i += 1;
            continue;
        }

        // Const a; Const b; CMP → Const 0/1 when both inline.
        if i + 2 < ops.len()
            && let (IlOp::Const { imm: a, loc }, IlOp::Const { imm: b, .. }, IlOp::Bin { op, .. }) =
                (&ops[i], &ops[i + 1], &ops[i + 2])
            && is_int_cmp(*op)
            && info.sp_before(i + 1).is_known()
            && info.sp_before(i + 2).is_known()
        {
            let r = eval_cmp(*op, *a, *b);
            out.push(IlOp::Const {
                imm: r,
                loc: *loc,
            });
            i += 3;
            continue;
        }

        // Double LogicalNot (NOT; NOT) → identity (drop both).
        if i + 1 < ops.len()
            && is_logical_not(&ops[i])
            && is_logical_not(&ops[i + 1])
            && info.sp_before(i + 1).is_known()
        {
            i += 2;
            continue;
        }

        // BinSlotImm identity: slot+0, slot-0, slot*1, slot/1 → Load slot
        if let IlOp::BinSlotImm {
            op,
            slot,
            imm,
            loc,
        } = &ops[i]
            && let Some(load) = bin_slot_imm_identity(*op, *slot, *imm, *loc)
        {
            out.push(load);
            i += 1;
            continue;
        }

        // Load/Const; Const/Load; Bin identity / zeroing.
        if i + 2 < ops.len()
            && info.sp_before(i + 1).is_known()
            && info.sp_before(i + 2).is_known()
            && let Some(rewritten) = try_bin_identity(&ops[i], &ops[i + 1], &ops[i + 2])
        {
            out.push(rewritten);
            i += 3;
            continue;
        }

        out.push(ops[i].clone());
        i += 1;
    }
    *ops = out;
}

fn is_logical_not(op: &IlOp) -> bool {
    matches!(op, IlOp::Byte { byte, .. } if *byte.bytecode() == Instruction::NOT)
        || matches!(op.as_encode_byte(), Some(b) if *b.bytecode() == Instruction::NOT)
}

fn is_int_cmp(op: Instruction) -> bool {
    matches!(
        op,
        Instruction::EQ
            | Instruction::NEQ
            | Instruction::LE
            | Instruction::LEQ
            | Instruction::GT
            | Instruction::GEQ
    )
}

fn eval_cmp(op: Instruction, a: i32, b: i32) -> i32 {
    let t = match op {
        Instruction::EQ => a == b,
        Instruction::NEQ => a != b,
        Instruction::LE => a < b,
        Instruction::LEQ => a <= b,
        Instruction::GT => a > b,
        Instruction::GEQ => a >= b,
        _ => return 0,
    };
    i32::from(t)
}

fn bin_slot_imm_identity(op: u8, slot: u8, imm: i16, loc: common::DebugLoc) -> Option<IlOp> {
    let insn = Instruction::from(op);
    let keep = match insn {
        Instruction::ADD | Instruction::SUB if imm == 0 => true,
        Instruction::MUL | Instruction::DIV if imm == 1 => true,
        _ => false,
    };
    if keep {
        Some(IlOp::Load {
            slot: slot as u32,
            loc,
        })
    } else if matches!(insn, Instruction::MUL) && imm == 0 {
        Some(IlOp::Const { imm: 0, loc })
    } else {
        None
    }
}

fn try_bin_identity(a: &IlOp, b: &IlOp, bin: &IlOp) -> Option<IlOp> {
    let IlOp::Bin { op, loc } = bin else {
        return None;
    };
    let loc = *loc;
    match (*op, a, b) {
        // x + 0 / x - 0 / x * 1 / x / 1 → x
        (Instruction::ADD | Instruction::SUB, x, IlOp::Const { imm: 0, .. })
        | (Instruction::MUL | Instruction::DIV, x, IlOp::Const { imm: 1, .. }) => {
            if matches!(x, IlOp::Load { .. } | IlOp::Const { .. } | IlOp::ConstPool { .. }) {
                Some(x.clone())
            } else {
                None
            }
        }
        // 0 + x / 1 * x → x
        (Instruction::ADD, IlOp::Const { imm: 0, .. }, x)
        | (Instruction::MUL, IlOp::Const { imm: 1, .. }, x) => {
            if matches!(x, IlOp::Load { .. } | IlOp::Const { .. } | IlOp::ConstPool { .. }) {
                Some(x.clone())
            } else {
                None
            }
        }
        // x - x / x * 0 / 0 * x → Const 0 (same Load slot or same Const)
        (Instruction::SUB, IlOp::Load { slot: s0, .. }, IlOp::Load { slot: s1, .. })
            if s0 == s1 =>
        {
            Some(IlOp::Const { imm: 0, loc })
        }
        (Instruction::SUB, IlOp::Const { imm: a, .. }, IlOp::Const { imm: b, .. }) if a == b => {
            Some(IlOp::Const { imm: 0, loc })
        }
        (Instruction::MUL, _, IlOp::Const { imm: 0, .. })
        | (Instruction::MUL, IlOp::Const { imm: 0, .. }, _) => Some(IlOp::Const { imm: 0, loc }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::DebugLoc;
    use super::sp::Sp;

    fn loc() -> DebugLoc {
        DebugLoc::unknown()
    }

    #[test]
    fn add_zero_folds_to_load() {
        let mut ops = vec![
            IlOp::Load { slot: 1, loc: loc() },
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        algebraic_simplify(&mut ops);
        assert!(matches!(ops[0], IlOp::Load { slot: 1, .. }));
        assert!(matches!(ops[1], IlOp::Return { .. }));
    }

    #[test]
    fn mul_zero_folds_to_const_zero() {
        let mut ops = vec![
            IlOp::Load { slot: 2, loc: loc() },
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::Bin {
                op: Instruction::MUL,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        algebraic_simplify(&mut ops);
        assert!(matches!(ops[0], IlOp::Const { imm: 0, .. }));
    }

    #[test]
    fn sub_same_load_folds_to_zero() {
        let mut ops = vec![
            IlOp::Load { slot: 4, loc: loc() },
            IlOp::Load { slot: 4, loc: loc() },
            IlOp::Bin {
                op: Instruction::SUB,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        algebraic_simplify(&mut ops);
        assert!(matches!(ops[0], IlOp::Const { imm: 0, .. }));
    }

    #[test]
    fn cmp_const_folds() {
        let mut ops = vec![
            IlOp::Const { imm: 3, loc: loc() },
            IlOp::Const { imm: 3, loc: loc() },
            IlOp::Bin {
                op: Instruction::EQ,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        algebraic_simplify(&mut ops);
        assert!(matches!(ops[0], IlOp::Const { imm: 1, .. }));
    }

    #[test]
    fn double_not_eliminated() {
        let mut ops = vec![
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::byte(common::Byte::new(Instruction::NOT)),
            IlOp::byte(common::Byte::new(Instruction::NOT)),
            IlOp::Return { loc: loc() },
        ];
        algebraic_simplify(&mut ops);
        assert_eq!(ops.len(), 2);
        assert!(matches!(ops[0], IlOp::Const { imm: 1, .. }));
    }

    #[test]
    fn bin_slot_imm_add_zero_to_load() {
        let mut ops = vec![
            IlOp::BinSlotImm {
                op: Instruction::ADD as u8,
                slot: 3,
                imm: 0,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        algebraic_simplify(&mut ops);
        assert!(matches!(ops[0], IlOp::Load { slot: 3, .. }));
    }

    #[test]
    fn refuses_when_sp_unknown() {
        let mut ops = vec![
            IlOp::byte(common::Byte::new(Instruction::FORMAT)),
            IlOp::Load { slot: 1, loc: loc() },
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        let before = ops.clone();
        algebraic_simplify(&mut ops);
        // Window starting at Load has Unknown SP-in after FORMAT.
        assert_eq!(ops.len(), before.len());
    }

    #[test]
    fn zero_plus_load_folds_to_load() {
        let mut ops = vec![
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::Load { slot: 5, loc: loc() },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        algebraic_simplify(&mut ops);
        assert!(matches!(ops[0], IlOp::Load { slot: 5, .. }));
        assert!(matches!(ops[1], IlOp::Return { .. }));
    }

    #[test]
    fn bin_slot_imm_mul_zero_to_const_zero() {
        let mut ops = vec![
            IlOp::BinSlotImm {
                op: Instruction::MUL as u8,
                slot: 2,
                imm: 0,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        algebraic_simplify(&mut ops);
        assert!(matches!(ops[0], IlOp::Const { imm: 0, .. }));
    }

    #[test]
    fn const_pool_plus_zero_folds_to_pool() {
        let mut ops = vec![
            IlOp::ConstPool { idx: 3, loc: loc() },
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        algebraic_simplify(&mut ops);
        assert!(matches!(ops[0], IlOp::ConstPool { idx: 3, .. }));
        assert!(matches!(ops[1], IlOp::Return { .. }));
    }

    #[test]
    fn analyze_known_gate_smoke() {
        let ops = vec![IlOp::Const { imm: 1, loc: loc() }];
        assert!(matches!(sp::analyze(&ops).sp_before(0), Sp::Known(0)));
    }
}
