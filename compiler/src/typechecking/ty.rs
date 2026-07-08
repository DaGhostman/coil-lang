//! HM types: monotypes (`Ty`), polytypes (`Scheme`), and type-variable
//! identifiers (`TyVarId`).
//!
//! This is Phase 1 of the HM rewrite (see `HM_TYPECHECKER_PLAN.md`). No
//! inference or unification lives here yet — just the data definitions and
//! free-variable helpers.
//!
//! Type variables are minted by `Checker` (added in a later phase) and only
//! remain valid for that `Checker`'s lifetime. They are stored as bare
//! `TyVarId(u32)` values, which keeps `Ty` cheap to clone (no arenas, no
//! refcounting). We can switch to an arena-based representation in a later
//! phase if shared mutable state (e.g. union-find) becomes useful.

use std::collections::HashSet;

/// Identifier for a type variable.
///
/// IDs are minted by `Checker` and are only meaningful for the lifetime of
/// that `Checker`. They are ordered by minting time so debug output is
/// stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TyVarId(pub u32);

impl TyVarId {
    /// Construct a `TyVarId` from a raw integer. Only callable from inside
    /// the typechecking module — `Checker` is responsible for minting fresh
    /// IDs.
    #[allow(dead_code)]
    pub(crate) fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// The underlying integer. Used by the pretty-printer.
    pub fn raw(self) -> u32 {
        self.0
    }
}

/// Monomorphic types in the HM type system.
///
/// - `Var(v)` is a placeholder that will be resolved during unification.
/// - `Con(name)` is a type constructor: `int`, `float`, `Foo`, …
/// - `Fun(a, b)` is a function type `a -> b`.
/// - `App(c, args)` is a type-level application, e.g. `Foo<int, string>`.
/// - `List(inner)` is sugar over `App(Con("List"), [inner])`.
/// - `Sum { name, variants }` is an algebraic sum type
///   (`enum Option { None, Some(int) }`). The variant list is the
///   source-declaration order; the `name` field is the enum's name
///   (used for diagnostic rendering and isorecursive encoding of
///   recursive payloads — see `infer::register_enum`).
/// - `Constructor { owner, tag, arity }` is the type of a specific
///   variant inside a sum. The `owner` is the parent sum type (kept
///   as a `Box` so the `Ty` is cheap to clone). `tag` is the
///   zero-based variant index in the owner's `variants` list;
///   `arity` is cached to spare codegen a per-call lookup. `arity`
///   counts the total number of fields (0 for Unit, N for Tuple/Record).
///
/// Phase 24 added three variants for typed aggregates:
/// - `Tuple(Vec<Ty>)` — heterogeneous product type `(T1, T2, ...)`. Each
///   element has its own (potentially distinct) type. The vec is in
///   source/declaration order. The arity is fixed (length is type-level).
/// - `Array { element, length }` — homogeneous collection `[T]` or `[T; N]`.
///   `length` is `ArrayLength::Static(N)` for a compile-time-known length
///   (literal `[1, 2, 3]` or `[int; 5]` annotation) and `ArrayLength::Dynamic`
///   for runtime-determined length (function return, parameter, etc.).
///   Static-length arrays enable compile-time out-of-bounds detection.
/// - `Record { fields }` — anonymous dict `{ name: T, name: T, ... }`.
///   Field names are unique within a record; structurally-equal field
///   sets unify. Dicts are mutable (Phase 25). The fields are in
///   declaration order at construction; the typechecker canonically
///   sorts them lex-by-name for unification determinism.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    Var(TyVarId),
    Con(String),
    Fun(Box<Ty>, Box<Ty>),
    App(Box<Ty>, Vec<Ty>),
    List(Box<Ty>),
    Sum {
        name: String,
        variants: Vec<(String, EnumVariantPayloadTy)>,
    },
    Constructor {
        owner: Box<Ty>,
        tag: u32,
        arity: usize,
    },
    /// `(T1, T2, ..., Tn)` — heterogeneous tuple. Length is fixed.
    Tuple(Vec<Ty>),
    /// `[T]` (dynamic length) or `[T; N]` (static length N).
    Array {
        element: Box<Ty>,
        length: ArrayLength,
    },
    /// `{ name: T, ... }` — anonymous dict / record. Structurally typed
    /// (Phase 25). Mutable.
    Record {
        fields: Vec<(String, Ty)>,
    },
}

/// The length component of `Ty::Array`. `Static(N)` makes the array's
/// length a type-level constant (compile-time known) and lets the
/// typechecker flag constant out-of-bounds indices. `Dynamic` is for
/// arrays whose length is only known at runtime (function returns,
/// JSON-decoded arrays, SQL results — Phase 24 user requirement).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrayLength {
    Static(usize),
    Dynamic,
}

