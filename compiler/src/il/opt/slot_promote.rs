//! Local slot promotion (conservative first slice).
//!
//! **Landed**
//! - Straight-line alias forwarding: `LOAD a; STORE b` rewrites later `LOAD` /
//!   `BinSlot*` uses of `b` to `a`. Const/ConstPool clones at LOAD sites are
//!   left to `copy_prop` (re-cloning after LICM breaks call-arg peel packing).
//! - Same-def joins: seed a block's binding map when every predecessor is a
//!   forward edge and all agree on the same binding for a slot.
//! - Loop-invariant aliases: at a header with a back-edge, forward-pred
//!   bindings whose slots (and deps) are not stored in the natural loop may
//!   enter the loop (covers LICM `LOAD temp; STORE local` copies).
//! - Elide unused `<producer>; STORE b` when tell allows **or** a later
//!   straight-line store to a higher-or-equal slot dominates the cursor floor
//!   (labels/jumps/calls refuse).
//! - Uses `il::tell` known-cursor as a gate on LOAD→producer replacement
//!   (same proof surface as `copy_prop`); dead stores are left to
//!   `dead_store_at` except for the alias-elide cleanup above.
//!
//! **Deferred**
//! - Full SSA rename / φ nodes / general loop-carried promotion.
//! - Keeping values on the operand stack across calls or unknown SP.
//! - Store-destination coalescing (`STORE t; …; LOAD t; STORE s`) — live-range
//!   overlap (e.g. mandelbrot `tr`/`zr`) makes this unsafe without richer
//!   liveness.
//! - Address-taken / aggregate / residual `Byte` promotion.

use std::collections::{HashMap, HashSet};

use common::Instruction;

use crate::il::op::{IlJumpKind, IlOp, Label};

/// Binding of a local slot to a virtual value within the promotion region.
#[derive(Clone)]
enum Binding {
    /// Pure producer that may be cloned at a later `LOAD`.
    Producer { op: IlOp, deps: Vec<u32> },
    /// Slot holds the same value as `src` (`LOAD src; STORE dest`).
    Alias { src: u32 },
}

impl Binding {
    fn agrees_with(&self, other: &Binding) -> bool {
        match (self, other) {
            (Binding::Alias { src: a }, Binding::Alias { src: b }) => a == b,
            (Binding::Producer { op: a, .. }, Binding::Producer { op: b, .. }) => {
                producer_key(a) == producer_key(b) && producer_key(a).is_some()
            }
            (Binding::Alias { src }, Binding::Producer { op: IlOp::Load { slot, .. }, .. })
            | (Binding::Producer { op: IlOp::Load { slot, .. }, .. }, Binding::Alias { src }) => {
                src == slot
            }
            _ => false,
        }
    }

    fn depends_on(&self, slot: u32) -> bool {
        match self {
            Binding::Alias { src } => *src == slot,
            Binding::Producer { deps, .. } => deps.contains(&slot),
        }
    }

    fn depends_on_any(&self, slots: &HashSet<u32>) -> bool {
        match self {
            Binding::Alias { src } => slots.contains(src),
            Binding::Producer { deps, .. } => deps.iter().any(|d| slots.contains(d)),
        }
    }
}

fn producer_key(op: &IlOp) -> Option<u64> {
    let b = op.as_encode_byte()?;
    Some(((*b.bytecode() as u64) << 32) | (b.operand_u32() as u64))
}

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

