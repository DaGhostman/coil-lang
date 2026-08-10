//! Counted-loop array facts, and the loop-invariant hoists they license.
//!
//! The analysis answers one question per natural loop: *can the length of the
//! arrays this loop addresses change while it runs?* Element writes cannot —
//! `StoreIndex` overwrites a slot in place — so `while i < len(a) { a[i] = … }`
//! still has an invariant `len(a)` even though the array is mutated. Anything
//! that could grow, shrink or rebind an array (`ArrayPush`, a call, a host
//! native, an unmodelled opcode) refuses the whole region.
//!
//! When the length is invariant, the `LOAD a; ArrayLen; STORE t` triple codegen
//! leaves in the loop header moves to the preheader. **No bounds check is ever
//! removed**: `Index` / `StoreIndex` keep their in-VM range test, so an
//! out-of-range read still yields `-1` and an out-of-range write is still a
//! no-op. Refused shapes are listed in `docs/internals/limitations.md`.

use common::Instruction;

use super::licm::{
    NaturalLoop, find_natural_loops, insert_preheader_ops, loop_has_barrier, slots_stored_in_loop,
    store_count_in_loop,
};
use super::op::{IlOp, Label};
use super::sp;

/// Why a loop refused the length hoist, in the order the checks run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    /// Header stack height is not statically known.
    HeaderSpUnknown,
    /// A call, host native, field access or unmodelled opcode in the body: it
    /// could reach the array through an alias we cannot see.
    OpaqueOp,
    /// The body can change an array's length (`ArrayPush`, `MakeArray`, …).
    LengthMayChange,
    /// An `Index` / `StoreIndex` target is not a plain slot load, so we cannot
    /// say which array it addresses.
    UnresolvedTarget,
    /// The loop addresses no array through a slot — nothing for P2 to prove.
    NoAddressedArray,
    /// No `LOAD a; ArrayLen; STORE t` triple sits in the body.
    NoLenTriple,
}

/// A body-resident `LOAD a; ArrayLen; STORE t` whose length is invariant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LenTriple {
    /// Index of the `LOAD a`.
    pub at: usize,
    pub array_slot: u32,
    pub len_slot: u32,
}

/// What the analysis found for one natural loop.
#[derive(Clone, Debug)]
pub(super) struct LoopArrayFacts {
    pub header_label: Label,
    /// `Index` sites addressing an invariant array slot.
    pub index_sites: usize,
    /// `StoreIndex` sites addressing an invariant array slot.
    pub store_index_sites: usize,
    pub len_hoist: Option<LenTriple>,
    pub refusal: Option<Refusal>,
}

/// Per-loop array facts, innermost loop first.
pub(super) fn loop_array_facts(ops: &[IlOp]) -> Vec<LoopArrayFacts> {
    let info = sp::analyze(ops);
    let mut loops = find_natural_loops(ops);
    loops.sort_by_key(|l| std::cmp::Reverse(l.header));
    loops
        .iter()
        .map(|lp| {
            let mut facts = LoopArrayFacts {
                header_label: lp.header_label,
                index_sites: 0,
                store_index_sites: 0,
                len_hoist: None,
                refusal: None,
            };
            if !info.sp_before(lp.header).is_known() {
                facts.refusal = Some(Refusal::HeaderSpUnknown);
                return facts;
            }
            if loop_has_barrier(ops, lp) || !loop_is_modelled(ops, lp) {
                facts.refusal = Some(Refusal::OpaqueOp);
                return facts;
            }
            if loop_may_change_length(ops, lp) {
                facts.refusal = Some(Refusal::LengthMayChange);
                return facts;
            }
            let stored = slots_stored_in_loop(ops, lp);
            match addressed_arrays(ops, lp) {
                None => {
                    facts.refusal = Some(Refusal::UnresolvedTarget);
                    return facts;
                }
                Some(sites) => {
                    for (slot, reads, writes) in sites {
                        // A rebound `Vec` local is a different array each pass.
                        if stored.contains(&slot) {
                            continue;
                        }
                        facts.index_sites += reads;
                        facts.store_index_sites += writes;
                    }
                }
            }
            if facts.index_sites + facts.store_index_sites == 0 {
                facts.refusal = Some(Refusal::NoAddressedArray);
                return facts;
            }
            facts.len_hoist = find_len_triple(ops, lp, &stored);
            if facts.len_hoist.is_none() {
                facts.refusal = Some(Refusal::NoLenTriple);
            }
            facts
        })
        .collect()
}

