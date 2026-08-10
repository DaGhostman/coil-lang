//! Loop range / bounds analysis (proof-only + ArrayLen LICM).
//!
//! Identifies counted loops (`0..n` / `0..len`) with an invariant array, proves
//! `0 <= i < len` for in-body `Index` / `StoreIndex` when the bound is the
//! array's length (or a fill-loop-equal `n`), and hoists invariant
//! `LOAD; ArrayLen; STORE` triples into the preheader. Fail-closed: unknown or
//! mutation-sensitive paths keep checked ops. No new opcodes.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use common::Instruction;
#[cfg(test)]
use common::Byte;

use super::op::{IlJumpKind, IlOp, Label};
use super::sp;

thread_local! {
    static LAST_STATS: RefCell<BoundsStats> = const { RefCell::new(BoundsStats::new()) };
}

/// Counters from the most recent [`loop_bounds`] run on this thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoundsStats {
    /// `LOAD; ArrayLen; STORE` triples moved to a preheader.
    pub array_len_hoists: u32,
    /// `Index` sites proven in-bounds for the iteration.
    pub proven_index: u32,
    /// `Index` sites left checked (fail-closed).
    pub checked_index: u32,
    /// `StoreIndex` sites proven in-bounds.
    pub proven_store_index: u32,
    /// `StoreIndex` sites left checked.
    pub checked_store_index: u32,
}

impl BoundsStats {
    const fn new() -> Self {
        Self {
            array_len_hoists: 0,
            proven_index: 0,
            checked_index: 0,
            proven_store_index: 0,
            checked_store_index: 0,
        }
    }
}

/// Stats from the last compile's accumulated [`loop_bounds`] runs on this thread.
pub fn last_bounds_stats() -> BoundsStats {
    LAST_STATS.with(|c| *c.borrow())
}

/// Clear accumulated bounds counters (call at compile start).
pub fn reset_bounds_stats() {
    LAST_STATS.with(|c| *c.borrow_mut() = BoundsStats::new());
}

/// Hoist invariant ArrayLen materializations and record index proofs.
pub fn loop_bounds(ops: &mut Vec<IlOp>) {
    let mut stats = BoundsStats::new();
    if ops.len() < 4 {
        return;
    }

    // Hoist one triple per call; iterate like cast LICM for nested loops.
    for _ in 0..find_natural_loops(ops).len().saturating_add(1) {
        if !hoist_array_len(ops, &mut stats) {
            break;
        }
    }

    analyze_index_proofs(ops, &mut stats);
    LAST_STATS.with(|c| {
        let mut acc = c.borrow_mut();
        acc.array_len_hoists = acc.array_len_hoists.saturating_add(stats.array_len_hoists);
        acc.proven_index = acc.proven_index.saturating_add(stats.proven_index);
        acc.checked_index = acc.checked_index.saturating_add(stats.checked_index);
        acc.proven_store_index = acc
            .proven_store_index
            .saturating_add(stats.proven_store_index);
        acc.checked_store_index = acc
            .checked_store_index
            .saturating_add(stats.checked_store_index);
    });
}

#[derive(Clone, Debug)]
struct NaturalLoop {
    header: usize,
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

fn is_array_len(op: &IlOp) -> bool {
    match op {
        IlOp::Byte { byte, .. } => *byte.bytecode() == Instruction::ArrayLen,
        other => other
            .as_encode_byte()
            .is_some_and(|b| *b.bytecode() == Instruction::ArrayLen),
    }
}

fn is_store_index(op: &IlOp) -> bool {
    match op {
        IlOp::Byte { byte, .. } => *byte.bytecode() == Instruction::StoreIndex,
        other => other
            .as_encode_byte()
            .is_some_and(|b| *b.bytecode() == Instruction::StoreIndex),
    }
}

fn is_array_push(op: &IlOp) -> bool {
    match op {
        IlOp::Byte { byte, .. } => *byte.bytecode() == Instruction::ArrayPush,
        other => other
            .as_encode_byte()
            .is_some_and(|b| *b.bytecode() == Instruction::ArrayPush),
    }
}

fn is_le_cmp(op: &IlOp) -> bool {
    matches!(op, IlOp::Bin { op: Instruction::LE, .. })
        || op
            .as_encode_byte()
            .is_some_and(|b| *b.bytecode() == Instruction::LE)
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
                    Instruction::STORE | Instruction::StorePop
                ) =>
            {
                for k in 0..byte.load_store_count() {
                    s.insert(byte.load_store_slot_at(k));
                }
            }
            _ => {}
        }
    }
    s
}

