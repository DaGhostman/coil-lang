//! Per-function IL module view over the flat [`super::CodeBuf`] stream.
//!
//! Splits recorded [`super::IlFunc`] emitting spans into owned bodies for
//! scoped opts / GVN, then concatenates back. Prologue and inter-function
//! glue stay outside function bodies.

use super::func::IlFunc;
use super::op::IlOp;
use super::opt::{self, OptimizeOptions};

/// One function's owned IL ops (labels inclusive at span edges).
#[derive(Clone)]
pub struct IlFuncBody {
    #[allow(dead_code)] // retained for tooling / IlModule consumers
    pub meta: IlFunc,
    pub ops: Vec<IlOp>,
}

/// Flat stream partitioned into prologue, function bodies, and glue.
#[derive(Clone, Default)]
pub struct IlModule {
    pub prologue: Vec<IlOp>,
    pub funcs: Vec<IlFuncBody>,
    /// Ops after the last function (and any between-func gaps folded in order).
    /// Between-func glue is stored in [`IlFuncBody`] order via [`Self::glue`].
    pub glue: Vec<Vec<IlOp>>,
    pub epilogue: Vec<IlOp>,
}

impl IlModule {
    /// Split a flat op buffer using emitting spans from `funcs`.
    ///
    /// `glue[i]` is the gap after `funcs[i]` (before the next func or epilogue).
    /// Empty `funcs` yields the whole buffer as prologue.
    pub fn from_flat(ops: &[IlOp], funcs: &[IlFunc]) -> Self {
        if funcs.is_empty() {
            return Self {
                prologue: ops.to_vec(),
                funcs: Vec::new(),
                glue: Vec::new(),
                epilogue: Vec::new(),
            };
        }

        let mut ranges: Vec<(usize, usize, usize)> = funcs
            .iter()
            .enumerate()
            .filter(|(_, f)| f.code_start < f.code_end)
            .map(|(i, f)| {
                let (s, e) = opt::emitting_range_to_raw(ops, f.code_start, f.code_end);
                (i, s, e)
            })
            .filter(|(_, s, e)| s < e)
            .collect();
        ranges.sort_by_key(|&(_, s, _)| s);

        let mut module = Self::default();
        let mut cursor = 0usize;
        for (fi, raw_start, raw_end) in &ranges {
            if cursor < *raw_start {
                let gap = ops[cursor..*raw_start].to_vec();
                if module.funcs.is_empty() {
                    module.prologue = gap;
                } else {
                    module.glue.push(gap);
                }
            } else if !module.funcs.is_empty() {
                module.glue.push(Vec::new());
            }
            module.funcs.push(IlFuncBody {
                meta: funcs[*fi].clone(),
                ops: ops[*raw_start..*raw_end].to_vec(),
            });
            cursor = *raw_end;
        }
        while module.glue.len() + 1 < module.funcs.len() {
            module.glue.push(Vec::new());
        }
        if cursor < ops.len() {
            module.epilogue = ops[cursor..].to_vec();
        }
        module
    }

    /// Concatenate prologue / bodies / glue / epilogue into one op stream.
    pub fn to_flat(&self) -> Vec<IlOp> {
        let mut out = Vec::new();
        out.extend(self.prologue.iter().cloned());
        for (i, body) in self.funcs.iter().enumerate() {
            out.extend(body.ops.iter().cloned());
            if let Some(g) = self.glue.get(i) {
                out.extend(g.iter().cloned());
            }
        }
        out.extend(self.epilogue.iter().cloned());
        out
    }