/// Hoist every provably invariant `len(a)` out of its loop. Returns whether any
/// loop was rewritten.
pub(super) fn hoist_invariant_array_len(ops: &mut Vec<IlOp>) -> bool {
    let mut changed = false;
    // One hoist per pass invalidates indices; a triple carried out of an inner
    // loop becomes a candidate in the enclosing one, hence the per-loop budget.
    for _ in 0..find_natural_loops(ops).len().saturating_mul(2) + 2 {
        if !hoist_one_array_len(ops) {
            break;
        }
        changed = true;
    }
    changed
}

fn hoist_one_array_len(ops: &mut Vec<IlOp>) -> bool {
    let facts = loop_array_facts(ops);
    for f in &facts {
        let Some(triple) = f.len_hoist else {
            continue;
        };
        let Some(lp) = find_natural_loops(ops)
            .into_iter()
            .find(|l| l.header_label == f.header_label)
        else {
            continue;
        };
        if hoist_materialization(ops, &lp, triple.at, 3, triple.len_slot) {
            return true;
        }
    }
    false
}

/// Move the stack-neutral run at `[at, at + len)`, which ends in `STORE dest`,
/// into `lp`'s preheader, reusing `dest` so no copy is left behind.
///
/// Safe when the preheader store's cursor floor (`dest + 1`) survives the whole
/// loop: the cursor is monotone in its input, so proving every in-loop stack
/// height stays at or above the header's proves every in-loop push lands above
/// `dest`. Returns false and leaves `ops` untouched when that fails.
pub(super) fn hoist_materialization(
    ops: &mut Vec<IlOp>,
    lp: &NaturalLoop,
    at: usize,
    len: usize,
    dest: u32,
) -> bool {
    if at < lp.body_start() || at + len > lp.latch {
        return false;
    }
    // A run with a net stack effect would unbalance the body it leaves.
    let mut net = 0i32;
    for op in &ops[at..at + len] {
        match sp::stack_delta(op) {
            Some(d) => net += d,
            None => return false,
        }
    }
    if net != 0 {
        return false;
    }
    if store_count_in_loop(ops, lp, dest) != 1 {
        return false;
    }
    if !cursor_floor_survives_loop(ops, lp) {
        return false;
    }
    // Reads before the run (or outside the loop) would observe the pre-hoist
    // value of `dest`, which the hoist changes.
    if reads_slot_outside(ops, lp, at + len, dest) {
        return false;
    }

    let run: Vec<IlOp> = ops[at..at + len].to_vec();
    ops.drain(at..at + len);
    let header_label = lp.header_label;
    let Some(lp2) = find_natural_loops(ops)
        .into_iter()
        .find(|l| l.header_label == header_label)
    else {
        // Re-insert rather than leave the loop without its definition.
        ops.splice(at..at, run);
        return false;
    };
    insert_preheader_ops(ops, &lp2, run);
    true
}

/// True when every stack height inside the loop is known and at least the
/// header's — the premise that keeps a preheader cursor floor alive.
fn cursor_floor_survives_loop(ops: &[IlOp], lp: &NaturalLoop) -> bool {
    let info = sp::analyze(ops);
    let Some(header) = info.sp_before(lp.header).known() else {
        return false;
    };
    (lp.header..=lp.latch).all(|i| info.sp_before(i).known().is_some_and(|h| h >= header))
}

/// True when `slot` is read anywhere before `from` in the loop, or outside it.
fn reads_slot_outside(ops: &[IlOp], lp: &NaturalLoop, from: usize, slot: u32) -> bool {
    ops.iter().enumerate().any(|(i, op)| {
        if i >= from && i <= lp.latch {
            return false;
        }
        load_slots(op).contains(&slot)
    })
}