fn shape_sensitive_load(ops: &[IlOp], load_idx: usize) -> bool {
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

/// Effects that kill all live bindings inside a block.
///
/// `Jump` / `Label` are intentionally excluded: block boundaries own control
/// flow, and clearing at a trailing `JMP` would drop the out-map successors need.
fn promote_barrier(op: &IlOp) -> bool {
    matches!(
        op,
        IlOp::Entry { .. }
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
                Instruction::HostInvoke
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

fn invalidate_slot(bindings: &mut HashMap<u32, Binding>, slot: u32) {
    bindings.retain(|bound, binding| *bound != slot && !binding.depends_on(slot));
}

fn resolve_alias(bindings: &HashMap<u32, Binding>, mut slot: u32) -> u32 {
    let mut seen = HashSet::new();
    while seen.insert(slot) {
        match bindings.get(&slot) {
            Some(Binding::Alias { src }) => slot = *src,
            _ => break,
        }
    }
    slot
}

fn meet_bindings(preds: &[&HashMap<u32, Binding>]) -> HashMap<u32, Binding> {
    if preds.is_empty() {
        return HashMap::new();
    }
    let mut out = preds[0].clone();
    for other in &preds[1..] {
        out.retain(|slot, binding| {
            other
                .get(slot)
                .is_some_and(|theirs| binding.agrees_with(theirs))
        });
    }
    // Fail closed: a pred that never bound `slot` means an unknown reaching def.
    out.retain(|slot, _| preds.iter().all(|p| p.contains_key(slot)));
    out
}

#[derive(Clone, Debug)]
struct Block {
    start: usize,
    end: usize,
    succs: Vec<usize>,
}

fn build_blocks(ops: &[IlOp]) -> Vec<Block> {
    if ops.is_empty() {
        return Vec::new();
    }
    let mut leaders: HashSet<usize> = HashSet::new();
    leaders.insert(0);
    let mut label_at: HashMap<u32, usize> = HashMap::new();
    for (i, op) in ops.iter().enumerate() {
        if let IlOp::Label(Label(id)) = op {
            label_at.insert(*id, i);
            leaders.insert(i);
        }
    }
    for (i, op) in ops.iter().enumerate() {
        if let IlOp::Jump { target, .. } = op {
            if let Some(&t) = label_at.get(&target.0) {
                leaders.insert(t);
            }
            if i + 1 < ops.len() {
                leaders.insert(i + 1);
            }
        } else if matches!(
            op,
            IlOp::Return { .. }
                | IlOp::Halt { .. }
                | IlOp::LoadReturnSlot { .. }
                | IlOp::ConstReturnImm { .. }
                | IlOp::BinReturn { .. }
        ) && i + 1 < ops.len()
        {
            leaders.insert(i + 1);
        }
    }
    let mut starts: Vec<usize> = leaders.into_iter().collect();
    starts.sort_unstable();
    let mut blocks: Vec<Block> = Vec::new();
    for (bi, &start) in starts.iter().enumerate() {
        let end = starts.get(bi + 1).copied().unwrap_or(ops.len());
        blocks.push(Block {
            start,
            end,
            succs: Vec::new(),
        });
    }
    let block_at: HashMap<usize, usize> = blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (b.start, i))
        .collect();

    for bi in 0..blocks.len() {
        let end = blocks[bi].end;
        if end == blocks[bi].start {
            continue;
        }
        let last = end - 1;
        match &ops[last] {
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target,
                ..
            } => {
                if let Some(&t) = label_at.get(&target.0)
                    && let Some(&sb) = block_at.get(&t)
                {
                    blocks[bi].succs.push(sb);
                }
            }
            IlOp::Jump { target, .. } => {
                if let Some(&t) = label_at.get(&target.0)
                    && let Some(&sb) = block_at.get(&t)
                {
                    blocks[bi].succs.push(sb);
                }
                if end < ops.len()
                    && let Some(&fb) = block_at.get(&end)
                {
                    blocks[bi].succs.push(fb);
                }
            }
            IlOp::Return { .. }
            | IlOp::Halt { .. }
            | IlOp::LoadReturnSlot { .. }
            | IlOp::ConstReturnImm { .. }
            | IlOp::BinReturn { .. } => {}
            _ => {
                if end < ops.len()
                    && let Some(&fb) = block_at.get(&end)
                {
                    blocks[bi].succs.push(fb);
                }
            }
        }
    }
    blocks
}

fn preds_of(blocks: &[Block]) -> Vec<Vec<usize>> {
    let mut preds = vec![Vec::new(); blocks.len()];
    for (i, b) in blocks.iter().enumerate() {
        for &s in &b.succs {
            preds[s].push(i);
        }
    }
    preds
}

/// Natural-loop block set for `header`: header plus nodes that reach it via
/// back-edge paths (standard back-edge expansion).
fn loop_block_set(header: usize, preds: &[Vec<usize>], blocks: &[Block]) -> HashSet<usize> {
    let mut set = HashSet::from([header]);
    let mut stack: Vec<usize> = preds[header]
        .iter()
        .copied()
        .filter(|&p| blocks[p].start >= blocks[header].start)
        .collect();
    while let Some(b) = stack.pop() {
        if set.insert(b) {
            stack.extend(preds[b].iter().copied());
        }
    }
    set
}

fn slots_stored_in_blocks(ops: &[IlOp], blocks: &[Block], members: &HashSet<usize>) -> HashSet<u32> {
    let mut stored = HashSet::new();
    for &bi in members {
        for i in blocks[bi].start..blocks[bi].end {
            match &ops[i] {
                IlOp::StorePop { slot, .. } => {
                    stored.insert(*slot);
                }
                other => {
                    if let Some(byte) = other.as_encode_byte()
                        && matches!(
                            *byte.bytecode(),
                            Instruction::STORE | Instruction::StorePop
                        )
                    {
                        for k in 0..byte.load_store_count() {
                            stored.insert(byte.load_store_slot_at(k));
                        }
                    }
                }
            }
        }
    }
    stored
}

fn rewrite_slot_uses(op: &mut IlOp, from: u32, to: u32) -> bool {
    match op {
        IlOp::Load { slot, .. } | IlOp::LoadReturnSlot { slot, .. } if *slot == from => {
            *slot = to;
            true
        }
        IlOp::BinSlotImm { slot, .. } if *slot as u32 == from => {
            *slot = to as u8;
            true
        }
        IlOp::BinSlotSlot { a, b, .. } => {
            let mut changed = false;
            if *a as u32 == from {
                *a = to as u8;
                changed = true;
            }
            if *b as u32 == from {
                *b = to as u8;
                changed = true;
            }
            changed
        }
        _ => false,
    }
}

fn transfer_block(
    ops: &mut [IlOp],
    block: &Block,
    mut bindings: HashMap<u32, Binding>,
    cursor: &crate::il::tell::TellInfo,
) -> HashMap<u32, Binding> {
    let mut i = block.start;
    while i < block.end {
        if matches!(ops[i], IlOp::Label(_)) {
            i += 1;
            continue;
        }

        // Rewrite BinSlot* / Load uses of aliased slots before handling defs.
        if let Some(slots) = match &ops[i] {
            IlOp::BinSlotImm { slot, .. } => Some(vec![*slot as u32]),
            IlOp::BinSlotSlot { a, b, .. } => Some(vec![*a as u32, *b as u32]),
            IlOp::Load { slot, .. } | IlOp::LoadReturnSlot { slot, .. } => Some(vec![*slot]),
            _ => None,
        } {
            for slot in slots {
                let resolved = resolve_alias(&bindings, slot);
                if resolved != slot {
                    rewrite_slot_uses(&mut ops[i], slot, resolved);
                }
            }
        }

        if let IlOp::Load { slot, .. } = ops[i]
            && cursor.tell_before(i).known().is_some()
            && !shape_sensitive_load(ops, i)
            && let Some(binding) = bindings.get(&slot).cloned()
        {
            match binding {
                // Keep values in slots: rewrite to the alias source LOAD rather
                // than cloning Const/ConstPool onto the stack. Cloning constants
                // here undoes call-arg peel packing (`LOAD n=3` of temps) and
                // staged Index reloads that copy_prop intentionally leaves when
                // the use is a multi-slot LOAD / residual form.
                Binding::Alias { src } => {
                    let src = resolve_alias(&bindings, src);
                    if src != slot {
                        ops[i] = IlOp::Load {
                            slot: src,
                            loc: ops[i].loc(),
                        };
                    }
                }
                Binding::Producer {
                    op: IlOp::Load { slot: src, .. },
                    ..
                } => {
                    let src = resolve_alias(&bindings, src);
                    if src != slot {
                        ops[i] = IlOp::Load {
                            slot: src,
                            loc: ops[i].loc(),
                        };
                    }
                }
                Binding::Producer {
                    op: producer @ (IlOp::BinSlotImm { .. } | IlOp::BinSlotSlot { .. }),
                    ..
                } => {
                    let mut replacement = producer;
                    replacement.set_loc(ops[i].loc());
                    ops[i] = replacement;
                }
                Binding::Producer { .. } => {
                    // Const / ConstPool / String: leave the LOAD. Straight-line
                    // copy_prop already handles safe cases; re-cloning here after
                    // LICM breaks peel/staging shapes.
                }
            }
        }

        if i + 1 < block.end
            && let IlOp::StorePop { slot, .. } = &ops[i + 1]
            && let Some(dependencies) = copy_producer_dependencies(&ops[i])
            && !dependencies.contains(slot)
        {
            let dest = *slot;
            invalidate_slot(&mut bindings, dest);
            let binding = if let IlOp::Load { slot: src, .. } = &ops[i] {
                Binding::Alias { src: *src }
            } else {
                Binding::Producer {
                    op: ops[i].clone(),
                    deps: dependencies,
                }
            };
            bindings.insert(dest, binding);
            i += 2;
            continue;
        }

        match &ops[i] {
            IlOp::StorePop { slot, .. } => invalidate_slot(&mut bindings, *slot),
            op if promote_barrier(op) => bindings.clear(),
            _ => {}
        }
        i += 1;
    }
    bindings
}

/// Promote local slots to virtual values within a function body.
///
/// `entry_tell` seeds the cursor model; unknown tell refuses LOAD→producer
/// replacement but still allows alias operand rewriting when bindings exist.
pub(super) fn slot_promote(ops: &mut Vec<IlOp>, entry_tell: u32) {
    if ops.len() < 2 {
        return;
    }
    let blocks = build_blocks(ops);
    if blocks.is_empty() {
        return;
    }
    let preds = preds_of(&blocks);
    let cursor = crate::il::tell::analyze_il_at(ops, entry_tell);

    let mut out_bindings: Vec<HashMap<u32, Binding>> = vec![HashMap::new(); blocks.len()];

    for bi in 0..blocks.len() {
        let back_preds: Vec<usize> = preds[bi]
            .iter()
            .copied()
            .filter(|&p| blocks[p].start >= blocks[bi].start)
            .collect();
        let forward_preds: Vec<usize> = preds[bi]
            .iter()
            .copied()
            .filter(|&p| blocks[p].start < blocks[bi].start)
            .collect();

        let in_map = if preds[bi].is_empty() {
            HashMap::new()
        } else if back_preds.is_empty() {
            let pred_maps: Vec<&HashMap<u32, Binding>> =
                forward_preds.iter().map(|&p| &out_bindings[p]).collect();
            meet_bindings(&pred_maps)
        } else if forward_preds.is_empty() {
            // Only back-edges (e.g. tight header): fail closed.
            HashMap::new()
        } else {
            // Loop header: carry forward-pred bindings that the loop does not
            // redefine (invariant aliases / producers). Ignore back-edge outs.
            let pred_maps: Vec<&HashMap<u32, Binding>> =
                forward_preds.iter().map(|&p| &out_bindings[p]).collect();
            let mut map = meet_bindings(&pred_maps);
            let members = loop_block_set(bi, &preds, &blocks);
            let stored = slots_stored_in_blocks(ops, &blocks, &members);
            map.retain(|slot, binding| {
                !stored.contains(slot) && !binding.depends_on_any(&stored)
            });
            map
        };
        out_bindings[bi] = transfer_block(ops, &blocks[bi], in_map, &cursor);
    }

    // dead_store_at treats labels/jumps as barriers (loop-carried caution), so
    // elide `LOAD a; STORE b` here when `b` has no remaining uses and tell allows.
    elide_unused_alias_stores(ops, entry_tell);
}

fn slot_used_anywhere(ops: &[IlOp], slot: u32) -> bool {
    for op in ops {
        match op {
            IlOp::Load { slot: s, .. } | IlOp::LoadReturnSlot { slot: s, .. } => {
                if *s == slot {
                    return true;
                }
            }
            IlOp::BinSlotImm { slot: s, .. } => {
                if *s as u32 == slot {
                    return true;
                }
            }
            IlOp::BinSlotSlot { a, b, .. } => {
                if *a as u32 == slot || *b as u32 == slot {
                    return true;
                }
            }
            IlOp::StorePop { .. } => {}
            other => {
                if let Some(byte) = other.as_encode_byte() {
                    let insn = *byte.bytecode();
                    // Residual fused / packed forms: fail closed.
                    if matches!(
                        insn,
                        Instruction::BinSlotImm
                            | Instruction::BinSlotSlot
                            | Instruction::BinSlotImmStore
                            | Instruction::BinSlotSlotStore
                            | Instruction::BinSlotImmJmpf
                            | Instruction::BinSlotSlotJmpf
                            | Instruction::BinSlotSlotConstJmpf
                            | Instruction::FloatChainStore
                    ) {
                        return true;
                    }
                    if matches!(
                        insn,
                        Instruction::LOAD
                            | Instruction::STORE
                            | Instruction::StorePop
                            | Instruction::LoadReturnSlot
                    ) {
                        for k in 0..byte.load_store_count() {
                            if byte.load_store_slot_at(k) == slot {
                                return true;
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

/// True when a later straight-line `STORE` to `slot >= dest` makes this store's
/// cursor floor redundant, with no control/effect barrier in between.
fn later_store_dominates_floor(ops: &[IlOp], store_idx: usize, dest: u32) -> bool {
    for op in ops.iter().skip(store_idx + 1) {
        match op {
            IlOp::StorePop { slot, .. } if *slot >= dest => return true,
            IlOp::Label(_)
            | IlOp::Jump { .. }
            | IlOp::Entry { .. }
            | IlOp::HostInvoke { .. }
            | IlOp::Print { .. }
            | IlOp::Return { .. }
            | IlOp::Halt { .. }
            | IlOp::Byte { .. } => return false,
            other if promote_barrier(other) => return false,
            _ => {}
        }
    }
    false
}

/// Drop `LOAD a; STORE b` when `b` is unused afterward and either the cursor
/// proof or a dominating later store shows the floor is redundant.
///
/// Only alias copies are eligible — `CONST; STORE` materializations for `let`
/// bindings must remain even when a later use was producer-forwarded.
fn elide_unused_alias_stores(ops: &mut Vec<IlOp>, entry_tell: u32) {
    let cursor = crate::il::tell::analyze_il_at(ops, entry_tell);
    let mut remove: HashSet<usize> = HashSet::new();
    let mut i = 0;
    while i + 1 < ops.len() {
        if remove.contains(&i) {
            i += 1;
            continue;
        }
        if let (IlOp::Load { .. }, IlOp::StorePop { slot: dest, .. }) = (&ops[i], &ops[i + 1]) {
            let dest = *dest;
            let mut rest: Vec<IlOp> = Vec::with_capacity(ops.len() - 2);
            for (idx, op) in ops.iter().enumerate() {
                if idx == i || idx == i + 1 || remove.contains(&idx) {
                    continue;
                }
                rest.push(op.clone());
            }
            let floor_ok = cursor.can_remove_one_value_store(i, dest)
                || later_store_dominates_floor(ops, i + 1, dest);
            if !slot_used_anywhere(&rest, dest) && floor_ok {
                remove.insert(i);
                remove.insert(i + 1);
                i += 2;
                continue;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::il::op::IlJumpKind;
    use common::DebugLoc;

    fn loc() -> DebugLoc {
        DebugLoc::unknown()
    }

    #[test]
    fn forwards_alias_load_through_store_load() {
        // LOAD src; STORE t; LOAD t → LOAD src (Const clones stay with copy_prop).
        let mut ops = vec![
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::StorePop {
                slot: 1,
                loc: loc(),
            },
            IlOp::Load {
                slot: 1,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        slot_promote(&mut ops, 3);
        assert!(
            ops.iter().any(|op| matches!(op, IlOp::Load { slot: 0, .. })),
            "use should read alias source slot 0"
        );
        assert!(
            !ops.iter().any(|op| matches!(op, IlOp::Load { slot: 1, .. })),
            "LOAD of dest slot should be rewritten"
        );
    }

    #[test]
    fn rewrites_bin_slot_through_alias() {
        let mut ops = vec![
            IlOp::Load {
                slot: 5,
                loc: loc(),
            },
            IlOp::StorePop {
                slot: 6,
                loc: loc(),
            },
            IlOp::BinSlotImm {
                op: Instruction::ADD as u8,
                slot: 6,
                imm: 1,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        slot_promote(&mut ops, 7);
        assert!(
            ops.iter().any(|op| matches!(op, IlOp::BinSlotImm { slot: 5, imm: 1, .. })),
            "BinSlotImm should read the alias source"
        );
        assert!(
            !ops.iter()
                .any(|op| matches!(op, IlOp::StorePop { slot: 6, .. })),
            "unused alias store should elide"
        );
    }

    #[test]
    fn same_def_join_forwards_alias_across_diamond() {
        // Both preds leave slot 1 as Alias(0).
        let mut ops = vec![
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::StorePop {
                slot: 1,
                loc: loc(),
            },
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(1),
                loc: loc(),
            },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(2),
                loc: loc(),
            },
            IlOp::Label(Label(1)),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(2),
                loc: loc(),
            },
            IlOp::Label(Label(2)),
            IlOp::Load {
                slot: 1,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        slot_promote(&mut ops, 3);
        assert!(
            ops.iter().any(|op| matches!(op, IlOp::Load { slot: 0, .. })),
            "join LOAD should read the agreed alias source"
        );
        assert!(
            !ops.iter().any(|op| matches!(op, IlOp::Load { slot: 1, .. })),
            "join should not leave LOAD of dest slot 1"
        );
    }

    #[test]
    fn refuses_loop_carried_promotion() {
        // Header join has a back-edge; even if the forward edge stores CONST 1,
        // the latch may redefine the slot — fail closed.
        let mut ops = vec![
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::StorePop {
                slot: 1,
                loc: loc(),
            },
            IlOp::Label(Label(0)),
            IlOp::Load {
                slot: 1,
                loc: loc(),
            },
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: loc(),
            },
            IlOp::StorePop {
                slot: 1,
                loc: loc(),
            },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
        ];
        slot_promote(&mut ops, 3);
        assert!(
            matches!(ops[3], IlOp::Load { slot: 1, .. }),
            "loop header must keep LOAD"
        );
    }

    #[test]
    fn disagreeing_join_preds_keep_load() {
        let mut ops = vec![
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(1),
                loc: loc(),
            },
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::StorePop {
                slot: 1,
                loc: loc(),
            },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(2),
                loc: loc(),
            },
            IlOp::Label(Label(1)),
            IlOp::Const { imm: 2, loc: loc() },
            IlOp::StorePop {
                slot: 1,
                loc: loc(),
            },
            IlOp::Label(Label(2)),
            IlOp::Load {
                slot: 1,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        slot_promote(&mut ops, 3);
        assert!(matches!(ops[9], IlOp::Load { slot: 1, .. }));
    }

    #[test]
    fn invariant_alias_enters_loop_when_slots_not_stored() {
        // LOAD 5; STORE 6; then a loop that only reads 6 via BinSlotImm.
        let mut ops = vec![
            IlOp::Load {
                slot: 5,
                loc: loc(),
            },
            IlOp::StorePop {
                slot: 6,
                loc: loc(),
            },
            IlOp::Label(Label(0)),
            IlOp::BinSlotImm {
                op: Instruction::ADD as u8,
                slot: 6,
                imm: 1,
                loc: loc(),
            },
            IlOp::Pop { loc: loc() },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
        ];
        slot_promote(&mut ops, 7);
        assert!(
            ops.iter()
                .any(|op| matches!(op, IlOp::BinSlotImm { slot: 5, imm: 1, .. })),
            "loop body should read alias source slot 5"
        );
        assert!(
            !ops.iter()
                .any(|op| matches!(op, IlOp::StorePop { slot: 6, .. })),
            "unused alias store should elide across the loop"
        );
    }

    #[test]
    fn elides_unused_alias_store_when_tell_allows() {
        // Pure alias copy: LOAD 5; STORE 6 with uses only of slot 5 afterward.
        let mut ops = vec![
            IlOp::Load {
                slot: 5,
                loc: loc(),
            },
            IlOp::StorePop {
                slot: 6,
                loc: loc(),
            },
            IlOp::BinSlotImm {
                op: Instruction::ADD as u8,
                slot: 5,
                imm: 1,
                loc: loc(),
            },
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::StorePop {
                slot: 7,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        slot_promote(&mut ops, 8);
        assert!(
            !ops.iter()
                .any(|op| matches!(op, IlOp::StorePop { slot: 6, .. })),
            "unused alias store to 6 should elide (dominated by STORE 7)"
        );
    }

    #[test]
    fn clears_bindings_across_call() {
        let mut ops = vec![
            IlOp::Const { imm: 7, loc: loc() },
            IlOp::StorePop {
                slot: 1,
                loc: loc(),
            },
            IlOp::Entry {
                kind: crate::il::op::EntryKind::Call,
                arity: 0,
                target: Label(0),
                loc: loc(),
            },
            IlOp::Load {
                slot: 1,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        slot_promote(&mut ops, 3);
        assert!(matches!(ops[3], IlOp::Load { slot: 1, .. }));
    }
}
