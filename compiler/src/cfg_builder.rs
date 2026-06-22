//! CFG builder for the multi-pass compiler.
//!
//! Walks a typed AST and produces a CFG [`Function`]. Phase 0.2a
//! scope: **straight-line expressions only**. Control flow
//! ([`Expression::If`], [`Expression::Loop`], [`Expression::Match`],
//! [`Expression::Branch`]) is deferred to Phase 1, when the builder
//! will learn to split blocks, allocate fresh block IDs, and emit
//! [`Terminator::Branch`] / [`Terminator::Switch`] instead of the
//! trivial [`Terminator::Return`] produced today.
//!
//! ## Design
//!
//! The builder maintains a `Vec<Block>` (constructed bottom-up) and a
//! "current block" pointer. Every [`Inst`] emitted by the
//! expression-walker is appended to the current block. After walking
//! the function body, the builder sets the entry block's terminator
//! from the accumulated [`Builder::return_value`].
//!
//! SSA-lite (see [`crate::cfg`]): a single, function-global
//! `next_value: u32` counter mints fresh [`ValueId`]s. No dominance
//! frontiers, no phi-nodes — Phase 0.2a is single-block, so this is
//! trivially correct. Phase 1 may split the counter per-block, but
//! the existing codegen pattern (see
//! `compiler/src/lib.rs::compile_binary_operands`) already hints
//! that the linearizer can resolve cross-block references from the
//! `(NodeId) -> Ty` cache.
//!
//! See `MULTI_PASS_REFACTOR_PLAN.md` §3 for the high-level design.
//!
//! ## Scope (Phase 0.2a)
//!
//! Handles these AST variants:
//!
//! | AST variant              | CFG construction                              |
//! |--------------------------|------------------------------------------------|
//! | `Integer` / `Float` /    | `Inst::Const` / `Inst::ConstF` /               |
//! | `Bool` / `String`        | `Inst::ConstBool` / `Inst::ConstString`        |
//! | `Identifier(name)`       | look up `locals` then `params`; returns the    |
//! |                          | existing `ValueId` (no instruction emitted)    |
//! | `Add` / `Sub` / `Mul` /  | recursive `lhs`/`rhs`, then `Inst::BinOp`      |
//! | `Div` / `Mod` / `Eq` /   | (the int-vs-float choice is deferred to the    |
//! | `Neq` / `Lt` / `Le` /    | linearizer in Phase 3 — the builder emits the  |
//! | `Gt` / `Ge` / `And` /    | generic `BinOpKind` and lets the linearizer    |
//! | `Or` / `Shl` / `Shr` /   | pick the int or float variant from the type    |
//! | `Xor` / `BitAnd` /       | info)                                          |
//! | `BitOr` / `Pow`          |                                                |
//! | `Negate` / `Not` /       | recursive `expr`, then `Inst::UnaryOp`         |
//! | `Positive`               |                                                |
//! | `Call`                   | recursive `callee` + each `arg`, then           |
//! |                          | `Inst::Call { dst: Some(...), ... }`            |
//! | `Access(receiver, field)`| recursive `receiver`, then `Inst::LoadField`   |
//! |                          | with `field_index = 0` (placeholder — the      |
//! |                          | linearizer needs the receiver's enum name,     |
//! |                          | which is available in the type side-table)      |
//! | `Construct`              | recursive payload, then `Inst::MakeEnum`       |
//! |                          | with `tag = 0` and `arity = 0` (placeholder —   |
//! |                          | the linearizer resolves via the typechecker's  |
//! |                          | `tag_for` / `arity_for`)                       |
//! | `Block(children)`        | build each child; return the last value        |
//! | `Fragment(children)`     | special-case `[Variable(name), rhs]` as a let  |
//! |                          | binding; otherwise build each child and return |
//! |                          | the last value                                 |
//! | `Print(fmt, params)`     | build `fmt` and each `param`; return `None`    |
//! |                          | (the print instruction is emitted by the       |
//! |                          | linearizer in Phase 3)                         |
//! | `Return(expr)` /         | build `expr`; record in `return_value`; the    |
//! | `ImplicitReturn(expr)`   | terminator is emitted at the end of             |
//! |                          | `build_function`                               |
//! | `Assignment(lhs, rhs)`   | build `rhs`; update `locals[name]` (where      |
//! |                          | `name` is the LHS identifier)                  |
//!
//! Variants not listed are no-ops (return `None`) or panics
//! (control-flow variants, which are explicit Phase 1 work).
//!
//! ## Deferred to Phase 1
//!
//! - `If`, `Branch` — need `Terminator::Branch` and split blocks.
//! - `Loop` — need a back-edge and `Terminator::Branch`.
//! - `Match` — need `Terminator::Switch` and per-arm blocks.
//! - Multi-block predecessors — `fill_predecessors` is structured to
//!   handle this but is trivial for the single-block case produced
//!   today.

