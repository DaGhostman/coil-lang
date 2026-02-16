use std::collections::HashMap;

use crate::types::ty::{
    Field, GenericType, InterfaceDef, Method, StructDef, Type, TypeAlias, TypeVar, Variant,
};

/// Type substitution mapping for Hindley-Milner unification
#[derive(Clone, Default)]
pub struct Substitution {
    inner: HashMap<TypeVar, Type>,
}

impl Substitution {
    /// Create a new empty substitution
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    /// Apply substitution to a type
    pub fn apply(&self, ty: Type) -> Type {
        match ty {
            Type::TypeVar(ref tv) => self.inner.get(&tv).cloned().unwrap_or(ty),
            Type::Array(ty_box) => Type::Array(Box::new(self.apply(*ty_box))),
            Type::Function(params, ret_box) => {
                let new_params = params
                    .into_iter()
                    // .map(|p| Box::new(self.apply(*p)))
                    .collect();
                Type::Function(new_params, Box::new(self.apply(*ret_box)))
            }
            Type::Tuple(tys) => Type::Tuple(tys.into_iter().map(|t| self.apply(t)).collect()),
            Type::Struct(def) => {
                let fields = def
                    .fields
                    .into_iter()
                    .map(|f| Field::new(&f.name, self.apply(*f.ty)))
                    .collect();
                let mut new_def = StructDef::new(&def.name, fields);
                new_def.with_generics(def.generics);
                Type::Struct(new_def)
            }
            Type::Interface(def) => {
                let methods = def
                    .methods
                    .into_iter()
                    .map(|m| {
                        Method::new(
                            &m.name,
                            m.params.into_iter().map(|p| self.apply(*p)).collect(),
                            self.apply(*m.return_ty),
                        )
                    })
                    .collect();
                let mut new_def = InterfaceDef::new(&def.name, methods);
                new_def.with_generics(def.generics);
                new_def.extends(def.extends.iter().map(|s| s.as_str()).collect());
                Type::Interface(new_def)
            }
            Type::Generic(r#gen) => {
                let params = r#gen.params.into_iter().map(|p| self.apply(*p)).collect();
                Type::Generic(GenericType::new(&r#gen.name, params))
            }
            Type::Alias(alias) => {
                Type::Alias(TypeAlias::new(&alias.name, self.apply(*alias.target)))
            }
            Type::SumType(variants) => {
                let sum_variants = variants
                    .into_iter()
                    .map(|v| {
                        let mut variant = Variant::new(&v.name);
                        variant.with_fields(
                            v.fields
                                .into_iter()
                                .map(|f| Field::new(&f.name, self.apply(*f.ty)))
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

        for (var, ty) in self.inner.iter() {
            result.insert(var.clone(), other.apply(ty.clone()));
        }

        for (var, ty) in other.inner.iter() {
            if !self.inner.contains_key(var) {
                result.insert(var.clone(), ty.clone());
            }
        }

        Substitution { inner: result }
    }

    /// Extend substitution with a new mapping
    pub fn extend(&mut self, var: TypeVar, ty: Type) {
        // Ensure we don't substitute a variable with a type containing itself (occur-check)
        if !self.contains_var(&ty, &var) {
            self.inner.insert(var, ty);
        }
    }

    /// Check if a type contains a specific variable (used in extend)
    fn contains_var(&self, ty: &Type, var: &TypeVar) -> bool {
        match ty {
            Type::TypeVar(tv) => tv == var,
            Type::Array(t) => self.contains_var(&*t, var),
            Type::Function(params, ret) => {
                params.iter().any(|p| self.contains_var(&*p, var)) || self.contains_var(&*ret, var)
            }
            Type::Tuple(tys) => tys.iter().any(|t| self.contains_var(&*t, var)),
            Type::Struct(def) => def.fields.iter().any(|f| self.contains_var(&*f.ty, var)),
            Type::Interface(def) => def.methods.iter().any(|m| {
                m.params.iter().any(|p| self.contains_var(&*p, var))
                    || self.contains_var(&*m.return_ty, var)
            }),
            Type::Generic(r#gen) => r#gen.params.iter().any(|p| self.contains_var(&*p, var)),
            Type::Alias(alias) => self.contains_var(&*alias.target, var),
            Type::SumType(variants) => variants
                .iter()
                .any(|v| v.fields.iter().any(|f| self.contains_var(&*f.ty, var))),
            _ => false,
        }
    }

    /// Get value from substitution
    pub fn get(&self, key: &TypeVar) -> Option<&Type> {
        self.inner.get(key)
    }

    /// Check if substitution contains a key
    pub fn contains_key(&self, key: &TypeVar) -> bool {
        self.inner.contains_key(key)
    }
}
