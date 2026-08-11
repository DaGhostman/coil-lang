//! Operand-order canonicalization for stack IL.
//!
//! Rewrites Known-SP windows into preferred forms so fuse-select, algebraic
//! peeps, and GVN/CSE match more often:
//! - `Const; Load; op` → `Load; Const; op'` (const on RHS)
//! - `Load a; Load b; op` with `a > b` → swapped loads (+ cmp polarity flip)
//!
//! Refuses: Unknown SP, float ops, `ConstPool`, residual `Byte`, and
//! non-commutative ops (`SUB`/`DIV`/`MOD`/`SHL`/`SHR`/`Pow`). No float reassoc.

use common::Instruction;

use super::op::IlOp;
use super::sp;

/// Normalize operand order in place when SP-in is Known for the window.
pub fn canonicalize_operand_order(ops: &mut Vec<IlOp>) {
    if ops.len() < 3 {
        return;
    }
    let info = sp::analyze(ops);
    let mut out = Vec::with_capacity(ops.len());
    let mut i = 0;
    while i < ops.len() {
        if i + 2 < ops.len()
            && info.sp_before(i).is_known()
            && info.sp_before(i + 1).is_known()
            && info.sp_before(i + 2).is_known()
        {
            if let Some(rewritten) = try_const_load_bin(&ops[i], &ops[i + 1], &ops[i + 2]) {
                out.extend(rewritten);
                i += 3;
                continue;
            }
            if let Some(rewritten) = try_load_load_bin(&ops[i], &ops[i + 1], &ops[i + 2]) {
                out.extend(rewritten);
                i += 3;
                continue;
            }
        }
        out.push(ops[i].clone());
        i += 1;
    }
    *ops = out;
}

fn is_commute_keep(op: Instruction) -> bool {
    matches!(
        op,
        Instruction::ADD
            | Instruction::MUL
            | Instruction::EQ
            | Instruction::NEQ
            | Instruction::AND
            | Instruction::OR
            | Instruction::BITAND
            | Instruction::BITOR
            | Instruction::XOR
    )
}

fn flip_ordered_cmp(op: Instruction) -> Option<Instruction> {
    Some(match op {
        Instruction::LE => Instruction::GT,
        Instruction::GT => Instruction::LE,
        Instruction::LEQ => Instruction::GEQ,
        Instruction::GEQ => Instruction::LEQ,
        _ => return None,
    })
}

fn swap_binop(op: Instruction) -> Option<Instruction> {
    if is_commute_keep(op) {
        Some(op)
    } else {
        flip_ordered_cmp(op)
    }
}

/// `Const k; Load s; op` → `Load s; Const k; op'` when swappable.
fn try_const_load_bin(a: &IlOp, b: &IlOp, c: &IlOp) -> Option<[IlOp; 3]> {
    let (IlOp::Const { imm, loc }, IlOp::Load { slot, .. }, IlOp::Bin { op, .. }) = (a, b, c)
    else {
        return None;
    };
    let op2 = swap_binop(*op)?;
    Some([
        IlOp::Load {
            slot: *slot,
            loc: *loc,
        },
        IlOp::Const {
            imm: *imm,
            loc: *loc,
        },
        IlOp::Bin {
            op: op2,
            loc: *loc,
        },
    ])
}

