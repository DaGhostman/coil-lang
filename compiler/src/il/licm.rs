//! Loop-invariant code motion for Known-SP natural loops.

use std::collections::{HashMap, HashSet};

use common::{Byte, Instruction};

use super::op::{IlJumpKind, IlOp, Label};
use super::sp;

/// Hoist loop-invariant `Const` / `Load` / `BinSlotImm` / `BinSlotSlot` out of
/// natural loops when header SP-in is Known. Also sinks repeated `STRING`/`DATA`
/// field-key literals into preheader temps (GetField/SetField loops).
pub fn licm(ops: &mut Vec<IlOp>) {
    if ops.len() < 4 {
        return;
    }
    if licm_string_keys(ops) {
        return;
    }
    licm_stack_producers(ops);
}

fn licm_stack_producers(ops: &mut Vec<IlOp>) {
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
                IlOp::BinSlotImm { slot, .. } if !stored.contains(&(*slot as u32)) => {
                    hoist.push((i, ops[i].clone()));
                }
                IlOp::BinSlotSlot { a, b, .. }
                    if !stored.contains(&(*a as u32)) && !stored.contains(&(*b as u32)) =>
                {
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
        // Simpler gate: hoist at most a single invariant op per loop when it
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
            IlOp::Const { .. }
                | IlOp::ConstPool { .. }
                | IlOp::Load { .. }
                | IlOp::BinSlotImm { .. }
                | IlOp::BinSlotSlot { .. }
        ) {
            continue;
        }
        apply_hoist(ops, &lp, fi, cand.clone());
        // Indices invalidated; only one hoist per call — rebuild on next pass.
        break;
    }
}

/// Hoist repeated `STRING`/`DATA` key literals out of GetField/SetField loops
/// into preheader temps. Returns true when a transform was applied.
fn licm_string_keys(ops: &mut Vec<IlOp>) -> bool {
    let info = sp::analyze(ops);
    let mut loops = find_natural_loops(ops);
    loops.sort_by_key(|l| std::cmp::Reverse(l.header));
    for lp in &loops {
        if !info.sp_before(lp.header).is_known() {
            continue;
        }
        if loop_has_hard_barrier(ops, lp) {
            continue;
        }
        // Only for field-key shaped loops (GetField / SetField present).
        if !loop_has_field_ops(ops, lp) {
            continue;
        }
        let runs = find_string_runs(ops, lp.body_start(), lp.latch);
        if runs.is_empty() {
            continue;
        }
        // Group identical literals; only rewrite keys that appear ≥2 times.
        let mut by_chars: HashMap<Vec<u32>, Vec<(usize, usize)>> = HashMap::new();
        for (start, end, chars) in &runs {
            by_chars
                .entry(chars.clone())
                .or_default()
                .push((*start, *end));
        }
        let mut rewrite: Vec<(Vec<u32>, Vec<(usize, usize)>)> = by_chars
            .into_iter()
            .filter(|(_, sites)| sites.len() >= 2)
            .collect();
        if rewrite.is_empty() {
            // Still hoist a single-use key if the loop body is otherwise hot —
            // require at least one string run (field access loops).
            rewrite = runs
                .into_iter()
                .map(|(s, e, c)| (c, vec![(s, e)]))
                .collect();
        }
        rewrite.sort_by_key(|(_, sites)| sites[0].0);

        let mut next_slot = max_slot_used(ops).saturating_add(1);
        if next_slot > 250 {
            continue;
        }

        // Materialize unique strings in one preheader (first rewrite only this loop).
        let loc = ops[lp.header].loc();
        let mut materialize: Vec<IlOp> = Vec::new();
        let mut slot_for: HashMap<Vec<u32>, u32> = HashMap::new();
        for (chars, _) in &rewrite {
            if slot_for.contains_key(chars) {
                continue;
            }
            let slot = next_slot;
            next_slot += 1;
            if next_slot > 255 {
                return false;
            }
            slot_for.insert(chars.clone(), slot);
            materialize.push(IlOp::byte(
                Byte::new(Instruction::STRING).with_operand_u32(chars.len() as u32),
            ));
            for &ch in chars {
                materialize.push(IlOp::byte(
                    Byte::new(Instruction::DATA).with_operand_u32(ch),
                ));
            }
            materialize.push(IlOp::StorePop { slot, loc });
        }

        // Replace in-loop runs with LOAD (highest index first so offsets stay valid).
        let mut replacements: Vec<(usize, usize, u32)> = Vec::new();
        for (chars, sites) in &rewrite {
            let slot = slot_for[chars];
            for &(start, end) in sites {
                replacements.push((start, end, slot));
            }
        }
        replacements.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
        for (start, end, slot) in replacements {
            ops.splice(
                start..end,
                std::iter::once(IlOp::Load { slot, loc }),
            );
        }

        // Re-find loop after splice (header label unchanged; indices shifted).
        let Some(lp2) = find_natural_loops(ops)
            .into_iter()
            .find(|l| l.header_label == lp.header_label)
        else {
            return true;
        };
        insert_preheader_ops(ops, &lp2, materialize);
        return true;
    }
    false
}

