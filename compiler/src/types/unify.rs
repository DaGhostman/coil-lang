use std::collections::HashMap;

use parser::SimpleSpan;

use crate::types::type::{Type, TypeVar};
use crate::types::substitution::Substitution;

/// Result of unification
#[derive(Debug, Clone)]
pub enum UnifyResult {
    Success(Substitution),
    Failure(String),
}

impl UnifyResult {
    pub fn success(subst: Substitution) -> Self {
        UnifyResult::Success(subst)
    }

    pub fn failure(msg: &str) -> Self {
        UnifyResult::Failure(msg.to_string())
    }
}

/// Hindley-Milner unification algorithm
pub fn unify_types(t1: &Type, t2: &Type, substitution: &mut Substitution) -> UnifyResult {
    // Apply current substitution to both types
    let t1 = apply_substitution(t1, substitution);
    let t2 = apply_substitution(&t2, substitution);

    match (t1, t2) {
        // Same type
        (Type::Int, Type::Int)
        | (Type::Float, Type::Float)
        | (Type::String, Type::String)
        | (Type::Bool, Type::Bool)
        | (Type::Void, Type::Void)
        | (Type::None, Type::None) => UnifyResult::success(substitution.clone()),

        // Type variables
        (Type::TypeVar(tv1), Type::TypeVar(tv2)) => {
            if tv1 == tv2 {
                UnifyResult::success(substitution.clone())
            } else {
                // Create binding: tv1 = tv2
                substitution.extend(tv1.clone(), Type::TypeVar(tv2.clone()));
                UnifyResult::success(substitution.clone())
            }
        }

        // Type variable with concrete type
        (Type::TypeVar(tv), ty) => {
            if occurs_check(tv, &ty, substitution) {
                UnifyResult::failure("Recursive type binding detected")
            } else {
                substitution.extend(tv.clone(), ty);
                UnifyResult::success(substitution.clone())
            }
        }

        (ty, Type::TypeVar(tv)) => {
            if occurs_check(tv, &ty, substitution) {
                UnifyResult::failure("Recursive type binding detected")
            } else {
                substitution.extend(tv.clone(), ty);
                UnifyResult::success(substitution.clone())
            }
        }

        // Compound types
        (Type::Array(t1), Type::Array(t2)) => unify_types(t1, t2, substitution),

        (Type::Function(p1, r1), Type::Function(p2, r2)) => {
            if p1.len() != p2.len() {
                return UnifyResult::failure("Function parameter count mismatch");
            }

            // Unify all parameters
            let mut subst = substitution.clone();
            for (param1, param2) in p1.iter().zip(p2.iter()) {
                match unify_types(param1, param2, &mut subst) {
                    UnifyResult::Success(s) => subst = s,
                    UnifyResult::Failure(msg) => return UnifyResult::failure(&msg),
                }
            }

            // Unify return types
            match unify_types(r1, r2, &mut subst) {
                UnifyResult::Success(s) => UnifyResult::success(s),
                UnifyResult::Failure(msg) => UnifyResult::failure(&msg),
            }
        }

        (Type::Tuple(t1), Type::Tuple(t2)) => {
            if t1.len() != t2.len() {
                return UnifyResult::failure("Tuple size mismatch");
            }

            let mut subst = substitution.clone();
            for (item1, item2) in t1.iter().zip(t2.iter()) {
                match unify_types(item1, item2, &mut subst) {
                    UnifyResult::Success(s) => subst = s,
                    UnifyResult::Failure(msg) => return UnifyResult::failure(&msg),
                }
            }

            UnifyResult::success(subst)
        }

        // Struct and Interface types - for now, require exact match
        (Type::Struct(s1), Type::Struct(s2)) => {
            if s1.name != s2.name {
                return UnifyResult::failure("Struct name mismatch");
            }
            UnifyResult::success(substitution.clone())
        }

        (Type::Interface(i1), Type::Interface(i2)) => {
            if i1.name != i2.name {
                return UnifyResult::failure("Interface name mismatch");
            }
            UnifyResult::success(substitution.clone())
        }

        // Generic types
        (Type::Generic(g1), Type::Generic(g2)) => {
            if g1.name != g2.name {
                return UnifyResult::failure("Generic name mismatch");
            }
            if g1.params.len() != g2.params.len() {
                return UnifyResult::failure("Generic parameter count mismatch");
            }

            let mut subst = substitution.clone();
            for (p1, p2) in g1.params.iter().zip(g2.params.iter()) {
                match unify_types(p1, p2, &mut subst) {
                    UnifyResult::Success(s) => subst = s,
                    UnifyResult::Failure(msg) => return UnifyResult::failure(&msg),
                }
            }

            UnifyResult::success(subst)
        }

        // Sum types
        (Type::SumType(v1), Type::SumType(v2)) => {
            if v1.len() != v2.len() {
                return UnifyResult::failure("Sum type variant count mismatch");
            }

            let mut subst = substitution.clone();
            for (variant1, variant2) in v1.iter().zip(v2.iter()) {
                if variant1.name != variant2.name {
                    return UnifyResult::failure("Sum type variant name mismatch");
                }
                // TODO: unify variant fields
            }

            UnifyResult::success(subst)
        }

        // Type alias
        (Type::Alias(a1), Type::Alias(a2)) => {
            if a1.name != a2.name {
                return UnifyResult::failure("Alias name mismatch");
            }
            UnifyResult::success(substitution.clone())
        }

        // All other cases - failure
        _ => UnifyResult::failure("Type mismatch"),
    }
}

