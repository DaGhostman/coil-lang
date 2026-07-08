//! Substitutions: mappings from type variables to types, with `apply`,
//! `compose`, `union`, and free-variable helpers.
//!
//! A substitution is stored as a `Vec<(TyVarId, Ty)>`. Most substitutions
//! produced by unification are tiny (single bindings) so `Vec` is fine.
//!
//! ## `apply` is non-recursive
//!
//! `apply_ty(s, Var(α))` performs **one** lookup: if `α ∈ dom(s)`, it
//! returns `s[α]` directly (which may itself be a `Var(β)`); otherwise it
//! returns `Var(α)` unchanged. It does not chase the chain, so
//! `apply({α → β, β → int}, α) == Var(β)`, **not** `int`.
//!
//! This matches the textbook formulation in Pierce/Diehl. The reason is
//! that `compose(s1, s2)` cannot be given a faithful set-of-pairs
//! representation if `apply` chases chains — the textbook proof shows that
//! a faithful `compose` is only possible with non-recursive `apply`.
//!
//! Callers that want a fully-resolved type for diagnostics or
//! pretty-printing can use [`apply_ty_prune`], which reapplies until no
//! more changes occur.

use std::collections::HashSet;

use super::ty::{Scheme, Ty, TyVarId, ftv_scheme, ftv_ty};

/// A partial map from `TyVarId` to `Ty`.
///
/// `mappings` is iterated in insertion order, which makes the result of
/// `compose` deterministic (important for snapshot tests).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subst {
    mappings: Vec<(TyVarId, Ty)>,
}

impl Subst {
    /// Build an empty substitution.
    pub fn empty() -> Self {
        Self {
            mappings: Vec::new(),
        }
    }

    /// Build a substitution with a single binding.
    pub fn singleton(v: TyVarId, ty: Ty) -> Self {
        Self {
            mappings: vec![(v, ty)],
        }
    }

    /// Insert (or overwrite) a binding. Last write wins, matching the
    /// canonical "most recent substitution wins" semantics used by
    /// `compose`.
    pub fn insert(&mut self, v: TyVarId, ty: Ty) {
        if let Some(slot) = self.mappings.iter_mut().find(|(k, _)| *k == v) {
            slot.1 = ty;
            return;
        }
        self.mappings.push((v, ty));
    }

    /// Remove any binding for `v`. Returns the removed value if there was
    /// one.
    pub fn remove(&mut self, v: TyVarId) -> Option<Ty> {
        let pos = self.mappings.iter().position(|(k, _)| *k == v)?;
        Some(self.mappings.remove(pos).1)
    }

    /// True if `v` is in the domain of this substitution.
    pub fn contains(&self, v: TyVarId) -> bool {
        self.mappings.iter().any(|(k, _)| *k == v)
    }

    /// Look up the binding for `v`, if any.
    pub fn get(&self, v: TyVarId) -> Option<&Ty> {
        self.mappings.iter().find(|(k, _)| *k == v).map(|(_, t)| t)
    }

    /// Number of bindings.
    pub fn len(&self) -> usize {
        self.mappings.len()
    }

    /// True if the substitution has no bindings.
    pub fn is_empty(&self) -> bool {
        self.mappings.is_empty()
    }

    /// Iterate over `(variable, type)` pairs in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (TyVarId, &Ty)> {
        self.mappings.iter().map(|(k, v)| (*k, v))
    }

    /// Free variables of the codomain (i.e. of every mapped-to type).
    /// Useful when checking whether a substitution is closed.
    pub fn ftv(&self) -> HashSet<TyVarId> {
        let mut acc = HashSet::new();
        for (_, t) in &self.mappings {
            acc.extend(ftv_ty(t));
        }
        acc
    }
}

impl From<Vec<(TyVarId, Ty)>> for Subst {
    fn from(v: Vec<(TyVarId, Ty)>) -> Self {
        let mut s = Subst::empty();
        for (k, val) in v {
            s.insert(k, val);
        }
        s
    }
}

