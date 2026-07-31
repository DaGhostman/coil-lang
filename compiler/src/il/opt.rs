//! IL optimization passes unlocked by symbolic labels.

use common::Instruction;

use super::op::{IlJumpKind, IlOp, Label};

/// Options for [`optimize`].
#[derive(Clone)]
pub struct OptimizeOptions {
    /// Collapse `JMP L` where `L` begins with `JMP L2` into `JMP L2`.
    pub jump_thread: bool,
    /// Remove unreachable ops after unconditional JMP / RETURN until a label.
    pub dead_block: bool,
    /// Drop redundant `DUPLICATE; POP` and `LOAD s; StorePop s`.
    pub stack_dce: bool,
    /// Sink identical `LOAD`/`CONST` producers into a join `RETURN` and fuse.
    pub return_convoy: bool,
    /// Sink identical binop / BinSlot* tails into a return-label cluster.
    pub bin_join_convoy: bool,
    /// Sink identical multi-op suffixes (len 2..=4) at return / non-return joins.
    pub multi_op_join_convoy: bool,
}

impl Default for OptimizeOptions {
    fn default() -> Self {
        Self {
            jump_thread: true,
            // JMP + RETURN/HALT/*Return sweep; entry labels + CALL-0
            // continuations labeled.
            dead_block: true,
            stack_dce: true,
            return_convoy: true,
            bin_join_convoy: true,
            multi_op_join_convoy: true,
        }
    }
}

/// Run IL opts in place. Safe to call before [`super::lower`].
pub fn optimize(ops: &mut Vec<IlOp>, opts: &OptimizeOptions) {
    if opts.jump_thread {
        jump_thread(ops);
    }
    if opts.dead_block {
        eliminate_dead_blocks(ops);
    }
    if opts.stack_dce {
        stack_dce(ops);
    }
    if opts.return_convoy {
        return_convoy(ops);
    }
    if opts.bin_join_convoy {
        bin_join_convoy(ops);
    }
    if opts.multi_op_join_convoy {
        multi_op_join_convoy(ops);
    }
}

fn label_targets(ops: &[IlOp]) -> std::collections::HashMap<u32, usize> {
    let mut map = std::collections::HashMap::new();
    for (i, op) in ops.iter().enumerate() {
        if let IlOp::Label(Label(id)) = op {
            map.insert(*id, i);
        }
    }
    map
}

fn jump_thread(ops: &mut Vec<IlOp>) {
    let targets = label_targets(ops);
    for i in 0..ops.len() {
        let IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            target,
            loc,
        } = ops[i]
        else {
            continue;
        };
        let Some(&idx) = targets.get(&target.0) else {
            continue;
        };
        let mut j = idx;
        while j < ops.len() {
            match &ops[j] {
                IlOp::Label(_) => j += 1,
                IlOp::Jump {
                    kind: IlJumpKind::Unconditional,
                    target: t2,
                    ..
                } => {
                    ops[i] = IlOp::Jump {
                        kind: IlJumpKind::Unconditional,
                        target: *t2,
                        loc,
                    };
                    break;
                }
                _ => break,
            }
        }
    }
}

fn is_unconditional_jmp(op: &IlOp) -> bool {
    matches!(
        op,
        IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            ..
        }
    )
}

fn is_return_terminator(op: &IlOp) -> bool {
    matches!(
        op,
        IlOp::Return { .. }
            | IlOp::Halt { .. }
            | IlOp::LoadReturnSlot { .. }
            | IlOp::ConstReturnImm { .. }
            | IlOp::BinReturn { .. }
    ) || matches!(
        op.as_encode_byte(),
        Some(b) if matches!(
            *b.bytecode(),
            Instruction::RETURN
                | Instruction::HALT
                | Instruction::LoadReturnSlot
                | Instruction::ConstReturnImm
                | Instruction::BinReturn
        )
    )
}

fn eliminate_dead_blocks(ops: &mut Vec<IlOp>) {
    let mut out = Vec::with_capacity(ops.len());
    let mut reachable = true;
    for op in ops.drain(..) {
        if matches!(op, IlOp::Label(_)) {
            reachable = true;
            out.push(op);
            continue;
        }
        if !reachable {
            continue;
        }
        // Sweep after JMP and RETURN/HALT/*Return. Entry labels + CALL-0
        // continuations must be labeled so live code is not treated as
        // fall-through-after-terminator.
        let term = is_unconditional_jmp(&op) || is_return_terminator(&op);
        out.push(op);
        if term {
            reachable = false;
        }
    }
    *ops = out;
}


fn stack_dce(ops: &mut Vec<IlOp>) {
    let mut out = Vec::with_capacity(ops.len());
    let mut i = 0;
    while i < ops.len() {
        if i + 1 < ops.len()
            && matches!(&ops[i], IlOp::Dup { .. })
            && matches!(&ops[i + 1], IlOp::Pop { .. })
        {
            i += 2;
            continue;
        }
        if i + 1 < ops.len()
            && let (IlOp::Load { slot: s0, .. }, IlOp::StorePop { slot: s1, .. }) =
                (&ops[i], &ops[i + 1])
            && s0 == s1
        {
            i += 2;
            continue;
        }
        // Residual Byte fallback (pre-absorb fragments / tests).
        if i + 1 < ops.len()
            && let (Some(b0), Some(b1)) = (ops[i].as_encode_byte(), ops[i + 1].as_encode_byte())
            && *b0.bytecode() == Instruction::DUPLICATE
            && *b1.bytecode() == Instruction::POP
            && matches!(&ops[i], IlOp::Byte { .. })
            && matches!(&ops[i + 1], IlOp::Byte { .. })
        {
            i += 2;
            continue;
        }
        if i + 1 < ops.len()
            && let (Some(b0), Some(b1)) = (ops[i].as_encode_byte(), ops[i + 1].as_encode_byte())
            && *b0.bytecode() == Instruction::LOAD
            && *b1.bytecode() == Instruction::StorePop
            && b0.operand_u32() == b1.operand_u32()
            && matches!(&ops[i], IlOp::Byte { .. })
            && matches!(&ops[i + 1], IlOp::Byte { .. })
        {
            i += 2;
            continue;
        }
        out.push(ops[i].clone());
        i += 1;
    }
    *ops = out;
}

/// True if `byte` is a sinkable return producer (`LOAD s` or inline `CONST k`).
fn is_return_producer(byte: &common::Byte) -> bool {
    match *byte.bytecode() {
        Instruction::LOAD => byte.operand_u32() <= 255,
        Instruction::CONST => byte.operand_u32() & common::Byte::POOL_FLAG == 0,
        _ => false,
    }
}

fn fuse_producer_with_return(producer: common::Byte) -> IlOp {
    match *producer.bytecode() {
        Instruction::LOAD => IlOp::LoadReturnSlot {
            slot: producer.operand_u32(),
            loc: common::DebugLoc::unknown(),
        },
        Instruction::CONST => IlOp::ConstReturnImm {
            imm: producer.operand_u32(),
            loc: common::DebugLoc::unknown(),
        },
        _ => unreachable!("is_return_producer gate"),
    }
}

/// Producer must sit immediately before `idx` (no intervening labels).
fn immediate_byte_before(ops: &[IlOp], idx: usize) -> Option<(usize, common::Byte)> {
    if idx == 0 {
        return None;
    }
    let b = ops[idx - 1].as_encode_byte()?;
    Some((idx - 1, b))
}

fn immediate_producer_before(ops: &[IlOp], idx: usize) -> Option<(usize, common::Byte)> {
    let (i, b) = immediate_byte_before(ops, idx)?;
    if is_return_producer(&b) {
        Some((i, b))
    } else {
        None
    }
}

fn is_plain_binop(byte: &common::Byte) -> bool {
    matches!(
        *byte.bytecode(),
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
    )
}