use std::collections::HashMap;

// `MatchArm` and `Pattern` are imported for Phase 1 work
// (`Expression::Match` arm) that's deferred; silencing the
// unused-import warning keeps the Phase 0.2a diff minimal.
#[allow(unused_imports)]
use parser::ast::{Expression, MatchArm, Output, Pattern};

use crate::cfg::{
    BinOpKind, Block, BlockId, Function, Inst, Terminator, TypeRef, UnaryOpKind, ValueId,
};

/// Builds a CFG [`Function`] by walking a typed AST.
///
/// The builder is single-use per function: call [`Builder::new`],
/// then [`Builder::build_function`], then drop or reuse for another
/// function. There is no internal state that needs explicit cleanup
/// between calls.
///
/// ## Invariants
///
/// - `next_value` is monotonically increasing across the entire
///   builder lifetime. Reusing the builder for multiple functions
///   does NOT reset it; this means `ValueId` numbering is unique
///   across all functions built by a single builder. The
///   linearizer (Phase 3) is responsible for resolving
///   cross-function references via the function's `params` list and
///   the call site's `Inst::Call.callee`.
/// - `next_block` is reset only by [`Builder::new`]. Phase 0.2a
///   only ever allocates one block per function, so this is not a
///   problem in practice.
/// - `locals` and `params` are NOT reset between functions — each
///   [`Builder::build_function`] call rewrites them from scratch
///   for the new function's parameter list. This is intentional:
///   any stale entries from a previous function are harmless because
///   `build_function` overwrites them.
///
/// `#[allow(dead_code)]` silences the "fields/methods never read"
/// warnings until the builder has callers (Phase 0.2a tests are
/// committed separately; Phase 1+ will add real consumers).
#[allow(dead_code)]
pub struct Builder {
    /// Counter for fresh SSA [`ValueId`]s. Monotonic across the
    /// builder's lifetime (see type docs).
    next_value: u32,

    /// Counter for fresh [`BlockId`]s. Phase 0.2a only allocates
    /// one block per function, so this is essentially unused in
    /// practice; it's here for Phase 1 compatibility.
    next_block: u32,

    /// All blocks built so far, in construction order. Phase 0.2a
    /// always has exactly one entry here (the function's single
    /// block). [`Builder::build_function`] drains this `Vec` into
    /// the returned [`Function`].
    blocks: Vec<Block>,

    /// The block currently being built. New instructions emitted by
    /// [`Builder::build_expression`] are appended here. Phase 0.2a
    /// never switches blocks mid-function; the field exists for
    /// Phase 1.
    current: BlockId,

    /// Let-bound variable name → SSA [`ValueId`]. Populated by
    /// [`Builder::build_expression`] when entering a Fragment's
    /// let-binding shape (`[Variable(name), rhs]`). Consulted by
    /// `Identifier` lookups (which check `locals` first, then
    /// `params`).
    locals: HashMap<String, ValueId>,

    /// Function parameter name → SSA [`ValueId`]. Populated by
    /// [`Builder::build_function`] when walking `func.args`.
    /// Consulted by `Identifier` lookups.
    params: HashMap<String, ValueId>,

    /// Function parameters as `(value_id, name)` pairs, in source
    /// order. Used to populate [`Function::params`] at the end of
    /// [`Builder::build_function`] and to drive the
    /// `Inst::Param { index, ... }` emission.
    param_list: Vec<(ValueId, String)>,

    /// The function's return value, accumulated during
    /// [`Builder::build_expression`]. Set when the body hits a
    /// `Return` or `ImplicitReturn`. Used to set the entry block's
    /// terminator at the end of [`Builder::build_function`].
    return_value: Option<ValueId>,

