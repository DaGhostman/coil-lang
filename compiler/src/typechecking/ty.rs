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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    Var(TyVarId),
    Con(String),
    Fun(Box<Ty>, Box<Ty>),
    App(Box<Ty>, Vec<Ty>),
    List(Box<Ty>),
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
        let ty = Ty::Fun(Box::new(v(0)), Box::new(Ty::Fun(Box::new(v(0)), Box::new(v(1)))));
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
}