fn is_bin_slot_tail(byte: &common::Byte) -> bool {
    matches!(
        *byte.bytecode(),
        Instruction::BinSlotImm | Instruction::BinSlotSlot
    )
}

/// True if `byte` is a sinkable bin-join tail (plain binop or BinSlot*).
fn is_bin_join_tail(byte: &common::Byte) -> bool {
    is_plain_binop(byte) || is_bin_slot_tail(byte)
}

fn fuse_binop_to_bin_return(op: common::Byte) -> IlOp {
    IlOp::BinReturn {
        op: *op.bytecode(),
        loc: common::DebugLoc::unknown(),
    }
}

/// Kind of join after a label cluster for multi-op suffix sinking.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum JoinKind {
    /// Cluster is followed by plain `RETURN`; place suffix before it.
    Return,
    /// Cluster is followed by a shared continuation; place suffix after labels.
    NonReturn,
}

/// Find `[cluster_start, cluster_end]` of Labels immediately before a plain RETURN at `r`.
fn return_label_cluster(ops: &[IlOp], r: usize) -> Option<(usize, usize)> {
    if !ops[r].is_plain_return() {
        return None;
    }
    if r == 0 || !matches!(ops[r - 1], IlOp::Label(_)) {
        return None;
    }
    let cluster_end = r - 1;
    let mut cluster_start = cluster_end;
    while cluster_start > 0 && matches!(ops[cluster_start - 1], IlOp::Label(_)) {
        cluster_start -= 1;
    }
    Some((cluster_start, cluster_end))
}

/// Label run starting at `i` with an unambiguous post-cluster consumer.
///
/// Return clusters keep today's rewrite. Non-return requires a non-label
/// consumer that is not an unconditional jump-only terminator (no local work).
fn join_label_cluster(ops: &[IlOp], i: usize) -> Option<(usize, usize, JoinKind)> {
    if !matches!(ops.get(i), Some(IlOp::Label(_))) {
        return None;
    }
    // Only the start of a consecutive label run.
    if i > 0 && matches!(ops[i - 1], IlOp::Label(_)) {
        return None;
    }
    let cluster_start = i;
    let mut cluster_end = i;
    while cluster_end + 1 < ops.len() && matches!(ops[cluster_end + 1], IlOp::Label(_)) {
        cluster_end += 1;
    }
    let after = cluster_end + 1;
    if after >= ops.len() {
        return None;
    }
    let consumer = &ops[after];
    if consumer.is_plain_return() {
        return Some((cluster_start, cluster_end, JoinKind::Return));
    }
    // Unconditional jump-only: no local work at the join.
    if matches!(
        consumer,
        IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            ..
        }
    ) {
        return None;
    }
    // Fused *Return / HALT: leave to return_convoy / dead_block.
    if is_return_terminator(consumer) {
        return None;
    }
    // Non-label emitting (or control) consumer — shared continuation.
    if matches!(consumer, IlOp::Label(_)) {
        return None;
    }
    Some((cluster_start, cluster_end, JoinKind::NonReturn))
}

/// Sink identical binop / `BinSlot*` tails into a return-label cluster.
///
/// - Plain binop `OP` on every pred → `BinReturn(OP)`.
/// - Identical `BinSlotImm`/`BinSlotSlot` → keep one copy before `RETURN`.
fn bin_join_convoy(ops: &mut Vec<IlOp>) {
    // (cluster_start, cluster_end, tail_byte, emit_bin_return)
    let mut joins: Vec<(usize, usize, common::Byte, bool)> = Vec::new();
    let mut r = 0usize;
    while r < ops.len() {
        let Some((cluster_start, cluster_end)) = return_label_cluster(ops, r) else {
            r += 1;
            continue;
        };
        let cluster = label_cluster_ids(ops, cluster_start, cluster_end);
        let Some((_, fall_t)) = immediate_byte_before(ops, cluster_start) else {
            r += 1;
            continue;
        };
        if !is_bin_join_tail(&fall_t) {
            r += 1;
            continue;
        }

        let mut ok = true;
        let mut jump_preds = 0usize;
        for (j, op) in ops.iter().enumerate() {
            let IlOp::Jump {
                kind,
                target,
                ..
            } = op
            else {
                continue;
            };
            if !cluster.iter().any(|l| l == target) {
                continue;
            }
            if *kind != IlJumpKind::Unconditional {
                ok = false;
                break;
            }
            let Some((_p_idx, t)) = immediate_byte_before(ops, j) else {
                ok = false;
                break;
            };
            if t != fall_t {
                ok = false;
                break;
            }
            jump_preds += 1;
        }
        if !ok || jump_preds == 0 {
            r += 1;
            continue;
        }

        let emit_bin_return = is_plain_binop(&fall_t);
        joins.push((cluster_start, cluster_end, fall_t, emit_bin_return));
        r += 1;
    }

    if joins.is_empty() {
        return;
    }

    let mut remove_tail_at: std::collections::HashSet<usize> = std::collections::HashSet::new();
    // cluster_start → (cluster_end, optional fused BinReturn, keep_slot_tail)
    let mut rewrite: std::collections::HashMap<usize, (usize, Option<IlOp>, Option<common::Byte>)> =
        std::collections::HashMap::new();

    for (cluster_start, cluster_end, tail, emit_bin_return) in &joins {
        let cluster = label_cluster_ids(ops, *cluster_start, *cluster_end);
        if let Some((fall_idx, t)) = immediate_byte_before(ops, *cluster_start)
            && t == *tail
        {
            remove_tail_at.insert(fall_idx);
        }
        for (j, op) in ops.iter().enumerate() {
            if let IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target,
                ..
            } = op
                && cluster.iter().any(|l| l == target)
                && let Some((t_idx, t)) = immediate_byte_before(ops, j)
                && t == *tail
            {
                remove_tail_at.insert(t_idx);
            }
        }
        if *emit_bin_return {
            rewrite.insert(
                *cluster_start,
                (*cluster_end, Some(fuse_binop_to_bin_return(*tail)), None),
            );
        } else {
            // Keep one BinSlot* before RETURN after the label cluster.
            rewrite.insert(*cluster_start, (*cluster_end, None, Some(*tail)));
        }
    }

    let mut out = Vec::with_capacity(ops.len());
    let mut idx = 0;
    while idx < ops.len() {
        if remove_tail_at.contains(&idx) {
            idx += 1;
            continue;
        }
        if let Some((cluster_end, fused, keep_slot)) = rewrite.remove(&idx) {
            for k in idx..=cluster_end {
                out.push(ops[k].clone());
            }
            if let Some(f) = fused {
                out.push(f);
                idx = cluster_end + 2; // skip RETURN
            } else if let Some(slot_tail) = keep_slot {
                out.push(IlOp::byte(slot_tail));
                // Keep the original RETURN after the cluster.
                out.push(ops[cluster_end + 1].clone());
                idx = cluster_end + 2;
            } else {
                idx = cluster_end + 1;
            }
            continue;
        }
        out.push(ops[idx].clone());
        idx += 1;
    }
    *ops = out;
}

const MULTI_OP_SUFFIX_MAX: usize = 4;

fn is_multi_op_suffix_op(op: &IlOp) -> bool {
    if matches!(
        op,
        IlOp::Label(_) | IlOp::Jump { .. } | IlOp::Entry { .. } | IlOp::PrologueJmp { .. }
    ) || op.is_plain_return()
        || is_return_terminator(op)
    {
        return false;
    }
    op.as_encode_byte().is_some()
}

fn suffix_before(ops: &[IlOp], end: usize, len: usize) -> Option<&[IlOp]> {
    if len < 2 || end < len {
        return None;
    }
    let start = end - len;
    let slice = &ops[start..end];
    if slice.iter().all(is_multi_op_suffix_op) {
        Some(slice)
    } else {
        None
    }
}

fn suffixes_equal(a: &[IlOp], b: &[IlOp]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(x, y)| x.as_encode_byte() == y.as_encode_byte())
}