impl ArrayLength {
    /// True iff this length is known at compile time. Used by the
    /// typechecker's index-out-of-bounds check (Phase 24).
    pub fn is_static(&self) -> bool {
        matches!(self, ArrayLength::Static(_))
    }
}

/// The payload shape of a single `Ty::Sum` variant. Phase 17B
/// introduced this as an EXPLICIT shape enum (not a synthetic-name
/// trick — see the 17B red-team finding #1). The shape is preserved
/// end-to-end through unification, pretty-printing, and codegen
/// reordering.
///
/// Field naming rules:
///
/// - `Unit` — no fields.
/// - `Tuple(Vec<Ty>)` — positional types, in declaration order.
/// - `Record(Vec<(String, Ty)>)` — `(field_name, field_type)` pairs,
///   in declaration order. Field names are needed for matching
///   record-pattern bindings to their declaration-order slot
///   positions and for typechecker shape-mismatch errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnumVariantPayloadTy {
    Unit,
    Tuple(Vec<Ty>),
    Record(Vec<(String, Ty)>),
}

impl EnumVariantPayloadTy {
    /// Number of fields (0 for Unit, N for Tuple/Record). Used by
    /// codegen to know how many stack values to push/pop.
    pub fn field_count(&self) -> usize {
        match self {
            EnumVariantPayloadTy::Unit => 0,
            EnumVariantPayloadTy::Tuple(tys) => tys.len(),
            EnumVariantPayloadTy::Record(fields) => fields.len(),
        }
    }

    /// Iterate the field types in declaration order. Used by the
    /// typechecker to unify record-pattern / record-call-site
    /// shapes against the declared shape (declaration order).
    pub fn field_types(&self) -> Vec<&Ty> {
        match self {
            EnumVariantPayloadTy::Unit => Vec::new(),
            EnumVariantPayloadTy::Tuple(tys) => tys.iter().collect(),
            EnumVariantPayloadTy::Record(fields) => fields.iter().map(|(_, ty)| ty).collect(),
        }
    }

    /// `(field_name, field_type)` pairs in declaration order. The
    /// bridge between Tuple and Record for codegen reordering —
    /// tuple variants get synthetic names `"0"`, `"1"`, …; record
    /// variants get their declared names. This helper is used ONLY
    /// at the codegen level (see 17B red-team finding #1: not in
    /// Display, unify, or the AST data structure).
    pub fn field_pairs(&self) -> Vec<(String, Ty)> {
        match self {
            EnumVariantPayloadTy::Unit => Vec::new(),
            EnumVariantPayloadTy::Tuple(tys) => tys
                .iter()
                .enumerate()
                .map(|(i, ty)| (i.to_string(), ty.clone()))
                .collect(),
            EnumVariantPayloadTy::Record(fields) => fields.clone(),
        }
    }
}

impl Ty {
    /// Convenience constructor for a type constructor from a static name.
    pub fn con(name: &'static str) -> Ty {
        Ty::Con(name.to_string())
    }
}

/// A type scheme: a type possibly quantified over some type variables.
///
/// `Scheme { bounds: vec![], ty: Var(α) }` represents `α` (with no
/// quantification; equivalently `∀α. α` if we later add `α` to `bounds`).
/// `bounds` is reserved for future use (e.g. type-class constraints).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scheme {
    pub bounds: Vec<TyVarId>,
    pub ty: Ty,
}

impl Scheme {
    /// Wrap a type as a monomorphic scheme (no quantified variables).
    pub fn mono(ty: Ty) -> Self {
        Self {
            bounds: Vec::new(),
            ty,
        }
    }
}

// --- Built-in type constructors (as static name strings) ---

/// Name of the `int` type constructor.
pub const INT: &str = "int";
/// Name of the `float` type constructor.
pub const FLOAT: &str = "float";
/// Name of the `string` type constructor.
pub const STRING: &str = "string";
/// Name of the `bool` type constructor.
pub const BOOL: &str = "bool";
/// Name of the `unit` type constructor.
pub const UNIT: &str = "unit";
/// Name of the `List` type constructor.
#[allow(dead_code)] // reserved for future list-type support
pub const LIST: &str = "List";

/// Build the `int` type.
pub fn int() -> Ty {
    Ty::Con(INT.into())
}

/// Build the `float` type.
pub fn float() -> Ty {
    Ty::Con(FLOAT.into())
}

/// Build the `string` type.
pub fn string() -> Ty {
    Ty::Con(STRING.into())
}

/// Build the `bool` type.
pub fn boolean() -> Ty {
    Ty::Con(BOOL.into())
}

/// Build the `unit` type (used for side-effecting expressions, `print`, etc.).
pub fn unit() -> Ty {
    Ty::Con(UNIT.into())
}

/// Build the `List<t>` type.
pub fn list(inner: Ty) -> Ty {
    Ty::List(Box::new(inner))
}

