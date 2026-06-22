//! Linearizer: convert a CFG `Function` into stack-based bytecode.
//!
//! Phase 0.3a scope: straight-line functions only — single block,
//! sequential instructions. Multi-block CFGs (control flow) are
//! deferred to Phase 1 (the linearizer panics on multi-block
//! functions for now).
//!
//! See [`MULTI_PASS_REFACTOR_PLAN.md`](../MULTI_PASS_REFACTOR_PLAN.md)
//! §3 for the high-level design.
//!
//! # What this linearizer does
//!
//! Walks a CFG [`Function`] block-by-block and emits one
//! [`common::Byte`] per instruction. The emitted bytecode is the
//! existing stack-based form (no register allocator yet — that's
//! Phase 3).
//!
//! # What this linearizer does NOT do
//!
//! - **SSA value tracking.** The linearizer does NOT track which
//!   stack slot holds which SSA [`ValueId`]. For straight-line
//!   code, this works because each value is produced and
//!   immediately consumed in source order (the stack-top
//!   invariant). Phase 0.4 will discover what breaks in this
//!   simplification and fix it incrementally.
//!
//! - **Multi-block linearization.** Panics if `cfg.blocks.len() > 1`.
//!   Control-flow terminators (`Jump`, `Branch`, `Switch`) also
//!   panic. Phase 1 will add the multi-block pass.
//!
//! - **Call target resolution.** Function call targets are
//!   resolved by the existing pipeline (see `compiler/src/lib.rs`),
//!   not here. The linearizer emits a `JMP u32::MAX` placeholder
//!   that the upstream patch step fills in.
//!
//! # Operand conventions
//!
//! [`common::Byte`] has two operand fields:
//! - `operands: u32` — small immediates (slot offsets, arities,
//!   tags). Set via [`common::Byte::with_operand_u32`].
//! - `value: u64` — full-width immediates (i64/f64 constants).
//!   Set via [`common::Byte::new_with_value`].
//!
//! Constants (`Inst::Const`, `Inst::ConstF`, `Inst::ConstBool`)
//! use `value`; everything else uses `operands`.

use common::{Byte, Instruction, Value};

use crate::cfg::{
    BinOpKind, Function, Inst, Terminator, UnaryOpKind,
};

/// Linearize a CFG [`Function`] into stack-based bytecode.
///
/// # Phase 0.3a scope
///
/// - **Single-block functions only.** Multi-block CFGs panic
///   (Phase 1 will add the multi-block linearization).
/// - **Sequential instruction emission.** SSA values are NOT
///   tracked — the linearizer assumes each value is at the
///   expected stack position (stack-top invariant for
///   straight-line code).
/// - **Call target is a placeholder.** `JMP u32::MAX`; the
///   upstream pipeline patches it after linearization.
///
/// # Returns
///
/// A `Vec<Byte>` of bytecode instructions. The vector is empty
/// for an empty block (an unreachable terminator with no
/// instructions).
///
/// `#[allow(dead_code)]` — this module is wired into the
/// pipeline in Phase 0.4; until then the compiler can't see
/// external callers, and Rust's dead-code lint would otherwise
/// flag the entry point and its helpers (same pattern as
/// `cfg_builder`).
#[allow(dead_code)]
pub fn linearize(cfg: &Function) -> Vec<Byte> {
    assert_eq!(
        cfg.blocks.len(),
        1,
        "Phase 0.3a: multi-block CFG not yet supported. \
         Function `{}` has {} blocks; control flow is Phase 1.",
        cfg.name,
        cfg.blocks.len()
    );

    let block = &cfg.blocks[0];
    let mut bytecode = Vec::new();

    // Emit instructions in order.
    for inst in &block.insts {
        emit_inst(inst, &mut bytecode);
    }

    // Emit terminator.
    emit_terminator(&block.terminator, &mut bytecode);

    bytecode
}