    /// The function's return type. Phase 0.2a defaults to
    /// [`TypeRef::Unknown`]; Phase 1 will populate this from the
    /// `Function.returns` field in the AST.
    return_ty: TypeRef,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

// `#[allow(dead_code)]` on the struct doesn't propagate to its
// inherent impl block; silence the "associated items never used"
// warning here too.
#[allow(dead_code)]
impl Builder {
    /// Construct a fresh builder with empty state. Both `locals`
    /// and `params` start empty; the value/block counters start at
    /// zero.
    pub fn new() -> Self {
        Builder {
            next_value: 0,
            next_block: 0,
            blocks: Vec::new(),
            current: BlockId::INVALID,
            locals: HashMap::new(),
            params: HashMap::new(),
            param_list: Vec::new(),
            return_value: None,
            return_ty: TypeRef::Unknown,
        }
    }

    /// Allocate a fresh SSA [`ValueId`]. Does NOT register the
    /// value anywhere — the caller is expected to immediately use
    /// it as the `dst` of an instruction, otherwise the value is
    /// dead and the linearizer (Phase 3) will skip it.
    pub fn fresh_value(&mut self) -> ValueId {
        let v = ValueId(self.next_value);
        self.next_value += 1;
        v
    }

    /// Allocate a fresh [`BlockId`]. Phase 0.2a does not call this
    /// (only one block per function is allocated, inline at the top
    /// of [`Builder::build_function`]); the method is here for
    /// Phase 1's split-block codegen.
    #[allow(dead_code)]
    pub fn fresh_block(&mut self) -> BlockId {
        let b = BlockId(self.next_block);
        self.next_block += 1;
        b
    }

    /// Return a mutable reference to the block currently being
    /// built. New [`Inst`]s are appended to
    /// `self.current_block_mut().insts`. Panics if no current
    /// block has been allocated (i.e., [`Builder::build_function`]
    /// has not been called).
    pub fn current_block_mut(&mut self) -> &mut Block {
        let id = self.current;
        &mut self.blocks[id.index()]
    }

    /// Build the CFG for a function declaration. The argument is
    /// the [`Expression::Function`] node; we destructure it inline.
    ///
    /// Returns the completed [`Function`] with one block
    /// (straight-line), a [`Terminator::Return`] (or `Unreachable`
    /// if the body never returns), and the parameters pre-allocated
    /// as `Inst::Param` instructions.
    ///
    /// ## What this does NOT do
    ///
    /// - It does NOT resolve enum tags or arities for
    ///   `Inst::MakeEnum`. Phase 3's linearizer resolves those via
    ///   the typechecker's `tag_for` / `arity_for` helpers. For
    ///   Phase 0.2a, `Construct` emits `MakeEnum { tag: 0, arity:
    ///   0 }` and the linearizer patches it.
    /// - It does NOT resolve field indices for `Inst::LoadField`.
    ///   Same story.
    /// - It does NOT emit the `prologue` (the `CALL / JMP / HALT`
    ///   triplet that starts every program in the existing single-
    ///   pass codegen). The prologue is the linearizer's job.
    pub fn build_function(&mut self, func: &Expression) -> Function {
        // Destructure the function declaration. The parser wraps
        // the function body in `Expression::Block(Vec<Output>)`,
        // so the typical shape is `Function { name, args:
        // Output(Fragment([Argument, ...])), returns: Option<&str>,
        // body: Output(Block([...])) }`.
        let (name, args, body) = match func {
            Expression::Function {
                name, args, body, ..
            } => (*name, args, body),
            _ => panic!(
                "cfg_builder::build_function called on a non-Function expression: {:?}",
                func
            ),
        };

        // 1. Extract parameters from `args`. `args` is a
        //    `Fragment` of `Argument(ty, name)` nodes.
        if let Expression::Fragment(children) = args.1.as_ref() {
            for (i, child) in children.iter().enumerate() {
                if let Expression::Argument(_ty, param_name) = child.1.as_ref() {
                    let v = self.fresh_value();
                    self.params.insert(param_name.to_string(), v);
                    self.param_list.push((v, param_name.to_string()));
                    // Emit `Inst::Param { dst, index }` after the
                    // block is allocated below; remember the index
                    // for now.
                    let _ = i; // index is recomputed below from param_list order
                }
            }
        }

        // 2. Allocate the entry block (block 0).
        let entry = self.fresh_block();
        self.blocks.push(Block::new(entry));
        self.current = entry;

        // 3. Emit `Inst::Param` for each parameter, in source
        //    order. The `index` operand matches the parameter's
        //    position in the function signature.
        //
        //    Borrow-checker note: we collect `(index, dst)` pairs
        //    first to avoid overlapping `&self.param_list` (immutable)
        //    and `self.current_block_mut()` (mutable) borrows.
        let param_indices: Vec<(u16, ValueId)> = self
            .param_list
            .iter()
            .enumerate()
            .map(|(i, (vid, _name))| (i as u16, *vid))
            .collect();
        for (index, dst) in param_indices {
            self.current_block_mut()
                .insts
                .push(Inst::Param { dst, index });
        }

        // 4. Walk the function body. The body's outer shape is
        //    `Expression::Block(Vec<Output>)`; we delegate to
        //    `build_expression` which handles Block / Fragment /
        //    Return / etc. uniformly.
        //
        //    The body's return value (if any) is accumulated in
        //    `self.return_value` via the Return / ImplicitReturn
        //    arms.
        let body_inner = body.1.as_ref();
        self.build_expression(body_inner);

        // 5. Set the entry block's terminator. If the body hit a
        //    `Return` or `ImplicitReturn`, use that value;
        //    otherwise default to `Return(None)` (i.e., the
        //    function returns unit).
        let term = match self.return_value {
            Some(v) => Terminator::Return(Some(v)),
            None => Terminator::Return(None),
        };
        self.current_block_mut().terminator = term;

        // 6. Assemble the Function. `blocks` is moved out of the
        //    builder and the predecessors are filled in by
        //    `fill_predecessors` (a trivial pass for single-block
        //    functions today; structured to handle multi-block
        //    CFGs in Phase 1).
        let mut fn_obj = Function {
            name: name.to_string(),
            params: std::mem::take(&mut self.param_list),
            return_ty: self.return_ty,
            blocks: std::mem::take(&mut self.blocks),
            entry,
        };
        Self::fill_predecessors(&mut fn_obj);

        // 7. Reset per-function state so the builder can be reused
        //    for another function. `next_value` / `next_block` are
        //    intentionally NOT reset (see type docs).
        self.locals.clear();
        self.params.clear();
        self.return_value = None;
        // Note: `self.return_ty` is also reset to `Unknown` for the
        // next function. This is the conservative default; Phase 1
        // will populate it from `func.returns`.
        self.return_ty = TypeRef::Unknown;

        fn_obj
    }