/// Build the `(T1, T2, ..., Tn)` heterogeneous tuple type.
pub fn tuple(tys: Vec<Ty>) -> Ty {
    Ty::Tuple(tys)
}

/// Build the `[T]` (dynamic-length) array type.
pub fn array(element: Ty) -> Ty {
    Ty::Array {
        element: Box::new(element),
        length: ArrayLength::Dynamic,
    }
}

/// Build the `[T; N]` (fixed-length) array type.
pub fn array_fixed(element: Ty, length: usize) -> Ty {
    Ty::Array {
        element: Box::new(element),
        length: ArrayLength::Static(length),
    }
}

/// Build the `{ name: T, ... }` anonymous record type.
pub fn record(fields: Vec<(String, Ty)>) -> Ty {
    Ty::Record { fields }
}

// --- Free type variables ---

/// Free type variables of a `Ty`.
pub fn ftv_ty(ty: &Ty) -> HashSet<TyVarId> {
    let mut acc = HashSet::new();
    go(ty, &mut acc);
    acc
}

fn go(ty: &Ty, acc: &mut HashSet<TyVarId>) {
    match ty {
        Ty::Var(v) => {
            acc.insert(*v);
        }
        Ty::Con(_) => {}
        Ty::Fun(a, b) => {
            go(a, acc);
            go(b, acc);
        }
        Ty::App(_, args) => {
            for a in args {
                go(a, acc);
            }
        }
        Ty::List(inner) => {
            go(inner, acc);
        }
        // Sum types: the `name` field carries no free variables.
        // Variant payload types may be polymorphic (e.g. `enum Pair
        // { P(int, T) }` where T is fresh) so we walk them.
        // Recursive payloads use `Ty::Con("EnumName")` for the
        // self-reference, which has no free variables — so the
        // recursion terminates without an explicit depth check.
        Ty::Sum { variants, .. } => {
            for (_, payload) in variants {
                for p in payload.field_types() {
                    go(p, acc);
                }
            }
        }
        // Constructor types: the tag and arity are inert; the
        // interesting part is the owner (which is always the parent
        // sum type).
        Ty::Constructor { owner, .. } => {
            go(owner, acc);
        }
        // Phase 24 aggregate types. Tuple elements and Array
        // elements contribute free variables; Record fields
        // contribute free variables; the lengths and field NAMES are
        // inert (no free vars).
        Ty::Tuple(tys) => {
            for t in tys {
                go(t, acc);
            }
        }
        Ty::Array { element, .. } => {
            go(element, acc);
        }
        Ty::Record { fields } => {
            for (_, fty) in fields {
                go(fty, acc);
            }
        }
    }
}

