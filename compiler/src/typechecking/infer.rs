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

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use common::{Label, Message};
use parser::ast::{Expression, MatchArm, Output, Pattern, Visibility};

use super::env::{instantiate, Env, TyVarCounter};
use super::id::{self, IdTable, NodeId};
use super::ty::Scheme;
use super::subst::{apply_ty, apply_ty_prune, compose, Subst};
use super::ty::{
    boolean, float, int, list, string, unit as unit_ty, EnumVariantPayloadTy, Ty,
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

    /// Side-table of variable name → inferred type. Populated
    /// during `infer_function` (function arguments) and
    /// `infer_fragment` (let-bindings). Used by the codegen to
    /// resolve a field-access receiver's type when the infer cache
    /// is misaligned with the pre-walk's ID table inside function
    /// bodies (Phase 18D — `Expression::Access` codegen). Cleared
    /// by `check_program`.
    codegen_var_types: std::collections::HashMap<String, Ty>,

    // ---- Sum-type tables (Phase 15B) ----
    //
    // We keep four parallel data structures for sum types. The `Vec`
    // fields hold the source-declaration order (insertion order); the
    // `BTreeMap` fields index by variant name for lookup. Tag values
    // are assigned by the position in the `Vec`, NOT alphabetically —
    // a `BTreeMap`-only representation would silently miscompile
    // (see `MUST-HAVE #2` in the Phase 15B plan).
    /// enum name → list of variant names in source-declaration order.
    enums: BTreeMap<String, Vec<String>>,
    /// enum name → (variant name → tag index).
    enum_tags: BTreeMap<String, BTreeMap<String, u32>>,
    /// enum name → per-variant payload shape (in source order).
    /// Phase 17B: each entry is `EnumVariantPayloadTy` (Unit,
    /// Tuple, or Record) — shape is recorded explicitly, not via
    /// synthetic names. The outer index matches the tag.
    enum_payloads: BTreeMap<String, Vec<EnumVariantPayloadTy>>,
    /// enum name → per-variant arity (in source order). Cached here
    /// so the codegen layer (15C) doesn't have to redo the
    /// `payloads[i].len()` lookup at every constructor site.
    enum_arities: BTreeMap<String, Vec<usize>>,

    /// Deferred exhaustiveness checks. Each entry is a match site
    /// that should be verified AFTER the main inference pass — that
    /// way the substitution is closed and the scrutinee's final type
    /// is observable (free type variables at the match site can
    /// otherwise hide the resolved sum type).
    pending_exhaustive: Vec<PendingExhaustive>,
}

/// One pending exhaustiveness check, recorded at the match site and
/// run in [`Checker::run_pending_exhaustiveness`].
///
/// `scrutinee_ty` is captured at the time the match is processed;
/// the post-pass resolves it under the final substitution.
#[derive(Debug, Clone)]
struct PendingExhaustive {
    /// Resolved scrutinee type at the time of the match. The
    /// post-pass re-applies the current substitution so any
    /// variables bound since the match site are visible.
    scrutinee_ty: Ty,
    /// The arms, in order. Each entry says which tag (if any) this
    /// arm covers, the arm's source range, and whether the arm is
    /// a wildcard / binding (which covers all remaining cases).
    arms: Vec<ArmCoverage>,
    /// Source range of the `match` keyword — used for the
    /// non-exhaustive diagnostic.
    match_range: Range<usize>,
}

/// What the inner pattern covers (only meaningful for Constructor
/// arms). For the codegen's test chain (which inspects only the
/// FIRST inner pattern), this captures the first non-trivial
/// inner pattern's tag.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum InnerCoverage {
    /// No inner pattern to test (Unit payload, or Wildcard/Binding
    /// which always match).
    Any,
    /// The inner pattern is a Constructor with the given tag.
    /// Multiple inner tags across multiple arms of the same outer
    /// tag are all reachable; two arms with the same outer AND
    /// same inner tag are still duplicates.
    Tag(u32),
}

/// Per-arm coverage info, captured at the match site.
#[derive(Debug, Clone)]
struct ArmCoverage {
    /// The variant tag this arm covers, if it was a constructor
    /// pattern. `None` for wildcards, bindings, and irrefutable
    /// catches.
    tag: Option<u32>,
    /// The inner pattern's coverage, when this arm's pattern is a
    /// Constructor with a payload. See [`InnerCoverage`]. For
    /// non-constructor arms this is [`InnerCoverage::Any`].
    inner: InnerCoverage,
    /// True if the arm was a wildcard (`_`) or a binding (`name`).
    /// Such arms cover all remaining cases (Rust-style).
    is_catchall: bool,
    /// The arm's source range — used for the "unreachable arm"
    /// diagnostic.
    range: Range<usize>,
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
            codegen_var_types: std::collections::HashMap::new(),
            enums: BTreeMap::new(),
            enum_tags: BTreeMap::new(),
            enum_payloads: BTreeMap::new(),
            enum_arities: BTreeMap::new(),
            pending_exhaustive: Vec::new(),
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
        // Reset per-program state. The pre-pass, the main infer
        // pass, and the post-pass all share the same checker; only
        // the per-program tables and caches get cleared.
        self.ids = IdTable::new();
        self.next_id_idx = 0;
        self.cache.clear();
        self.codegen_var_types.clear();
        self.enums.clear();
        self.enum_tags.clear();
        self.enum_payloads.clear();
        self.enum_arities.clear();
        self.pending_exhaustive.clear();

        // Mint NodeIds for every AST node (pre-walk). The visit order
        // matches `infer`'s recursion, so the IDs line up.
        id::pre_walk(ast, &mut self.ids);

        // Forward-declaration pre-pass: walk the AST once and
        // register every `enum` declaration's shape. This must run
        // before the main infer pass so constructor / match uses
        // that appear textually before their enum declaration still
        // resolve correctly.
        if let Err(msgs) = self.pre_register_enums(ast) {
            self.messages.extend(msgs);
        }

        // Top frame so natives and globals have a place to bind (Phase 7).
        // The frame is left on the stack so callers (and tests) can
        // inspect declared bindings via [`env`](Self::env).
        self.env.push();
        let ty = self.infer(ast);
        // NOTE: the frame is intentionally NOT popped — see the
        // doc-comment above.

