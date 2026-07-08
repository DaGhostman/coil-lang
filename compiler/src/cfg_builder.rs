//! CFG builder for the multi-pass compiler.
//!
//! Walks a typed AST and produces a CFG [`Function`]. Phase 0.2a
//! scope: **straight-line expressions only**. Control flow
//! ([`Expression::If`], [`Expression::Loop`], [`Expression::Match`],
//! [`Expression::Branch`]) is handled by the Phase 1.x refactor:
//! the builder now splits blocks, allocates fresh block IDs, and
//! emits [`Terminator::Branch`] / [`Terminator::Jump`] /
//! [`Terminator::Switch`] instead of the trivial
//! [`Terminator::Return`] produced for straight-line code.
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
//! ## Deferred to Phase 1+
//!
//! - `Expression::Match` pattern bindings (e.g., `Some(v) => v`
//!   binding `v` to the scrutinee's first payload). Phase 1.3
//!   accepts `Constructor` patterns but does not emit binding
//!   code; the body sees whatever the scrutinee evaluation
//!   produced. Full binding support is Phase 1.4+ work.

use std::collections::HashMap;

use parser::ast::{Expression, MatchArm, Output, Pattern};

use crate::cfg::{
    BinOpKind, Block, BlockId, Function, Inst, Terminator, TypeRef, UnaryOpKind, ValueId,
};

