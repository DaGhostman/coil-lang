//! Unification for Hindley–Milner type inference.
//!
//! Implements Robinson's algorithm with occurs check. [`unify_with`] takes
//! two types and an existing substitution, and returns an extended
//! substitution that makes the two types equal; if no such substitution
//! exists, it returns a [`UnifyError`].
//!
//! ## Algorithm
//!
//! 1. Bring both types up to date by applying the current substitution
//!    (single lookup per variable). This means we always see the most
//!    recent bindings on both sides.
//! 2. Decompose the pair:
//!    - Same constructor (`int` with `int`, `Fun(a, b)` with `Fun(c, d)`):
//!      unify sub-pairs in sequence, threading the accumulated
//!      substitution.
//!    - `Var(α)` on either side: bind `α` to the other type, after the
//!      occurs check.
//!    - Two identical `Var`s: trivially equal (no occurs check).
//!    - Different constructors: `Mismatch`.
//!
//! `Var(α)` with itself succeeds without an occurs check.
//!
//! ## Errors
//!
//! - `Mismatch { left, right }`: the constructors are fundamentally
//!   incompatible (`int` with `float`, `Foo` with `Bar`, mismatched arity
//!   on `App`, …).
//! - `Occurs { var, ty }`: binding `var` to `ty` would create an infinite
//!   type (`α = α -> α`, `α = List<α>`, …).
//!
//! Spans are deliberately not carried in `UnifyError`. The infer pass
//! (Phase 4) tracks which expression caused a unification failure and
//! attaches the span at the diagnostic layer (Phase 8).

use super::subst::{apply_ty, compose, Subst};
use super::ty::{ftv_ty, Ty, TyVarId};

/// Failure modes for unification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnifyError {
    /// Two non-variable types that cannot be unified.
    Mismatch { left: Ty, right: Ty },
    /// `var` occurs in `ty`, so unifying would create an infinite type.
    Occurs { var: TyVarId, ty: Ty },
}

impl std::fmt::Display for UnifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnifyError::Mismatch { left, right } => {
                write!(f, "cannot unify {} with {}", left, right)
            }
            UnifyError::Occurs { var, ty } => {
                write!(f, "occurs check failed: t{} occurs in {}", var.raw(), ty)
            }
        }
    }
}

impl std::error::Error for UnifyError {}

/// Unify two types, starting from the empty substitution.
///
/// Convenience wrapper over [`unify_with`] for tests and one-shot
/// unifications.
#[allow(dead_code)] // exposed for tests and one-shot use
pub fn unify(t1: &Ty, t2: &Ty) -> Result<Subst, UnifyError> {
    unify_with(&Subst::empty(), t1, t2)
}

/// Unify two types under an existing substitution.
///
/// Returns the extended substitution. The input `subst` is left
/// unchanged.
pub fn unify_with(subst: &Subst, t1: &Ty, t2: &Ty) -> Result<Subst, UnifyError> {
    // Bring both sides up to date with the current substitution so we
    // always see the most recent bindings when decomposing.
    let t1 = apply_ty(subst, t1);
    let t2 = apply_ty(subst, t2);

    match (t1, t2) {
        // Identical type variables: trivially equal (no occurs check).
        (Ty::Var(v1), Ty::Var(v2)) if v1 == v2 => Ok(subst.clone()),

        // Same type constructor (e.g. `int` with `int`).
        (Ty::Con(a), Ty::Con(b)) if a == b => Ok(subst.clone()),

        // Function types: unify arg and return in sequence.
        (Ty::Fun(a1, b1), Ty::Fun(a2, b2)) => {
            let s = unify_with(subst, a1.as_ref(), a2.as_ref())?;
            unify_with(&s, b1.as_ref(), b2.as_ref())
        }

        // List types: unify the element type.
        (Ty::List(a), Ty::List(b)) => unify_with(subst, a.as_ref(), b.as_ref()),

        // Type applications: must have the same constructor and matching
        // arity; then unify args pairwise.
        (Ty::App(c1, args1), Ty::App(c2, args2)) => {
            let s = unify_with(subst, c1.as_ref(), c2.as_ref())?;
            if args1.len() != args2.len() {
                return Err(UnifyError::Mismatch {
                    left: Ty::App(c1, args1),
                    right: Ty::App(c2, args2),
                });
            }
            let mut current = s;
            for (a, b) in args1.iter().zip(args2.iter()) {
                current = unify_with(&current, a, b)?;
            }
            Ok(current)
        }

        // Type variable on either side: bind, with occurs check.
        (Ty::Var(v), t) => bind_var(subst, v, t),
        (t, Ty::Var(v)) => bind_var(subst, v, t),

        // Anything else: the constructors are incompatible.
        (left, right) => Err(UnifyError::Mismatch { left, right }),
    }
}

