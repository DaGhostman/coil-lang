//! Control Flow Graph (CFG) IR for the multi-pass compiler.
//!
//! This module defines the foundational data structures for the
//! multi-pass refactor described in MULTI_PASS_REFACTOR_PLAN.md.
//!
//! The CFG is an intermediate representation (IR) between the typed
//! AST and the bytecode. It replaces the current single-pass
//! codegen with a multi-pass approach where:
//!
//!   1. The typed AST is walked to produce a CFG (`cfg_builder`)
//!   2. SSA values are numbered block-locally (SSA-lite)
//!   3. Liveness analysis computes live ranges
//!   4. Linear-scan register allocation maps SSA values to
//!      registers / slots
//!   5. Bytecode emission linearizes the CFG into bytecode
//!
//! This file defines ONLY the data structures. The builder, SSA,
//! liveness, allocation, and emission passes will be added in
//! subsequent phases.
//!
//! ## Design notes
//!
//! - **Block-local SSA numbering** (SSA-lite): Each [`Block`]
//!   gets its own counter for fresh [`ValueId`]s. No dominance
//!   frontiers, no phi-nodes. This was validated by Experiment A
//!   (commit `faa7aaf`).
//!
//! - **Explicit graph structure**: Each [`Block`] has explicit
//!   [`Block::predecessors`] (filled in after construction). This
//!   enables backward dataflow analysis (liveness).
//!
//! - **Variable-length instruction format**: [`Inst`] is an enum
//!   with variants for each instruction kind. The variants carry
//!   operands as fields; the linearizer is responsible for
//!   encoding them into bytecode.
//!
//! - **No arena allocation**: For Phase 0, we use `Vec<T>`
//!   directly. If profiling later shows arena allocation would be
//!   beneficial, we can switch to `typed_arena` or `bumpalo`.

use std::fmt;

/// Unique identifier for a block within a function.
///
/// BlockIds are dense indices into [`Function::blocks`]; a function
/// with `N` blocks uses `BlockId(0)` through `BlockId(N - 1)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

impl BlockId {
    /// Sentinel for "not a valid block". Used during construction
    /// (an unset predecessor slot) and in error-recovery paths.
    pub const INVALID: BlockId = BlockId(u32::MAX);

    /// Wrap an index as a `BlockId`. Caller is responsible for
    /// ensuring the index is in range.
    pub fn new(index: u32) -> Self {
        BlockId(index)
    }

    /// Return the index suitable for use as a `Vec` subscript.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bb{}", self.0)
    }
}

/// Unique identifier for an SSA value within a function.
///
/// SSA-lite: block-local numbering (see module docs). A `ValueId`
/// is unique across the whole function (not just the block), but
/// the fresh-value counter is per-block — this avoids needing
/// dominance frontiers and phi-nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ValueId(pub u32);

impl ValueId {
    /// Sentinel for "not a valid value". Used in error-recovery
    /// paths and for unset `Call::dst` slots when the call's
    /// return value is discarded.
    pub const INVALID: ValueId = ValueId(u32::MAX);

    /// Wrap an index as a `ValueId`. Caller is responsible for
    /// ensuring the index is in range.
    pub fn new(index: u32) -> Self {
        ValueId(index)
    }

