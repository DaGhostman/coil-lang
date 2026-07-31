//! Loop-invariant code motion for Known-SP natural loops.

use std::collections::{HashMap, HashSet};

use super::op::{IlJumpKind, IlOp, Label};
use super::sp;

/// Hoist loop-invariant `Const` / `Load` out of natural loops when header
/// SP-in is Known. Inserts a preheader label and redirects external entries.
pub fn licm(ops: &mut Vec<IlOp>) {
    if ops.len() < 4 {
        return;
    }
    let info = sp::analyze(ops);
    let loops = find_natural_loops(ops);
    // Process innermost-first (later header index first) so index shifts stay local.
    let mut loops = loops;
    loops.sort_by_key(|l| std::cmp::Reverse(l.header));

    for lp in loops {
        if !info.sp_before(lp.header).is_known() {
            continue;
        }
        if loop_has_barrier(ops, &lp) {
            continue;
        }
        let stored = slots_stored_in_loop(ops, &lp);
        let mut hoist: Vec<(usize, IlOp)> = Vec::new();
        for i in lp.body_start()..lp.latch {
            match &ops[i] {
                IlOp::Const { .. } | IlOp::ConstPool { .. } => {
                    hoist.push((i, ops[i].clone()));
                }
                IlOp::Load { slot, .. } if !stored.contains(slot) => {
                    hoist.push((i, ops[i].clone()));
                }
                _ => {}
            }
        }
        if hoist.is_empty() {
            continue;
        }
        // Only hoist when net stack effect of hoisted ops is applied once at
        // preheader — v1: require each candidate is immediately used by a pure
        // consumer inside the loop (skip orphan Const that changes SP freely).
        // Simpler gate: hoist at most a single Const or Load per loop when it
        // appears as the first emitting op after the header label.
        let first_emit = (lp.body_start()..lp.latch).find(|&i| !matches!(ops[i], IlOp::Label(_)));
        let Some(fi) = first_emit else {
            continue;
        };
        let Some((_, cand)) = hoist.iter().find(|(i, _)| *i == fi) else {
            continue;
        };
        if !matches!(
            cand,
            IlOp::Const { .. } | IlOp::ConstPool { .. } | IlOp::Load { .. }
        ) {
            continue;
        }
        apply_hoist(ops, &lp, fi, cand.clone());
        // Indices invalidated; only one hoist per call — rebuild on next pass.
        break;
    }
}

#[derive(Clone, Debug)]
struct NaturalLoop {
    header: usize,
    /// Index of back-edge `Jump` (unconditional) to header.
    latch: usize,
    header_label: Label,
}

impl NaturalLoop {
    fn body_start(&self) -> usize {
        self.header + 1
    }
}

fn find_natural_loops(ops: &[IlOp]) -> Vec<NaturalLoop> {
    let mut label_at: HashMap<u32, usize> = HashMap::new();
    for (i, op) in ops.iter().enumerate() {
        if let IlOp::Label(Label(id)) = op {
            label_at.insert(*id, i);
        }
    }
    let mut out = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        let IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            target,
            ..
        } = op
        else {
            continue;
        };
        let Some(&h) = label_at.get(&target.0) else {
            continue;
        };
        // Back-edge: jump target is before the jump.
        if h >= i {
            continue;
        }
        out.push(NaturalLoop {
            header: h,
            latch: i,
            header_label: *target,
        });
    }
    out
}

fn loop_has_barrier(ops: &[IlOp], lp: &NaturalLoop) -> bool {
    for i in lp.header..=lp.latch {
        match &ops[i] {
            IlOp::HostInvoke { .. }
            | IlOp::Print { .. }
            | IlOp::SetField { .. }
            | IlOp::Entry { .. }
            | IlOp::GetField { .. } => return true,
            IlOp::Jump {
                kind: IlJumpKind::JumpIfMatch { .. },
                ..
            } => return true,
            IlOp::Byte { byte, .. }
                if matches!(
                    *byte.bytecode(),
                    common::Instruction::HostInvoke
                        | common::Instruction::PRINT
                        | common::Instruction::SetField
                        | common::Instruction::GetField
                        | common::Instruction::CALL
                        | common::Instruction::FORMAT
                        | common::Instruction::FfiInvoke
                ) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

fn slots_stored_in_loop(ops: &[IlOp], lp: &NaturalLoop) -> HashSet<u32> {
    let mut s = HashSet::new();
    for i in lp.header..=lp.latch {
        if let IlOp::StorePop { slot, .. } = &ops[i] {
            s.insert(*slot);
        }
    }
    s
}

fn apply_hoist(ops: &mut Vec<IlOp>, lp: &NaturalLoop, hoist_idx: usize, cand: IlOp) {
    let pre = Label(ops.iter().filter_map(|op| {
        if let IlOp::Label(Label(id)) = op {
            Some(*id)
        } else {
            None
        }
    }).max().unwrap_or(0).wrapping_add(1));

    // Redirect external jumps that targeted the header to the preheader,
    // except the latch back-edge (keeps jumping to header).
    for (i, op) in ops.iter_mut().enumerate() {
        if i == lp.latch {
            continue;
        }
        if let IlOp::Jump { target, .. } = op
            && *target == lp.header_label
        {
            *target = pre;
        }
    }

    // Remove hoisted op from body.
    ops.remove(hoist_idx);
    // Header index may shift if hoist was before header (it wasn't).
    let header = if hoist_idx < lp.header {
        lp.header - 1
    } else {
        lp.header
    };

    // Insert preheader: Label(pre); cand; JMP header
    let insert_at = header;
    let jmp = IlOp::Jump {
        kind: IlJumpKind::Unconditional,
        target: lp.header_label,
        loc: cand.loc(),
    };
    ops.insert(insert_at, IlOp::Label(pre));
    ops.insert(insert_at + 1, cand);
    ops.insert(insert_at + 2, jmp);
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::DebugLoc;

    fn loc() -> DebugLoc {
        DebugLoc::unknown()
    }

    #[test]
    fn hoists_const_out_of_while_shaped_loop() {
        // Lh: Const 1; …; JMP Lh  → preheader gets Const.
        let mut ops = vec![
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
            IlOp::Label(Label(0)),
            IlOp::Const { imm: 42, loc: loc() },
            IlOp::Pop { loc: loc() },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
            IlOp::Halt { loc: loc() },
        ];
        licm(&mut ops);
        // Const should appear before the header label (in preheader).
        let header = ops
            .iter()
            .position(|op| matches!(op, IlOp::Label(Label(0))))
            .unwrap();
        assert!(
            ops[..header]
                .iter()
                .any(|op| matches!(op, IlOp::Const { imm: 42, .. })),
            "Const should be hoisted before header"
        );
        assert!(
            !ops[header + 1..]
                .iter()
                .take_while(|op| !matches!(op, IlOp::Jump { .. }))
                .any(|op| matches!(op, IlOp::Const { imm: 42, .. })),
            "Const should leave the loop body"
        );
    }

    #[test]
    fn refuses_when_load_slot_stored_in_loop() {
        let mut ops = vec![
            IlOp::Label(Label(0)),
            IlOp::Load { slot: 1, loc: loc() },
            IlOp::StorePop { slot: 1, loc: loc() },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
        ];
        let before = ops.clone();
        licm(&mut ops);
        assert_eq!(ops.len(), before.len());
    }
}