/// Compute a placeholder tag for a `(enum_name, variant_name)`
/// pair using FNV-1a 32-bit hashing.
///
/// This is a **Phase 1.3 placeholder**. The real tag resolution
/// comes from the typechecker's `tag_for` helper (see
/// `compiler/src/typechecking/infer.rs`), which uses
/// source-declaration order. The FNV-1a hash is deterministic
/// and 32-bit but does NOT match the real tag values; it's only
/// used to populate the `cases` vector with non-zero placeholder
/// tags so the Switch terminator is well-formed. The linearizer
/// (Phase 1.3+ codegen) will replace these with real tags.
///
/// Collisions are possible across different enum/variant
/// combinations but are extremely unlikely for typical program
/// sizes (the hash space is 2^32). The linearizer's
/// post-processing handles any collisions in the real
/// implementation.
fn hash_variant(enum_name: &str, variant_name: &str) -> u32 {
    let mut hash: u32 = 2166136261; // FNV-1a 32-bit offset basis
    for byte in enum_name.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(16777619); // FNV-1a 32-bit prime
    }
    for byte in b"::".iter().copied() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(16777619);
    }
    for byte in variant_name.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(16777619);
    }
    hash
}

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

    /// Look up the slot index for a function parameter by name.
    /// Returns the parameter's position in `param_list` (0..arity),
    /// or `None` if the name isn't a parameter.
    ///
    /// Phase 1.6: used by [`Builder::build_expression`]'s
    /// `Expression::Return` Identifier-of-param arm to emit a
    /// fresh `Inst::Param { dst, index }` in the current block
    /// before a `Terminator::Return(None)`. The slot index is the
    /// `Inst::Param.index` operand, which the linearizer
    /// translates directly into a `LOAD <slot>` opcode.
    ///
    /// Locals are deliberately NOT supported by this helper —
    /// locals don't have a known stack slot at codegen time
    /// (they live in the SSA value's stack position from the
    /// last write). Supporting locals would require a real
    /// register allocator; deferred to a future phase.
    #[allow(dead_code)]
    fn lookup_param_slot_index(&self, name: &str) -> Option<u16> {
        for (i, (_vid, param_name)) in self.param_list.iter().enumerate() {
            if param_name == name {
                return Some(i as u16);
            }
        }
        None
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
            // Build the format string and each param. The first
            // arg is the format string itself (a `ConstString`
            // SSA value pushed by the String-literal arm above).
            // The actual print instruction (`PRINT` for the
            // simple case in Phase 1.6) is the linearizer's job.
            //
            // Phase 1.6 supports only `print "literal";` — the
            // `is_straight_line` lift in `compiler/src/lib.rs`
            // gates Print from the CFG path when the format is
            // a constant string with no params. Format
            // specifiers (`%i`, `%f`, etc.) fall back to the
            // single-pass path.
            //
            // Returns `None` because Print is a statement, not
            // an expression (the printed value is the side
            // effect, not a SSA result).
            Expression::Print(fmt, params) => {
                let fmt_value = self.build_expression(fmt.1.as_ref())?;
                let mut args = vec![fmt_value];
                if let Some(items) = params {
                    for p in items {
                        args.push(self.build_expression(p.1.as_ref())?);
                    }
                }
                self.current_block_mut().insts.push(Inst::Print { args });
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
            //
            // We also set the CURRENT block's terminator to
            // `Terminator::Return`. Without this, `build_if` /
            // `build_while`'s post-loop would silently overwrite
            // the Return with `Jump(join_block)` / `Jump(header)`
            // when the return is inside an `if` branch / `while`
            // body, converting early returns into fall-throughs.
            // The post-loop is now conditional on
            // `Terminator::Unreachable` (see those functions) so
            // any block that already has a Return terminator is
            // left alone.
            //
            // Phase 1.6 fix — Identifier returns for params:
            // when the return value is a simple `Identifier(name)`
            // referring to a known function parameter, emit a
            // fresh `Inst::Param { dst: v_new, index: param_index }`
            // in the current block BEFORE the Return. The
            // linearizer translates that into `LOAD <slot>` +
            // `RETURN`, so the bare `RETURN` (emitted for
            // `Return(None)`) pops the correct value. Without
            // this, the pre-1.6 code emitted `Terminator::Return(
            // Some(ssa_value))` with no preceding instruction in
            // the current block; the linearizer emits only bare
            // `RETURN` regardless of the SSA value (the linearizer
            // doesn't track SSA value locations yet), so it pops
            // whatever's on the stack top. For a child block (an
            // if/else branch) where the parent didn't push the
            // value, the stack may be empty by the time the
            // branch's Return is reached — `RETURN` pops garbage.
            //
            // Documented limitation (NOT fixed in this commit):
            // complex return values like `return a + b;` or
            // `return local_var;` in a child block are NOT fixed
            // here. The builder's `build_expression` doesn't track
            // where intermediate SSA values are stored on the
            // stack, so we can't reload them. A real register
            // allocator would close this gap; deferred to a
            // future phase. For now, only Identifier-returns of
            // PARAMETERS are fixed — locals and complex
            // expressions fall through to the original (still
            // potentially buggy) behavior.
            Expression::Return(expr) | Expression::ImplicitReturn(expr) => {
                let return_expr_ref = expr.1.as_ref();

                // Phase 1.6: Identifier-of-param fast path. If the
                // return value is a bare identifier referring to a
                // known parameter, emit a fresh Param instruction
                // in the current block. The terminator becomes
                // `Return(None)` — the linearizer will emit
                // `LOAD <slot>` (from the Param) followed by bare
                // `RETURN`, which correctly pops the just-loaded
                // value.
                //
                // We deliberately do NOT update `self.return_value`
                // here — `build_function_from_parts`'s final-step
                // terminator derivation (line 426-440) reads
                // `return_value` to decide between `Return(Some(_))`
                // and `Return(None)`. Leaving `return_value` as
                // `None` ensures the final derivation picks
                // `Return(None)`, preserving the fast-path
                // terminator we just set.
                //
                // The parser wraps the return's expression in
                // `Expression::Expr(...)`, so we unwrap that
                // wrapper before pattern-matching on Identifier.
                // Bare `Expression::Identifier(...)` is also
                // accepted (e.g., when the cfg_builder is invoked
                // from unit tests that build the AST manually).
                let inner_expr = match return_expr_ref {
                    Expression::Expr(inner) => inner.1.as_ref(),
                    other => other,
                };
                if let Expression::Identifier(name) = inner_expr {
                    if let Some(slot_index) = self.lookup_param_slot_index(*name) {
                        let new_ssa = self.fresh_value();
                        self.current_block_mut().insts.push(Inst::Param {
                            dst: new_ssa,
                            index: slot_index,
                        });
                        self.current_block_mut().terminator = Terminator::Return(None);
                        return None;
                    }
                }

                // Fallback: original behavior for constants,
                // binary expressions, locals, etc. May be
                // incorrect for child blocks — see documented
                // limitation above.
                let v = self.build_expression(return_expr_ref);
                self.return_value = v;
                self.current_block_mut().terminator = match v {
                    Some(val) => Terminator::Return(Some(val)),
                    None => Terminator::Return(None),
                };
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
            // Phase 28 — type aliases produce no runtime
            // instructions (their RHS is substituted at
            // parse / typecheck time).
            Expression::TypeAlias { .. } => None,
            Expression::EnumVariant { .. } => None,
            // FFI declaration blocks carry no runtime value —
            // the symbol is resolved at VM startup (via dlopen).
            // We return None for the value ID; the typechecker
            // already registered each declared function in the
            // top frame.
            Expression::ExternBlock { .. } => None,

            // ---- Phase 1: control flow ----
            //
            // `If` (Phase 1.0), `Loop` (Phase 1.1), and `Match`
            // (Phase 1.3) are the control-flow variants
            // currently implemented. All produce multi-block
            // CFGs with `Branch` / `Jump` / `Switch` terminators
            // — see the `build_if`, `build_while`, and
            // `build_match` helpers below for the block
            // structure.
            //
            // `Branch` is a helper variant that ONLY appears as a
            // child of `If`. If it appears at the top level (out
            // of context), it's a malformed AST — panic loudly.
            //
            // `Expression::Loop` is the AST shape for `while`
            // loops (the legacy `for` iterator field is the
            // condition; the `identifier` field is unused by the
            // codegen and the CFG builder). The pre-1.1
            // single-pass codegen (`compiler/src/lib.rs::do_compile`)
            // interprets `Loop { iterable: cond, body }` as
            // `while cond { body }`, so we mirror that
            // interpretation here.
            Expression::If(branches) => self.build_if(branches),
            Expression::Branch(_, _) => panic!(
                "cfg_builder::build_expression: `Branch` appeared \
                 outside of an `If` context (malformed AST)"
            ),
            Expression::Loop {
                identifier: _,
                iterable,
                body,
            } => self.build_while(iterable.1.as_ref(), body.1.as_ref()),
            Expression::Match { scrutinee, arms } => self.build_match(scrutinee.1.as_ref(), arms),

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

            // ---- Userland FFI builtins (Phase 22b / FFI userland API) ----
            //
            // The cfg_builder doesn't yet know how to emit
            // `FfiLoad` / `DeclareFFI` / `FfiInvoke`. The
            // executor's `catch_unwind` safety net catches these
            // panics and falls back to the legacy single-pass
            // codegen, which DOES handle them.
            Expression::Dload(_) | Expression::Declare(_) | Expression::Invoke(_) => panic!(
                "cfg_builder::build_expression: userland FFI \
                 builtins are not yet supported on the CFG path"
            ),

            // ---- Aggregates (Phase 23) ----
            //
            // Tuple/Array construction, dicts, and `t[i]`
            // indexing are handled by the legacy single-pass
            // codegen today; the cfg builder panics so the
            // executor's `catch_unwind` fallback kicks in.
            Expression::Tuple(_)
            | Expression::Array(_)
            | Expression::Dict(_)
            | Expression::Index(_, _) => panic!(
                "cfg_builder::build_expression: aggregates \
                 (tuples/arrays/dicts/indexing) are not yet supported \
                 on the CFG path; the legacy single-pass codegen \
                 handles them via the catch_unwind fallback"
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
    fn build_binop(&mut self, lhs: &Output, rhs: &Output, op: BinOpKind) -> Option<ValueId> {
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
    /// **Critical invariant for the linearizer:** the block
    /// immediately following a `Branch` terminator's block in
    /// declaration order MUST be the Branch's `true_bb` (the
    /// `then` block). The linearizer emits a `JMPF` for each
    /// `Branch`; the `JMPF` jumps to `false_bb` when the
    /// condition is false and falls through to the NEXT block
    /// in declaration order when the condition is true. If the
    /// next block isn't `true_bb`, the `then` body is silently
    /// skipped at runtime.
    ///
    /// Block ID assignment (the then-block comes IMMEDIATELY
    /// after the Branch's block, the false_target / next-branch
    /// cond eval block comes after, and `join_block` comes last):
    ///
    /// | BlockId | Role                                                |
    /// |---------|-----------------------------------------------------|
    /// | 0       | entry (already pushed)                              |
    /// | 1       | then_0 (Branch's true_bb; reached by JMPF fall-through) |
    /// | 2       | false_target_0 (= next branch's cond / else block) |
    /// | 3       | then_1 (false_target_0's Branch true_bb)            |
    /// | 4       | false_target_1 (= next branch's cond / else block) |
    /// | ...     | (etc.)                                              |
    /// | N       | join_block (last; target of every Jump-to-join)     |
    ///
    /// For `if cond { body }` (no else): the Branch's false arm
    /// points at `join_block` directly (no separate false_target
    /// is allocated), and the push order is `[entry, then, join]`.
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

        // Phase 1.5 fix: defer the `Jump(join_block)` terminators
        // until `join_block` is finally allocated. The pre-1.5
        // code allocated `join_block` first and pushed it at
        // index 1, which made the linearizer's JMPF fall-through
        // land on `join_block` instead of `then_block` (the
        // wrong code path at runtime).
        //
        // We track every block that needs a `Jump(join_block)`
        // terminator here and patch them all in one shot after
        // `join_block` is allocated at the end of the loop.
        let mut blocks_needing_jump_to_join: Vec<BlockId> = Vec::new();
        let mut pending_join_block: Option<BlockId> = None;

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

                    // Push then_block IMMEDIATELY so it becomes
                    // the next block in declaration order — this
                    // is the linearizer's JMPF fall-through
                    // target (the Branch's true_bb).
                    let then_block = self.fresh_block();
                    self.blocks.push(Block::new(then_block));

                    // Allocate and push the false_target next.
                    //
                    // For the last branch with cond=Some (no
                    // else), the false arm goes directly to
                    // `join_block` — we allocate `join_block`
                    // here and push it as the false_target. The
                    // push order becomes
                    // `[entry, then, join]`.
                    //
                    // For non-last branches, push a fresh
                    // false_target where either the next
                    // branch's cond is evaluated (chained
                    // else-if) or the else's body lives (when
                    // the next branch has cond=None). The push
                    // order becomes `[entry, then, false_target,
                    // ...]`.
                    let false_target = if is_last {
                        // false_target IS the join_block.
                        // Allocate and push it now so its
                        // BlockId (= then_block+1) lines up
                        // with its Vec index. The linearizer's
                        // JMPF will jump to this offset when
                        // the condition is false.
                        let jb = self.fresh_block();
                        self.blocks.push(Block::new(jb));
                        pending_join_block = Some(jb);
                        jb
                    } else {
                        // false_target is a separate block
                        // (next branch's cond eval / else
                        // body). Allocate and push it so its
                        // BlockId lines up with its Vec index.
                        let ft = self.fresh_block();
                        self.blocks.push(Block::new(ft));
                        ft
                    };

                    // Set the current block's terminator to
                    // `Branch`. The current block is the
                    // previous iteration's false_target
                    // (chained else-if case) or the entry
                    // (first iteration case). The next block
                    // after it in declaration order is
                    // `then_block`, which is what the linearizer
                    // requires for the JMPF fall-through.
                    self.current_block_mut().terminator = Terminator::Branch {
                        cond: cond_v,
                        true_bb: then_block,
                        false_bb: false_target,
                    };

                    // Switch to then_block, build body, defer
                    // its Jump(join_block) terminator (we don't
                    // know join_block's BlockId yet for the
                    // non-last cases).
                    self.current = then_block;
                    let _then_value = self.build_expression(body.1.as_ref());
                    blocks_needing_jump_to_join.push(then_block);

                    // Advance current to false_target for the
                    // next iteration. If false_target IS
                    // join_block (last branch with cond=Some,
                    // no else), `current` is now the join_block
                    // and the loop ends with current pointing
                    // at the right place — no further advance
                    // needed.
                    self.current = false_target;
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

                    // Build the body in the current block
                    // (the previous iteration's false_target
                    // — which becomes the else block).
                    let _body_value = self.build_expression(body.1.as_ref());
                    blocks_needing_jump_to_join.push(self.current);

                    // Allocate and push `join_block` LAST so
                    // its BlockId (= current count of blocks)
                    // lines up with its Vec index. This is
                    // the block every preceding Jump will
                    // target.
                    let jb = self.fresh_block();
                    self.blocks.push(Block::new(jb));
                    pending_join_block = Some(jb);
                }
            }
        }

        // Patch every deferred Jump-to-join terminator now that
        // `join_block` is allocated. The terminator's target is
        // the same join_block for every block in the list.
        //
        // Phase 1.6 fix: only overwrite a block's terminator if
        // it is still `Terminator::Unreachable`. A block whose
        // body contained a `return` statement now has
        // `Terminator::Return(...)` (set by the Return arm in
        // `build_expression`); the unconditional overwrite would
        // silently convert the early return into a fall-through
        // to `join_block`. The Return terminator is the
        // authoritative control transfer for that block, so it
        // is left alone.
        let join_block = pending_join_block.expect(
            "cfg_builder::build_if: join_block must be \
             allocated by the end of the loop",
        );
        for bb in blocks_needing_jump_to_join {
            let block = &mut self.blocks[bb.index()];
            if matches!(block.terminator, Terminator::Unreachable) {
                block.terminator = Terminator::Jump(join_block);
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

    /// Build a multi-block CFG for a `while` loop
    /// (`Expression::Loop { iterable: cond, body }`).
    ///
    /// Each loop produces three fresh blocks — the header, the
    /// body, and the exit — plus a `Jump` terminator on the
    /// previous block (the entry edge into the loop).
    ///
    /// ## Block structure
    ///
    /// For `while cond { body }`:
    ///
    /// ```text
    /// [prev_block]    Jump loop_header       (entry edge)
    /// [loop_header]   ...evaluate cond...
    ///                 Branch cond → loop_body, loop_exit
    /// [loop_body]     ...body...
    ///                 Jump loop_header       (back-edge)
    /// [loop_exit]     ...continuation...
    /// ```
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
    /// | BlockId | Role                       |
    /// |---------|----------------------------|
    /// | 0       | prev_block (entry, already pushed) |
    /// | 1       | loop_header                |
    /// | 2       | loop_body                  |
    /// | 3       | loop_exit                  |
    ///
    /// The push order (header, body, exit) is deliberate: the
    /// linearizer's `Branch` terminator assumes `true_bb` is
    /// reachable by fall-through to the NEXT block in
    /// declaration order. Pushing body immediately after header
    /// makes that true (loop_body is BlockId(2), the block
    /// right after header BlockId(1) in declaration order).
    ///
    /// ## Value handling
    ///
    /// Returns `None`. Like `if`, a while-loop expression
    /// doesn't produce a value (no phi at the exit would be
    /// needed for a value-producing loop).
    ///
    /// ## Phase 1.1 limitations
    ///
    /// - The body's terminator is always overwritten with
    ///   `Jump(loop_header)` after the body is built. If the
    ///   body contains a `return`, the back-edge Jump clobbers
    ///   the Return terminator (same known limitation as
    ///   `build_if`'s join block, documented above).
    ///   `self.return_value` is still set by any inner Return,
    ///   but the outer Return terminator ends up on
    ///   `loop_exit` (where the linearizer's `step 5` puts it)
    ///   rather than at the inner return site. Phase 1.2+ can
    ///   lift this by tracking inner-block returns
    ///   terminator-side.
    /// - The `identifier` field of `Expression::Loop` is
    ///   silently ignored. The legacy single-pass codegen
    ///   also doesn't use it (the `for x in iter` syntax was
    ///   never wired into the parser). Phase 1.2+ can wire
    ///   it as a binding variable if needed.
    fn build_while(&mut self, cond: &Expression, body: &Expression) -> Option<ValueId> {
        // Allocate the three blocks BEFORE pushing so the
        // BlockIds are in the canonical (header, body, exit)
        // order. This is what makes the linearizer's
        // "true_bb is reached by fall-through" expectation
        // hold (the body is the block immediately after the
        // header in declaration order).
        let header_block = self.fresh_block();
        let body_block = self.fresh_block();
        let exit_block = self.fresh_block();

        // Push the three blocks in declaration order so that
        // `self.blocks[i].id == BlockId(i)` for each.
        self.blocks.push(Block::new(header_block));
        self.blocks.push(Block::new(body_block));
        self.blocks.push(Block::new(exit_block));

        // The previous block's terminator (whatever it was —
        // `Unreachable` for a top-level loop, or some other
        // control flow for a nested loop) is now overwritten
        // with `Jump(header)`. This is the entry edge into
        // the loop. The header's BlockId is stable, so the
        // linearizer will patch this JMP with the header's
        // absolute bytecode offset.
        self.current_block_mut().terminator = Terminator::Jump(header_block);

        // Switch to header. Build the condition expression
        // in the header block. If the condition fails to
        // produce a value (defensive recovery — the
        // typechecker should ensure cond has a value), we
        // substitute a fresh ValueId. The linearizer may
        // emit undefined-value diagnostics in this corner
        // case; in practice, well-formed programs always
        // produce a cond value.
        self.current = header_block;
        let cond_v = self
            .build_expression(cond)
            .unwrap_or_else(|| self.fresh_value());
        self.current_block_mut().terminator = Terminator::Branch {
            cond: cond_v,
            true_bb: body_block,
            false_bb: exit_block,
        };

        // Switch to body. Build the body. The body's
        // continuation block (which may be the body_block
        // itself for a straight-line body, or a deeper join
        // block if the body contains an inner `if`) ends
        // with `Jump(header)` — the back-edge.
        //
        // Phase 1.6 fix: only overwrite the terminator if it
        // is still `Terminator::Unreachable`. A body that
        // contained a `return` statement now has
        // `Terminator::Return(...)` (set by the Return arm in
        // `build_expression`); the unconditional overwrite
        // would silently convert the early return into a
        // back-edge to the header. The Return terminator is
        // the authoritative control transfer for that block,
        // so it is left alone.
        self.current = body_block;
        let _ = self.build_expression(body);
        if matches!(self.current_block_mut().terminator, Terminator::Unreachable) {
            self.current_block_mut().terminator = Terminator::Jump(header_block);
        }

        // Switch to exit. The loop's continuation code
        // (anything after the loop in the parent block)
        // lands here.
        self.current = exit_block;

        // While loops don't produce values.
        None
    }

    /// Build a multi-block CFG for a `match` expression
    /// ([`Expression::Match`]). Each arm's body becomes its own
    /// basic block; the current (entry) block is terminated with
    /// a [`Terminator::Switch`]; all arms Jump to a fresh join
    /// block where execution continues after the match.
    ///
    /// ## Block structure
    ///
    /// For `match scrutinee { Arm0 => body0, Arm1 => body1 }`:
    ///
    /// ```text
    /// [match_block]     ...evaluate scrutinee...
    ///                   Switch scrutinee → [(tag0, arm0_block)],
    ///                                   default: arm1_block
    ///
    /// [arm0_block]      ...body0...
    ///                   Jump join_block
    ///
    /// [arm1_block]      ...body1...
    ///                   Jump join_block     (default arm — last arm)
    ///
    /// [join_block]      ...continuation...
    /// ```
    ///
    /// For an N-arm match, there are N+2 blocks (match_block,
    /// N arm_blocks, join_block). The LAST arm is ALWAYS the
    /// `default` target of the Switch — this matches the
    /// existing single-pass codegen's "last arm is reached by
    /// fall-through" behavior (see `compiler/src/lib.rs`'s Match
    /// arm, line 2373).
    ///
    /// ## Block ID assignment
    ///
    /// `self.blocks` is a `Vec<Block>` indexed by `BlockId`. We
    /// allocate and push blocks in `BlockId` order so that
    /// `self.blocks[i].id == BlockId(i)`. This is what makes
    /// `self.current_block_mut()` correct (it indexes
    /// `self.blocks[self.current.index()]`).
    ///
    /// Block ID assignment for a 2-arm match:
    ///
    /// | BlockId | Role                          |
    /// |---------|-------------------------------|
    /// | 0       | match_block (entry, already pushed) |
    /// | 1       | arm_0                         |
    /// | 2       | arm_1 (default — last arm)    |
    /// | 3       | join_block                    |
    ///
    /// For an N-arm match, arm_blocks get BlockIds 1..=N and
    /// join_block gets BlockId N+1.
    ///
    /// ## Pattern support (Phase 1.3)
    ///
    /// - `Constructor(Enum::Variant(args))` — non-last arms
    ///   become Switch cases (tag from `hash_variant`, a
    ///   placeholder; the linearizer resolves real tags via the
    ///   typechecker's `tag_for` helper in a future phase).
    ///   Last-arm constructors are accepted but their tag is
    ///   silently dropped (the Switch's default catches them).
    /// - `Wildcard` — must be the LAST arm (the default). Wildcard
    ///   as a non-last arm is malformed and panics.
    /// - `Binding` — must be the LAST arm (the default). Binding
    ///   as a non-last arm is malformed and panics.
    /// - Nested constructor patterns (`Some(Some(v))`, record
    ///   patterns, etc.) — accepted but the payload binding is
    ///   silently dropped. Phase 1.4+ will emit binding code.
    ///
    /// ## Value handling
    ///
    /// Returns `None`. Match expressions as values would require
    /// phi-nodes at the join (the value depends on which arm
    /// was taken); SSA-lite punts on this. Users can use
    /// let-bindings or statement-form `match` to handle values.
    ///
    /// ## Phase 1.3 limitations
    ///
    /// - Returns in arm bodies are tracked via
    ///   `self.return_value` but the arm's terminator is still
    ///   overwritten with `Jump(join_block)`. Same known
    ///   limitation as `build_if`'s branches and `build_while`'s
    ///   body. Phase 1.4+ can lift this.
    /// - Tag resolution is placeholder (FNV-1a hash, not real
    ///   position-based tags from the typechecker). The linearizer
    ///   needs the typechecker's enum registry to produce real
    ///   tags.
    /// - Constructor pattern bindings (`Some(v) => ...`) are
    ///   silently dropped. Phase 1.4+ will emit binding code.
    fn build_match<'a>(
        &mut self,
        scrutinee: &Expression<'a>,
        arms: &[MatchArm<'a>],
    ) -> Option<ValueId> {
        // Defensive: empty Match is malformed (the parser doesn't
        // produce one). No-op to keep the builder non-panicking
        // for malformed ASTs.
        if arms.is_empty() {
            return None;
        }

        // 1. Allocate blocks for each arm and the join. We push
        //    them in source order so BlockId assignment is
        //    predictable: arm_blocks get BlockIds 1..=N, join
        //    gets BlockId N+1.
        let arm_blocks: Vec<BlockId> = arms.iter().map(|_| self.fresh_block()).collect();
        let join_block = self.fresh_block();

        // Push the arm blocks (in source order), then the join
        // block. After this loop, `self.blocks[i].id ==
        // BlockId(i)` holds for the new blocks (the entry
        // block at BlockId(0) was already pushed by
        // `build_function_from_parts`).
        for ab in &arm_blocks {
            self.blocks.push(Block::new(*ab));
        }
        self.blocks.push(Block::new(join_block));

        // 2. Evaluate the scrutinee in the current block (the
        //    entry block — `self.current` is unchanged from
        //    `build_function_from_parts`'s assignment). If the
        //    scrutinee fails to produce a value (defensive
        //    recovery — the typechecker should ensure the
        //    scrutinee has a value), substitute a fresh ValueId.
        //    The linearizer may emit undefined-value diagnostics
        //    in this corner case.
        let scrut_v = self
            .build_expression(scrutinee)
            .unwrap_or_else(|| self.fresh_value());

        // 3. Classify each arm's pattern into a Switch case or
        //    default.
        //
        //    Phase 1.3 strategy:
        //    - Non-last arms: must be `Constructor` → Switch case
        //      with a placeholder FNV-1a tag. `Wildcard` or
        //      `Binding` in a non-last position is malformed and
        //      panics.
        //    - Last arm: always the default, regardless of its
        //      pattern (`Constructor`, `Wildcard`, or `Binding`).
        //      For `Constructor` last arms, the tag is dropped
        //      (the Switch's default catches any non-matching
        //      tag, including the last arm's own tag).
        let mut cases: Vec<(u32, BlockId)> = Vec::new();

        for (i, arm) in arms.iter().enumerate() {
            let arm_block = arm_blocks[i];
            let is_last = i + 1 == arms.len();

            match &arm.pattern {
                Pattern::Wildcard => {
                    if !is_last {
                        panic!(
                            "cfg_builder::build_match: Wildcard \
                             pattern must be the LAST arm (got \
                             arm {})",
                            i
                        );
                    }
                    // Wildcard as last arm → default (no case
                    // needed).
                }
                Pattern::Binding { .. } => {
                    if !is_last {
                        panic!(
                            "cfg_builder::build_match: Binding \
                             pattern must be the LAST arm (got \
                             arm {})",
                            i
                        );
                    }
                    // Binding as last arm → default (no case
                    // needed). Note: actually binding `name` to
                    // the scrutinee requires `STORE` code, which
                    // is Phase 1.4+ work. For Phase 1.3, the
                    // binding is accepted but the binding code
                    // is silently dropped.
                }
                Pattern::Constructor {
                    enum_name,
                    variant_name,
                    payload: _,
                } => {
                    if !is_last {
                        let tag = hash_variant(enum_name, variant_name);
                        cases.push((tag, arm_block));
                    }
                    // Constructor as last arm → default (no case
                    // needed). The tag is dropped; the Switch's
                    // default catches this arm's tag at runtime.
                    // Note: this means runtime dispatch would
                    // incorrectly route the last arm's tag to
                    // the default block (it IS the default, so
                    // this is correct in this simple scheme).
                }
            }
        }

        let last_arm_block = arm_blocks[arms.len() - 1];

        // 4. Set the Switch terminator on the current block
        //    (the match_block / entry block). The previous
        //    `Unreachable` placeholder terminator is overwritten.
        self.current_block_mut().terminator = Terminator::Switch {
            scrutinee: scrut_v,
            cases,
            default: last_arm_block,
        };

        // 5. Build each arm's body in its own block.
        //
        //    TODO (Phase 1.4+): emit pattern-binding code
        //    (e.g., `STORE` for `Some(v) => v`'s `v` binding).
        //    For Phase 1.3 we accept `Constructor` patterns but
        //    do not emit binding code; the body sees whatever
        //    the scrutinee evaluation produced (the runtime
        //    semantics are not yet correct for constructor
        //    pattern bindings — Phase 1.4+ will fix this).
        for (i, arm) in arms.iter().enumerate() {
            let arm_block = arm_blocks[i];
            self.current = arm_block;
            let _body_value = self.build_expression(arm.body.1.as_ref());
            self.current_block_mut().terminator = Terminator::Jump(join_block);
        }

        // 6. Continue in the join block. Subsequent code
        //    (anything after the match in the parent block)
        //    lands here.
        self.current = join_block;

        // Match expressions as values return None (phi-node
        // deferral).
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
                        true_bb, false_bb, ..
                    } => vec![*true_bb, *false_bb],
                    Terminator::Switch { cases, default, .. } => {
                        let mut s: Vec<BlockId> = cases.iter().map(|(_, bb)| *bb).collect();
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

    fn gt(lhs: Expression<'static>, rhs: Expression<'static>) -> Expression<'static> {
        Expression::Gt(e(lhs), e(rhs))
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

    /// One function parameter: `Argument(Output(Type(name)), name)`.
    /// Phase 24 — the type is a full `Output` so aggregate
    /// types can be expressed in tests too.
    fn argument(ty: &'static str, name: &'static str) -> Expression<'static> {
        let ty_out: Output<'static> = (
            parser::SimpleSpan {
                start: 0,
                end: 0,
                context: (),
            },
            Box::new(Expression::Type(ty)),
        );
        Expression::Argument(ty_out, name)
    }

    /// Build a complete `Expression::Function`. The `returns`
    /// field is now an `Option<Output<'static>>` (Phase 24 —
    /// full type annotation); most tests still pass
    /// `Some("int")` which is auto-wrapped via
    /// `e(Expression::Type(name))`.
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
        let returns_output: Option<Output<'static>> =
            returns.map(|s| e(Expression::Type(s)));
        Expression::Function {
            name,
            args: e(Expression::Fragment(args_vec)),
            returns: returns_output,
            body: e(body),
        }
    }

    /// Build a single `Branch` AST node: `Branch(Option<Output>,
    /// Output)`. The `cond` is `None` for an `else` branch.
    /// `cond` of `None` produces `Branch(None, body)` (the else
    /// form).
    fn branch(cond: Option<Expression<'static>>, body: Expression<'static>) -> Expression<'static> {
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
    fn if_single(cond: Expression<'static>, body: Expression<'static>) -> Expression<'static> {
        if_expr(vec![(Some(cond), body)])
    }

    /// Convenience for `if cond { then_b } else { else_b }` — two
    /// branches, the second with no condition (the else).
    fn if_else(
        cond: Expression<'static>,
        then_b: Expression<'static>,
        else_b: Expression<'static>,
    ) -> Expression<'static> {
        if_expr(vec![(Some(cond), then_b), (None, else_b)])
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
        if_expr(vec![(Some(c1), b1), (Some(c2), b2), (None, b3)])
    }

    /// Build a `while` loop AST node. Mirrors the pre-1.1
    /// single-pass codegen's interpretation of
    /// `Expression::Loop` (the legacy `for` iterator field is
    /// the condition; the `identifier` is left as `None`).
    ///
    /// `while cond { body }` →
    /// `Expression::Loop { identifier: None, iterable: cond,
    /// body: body }`.
    fn while_loop(cond: Expression<'static>, body: Expression<'static>) -> Expression<'static> {
        Expression::Loop {
            identifier: None,
            iterable: e(cond),
            body: e(body),
        }
    }

    /// Build a `match` AST node: `match scrutinee { pat => body, ... }`.
    /// Wraps the scrutinee in an `Output` and produces the
    /// `Expression::Match` variant. The arms are pre-built via
    /// [`match_arm`].
    fn match_expr(
        scrutinee: Expression<'static>,
        arms: Vec<MatchArm<'static>>,
    ) -> Expression<'static> {
        Expression::Match {
            scrutinee: e(scrutinee),
            arms,
        }
    }

    /// Build a single `MatchArm` AST node: `pattern => body`. The
    /// body is wrapped in an `Output`.
    fn match_arm(pattern: Pattern<'static>, body: Expression<'static>) -> MatchArm<'static> {
        MatchArm {
            pattern,
            body: e(body),
        }
    }

    /// Build a wildcard pattern `_`.
    fn wildcard_pattern() -> Pattern<'static> {
        Pattern::Wildcard
    }

    /// Build a binding pattern `name`. The name is stored but the
    /// binding code (Phase 1.4+) is not yet emitted by the
    /// builder.
    fn binding_pattern(name: &'static str) -> Pattern<'static> {
        Pattern::Binding { name }
    }

    /// Build a constructor pattern `EnumName::VariantName` with a
    /// `Unit` payload. Tuple and Record payloads are deferred
    /// (Phase 1.4+); this helper covers the common case
    /// `Option::None` / `Option::Some` (used as a Unit pattern
    /// for these tests since we don't bind the payload).
    fn constructor_pattern(
        enum_name: &'static str,
        variant_name: &'static str,
    ) -> Pattern<'static> {
        Pattern::Constructor {
            enum_name,
            variant_name,
            payload: parser::ast::PatternPayload::Unit,
        }
    }

    // -----------------------------------------------------------------
    // Basic expressions: constants
    // -----------------------------------------------------------------

    #[test]
    fn build_int_constant_produces_const_inst() {
        // `fn f() -> int { return 42; }`
        let func = function("f", vec![], Some("int"), block(vec![stmt(ret(int(42)))]));
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
        assert!(has_constf, "expected ConstF(3.14) in {:?}", blk.insts);
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

        let has_constbool = blk
            .insts
            .iter()
            .any(|i| matches!(i, Inst::ConstBool { value: true, .. }));
        assert!(has_constbool, "expected ConstBool(true) in {:?}", blk.insts);
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

        let has_conststring = blk.insts.iter().any(|i| {
            matches!(
                i,
                Inst::ConstString { value, .. } if value == "hello"
            )
        });
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
        //
        // Phase 1.6: the Return arm's Identifier-of-param fast
        // path emits a fresh `Inst::Param { dst: v1, index: 0 }`
        // in the entry block so the value is on the stack top
        // when the linearizer emits bare `RETURN` for
        // `Return(None)`. The original `Inst::Param { dst: v0,
        // index: 0 }` from `build_function` is still present
        // (it binds `x`'s SSA value for downstream uses). Net
        // effect: two `Inst::Param { index: 0 }` instructions,
        // both loading slot 0; the linearizer emits `LOAD 0,
        // LOAD 0, RETURN` (the second LOAD is wasted but
        // harmless — the value is correctly on the stack top).
        //
        // Pre-1.6 this test asserted exactly 1 Param and
        // `Return(Some(v0))`. After the fast path, the block has
        // 2 Params and `Return(None)`.
        let func = function(
            "f",
            vec![("int", "x")],
            Some("int"),
            block(vec![stmt(ret(ident("x")))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);
        let blk = &cfg.blocks[0];

        // Two Params — both load slot 0 (one from
        // `build_function`, one from the Return arm's fast
        // path).
        let params: Vec<_> = blk
            .insts
            .iter()
            .filter_map(|i| match i {
                Inst::Param { dst, index } => Some((*dst, *index)),
                _ => None,
            })
            .collect();
        assert_eq!(params.len(), 2, "expected 2 Param insts");
        assert_eq!(params[0], (ValueId(0), 0));
        assert_eq!(params[1], (ValueId(1), 0));

        // No other insts (Identifier lookups don't emit
        // additional insts).
        assert_eq!(
            blk.insts.len(),
            2,
            "expected only the two Param insts, got {:?}",
            blk.insts
        );

        // Terminator is Return(None) — the fast path
        // materializes the value on the stack via the second
        // Param, so the linearizer's bare RETURN pops the
        // correct value.
        assert!(
            matches!(blk.terminator, Terminator::Return(None)),
            "expected Return(None), got {:?}",
            blk.terminator
        );

        // params list has one entry: (v0, "x") — only the
        // original Param is registered in the function's
        // signature; the fast-path Param is anonymous and only
        // exists to push the value.
        assert_eq!(cfg.params, vec![(ValueId(0), "x".to_string())]);
    }

    // -----------------------------------------------------------------
    // Phase 1.6: Identifier-of-param return fast path
    //
    // These tests verify the fix for Linearizer bug 2:
    // `Terminator::Return(Some(ValueId))` emitted bare `RETURN`,
    // which popped whatever was on the stack. For child blocks
    // (if/else branches) the stack could be empty by the time
    // the Return was reached, so RETURN popped garbage.
    //
    // The fix: when the return value is a bare Identifier
    // referring to a known parameter, emit a fresh
    // `Inst::Param { dst, index }` in the current block. The
    // linearizer translates that into `LOAD <slot>` followed by
    // `RETURN`, correctly returning the parameter's value.
    // -----------------------------------------------------------------

    #[test]
    fn build_return_param_in_if_branch_emits_param_in_branch_block() {
        // `fn max(int a, int b) -> int { if a > b { return a; } else { return b; } }`
        //
        // The fast path should emit a Param instruction in EACH
        // branch block (then_block for `a`, false_target for
        // `b`) so the linearizer's bare RETURN pops the correct
        // value. Pre-1.6 each branch's Return was
        // `Terminator::Return(Some(v0_or_v1))` with no
        // preceding instruction, so the linearizer emitted bare
        // RETURN — popping whatever (typically garbage) was on
        // the stack.
        //
        // Expected block structure for `if a > b { return a; } else { return b; }`:
        //   0 (entry):      Param(v0, 0), Param(v1, 1), BinOp(Gt, v2, v0, v1)
        //                   terminator: Branch { cond: v2, true_bb: 1, false_bb: 2 }
        //   1 (then_block): Param(v3, 0)  <- NEW: fast-path Param for `a`
        //                   terminator: Return(None)
        //   2 (else_block): Param(v4, 1)  <- NEW: fast-path Param for `b`
        //                   terminator: Return(None)
        //   3 (join_block): terminator: Return(None) (no value to return —
        //                       neither branch falls through to join_block)
        let func = function(
            "max",
            vec![("int", "a"), ("int", "b")],
            Some("int"),
            block(vec![stmt(if_else(
                gt(ident("a"), ident("b")),
                block(vec![stmt(ret(ident("a")))]),
                block(vec![stmt(ret(ident("b")))]),
            ))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        // 4 blocks: entry, then, else, join.
        assert_eq!(
            cfg.blocks.len(),
            4,
            "expected 4 blocks, got {}",
            cfg.blocks.len()
        );

        // Block 1 (then_block): single Param for `a` (index 0),
        // Return(None) terminator.
        let then_block = &cfg.blocks[1];
        let then_params: Vec<_> = then_block
            .insts
            .iter()
            .filter_map(|i| match i {
                Inst::Param { dst, index } => Some((*dst, *index)),
                _ => None,
            })
            .collect();
        assert_eq!(
            then_params.len(),
            1,
            "then_block should have 1 Param, got {:?}",
            then_block.insts
        );
        assert_eq!(
            then_params[0],
            (ValueId(3), 0),
            "then_block Param should load slot 0 (param `a`)"
        );
        assert!(
            matches!(then_block.terminator, Terminator::Return(None)),
            "then_block terminator should be Return(None), got {:?}",
            then_block.terminator
        );

        // Block 2 (else_block): single Param for `b` (index 1),
        // Return(None) terminator.
        let else_block = &cfg.blocks[2];
        let else_params: Vec<_> = else_block
            .insts
            .iter()
            .filter_map(|i| match i {
                Inst::Param { dst, index } => Some((*dst, *index)),
                _ => None,
            })
            .collect();
        assert_eq!(
            else_params.len(),
            1,
            "else_block should have 1 Param, got {:?}",
            else_block.insts
        );
        assert_eq!(
            else_params[0],
            (ValueId(4), 1),
            "else_block Param should load slot 1 (param `b`)"
        );
        assert!(
            matches!(else_block.terminator, Terminator::Return(None)),
            "else_block terminator should be Return(None), got {:?}",
            else_block.terminator
        );

        // Block 3 (join_block): no params (the fast path's
        // terminator overwrite is conditional on
        // Terminator::Unreachable; build_if already set the
        // entry's Branch terminator, so the join block's
        // terminator stays at Return(None) from the final-step
        // derivation).
        let join_block = &cfg.blocks[3];
        assert!(
            matches!(join_block.terminator, Terminator::Return(None)),
            "join_block terminator should be Return(None), got {:?}",
            join_block.terminator
        );
    }

    #[test]
    fn build_return_local_identifier_does_not_use_fast_path() {
        // `fn f(int x) -> int { let y = x; return y; }`
        //
        // Locals are NOT supported by the fast path — they
        // don't have a known stack slot. The Return arm should
        // fall through to the original behavior:
        // `build_expression(Identifier("y"))` returns
        // `Some(locals["y"])` (which equals `params["x"]` =
        // `v0`), and the terminator is `Return(Some(v0))`.
        //
        // This test guards against future regressions where the
        // fast path accidentally extends to locals (which would
        // be incorrect without a register allocator).
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

        // Only the original Param (x → v0); no fast-path Param
        // for the local `y`.
        let params: Vec<_> = blk
            .insts
            .iter()
            .filter_map(|i| match i {
                Inst::Param { dst, index } => Some((*dst, *index)),
                _ => None,
            })
            .collect();
        assert_eq!(
            params.len(),
            1,
            "expected only the original Param (locals shouldn't trigger fast path), got {:?}",
            params
        );
        assert_eq!(params[0], (ValueId(0), 0));

        // Terminator is the original `Return(Some(v0))` — the
        // local's SSA value shares v0 with the parameter
        // (because `let y = x;` resolves `x` to v0 and stores
        // it in `locals["y"]`).
        assert!(
            matches!(blk.terminator, Terminator::Return(Some(v)) if v == ValueId(0)),
            "expected Return(Some(v0)) for local return, got {:?}",
            blk.terminator
        );
    }

    #[test]
    fn build_return_binary_expression_does_not_use_fast_path() {
        // `fn f(int a, int b) -> int { return a + b; }`
        //
        // Binary expressions are NOT supported by the fast path
        // — the intermediate BinOp result's stack slot isn't
        // known to the builder (it would require a register
        // allocator). The Return arm should fall through to the
        // original behavior.
        //
        // This is the documented limitation: complex return
        // values in child blocks are still buggy. The unit test
        // here only verifies that the entry-block case produces
        // the expected CFG (BinOp in the entry block,
        // `Return(Some(v_binop))`).
        let func = function(
            "f",
            vec![("int", "a"), ("int", "b")],
            Some("int"),
            block(vec![stmt(ret(add(ident("a"), ident("b"))))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);
        let blk = &cfg.blocks[0];

        // 2 Params (a, b) + 1 BinOp = 3 insts. NO fast-path
        // Param — the binary expression doesn't trigger it.
        let params: Vec<_> = blk
            .insts
            .iter()
            .filter_map(|i| match i {
                Inst::Param { dst, index } => Some((*dst, *index)),
                _ => None,
            })
            .collect();
        assert_eq!(
            params.len(),
            2,
            "expected 2 Params (no fast-path Param for BinOp), got {:?}",
            params
        );
        assert_eq!(params[0], (ValueId(0), 0));
        assert_eq!(params[1], (ValueId(1), 1));

        // One BinOp inst.
        let binops = blk
            .insts
            .iter()
            .filter(|i| matches!(i, Inst::BinOp { .. }))
            .count();
        assert_eq!(binops, 1, "expected 1 BinOp inst");

        // Terminator: Return(Some(v2)) — the BinOp's ValueId.
        assert!(
            matches!(blk.terminator, Terminator::Return(Some(v)) if v == ValueId(2)),
            "expected Return(Some(v2)) for binary return, got {:?}",
            blk.terminator
        );
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
            Inst::BinOp { op, dst, lhs, rhs } => {
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
            block(vec![stmt(ret(add(
                ident("a"),
                mul(ident("b"), ident("c")),
            )))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);
        let blk = &cfg.blocks[0];

        // 3 Params + 1 Mul + 1 Add = 5 insts.
        assert_eq!(blk.insts.len(), 5, "insts: {:?}", blk.insts);

        // Inst 3 (after the 3 Params) is the Mul.
        match &blk.insts[3] {
            Inst::BinOp { op, dst, lhs, rhs } => {
                assert_eq!(*op, BinOpKind::Mul);
                assert_eq!(*dst, ValueId(3));
                assert_eq!(*lhs, ValueId(1), "lhs should be b's v1");
                assert_eq!(*rhs, ValueId(2), "rhs should be c's v2");
            }
            other => panic!("expected BinOp(Mul), got {:?}", other),
        }

        // Inst 4 is the Add, using the Mul's result as its rhs.
        match &blk.insts[4] {
            Inst::BinOp { op, dst, lhs, rhs } => {
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
            Inst::BinOp { op, dst, lhs, rhs } => {
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
        let func = function("f", vec![], Some("int"), block(vec![stmt(ret(int(42)))]));
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
        let has_const_5 = blk
            .insts
            .iter()
            .any(|i| matches!(i, Inst::Const { value: 5, .. }));
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
        let func = function("f", vec![], Some("int"), block(vec![stmt(ret(int(0)))]));
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
            Inst::BinOp { op, dst, lhs, rhs } => {
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
        assert_eq!(blk.insts.len(), 5, "expected 5 insts, got {:?}", blk.insts);

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
            Inst::BinOp { op, dst, lhs, rhs } => {
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
            Inst::BinOp { op, dst, lhs, rhs } => {
                assert_eq!(*op, BinOpKind::Mul);
                assert_eq!(*dst, ValueId(4));
                assert_eq!(*lhs, ValueId(2), "lhs should be y's ValueId (v2)");
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
        // `fn f() -> int { if true { 1; } return 0; }`
        //
        // The if-body uses a bare integer statement (`1;`) so the
        // then_block has no Return terminator — this test is about
        // the build_if BLOCK STRUCTURE, not early-return semantics.
        // The early-return case is covered by
        // `build_if_then_block_with_return_keeps_return_terminator`.
        //
        // Block structure (Phase 1.5 fix — then_block comes
        // IMMEDIATELY after entry so the linearizer's JMPF
        // fall-through lands on it):
        //   0 (entry):      ConstBool(true) → v0,  Branch(v0, 1, 2)
        //   1 (then_block): Const(1) → v1,          Jump(2)
        //   2 (join):       Const(0) → v2,          Return(Some(v2))
        //
        // Branch's true target is the then_block (BlockId(1));
        // Branch's false target is the join_block (BlockId(2))
        // because there's no else. The then_block sits at
        // Vec index 1 (the block right after entry), which is
        // what the linearizer's "true_bb is reached by
        // fall-through" contract requires.
        let func = function(
            "f",
            vec![],
            Some("int"),
            block(vec![
                stmt(if_single(bool_lit(true), block(vec![stmt(int(1))]))),
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
            Terminator::Branch {
                cond,
                true_bb,
                false_bb,
            } => {
                // cond is the ValueId from ConstBool(true); we
                // don't pin it to a specific number (depends on
                // the param block setup), but it must be Some.
                let _ = cond;
                assert_eq!(
                    *true_bb,
                    BlockId(1),
                    "Branch.true_bb should be then_block (BlockId 1, \
                     immediately after entry — the JMPF fall-through target)"
                );
                assert_eq!(
                    *false_bb,
                    BlockId(2),
                    "Branch.false_bb should be join_block (no else)"
                );
            }
            other => panic!("entry should have Branch terminator, got {:?}", other),
        }

        // Block 1 (then_block): Const(1) followed by Jump(2).
        let then_block = &cfg.blocks[1];
        assert!(
            then_block
                .insts
                .iter()
                .any(|i| matches!(i, Inst::Const { value: 1, .. })),
            "then_block should contain Const(1) (the `1;` body)"
        );
        match &then_block.terminator {
            Terminator::Jump(bb) => assert_eq!(
                *bb,
                BlockId(2),
                "then_block should Jump to join_block (BlockId 2)"
            ),
            other => panic!("then_block should have Jump(2) terminator, got {:?}", other),
        }

        // Block 2 (join): Const(0) followed by Return(Some(...)).
        let join = &cfg.blocks[2];
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
    }

    #[test]
    fn build_if_else_produces_four_blocks() {
        // `fn f() -> int { if c { 1; } else { 2; } }`
        //
        // The if-body and else-body use bare integer statements
        // (`1;`, `2;`) so neither block has a Return terminator —
        // this test is about the build_if BLOCK STRUCTURE, not
        // early-return semantics. The early-return case is
        // covered by
        // `build_if_then_block_with_return_keeps_return_terminator`.
        //
        // Block structure (Phase 1.5 fix — then_block comes
        // IMMEDIATELY after entry):
        //   0 (entry):      Cond(c) → v0,  Branch(v0, 1, 2)
        //   1 (then_block): Const(1) → v1, Jump(3)
        //   2 (else_block): Const(2) → v2, Jump(3)
        //   3 (join):       Return(None)   (no continuation after the if)
        //
        // For multi-branch `If`, each branch's body is its own
        // block. The Branch's true target (then_block) sits at
        // Vec index 1 (immediately after entry), which is the
        // JMPF fall-through target. The Branch's false target
        // (else_block) is the next block at Vec index 2.
        let func = function(
            "f",
            vec![],
            Some("int"),
            block(vec![stmt(if_else(
                bool_lit(true),
                block(vec![stmt(int(1))]),
                block(vec![stmt(int(2))]),
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
            Terminator::Branch {
                true_bb, false_bb, ..
            } => {
                assert_eq!(
                    *true_bb,
                    BlockId(1),
                    "Branch.true_bb should be then_block (BlockId 1)"
                );
                assert_eq!(
                    *false_bb,
                    BlockId(2),
                    "Branch.false_bb should be the else block (BlockId 2)"
                );
            }
            other => panic!("expected Branch terminator, got {:?}", other),
        }

        // Then block and else block both Jump to join_block
        // (BlockId(3)).
        assert!(
            matches!(&cfg.blocks[1].terminator, Terminator::Jump(bb) if *bb == BlockId(3)),
            "then_block should Jump to join_block (BlockId 3)"
        );
        assert!(
            matches!(&cfg.blocks[2].terminator, Terminator::Jump(bb) if *bb == BlockId(3)),
            "else_block should Jump to join_block (BlockId 3)"
        );

        // Join block (BlockId(3)) has Return terminator (the
        // body was just the if; no instructions after, so
        // default Return(None)).
        assert!(
            matches!(&cfg.blocks[3].terminator, Terminator::Return(_)),
            "join block should have Return terminator, got {:?}",
            cfg.blocks[3].terminator
        );
    }

    #[test]
    fn build_if_else_if_else_produces_six_blocks() {
        // `fn f() -> int {
        //      if c1 { 1; }
        //      else if c2 { 2; }
        //      else { 3; }
        //  }`
        //
        // The branch bodies use bare integer statements
        // (`1;`, `2;`, `3;`) so no branch has a Return
        // terminator — this test is about the build_if BLOCK
        // STRUCTURE, not early-return semantics. The
        // early-return case is covered by
        // `build_if_then_block_with_return_keeps_return_terminator`.
        //
        // Block structure (Phase 1.5 fix — then_block always
        // comes IMMEDIATELY after the Branch's block; the
        // false_target (= next-branch cond eval / else block)
        // comes after that, and join_block comes last):
        //   0 (entry):            Cond(c1) → v0,  Branch(v0, 1, 2)
        //   1 (then_0):           Const(1),      Jump(5)
        //   2 (false_target_0):   Cond(c2) → v1,  Branch(v1, 3, 4)
        //   3 (then_1):           Const(2),      Jump(5)
        //   4 (false_target_1 /   Const(3),      Jump(5)
        //       else):
        //   5 (join):             Return(None)
        //
        // Note: the block indices interleave with construction
        // order. The push order is
        // `[entry, then_0, false_target_0, then_1, false_target_1, join]`.
        let func = function(
            "f",
            vec![],
            Some("int"),
            block(vec![stmt(if_else_if_else(
                bool_lit(true),
                block(vec![stmt(int(1))]),
                bool_lit(false),
                block(vec![stmt(int(2))]),
                block(vec![stmt(int(3))]),
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
        // (BlockId(1)), false → false_target_0 (BlockId(2)).
        match &cfg.blocks[0].terminator {
            Terminator::Branch {
                true_bb, false_bb, ..
            } => {
                assert_eq!(
                    *true_bb,
                    BlockId(1),
                    "entry's Branch.true_bb should be then_0 (BlockId 1)"
                );
                assert_eq!(
                    *false_bb,
                    BlockId(2),
                    "entry's Branch.false_bb should be false_target_0 (BlockId 2)"
                );
            }
            other => panic!("entry should have Branch terminator, got {:?}", other),
        }

        // false_target_0 (BlockId(2)): Branch with true → then_1
        // (BlockId(3)), false → else (BlockId(4)).
        match &cfg.blocks[2].terminator {
            Terminator::Branch {
                true_bb, false_bb, ..
            } => {
                assert_eq!(
                    *true_bb,
                    BlockId(3),
                    "false_target_0's Branch.true_bb should be then_1 (BlockId 3)"
                );
                assert_eq!(
                    *false_bb,
                    BlockId(4),
                    "false_target_0's Branch.false_bb should be else (BlockId 4)"
                );
            }
            other => panic!(
                "false_target_0 should have Branch terminator, got {:?}",
                other
            ),
        }

        // All three body blocks (1, 3, 4) Jump to join_block
        // (BlockId(5)).
        assert!(
            matches!(&cfg.blocks[1].terminator, Terminator::Jump(bb) if *bb == BlockId(5)),
            "then_0 (BlockId 1) should Jump to join_block (BlockId 5)"
        );
        assert!(
            matches!(&cfg.blocks[3].terminator, Terminator::Jump(bb) if *bb == BlockId(5)),
            "then_1 (BlockId 3) should Jump to join_block (BlockId 5)"
        );
        assert!(
            matches!(&cfg.blocks[4].terminator, Terminator::Jump(bb) if *bb == BlockId(5)),
            "else (BlockId 4) should Jump to join_block (BlockId 5)"
        );

        // Join block (BlockId(5)): Return terminator.
        assert!(
            matches!(&cfg.blocks[5].terminator, Terminator::Return(_)),
            "join block should have Return terminator"
        );
    }

    #[test]
    fn build_if_predecessors_are_filled_correctly() {
        // `fn f() -> int { if true { 1; } else { 2; } }`
        //
        // The branch bodies use bare integer statements so
        // neither branch has a Return terminator — this test
        // is about the build_if predecessors (which require
        // each branch to Jump to join, not Return). The
        // early-return case is covered by
        // `build_if_then_block_with_return_keeps_return_terminator`.
        //
        // Block IDs (Phase 1.5 fix — then_block is at index 1,
        // immediately after entry, so the linearizer's JMPF
        // fall-through lands on it):
        //   entry (0):  []
        //   then  (1):  [entry(0)]            (entry's Branch true)
        //   else  (2):  [entry(0)]            (entry's Branch false)
        //   join  (3):  [then(1), else(2)]    (each Jumps to join)
        let func = function(
            "f",
            vec![],
            Some("int"),
            block(vec![stmt(if_else(
                bool_lit(true),
                block(vec![stmt(int(1))]),
                block(vec![stmt(int(2))]),
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
            cfg.blocks[1].predecessors,
            vec![BlockId(0)],
            "then_block should have entry as predecessor"
        );
        assert_eq!(
            cfg.blocks[2].predecessors,
            vec![BlockId(0)],
            "else_block should have entry as predecessor"
        );

        // Join block has then AND else as predecessors (each
        // Jump targets join). Order is not guaranteed; compare
        // via a sorted copy (BlockId doesn't derive Ord, so use
        // sort_by_key).
        let mut join_preds = cfg.blocks[3].predecessors.clone();
        join_preds.sort_by_key(|b| b.0);
        assert_eq!(
            join_preds,
            vec![BlockId(1), BlockId(2)],
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
                b.insts
                    .iter()
                    .any(|i| matches!(i, Inst::Const { value: 0, .. }))
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

    // -----------------------------------------------------------------
    // While loops (Phase 1.1)
    // -----------------------------------------------------------------

    #[test]
    fn build_while_loop_produces_four_blocks() {
        // `fn f() -> int {
        //     while false { return 1; }
        //     return 0;
        // }`
        //
        // Block structure:
        //   0 (entry):   Jump(1)
        //   1 (header):  ConstBool(false) → v0, Branch(v0, 2, 3)
        //   2 (body):    Const(1) → v1,      Jump(1)
        //   3 (exit):    Const(0) → v2,      Return(Some(v2))
        let func = function(
            "f",
            vec![],
            Some("int"),
            block(vec![
                stmt(while_loop(bool_lit(false), block(vec![stmt(ret(int(1)))]))),
                stmt(ret(int(0))),
            ]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        assert_eq!(
            cfg.blocks.len(),
            4,
            "expected 4 blocks for while loop (entry + header + body + exit), got {}",
            cfg.blocks.len()
        );
    }

    #[test]
    fn build_while_loop_header_branches_to_body_and_exit() {
        // `fn f() -> int { while true { return 1; } return 0; }`
        //
        // Block IDs: 0 (entry), 1 (header), 2 (body), 3 (exit).
        //
        // The header's Branch should have:
        //   true_bb = body_block = BlockId(2)
        //   false_bb = exit_block = BlockId(3)
        //
        // Note: the body block (BlockId(2)) comes IMMEDIATELY
        // after the header (BlockId(1)) in declaration order,
        // which is what the linearizer's "true_bb reachable
        // by fall-through" expectation requires.
        let func = function(
            "f",
            vec![],
            Some("int"),
            block(vec![
                stmt(while_loop(bool_lit(true), block(vec![stmt(ret(int(1)))]))),
                stmt(ret(int(0))),
            ]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        // Find the header block: it's the one with a Branch
        // terminator whose condition is the bool_const(true).
        let header_idx = cfg
            .blocks
            .iter()
            .position(|b| {
                b.insts
                    .iter()
                    .any(|i| matches!(i, Inst::ConstBool { value: true, .. }))
                    && matches!(b.terminator, Terminator::Branch { .. })
            })
            .expect("expected a header block with ConstBool(true) and Branch");

        match &cfg.blocks[header_idx].terminator {
            Terminator::Branch {
                true_bb, false_bb, ..
            } => {
                assert_eq!(
                    *true_bb,
                    BlockId(header_idx as u32 + 1),
                    "header's Branch.true_bb should be the body block \
                     (the block immediately after the header)"
                );
                assert_eq!(
                    *false_bb,
                    BlockId(header_idx as u32 + 2),
                    "header's Branch.false_bb should be the exit block \
                     (the block immediately after the body)"
                );
                assert_ne!(
                    true_bb, false_bb,
                    "true_bb and false_bb should be different blocks"
                );
            }
            other => panic!(
                "expected Branch terminator at header (idx={}), got {:?}",
                header_idx, other
            ),
        }
    }

    #[test]
    fn build_while_loop_body_terminator_jumps_back_to_header() {
        // `fn f() -> int { while true { 1; } return 0; }`
        //
        // The body uses a bare integer statement (`1;`) so the
        // body block has no Return terminator — this test is
        // about the build_while BLOCK STRUCTURE (the back-edge
        // Jump), not early-return semantics. The early-return
        // case is covered by
        // `build_while_body_with_return_keeps_return_terminator`.
        //
        // Block IDs: 0 (entry), 1 (header), 2 (body), 3 (exit).
        // The body (BlockId(2)) should Jump back to the header
        // (BlockId(1)) — the back-edge.
        let func = function(
            "f",
            vec![],
            Some("int"),
            block(vec![
                stmt(while_loop(bool_lit(true), block(vec![stmt(int(1))]))),
                stmt(ret(int(0))),
            ]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        // Find the header's BlockId by its Branch terminator.
        let header_id = cfg
            .blocks
            .iter()
            .find(|b| matches!(b.terminator, Terminator::Branch { .. }))
            .map(|b| b.id)
            .expect("expected a header block with Branch terminator");

        // Find the body block: it has a Jump terminator whose
        // target is the header. (The entry block also has a
        // Jump terminator, but its target is the header too —
        // it's the entry edge, not the back-edge. The body
        // block is the one with the body's instruction
        // (Const(1)) and the back-edge Jump.)
        let body_block = cfg
            .blocks
            .iter()
            .find(|b| {
                b.insts
                    .iter()
                    .any(|i| matches!(i, Inst::Const { value: 1, .. }))
                    && matches!(b.terminator, Terminator::Jump(target) if target == header_id)
            })
            .expect("expected a body block with Const(1) and Jump back-edge");

        if let Terminator::Jump(target) = body_block.terminator {
            assert_eq!(
                target, header_id,
                "body's Jump target should be the header (the back-edge)"
            );
        } else {
            panic!(
                "expected Jump terminator on body block, got {:?}",
                body_block.terminator
            );
        }
    }

    #[test]
    fn build_while_loop_predecessors_are_filled_correctly() {
        // `fn f() -> int { while true { 1; } return 0; }`
        //
        // The body uses a bare integer statement (`1;`) so the
        // body block has no Return terminator — this test is
        // about the build_while predecessors (which require the
        // body to Jump back to the header, not Return). The
        // early-return case is covered by
        // `build_while_body_with_return_keeps_return_terminator`.
        //
        // Block IDs: 0 (entry), 1 (header), 2 (body), 3 (exit).
        //
        // Expected predecessors:
        //   entry (0):  []
        //   header (1): [entry (initial entry edge), body (back-edge)]
        //   body (2):   [header (Branch true arm)]
        //   exit (3):   [header (Branch false arm)]
        let func = function(
            "f",
            vec![],
            Some("int"),
            block(vec![
                stmt(while_loop(bool_lit(true), block(vec![stmt(int(1))]))),
                stmt(ret(int(0))),
            ]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        // Identify each block by its role:
        //   entry:  Jump terminator targeting the header (BlockId(1))
        //   header: Branch terminator (the only Branch)
        //   body:   Const(1) + Jump terminator targeting header
        //   exit:   Return terminator (the only Return)
        let entry_id = cfg
            .blocks
            .iter()
            .find(|b| matches!(b.terminator, Terminator::Jump(t) if t == BlockId(1)))
            .map(|b| b.id)
            .expect("expected an entry block with Jump(1) terminator");
        let header_id = cfg
            .blocks
            .iter()
            .find(|b| matches!(b.terminator, Terminator::Branch { .. }))
            .map(|b| b.id)
            .expect("expected a header block with Branch terminator");
        let body_id = cfg
            .blocks
            .iter()
            .find(|b| matches!(b.terminator, Terminator::Jump(t) if t == header_id && b.insts.iter().any(|i| matches!(i, Inst::Const { value: 1, .. }))))
            .map(|b| b.id)
            .expect("expected a body block with Const(1) and Jump back-edge");
        let exit_id = cfg
            .blocks
            .iter()
            .find(|b| matches!(b.terminator, Terminator::Return(_)))
            .map(|b| b.id)
            .expect("expected an exit block with Return terminator");

        // Entry has no predecessors.
        assert!(
            cfg.blocks[entry_id.index()].predecessors.is_empty(),
            "entry block should have no predecessors, got {:?}",
            cfg.blocks[entry_id.index()].predecessors
        );

        // Header has [entry, body] as predecessors (initial
        // entry edge + back-edge from body). Order is not
        // guaranteed; compare via a sorted copy.
        let mut header_preds = cfg.blocks[header_id.index()].predecessors.clone();
        header_preds.sort_by_key(|b| b.0);
        let mut expected_header_preds = vec![entry_id, body_id];
        expected_header_preds.sort_by_key(|b| b.0);
        assert_eq!(
            header_preds, expected_header_preds,
            "header should have [entry, body] as predecessors"
        );

        // Body has header as sole predecessor (Branch true arm).
        assert_eq!(
            cfg.blocks[body_id.index()].predecessors,
            vec![header_id],
            "body block should have header as sole predecessor"
        );

        // Exit has header as sole predecessor (Branch false arm).
        assert_eq!(
            cfg.blocks[exit_id.index()].predecessors,
            vec![header_id],
            "exit block should have header as sole predecessor"
        );
    }

    // -----------------------------------------------------------------
    // Phase 1.6: early-return preservation.
    //
    // Before Phase 1.6, `Expression::Return` only set
    // `self.return_value`; the current block's terminator stayed
    // `Unreachable`. The post-loop in `build_if` /
    // `build_while` then unconditionally overwrote the block's
    // terminator with `Jump(join_block)` / `Jump(header_block)`,
    // silently converting early returns into fall-throughs.
    //
    // Phase 1.6 fixes both halves of the bug:
    //   1. `Expression::Return` now sets the current block's
    //      terminator to `Terminator::Return`.
    //   2. The post-loop is conditional on
    //      `Terminator::Unreachable`, so blocks with a Return
    //      terminator are left alone.
    //
    // The tests below verify both halves end-to-end.
    // -----------------------------------------------------------------

    #[test]
    fn build_if_then_block_with_return_keeps_return_terminator() {
        // `fn f() -> int { if c { return 1; } else { return 2; } }`
        //
        // Both the then_block and the else_block contain a
        // `return` statement. After Phase 1.6, both blocks should
        // have `Terminator::Return(...)` instead of being
        // overwritten with `Jump(join_block)`.
        //
        // Block structure:
        //   0 (entry):      Cond(c) → v0,  Branch(v0, 1, 2)
        //   1 (then_block): Const(1) → v1, Return(Some(v1))
        //   2 (else_block): Const(2) → v2, Return(Some(v2))
        //   3 (join):       Return(None)  (no continuation)
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

        // then_block (BlockId 1) keeps its Return terminator.
        assert!(
            matches!(&cfg.blocks[1].terminator, Terminator::Return(Some(_))),
            "then_block should have Return(Some(_)) terminator, got {:?}",
            cfg.blocks[1].terminator
        );

        // else_block (BlockId 2) keeps its Return terminator.
        assert!(
            matches!(&cfg.blocks[2].terminator, Terminator::Return(Some(_))),
            "else_block should have Return(Some(_)) terminator, got {:?}",
            cfg.blocks[2].terminator
        );

        // join_block (BlockId 3) has Return (no continuation).
        assert!(
            matches!(&cfg.blocks[3].terminator, Terminator::Return(_)),
            "join block should have Return terminator, got {:?}",
            cfg.blocks[3].terminator
        );
    }

    #[test]
    fn build_if_with_only_one_return_keeps_other_as_jump() {
        // `fn f() -> int { if c { return 1; } else { 2; } }`
        //
        // Mixed case: only one branch has a `return`. After
        // Phase 1.6, the branch with the return keeps its Return
        // terminator; the branch without falls through to
        // join_block via the post-loop's Jump(join) patch.
        //
        // Block structure:
        //   0 (entry):      Cond(c) → v0,  Branch(v0, 1, 2)
        //   1 (then_block): Const(1) → v1, Return(Some(v1))
        //   2 (else_block): Const(2) → v2, Jump(3)
        //   3 (join):       Return(None)  (no continuation)
        let func = function(
            "f",
            vec![],
            Some("int"),
            block(vec![stmt(if_else(
                bool_lit(true),
                block(vec![stmt(ret(int(1)))]),
                block(vec![stmt(int(2))]),
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

        // then_block has Return (early return inside the branch).
        assert!(
            matches!(&cfg.blocks[1].terminator, Terminator::Return(Some(_))),
            "then_block should have Return(Some(_)) terminator, got {:?}",
            cfg.blocks[1].terminator
        );

        // else_block falls through to join_block via Jump.
        assert!(
            matches!(&cfg.blocks[2].terminator, Terminator::Jump(bb) if *bb == BlockId(3)),
            "else_block should have Jump(3) terminator, got {:?}",
            cfg.blocks[2].terminator
        );

        // join_block has Return (no continuation after the if).
        assert!(
            matches!(&cfg.blocks[3].terminator, Terminator::Return(_)),
            "join block should have Return terminator, got {:?}",
            cfg.blocks[3].terminator
        );
    }

    #[test]
    fn build_while_body_with_return_keeps_return_terminator() {
        // `fn f() -> int { while c { return 1; } return 0; }`
        //
        // The body contains a `return`. After Phase 1.6, the
        // body block keeps its Return terminator instead of
        // being overwritten with `Jump(header)`. The back-edge
        // Jump is no longer emitted for that block.
        //
        // Block structure:
        //   0 (entry):   Jump(1)
        //   1 (header):  ConstBool(c) → v0, Branch(v0, 2, 3)
        //   2 (body):    Const(1) → v1,     Return(Some(v1))
        //   3 (exit):    Const(0) → v2,     Return(Some(v2))
        let func = function(
            "f",
            vec![],
            Some("int"),
            block(vec![
                stmt(while_loop(bool_lit(true), block(vec![stmt(ret(int(1)))]))),
                stmt(ret(int(0))),
            ]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        assert_eq!(
            cfg.blocks.len(),
            4,
            "expected 4 blocks (entry + header + body + exit), got {}",
            cfg.blocks.len()
        );

        // Find the body block: it's the one with Const(1) AND
        // Return (not Jump back-edge).
        let body_block = cfg
            .blocks
            .iter()
            .find(|b| {
                b.insts
                    .iter()
                    .any(|i| matches!(i, Inst::Const { value: 1, .. }))
                    && matches!(b.terminator, Terminator::Return(_))
            })
            .expect("expected a body block with Const(1) and Return");

        assert!(
            matches!(body_block.terminator, Terminator::Return(Some(_))),
            "body block should have Return(Some(_)) terminator (early return), got {:?}",
            body_block.terminator
        );
    }

    #[test]
    fn build_while_body_with_no_return_keeps_back_edge_jump() {
        // `fn f() -> int { while c { 1; } return 0; }`
        //
        // Regression check: the body has NO `return`. After
        // Phase 1.6, the body block should still have
        // `Jump(header)` (the back-edge). The conditional
        // post-loop must NOT clobber the Jump.
        //
        // Block structure:
        //   0 (entry):   Jump(1)
        //   1 (header):  ConstBool(c) → v0, Branch(v0, 2, 3)
        //   2 (body):    Const(1) → v1,     Jump(1)  ← back-edge
        //   3 (exit):    Const(0) → v2,     Return(Some(v2))
        let func = function(
            "f",
            vec![],
            Some("int"),
            block(vec![
                stmt(while_loop(bool_lit(true), block(vec![stmt(int(1))]))),
                stmt(ret(int(0))),
            ]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        assert_eq!(
            cfg.blocks.len(),
            4,
            "expected 4 blocks (entry + header + body + exit), got {}",
            cfg.blocks.len()
        );

        // Find the header's BlockId by its Branch terminator.
        let header_id = cfg
            .blocks
            .iter()
            .find(|b| matches!(b.terminator, Terminator::Branch { .. }))
            .map(|b| b.id)
            .expect("expected a header block with Branch terminator");

        // The body block must have Jump(header) — the back-edge.
        let body_block = cfg
            .blocks
            .iter()
            .find(|b| {
                b.insts
                    .iter()
                    .any(|i| matches!(i, Inst::Const { value: 1, .. }))
                    && matches!(&b.terminator, Terminator::Jump(target) if *target == header_id)
            })
            .expect("expected a body block with Const(1) and Jump back-edge");

        if let Terminator::Jump(target) = body_block.terminator {
            assert_eq!(
                target, header_id,
                "body's Jump target should be the header (the back-edge)"
            );
        } else {
            panic!(
                "expected Jump terminator on body block, got {:?}",
                body_block.terminator
            );
        }
    }

    #[test]
    fn build_function_level_return_sets_entry_block_terminator() {
        // `fn f() -> int { return 42; }`
        //
        // Phase 1.6 fix: Expression::Return now sets the
        // current block's terminator to Return. For a
        // function-level return, the current block IS the entry
        // block, so the entry block's terminator should be
        // Return(Some(v42)) — not overwritten later.
        let func = function("f", vec![], Some("int"), block(vec![stmt(ret(int(42)))]));
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        assert_eq!(
            cfg.blocks.len(),
            1,
            "function-level return should produce 1 block"
        );

        // Entry block should have Return(Some(v0)) — the
        // ValueId of Const(42).
        let entry = &cfg.blocks[0];
        assert!(
            matches!(&entry.terminator, Terminator::Return(Some(v)) if *v == ValueId(0)),
            "entry block should have Return(Some(v0)) terminator, got {:?}",
            entry.terminator
        );
    }

    // -----------------------------------------------------------------
    // Match expressions (Phase 1.3)
    //
    // The builder produces a multi-block CFG with a `Switch`
    // terminator on the entry block. Each arm gets its own
    // block; the last arm is the Switch's `default` target; all
    // arms Jump to a join block.
    //
    // Pattern support for Phase 1.3:
    //   - `Constructor(Enum::Variant(args))` — non-last arms
    //     become Switch cases (tag from placeholder FNV-1a
    //     hash). Last-arm constructors become the default.
    //   - `Wildcard` — must be the LAST arm (default).
    //   - `Binding` — must be the LAST arm (default). Binding
    //     code is silently dropped (Phase 1.4+).
    // -----------------------------------------------------------------

    #[test]
    fn build_match_two_arms_produces_four_blocks() {
        // `fn f(int x) -> int {
        //     match x { Some => return 1; None => return 0; }
        // }`
        //
        // Block structure for a 2-arm match:
        //   0 (match_block):  Param(x → v0), Switch scrutinee=v0,
        //                     [Some_tag → arm_0], default=arm_1
        //   1 (arm_0):        Const(1) → v1, Jump(3)
        //   2 (arm_1, default): Const(0) → v2, Jump(3)
        //   3 (join):         Return(None)
        //
        // Note: the scrutinee `x` is just an Identifier lookup,
        // so no extra instruction is emitted for it. The Switch's
        // scrutinee operand is the existing v0.
        let func = function(
            "f",
            vec![("int", "x")],
            Some("int"),
            block(vec![stmt(match_expr(
                ident("x"),
                vec![
                    match_arm(
                        constructor_pattern("Option", "Some"),
                        block(vec![stmt(ret(int(1)))]),
                    ),
                    match_arm(
                        constructor_pattern("Option", "None"),
                        block(vec![stmt(ret(int(0)))]),
                    ),
                ],
            ))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        // 4 blocks: match + arm_0 + arm_1 (default) + join.
        assert_eq!(
            cfg.blocks.len(),
            4,
            "expected 4 blocks for 2-arm match, got {}",
            cfg.blocks.len()
        );

        // BlockId roles (assigned in push order):
        //   0 = match_block, 1 = arm_0, 2 = arm_1 (default),
        //   3 = join_block.
        assert_eq!(cfg.blocks[0].id, BlockId(0), "match_block id");
        assert_eq!(cfg.blocks[1].id, BlockId(1), "arm_0 id");
        assert_eq!(cfg.blocks[2].id, BlockId(2), "arm_1 id");
        assert_eq!(cfg.blocks[3].id, BlockId(3), "join_block id");
    }

    #[test]
    fn build_match_uses_switch_terminator_with_one_case() {
        // 2-arm match with constructor patterns. The Switch
        // terminator should have:
        //   - cases = [(Some_tag, arm_0)]
        //   - default = arm_1 (the last arm)
        let func = function(
            "f",
            vec![("int", "x")],
            Some("int"),
            block(vec![stmt(match_expr(
                ident("x"),
                vec![
                    match_arm(
                        constructor_pattern("Option", "Some"),
                        block(vec![stmt(ret(int(1)))]),
                    ),
                    match_arm(
                        constructor_pattern("Option", "None"),
                        block(vec![stmt(ret(int(0)))]),
                    ),
                ],
            ))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        // Find the match block (the one with Switch terminator).
        let match_block = cfg
            .blocks
            .iter()
            .find(|b| matches!(b.terminator, Terminator::Switch { .. }))
            .expect("expected a Switch terminator on the match block");

        // match_block should be BlockId(0) (the entry).
        assert_eq!(
            match_block.id,
            BlockId(0),
            "match_block should be the entry (BlockId 0)"
        );

        match &match_block.terminator {
            Terminator::Switch {
                scrutinee,
                cases,
                default,
            } => {
                // The scrutinee is `x`, which is the Param
                // (ValueId(0)).
                assert_eq!(
                    *scrutinee,
                    ValueId(0),
                    "scrutinee should be x's ValueId (v0)"
                );
                // Exactly one case (the first arm — Some).
                assert_eq!(cases.len(), 1, "expected 1 case (first arm only)");
                // The case target should be arm_0 (BlockId(1)).
                assert_eq!(
                    cases[0].1,
                    BlockId(1),
                    "first case target should be arm_0 (BlockId 1)"
                );
                // The default should be arm_1 (BlockId(2)) —
                // the last arm.
                assert_eq!(*default, BlockId(2), "default should be arm_1 (BlockId 2)");
            }
            other => panic!("expected Switch terminator on match block, got {:?}", other),
        }
    }

    #[test]
    fn build_match_arms_end_with_jump_to_join() {
        // After the Switch, both arm blocks should end with
        // Jump(join_block) — the canonical threaded-code layout.
        let func = function(
            "f",
            vec![("int", "x")],
            Some("int"),
            block(vec![stmt(match_expr(
                ident("x"),
                vec![
                    match_arm(
                        constructor_pattern("Option", "Some"),
                        block(vec![stmt(ret(int(1)))]),
                    ),
                    match_arm(
                        constructor_pattern("Option", "None"),
                        block(vec![stmt(ret(int(0)))]),
                    ),
                ],
            ))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        // arm_0 (BlockId 1) and arm_1 (BlockId 2) should both
        // Jump to join_block (BlockId 3).
        assert!(
            matches!(&cfg.blocks[1].terminator, Terminator::Jump(bb) if *bb == BlockId(3)),
            "arm_0 should Jump to join_block (BlockId 3), got {:?}",
            cfg.blocks[1].terminator
        );
        assert!(
            matches!(&cfg.blocks[2].terminator, Terminator::Jump(bb) if *bb == BlockId(3)),
            "arm_1 should Jump to join_block (BlockId 3), got {:?}",
            cfg.blocks[2].terminator
        );

        // Join block (BlockId 3) should have Return terminator
        // (no continuation code after the match).
        assert!(
            matches!(&cfg.blocks[3].terminator, Terminator::Return(_)),
            "join_block should have Return terminator, got {:?}",
            cfg.blocks[3].terminator
        );
    }

    #[test]
    fn build_match_wildcard_as_last_arm_is_default() {
        // `match x { Some(_) => return 1; _ => return 0; }`
        //
        // Block structure:
        //   0 (match_block):  Param(x → v0), Switch scrutinee=v0,
        //                     [Some_tag → arm_0], default=arm_1
        //   1 (arm_0):        Const(1) → v1, Jump(3)
        //   2 (arm_1, _):     Const(0) → v2, Jump(3)
        //   3 (join):         Return(None)
        let func = function(
            "f",
            vec![("int", "x")],
            Some("int"),
            block(vec![stmt(match_expr(
                ident("x"),
                vec![
                    match_arm(
                        constructor_pattern("Option", "Some"),
                        block(vec![stmt(ret(int(1)))]),
                    ),
                    match_arm(wildcard_pattern(), block(vec![stmt(ret(int(0)))])),
                ],
            ))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        // 4 blocks: match + arm_0 + wildcard_arm + join.
        assert_eq!(cfg.blocks.len(), 4);

        // Switch should have 1 case (Some) and the default
        // should be the wildcard arm (BlockId(2)).
        let match_block = cfg
            .blocks
            .iter()
            .find(|b| matches!(b.terminator, Terminator::Switch { .. }))
            .unwrap();
        match &match_block.terminator {
            Terminator::Switch { cases, default, .. } => {
                assert_eq!(cases.len(), 1, "expected 1 case (the Some arm)");
                assert_eq!(
                    *default,
                    BlockId(2),
                    "default should be the wildcard arm (BlockId 2)"
                );
            }
            other => panic!("expected Switch terminator, got {:?}", other),
        }

        // The wildcard arm should be BlockId(2) and should
        // contain Const(0) (the `return 0` body).
        let wildcard_arm = &cfg.blocks[2];
        assert!(
            wildcard_arm
                .insts
                .iter()
                .any(|i| matches!(i, Inst::Const { value: 0, .. })),
            "wildcard arm should contain Const(0)"
        );
    }

    #[test]
    fn build_match_single_arm_has_no_cases() {
        // `fn f(int x) -> int { match x { _ => return 0; } }`
        //
        // A single Wildcard arm — the degenerate case. The Switch
        // has 0 cases (no constructor arms) and the default is
        // the single arm.
        //
        // Block structure:
        //   0 (match_block):  Param(x → v0), Switch scrutinee=v0,
        //                     [], default=arm_0
        //   1 (arm_0, _):     Const(0) → v1, Jump(2)
        //   2 (join):         Return(None)
        let func = function(
            "f",
            vec![("int", "x")],
            Some("int"),
            block(vec![stmt(match_expr(
                ident("x"),
                vec![match_arm(
                    wildcard_pattern(),
                    block(vec![stmt(ret(int(0)))]),
                )],
            ))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        // 3 blocks: match + arm_0 + join.
        assert_eq!(
            cfg.blocks.len(),
            3,
            "expected 3 blocks for single-arm match, got {}",
            cfg.blocks.len()
        );

        // Switch has 0 cases and default is arm_0 (BlockId(1)).
        match &cfg.blocks[0].terminator {
            Terminator::Switch { cases, default, .. } => {
                assert_eq!(cases.len(), 0, "expected 0 cases for single Wildcard arm");
                assert_eq!(
                    *default,
                    BlockId(1),
                    "default should be the single arm (BlockId 1)"
                );
            }
            other => panic!("expected Switch terminator, got {:?}", other),
        }

        // arm_0 Jumps to join_block (BlockId(2)).
        assert!(
            matches!(&cfg.blocks[1].terminator, Terminator::Jump(bb) if *bb == BlockId(2)),
            "arm_0 should Jump to join_block (BlockId 2)"
        );
    }

    #[test]
    fn build_match_predecessors_are_filled_correctly() {
        // 2-arm match: the predecessor pass should fill:
        //   match_block (0): []      (entry, no predecessors)
        //   arm_0 (1):        [0]     (Switch case target)
        //   arm_1 (2):        [0]     (Switch default target)
        //   join_block (3):   [1, 2]  (both arms Jump to join)
        let func = function(
            "f",
            vec![("int", "x")],
            Some("int"),
            block(vec![stmt(match_expr(
                ident("x"),
                vec![
                    match_arm(
                        constructor_pattern("Option", "Some"),
                        block(vec![stmt(ret(int(1)))]),
                    ),
                    match_arm(
                        constructor_pattern("Option", "None"),
                        block(vec![stmt(ret(int(0)))]),
                    ),
                ],
            ))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        // match_block has no predecessors.
        assert!(
            cfg.blocks[0].predecessors.is_empty(),
            "match_block should have no predecessors"
        );

        // arm_0 has match_block as sole predecessor (Switch case).
        assert_eq!(
            cfg.blocks[1].predecessors,
            vec![BlockId(0)],
            "arm_0 should have match_block as sole predecessor"
        );

        // arm_1 has match_block as sole predecessor (Switch default).
        assert_eq!(
            cfg.blocks[2].predecessors,
            vec![BlockId(0)],
            "arm_1 should have match_block as sole predecessor"
        );

        // join_block has arm_0 AND arm_1 as predecessors. Order
        // is not guaranteed; compare via a sorted copy.
        let mut join_preds = cfg.blocks[3].predecessors.clone();
        join_preds.sort_by_key(|b| b.0);
        assert_eq!(
            join_preds,
            vec![BlockId(1), BlockId(2)],
            "join_block should have arm_0 + arm_1 as predecessors"
        );
    }

    #[test]
    fn build_match_scrutinee_evaluation_lands_in_match_block() {
        // `fn f(int x) -> int {
        //     match x + 1 { Some => return 1; None => return 0; }
        // }`
        //
        // The scrutinee is `x + 1` — a binary expression. Its
        // evaluation should land in the match_block (BlockId(0))
        // as two instructions: `Const(1)` (v1) and
        // `BinOp(Add, v2, v0, v1)`. The Switch's scrutinee
        // operand should reference v2 (the Add's result).
        let func = function(
            "f",
            vec![("int", "x")],
            Some("int"),
            block(vec![stmt(match_expr(
                add(ident("x"), int(1)),
                vec![
                    match_arm(
                        constructor_pattern("Option", "Some"),
                        block(vec![stmt(ret(int(1)))]),
                    ),
                    match_arm(
                        constructor_pattern("Option", "None"),
                        block(vec![stmt(ret(int(0)))]),
                    ),
                ],
            ))]),
        );
        let mut builder = Builder::new();
        let cfg = builder.build_function(&func);

        // The match_block should contain Param + Const + BinOp
        // (the scrutinee evaluation).
        let match_block = &cfg.blocks[0];

        // Param(x → v0).
        let has_param = match_block.insts.iter().any(|i| {
            matches!(
                i,
                Inst::Param {
                    dst: ValueId(0),
                    index: 0
                }
            )
        });
        assert!(has_param, "match_block should contain Param(x → v0)");

        // Const(1) → v1.
        let has_const_1 = match_block.insts.iter().any(|i| {
            matches!(
                i,
                Inst::Const {
                    dst: ValueId(1),
                    value: 1
                }
            )
        });
        assert!(has_const_1, "match_block should contain Const(1)");

        // BinOp(Add, v2, v0, v1).
        let has_add = match_block.insts.iter().any(|i| {
            matches!(
                i,
                Inst::BinOp {
                    op: BinOpKind::Add,
                    dst: ValueId(2),
                    lhs: ValueId(0),
                    rhs: ValueId(1),
                }
            )
        });
        assert!(has_add, "match_block should contain BinOp(Add, v2, v0, v1)");

        // Switch's scrutinee operand should be v2 (the Add's result).
        match &match_block.terminator {
            Terminator::Switch { scrutinee, .. } => {
                assert_eq!(
                    *scrutinee,
                    ValueId(2),
                    "Switch's scrutinee should be the Add's ValueId (v2)"
                );
            }
            other => panic!("expected Switch terminator, got {:?}", other),
        }
    }

    #[test]
    #[should_panic(expected = "Wildcard pattern must be the LAST arm")]
    fn build_match_wildcard_as_non_last_arm_panics() {
        // `match x { _ => 1, None => 0 }` — Wildcard is NOT the
        // last arm. The builder should panic.
        let func = function(
            "f",
            vec![("int", "x")],
            Some("int"),
            block(vec![stmt(match_expr(
                ident("x"),
                vec![
                    match_arm(wildcard_pattern(), block(vec![stmt(ret(int(1)))])),
                    match_arm(
                        constructor_pattern("Option", "None"),
                        block(vec![stmt(ret(int(0)))]),
                    ),
                ],
            ))]),
        );
        let mut builder = Builder::new();
        let _ = builder.build_function(&func);
    }

    #[test]
    #[should_panic(expected = "Binding pattern must be the LAST arm")]
    fn build_match_binding_as_non_last_arm_panics() {
        // `match x { y => 1, None => 0 }` — Binding is NOT the
        // last arm. The builder should panic.
        let func = function(
            "f",
            vec![("int", "x")],
            Some("int"),
            block(vec![stmt(match_expr(
                ident("x"),
                vec![
                    match_arm(binding_pattern("y"), block(vec![stmt(ret(int(1)))])),
                    match_arm(
                        constructor_pattern("Option", "None"),
                        block(vec![stmt(ret(int(0)))]),
                    ),
                ],
            ))]),
        );
        let mut builder = Builder::new();
        let _ = builder.build_function(&func);
    }
}
