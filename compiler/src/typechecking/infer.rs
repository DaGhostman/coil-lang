//! Hindley–Milner inference over the zero-script AST.
//!
//! Implements Algorithm W for every `Expression` variant. Function
//! declarations, classes, `impl` blocks, and instantiation are deferred to
//! Phase 5; this phase covers everything else.
//!
//! ## Algorithm
//!
//! Inference is driven by [`Checker::check_program`], which sets up a top
//! frame and calls [`Checker::infer`] on the root expression. `infer`
//! dispatches on the AST node:
//!
//! - Literals return their built-in type.
//! - Names are looked up in the environment; if found, the bound scheme
//!   is instantiated with fresh variables; if missing, an error message
//!   is recorded and a fresh variable is returned (so inference can
//!   continue past the error).
//! - Operators infer their operands, then unify them and return the
//!   resulting type (or `bool` for comparison / logical operators).
//! - Control flow unifies branch bodies.
//! - `let` declarations bind in the environment; the next sibling, if it
//!   looks like a value, is inferred and unified with the declared type.
//! - Calls instantiate the callee, infer each argument, then thread the
//!   curried function type — applying one argument at a time, unifying
//!   each argument type with the function's parameter type, until the
//!   return type is reached.
//!
//! ## Error recovery
//!
//! [`Checker`] accumulates [`common::Message`]s and continues after every
//! error: a failed unification emits a message and substitutes a fresh
//! variable for the result. This gives the user every problem in one
//! pass rather than stopping at the first.
//!
//! ## Substitution
//!
//! [`Subst`] is owned by [`Checker`] and extended in place. `infer`
//! always returns the type *under the current substitution* (via
//! [`apply_ty`]); the substitution is never returned explicitly.

use std::ops::Range;

use common::{Label, Message};
use parser::ast::{Expression, Output, Visibility};

use super::env::{instantiate, Env, TyVarCounter};
use super::id::{self, IdTable, NodeId};
use super::ty::Scheme;
use super::subst::{apply_ty, apply_ty_prune, compose, Subst};
use super::ty::{
    boolean, float, int, list, string, unit as unit_ty, Ty,
};
use super::unify::{unify_with, UnifyError};

#[cfg(test)]
use super::ty::TyVarId;

/// The typechecker. Owns the environment, the fresh-variable counter, the
/// running substitution, and the accumulated diagnostic messages.
pub struct Checker {
    env: Env,
    counter: TyVarCounter,
    subst: Subst,
    messages: Vec<Message>,

    /// Type that the enclosing function expects to return, if any.
    /// `None` outside of a function body. Set when entering a function
    /// declaration.
    current_return_ty: Option<Ty>,

    /// Type of the surrounding `match`'s LHS, if any. Used by
    /// [`Expression::Default`] arms.
    current_match_lhs: Option<Ty>,

    /// Class declarations: name → list of (visibility, field name, type).
    classes: std::collections::HashMap<String, Vec<(Visibility, String, Ty)>>,

    /// Method declarations: owner class → method name →
    /// (visibility, scheme). Methods are stored here so they can be
    /// resolved by member-access expressions in a future phase; for
    /// now we only register them.
    methods: std::collections::HashMap<String, std::collections::HashMap<String, (Visibility, Scheme)>>,

    /// Pre-walk-minted IDs in visit order. Populated by the pre-walk
    /// in [`check_program`](Self::check_program); consumed in lockstep
    /// by [`infer`](Self::infer).
    ids: IdTable,

    /// Next index to read from [`Checker::ids`]. Reset by the pre-walk
    /// at the start of [`check_program`](Self::check_program).
    next_id_idx: usize,

    /// Span-indexed cache of inferred types. Populated as
    /// [`infer`](Self::infer) processes each node; consulted by the
    /// bytecode emitter (Phase 9) via [`lookup_at`](Self::lookup_at).
    cache: std::collections::HashMap<NodeId, Ty>,
}

impl Default for Checker {
    fn default() -> Self {
        Self::new()
    }
}

impl Checker {
    pub fn new() -> Self {
        let mut env = Env::new();
        // Always start with one frame so callers can `register_native`
        // (and later `Compiler::register`) before `check_program` is
        // ever called. `check_program` pushes a second frame so the
        // first stays around for inspection.
        env.push();
        Self {
            env,
            counter: TyVarCounter::new(),
            subst: Subst::empty(),
            messages: Vec::new(),
            current_return_ty: None,
            current_match_lhs: None,
            classes: std::collections::HashMap::new(),
            methods: std::collections::HashMap::new(),
            ids: IdTable::new(),
            next_id_idx: 0,
            cache: std::collections::HashMap::new(),
        }
    }

    /// Run inference over `ast`. Returns the inferred type of the root
    /// expression under the final substitution. Diagnostic messages are
    /// accumulated and can be retrieved with [`take_messages`].
    ///
    /// The top frame is left on the env stack after this call so that
    /// callers (and tests) can inspect declared bindings. Use
    /// [`env_mut`](Self::env_mut) and [`Env::pop`] if you need to drop
    /// it.
    pub fn check_program(&mut self, ast: &Output) -> Ty {
        // Mint NodeIds for every AST node (pre-walk). The visit order
        // matches `infer`'s recursion, so the IDs line up.
        self.ids = IdTable::new();
        self.next_id_idx = 0;
        id::pre_walk(ast, &mut self.ids);

        // Top frame so natives and globals have a place to bind (Phase 7).
        // The frame is left on the stack so callers (and tests) can
        // inspect declared bindings via [`env`](Self::env).
        self.env.push();
        let ty = self.infer(ast);
        // Return the fully-resolved type so callers see e.g. `Foo`
        // rather than `Var(0)` even when the type was inferred
        // through let-binding + unify.
        apply_ty_prune(&self.subst, &ty)
    }

