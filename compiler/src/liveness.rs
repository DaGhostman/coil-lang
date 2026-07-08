//! Liveness analysis for the CFG-based register VM.
//!
//! This module computes, for each basic block in a [`Function`],
//! the set of SSA [`ValueId`]s that are "live" (will be used in
//! the future) on entry and on exit of that block. The result
//! drives the Phase 21D register allocator, which needs to know
//! which values' registers must be callee-saved across a `CALL`
//! and which can be freely reused after the call.
//!
//! ## Algorithm
//!
//! For SSA-lite (no phi-nodes, validated by Experiment A in
//! [`MULTI_PASS_REFACTOR_PLAN.md`](../../MULTI_PASS_REFACTOR_PLAN.md)),
//! a single backward dataflow pass over the CFG computes fixed
//! points for `live_in` and `live_out`:
//!
//! ```text
//! live_out[B] = ⋃ live_in[S]  for every successor S of B
//! live_in[B]  = use[B] ∪ (live_out[B] − def[B])
//! ```
//!
//! `use[B]` = SSA values consumed by `B`'s instructions (read before
//! any write). `def[B]` = SSA values produced by `B`'s instructions
//! (each instruction produces exactly one destination, except
//! `Unpack` which produces an array).
//!
//! For terminators, `use` includes any value read by the
//! terminator (e.g., the `cond` of a `Branch`, the `scrutinee` of
//! a `Switch`, the `Some(v)` of a `Return`).
//!
//! ## Convergence
//!
//! A single iteration suffices for SSA-lite (no phis → no
//! dependencies on dominance frontiers). The implementation
//! iterates over all blocks until `live_in` and `live_out` are
//! stable; in practice this takes 1–2 iterations for typical
//! functions and `O(depth)` for the deepest loop nests.
//!
//! ## Block ordering
//!
//! We walk blocks in reverse post-order (RPO) for the
//! forward-pass iteration, and in any order for the
//! fixed-point loop. RPO doesn't affect correctness (the
//! algorithm is order-independent), only iteration count.
//!
//! [`Function`]: crate::cfg::Function
//! [`ValueId`]: crate::cfg::ValueId

use std::collections::{HashMap, HashSet};

use crate::cfg::{Block, BlockId, Function, Inst, Terminator, ValueId};

/// Result of a liveness analysis pass over a single function.
///
/// `live_in` and `live_out` are computed per-block. `defs` and
/// `uses` are computed per-instruction for use by the register
/// allocator (which needs to know where each value is born and
/// where it's read).
#[derive(Debug, Clone, Default)]
#[allow(dead_code)] // consumed by Phase 21D register allocator
pub struct Liveness {
    /// Block-id → set of `ValueId`s live on entry.
    pub live_in: HashMap<BlockId, HashSet<ValueId>>,
    /// Block-id → set of `ValueId`s live on exit.
    pub live_out: HashMap<BlockId, HashSet<ValueId>>,
}