    /// Return the index suitable for use as a `Vec` subscript.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Display for ValueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}

/// Instruction kinds for the CFG.
///
/// In Phase 0, this is a placeholder. The actual instruction set
/// will be finalized when the register VM is built (Phase 3).
#[derive(Debug, Clone)]
pub enum Inst {
    /// Push an integer constant: `dst = value`.
    Const {
        dst: ValueId,
        value: i64,
    },
    /// Push a floating-point constant: `dst = value`.
    ConstF {
        dst: ValueId,
        value: f64,
    },
    /// Push a boolean constant: `dst = value`.
    ConstBool {
        dst: ValueId,
        value: bool,
    },
    /// Push a string constant: `dst = value`.
    ConstString {
        dst: ValueId,
        value: String,
    },
    /// Reference a function parameter: `dst = params[index]`.
    Param {
        dst: ValueId,
        index: u16,
    },
    /// Binary operation: `dst = lhs OP rhs`.
    BinOp {
        op: BinOpKind,
        dst: ValueId,
        lhs: ValueId,
        rhs: ValueId,
    },
    /// Unary operation: `dst = OP src`.
    UnaryOp {
        op: UnaryOpKind,
        dst: ValueId,
        src: ValueId,
    },
    /// Function call: `dst = callee(args)`.
    ///
    /// `dst` is `None` when the call's return value is discarded
    /// (e.g. a top-level statement like `print(...)` or a bare
    /// expression statement).
    Call {
        dst: Option<ValueId>,
        callee: ValueId,
        args: Vec<ValueId>,
    },
    /// Load a record field: `dst = src.field_index`.
    LoadField {
        dst: ValueId,
        src: ValueId,
        field_index: u16,
    },
    /// Unpack an enum's payload: `dst_i = scrutinee.payload[i]`.
    ///
    /// The number of destinations equals the variant's arity
    /// (encoded by `dst.len()`). The linearizer is responsible
    /// for emitting the matching sequence of bytecode instructions
    /// (`UNPACK`, `UNPACK_AT`, etc.) based on the variant's shape
    /// (Unit / Tuple / Record).
    Unpack {
        dst: Vec<ValueId>,
        scrutinee: ValueId,
    },
    /// Make an enum value:
    /// `dst = Enum::Variant(payload_0, ..., payload_n)`.
    MakeEnum {
        dst: ValueId,
        tag: u32,
        payload: Vec<ValueId>,
    },
    /// Print: `print(args[0], args[1], ..., args[n])`.
    ///
    /// `args[0]` is the format string; the remaining elements are
    /// the parameters to format. The linearizer is responsible
    /// for emitting the matching bytecode (`PRINT` for the
    /// simple case of just a format string with no params;
    /// `FORMAT` + `PRINT` for the full case with format
    /// specifiers — the latter is not yet implemented in Phase 1.6).
    ///
    /// Phase 1.6 supports only the no-params case. The
    /// `is_straight_line` lift in `compiler/src/lib.rs` only
    /// allows `print "literal";` through the CFG path — programs
    /// with format specifiers (`print "%i", x;`) still fall
    /// back to the single-pass codegen.
    Print {
        args: Vec<ValueId>,
    },
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOpKind {
    // Integer arithmetic.
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    // Float arithmetic.
    AddF,
    SubF,
    MulF,
    DivF,
    ModF,
    // Integer comparison.
    Eq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
    // Float comparison.
    EqF,
    NeqF,
    LtF,
    LeF,
    GtF,
    GeF,
    // Logical (operate on bools).
    And,
    Or,
    // Bitwise.
    Shl,
    Shr,
    Xor,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOpKind {
    /// Integer negation.
    Neg,
    /// Float negation.
    NegF,
    /// Logical not.
    Not,
}

/// Block terminator. After this, control transfers out of the
/// block.
#[derive(Debug, Clone)]
pub enum Terminator {
    /// Unconditional jump to another block.
    Jump(BlockId),
    /// Conditional branch on `cond`. If true, jump to `true_bb`;
    /// otherwise fall through to `false_bb`.
    Branch {
        cond: ValueId,
        true_bb: BlockId,
        false_bb: BlockId,
    },
    /// Multi-way dispatch (match expression).
    ///
    /// `cases` are `(tag_value, target_block)`. `default` is the
    /// fallthrough arm (the LAST arm in source order, reached
    /// when no case matches).
    Switch {
        scrutinee: ValueId,
        cases: Vec<(u32, BlockId)>,
        default: BlockId,
    },
    /// Function return with optional value (`None` for unit).
    Return(Option<ValueId>),
    /// Unreachable (after `return` in a non-returning context, or
    /// after `unreachable!()` calls).
    Unreachable,
}

/// A basic block in the CFG.
///
/// Each block has:
/// - A unique [`BlockId`] (its position in [`Function::blocks`])
/// - A list of straight-line [`Inst`]s (no internal control flow)
/// - A [`Terminator`] (the control transfer out of the block)
/// - A list of [`BlockId`] predecessors (filled in after
///   construction by walking the terminators of all blocks)
///
/// A block is "sealed" (all its predecessors have been filled in)
/// once [`crate::cfg_builder::compute_predecessors`] has run.
#[derive(Debug, Clone)]
pub struct Block {
    pub id: BlockId,
    pub insts: Vec<Inst>,
    pub terminator: Terminator,
    pub predecessors: Vec<BlockId>,
}

impl Block {
    /// Construct a new block with the given ID, an empty
    /// instruction list, and a placeholder `Unreachable`
    /// terminator.
    pub fn new(id: BlockId) -> Self {
        Block {
            id,
            insts: Vec::new(),
            terminator: Terminator::Unreachable,
            predecessors: Vec::new(),
        }
    }

    /// Builder-style helper: set the terminator and return
    /// `self`.
    pub fn with_terminator(mut self, term: Terminator) -> Self {
        self.terminator = term;
        self
    }
}

impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}:", self.id)?;
        if !self.predecessors.is_empty() {
            write!(f, "  ; predecessors: [")?;
            for (i, pred) in self.predecessors.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{}", pred)?;
            }
            writeln!(f, "]")?;
        }
        for inst in &self.insts {
            writeln!(f, "  {}", inst)?;
        }
        write!(f, "  {}", self.terminator)
    }
}

