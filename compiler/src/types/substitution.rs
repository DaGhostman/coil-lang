use std::collections::HashMap;

use crate::types::type::{Type, TypeVar, Field, StructDef, Method, InterfaceDef, GenericType, TypeAlias, Variant};

/// Type substitution mapping for Hindley-Milner unification
pub type Substitution = HashMap<TypeVar, Type>;

impl Substitution {
    /// Create a new empty substitution
    pub fn new() -> Self {
        HashMap::new()
    }

    /// Apply substitution to a type
    pub fn apply(&self, ty: Type) -> Type {
        match ty {
            Type::TypeVar(tv) => self.get(&tv).cloned().unwrap_or(ty),
            Type::Array(ty) => Type::Array(self.apply(ty)),
            Type::Function(params, ret) => {
                Type::Function(params.into_iter().map(|p| self.apply(p)).collect(), self.apply(ret))
            }
            Type::Tuple(tys) => {
                Type::Tuple(tys.into_iter().map(|t| self.apply(t)).collect())
            }
            Type::Struct(def) => {
                let fields = def
                    .fields
                    .into_iter()
                    .map(|f| Field {
                        name: f.name,
                        ty: self.apply(f.ty),
                        span: f.span,
                    })
                    .collect();
                let mut new_def = StructDef::new(&def.name, fields, def.span);
                new_def.with_generics(def.generics);
                Type::Struct(new_def)
            }
            Type::Interface(def) => {
                let methods = def
                    .methods
                    .into_iter()
                    .map(|m| Method {
                        name: m.name,
                        params: m
                            .params
                            .into_iter()
                            .map(|p| self.apply(p))
                            .collect(),
                        return_ty: self.apply(m.return_ty),
                        default_impl: m.default_impl,
                        span: m.span,
                    })
                    .collect();
                let mut new_def = InterfaceDef::new(&def.name, methods, def.span);
                new_def.with_generics(def.generics);
                new_def.extends(def.extends.iter().map(|s| s.as_str()).collect());
                Type::Interface(new_def)
            }
            Type::Generic(gen) => {
                let params = gen
                    .params
                    .into_iter()
                    .map(|p| self.apply(p))
                    .collect();
                Type::Generic(GenericType::new(&gen.name, params, gen.span))
            }
            Type::Alias(alias) => {
                Type::Alias(TypeAlias::new(&alias.name, self.apply(alias.target), alias.span))
            }
            Type::SumType(variants) => {
                let sum_variants = variants
                    .into_iter()
                    .map(|v| {
                        let mut variant = Variant::new(&v.name, v.span);
                        variant.with_fields(
                            v
                                .fields
                                .into_iter()
                                .map(|f| Field::new(&f.name, self.apply(f.ty), f.span))
                                .collect(),
                        );
                        variant
                    })
                    .collect();
                Type::SumType(sum_variants)
            }
            _ => ty,
        }
    }

    /// Compose two substitutions
    pub fn compose(&self, other: &Substitution) -> Substitution {
        let mut result = HashMap::new();

        for (var, ty) in self {
            result.insert(var.clone(), other.apply(ty.clone()));
        }

        for (var, ty) in other {
            if !self.contains_key(var) {
                result.insert(var.clone(), ty.clone());
            }
        }

        result
    }

    /// Extend substitution with a new mapping
    pub fn extend(&mut self, var: TypeVar, ty: Type) {
        // Ensure we don't substitute a variable with a type containing itself (occur-check)
        if !self.contains_var(&ty, &var) {
            self.insert(var, ty);
        }
    }

    /// Check if a type contains a specific variable
    fn contains_var(&self, ty: &Type, var: &TypeVar) -> bool {
        match ty {
            Type::TypeVar(tv) => tv == var,
            Type::Array(t) => self.contains_var(t, var),
            Type::Function(params, ret) => {
                params.iter().any(|p| self.contains_var(p, var)) || self.contains_var(ret, var)
            }
            Type::Tuple(tys) => tys.iter().any(|t| self.contains_var(t, var)),
            Type::Struct(def) => def
                .fields
                .iter()
                .any(|f| self.contains_var(&f.ty, var)),
            Type::Interface(def) => def
                .methods
                .iter()
                .any(|m| m.params.iter().any(|p| self.contains_var(p, var)) || self.contains_var(&m.return_ty, var)),
            Type::Generic(gen) => gen.params.iter().any(|p| self.contains_var(p, var)),
            Type::Alias(alias) => self.contains_var(&alias.target, var),
            Type::SumType(variants) => variants
                .iter()
                .any(|v| v.fields.iter().any(|f| self.contains_var(&f.ty, var))),
            _ => false,
        }
    }
}

impl Default for Substitution {
    fn default() -> Self {
        Self::new()
    }
}