/// Jump kinds allowed as convoy predecessors (SP fail-closed at the join).
fn is_multi_op_join_pred_kind(kind: IlJumpKind) -> bool {
    matches!(
        kind,
        IlJumpKind::Unconditional
            | IlJumpKind::JumpIfFalse
            | IlJumpKind::JumpIfTrue
            | IlJumpKind::JumpIfMatch { .. }
    )
}

/// Sink identical multi-op compute suffixes into a return or non-return join.
///
/// Length cap is [`MULTI_OP_SUFFIX_MAX`]. Single-op tails stay with
/// [`bin_join_convoy`] / [`return_convoy`] (return-only; no `len==1` for
/// non-return). Requires agreeing SP at suffix starts and at the join
/// (see [`super::sp::analyze`]). Accepts `JMP` / `JMPF` / `JMPT` /
/// `JumpIfMatch` into the cluster. When fall-through has no suffix, the
/// template comes from the first jump pred.
fn multi_op_join_convoy(ops: &mut Vec<IlOp>) {
    let info = super::sp::analyze(ops);
    // (cluster_start, cluster_end, kind, suffix)
    let mut joins: Vec<(usize, usize, JoinKind, Vec<IlOp>)> = Vec::new();
    let mut i = 0usize;
    while i < ops.len() {
        let Some((cluster_start, cluster_end, kind)) = join_label_cluster(ops, i) else {
            i += 1;
            continue;
        };
        let cluster = label_cluster_ids(ops, cluster_start, cluster_end);

        let mut jump_pred_ends: Vec<usize> = Vec::new();
        let mut ok_edges = true;
        for (j, op) in ops.iter().enumerate() {
            let IlOp::Jump {
                kind: jk,
                target,
                ..
            } = op
            else {
                continue;
            };
            if !cluster.iter().any(|l| l == target) {
                continue;
            }
            if !is_multi_op_join_pred_kind(*jk) {
                ok_edges = false;
                break;
            }
            jump_pred_ends.push(j);
        }
        if !ok_edges || jump_pred_ends.is_empty() {
            i = cluster_end + 1;
            continue;
        }

        let join_sp = info.sp_before(cluster_start);
        if !join_sp.is_known() {
            i = cluster_end + 1;
            continue;
        }

        let mut chosen: Option<Vec<IlOp>> = None;
        'len: for len in (2..=MULTI_OP_SUFFIX_MAX).rev() {
            let fall = suffix_before(ops, cluster_start, len);
            let (template, template_start) = if let Some(f) = fall {
                (f, cluster_start - len)
            } else if let Some(suf) = suffix_before(ops, jump_pred_ends[0], len) {
                (suf, jump_pred_ends[0] - len)
            } else {
                continue;
            };
            let Some(template_sp) = info.sp_before(template_start).known() else {
                continue;
            };

            if let Some(f) = fall {
                if !suffixes_equal(template, f) {
                    continue;
                }
            }

            for &j in &jump_pred_ends {
                let Some(suf) = suffix_before(ops, j, len) else {
                    continue 'len;
                };
                if !suffixes_equal(template, suf) {
                    continue 'len;
                }
                let Some(jsp) = info.sp_before(j - len).known() else {
                    continue 'len;
                };
                if jsp != template_sp {
                    continue 'len;
                }
            }

            chosen = Some(template.to_vec());
            break;
        }

        let Some(suffix) = chosen else {
            i = cluster_end + 1;
            continue;
        };
        joins.push((cluster_start, cluster_end, kind, suffix));
        i = cluster_end + 1;
    }

    if joins.is_empty() {
        return;
    }

    let mut remove_at: std::collections::HashSet<usize> = std::collections::HashSet::new();
    // cluster_start → (cluster_end, kind, suffix)
    let mut rewrite: std::collections::HashMap<usize, (usize, JoinKind, Vec<IlOp>)> =
        std::collections::HashMap::new();

    for (cluster_start, cluster_end, kind, suffix) in &joins {
        let len = suffix.len();
        let cluster = label_cluster_ids(ops, *cluster_start, *cluster_end);
        // Strip fall-through only when it actually carries the suffix.
        if let Some(fall) = suffix_before(ops, *cluster_start, len)
            && suffixes_equal(fall, suffix)
        {
            for i in (*cluster_start - len)..*cluster_start {
                remove_at.insert(i);
            }
        }
        for (j, op) in ops.iter().enumerate() {
            if let IlOp::Jump {
                kind: jk,
                target,
                ..
            } = op
                && is_multi_op_join_pred_kind(*jk)
                && cluster.iter().any(|l| l == target)
            {
                for i in (j - len)..j {
                    remove_at.insert(i);
                }
            }
        }
        rewrite.insert(*cluster_start, (*cluster_end, *kind, suffix.clone()));
    }

    let mut out = Vec::with_capacity(ops.len());
    let mut idx = 0;
    while idx < ops.len() {
        if remove_at.contains(&idx) {
            idx += 1;
            continue;
        }
        if let Some((cluster_end, kind, suffix)) = rewrite.remove(&idx) {
            for k in idx..=cluster_end {
                out.push(ops[k].clone());
            }
            out.extend(suffix);
            match kind {
                JoinKind::Return => {
                    // Keep RETURN after the original cluster.
                    out.push(ops[cluster_end + 1].clone());
                    idx = cluster_end + 2;
                }
                JoinKind::NonReturn => {
                    // Existing post-join ops follow from the original stream.
                    idx = cluster_end + 1;
                }
            }
            continue;
        }
        out.push(ops[idx].clone());
        idx += 1;
    }
    *ops = out;
}

/// Labels from `start` through `end` inclusive (all must be `Label`).
fn label_cluster_ids(ops: &[IlOp], start: usize, end: usize) -> Vec<Label> {
    (start..=end)
        .filter_map(|i| match &ops[i] {
            IlOp::Label(l) => Some(*l),
            _ => None,
        })
        .collect()
}