/// `Load a; Load b; op` with `a > b` → swapped loads (+ flipped cmp).
fn try_load_load_bin(a: &IlOp, b: &IlOp, c: &IlOp) -> Option<[IlOp; 3]> {
    let (IlOp::Load { slot: sa, loc }, IlOp::Load { slot: sb, .. }, IlOp::Bin { op, .. }) = (a, b, c)
    else {
        return None;
    };
    if *sa <= *sb {
        return None;
    }
    let op2 = swap_binop(*op)?;
    Some([
        IlOp::Load {
            slot: *sb,
            loc: *loc,
        },
        IlOp::Load {
            slot: *sa,
            loc: *loc,
        },
        IlOp::Bin {
            op: op2,
            loc: *loc,
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::DebugLoc;

    fn loc() -> DebugLoc {
        DebugLoc::unknown()
    }

    #[test]
    fn const_load_add_swaps_to_load_const_add() {
        let mut ops = vec![
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: loc(),
            },
        ];
        canonicalize_operand_order(&mut ops);
        assert!(matches!(ops[0], IlOp::Load { slot: 0, .. }));
        assert!(matches!(ops[1], IlOp::Const { imm: 1, .. }));
        assert!(matches!(ops[2], IlOp::Bin { op: Instruction::ADD, .. }));
    }

    #[test]
    fn const_load_mul_swaps() {
        let mut ops = vec![
            IlOp::Const { imm: 3, loc: loc() },
            IlOp::Load {
                slot: 2,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::MUL,
                loc: loc(),
            },
        ];
        canonicalize_operand_order(&mut ops);
        assert!(matches!(ops[0], IlOp::Load { slot: 2, .. }));
        assert!(matches!(ops[1], IlOp::Const { imm: 3, .. }));
        assert!(matches!(ops[2], IlOp::Bin { op: Instruction::MUL, .. }));
    }

    #[test]
    fn const_load_eq_keeps_eq() {
        let mut ops = vec![
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::Load {
                slot: 1,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::EQ,
                loc: loc(),
            },
        ];
        canonicalize_operand_order(&mut ops);
        assert!(matches!(ops[0], IlOp::Load { slot: 1, .. }));
        assert!(matches!(ops[1], IlOp::Const { imm: 0, .. }));
        assert!(matches!(ops[2], IlOp::Bin { op: Instruction::EQ, .. }));
    }

    #[test]
    fn const_load_le_becomes_load_const_gt() {
        let mut ops = vec![
            IlOp::Const { imm: 5, loc: loc() },
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::LE,
                loc: loc(),
            },
        ];
        canonicalize_operand_order(&mut ops);
        assert!(matches!(ops[0], IlOp::Load { slot: 0, .. }));
        assert!(matches!(ops[1], IlOp::Const { imm: 5, .. }));
        assert!(matches!(ops[2], IlOp::Bin { op: Instruction::GT, .. }));
    }

    #[test]
    fn const_load_leq_becomes_geq() {
        let mut ops = vec![
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::LEQ,
                loc: loc(),
            },
        ];
        canonicalize_operand_order(&mut ops);
        assert!(matches!(ops[2], IlOp::Bin { op: Instruction::GEQ, .. }));
    }

    #[test]
    fn const_load_gt_becomes_le() {
        let mut ops = vec![
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::GT,
                loc: loc(),
            },
        ];
        canonicalize_operand_order(&mut ops);
        assert!(matches!(ops[2], IlOp::Bin { op: Instruction::LE, .. }));
    }

    #[test]
    fn const_load_geq_becomes_leq() {
        let mut ops = vec![
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::GEQ,
                loc: loc(),
            },
        ];
        canonicalize_operand_order(&mut ops);
        assert!(matches!(ops[2], IlOp::Bin { op: Instruction::LEQ, .. }));
    }

    #[test]
    fn load_const_add_unchanged() {
        let mut ops = vec![
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: loc(),
            },
        ];
        let before = ops.clone();
        canonicalize_operand_order(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn const_load_sub_refused() {
        let mut ops = vec![
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::SUB,
                loc: loc(),
            },
        ];
        let before = ops.clone();
        canonicalize_operand_order(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn unknown_sp_refused() {
        let mut ops = vec![
            IlOp::byte(common::Byte::new(Instruction::FfiInvoke)),
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: loc(),
            },
        ];
        let before = ops.clone();
        canonicalize_operand_order(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn load_high_load_low_add_swaps_slots() {
        let mut ops = vec![
            IlOp::Load {
                slot: 3,
                loc: loc(),
            },
            IlOp::Load {
                slot: 1,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: loc(),
            },
        ];
        canonicalize_operand_order(&mut ops);
        assert!(matches!(ops[0], IlOp::Load { slot: 1, .. }));
        assert!(matches!(ops[1], IlOp::Load { slot: 3, .. }));
        assert!(matches!(ops[2], IlOp::Bin { op: Instruction::ADD, .. }));
    }

    #[test]
    fn load_low_load_high_add_unchanged() {
        let mut ops = vec![
            IlOp::Load {
                slot: 1,
                loc: loc(),
            },
            IlOp::Load {
                slot: 3,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: loc(),
            },
        ];
        let before = ops.clone();
        canonicalize_operand_order(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn load_high_load_low_le_becomes_gt() {
        let mut ops = vec![
            IlOp::Load {
                slot: 4,
                loc: loc(),
            },
            IlOp::Load {
                slot: 2,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::LE,
                loc: loc(),
            },
        ];
        canonicalize_operand_order(&mut ops);
        assert!(matches!(ops[0], IlOp::Load { slot: 2, .. }));
        assert!(matches!(ops[1], IlOp::Load { slot: 4, .. }));
        assert!(matches!(ops[2], IlOp::Bin { op: Instruction::GT, .. }));
    }

    #[test]
    fn load_high_load_low_sub_refused() {
        let mut ops = vec![
            IlOp::Load {
                slot: 5,
                loc: loc(),
            },
            IlOp::Load {
                slot: 1,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::SUB,
                loc: loc(),
            },
        ];
        let before = ops.clone();
        canonicalize_operand_order(&mut ops);
        assert!(ops == before);
    }

    /// Preferred shape after Rewrite A — what `BinSlotImm` fuse expects.
    #[test]
    fn preferred_load_const_add_shape_after_canon() {
        let mut ops = vec![
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        canonicalize_operand_order(&mut ops);
        assert!(matches!(ops[0], IlOp::Load { slot: 0, .. }));
        assert!(matches!(ops[1], IlOp::Const { imm: 1, .. }));
        assert!(matches!(ops[2], IlOp::Bin { op: Instruction::ADD, .. }));
        assert!(matches!(ops[3], IlOp::Return { .. }));
    }

    #[test]
    fn const_load_div_and_pow_refused() {
        for op in [Instruction::DIV, Instruction::Pow, Instruction::SHL] {
            let mut ops = vec![
                IlOp::Const { imm: 2, loc: loc() },
                IlOp::Load {
                    slot: 0,
                    loc: loc(),
                },
                IlOp::Bin { op, loc: loc() },
            ];
            let before = ops.clone();
            canonicalize_operand_order(&mut ops);
            assert!(ops == before, "must refuse non-commutative {:?}", op);
        }
    }

    #[test]
    fn const_pool_load_add_refused() {
        let mut ops = vec![
            IlOp::ConstPool {
                idx: 0,
                loc: loc(),
            },
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: loc(),
            },
        ];
        let before = ops.clone();
        canonicalize_operand_order(&mut ops);
        assert!(ops == before, "ConstPool must not rewrite like Const");
    }

    #[test]
    fn const_load_addf_refused() {
        let mut ops = vec![
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::ADDF,
                loc: loc(),
            },
        ];
        let before = ops.clone();
        canonicalize_operand_order(&mut ops);
        assert!(ops == before, "float ops must not reassoc via canon");
    }

    #[test]
    fn load_high_load_low_bitand_keeps_op() {
        let mut ops = vec![
            IlOp::Load {
                slot: 5,
                loc: loc(),
            },
            IlOp::Load {
                slot: 1,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::BITAND,
                loc: loc(),
            },
        ];
        canonicalize_operand_order(&mut ops);
        assert!(matches!(ops[0], IlOp::Load { slot: 1, .. }));
        assert!(matches!(ops[1], IlOp::Load { slot: 5, .. }));
        assert!(matches!(
            ops[2],
            IlOp::Bin {
                op: Instruction::BITAND,
                ..
            }
        ));
    }

    #[test]
    fn optimize_with_canon_disabled_keeps_const_load() {
        use super::super::opt::{OptimizeOptions, optimize};
        let mut ops = vec![
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        optimize(
            &mut ops,
            &OptimizeOptions {
                canon: false,
                algebraic: false,
                ..OptimizeOptions::default()
            },
            &mut Vec::new(),
        );
        assert!(
            matches!(ops[0], IlOp::Const { imm: 1, .. }),
            "canon:false must leave Const;Load;ADD"
        );
    }

    #[test]
    fn successive_windows_both_canonicalize() {
        let mut ops = vec![
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: loc(),
            },
            IlOp::Const { imm: 2, loc: loc() },
            IlOp::Load {
                slot: 1,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::MUL,
                loc: loc(),
            },
        ];
        canonicalize_operand_order(&mut ops);
        assert!(matches!(ops[0], IlOp::Load { slot: 0, .. }));
        assert!(matches!(ops[1], IlOp::Const { imm: 1, .. }));
        assert!(matches!(ops[3], IlOp::Load { slot: 1, .. }));
        assert!(matches!(ops[4], IlOp::Const { imm: 2, .. }));
    }
}
