//! IL optimization passes unlocked by symbolic labels.



use super::op::IlOp;

/// Options for [`optimize`].
#[derive(Clone)]
pub struct OptimizeOptions {
    /// Collapse `JMP L` where `L` begins with `JMP L2` into `JMP L2`.
    pub jump_thread: bool,
    /// Remove unreachable ops after unconditional JMP / RETURN until a label.
    pub dead_block: bool,
    /// Drop redundant `DUPLICATE; POP` and `LOAD s; StorePop s`.
    pub stack_dce: bool,
    /// `StorePop s; Load s` → `Dup; StorePop s`; dead-store elimination.
    pub mem_fwd: bool,
    /// Algebraic / strength peeps (x+0, x*1, cmp fold, …) when SP Known.
    pub algebraic: bool,
    /// Hoist invariant Const/Load out of Known-SP natural loops.
    pub licm: bool,
    /// Sink identical `LOAD`/`CONST` producers into a join `RETURN` and fuse.
    pub return_convoy: bool,
    /// Clone plain `RETURN` onto jump-only preds of mixed return joins.
    pub clone_shared_return: bool,
    /// Sink identical binop / BinSlot* tails into a return-label cluster.
    pub bin_join_convoy: bool,
    /// Sink identical multi-op suffixes (len 2..=4) at return / non-return joins.
    pub multi_op_join_convoy: bool,
}

impl Default for OptimizeOptions {
    fn default() -> Self {
        Self {
            jump_thread: true,
            dead_block: true,
            stack_dce: true,
            mem_fwd: true,
            algebraic: true,
            licm: true,
            return_convoy: true,
            clone_shared_return: true,
            bin_join_convoy: true,
            multi_op_join_convoy: true,
        }
    }
}

/// Run IL opts in place. Safe to call before [`super::lower`].
pub fn optimize(ops: &mut Vec<IlOp>, opts: &OptimizeOptions) {
    optimize_at(ops, opts, 0);
}

/// Like [`optimize`], seeding SP analysis at `entry_sp` for the op buffer.
pub fn optimize_at(ops: &mut Vec<IlOp>, opts: &OptimizeOptions, entry_sp: i32) {
    if opts.jump_thread {
        jump_thread(ops);
    }
    if opts.dead_block {
        eliminate_dead_blocks(ops);
    }
    if opts.stack_dce {
        stack_dce(ops);
    }
    if opts.mem_fwd {
        mem_fwd(ops, entry_sp);
        dead_store(ops);
    }
    if opts.algebraic {
        super::algebraic::algebraic_simplify(ops);
    }
    if opts.licm {
        // LICM still seeds at 0; entry_sp plumbing is mem_fwd-critical today.
        let _ = entry_sp;
        super::licm::licm(ops);
    }
    if opts.clone_shared_return {
        clone_shared_return(ops);
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

/// Run [`optimize`] on each [`super::IlFunc`] emitting span; leave prologue and
/// inter-function glue untouched. Falls back to whole-buffer opts when `funcs`
/// is empty (unit tests / buffers without `record_func`).
///
/// Thin flat-buffer wrapper over [`super::IlModule::optimize_and_flatten`].
/// Production lower uses [`super::lower_module`] on an owning module; this
/// stays for unit tests that mutate a bare `Vec<IlOp>`.
///
/// Whole-buffer [`multi_op_join_convoy`] is required: scoped multi_op can treat
/// JMPF/fall-through diamonds as SP-known and mis-sink (e.g. `examples/fib.hy`).
#[allow(dead_code)]
pub fn optimize_per_func(ops: &mut Vec<IlOp>, funcs: &[super::IlFunc], opts: &OptimizeOptions) {
    if funcs.is_empty() {
        optimize(ops, opts);
        return;
    }

    let mut module = super::IlModule::from_flat(ops, funcs);
    *ops = module.optimize_and_flatten(opts);
}

/// Map inclusive-exclusive emitting indices to a raw op range, including
/// leading labels bound at `emit_start`.
pub(crate) fn emitting_range_to_raw(
    ops: &[IlOp],
    emit_start: usize,
    emit_end: usize,
) -> (usize, usize) {
    let mut emitting = 0usize;
    let mut raw_start: Option<usize> = None;
    let mut raw_end: Option<usize> = None;
    for (i, op) in ops.iter().enumerate() {
        if emitting == emit_start && raw_start.is_none() {
            let mut s = i;
            while s > 0 && !ops[s - 1].emits_code() {
                s -= 1;
            }
            raw_start = Some(s);
        }
        if !op.emits_code() {
            continue;
        }
        emitting += 1;
        if emitting == emit_end {
            raw_end = Some(i + 1);
            break;
        }
    }
    (
        raw_start.unwrap_or(0),
        raw_end.unwrap_or_else(|| {
            // emit_end past buffer: take through end once start was found.
            if raw_start.is_some() { ops.len() } else { 0 }
        }),
    )
}

mod cfg;
mod convoy;
mod dce;

use cfg::{eliminate_dead_blocks, jump_thread};
use dce::{dead_store, mem_fwd, stack_dce};
use convoy::{bin_join_convoy, clone_shared_return, return_convoy};
pub(crate) use convoy::multi_op_join_convoy;