/// Sink identical `LOAD`/`CONST` producers into a return-label cluster and fuse
/// to `LoadReturnSlot` / `ConstReturnImm`.
///
/// A cluster is one or more consecutive `Label`s immediately before bare
/// `RETURN`. The **first** label is the stack join (JMPs target it); trailing
/// labels are PC aliases (e.g. `Label(join); Label(ret); RETURN`).
fn return_convoy(ops: &mut Vec<IlOp>) {
    // (cluster_start, cluster_end, join, producer)
    let mut joins: Vec<(usize, usize, Label, common::Byte)> = Vec::new();
    let mut r = 0usize;
    while r < ops.len() {
        let Some((cluster_start, cluster_end)) = return_label_cluster(ops, r) else {
            r += 1;
            continue;
        };
        let IlOp::Label(join) = ops[cluster_start] else {
            r += 1;
            continue;
        };
        let cluster = label_cluster_ids(ops, cluster_start, cluster_end);
        let Some((_, fall_p)) = immediate_producer_before(ops, cluster_start) else {
            r += 1;
            continue;
        };

        let mut ok = true;
        let mut jump_preds = 0usize;
        for (j, op) in ops.iter().enumerate() {
            let IlOp::Jump {
                kind,
                target,
                ..
            } = op
            else {
                continue;
            };
            if !cluster.iter().any(|l| l == target) {
                continue;
            }
            if *kind != IlJumpKind::Unconditional {
                ok = false;
                break;
            }
            let Some((_p_idx, p)) = immediate_producer_before(ops, j) else {
                ok = false;
                break;
            };
            if p != fall_p {
                ok = false;
                break;
            }
            jump_preds += 1;
        }
        if !ok || jump_preds == 0 {
            r += 1;
            continue;
        }

        joins.push((cluster_start, cluster_end, join, fall_p));
        r += 1;
    }

    if joins.is_empty() {
        return;
    }

    let mut remove_producer_at: std::collections::HashSet<usize> =
        std::collections::HashSet::new();
    // cluster_start → fused op; keep all labels in [start,end], replace RETURN
    let mut fuse_at_cluster: std::collections::HashMap<usize, (usize, IlOp)> =
        std::collections::HashMap::new();

    for (cluster_start, cluster_end, join, producer) in &joins {
        let cluster = label_cluster_ids(ops, *cluster_start, *cluster_end);
        if let Some((fall_idx, p)) = immediate_producer_before(ops, *cluster_start)
            && p == *producer
        {
            remove_producer_at.insert(fall_idx);
        }
        for (j, op) in ops.iter().enumerate() {
            if let IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target,
                ..
            } = op
                && cluster.iter().any(|l| l == target)
                && let Some((p_idx, p)) = immediate_producer_before(ops, j)
                && p == *producer
            {
                remove_producer_at.insert(p_idx);
            }
        }
        let _ = join;
        fuse_at_cluster.insert(
            *cluster_start,
            (*cluster_end, fuse_producer_with_return(*producer)),
        );
    }

    let mut out = Vec::with_capacity(ops.len());
    let mut idx = 0;
    while idx < ops.len() {
        if remove_producer_at.contains(&idx) {
            idx += 1;
            continue;
        }
        if let Some((cluster_end, fused)) = fuse_at_cluster.remove(&idx) {
            // Keep Label cluster, replace following RETURN with fused.
            for k in idx..=cluster_end {
                out.push(ops[k].clone());
            }
            out.push(fused);
            idx = cluster_end + 2; // skip cluster + RETURN
            continue;
        }
        out.push(ops[idx].clone());
        idx += 1;
    }
    *ops = out;
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{Byte, Instruction};

    fn is_insn(op: &IlOp, i: Instruction) -> bool {
        op.instruction() == Some(i)
    }

    #[test]
    fn jump_thread_collapses_goto_goto() {
        let mut ops = vec![
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Byte {
                byte: Byte::new(Instruction::CONST).with_const_inline(1),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Label(Label(0)),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(1),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Label(Label(1)),
            IlOp::Byte {
                byte: Byte::new(Instruction::HALT),
                loc: common::DebugLoc::unknown(),
            },
        ];
        jump_thread(&mut ops);
        match &ops[0] {
            IlOp::Jump {
                target: Label(1), ..
            } => {}
            _ => panic!("expected JMP L1 after jump threading"),
        }
    }

    #[test]
    fn stack_dce_removes_dup_pop() {
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::DUPLICATE)),
            IlOp::byte(Byte::new(Instruction::POP)),
            IlOp::byte(Byte::new(Instruction::HALT)),
        ];
        stack_dce(&mut ops);
        assert_eq!(ops.len(), 1);
    }

    #[test]
    fn stack_dce_removes_load_store_pop_same_slot() {
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::LOAD).with_operand_u32(4)),
            IlOp::byte(Byte::new(Instruction::StorePop).with_operand_u32(4)),
            IlOp::byte(Byte::new(Instruction::HALT)),
        ];
        stack_dce(&mut ops);
        assert_eq!(ops.len(), 1);
        assert!(is_insn(&ops[0], Instruction::HALT));
    }

    #[test]
    fn stack_dce_keeps_load_store_pop_different_slots() {
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::LOAD).with_operand_u32(1)),
            IlOp::byte(Byte::new(Instruction::StorePop).with_operand_u32(2)),
            IlOp::byte(Byte::new(Instruction::HALT)),
        ];
        let before = ops.clone();
        stack_dce(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn dead_block_drops_after_unconditional_jmp() {
        let mut ops = vec![
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(1)),
            IlOp::Label(Label(0)),
            IlOp::byte(Byte::new(Instruction::HALT)),
        ];
        eliminate_dead_blocks(&mut ops);
        assert_eq!(ops.len(), 3);
        assert!(matches!(ops[1], IlOp::Label(Label(0))));
    }

    #[test]
    fn dead_block_drops_after_return_until_label() {
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::RETURN)),
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(1)),
            IlOp::Label(Label(0)),
            IlOp::byte(Byte::new(Instruction::HALT)),
        ];
        eliminate_dead_blocks(&mut ops);
        assert_eq!(ops.len(), 3);
        assert!(is_insn(&ops[0], Instruction::RETURN));
        assert!(matches!(ops[1], IlOp::Label(Label(0))));
    }

    #[test]
    fn dead_block_drops_after_fused_return_until_label() {
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::ConstReturnImm).with_operand_u32(0)),
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(99)),
            IlOp::Label(Label(0)),
            IlOp::byte(Byte::new(Instruction::HALT)),
        ];
        eliminate_dead_blocks(&mut ops);
        assert_eq!(ops.len(), 3);
        assert!(is_insn(&ops[0], Instruction::ConstReturnImm));
        assert!(matches!(ops[1], IlOp::Label(Label(0))));
    }

    #[test]
    fn return_convoy_fuses_agreeing_const_join() {
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(0)),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(0)),
            IlOp::Label(Label(0)),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        return_convoy(&mut ops);
        assert!(
            ops.iter().any(|op| is_insn(op, Instruction::ConstReturnImm)),
            "expected ConstReturnImm"
        );
        assert!(
            !ops.iter().any(|op| is_insn(op, Instruction::CONST)),
            "producers should be stripped"
        );
        assert!(ops.iter().any(|op| matches!(op, IlOp::Label(Label(0)))));
        assert!(ops.iter().any(|op| matches!(
            op,
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                ..
            }
        )));
    }

    #[test]
    fn return_convoy_fuses_agreeing_load_join() {
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::LOAD).with_operand_u32(0)),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::LOAD).with_operand_u32(0)),
            IlOp::Label(Label(0)),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        return_convoy(&mut ops);
        assert!(ops.iter().any(|op| {
            matches!(op, IlOp::LoadReturnSlot { slot: 0, .. })
                || op
                    .as_encode_byte()
                    .is_some_and(|b| *b.bytecode() == Instruction::LoadReturnSlot && b.operand_u32() == 0)
        }));
    }

    #[test]
    fn return_convoy_skips_disagreeing_consts() {
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(0)),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(1)),
            IlOp::Label(Label(0)),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        let before = ops.clone();
        return_convoy(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn return_convoy_skips_jump_without_producer() {
        // JMP to join with a value already on the stack (no LOAD/CONST before JMP).
        let mut ops = vec![
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(0)),
            IlOp::Label(Label(0)),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        let before = ops.clone();
        return_convoy(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn return_convoy_skips_conditional_jump_into_cluster() {
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(0)),
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(0)),
            IlOp::Label(Label(0)),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        let before = ops.clone();
        return_convoy(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn return_convoy_skips_disagreeing_consts_in_label_cluster() {
        // Ord-shaped: Label(join); Label(ret); RETURN with mixed CONST 0/1.
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(0)),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(54),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(1)),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(54),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(0)),
            IlOp::Label(Label(54)),
            IlOp::Label(Label(48)),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        let before = ops.clone();
        return_convoy(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn return_convoy_fuses_agreeing_const_through_label_cluster() {
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(0)),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(54),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(0)),
            IlOp::Label(Label(54)),
            IlOp::Label(Label(48)),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        return_convoy(&mut ops);
        assert!(ops.iter().any(|op| {
            matches!(
                op.instruction(),
                Some(Instruction::ConstReturnImm)
            ) || matches!(op, IlOp::ConstReturnImm { .. })
        }));
        assert!(ops.iter().any(|op| matches!(op, IlOp::Label(Label(54)))));
        assert!(ops.iter().any(|op| matches!(op, IlOp::Label(Label(48)))));
        assert!(
            !ops.iter().any(|op| is_insn(op, Instruction::CONST)),
            "producers should be stripped"
        );
    }

    #[test]
    fn return_convoy_fuses_agreeing_load_through_label_cluster() {
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::LOAD).with_operand_u32(2)),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(1),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::LOAD).with_operand_u32(2)),
            IlOp::Label(Label(1)),
            IlOp::Label(Label(9)),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        return_convoy(&mut ops);
        assert!(ops.iter().any(|op| {
            matches!(op, IlOp::LoadReturnSlot { slot: 2, .. })
                || op.as_encode_byte().is_some_and(|b| {
                    *b.bytecode() == Instruction::LoadReturnSlot && b.operand_u32() == 2
                })
        }));
    }

    #[test]
    fn bin_join_convoy_fuses_agreeing_binop_to_bin_return() {
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::ADD)),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::ADD)),
            IlOp::Label(Label(0)),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        bin_join_convoy(&mut ops);
        assert!(ops.iter().any(|op| {
            matches!(op, IlOp::BinReturn { op: Instruction::ADD, .. })
                || op.as_encode_byte().is_some_and(|b| {
                    *b.bytecode() == Instruction::BinReturn
                        && b.bin_return_op() == Instruction::ADD as u8
                })
        }));
        assert!(
            !ops.iter().any(|op| is_insn(op, Instruction::ADD)),
            "plain ADDs should be stripped"
        );
        assert!(
            !ops.iter().any(|op| is_insn(op, Instruction::RETURN)),
            "RETURN should be replaced by BinReturn"
        );
    }

    #[test]
    fn bin_join_convoy_sinks_identical_bin_slot_slot() {
        let slot = Byte::new(Instruction::BinSlotSlot).with_bin_slot_slot(
            Instruction::ADD as u8,
            0,
            1,
        );
        let mut ops = vec![
            IlOp::byte(slot),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(slot),
            IlOp::Label(Label(0)),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        bin_join_convoy(&mut ops);
        let slot_count = ops
            .iter()
            .filter(|op| is_insn(op, Instruction::BinSlotSlot))
            .count();
        assert_eq!(slot_count, 1, "exactly one BinSlotSlot before RETURN");
        assert!(ops.iter().any(|op| is_insn(op, Instruction::RETURN)));
    }

    #[test]
    fn bin_join_convoy_sinks_identical_bin_slot_imm() {
        let imm = Byte::new(Instruction::BinSlotImm).with_bin_slot_imm(
            Instruction::ADD as u8,
            0,
            1,
        );
        let mut ops = vec![
            IlOp::byte(imm),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(imm),
            IlOp::Label(Label(0)),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        bin_join_convoy(&mut ops);
        let imm_count = ops
            .iter()
            .filter(|op| is_insn(op, Instruction::BinSlotImm))
            .count();
        assert_eq!(imm_count, 1, "exactly one BinSlotImm before RETURN");
        assert!(
            !ops.iter().any(|op| is_insn(op, Instruction::BinReturn)),
            "BinSlotImm must stay as slot tail, not BinReturn"
        );
        assert!(ops.iter().any(|op| is_insn(op, Instruction::RETURN)));
    }

    #[test]
    fn bin_join_convoy_skips_disagreeing_binops() {
        // Ord-shaped: Lt arm ends in LE, Gt arm in GT — must not convoy.
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::LE)),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(54),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::GT)),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(54),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::EQ)),
            IlOp::Label(Label(54)),
            IlOp::Label(Label(48)),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        let before = ops.clone();
        bin_join_convoy(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn bin_join_convoy_skips_conditional_jump_into_cluster() {
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::ADD)),
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::ADD)),
            IlOp::Label(Label(0)),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        let before = ops.clone();
        bin_join_convoy(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn bin_join_convoy_fuses_through_label_cluster() {
        let mut ops = vec![
            IlOp::byte(Byte::new(Instruction::SUB)),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(1),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::SUB)),
            IlOp::Label(Label(1)),
            IlOp::Label(Label(9)),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        bin_join_convoy(&mut ops);
        assert!(ops.iter().any(|op| {
            matches!(op, IlOp::BinReturn { op: Instruction::SUB, .. })
                || op.as_encode_byte().is_some_and(|b| {
                    *b.bytecode() == Instruction::BinReturn
                        && b.bin_return_op() == Instruction::SUB as u8
                })
        }));
        assert!(ops.iter().any(|op| matches!(op, IlOp::Label(Label(1)))));
        assert!(ops.iter().any(|op| matches!(op, IlOp::Label(Label(9)))));
    }

    #[test]
    fn return_convoy_accepts_typed_const_ops() {
        let mut ops = vec![
            IlOp::Const {
                imm: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Const {
                imm: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Label(Label(0)),
            IlOp::Return {
                loc: common::DebugLoc::unknown(),
            },
        ];
        return_convoy(&mut ops);
        assert!(ops.iter().any(|op| matches!(op, IlOp::ConstReturnImm { imm: 0, .. })));
        assert!(!ops.iter().any(|op| matches!(op, IlOp::Const { .. })));
    }

    #[test]
    fn return_convoy_accepts_typed_load_ops() {
        let mut ops = vec![
            IlOp::Load {
                slot: 3,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Load {
                slot: 3,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Label(Label(0)),
            IlOp::Return {
                loc: common::DebugLoc::unknown(),
            },
        ];
        return_convoy(&mut ops);
        assert!(ops
            .iter()
            .any(|op| matches!(op, IlOp::LoadReturnSlot { slot: 3, .. })));
        assert!(!ops.iter().any(|op| matches!(op, IlOp::Load { .. })));
    }

    #[test]
    fn bin_join_convoy_accepts_typed_bin_ops() {
        let mut ops = vec![
            IlOp::Bin {
                op: Instruction::MUL,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Bin {
                op: Instruction::MUL,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Label(Label(0)),
            IlOp::Return {
                loc: common::DebugLoc::unknown(),
            },
        ];
        bin_join_convoy(&mut ops);
        assert!(ops
            .iter()
            .any(|op| matches!(op, IlOp::BinReturn { op: Instruction::MUL, .. })));
        assert!(!ops.iter().any(|op| matches!(op, IlOp::Bin { .. })));
        assert!(!ops.iter().any(|op| matches!(op, IlOp::Return { .. })));
    }

    #[test]
    fn bin_join_convoy_sinks_typed_bin_slot_imm() {
        let imm = IlOp::BinSlotImm {
            op: Instruction::ADD as u8,
            slot: 0,
            imm: 1,
            loc: common::DebugLoc::unknown(),
        };
        let mut ops = vec![
            imm.clone(),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            imm,
            IlOp::Label(Label(0)),
            IlOp::Return {
                loc: common::DebugLoc::unknown(),
            },
        ];
        bin_join_convoy(&mut ops);
        let imm_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::BinSlotImm { .. }))
            .count();
        assert_eq!(imm_count, 1);
        assert!(ops.iter().any(|op| matches!(op, IlOp::Return { .. })));
    }

    #[test]
    fn stack_dce_removes_typed_dup_pop() {
        let mut ops = vec![
            IlOp::Dup {
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Pop {
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Halt {
                loc: common::DebugLoc::unknown(),
            },
        ];
        stack_dce(&mut ops);
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], IlOp::Halt { .. }));
    }

    fn load_const_add_suffix() -> Vec<IlOp> {
        vec![
            IlOp::Load {
                slot: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Const {
                imm: 1,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: common::DebugLoc::unknown(),
            },
        ]
    }

    #[test]
    fn multi_op_join_convoy_sinks_identical_suffix() {
        // Diamond: both arms end with Load;Const;ADD then join+RETURN.
        let suf = load_const_add_suffix();
        let mut ops = vec![
            IlOp::Const {
                imm: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(1),
                loc: common::DebugLoc::unknown(),
            },
        ];
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Label(Label(1)));
        ops.extend(suf);
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::Return {
            loc: common::DebugLoc::unknown(),
        });

        multi_op_join_convoy(&mut ops);

        let load_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Load { .. }))
            .count();
        let add_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Bin { op: Instruction::ADD, .. }))
            .count();
        assert_eq!(load_count, 1, "suffix should appear once after join");
        assert_eq!(add_count, 1);
        assert!(ops.iter().any(|op| matches!(op, IlOp::Label(Label(0)))));
        assert!(ops.iter().any(|op| matches!(op, IlOp::Return { .. })));
        assert!(ops.iter().any(|op| matches!(
            op,
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                ..
            }
        )));
    }

    #[test]
    fn multi_op_join_convoy_skips_disagreeing_suffixes() {
        // Ord-shaped: one arm LOAD;CONST 0;ADD, other LOAD;CONST 1;ADD.
        let mut ops = vec![
            IlOp::Load {
                slot: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Const {
                imm: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(54),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Load {
                slot: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Const {
                imm: 1,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(54),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Load {
                slot: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Const {
                imm: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Label(Label(54)),
            IlOp::Label(Label(48)),
            IlOp::Return {
                loc: common::DebugLoc::unknown(),
            },
        ];
        let before = ops.clone();
        multi_op_join_convoy(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn multi_op_join_convoy_skips_jmpf_fallthrough_unknown_sp() {
        // JMPF + fall-through identical S: JMPF is −1 vs fall-through 0 → join
        // SP Unknown → refuse (fail closed).
        let suf = load_const_add_suffix();
        let mut ops = Vec::new();
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfFalse,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.extend(suf);
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::Return {
            loc: common::DebugLoc::unknown(),
        });
        let before = ops.clone();
        multi_op_join_convoy(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn multi_op_join_convoy_sinks_identical_suffix_via_jmpf() {
        // Two arms: S; JMPF Ljoin — both −1, join SP Known; no fall-through.
        let suf = load_const_add_suffix();
        let mut ops = Vec::new();
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfFalse,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.extend(suf);
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfFalse,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::Return {
            loc: common::DebugLoc::unknown(),
        });

        multi_op_join_convoy(&mut ops);

        let load_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Load { .. }))
            .count();
        let add_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Bin { op: Instruction::ADD, .. }))
            .count();
        assert_eq!(load_count, 1, "suffix should appear once after join");
        assert_eq!(add_count, 1);
        let jmpf_count = ops
            .iter()
            .filter(|op| {
                matches!(
                    op,
                    IlOp::Jump {
                        kind: IlJumpKind::JumpIfFalse,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(jmpf_count, 2, "JMPF ops kept; only S stripped");
        assert!(ops.iter().any(|op| matches!(op, IlOp::Return { .. })));
    }

    #[test]
    fn multi_op_join_convoy_sinks_identical_suffix_via_jmpt() {
        let suf = load_const_add_suffix();
        let mut ops = Vec::new();
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfTrue,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.extend(suf);
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfTrue,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::StorePop {
            slot: 2,
            loc: common::DebugLoc::unknown(),
        });

        multi_op_join_convoy(&mut ops);

        let load_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Load { .. }))
            .count();
        assert_eq!(load_count, 1);
        let store_idx = ops
            .iter()
            .position(|op| matches!(op, IlOp::StorePop { slot: 2, .. }))
            .expect("StorePop kept");
        let add_idx = ops
            .iter()
            .position(|op| matches!(op, IlOp::Bin { op: Instruction::ADD, .. }))
            .expect("ADD sunk");
        assert!(add_idx < store_idx);
    }

    #[test]
    fn multi_op_join_convoy_skips_disagreeing_jmpf_suffixes() {
        let mut ops = vec![
            IlOp::Load {
                slot: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Const {
                imm: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Load {
                slot: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Const {
                imm: 1,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Label(Label(0)),
            IlOp::Return {
                loc: common::DebugLoc::unknown(),
            },
        ];
        let before = ops.clone();
        multi_op_join_convoy(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn multi_op_join_convoy_skips_mixed_jmpf_jmp_unknown_sp() {
        // Identical S on both arms, but JMPF is −1 vs JMP 0 at the join → Unknown.
        let suf = load_const_add_suffix();
        let mut ops = Vec::new();
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfFalse,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.extend(suf);
        ops.push(IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::Return {
            loc: common::DebugLoc::unknown(),
        });
        let before = ops.clone();
        multi_op_join_convoy(&mut ops);
        assert!(ops == before);
    }

    fn load_not_const_add_suffix() -> Vec<IlOp> {
        // Net SP +1 (needed for sequential JMPF diamonds to agree at the join).
        vec![
            IlOp::Load {
                slot: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::byte(Byte::new(Instruction::NOT)),
            IlOp::Const {
                imm: 1,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: common::DebugLoc::unknown(),
            },
        ]
    }

    #[test]
    fn multi_op_join_convoy_prefers_longest_suffix_via_jmpf() {
        let suf = load_not_const_add_suffix();
        let mut ops = Vec::new();
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfFalse,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.extend(suf);
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfFalse,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::Return {
            loc: common::DebugLoc::unknown(),
        });

        multi_op_join_convoy(&mut ops);

        let load_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Load { .. }))
            .count();
        let not_count = ops
            .iter()
            .filter(|op| {
                matches!(
                    op.as_encode_byte().as_ref().map(|b| *b.bytecode()),
                    Some(Instruction::NOT)
                )
            })
            .count();
        let const_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Const { imm: 1, .. }))
            .count();
        assert_eq!(load_count, 1);
        assert_eq!(not_count, 1, "length-4 jump-pred template keeps NOT");
        assert_eq!(const_count, 1);
        let jmpf_count = ops
            .iter()
            .filter(|op| {
                matches!(
                    op,
                    IlOp::Jump {
                        kind: IlJumpKind::JumpIfFalse,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(jmpf_count, 2, "JMPFs must not be stripped by jump-pred rewrite");
    }

    #[test]
    fn multi_op_join_convoy_jump_pred_template_keeps_pre_join_ops() {
        // All-jump diamond: ops between last pred and join are not the suffix.
        let suf = load_const_add_suffix();
        let mut ops = Vec::new();
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfFalse,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.extend(suf);
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfFalse,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        // Net-zero pre-join junk — must survive (Load+StorePop, not part of S).
        ops.push(IlOp::Load {
            slot: 9,
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::StorePop {
            slot: 9,
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::Return {
            loc: common::DebugLoc::unknown(),
        });

        multi_op_join_convoy(&mut ops);

        let load9 = ops
            .iter()
            .position(|op| matches!(op, IlOp::Load { slot: 9, .. }))
            .expect("pre-join Load kept");
        let store9 = ops
            .iter()
            .position(|op| matches!(op, IlOp::StorePop { slot: 9, .. }))
            .expect("pre-join StorePop kept");
        let lab = ops
            .iter()
            .position(|op| matches!(op, IlOp::Label(Label(0))))
            .expect("join label");
        let add_idx = ops
            .iter()
            .position(|op| matches!(op, IlOp::Bin { op: Instruction::ADD, .. }))
            .expect("suffix sunk after join");
        assert!(load9 < store9 && store9 < lab && lab < add_idx);
        let sunk_loads = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Load { slot: 0, .. }))
            .count();
        assert_eq!(sunk_loads, 1);
    }

    #[test]
    fn multi_op_join_convoy_sinks_jmpf_through_label_cluster() {
        // Jump-pred template into a multi-label return cluster.
        let suf = load_const_add_suffix();
        let mut ops = Vec::new();
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfFalse,
            target: Label(54),
            loc: common::DebugLoc::unknown(),
        });
        ops.extend(suf);
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfFalse,
            target: Label(54),
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Label(Label(54)));
        ops.push(IlOp::Label(Label(48)));
        ops.push(IlOp::Return {
            loc: common::DebugLoc::unknown(),
        });

        multi_op_join_convoy(&mut ops);

        let load_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Load { .. }))
            .count();
        assert_eq!(load_count, 1);
        let lab54 = ops
            .iter()
            .position(|op| matches!(op, IlOp::Label(Label(54))))
            .expect("outer join");
        let lab48 = ops
            .iter()
            .position(|op| matches!(op, IlOp::Label(Label(48))))
            .expect("inner label");
        let add_idx = ops
            .iter()
            .position(|op| matches!(op, IlOp::Bin { op: Instruction::ADD, .. }))
            .expect("ADD sunk");
        assert!(lab54 < lab48 && lab48 < add_idx);
    }

    #[test]
    fn multi_op_join_convoy_sinks_identical_suffix_via_jump_if_match() {
        // Two arms: S; JumpIfMatch Ljoin — both −1, join SP Known; no fall-through.
        let suf = load_const_add_suffix();
        let mut ops = Vec::new();
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfMatch { tag: 0, arity: 1 },
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.extend(suf);
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfMatch { tag: 1, arity: 1 },
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::Return {
            loc: common::DebugLoc::unknown(),
        });

        multi_op_join_convoy(&mut ops);

        let load_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Load { .. }))
            .count();
        let add_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Bin { op: Instruction::ADD, .. }))
            .count();
        assert_eq!(load_count, 1, "suffix should appear once after join");
        assert_eq!(add_count, 1);
        let jim_count = ops
            .iter()
            .filter(|op| {
                matches!(
                    op,
                    IlOp::Jump {
                        kind: IlJumpKind::JumpIfMatch { .. },
                        ..
                    }
                )
            })
            .count();
        assert_eq!(jim_count, 2, "JumpIfMatch ops kept; only S stripped");
        assert!(ops.iter().any(|op| matches!(op, IlOp::Return { .. })));
    }

    #[test]
    fn multi_op_join_convoy_skips_disagreeing_jump_if_match_suffixes() {
        let mut ops = vec![
            IlOp::Load {
                slot: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Const {
                imm: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfMatch { tag: 0, arity: 1 },
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Load {
                slot: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Const {
                imm: 1,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfMatch { tag: 1, arity: 1 },
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Label(Label(0)),
            IlOp::Return {
                loc: common::DebugLoc::unknown(),
            },
        ];
        let before = ops.clone();
        multi_op_join_convoy(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn multi_op_join_convoy_prefers_longest_suffix_via_jump_if_match() {
        // Net-+1 len-4 suffix so sequential JumpIfMatch diamonds agree at join.
        let suf = load_not_const_add_suffix();
        let mut ops = Vec::new();
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfMatch { tag: 0, arity: 1 },
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.extend(suf);
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfMatch { tag: 1, arity: 1 },
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::Return {
            loc: common::DebugLoc::unknown(),
        });

        multi_op_join_convoy(&mut ops);

        let load_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Load { .. }))
            .count();
        let not_count = ops
            .iter()
            .filter(|op| {
                matches!(
                    op.as_encode_byte().as_ref().map(|b| *b.bytecode()),
                    Some(Instruction::NOT)
                )
            })
            .count();
        assert_eq!(load_count, 1);
        assert_eq!(not_count, 1, "length-4 JumpIfMatch template keeps NOT");
        let jim_count = ops
            .iter()
            .filter(|op| {
                matches!(
                    op,
                    IlOp::Jump {
                        kind: IlJumpKind::JumpIfMatch { .. },
                        ..
                    }
                )
            })
            .count();
        assert_eq!(jim_count, 2, "JumpIfMatch ops survive jump-pred rewrite");
    }

    #[test]
    fn multi_op_join_convoy_sinks_jump_if_match_through_label_cluster() {
        let suf = load_const_add_suffix();
        let mut ops = Vec::new();
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfMatch { tag: 0, arity: 1 },
            target: Label(54),
            loc: common::DebugLoc::unknown(),
        });
        ops.extend(suf);
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfMatch { tag: 1, arity: 1 },
            target: Label(54),
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Label(Label(54)));
        ops.push(IlOp::Label(Label(48)));
        ops.push(IlOp::Return {
            loc: common::DebugLoc::unknown(),
        });

        multi_op_join_convoy(&mut ops);

        let load_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Load { .. }))
            .count();
        assert_eq!(load_count, 1);
        let lab54 = ops
            .iter()
            .position(|op| matches!(op, IlOp::Label(Label(54))))
            .expect("outer join");
        let lab48 = ops
            .iter()
            .position(|op| matches!(op, IlOp::Label(Label(48))))
            .expect("inner label");
        let add_idx = ops
            .iter()
            .position(|op| matches!(op, IlOp::Bin { op: Instruction::ADD, .. }))
            .expect("ADD sunk");
        assert!(lab54 < lab48 && lab48 < add_idx);
    }

    #[test]
    fn multi_op_join_convoy_skips_mixed_jump_if_match_jmp_unknown_sp() {
        // JumpIfMatch (−1) + unconditional JMP (0) into same join → Unknown SP.
        let suf = load_const_add_suffix();
        let mut ops = Vec::new();
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfMatch { tag: 0, arity: 1 },
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.extend(suf);
        ops.push(IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::Return {
            loc: common::DebugLoc::unknown(),
        });
        let before = ops.clone();
        multi_op_join_convoy(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn multi_op_join_convoy_skips_unknown_join_sp() {
        // Identical suffixes, but then-arm pushes an extra const first so join
        // heights disagree → SP Unknown → refuse sink.
        let suf = load_const_add_suffix();
        let mut ops = vec![
            IlOp::Const {
                imm: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(1),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Const {
                imm: 99,
                loc: common::DebugLoc::unknown(),
            },
        ];
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Label(Label(1)));
        ops.extend(suf);
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::Return {
            loc: common::DebugLoc::unknown(),
        });
        let before = ops.clone();
        multi_op_join_convoy(&mut ops);
        assert!(ops == before);
    }

    fn load_two_const_add_suffix() -> Vec<IlOp> {
        vec![
            IlOp::Load {
                slot: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Const {
                imm: 1,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Const {
                imm: 2,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: common::DebugLoc::unknown(),
            },
        ]
    }

    #[test]
    fn multi_op_join_convoy_prefers_longest_suffix() {
        // Matching length-4 (and nested length-2/3) — sink the longest once.
        let suf = load_two_const_add_suffix();
        let mut ops = vec![
            IlOp::Const {
                imm: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(1),
                loc: common::DebugLoc::unknown(),
            },
        ];
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Label(Label(1)));
        ops.extend(suf);
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::Return {
            loc: common::DebugLoc::unknown(),
        });

        multi_op_join_convoy(&mut ops);

        let load_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Load { .. }))
            .count();
        let const_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Const { imm: 1 | 2, .. }))
            .count();
        let add_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Bin { op: Instruction::ADD, .. }))
            .count();
        assert_eq!(load_count, 1);
        assert_eq!(const_count, 2, "both consts from the length-4 suffix");
        assert_eq!(add_count, 1);
    }

    #[test]
    fn multi_op_join_convoy_sinks_length_two() {
        let suf = vec![
            IlOp::Load {
                slot: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Const {
                imm: 1,
                loc: common::DebugLoc::unknown(),
            },
        ];
        let mut ops = vec![
            IlOp::Const {
                imm: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(1),
                loc: common::DebugLoc::unknown(),
            },
        ];
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Label(Label(1)));
        ops.extend(suf);
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::Return {
            loc: common::DebugLoc::unknown(),
        });

        multi_op_join_convoy(&mut ops);

        let load_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Load { .. }))
            .count();
        assert_eq!(load_count, 1);
        assert!(ops.iter().any(|op| matches!(op, IlOp::Return { .. })));
    }

    #[test]
    fn multi_op_join_convoy_sinks_identical_suffix_non_return() {
        // Diamond into shared continuation (StorePop), not RETURN.
        let suf = load_const_add_suffix();
        let mut ops = vec![
            IlOp::Const {
                imm: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(1),
                loc: common::DebugLoc::unknown(),
            },
        ];
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Label(Label(1)));
        ops.extend(suf);
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::StorePop {
            slot: 2,
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Halt {
            loc: common::DebugLoc::unknown(),
        });

        multi_op_join_convoy(&mut ops);

        let load_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Load { .. }))
            .count();
        let add_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Bin { op: Instruction::ADD, .. }))
            .count();
        assert_eq!(load_count, 1, "suffix should appear once after join");
        assert_eq!(add_count, 1);
        // Suffix then StorePop: Load … ADD StorePop Halt
        let store_idx = ops
            .iter()
            .position(|op| matches!(op, IlOp::StorePop { slot: 2, .. }))
            .expect("StorePop kept");
        let add_idx = ops
            .iter()
            .position(|op| matches!(op, IlOp::Bin { op: Instruction::ADD, .. }))
            .expect("ADD sunk");
        assert!(add_idx < store_idx, "suffix before shared continuation");
        assert!(ops.iter().any(|op| matches!(op, IlOp::Halt { .. })));
    }

    #[test]
    fn multi_op_join_convoy_skips_disagreeing_suffixes_non_return() {
        let mut ops = vec![
            IlOp::Load {
                slot: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Const {
                imm: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Load {
                slot: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Const {
                imm: 1,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Label(Label(0)),
            IlOp::StorePop {
                slot: 2,
                loc: common::DebugLoc::unknown(),
            },
        ];
        let before = ops.clone();
        multi_op_join_convoy(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn multi_op_join_convoy_skips_jmpf_fallthrough_unknown_sp_non_return() {
        // JMPF + fall-through identical S: join SP Unknown → refuse (same as return).
        let suf = load_const_add_suffix();
        let mut ops = Vec::new();
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfFalse,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.extend(suf);
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::StorePop {
            slot: 2,
            loc: common::DebugLoc::unknown(),
        });
        let before = ops.clone();
        multi_op_join_convoy(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn multi_op_join_convoy_skips_jump_only_join() {
        // Labels followed only by unconditional JMP — no local work.
        let suf = load_const_add_suffix();
        let mut ops = Vec::new();
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.extend(suf);
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            target: Label(9),
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Label(Label(9)));
        ops.push(IlOp::Halt {
            loc: common::DebugLoc::unknown(),
        });
        let before = ops.clone();
        multi_op_join_convoy(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn multi_op_join_convoy_skips_halt_join_consumer() {
        // HALT after labels is a terminator — leave to dead_block, not NonReturn sink.
        let suf = load_const_add_suffix();
        let mut ops = Vec::new();
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.extend(suf);
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::Halt {
            loc: common::DebugLoc::unknown(),
        });
        let before = ops.clone();
        multi_op_join_convoy(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn multi_op_join_convoy_skips_fused_return_join_consumer() {
        // Fused *Return after labels must not be treated as a NonReturn continuation.
        let suf = load_const_add_suffix();
        let mut ops = Vec::new();
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.extend(suf);
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::BinReturn {
            op: Instruction::ADD,
            loc: common::DebugLoc::unknown(),
        });
        let before = ops.clone();
        multi_op_join_convoy(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn multi_op_join_convoy_sinks_identical_suffix_via_jump_if_match_non_return() {
        let suf = load_const_add_suffix();
        let mut ops = Vec::new();
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfMatch { tag: 0, arity: 1 },
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.extend(suf);
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfMatch { tag: 1, arity: 1 },
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::StorePop {
            slot: 2,
            loc: common::DebugLoc::unknown(),
        });

        multi_op_join_convoy(&mut ops);

        let load_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Load { .. }))
            .count();
        assert_eq!(load_count, 1);
        let store_idx = ops
            .iter()
            .position(|op| matches!(op, IlOp::StorePop { slot: 2, .. }))
            .expect("StorePop kept");
        let add_idx = ops
            .iter()
            .position(|op| matches!(op, IlOp::Bin { op: Instruction::ADD, .. }))
            .expect("ADD sunk");
        assert!(add_idx < store_idx);
    }

    #[test]
    fn multi_op_join_convoy_skips_unknown_join_sp_non_return() {
        // Identical suffixes, mismatched arm heights → Unknown join SP → refuse.
        let suf = load_const_add_suffix();
        let mut ops = vec![
            IlOp::Const {
                imm: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(1),
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Const {
                imm: 99,
                loc: common::DebugLoc::unknown(),
            },
        ];
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Label(Label(1)));
        ops.extend(suf);
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::StorePop {
            slot: 2,
            loc: common::DebugLoc::unknown(),
        });
        let before = ops.clone();
        multi_op_join_convoy(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn multi_op_join_convoy_skips_without_jump_preds() {
        // Fall-through into a labeled continuation with no JMP preds — refuse.
        let suf = load_const_add_suffix();
        let mut ops = Vec::new();
        ops.extend(suf);
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::StorePop {
            slot: 2,
            loc: common::DebugLoc::unknown(),
        });
        let before = ops.clone();
        multi_op_join_convoy(&mut ops);
        assert!(ops == before);
    }

    #[test]
    fn multi_op_join_convoy_sinks_through_label_cluster_non_return() {
        // Multi-label join (JMPF diamond so both arms have known SP): sink after cluster.
        let suf = load_const_add_suffix();
        let mut ops = vec![
            IlOp::Const {
                imm: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(1),
                loc: common::DebugLoc::unknown(),
            },
        ];
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            target: Label(54),
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Label(Label(1)));
        ops.extend(suf);
        ops.push(IlOp::Label(Label(54)));
        ops.push(IlOp::Label(Label(48)));
        ops.push(IlOp::StorePop {
            slot: 2,
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Halt {
            loc: common::DebugLoc::unknown(),
        });

        multi_op_join_convoy(&mut ops);

        let load_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Load { .. }))
            .count();
        assert_eq!(load_count, 1, "suffix should appear once after cluster");
        assert!(ops.iter().any(|op| matches!(op, IlOp::Label(Label(54)))));
        assert!(ops.iter().any(|op| matches!(op, IlOp::Label(Label(48)))));
        let lab48 = ops
            .iter()
            .position(|op| matches!(op, IlOp::Label(Label(48))))
            .expect("inner label");
        let add_idx = ops
            .iter()
            .position(|op| matches!(op, IlOp::Bin { op: Instruction::ADD, .. }))
            .expect("ADD sunk");
        let store_idx = ops
            .iter()
            .position(|op| matches!(op, IlOp::StorePop { slot: 2, .. }))
            .expect("StorePop");
        assert!(lab48 < add_idx && add_idx < store_idx);
    }

    #[test]
    fn multi_op_join_convoy_prefers_longest_suffix_non_return() {
        let suf = load_two_const_add_suffix();
        let mut ops = vec![
            IlOp::Const {
                imm: 0,
                loc: common::DebugLoc::unknown(),
            },
            IlOp::Jump {
                kind: IlJumpKind::JumpIfFalse,
                target: Label(1),
                loc: common::DebugLoc::unknown(),
            },
        ];
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            target: Label(0),
            loc: common::DebugLoc::unknown(),
        });
        ops.push(IlOp::Label(Label(1)));
        ops.extend(suf);
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::StorePop {
            slot: 2,
            loc: common::DebugLoc::unknown(),
        });

        multi_op_join_convoy(&mut ops);

        let load_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Load { .. }))
            .count();
        let const_count = ops
            .iter()
            .filter(|op| matches!(op, IlOp::Const { imm: 1 | 2, .. }))
            .count();
        assert_eq!(load_count, 1);
        assert_eq!(const_count, 2, "length-4 suffix keeps both consts once");
        let add_idx = ops
            .iter()
            .position(|op| matches!(op, IlOp::Bin { op: Instruction::ADD, .. }))
            .expect("ADD");
        let store_idx = ops
            .iter()
            .position(|op| matches!(op, IlOp::StorePop { slot: 2, .. }))
            .expect("StorePop");
        assert!(add_idx < store_idx);
    }
}