impl fmt::Display for Inst {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Inst::Const { dst, value } => write!(f, "{} = const.i64 {}", dst, value),
            Inst::ConstF { dst, value } => write!(f, "{} = const.f64 {}", dst, value),
            Inst::ConstBool { dst, value } => {
                write!(f, "{} = const.bool {}", dst, value)
            }
            Inst::ConstString { dst, value } => {
                write!(f, "{} = const.string {:?}", dst, value)
            }
            Inst::Param { dst, index } => write!(f, "{} = param {}", dst, index),
            Inst::BinOp { op, dst, lhs, rhs } => {
                write!(f, "{} = {} {} {}", dst, op, lhs, rhs)
            }
            Inst::UnaryOp { op, dst, src } => {
                write!(f, "{} = {} {}", dst, op, src)
            }
            Inst::Call { dst, callee, args } => {
                let dst_str = dst.map(|d| format!("{} = ", d)).unwrap_or_default();
                let args_str = args
                    .iter()
                    .map(|a| a.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "{}{}({})", dst_str, callee, args_str)
            }
            Inst::LoadField { dst, src, field_index } => {
                write!(f, "{} = {}.field[{}]", dst, src, field_index)
            }
            Inst::Unpack { dst, scrutinee } => {
                let dst_str = dst
                    .iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "({}) = unpack {}", dst_str, scrutinee)
            }
            Inst::MakeEnum { dst, tag, payload } => {
                let payload_str = payload
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "{} = make_enum tag={} [{}]", dst, tag, payload_str)
            }
            Inst::Print { args } => {
                let args_str = args
                    .iter()
                    .map(|a| a.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "print({})", args_str)
            }
        }
    }
}

impl fmt::Display for BinOpKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            BinOpKind::Add => "+",
            BinOpKind::Sub => "-",
            BinOpKind::Mul => "*",
            BinOpKind::Div => "/",
            BinOpKind::Mod => "%",
            BinOpKind::AddF => "+f",
            BinOpKind::SubF => "-f",
            BinOpKind::MulF => "*f",
            BinOpKind::DivF => "/f",
            BinOpKind::ModF => "%f",
            BinOpKind::Eq => "==",
            BinOpKind::Neq => "!=",
            BinOpKind::Lt => "<",
            BinOpKind::Le => "<=",
            BinOpKind::Gt => ">",
            BinOpKind::Ge => ">=",
            BinOpKind::EqF => "==f",
            BinOpKind::NeqF => "!=f",
            BinOpKind::LtF => "<f",
            BinOpKind::LeF => "<=f",
            BinOpKind::GtF => ">f",
            BinOpKind::GeF => ">=f",
            BinOpKind::And => "and",
            BinOpKind::Or => "or",
            BinOpKind::Shl => "shl",
            BinOpKind::Shr => "shr",
            BinOpKind::Xor => "xor",
        };
        f.write_str(s)
    }
}