/// Emit a single CFG [`Inst`] as one or more bytecode instructions.
///
/// `#[allow(dead_code)]` — see `linearize` for the rationale.
#[allow(dead_code)]
fn emit_inst(inst: &Inst, bc: &mut Vec<Byte>) {
    match inst {
        Inst::Const { dst: _, value } => {
            // CONST with full 64-bit immediate. Round-trip via
            // `Value::from` so the encoding matches the existing
            // codegen path (which uses `Value::from(*num).raw() as _`).
            bc.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(*value).raw() as u64,
            ));
        }
        Inst::ConstF { dst: _, value } => {
            bc.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(*value).raw() as u64,
            ));
        }
        Inst::ConstBool { dst: _, value } => {
            bc.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(*value).raw() as u64,
            ));
        }
        Inst::ConstString { dst: _, value } => {
            // STRING + DATA sequence: emit one DATA byte per char
            // first, then insert STRING with the char count at the
            // front. Matches the existing `Expression::String`
            // codegen (see `compiler/src/lib.rs` ~line 1827).
            //
            // NOTE: this is a placeholder for Phase 0.3a. The real
            // string emission includes UTF-8 encoding and heap
            // allocation via `INIT`. The existing codegen handles
            // that; for Phase 0.4 we'll decide whether the
            // linearizer needs to emit the same allocation sequence
            // or whether the upstream pipeline rewrites the
            // linearized form to add the allocation.
            let chars: Vec<char> = value.chars().collect();
            let count = chars.len() as u32;
            for c in chars {
                bc.push(Byte::new(Instruction::DATA).with_operand_u32(c as u32));
            }
            bc.push(Byte::new(Instruction::STRING).with_operand_u32(count));
        }
        Inst::Param { dst: _, index } => {
            // LOAD reads from `stack[frame.sp + operand]`. The
            // parameter at source position `index` lives at
            // `stack[frame.sp + index]`, so the operand is the
            // index directly.
            bc.push(Byte::new(Instruction::LOAD).with_operand_u32(*index as u32));
        }
        Inst::BinOp { op, dst: _, lhs: _, rhs: _ } => {
            // The operands `lhs` and `rhs` are SSA [`ValueId`]s;
            // for straight-line code the values are still on the
            // stack at the expected positions (stack-top invariant).
            // The linearizer does NOT track SSA values (Phase 1+).
            // We just emit the operation.
            bc.push(map_binop(*op));
        }
        Inst::UnaryOp { op, dst: _, src: _ } => {
            bc.push(map_unaryop(*op));
        }
        Inst::Call { dst, callee: _, args } => {
            // CALL arity + JMP target. The target is a placeholder
            // (u32::MAX) — the upstream pipeline patches it after
            // linearization (see `compiler/src/lib.rs` for the
            // existing patch path).
            //
            // CALL pushes the result (if any) onto the stack; we
            // don't emit anything for the result — the VM does it.
            bc.push(Byte::new(Instruction::CALL).with_operand_u32(args.len() as u32));
            bc.push(Byte::new(Instruction::JMP).with_operand_u32(u32::MAX));
            // The `dst` SSA value lives on the stack after CALL
            // returns. The linearizer doesn't track it (Phase 1+).
            let _ = dst;
        }
        Inst::LoadField {
            dst: _,
            src: _,
            field_index,
        } => {
            // LoadField pops the receiver (an `Object::Enum`) and
            // pushes `payload[field_index]`. The receiver is
            // assumed to be at the stack top (stack-top invariant).
            //
            // Layout (see `common/src/opcode.rs`):
            //   operands[15:0]  = field_index
            //   operands[31:16] = reserved
            bc.push(
                Byte::new(Instruction::LoadField).with_operand_u32(*field_index as u32),
            );
        }
        Inst::Unpack { dst: _, scrutinee: _ } => {
            // UNPACK pops the scrutinee and pushes the payload
            // values. The arity is `dst.len()`.
            //
            // For Phase 0.3a, the linearizer only emits this for
            // single-block functions where the unpack is followed
            // by a RETURN — so the actual UNPACK instruction is
            // not strictly necessary (we could just return the
            // scrutinee directly). We emit it for fidelity with
            // the spec; the resulting bytecode is a no-op in the
            // simplest cases.
            //
            // Phase 1 will wire this properly for multi-block
            // match arms.
            bc.push(Byte::new(Instruction::Unpack).with_operand_u32(0));
        }
        Inst::MakeEnum {
            dst: _,
            tag,
            payload,
        } => {
            // MAKE_ENUM pops `payload.len()` values and pushes the
            // enum. The values are at the stack top in REVERSE
            // declaration order (per the codegen convention — see
            // Phase 15C's reverse-emit decision).
            //
            // Layout (see `common/src/opcode.rs`):
            //   operands[31:16] = tag
            //   operands[15:0]  = arity
            let arity = (payload.len() as u32) & 0xFFFF;
            let tag_shifted = (*tag & 0xFFFF) << 16;
            bc.push(
                Byte::new(Instruction::MakeEnum).with_operand_u32(tag_shifted | arity),
            );
        }
    }
}