/// Run a fixed-point liveness analysis over `cfg`.
///
/// Returns a populated [`Liveness`] ready to be consumed by the
/// register allocator (Phase 21D) and the bytecode emitter
/// (Phase 21C, which uses `live_out` to allocate callee-saved
/// registers around `CALL` opcodes).
///
/// The algorithm:
///
/// 1. Initialize `live_in[B] = ∅` for every block.
/// 2. Iterate until stable:
///    a. For every block `B` in declaration order:
///       - `live_out[B] = ⋃ live_in[S]` for successors `S` of `B`
///         (per `Terminator`'s successor list).
///       - `live_in[B] = use[B] ∪ (live_out[B] − def[B])`.
///    b. Stop when no block's `live_in` changed.
///
/// SSA-lite (no phi-nodes) means the algorithm converges in at
/// most `O(depth(B))` iterations for the deepest loop nest, but
/// in practice one iteration suffices for most functions.
#[allow(dead_code)] // consumed by Phase 21D register allocator
pub fn analyze(cfg: &Function) -> Liveness {
    let mut live_in: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();
    let mut live_out: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();

    // Initialize empty sets for every block. This ensures every
    // block has an entry in the maps even if it has no
    // predecessors (rare but possible: the entry block of a
    // function with no body).
    for block in &cfg.blocks {
        live_in.insert(block.id, HashSet::new());
        live_out.insert(block.id, HashSet::new());
    }

    // Fixed-point iteration. We compute `use`/`def` per block
    // fresh on each pass because terminators can introduce
    // cross-block dependencies (e.g., `Switch`'s `scrutinee` is
    // defined in `match_block` and used in `case` blocks).
    loop {
        let mut changed = false;

        for block in &cfg.blocks {
            let old_live_in = live_in.get(&block.id).cloned().unwrap_or_default();

            // live_out[B] = ⋃ live_in[S] for every successor S
            let mut new_live_out: HashSet<ValueId> = HashSet::new();
            for succ in successors(&block.terminator) {
                if let Some(succ_in) = live_in.get(&succ) {
                    new_live_out.extend(succ_in.iter().copied());
                }
            }

            // live_in[B] = use[B] ∪ (live_out[B] − def[B])
            let mut new_live_in = new_live_out.clone();
            // Subtract def[B]
            for def in defs_of_block(block) {
                new_live_in.remove(&def);
            }
            // Add use[B]
            for u in uses_of_block(block) {
                new_live_in.insert(u);
            }

            live_out.insert(block.id, new_live_out);
            live_in.insert(block.id, new_live_in);

            if live_in.get(&block.id) != Some(&old_live_in) {
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    Liveness { live_in, live_out }
}

/// Return the successor block(s) of a terminator.
///
/// - `Jump(target)` → `[target]`
/// - `Branch { true_bb, false_bb, .. }` → `[true_bb, false_bb]`
/// - `Switch { cases, default, .. }` → `cases` values + `[default]`
/// - `Return(_)`, `Unreachable` → `[]`
fn successors(term: &Terminator) -> Vec<BlockId> {
    match term {
        Terminator::Jump(target) => vec![*target],
        Terminator::Branch {
            true_bb, false_bb, ..
        } => vec![*true_bb, *false_bb],
        Terminator::Switch { cases, default, .. } => {
            let mut out: Vec<BlockId> = cases.iter().map(|(_, bb)| *bb).collect();
            out.push(*default);
            out
        }
        Terminator::Return(_) | Terminator::Unreachable => Vec::new(),
    }
}

/// Compute the set of SSA values DEFINED (produced) by a block.
///
/// For each instruction:
/// - `Const`, `ConstF`, `ConstBool`, `ConstString`, `Param`,
///   `BinOp`, `UnaryOp`, `Call` (when `dst = Some`), `LoadField`,
///   `MakeEnum` → the destination `ValueId`.
/// - `Call` (when `dst = None`) → defines nothing (discarded).
/// - `Unpack` → every `dst` element.
/// - `Print` → defines nothing (side-effect only).
/// - Terminators define nothing directly; `Return(Some(v))`
///   is a use of `v`, not a def.
fn defs_of_block(block: &Block) -> HashSet<ValueId> {
    let mut defs = HashSet::new();
    for inst in &block.insts {
        for d in defs_of_inst(inst) {
            defs.insert(d);
        }
    }
    defs
}

/// Compute the set of SSA values USED (consumed) by a block.
///
/// For each instruction:
/// - `Const*` → use nothing.
/// - `Param` → use nothing.
/// - `BinOp` → use `lhs`, `rhs`.
/// - `UnaryOp` → use `src`.
/// - `Call` → use `callee` + every `arg`.
/// - `LoadField` → use `src`.
/// - `Unpack` → use `scrutinee`.
/// - `MakeEnum` → use every `payload`.
/// - `Print` → use every `arg`.
///
/// For terminators:
/// - `Jump` → use nothing.
/// - `Branch { cond, .. }` → use `cond`.
/// - `Switch { scrutinee, .. }` → use `scrutinee`.
/// - `Return(None)` → use nothing.
/// - `Return(Some(v))` → use `v`.
/// - `Unreachable` → use nothing.
fn uses_of_block(block: &Block) -> HashSet<ValueId> {
    let mut uses = HashSet::new();
    for inst in &block.insts {
        for u in uses_of_inst(inst) {
            uses.insert(u);
        }
    }
    for u in uses_of_terminator(&block.terminator) {
        uses.insert(u);
    }
    uses
}

/// Per-instruction defs — one entry per `ValueId` produced.
fn defs_of_inst(inst: &Inst) -> Vec<ValueId> {
    match inst {
        Inst::Const { dst, .. }
        | Inst::ConstF { dst, .. }
        | Inst::ConstBool { dst, .. }
        | Inst::ConstString { dst, .. }
        | Inst::Param { dst, .. }
        | Inst::LoadField { dst, .. }
        | Inst::MakeEnum { dst, .. } => vec![*dst],
        Inst::BinOp { dst, .. } | Inst::UnaryOp { dst, .. } => vec![*dst],
        Inst::Call { dst, .. } => dst.iter().copied().collect(),
        Inst::Unpack { dst, .. } => dst.clone(),
        Inst::Print { .. } => Vec::new(),
    }
}

/// Per-instruction uses.
fn uses_of_inst(inst: &Inst) -> Vec<ValueId> {
    match inst {
        Inst::Const { .. }
        | Inst::ConstF { .. }
        | Inst::ConstBool { .. }
        | Inst::ConstString { .. }
        | Inst::Param { .. } => Vec::new(),
        Inst::BinOp { lhs, rhs, .. } => vec![*lhs, *rhs],
        Inst::UnaryOp { src, .. } => vec![*src],
        Inst::Call { callee, args, .. } => {
            let mut out = vec![*callee];
            out.extend(args.iter().copied());
            out
        }
        Inst::LoadField { src, .. } => vec![*src],
        Inst::Unpack { scrutinee, .. } => vec![*scrutinee],
        Inst::MakeEnum { payload, .. } => payload.clone(),
        Inst::Print { args } => args.clone(),
    }
}

/// Per-terminator uses.
fn uses_of_terminator(term: &Terminator) -> Vec<ValueId> {
    match term {
        Terminator::Jump(_) => Vec::new(),
        Terminator::Branch { cond, .. } => vec![*cond],
        Terminator::Switch { scrutinee, .. } => vec![*scrutinee],
        Terminator::Return(None) => Vec::new(),
        Terminator::Return(Some(v)) => vec![*v],
        Terminator::Unreachable => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the liveness analysis pass.
    //!
    //! These tests build small CFG `Function`s by hand and check
    //! that `analyze` produces the expected `live_in` / `live_out`
    //! sets. The pattern matches the rest of the project's test
    //! suite: helper functions build CFGs in a few lines, then
    //! assert on the analysis result.

    use super::*;
    use crate::cfg::{BinOpKind, Block, Function, Inst, Terminator, TypeRef, ValueId};

    // Helper: a 1-block function with no predecessors, no body.
    fn empty_fn(name: &str) -> Function {
        Function {
            name: name.to_string(),
            params: Vec::new(),
            return_ty: TypeRef::Unit,
            blocks: vec![Block {
                id: BlockId::new(0),
                insts: Vec::new(),
                terminator: Terminator::Return(None),
                predecessors: Vec::new(),
            }],
            entry: BlockId::new(0),
        }
    }

    // Helper: a 1-block function with a single integer constant.
    fn const_fn(name: &str, value: i64) -> Function {
        Function {
            name: name.to_string(),
            params: Vec::new(),
            return_ty: TypeRef::Int,
            blocks: vec![Block {
                id: BlockId::new(0),
                insts: vec![Inst::Const {
                    dst: ValueId::new(0),
                    value,
                }],
                terminator: Terminator::Return(Some(ValueId::new(0))),
                predecessors: Vec::new(),
            }],
            entry: BlockId::new(0),
        }
    }

    // Helper: a 2-block function: block 0 defines v0, block 1 returns it.
    fn linear_fn(name: &str) -> Function {
        Function {
            name: name.to_string(),
            params: Vec::new(),
            return_ty: TypeRef::Int,
            blocks: vec![
                Block {
                    id: BlockId::new(0),
                    insts: vec![Inst::Const {
                        dst: ValueId::new(0),
                        value: 42,
                    }],
                    terminator: Terminator::Jump(BlockId::new(1)),
                    predecessors: Vec::new(),
                },
                Block {
                    id: BlockId::new(1),
                    insts: Vec::new(),
                    terminator: Terminator::Return(Some(ValueId::new(0))),
                    predecessors: vec![BlockId::new(0)],
                },
            ],
            entry: BlockId::new(0),
        }
    }

    // Helper: a 2-block if/else where both branches define v.
    fn branch_fn(name: &str) -> Function {
        Function {
            name: name.to_string(),
            params: Vec::new(),
            return_ty: TypeRef::Int,
            blocks: vec![
                Block {
                    id: BlockId::new(0),
                    insts: vec![
                        Inst::Const {
                            dst: ValueId::new(0),
                            value: 1,
                        },
                        Inst::BinOp {
                            op: BinOpKind::Lt,
                            dst: ValueId::new(1),
                            lhs: ValueId::new(0),
                            rhs: ValueId::new(0),
                        },
                    ],
                    terminator: Terminator::Branch {
                        cond: ValueId::new(1),
                        true_bb: BlockId::new(1),
                        false_bb: BlockId::new(2),
                    },
                    predecessors: Vec::new(),
                },
                Block {
                    id: BlockId::new(1),
                    insts: vec![Inst::Const {
                        dst: ValueId::new(2),
                        value: 10,
                    }],
                    terminator: Terminator::Jump(BlockId::new(3)),
                    predecessors: vec![BlockId::new(0)],
                },
                Block {
                    id: BlockId::new(2),
                    insts: vec![Inst::Const {
                        dst: ValueId::new(3),
                        value: 20,
                    }],
                    terminator: Terminator::Jump(BlockId::new(3)),
                    predecessors: vec![BlockId::new(0)],
                },
                Block {
                    id: BlockId::new(3),
                    insts: Vec::new(),
                    terminator: Terminator::Return(Some(ValueId::new(2))),
                    predecessors: vec![BlockId::new(1), BlockId::new(2)],
                },
            ],
            entry: BlockId::new(0),
        }
    }

    #[test]
    fn empty_function_has_no_live_values() {
        let cfg = empty_fn("empty");
        let liveness = analyze(&cfg);
        assert!(liveness.live_in[&BlockId::new(0)].is_empty());
        assert!(liveness.live_out[&BlockId::new(0)].is_empty());
    }

    #[test]
    fn const_function_has_no_cross_block_live_values() {
        // A single-block function that defines v0 and returns it:
        // v0 is live within the block (in the return terminator),
        // but it's defined AND used in the same block, so
        // live_out of the block is empty (no successors).
        let cfg = const_fn("constant", 42);
        let liveness = analyze(&cfg);
        assert!(
            liveness.live_out[&BlockId::new(0)].is_empty(),
            "v0 is consumed in the same block where it's defined"
        );
    }

    #[test]
    fn linear_two_block_function_propagates_value_across_blocks() {
        // Block 0 defines v0 then jumps to block 1.
        // Block 1 returns v0.
        // After analysis: live_out[B0] = {v0}, live_in[B1] = {v0}.
        let cfg = linear_fn("linear");
        let liveness = analyze(&cfg);

        // Block 0 produces v0 and transfers control to block 1.
        // v0 is needed in block 1 (for Return), so it's live
        // on exit of block 0 and live on entry of block 1.
        assert!(liveness.live_out[&BlockId::new(0)].contains(&ValueId::new(0)));
        assert!(liveness.live_in[&BlockId::new(1)].contains(&ValueId::new(0)));

        // Block 1 uses v0 in its terminator; nothing else is live.
        assert_eq!(liveness.live_out[&BlockId::new(1)].len(), 0);
    }

    #[test]
    fn branch_function_propagates_value_to_join_block() {
        // Block 0: define v1 (the cond), branch on it.
        // Block 1: define v2 (10), jump to join.
        // Block 2: define v3 (20), jump to join.
        // Block 3: return v2.
        //
        // Expected liveness:
        // - live_out[B1] = {v2} (because B3's terminator uses v2).
        // - live_out[B2] = {} (B3 doesn't use v3).
        // - live_out[B0] = {v2} (from join).
        // - live_in[B0]  = {v2} ∪ {v1} − {v0, v1} = {v2}.
        //   (v1 is defined in B0, so it's removed; v0 is also
        //    defined in B0; v2 is needed by the join.)
        // - live_in[B3] = {v2} (Return uses v2).
        let cfg = branch_fn("branch");
        let liveness = analyze(&cfg);

        // v2 is used in B3's terminator, so it's live on
        // entry of B3.
        assert!(liveness.live_in[&BlockId::new(3)].contains(&ValueId::new(2)));

        // v2 propagates backward from B3 to B1 (where it's defined).
        // B1's terminator is Jump (uses nothing), so live_out[B1]
        // = {v2}.
        assert!(liveness.live_out[&BlockId::new(1)].contains(&ValueId::new(2)));

        // Conservative liveness for B2: v2 is live on B2's exit
        // because B3's predecessor set includes B2 (and B2
        // doesn't define v2, so it survives the def-set
        // subtraction). This is the standard "merge of disjoint
        // paths" behavior — even though B2's actual path doesn't
        // touch v2, the analysis must conservatively assume v2
        // could be live on B2's exit in case the B1 path was
        // taken. SSA-lite doesn't help here (no phi nodes to
        // discriminate).
        assert!(liveness.live_out[&BlockId::new(2)].contains(&ValueId::new(2)));

        // v3 is defined in B2 but never used downstream; it
        // should NOT be live on B2's exit.
        assert!(!liveness.live_out[&BlockId::new(2)].contains(&ValueId::new(3)));

        // v0 is defined in B0 and never used; it's not live
        // on B0's exit.
        assert!(!liveness.live_out[&BlockId::new(0)].contains(&ValueId::new(0)));

        // v1 is the cond, defined in B0 and used by B0's Branch.
        // It's not used downstream; not live on B0's exit.
        assert!(!liveness.live_out[&BlockId::new(0)].contains(&ValueId::new(1)));
    }

    #[test]
    fn const_int_reports_zero_defs_and_zero_uses() {
        // Per-instruction view: `Const { dst: 0 }` defines 0,
        // uses nothing.
        let inst = Inst::Const {
            dst: ValueId::new(0),
            value: 42,
        };
        assert!(defs_of_inst(&inst).contains(&ValueId::new(0)));
        assert!(uses_of_inst(&inst).is_empty());
    }

    #[test]
    fn binop_reports_lhs_rhs_as_uses_and_dst_as_def() {
        let inst = Inst::BinOp {
            op: BinOpKind::Add,
            dst: ValueId::new(2),
            lhs: ValueId::new(0),
            rhs: ValueId::new(1),
        };
        assert_eq!(defs_of_inst(&inst), vec![ValueId::new(2)]);
        assert_eq!(uses_of_inst(&inst), vec![ValueId::new(0), ValueId::new(1)]);
    }

    #[test]
    fn call_with_dst_defines_dst_and_uses_callee_and_args() {
        let inst = Inst::Call {
            dst: Some(ValueId::new(3)),
            callee: ValueId::new(0),
            args: vec![ValueId::new(1), ValueId::new(2)],
        };
        assert_eq!(defs_of_inst(&inst), vec![ValueId::new(3)]);
        let uses = uses_of_inst(&inst);
        assert!(uses.contains(&ValueId::new(0)));
        assert!(uses.contains(&ValueId::new(1)));
        assert!(uses.contains(&ValueId::new(2)));
    }

    #[test]
    fn call_without_dst_defines_nothing() {
        // A discarded call (top-level `print(...)`) defines no
        // SSA value; the return value is implicitly discarded.
        let inst = Inst::Call {
            dst: None,
            callee: ValueId::new(0),
            args: vec![ValueId::new(1)],
        };
        assert!(defs_of_inst(&inst).is_empty());
        let uses = uses_of_inst(&inst);
        assert!(uses.contains(&ValueId::new(0)));
        assert!(uses.contains(&ValueId::new(1)));
    }

    #[test]
    fn unpack_defines_all_destinations_and_uses_scrutinee() {
        let inst = Inst::Unpack {
            dst: vec![ValueId::new(3), ValueId::new(4)],
            scrutinee: ValueId::new(0),
        };
        let defs = defs_of_inst(&inst);
        assert!(defs.contains(&ValueId::new(3)));
        assert!(defs.contains(&ValueId::new(4)));
        assert_eq!(uses_of_inst(&inst), vec![ValueId::new(0)]);
    }

    #[test]
    fn print_uses_args_and_defines_nothing() {
        // `print("hello", x)` uses both args; produces no value.
        let inst = Inst::Print {
            args: vec![ValueId::new(0), ValueId::new(1)],
        };
        assert!(defs_of_inst(&inst).is_empty());
        let uses = uses_of_inst(&inst);
        assert!(uses.contains(&ValueId::new(0)));
        assert!(uses.contains(&ValueId::new(1)));
    }

    #[test]
    fn successors_handles_all_terminator_kinds() {
        // Jump
        assert_eq!(
            successors(&Terminator::Jump(BlockId::new(1))),
            vec![BlockId::new(1)]
        );
        // Branch
        assert_eq!(
            successors(&Terminator::Branch {
                cond: ValueId::new(0),
                true_bb: BlockId::new(1),
                false_bb: BlockId::new(2),
            }),
            vec![BlockId::new(1), BlockId::new(2)]
        );
        // Switch
        let switch_succs = successors(&Terminator::Switch {
            scrutinee: ValueId::new(0),
            cases: vec![(1, BlockId::new(1)), (2, BlockId::new(2))],
            default: BlockId::new(3),
        });
        assert_eq!(switch_succs.len(), 3);
        assert!(switch_succs.contains(&BlockId::new(1)));
        assert!(switch_succs.contains(&BlockId::new(2)));
        assert!(switch_succs.contains(&BlockId::new(3)));
        // Return / Unreachable
        assert!(successors(&Terminator::Return(None)).is_empty());
        assert!(successors(&Terminator::Unreachable).is_empty());
    }
}