impl fmt::Display for UnaryOpKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            UnaryOpKind::Neg => "-",
            UnaryOpKind::NegF => "-f",
            UnaryOpKind::Not => "!",
        };
        f.write_str(s)
    }
}

impl fmt::Display for Terminator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Terminator::Jump(bb) => write!(f, "jump {}", bb),
            Terminator::Branch { cond, true_bb, false_bb } => {
                write!(f, "branch {}, {}, {}", cond, true_bb, false_bb)
            }
            Terminator::Switch { scrutinee, cases, default } => {
                write!(f, "switch {} [", scrutinee)?;
                for (i, (tag, bb)) in cases.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{} -> {}", tag, bb)?;
                }
                write!(f, "] default {}", default)
            }
            Terminator::Return(None) => write!(f, "return"),
            Terminator::Return(Some(v)) => write!(f, "return {}", v),
            Terminator::Unreachable => write!(f, "unreachable"),
        }
    }
}

/// A function in the CFG.
///
/// Each function has:
/// - A `name` (for diagnostics; the codegen Interner still
///   resolves the real string ID)
/// - A list of `params` (each is `(value_id, name)`)
/// - A `return_ty` (placeholder type info — see [`TypeRef`])
/// - A list of [`Block`]s
/// - An `entry` block ID (the first block to execute)
#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub params: Vec<(ValueId, String)>,
    pub return_ty: TypeRef,
    pub blocks: Vec<Block>,
    pub entry: BlockId,
}

/// Placeholder for the type system.
///
/// In Phase 0.1, we don't need full type information in the CFG —
/// the codegen side-tables (e.g., the HM typechecker's
/// `codegen_var_types`) still provide it. In later phases, the CFG
/// will carry its own type info and the side-tables can be
/// removed.
///
/// `Named(u32)` refers to a named type (enum, function, etc.) by
/// its interned ID. The interning happens in the HM typechecker;
/// the CFG only carries the opaque ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeRef {
    /// Type is unknown (during construction, before late type
    /// resolution). Linearizer treats this as a slot-only value.
    Unknown,
    Int,
    Float,
    Bool,
    String,
    Unit,
    /// Reference to a named type (enum, function, etc.) by
    /// interned ID.
    Named(u32),
}

impl fmt::Display for TypeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            TypeRef::Unknown => "?",
            TypeRef::Int => "int",
            TypeRef::Float => "float",
            TypeRef::Bool => "bool",
            TypeRef::String => "string",
            TypeRef::Unit => "()",
            TypeRef::Named(id) => return write!(f, "named#{}", id),
        };
        f.write_str(s)
    }
}