/// Apply substitution to a type
fn apply_substitution(ty: &Type, substitution: &Substitution) -> Type {
    match ty {
        Type::TypeVar(tv) => substitution.get(tv).cloned().unwrap_or_else(|| ty.clone()),
        Type::Array(t) => Type::Array(apply_substitution(t, substitution)),
        Type::Function(params, ret) => {
            let new_params = params
                .iter()
                .map(|p| apply_substitution(p, substitution))
                .collect();
            let new_ret = apply_substitution(ret, substitution);
            Type::Function(new_params, new_ret)
        }
        Type::Tuple(tys) => {
            let new_tys = tys
                .iter()
                .map(|t| apply_substitution(t, substitution))
                .collect();
            Type::Tuple(new_tys)
        }
        Type::Struct(def) => {
            let new_fields = def
                .fields
                .iter()
                .map(|f| {
                    super::type::Field::new(
                        &f.name,
                        apply_substitution(&f.ty, substitution),
                        f.span.clone(),
                    )
                })
                .collect();
            let mut new_def = super::type::StructDef::new(&def.name, new_fields, def.span.clone());
            new_def.with_generics(def.generics.clone());
            Type::Struct(new_def)
        }
        Type::Interface(def) => {
            let new_methods = def
                .methods
                .iter()
                .map(|m| {
                    let new_params = m
                        .params
                        .iter()
                        .map(|p| apply_substitution(p, substitution))
                        .collect();
                    super::type::Method::new(
                        &m.name,
                        new_params,
                        apply_substitution(&m.return_ty, substitution),
                        m.span.clone(),
                    )
                })
                .collect();
            let mut new_def = super::type::InterfaceDef::new(
                &def.name,
                new_methods,
                def.span.clone(),
            );
            new_def.with_generics(def.generics.clone());
            new_def.extends(def.extends.iter().map(|s| s.as_str()).collect());
            Type::Interface(new_def)
        }
        Type::Generic(gen) => {
            let new_params = gen
                .params
                .iter()
                .map(|p| apply_substitution(p, substitution))
                .collect();
            Type::Generic(super::type::GenericType::new(
                &gen.name,
                new_params,
                gen.span.clone(),
            ))
        }
        Type::Alias(alias) => {
            Type::Alias(super::type::TypeAlias::new(
                &alias.name,
                apply_substitution(&alias.target, substitution),
                alias.span.clone(),
            ))
        }
        Type::SumType(variants) => {
            let new_variants = variants
                .iter()
                .map(|v| {
                    let mut variant = super::type::Variant::new(&v.name, v.span.clone());
                    let new_fields = v
                        .fields
                        .iter()
                        .map(|f| {
                            super::type::Field::new(
                                &f.name,
                                apply_substitution(&f.ty, substitution),
                                f.span.clone(),
                            )
                        })
                        .collect();
                    variant.with_fields(new_fields);
                    variant
                })
                .collect();
            Type::SumType(new_variants)
        }
        _ => ty.clone(),
    }
}

/// Occur-check: prevents infinite types like T = List<T>
fn occurs_check(tv: &TypeVar, ty: &Type, substitution: &Substitution) -> bool {
    match ty {
        Type::TypeVar(t) => {
            if t == tv {
                true
            } else if let Some(subst) = substitution.get(t) {
                occurs_check(tv, subst, substitution)
            } else {
                false
            }
        }
        Type::Array(t) => occurs_check(tv, t, substitution),
        Type::Function(params, ret) => {
            params.iter().any(|p| occurs_check(tv, p, substitution))
                || occurs_check(tv, ret, substitution)
        }
        Type::Tuple(tys) => tys.iter().any(|t| occurs_check(tv, t, substitution)),
        Type::Struct(def) => def
            .fields
            .iter()
            .any(|f| occurs_check(tv, &f.ty, substitution)),
        Type::Interface(def) => def
            .methods
            .iter()
            .any(|m| m.params.iter().any(|p| occurs_check(tv, p, substitution)) || occurs_check(tv, &m.return_ty, substitution)),
        Type::Generic(gen) => gen.params.iter().any(|p| occurs_check(tv, p, substitution)),
        Type::Alias(alias) => occurs_check(tv, &alias.target, substitution),
        Type::SumType(variants) => variants
            .iter()
            .any(|v| v.fields.iter().any(|f| occurs_check(tv, &f.ty, substitution))),
        _ => false,
    }
}