    /// Walk an [`Expression`] and emit the corresponding
    /// [`Inst`]s into the current block. Returns the [`ValueId`]
    /// that holds the expression's value, or `None` if the
    /// expression is void (e.g., `Print`, `Return`, or a bare
    /// `Block` whose children are all void).
    ///
    /// For Phase 0.2a, this is a single straight-line walk: every
    /// instruction lands in `self.current_block_mut()` and no
    /// block splitting happens. Phase 1 will extend the function
    /// to split blocks for control flow.
    pub fn build_expression(&mut self, expr: &Expression) -> Option<ValueId> {
        match expr {
            // ---- Constants ----
            //
            // The AST does NOT have a `Literal` type (see
            // `parser/src/ast.rs`); instead each constant kind is
            // its own variant. We map each to the corresponding
            // CFG instruction.
            Expression::Integer(n) => {
                let dst = self.fresh_value();
                self.current_block_mut()
                    .insts
                    .push(Inst::Const { dst, value: *n });
                Some(dst)
            }
            Expression::Float(f) => {
                let dst = self.fresh_value();
                self.current_block_mut()
                    .insts
                    .push(Inst::ConstF { dst, value: *f });
                Some(dst)
            }
            Expression::Bool(b) => {
                let dst = self.fresh_value();
                self.current_block_mut()
                    .insts
                    .push(Inst::ConstBool { dst, value: *b });
                Some(dst)
            }
            Expression::String(s) => {
                let dst = self.fresh_value();
                self.current_block_mut().insts.push(Inst::ConstString {
                    dst,
                    value: (*s).to_string(),
                });
                Some(dst)
            }

            // ---- Identifier lookup ----
            //
            // Check `locals` first (let-bound), then `params`.
            // Returns the existing ValueId — no instruction
            // emitted, because the value was defined earlier (by
            // an Inst::Param for a parameter, or by an Inst::Const
            // / Inst::BinOp for a let-bound RHS).
            //
            // If the name isn't in either map, the typechecker
            // would have already reported an error upstream. The
            // builder returns `None` defensively (the linearizer
            // will see no consumer for the missing SSA value).
            Expression::Identifier(name) => {
                // `name` is `&&'expr str` from the
                // `Expression::Identifier(&'expr str)`
                // destructure. `HashMap::get` wants `&str`, so we
                // deref once.
                self.locals
                    .get(*name)
                    .or_else(|| self.params.get(*name))
                    .copied()
            }

            // ---- Binary operators ----
            //
            // The AST has one variant per operator (Add, Sub,
            // Mul, ...); each maps to a `BinOpKind` (Add, Sub,
            // ...). The int-vs-float selection (`Add` vs `AddF`)
            // is the linearizer's responsibility — the linearizer
            // has access to the typechecker's `(NodeId) -> Ty`
            // cache and can pick the right variant based on the
            // operand types. For Phase 0.2a, we emit the generic
            // `BinOpKind::Add` and let the linearizer specialize.
            Expression::Add(lhs, rhs) => self.build_binop(lhs, rhs, BinOpKind::Add),
            Expression::Sub(lhs, rhs) => self.build_binop(lhs, rhs, BinOpKind::Sub),
            Expression::Mul(lhs, rhs) => self.build_binop(lhs, rhs, BinOpKind::Mul),
            Expression::Div(lhs, rhs) => self.build_binop(lhs, rhs, BinOpKind::Div),
            Expression::Mod(lhs, rhs) => self.build_binop(lhs, rhs, BinOpKind::Mod),
            Expression::Pow(lhs, rhs) => {
                // No `Pow` in the CFG `BinOpKind` enum (it's not
                // used by the current target VM). For Phase 0.2a
                // we synthesize a `Mul` (the closest semantic
                // analog) and let the linearizer complain if the
                // typechecker has any opinion. This is a
                // placeholder; Phase 1+ will add `Pow` to
                // `BinOpKind` if needed.
                self.build_binop(lhs, rhs, BinOpKind::Mul)
            }
            Expression::Shl(lhs, rhs) => self.build_binop(lhs, rhs, BinOpKind::Shl),
            Expression::Shr(lhs, rhs) => self.build_binop(lhs, rhs, BinOpKind::Shr),
            Expression::Xor(lhs, rhs) => self.build_binop(lhs, rhs, BinOpKind::Xor),
            Expression::BitAnd(lhs, rhs) => self.build_binop(lhs, rhs, BinOpKind::And),
            Expression::BitOr(lhs, rhs) => self.build_binop(lhs, rhs, BinOpKind::Or),
            Expression::Eq(lhs, rhs) => self.build_binop(lhs, rhs, BinOpKind::Eq),
            Expression::Neq(lhs, rhs) => self.build_binop(lhs, rhs, BinOpKind::Neq),
            Expression::Le(lhs, rhs) => self.build_binop(lhs, rhs, BinOpKind::Lt),
            Expression::Leq(lhs, rhs) => self.build_binop(lhs, rhs, BinOpKind::Le),
            Expression::Gt(lhs, rhs) => self.build_binop(lhs, rhs, BinOpKind::Gt),
            Expression::Geq(lhs, rhs) => self.build_binop(lhs, rhs, BinOpKind::Ge),
            Expression::And(lhs, rhs) => self.build_binop(lhs, rhs, BinOpKind::And),
            Expression::Or(lhs, rhs) => self.build_binop(lhs, rhs, BinOpKind::Or),

            // ---- Unary operators ----
            Expression::Negate(operand) => {
                let src = self.build_expression(operand.1.as_ref())?;
                let dst = self.fresh_value();
                self.current_block_mut().insts.push(Inst::UnaryOp {
                    op: UnaryOpKind::Neg,
                    dst,
                    src,
                });
                Some(dst)
            }
            Expression::Not(operand) => {
                let src = self.build_expression(operand.1.as_ref())?;
                let dst = self.fresh_value();
                self.current_block_mut().insts.push(Inst::UnaryOp {
                    op: UnaryOpKind::Not,
                    dst,
                    src,
                });
                Some(dst)
            }
            Expression::Positive(operand) => {
                // `+x` is a no-op unary — the operand is its own
                // value. We delegate to `build_expression` and
                // return the operand's ValueId directly (no fresh
                // allocation, no instruction).
                self.build_expression(operand.1.as_ref())
            }

            // ---- Function call ----
            //
            // Build the callee expression (typically an Identifier
            // lookup, which returns the function's `ValueId`) and
            // each argument. Emit `Inst::Call` with `dst = Some`
            // (Phase 0.2a always keeps the return value; if the
            // caller discards it, the linearizer can elide the
            // value).
            Expression::Call { name, args } => {
                let callee = self.build_expression(name.1.as_ref())?;
                let arg_values = match args {
                    Some(items) => items
                        .iter()
                        .map(|arg| self.build_expression(arg.1.as_ref()))
                        .collect::<Option<Vec<_>>>()?,
                    None => Vec::new(),
                };
                let dst = self.fresh_value();
                self.current_block_mut().insts.push(Inst::Call {
                    dst: Some(dst),
                    callee,
                    args: arg_values,
                });
                Some(dst)
            }

            // ---- Field access ----
            //
            // Build the receiver, then emit `Inst::LoadField` with
            // `field_index = 0` (placeholder). The linearizer (Phase 3)
            // needs the receiver's enum name to compute the correct
            // field index; that information comes from the
            // typechecker's side-table. For Phase 0.2a, we emit the
            // placeholder and trust the linearizer to resolve it.
            Expression::Access(receiver, _field) => {
                let src = self.build_expression(receiver.1.as_ref())?;
                let dst = self.fresh_value();
                self.current_block_mut().insts.push(Inst::LoadField {
                    dst,
                    src,
                    field_index: 0,
                });
                Some(dst)
            }

            // ---- Constructor application ----
            //
            // Build the payload in declaration order. Emit
            // `Inst::MakeEnum` with `tag = 0` and `arity = 0`
            // (placeholders — the linearizer resolves via the
            // typechecker's `tag_for` / `arity_for`).
            //
            // The AST distinguishes Unit / Tuple / Record shapes
            // via `EnumConstructPayload`. For Unit, no payload
            // values are produced. For Tuple and Record, the
            // payload expressions are built in declaration order
            // (the parser ensures source order matches declaration
            // order for tuples; for records, the codegen reorders
            // to declaration order — Phase 0.2a skips the reorder
            // and emits in source order, which is a known
            // limitation that Phase 3 will fix).
            Expression::Construct {
                enum_name: _,
                variant_name: _,
                fields,
            } => {
                use parser::ast::EnumConstructPayload;
                let payload_values = match fields {
                    EnumConstructPayload::Unit => Vec::new(),
                    EnumConstructPayload::Tuple(args) => args
                        .iter()
                        .map(|arg| self.build_expression(arg.1.as_ref()))
                        .collect::<Option<Vec<_>>>()?,
                    EnumConstructPayload::Record(parts) => parts
                        .iter()
                        .map(|part| self.build_expression(part.value.1.as_ref()))
                        .collect::<Option<Vec<_>>>()?,
                };
                let dst = self.fresh_value();
                self.current_block_mut().insts.push(Inst::MakeEnum {
                    dst,
                    tag: 0,
                    payload: payload_values,
                });
                Some(dst)
            }

            // ---- Block ----
            //
            // A `Block(Vec<Output>)` is a sequence of statements.
            // Build each child in source order; the LAST child's
            // value (if any) is the Block's value. Pre-block
            // children with values are emitted but their values
            // become dead — the linearizer can elide them.
            Expression::Block(children) => {
                let mut last_value = None;
                for child in children {
                    last_value = self.build_expression(child.1.as_ref());
                }
                last_value
            }

            // ---- Fragment ----
            //
            // Special-case: `Fragment([Variable(name), rhs])` is
            // the parser's representation of `let name = rhs;`.
            // We register the binding in `locals` so subsequent
            // `Identifier` lookups resolve to the RHS's ValueId.
            //
            // Any other Fragment shape falls through to the
            // general "build each child in sequence" path. This
            // matches the existing single-pass codegen's fallback
            // (see `compiler/src/lib.rs::do_compile`'s Fragment
            // arm, line 1101).
            Expression::Fragment(children) => {
                if children.len() == 2 {
                    if let Expression::Variable(name, _ty) = children[0].1.as_ref() {
                        let rhs_value = self.build_expression(children[1].1.as_ref());
                        if let Some(v) = rhs_value {
                            self.locals.insert((*name).to_string(), v);
                        }
                        return None;
                    }
                }
                let mut last_value = None;
                for child in children {
                    last_value = self.build_expression(child.1.as_ref());
                }
                last_value
            }

            // ---- Print ----
            //
            // Build the format string and each param. The actual
            // print instruction (`FORMAT` + `PRINT` in the
            // existing VM) is the linearizer's job (Phase 3) —
            // for Phase 0.2a, we just emit the side-effecting
            // values and trust the linearizer to wrap them in the
            // right opcodes.
            //
            // Returns `None` because Print is a statement, not an
            // expression (the printed value is the side effect,
            // not a SSA result).
            Expression::Print(fmt, params) => {
                let _fmt_value = self.build_expression(fmt.1.as_ref());
                if let Some(items) = params {
                    for p in items {
                        let _ = self.build_expression(p.1.as_ref());
                    }
                }
                None
            }

            // ---- Format ----
            //
            // Mirrors Print but without the trailing PRINT opcode.
            // The existing codegen treats this identically to
            // Print minus the print emission; Phase 0.2a mirrors
            // that by building the side-effecting values and
            // returning `None`.
            Expression::Format(fmt, params) => {
                let _fmt_value = self.build_expression(fmt.1.as_ref());
                if let Some(items) = params {
                    for p in items {
                        let _ = self.build_expression(p.1.as_ref());
                    }
                }
                None
            }

            // ---- Return / ImplicitReturn ----
            //
            // Build the returned expression and record the value
            // in `self.return_value`. The entry block's terminator
            // is set at the end of `build_function` based on this
            // accumulator. Both variants behave identically in
            // the CFG — the existing codegen's distinction (RETURN
            // vs no-RETURN) is a bytecode-emission concern, not a
            // CFG-structure concern.
            Expression::Return(expr) | Expression::ImplicitReturn(expr) => {
                let v = self.build_expression(expr.1.as_ref());
                self.return_value = v;
                None
            }

            // ---- Assignment ----
            //
            // Reassignment (`x = rhs;`). Build the RHS and update
            // `locals[name]` (where `name` is the LHS
            // identifier). The LHS is typically
            // `Expression::Identifier(name)`; anything else is
            // silently ignored (the typechecker would have
            // reported an error).
            //
            // Note: this is a higher-level SSA "rebinding" — the
            // new ValueId replaces the old one in `locals`. The
            // linearizer (Phase 3) is responsible for emitting the
            // equivalent `STORE_POP` opcode that the existing
            // single-pass codegen emits for this case.
            Expression::Assignment(lhs, rhs) => {
                let new_value = self.build_expression(rhs.1.as_ref());
                if let (Some(v), Expression::Identifier(name)) = (new_value, lhs.1.as_ref()) {
                    self.locals.insert((*name).to_string(), v);
                }
                None
            }

            // ---- Variable (bare, outside Fragment) ----
            //
            // Should not appear in a well-formed AST outside of
            // Fragment's let-binding shape. If we encounter one,
            // we no-op (return None) — the typechecker would have
            // rejected the source upstream.
            Expression::Variable(_, _) => None,

            // ---- Argument / Type ----
            //
            // Metadata variants that should not appear inside a
            // function body. `Argument` is for function signatures
            // (extracted by `build_function` before
            // `build_expression` is called); `Type` is for enum
            // declarations. Both are no-ops here.
            Expression::Argument(_, _) => None,
            Expression::Type(_) => None,

            // ---- Wrappers ----
            //
            // `Statement`, `ExprStatement`, `Expr`, `Group` all
            // wrap a single inner expression. Delegate to the
            // inner expression and return its value.
            Expression::Statement(inner) | Expression::ExprStatement(inner) => {
                self.build_expression(inner.1.as_ref())
            }
            Expression::Expr(inner) | Expression::Group(inner) => {
                self.build_expression(inner.1.as_ref())
            }

            // ---- Top-level / out-of-scope ----
            //
            // These variants describe top-level program structure
            // (or are dead code paths). They should not appear
            // inside a function body. We no-op (return None) to
            // keep the builder non-panicking for malformed ASTs.
            Expression::Program(_) => None,
            Expression::Noop(_) => None,
            Expression::Module(_, _) => None,
            Expression::Comment(_) => None,
            Expression::Use { .. } => None,
            Expression::List(_) => None,
            Expression::Defer(_) => None,
            Expression::Constant(_, _) => None,
            Expression::Implementation(_, _, _) => None,
            Expression::Class(_, _) => None,
            Expression::Field(_, _, _) => None,
            Expression::Method(_, _) => None,
            Expression::Member(_) => None,
            Expression::Update(_, _) => None,
            Expression::Instantiate(_, _) => None,
            Expression::Default(_) => None,
            Expression::Inc(_) => None,
            Expression::Dec(_) => None,
            Expression::Yield(_) => None,
            Expression::Resume(_, _) => None,
            Expression::EnumDecl { .. } => None,
            Expression::EnumVariant { .. } => None,

            // ---- Phase 1: control flow ----
            //
            // These variants require multi-block CFGs and Branch
            // / Switch terminators. Phase 0.2a panics on them so
            // the missing functionality is loud (rather than
            // silently producing incorrect bytecode).
            Expression::If(_) => panic!(
                "cfg_builder::build_expression: `if` is not implemented \
                 in Phase 0.2a (deferred to Phase 1)"
            ),
            Expression::Branch(_, _) => panic!(
                "cfg_builder::build_expression: `if` branch is not \
                 implemented in Phase 0.2a (deferred to Phase 1)"
            ),
            Expression::Loop { .. } => panic!(
                "cfg_builder::build_expression: `loop` is not \
                 implemented in Phase 0.2a (deferred to Phase 1)"
            ),
            Expression::Match { .. } => panic!(
                "cfg_builder::build_expression: `match` is not \
                 implemented in Phase 0.2a (deferred to Phase 1)"
            ),

            // ---- Function-in-function ----
            //
            // Nested function declarations are not supported by
            // the existing single-pass codegen and are out of
            // scope for the CFG builder. Panic loudly so the
            // missing functionality is obvious.
            Expression::Function { .. } => panic!(
                "cfg_builder::build_expression: nested function \
                 declarations are not supported"
            ),
        }
    }

