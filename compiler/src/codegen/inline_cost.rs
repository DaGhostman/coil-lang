//! Cost-based inlining policy (COI-124).
//!
//! Shape safety stays in [`super::compiler::Compiler::is_tiny_inline_il`]; this
//! module decides *whether* a structurally eligible body is worth copying.
//! Coil has no `#[inline]` / `#[no_inline]` attrs yet; [`CallInfo`] carries
//! those bits for when they exist and for tests.

use crate::il::{EntryKind, IlOp, Label};
use common::Instruction;

/// Thresholds for [`should_inline_function`].
#[derive(Clone, Debug)]
pub struct InlineCostOptions {
    /// Default budget for a call site without profile data.
    pub max_inline_cost: usize,
    /// Tighter budget when [`CallInfo::hot`] (PGO / future).
    pub hot_call_threshold: usize,
    /// Relaxed budget when [`CallInfo::force_inline`].
    pub cold_call_threshold: usize,
}

impl Default for InlineCostOptions {
    fn default() -> Self {
        Self {
            max_inline_cost: 100,
            hot_call_threshold: 50,
            cold_call_threshold: 200,
        }
    }
}

/// Call-site facts for the inliner.
#[derive(Clone, Debug, Default)]
pub struct CallInfo {
    pub recursive: bool,
    pub hot: bool,
    /// Language `#[inline]` once it exists.
    pub force_inline: bool,
    /// Language `#[no_inline]` once it exists.
    pub no_inline: bool,
}

/// Weighted size of a callee body. Calls cost more than arithmetic.
pub fn estimate_inline_cost(ops: &[IlOp]) -> usize {
    let mut cost = 0usize;
    for op in ops {
        if !op.emits_code() {
            continue;
        }
        cost = cost.saturating_add(op_weight(op));
    }
    cost
}

fn op_weight(op: &IlOp) -> usize {
    if is_callish(op) {
        25
    } else {
        1
    }
}

fn is_callish(op: &IlOp) -> bool {
    matches!(
        op,
        IlOp::Entry {
            kind: EntryKind::Call | EntryKind::TailCall | EntryKind::MakeCoro,
            ..
        } | IlOp::HostInvoke { .. }
    ) || matches!(
        op.as_encode_byte(),
        Some(b) if matches!(
            *b.bytecode(),
            Instruction::CALL
                | Instruction::TailCall
                | Instruction::MakeCoro
                | Instruction::HostInvoke
                | Instruction::CallIndirect
        )
    )
}

/// Cost + policy gate. Recursion and `no_inline` always refuse.
pub fn should_inline_function(cost: usize, call: &CallInfo, opts: &InlineCostOptions) -> bool {
    if call.no_inline {
        return false;
    }
    if call.recursive {
        return false;
    }
    let limit = if call.force_inline {
        opts.cold_call_threshold
    } else if call.hot {
        opts.hot_call_threshold
    } else {
        opts.max_inline_cost
    };
    cost <= limit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::il::{EntryKind, IlOp, Label};
    use common::DebugLoc;

    fn loc() -> DebugLoc {
        DebugLoc::unknown()
    }

    #[test]
    fn estimate_counts_calls_heavier() {
        let arith = vec![
            IlOp::Load { slot: 0, loc: loc() },
            IlOp::Const { imm: 1, loc: loc() },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: loc(),
            },
        ];
        assert_eq!(estimate_inline_cost(&arith), 3);
        let call = vec![IlOp::Entry {
            kind: EntryKind::Call,
            target: Label(0),
            arity: 0,
            loc: loc(),
        }];
        assert_eq!(estimate_inline_cost(&call), 25);
    }

    #[test]
    fn should_inline_respects_cost_and_flags() {
        let opts = InlineCostOptions::default();
        let mut call = CallInfo::default();
        assert!(should_inline_function(3, &call, &opts));
        assert!(!should_inline_function(101, &call, &opts));
        call.recursive = true;
        assert!(!should_inline_function(3, &call, &opts));
        call.recursive = false;
        call.no_inline = true;
        assert!(!should_inline_function(3, &call, &opts));
        call.no_inline = false;
        call.force_inline = true;
        assert!(should_inline_function(150, &call, &opts));
        assert!(!should_inline_function(201, &call, &opts));
        call.force_inline = false;
        call.hot = true;
        assert!(!should_inline_function(60, &call, &opts));
        assert!(should_inline_function(40, &call, &opts));
    }
}