/// Bind a type variable to a type, with occurs check.
///
/// `subst` is extended with `var → ty` (the new binding is composed so
/// that subsequent unifications see the value already resolved under
/// `subst`).
fn bind_var(subst: &Subst, var: TyVarId, ty: Ty) -> Result<Subst, UnifyError> {
    if ftv_ty(&ty).contains(&var) {
        return Err(UnifyError::Occurs { var, ty });
    }
    let new_binding = Subst::singleton(var, ty);
    // `compose(subst, new_binding)` applies `new_binding` first, then
    // `subst` — which means the new binding's value is eagerly resolved
    // under `subst` before subsequent unifications see it.
    Ok(compose(subst, &new_binding))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typechecking::subst::apply_ty_prune;
    use crate::typechecking::ty::{boolean, float, int, list, string};

    fn v(i: u32) -> Ty {
        Ty::Var(TyVarId(i))
    }

    fn fun(a: Ty, b: Ty) -> Ty {
        Ty::Fun(Box::new(a), Box::new(b))
    }

    // ---- Basic success cases ----

    #[test]
    fn unify_same_constructor_succeeds() {
        assert_eq!(unify(&int(), &int()).unwrap(), Subst::empty());
        assert_eq!(unify(&float(), &float()).unwrap(), Subst::empty());
        assert_eq!(unify(&string(), &string()).unwrap(), Subst::empty());
        assert_eq!(unify(&boolean(), &boolean()).unwrap(), Subst::empty());
    }

    #[test]
    fn unify_var_with_constructor_binds() {
        let s = unify(&v(0), &int()).unwrap();
        assert_eq!(apply_ty(&s, &v(0)), int());
    }

    #[test]
    fn unify_constructor_with_var_binds() {
        let s = unify(&int(), &v(0)).unwrap();
        assert_eq!(apply_ty(&s, &v(0)), int());
    }

    #[test]
    fn unify_var_with_itself_succeeds() {
        // Same variable on both sides — trivially equal, no occurs check.
        assert_eq!(unify(&v(0), &v(0)).unwrap(), Subst::empty());
        assert_eq!(unify(&v(42), &v(42)).unwrap(), Subst::empty());
    }

    #[test]
    fn unify_two_different_vars_binds_left_to_right() {
        // v(0) is on the left, so we bind v(0) → v(1). v(1) is unchanged.
        let s = unify(&v(0), &v(1)).unwrap();
        assert_eq!(apply_ty(&s, &v(0)), v(1));
        assert_eq!(apply_ty(&s, &v(1)), v(1));
    }

    // ---- Failure: mismatch ----

    #[test]
    fn unify_different_constructors_is_mismatch() {
        let err = unify(&int(), &float()).unwrap_err();
        assert!(matches!(err, UnifyError::Mismatch { .. }));
    }

    #[test]
    fn unify_fun_with_int_is_mismatch() {
        let err = unify(&fun(int(), string()), &int()).unwrap_err();
        assert!(matches!(err, UnifyError::Mismatch { .. }));
    }

    #[test]
    fn unify_list_with_non_list_is_mismatch() {
        let err = unify(&list(int()), &int()).unwrap_err();
        assert!(matches!(err, UnifyError::Mismatch { .. }));
    }

    #[test]
    fn unify_app_arity_mismatch_is_mismatch() {
        let foo = Ty::con("Foo");
        let err = unify(
            &Ty::App(Box::new(foo.clone()), vec![v(0)]),
            &Ty::App(Box::new(foo.clone()), vec![int(), boolean()]),
        )
        .unwrap_err();
        assert!(matches!(err, UnifyError::Mismatch { .. }));
    }

    #[test]
    fn unify_app_constructor_mismatch_is_mismatch() {
        let foo = Ty::con("Foo");
        let bar = Ty::con("Bar");
        let err = unify(
            &Ty::App(Box::new(foo), vec![int()]),
            &Ty::App(Box::new(bar), vec![int()]),
        )
        .unwrap_err();
        assert!(matches!(err, UnifyError::Mismatch { .. }));
    }

    // ---- Failure: occurs check ----

    #[test]
    fn occurs_check_rejects_alpha_equals_alpha_to_alpha() {
        // α = α -> α
        let err = unify(&v(0), &fun(v(0), v(0))).unwrap_err();
        assert!(matches!(err, UnifyError::Occurs { .. }));
    }

    #[test]
    fn occurs_check_rejects_var_inside_fun_arg() {
        // α = (α -> int) -> int
        let err = unify(&v(0), &fun(fun(v(0), int()), int())).unwrap_err();
        assert!(matches!(err, UnifyError::Occurs { .. }));
    }

    #[test]
    fn occurs_check_rejects_var_inside_list() {
        // α = List<α>
        let err = unify(&v(0), &list(v(0))).unwrap_err();
        assert!(matches!(err, UnifyError::Occurs { .. }));
    }

    #[test]
    fn occurs_check_rejects_var_inside_app_arg() {
        let foo = Ty::con("Foo");
        let err = unify(&v(0), &Ty::App(Box::new(foo), vec![v(0)])).unwrap_err();
        assert!(matches!(err, UnifyError::Occurs { .. }));
    }

    #[test]
    fn occurs_check_does_not_fire_on_independent_vars() {
        // α = β -> γ : should succeed (α is fresh).
        let s = unify(&v(0), &fun(v(1), v(2))).unwrap();
        assert_eq!(apply_ty(&s, &v(0)), fun(v(1), v(2)));
    }

    // ---- Decomposition ----

    #[test]
    fn unify_fun_decomposes_into_args_and_return() {
        // (α -> β) ~ (int -> bool) binds α = int, β = bool.
        let s = unify(&fun(v(0), v(1)), &fun(int(), boolean())).unwrap();
        assert_eq!(apply_ty(&s, &v(0)), int());
        assert_eq!(apply_ty(&s, &v(1)), boolean());
    }

    #[test]
    fn unify_list_decomposes() {
        let s = unify(&list(v(0)), &list(int())).unwrap();
        assert_eq!(apply_ty(&s, &v(0)), int());
    }

    #[test]
    fn unify_app_decomposes() {
        // Foo<α> ~ Foo<int>
        let foo = Ty::con("Foo");
        let s = unify(
            &Ty::App(Box::new(foo.clone()), vec![v(0)]),
            &Ty::App(Box::new(foo), vec![int()]),
        )
        .unwrap();
        assert_eq!(apply_ty(&s, &v(0)), int());
    }

    #[test]
    fn unify_nested_fun_decomposes_recursively() {
        // ((α -> β) -> γ) ~ ((int -> bool) -> string)
        let lhs = fun(fun(v(0), v(1)), v(2));
        let rhs = fun(fun(int(), boolean()), string());
        let s = unify(&lhs, &rhs).unwrap();
        assert_eq!(apply_ty(&s, &v(0)), int());
        assert_eq!(apply_ty(&s, &v(1)), boolean());
        assert_eq!(apply_ty(&s, &v(2)), string());
    }

    // ---- With existing substitution ----

    #[test]
    fn unify_with_existing_subst_extends_it() {
        // Start with γ = string, unify v(0) with v(2). v(0) should
        // resolve to string.
        let s0 = Subst::singleton(TyVarId(2), string());
        let s = unify_with(&s0, &v(0), &v(2)).unwrap();
        assert_eq!(apply_ty(&s, &v(0)), string());
        assert_eq!(apply_ty(&s, &v(2)), string());
    }

    #[test]
    fn unify_resolves_both_sides_under_existing_subst() {
        // Starting with γ = string, unify Fun(γ, α) with Fun(string, bool).
        // α should resolve to bool; γ already in s0.
        let s0 = Subst::singleton(TyVarId(2), string());
        let s = unify_with(&s0, &fun(v(2), v(0)), &fun(string(), boolean())).unwrap();
        assert_eq!(apply_ty(&s, &v(0)), boolean());
        assert_eq!(apply_ty(&s, &v(2)), string());
    }

    // ---- Algorithm-W-like chaining ----

    #[test]
    fn chained_unifications_propagate_through_subst() {
        // γ ~ α ; α ~ β ; β ~ int
        let s1 = unify(&v(2), &v(0)).unwrap();
        let s2 = unify_with(&s1, &v(0), &v(1)).unwrap();
        let s3 = unify_with(&s2, &v(1), &int()).unwrap();

        // Single-apply sees the next variable in the chain.
        assert_eq!(apply_ty(&s3, &v(0)), v(1));
        assert_eq!(apply_ty(&s3, &v(1)), int());
        assert_eq!(apply_ty(&s3, &v(2)), v(0));

        // Pruned apply sees the fully resolved type.
        assert_eq!(apply_ty_prune(&s3, &v(0)), int());
        assert_eq!(apply_ty_prune(&s3, &v(1)), int());
        assert_eq!(apply_ty_prune(&s3, &v(2)), int());
    }

    #[test]
    fn unify_propagates_through_aliased_vars() {
        // v(0) ~ v(1); then v(0) ~ int.
        let s1 = unify(&v(0), &v(1)).unwrap();
        let s2 = unify_with(&s1, &v(0), &int()).unwrap();

        // Single-apply leaves the alias intact: v(0) is bound to v(1),
        // which is itself bound to int. To get the final type the caller
        // has to reapplied (apply_ty_prune).
        assert_eq!(apply_ty(&s2, &v(0)), v(1));
        assert_eq!(apply_ty(&s2, &v(1)), int());

        // Pruned apply follows the alias chain.
        assert_eq!(apply_ty_prune(&s2, &v(0)), int());
        assert_eq!(apply_ty_prune(&s2, &v(1)), int());
    }

    // ---- Idempotence invariant ----

    fn is_idempotent(s: &Subst) -> bool {
        for (var, ty) in s.iter() {
            if ftv_ty(ty).contains(&var) {
                return false;
            }
        }
        true
    }

    #[test]
    fn result_is_idempotent_simple() {
        let s = unify(&fun(v(0), v(1)), &fun(int(), boolean())).unwrap();
        assert!(is_idempotent(&s), "substitution should be idempotent: {s:?}");
    }

    #[test]
    fn result_is_idempotent_nested_fun() {
        let s = unify(&v(0), &fun(v(1), v(2))).unwrap();
        assert!(is_idempotent(&s), "substitution should be idempotent: {s:?}");
    }

    #[test]
    fn result_is_idempotent_chained() {
        let s1 = unify(&v(2), &v(0)).unwrap();
        let s2 = unify_with(&s1, &v(0), &v(1)).unwrap();
        let s3 = unify_with(&s2, &v(1), &int()).unwrap();
        assert!(is_idempotent(&s3), "substitution should be idempotent: {s3:?}");
    }
}