    /// Walk the operands of a binary operator, emit the
    /// corresponding `Inst::BinOp`, and return the destination
    /// `ValueId`. Helper to keep the per-operator arms in
    /// `build_expression` short.
    ///
    /// If either operand fails to produce a value (e.g., a
    /// type-checker error upstream), this returns `None` and emits
    /// no instruction — the linearizer will see the missing
    /// operand and skip the BinOp emission.
    fn build_binop(
        &mut self,
        lhs: &Output,
        rhs: &Output,
        op: BinOpKind,
    ) -> Option<ValueId> {
        let lhs_v = self.build_expression(lhs.1.as_ref())?;
        let rhs_v = self.build_expression(rhs.1.as_ref())?;
        let dst = self.fresh_value();
        self.current_block_mut().insts.push(Inst::BinOp {
            op,
            dst,
            lhs: lhs_v,
            rhs: rhs_v,
        });
        Some(dst)
    }

    /// Fill in the `predecessors` field of every block by walking
    /// each block's terminator. For Phase 0.2a this is trivial
    /// (single block with `Terminator::Return` has no successors),
    /// but the function is structured to handle multi-block CFGs in
    /// Phase 1.
    ///
    /// Algorithm:
    ///   1. Clear every block's `predecessors` (so the function is
    ///      idempotent if called twice on the same Function).
    ///   2. For each block, determine its successors from the
    ///      terminator:
    ///        - `Terminator::Jump(bb)` → `[bb]`
    ///        - `Terminator::Branch { true_bb, false_bb, .. }` →
    ///          `[true_bb, false_bb]`
    ///        - `Terminator::Switch { cases, default, .. }` →
    ///          `[cases..., default]`
    ///        - `Terminator::Return` / `Terminator::Unreachable` →
    ///          `[]`
    ///   3. For each successor, append the current block's ID to
    ///      the successor's `predecessors` (deduplicating).
    fn fill_predecessors(func: &mut Function) {
        // Step 1: clear all predecessors.
        for block in &mut func.blocks {
            block.predecessors.clear();
        }

        // Step 2: compute successors for each block. Collect
        // (block_id, successor_ids) pairs first to avoid borrow
        // conflicts in step 3.
        let successors: Vec<(BlockId, Vec<BlockId>)> = func
            .blocks
            .iter()
            .map(|b| {
                let succs = match &b.terminator {
                    Terminator::Jump(bb) => vec![*bb],
                    Terminator::Branch {
                        true_bb,
                        false_bb,
                        ..
                    } => vec![*true_bb, *false_bb],
                    Terminator::Switch { cases, default, .. } => {
                        let mut s: Vec<BlockId> =
                            cases.iter().map(|(_, bb)| *bb).collect();
                        s.push(*default);
                        s
                    }
                    Terminator::Return(_) | Terminator::Unreachable => Vec::new(),
                };
                (b.id, succs)
            })
            .collect();

        // Step 3: for each (pred, succs) pair, append `pred` to
        // each successor's predecessors list (deduplicating).
        for (pred_id, succs) in successors {
            for succ_id in succs {
                if let Some(succ_block) = func.blocks.get_mut(succ_id.index()) {
                    if !succ_block.predecessors.contains(&pred_id) {
                        succ_block.predecessors.push(pred_id);
                    }
                }
            }
        }
    }
}

// (End of file.)