/// Emit a CFG [`Terminator`] as one or more bytecode instructions.
///
/// `#[allow(dead_code)]` — see `linearize` for the rationale.
#[allow(dead_code)]
fn emit_terminator(term: &Terminator, bc: &mut Vec<Byte>) {
    match term {
        Terminator::Return(None) => {
            // Unit return — nothing on the stack.
            bc.push(Byte::new(Instruction::RETURN));
        }
        Terminator::Return(Some(_)) => {
            // Value return — the value should already be on the
            // stack from the prior instruction (stack-top
            // invariant). RETURN pops it and returns it.
            bc.push(Byte::new(Instruction::RETURN));
        }
        Terminator::Unreachable => {
            // `HALT` is the canonical "unreachable" terminator
            // (matches the prologue pattern in
            // `compiler/src/lib.rs::Default for Compiler`).
            bc.push(Byte::new(Instruction::HALT));
        }
        // Multi-block terminators: panic for Phase 0.3a. Phase 1
        // will wire Jump/Branch/Switch into the multi-block
        // linearization pass.
        other => panic!(
            "Phase 0.3a: control-flow terminator `{}` not yet supported \
             (only Return and Unreachable are allowed in single-block \
             functions; control flow is Phase 1)",
            other
        ),
    }
}

/// Map a CFG [`BinOpKind`] to the corresponding stack-based
/// [`Instruction`].
///
/// **Known gaps in the existing VM** (no opcode exists for):
/// - `EqF` / `NeqF` — float equality / inequality
///
/// These panics honestly rather than silently producing wrong
/// bytecode. The existing AST/codegen never produces these
/// variants, so the linearizer is also not expected to see them
/// in Phase 0.4. Adding them is a Phase 3+ VM task.
///
/// `#[allow(dead_code)]` — see `linearize` for the rationale.
#[allow(dead_code)]
fn map_binop(op: BinOpKind) -> Byte {
    use BinOpKind::*;
    let i = match op {
        // Integer arithmetic.
        Add => Instruction::ADD,
        Sub => Instruction::SUB,
        Mul => Instruction::MUL,
        Div => Instruction::DIV,
        Mod => Instruction::MOD,
        // Float arithmetic.
        AddF => Instruction::ADDF,
        SubF => Instruction::SUBF,
        MulF => Instruction::MULF,
        DivF => Instruction::DIVF,
        ModF => Instruction::MODF,
        // Integer comparison.
        Eq => Instruction::EQ,
        Neq => Instruction::NEQ,
        Lt => Instruction::LE,
        Le => Instruction::LEQ,
        Gt => Instruction::GT,
        Ge => Instruction::GEQ,
        // Float comparison — partial. The VM lacks `EQF` and
        // `NEQF` opcodes; only the relational forms exist.
        LtF => Instruction::LEF,
        LeF => Instruction::LEQF,
        GtF => Instruction::GTF,
        GeF => Instruction::GEQF,
        // Logical.
        And => Instruction::AND,
        Or => Instruction::OR,
        // Bitwise.
        Shl => Instruction::SHL,
        Shr => Instruction::SHR,
        Xor => Instruction::XOR,
        // VM gap: float equality / inequality. The existing VM
        // doesn't have `EQF` / `NEQF` opcodes (see
        // `common/src/opcode.rs`). Panic honestly.
        EqF | NeqF => panic!(
            "Phase 0.3a linearizer: float `{}` has no VM opcode target \
             (the existing VM lacks EQF/NEQF). This is a known VM gap; \
             adding the opcode is Phase 3+.",
            op
        ),
    };
    Byte::new(i)
}

