//! CFG-local value numbering for stack IL.
//!
//! Builds a simple block CFG from labels and jumps inside one function body,
//! then CSE's identical pure stack producers (`Const` / `Load` / `Bin` /
//! `BinSlot*` / `Index`) within a block. At joins, sinks a redundant producer
//! only when every predecessor ends with the same pure op and SP-in agrees.
//!
//! Limitations: no SSA rename of slots; effectful ops (`StorePop`, calls,
//! HostInvoke, …) are barriers; does not replace Ord-sensitive convoy refuse
//! rules — GVN feeds cleaner identical tails into those passes.

use std::collections::{HashMap, HashSet};

use common::Instruction;

use super::op::{IlJumpKind, IlOp, Label};
use super::sp;

/// Pure stack producer suitable for local numbering / join CSE.
fn is_pure_producer(op: &IlOp) -> bool {
    matches!(
        op,
        IlOp::Const { .. }
            | IlOp::Load { .. }
            | IlOp::Bin { .. }
            | IlOp::BinSlotImm { .. }
            | IlOp::BinSlotSlot { .. }
            | IlOp::Index { .. }
            | IlOp::Dup { .. }
    ) || matches!(
        op.as_encode_byte(),
        Some(b) if matches!(
            *b.bytecode(),
            Instruction::CONST
                | Instruction::LOAD
                | Instruction::ADD
                | Instruction::SUB
                | Instruction::MUL
                | Instruction::DIV
                | Instruction::MOD
                | Instruction::BinSlotImm
                | Instruction::BinSlotSlot
                | Instruction::Index
                | Instruction::DUPLICATE
        )
    )
}

fn producer_key(op: &IlOp) -> Option<u64> {
    let b = op.as_encode_byte()?;
    if !is_pure_producer(op) {
        return None;
    }
    // Pack opcode + operand for identity.
    Some(((*b.bytecode() as u64) << 32) | (b.operand_u32() as u64))
}

