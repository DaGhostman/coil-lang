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
