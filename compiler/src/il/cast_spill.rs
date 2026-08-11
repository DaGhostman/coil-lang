//! Spill `CastIntToFloat` that sits inside a float-arith→STORE window.
//!
//! Fuse-select's `FloatChainStore` stage0 only accepts `LOAD; LOAD; op` or
//! float `BinSlotSlot`. An intervening cast blocks that shape (mandelbrot `cr`).
//! Rewriting `CastIntToFloat` → `CastIntToFloat; STORE t; LOAD t` leaves a
//! float `LOAD` for the chain while preserving stack height (net-zero).

use common::Instruction;

use super::op::IlOp;
use super::sp;

/// Spill casts that block a float-arith→STORE window. Fail-closed on Unknown SP.
pub fn spill_cast_before_float_chain(ops: &mut Vec<IlOp>) {
    if ops.len() < 4 {
        return;
    }
    let mut max_slot = max_slot_used(ops);
    loop {
        let info = sp::analyze(ops);
        let mut spilled = false;
        let mut i = 0;
        while i < ops.len() {
            if !is_cast_int_to_float(&ops[i]) || !info.sp_before(i).is_known() {
                i += 1;
                continue;
            }
            if i + 2 < ops.len()
                && let (IlOp::StorePop { slot: s1, .. }, IlOp::Load { slot: s2, .. }) =
                    (&ops[i + 1], &ops[i + 2])
                && s1 == s2
            {
                i += 1;
                continue;
            }
            if !cast_blocks_float_chain(ops, i) {
                i += 1;
                continue;
            }
            let loc = ops[i].loc();
            max_slot = max_slot.saturating_add(1);
            let temp = max_slot;
            ops.insert(i + 1, IlOp::StorePop { slot: temp, loc });
            ops.insert(i + 2, IlOp::Load { slot: temp, loc });
            spilled = true;
            break;
        }
        if !spilled {
            break;
        }
    }
}

fn max_slot_used(ops: &[IlOp]) -> u32 {
    let mut m = 0u32;
    for op in ops {
        match op {
            IlOp::Load { slot, .. } | IlOp::StorePop { slot, .. } => m = m.max(*slot),
            IlOp::BinSlotImm { slot, .. } => m = m.max(*slot as u32),
            IlOp::BinSlotSlot { a, b, .. } => m = m.max(*a as u32).max(*b as u32),
            IlOp::Byte { byte, .. }
                if matches!(
                    *byte.bytecode(),
                    Instruction::LOAD | Instruction::STORE | Instruction::StorePop
                ) =>
            {
                for k in 0..byte.load_store_count() {
                    m = m.max(byte.load_store_slot_at(k));
                }
            }
            _ => {}
        }
    }
    m
}

fn is_cast_int_to_float(op: &IlOp) -> bool {
    match op {
        IlOp::Byte { byte, .. } => *byte.bytecode() == Instruction::CastIntToFloat,
        other => other
            .as_encode_byte()
            .is_some_and(|b| *b.bytecode() == Instruction::CastIntToFloat),
    }
}

fn is_float_arith(op: &IlOp) -> bool {
    match op {
        IlOp::Bin { op, .. } => matches!(
            *op,
            Instruction::ADDF
                | Instruction::SUBF
                | Instruction::MULF
                | Instruction::DIVF
                | Instruction::MODF
        ),
        IlOp::BinSlotSlot { op, .. } | IlOp::BinSlotImm { op, .. } => matches!(
            Instruction::from(*op),
            Instruction::ADDF
                | Instruction::SUBF
                | Instruction::MULF
                | Instruction::DIVF
                | Instruction::MODF
        ),
        other => other.as_encode_byte().is_some_and(|b| {
            matches!(
                *b.bytecode(),
                Instruction::ADDF
                    | Instruction::SUBF
                    | Instruction::MULF
                    | Instruction::DIVF
                    | Instruction::MODF
            )
        }),
    }
}

fn is_store(op: &IlOp) -> bool {
    matches!(op, IlOp::StorePop { .. })
        || op.as_encode_byte().is_some_and(|b| {
            matches!(
                *b.bytecode(),
                Instruction::STORE | Instruction::StorePop | Instruction::FloatChainStore
            )
        })
}

fn cast_blocks_float_chain(ops: &[IlOp], cast_i: usize) -> bool {
    let window_end = ops.len().min(cast_i + 1 + 10);
    let mut float_ops = 0usize;
    let mut saw_store = false;
    for op in ops.iter().take(window_end).skip(cast_i + 1) {
        if is_float_arith(op) {
            float_ops += 1;
        }
        if is_store(op) {
            saw_store = true;
            break;
        }
        if is_cast_int_to_float(op)
            || matches!(
                op,
                IlOp::Jump { .. } | IlOp::Label(_) | IlOp::Return { .. } | IlOp::Halt { .. }
            )
        {
            break;
        }
    }
    saw_store && float_ops >= 2
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{Byte, DebugLoc};

    fn loc() -> DebugLoc {
        DebugLoc::unknown()
    }

    #[test]
    fn spills_cast_inside_float_arith_store_window() {
        let mut ops = vec![
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::byte(Byte::new(Instruction::CastIntToFloat)),
            IlOp::ConstPool {
                idx: 0,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::MULF,
                loc: loc(),
            },
            IlOp::ConstPool {
                idx: 1,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::SUBF,
                loc: loc(),
            },
            IlOp::StorePop {
                slot: 1,
                loc: loc(),
            },
        ];
        spill_cast_before_float_chain(&mut ops);
        assert!(is_cast_int_to_float(&ops[1]));
        assert!(matches!(ops[2], IlOp::StorePop { slot: 2, .. }));
        assert!(matches!(ops[3], IlOp::Load { slot: 2, .. }));
    }

    #[test]
    fn refuses_cast_without_float_chain_store_window() {
        let mut ops = vec![
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::byte(Byte::new(Instruction::CastIntToFloat)),
            IlOp::StorePop {
                slot: 1,
                loc: loc(),
            },
        ];
        let before = ops.clone();
        spill_cast_before_float_chain(&mut ops);
        assert!(ops == before);
    }
}