#[derive(Clone, Debug)]
struct Block {
    start: usize,
    end: usize, // exclusive
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
        let start = blocks[bi].start;
        let end = blocks[bi].end;
        if end == start {
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
        let _ = start;
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

/// Local CSE within each block: drop `op; op` when both are the same pure
/// producer (second is redundant on the stack — only for Dup of identical
/// Const/Load pairs we replace with Dup; for now remove exact `Const k; Const k`
/// by replacing the second with `Dup`).
fn gvn_within_blocks(ops: &mut Vec<IlOp>, blocks: &[Block]) {
    for b in blocks {
        let mut last_key: Option<u64> = None;
        let mut last_idx: Option<usize> = None;
        for i in b.start..b.end {
            if matches!(ops[i], IlOp::Label(_)) {
                last_key = None;
                last_idx = None;
                continue;
            }
            if !is_pure_producer(&ops[i]) {
                last_key = None;
                last_idx = None;
                continue;
            }
            let key = producer_key(&ops[i]);
            if let (Some(k), Some(pk), Some(pi)) = (key, last_key, last_idx)
                && k == pk
                && matches!(
                    &ops[pi],
                    IlOp::Const { .. } | IlOp::Load { .. }
                )
                && matches!(&ops[i], IlOp::Const { .. } | IlOp::Load { .. })
            {
                // Replace second identical Const/Load with Dup.
                ops[i] = IlOp::Dup {
                    loc: ops[i].loc(),
                };
                last_key = producer_key(&ops[i]);
                last_idx = Some(i);
                continue;
            }
            last_key = key;
            last_idx = Some(i);
        }
    }
}

/// At a join block, if every pred ends with the same pure Const/Load and the
/// join's first emitting op is that same producer, drop the join copy (preds
/// already leave it on the stack). Requires Known agreeing SP at join.
fn gvn_at_joins(ops: &mut Vec<IlOp>, blocks: &[Block]) {
    if blocks.is_empty() {
        return;
    }
    let info = sp::analyze(ops);
    let preds = preds_of(blocks);
    let mut remove: HashSet<usize> = HashSet::new();

    for (bi, b) in blocks.iter().enumerate() {
        if preds[bi].len() < 2 {
            continue;
        }
        if !info.sp_before(b.start).is_known() {
            continue;
        }
        // First emitting op in the join block.
        let mut join_prod = None;
        for i in b.start..b.end {
            if matches!(ops[i], IlOp::Label(_)) {
                continue;
            }
            if is_pure_producer(&ops[i])
                && matches!(&ops[i], IlOp::Const { .. } | IlOp::Load { .. })
            {
                join_prod = Some(i);
            }
            break;
        }
        let Some(ji) = join_prod else {
            continue;
        };
        let Some(jk) = producer_key(&ops[ji]) else {
            continue;
        };

        let mut ok = true;
        for &p in &preds[bi] {
            let pe = blocks[p].end;
            if pe == blocks[p].start {
                ok = false;
                break;
            }
            // Last emitting op before terminator / jump.
            let mut found = None;
            for i in (blocks[p].start..pe).rev() {
                if matches!(ops[i], IlOp::Label(_)) {
                    continue;
                }
                if matches!(ops[i], IlOp::Jump { .. }) {
                    continue;
                }
                if is_return_like(&ops[i]) {
                    continue;
                }
                found = Some(i);
                break;
            }
            let Some(pi) = found else {
                ok = false;
                break;
            };
            if producer_key(&ops[pi]) != Some(jk) {
                ok = false;
                break;
            }
            if !matches!(&ops[pi], IlOp::Const { .. } | IlOp::Load { .. }) {
                ok = false;
                break;
            }
        }
        if ok {
            remove.insert(ji);
        }
    }

    if remove.is_empty() {
        return;
    }
    let mut out = Vec::with_capacity(ops.len());
    for (i, op) in ops.iter().enumerate() {
        if !remove.contains(&i) {
            out.push(op.clone());
        }
    }
    *ops = out;
}

fn is_return_like(op: &IlOp) -> bool {
    matches!(
        op,
        IlOp::Return { .. }
            | IlOp::Halt { .. }
            | IlOp::LoadReturnSlot { .. }
            | IlOp::ConstReturnImm { .. }
            | IlOp::BinReturn { .. }
    )
}

/// Run CFG-local GVN on a single function body in place.
pub fn cfg_gvn(ops: &mut Vec<IlOp>) {
    if ops.len() < 2 {
        return;
    }
    let blocks = build_blocks(ops);
    gvn_within_blocks(ops, &blocks);
    // Recompute blocks after possible Dup rewrites (indices unchanged).
    let blocks = build_blocks(ops);
    gvn_at_joins(ops, &blocks);
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::DebugLoc;

    fn loc() -> DebugLoc {
        DebugLoc::unknown()
    }

    #[test]
    fn within_block_dup_replaces_second_identical_const() {
        let mut ops = vec![
            IlOp::Const {
                imm: 3,
                loc: loc(),
            },
            IlOp::Const {
                imm: 3,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        cfg_gvn(&mut ops);
        assert!(matches!(ops[0], IlOp::Const { imm: 3, .. }));
        assert!(matches!(ops[1], IlOp::Dup { .. }));
    }

    #[test]
    fn within_block_dup_replaces_second_identical_load() {
        let mut ops = vec![
            IlOp::Load { slot: 2, loc: loc() },
            IlOp::Load { slot: 2, loc: loc() },
            IlOp::Return { loc: loc() },
        ];
        cfg_gvn(&mut ops);
        assert!(matches!(ops[0], IlOp::Load { slot: 2, .. }));
        assert!(matches!(ops[1], IlOp::Dup { .. }));
    }

    #[test]
    fn within_block_store_pop_is_barrier() {
        // Effectful StorePop must reset numbering — second Const stays Const.
        let mut ops = vec![
            IlOp::Const {
                imm: 3,
                loc: loc(),
            },
            IlOp::StorePop { slot: 0, loc: loc() },
            IlOp::Const {
                imm: 3,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        cfg_gvn(&mut ops);
        assert!(matches!(ops[2], IlOp::Const { imm: 3, .. }));
        assert!(!ops.iter().any(|op| matches!(op, IlOp::Dup { .. })));
    }

    #[test]
    fn join_cse_drops_redundant_const_on_jmpf_diamond() {
        // JMPF diamond with agreeing Known SP: join CONST is redundant.
        let mut ops = vec![
            IlOp::Const {
                imm: 0,
                loc: loc(),
            },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(1),
                loc: loc(),
            },
            IlOp::Const {
                imm: 1,
                loc: loc(),
            },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(2),
                loc: loc(),
            },
            IlOp::Label(Label(1)),
            IlOp::Const {
                imm: 1,
                loc: loc(),
            },
            IlOp::Label(Label(2)),
            IlOp::Const {
                imm: 1,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        cfg_gvn(&mut ops);
        let consts = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Const { imm: 1, .. }))
            .count();
        assert_eq!(consts, 2, "pred consts kept; join copy dropped");
        assert!(ops.iter().any(|op| matches!(op, IlOp::Return { .. })));
    }

    #[test]
    fn join_cse_drops_redundant_load_on_jmpf_diamond() {
        // Same Known-SP diamond as const join CSE, with Load producers.
        let mut ops = vec![
            IlOp::Const {
                imm: 0,
                loc: loc(),
            },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(1),
                loc: loc(),
            },
            IlOp::Load { slot: 3, loc: loc() },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(2),
                loc: loc(),
            },
            IlOp::Label(Label(1)),
            IlOp::Load { slot: 3, loc: loc() },
            IlOp::Label(Label(2)),
            IlOp::Load { slot: 3, loc: loc() },
            IlOp::Return { loc: loc() },
        ];
        let info = sp::analyze(&ops);
        assert!(
            info.sp_before(6).is_known() || info.sp_before(7).is_known(),
            "precondition: join region has Known SP"
        );
        cfg_gvn(&mut ops);
        let loads = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Load { slot: 3, .. }))
            .count();
        assert_eq!(loads, 2, "pred loads kept; join Load dropped when SP Known");
        assert!(ops.iter().any(|op| matches!(op, IlOp::Return { .. })));
    }

    #[test]
    fn join_cse_keeps_disagreeing_const() {
        let mut ops = vec![
            IlOp::Const {
                imm: 0,
                loc: loc(),
            },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(1),
                loc: loc(),
            },
            IlOp::Const {
                imm: 1,
                loc: loc(),
            },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(2),
                loc: loc(),
            },
            IlOp::Label(Label(1)),
            IlOp::Const {
                imm: 2,
                loc: loc(),
            },
            IlOp::Label(Label(2)),
            IlOp::Const {
                imm: 1,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
        ];
        let before_len = ops.len();
        cfg_gvn(&mut ops);
        assert_eq!(ops.len(), before_len, "disagreeing preds must not drop join");
        assert!(ops.iter().any(|op| matches!(op, IlOp::Const { imm: 1, .. })));
        assert!(ops.iter().any(|op| matches!(op, IlOp::Const { imm: 2, .. })));
    }
}
