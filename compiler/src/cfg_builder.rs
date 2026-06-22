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

        self.build_function_from_parts(name, args.1.as_ref(), body.1.as_ref())
    }

    /// Build a CFG [`Function`] from the function's `name`, `args`,
    /// and `body` directly, without requiring an
    /// [`Expression::Function`] wrapper.
    ///
    /// ## Phase 0.3 integration
    ///
    /// The compiler's [`do_compile`](crate::Compiler::do_compile) uses
    /// this entry point to integrate the CFG path with the existing
    /// single-pass codegen. It destructures the original
    /// `Expression::Function` arm and forwards the fields directly,
    /// avoiding the cost of cloning `args` and `body` to synthesize a
    /// new `Expression::Function` wrapper.
    ///
    /// `args` is expected to be an [`Expression::Fragment`] of
    /// [`Expression::Argument`] children; `body` is the function body
    /// (typically an [`Expression::Block`]).
    ///
    /// The semantics are IDENTICAL to [`Builder::build_function`]
    /// when called on an `Expression::Function` with the same fields.
    /// Splitting the entry point is purely a convenience for the
    /// codegen integration.
    pub fn build_function_from_parts<'a>(
        &mut self,
        name: &'a str,
        args: &Expression<'a>,
        body: &Expression<'a>,
    ) -> Function {
        // 1. Extract parameters from `args`. `args` is a
        //    `Fragment` of `Argument(ty, name)` nodes.
        if let Expression::Fragment(children) = args {
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
        self.build_expression(body);

        // 5. Set the terminator for the function body's
        //    continuation point. For Phase 0.2a (straight-line
        //    code), `self.current` is still the entry block — we
        //    overwrite its `Unreachable` terminator with `Return`.
        //    For Phase 1.0+ (multi-block control flow), the
        //    body's continuation may have moved `self.current` to
        //    a join block (after an `if`); in that case we set
        //    the join block's terminator and LEAVE the entry
        //    block's terminator alone (it was set to `Branch` by
        //    `build_if` and is authoritative).
        //
        //    The two invariants are:
        //    - The current block (join block, or entry block in
        //      the straight-line case) always ends in `Return`.
        //    - The entry block is only overwritten when its
        //      terminator is still `Unreachable` — i.e., the body
        //      was straight-line and step 1's `self.current`
        //      assignment IS the entry block.
        let term = match self.return_value {
            Some(v) => Terminator::Return(Some(v)),
            None => Terminator::Return(None),
        };
        self.current_block_mut().terminator = term.clone();

        let entry_block = &mut self.blocks[entry.index()];
        if matches!(entry_block.terminator, Terminator::Unreachable) {
            // Straight-line body — the entry block was the
            // current block in step 1, and step 1 already set
            // it to `term`. The condition is true only if the
            // body emitted NO instructions that advanced the
            // current block (e.g., a no-op body), in which case
            // we redundantly set the same terminator. Safe.
            entry_block.terminator = term;
        }
        // Otherwise: the entry block's terminator was set by
        // `build_if` to `Branch` (or by some future control-
        // flow helper to `Jump` / `Switch`). The Branch is the
        // authoritative control transfer; do not overwrite.

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
            // `If` is the only control-flow variant implemented in
            // Phase 1.0. It produces a multi-block CFG with
            // `Branch` terminators — see the `build_if` helper
            // below for the block structure. `Loop` and `Match`
            // remain deferred to later sub-phases.
            //
            // `Branch` is a helper variant that ONLY appears as a
            // child of `If`. If it appears at the top level (out
            // of context), it's a malformed AST — panic loudly.
            Expression::If(branches) => self.build_if(branches),
            Expression::Branch(_, _) => panic!(
                "cfg_builder::build_expression: `Branch` appeared \
                 outside of an `If` context (malformed AST)"
            ),
            Expression::Loop { .. } => panic!(
                "cfg_builder::build_expression: `loop` is not \
                 implemented in Phase 1.0 (deferred to Phase 1.1+)"
            ),
            Expression::Match { .. } => panic!(
                "cfg_builder::build_expression: `match` is not \
                 implemented in Phase 1.0 (deferred to Phase 1.1+)"
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

    /// Build a multi-block CFG for an `if` (or `if/else if/else`)
    /// expression.
    ///
    /// Each branch's body becomes its own basic block, joined at
    /// a fresh `join_block` where execution continues after the
    /// `if`.
    ///
    /// ## Block structure
    ///
    /// For `if cond { then_branch } else { else_branch }`:
    ///
    /// ```text
    /// [entry]
    ///   ...
    ///   cond = ...
    ///   Branch cond_v → then_block, else_block
    ///
    /// [then_block]
    ///   ...then_branch body...
    ///   Jump join_block
    ///
    /// [else_block]
    ///   ...else_branch body...
    ///   Jump join_block
    ///
    /// [join_block]
    ///   ...continuation...
    /// ```
    ///
    /// For `if cond { then_branch }` (no `else`), the `Branch`
    /// terminator's false arm points directly at `join_block` —
    /// no separate else block is allocated.
    ///
    /// For `if c1 { b1 } else if c2 { b2 } else { b3 }`, the
    /// false arm of branch 0's `Branch` points at a fresh
    /// fallthrough block where `c2` is evaluated, and the false
    /// arm of branch 1's `Branch` points at the else block (the
    /// last branch, with `cond = None`, executes its body
    /// unconditionally in its current block).
    ///
    /// ## Block ID assignment
    ///
    /// `self.blocks` is a `Vec<Block>` indexed by `BlockId`. We
    /// allocate and push blocks in `BlockId` order so that
    /// `self.blocks[i].id == BlockId(i)`. This is what makes
    /// `self.current_block_mut()` correct (it indexes
    /// `self.blocks[self.current.index()]`).
    ///
    /// Block ID assignment:
    ///
    /// | BlockId | Role                     |
    /// |---------|--------------------------|
    /// | 0       | entry (already pushed)   |
    /// | 1       | join_block               |
    /// | 2       | then_0                   |
    /// | 3       | false_target_0 (= next branch's cond / else) |
    /// | 4       | then_1                   |
    /// | 5       | false_target_1 (= next branch's cond / else) |
    /// | ...     | (etc.)                   |
    ///
    /// For `if cond { body }` (no else): the Branch's false arm
    /// points at `join_block` directly (no separate false_target
    /// is allocated).
    ///
    /// ## Value handling
    ///
    /// Returns `None`. If-expressions as values require phi-nodes
    /// at the join (the value depends on which branch was taken);
    /// SSA-lite punts on this. Users can use let-bindings or
    /// statement-form `if` to handle values.
    ///
    /// ## Phase 1.0 limitations
    ///
    /// - Returns in nested blocks are tracked via
    ///   `self.return_value` but the terminator of the nested
    ///   block is still set to `Jump(join_block)`. The
    ///   `return_value` accumulator reflects the LAST return
    ///   encountered (which may be inside a nested block), and
    ///   the join block's terminator uses that value. Proper
    ///   handling of returns in nested blocks is Phase 1.1+
    ///   work.
    /// - The condition's `ValueId` is obtained from
    ///   `build_expression`. If it returns `None` (defensive
    ///   recovery — the typechecker should ensure the cond has
    ///   a value), we substitute a fresh `ValueId`. The
    ///   linearizer may emit undefined-value diagnostics in this
    ///   corner case; in practice, well-formed programs always
    ///   produce a cond value.
    fn build_if<'a>(&mut self, branches: &'a [Output<'a>]) -> Option<ValueId> {
        // Defensive: an empty `If` produces no CFG. This
        // shouldn't happen for well-formed ASTs (the parser
        // always wraps `if cond { body }` in a single-branch
        // `If`), but we no-op rather than panic to keep the
        // builder non-panicking for malformed ASTs.
        if branches.is_empty() {
            return None;
        }

        let num_branches = branches.len();

        // Allocate and push the join_block FIRST so its
        // BlockId (= 1) is in the canonical "lowest index after
        // entry" position. The join's CONTENTS are filled later
        // (by the body's continuation code), but its INDEX must
        // be stable so that the Branch / Jump targets can be
        // resolved via `self.blocks[BlockId(1).index()]`.
        let join_block = self.fresh_block();
        self.blocks.push(Block::new(join_block));

        for (i, branch_output) in branches.iter().enumerate() {
            let branch_expr = branch_output.1.as_ref();
            let (cond_opt, body) = match branch_expr {
                Expression::Branch(c, b) => (c.as_ref(), b),
                other => panic!(
                    "cfg_builder::build_if: If branch must be \
                     Expression::Branch, got {:?}",
                    other
                ),
            };

            let is_last = i + 1 == num_branches;

            match cond_opt {
                Some(cond_output) => {
                    // Branch with a condition. Build cond in the
                    // CURRENT block (which is either the entry,
                    // or the previous iteration's false_target).
                    let cond_v = self
                        .build_expression(cond_output.1.as_ref())
                        .unwrap_or_else(|| self.fresh_value());

                    // Push then_block (BlockId 2, 4, ...).
                    let then_block = self.fresh_block();
                    self.blocks.push(Block::new(then_block));

                    // Push false_target if not last.
                    //
                    // For the last branch with cond=Some, the
                    // false arm goes directly to join_block (no
                    // else branch — fall through to the join).
                    //
                    // For non-last branches, push a fresh
                    // false_target (BlockId 3, 5, ...) — this
                    // becomes either the next branch's cond
                    // evaluation block, or (if the next branch
                    // has cond=None) the else block where the
                    // body's code goes.
                    let false_target = if is_last {
                        join_block
                    } else {
                        let ft = self.fresh_block();
                        self.blocks.push(Block::new(ft));
                        ft
                    };

                    // Set the current block's terminator to
                    // Branch.
                    self.current_block_mut().terminator =
                        Terminator::Branch {
                            cond: cond_v,
                            true_bb: then_block,
                            false_bb: false_target,
                        };

                    // Switch to then_block, build body, set
                    // Jump(join_block).
                    self.current = then_block;
                    let _then_value =
                        self.build_expression(body.1.as_ref());
                    self.current_block_mut().terminator =
                        Terminator::Jump(join_block);

                    // Advance current to false_target for the
                    // next iteration. If false_target IS
                    // join_block (last branch with cond=Some,
                    // no else), we don't advance — the body's
                    // continuation code will land in join_block
                    // after the loop ends.
                    if !is_last {
                        self.current = false_target;
                    }
                }
                None => {
                    // Else branch (no condition). The current
                    // block (which was the previous iteration's
                    // false_target) IS where the body goes.
                    //
                    // Defensive: `cond = None` should only
                    // appear in the LAST branch (i.e., the
                    // `else` of an `if c { b } else { e }`).
                    // If it appears earlier, the AST is
                    // malformed.
                    if !is_last {
                        panic!(
                            "cfg_builder::build_if: `else` \
                             branch (cond=None) is not the \
                             last branch — malformed AST"
                        );
                    }

                    let _body_value =
                        self.build_expression(body.1.as_ref());
                    self.current_block_mut().terminator =
                        Terminator::Jump(join_block);
                    // No advance — there should be no more
                    // branches.
                }
            }
        }

        // After all branches, set self.current to the
        // join_block. The body's continuation code (anything
        // after the if in the parent block) will land in
        // join_block.
        self.current = join_block;

        // Phase 1.0: if-expressions don't produce values
        // (phi-nodes at the join would be needed for that).
        None
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

#[cfg(test)]
mod tests {
    //! Unit tests for the straight-line CFG builder.
    //!
    //! ## Construction approach
    //!
    //! AST `Expression<'expr>` nodes are constructed directly using
    //! `'static` lifetimes. All string literals are `&'static str`,
    //! and `Output<'static>` tuples are built with helper functions
    //! (`e`, `ident`, `int`, etc.) at the top of this module. No
    //! `Box::leak` is used — `'static` is sufficient because every
    //! test function has a string-literal source it can borrow from
    //! for the lifetime of the program.
    //!
    //! This mirrors the lifetime pattern that production code will
    //! see (the parser produces `Expression<'src>` borrowed from the
    //! source string), and avoids the parser entirely so the tests
    //! exercise the builder in isolation.
    //!
    //! `SimpleSpan` is constructed directly via its public fields
    //! (`start`, `end`, `context`) — see the chumsky source for the
    //! struct definition.

    use super::*;
    use crate::cfg::{BinOpKind, BlockId, ValueId};
    use parser::SimpleSpan;
    use parser::ast::Expression;

    // -----------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------

    /// Zero-width span with unit context — the placeholder span used
    /// by every helper-built `Output`. The CFG builder ignores span
    /// contents for non-diagnostic code paths, so a uniform placeholder
    /// is fine.
    fn span() -> SimpleSpan {
        SimpleSpan {
            start: 0,
            end: 0,
            context: (),
        }
    }

    /// Wrap an `Expression` in the `Output` tuple shape the builder
    /// expects: `(span, Box<Expression>)`.
    fn e(inner: Expression<'static>) -> Output<'static> {
        (span(), Box::new(inner))
    }

    fn ident(name: &'static str) -> Expression<'static> {
        Expression::Identifier(name)
    }

    fn int(n: i64) -> Expression<'static> {
        Expression::Integer(n)
    }

    fn float(f: f64) -> Expression<'static> {
        Expression::Float(f)
    }

    fn bool_lit(b: bool) -> Expression<'static> {
        Expression::Bool(b)
    }

    fn string_lit(s: &'static str) -> Expression<'static> {
        Expression::String(s)
    }

    fn add(lhs: Expression<'static>, rhs: Expression<'static>) -> Expression<'static> {
        Expression::Add(e(lhs), e(rhs))
    }

    fn mul(lhs: Expression<'static>, rhs: Expression<'static>) -> Expression<'static> {
        Expression::Mul(e(lhs), e(rhs))
    }

    fn div(lhs: Expression<'static>, rhs: Expression<'static>) -> Expression<'static> {
        Expression::Div(e(lhs), e(rhs))
    }

    /// `let name = rhs;` — produces the Fragment shape that the parser
    /// emits. The builder's special-case at `build_expression`'s
    /// `Fragment` arm treats `Fragment([Variable(name), rhs])` as a
    /// let-binding.
    fn let_binding(name: &'static str, rhs: Expression<'static>) -> Expression<'static> {
        Expression::Fragment(vec![e(Expression::Variable(name, None)), e(rhs)])
    }

    /// Wrap an inner expression in `Statement(...)` — matches what the
    /// parser produces for `expr;` and `let x = ...;` and `return ...;`.
    fn stmt(inner: Expression<'static>) -> Expression<'static> {
        Expression::Statement(e(inner))
    }

    /// A body-shaped block: `Block([stmt, stmt, ...])`.
    fn block(children: Vec<Expression<'static>>) -> Expression<'static> {
        Expression::Block(children.into_iter().map(e).collect())
    }

    /// `return expr;`
    fn ret(inner: Expression<'static>) -> Expression<'static> {
        Expression::Return(e(inner))
    }

    /// One function parameter: `Argument(ty, name)`.
    fn argument(ty: &'static str, name: &'static str) -> Expression<'static> {
        Expression::Argument(ty, name)
    }

    /// Build a complete `Expression::Function`. The `returns` field is
    /// ignored by the builder (it defaults to `TypeRef::Unknown`) but
    /// is parameterized for realism — most tests use `Some("int")`.
    fn function(
        name: &'static str,
        args: Vec<(&'static str, &'static str)>,
        returns: Option<&'static str>,
        body: Expression<'static>,
    ) -> Expression<'static> {
        let args_vec: Vec<Output<'static>> = args
            .into_iter()
            .map(|(ty, arg_name)| e(argument(ty, arg_name)))
            .collect();
        Expression::Function {
            name,
            args: e(Expression::Fragment(args_vec)),
            returns,
            body: e(body),
        }
    }

    /// Build a single `Branch` AST node: `Branch(Option<Output>,
    /// Output)`. The `cond` is `None` for an `else` branch.
    /// `cond` of `None` produces `Branch(None, body)` (the else
    /// form).
    fn branch(
        cond: Option<Expression<'static>>,
        body: Expression<'static>,
    ) -> Expression<'static> {
        Expression::Branch(cond.map(e), e(body))
    }

    /// Build an `if` (or `if/else if/else`) AST node from a
    /// sequence of `(Option<cond>, body)` pairs.
    ///
    /// Each pair becomes an `Expression::Branch`. The last pair
    /// SHOULD have `cond = None` (representing `else`); a non-
    /// last `cond = None` is malformed and would panic at
    /// `build_if` time.
    fn if_expr(
        branches: Vec<(Option<Expression<'static>>, Expression<'static>)>,
    ) -> Expression<'static> {
        let branches_out: Vec<Output<'static>> = branches
            .into_iter()
            .map(|(cond, body)| e(branch(cond, body)))
            .collect();
        Expression::If(branches_out)
    }

    /// Convenience for `if cond { body }` — single branch with a
    /// condition, no else.
    fn if_single(
        cond: Expression<'static>,
        body: Expression<'static>,
    ) -> Expression<'static> {
        if_expr(vec![(Some(cond), body)])
    }

    /// Convenience for `if cond { then_b } else { else_b }` — two
    /// branches, the second with no condition (the else).
    fn if_else(
        cond: Expression<'static>,
        then_b: Expression<'static>,
        else_b: Expression<'static>,
    ) -> Expression<'static> {
        if_expr(vec![
            (Some(cond), then_b),
            (None, else_b),
        ])
    }

    /// Convenience for
    /// `if c1 { b1 } else if c2 { b2 } else { b3 }` — three
    /// branches.
    fn if_else_if_else(
        c1: Expression<'static>,
        b1: Expression<'static>,
        c2: Expression<'static>,
        b2: Expression<'static>,
        b3: Expression<'static>,
    ) -> Expression<'static> {
        if_expr(vec![
            (Some(c1), b1),
            (Some(c2), b2),
            (None, b3),
        ])
    }

    // -----------------------------------------------------------------
    // Basic expressions: constants
    // -----------------------------------------------------------------

    #[test]
    fn build_int_constant_produces_const_inst() {
        // `fn f() -> int { return 42; }`
        let func = function(
            "f",
            vec![],
            Some("int"),
            block(vec![stmt(ret(int(42)))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        assert_eq!(cfg.blocks.len(), 1, "expected single block");
        let blk = &cfg.blocks[0];

        // Exactly one Const(42) instruction.
        let consts: Vec<_> = blk
            .insts
            .iter()
            .filter_map(|i| match i {
                Inst::Const { dst, value } => Some((*dst, *value)),
                _ => None,
            })
            .collect();
        assert_eq!(
            consts.len(),
            1,
            "expected 1 Const inst, got {:?}",
            blk.insts
        );
        assert_eq!(consts[0], (ValueId(0), 42));

        // Terminator is Return(Some(v0)).
        assert!(
            matches!(blk.terminator, Terminator::Return(Some(v)) if v == ValueId(0)),
            "expected Return(Some(v0)), got {:?}",
            blk.terminator
        );
    }

    #[test]
    fn build_float_constant_produces_constf_inst() {
        // `fn f() -> float { return 3.14; }`
        let func = function(
            "f",
            vec![],
            Some("float"),
            block(vec![stmt(ret(float(3.14)))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);
        let blk = &cfg.blocks[0];

        let has_constf = blk
            .insts
            .iter()
            .any(|i| matches!(i, Inst::ConstF { value, .. } if (*value - 3.14).abs() < 1e-9));
        assert!(
            has_constf,
            "expected ConstF(3.14) in {:?}",
            blk.insts
        );
        assert!(matches!(blk.terminator, Terminator::Return(Some(_))));
    }

    #[test]
    fn build_bool_constant_produces_constbool_inst() {
        // `fn f() -> bool { return true; }`
        let func = function(
            "f",
            vec![],
            Some("bool"),
            block(vec![stmt(ret(bool_lit(true)))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);
        let blk = &cfg.blocks[0];

        let has_constbool = blk.insts.iter().any(|i| matches!(
            i,
            Inst::ConstBool { value: true, .. }
        ));
        assert!(
            has_constbool,
            "expected ConstBool(true) in {:?}",
            blk.insts
        );
        assert!(matches!(blk.terminator, Terminator::Return(Some(_))));
    }

    #[test]
    fn build_string_constant_produces_conststring_inst() {
        // `fn f() -> string { return "hello"; }`
        let func = function(
            "f",
            vec![],
            Some("string"),
            block(vec![stmt(ret(string_lit("hello")))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);
        let blk = &cfg.blocks[0];

        let has_conststring = blk.insts.iter().any(|i| matches!(
            i,
            Inst::ConstString { value, .. } if value == "hello"
        ));
        assert!(
            has_conststring,
            "expected ConstString(\"hello\") in {:?}",
            blk.insts
        );
        assert!(matches!(blk.terminator, Terminator::Return(Some(_))));
    }

    // -----------------------------------------------------------------
    // Identifier lookup: function params
    // -----------------------------------------------------------------

    #[test]
    fn build_identifier_param_lookup() {
        // `fn f(int x) -> int { return x; }`
        let func = function(
            "f",
            vec![("int", "x")],
            Some("int"),
            block(vec![stmt(ret(ident("x")))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);
        let blk = &cfg.blocks[0];

        // Exactly one Param (index 0, dst v0).
        let params: Vec<_> = blk
            .insts
            .iter()
            .filter_map(|i| match i {
                Inst::Param { dst, index } => Some((*dst, *index)),
                _ => None,
            })
            .collect();
        assert_eq!(params.len(), 1, "expected 1 Param inst");
        assert_eq!(params[0], (ValueId(0), 0));

        // No other insts (Identifier lookups don't emit insts).
        assert_eq!(
            blk.insts.len(),
            1,
            "expected only the Param inst, got {:?}",
            blk.insts
        );

        // Terminator is Return(Some(v0)) — the param's ValueId.
        assert!(
            matches!(blk.terminator, Terminator::Return(Some(v)) if v == ValueId(0)),
            "expected Return(Some(v0)), got {:?}",
            blk.terminator
        );

        // params list has one entry: (v0, "x").
        assert_eq!(cfg.params, vec![(ValueId(0), "x".to_string())]);
    }

    // -----------------------------------------------------------------
    // Binary expressions
    // -----------------------------------------------------------------

    #[test]
    fn build_add_produces_binop_inst() {
        // `fn f(int a, int b) -> int { return a + b; }`
        let func = function(
            "f",
            vec![("int", "a"), ("int", "b")],
            Some("int"),
            block(vec![stmt(ret(add(ident("a"), ident("b"))))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);
        let blk = &cfg.blocks[0];

        // 2 Param + 1 BinOp(Add) = 3 insts.
        assert_eq!(blk.insts.len(), 3, "insts: {:?}", blk.insts);

        // First two are Params (a → v0, b → v1).
        match &blk.insts[0] {
            Inst::Param { dst, index } => {
                assert_eq!(*dst, ValueId(0));
                assert_eq!(*index, 0);
            }
            other => panic!("expected Param, got {:?}", other),
        }
        match &blk.insts[1] {
            Inst::Param { dst, index } => {
                assert_eq!(*dst, ValueId(1));
                assert_eq!(*index, 1);
            }
            other => panic!("expected Param, got {:?}", other),
        }

        // Third is BinOp(Add, v2, v0, v1).
        match &blk.insts[2] {
            Inst::BinOp {
                op,
                dst,
                lhs,
                rhs,
            } => {
                assert_eq!(*op, BinOpKind::Add);
                assert_eq!(*dst, ValueId(2));
                assert_eq!(*lhs, ValueId(0));
                assert_eq!(*rhs, ValueId(1));
            }
            other => panic!("expected BinOp(Add), got {:?}", other),
        }

        assert!(
            matches!(blk.terminator, Terminator::Return(Some(v)) if v == ValueId(2)),
            "expected Return(Some(v2)), got {:?}",
            blk.terminator
        );
    }

    #[test]
    fn build_nested_binary_uses_correct_ssa_order() {
        // `fn f(int a, int b, int c) -> int { return a + b * c; }`
        //
        // The parser's Pratt precedence puts `*` higher than `+`, so
        // this parses as `Add(a, Mul(b, c))`. The builder walks the
        // AST in pre-order, so the Mul is built BEFORE the Add:
        //
        //   Params: a → v0, b → v1, c → v2
        //   Mul:    v3 = b * c
        //   Add:    v4 = a + v3
        //
        // The Mul's ValueId is freshly minted before the Add's, even
        // though it's nested under the Add — the walk is
        // left-then-right-then-self.
        let func = function(
            "f",
            vec![("int", "a"), ("int", "b"), ("int", "c")],
            Some("int"),
            block(vec![stmt(ret(add(ident("a"), mul(ident("b"), ident("c")))))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);
        let blk = &cfg.blocks[0];

        // 3 Params + 1 Mul + 1 Add = 5 insts.
        assert_eq!(blk.insts.len(), 5, "insts: {:?}", blk.insts);

        // Inst 3 (after the 3 Params) is the Mul.
        match &blk.insts[3] {
            Inst::BinOp {
                op,
                dst,
                lhs,
                rhs,
            } => {
                assert_eq!(*op, BinOpKind::Mul);
                assert_eq!(*dst, ValueId(3));
                assert_eq!(*lhs, ValueId(1), "lhs should be b's v1");
                assert_eq!(*rhs, ValueId(2), "rhs should be c's v2");
            }
            other => panic!("expected BinOp(Mul), got {:?}", other),
        }

        // Inst 4 is the Add, using the Mul's result as its rhs.
        match &blk.insts[4] {
            Inst::BinOp {
                op,
                dst,
                lhs,
                rhs,
            } => {
                assert_eq!(*op, BinOpKind::Add);
                assert_eq!(*dst, ValueId(4));
                assert_eq!(*lhs, ValueId(0), "lhs should be a's v0");
                assert_eq!(*rhs, ValueId(3), "rhs should be the Mul's v3");
            }
            other => panic!("expected BinOp(Add), got {:?}", other),
        }

        // Terminator returns the Add's result (v4).
        assert!(
            matches!(blk.terminator, Terminator::Return(Some(v)) if v == ValueId(4)),
            "expected Return(Some(v4)), got {:?}",
            blk.terminator
        );

        // SSA values used: v0..=v4 — exactly 5 distinct ValueIds.
        let mut seen = std::collections::HashSet::new();
        for inst in &blk.insts {
            let vids: Vec<ValueId> = match inst {
                Inst::Param { dst, .. } => vec![*dst],
                Inst::Const { dst, .. } => vec![*dst],
                Inst::BinOp { dst, lhs, rhs, .. } => vec![*dst, *lhs, *rhs],
                _ => vec![],
            };
            for v in vids {
                seen.insert(v);
            }
        }
        assert_eq!(
            seen.len(),
            5,
            "expected 5 distinct SSA values, got {:?}",
            seen
        );
    }

    #[test]
    fn build_division_produces_div_inst() {
        // `fn f(int a, int b) -> int { return a / b; }`
        let func = function(
            "f",
            vec![("int", "a"), ("int", "b")],
            Some("int"),
            block(vec![stmt(ret(div(ident("a"), ident("b"))))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);
        let blk = &cfg.blocks[0];

        // Find the BinOp and check its op kind.
        let binop_op = blk.insts.iter().find_map(|i| match i {
            Inst::BinOp { op, .. } => Some(*op),
            _ => None,
        });
        assert_eq!(
            binop_op,
            Some(BinOpKind::Div),
            "expected BinOp(Div) in {:?}",
            blk.insts
        );
    }

    // -----------------------------------------------------------------
    // Let bindings
    // -----------------------------------------------------------------

    #[test]
    fn build_let_binding_creates_local() {
        // `fn f(int x) -> int { let y = x; return y; }`
        //
        // The let-binding's RHS is just an identifier lookup, so the
        // block emits:
        //   Param(v0, 0)  -- x
        //   (no inst for let, but locals["y"] = v0)
        //   (no inst for return y, just terminator Return(Some(v0)))
        let func = function(
            "f",
            vec![("int", "x")],
            Some("int"),
            block(vec![
                stmt(let_binding("y", ident("x"))),
                stmt(ret(ident("y"))),
            ]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);
        let blk = &cfg.blocks[0];

        // Only the Param — no other insts (let-bindings and
        // Identifier lookups don't emit instructions).
        assert_eq!(
            blk.insts.len(),
            1,
            "expected only the Param, got {:?}",
            blk.insts
        );
        assert!(matches!(
            blk.insts[0],
            Inst::Param {
                dst: ValueId(0),
                index: 0,
            }
        ));

        // Return references the same v0 (y resolves to x's ValueId).
        assert!(
            matches!(blk.terminator, Terminator::Return(Some(v)) if v == ValueId(0)),
            "expected Return(Some(v0)) — `y` should share x's ValueId"
        );
    }

    #[test]
    fn build_let_binding_with_expression() {
        // `fn f(int x) -> int { let y = x + 1; return y; }`
        //
        // Block emits:
        //   Param(v0, 0)  -- x
        //   Const(v1, 1)
        //   BinOp(Add, v2, v0, v1)
        //   (no inst for return y; terminator uses v2)
        let func = function(
            "f",
            vec![("int", "x")],
            Some("int"),
            block(vec![
                stmt(let_binding("y", add(ident("x"), int(1)))),
                stmt(ret(ident("y"))),
            ]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);
        let blk = &cfg.blocks[0];

        // 3 insts: Param, Const, BinOp.
        assert_eq!(blk.insts.len(), 3, "insts: {:?}", blk.insts);

        // Inst 1 is Const(1).
        match &blk.insts[1] {
            Inst::Const { dst, value } => {
                assert_eq!(*dst, ValueId(1));
                assert_eq!(*value, 1);
            }
            other => panic!("expected Const(1), got {:?}", other),
        }

        // Inst 2 is BinOp(Add, v2, v0, v1).
        match &blk.insts[2] {
            Inst::BinOp {
                op,
                dst,
                lhs,
                rhs,
            } => {
                assert_eq!(*op, BinOpKind::Add);
                assert_eq!(*dst, ValueId(2));
                assert_eq!(*lhs, ValueId(0));
                assert_eq!(*rhs, ValueId(1));
            }
            other => panic!("expected BinOp(Add), got {:?}", other),
        }

        // Return references v2 (y's ValueId).
        assert!(
            matches!(blk.terminator, Terminator::Return(Some(v)) if v == ValueId(2)),
            "expected Return(Some(v2))"
        );
    }

    #[test]
    fn build_multiple_let_bindings() {
        // `fn f(int x) -> int { let a = x; let b = a + 1; return b; }`
        //
        // Emits:
        //   Param(v0, 0)  -- x
        //   Const(v1, 1)
        //   BinOp(Add, v2, v0, v1)  -- a+1, where a = x = v0
        //
        // locals["a"] = v0  (first let)
        // locals["b"] = v2  (second let)
        //
        // Return(Some(v2)) — b's ValueId.
        let func = function(
            "f",
            vec![("int", "x")],
            Some("int"),
            block(vec![
                stmt(let_binding("a", ident("x"))),
                stmt(let_binding("b", add(ident("a"), int(1)))),
                stmt(ret(ident("b"))),
            ]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);
        let blk = &cfg.blocks[0];

        // 3 insts: Param + Const + BinOp.
        assert_eq!(blk.insts.len(), 3, "insts: {:?}", blk.insts);

        // Last inst is the Add (a + 1).
        match &blk.insts[2] {
            Inst::BinOp { dst, .. } => assert_eq!(*dst, ValueId(2)),
            other => panic!("expected BinOp, got {:?}", other),
        }

        // Return references v2 (b = a+1).
        assert!(
            matches!(blk.terminator, Terminator::Return(Some(v)) if v == ValueId(2)),
            "expected Return(Some(v2))"
        );
    }

    // -----------------------------------------------------------------
    // Return variants
    // -----------------------------------------------------------------

    #[test]
    fn build_return_with_value() {
        // `fn f() -> int { return 42; }`
        //
        // The Return arm sets `return_value = Some(v0)`; the
        // terminator is emitted at the end of `build_function`.
        let func = function(
            "f",
            vec![],
            Some("int"),
            block(vec![stmt(ret(int(42)))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);
        let blk = &cfg.blocks[0];

        // Exactly one Const (the literal 42).
        let const_count = blk
            .insts
            .iter()
            .filter(|i| matches!(i, Inst::Const { .. }))
            .count();
        assert_eq!(const_count, 1);

        // Terminator: Return(Some(v0)).
        match &blk.terminator {
            Terminator::Return(Some(v)) => assert_eq!(*v, ValueId(0)),
            other => panic!("expected Return(Some(v0)), got {:?}", other),
        }
    }

    #[test]
    fn build_return_without_value() {
        // `fn f() { let y = 5; }`
        //
        // No explicit Return / ImplicitReturn — the body's last child
        // is a let-binding (Statement wrapping Fragment), which
        // returns None. So `return_value` stays None and the
        // terminator is `Return(None)` — the function returns unit.
        //
        // Note: the body still emits the Const(5) inst for the
        // let-binding's RHS; only the RETURN value is absent.
        let func = function(
            "f",
            vec![],
            None,
            block(vec![stmt(let_binding("y", int(5)))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);
        let blk = &cfg.blocks[0];

        // The RHS emits a Const(5) inst.
        let has_const_5 = blk.insts.iter().any(|i| matches!(
            i,
            Inst::Const { value: 5, .. }
        ));
        assert!(has_const_5, "expected Const(5) in {:?}", blk.insts);

        // Terminator is Return(None) — no explicit return.
        assert!(
            matches!(blk.terminator, Terminator::Return(None)),
            "expected Return(None), got {:?}",
            blk.terminator
        );

        // Function has no params.
        assert!(cfg.params.is_empty());
    }

    // -----------------------------------------------------------------
    // Block structure
    // -----------------------------------------------------------------

    #[test]
    fn build_simple_function_has_one_block() {
        // The simplest possible function — a single block, one Const,
        // one Return. No control flow, no let bindings.
        let func = function(
            "f",
            vec![],
            Some("int"),
            block(vec![stmt(ret(int(0)))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        assert_eq!(cfg.blocks.len(), 1, "expected exactly one block");
        assert_eq!(cfg.entry, BlockId(0));
    }

    #[test]
    fn build_function_with_let_then_return_has_correct_inst_count() {
        // `fn f(int x) -> int { let y = x + 1; return y; }`
        //
        // Inst count: Param(x) + Const(1) + BinOp(Add) = 3 insts.
        // The let-binding and the return statement are NOT insts
        // (let-binding registers a local; return sets return_value).
        let func = function(
            "f",
            vec![("int", "x")],
            Some("int"),
            block(vec![
                stmt(let_binding("y", add(ident("x"), int(1)))),
                stmt(ret(ident("y"))),
            ]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);
        let blk = &cfg.blocks[0];

        assert_eq!(blk.insts.len(), 3, "insts: {:?}", blk.insts);

        // Sanity: exactly one of each.
        let n_params = blk
            .insts
            .iter()
            .filter(|i| matches!(i, Inst::Param { .. }))
            .count();
        let n_consts = blk
            .insts
            .iter()
            .filter(|i| matches!(i, Inst::Const { .. }))
            .count();
        let n_binops = blk
            .insts
            .iter()
            .filter(|i| matches!(i, Inst::BinOp { .. }))
            .count();
        assert_eq!(n_params, 1);
        assert_eq!(n_consts, 1);
        assert_eq!(n_binops, 1);
    }

    #[test]
    fn predecessors_field_is_empty_for_single_block_function() {
        // Single-block functions have no predecessors (no other
        // block can transfer control to them). The fill_predecessors
        // pass must produce an empty `predecessors` Vec.
        let func = function(
            "f",
            vec![("int", "x")],
            Some("int"),
            block(vec![stmt(ret(ident("x")))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);
        let blk = &cfg.blocks[0];

        assert!(
            blk.predecessors.is_empty(),
            "single-block function should have empty predecessors, got {:?}",
            blk.predecessors
        );
    }

    // -----------------------------------------------------------------
    // Integration: end-to-end simple functions
    // -----------------------------------------------------------------

    #[test]
    fn build_double_function_returns_x_times_2() {
        // `fn double(int x) -> int { return x * 2; }`
        //
        // Expected CFG:
        //   - 1 block
        //   - 3 instructions: Param(x → v0, index 0),
        //                     Const(2 → v1),
        //                     BinOp(Mul, v2, v0, v1)
        //   - 1 terminator: Return(Some(v2))
        //   - 1 param: (v0, "x")
        //   - 3 SSA values total (v0, v1, v2)
        let func = function(
            "double",
            vec![("int", "x")],
            Some("int"),
            block(vec![stmt(ret(mul(ident("x"), int(2))))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        // 1 block.
        assert_eq!(cfg.blocks.len(), 1, "expected 1 block");
        let blk = &cfg.blocks[0];

        // 3 insts.
        assert_eq!(
            blk.insts.len(),
            3,
            "expected 3 insts (Param + Const + BinOp), got {:?}",
            blk.insts
        );

        // Verify each inst exactly.
        match &blk.insts[0] {
            Inst::Param { dst, index } => {
                assert_eq!(*dst, ValueId(0));
                assert_eq!(*index, 0);
            }
            other => panic!("inst 0: expected Param, got {:?}", other),
        }
        match &blk.insts[1] {
            Inst::Const { dst, value } => {
                assert_eq!(*dst, ValueId(1));
                assert_eq!(*value, 2);
            }
            other => panic!("inst 1: expected Const(2), got {:?}", other),
        }
        match &blk.insts[2] {
            Inst::BinOp {
                op,
                dst,
                lhs,
                rhs,
            } => {
                assert_eq!(*op, BinOpKind::Mul);
                assert_eq!(*dst, ValueId(2));
                assert_eq!(*lhs, ValueId(0));
                assert_eq!(*rhs, ValueId(1));
            }
            other => panic!("inst 2: expected BinOp(Mul), got {:?}", other),
        }

        // Terminator: Return(Some(v2)).
        match &blk.terminator {
            Terminator::Return(Some(v)) => assert_eq!(*v, ValueId(2)),
            other => panic!("expected Return(Some(v2)), got {:?}", other),
        }

        // 1 param: (v0, "x").
        assert_eq!(cfg.params, vec![(ValueId(0), "x".to_string())]);
        assert_eq!(cfg.name, "double");

        // 3 SSA values: v0 (Param), v1 (Const), v2 (BinOp).
        let mut seen = std::collections::HashSet::new();
        for inst in &blk.insts {
            for v in match inst {
                Inst::Param { dst, .. } => vec![*dst],
                Inst::Const { dst, .. } => vec![*dst],
                Inst::BinOp { dst, lhs, rhs, .. } => {
                    vec![*dst, *lhs, *rhs]
                }
                _ => vec![],
            } {
                seen.insert(v);
            }
        }
        assert_eq!(
            seen.len(),
            3,
            "expected 3 distinct SSA values, got {:?}",
            seen
        );
    }

    #[test]
    fn build_let_chain_function() {
        // `fn f(int x) -> int { let y = x + 1; let z = y * 2; return z; }`
        //
        // Emits (in order):
        //   Param(v0, 0)  -- x
        //   Const(v1, 1)
        //   BinOp(Add, v2, v0, v1)   -- y = x + 1
        //   Const(v3, 2)
        //   BinOp(Mul, v4, v2, v3)   -- z = y * 2
        //   terminator Return(Some(v4))   -- z = v4
        //
        // locals["y"] = v2, locals["z"] = v4. Neither let-binding nor
        // return emits an instruction.
        let func = function(
            "f",
            vec![("int", "x")],
            Some("int"),
            block(vec![
                stmt(let_binding("y", add(ident("x"), int(1)))),
                stmt(let_binding("z", mul(ident("y"), int(2)))),
                stmt(ret(ident("z"))),
            ]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);
        let blk = &cfg.blocks[0];

        // 5 insts.
        assert_eq!(
            blk.insts.len(),
            5,
            "expected 5 insts, got {:?}",
            blk.insts
        );

        // Inst 0: Param(x → v0).
        match &blk.insts[0] {
            Inst::Param { dst, index } => {
                assert_eq!(*dst, ValueId(0));
                assert_eq!(*index, 0);
            }
            other => panic!("inst 0: expected Param, got {:?}", other),
        }

        // Inst 1: Const(1 → v1).
        match &blk.insts[1] {
            Inst::Const { dst, value } => {
                assert_eq!(*dst, ValueId(1));
                assert_eq!(*value, 1);
            }
            other => panic!("inst 1: expected Const(1), got {:?}", other),
        }

        // Inst 2: BinOp(Add, v2, v0, v1) — the first let-binding's RHS.
        match &blk.insts[2] {
            Inst::BinOp {
                op,
                dst,
                lhs,
                rhs,
            } => {
                assert_eq!(*op, BinOpKind::Add);
                assert_eq!(*dst, ValueId(2));
                assert_eq!(*lhs, ValueId(0));
                assert_eq!(*rhs, ValueId(1));
            }
            other => panic!("inst 2: expected BinOp(Add), got {:?}", other),
        }

        // Inst 3: Const(2 → v3).
        match &blk.insts[3] {
            Inst::Const { dst, value } => {
                assert_eq!(*dst, ValueId(3));
                assert_eq!(*value, 2);
            }
            other => panic!("inst 3: expected Const(2), got {:?}", other),
        }

        // Inst 4: BinOp(Mul, v4, v2, v3) — the second let-binding's RHS,
        // using the first let-binding's ValueId as the lhs.
        match &blk.insts[4] {
            Inst::BinOp {
                op,
                dst,
                lhs,
                rhs,
            } => {
                assert_eq!(*op, BinOpKind::Mul);
                assert_eq!(*dst, ValueId(4));
                assert_eq!(
                    *lhs, ValueId(2),
                    "lhs should be y's ValueId (v2)"
                );
                assert_eq!(*rhs, ValueId(3));
            }
            other => panic!("inst 4: expected BinOp(Mul), got {:?}", other),
        }

        // Terminator: Return(Some(v4)) — z's ValueId.
        match &blk.terminator {
            Terminator::Return(Some(v)) => assert_eq!(*v, ValueId(4)),
            other => panic!("expected Return(Some(v4)), got {:?}", other),
        }
    }

    // -----------------------------------------------------------------
    // Phase 1.0: control-flow expressions — `if` / `if/else` /
    // `if/else if/else`. Each branch's body becomes its own basic
    // block; the entry block's terminator becomes `Branch`; the
    // join block is reached by `Jump` from each branch's body.
    // -----------------------------------------------------------------

    #[test]
    fn build_if_single_branch_produces_three_blocks() {
        // `fn f() -> int { if true { return 1; } return 0; }`
        //
        // Block structure:
        //   0 (entry):  ConstBool(true) → v0,  Branch(v0, 2, 1)
        //   1 (join):   Const(0) → v1,         Return(Some(v1))
        //   2 (then):   Const(1) → v2,          Jump(1)
        //
        // Branch's true target is the then_block (BlockId(2));
        // Branch's false target is the join_block (BlockId(1))
        // because there's no else.
        let func = function(
            "f",
            vec![],
            Some("int"),
            block(vec![
                stmt(if_single(bool_lit(true), block(vec![stmt(ret(int(1)))]))),
                stmt(ret(int(0))),
            ]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        assert_eq!(
            cfg.blocks.len(),
            3,
            "expected 3 blocks (entry + then + join), got {}",
            cfg.blocks.len()
        );

        // Block 0 (entry): ConstBool(true) followed by Branch.
        let entry = &cfg.blocks[0];
        let has_constbool = entry
            .insts
            .iter()
            .any(|i| matches!(i, Inst::ConstBool { value: true, .. }));
        assert!(has_constbool, "entry should contain ConstBool(true)");
        match &entry.terminator {
            Terminator::Branch { cond, true_bb, false_bb } => {
                // cond is the ValueId from ConstBool(true); we
                // don't pin it to a specific number (depends on
                // the param block setup), but it must be Some.
                let _ = cond;
                assert_eq!(
                    *true_bb,
                    BlockId(2),
                    "Branch.true_bb should be then_block"
                );
                assert_eq!(
                    *false_bb,
                    BlockId(1),
                    "Branch.false_bb should be join_block (no else)"
                );
            }
            other => panic!("entry should have Branch terminator, got {:?}", other),
        }

        // Block 1 (join): Const(0) followed by Return(Some(...)).
        let join = &cfg.blocks[1];
        assert!(
            join.insts
                .iter()
                .any(|i| matches!(i, Inst::Const { value: 0, .. })),
            "join should contain Const(0) (the `return 0` after the if)"
        );
        assert!(
            matches!(join.terminator, Terminator::Return(Some(_))),
            "join should have Return(Some(_)) terminator, got {:?}",
            join.terminator
        );

        // Block 2 (then_block): Const(1) followed by Jump(1).
        let then_block = &cfg.blocks[2];
        assert!(
            then_block
                .insts
                .iter()
                .any(|i| matches!(i, Inst::Const { value: 1, .. })),
            "then_block should contain Const(1) (the `return 1` body)"
        );
        match &then_block.terminator {
            Terminator::Jump(bb) => assert_eq!(
                *bb,
                BlockId(1),
                "then_block should Jump to join_block"
            ),
            other => panic!(
                "then_block should have Jump(1) terminator, got {:?}",
                other
            ),
        }
    }

    #[test]
    fn build_if_else_produces_four_blocks() {
        // `fn f() -> int { if c { return 1; } else { return 2; } }`
        //
        // Block structure:
        //   0 (entry):    Cond(c) → v0,  Branch(v0, 2, 3)
        //   1 (join):     Return(None)   (no continuation after the if)
        //   2 (then):     Const(1) → v1, Jump(1)
        //   3 (else):     Const(2) → v2, Jump(1)
        //
        // For multi-branch `If`, each branch's body is its own
        // block. The Branch's false target is a fresh
        // "fallthrough" block (which becomes the next branch's
        // entry, or the else block for the last cond=Some
        // branch).
        let func = function(
            "f",
            vec![],
            Some("int"),
            block(vec![stmt(if_else(
                bool_lit(true),
                block(vec![stmt(ret(int(1)))]),
                block(vec![stmt(ret(int(2)))]),
            ))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        assert_eq!(
            cfg.blocks.len(),
            4,
            "expected 4 blocks (entry + then + else + join), got {}",
            cfg.blocks.len()
        );

        // Entry: Branch with true → then_block, false → else_block.
        match &cfg.blocks[0].terminator {
            Terminator::Branch { true_bb, false_bb, .. } => {
                assert_eq!(*true_bb, BlockId(2));
                assert_eq!(
                    *false_bb,
                    BlockId(3),
                    "Branch.false_bb should be the else block"
                );
            }
            other => panic!("expected Branch terminator, got {:?}", other),
        }

        // Then block and else block both Jump to join_block.
        assert!(
            matches!(&cfg.blocks[2].terminator, Terminator::Jump(bb) if *bb == BlockId(1)),
            "then_block should Jump to join_block"
        );
        assert!(
            matches!(&cfg.blocks[3].terminator, Terminator::Jump(bb) if *bb == BlockId(1)),
            "else_block should Jump to join_block"
        );

        // Join block (BlockId(1)) has Return terminator (the body
        // was just the if; no instructions after, so default
        // Return(None)).
        assert!(
            matches!(&cfg.blocks[1].terminator, Terminator::Return(_)),
            "join block should have Return terminator, got {:?}",
            cfg.blocks[1].terminator
        );
    }

    #[test]
    fn build_if_else_if_else_produces_six_blocks() {
        // `fn f() -> int {
        //      if c1 { return 1; }
        //      else if c2 { return 2; }
        //      else { return 3; }
        //  }`
        //
        // Block structure:
        //   0 (entry):       Cond(c1) → v0,  Branch(v0, 3, 2)
        //   1 (join):        Return(None)
        //   2 (false_0):     Cond(c2) → v1,  Branch(v1, 4, 5)
        //   3 (then_0):      Const(1),      Jump(1)
        //   4 (then_1):      Const(2),      Jump(1)
        //   5 (else):        Const(3),      Jump(1)
        //
        // Note: the block indices interleave with construction
        // order. false_0 is allocated before then_1 because the
        // Branch in entry needs false_0 first. BlockId(0) is
        // always entry.
        let func = function(
            "f",
            vec![],
            Some("int"),
            block(vec![stmt(if_else_if_else(
                bool_lit(true),
                block(vec![stmt(ret(int(1)))]),
                bool_lit(false),
                block(vec![stmt(ret(int(2)))]),
                block(vec![stmt(ret(int(3)))]),
            ))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        assert_eq!(
            cfg.blocks.len(),
            6,
            "expected 6 blocks for if-elseif-else, got {}",
            cfg.blocks.len()
        );

        // Entry (BlockId(0)): Branch with true → then_0
        // (BlockId(2)), false → false_0 (BlockId(3)).
        //
        // The push order is: entry, join, then_0, false_0,
        // then_1, false_1. BlockIds 2 and 3 are then_0 and
        // false_0 respectively.
        match &cfg.blocks[0].terminator {
            Terminator::Branch { true_bb, false_bb, .. } => {
                assert_eq!(
                    *true_bb,
                    BlockId(2),
                    "entry's Branch.true_bb should be then_0 (BlockId 2)"
                );
                assert_eq!(
                    *false_bb,
                    BlockId(3),
                    "entry's Branch.false_bb should be false_0 (BlockId 3)"
                );
            }
            other => panic!("entry should have Branch terminator, got {:?}", other),
        }

        // false_0 (BlockId(3)): Branch with true → then_1
        // (BlockId(4)), false → else (BlockId(5)).
        match &cfg.blocks[3].terminator {
            Terminator::Branch { true_bb, false_bb, .. } => {
                assert_eq!(
                    *true_bb,
                    BlockId(4),
                    "false_0's Branch.true_bb should be then_1 (BlockId 4)"
                );
                assert_eq!(
                    *false_bb,
                    BlockId(5),
                    "false_0's Branch.false_bb should be else (BlockId 5)"
                );
            }
            other => panic!("false_0 should have Branch terminator, got {:?}", other),
        }

        // All three body blocks (2, 4, 5) Jump to join_block
        // (BlockId(1)).
        assert!(
            matches!(&cfg.blocks[2].terminator, Terminator::Jump(bb) if *bb == BlockId(1)),
            "then_0 (BlockId 2) should Jump to join_block (BlockId 1)"
        );
        assert!(
            matches!(&cfg.blocks[4].terminator, Terminator::Jump(bb) if *bb == BlockId(1)),
            "then_1 (BlockId 4) should Jump to join_block (BlockId 1)"
        );
        assert!(
            matches!(&cfg.blocks[5].terminator, Terminator::Jump(bb) if *bb == BlockId(1)),
            "else (BlockId 5) should Jump to join_block (BlockId 1)"
        );

        // Join block (BlockId(1)): Return terminator.
        assert!(
            matches!(&cfg.blocks[1].terminator, Terminator::Return(_)),
            "join block should have Return terminator"
        );
    }

    #[test]
    fn build_if_predecessors_are_filled_correctly() {
        // `fn f() -> int { if true { return 1; } else { return 2; } }`
        //
        // Block IDs: 0 (entry), 1 (join), 2 (then), 3 (else).
        // Expected predecessors:
        //   entry (0):  []
        //   join (1):   [then(2), else(3)]  (each Jumps to join)
        //   then (2):   [entry(0)]            (entry's Branch true)
        //   else (3):   [entry(0)]            (entry's Branch false)
        let func = function(
            "f",
            vec![],
            Some("int"),
            block(vec![stmt(if_else(
                bool_lit(true),
                block(vec![stmt(ret(int(1)))]),
                block(vec![stmt(ret(int(2)))]),
            ))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        // Entry has no predecessors.
        assert!(
            cfg.blocks[0].predecessors.is_empty(),
            "entry block should have no predecessors, got {:?}",
            cfg.blocks[0].predecessors
        );

        // Then and else blocks each have entry as their sole
        // predecessor (the Branch terminator's true / false
        // targets).
        assert_eq!(
            cfg.blocks[2].predecessors,
            vec![BlockId(0)],
            "then_block should have entry as predecessor"
        );
        assert_eq!(
            cfg.blocks[3].predecessors,
            vec![BlockId(0)],
            "else_block should have entry as predecessor"
        );

        // Join block has then AND else as predecessors (each
        // Jump targets join). Order is not guaranteed; compare
        // via a sorted copy (BlockId doesn't derive Ord, so use
        // sort_by_key).
        let mut join_preds = cfg.blocks[1].predecessors.clone();
        join_preds.sort_by_key(|b| b.0);
        assert_eq!(
            join_preds,
            vec![BlockId(2), BlockId(3)],
            "join should have then + else as predecessors"
        );
    }

    #[test]
    fn build_if_join_block_continues_with_subsequent_code() {
        // `fn f() -> int {
        //      if true { return 1; }
        //      return 0;
        //  }`
        //
        // After the if, the join_block contains the `return 0`.
        // The join_block's terminator is Return(Some(v)) where v
        // is the ValueId for `0`.
        let func = function(
            "f",
            vec![],
            Some("int"),
            block(vec![
                stmt(if_single(bool_lit(true), block(vec![stmt(ret(int(1)))]))),
                stmt(ret(int(0))),
            ]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        // Find the join block: it's the one with `Const(0)` AND
        // `Return(Some(_))` terminator.
        let join_idx = cfg
            .blocks
            .iter()
            .position(|b| {
                b.insts.iter().any(|i| matches!(i, Inst::Const { value: 0, .. }))
                    && matches!(b.terminator, Terminator::Return(Some(_)))
            })
            .expect("expected a join block with Const(0) and Return");

        let join = &cfg.blocks[join_idx];
        let ret_value = match &join.terminator {
            Terminator::Return(Some(v)) => *v,
            other => panic!("expected Return(Some(_)) at join, got {:?}", other),
        };
        // The ret_value should match the ValueId of the Const(0).
        let const_zero_vid = join
            .insts
            .iter()
            .find_map(|i| match i {
                Inst::Const { dst, value: 0 } => Some(*dst),
                _ => None,
            })
            .expect("expected Const(0) in join block");
        assert_eq!(
            ret_value, const_zero_vid,
            "Return value should be the ValueId of Const(0)"
        );
    }

    #[test]
    fn build_if_empty_branches_noops() {
        // An empty `If` is malformed (the parser doesn't
        // produce one) but the builder should be defensive and
        // not panic — it should no-op, leaving the current
        // block unchanged.
        //
        // We construct an empty If directly via the helper and
        // verify the resulting CFG has only the entry block
        // (no Join, no extra blocks).
        let func = function(
            "f",
            vec![],
            Some("int"),
            block(vec![stmt(Expression::If(Vec::new()))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        // Just the entry block — empty If produces no CFG.
        assert_eq!(
            cfg.blocks.len(),
            1,
            "empty If should not create extra blocks"
        );
    }
}