/// Map a CFG [`UnaryOpKind`] to the corresponding stack-based
/// [`Instruction`].
///
/// **Known gap in the existing VM**: there's no `NEGF` opcode —
/// only integer `NEG`. The existing AST/codegen doesn't produce
/// `NegF`, so the linearizer also doesn't expect to see it in
/// Phase 0.4. Adding the opcode is a Phase 3+ VM task.
///
/// `#[allow(dead_code)]` — see `linearize` for the rationale.
#[allow(dead_code)]
fn map_unaryop(op: UnaryOpKind) -> Byte {
    use UnaryOpKind::*;
    let i = match op {
        Neg => Instruction::NEG,
        Not => Instruction::NOT,
        NegF => panic!(
            "Phase 0.3a linearizer: float negation `-f` has no VM \
             opcode target (the existing VM lacks NEGF). This is a \
             known VM gap; adding the opcode is Phase 3+."
        ),
    };
    Byte::new(i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::{Block, BlockId, Function, TypeRef, ValueId};

    /// Build a single-block function with no instructions and a
    /// `Return(None)` terminator. Used as the basis for the
    /// straight-line tests.
    fn fn_returning_unit(name: &str) -> Function {
        let block = Block::new(BlockId(0)).with_terminator(Terminator::Return(None));
        Function {
            name: name.to_string(),
            params: vec![],
            return_ty: TypeRef::Unit,
            blocks: vec![block],
            entry: BlockId(0),
        }
    }

    /// Build a single-block function with a single integer constant
    /// and a `Return` terminator. The const goes to `dst`, the
    /// return points at `dst`.
    fn fn_returning_const(name: &str, value: i64) -> Function {
        let dst = ValueId(0);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::Const { dst, value });
        block.terminator = Terminator::Return(Some(dst));
        Function {
            name: name.to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![block],
            entry: BlockId(0),
        }
    }

    // ============================================================
    // Top-level linearize
    // ============================================================

    #[test]
    fn linearize_empty_block_emits_only_return() {
        let f = fn_returning_unit("empty");
        let bc = linearize(&f);
        assert_eq!(bc.len(), 1, "expected one byte (RETURN), got {:?}", bc);
        assert!(matches!(bc[0].bytecode(), Instruction::RETURN));
    }

    #[test]
    #[should_panic(expected = "multi-block CFG not yet supported")]
    fn linearize_multi_block_function_panics() {
        // Two blocks — dispatch + return. Phase 0.3a must panic.
        let dst = ValueId(0);
        let mut b0 = Block::new(BlockId(0));
        b0.insts.push(Inst::Const { dst, value: 1 });
        b0.terminator = Terminator::Jump(BlockId(1));
        let b1 = Block::new(BlockId(1)).with_terminator(Terminator::Return(Some(dst)));
        let f = Function {
            name: "multi".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![b0, b1],
            entry: BlockId(0),
        };
        let _ = linearize(&f);
    }

    // ============================================================
    // Constants
    // ============================================================

    #[test]
    fn linearize_const_int_emits_const_with_value() {
        let f = fn_returning_const("one", 42);
        let bc = linearize(&f);
        assert_eq!(bc.len(), 2);
        assert!(matches!(bc[0].bytecode(), Instruction::CONST));
        assert_eq!(bc[0].value_u32(), 42, "value field carries the int");
        assert!(matches!(bc[1].bytecode(), Instruction::RETURN));
    }

    #[test]
    fn linearize_const_negative_int_emits_const_with_value() {
        let f = fn_returning_const("neg", -7);
        let bc = linearize(&f);
        assert_eq!(bc.len(), 2);
        assert!(matches!(bc[0].bytecode(), Instruction::CONST));
        // -7 as u32 (two's complement) is 0xFFFFFFF9.
        assert_eq!(bc[0].value_u32(), -7_i32 as u32);
    }

    #[test]
    fn linearize_const_bool_true_emits_const_one() {
        let dst = ValueId(0);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::ConstBool { dst, value: true });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "t".to_string(),
            params: vec![],
            return_ty: TypeRef::Bool,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f);
        assert_eq!(bc.len(), 2);
        assert!(matches!(bc[0].bytecode(), Instruction::CONST));
        assert_eq!(bc[0].value_u32(), 1);
    }

    #[test]
    fn linearize_const_bool_false_emits_const_zero() {
        let dst = ValueId(0);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::ConstBool { dst, value: false });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Bool,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f);
        assert_eq!(bc.len(), 2);
        assert_eq!(bc[0].value_u32(), 0);
    }

    #[test]
    fn linearize_const_float_emits_const_with_bits() {
        let dst = ValueId(0);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::ConstF { dst, value: 1.5_f64 });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "half".to_string(),
            params: vec![],
            return_ty: TypeRef::Float,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f);
        assert_eq!(bc.len(), 2);
        assert!(matches!(bc[0].bytecode(), Instruction::CONST));
        // The value field carries the f64 bits.
        assert_eq!(bc[0].value_u32(), 1.5_f64.to_bits() as u32);
    }

    #[test]
    fn linearize_const_string_emits_string_then_data() {
        let dst = ValueId(0);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::ConstString {
            dst,
            value: "abc".to_string(),
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "s".to_string(),
            params: vec![],
            return_ty: TypeRef::String,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f);
        // 3 DATA bytes + 1 STRING byte + 1 RETURN = 5.
        assert_eq!(bc.len(), 5);
        // First three are DATA, last of the prefix is STRING.
        for (i, byte) in bc.iter().take(3).enumerate() {
            assert!(
                matches!(byte.bytecode(), Instruction::DATA),
                "byte {} should be DATA, got {:?}",
                i,
                byte.bytecode()
            );
        }
        assert!(matches!(bc[3].bytecode(), Instruction::STRING));
        assert_eq!(bc[3].operand_u32(), 3, "STRING operand is char count");
        assert!(matches!(bc[4].bytecode(), Instruction::RETURN));
    }

    // ============================================================
    // Param
    // ============================================================

    #[test]
    fn linearize_param_emits_load_with_slot_index() {
        let dst = ValueId(0);
        let param_vid = ValueId(1);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::Param { dst, index: 2 });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![(param_vid, "x".to_string())],
            return_ty: TypeRef::Int,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f);
        assert_eq!(bc.len(), 2);
        assert!(matches!(bc[0].bytecode(), Instruction::LOAD));
        assert_eq!(bc[0].operand_u32(), 2, "LOAD operand is the slot index");
    }

    // ============================================================
    // BinOps (sample — full mapping is exhaustively tested below)
    // ============================================================

    #[test]
    fn linearize_binop_add_emits_add() {
        let dst = ValueId(2);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::BinOp {
            op: BinOpKind::Add,
            dst,
            lhs: ValueId(0),
            rhs: ValueId(1),
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "add".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f);
        assert_eq!(bc.len(), 2);
        assert!(matches!(bc[0].bytecode(), Instruction::ADD));
    }

    #[test]
    fn linearize_binop_all_supported_variants_map_correctly() {
        // Every BinOpKind with a VM opcode target. `EqF` and
        // `NeqF` are excluded — the VM has no EQF/NEQF opcodes;
        // see `linearize_binop_eqf_panics` below.
        let cases: &[(BinOpKind, Instruction)] = &[
            (BinOpKind::Add, Instruction::ADD),
            (BinOpKind::Sub, Instruction::SUB),
            (BinOpKind::Mul, Instruction::MUL),
            (BinOpKind::Div, Instruction::DIV),
            (BinOpKind::Mod, Instruction::MOD),
            (BinOpKind::AddF, Instruction::ADDF),
            (BinOpKind::SubF, Instruction::SUBF),
            (BinOpKind::MulF, Instruction::MULF),
            (BinOpKind::DivF, Instruction::DIVF),
            (BinOpKind::ModF, Instruction::MODF),
            (BinOpKind::Eq, Instruction::EQ),
            (BinOpKind::Neq, Instruction::NEQ),
            (BinOpKind::Lt, Instruction::LE),
            (BinOpKind::Le, Instruction::LEQ),
            (BinOpKind::Gt, Instruction::GT),
            (BinOpKind::Ge, Instruction::GEQ),
            (BinOpKind::LtF, Instruction::LEF),
            (BinOpKind::LeF, Instruction::LEQF),
            (BinOpKind::GtF, Instruction::GTF),
            (BinOpKind::GeF, Instruction::GEQF),
            (BinOpKind::And, Instruction::AND),
            (BinOpKind::Or, Instruction::OR),
            (BinOpKind::Shl, Instruction::SHL),
            (BinOpKind::Shr, Instruction::SHR),
            (BinOpKind::Xor, Instruction::XOR),
        ];
        assert_eq!(cases.len(), 25, "every supported BinOpKind must be mapped");

        for (op, expected) in cases {
            let dst = ValueId(2);
            let mut block = Block::new(BlockId(0));
            block.insts.push(Inst::BinOp {
                op: *op,
                dst,
                lhs: ValueId(0),
                rhs: ValueId(1),
            });
            block.terminator = Terminator::Return(Some(dst));
            let f = Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: TypeRef::Int,
                blocks: vec![block],
                entry: BlockId(0),
            };
            let bc = linearize(&f);
            assert_eq!(bc.len(), 2);
            assert_eq!(
                bc[0].bytecode(),
                &expected.clone(),
                "BinOpKind::{:?} should emit {:?}",
                op,
                expected
            );
        }
    }

    #[test]
    #[should_panic(expected = "float `==f` has no VM opcode target")]
    fn linearize_binop_eqf_panics() {
        let dst = ValueId(2);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::BinOp {
            op: BinOpKind::EqF,
            dst,
            lhs: ValueId(0),
            rhs: ValueId(1),
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let _ = linearize(&f);
    }

    #[test]
    #[should_panic(expected = "float `!=f` has no VM opcode target")]
    fn linearize_binop_neqf_panics() {
        let dst = ValueId(2);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::BinOp {
            op: BinOpKind::NeqF,
            dst,
            lhs: ValueId(0),
            rhs: ValueId(1),
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let _ = linearize(&f);
    }

    // ============================================================
    // UnaryOps
    // ============================================================

    #[test]
    fn linearize_unaryop_supported_variants_map_correctly() {
        // `NegF` is excluded — the VM has no NEGF opcode;
        // see `linearize_unaryop_negf_panics` below.
        let cases: &[(UnaryOpKind, Instruction)] = &[
            (UnaryOpKind::Neg, Instruction::NEG),
            (UnaryOpKind::Not, Instruction::NOT),
        ];
        assert_eq!(cases.len(), 2, "supported UnaryOpKind variants");

        for (op, expected) in cases {
            let dst = ValueId(1);
            let mut block = Block::new(BlockId(0));
            block.insts.push(Inst::UnaryOp {
                op: *op,
                dst,
                src: ValueId(0),
            });
            block.terminator = Terminator::Return(Some(dst));
            let f = Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: TypeRef::Int,
                blocks: vec![block],
                entry: BlockId(0),
            };
            let bc = linearize(&f);
            assert_eq!(bc.len(), 2);
            assert_eq!(
                bc[0].bytecode(),
                &expected.clone(),
                "UnaryOpKind::{:?} should emit {:?}",
                op,
                expected
            );
        }
    }

    #[test]
    #[should_panic(expected = "float negation `-f` has no VM opcode target")]
    fn linearize_unaryop_negf_panics() {
        let dst = ValueId(1);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::UnaryOp {
            op: UnaryOpKind::NegF,
            dst,
            src: ValueId(0),
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let _ = linearize(&f);
    }

    // ============================================================
    // Call
    // ============================================================

    #[test]
    fn linearize_call_emits_call_then_jmp_placeholder() {
        let callee = ValueId(0);
        let dst = ValueId(3);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::Call {
            dst: Some(dst),
            callee,
            args: vec![ValueId(1), ValueId(2)],
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f);
        assert_eq!(bc.len(), 3, "CALL + JMP + RETURN");
        assert!(matches!(bc[0].bytecode(), Instruction::CALL));
        assert_eq!(bc[0].operand_u32(), 2, "CALL operand is the arity");
        assert!(matches!(bc[1].bytecode(), Instruction::JMP));
        assert_eq!(bc[1].operand_u32(), u32::MAX, "JMP target is a placeholder");
        assert!(matches!(bc[2].bytecode(), Instruction::RETURN));
    }

    #[test]
    fn linearize_call_with_no_args_emits_call_with_zero_arity() {
        let callee = ValueId(0);
        let dst = ValueId(1);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::Call {
            dst: Some(dst),
            callee,
            args: vec![],
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Unit,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f);
        assert_eq!(bc[0].operand_u32(), 0, "CALL operand is the arity (0)");
    }

    // ============================================================
    // LoadField
    // ============================================================

    #[test]
    fn linearize_load_field_emits_load_field_with_field_index() {
        let dst = ValueId(1);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::LoadField {
            dst,
            src: ValueId(0),
            field_index: 3,
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f);
        assert_eq!(bc.len(), 2);
        assert!(matches!(bc[0].bytecode(), Instruction::LoadField));
        assert_eq!(bc[0].operand_u32(), 3);
    }

    // ============================================================
    // MakeEnum
    // ============================================================

    #[test]
    fn linearize_make_enum_packs_tag_and_arity() {
        let dst = ValueId(0);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::MakeEnum {
            dst,
            tag: 0x1234,
            payload: vec![ValueId(1), ValueId(2), ValueId(3)],
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Named(0),
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f);
        assert_eq!(bc.len(), 2);
        assert!(matches!(bc[0].bytecode(), Instruction::MakeEnum));
        // operands[31:16] = tag, operands[15:0] = arity.
        let operand = bc[0].operand_u32();
        assert_eq!(operand >> 16, 0x1234, "upper 16 bits = tag");
        assert_eq!(operand & 0xFFFF, 3, "lower 16 bits = arity");
    }

    #[test]
    fn linearize_make_enum_with_zero_arity() {
        let dst = ValueId(0);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::MakeEnum {
            dst,
            tag: 0x0007,
            payload: vec![],
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Named(0),
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f);
        let operand = bc[0].operand_u32();
        assert_eq!(operand >> 16, 0x0007);
        assert_eq!(operand & 0xFFFF, 0);
    }

    // ============================================================
    // Unpack
    // ============================================================

    #[test]
    fn linearize_unpack_emits_unpack() {
        let dst = ValueId(1);
        let scrutinee = ValueId(0);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::Unpack {
            dst: vec![dst],
            scrutinee,
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f);
        assert_eq!(bc.len(), 2);
        assert!(matches!(bc[0].bytecode(), Instruction::Unpack));
    }

    // ============================================================
    // Terminators
    // ============================================================

    #[test]
    fn linearize_unreachable_emits_halt() {
        let mut block = Block::new(BlockId(0));
        block.terminator = Terminator::Unreachable;
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Unit,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f);
        assert_eq!(bc.len(), 1);
        assert!(matches!(bc[0].bytecode(), Instruction::HALT));
    }

    #[test]
    #[should_panic(expected = "control-flow terminator")]
    fn linearize_jump_terminator_panics() {
        let mut block = Block::new(BlockId(0));
        block.terminator = Terminator::Jump(BlockId(1));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Unit,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let _ = linearize(&f);
    }

    #[test]
    #[should_panic(expected = "control-flow terminator")]
    fn linearize_branch_terminator_panics() {
        let mut block = Block::new(BlockId(0));
        block.terminator = Terminator::Branch {
            cond: ValueId(0),
            true_bb: BlockId(1),
            false_bb: BlockId(2),
        };
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Unit,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let _ = linearize(&f);
    }

    #[test]
    #[should_panic(expected = "control-flow terminator")]
    fn linearize_switch_terminator_panics() {
        let mut block = Block::new(BlockId(0));
        block.terminator = Terminator::Switch {
            scrutinee: ValueId(0),
            cases: vec![(1, BlockId(1))],
            default: BlockId(2),
        };
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Unit,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let _ = linearize(&f);
    }

    // ============================================================
    // Integration: multi-instruction sequence
    // ============================================================

    #[test]
    fn linearize_sequence_emits_instructions_in_order() {
        // Return(Const(5) + Const(3)). Both consts, then ADD, then
        // RETURN. Order matters.
        let v0 = ValueId(0);
        let v1 = ValueId(1);
        let v2 = ValueId(2);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::Const { dst: v0, value: 5 });
        block.insts.push(Inst::Const { dst: v1, value: 3 });
        block.insts.push(Inst::BinOp {
            op: BinOpKind::Add,
            dst: v2,
            lhs: v0,
            rhs: v1,
        });
        block.terminator = Terminator::Return(Some(v2));
        let f = Function {
            name: "add".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f);
        assert_eq!(bc.len(), 4);
        assert!(matches!(bc[0].bytecode(), Instruction::CONST));
        assert_eq!(bc[0].value_u32(), 5);
        assert!(matches!(bc[1].bytecode(), Instruction::CONST));
        assert_eq!(bc[1].value_u32(), 3);
        assert!(matches!(bc[2].bytecode(), Instruction::ADD));
        assert!(matches!(bc[3].bytecode(), Instruction::RETURN));
    }

    #[test]
    fn linearize_function_name_appears_in_panic_message() {
        // Build a multi-block function and assert the panic message
        // includes the function name. Catches the
        // "panic forgot the name" regression.
        let mut b0 = Block::new(BlockId(0));
        b0.terminator = Terminator::Jump(BlockId(1));
        let b1 = Block::new(BlockId(1)).with_terminator(Terminator::Return(None));
        let f = Function {
            name: "my_special_fn".to_string(),
            params: vec![],
            return_ty: TypeRef::Unit,
            blocks: vec![b0, b1],
            entry: BlockId(0),
        };
        let result = std::panic::catch_unwind(|| linearize(&f));
        match result {
            Err(e) => {
                let msg = if let Some(s) = e.downcast_ref::<String>() {
                    s.clone()
                } else if let Some(s) = e.downcast_ref::<&'static str>() {
                    s.to_string()
                } else {
                    String::from("<unknown panic payload>")
                };
                assert!(
                    msg.contains("my_special_fn"),
                    "panic message should include function name: {}",
                    msg
                );
                assert!(
                    msg.contains("2 blocks"),
                    "panic message should include block count: {}",
                    msg
                );
            }
            Ok(_) => panic!("expected linearize to panic on multi-block function"),
        }
    }
}