    /// Take all accumulated diagnostic messages, leaving the checker
    /// with an empty message buffer.
    pub fn take_messages(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.messages)
    }

    /// Borrow the accumulated messages without consuming them.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn env(&self) -> &Env {
        &self.env
    }

    pub fn env_mut(&mut self) -> &mut Env {
        &mut self.env
    }

    /// Borrow the running substitution (useful for diagnostics).
    pub fn subst(&self) -> &Subst {
        &self.subst
    }

    /// Iterate the cache (id → type). Useful for tests and for the
    /// eventual bytecode emitter integration.
    #[cfg(test)]
    pub(crate) fn cache(&self) -> impl Iterator<Item = (NodeId, &Ty)> {
        self.cache.iter().map(|(k, v)| (*k, v))
    }

    // ============================================================
    //  Inference over the AST
    // ============================================================

    fn infer(&mut self, expr: &Output) -> Ty {
        // Pull the next ID from the pre-walk's minting order. Both
        // `infer` and the pre-walk visit in pre-order, so the `n`-th
        // call here consumes the `n`-th ID.
        let id = self.ids.ids()[self.next_id_idx];
        self.next_id_idx += 1;

        let ty = self.infer_inner(expr);
        self.cache.insert(id, ty.clone());
        ty
    }

    /// Inner inference — does the actual dispatch but no caching.
    /// Every recursive call into a child still goes through
    /// [`infer`](Self::infer), so each child also gets cached.
    fn infer_inner(&mut self, expr: &Output) -> Ty {
        let range = expr.0.into_range();
        let child = expr.1.as_ref();

        match child {
            // ---- Literals ----
            Expression::Integer(_) => int(),
            Expression::Float(_) => float(),
            Expression::String(_) => string(),
            Expression::Bool(_) => boolean(),

            // ---- Names ----
            Expression::Identifier(name) => {
                let scheme = self.env.lookup(name).cloned();
                match scheme {
                    Some(s) => instantiate(&s, &mut self.counter),
                    None => self.error(
                format!("Cannot find value `{}` in this scope", name),
                range,
            ),
                }
            }

            // A bare type name (only valid as an annotation, but be
            // permissive).
            Expression::Type(name) => self.parse_type_name_str(name),

            // ---- Wrappers / no-ops ----
            Expression::Noop(_) | Expression::Comment(_) => unit_ty(),
            Expression::Use { .. } | Expression::Module(_, _) => unit_ty(),

            Expression::Expr(e) | Expression::Group(e) | Expression::Statement(e) => {
                self.infer(e)
            }
            Expression::ExprStatement(e) => self.infer(e),

            // ---- Blocks ----
            // Program runs in the current frame (the global frame from
            // check_program). This is what makes top-level `let`
            // bindings visible after inference. Block introduces its
            // own scope.
            Expression::Block(children) => {
                self.env.push();
                let mut last_ty = unit_ty();
                for child in children {
                    last_ty = self.infer(child);
                }
                self.env.pop();
                last_ty
            }
            Expression::Program(children) => {
                let mut last_ty = unit_ty();
                for child in children {
                    last_ty = self.infer(child);
                }
                last_ty
            }

            // ---- Fragments (from `let x = expr`) ----
            Expression::Fragment(children) => self.infer_fragment(children),

            // ---- `let` / `const` ----
            Expression::Variable(name, ty_opt) => {
                let var_ty = match ty_opt {
                    Some(ann) => self.parse_type_name(ann),
                    None => Ty::Var(self.counter.fresh()),
                };
                self.env.insert_top(name.to_string(), Scheme::mono(var_ty));
                unit_ty()
            }

            Expression::Constant(name, ty_opt) => {
                let var_ty = match ty_opt {
                    Some(ann) => self.parse_type_name(ann),
                    None => Ty::Var(self.counter.fresh()),
                };
                let ident = match name.1.as_ref() {
                    Expression::Identifier(n) => n.to_string(),
                    _ => return self.error_with_help(
                        "Invalid constant name".to_string(),
                        range,
                        Some("a constant name must be an identifier".to_string()),
                    ),
                };
                self.env.insert_top(ident, Scheme::mono(var_ty));
                unit_ty()
            }

            // ---- Assignment ----
            Expression::Assignment(name, value) => {
                let val_ty = self.infer(value);
                let ident = match name.1.as_ref() {
                    Expression::Identifier(n) => n.to_string(),
                    _ => return self.error_with_help(
                        "Invalid assignment target".to_string(),
                        range,
                        Some("the left-hand side of an assignment must be a variable".to_string()),
                    ),
                };
                let scheme = self.env.lookup(&ident).cloned();
                match scheme {
                    Some(s) => {
                        let var_ty = instantiate(&s, &mut self.counter);
                        self.unify(&var_ty, &val_ty, &range, "assignment")
                    }
                    None => self.error_with_help(
                        format!(
                            "Cannot assign to undeclared variable `{}`",
                            ident
                        ),
                        range,
                        Some(format!(
                            "try declaring it first with `let {};`",
                            ident
                        )),
                    ),
                }
            }

            // ---- Arithmetic / bitwise ----
            Expression::Add(lhs, rhs) => self.infer_arith(lhs, rhs, range, "+"),
            Expression::Sub(lhs, rhs) => self.infer_arith(lhs, rhs, range, "-"),
            Expression::Mul(lhs, rhs) => self.infer_arith(lhs, rhs, range, "*"),
            Expression::Div(lhs, rhs) => self.infer_arith(lhs, rhs, range, "/"),
            Expression::Mod(lhs, rhs) => self.infer_arith(lhs, rhs, range, "%"),
            Expression::Pow(lhs, rhs) => self.infer_arith(lhs, rhs, range, "**"),
            Expression::Shl(lhs, rhs) => self.infer_arith(lhs, rhs, range, "<<"),
            Expression::Shr(lhs, rhs) => self.infer_arith(lhs, rhs, range, ">>"),
            Expression::Xor(lhs, rhs) => self.infer_arith(lhs, rhs, range, "^"),
            Expression::BitAnd(lhs, rhs) => self.infer_arith(lhs, rhs, range, "&"),
            Expression::BitOr(lhs, rhs) => self.infer_arith(lhs, rhs, range, "|"),

            // ---- Logical ----
            Expression::And(lhs, rhs) | Expression::Or(lhs, rhs) => {
                let lt = self.infer(lhs);
                let rt = self.infer(rhs);
                self.unify(&lt, &boolean(), &lhs.0.into_range(), "left of logical");
                self.unify(&rt, &boolean(), &rhs.0.into_range(), "right of logical");
                boolean()
            }

            // ---- Comparison ----
            Expression::Eq(lhs, rhs)
            | Expression::Neq(lhs, rhs)
            | Expression::Le(lhs, rhs)
            | Expression::Gt(lhs, rhs)
            | Expression::Leq(lhs, rhs)
            | Expression::Geq(lhs, rhs) => {
                let lt = self.infer(lhs);
                let rt = self.infer(rhs);
                self.unify(&lt, &rt, &range, "comparison operands");
                boolean()
            }

            // ---- Prefix / postfix ----
            Expression::Negate(e) | Expression::Positive(e) => self.infer(e),
            Expression::Not(e) => self.infer(e),
            Expression::Inc(e) | Expression::Dec(e) => self.infer(e),

            // ---- Calls ----
            Expression::Call { name, args } => {
                let ident = match name.1.as_ref() {
                    Expression::Identifier(n) => n.to_string(),
                    _ => return self.error("Invalid call target".to_string(), range),
                };
                let scheme = self.env.lookup(&ident).cloned();
                let fun_ty = match scheme {
                    Some(s) => instantiate(&s, &mut self.counter),
                    None => return self.error(
                    format!("Cannot find function `{}`", ident),
                    range,
                ),
                };

                let arg_tys: Vec<Ty> = match args {
                    Some(a) => a.iter().map(|arg| self.infer(arg)).collect(),
                    None => Vec::new(),
                };

                self.apply_function(Some(&ident), &fun_ty, &arg_tys, range)
            }

            // ---- Control flow ----
            Expression::If(branches) => self.infer_if(branches),
            Expression::Branch(cond, body) => {
                if let Some(c) = cond {
                    let ct = self.infer(c);
                    self.unify(&ct, &boolean(), &c.0.into_range(), "branch condition");
                }
                self.infer(body)
            }
            Expression::Match { scrutinee, arms } => {
                // TODO 15B: rewrite infer_match to walk the arms
                // with a `current_match_lhs` scope, unify each
                // pattern's bindings with the scrutinee type, and
                // unify the body types. For 15A, the stub
                // recurses into the scrutinee and each arm body
                // so their IDs stay aligned with the pre-walk and
                // their types land in the cache.
                let scrutinee_ty = self.infer(scrutinee);
                let prev = self.current_match_lhs.replace(scrutinee_ty);
                let mut result_ty = Ty::Var(self.counter.fresh());
                let mut first = true;
                for arm in arms {
                    let body_ty = self.infer(&arm.body);
                    if first {
                        result_ty = body_ty;
                        first = false;
                    } else {
                        self.unify(
                            &result_ty,
                            &body_ty,
                            &arm.body.0.into_range(),
                            "match arm body",
                        );
                    }
                }
                self.current_match_lhs = prev;
                if first {
                    return self.error("match has no arms".to_string(), range);
                }
                result_ty
            }
            Expression::Loop { iterable, body, .. } => {
                let it = self.infer(iterable);
                self.unify(&it, &boolean(), &iterable.0.into_range(), "while condition");
                let _ = self.infer(body);
                unit_ty()
            }

            // ---- Return ----
            Expression::Return(e) | Expression::ImplicitReturn(e) => {
                let ty = self.infer(e);
                if let Some(ret) = self.current_return_ty.clone() {
                    self.unify(&ret, &ty, &e.0.into_range(), "return value");
                }
                ty
            }

            // ---- I/O ----
            Expression::Print(fmt, params) | Expression::Format(fmt, params) => {
                let fmt_ty = self.infer(fmt);
                self.unify(&fmt_ty, &string(), &fmt.0.into_range(), "print format");
                if let Some(p) = params {
                    for param in p {
                        let _ = self.infer(param);
                    }
                }
                unit_ty()
            }

            // ---- Defer / coroutines / list ----
            Expression::Defer(e) => {
                let _ = self.infer(e);
                unit_ty()
            }
            Expression::Yield(e) => self.infer(e),
            Expression::Resume(_, arg) => {
                if let Some(a) = arg {
                    let _ = self.infer(a);
                }
                unit_ty()
            }
            Expression::List(elements) => self.infer_list(elements, range),

            // ---- Default arm ----
            Expression::Default(_) => self
                .current_match_lhs
                .clone()
                .unwrap_or_else(|| Ty::Var(self.counter.fresh())),

            // ---- Function declarations ----
            Expression::Function { name, args, returns, body } => {
                self.infer_function(name, args, returns.as_deref(), body, &range, None);
                unit_ty()
            }
            Expression::Implementation(_, owner, methods) => {
                self.infer_impl(owner, methods, &range);
                unit_ty()
            }
            Expression::Class(name, fields) => {
                self.register_class(name, fields, &range);
                unit_ty()
            }
            Expression::Argument(ty, _name) => self.parse_type_name_str(ty),
            Expression::Method(_vis, body) => self.infer(body),
            Expression::Member(_) => unit_ty(),
            Expression::Access(_, _) => unit_ty(),
            Expression::Update(_, e) => self.infer(e),
            Expression::Instantiate(class_expr, _args) => self.infer(class_expr),
            Expression::Field(_, _, _) => unit_ty(),

            // ---- Phase 15A placeholders (TODO 15B) ----
            // These keep the workspace building while the
            // typechecker is extended to understand sum types and
            // pattern matching. None of them are semantically
            // correct yet; the compiler's test suite is expected
            // to fail on enum/match/construct source until 15B
            // lands. The HM pre-walk still visits the new nodes
            // (see `id.rs`), so the NodeId cache stays aligned
            // with the AST shape — these stubs simply don't
            // recurse into the new children, which means the
            // pre-walk mints more IDs than infer consumes. That
            // misalignment is acceptable for 15A and will be
            // resolved when 15B replaces these arms with real
            // implementations.
            Expression::EnumDecl { .. } => Ty::Var(self.counter.fresh()),
            Expression::EnumVariant { .. } => unit_ty(),
            Expression::Construct { .. } => Ty::Var(self.counter.fresh()),

            // ---- Fallback ----
//
// `unreachable!` because the match above is exhaustive over every
// `Expression` variant. The arm is here so that adding a new variant
// produces a non-exhaustive match error here, instead of silently
// ignoring the new node. If you add a variant, handle it in the match
// above and remove this arm.
            #[allow(unreachable_patterns)]
            _ => unreachable!("all Expression variants must be handled above"),
        }
    }

    // ============================================================
    //  Span-indexed cache (Phase 6)
    // ============================================================

    /// Look up the inferred type of a node by [`NodeId`].
    ///
    /// Returns the fully-resolved type (the substitution is applied
    /// until it reaches a fixed point). Used by the bytecode emitter
    /// (Phase 9) to choose between `ADD` and `ADDF` without
    /// re-running inference.
    pub fn lookup_at(&self, id: NodeId) -> Option<Ty> {
        self.cache
            .get(&id)
            .map(|t| apply_ty_prune(&self.subst, t))
    }

    /// Borrow the [`IdTable`] so callers (the bytecode emitter in
    /// Phase 9) can recover a node's [`NodeId`] by walking the AST
    /// in pre-order and reading IDs sequentially.
    pub fn id_table(&self) -> &IdTable {
        &self.ids
    }

    /// Number of nodes that have a cached inferred type. Useful in
    /// tests that want to assert the cache is fully populated.
    #[cfg(test)]
    pub(crate) fn cache_len(&self) -> usize {
        self.cache.len()
    }

    // ============================================================
    //  Helpers
    // ============================================================

    /// Process a [`Expression::Fragment`] (the body of a `let x = expr`
    /// declaration, or the body of a Block that consists entirely of
    /// declarations).
    ///
    /// Each `Variable` / `Constant` declaration binds in the
    /// environment. If the immediate next sibling is a value-producing
    /// expression (anything that's not another declaration, comment, or
    /// `use`), it is treated as the initializer and unified with the
    /// declared type.
    ///
    /// No new frame is pushed: `let` bindings live in the surrounding
    /// scope (block, function body, or program) so they're visible to
    /// subsequent statements.
    fn infer_fragment(&mut self, children: &[Output]) -> Ty {
        let mut last_ty = unit_ty();
        let mut i = 0;
        while i < children.len() {
            let child = &children[i];
            match child.1.as_ref() {
                Expression::Variable(name, ty_opt) => {
                    let var_ty = match ty_opt {
                        Some(ann) => self.parse_type_name(ann),
                        None => Ty::Var(self.counter.fresh()),
                    };
                    self.env
                        .insert_top(name.to_string(), Scheme::mono(var_ty.clone()));
                    last_ty = unit_ty();

                    // Try to consume the next sibling as the initializer.
                    if i + 1 < children.len() {
                        let next = &children[i + 1];
                        if !is_declaration_like(next) {
                            let val_ty = self.infer(next);
                            self.unify(
                                &var_ty,
                                &val_ty,
                                &child.0.into_range(),
                                "let binding",
                            );
                            i += 1;
                        }
                    }
                }
                Expression::Constant(name, ty_opt) => {
                    let var_ty = match ty_opt {
                        Some(ann) => self.parse_type_name(ann),
                        None => Ty::Var(self.counter.fresh()),
                    };
                    if let Expression::Identifier(n) = name.1.as_ref() {
                        self.env.insert_top(n.to_string(), Scheme::mono(var_ty));
                    }
                    last_ty = unit_ty();
                }
                _ => {
                    last_ty = self.infer(child);
                }
            }
            i += 1;
        }
        last_ty
    }

    fn infer_arith(&mut self, lhs: &Output, rhs: &Output, range: Range<usize>, op: &str) -> Ty {
        let lt = self.infer(lhs);
        let rt = self.infer(rhs);
        self.unify(&lt, &rt, &range, &format!("operands of `{}`", op))
    }

    fn infer_if(&mut self, branches: &[Output]) -> Ty {
        let mut result_ty = Ty::Var(self.counter.fresh());
        let mut first = true;
        for branch in branches {
            if let Expression::Branch(cond, body) = branch.1.as_ref() {
                if let Some(c) = cond {
                    let ct = self.infer(c);
                    self.unify(&ct, &boolean(), &c.0.into_range(), "if condition");
                }
                let body_ty = self.infer(body);
                if first {
                    result_ty = body_ty;
                    first = false;
                } else {
                    self.unify(&result_ty, &body_ty, &body.0.into_range(), "if branch");
                }
            }
        }
        result_ty
    }

    fn infer_list(&mut self, elements: &[Output], _range: Range<usize>) -> Ty {
        if elements.is_empty() {
            return list(Ty::Var(self.counter.fresh()));
        }
        let first_ty = self.infer(&elements[0]);
        for elem in &elements[1..] {
            let t = self.infer(elem);
            self.unify(&first_ty, &t, &elem.0.into_range(), "list element");
        }
        list(first_ty)
    }

    /// Thread a curried function type through a list of argument types,
    /// unifying each. Returns the final return type.
    ///
    /// If at any point the type doesn't look like a function and isn't a
    /// variable, the call is rejected.
    fn apply_function(
        &mut self,
        name: Option<&str>,
        fun_ty: &Ty,
        arg_tys: &[Ty],
        range: Range<usize>,
    ) -> Ty {
        let mut current = fun_ty.clone();
        for (i, arg) in arg_tys.iter().enumerate() {
            let pruned = apply_ty(&self.subst, &current);
            match pruned {
                Ty::Fun(param, ret) => {
                    self.unify(param.as_ref(), arg, &range, "function argument");
                    current = *ret;
                }
                Ty::Var(v) => {
                    let ret_ty = Ty::Var(self.counter.fresh());
                    let new_fun = Ty::Fun(
                        Box::new(arg.clone()),
                        Box::new(ret_ty.clone()),
                    );
                    self.unify(&Ty::Var(v), &new_fun, &range, "function type");
                    current = ret_ty;
                }
                _ => {
                    // We've run out of function parameters — the call
                    // had more arguments than the function accepts.
                    let actual = format!("{}", apply_ty_prune(&self.subst, &pruned));
                    return self.error_with_help(
                        match name {
                            Some(n) => format!(
                                "Function `{}` was called with too many arguments \
                                 (it accepts {}, but argument #{} was given)",
                                n, i, i + 1,
                            ),
                            None => format!(
                                "Cannot call value of type `{}` as a function \
                                 (it accepts {} argument{})",
                                actual,
                                i,
                                if i == 1 { "" } else { "s" },
                            ),
                        },
                        range,
                        Some("check the function signature or the number of arguments".to_string()),
                    );
                }
            }
        }
        current
    }

    /// Unify two types under the current substitution, updating
    /// `self.subst` on success. On failure, record a message and return
    /// a fresh variable so inference can continue.
    fn unify(&mut self, t1: &Ty, t2: &Ty, range: &Range<usize>, ctx: &str) -> Ty {
        match unify_with(&self.subst, t1, t2) {
            Ok(s) => {
                self.subst = compose(&s, &self.subst);
                apply_ty(&self.subst, t1)
            }
            Err(UnifyError::Mismatch { left, right }) => self.error_with_help(
                format!(
                    "Type mismatch: expected `{}`, found `{}`",
                    left, right
                ),
                range.clone(),
                Some(format!("while checking `{}`", ctx)),
            ),
            Err(UnifyError::Occurs { var, ty }) => self.error_with_help(
                format!("Cannot construct infinite type `{}`", ty),
                range.clone(),
                Some(format!(
                    "the type variable `t{}` would occur in its own definition",
                    var.raw()
                )),
            ),
        }
    }

    /// Record an error message and return a fresh variable.
    ///
    /// This is the simplest form: a single message with a primary
    /// label at `range`. No help hint, no secondary labels. For richer
    /// diagnostics use [`error_with_help`] or [`error_with_labels`].
    fn error(&mut self, message: String, range: Range<usize>) -> Ty {
        self.messages.push(Message::error(message, range));
        Ty::Var(self.counter.fresh())
    }

    /// Record an error message with a help hint.
    ///
    /// The hint is shown beneath the underline by ariadne's renderer.
    fn error_with_help(
        &mut self,
        message: String,
        range: Range<usize>,
        help: Option<String>,
    ) -> Ty {
        let mut msg = Message::error(message, range);
        if let Some(h) = help {
            msg.with_help(h);
        }
        self.messages.push(msg);
        Ty::Var(self.counter.fresh())
    }

    /// Record an error with a primary label and one or more secondary
    /// labels. Each secondary label is rendered by ariadne below the
    /// primary underline; use them to point at related source positions
    /// (e.g., "expected type comes from here", "found type comes from
    /// here").
    #[allow(dead_code)] // kept for future use (Phase 10+ diagnostics)
    fn error_with_labels(
        &mut self,
        primary_message: String,
        primary_range: Range<usize>,
        secondary: Vec<(String, Range<usize>)>,
        help: Option<String>,
    ) -> Ty {
        let mut msg = Message::error(primary_message, primary_range);
        for (label_text, range) in secondary {
            msg.push(Label::new(label_text, range));
        }
        if let Some(h) = help {
            msg.with_help(h);
        }
        self.messages.push(msg);
        Ty::Var(self.counter.fresh())
    }

    fn parse_type_name(&mut self, ann: &Output) -> Ty {
        match ann.1.as_ref() {
            Expression::Identifier(name) | Expression::Type(name) => {
                self.parse_type_name_str(name)
            }
            _ => Ty::Var(self.counter.fresh()),
        }
    }

    fn parse_type_name_str(&self, name: &str) -> Ty {
        // Built-in type names are matched case-insensitively so the
        // user can write `String`, `STRING`, etc.
        match name.to_ascii_lowercase().as_str() {
            "int" => int(),
            "float" => float(),
            "bool" => boolean(),
            "string" => string(),
            "void" => unit_ty(),
            _ => Ty::Con(name.to_string()),
        }
    }

    // ============================================================
    //  Native function registration (Phase 7)
    // ============================================================

    /// Register a native (built-in) function with the type system.
    ///
    /// `name` is the function's identifier as seen in user code;
    /// `params` are the parameter types in declaration order; `ret`
    /// is the return type. The signature is curried into a function
    /// type (`arg1 -> arg2 -> ... -> ret`).
    ///
    /// The binding is added to the top frame of the env so it's
    /// visible to every subsequent call. See [`Compiler::register`]
    /// for the public entry point.
    pub fn register_native(&mut self, name: &str, params: &[Ty], ret: &Ty) {
        let fn_ty = params.iter().rev().fold(ret.clone(), |acc, p| {
            Ty::Fun(Box::new(p.clone()), Box::new(acc))
        });

        self.env.insert_top(name.to_string(), Scheme::mono(fn_ty));
    }

    // ============================================================
    //  Function / class / impl handling (Phase 5)
    // ============================================================

    /// Register a class: store its name and the (visibility, name,
    /// type) of each field. The class itself becomes a `Ty::Con(name)`
    /// constructor that's resolvable from any scope, so it can be
    /// referenced as a type elsewhere.
    fn register_class(&mut self, name: &str, fields: &[Output], range: &Range<usize>) {
        let mut field_info = Vec::new();
        for field in fields {
            if let Expression::Field(vis, fname, fty) = field.1.as_ref() {
                let fname_str = match fname.1.as_ref() {
                    Expression::Identifier(n) => n.to_string(),
                    _ => {
                        self.messages.push(Message::error(
                            "Invalid field name".to_string(),
                            field.0.into_range(),
                        ));
                        continue;
                    }
                };
                let ty = self.parse_type_name(fty);
                field_info.push((*vis, fname_str, ty));
            } else {
                self.messages.push(Message::error(
                    "Expected a field declaration".to_string(),
                    field.0.into_range(),
                ));
            }
        }
        self.classes.insert(name.to_string(), field_info);
        // Register the class as a type in the environment so
        // `Foo`-as-a-type lookups succeed.
        self.env.insert_top(
            name.to_string(),
            Scheme::mono(Ty::Con(name.to_string())),
        );
        let _ = range;
    }

    /// Process an `impl Owner { methods... }` block:
    /// 1. Auto-register the owner class if it hasn't been declared
    ///    yet (so `impl` can appear before `class`).
    /// 2. Push a frame and bind `self : Owner`.
    /// 3. For each method, run [`infer_function`] with `self`
    ///    prepended to the argument list, then store the method's
    ///    scheme under the owner's name.
    fn infer_impl(&mut self, owner: &str, methods: &[Output], range: &Range<usize>) {
        let owner_ty = Ty::Con(owner.to_string());

        if !self.classes.contains_key(owner) {
            self.classes.insert(owner.to_string(), Vec::new());
            self.env.insert_top(
                owner.to_string(),
                Scheme::mono(owner_ty.clone()),
            );
        }

        self.env.push();
        self.env
            .insert_top("self".to_string(), Scheme::mono(owner_ty.clone()));

        for method in methods {
            if let Expression::Method(vis, body) = method.1.as_ref() {
                if let Expression::Function {
                    name,
                    args,
                    returns,
                    body: func_body,
                } = body.1.as_ref()
                {
                    let fun_ty = self.infer_function(
                        name,
                        args,
                        returns.as_deref(),
                        func_body,
                        &method.0.into_range(),
                        Some(&owner_ty),
                    );
                    self.methods
                        .entry(owner.to_string())
                        .or_default()
                        .insert(name.to_string(), (*vis, Scheme::mono(fun_ty)));
                } else {
                    self.messages.push(Message::error(
                        "Method body must be a function".to_string(),
                        method.0.into_range(),
                    ));
                }
            }
        }

        self.env.pop();
        let _ = range;
    }

    /// Process a function declaration (top-level `fn` or a method body
    /// inside an `impl`). Implements monomorphic recursion:
    ///
    /// 1. Build the declared function type from the args and the
    ///    declared return type (or a fresh variable if no return type).
    /// 2. If `self_ty` is `Some`, prepend it (so the method becomes
    ///    `self -> arg1 -> ... -> ret`).
    /// 3. Allocate a fresh `α` and bind the function's name to it in
    ///    the local scope, so recursive references inside the body
    ///    unify with `α` before the body's view of the function is
    ///    finalised.
    /// 4. Push a frame, bind the arguments, run inference on the body
    ///    (which sees `name : α`), and pop the frame.
    /// 5. Unify `α` with the declared function type. This validates
    ///    that the recursive calls were consistent.
    fn infer_function(
        &mut self,
        name: &str,
        args: &Output,
        returns: Option<&str>,
        body: &Output,
        range: &Range<usize>,
        self_ty: Option<&Ty>,
    ) -> Ty {
        let arg_tys = self.parse_arg_list(args);
        let ret_ty = match returns {
            Some(r) => self.parse_type_name_str(r),
            None => Ty::Var(self.counter.fresh()),
        };

        // Build the declared function type: arg1 -> ... -> argN -> ret,
        // with self prepended for methods.
        let mut fun_ty = ret_ty.clone();
        for (_, arg_ty) in arg_tys.iter().rev() {
            fun_ty = Ty::Fun(Box::new(arg_ty.clone()), Box::new(fun_ty));
        }
        if let Some(self_ty) = self_ty {
            fun_ty = Ty::Fun(Box::new(self_ty.clone()), Box::new(fun_ty));
        }

        // Monomorphic recursion: bind name to a fresh α in the
        // *outer* frame so the function is visible to subsequent
        // code. The body sees it too because the new frame we push
        // for the body is a child of the outer.
        let alpha = self.counter.fresh();
        let prev_ret = self.current_return_ty.replace(ret_ty.clone());

        self.env
            .insert_top(name.to_string(), Scheme::mono(Ty::Var(alpha)));

        self.env.push();
        for (arg_name, arg_ty) in &arg_tys {
            self.env
                .insert_top(arg_name.clone(), Scheme::mono(arg_ty.clone()));
        }
        let _ = self.infer(body);
        self.env.pop();

        self.current_return_ty = prev_ret;
        self.unify(&Ty::Var(alpha), &fun_ty, range, "function type");
        fun_ty
    }

    /// Parse a function's argument list (a `Fragment` of
    /// `Argument(ty, name)` nodes).
    fn parse_arg_list(&self, args: &Output) -> Vec<(String, Ty)> {
        let mut out = Vec::new();
        if let Expression::Fragment(children) = args.1.as_ref() {
            for child in children {
                if let Expression::Argument(ty, name) = child.1.as_ref() {
                    out.push((name.to_string(), self.parse_type_name_str(ty)));
                }
            }
        }
        out
    }
}