    /// Per-func opts (excluding multi_op) + CFG GVN on each body, then
    /// whole-buffer [`opt::multi_op_join_convoy`] on the concatenated stream.
    pub fn optimize_and_flatten(&mut self, opts: &OptimizeOptions) -> Vec<IlOp> {
        let mut per = opts.clone();
        let run_multi = per.multi_op_join_convoy;
        per.multi_op_join_convoy = false;

        if self.funcs.is_empty() {
            let mut ops = self.to_flat();
            opt::optimize(&mut ops, opts);
            return ops;
        }

        for body in &mut self.funcs {
            opt::optimize(&mut body.ops, &per);
            super::gvn::cfg_gvn(&mut body.ops);
        }

        let mut flat = self.to_flat();
        if run_multi {
            opt::multi_op_join_convoy(&mut flat);
        }
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::il::op::{IlJumpKind, IlOp, Label};
    use common::{Byte, DebugLoc, Instruction};

    fn loc() -> DebugLoc {
        DebugLoc::unknown()
    }

    fn load_const_add_suffix() -> Vec<IlOp> {
        vec![
            IlOp::Load {
                slot: 0,
                loc: loc(),
            },
            IlOp::Const {
                imm: 1,
                loc: loc(),
            },
            IlOp::Bin {
                op: Instruction::ADD,
                loc: loc(),
            },
        ]
    }

    #[test]
    fn from_flat_splits_prologue_body_epilogue() {
        let ops = vec![
            IlOp::Const {
                imm: 1,
                loc: loc(),
            },
            IlOp::Pop { loc: loc() },
            IlOp::Const {
                imm: 2,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
            IlOp::Halt { loc: loc() },
        ];
        let funcs = vec![IlFunc::new("f", None, 2, 4)];
        let m = IlModule::from_flat(&ops, &funcs);
        assert_eq!(m.prologue.len(), 2);
        assert_eq!(m.funcs.len(), 1);
        assert_eq!(m.funcs[0].ops.len(), 2);
        assert_eq!(m.epilogue.len(), 1);
        assert_eq!(m.to_flat().len(), ops.len());
    }

    #[test]
    fn from_flat_preserves_inter_func_glue() {
        // prologue | f0 body | glue | f1 body | epilogue
        let ops = vec![
            IlOp::Const {
                imm: 0,
                loc: loc(),
            },
            IlOp::Const {
                imm: 1,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
            IlOp::Dup { loc: loc() },
            IlOp::Pop { loc: loc() },
            IlOp::Const {
                imm: 2,
                loc: loc(),
            },
            IlOp::Return { loc: loc() },
            IlOp::Halt { loc: loc() },
        ];
        let funcs = vec![
            IlFunc::new("a", None, 1, 3),
            IlFunc::new("b", None, 5, 7),
        ];
        let m = IlModule::from_flat(&ops, &funcs);
        assert_eq!(m.prologue.len(), 1);
        assert_eq!(m.funcs.len(), 2);
        assert_eq!(m.glue.len(), 1);
        assert_eq!(m.glue[0].len(), 2);
        assert!(matches!(m.glue[0][0], IlOp::Dup { .. }));
        assert_eq!(m.epilogue.len(), 1);
        assert_eq!(m.to_flat().len(), ops.len());
    }

    #[test]
    fn empty_funcs_optimizes_whole_buffer() {
        let mut m = IlModule {
            prologue: vec![
                IlOp::Dup { loc: loc() },
                IlOp::Pop { loc: loc() },
                IlOp::Const {
                    imm: 1,
                    loc: loc(),
                },
                IlOp::Return { loc: loc() },
            ],
            ..IlModule::default()
        };
        let flat = m.optimize_and_flatten(&OptimizeOptions::default());
        assert!(!flat.iter().any(|op| matches!(op, IlOp::Dup { .. })));
        assert!(flat.iter().any(|op| matches!(op, IlOp::ConstReturnImm { .. })
            || matches!(op, IlOp::Return { .. })));
    }

    #[test]
    fn optimize_and_flatten_dces_body_only() {
        let ops = vec![
            IlOp::Dup { loc: loc() },
            IlOp::Pop { loc: loc() },
            IlOp::Const {
                imm: 1,
                loc: loc(),
            },
            IlOp::Dup { loc: loc() },
            IlOp::Pop { loc: loc() },
            IlOp::Return { loc: loc() },
        ];
        let funcs = vec![IlFunc::new("f", None, 2, 6)];
        let mut m = IlModule::from_flat(&ops, &funcs);
        let flat = m.optimize_and_flatten(&OptimizeOptions {
            multi_op_join_convoy: false,
            ..OptimizeOptions::default()
        });
        assert!(matches!(flat[0], IlOp::Dup { .. }));
        assert!(matches!(flat[1], IlOp::Pop { .. }));
        assert!(!flat[2..].iter().any(|op| matches!(op, IlOp::Dup { .. })));
        let _ = IlJumpKind::Unconditional;
        let _ = Label(0);
    }

    #[test]
    fn multi_op_on_full_buffer_refuses_when_prologue_poisons_sp() {
        // PRINT in prologue Unknown-poisons SP. Scoped multi_op on the body
        // alone would see Known SP and sink; whole-buffer multi_op must refuse.
        let suf = load_const_add_suffix();
        let mut ops = vec![IlOp::byte(Byte::new(Instruction::PRINT))];
        let body_start = ops.len();
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfFalse,
            target: Label(0),
            loc: loc(),
        });
        ops.extend(suf);
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfFalse,
            target: Label(0),
            loc: loc(),
        });
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::Return { loc: loc() });
        let body_emit_end = ops.iter().filter(|op| op.emits_code()).count();

        // Confirm scoped multi_op would incorrectly sink.
        let mut body_only: Vec<IlOp> = ops[body_start..].to_vec();
        opt::multi_op_join_convoy(&mut body_only);
        let scoped_loads = body_only
            .iter()
            .filter(|op| matches!(op, IlOp::Load { .. }))
            .count();
        assert_eq!(
            scoped_loads, 1,
            "precondition: body-only multi_op sinks (Known SP)"
        );

        let funcs = vec![IlFunc::new("f", None, 1, body_emit_end)];
        let mut m = IlModule::from_flat(&ops, &funcs);
        let flat = m.optimize_and_flatten(&OptimizeOptions::default());
        let loads = flat
            .iter()
            .filter(|op| matches!(op, IlOp::Load { .. }))
            .count();
        assert_eq!(
            loads, 2,
            "full-buffer multi_op must refuse when prologue poisons SP"
        );
    }

    #[test]
    fn multi_op_on_full_buffer_still_sinks_clean_body() {
        let suf = load_const_add_suffix();
        let mut ops = Vec::new();
        ops.extend(suf.clone());
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfFalse,
            target: Label(0),
            loc: loc(),
        });
        ops.extend(suf);
        ops.push(IlOp::Jump {
            kind: IlJumpKind::JumpIfFalse,
            target: Label(0),
            loc: loc(),
        });
        ops.push(IlOp::Label(Label(0)));
        ops.push(IlOp::Return { loc: loc() });
        let emit_end = ops.iter().filter(|op| op.emits_code()).count();
        let funcs = vec![IlFunc::new("f", None, 0, emit_end)];
        let mut m = IlModule::from_flat(&ops, &funcs);
        let flat = m.optimize_and_flatten(&OptimizeOptions::default());
        let loads = flat
            .iter()
            .filter(|op| matches!(op, IlOp::Load { .. }))
            .count();
        assert_eq!(loads, 1, "clean body must still sink via whole-buffer multi_op");
    }
}