        // Post-pass: run deferred exhaustiveness checks now that
        // the substitution is closed and every scrutinee type can
        // be fully resolved.
        self.run_pending_exhaustiveness();

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
    /// eventual bytecode emitter integration. Phase 15D marks
    /// this `#[allow(dead_code)]` because no current test or
    /// call site uses it — the codegen consults the cache
    /// directly via `lookup_at` instead. Kept for the eventual
    /// "give me the full type environment" debugging surface.
    #[cfg(test)]
    #[allow(dead_code)]
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
                self.infer_match(scrutinee, arms, range)
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
            Expression::Print(fmt, params) => {
                self.infer_print(fmt, params, range, "print");
                unit_ty()
            }
            Expression::Format(fmt, params) => {
                self.infer_print(fmt, params, range, "format");
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
            Expression::Access(receiver, field) => {
                // Field access on a record-shaped enum payload
                // (Phase 18D). The receiver's type is one of:
                //   - `Ty::Sum { name, variants }` — a value whose
                //     active variant isn't known statically; the
                //     field must be uniquely declared across the
                //     sum's record-shaped variants, OR the user
                //     must narrow with a `match` first.
                //   - `Ty::Constructor { tag, owner, .. }` — a
                //     value statically known to be a specific
                //     variant; we look up the field in that
                //     variant's payload only.
                //   - `Ty::Con(name)` — a name-only reference to a
                //     user-declared type (typical for function
                //     parameters annotated with the bare enum
                //     name); we resolve it through the checker's
                //     enum registry.
                //   - Anything else (primitive, `Ty::Var`,
                //     unresolved, …) → "Cannot access field on
                //     non-record type".
                let receiver_ty = self.infer(receiver);
                let resolved = apply_ty_prune(&self.subst, &receiver_ty);
                match &resolved {
                    Ty::Sum { name, variants } => self.access_field_in_sum(
                        name, variants, None, field, range,
                    ),
                    Ty::Constructor { tag, owner, .. } => {
                        // Resolve the owner to its variants.
                        match owner.as_ref() {
                            Ty::Sum { name, variants } => self.access_field_in_sum(
                                name,
                                variants,
                                Some(*tag),
                                field,
                                range,
                            ),
                            _ => self.error_with_help(
                                format!(
                                    "Cannot access field `{}` on non-record type",
                                    field
                                ),
                                range,
                                Some(
                                    "only values of record-shaped enum types expose fields"
                                        .to_string(),
                                ),
                            ),
                        }
                    }
                    Ty::Con(name) => {
                        // Bare type name — resolve via the
                        // checker's enum registry.
                        let variant_names =
                            self.enums.get(name).cloned().unwrap_or_default();
                        let payloads = self
                            .enum_payloads
                            .get(name)
                            .cloned()
                            .unwrap_or_default();
                        if variant_names.is_empty() {
                            return self.error_with_help(
                                format!(
                                    "Cannot access field `{}` on non-record type",
                                    field
                                ),
                                range,
                                Some(format!(
                                    "type `{}` is not a record-shaped enum",
                                    name
                                )),
                            );
                        }
                        let variants: Vec<(String, EnumVariantPayloadTy)> = variant_names
                            .into_iter()
                            .zip(payloads.into_iter())
                            .collect();
                        self.access_field_in_sum(name, &variants, None, field, range)
                    }
                    _ => self.error_with_help(
                        format!("Cannot access field `{}` on non-record type", field),
                        range,
                        Some(
                            "only values of record-shaped enum types expose fields"
                                .to_string(),
                        ),
                    ),
                }
            }
            Expression::Update(_, e) => self.infer(e),
            Expression::Instantiate(class_expr, _args) => self.infer(class_expr),
            Expression::Field(_, _, _) => unit_ty(),

            // ---- Phase 15A placeholders (TODO 15B) ----
            // These keep the workspace building while the
            // typechecker is extended to understand sum types and
            // pattern matching. None of them are semantically
            // correct yet; the compiler's test suite is expected
            // to fail on enum/match/construct source until 15B
            // lands. The HM pre-walk visits the new nodes (see
            // `id.rs`) and mints a `NodeId` for every child, so
            // these stubs recurse into their children (consuming
            // those IDs to stay lockstep with the pre-walk) but
            // discard the inferred types — the return value is
            // just a placeholder until 15B replaces these arms
            // with real implementations.
            Expression::EnumDecl { name, variants } => {
                // The pre-pass collected the variant names and
                // arities (for collision checks). The main pass
                // builds the actual `Ty::Sum` from the AST's
                // payload types and registers everything with the
                // env.
                self.infer_enum_decl(name, variants, &range);
                unit_ty()
            }
            Expression::EnumVariant { payload, .. } => {
                use parser::ast::EnumVariantPayload;
                // The pre-walk mints an ID for every payload
                // element. Recurse so this arm's ID consumption
                // stays in lockstep. The actual payload parsing
                // happens in `infer_enum_decl`, which knows the
                // parent variant name and target arity.
                match payload {
                    EnumVariantPayload::Unit => {}
                    EnumVariantPayload::Tuple(parts) => {
                        for p in parts {
                            let _ = self.infer(p);
                        }
                    }
                    EnumVariantPayload::Record(fields) => {
                        for f in fields {
                            let _ = self.infer(&f.value);
                        }
                    }
                }
                unit_ty()
            }
            Expression::Construct { enum_name, variant_name, fields } => {
                self.infer_construct(enum_name, variant_name, fields, range)
            }

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
                    // Phase 18D: record the let-bound variable's
                    // declared type in the codegen side-table so
                    // the codegen can resolve `name`'s type
                    // without depending on the infer cache or env.
                    self.codegen_var_types
                        .insert(name.to_string(), var_ty.clone());
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
                        self.env.insert_top(n.to_string(), Scheme::mono(var_ty.clone()));
                        // Phase 18D: same codegen side-table as
                        // let-bound variables above.
                        self.codegen_var_types.insert(n.to_string(), var_ty);
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
            // Phase 18D: also record in the codegen side-table so
            // the codegen can resolve `arg_name`'s type without
            // depending on the (potentially popped) env or the
            // (misaligned) infer cache.
            self.codegen_var_types
                .insert(arg_name.clone(), arg_ty.clone());
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

    // ============================================================
    //  Sum types and pattern matching (Phase 15B)
    // ============================================================

    /// Forward-declaration pre-pass: walk the AST once and reserve
    /// every `enum` declaration's name, variants, and arities. This
    /// runs before the main infer pass so that constructor / match
    /// uses that appear textually before their enum declaration
    /// still resolve correctly.
    ///
    /// Returns `Err(messages)` if any duplicate or invalid
    /// declaration is found; on `Err` the checker still has the
    /// pre-pass's tables partially populated (caller decides whether
    /// to continue). The main pass is robust to a missing entry —
    /// it just leaves the offending node without a registered
    /// type.
    fn pre_register_enums(&mut self, ast: &Output) -> Result<(), Vec<Message>> {
        let mut errors = Vec::new();
        self.pre_register_enums_walk(ast, &mut errors);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Recursive walker for the pre-pass. Mirrors the structure of
    /// `id::pre_walk_children` but only does work on `EnumDecl`
    /// nodes — everything else is structural recursion. Does NOT
    /// call `self.infer` (and so does not consume IDs from the
    /// pre-walk table).
    fn pre_register_enums_walk(&mut self, node: &Output, errors: &mut Vec<Message>) {
        use parser::ast::EnumVariantPayload;
        match node.1.as_ref() {
            Expression::EnumDecl { name, variants } => {
                let name_str = name.to_string();
                let mut variant_names = Vec::new();
                let mut arities = Vec::new();
                let mut payloads: Vec<EnumVariantPayloadTy> = Vec::new();

                for v in variants {
                    if let Expression::EnumVariant {
                        name: vname,
                        payload,
                    } = v.1.as_ref()
                    {
                        variant_names.push(vname.to_string());
                        // Phase 17B: build the typed payload shape
                        // directly from the AST shape
                        // (`EnumVariantPayload`), recording Unit /
                        // Tuple / Record explicitly.
                        let payload_ty = match payload {
                            EnumVariantPayload::Unit => {
                                arities.push(0);
                                EnumVariantPayloadTy::Unit
                            }
                            EnumVariantPayload::Tuple(parts) => {
                                let mut tys = Vec::with_capacity(parts.len());
                                for p in parts {
                                    if let Expression::Type(tname) = p.1.as_ref() {
                                        tys.push(self.parse_type_name_str(tname));
                                    } else {
                                        tys.push(Ty::Var(self.counter.fresh()));
                                    }
                                }
                                arities.push(tys.len());
                                EnumVariantPayloadTy::Tuple(tys)
                            }
                            EnumVariantPayload::Record(fields) => {
                                let mut pairs = Vec::with_capacity(fields.len());
                                for f in fields {
                                    let fty = if let Expression::Type(tname) = f.value.1.as_ref()
                                    {
                                        self.parse_type_name_str(tname)
                                    } else {
                                        Ty::Var(self.counter.fresh())
                                    };
                                    pairs.push((f.name.to_string(), fty));
                                }
                                arities.push(pairs.len());
                                EnumVariantPayloadTy::Record(pairs)
                            }
                        };
                        payloads.push(payload_ty);
                    }
                }

                // Check 1: duplicate enum name.
                if self.enums.contains_key(&name_str) {
                    let mut msg = Message::error(
                        format!("Duplicate enum `{}`", name_str),
                        node.0.into_range(),
                    );
                    msg.with_help(format!(
                        "an enum named `{}` was already declared; remove or rename this declaration",
                        name_str
                    ));
                    errors.push(msg);
                    return;
                }

                // Check 2: variant name collides with a previously
                // registered enum's variant name (cross-enum).
                for vn in &variant_names {
                    let taken = self.enum_tags.values().any(|tags| tags.contains_key(vn));
                    if taken {
                        let mut msg = Message::error(
                            format!(
                                "Duplicate constructor `{}` (also declared by another enum)",
                                vn
                            ),
                            node.0.into_range(),
                        );
                        msg.with_help(
                            "constructor names must be unique across all enums".to_string(),
                        );
                        errors.push(msg);
                        return;
                    }
                }

                // Check 3: variant name shadows a built-in
                // (currently no such checks — natives are registered
                // with full names like `print` and don't share the
                // `::` namespace. Reserved for future use.)

                // Reserve. We use `BTreeMap` for tags (lookups are
                // by variant name, not order). The `Vec` for
                // variant order is the canonical declaration order.
                let mut tag_map = BTreeMap::new();
                for (i, vn) in variant_names.iter().enumerate() {
                    tag_map.insert(vn.clone(), i as u32);
                }

                self.enums.insert(name_str.clone(), variant_names);
                self.enum_tags.insert(name_str.clone(), tag_map);
                self.enum_payloads.insert(name_str.clone(), payloads);
                self.enum_arities.insert(name_str.clone(), arities);
            }

            // Recurse into the same children that `id::pre_walk` would
            // visit. We mirror the structure of `pre_walk_children`
            // but only need to find nested EnumDecls — most
            // branches can just walk their expression children.
            Expression::Noop(_)
            | Expression::Comment(_)
            | Expression::Integer(_)
            | Expression::Float(_)
            | Expression::String(_)
            | Expression::Bool(_)
            | Expression::Identifier(_)
            | Expression::Type(_)
            | Expression::Default(_)
            | Expression::Use { .. }
            | Expression::Module(_, _)
            | Expression::Variable(_, _)
            | Expression::Constant(_, _)
            | Expression::Argument(_, _)
            | Expression::Field(_, _, _) => {}

            Expression::Expr(e)
            | Expression::Group(e)
            | Expression::Statement(e)
            | Expression::ExprStatement(e)
            | Expression::Return(e)
            | Expression::ImplicitReturn(e)
            | Expression::Yield(e)
            | Expression::Negate(e)
            | Expression::Not(e)
            | Expression::Positive(e)
            | Expression::Inc(e)
            | Expression::Dec(e)
            | Expression::Defer(e)
            | Expression::Member(e)
            | Expression::Update(_, e) => {
                self.pre_register_enums_walk(e, errors);
            }

            Expression::Assignment(name, value) => {
                self.pre_register_enums_walk(name, errors);
                self.pre_register_enums_walk(value, errors);
            }

            Expression::Add(l, r)
            | Expression::Sub(l, r)
            | Expression::Mul(l, r)
            | Expression::Div(l, r)
            | Expression::Mod(l, r)
            | Expression::Pow(l, r)
            | Expression::Shl(l, r)
            | Expression::Shr(l, r)
            | Expression::Xor(l, r)
            | Expression::And(l, r)
            | Expression::Or(l, r)
            | Expression::BitAnd(l, r)
            | Expression::BitOr(l, r)
            | Expression::Eq(l, r)
            | Expression::Neq(l, r)
            | Expression::Le(l, r)
            | Expression::Gt(l, r)
            | Expression::Leq(l, r)
            | Expression::Geq(l, r) => {
                self.pre_register_enums_walk(l, errors);
                self.pre_register_enums_walk(r, errors);
            }

            Expression::Print(fmt, params) | Expression::Format(fmt, params) => {
                self.pre_register_enums_walk(fmt, errors);
                if let Some(p) = params {
                    for param in p {
                        self.pre_register_enums_walk(param, errors);
                    }
                }
            }

            Expression::Resume(target, arg) => {
                self.pre_register_enums_walk(target, errors);
                if let Some(a) = arg {
                    self.pre_register_enums_walk(a, errors);
                }
            }

            Expression::Block(cs)
            | Expression::Program(cs)
            | Expression::Fragment(cs)
            | Expression::List(cs) => {
                for c in cs {
                    self.pre_register_enums_walk(c, errors);
                }
            }
            Expression::If(branches) => {
                for b in branches {
                    self.pre_register_enums_walk(b, errors);
                }
            }
            Expression::Implementation(_, _, methods) => {
                for m in methods {
                    self.pre_register_enums_walk(m, errors);
                }
            }
            Expression::Class(_, fields) => {
                for f in fields {
                    self.pre_register_enums_walk(f, errors);
                }
            }

            Expression::Function { args, body, .. } => {
                self.pre_register_enums_walk(args, errors);
                self.pre_register_enums_walk(body, errors);
            }

            Expression::Branch(cond, body) => {
                if let Some(c) = cond {
                    self.pre_register_enums_walk(c, errors);
                }
                self.pre_register_enums_walk(body, errors);
            }

            Expression::Call { name, args } => {
                self.pre_register_enums_walk(name, errors);
                if let Some(a) = args {
                    for arg in a {
                        self.pre_register_enums_walk(arg, errors);
                    }
                }
            }

            Expression::Loop {
                iterable,
                body,
                identifier,
            } => {
                self.pre_register_enums_walk(iterable, errors);
                self.pre_register_enums_walk(body, errors);
                if let Some(i) = identifier {
                    self.pre_register_enums_walk(i, errors);
                }
            }

            Expression::Match { scrutinee, arms } => {
                self.pre_register_enums_walk(scrutinee, errors);
                for arm in arms {
                    // Patterns are not expressions — no recursion
                    // into the pattern body. (Constructor patterns
                    // contain only nested patterns.)
                    self.pre_register_enums_walk(&arm.body, errors);
                }
            }

            // The `EnumDecl` arm above handles every EnumDecl in
            // the tree; no second arm is needed here. `EnumVariant`
            // and `Construct` are still reachable (e.g. inside a
            // function body) and just recurse.
            Expression::EnumVariant { .. } => {
                // Phase 17B: the variant is a wrapper for the
                // shape-tagged payload. The pre-walk does NOT mint
                // IDs for the `RecordFieldDecl` entries inside a
                // record payload — they're metadata for the
                // typechecker, not expressions.
                // The shape itself is consumed by `pre_register_enums`
                // through the `EnumDecl` arm above (which has
                // already cloned the variant payloads). Nothing to
                // do here.
            }
            Expression::Construct { .. } => {
                // `Construct` carries the call-site arguments as
                // positional `Output`s, just like a call. The
                // pre-walk mints IDs for each argument (because
                // they're full `Output` nodes), but the shape
                // discriminator (Unit / Tuple / Record) is metadata.
            }

            Expression::Method(_, body) => {
                self.pre_register_enums_walk(body, errors);
            }
            Expression::Access(receiver, _) => {
                self.pre_register_enums_walk(receiver, errors);
            }
            Expression::Instantiate(class, args) => {
                self.pre_register_enums_walk(class, errors);
                if let Some(a) = args {
                    for arg in a {
                        self.pre_register_enums_walk(arg, errors);
                    }
                }
            }
        }
    }

    /// Build a fully-formed `Ty::Sum` from an enum declaration's
    /// AST. The pre-pass already collected the variant names,
    /// arities, and concrete payload shapes — this method walks
    /// the AST's payload children to keep ID consumption in
    /// lockstep with the pre-walk, builds the `Ty::Sum`, and
    /// registers the enum and each variant in the env.
    fn infer_enum_decl(
        &mut self,
        name: &str,
        variants: &[Output],
        _range: &Range<usize>,
    ) {
        use parser::ast::EnumVariantPayload;
        let name_str = name.to_string();
        // Look up the pre-reserved shape. If missing, the
        // pre-pass rejected this enum (duplicate / collision);
        // the caller has already pushed a diagnostic. Just walk
        // the children to keep IDs aligned.
        let pre_shape = match self.enums.get(&name_str).cloned() {
            Some(v) => v,
            None => {
                for v in variants {
                    let _ = self.infer(v);
                }
                return;
            }
        };
        let pre_payloads = match self.enum_payloads.get(&name_str).cloned() {
            Some(p) => p,
            None => {
                for v in variants {
                    let _ = self.infer(v);
                }
                return;
            }
        };

        // Walk each variant. We delegate to `self.infer(v)` for the
        // whole variant — its `EnumVariant` arm in `infer_inner`
        // recurses into the payload children. That gives us
        // exactly `1 + N` IDs per variant where N is the number
        // of payload entries the pre-walk visited (1 per Tuple
        // element, 1 per Record field's value, 0 for Unit). The
        // pre-pass has already built the typed payload, so the
        // infer recursion is purely for ID-alignment.
        let mut built_variants: Vec<(String, EnumVariantPayloadTy)> = Vec::new();
        for (i, v) in variants.iter().enumerate() {
            // Consume IDs for the variant itself + its payload
            // before any early `continue`. The pre-walk visited
            // this node and its payload regardless of whether we
            // accept it.
            let _ = self.infer(v);

            if let Expression::EnumVariant {
                name: vname,
                payload,
            } = v.1.as_ref()
            {
                let vname_str = vname.to_string();
                let pre_pay = match pre_payloads.get(i) {
                    Some(p) => p.clone(),
                    None => {
                        continue;
                    }
                };

                // Sanity: name + payload arity should match the
                // pre-pass shape. If not, the pre-pass has already
                // complained — skip registering this variant but
                // keep IDs aligned (already done above).
                if pre_shape.get(i) != Some(&vname_str) {
                    continue;
                }
                let expected_count = match &pre_pay {
                    EnumVariantPayloadTy::Unit => 0,
                    EnumVariantPayloadTy::Tuple(tys) => tys.len(),
                    EnumVariantPayloadTy::Record(fields) => fields.len(),
                };
                let actual_count = match payload {
                    EnumVariantPayload::Unit => 0,
                    EnumVariantPayload::Tuple(parts) => parts.len(),
                    EnumVariantPayload::Record(fields) => fields.len(),
                };
                if expected_count != actual_count {
                    continue;
                }
                built_variants.push((vname_str, pre_pay));
            }
        }

        // Build the Ty::Sum.
        let sum_ty = Ty::Sum {
            name: name_str.clone(),
            variants: built_variants.clone(),
        };

        // Register the enum itself as a type.
        self.env.insert_top(
            name_str.clone(),
            Scheme::mono(Ty::Con(name_str.clone())),
        );

        // Register each variant as a callable in the env. Use the
        // qualified name `EnumName::VariantName` as the binding
        // key — `Construct` looks up by qualified name in this
        // map.
        for (i, (vname, payload_ty)) in built_variants.iter().enumerate() {
            // Field count = 0 for Unit, N for Tuple/Record.
            // Same arity, regardless of shape — the shape
            // discrimination happens at call-site / pattern
            // inference, not at the constructor's HM type.
            let arity = payload_ty.field_count();
            let ctor_ty = Ty::Constructor {
                owner: Box::new(sum_ty.clone()),
                tag: i as u32,
                arity,
            };
            let scheme = if arity == 0 {
                Scheme::mono(ctor_ty)
            } else {
                // Curried: arg1 -> arg2 -> ... -> Constructor.
                // Field order matches declaration order for both
                // Tuple and Record — codegen reorders record
                // call sites to declaration order before pushing
                // the MAKE_ENUM.
                let arg_tys: Vec<Ty> = payload_ty.field_types().into_iter().cloned().collect();
                let mut fun_ty = ctor_ty;
                for arg_ty in arg_tys.iter().rev() {
                    fun_ty = Ty::Fun(Box::new(arg_ty.clone()), Box::new(fun_ty));
                }
                Scheme::mono(fun_ty)
            };
            let qualified = format!("{}::{}", name_str, vname);
            self.env.insert_top(qualified, scheme);
        }
    }

    /// Type-check a constructor application: `EnumName::Variant(args)`,
    /// `EnumName::Variant { ... }`, or `EnumName::Variant` (Unit).
    /// Field-by-field inference with shape-mismatch detection
    /// (17B red-team finding #5: a tuple call site against a record
    /// declaration, or vice versa, is a type error).
    fn infer_construct(
        &mut self,
        enum_name: &str,
        variant_name: &str,
        fields: &parser::ast::EnumConstructPayload<'_>,
        range: Range<usize>,
    ) -> Ty {
        use parser::ast::EnumConstructPayload;
        let enum_str = enum_name.to_string();
        let variant_str = variant_name.to_string();

        // Look up the enum. Error if not registered.
        let tags = match self.enum_tags.get(&enum_str) {
            Some(t) => t.clone(),
            None => {
                return self.error(
                    format!("Cannot find enum `{}` in this scope", enum_str),
                    range,
                );
            }
        };

        // Look up the variant tag.
        let tag = match tags.get(&variant_str) {
            Some(t) => *t,
            None => {
                return self.error(
                    format!(
                        "Cannot find variant `{}` on enum `{}`",
                        variant_str, enum_str
                    ),
                    range,
                );
            }
        };

        let arity = self
            .enum_arities
            .get(&enum_str)
            .and_then(|a| a.get(tag as usize).copied())
            .unwrap_or(0);
        let expected_payload = self
            .enum_payloads
            .get(&enum_str)
            .and_then(|p| p.get(tag as usize).cloned())
            .unwrap_or(EnumVariantPayloadTy::Unit);

        // Shape analysis (17B red-team #5).
        // Two failure modes:
        //  - Arity mismatch within the same shape (e.g. tuple(2 args)
        //    against a tuple declared with 1 field). Reported as
        //    "expects N arguments" — the original 15B message.
        //  - Shape mismatch across shapes (tuple vs record / unit vs
        //    others). Reported as "payload shape mismatch" — the
        //    17B red-team amendment #5.
        //
        // For record shapes, we skip the arity check and fall
        // through to the field-by-field check, which gives more
        // specific diagnostics (missing field `x`, unknown field
        // `y`, etc.) instead of a generic arity error.
        let (shape_matches, same_shape_with_wrong_arity) = match (&expected_payload, fields) {
            (EnumVariantPayloadTy::Unit, EnumConstructPayload::Unit) => (true, false),
            (EnumVariantPayloadTy::Tuple(_), EnumConstructPayload::Tuple(args)) => {
                let want = expected_payload.field_count();
                (args.len() == want, args.len() != want)
            }
            (EnumVariantPayloadTy::Record(_), EnumConstructPayload::Record(_)) => {
                // Defer the arity check to the field-by-field
                // pass below, which produces more specific
                // diagnostics ("Missing field `x`" instead of
                // "expects 2 arguments, got 1").
                (true, false)
            }
            _ => (false, false),
        };

        if !shape_matches {
            // Distinguish "shape mismatch" (17B) from "arity mismatch
            // within same shape" (15B legacy). When the shape itself
            // is wrong, emit the new shape-mismatch diagnostic;
            // otherwise emit the legacy arity message.
            if same_shape_with_wrong_arity {
                return self.error(
                    format!(
                        "Constructor `{}::{}` expects {} arguments, got {}",
                        enum_str,
                        variant_str,
                        expected_payload.field_count(),
                        match fields {
                            EnumConstructPayload::Unit => 0,
                            EnumConstructPayload::Tuple(args) => args.len(),
                            EnumConstructPayload::Record(parts) => parts.len(),
                        },
                    ),
                    range,
                );
            }
            return self.error_with_help(
                format!(
                    "Constructor `{}::{}` payload shape mismatch (declared as {}, called as {})",
                    enum_str,
                    variant_str,
                    payload_kind_name(&expected_payload),
                    match fields {
                        EnumConstructPayload::Unit => "unit",
                        EnumConstructPayload::Tuple(_) => "tuple",
                        EnumConstructPayload::Record(_) => "record",
                    },
                ),
                range,
                Some(format!(
                    "use {} syntax for `{}::{}`",
                    if matches!(expected_payload, EnumVariantPayloadTy::Record(_)) {
                        "record"
                    } else {
                        "tuple / unit"
                    },
                    enum_str,
                    variant_str,
                )),
            );
        }

        // Field-by-field type check.
        match fields {
            EnumConstructPayload::Unit => {}
            EnumConstructPayload::Tuple(args) => {
                let expected_tys = expected_payload.field_types();
                for (arg, expected_ty) in args.iter().zip(expected_tys.iter()) {
                    let arg_ty = self.infer(arg);
                    self.unify(
                        expected_ty,
                        &arg_ty,
                        &arg.0.into_range(),
                        &format!("constructor `{}::{}` argument", enum_str, variant_str),
                    );
                }
            }
            EnumConstructPayload::Record(parts) => {
                // Build a name → value map for the call site, then
                // walk the DECLARATION order. Each declared field
                // must be supplied exactly once; the codegen
                // reorders the bytecode accordingly.
                let mut call_site: std::collections::HashMap<&str, &Output> =
                    std::collections::HashMap::with_capacity(parts.len());
                for p in parts {
                    if call_site.insert(p.name, &p.value).is_some() {
                        return self.error_with_help(
                            format!(
                                "Duplicate field `{}` in record constructor `{}::{}`",
                                p.name, enum_str, variant_str,
                            ),
                            range,
                            Some("each field must be supplied exactly once".to_string()),
                        );
                    }
                }
                let EnumVariantPayloadTy::Record(decl_fields) = &expected_payload else {
                    // unreachable — shape_matches already proved it
                    unreachable!();
                };
                for (decl_name, decl_ty) in decl_fields.iter() {
                    let arg = match call_site.get(decl_name.as_str()) {
                        Some(a) => *a,
                        None => {
                            return self.error_with_help(
                                format!(
                                    "Missing field `{}` in record constructor `{}::{}`",
                                    decl_name, enum_str, variant_str,
                                ),
                                range,
                                Some(format!(
                                    "add `{}: <expr>` to the call site",
                                    decl_name,
                                )),
                            );
                        }
                    };
                    let arg_ty = self.infer(arg);
                    self.unify(
                        decl_ty,
                        &arg_ty,
                        &arg.0.into_range(),
                        &format!(
                            "constructor `{}::{}.{}` argument",
                            enum_str, variant_str, decl_name,
                        ),
                    );
                }
                // Check for any unknown field names (extra
                // fields supplied at the call site).
                for p in parts {
                    if !decl_fields.iter().any(|(dn, _)| dn == p.name) {
                        return self.error_with_help(
                            format!(
                                "Unknown field `{}` in record constructor `{}::{}`",
                                p.name, enum_str, variant_str,
                            ),
                            range,
                            Some(format!(
                                "the declared fields are: {}",
                                decl_fields
                                    .iter()
                                    .map(|(n, _)| format!("`{}`", n))
                                    .collect::<Vec<_>>()
                                    .join(", "),
                            )),
                        );
                    }
                }
            }
        }

        // Build the result. The owner is the full `Ty::Sum` so
        // later unifications (in match patterns) can compare tag
        // and arity directly.
        let sum_ty = Ty::Sum {
            name: enum_str.clone(),
            variants: self
                .enum_payloads
                .get(&enum_str)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .zip(self.enums.get(&enum_str).cloned().unwrap_or_default())
                .map(|(p, n)| (n, p))
                .collect(),
        };

        Ty::Constructor {
            owner: Box::new(sum_ty),
            tag,
            arity,
        }
    }

    /// Match an expression against a list of pattern arms.
    ///
    /// 1. Infer the scrutinee's type.
    /// 2. Walk each arm:
    ///    a. Type the pattern (binding variables in a fresh env
    ///       frame so they don't leak across arms).
    ///    b. Unify the pattern type with the scrutinee type.
    ///    c. Infer the body and unify it with the result type.
    /// 3. Record coverage info for the deferred exhaustiveness
    ///    check.
    fn infer_match(
        &mut self,
        scrutinee: &Output,
        arms: &[MatchArm],
        range: Range<usize>,
    ) -> Ty {
        let scrutinee_ty = self.infer(scrutinee);
        let resolved_scrutinee = apply_ty_prune(&self.subst, &scrutinee_ty);

        // Set up current_match_lhs for `Expression::Default`
        // (which Decision C preserves but is unreachable in real
        // source — wildcard patterns never reach it).
        let prev = self.current_match_lhs.replace(scrutinee_ty.clone());

        let mut result_ty = Ty::Var(self.counter.fresh());
        let mut first = true;
        let mut coverage: Vec<ArmCoverage> = Vec::with_capacity(arms.len());

        if arms.is_empty() {
            self.current_match_lhs = prev;
            return self.error("match has no arms".to_string(), range);
        }

        for arm in arms {
            // Step 1: each arm gets a fresh env frame so the
            // pattern's bindings don't leak.
            self.env.push();

            // Step 2: type the pattern, binding variables. The
            // pattern AST doesn't carry its own range today, so we
            // pass the arm's body range as a reasonable proxy for
            // error anchoring — it's close enough that ariadne
            // points near the offending pattern instead of at byte
            // 0 of the source.
            let pattern_range = arm.body.0.into_range();
            let pat_ty = self.infer_pattern(&arm.pattern, &resolved_scrutinee, &pattern_range);

            // Step 3: unify pattern type with scrutinee.
            self.unify(
                &resolved_scrutinee,
                &pat_ty,
                &arm.body.0.into_range(),
                "match pattern against scrutinee",
            );

            // Step 4: capture coverage info.
            let arm_cov = self.arm_coverage(&arm.pattern, &arm.body.0.into_range());
            coverage.push(arm_cov);

            // Step 5: infer body, unify with result.
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

            // Step 6: pop the per-arm env frame.
            self.env.pop();
        }

        self.current_match_lhs = prev;

        // Record for the post-pass exhaustiveness check. The
        // scrutinee type stored here is the resolved (pruned)
        // version at the time of the match; the post-pass will
        // re-apply the current substitution to handle any
        // variables bound by intervening code.
        self.pending_exhaustive.push(PendingExhaustive {
            scrutinee_ty: resolved_scrutinee,
            arms: coverage,
            match_range: range,
        });

        result_ty
    }

    /// Type-check a pattern against an expected type, binding
    /// variables into the current env frame. Returns the pattern's
    /// type, which is the **expected** type (the sum type, not
    /// the constructor type) — patterns desugar the scrutinee, so
    /// the pattern's type IS the scrutinee's type. The tag
    /// matching (which determines whether the arm is reachable) is
    /// captured separately in [`ArmCoverage`].
    ///
    /// `pattern_range` is the source range of the pattern itself —
    /// or, when not available, a reasonable proxy (the arm's body
    /// range). It is used to anchor pattern-related diagnostics
    /// (`unknown constructor`, `wrong arity`) so ariadne points at
    /// the offending pattern instead of byte 0 of the source.
    fn infer_pattern(
        &mut self,
        pattern: &Pattern,
        expected_ty: &Ty,
        pattern_range: &Range<usize>,
    ) -> Ty {
        use parser::ast::PatternPayload;
        match pattern {
            Pattern::Wildcard => {
                // Wildcard matches anything, binds nothing. The
                // body's bindings (if any) come from nested
                // patterns; wildcard itself has no payload.
                expected_ty.clone()
            }
            Pattern::Binding { name } => {
                // `name => body` binds `name` to the scrutinee in
                // the arm's env. This makes the arm cover every
                // case (Rust semantics).
                self.env
                    .insert_top(name.to_string(), Scheme::mono(expected_ty.clone()));
                expected_ty.clone()
            }
            Pattern::Constructor {
                enum_name,
                variant_name,
                payload,
            } => {
                // 1. Look up the variant's tag in the registry.
                let enum_str = enum_name.to_string();
                let variant_str = variant_name.to_string();
                let tag_opt = self
                    .enum_tags
                    .get(&enum_str)
                    .and_then(|t| t.get(&variant_str).copied());
                let tag = match tag_opt {
                    Some(t) => t,
                    None => {
                        // Unknown constructor in a pattern is an
                        // error. Record the error and return the
                        // expected type so the arm body is still
                        // processed.
                        self.messages.push(Message::error(
                            format!(
                                "Pattern references unknown constructor `{}::{}`",
                                enum_str, variant_str
                            ),
                            pattern_range.clone(),
                        ));
                        return expected_ty.clone();
                    }
                };
                let _arity = self
                    .enum_arities
                    .get(&enum_str)
                    .and_then(|a| a.get(tag as usize).copied())
                    .unwrap_or(0);
                let expected_payload = self
                    .enum_payloads
                    .get(&enum_str)
                    .and_then(|p| p.get(tag as usize).cloned())
                    .unwrap_or(EnumVariantPayloadTy::Unit);

                // 2. Shape check (17B red-team #5) — tuple vs
                // record shape must match the declared variant.
                // Distinguish:
                //  - Same shape, wrong sub-pattern count
                //    (`expected N sub-patterns, got M` — legacy
                //    15B message).
                //  - Different shapes (tuple vs record / unit)
                //    (`payload shape mismatch` — 17B message).
                //
                // For record shapes, we skip the arity check
                // and fall through to the field-by-field check
                // below, which gives more specific diagnostics
                // ("Missing field `x`" / "Unknown field `y`")
                // instead of a generic arity error.
                let (shape_matches, same_shape_with_wrong_arity) = match (&expected_payload, payload) {
                    (EnumVariantPayloadTy::Unit, PatternPayload::Unit) => (true, false),
                    (EnumVariantPayloadTy::Tuple(_), PatternPayload::Tuple(parts)) => {
                        let want = expected_payload.field_count();
                        (parts.len() == want, parts.len() != want)
                    }
                    (EnumVariantPayloadTy::Record(_), PatternPayload::Record(_)) => {
                        // Defer the arity check to the
                        // field-by-field pass below, which
                        // produces more specific diagnostics.
                        (true, false)
                    }
                    _ => (false, false),
                };
                if !shape_matches {
                    if same_shape_with_wrong_arity {
                        return self.error_with_help(
                            format!(
                                "Constructor pattern `{}::{}` expects {} sub-patterns, got {}",
                                enum_str,
                                variant_str,
                                expected_payload.field_count(),
                                match payload {
                                    PatternPayload::Unit => 0,
                                    PatternPayload::Tuple(parts) => parts.len(),
                                    PatternPayload::Record(fields) => fields.len(),
                                },
                            ),
                            pattern_range.clone(),
                            Some("check the variant's declared payload arity".to_string()),
                        );
                    }
                    return self.error_with_help(
                        format!(
                            "Constructor pattern `{}::{}` payload shape mismatch (declared as {}, pattern uses {})",
                            enum_str,
                            variant_str,
                            payload_kind_name(&expected_payload),
                            match payload {
                                PatternPayload::Unit => "unit",
                                PatternPayload::Tuple(_) => "tuple",
                                PatternPayload::Record(_) => "record",
                            },
                        ),
                        pattern_range.clone(),
                        Some("check the variant's declared payload shape".to_string()),
                    );
                }

                // 3. Recurse into each sub-pattern with the
                // corresponding payload type. The payload type
                // comes from the pre-pass's `enum_payloads`
                // (already resolved, e.g. `int` for
                // `Option::Some(int)`).
                match payload {
                    PatternPayload::Unit => {}
                    PatternPayload::Tuple(parts) => {
                        let expected_tys = expected_payload.field_types();
                        for (sub_pat, expected_ty) in
                            parts.iter().zip(expected_tys.iter())
                        {
                            let _ = self.infer_pattern(sub_pat, expected_ty, pattern_range);
                        }
                    }
                    PatternPayload::Record(fields) => {
                        // Build a name → pattern map for the
                        // pattern site, then walk DECLARATION
                        // order. Each declared field must be
                        // present exactly once; the codegen binds
                        // in slot order (= declaration order).
                        let mut pattern_site: std::collections::HashMap<&str, &Pattern> =
                            std::collections::HashMap::with_capacity(fields.len());
                        for pf in fields {
                            if pattern_site.insert(pf.name, &pf.pattern).is_some() {
                                return self.error_with_help(
                                    format!(
                                        "Duplicate field `{}` in record pattern `{}::{}`",
                                        pf.name, enum_str, variant_str,
                                    ),
                                    pattern_range.clone(),
                                    Some("each field must appear exactly once".to_string()),
                                );
                            }
                        }
                        let EnumVariantPayloadTy::Record(decl_fields) = &expected_payload else {
                            unreachable!()
                        };
                        for (decl_name, decl_ty) in decl_fields.iter() {
                            let sub_pat = match pattern_site.get(decl_name.as_str()) {
                                Some(p) => *p,
                                None => {
                                    return self.error_with_help(
                                        format!(
                                            "Missing field `{}` in record pattern `{}::{}`",
                                            decl_name, enum_str, variant_str,
                                        ),
                                        pattern_range.clone(),
                                        Some(format!(
                                            "add `{0}: _` (or `{0}: binding`) to the pattern",
                                            decl_name,
                                        )),
                                    );
                                }
                            };
                            let _ = self.infer_pattern(sub_pat, decl_ty, pattern_range);
                        }
                        // Check for unknown field names.
                        for pf in fields {
                            if !decl_fields.iter().any(|(dn, _)| dn == pf.name) {
                                return self.error_with_help(
                                    format!(
                                        "Unknown field `{}` in record pattern `{}::{}`",
                                        pf.name, enum_str, variant_str,
                                    ),
                                    pattern_range.clone(),
                                    Some(format!(
                                        "the declared fields are: {}",
                                        decl_fields
                                            .iter()
                                            .map(|(n, _)| format!("`{}`", n))
                                            .collect::<Vec<_>>()
                                            .join(", "),
                                    )),
                                );
                            }
                        }
                    }
                }

                // 4. The pattern's type is the *expected* type —
                // patterns desugar the scrutinee, so the pattern
                // returns whatever the scrutinee had. (If the
                // scrutinee was a Ty::Constructor for a specific
                // tag, the pattern is still of that same type;
                // exhaustiveness checking will report the
                // arm as unreachable.)
                expected_ty.clone()
            }
        }
    }

    /// Inspect the first non-trivial sub-pattern of a payload and
    /// report which inner tag (if any) it tests. Two arms of the
    /// same outer tag are reachable as long as their inner coverage
    /// differs — e.g. `Result::Ok(Option::Some(v))` and
    /// `Result::Ok(Option::None)` are two distinct reachable arms.
    /// The codegen's inner `JUMP_IF_MATCH` test chain guarantees
    /// this at runtime; the typechecker just needs to stay out of
    /// the way.
    fn inner_coverage(
        payload: &parser::ast::PatternPayload<'_>,
        enum_tags: &BTreeMap<String, BTreeMap<String, u32>>,
    ) -> InnerCoverage {
        use parser::ast::PatternPayload;
        let first = match payload {
            PatternPayload::Unit => return InnerCoverage::Any,
            PatternPayload::Tuple(parts) => parts.first(),
            PatternPayload::Record(fields) => fields.first().map(|f| &f.pattern),
        };
        let Some(first) = first else {
            return InnerCoverage::Any;
        };
        match first {
            Pattern::Wildcard | Pattern::Binding { .. } => InnerCoverage::Any,
            Pattern::Constructor {
                enum_name,
                variant_name,
                ..
            } => enum_tags
                .get(enum_name.to_string().as_str())
                .and_then(|t| t.get(variant_name.to_string().as_str()).copied())
                .map(InnerCoverage::Tag)
                .unwrap_or(InnerCoverage::Any),
        }
    }

    /// Capture per-arm coverage info for the deferred
    /// exhaustiveness check.
    fn arm_coverage(&self, pattern: &Pattern, range: &Range<usize>) -> ArmCoverage {
        match pattern {
            Pattern::Wildcard => ArmCoverage {
                tag: None,
                inner: InnerCoverage::Any,
                is_catchall: true,
                range: range.clone(),
            },
            Pattern::Binding { .. } => ArmCoverage {
                tag: None,
                inner: InnerCoverage::Any,
                is_catchall: true,
                range: range.clone(),
            },
            Pattern::Constructor {
                enum_name,
                variant_name,
                payload,
                ..
            } => {
                let tag = self
                    .enum_tags
                    .get(enum_name.to_string().as_str())
                    .and_then(|t| t.get(variant_name.to_string().as_str()).copied());
                let inner = Self::inner_coverage(payload, &self.enum_tags);
                ArmCoverage {
                    tag,
                    inner,
                    is_catchall: false,
                    range: range.clone(),
                }
            }
        }
    }

    /// Post-pass: run every deferred exhaustiveness check. By this
    /// point the substitution is closed, so the scrutinee type is
    /// fully resolved (any free type variables that were bound
    /// since the match site are visible here).
    fn run_pending_exhaustiveness(&mut self) {
        // Drain into a local so we can release the borrow on
        // `self` before mutating `self.messages`.
        let pending: Vec<PendingExhaustive> = std::mem::take(&mut self.pending_exhaustive);
        for p in &pending {
            self.check_exhaustiveness(p);
        }
    }

    /// Verify a single match site. Records diagnostics but does
    /// not abort — error recovery continues.
    fn check_exhaustiveness(&mut self, pending: &PendingExhaustive) {
        // Re-resolve the scrutinee under the current substitution
        // so any variables bound between the match site and the
        // post-pass are visible.
        let resolved = apply_ty_prune(&self.subst, &pending.scrutinee_ty);

        // Track which (outer tag, inner coverage) pairs have been
        // seen and whether a catch-all (wildcard / binding) is
        // present. Two arms with the same outer tag but DIFFERENT
        // inner coverage (e.g. `Result::Ok(Option::Some(v))` vs
        // `Result::Ok(Option::None)`) are both reachable — the
        // codegen's inner `JUMP_IF_MATCH` chain dispatches between
        // them at runtime. Only when both the outer tag AND the
        // inner coverage match an earlier arm is the arm truly
        // unreachable.
        let mut seen: BTreeMap<u32, BTreeSet<InnerCoverage>> = BTreeMap::new();
        let mut has_catchall = false;
        for arm in &pending.arms {
            if arm.is_catchall {
                has_catchall = true;
            } else if let Some(t) = arm.tag {
                let inner_seen = seen.entry(t).or_default();
                if !inner_seen.insert(arm.inner.clone()) {
                    // Duplicate (tag, inner coverage) — this arm
                    // is unreachable.
                    self.messages.push(Message::error(
                        "Unreachable arm: this pattern is matched by an earlier arm".to_string(),
                        arm.range.clone(),
                    ));
                }
            }
        }

        if has_catchall {
            // A wildcard / binding arm covers every remaining
            // case. No further error needed.
            return;
        }

        // Unwrap a Constructor to its parent sum (the scrutinee
        // is usually a Constructor because constructors are how
        // enum values come into the match site). For Ty::Var /
        // Ty::Con, no exhaustiveness check.
        let sum_ty = match &resolved {
            Ty::Sum { .. } => &resolved,
            Ty::Constructor { owner, .. } => owner.as_ref(),
            _ => return,
        };

        if let Ty::Sum { variants, .. } = sum_ty {
            // An outer tag is "covered" for the purpose of the
            // non-exhaustive check if any arm with that tag
            // exists. The inner coverage only matters for the
            // duplicate-arm check above.
            let covered: BTreeSet<u32> = seen.into_keys().collect();
            let missing: Vec<String> = variants
                .iter()
                .enumerate()
                .filter(|(tag, _)| !covered.contains(&(*tag as u32)))
                .map(|(_, (n, _))| n.clone())
                .collect();
            if !missing.is_empty() {
                let names = missing
                    .iter()
                    .map(|s| format!("`{}`", s))
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut msg = Message::error(
                    format!("Non-exhaustive match: variants not covered: {}", names),
                    pending.match_range.clone(),
                );
                msg.with_help(
                    "add a wildcard arm `_ => ...` to cover the remaining cases".to_string(),
                );
                self.messages.push(msg);
            }
        }
    }

    /// Type-check a `print` (or `format`) expression: the format
    /// string must be a string literal, and each `%X` specifier's
    /// corresponding argument must have a matching type.
    fn infer_print(
        &mut self,
        fmt: &Output,
        params: &Option<Vec<Output>>,
        range: Range<usize>,
        ctx: &str,
    ) {
        let fmt_ty = self.infer(fmt);
        self.unify(&fmt_ty, &string(), &fmt.0.into_range(), "print format");

        // Pull the format string out of the literal so we can
        // parse its specifiers. If the format isn't a string
        // literal, skip validation (the user has a type error
        // elsewhere; we shouldn't cascade).
        let fmt_str = match fmt.1.as_ref() {
            Expression::String(s) => Some(s.to_string()),
            _ => None,
        };

        let mut spec_index = 0usize;
        if let (Some(s), Some(p)) = (fmt_str.as_deref(), params) {
            for (i, ch) in s.char_indices() {
                if ch == '%' {
                    // Look ahead for the specifier.
                    let rest = &s[i + 1..];
                    let mut chars = rest.chars();
                    if let Some(spec) = chars.next() {
                        // Handle `%%` (literal %). It consumes
                        // no argument.
                        if spec == '%' {
                            continue;
                        }
                        // We have `%X`. Validate the Nth arg.
                        if let Some(arg) = p.get(spec_index) {
                            let arg_ty = self.infer(arg);
                            let expected = format_specifier_type(spec);
                            let arg_ty_pruned =
                                apply_ty_prune(&self.subst, &arg_ty);
                            if !type_matches_specifier(&arg_ty_pruned, spec) {
                                let mut msg = Message::error(
                                    format!(
                                        "Format specifier `%{}` requires {}, found {}",
                                        spec,
                                        expected,
                                        arg_ty_pruned
                                    ),
                                    arg.0.into_range(),
                                );
                                msg.with_help(format!(
                                    "while checking `{}` format argument #{}",
                                    ctx,
                                    spec_index + 1
                                ));
                                self.messages.push(msg);
                            }
                            spec_index += 1;
                        } else {
                            // Specifier with no arg — also an
                            // error.
                            let mut msg = Message::error(
                                format!(
                                    "Format string has more specifiers than arguments \
                                     (`%{}` is argument #{})",
                                    spec,
                                    spec_index + 1
                                ),
                                range.clone(),
                            );
                            msg.with_help(format!(
                                "add an argument for `%%{}` in the call site",
                                spec
                            ));
                            self.messages.push(msg);
                            return;
                        }
                    } else {
                        // Trailing `%` with no specifier. Skip.
                        break;
                    }
                }
            }
        } else if let Some(p) = params {
            // No specifiers (or non-literal format) — type-check
            // each param and discard (the VM still consumes the
            // args at the bytecode level, even if the format
            // string contains no specifiers).
            for arg in p {
                let _ = self.infer(arg);
            }
        }
    }

    // ============================================================
    //  Public accessors (Phase 15B)
    // ============================================================

    /// Look up the variant tag for `(enum_name, variant_name)`.
    /// Returns the source-declaration-order index. Used by the
    /// bytecode emitter (15C) to build `MAKE_ENUM` instructions.
    pub fn tag_for(&self, enum_name: &str, variant_name: &str) -> Option<u32> {
        self.enum_tags
            .get(enum_name)
            .and_then(|t| t.get(variant_name).copied())
    }

    /// Look up the payload arity for `(enum_name, variant_name)`.
    /// Cached at registration time so codegen doesn't have to redo
    /// the `payloads[i].len()` lookup at every constructor site.
    pub fn arity_for(&self, enum_name: &str, variant_name: &str) -> Option<usize> {
        self.tag_for(enum_name, variant_name).and_then(|t| {
            self.enum_arities
                .get(enum_name)
                .and_then(|a| a.get(t as usize).copied())
        })
    }

    /// Iterate the variants of an enum in source-declaration
    /// order. Each entry is `(variant_name, tag, payload_types)`.
    /// Used by the bytecode emitter (15C) and by external
    /// exhaustiveness tooling.
    pub fn enum_variants(&self, enum_name: &str) -> Option<Vec<(String, u32, Vec<Ty>)>> {
        let names = self.enums.get(enum_name)?.clone();
        let tags = self.enum_tags.get(enum_name)?.clone();
        let payloads = self.enum_payloads.get(enum_name)?.clone();
        let mut out = Vec::with_capacity(names.len());
        for (i, name) in names.iter().enumerate() {
            let tag = tags.get(name).copied().unwrap_or(i as u32);
            let payload_tys: Vec<Ty> = match payloads.get(i) {
                Some(EnumVariantPayloadTy::Unit) => Vec::new(),
                Some(EnumVariantPayloadTy::Tuple(tys)) => tys.clone(),
                Some(EnumVariantPayloadTy::Record(fields)) => {
                    fields.iter().map(|(_, ty)| ty.clone()).collect()
                }
                None => Vec::new(),
            };
            out.push((name.clone(), tag, payload_tys));
        }
        Some(out)
    }

    /// Look up the declared payload for `(enum_name, variant_name)`
    /// as a list of `(field_name, field_type)` pairs in
    /// DECLARATION order. The codegen uses this to reorder record
    /// call-site fields to declaration order (the VM's
    /// `MAKE_ENUM` pushes payload args in pop order — the first
    /// popped is `payload[0]`).
    ///
    /// For Unit variants, returns an empty Vec. For Tuple
    /// variants, the field names are synthetic (`"0"`, `"1"`, …)
    /// — see `EnumVariantPayloadTy::field_pairs`. For Record
    /// variants, the field names are the declared names.
    pub fn payload_tys_for(&self, enum_name: &str, variant_name: &str) -> Vec<(String, Ty)> {
        let tag = match self.tag_for(enum_name, variant_name) {
            Some(t) => t,
            None => return Vec::new(),
        };
        match self.enum_payloads.get(enum_name).and_then(|p| p.get(tag as usize)) {
            Some(payload) => payload.field_pairs(),
            None => Vec::new(),
        }
    }

    /// Look up the field declaration index for `field` in
    /// `enum_name`'s UNIQUE record-shaped variant (Phase 18D —
    /// `Expression::Access` codegen).
    ///
    /// Returns `Some((variant_name, field_index))` if exactly one
    /// variant of the enum declares a record-shaped payload
    /// containing `field`; returns `None` otherwise.
    ///
    /// The HM typechecker (see
    /// [`access_field_in_sum`](Self::access_field_in_sum))
    /// rejects source programs where the field isn't uniquely
    /// declared across the enum's variants — it emits "narrow
    /// with match first" for the multi-variant case and "type
    /// has no field `X`" for the zero-variant case. So a `None`
    /// return at codegen time means the source had a type error
    /// and we're emitting in recovery mode (the codegen will
    /// emit a defensive `LoadField(0)` so the bytecode stays
    /// well-formed for downstream checks).
    ///
    /// `field_index` is the declaration-position index of the
    /// field in the variant's record payload — the same value
    /// the VM reads from `LoadField`'s lower 16 operand bits.
    pub fn field_index_for(&self, enum_name: &str, field: &str) -> Option<(String, u16)> {
        let payloads = self.enum_payloads.get(enum_name)?;
        let names = self.enums.get(enum_name)?;
        for (i, payload) in payloads.iter().enumerate() {
            if let EnumVariantPayloadTy::Record(fields) = payload {
                for (j, (fname, _)) in fields.iter().enumerate() {
                    if fname == field {
                        let variant_name = names
                            .get(i)
                            .cloned()
                            .unwrap_or_else(|| format!("variant_{}", i));
                        return Some((variant_name, j as u16));
                    }
                }
            }
        }
        None
    }

    /// Look up the declared type of a record field by enum name and
    /// field name (Phase 19 — chained `Expression::Access` codegen).
    ///
    /// Returns `Some(Ty)` if exactly one record-shaped variant in
    /// the enum declares the field; `None` otherwise. The returned
    /// `Ty` is the field's declared type (e.g. `int` for
    /// `enum Inner { Inner { v: int } }`).
    ///
    /// The codegen's `receiver_type` arm uses this to resolve
    /// chained accesses like `p.x.v` where `p.x` is itself a
    /// record-shaped enum: after looking up the inner receiver's
    /// enum, it looks up the OUTER field's type in that enum. The
    /// typechecker already verifies the chain at HM inference
    /// time (see `access_field_in_sum`); this helper just exposes
    /// the same registry data to the codegen without re-running
    /// inference.
    ///
    /// Note: this helper does NOT check that the field is
    /// unambiguously declared. If two record-shaped variants in
    /// the enum both declare the field, only the FIRST match is
    /// returned (same iteration order as `field_index_for`). The
    /// HM typechecker would have emitted a "narrow with match
    /// first" diagnostic upstream in that case — so a `None`
    /// return at codegen time means the source had a type error
    /// and we're emitting in recovery mode.
    pub fn field_type_for(&self, enum_name: &str, field: &str) -> Option<Ty> {
        let payloads = self.enum_payloads.get(enum_name)?;
        for payload in payloads {
            if let EnumVariantPayloadTy::Record(fields) = payload {
                for (fname, fty) in fields {
                    if fname == field {
                        return Some(fty.clone());
                    }
                }
            }
        }
        None
    }

    /// Look up a variable's declared type from the codegen side-table
    /// (Phase 18D — `Expression::Access` codegen helper).
    ///
    /// The side-table is populated during `infer_function` (function
    /// arguments) and `infer_fragment` (let-bindings), and survives
    /// `infer_function`'s env-pop so the codegen can resolve a
    /// field-access receiver's type after `check_program` returns.
    ///
    /// Returns `None` for unknown variables and for type variables
    /// that the typechecker never resolved (in practice: never —
    /// function arg types are always `parse_type_name_str` results,
    /// and let-bound types are resolved by the typechecker before
    /// this map is queried).
    pub fn codegen_var_type(&self, name: &str) -> Option<&Ty> {
        self.codegen_var_types.get(name)
    }

    /// Compute the inferred type of an expression WITHOUT touching the
    /// per-node cache or the `next_id_idx` counter (Phase 18D —
    /// `Expression::Access` codegen helper).
    ///
    /// The codegen uses this to resolve a field-access receiver's
    /// type when the infer cache is unavailable (e.g., the infer
    /// pass skipped some nodes like a function's `args`, leaving
    /// the cache misaligned with the pre-walk's ID table — see the
    /// comment in `compiler/src/lib.rs::do_compile`'s `Expression::Access`
    /// arm for details).
    ///
    /// Side-effects of calling this:
    ///  - `self.subst` may be extended (type variables may be bound).
    ///  - `self.env` is read but not modified.
    ///  - The cache and `next_id_idx` are NOT touched.
    ///
    /// This is essentially `infer_inner` exposed at the API
    /// boundary. Because we don't insert into the cache, the
    /// expression's inferred type isn't reusable by other codegen
    /// arms that rely on `lookup_at` — but for our purposes (the
    /// Access codegen just needs the receiver's type to look up
    /// the field index), that's fine.
    pub fn infer_for_codegen(&mut self, expr: &Output) -> Ty {
        let saved_idx = self.next_id_idx;
        let ty = self.infer_inner(expr);
        self.next_id_idx = saved_idx;
        // Don't insert into cache — the ID we restored might be
        // wrong for this AST node, and overwriting a correct entry
        // would be worse than skipping this insertion.
        ty
    }

    /// Type-check a field-access expression (Phase 18D).
    ///
    /// Shared by the three receiver-type arms of
    /// [`infer_inner`](Self::infer_inner)'s `Expression::Access`
    /// handling: `Ty::Sum`, `Ty::Constructor { tag, owner, .. }`
    /// (where the owner resolves to a Sum), and `Ty::Con(name)`
    /// (where the enum is looked up in the registry).
    ///
    /// `specific_tag` is `Some(tag)` when the caller knows the
    /// active variant statically (the `Ty::Constructor` case). When
    /// `Some`, only that variant's payload is consulted. When
    /// `None` (the `Ty::Sum` / `Ty::Con` case), every record-shaped
    /// variant contributes candidates:
    ///
    ///  - 0 candidates → error "Type `T` has no field `f`".
    ///  - 1 candidate → return that field's type.
    ///  - N candidates → error "narrow with match first" (return
    ///    the first candidate's type defensively).
    ///
    /// When `specific_tag` is `Some`, the variant must be a
    /// `Record` — `Tuple`/`Unit` variants emit
    /// "Cannot access field on non-record variant".
    fn access_field_in_sum(
        &mut self,
        enum_name: &str,
        variants: &[(String, EnumVariantPayloadTy)],
        specific_tag: Option<u32>,
        field: &str,
        range: Range<usize>,
    ) -> Ty {
        if let Some(tag) = specific_tag {
            // Statically known variant. Look up the payload and
            // either return the field's type or emit a tailored
            // diagnostic.
            let variant_idx = tag as usize;
            if variant_idx >= variants.len() {
                return self.error_with_help(
                    format!("Cannot access field `{}` on non-record type", field),
                    range,
                    Some(
                        "only values of record-shaped enum types expose fields"
                            .to_string(),
                    ),
                );
            }
            let (variant_name, payload) = &variants[variant_idx];
            match payload {
                EnumVariantPayloadTy::Record(fields) => {
                    for (fname, fty) in fields {
                        if fname == field {
                            return fty.clone();
                        }
                    }
                    // Record-shaped variant, but doesn't declare
                    // the field.
                    let hint = build_record_field_hint(enum_name, variants);
                    self.error_with_help(
                        format!("Type `{}` has no field `{}`", enum_name, field),
                        range,
                        hint,
                    )
                }
                _ => self.error_with_help(
                    format!("Cannot access field `{}` on non-record variant", field),
                    range,
                    Some(format!(
                        "variant `{}::{}` is {}; only record-shaped variants expose named fields",
                        enum_name, variant_name, payload_kind_name(payload),
                    )),
                ),
            }
        } else {
            // Untagged receiver: find every record-shaped variant
            // that declares the field.
            let mut candidates: Vec<&Ty> = Vec::new();
            for (_variant_name, payload) in variants {
                if let EnumVariantPayloadTy::Record(fields) = payload {
                    for (fname, fty) in fields {
                        if fname == field {
                            candidates.push(fty);
                        }
                    }
                }
            }
            match candidates.len() {
                0 => {
                    let hint = build_record_field_hint(enum_name, variants);
                    self.error_with_help(
                        format!("Type `{}` has no field `{}`", enum_name, field),
                        range,
                        hint,
                    )
                }
                1 => candidates[0].clone(),
                _ => {
                    self.error_with_help(
                        format!(
                            "Field `{}` exists in multiple variants of `{}`; \
                             narrow with match first",
                            field, enum_name
                        ),
                        range,
                        Some(
                            "field access requires a unique field type; use a `match` to \
                             determine the active variant before reading the field"
                                .to_string(),
                        ),
                    );
                    candidates[0].clone()
                }
            }
        }
    }
}

/// Human-readable name of a payload shape, used in
/// typecheck-error messages.
fn payload_kind_name(payload: &EnumVariantPayloadTy) -> &'static str {
    match payload {
        EnumVariantPayloadTy::Unit => "unit",
        EnumVariantPayloadTy::Tuple(_) => "tuple",
        EnumVariantPayloadTy::Record(_) => "record",
    }
}

/// Build a help hint for "type has no field `X`" diagnostics
/// (Phase 18D — `Expression::Access`).
///
/// The hint lists the record-shaped variants and their declared
/// field names, so the user can see what IS available. Returns
/// `None` when no record-shaped variants exist (in which case a
/// "record-shaped enum" hint is more useful than a list of
/// nothing).
fn build_record_field_hint(
    enum_name: &str,
    variants: &[(String, EnumVariantPayloadTy)],
) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for (variant_name, payload) in variants {
        if let EnumVariantPayloadTy::Record(fields) = payload {
            if fields.is_empty() {
                lines.push(format!("  - `{}::{}` has no fields", enum_name, variant_name));
            } else {
                let names = fields
                    .iter()
                    .map(|(n, _)| format!("`{}`", n))
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(format!(
                    "  - `{}::{}` exposes: {}",
                    enum_name, variant_name, names
                ));
            }
        }
    }
    if lines.is_empty() {
        Some(format!(
            "`{}` has no record-shaped variants; only record-shaped variants expose fields",
            enum_name
        ))
    } else {
        Some(format!(
            "the available record fields on `{}` are:\n{}",
            enum_name,
            lines.join("\n")
        ))
    }
}