/// Free type variables of a `Scheme` (excluding the quantified ones).
pub fn ftv_scheme(s: &Scheme) -> HashSet<TyVarId> {
    let mut acc = ftv_ty(&s.ty);
    let bound: HashSet<_> = s.bounds.iter().copied().collect();
    acc.retain(|v| !bound.contains(v));
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(i: u32) -> Ty {
        Ty::Var(TyVarId(i))
    }

    #[test]
    fn ftv_of_var_is_just_that_var() {
        assert_eq!(ftv_ty(&v(0)), HashSet::from([TyVarId(0)]));
    }

    #[test]
    fn ftv_of_con_is_empty() {
        assert!(ftv_ty(&int()).is_empty());
        assert!(ftv_ty(&float()).is_empty());
    }

    #[test]
    fn ftv_of_fun_is_union_of_args() {
        let ty = Ty::Fun(Box::new(v(0)), Box::new(v(1)));
        assert_eq!(ftv_ty(&ty), HashSet::from([TyVarId(0), TyVarId(1)]));
    }

    #[test]
    fn ftv_of_app_walks_args() {
        let ty = Ty::App(Box::new(Ty::Con("Foo".into())), vec![v(0), v(2)]);
        assert_eq!(ftv_ty(&ty), HashSet::from([TyVarId(0), TyVarId(2)]));
    }

    #[test]
    fn ftv_of_list_walks_inner() {
        let ty = list(v(3));
        assert_eq!(ftv_ty(&ty), HashSet::from([TyVarId(3)]));
    }

    #[test]
    fn ftv_of_nested_fun_dedups() {
        let ty = Ty::Fun(
            Box::new(v(0)),
            Box::new(Ty::Fun(Box::new(v(0)), Box::new(v(1)))),
        );
        assert_eq!(ftv_ty(&ty), HashSet::from([TyVarId(0), TyVarId(1)]));
    }

    #[test]
    fn ftv_of_scheme_excludes_bounds() {
        let scheme = Scheme {
            bounds: vec![TyVarId(0)],
            ty: v(0),
        };
        assert!(ftv_scheme(&scheme).is_empty());
    }

    #[test]
    fn ftv_of_scheme_keeps_non_bound_vars() {
        let scheme = Scheme {
            bounds: vec![TyVarId(0)],
            ty: Ty::Fun(Box::new(v(0)), Box::new(v(1))),
        };
        assert_eq!(ftv_scheme(&scheme), HashSet::from([TyVarId(1)]));
    }

    #[test]
    fn scheme_mono_has_no_bounds() {
        let s = Scheme::mono(int());
        assert!(s.bounds.is_empty());
        assert_eq!(s.ty, int());
    }

    // ---- Sum / Constructor ----

    #[test]
    fn ftv_of_sum_walks_variant_payloads() {
        // enum E { A(int), B(string) }  — ftv is the union of the
        // payload free variables (here, just the vars inside the
        // payloads).
        let sum = Ty::Sum {
            name: "E".into(),
            variants: vec![
                ("A".into(), EnumVariantPayloadTy::Tuple(vec![v(0)])),
                ("B".into(), EnumVariantPayloadTy::Tuple(vec![string()])),
            ],
        };
        assert_eq!(ftv_ty(&sum), HashSet::from([TyVarId(0)]));
    }

    #[test]
    fn ftv_of_constructor_walks_owner() {
        // The owner of a Constructor is the parent sum. Its ftv is
        // the union of the owner's variant-payload ftvs.
        let sum = Ty::Sum {
            name: "E".into(),
            variants: vec![("A".into(), EnumVariantPayloadTy::Tuple(vec![v(1), v(2)]))],
        };
        let ctor = Ty::Constructor {
            owner: Box::new(sum),
            tag: 0,
            arity: 2,
        };
        assert_eq!(ftv_ty(&ctor), HashSet::from([TyVarId(1), TyVarId(2)]));
    }

    #[test]
    fn ftv_of_recursive_sum_is_empty_when_payloads_use_con() {
        // `enum Tree { Leaf, Node(int, Tree, Tree) }` — the
        // recursive reference inside `Node` is `Ty::Con("Tree")`
        // (the isorecursive encoding, see `register_enum`), which
        // has no free vars. So the entire sum has no free vars.
        let tree = Ty::Con("Tree".into());
        let sum = Ty::Sum {
            name: "Tree".into(),
            variants: vec![
                ("Leaf".into(), EnumVariantPayloadTy::Unit),
                (
                    "Node".into(),
                    EnumVariantPayloadTy::Tuple(vec![int(), tree.clone(), tree]),
                ),
            ],
        };
        assert!(ftv_ty(&sum).is_empty());
    }

    // ---- EnumVariantPayloadTy ----

    #[test]
    fn payload_field_count_unit() {
        assert_eq!(EnumVariantPayloadTy::Unit.field_count(), 0);
    }

    #[test]
    fn payload_field_count_tuple() {
        let p = EnumVariantPayloadTy::Tuple(vec![int(), string()]);
        assert_eq!(p.field_count(), 2);
    }

    #[test]
    fn payload_field_count_record() {
        let p = EnumVariantPayloadTy::Record(vec![("x".into(), int()), ("y".into(), string())]);
        assert_eq!(p.field_count(), 2);
    }

    #[test]
    fn payload_field_types_unit() {
        assert!(EnumVariantPayloadTy::Unit.field_types().is_empty());
    }

    #[test]
    fn payload_field_types_tuple() {
        let p = EnumVariantPayloadTy::Tuple(vec![int(), string()]);
        assert_eq!(p.field_types(), vec![&int(), &string()]);
    }

    #[test]
    fn payload_field_types_record() {
        let p = EnumVariantPayloadTy::Record(vec![("x".into(), int()), ("y".into(), string())]);
        assert_eq!(p.field_types(), vec![&int(), &string()]);
    }

    #[test]
    fn payload_field_pairs_unit() {
        assert!(EnumVariantPayloadTy::Unit.field_pairs().is_empty());
    }

    #[test]
    fn payload_field_pairs_tuple_uses_synthetic_names() {
        // Tuple `Foo(int, int)` → field_pairs returns
        // `[("0", int), ("1", int)]` (synthetic names — used by
        // codegen reordering). This is the ONLY place the
        // synthetic-name trick is applied.
        let p = EnumVariantPayloadTy::Tuple(vec![int(), int()]);
        assert_eq!(
            p.field_pairs(),
            vec![("0".into(), int()), ("1".into(), int())]
        );
    }

    #[test]
    fn payload_field_pairs_record_keeps_declared_names() {
        let p = EnumVariantPayloadTy::Record(vec![("x".into(), int()), ("y".into(), int())]);
        assert_eq!(
            p.field_pairs(),
            vec![("x".into(), int()), ("y".into(), int())]
        );
    }
}