/// Apply a substitution to a `Ty`.
///
/// Recurses through every type constructor. For `Var(v)`, if `v` is bound,
/// the **bound type itself** is returned (single lookup) — we do not chase
/// the chain. This is what makes [`compose`] correct.
pub fn apply_ty(subst: &Subst, ty: &Ty) -> Ty {
    match ty {
        Ty::Var(v) => match subst.get(*v) {
            Some(t) => t.clone(),
            None => Ty::Var(*v),
        },
        Ty::Con(_) => ty.clone(),
        Ty::Fun(a, b) => Ty::Fun(Box::new(apply_ty(subst, a)), Box::new(apply_ty(subst, b))),
        Ty::App(c, args) => Ty::App(
            Box::new(apply_ty(subst, c)),
            args.iter().map(|t| apply_ty(subst, t)).collect(),
        ),
        Ty::List(inner) => Ty::List(Box::new(apply_ty(subst, inner))),
        Ty::Sum { name, variants } => Ty::Sum {
            name: name.clone(),
            variants: variants
                .iter()
                .map(|(n, payload)| {
                    let new_payload = match payload {
                        crate::typechecking::ty::EnumVariantPayloadTy::Unit => {
                            crate::typechecking::ty::EnumVariantPayloadTy::Unit
                        }
                        crate::typechecking::ty::EnumVariantPayloadTy::Tuple(tys) => {
                            crate::typechecking::ty::EnumVariantPayloadTy::Tuple(
                                tys.iter().map(|t| apply_ty(subst, t)).collect(),
                            )
                        }
                        crate::typechecking::ty::EnumVariantPayloadTy::Record(fields) => {
                            crate::typechecking::ty::EnumVariantPayloadTy::Record(
                                fields
                                    .iter()
                                    .map(|(n, t)| (n.clone(), apply_ty(subst, t)))
                                    .collect(),
                            )
                        }
                    };
                    (n.clone(), new_payload)
                })
                .collect(),
        },
        Ty::Constructor { owner, tag, arity } => Ty::Constructor {
            owner: Box::new(apply_ty(subst, owner)),
            tag: *tag,
            arity: *arity,
        },
        // Phase 24 — recurse through aggregate types. The
        // length component of `Array` and the field names of
        // `Record` are inert (no free vars).
        Ty::Tuple(tys) => Ty::Tuple(tys.iter().map(|t| apply_ty(subst, t)).collect()),
        Ty::Array { element, length } => Ty::Array {
            element: Box::new(apply_ty(subst, element)),
            length: length.clone(),
        },
        Ty::Record { fields } => Ty::Record {
            fields: fields
                .iter()
                .map(|(n, t)| (n.clone(), apply_ty(subst, t)))
                .collect(),
        },
    }
}

/// Apply a substitution repeatedly until no more changes occur.
///
/// Useful for diagnostics and pretty-printing. Bounded only by the
/// structure of the substitution; under the occurs-check invariant (Phase
/// 2), the loop terminates.
pub fn apply_ty_prune(subst: &Subst, ty: &Ty) -> Ty {
    let mut current = apply_ty(subst, ty);
    loop {
        let next = apply_ty(subst, &current);
        if next == current {
            return current;
        }
        current = next;
    }
}

/// Apply a substitution to a `Scheme`.
///
/// Quantified variables are preserved. This is correct as long as `subst`
/// does not bind any of `s.bounds`; that invariant is maintained by
/// `instantiate` (Phase 3) which mints fresh variables for every
/// quantified variable before applying the substitution.
#[allow(dead_code)] // exposed for diagnostic helpers
pub fn apply_scheme(subst: &Subst, s: &Scheme) -> Scheme {
    Scheme {
        bounds: s.bounds.clone(),
        ty: apply_ty(subst, &s.ty),
    }
}

/// Compose two substitutions: `s1 ∘ s2` applies `s2` first, then `s1`.
///
/// Formally: `apply(compose(s1, s2), t) == apply(s1, apply(s2, t))`.
///
/// Construction:
///   1. For every `(α, t)` in `s2`, write `α → apply(s1, t)` into the
///      result.
///   2. Add every binding `(α, t)` from `s1` whose key is not already in
///      the result (i.e. not shadowed by `s2`).
///
/// For shared keys, the result is `s1(s2(α))` — `s2`'s value wins (after
/// `s1` is applied to it), because `s1 ∘ s2` mathematically means `s2`
/// runs first.
///
/// Note: this is *not* "extend with override" semantics. Use [`union`] for
/// that (it lets the right-hand side's bindings win, which is what
/// Algorithm W wants when combining substitutions from recursive calls
/// where keys are typically disjoint).
pub fn compose(s1: &Subst, s2: &Subst) -> Subst {
    let mut result = Subst::empty();
    for (v, t) in s2.iter() {
        result.insert(v, apply_ty(s1, t));
    }
    for (v, t) in s1.iter() {
        if !result.contains(v) {
            result.insert(v, t.clone());
        }
    }
    result
}

/// Union of two substitutions. For keys in both, `s2`'s value wins
/// (consistent with `compose`).
#[allow(dead_code)] // exposed for diagnostic helpers
pub fn union(s1: &Subst, s2: &Subst) -> Subst {
    let mut result = s1.clone();
    for (v, t) in s2.iter() {
        result.insert(v, t.clone());
    }
    result
}

/// Re-export of `ftv_ty` at the module level for ergonomics.
#[allow(dead_code)]
pub fn ftv(ty: &Ty) -> HashSet<TyVarId> {
    ftv_ty(ty)
}