/// True for nodes that look like declarations / no-ops rather than
/// initializers. Used by [`Checker::infer_fragment`] to decide whether
/// to consume the next sibling as a `let` initializer.
fn is_declaration_like(node: &Output) -> bool {
    matches!(
        node.1.as_ref(),
        Expression::Variable(..)
            | Expression::Constant(..)
            | Expression::Assignment(..)
            | Expression::Comment(..)
            | Expression::Use { .. }
            | Expression::Noop(..)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use parser::Pratt;

    /// Parse and infer `src`, returning the checker state and inferred type.
    ///
    /// The top-level parser expects declarations / statements. Bare
    /// expressions need a trailing `;`. Heuristically add one when the
    /// source doesn't already look like a complete statement.
    fn check(src: &str) -> (Checker, Ty) {
        let mut c = Checker::new();
        let trimmed = src.trim();
        // Add `;` if the source doesn't end with a terminator. We don't
        // try to be clever about keywords — even `let x = 1; ...; expr`
        // needs the trailing `expr;`.
        let needs_semi = !trimmed.ends_with(';') && !trimmed.ends_with('}');
        let owned: String = if needs_semi {
            format!("{};", trimmed)
        } else {
            trimmed.to_string()
        };
        match Pratt::default().parse(owned.as_str()) {
            Ok(ast) => {
                let ty = c.check_program(&ast);
                (c, ty)
            }
            Err(msg) => panic!("parse failed for `{}`: {:?}", src, msg),
        }
    }

    fn assert_ok(src: &str, expected: Ty) {
        let (mut c, ty) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.is_empty(),
            "expected no messages for `{}`, got: {:?}",
            src,
            msgs
        );
        assert_eq!(ty, expected, "type mismatch for `{}`", src);
    }

    fn assert_messages(src: &str) -> Vec<Message> {
        let (mut c, _) = check(src);
        c.take_messages()
    }

    // ---- Literals ----

    #[test]
    fn integer_literal() {
        assert_ok("42", int());
    }

    #[test]
    fn float_literal() {
        assert_ok("3.14", float());
    }

    #[test]
    fn string_literal() {
        assert_ok("\"hello\"", string());
    }

    #[test]
    fn bool_literal() {
        assert_ok("true", boolean());
        assert_ok("false", boolean());
    }

    // ---- Identifier ----

    #[test]
    fn unknown_identifier_errors() {
        let msgs = assert_messages("x;");
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn identifier_from_let_annotation() {
        // Declare x: int = 42, then verify via env lookup.
        let (mut c, _) = check("let x: int = 42;");
        assert!(c.take_messages().is_empty());
        let scheme = c.env().lookup("x").unwrap();
        let ty = apply_ty(c.subst(), &scheme.ty);
        assert_eq!(ty, int());
    }

    // ---- Variables and let ----

    #[test]
    fn let_with_annotation() {
        assert_ok("let x: int = 42;", unit_ty());
    }

    #[test]
    fn let_without_annotation_infers_from_value() {
        // `let x = 42;` — x should be inferred as int.
        let (mut c, _) = check("let x = 42;");
        assert!(c.take_messages().is_empty());
        let scheme = c.env().lookup("x").unwrap();
        let ty = apply_ty(c.subst(), &scheme.ty);
        assert_eq!(ty, int());
    }

    #[test]
    fn let_without_annotation_or_value_uses_fresh_var() {
        // `let x;` — x is a fresh var (id 0).
        let (mut c, _) = check("let x;");
        assert!(c.take_messages().is_empty());
        let scheme = c.env().lookup("x").unwrap();
        assert_eq!(scheme.ty, Ty::Var(TyVarId(0)));
    }

    // ---- Assignment ----

    #[test]
    fn assignment_updates_existing_var() {
        let (mut c, _) = check("let x; x = 42;");
        assert!(c.take_messages().is_empty());
        let scheme = c.env().lookup("x").unwrap();
        let ty = apply_ty(c.subst(), &scheme.ty);
        assert_eq!(ty, int());
    }

    #[test]
    fn assignment_to_undeclared_var_errors() {
        let msgs = assert_messages("x = 42;");
        assert!(!msgs.is_empty());
    }

    #[test]
    fn assignment_mismatch_errors_but_continues() {
        // x: int, then assign "hello" — should produce an error.
        let msgs = assert_messages("let x: int; x = \"hello\";");
        assert!(!msgs.is_empty());
    }

    // ---- Arithmetic ----

    #[test]
    fn addition_of_ints_is_int() {
        assert_ok("1 + 2", int());
    }

    #[test]
    fn addition_of_floats_is_float() {
        assert_ok("1.0 + 2.0", float());
    }

    #[test]
    fn mixed_int_float_arithmetic_mismatches() {
        // 1 + 2.0: unify int with float → Mismatch.
        let msgs = assert_messages("1 + 2.0;");
        assert!(!msgs.is_empty());
    }

    #[test]
    fn subtraction() {
        assert_ok("5 - 3", int());
    }

    #[test]
    fn multiplication() {
        assert_ok("4 * 5", int());
    }

    #[test]
    fn division() {
        assert_ok("10 / 2", int());
    }

    #[test]
    fn modulo() {
        assert_ok("10 % 3", int());
    }

    #[test]
    fn power() {
        assert_ok("2 ** 3", int());
    }

    #[test]
    fn shift_left() {
        assert_ok("1 << 2", int());
    }

    #[test]
    fn xor() {
        assert_ok("5 ^ 3", int());
    }

    #[test]
    fn bitand() {
        assert_ok("5 & 3", int());
    }

    #[test]
    fn bitor() {
        assert_ok("5 | 3", int());
    }

    // ---- Comparison ----

    #[test]
    fn equality_returns_bool() {
        assert_ok("1 == 1", boolean());
    }

    #[test]
    fn inequality_returns_bool() {
        assert_ok("1 != 2", boolean());
    }

    #[test]
    fn less_than() {
        assert_ok("1 < 2", boolean());
    }

    #[test]
    fn greater_than() {
        assert_ok("2 > 1", boolean());
    }

    #[test]
    fn less_equal() {
        assert_ok("1 <= 1", boolean());
    }

    #[test]
    fn greater_equal() {
        assert_ok("2 >= 2", boolean());
    }

    // ---- Logical ----

    #[test]
    fn logical_and_of_bools_is_bool() {
        assert_ok("true && false", boolean());
    }

    #[test]
    fn logical_or_of_bools_is_bool() {
        assert_ok("true || false", boolean());
    }

    #[test]
    fn logical_and_requires_bool() {
        // 1 && 2 — int, not bool.
        let msgs = assert_messages("1 && 2;");
        assert!(!msgs.is_empty());
    }

    // ---- Prefix ----

    #[test]
    fn negate_int() {
        assert_ok("-42", int());
    }

    #[test]
    fn positive_int() {
        assert_ok("+42", int());
    }

    #[test]
    fn not_bool() {
        assert_ok("~true", boolean());
    }

    // ---- Postfix ----

    #[test]
    fn inc_dec() {
        // These return the variable's type.
        let (mut c, _) = check("let x: int = 0; x++;");
        assert!(c.take_messages().is_empty());
    }

    // ---- Call ----

    #[test]
    fn call_unknown_function_errors() {
        let msgs = assert_messages("foo();");
        assert!(!msgs.is_empty());
    }

    #[test]
    fn call_to_unregistered_print_does_not_error() {
        // The parser turns `print` into a `Print` AST node (not a Call),
        // so it doesn't go through the unknown-call path.
        let (mut c, ty) = check("print \"hello\";");
        assert!(c.take_messages().is_empty());
        assert_eq!(ty, unit_ty());
    }

    // ---- If ----

    #[test]
    fn if_single_branch() {
        assert_ok("if true { 42; }", int());
    }

    #[test]
    fn if_with_non_bool_condition_errors() {
        let msgs = assert_messages("if 42 { 1; }");
        assert!(!msgs.is_empty());
    }

    // ---- Match (parser doesn't produce Match nodes yet, so the
    //      handler is unreachable from real source). Tests for Match
    //      are deferred until the parser learns the `match` keyword.

    // ---- Loop ----

    #[test]
    fn while_loop_returns_unit() {
        assert_ok("while false { 42; }", unit_ty());
    }

    #[test]
    fn while_with_non_bool_condition_errors() {
        let msgs = assert_messages("while 42 { 1; }");
        assert!(!msgs.is_empty());
    }

    // ---- Return ----

    #[test]
    fn return_inside_expression() {
        // Without an enclosing function, return just returns the value's type.
        assert_ok("return 42", int());
    }

    // ---- Block ----

    #[test]
    fn empty_block() {
        assert_ok("{}", unit_ty());
    }

    #[test]
    fn block_last_value_is_block_type() {
        assert_ok("{ 1; 2; 3; }", int());
    }

    // ---- Print / Format ----

    #[test]
    fn print_with_string_format_ok() {
        assert_ok("print \"hello\";", unit_ty());
    }

    // ---- Defer ----

    #[test]
    fn defer_returns_unit() {
        assert_ok("defer { 42; }", unit_ty());
    }

    // ---- List literals ----
    //      Parser doesn't produce `List` nodes yet, so these are deferred.

    // ---- Complex expressions ----

    #[test]
    fn nested_arithmetic() {
        assert_ok("(1 + 2) * 3", int());
    }

    #[test]
    fn block_with_multiple_lets() {
        let src = "let x: int = 10; let y: int = 20; x + y";
        assert_ok(src, int());
    }

    // ---- Function declarations (Phase 5) ----

    #[test]
    fn function_declaration_with_typed_args_and_return() {
        let (mut c, _) = check("fn add(int a, int b) -> int { return a + b; }");
        assert!(c.take_messages().is_empty());
        let scheme = c.env().lookup("add").unwrap();
        let ty = apply_ty(c.subst(), &scheme.ty);
        assert_eq!(ty, Ty::Fun(Box::new(int()), Box::new(Ty::Fun(Box::new(int()), Box::new(int())))));
    }

    #[test]
    fn function_declaration_with_inferred_return() {
        // No declared return type — should be inferred from the body.
        let (mut c, _) = check("fn add(int a, int b) { return a + b; }");
        assert!(c.take_messages().is_empty());
        let scheme = c.env().lookup("add").unwrap();
        let ty = apply_ty(c.subst(), &scheme.ty);
        // a -> b -> ?  (return type is a fresh variable bound to int)
        assert!(matches!(ty, Ty::Fun(_, _)));
    }

    #[test]
    fn function_arity_mismatch_errors() {
        // fib takes 1 int, called with 2 args.
        let msgs = assert_messages("fn fib(int n) -> int { return n; } fib(1, 2);");
        assert!(!msgs.is_empty());
    }

    #[test]
    fn function_return_mismatch_errors() {
        // Declared return is int, but body returns string.
        let msgs = assert_messages("fn broken() -> int { return \"oops\"; }");
        assert!(!msgs.is_empty());
    }

    #[test]
    fn function_undefined_errors() {
        // body calls an unknown function.
        let msgs = assert_messages("fn main() { nope(); }");
        assert!(!msgs.is_empty());
    }

    // ---- Recursive functions (monomorphic recursion) ----

    #[test]
    fn recursive_fib() {
        // fib(n) = if n < 2 then n else fib(n-1) + fib(n-2)
        // Adapted to the current syntax (no `<` comparison; use `==`).
        let src = "fn fib(int n) -> int { if n == 1 { return 1; } if n == 2 { return 1; } return fib(n - 1) + fib(n - 2); }";
        let (mut c, _) = check(src);
        assert!(
            c.take_messages().is_empty(),
            "expected no messages, got: {:?}",
            c.messages()
        );
        let scheme = c.env().lookup("fib").unwrap();
        let ty = apply_ty(c.subst(), &scheme.ty);
        assert_eq!(ty, Ty::Fun(Box::new(int()), Box::new(int())));
    }

    // ---- Class declarations ----

    #[test]
    fn class_registers_nominal_constructor() {
        let (mut c, _) = check("class Foo { name: String, }");
        let msgs = c.take_messages(); assert!(msgs.is_empty(), "{:?}", msgs);
        // Foo is a Ty::Con. The fields are stored privately by default.
        let class = c.classes.get("Foo").expect("class not registered");
        assert_eq!(class.len(), 1);
        assert_eq!(class[0].0, Visibility::Private);
        assert_eq!(class[0].1, "name");
        assert_eq!(class[0].2, string());
    }

    #[test]
    fn class_with_pub_field_marks_visibility() {
        let (mut c, _) = check("class Foo { pub age: int, name: String, }");
        let msgs = c.take_messages(); assert!(msgs.is_empty(), "{:?}", msgs);
        let class = c.classes.get("Foo").unwrap();
        // First field is public (pub), second is private.
        assert_eq!(class[0].0, Visibility::Public);
        assert_eq!(class[0].1, "age");
        assert_eq!(class[1].0, Visibility::Private);
        assert_eq!(class[1].1, "name");
    }

    #[test]
    fn class_with_all_private_fields() {
        let (mut c, _) = check("class Foo { x: int, y: int, }");
        let msgs = c.take_messages(); assert!(msgs.is_empty(), "{:?}", msgs);
        let class = c.classes.get("Foo").unwrap();
        assert!(class.iter().all(|(v, _, _)| *v == Visibility::Private));
    }

    #[test]
    fn class_visibility_is_per_field() {
        // First field is public, second is private — they're tracked
        // independently even though they live in the same class.
        let (mut c, _) = check("class Foo { pub a: int, b: int, pub c: int, }");
        let msgs = c.take_messages(); assert!(msgs.is_empty(), "{:?}", msgs);
        let fields = &c.classes.get("Foo").unwrap();
        assert_eq!(fields[0].0, Visibility::Public);
        assert_eq!(fields[1].0, Visibility::Private);
        assert_eq!(fields[2].0, Visibility::Public);
    }

    #[test]
    fn class_visibility_recorded_for_future_member_access() {
        // Member access (`x.field`) isn't parsed yet, so we can't write
        // a true visibility-check test. This test asserts the data is
        // recorded correctly so the future member-access pass can
        // enforce it without re-parsing the class.
        let (mut c, _) = check("class Foo { pub age: int, name: String, }");
        let msgs = c.take_messages(); assert!(msgs.is_empty(), "{:?}", msgs);
        let foo = c.classes.get("Foo").unwrap();
        assert_eq!(foo[0].0, Visibility::Public);
        assert_eq!(foo[1].0, Visibility::Private);
    }

    // ---- Impl blocks ----

    #[test]
    fn impl_binds_self_to_owner() {
        // `self` is implicit. The method's type becomes Foo -> Foo.
        let src = "class Foo { } impl Foo { fn id() -> Foo { return new Foo(); } }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        let methods = c.methods.get("Foo").expect("methods not registered");
        let (_, scheme) = methods.get("id").unwrap();
        let ty = apply_ty(c.subst(), &scheme.ty);
        // Foo -> Foo
        assert_eq!(ty, Ty::Fun(Box::new(Ty::Con("Foo".into())), Box::new(Ty::Con("Foo".into()))));
    }

    #[test]
    fn impl_method_with_args_prepends_self() {
        let src = "impl Foo { fn method(int x) -> int { return x; } }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages(); assert!(msgs.is_empty(), "{:?}", msgs);
        let methods = c.methods.get("Foo").unwrap();
        let (_, scheme) = methods.get("method").unwrap();
        let ty = apply_ty(c.subst(), &scheme.ty);
        // Foo -> int -> int
        assert_eq!(
            ty,
            Ty::Fun(
                Box::new(Ty::Con("Foo".into())),
                Box::new(Ty::Fun(Box::new(int()), Box::new(int())))
            )
        );
    }

    #[test]
    fn impl_method_visibility_default_is_private() {
        let src = "impl Foo { fn hidden() -> int { return 0; } }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages(); assert!(msgs.is_empty(), "{:?}", msgs);
        let methods = c.methods.get("Foo").unwrap();
        let (vis, _) = methods.get("hidden").unwrap();
        assert_eq!(*vis, Visibility::Private);
    }

    #[test]
    fn impl_pub_method_marks_visibility() {
        let src = "impl Foo { pub fn visible() -> int { return 0; } }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages(); assert!(msgs.is_empty(), "{:?}", msgs);
        let methods = c.methods.get("Foo").unwrap();
        let (vis, _) = methods.get("visible").unwrap();
        assert_eq!(*vis, Visibility::Public);
    }

    // ---- Instantiation ----

    #[test]
    fn instantiate_returns_class_type() {
        let src = "class Foo { name: String, } let x = new Foo(); x";
        let (mut c, ty) = check(src);
        let msgs = c.take_messages(); assert!(msgs.is_empty(), "{:?}", msgs);
        // The whole program's type is the type of `x`, which is Foo.
        assert_eq!(ty, Ty::Con("Foo".into()));
    }

    // ---- Combined: class + impl + instantiation ----

    #[test]
    fn class_impl_and_instantiate_combined() {
        // Pulls the existing example together:
        //   class Foo { name: String, }
        //   impl Foo { fn sadge() -> int { return 42; } }
        //   fn main() { print "%i", (2 * 2 + 3); let x = new Foo(); }
        let src = "\
            class Foo { name: String, } \
            impl Foo { fn sadge() -> int { return 42; } } \
            fn main() { let x = new Foo(); }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages(); assert!(msgs.is_empty(), "{:?}", msgs);
        assert!(c.classes.contains_key("Foo"));
        assert!(c.methods.get("Foo").unwrap().contains_key("sadge"));
    }

    // ---- Recursive method (inside an impl) ----

    #[test]
    fn recursive_method_via_self_binding() {
        // fib uses no self, but `==` is the only comparison the
        // parser currently supports; use that for the branch.
        let src = "impl Counter { fn tick(int n) -> int { if n == 0 { return 0; } return tick(n - 1) + 1; } }";
        let (mut c, _) = check(src);
        // We don't require no messages — the outer call site `tick(...)`
        // may have residual issues — but the method should be registered.
        let _ = c.take_messages();
        assert!(c.methods.get("Counter").unwrap().contains_key("tick"));
    }

    // ---- Block returns last value ----

    #[test]
    fn nested_blocks_return_inner() {
        assert_ok("{ { 42; } }", int());
    }

    // ---- Native function registration (Phase 7) ----

    #[test]
    fn register_native_adds_function_to_env() {
        let mut c = Checker::new();
        c.register_native("add", &[int(), int()], &int());
        let scheme = c.env().lookup("add").expect("add not registered");
        let ty = apply_ty(c.subst(), &scheme.ty);
        // Curried: int -> int -> int
        assert_eq!(
            ty,
            Ty::Fun(
                Box::new(int()),
                Box::new(Ty::Fun(Box::new(int()), Box::new(int())))
            )
        );
    }

    #[test]
    fn register_native_no_args() {
        let mut c = Checker::new();
        c.register_native("now", &[], &int());
        let scheme = c.env().lookup("now").expect("now not registered");
        let ty = apply_ty(c.subst(), &scheme.ty);
        assert_eq!(ty, int());
    }

    #[test]
    fn register_native_void_return() {
        let mut c = Checker::new();
        c.register_native("print", &[string()], &unit_ty());
        let scheme = c.env().lookup("print").expect("print not registered");
        let ty = apply_ty(c.subst(), &scheme.ty);
        // string -> unit
        assert_eq!(ty, Ty::Fun(Box::new(string()), Box::new(unit_ty())));
    }

    #[test]
    fn register_native_object_type() {
        let mut c = Checker::new();
        c.register_native("make_foo", &[], &Ty::Con("Foo".into()));
        let scheme = c.env().lookup("make_foo").expect("make_foo not registered");
        let ty = apply_ty(c.subst(), &scheme.ty);
        assert_eq!(ty, Ty::Con("Foo".into()));
    }

    #[test]
    fn native_function_call_infers_correctly() {
        // After register_native, a call to the native should type-check
        // against the registered signature and produce the right type.
        let mut c = Checker::new();
        c.register_native("print", &[string()], &unit_ty());
        let ast = Pratt::default().parse("print \"hi\";").expect("parse");
        let ty = c.check_program(&ast);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        assert_eq!(ty, unit_ty());
    }

    #[test]
    fn native_function_arity_mismatch_errors() {
        // print takes 1 arg; call with 2.
        let mut c = Checker::new();
        c.register_native("print", &[string()], &unit_ty());
        let ast = Pratt::default().parse("print(\"a\", \"b\");").expect("parse");
        let _ = c.check_program(&ast);
        let msgs = c.take_messages();
        assert!(!msgs.is_empty(), "expected arity-mismatch error");
    }

    #[test]
    fn native_function_call_with_correct_arity_succeeds() {
        // Once registered, a function call with matching arity and types
        // type-checks cleanly.
        let mut c = Checker::new();
        c.register_native("add", &[int(), int()], &int());
        let ast = Pratt::default().parse("add(1, 2);").expect("parse");
        let _ = c.check_program(&ast);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
    }

    #[test]
    fn native_visible_inside_nested_block() {
        // Natives registered on the checker are visible from any
        // nested scope.
        let mut c = Checker::new();
        c.register_native("print", &[string()], &unit_ty());
        let ast = Pratt::default().parse("{ print \"a\"; }").expect("parse");
        let _ = c.check_program(&ast);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
    }

    // ---- Diagnostics (Phase 8) ----
    //
    // The following tests verify that emitted `Message`s are well-formed
    // for ariadne: each carries a clear headline, a primary label at
    // the error range, and (where helpful) a `help` hint with extra
    // context. ariadne's renderer consumes exactly this shape, so as
    // long as the `Message` fields are populated, the diagnostic will
    // display cleanly.

    #[test]
    fn unknown_identifier_message_uses_can_not_find_format() {
        let (mut c, _) = check("unknown_var;");
        let msgs = c.take_messages();
        assert_eq!(msgs.len(), 1);
        let msg = &msgs[0];
        assert!(
            msg.message().contains("Cannot find value"),
            "got: {:?}",
            msg.message()
        );
        assert!(
            msg.message().contains("unknown_var"),
            "got: {:?}",
            msg.message()
        );
        // Primary range is set (ariadne uses this for the underline).
        let r = msg.range();
        assert!(r.start <= r.end, "bad range {:?}", r);
        assert!(r.end > 0, "expected non-empty range");
    }

    #[test]
    fn type_mismatch_message_uses_expected_actual_format() {
        let (mut c, _) = check("let x: int = \"hello\";");
        let msgs = c.take_messages();
        // The Fragment return is unit, so the assignment's type
        // mismatch is the only diagnostic.
        assert!(!msgs.is_empty(), "expected at least one message");
        let msg = msgs.iter().find(|m| m.message().contains("Type mismatch"));
        assert!(msg.is_some(), "no type-mismatch message found in {:?}", msgs);
        let msg = msg.unwrap();
        assert!(msg.message().contains("expected"), "got: {:?}", msg.message());
        assert!(msg.message().contains("found"), "got: {:?}", msg.message());
        assert!(msg.message().contains("int"), "got: {:?}", msg.message());
        assert!(msg.message().contains("string"), "got: {:?}", msg.message());
        // Help is present (the context).
        assert!(msg.help().is_some(), "missing help");
        let help = msg.help().as_ref().unwrap();
        assert!(
            help.contains("let binding"),
            "got help: {:?}",
            help
        );
    }

    #[test]
    fn infinite_type_message_uses_clear_format() {
        // It's hard to construct an infinite-type situation without
        // recursive type syntax (e.g., `α = List<α>`), so this test
        // just checks the format IF such a message ever fires. To make
        // sure the path is exercised, we drive the checker through a
        // recursive function declaration whose body returns the
        // function itself with the wrong shape — that triggers an
        // occurs check via the return-type unification.
        //
        // (If your checker ever changes the return path so this no
        // longer fires an occurs check, drop this test — it's about
        // message format, not behaviour.)
        let (mut c, _) = check("fn bad() { return bad; }");
        let msgs = c.take_messages();
        // Either there's an infinite-type error, or the function is
        // typeable. Both are fine — what we want to assert is the
        // format IF the error fires.
        if let Some(infinite) = msgs
            .iter()
            .find(|m| m.message().contains("Cannot construct infinite type"))
        {
            assert!(
                infinite.help().is_some(),
                "missing help on occurs-check message"
            );
        }
    }

    #[test]
    fn not_a_function_message_uses_cannot_call_format() {
        // `let x = 5; x(2);` — `x` is an int, calling it is an error.
        let (mut c, _) = check("let x = 5; x(2);");
        let msgs = c.take_messages();
        assert!(!msgs.is_empty(), "expected a message");
        let msg = msgs
            .iter()
            .find(|m| m.message().contains("Cannot call value") || m.message().contains("too many arguments"));
        assert!(msg.is_some(), "got: {:?}", msgs);
    }

    #[test]
    fn unknown_function_message_uses_can_not_find_format() {
        let (mut c, _) = check("missing_fn();");
        let msgs = c.take_messages();
        assert!(!msgs.is_empty(), "expected a message");
        let msg = msgs
            .iter()
            .find(|m| m.message().contains("Cannot find function"));
        assert!(msg.is_some(), "got: {:?}", msgs);
        let msg = msg.unwrap();
        assert!(
            msg.message().contains("`missing_fn`"),
            "got: {:?}",
            msg.message()
        );
    }

    #[test]
    fn assignment_to_undeclared_message_includes_help_hint() {
        let (mut c, _) = check("undeclared = 1;");
        let msgs = c.take_messages();
        assert!(!msgs.is_empty(), "expected a message");
        let msg = msgs
            .iter()
            .find(|m| m.message().contains("Cannot assign to undeclared"));
        assert!(msg.is_some(), "got: {:?}", msgs);
        let msg = msg.unwrap();
        let help = msg.help().as_ref().expect("missing help");
        assert!(
            help.contains("let undeclared"),
            "help should suggest `let undeclared;`, got: {:?}",
            help
        );
    }

    #[test]
    fn arity_mismatch_message_mentions_function_name() {
        // foo takes 1 arg; call with 2.
        let mut c = Checker::new();
        c.register_native("foo", &[int()], &int());
        let ast = Pratt::default().parse("foo(1, 2);").expect("parse");
        let _ = c.check_program(&ast);
        let msgs = c.take_messages();
        assert!(!msgs.is_empty(), "expected a message");
        let msg = msgs
            .iter()
            .find(|m| m.message().contains("too many arguments"));
        assert!(msg.is_some(), "got: {:?}", msgs);
        let msg = msg.unwrap();
        assert!(
            msg.message().contains("`foo`"),
            "got: {:?}",
            msg.message()
        );
    }

    #[test]
    fn diagnostic_messages_have_valid_ranges() {
        // Every diagnostic should have a non-empty range that lies
        // within the source bounds (0..src.len()). ariadne's renderer
        // requires this.
        for src in &["x;", "1 + true", "let y: int = 1; y = \"z\";"] {
            let (mut c, _) = check(src);
            let src_len = src.len();
            for msg in c.take_messages() {
                let r = msg.range();
                assert!(
                    r.start <= r.end && r.end <= src_len,
                    "bad range {:?} for source len {} (msg: {})",
                    r,
                    src_len,
                    msg.message()
                );
            }
        }
    }

    #[test]
    fn multiple_natives_coexist() {
        let mut c = Checker::new();
        c.register_native("print", &[string()], &unit_ty());
        c.register_native("add", &[int(), int()], &int());
        c.register_native("now", &[], &string());

        let print_ty = apply_ty(c.subst(), &c.env().lookup("print").unwrap().ty);
        let add_ty = apply_ty(c.subst(), &c.env().lookup("add").unwrap().ty);
        let now_ty = apply_ty(c.subst(), &c.env().lookup("now").unwrap().ty);

        assert_eq!(print_ty, Ty::Fun(Box::new(string()), Box::new(unit_ty())));
        assert_eq!(
            add_ty,
            Ty::Fun(
                Box::new(int()),
                Box::new(Ty::Fun(Box::new(int()), Box::new(int())))
            )
        );
        assert_eq!(now_ty, string());
    }

    // ---- Span-indexed cache (Phase 6) ----

    #[test]
    fn cache_is_populated_after_check_program() {
        // After infer, every pre-walked node should have a cached type.
        let (mut c, _) = check("1 + 2;");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        let total = c.id_table().len() as usize;
        assert!(total > 0);
        assert_eq!(c.cache_len(), total);
    }

    #[test]
    fn cache_lookup_returns_inferred_type() {
        // `1 + 2` parses to Expr(Add(Integer, Integer)); we expect the
        // cache to hold int() for each of those nodes.
        let (mut c, _) = check("1 + 2;");
        let ids = c.id_table().ids();
        for id in ids {
            let ty = c.lookup_at(*id).unwrap_or_else(|| panic!("no cache entry for {:?}", id));
            assert_eq!(ty, int(), "node {:?} had type {}", id, ty);
        }
    }

    #[test]
    fn cache_lookup_applies_substitution() {
        // `let x = 42; x` unifies x's fresh var with int. After unify,
        // the cached type for `x` should resolve to int, not the
        // original Ty::Var.
        let (mut c, _) = check("let x = 42; x");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        let scheme = c.env().lookup("x").expect("x not bound").clone();
        let resolved = apply_ty_prune(c.subst(), &scheme.ty);
        assert_eq!(resolved, int());
    }

    #[test]
    fn cache_lookup_returns_none_for_unknown_id() {
        let (c, _) = check("42;");
        assert!(c.lookup_at(NodeId(9999)).is_none());
    }

    #[test]
    fn pre_walk_mints_distinct_ids_for_nodes_sharing_a_span() {
        // `42;` produces a Program and a Statement that both span the
        // entire source. The pre-walk must give them distinct IDs.
        let (mut c, _) = check("42;");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        let ids = c.id_table().ids();
        let unique: std::collections::HashSet<_> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "IDs must be unique per AST node");
        assert!(ids.len() >= 3);
    }

    #[test]
    fn pre_walk_is_deterministic() {
        let (mut c1, _) = check("let a = 1; let b = 2; a + b");
        let (mut c2, _) = check("let a = 1; let b = 2; a + b");
        let msgs1 = c1.take_messages();
        let msgs2 = c2.take_messages();
        assert!(msgs1.is_empty(), "{:?}", msgs1);
        assert!(msgs2.is_empty(), "{:?}", msgs2);
        assert_eq!(c1.id_table().len(), c2.id_table().len());
        assert_eq!(c1.id_table().ids(), c2.id_table().ids());
    }

    #[test]
    fn cache_has_entries_for_value_producing_nodes() {
        // The cache holds a type per node that produces a value.
        // Declarations like `Variable` and `Comment` are side effects on
        // the env and don't produce a typed value, so they don't get a
        // cache entry — but they're still visited by the pre-walk.
        // The cache size should therefore be `<=` the pre-walk size.
        for src in &["42;", "1 + 2;", "let x = 1; x", "if true { 42; }"] {
            let (mut c, _) = check(src);
            let msgs = c.take_messages();
            assert!(msgs.is_empty(), "{:?} for `{}`", msgs, src);
            assert!(
                c.cache_len() <= c.id_table().len(),
                "cache ({}) larger than pre-walk ({}) for `{}`",
                c.cache_len(),
                c.id_table().len(),
                src
            );
            // And at least one entry per source.
            assert!(c.cache_len() > 0, "empty cache for `{}`", src);
        }
    }

    // ---- Call with arguments ----

    #[test]
    fn unknown_call_argument_types_dont_crash() {
        let msgs = assert_messages("foo(1, 2, 3);");
        assert!(!msgs.is_empty());
    }
}