/// The invariant `LOAD a; ArrayLen; STORE t` triple in the body, if any.
fn find_len_triple(
    ops: &[IlOp],
    lp: &NaturalLoop,
    stored: &std::collections::HashSet<u32>,
) -> Option<LenTriple> {
    let mut i = lp.body_start();
    while i + 2 < lp.latch {
        if let Some(array_slot) = single_load_slot(&ops[i])
            && is_array_len(&ops[i + 1])
            && let Some(len_slot) = single_store_slot(&ops[i + 2])
            && !stored.contains(&array_slot)
            && array_slot != len_slot
            && store_count_in_loop(ops, lp, len_slot) == 1
        {
            return Some(LenTriple {
                at: i,
                array_slot,
                len_slot,
            });
        }
        i += 1;
    }
    None
}

/// Read / write counts per array slot addressed by `Index` / `StoreIndex`.
/// `None` when any site's target cannot be resolved to a slot.
fn addressed_arrays(ops: &[IlOp], lp: &NaturalLoop) -> Option<Vec<(u32, usize, usize)>> {
    let mut sites: Vec<(u32, usize, usize)> = Vec::new();
    for i in lp.header..=lp.latch {
        let operands = match indexing_operands(&ops[i]) {
            Some(n) => n,
            None => continue,
        };
        let slot = addressed_slot(ops, i, operands)?;
        let entry = match sites.iter_mut().find(|(s, _, _)| *s == slot) {
            Some(e) => e,
            None => {
                sites.push((slot, 0, 0));
                sites.last_mut().expect("just pushed")
            }
        };
        if operands == 2 {
            entry.1 += 1;
        } else {
            entry.2 += 1;
        }
    }
    Some(sites)
}

/// Stack operands an addressing op consumes: 2 for `Index`, 3 for `StoreIndex`.
fn indexing_operands(op: &IlOp) -> Option<usize> {
    match op {
        IlOp::Index { .. } => Some(2),
        other => match other.as_encode_byte().map(|b| *b.bytecode()) {
            Some(Instruction::Index) => Some(2),
            Some(Instruction::StoreIndex) => Some(3),
            _ => None,
        },
    }
}

/// Slot holding the array an addressing op at `at` targets.
///
/// Walks back attributing one operand to each single-value producer (`CONST`,
/// `LOAD`, a binop result, …) until the deepest one is reached; that one must be
/// a slot load. Anything whose contribution we cannot attribute — a nested
/// `Index`, a `Dup`, a jump — gives up.
fn addressed_slot(ops: &[IlOp], at: usize, operands: usize) -> Option<u32> {
    let mut need = operands;
    let mut i = at;
    while i > 0 && need > 0 {
        i -= 1;
        if matches!(ops[i], IlOp::Label(_)) {
            continue;
        }
        let slots = load_slots(&ops[i]);
        if !slots.is_empty() {
            if need <= slots.len() {
                return slots.get(slots.len() - need).copied();
            }
            need -= slots.len();
            continue;
        }
        // A non-load producer can cover an inner operand but never the target.
        if sp::stack_delta(&ops[i]) == Some(1) && need > 1 {
            need -= 1;
            continue;
        }
        return None;
    }
    None
}

/// True when no op in the loop can change the length of an existing array.
/// `StoreIndex` is allowed: it overwrites one element in place.
fn loop_may_change_length(ops: &[IlOp], lp: &NaturalLoop) -> bool {
    (lp.header..=lp.latch).any(|i| {
        matches!(
            ops[i].as_encode_byte().map(|b| *b.bytecode()),
            Some(
                Instruction::ArrayPush
                    | Instruction::MakeArray
                    | Instruction::MakeDict
                    | Instruction::CodePtr
                    | Instruction::MakePolyFn
            )
        )
    })
}

/// True when every op in the loop has a modelled stack effect. The long tail
/// (FFI, coroutines, `Seek`, statics) fails closed in [`sp::stack_delta`].
fn loop_is_modelled(ops: &[IlOp], lp: &NaturalLoop) -> bool {
    (lp.header..=lp.latch).all(|i| sp::stack_delta(&ops[i]).is_some())
}

fn is_array_len(op: &IlOp) -> bool {
    op.as_encode_byte()
        .is_some_and(|b| *b.bytecode() == Instruction::ArrayLen)
}

/// Slots a `LOAD` pushes, in push order; empty for anything else.
fn load_slots(op: &IlOp) -> Vec<u32> {
    match op {
        IlOp::Load { slot, .. } => vec![*slot],
        IlOp::Byte { byte, .. } if *byte.bytecode() == Instruction::LOAD => (0..byte
            .load_store_count())
            .map(|k| byte.load_store_slot_at(k))
            .collect(),
        _ => Vec::new(),
    }
}