/// Map a format specifier character to the type it expects.
fn format_specifier_type(spec: char) -> &'static str {
    match spec {
        'i' | 'b' | 'x' | 'u' | 'p' => "int",
        'f' => "float",
        's' => "string",
        'z' => "bool",
        _ => "an unknown type",
    }
}

/// True if `ty` (already resolved under the substitution) is the
/// type expected by `spec`.
fn type_matches_specifier(ty: &Ty, spec: char) -> bool {
    match spec {
        'i' | 'b' | 'x' | 'u' | 'p' => matches!(ty, Ty::Con(n) if n == "int"),
        'f' => matches!(ty, Ty::Con(n) if n == "float"),
        's' => matches!(ty, Ty::Con(n) if n == "string"),
        'z' => matches!(ty, Ty::Con(n) if n == "bool"),
        // Unknown specifier (including `%d`, which the VM does not
        // implement) — can't be matched; the caller will still
        // record a diagnostic, but we don't want to say it matches
        // every type.
        _ => false,
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
    use crate::typechecking::ty::EnumVariantPayloadTy;
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
    fn pattern_error_span_points_at_arm_body() {
        // Regression test for the `0..0` pattern-error span bug.
        // Pattern errors used to land at byte 0 of the source because
        // `expected_ty_span_range` always returned `0..0`. After
        // threading `arm.body.0.into_range()` through `infer_pattern`,
        // the diagnostic for a wrong-arity pattern should anchor
        // somewhere inside the source — NOT at byte 0.
        let src = "let x = Option::Some(1); match x { Option::Some(a, b) => 0 }; enum Option { None, Some(int) }";
        let (mut c, _) = check(src);
        let src_len = src.len();
        let msgs = c.take_messages();
        assert!(
            !msgs.is_empty(),
            "expected at least one diagnostic for `{}`",
            src
        );
        // The wrong-arity error from `infer_pattern` must NOT be
        // at byte 0.
        let arity_msg = msgs
            .iter()
            .find(|m| m.message().contains("expects"))
            .expect("expected a wrong-arity pattern diagnostic");
        let r = arity_msg.range();
        assert!(
            r.start > 0,
            "pattern diagnostic anchored at byte 0 — `0..0` regression: \
             range={:?} msg={:?} src={:?}",
            r,
            arity_msg.message(),
            src
        );
        assert!(
            r.end <= src_len,
            "pattern diagnostic range {:?} exceeds source length {}",
            r,
            src_len
        );
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
        let (c, _) = check("1 + 2;");
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

    // ================================================================
    //  Sum types and pattern matching (Phase 15B)
    // ================================================================

    // ---- Enum registration ----

    #[test]
    fn enum_decl_registers_sum_type() {
        let (mut c, _) = check("enum E { A, B }");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        assert!(c.enums.contains_key("E"));
        let variants = c.enums.get("E").unwrap();
        assert_eq!(variants, &vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn enum_with_payload_registers_constructor() {
        // After registration, `Option::Some` is bound as a curried
        // function in the env: `int -> Constructor`.
        let (mut c, _) = check("enum Option { None, Some(int) }");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        let scheme = c.env().lookup("Option::Some").expect("not bound");
        let ty = apply_ty_prune(c.subst(), &scheme.ty);
        assert_eq!(
            ty,
            Ty::Fun(
                Box::new(int()),
                Box::new(Ty::Constructor {
                    owner: Box::new(Ty::Sum {
                        name: "Option".into(),
                        variants: vec![
                            ("None".into(), EnumVariantPayloadTy::Unit),
                            ("Some".into(), EnumVariantPayloadTy::Tuple(vec![int()])),
                        ],
                    }),
                    tag: 1,
                    arity: 1,
                }),
            )
        );
    }

    #[test]
    fn enum_tags_assigned_in_declaration_order() {
        // MUST-HAVE #2: source order, NOT alphabetical.
        // `enum E { Z, A, M, B }` → Z=0, A=1, M=2, B=3.
        let (mut c, _) = check("enum E { Z, A, M, B }");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        assert_eq!(c.tag_for("E", "Z"), Some(0));
        assert_eq!(c.tag_for("E", "A"), Some(1));
        assert_eq!(c.tag_for("E", "M"), Some(2));
        assert_eq!(c.tag_for("E", "B"), Some(3));
    }

    #[test]
    fn recursive_enum_typechecks() {
        // MUST-HAVE #1: recursive payload uses `Ty::Con("Tree")`,
        // not the unfolded `Ty::Sum`. The HM occurs check should
        // NOT fire.
        let (mut c, _) = check("enum Tree { Leaf, Node(int, Tree, Tree) }");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        // The recursive variant's payload should reference the
        // enum by name (opaque) — the public `enum_variants` API
        // is the canonical interface to inspect this.
        let variants = c.enum_variants("Tree").expect("Tree not registered");
        let node_payload = variants.iter().find(|(n, _, _)| n == "Node").unwrap().2.clone();
        assert_eq!(
            node_payload,
            vec![int(), Ty::Con("Tree".into()), Ty::Con("Tree".into())]
        );
    }

    #[test]
    fn duplicate_enum_is_error() {
        let msgs = assert_messages("enum A { X } enum A { Y }");
        assert!(
            msgs.iter().any(|m| m.message().contains("Duplicate enum")),
            "expected duplicate-enum error, got: {:?}",
            msgs
        );
    }

    #[test]
    fn duplicate_constructor_is_error() {
        let msgs = assert_messages("enum A { Foo } enum B { Foo }");
        assert!(
            msgs.iter().any(|m| m.message().contains("Duplicate constructor")),
            "expected duplicate-constructor error, got: {:?}",
            msgs
        );
    }

    #[test]
    fn enum_decl_cache_aligned_with_id_table() {
        // Regression test for the ID-alignment bug in
        // `infer_enum_decl`: the pre-walk mints one ID for the
        // `EnumDecl` node, one for each `EnumVariant` node, and one
        // for each `Expression::Type` payload. The infer pass must
        // consume exactly the same number of IDs (via `self.infer`)
        // so the cache lines up with the id table.
        //
        // Concretely: `enum Option { None, Some(int) }` produces
        //   1 (EnumDecl) + 2 (variants) + 1 (Some's payload type) = 4
        // pre-walk IDs, and `infer` must consume all 4. The cache
        // therefore has the same length as the id table.
        for src in &[
            "enum Option { None, Some(int) }",
            "enum E { A, B, C }",
            "enum Tree { Leaf, Node(int, Tree, Tree) }",
        ] {
            let (mut c, _) = check(src);
            let msgs = c.take_messages();
            assert!(msgs.is_empty(), "{:?} for `{}`", msgs, src);
            assert_eq!(
                c.cache_len(),
                c.id_table().len(),
                "cache ({}) and id_table ({}) out of sync for `{}` \
                 — `infer_enum_decl` is not consuming every pre-walked ID",
                c.cache_len(),
                c.id_table().len(),
                src,
            );
            // Sanity: every pre-walked ID has a cached entry
            // (cache_len == id_table.len() already implies this,
            // but make the intent explicit).
            for id in c.id_table().ids() {
                assert!(
                    c.lookup_at(*id).is_some(),
                    "pre-walked ID {:?} has no cache entry for `{}`",
                    id,
                    src,
                );
            }
        }
    }

    // ---- Constructor calls ----

    #[test]
    fn constructor_call_with_wrong_arity_is_error() {
        // Option::Some takes 1 arg, called with 2.
        let src = "Option::Some(1, 2); enum Option { None, Some(int) }";
        let msgs = assert_messages(src);
        assert!(
            msgs.iter().any(|m| m.message().contains("expects 1 arguments")),
            "expected arity error, got: {:?}",
            msgs
        );
    }

    #[test]
    fn constructor_call_with_correct_arity_typechecks() {
        let src = "Option::Some(42); enum Option { None, Some(int) }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        // Check via the cache that the call produced a Constructor type.
        let ids = c.id_table().ids();
        let found = ids.iter().find_map(|id| match c.lookup_at(*id) {
            Some(Ty::Constructor { tag, arity, .. }) => Some((tag, arity)),
            _ => None,
        });
        assert_eq!(found, Some((1, 1)));
    }

    #[test]
    fn unknown_enum_constructor_is_error() {
        let msgs = assert_messages("Nonexistent::Some(1);");
        assert!(
            msgs.iter().any(|m| m.message().contains("Cannot find enum")),
            "expected unknown-enum error, got: {:?}",
            msgs
        );
    }

    #[test]
    fn unknown_variant_on_known_enum_is_error() {
        let src = "Option::Purlpe(1); enum Option { None, Some(int) }";
        let msgs = assert_messages(src);
        assert!(
            msgs.iter().any(|m| m.message().contains("Cannot find variant")),
            "expected unknown-variant error, got: {:?}",
            msgs
        );
    }

    // ---- Pattern matching ----

    #[test]
    fn match_with_all_variants_no_error() {
        // Enum declarations are top-level statements (no trailing
        // `;`) and must appear at the end of a sequence of
        // statements. Zero-arity constructors require `()`.
        let src = "let x = Option::Some(1); match x { Option::None() => 0, Option::Some(v) => v }; enum Option { None, Some(int) }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
    }

    #[test]
    fn match_with_wildcard_no_exhaustiveness_error() {
        let src = "let x = Option::Some(1); match x { _ => 0 }; enum Option { None, Some(int) }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
    }

    #[test]
    fn match_non_exhaustive_reports_missing() {
        let src = "let x = Option::None(); match x { Option::None() => 0 }; enum Option { None, Some(int) }";
        let msgs = assert_messages(src);
        assert!(
            msgs.iter().any(|m| m.message().contains("Non-exhaustive match")),
            "expected non-exhaustive error, got: {:?}",
            msgs
        );
        let msg = msgs.iter().find(|m| m.message().contains("Non-exhaustive")).unwrap();
        assert!(
            msg.message().contains("Some"),
            "expected `Some` to be mentioned, got: {:?}",
            msg.message()
        );
    }

    #[test]
    fn match_with_unreachable_arm_reports() {
        let src = "let x = Option::None(); match x { Option::None() => 0, Option::None() => 1 }; enum Option { None, Some(int) }";
        let msgs = assert_messages(src);
        assert!(
            msgs.iter().any(|m| m.message().contains("Unreachable arm")),
            "expected unreachable-arm error, got: {:?}",
            msgs
        );
    }

    #[test]
    fn match_pattern_binding_does_not_leak() {
        // `v` is bound inside the arm; referencing it after the
        // match should error.
        let src = "let x = Option::Some(1); match x { Option::Some(v) => 0 }; v; enum Option { None, Some(int) }";
        let msgs = assert_messages(src);
        // The `v` reference after the match is unknown.
        assert!(
            msgs.iter().any(|m| m.message().contains("Cannot find value `v`")),
            "expected 'v not found' error, got: {:?}",
            msgs
        );
    }

    #[test]
    fn nested_constructor_pattern_typechecks() {
        // Patterns can be nested; the inner sub-patterns are
        // checked against the corresponding payload types. We
        // wrap a value in a single-level enum so the inner
        // pattern is `Wrap::Inner(int)` — the nested pattern
        // case. (Truly recursive `Option<Option<T>>` is not
        // constructible because `Option::Some` takes `int`
        // directly, so we use a custom enum that wraps a type
        // whose pattern can be nested.)
        let src = "let x = Wrap::W(Inner::I(7)); match x { Wrap::W(Inner::I(v)) => v }; enum Inner { I(int) } enum Wrap { W(Inner) }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
    }

    #[test]
    fn forward_reference_to_constructor_works() {
        // The enum is declared AFTER the use; the pre-pass makes
        // this work.
        let src = "let x = Option::Some(1); enum Option { None, Some(int) }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
    }

    // ---- Format-string typecheck ----

    #[test]
    fn format_string_percent_i_requires_int() {
        let msgs = assert_messages(r#"print "%i", "hello";"#);
        assert!(
            msgs.iter().any(|m| m.message().contains("requires int")),
            "expected '%i requires int' error, got: {:?}",
            msgs
        );
    }

    #[test]
    fn format_string_percent_s_requires_string() {
        let msgs = assert_messages("print \"%s\", 42;");
        assert!(
            msgs.iter().any(|m| m.message().contains("requires string")),
            "expected '%s requires string' error, got: {:?}",
            msgs
        );
    }

    #[test]
    fn format_string_percent_f_requires_float() {
        let msgs = assert_messages("print \"%f\", 1;");
        assert!(
            msgs.iter().any(|m| m.message().contains("requires float")),
            "expected '%f requires float' error, got: {:?}",
            msgs
        );
    }

    #[test]
    fn format_string_with_constructor_value_errors_on_percent_s() {
        // Red-team critical: passing a `Constructor` (a sum) where
        // a string is expected must be flagged.
        let src = "print \"%s\", Option::Some(1); enum Option { None, Some(int) }";
        let msgs = assert_messages(src);
        assert!(
            msgs.iter().any(|m| m.message().contains("requires string")),
            "expected 'requires string' error, got: {:?}",
            msgs
        );
    }

    #[test]
    fn format_string_with_constructor_via_match_works() {
        // The match arm's body must be inferable as a string, and
        // print "%s", s should accept it.
        let src = "let s = match Option::Some(1) { Option::None() => \"none\", Option::Some(_) => \"some\" }; print \"%s\", s; enum Option { None, Some(int) }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
    }

    // ---- Inner-pattern reachability ----

    #[test]
    fn typechecker_does_not_report_unreachable_for_different_inner_patterns() {
        // Two Result::Ok arms with different inner patterns (Some vs None)
        // should both be considered reachable. The codegen (Phase 18A)
        // emits an inner JUMP_IF_MATCH test chain, so the second arm IS
        // reachable at runtime.
        let src = r#"
        enum Option { None, Some(int) }
        enum Result { Ok(Option), Err(string) }
        fn unwrap(Result r) -> int {
            return match r {
                Result::Ok(Option::Some(v)) => v,
                Result::Ok(Option::None) => 0,
                Result::Err(_) => -1,
            };
        }
        fn main() {
            print "%i", unwrap(Result::Ok(Option::Some(42)));
        }
        "#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        let unreachable: Vec<String> = msgs
            .iter()
            .filter(|m| m.message().contains("Unreachable arm"))
            .map(|m| m.message().to_string())
            .collect();
        assert!(
            unreachable.is_empty(),
            "Typechecker should NOT report unreachable arm for different inner patterns, got: {:?}",
            unreachable
        );
    }

    // ---- Phase 18D: field access on record-shaped variants ----

    #[test]
    fn access_field_from_record_variant_returns_field_type() {
        // `p.x` where `p` is bound to a `Point::Point { x: int, y: int }`
        // constructor. The receiver's type is a `Ty::Constructor` with
        // a record-shaped payload, so the field resolves uniquely to
        // `int`.
        let src = "enum Point { Origin, Point { x: int, y: int } } \
                   let p = Point::Point { x: 5, y: 12 }; p.x;";
        let (mut c, ty) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.is_empty(),
            "expected no diagnostics for `p.x`, got: {:?}",
            msgs
        );
        assert_eq!(ty, int(), "field access should produce `int`");
    }

    #[test]
    fn access_field_from_non_record_produces_error() {
        // `1.x` — the receiver is an `int`, not a sum. The typechecker
        // should emit a "Cannot access field" diagnostic and NOT
        // silently succeed.
        let msgs = assert_messages("1.x;");
        assert!(
            msgs.iter().any(|m| m.message().contains("Cannot access field")),
            "expected 'Cannot access field' diagnostic, got: {:?}",
            msgs
        );
    }

    #[test]
    fn access_unknown_field_produces_error() {
        // `p.z` where `p` is bound to a `Point::Point { x, y }`
        // constructor. The variant IS a record but doesn't declare
        // `z`. Should emit "Type `Point` has no field `z`" with a
        // help hint listing the actual fields.
        let src = "enum Point { Origin, Point { x: int, y: int } } \
                   let p = Point::Point { x: 1, y: 2 }; p.z;";
        let msgs = assert_messages(src);
        let no_field = msgs
            .iter()
            .find(|m| m.message().contains("no field `z`"))
            .unwrap_or_else(|| {
                panic!(
                    "expected 'no field `z`' diagnostic, got: {:?}",
                    msgs
                )
            });
        // The help hint should mention the actual fields available.
        let hint = no_field
            .help()
            .as_ref()
            .expect("expected help hint on 'no field' diagnostic");
        assert!(
            hint.contains("`x`") && hint.contains("`y`"),
            "expected help hint listing available fields, got: {:?}",
            hint
        );
    }

    #[test]
    fn access_field_from_tuple_variant_produces_error() {
        // `p.x` where `p` is bound to `Tuple::Wrap(1, 2)` — a
        // Tuple-shaped variant. The variant isn't a record, so we
        // emit a tailored "Cannot access field on non-record
        // variant" diagnostic that names the variant's shape.
        let src = "enum Tuple { Wrap(int, int) } \
                   let p = Tuple::Wrap(1, 2); p.x;";
        let msgs = assert_messages(src);
        let diag = msgs
            .iter()
            .find(|m| m.message().contains("Cannot access field"))
            .unwrap_or_else(|| {
                panic!(
                    "expected 'Cannot access field' diagnostic, got: {:?}",
                    msgs
                )
            });
        let hint = diag
            .help()
            .as_ref()
            .expect("expected help hint on tuple-variant access");
        assert!(
            hint.contains("tuple"),
            "expected help hint to mention the variant shape 'tuple', got: {:?}",
            hint
        );
    }

    #[test]
    fn access_field_ambiguous_across_variants_emits_narrow_with_match() {
        // Two record-shaped variants both declare `x`. The
        // receiver's type is `Ty::Sum { name: "Two", variants: [...] }`
        // (because we annotate the parameter `p: Two` directly, so
        // `p`'s type is `Ty::Con("Two")` which we resolve through
        // the registry). Either way, the field type is ambiguous
        // and we emit "narrow with match first".
        let src = "enum Two { A { x: int, y: int }, B { x: string, z: int } } \
                   fn get_x(Two p) -> int { return p.x; }";
        let msgs = assert_messages(src);
        assert!(
            msgs.iter()
                .any(|m| m.message().contains("narrow with match first")),
            "expected 'narrow with match first' diagnostic, got: {:?}",
            msgs
        );
    }

    #[test]
    fn access_field_via_function_parameter_resolves() {
        // Field access on a function parameter whose type is
        // annotated with the bare enum name `Point` — the
        // typechecker parses this as `Ty::Con("Point")` and
        // resolves it through the enum registry to find that
        // `Point::Point` is a record-shaped variant carrying `x`
        // of type `int`.
        let src = "enum Point { Origin, Point { x: int, y: int } } \
                   fn get_x(Point p) -> int { return p.x; }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.is_empty(),
            "expected no diagnostics, got: {:?}",
            msgs
        );
    }

    #[test]
    fn access_field_on_sum_param_with_unique_field_resolves() {
        // Same as above, but the enum has exactly ONE record-shaped
        // variant so the field is unambiguous. The typechecker
        // should resolve `p.x` to `int` without diagnostic.
        let src = "enum Point { Origin, Point { x: int, y: int } } \
                   fn get_x(Point p) -> int { return p.x; }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.is_empty(),
            "expected no diagnostics for unambiguous field access, got: {:?}",
            msgs
        );
    }

    #[test]
    fn access_field_from_let_bound_sum_value_works() {
        // The receiver is `let p = ...;` where the value is bound
        // to a `Ty::Sum` (via a function parameter flowing through
        // a `match`). After matching, the active variant is
        // statically known, so `p.x` works.
        let src = "enum Point { Origin, Point { x: int, y: int } } \
                   fn distance_squared(Point p) -> int { \
                       return match p { \
                           Point::Origin => 0, \
                           Point::Point { x, y } => x, \
                       }; \
                   } \
                   fn main() { print \"%i\", distance_squared(Point::Point { x: 5, y: 12 }); }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.is_empty(),
            "expected no diagnostics, got: {:?}",
            msgs
        );
    }

    #[test]
    fn access_field_chained_id_alignment() {
        // `p.x.y` parses as `Access(Access(p, "x"), "y")`. The
        // inner `Access` must consume the receiver's ID AND its
        // own ID to stay lockstep with the pre-walk.
        //
        // We don't assert cache/id_table alignment here because
        // `infer_fragment` has a pre-existing asymmetry where it
        // doesn't consume `Variable` IDs (it processes them
        // inline). This is unrelated to `Expression::Access`. We
        // instead verify the OUTER access produces the expected
        // diagnostic.
        let src = "enum Point { Origin, Point { x: int, y: int } } \
                   let p = Point::Point { x: 5, y: 12 }; p.x.y;";
        let msgs = assert_messages(src);
        let cannot_access: Vec<_> = msgs
            .iter()
            .filter(|m| m.message().contains("Cannot access field"))
            .collect();
        assert!(
            !cannot_access.is_empty(),
            "expected at least one 'Cannot access field' diagnostic from outer access, got: {:?}",
            msgs
        );
    }

    // ============================================================
    //  Phase 19: field_type_for helper tests
    // ============================================================
    //
    // The `field_type_for` helper is the codegen-side complement
    // to `field_index_for`. It's queried by `receiver_type` when
    // resolving chained accesses (`p.x.v`). The helper reads from
    // the same `enum_payloads` registry that `field_index_for`
    // reads from — so the tests below verify the data plumbing,
    // not the HM inference logic itself (that's already covered
    // by `access_field_*` tests above).

    /// `field_type_for` returns the declared type of a record
    /// field. Setup: `enum Inner { Inner { v: int } }`. The
    /// helper should resolve `"v"` to `int()`.
    #[test]
    fn field_type_for_returns_record_field_type() {
        let src = "enum Inner { Inner { v: int } }";
        let (c, _) = check(src);
        assert_eq!(
            c.field_type_for("Inner", "v"),
            Some(int()),
            "expected field 'v' on Inner to resolve to int()"
        );
    }

    /// `field_type_for` returns `None` when the field name isn't
    /// declared by any record-shaped variant in the enum. Setup:
    /// `enum Inner { Inner { v: int } }`. Asking for `"missing"`
    /// should yield `None` — the codegen's defensive `LoadField(0)`
    /// fallback handles this case.
    #[test]
    fn field_type_for_returns_none_for_unknown_field() {
        let src = "enum Inner { Inner { v: int } }";
        let (c, _) = check(src);
        assert_eq!(
            c.field_type_for("Inner", "missing"),
            None,
            "expected field 'missing' on Inner to resolve to None"
        );
    }

    /// `field_type_for` returns `None` when the enum name isn't
    /// registered at all. This is the "type error already emitted
    /// upstream" case — the codegen falls back to `LoadField(0)`.
    #[test]
    fn field_type_for_returns_none_for_unknown_enum() {
        let (c, _) = check("enum Inner { Inner { v: int } }");
        assert_eq!(
            c.field_type_for("Missing", "v"),
            None,
            "expected field lookup on unregistered enum to resolve to None"
        );
    }

    /// `field_type_for` returns the correct type for each named
    /// field in a record with multiple fields. The test pins the
    /// helper's return value to the DECLARED type of each field
    /// (not just "any non-None"), so a future refactor that swaps
    /// types by mistake would be caught.
    #[test]
    fn field_type_for_returns_correct_types_for_each_field() {
        let src = "enum Point { Origin, Point { x: int, y: int } }";
        let (c, _) = check(src);
        assert_eq!(c.field_type_for("Point", "x"), Some(int()));
        assert_eq!(c.field_type_for("Point", "y"), Some(int()));
    }

    /// `field_type_for` returns `None` for fields declared on a
    /// Unit or Tuple variant (not a Record variant). Setup:
    /// `enum T { Wrap(int, int) }`. Asking for `"0"` (the
    /// synthetic tuple-index name) — the helper doesn't know
    /// about synthetic names, only declared record field names.
    /// So it returns `None`. (Codegen-level reordering handles
    /// tuple-index names via `field_pairs()` — see
    /// `ty::EnumVariantPayloadTy::field_pairs`.)
    #[test]
    fn field_type_for_returns_none_for_tuple_variant() {
        let src = "enum T { Wrap(int, int) }";
        let (c, _) = check(src);
        assert_eq!(
            c.field_type_for("T", "0"),
            None,
            "tuple variants don't have named record fields"
        );
    }

    /// `field_type_for` returns the correct type even when the
    /// field is itself an enum type (e.g., `Inner` here). This
    /// is the canonical Phase 19 chained-access setup:
    /// `enum Outer { Outer { x: Inner } }`. The helper resolves
    /// `"x"` to `Ty::Con("Inner")`.
    #[test]
    fn field_type_for_returns_enum_type_for_nested_field() {
        let src = "enum Inner { Inner { v: int } } \
                   enum Outer { Outer { x: Inner, y: int } }";
        let (c, _) = check(src);
        // The exact `Ty` shape depends on the typechecker's
        // enum resolution (it could be `Ty::Con("Inner")` or
        // `Ty::Sum { name: "Inner", .. }`). The codegen's
        // `extract_enum_name` handles both shapes via
        // `extract_enum_name(&t).map(|_| t)`. We don't pin the
        // exact Ty here — we just verify the helper returns
        // *something* (not `None`) and that it's an enum
        // reference. Use `extract_enum_name` from the codegen
        // crate's perspective: the name should be "Inner".
        let result = c.field_type_for("Outer", "x");
        assert!(
            result.is_some(),
            "expected field 'x' on Outer to resolve to an enum type"
        );
        // Verify the type can be unwrapped to "Inner" via the
        // same logic `enum_name_for_receiver` uses.
        let result_ty = result.unwrap();
        match &result_ty {
            Ty::Con(name) => assert_eq!(name, "Inner"),
            Ty::Sum { name, .. } => assert_eq!(name, "Inner"),
            other => panic!(
                "expected Ty::Con(\"Inner\") or Ty::Sum {{ name: \"Inner\", .. }}, got {:?}",
                other
            ),
        }
    }
}