fn find_string_runs(ops: &[IlOp], start: usize, end: usize) -> Vec<(usize, usize, Vec<u32>)> {
    let mut out = Vec::new();
    let mut i = start;
    while i < end {
        let Some(nchars) = string_op_len(&ops[i]) else {
            i += 1;
            continue;
        };
        let mut chars = Vec::with_capacity(nchars);
        let mut j = i + 1;
        let mut ok = true;
        for _ in 0..nchars {
            if j >= end {
                ok = false;
                break;
            }
            let Some(ch) = data_op_char(&ops[j]) else {
                ok = false;
                break;
            };
            chars.push(ch);
            j += 1;
        }
        if ok && chars.len() == nchars {
            out.push((i, j, chars));
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

fn string_op_len(op: &IlOp) -> Option<usize> {
    match op {
        IlOp::Byte { byte, .. } if *byte.bytecode() == Instruction::STRING => {
            Some(byte.operand_u32() as usize)
        }
        _ => None,
    }
}

fn data_op_char(op: &IlOp) -> Option<u32> {
    match op {
        IlOp::Byte { byte, .. } if *byte.bytecode() == Instruction::DATA => {
            Some(byte.operand_u32())
        }
        _ => None,
    }
}

fn max_slot_used(ops: &[IlOp]) -> u32 {
    let mut max = 0u32;
    for op in ops {
        match op {
            IlOp::Load { slot, .. } | IlOp::StorePop { slot, .. } => {
                max = max.max(*slot);
            }
            IlOp::BinSlotImm { slot, .. } => max = max.max(*slot as u32),
            IlOp::BinSlotSlot { a, b, .. } => {
                max = max.max(*a as u32).max(*b as u32);
            }
            IlOp::Byte { byte, .. }
                if matches!(
                    *byte.bytecode(),
                    Instruction::LOAD | Instruction::STORE | Instruction::StorePop
                ) =>
            {
                let n = byte.load_store_count();
                for i in 0..n {
                    max = max.max(byte.load_store_slot_at(i));
                }
            }
            _ => {}
        }
    }
    max
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

/// Barriers that block string-key LICM. GetField/SetField are allowed — the
/// keys themselves are still loop-invariant.
fn loop_has_hard_barrier(ops: &[IlOp], lp: &NaturalLoop) -> bool {
    for i in lp.header..=lp.latch {
        match &ops[i] {
            IlOp::HostInvoke { .. }
            | IlOp::Print { .. }
            | IlOp::Entry { .. } => return true,
            IlOp::Jump {
                kind: IlJumpKind::JumpIfMatch { .. },
                ..
            } => return true,
            IlOp::Byte { byte, .. }
                if matches!(
                    *byte.bytecode(),
                    common::Instruction::HostInvoke
                        | common::Instruction::PRINT
                        | common::Instruction::CALL
                        | common::Instruction::FORMAT
                        | common::Instruction::FfiInvoke
                        | common::Instruction::MakeDict
                        | common::Instruction::MakeArray
                ) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

fn loop_has_field_ops(ops: &[IlOp], lp: &NaturalLoop) -> bool {
    for i in lp.header..=lp.latch {
        match &ops[i] {
            IlOp::GetField { .. } | IlOp::SetField { .. } => return true,
            IlOp::Byte { byte, .. }
                if matches!(
                    *byte.bytecode(),
                    common::Instruction::GetField | common::Instruction::SetField
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
        match &ops[i] {
            IlOp::StorePop { slot, .. } => {
                s.insert(*slot);
            }
            IlOp::Byte { byte, .. }
                if matches!(
                    *byte.bytecode(),
                    common::Instruction::STORE | common::Instruction::StorePop
                ) =>
            {
                let n = byte.load_store_count();
                for i in 0..n {
                    s.insert(byte.load_store_slot_at(i));
                }
            }
            _ => {}
        }
    }
    s
}

fn apply_hoist(ops: &mut Vec<IlOp>, lp: &NaturalLoop, hoist_idx: usize, cand: IlOp) {
    // Remove hoisted op from body.
    ops.remove(hoist_idx);
    let header_label = lp.header_label;
    // Re-resolve loop after removal.
    let Some(lp2) = find_natural_loops(ops)
        .into_iter()
        .find(|l| l.header_label == header_label)
    else {
        return;
    };
    insert_preheader_ops(ops, &lp2, vec![cand]);
}

fn insert_preheader_ops(ops: &mut Vec<IlOp>, lp: &NaturalLoop, materialize: Vec<IlOp>) {
    if materialize.is_empty() {
        return;
    }
    let pre = Label(
        ops.iter()
            .filter_map(|op| {
                if let IlOp::Label(Label(id)) = op {
                    Some(*id)
                } else {
                    None
                }
            })
            .max()
            .unwrap_or(0)
            .wrapping_add(1),
    );

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

    let loc = materialize[0].loc();
    let insert_at = lp.header;
    let jmp = IlOp::Jump {
        kind: IlJumpKind::Unconditional,
        target: lp.header_label,
        loc,
    };
    ops.insert(insert_at, IlOp::Label(pre));
    let mut at = insert_at + 1;
    for op in materialize {
        ops.insert(at, op);
        at += 1;
    }
    ops.insert(at, jmp);
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

    #[test]
    fn hoists_load_when_slot_not_stored() {
        let mut ops = vec![
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
            IlOp::Label(Label(0)),
            IlOp::Load { slot: 2, loc: loc() },
            IlOp::Pop { loc: loc() },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
            IlOp::Halt { loc: loc() },
        ];
        licm(&mut ops);
        let header = ops
            .iter()
            .position(|op| matches!(op, IlOp::Label(Label(0))))
            .unwrap();
        assert!(
            ops[..header]
                .iter()
                .any(|op| matches!(op, IlOp::Load { slot: 2, .. })),
            "invariant Load should hoist to preheader"
        );
        assert!(
            !ops[header + 1..]
                .iter()
                .take_while(|op| !matches!(op, IlOp::Jump { .. }))
                .any(|op| matches!(op, IlOp::Load { slot: 2, .. })),
            "Load should leave the loop body"
        );
    }

    #[test]
    fn redirects_external_entry_to_preheader() {
        let mut ops = vec![
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
            IlOp::Label(Label(0)),
            IlOp::Const { imm: 7, loc: loc() },
            IlOp::Pop { loc: loc() },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
            IlOp::Halt { loc: loc() },
        ];
        licm(&mut ops);
        // First jump is external entry — must target preheader, not header.
        let IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            target,
            ..
        } = &ops[0]
        else {
            panic!("expected entry jump");
        };
        assert_ne!(*target, Label(0), "external entry must not keep header target");
        let latch = ops
            .iter()
            .rposition(|op| {
                matches!(
                    op,
                    IlOp::Jump {
                        kind: IlJumpKind::Unconditional,
                        target: Label(0),
                        ..
                    }
                )
            })
            .expect("latch back-edge to header");
        assert!(latch > 0);
    }

    #[test]
    fn refuses_when_host_invoke_in_loop() {
        let mut ops = vec![
            IlOp::Label(Label(0)),
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::HostInvoke {
                arity: 0,
                loc: loc(),
            },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
        ];
        let before = ops.clone();
        licm(&mut ops);
        assert!(ops == before, "HostInvoke is an effect barrier");
    }

    #[test]
    fn refuses_when_jump_if_match_in_loop() {
        let mut ops = vec![
            IlOp::Label(Label(0)),
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfMatch { tag: 0, arity: 0 },
                target: Label(1),
                loc: loc(),
            },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
            IlOp::Label(Label(1)),
            IlOp::Halt { loc: loc() },
        ];
        let before = ops.clone();
        licm(&mut ops);
        assert!(ops == before, "JumpIfMatch is a control barrier");
    }

    #[test]
    fn refuses_when_header_sp_unknown() {
        let mut ops = vec![
            IlOp::byte(common::Byte::new(common::Instruction::FfiInvoke)),
            IlOp::Label(Label(0)),
            IlOp::Const { imm: 9, loc: loc() },
            IlOp::Pop { loc: loc() },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
        ];
        let before = ops.clone();
        licm(&mut ops);
        assert!(ops == before, "Unknown header SP must refuse hoist");
    }

    #[test]
    fn hoists_bin_slot_imm_when_slot_not_stored() {
        let mut ops = vec![
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
            IlOp::Label(Label(0)),
            IlOp::BinSlotImm {
                op: common::Instruction::ADD as u8,
                slot: 2,
                imm: 1,
                loc: loc(),
            },
            IlOp::Pop { loc: loc() },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
            IlOp::Halt { loc: loc() },
        ];
        licm(&mut ops);
        let header = ops
            .iter()
            .position(|op| matches!(op, IlOp::Label(Label(0))))
            .unwrap();
        assert!(
            ops[..header].iter().any(|op| matches!(
                op,
                IlOp::BinSlotImm {
                    slot: 2,
                    imm: 1,
                    ..
                }
            )),
            "invariant BinSlotImm should hoist before header"
        );
        assert!(
            !ops[header + 1..]
                .iter()
                .take_while(|op| !matches!(op, IlOp::Jump { .. }))
                .any(|op| matches!(op, IlOp::BinSlotImm { .. })),
            "BinSlotImm should leave the loop body"
        );
    }

    #[test]
    fn refuses_bin_slot_imm_when_slot_stored_in_loop() {
        let mut ops = vec![
            IlOp::Label(Label(0)),
            IlOp::BinSlotImm {
                op: common::Instruction::ADD as u8,
                slot: 1,
                imm: 1,
                loc: loc(),
            },
            IlOp::StorePop { slot: 1, loc: loc() },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
        ];
        let before = ops.clone();
        licm(&mut ops);
        assert_eq!(ops.len(), before.len(), "stored dep must refuse BinSlotImm hoist");
    }

    #[test]
    fn hoists_bin_slot_slot_when_deps_not_stored() {
        let mut ops = vec![
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
            IlOp::Label(Label(0)),
            IlOp::BinSlotSlot {
                op: common::Instruction::ADD as u8,
                a: 1,
                b: 2,
                loc: loc(),
            },
            IlOp::Pop { loc: loc() },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
            IlOp::Halt { loc: loc() },
        ];
        licm(&mut ops);
        let header = ops
            .iter()
            .position(|op| matches!(op, IlOp::Label(Label(0))))
            .unwrap();
        assert!(
            ops[..header].iter().any(|op| matches!(
                op,
                IlOp::BinSlotSlot { a: 1, b: 2, .. }
            )),
            "invariant BinSlotSlot should hoist before header"
        );
    }

    #[test]
    fn hoists_repeated_string_keys_with_get_field() {
        use common::{Byte, Instruction};
        let str_x = |ops: &mut Vec<IlOp>| {
            ops.push(IlOp::byte(
                Byte::new(Instruction::STRING).with_operand_u32(1),
            ));
            ops.push(IlOp::byte(
                Byte::new(Instruction::DATA).with_operand_u32(b'x' as u32),
            ));
        };
        let mut ops = vec![
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
            IlOp::Label(Label(0)),
            IlOp::Load { slot: 0, loc: loc() },
        ];
        str_x(&mut ops);
        ops.push(IlOp::GetField { loc: loc() });
        ops.push(IlOp::Pop { loc: loc() });
        ops.push(IlOp::Load { slot: 0, loc: loc() });
        str_x(&mut ops);
        ops.push(IlOp::GetField { loc: loc() });
        ops.push(IlOp::Pop { loc: loc() });
        ops.push(IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            target: Label(0),
            loc: loc(),
        });
        ops.push(IlOp::Halt { loc: loc() });

        licm(&mut ops);

        let string_ops = ops
            .iter()
            .filter(|op| matches!(op.as_encode_byte(), Some(b) if *b.bytecode() == Instruction::STRING))
            .count();
        assert_eq!(string_ops, 1, "STRING should materialize once in preheader");
        let loads_of_temp = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Load { slot: 1, .. }))
            .count();
        assert!(
            loads_of_temp >= 2,
            "in-loop key uses should LOAD the hoisted temp"
        );
    }
}
