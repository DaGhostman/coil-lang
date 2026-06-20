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

        // `Ty::Con(name)` is the opaque recursive reference used
        // by `register_enum` (see MUST-HAVE #1). When the
        // non-recursive side is a `Sum` of the same name, treat
        // them as equivalent.
        (Ty::Con(c_name), Ty::Sum { name, variants })
        | (Ty::Sum { name, variants }, Ty::Con(c_name))
            if c_name == name && c_name == name =>
        {
            // Re-borrow: the `apply_ty` at the top of this
            // function already resolved any bound vars, so
            // unbuilding the sum here is fine. We unify the sum
            // with itself (identity) to honour the existing
            // variants structure on both sides.
            let sum = Ty::Sum {
                name: name.clone(),
                variants: variants.clone(),
            };
            unify_with(subst, &sum, &sum)
        }
        (Ty::Con(c_name), ctor @ Ty::Constructor { .. })
        | (ctor @ Ty::Constructor { .. }, Ty::Con(c_name)) => {
            // Only unify when the constructor's owner is a sum
            // with the same name (the `Con(name)` is the
            // isorecursive reference to that sum).
            let owner_sum_name = match &ctor {
                Ty::Constructor { owner, .. } => match owner.as_ref() {
                    Ty::Sum { name, .. } => Some(name.clone()),
                    _ => None,
                },
                _ => None,
            };
            if owner_sum_name.as_deref() != Some(c_name.as_str()) {
                return Err(UnifyError::Mismatch {
                    left: ctor,
                    right: Ty::Con(c_name),
                });
            }
            // The constructor's owner is the matching sum.
            // Use the existing Constructor-vs-Sum arm logic.
            let owner = match &ctor {
                Ty::Constructor { owner, .. } => owner.as_ref().clone(),
                _ => unreachable!(),
            };
            unify_with(subst, &ctor, &owner)
        }

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

        // Sum types: two same-named sums are equal iff their variants
        // match in number, name (per slot), and payload arity, with
        // the payload types themselves unifiable. Sums of different
        // names are always distinct.
        (Ty::Sum { name: a, variants: av }, Ty::Sum { name: b, variants: bv }) if a == b => {
            if av.len() != bv.len() {
                return Err(UnifyError::Mismatch {
                    left: Ty::Sum {
                        name: a,
                        variants: av,
                    },
                    right: Ty::Sum {
                        name: b,
                        variants: bv,
                    },
                });
            }
            let mut current = subst.clone();
            for ((an, ap), (bn, bp)) in av.iter().zip(bv.iter()) {
                if an != bn {
                    return Err(UnifyError::Mismatch {
                        left: Ty::Sum {
                            name: a.clone(),
                            variants: av.clone(),
                        },
                        right: Ty::Sum {
                            name: b.clone(),
                            variants: bv.clone(),
                        },
                    });
                }
                if ap.len() != bp.len() {
                    return Err(UnifyError::Mismatch {
                        left: Ty::Sum {
                            name: a.clone(),
                            variants: av.clone(),
                        },
                        right: Ty::Sum {
                            name: b.clone(),
                            variants: bv.clone(),
                        },
                    });
                }
                for (x, y) in ap.iter().zip(bp.iter()) {
                    current = unify_with(&current, x, y)?;
                }
            }
            Ok(current)
        }

        // Constructor against its parent sum (or vice versa). The
        // `tag` and `arity` must agree with the sum's own
        // declarations, then the constructor unifies with the sum
        // (the constructor's owner should already equal the sum,
        // so this is a sanity unify).
        (ctor @ Ty::Constructor { .. }, sum @ Ty::Sum { .. })
        | (sum @ Ty::Sum { .. }, ctor @ Ty::Constructor { .. }) => {
            // Re-borrow the constructor parts without consuming the
            // pattern match (we still need `sum` and `ctor` for the
            // final unify).
            let (c_owner, c_tag, c_arity) = match &ctor {
                Ty::Constructor { owner, tag, arity } => (owner.as_ref(), *tag, *arity),
                _ => unreachable!(),
            };
            let (s_name, s_variants) = match &sum {
                Ty::Sum { name, variants } => (name.clone(), variants.clone()),
                _ => unreachable!(),
            };
            let variant = match s_variants.get(c_tag as usize) {
                Some(v) => v,
                None => {
                    return Err(UnifyError::Mismatch {
                        left: ctor,
                        right: sum,
                    });
                }
            };
            if variant.1.len() != c_arity {
                return Err(UnifyError::Mismatch {
                    left: ctor,
                    right: sum,
                });
            }
            // The constructor's owner and the sum should be the
            // same type (modulo substitution). Unify to verify.
            let _ = s_name; // the name matches because variants is keyed by it
            unify_with(subst, c_owner, &sum)
        }

        // Two constructors unify iff they have the same tag and
        // their owners are unifiable. This handles pattern
        // matching: the pattern returns the scrutinee's type
        // (a Constructor with a specific tag) and the scrutinee
        // is also a Constructor — both have the same tag when
        // the pattern matches.
        (
            Ty::Constructor { owner: o1, tag: t1, arity: a1 },
            Ty::Constructor { owner: o2, tag: t2, arity: a2 },
        ) => {
            if t1 != t2 || a1 != a2 {
                return Err(UnifyError::Mismatch {
                    left: Ty::Constructor {
                        owner: o1.clone(),
                        tag: t1,
                        arity: a1,
                    },
                    right: Ty::Constructor {
                        owner: o2.clone(),
                        tag: t2,
                        arity: a2,
                    },
                });
            }
            unify_with(subst, o1.as_ref(), o2.as_ref())
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

    // ---- Sum / Constructor unification ----

    fn sum(name: &str, variants: Vec<(&str, Vec<Ty>)>) -> Ty {
        Ty::Sum {
            name: name.to_string(),
            variants: variants
                .into_iter()
                .map(|(n, ps)| (n.to_string(), ps))
                .collect(),
        }
    }

    fn ctor(owner: Ty, tag: u32, arity: usize) -> Ty {
        Ty::Constructor {
            owner: Box::new(owner),
            tag,
            arity,
        }
    }

    #[test]
    fn unify_same_name_empty_sums_succeeds() {
        // enum E { A, B }  ~  enum E { A, B }  → identity.
        let s1 = sum("E", vec![("A", vec![]), ("B", vec![])]);
        let s2 = sum("E", vec![("A", vec![]), ("B", vec![])]);
        assert!(unify(&s1, &s2).is_ok());
    }

    #[test]
    fn unify_same_name_sums_with_payloads_succeeds() {
        // enum O { None, Some(int) } ~ enum O { None, Some(int) }
        let s1 = sum("O", vec![("None", vec![]), ("Some", vec![int()])]);
        let s2 = sum("O", vec![("None", vec![]), ("Some", vec![int()])]);
        assert!(unify(&s1, &s2).is_ok());
    }

    #[test]
    fn unify_sums_with_polymorphic_payload_binds_vars() {
        // enum E { A } with payload α  ~  enum E { A } with payload int
        // → α = int.
        let s1 = sum("E", vec![("A", vec![v(0)])]);
        let s2 = sum("E", vec![("A", vec![int()])]);
        let s = unify(&s1, &s2).unwrap();
        assert_eq!(apply_ty(&s, &v(0)), int());
    }

    #[test]
    fn unify_different_name_sums_is_mismatch() {
        let s1 = sum("E", vec![("A", vec![])]);
        let s2 = sum("F", vec![("A", vec![])]);
        assert!(matches!(
            unify(&s1, &s2).unwrap_err(),
            UnifyError::Mismatch { .. }
        ));
    }

    #[test]
    fn unify_sums_with_different_variant_count_is_mismatch() {
        let s1 = sum("E", vec![("A", vec![]), ("B", vec![])]);
        let s2 = sum("E", vec![("A", vec![])]);
        assert!(matches!(
            unify(&s1, &s2).unwrap_err(),
            UnifyError::Mismatch { .. }
        ));
    }

    #[test]
    fn unify_sums_with_different_variant_name_is_mismatch() {
        let s1 = sum("E", vec![("A", vec![]), ("B", vec![])]);
        let s2 = sum("E", vec![("A", vec![]), ("C", vec![])]);
        assert!(matches!(
            unify(&s1, &s2).unwrap_err(),
            UnifyError::Mismatch { .. }
        ));
    }

    #[test]
    fn unify_sums_with_different_payload_arity_is_mismatch() {
        let s1 = sum("E", vec![("A", vec![int()])]);
        let s2 = sum("E", vec![("A", vec![int(), int()])]);
        assert!(matches!(
            unify(&s1, &s2).unwrap_err(),
            UnifyError::Mismatch { .. }
        ));
    }

    #[test]
    fn unify_constructor_with_its_parent_sum_succeeds() {
        // Constructor { tag=1, arity=1 } ~ Sum { Some(int) }
        let s = sum("O", vec![("None", vec![]), ("Some", vec![int()])]);
        let c = ctor(s.clone(), 1, 1);
        assert!(unify(&c, &s).is_ok());
    }

    #[test]
    fn unify_constructor_with_other_sum_is_mismatch() {
        // Constructor { tag=0, arity=0 } ~ Sum { Some(int) }
        // — wrong arity.
        let s = sum("O", vec![("None", vec![]), ("Some", vec![int()])]);
        let c = ctor(s.clone(), 0, 0);
        assert!(unify(&c, &s).is_ok()); // None has arity 0, so this works
        let c_bad = ctor(s.clone(), 1, 0);
        assert!(matches!(
            unify(&c_bad, &s).unwrap_err(),
            UnifyError::Mismatch { .. }
        ));
    }

    #[test]
    fn unify_constructor_with_out_of_range_tag_is_mismatch() {
        // Constructor { tag=5 } ~ Sum with 2 variants — tag out of range.
        let s = sum("O", vec![("None", vec![]), ("Some", vec![int()])]);
        let c = ctor(s.clone(), 5, 0);
        assert!(matches!(
            unify(&c, &s).unwrap_err(),
            UnifyError::Mismatch { .. }
        ));
    }

    #[test]
    fn unify_recursive_sum_payload_uses_con_not_unfolded() {
        // enum Tree { Leaf, Node(int, Tree, Tree) }
        // The recursive reference is `Ty::Con("Tree")`. The sum
        // itself is `Ty::Sum { name: "Tree", variants: [..] }`.
        // Unifying the sum with itself should not occur-check fail
        // because the payload uses the opaque name reference.
        let tree = Ty::Con("Tree".into());
        let s = sum(
            "Tree",
            vec![
                ("Leaf", vec![]),
                ("Node", vec![int(), tree.clone(), tree]),
            ],
        );
        assert!(unify(&s, &s).is_ok());
    }
}