impl fmt::Display for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "fn {}(", self.name)?;
        for (vid, name) in &self.params {
            writeln!(f, "  {}: {}", name, vid)?;
        }
        writeln!(f, ") -> {}", self.return_ty)?;
        writeln!(f, "entry: {}", self.entry)?;
        for block in &self.blocks {
            write!(f, "{}", block)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // =================================================================
    // BlockId
    // =================================================================

    #[test]
    fn block_id_new_creates_typed_id() {
        assert_eq!(BlockId::new(42).0, 42);
        assert_eq!(BlockId::new(0).0, 0);
    }

    #[test]
    fn block_id_invalid_sentinel() {
        assert_eq!(BlockId::INVALID.0, u32::MAX);
    }

    #[test]
    fn block_id_equality() {
        let a = BlockId::new(7);
        let b = BlockId::new(7);
        assert_eq!(a, b);
    }

    #[test]
    fn block_id_inequality() {
        let a = BlockId::new(7);
        let b = BlockId::new(8);
        assert_ne!(a, b);
    }

    #[test]
    fn block_id_hash_consistent_with_eq() {
        let mut set = HashSet::new();
        set.insert(BlockId::new(5));
        assert!(set.contains(&BlockId::new(5)));
        assert!(!set.contains(&BlockId::new(6)));
    }

    #[test]
    fn block_id_display() {
        assert_eq!(format!("{}", BlockId(5)), "bb5");
        assert_eq!(format!("{}", BlockId::new(0)), "bb0");
        assert_eq!(format!("{}", BlockId::new(42)), "bb42");
        assert_eq!(format!("{}", BlockId::INVALID), "bb4294967295");
    }

    // =================================================================
    // ValueId
    // =================================================================

    #[test]
    fn value_id_new_creates_typed_id() {
        assert_eq!(ValueId::new(42).0, 42);
        assert_eq!(ValueId::new(0).0, 0);
    }

    #[test]
    fn value_id_invalid_sentinel() {
        assert_eq!(ValueId::INVALID.0, u32::MAX);
    }

    #[test]
    fn value_id_equality() {
        let a = ValueId::new(7);
        let b = ValueId::new(7);
        assert_eq!(a, b);
    }

    #[test]
    fn value_id_hash_consistent_with_eq() {
        let mut set = HashSet::new();
        set.insert(ValueId::new(5));
        assert!(set.contains(&ValueId::new(5)));
        assert!(!set.contains(&ValueId::new(6)));
    }

    #[test]
    fn value_id_display() {
        assert_eq!(format!("{}", ValueId(7)), "v7");
        assert_eq!(format!("{}", ValueId::new(0)), "v0");
        assert_eq!(format!("{}", ValueId::new(123)), "v123");
    }

    // =================================================================
    // TypeRef
    // =================================================================

    #[test]
    fn type_ref_equality() {
        assert_eq!(TypeRef::Int, TypeRef::Int);
        assert_eq!(TypeRef::Float, TypeRef::Float);
        assert_eq!(TypeRef::Bool, TypeRef::Bool);
        assert_eq!(TypeRef::String, TypeRef::String);
        assert_eq!(TypeRef::Unit, TypeRef::Unit);
        assert_eq!(TypeRef::Unknown, TypeRef::Unknown);
        assert_eq!(TypeRef::Named(42), TypeRef::Named(42));
    }

    #[test]
    fn type_ref_inequality() {
        assert_ne!(TypeRef::Int, TypeRef::Float);
        assert_ne!(TypeRef::Int, TypeRef::Unknown);
        assert_ne!(TypeRef::Named(1), TypeRef::Named(2));
        assert_ne!(TypeRef::Bool, TypeRef::Int);
        assert_ne!(TypeRef::Unit, TypeRef::Named(0));
    }

    #[test]
    fn type_ref_clone() {
        let a = TypeRef::Named(99);
        let b = a.clone();
        assert_eq!(a, b);

        let c = TypeRef::Float;
        let d = c.clone();
        assert_eq!(c, d);
    }

    // =================================================================
    // Inst
    // =================================================================

    #[test]
    fn inst_const_i64_display() {
        let inst = Inst::Const {
            dst: ValueId(0),
            value: 42,
        };
        assert_eq!(format!("{}", inst), "v0 = const.i64 42");
    }

    #[test]
    fn inst_const_f64_display() {
        let inst = Inst::ConstF {
            dst: ValueId(1),
            value: 3.14,
        };
        assert_eq!(format!("{}", inst), "v1 = const.f64 3.14");
    }

    #[test]
    fn inst_const_bool_display() {
        let inst = Inst::ConstBool {
            dst: ValueId(2),
            value: true,
        };
        assert_eq!(format!("{}", inst), "v2 = const.bool true");
    }

    #[test]
    fn inst_const_string_display() {
        let inst = Inst::ConstString {
            dst: ValueId(3),
            value: "hello".to_string(),
        };
        assert_eq!(format!("{}", inst), "v3 = const.string \"hello\"");
    }

    #[test]
    fn inst_param_display() {
        let inst = Inst::Param {
            dst: ValueId(1),
            index: 0,
        };
        assert_eq!(format!("{}", inst), "v1 = param 0");
    }

    #[test]
    fn inst_binop_add_display() {
        let inst = Inst::BinOp {
            op: BinOpKind::Add,
            dst: ValueId(2),
            lhs: ValueId(0),
            rhs: ValueId(1),
        };
        assert_eq!(format!("{}", inst), "v2 = + v0 v1");
    }

    #[test]
    fn inst_unaryop_neg_display() {
        let inst = Inst::UnaryOp {
            op: UnaryOpKind::Neg,
            dst: ValueId(1),
            src: ValueId(0),
        };
        assert_eq!(format!("{}", inst), "v1 = - v0");
    }

    #[test]
    fn inst_call_display() {
        let inst = Inst::Call {
            dst: Some(ValueId(3)),
            callee: ValueId(0),
            args: vec![ValueId(1), ValueId(2)],
        };
        assert_eq!(format!("{}", inst), "v3 = v0(v1, v2)");
    }

    #[test]
    fn inst_load_field_display() {
        let inst = Inst::LoadField {
            dst: ValueId(1),
            src: ValueId(0),
            field_index: 2,
        };
        assert_eq!(format!("{}", inst), "v1 = v0.field[2]");
    }

    #[test]
    fn inst_unpack_display() {
        let inst = Inst::Unpack {
            dst: vec![ValueId(1), ValueId(2)],
            scrutinee: ValueId(0),
        };
        assert_eq!(format!("{}", inst), "(v1, v2) = unpack v0");
    }

    #[test]
    fn inst_make_enum_display() {
        let inst = Inst::MakeEnum {
            dst: ValueId(0),
            tag: 1,
            payload: vec![ValueId(1), ValueId(2)],
        };
        assert_eq!(format!("{}", inst), "v0 = make_enum tag=1 [v1, v2]");
    }

    #[test]
    fn inst_print_simple_display() {
        // `print "hello";` — single arg, the format string. The
        // Phase 1.6 simple case.
        let inst = Inst::Print {
            args: vec![ValueId(0)],
        };
        assert_eq!(format!("{}", inst), "print(v0)");
    }

    #[test]
    fn inst_print_with_format_specifier_args_display() {
        // `print "%i", x;` — Phase 1.6 builds this but the
        // linearizer doesn't handle it yet. Display still works.
        let inst = Inst::Print {
            args: vec![ValueId(0), ValueId(1)],
        };
        assert_eq!(format!("{}", inst), "print(v0, v1)");
    }

    // =================================================================
    // BinOpKind
    // =================================================================

    #[test]
    fn binop_int_arithmetic_display() {
        assert_eq!(format!("{}", BinOpKind::Add), "+");
        assert_eq!(format!("{}", BinOpKind::Sub), "-");
        assert_eq!(format!("{}", BinOpKind::Mul), "*");
        assert_eq!(format!("{}", BinOpKind::Div), "/");
        assert_eq!(format!("{}", BinOpKind::Mod), "%");
    }

    #[test]
    fn binop_float_arithmetic_display() {
        assert_eq!(format!("{}", BinOpKind::AddF), "+f");
        assert_eq!(format!("{}", BinOpKind::SubF), "-f");
        assert_eq!(format!("{}", BinOpKind::MulF), "*f");
        assert_eq!(format!("{}", BinOpKind::DivF), "/f");
        assert_eq!(format!("{}", BinOpKind::ModF), "%f");
    }

    #[test]
    fn binop_comparison_display() {
        assert_eq!(format!("{}", BinOpKind::Eq), "==");
        assert_eq!(format!("{}", BinOpKind::Neq), "!=");
        assert_eq!(format!("{}", BinOpKind::Lt), "<");
        assert_eq!(format!("{}", BinOpKind::Le), "<=");
        assert_eq!(format!("{}", BinOpKind::Gt), ">");
        assert_eq!(format!("{}", BinOpKind::Ge), ">=");
    }

    #[test]
    fn binop_logical_display() {
        assert_eq!(format!("{}", BinOpKind::And), "and");
        assert_eq!(format!("{}", BinOpKind::Or), "or");
    }

    #[test]
    fn binop_bitwise_display() {
        assert_eq!(format!("{}", BinOpKind::Shl), "shl");
        assert_eq!(format!("{}", BinOpKind::Shr), "shr");
        assert_eq!(format!("{}", BinOpKind::Xor), "xor");
    }

    // =================================================================
    // UnaryOpKind
    // =================================================================

    #[test]
    fn unaryop_display() {
        assert_eq!(format!("{}", UnaryOpKind::Neg), "-");
        assert_eq!(format!("{}", UnaryOpKind::NegF), "-f");
        assert_eq!(format!("{}", UnaryOpKind::Not), "!");
    }

    // =================================================================
    // Terminator
    // =================================================================

    #[test]
    fn terminator_jump_display() {
        let t = Terminator::Jump(BlockId(1));
        assert_eq!(format!("{}", t), "jump bb1");
    }

    #[test]
    fn terminator_branch_display() {
        let t = Terminator::Branch {
            cond: ValueId(0),
            true_bb: BlockId(1),
            false_bb: BlockId(2),
        };
        assert_eq!(format!("{}", t), "branch v0, bb1, bb2");
    }

    #[test]
    fn terminator_switch_display() {
        let t = Terminator::Switch {
            scrutinee: ValueId(0),
            cases: vec![(1, BlockId(1)), (2, BlockId(2))],
            default: BlockId(3),
        };
        assert_eq!(format!("{}", t), "switch v0 [1 -> bb1, 2 -> bb2] default bb3");
    }

    #[test]
    fn terminator_return_display() {
        let none = Terminator::Return(None);
        assert_eq!(format!("{}", none), "return");

        let some = Terminator::Return(Some(ValueId(5)));
        assert_eq!(format!("{}", some), "return v5");
    }

    #[test]
    fn terminator_unreachable_display() {
        let t = Terminator::Unreachable;
        assert_eq!(format!("{}", t), "unreachable");
    }

    // =================================================================
    // Block
    // =================================================================

    #[test]
    fn block_new_is_unreachable() {
        let b = Block::new(BlockId(0));
        assert!(b.insts.is_empty());
        assert!(matches!(b.terminator, Terminator::Unreachable));
        assert!(b.predecessors.is_empty());
        assert_eq!(b.id, BlockId(0));
    }

    #[test]
    fn block_with_terminator_sets_terminator() {
        let b = Block::new(BlockId(0)).with_terminator(Terminator::Jump(BlockId(1)));
        assert!(matches!(
            b.terminator,
            Terminator::Jump(target) if target == BlockId(1)
        ));
        // Other fields are unchanged.
        assert!(b.insts.is_empty());
        assert!(b.predecessors.is_empty());
        assert_eq!(b.id, BlockId(0));
    }

    #[test]
    fn block_display_includes_id_and_insts_and_terminator() {
        let b = Block {
            id: BlockId(0),
            insts: vec![Inst::Const {
                dst: ValueId(0),
                value: 5,
            }],
            terminator: Terminator::Return(Some(ValueId(0))),
            predecessors: vec![],
        };
        let s = format!("{}", b);
        assert!(s.contains("bb0:"), "missing block id: {:?}", s);
        assert!(s.contains("v0 = const.i64 5"), "missing inst: {:?}", s);
        assert!(s.contains("return v0"), "missing terminator: {:?}", s);
    }

    // =================================================================
    // Function
    // =================================================================

    #[test]
    fn function_new_with_empty_blocks() {
        let f = Function {
            name: "empty_fn".to_string(),
            params: vec![],
            return_ty: TypeRef::Unit,
            blocks: vec![],
            entry: BlockId(0),
        };
        assert_eq!(f.name, "empty_fn");
        assert_eq!(f.params.len(), 0);
        assert_eq!(f.return_ty, TypeRef::Unit);
        assert_eq!(f.blocks.len(), 0);
        assert_eq!(f.entry, BlockId(0));
    }

    #[test]
    fn function_display_includes_name_params_entry_blocks() {
        let f = Function {
            name: "my_fn".to_string(),
            params: vec![(ValueId(0), "x".to_string())],
            return_ty: TypeRef::Int,
            blocks: vec![Block::new(BlockId(0))],
            entry: BlockId(0),
        };
        let s = format!("{}", f);
        assert!(s.contains("my_fn"), "missing name: {:?}", s);
        assert!(s.contains("x: v0"), "missing param: {:?}", s);
        assert!(s.contains("entry: bb0"), "missing entry: {:?}", s);
        assert!(s.contains("int"), "missing return type: {:?}", s);
        assert!(s.contains("bb0:"), "missing block: {:?}", s);
    }

    // =================================================================
    // Integration: canonical unwrap_or_zero function
    // =================================================================

    #[test]
    fn canonical_unwrap_or_zero_function() {
        let opt = ValueId::new(0);
        let v = ValueId::new(1);
        let zero = ValueId::new(2);

        let dispatch = Block::new(BlockId::new(0)).with_terminator(Terminator::Switch {
            scrutinee: opt,
            cases: vec![(1, BlockId::new(1))], // Some -> bb1
            default: BlockId::new(2),          // None (default) -> bb2
        });
        let some_arm = Block {
            id: BlockId::new(1),
            insts: vec![Inst::Unpack {
                dst: vec![v],
                scrutinee: opt,
            }],
            terminator: Terminator::Return(Some(v)),
            predecessors: vec![BlockId::new(0)],
        };
        let none_arm = Block {
            id: BlockId::new(2),
            insts: vec![Inst::Const {
                dst: zero,
                value: 0,
            }],
            terminator: Terminator::Return(Some(zero)),
            predecessors: vec![BlockId::new(0)],
        };

        let func = Function {
            name: "unwrap_or_zero".to_string(),
            params: vec![(opt, "opt".to_string())],
            return_ty: TypeRef::Int,
            blocks: vec![dispatch, some_arm, none_arm],
            entry: BlockId::new(0),
        };

        // 3 blocks: dispatch + Some arm + None arm.
        assert_eq!(func.blocks.len(), 3);

        // Each arm has 1 inst + 1 Return terminator.
        assert_eq!(func.blocks[1].insts.len(), 1);
        assert!(matches!(func.blocks[1].terminator, Terminator::Return(_)));
        assert_eq!(func.blocks[2].insts.len(), 1);
        assert!(matches!(func.blocks[2].terminator, Terminator::Return(_)));

        // Dispatch block: 0 insts, Switch terminator.
        assert_eq!(func.blocks[0].insts.len(), 0);
        assert!(matches!(func.blocks[0].terminator, Terminator::Switch { .. }));

        let displayed = format!("{}", func);
        assert!(
            displayed.contains("unwrap_or_zero"),
            "missing name: {}",
            displayed
        );
        assert!(
            displayed.contains("entry: bb0"),
            "missing entry: {}",
            displayed
        );
        assert!(
            displayed.contains("opt"),
            "missing param name: {}",
            displayed
        );
        assert!(
            displayed.contains("switch"),
            "missing switch terminator: {}",
            displayed
        );
        assert!(
            displayed.contains("unpack"),
            "missing unpack inst: {}",
            displayed
        );
        assert!(
            displayed.contains("const.i64 0"),
            "missing const 0: {}",
            displayed
        );
        assert!(
            displayed.contains("return"),
            "missing return: {}",
            displayed
        );
    }
}