fn store_count_in_loop(ops: &[IlOp], lp: &NaturalLoop, slot: u32) -> usize {
    let mut n = 0;
    for i in lp.header..=lp.latch {
        match &ops[i] {
            IlOp::StorePop { slot: s, .. } if *s == slot => n += 1,
            IlOp::Byte { byte, .. }
                if matches!(
                    *byte.bytecode(),
                    Instruction::STORE | Instruction::StorePop
                ) =>
            {
                for k in 0..byte.load_store_count() {
                    if byte.load_store_slot_at(k) == slot {
                        n += 1;
                    }
                }
            }
            _ => {}
        }
    }
    n
}

/// True when the loop may change `arr_slot`'s length or rebind the slot.
fn array_length_sensitive(ops: &[IlOp], lp: &NaturalLoop, arr_slot: u32) -> bool {
    let stored = slots_stored_in_loop(ops, lp);
    if stored.contains(&arr_slot) {
        return true;
    }
    for i in lp.header..=lp.latch {
        let op = &ops[i];
        if is_array_push(op) {
            // Conservative: any push may extend an array reachable here.
            return true;
        }
        match op {
            IlOp::HostInvoke { .. }
            | IlOp::MakeArray { .. }
            | IlOp::Entry { .. }
            | IlOp::Print { .. } => return true,
            IlOp::Byte { byte, .. }
                if matches!(
                    *byte.bytecode(),
                    Instruction::HostInvoke
                        | Instruction::CALL
                        | Instruction::MakeArray
                        | Instruction::PRINT
                        | Instruction::FfiInvoke
                        | Instruction::FORMAT
                ) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

fn loop_has_hard_barrier(ops: &[IlOp], lp: &NaturalLoop) -> bool {
    for i in lp.header..=lp.latch {
        match &ops[i] {
            IlOp::HostInvoke { .. } | IlOp::Print { .. } | IlOp::Entry { .. } => return true,
            IlOp::Jump {
                kind: IlJumpKind::JumpIfMatch { .. },
                ..
            } => return true,
            IlOp::Byte { byte, .. }
                if matches!(
                    *byte.bytecode(),
                    Instruction::HostInvoke
                        | Instruction::PRINT
                        | Instruction::CALL
                        | Instruction::FORMAT
                        | Instruction::FfiInvoke
                ) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Hoist one `LOAD arr; ArrayLen; STORE len` when `arr` is length-invariant.
fn hoist_array_len(ops: &mut Vec<IlOp>, stats: &mut BoundsStats) -> bool {
    let info = sp::analyze(ops);
    let mut loops = find_natural_loops(ops);
    loops.sort_by_key(|l| std::cmp::Reverse(l.header));
    for lp in &loops {
        if !info.sp_before(lp.header).is_known() {
            continue;
        }
        // ArrayLen hoist allows StoreIndex / Index; refuse host/call barriers.
        if loop_has_hard_barrier(ops, lp) {
            continue;
        }
        let mut found: Option<(usize, u32, u32)> = None;
        let mut i = lp.body_start();
        while i + 2 < lp.latch {
            if let IlOp::Load { slot: arr, .. } = &ops[i]
                && is_array_len(&ops[i + 1])
                && let IlOp::StorePop { slot: len, .. } = &ops[i + 2]
                && store_count_in_loop(ops, lp, *len) == 1
                && !array_length_sensitive(ops, lp, *arr)
            {
                found = Some((i, *arr, *len));
                break;
            }
            i += 1;
        }
        let Some((idx, _arr, _len)) = found else {
            continue;
        };
        let triple: Vec<IlOp> = ops[idx..idx + 3].to_vec();
        ops.drain(idx..idx + 3);
        let header_label = lp.header_label;
        let Some(lp2) = find_natural_loops(ops)
            .into_iter()
            .find(|l| l.header_label == header_label)
        else {
            return false;
        };
        insert_preheader_ops(ops, &lp2, triple);
        stats.array_len_hoists += 1;
        return true;
    }
    false
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

#[derive(Clone, Debug)]
struct CountedLoop {
    lp: NaturalLoop,
    index_slot: u32,
    #[allow(dead_code)]
    bound_slot: u32,
    /// Array slots whose length equals `bound_slot` for this loop.
    len_arrays: HashSet<u32>,
}

/// Dominating `LOAD arr; ArrayLen; STORE bound` (possibly in a preheader).
fn array_len_defs(ops: &[IlOp]) -> HashMap<u32, u32> {
    // bound_slot -> arr_slot
    let mut map = HashMap::new();
    let mut i = 0;
    while i + 2 < ops.len() {
        if let IlOp::Load { slot: arr, .. } = &ops[i]
            && is_array_len(&ops[i + 1])
            && let IlOp::StorePop { slot: len, .. } = &ops[i + 2]
        {
            map.insert(*len, *arr);
            i += 3;
            continue;
        }
        i += 1;
    }
    map
}

/// Detect `idx < bound` header exits and non-negative +1 index updates.
fn detect_counted_loops(ops: &[IlOp], len_of: &HashMap<u32, u32>) -> Vec<CountedLoop> {
    let mut out = Vec::new();
    for lp in find_natural_loops(ops) {
        let Some((cmp_slot, bound_slot)) = header_lt_bound(ops, &lp) else {
            continue;
        };
        let Some(index_slot) = resolve_index_slot(ops, &lp, cmp_slot) else {
            continue;
        };
        if !index_init_nonneg(ops, lp.header, index_slot) {
            continue;
        }
        let stored = slots_stored_in_loop(ops, &lp);
        if stored.contains(&bound_slot) {
            continue;
        }
        let mut len_arrays = HashSet::new();
        if let Some(&arr) = len_of.get(&bound_slot)
            && !array_length_sensitive(ops, &lp, arr)
        {
            len_arrays.insert(arr);
        }
        // Fill-loop equality: bound equals length of arrays filled `0..bound`.
        for arr in fill_equal_arrays(ops, lp.header, bound_slot) {
            if !array_length_sensitive(ops, &lp, arr) {
                len_arrays.insert(arr);
            }
        }
        out.push(CountedLoop {
            lp,
            index_slot,
            bound_slot,
            len_arrays,
        });
    }
    out
}

/// Map a compare operand to the loop index: either the counted slot itself, or
/// a per-iteration snapshot (`LOAD idx; STORE tmp`) used only in the header.
fn resolve_index_slot(ops: &[IlOp], lp: &NaturalLoop, cmp_slot: u32) -> Option<u32> {
    if index_is_counted(ops, lp, cmp_slot) {
        return Some(cmp_slot);
    }
    // Snapshot of a counted index: every store to cmp is `LOAD idx; STORE cmp`,
    // and idx is the +1-counted induction variable.
    let mut src: Option<u32> = None;
    let mut i = lp.body_start();
    while i < lp.latch {
        if let IlOp::StorePop { slot, .. } = &ops[i]
            && *slot == cmp_slot
        {
            if i > 0
                && let IlOp::Load { slot: from, .. } = &ops[i - 1]
            {
                match src {
                    None => src = Some(*from),
                    Some(s) if s != *from => return None,
                    _ => {}
                }
            } else {
                return None;
            }
        }
        i += 1;
    }
    let idx = src?;
    if idx == cmp_slot {
        return None;
    }
    if index_is_counted(ops, lp, idx) {
        Some(idx)
    } else {
        None
    }
}

fn header_lt_bound(ops: &[IlOp], lp: &NaturalLoop) -> Option<(u32, u32)> {
    // Scan from body_start for the first LE + JMPF exit pattern.
    let mut i = lp.body_start();
    while i + 1 < lp.latch {
        // LOAD a; LOAD b; LE; JMPF
        if let IlOp::Load { slot: a, .. } = &ops[i]
            && i + 3 < lp.latch
            && let IlOp::Load { slot: b, .. } = &ops[i + 1]
            && is_le_cmp(&ops[i + 2])
            && matches!(
                &ops[i + 3],
                IlOp::Jump {
                    kind: IlJumpKind::JumpIfFalse,
                    ..
                }
            )
        {
            return Some((*a, *b));
        }
        // BinSlotSlot LE a,b ; JMPF
        if let IlOp::BinSlotSlot { op, a, b, .. } = &ops[i]
            && *op == Instruction::LE as u8
            && i + 1 < lp.latch
            && matches!(
                &ops[i + 1],
                IlOp::Jump {
                    kind: IlJumpKind::JumpIfFalse,
                    ..
                }
            )
        {
            return Some((*a as u32, *b as u32));
        }
        i += 1;
    }
    None
}

fn index_is_counted(ops: &[IlOp], lp: &NaturalLoop, index_slot: u32) -> bool {
    // Every store to index_slot must be `index + positive_const`.
    let mut saw_inc = false;
    let mut i = lp.body_start();
    while i < lp.latch {
        if let IlOp::StorePop { slot, .. } = &ops[i]
            && *slot == index_slot
        {
            // Look back for LOAD index; CONST k>0; ADD  or BinSlotImm ADD.
            if i >= 3
                && matches!(&ops[i - 1], IlOp::Bin { op: Instruction::ADD, .. })
                && matches!(&ops[i - 2], IlOp::Const { imm, .. } if *imm > 0)
                && matches!(&ops[i - 3], IlOp::Load { slot: s, .. } if *s == index_slot)
            {
                saw_inc = true;
                i += 1;
                continue;
            }
            if i >= 1
                && let IlOp::BinSlotImm {
                    op,
                    slot: src,
                    imm,
                    ..
                } = &ops[i - 1]
                && *op == Instruction::ADD as u8
                && *src as u32 == index_slot
                && *imm > 0
            {
                saw_inc = true;
                i += 1;
                continue;
            }
            // Unknown store to index — fail closed.
            return false;
        }
        i += 1;
    }
    saw_inc
}

fn index_init_nonneg(ops: &[IlOp], header: usize, index_slot: u32) -> bool {
    // Walk backwards for the last store to index_slot before the header.
    for i in (0..header).rev() {
        match &ops[i] {
            IlOp::StorePop { slot, .. } if *slot == index_slot => {
                if i > 0
                    && let IlOp::Const { imm, .. } = &ops[i - 1]
                {
                    return *imm >= 0;
                }
                // LOAD other; STORE index — unknown.
                return false;
            }
            IlOp::Byte { byte, .. }
                if matches!(
                    *byte.bytecode(),
                    Instruction::STORE | Instruction::StorePop
                ) =>
            {
                for k in 0..byte.load_store_count() {
                    if byte.load_store_slot_at(k) == index_slot {
                        return false;
                    }
                }
            }
            _ => {}
        }
    }
    // Param / unset — fail closed (could be negative).
    false
}

/// Arrays filled by a dominating `while i < n { arr.push(...); i++ }` from empty.
fn fill_equal_arrays(ops: &[IlOp], before: usize, bound_slot: u32) -> HashSet<u32> {
    let mut out = HashSet::new();
    for lp in find_natural_loops(ops) {
        if lp.latch >= before {
            continue;
        }
        let Some((cmp_slot, bnd)) = header_lt_bound(ops, &lp) else {
            continue;
        };
        if bnd != bound_slot {
            continue;
        }
        let Some(idx) = resolve_index_slot(ops, &lp, cmp_slot) else {
            continue;
        };
        if !index_init_nonneg(ops, lp.header, idx) {
            continue;
        }
        // Body must ArrayPush and not rebind a unique array slot.
        let mut push_arr: Option<u32> = None;
        let mut ok = true;
        let mut j = lp.body_start();
        while j < lp.latch {
            if is_array_push(&ops[j]) {
                // Look back for LOAD arr before the push (value then array on stack
                // for ArrayPush: codegen loads array then value → pop value, pop arr).
                // Stack: ... arr, value ; ArrayPush. Find LOAD arr near push.
                let mut arr_slot = None;
                for k in (lp.body_start()..j).rev() {
                    if let IlOp::Load { slot, .. } = &ops[k] {
                        // Prefer the load that isn't the pushed value's producer
                        // of a const — take the first Load of a slot that isn't idx.
                        if *slot != idx && *slot != bound_slot {
                            arr_slot = Some(*slot);
                            break;
                        }
                    }
                }
                let Some(a) = arr_slot else {
                    ok = false;
                    break;
                };
                match push_arr {
                    None => push_arr = Some(a),
                    Some(prev) if prev != a => {
                        ok = false;
                        break;
                    }
                    _ => {}
                }
            }
            j += 1;
        }
        if !ok {
            continue;
        }
        let Some(arr) = push_arr else {
            continue;
        };
        // Array slot must not be rebound in the fill loop; length grows via push only.
        let stored = slots_stored_in_loop(ops, &lp);
        if stored.contains(&arr) {
            continue;
        }
        // Require empty-looking start: last def of arr before fill is CALL/MakeArray/
        // with_capacity style — fail closed unless we see MakeArray 0 or a CALL store.
        if !array_starts_empty(ops, lp.header, arr) {
            continue;
        }
        out.insert(arr);
    }
    out
}

fn array_starts_empty(ops: &[IlOp], header: usize, arr_slot: u32) -> bool {
    for i in (0..header).rev() {
        match &ops[i] {
            IlOp::StorePop { slot, .. } if *slot == arr_slot => {
                // MakeArray arity 0 immediately before, or CALL (with_capacity / new).
                if i > 0 {
                    if let IlOp::MakeArray { arity: 0, .. } = &ops[i - 1] {
                        return true;
                    }
                    if matches!(&ops[i - 1], IlOp::Entry { .. }) {
                        return true;
                    }
                    if let IlOp::Byte { byte, .. } = &ops[i - 1]
                        && (*byte.bytecode() == Instruction::CALL
                            || (*byte.bytecode() == Instruction::MakeArray
                                && byte.operand_u32() == 0))
                    {
                        return true;
                    }
                }
                return false;
            }
            _ => {}
        }
    }
    false
}

fn analyze_index_proofs(ops: &[IlOp], stats: &mut BoundsStats) {
    let len_of = array_len_defs(ops);
    let counted = detect_counted_loops(ops, &len_of);
    if counted.is_empty() {
        // Still count all Index/StoreIndex as checked when no counted loops.
        for op in ops {
            if matches!(op, IlOp::Index { .. }) {
                stats.checked_index += 1;
            } else if is_store_index(op) {
                stats.checked_store_index += 1;
            }
        }
        return;
    }

    // Simulate stack slots as Option: Some(slot) means value is a LOAD of that slot.
    for (i, op) in ops.iter().enumerate() {
        let in_loop = counted.iter().find(|c| i >= c.lp.header && i <= c.lp.latch);
        if matches!(op, IlOp::Index { .. }) {
            if let Some(cl) = in_loop
                && index_at_proven(ops, i, cl)
            {
                stats.proven_index += 1;
            } else {
                stats.checked_index += 1;
            }
        } else if is_store_index(op) {
            if let Some(cl) = in_loop
                && store_index_at_proven(ops, i, cl)
            {
                stats.proven_store_index += 1;
            } else {
                stats.checked_store_index += 1;
            }
        }
    }
}

fn index_at_proven(ops: &[IlOp], index_op: usize, cl: &CountedLoop) -> bool {
    // Expect … LOAD arr; LOAD idx; Index  (idx may be loop index).
    if index_op < 2 {
        return false;
    }
    let IlOp::Load { slot: idx, .. } = &ops[index_op - 1] else {
        return false;
    };
    let IlOp::Load { slot: arr, .. } = &ops[index_op - 2] else {
        return false;
    };
    *idx == cl.index_slot && cl.len_arrays.contains(arr)
}

fn store_index_at_proven(ops: &[IlOp], store_op: usize, cl: &CountedLoop) -> bool {
    // … LOAD arr; LOAD idx; <value>; StoreIndex
    if store_op < 3 {
        return false;
    }
    // Value producer is store_op-1; idx at -2; arr at -3.
    let IlOp::Load { slot: idx, .. } = &ops[store_op - 2] else {
        return false;
    };
    let IlOp::Load { slot: arr, .. } = &ops[store_op - 3] else {
        return false;
    };
    *idx == cl.index_slot && cl.len_arrays.contains(arr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::DebugLoc;

    fn loc() -> DebugLoc {
        DebugLoc::unknown()
    }

    fn array_len_op() -> IlOp {
        IlOp::Byte {
            byte: Byte::new(Instruction::ArrayLen),
            loc: loc(),
        }
    }

    #[test]
    fn hoists_array_len_out_of_counted_loop() {
        reset_bounds_stats();
        // Pre: i=0. Loop: LOAD i; STORE t; LOAD arr; ArrayLen; STORE len;
        // LOAD t; LOAD len; LE; JMPF exit; LOAD arr; LOAD i; Index; POP;
        // i++; JMP header.
        let mut ops = vec![
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::StorePop {
                slot: 1,
                loc: loc(),
            },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
            IlOp::Label(Label(0)),
            IlOp::Load {
                slot: 1,
                loc: loc(),
            },
            IlOp::StorePop {
                slot: 3,
                loc: loc(),
            },
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            array_len_op(),
            IlOp::StorePop {
                slot: 4,
                loc: loc(),
            },
            IlOp::Load {
                slot: 3,
                loc: loc(),
            },
            IlOp::Load {
                slot: 4,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::LE,
                loc: loc(),
            },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(1),
                loc: loc(),
            },
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::Load {
                slot: 1,
                loc: loc(),
            },
            IlOp::Index { loc: loc() },
            IlOp::Pop { loc: loc() },
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
            IlOp::Label(Label(1)),
            IlOp::Halt { loc: loc() },
        ];
        loop_bounds(&mut ops);
        let stats = last_bounds_stats();
        assert!(
            stats.array_len_hoists >= 1,
            "ArrayLen should hoist; stats={stats:?}"
        );
        let header = ops
            .iter()
            .position(|op| matches!(op, IlOp::Label(Label(0))))
            .unwrap();
        assert!(
            !ops[header..=ops
                .iter()
                .rposition(|op| matches!(
                    op,
                    IlOp::Jump {
                        kind: IlJumpKind::Unconditional,
                        target: Label(0),
                        ..
                    }
                ))
                .unwrap()]
                .iter()
                .any(is_array_len),
            "ArrayLen must leave the loop body"
        );
        assert!(
            stats.proven_index >= 1,
            "Index under i < len(arr) should be proven; stats={stats:?}"
        );
    }

    #[test]
    fn refuses_array_len_hoist_when_push_in_loop() {
        reset_bounds_stats();
        let mut ops = vec![
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::StorePop {
                slot: 1,
                loc: loc(),
            },
            IlOp::Label(Label(0)),
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            array_len_op(),
            IlOp::StorePop {
                slot: 2,
                loc: loc(),
            },
            IlOp::Load {
                slot: 1,
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
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(1),
                loc: loc(),
            },
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Byte {
                byte: Byte::new(Instruction::ArrayPush),
                loc: loc(),
            },
            IlOp::Pop { loc: loc() },
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
            IlOp::Label(Label(1)),
            IlOp::Halt { loc: loc() },
        ];
        let before = ops.clone();
        loop_bounds(&mut ops);
        assert_eq!(last_bounds_stats().array_len_hoists, 0);
        assert!(
            ops.iter().filter(|op| is_array_len(op)).count()
                == before.iter().filter(|op| is_array_len(op)).count()
        );
    }

    #[test]
    fn proves_index_after_fill_loop_eq_bound() {
        reset_bounds_stats();
        // flags = MakeArray 0; i=0; while i < n { push; i++ }; then while p < n { Index }
        let mut ops = vec![
            // n in slot 0, flags → slot 1, i → 2, p → 3
            IlOp::MakeArray {
                arity: 0,
                loc: loc(),
            },
            IlOp::StorePop {
                slot: 1,
                loc: loc(),
            },
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::StorePop {
                slot: 2,
                loc: loc(),
            },
            // fill loop
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
            IlOp::Label(Label(0)),
            IlOp::Load {
                slot: 2,
                loc: loc(),
            },
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::LE,
                loc: loc(),
            },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(1),
                loc: loc(),
            },
            IlOp::Load {
                slot: 1,
                loc: loc(),
            },
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Byte {
                byte: Byte::new(Instruction::ArrayPush),
                loc: loc(),
            },
            IlOp::Pop { loc: loc() },
            IlOp::Load {
                slot: 2,
                loc: loc(),
            },
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: loc(),
            },
            IlOp::StorePop {
                slot: 2,
                loc: loc(),
            },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
            IlOp::Label(Label(1)),
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::StorePop {
                slot: 3,
                loc: loc(),
            },
            // scan loop
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(2),
                loc: loc(),
            },
            IlOp::Label(Label(2)),
            IlOp::Load {
                slot: 3,
                loc: loc(),
            },
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::LE,
                loc: loc(),
            },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(3),
                loc: loc(),
            },
            IlOp::Load {
                slot: 1,
                loc: loc(),
            },
            IlOp::Load {
                slot: 3,
                loc: loc(),
            },
            IlOp::Index { loc: loc() },
            IlOp::Pop { loc: loc() },
            IlOp::Load {
                slot: 3,
                loc: loc(),
            },
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: loc(),
            },
            IlOp::StorePop {
                slot: 3,
                loc: loc(),
            },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(2),
                loc: loc(),
            },
            IlOp::Label(Label(3)),
            IlOp::Halt { loc: loc() },
        ];
        loop_bounds(&mut ops);
        let stats = last_bounds_stats();
        assert!(
            stats.proven_index >= 1,
            "Index after fill-to-n should be proven; stats={stats:?}"
        );
    }

    #[test]
    fn hoists_array_len_with_fallthrough_header() {
        // Fall-through into header (no external JMP), matching codegen while shape.
        reset_bounds_stats();
        let mut ops = vec![
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::StorePop {
                slot: 1,
                loc: loc(),
            },
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::StorePop {
                slot: 2,
                loc: loc(),
            },
            IlOp::Label(Label(0)),
            IlOp::Load {
                slot: 1,
                loc: loc(),
            },
            IlOp::StorePop {
                slot: 3,
                loc: loc(),
            },
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            array_len_op(),
            IlOp::StorePop {
                slot: 4,
                loc: loc(),
            },
            IlOp::Load {
                slot: 3,
                loc: loc(),
            },
            IlOp::Load {
                slot: 4,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::LE,
                loc: loc(),
            },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(1),
                loc: loc(),
            },
            IlOp::Load {
                slot: 2,
                loc: loc(),
            },
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::Load {
                slot: 1,
                loc: loc(),
            },
            IlOp::Index { loc: loc() },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: loc(),
            },
            IlOp::StorePop {
                slot: 2,
                loc: loc(),
            },
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
            IlOp::Label(Label(1)),
            IlOp::Load {
                slot: 2,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        loop_bounds(&mut ops);
        let stats = last_bounds_stats();
        assert!(
            stats.array_len_hoists >= 1,
            "expected hoist; stats={stats:?}"
        );
        let header = ops
            .iter()
            .position(|op| matches!(op, IlOp::Label(Label(0))))
            .unwrap();
        assert!(
            !ops[header..]
                .iter()
                .take_while(|op| !matches!(
                    op,
                    IlOp::Jump {
                        kind: IlJumpKind::Unconditional,
                        target: Label(0),
                        ..
                    }
                ))
                .any(is_array_len),
            "ArrayLen must leave the loop body"
        );
    }

    #[test]
    fn full_optimize_still_hoists_array_len() {
        use crate::il::opt::{OptimizeOptions, optimize};
        reset_bounds_stats();
        let mut ops = vec![
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::StorePop { slot: 1, loc: loc() },
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::StorePop { slot: 2, loc: loc() },
            IlOp::Label(Label(0)),
            IlOp::Load { slot: 1, loc: loc() },
            IlOp::StorePop { slot: 3, loc: loc() },
            IlOp::Load { slot: 0, loc: loc() },
            array_len_op(),
            IlOp::StorePop { slot: 4, loc: loc() },
            IlOp::Load { slot: 3, loc: loc() },
            IlOp::Load { slot: 4, loc: loc() },
            IlOp::Bin { op: Instruction::LE, loc: loc() },
            IlOp::Jump { kind: IlJumpKind::JumpIfFalse, target: Label(1), loc: loc() },
            IlOp::Load { slot: 2, loc: loc() },
            IlOp::Load { slot: 0, loc: loc() },
            IlOp::Load { slot: 1, loc: loc() },
            IlOp::Index { loc: loc() },
            IlOp::Bin { op: Instruction::ADD, loc: loc() },
            IlOp::StorePop { slot: 2, loc: loc() },
            IlOp::Load { slot: 1, loc: loc() },
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Bin { op: Instruction::ADD, loc: loc() },
            IlOp::StorePop { slot: 1, loc: loc() },
            IlOp::Jump { kind: IlJumpKind::Unconditional, target: Label(0), loc: loc() },
            IlOp::Label(Label(1)),
            IlOp::Load { slot: 2, loc: loc() },
            IlOp::Return { loc: loc() },
        ];
        optimize(&mut ops, &OptimizeOptions::default(), &mut Vec::new());
        let stats = last_bounds_stats();
        let header = ops
            .iter()
            .position(|op| matches!(op, IlOp::Label(Label(0))))
            .unwrap();
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
            .unwrap();
        let body_lens = ops[header..=latch]
            .iter()
            .filter(|op| is_array_len(op))
            .count();
        assert_eq!(
            body_lens, 0,
            "ArrayLen should be outside loop; body_lens={body_lens}"
        );
        assert!(stats.array_len_hoists >= 1, "{stats:?}");
    }
}