fn single_load_slot(op: &IlOp) -> Option<u32> {
    match load_slots(op).as_slice() {
        [slot] => Some(*slot),
        _ => None,
    }
}

fn single_store_slot(op: &IlOp) -> Option<u32> {
    match op {
        IlOp::StorePop { slot, .. } => Some(*slot),
        IlOp::Byte { byte, .. }
            if matches!(
                *byte.bytecode(),
                Instruction::STORE | Instruction::StorePop
            ) && byte.load_store_count() == 1 =>
        {
            Some(byte.load_store_slot_at(0))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::il::op::IlJumpKind;
    use common::{Byte, DebugLoc};

    fn loc() -> DebugLoc {
        DebugLoc::unknown()
    }

    fn array_len() -> IlOp {
        IlOp::byte(Byte::new(Instruction::ArrayLen))
    }

    fn store_index() -> IlOp {
        IlOp::byte(Byte::new(Instruction::StoreIndex))
    }

    /// `while i < len(a) { acc = acc + a[i]; i = i + 1; }` after codegen: the
    /// length triple sits in the header block and must move to the preheader.
    fn read_loop() -> Vec<IlOp> {
        vec![
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::StorePop { slot: 1, loc: loc() },
            IlOp::Const { imm: 0, loc: loc() },
            IlOp::StorePop { slot: 2, loc: loc() },
            IlOp::Label(Label(0)),
            IlOp::Load { slot: 0, loc: loc() },
            array_len(),
            IlOp::StorePop { slot: 3, loc: loc() },
            IlOp::Load { slot: 2, loc: loc() },
            IlOp::Load { slot: 3, loc: loc() },
            IlOp::byte(Byte::new(Instruction::LE)),
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(1),
                loc: loc(),
            },
            IlOp::Load { slot: 0, loc: loc() },
            IlOp::Load { slot: 2, loc: loc() },
            IlOp::Index { loc: loc() },
            IlOp::StorePop { slot: 1, loc: loc() },
            IlOp::BinSlotImm {
                op: Instruction::ADD as u8,
                slot: 2,
                imm: 1,
                loc: loc(),
            },
            IlOp::StorePop { slot: 2, loc: loc() },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: loc(),
            },
            IlOp::Label(Label(1)),
            IlOp::Load { slot: 1, loc: loc() },
            IlOp::Return { loc: loc() },
        ]
    }

    fn array_len_ops_before_header(ops: &[IlOp]) -> usize {
        let header = ops
            .iter()
            .position(|op| matches!(op, IlOp::Label(Label(0))))
            .expect("header survives");
        ops[..header].iter().filter(|op| is_array_len(op)).count()
    }

    #[test]
    fn hoists_invariant_len_out_of_a_read_loop() {
        let mut ops = read_loop();
        assert!(hoist_invariant_array_len(&mut ops));
        assert_eq!(ops.iter().filter(|op| is_array_len(op)).count(), 1);
        assert_eq!(
            array_len_ops_before_header(&ops),
            1,
            "ArrayLen must sit in the preheader"
        );
    }

    #[test]
    fn hoists_invariant_len_across_element_writes() {
        // `while i < len(a) { a[i] = 0; i = i + 1; }` — `StoreIndex` overwrites
        // in place, so the length is still invariant.
        let mut ops = read_loop();
        let idx = ops
            .iter()
            .position(|op| matches!(op, IlOp::Index { .. }))
            .expect("index site");
        ops[idx] = store_index();
        ops.insert(idx, IlOp::Const { imm: 0, loc: loc() });
        assert!(hoist_invariant_array_len(&mut ops));
        assert_eq!(array_len_ops_before_header(&ops), 1);
    }

    #[test]
    fn refuses_when_the_loop_pushes_to_the_array() {
        // `while len(a) < n { a.push(…) }` — the length changes every pass.
        let mut ops = read_loop();
        let idx = ops
            .iter()
            .position(|op| matches!(op, IlOp::Index { .. }))
            .expect("index site");
        ops[idx] = IlOp::byte(Byte::new(Instruction::ArrayPush));
        let before = ops.clone();
        assert!(!hoist_invariant_array_len(&mut ops));
        assert!(ops == before);
        assert_eq!(
            loop_array_facts(&ops)[0].refusal,
            Some(Refusal::LengthMayChange)
        );
    }

    #[test]
    fn refuses_when_the_array_local_is_rebound() {
        let mut ops = read_loop();
        let latch = ops.len() - 4;
        ops.insert(latch, IlOp::StorePop { slot: 0, loc: loc() });
        ops.insert(latch, IlOp::Const { imm: 0, loc: loc() });
        let before = ops.clone();
        assert!(!hoist_invariant_array_len(&mut ops));
        assert!(ops == before, "a rebound Vec is a different array each pass");
    }

    #[test]
    fn refuses_when_a_call_could_reach_the_array() {
        let mut ops = read_loop();
        let idx = ops
            .iter()
            .position(|op| matches!(op, IlOp::Index { .. }))
            .expect("index site");
        ops[idx] = IlOp::Entry {
            kind: crate::il::op::EntryKind::Call,
            arity: 2,
            target: Label(9),
            loc: loc(),
        };
        let before = ops.clone();
        assert!(!hoist_invariant_array_len(&mut ops));
        assert!(ops == before);
        assert_eq!(loop_array_facts(&ops)[0].refusal, Some(Refusal::OpaqueOp));
    }

    #[test]
    fn refuses_when_the_loop_addresses_no_array() {
        // `while i < len(a) { acc = acc + 1; }`: nothing indexed, so P2 has no
        // aliasing question to answer and stays out.
        let mut ops = read_loop();
        let idx = ops
            .iter()
            .position(|op| matches!(op, IlOp::Index { .. }))
            .expect("index site");
        ops.splice(idx - 2..idx + 1, [IlOp::Const { imm: 1, loc: loc() }]);
        let before = ops.clone();
        assert!(!hoist_invariant_array_len(&mut ops));
        assert!(ops == before);
        assert_eq!(
            loop_array_facts(&ops)[0].refusal,
            Some(Refusal::NoAddressedArray)
        );
    }

    #[test]
    fn refuses_when_the_index_target_is_not_a_slot() {
        let mut ops = read_loop();
        let idx = ops
            .iter()
            .position(|op| matches!(op, IlOp::Index { .. }))
            .expect("index site");
        // Replace `LOAD a` with a `Dup`: the target is no longer a named slot.
        ops[idx - 2] = IlOp::Dup { loc: loc() };
        assert!(!hoist_invariant_array_len(&mut ops));
        assert_eq!(
            loop_array_facts(&ops)[0].refusal,
            Some(Refusal::UnresolvedTarget)
        );
    }

    #[test]
    fn reports_addressing_sites_per_invariant_array() {
        let ops = read_loop();
        let facts = &loop_array_facts(&ops)[0];
        assert_eq!(facts.index_sites, 1);
        assert_eq!(facts.store_index_sites, 0);
        assert_eq!(facts.refusal, None);
        assert_eq!(facts.len_hoist.map(|t| (t.array_slot, t.len_slot)), Some((0, 3)));
    }

    #[test]
    fn resolves_the_target_through_a_packed_load() {
        // `LOAD s0=a,s1=i; Index` — the packed form must resolve to `a`.
        let mut ops = read_loop();
        let idx = ops
            .iter()
            .position(|op| matches!(op, IlOp::Index { .. }))
            .expect("index site");
        ops.splice(
            idx - 2..idx,
            [IlOp::byte(
                Byte::new(Instruction::LOAD).with_load_store_packed(2, 0, 2, 0),
            )],
        );
        let facts = &loop_array_facts(&ops)[0];
        assert_eq!(facts.index_sites, 1);
        assert_eq!(facts.refusal, None);
    }

    #[test]
    fn refuses_when_the_len_temp_is_read_before_the_triple() {
        let mut ops = read_loop();
        let header = ops
            .iter()
            .position(|op| matches!(op, IlOp::Label(Label(0))))
            .expect("header");
        ops.insert(header + 1, IlOp::Pop { loc: loc() });
        ops.insert(header + 1, IlOp::Load { slot: 3, loc: loc() });
        let before = ops.clone();
        assert!(!hoist_invariant_array_len(&mut ops));
        assert!(ops == before);
    }
}