/// Re-export of `ftv_scheme` at the module level for ergonomics.
#[allow(dead_code)]
pub fn ftv_sch(s: &Scheme) -> HashSet<TyVarId> {
    ftv_scheme(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typechecking::ty::{float, int, list, string};

    fn v(i: u32) -> Ty {
        Ty::Var(TyVarId(i))
    }

    // ---- apply_ty ----

    #[test]
    fn apply_passes_through_constructor() {
        let s = Subst::empty();
        assert_eq!(apply_ty(&s, &int()), int());
        assert_eq!(apply_ty(&s, &float()), float());
        assert_eq!(apply_ty(&s, &string()), string());
    }

    #[test]
    fn apply_replaces_var_with_bound_type() {
        let s = Subst::singleton(TyVarId(0), int());
        assert_eq!(apply_ty(&s, &v(0)), int());
    }

    #[test]
    fn apply_unbound_var_is_identity() {
        let s = Subst::empty();
        assert_eq!(apply_ty(&s, &v(7)), v(7));
    }

    #[test]
    fn apply_recurses_through_fun() {
        let s = Subst::singleton(TyVarId(0), int());
        let ty = Ty::Fun(Box::new(v(0)), Box::new(string()));
        assert_eq!(
            apply_ty(&s, &ty),
            Ty::Fun(Box::new(int()), Box::new(string()))
        );
    }

    #[test]
    fn apply_recurses_through_app() {
        let s = Subst::singleton(TyVarId(0), int());
        let ty = Ty::App(Box::new(Ty::Con("Foo".into())), vec![v(0), v(1)]);
        assert_eq!(
            apply_ty(&s, &ty),
            Ty::App(Box::new(Ty::Con("Foo".into())), vec![int(), v(1)])
        );
    }

    #[test]
    fn apply_recurses_through_list() {
        let s = Subst::singleton(TyVarId(0), int());
        assert_eq!(apply_ty(&s, &list(v(0))), list(int()));
    }

    #[test]
    fn apply_does_not_chains_through_bound_var() {
        // With non-recursive apply, α → β, β → int resolves α to β (not
        // int). The chain is not chased.
        let mut s = Subst::empty();
        s.insert(TyVarId(0), v(1));
        s.insert(TyVarId(1), int());
        assert_eq!(apply_ty(&s, &v(0)), v(1));
    }

    #[test]
    fn apply_does_not_chains_in_fun() {
        let mut s = Subst::empty();
        s.insert(TyVarId(0), v(1));
        s.insert(TyVarId(1), int());
        let ty = Ty::Fun(Box::new(v(0)), Box::new(v(0)));
        assert_eq!(apply_ty(&s, &ty), Ty::Fun(Box::new(v(1)), Box::new(v(1))));
    }

    #[test]
    fn apply_prune_fully_resolves_chains() {
        // apply_ty_prune re-applies until no change.
        let mut s = Subst::empty();
        s.insert(TyVarId(0), v(1));
        s.insert(TyVarId(1), int());
        assert_eq!(apply_ty_prune(&s, &v(0)), int());
        assert_eq!(
            apply_ty_prune(&s, &Ty::Fun(Box::new(v(0)), Box::new(v(0)))),
            Ty::Fun(Box::new(int()), Box::new(int())),
        );
    }

    #[test]
    fn apply_prune_is_idempotent_on_already_resolved() {
        // Applying to a fully-resolved type is a no-op.
        let mut s = Subst::empty();
        s.insert(TyVarId(0), int());
        assert_eq!(apply_ty_prune(&s, &int()), int());
        assert_eq!(apply_ty_prune(&s, &v(0)), int());
    }

    // ---- apply_scheme ----

    #[test]
    fn apply_scheme_preserves_bounds() {
        let s = Subst::singleton(TyVarId(0), int());
        let scheme = Scheme {
            bounds: vec![TyVarId(0)],
            ty: v(0),
        };
        let result = apply_scheme(&s, &scheme);
        assert_eq!(result.bounds, vec![TyVarId(0)]);
        // Note: this is technically incorrect when the substitution binds a
        // quantified variable — `instantiate` (Phase 3) is responsible for
        // preventing that. Documented in the function's doc-comment.
        assert_eq!(result.ty, int());
    }

    #[test]
    fn apply_scheme_applies_to_free_vars() {
        let s = Subst::singleton(TyVarId(1), int());
        let scheme = Scheme {
            bounds: vec![TyVarId(0)],
            ty: Ty::Fun(Box::new(v(0)), Box::new(v(1))),
        };
        let result = apply_scheme(&s, &scheme);
        assert_eq!(result.ty, Ty::Fun(Box::new(v(0)), Box::new(int())));
    }

    // ---- compose ----

    #[test]
    fn compose_of_two_empties_is_empty() {
        assert_eq!(compose(&Subst::empty(), &Subst::empty()), Subst::empty());
    }

    #[test]
    fn compose_resolves_chains() {
        // s1 = {α → β}, s2 = {β → int}; compose should give α → β, β → int.
        // With non-recursive apply, α resolves to β (not int).
        let s1 = Subst::singleton(TyVarId(0), v(1));
        let s2 = Subst::singleton(TyVarId(1), int());
        let composed = compose(&s1, &s2);
        assert_eq!(apply_ty(&composed, &v(0)), v(1));
        assert_eq!(apply_ty(&composed, &v(1)), int());
    }

    #[test]
    fn compose_matches_double_apply() {
        // The defining equation: apply(compose(s1, s2), t) == apply(s1, apply(s2, t))
        let s1 = Subst::singleton(TyVarId(0), v(1));
        let s2 = Subst::singleton(TyVarId(1), string());

        let t = Ty::Fun(Box::new(v(0)), Box::new(v(0)));

        let lhs = apply_ty(&compose(&s1, &s2), &t);
        let rhs = apply_ty(&s1, &apply_ty(&s2, &t));
        assert_eq!(lhs, rhs);
    }

    #[test]
    fn compose_is_associative() {
        // (s3 ∘ s2) ∘ s1 == s3 ∘ (s2 ∘ s1)
        let s1 = Subst::singleton(TyVarId(0), v(2));
        let s2 = Subst::singleton(TyVarId(1), v(0));
        let s3 = Subst::singleton(TyVarId(3), v(1));

        let lhs = compose(&compose(&s3, &s2), &s1);
        let rhs = compose(&s3, &compose(&s2, &s1));

        let t = Ty::Fun(Box::new(v(3)), Box::new(v(3)));
        assert_eq!(apply_ty(&lhs, &t), apply_ty(&rhs, &t));
    }

    #[test]
    fn compose_left_keeps_disjoint_keys() {
        let s1 = Subst::singleton(TyVarId(0), int());
        let s2 = Subst::singleton(TyVarId(1), string());
        let composed = compose(&s1, &s2);
        assert_eq!(apply_ty(&composed, &v(0)), int());
        assert_eq!(apply_ty(&composed, &v(1)), string());
    }

    #[test]
    fn compose_for_shared_keys_applies_s1_to_s2_value() {
        // Mathematical compose: s1 ∘ s2(α) = s1(s2(α)).
        // For s1 = {α → int}, s2 = {α → string}, this gives s1(string)
        // = string (since string is not in s1's domain). s2's value wins
        // (after applying s1).
        let s1 = Subst::singleton(TyVarId(0), int());
        let s2 = Subst::singleton(TyVarId(0), string());
        let composed = compose(&s1, &s2);
        assert_eq!(apply_ty(&composed, &v(0)), string());
    }

    // ---- union ----

    #[test]
    fn union_combines_domains() {
        let s1 = Subst::singleton(TyVarId(0), int());
        let s2 = Subst::singleton(TyVarId(1), string());
        let u = union(&s1, &s2);
        assert_eq!(u.len(), 2);
        assert_eq!(apply_ty(&u, &v(0)), int());
        assert_eq!(apply_ty(&u, &v(1)), string());
    }

    #[test]
    fn union_lets_right_side_win_for_shared_keys() {
        let s1 = Subst::singleton(TyVarId(0), int());
        let s2 = Subst::singleton(TyVarId(0), string());
        let u = union(&s1, &s2);
        assert_eq!(apply_ty(&u, &v(0)), string());
    }

    // ---- Subst helpers ----

    #[test]
    fn subst_singleton_len_and_get() {
        let s = Subst::singleton(TyVarId(0), int());
        assert_eq!(s.len(), 1);
        assert!(!s.is_empty());
        assert!(s.contains(TyVarId(0)));
        assert!(!s.contains(TyVarId(1)));
        assert_eq!(s.get(TyVarId(0)), Some(&int()));
    }

    #[test]
    fn subst_insert_overwrites_existing() {
        let mut s = Subst::singleton(TyVarId(0), int());
        s.insert(TyVarId(0), string());
        assert_eq!(s.len(), 1);
        assert_eq!(s.get(TyVarId(0)), Some(&string()));
    }

    #[test]
    fn subst_remove_drops_binding() {
        let mut s = Subst::singleton(TyVarId(0), int());
        let removed = s.remove(TyVarId(0));
        assert_eq!(removed, Some(int()));
        assert!(s.is_empty());
    }

    #[test]
    fn subst_ftv_is_union_of_codomain_ftvs() {
        let mut s = Subst::empty();
        s.insert(TyVarId(0), Ty::Fun(Box::new(v(1)), Box::new(int())));
        s.insert(TyVarId(2), v(1));
        assert_eq!(s.ftv(), HashSet::from([TyVarId(1)]));
    }
}
