use std::fmt::Display;

use common::Byte;

/// Type bound for generics (e.g., T: Copy)
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct TypeBound {
    pub name: String,
}

impl TypeBound {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

/// Type variable for Hindley-Milner type inference
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct TypeVar {
    pub id: usize,
    pub name: String,
    pub bounds: Vec<TypeBound>,
}

impl TypeVar {
    pub fn new(id: usize, name: &str) -> Self {
        Self {
            id,
            name: name.to_string(),
            bounds: Vec::new(),
        }
    }

    pub fn with_bounds(mut self, bounds: Vec<TypeBound>) -> Self {
        self.bounds = bounds;
        self
    }

    pub fn add_bound(&mut self, bound: TypeBound) {
        self.bounds.push(bound);
    }
}

impl Display for TypeVar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// Field definition for structs
#[derive(Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub ty: Box<Type>,
}

impl Field {
    pub fn new(name: &str, ty: Type) -> Self {
        Self {
            name: name.to_string(),
            ty: Box::new(ty),
        }
    }
}

/// Method definition for interfaces
#[derive(Clone, PartialEq, Eq)]
pub struct Method {
    pub name: String,
    pub params: Vec<Box<Type>>,
    pub return_ty: Box<Type>,
    pub default_impl: Option<Vec<Byte>>,
}

impl Method {
    pub fn new(name: &str, params: Vec<Type>, return_ty: Type) -> Self {
        Self {
            name: name.to_string(),
            params: params.into_iter().map(|p| Box::new(p)).collect(),
            return_ty: Box::new(return_ty),
            default_impl: None,
        }
    }

    pub fn with_default_impl(&mut self, impl_bytes: Vec<Byte>) {
        self.default_impl = Some(impl_bytes);
    }
}

/// Structure definition for custom types
#[derive(Clone, PartialEq, Eq)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<Field>,
    pub generics: Vec<TypeVar>,
}

impl StructDef {
    pub fn new(name: &str, fields: Vec<Field>) -> Self {
        Self {
            name: name.to_string(),
            fields,
            generics: Vec::new(),
        }
    }

    pub fn with_generics(&mut self, generics: Vec<TypeVar>) {
        self.generics = generics;
    }
}

/// Interface definition for contracts
#[derive(Clone, PartialEq, Eq)]
pub struct InterfaceDef {
    pub name: String,
    pub methods: Vec<Method>,
    pub generics: Vec<TypeVar>,
    pub extends: Vec<String>,
}

impl InterfaceDef {
    pub fn new(name: &str, methods: Vec<Method>) -> Self {
        Self {
            name: name.to_string(),
            methods,
            generics: Vec::new(),
            extends: Vec::new(),
        }
    }

    pub fn with_generics(&mut self, generics: Vec<TypeVar>) {
        self.generics = generics;
    }

    pub fn extends(&mut self, interfaces: Vec<&str>) {
        self.extends = interfaces.iter().map(|s| s.to_string()).collect();
    }
}

/// Type alias definition
#[derive(Clone, PartialEq, Eq)]
pub struct TypeAlias {
    pub name: String,
    pub target: Box<Type>,
    pub generics: Vec<TypeVar>,
}

impl TypeAlias {
    pub fn new(name: &str, target: Type) -> Self {
        Self {
            name: name.to_string(),
            target: Box::new(target),
            generics: Vec::new(),
        }
    }

    pub fn with_generics(&mut self, generics: Vec<TypeVar>) {
        self.generics = generics;
    }
}

/// Generic type for parameterized types
#[derive(Clone, PartialEq, Eq)]
pub struct GenericType {
    pub name: String,
    pub params: Vec<Box<Type>>,
}

impl GenericType {
    pub fn new(name: &str, params: Vec<Type>) -> Self {
        Self {
            name: name.to_string(),
            params: params.into_iter().map(|p| Box::new(p)).collect(),
        }
    }
}

/// A generic function signature with type parameters
#[derive(Clone, PartialEq, Eq)]
pub struct GenericSignature {
    pub name: String,
    pub type_params: Vec<TypeVar>,
    pub params: Vec<Type>,
    pub return_ty: Box<Type>,
}

impl GenericSignature {
    pub fn new(name: &str, type_params: Vec<TypeVar>, params: Vec<Type>, return_ty: Type) -> Self {
        Self {
            name: name.to_string(),
            type_params,
            params,
            return_ty: Box::new(return_ty),
        }
    }

    /// Instantiate this generic signature with concrete types
    pub fn instantiate(&self, args: &[Type]) -> Type {
        if args.len() != self.type_params.len() {
            return Type::Void;
        }

        let mut substitution: std::collections::HashMap<usize, Type> =
            std::collections::HashMap::new();
        for (param, arg) in self.type_params.iter().zip(args.iter()) {
            substitution.insert(param.id, arg.clone());
        }

        fn apply_subst(ty: &Type, subst: &std::collections::HashMap<usize, Type>) -> Type {
            match ty {
                Type::TypeVar(tv) => subst.get(&tv.id).cloned().unwrap_or_else(|| ty.clone()),
                Type::Array(box_inner) => Type::Array(Box::new(apply_subst(box_inner, subst))),
                Type::Function(params, box_ret) => {
                    let new_params: Vec<Type> =
                        params.iter().map(|p| apply_subst(p, subst)).collect();
                    let new_ret = apply_subst(box_ret, subst);
                    Type::Function(new_params, Box::new(new_ret))
                }
                Type::Tuple(tys) => {
                    Type::Tuple(tys.iter().map(|t| apply_subst(t, subst)).collect())
                }
                Type::Generic(gt) => {
                    let new_params: Vec<Type> =
                        gt.params.iter().map(|p| apply_subst(p, subst)).collect();
                    Type::Generic(GenericType::new(&gt.name, new_params))
                }
                Type::Struct(def) => {
                    let new_fields: Vec<Field> = def
                        .fields
                        .iter()
                        .map(|f| Field::new(&f.name, apply_subst(&f.ty, subst)))
                        .collect();
                    let mut new_def = StructDef::new(&def.name, new_fields);
                    new_def.generics = def.generics.clone();
                    Type::Struct(new_def)
                }
                Type::Interface(def) => {
                    let new_methods: Vec<Method> = def
                        .methods
                        .iter()
                        .map(|m| {
                            let new_params: Vec<Box<Type>> = m
                                .params
                                .iter()
                                .map(|p| Box::new(apply_subst(p, subst)))
                                .collect();
                            let new_ret = apply_subst(&m.return_ty, subst);
                            Method {
                                name: m.name.clone(),
                                params: new_params,
                                return_ty: Box::new(new_ret),
                                default_impl: m.default_impl.clone(),
                            }
                        })
                        .collect();
                    let mut new_def = InterfaceDef::new(&def.name, new_methods);
                    new_def.generics = def.generics.clone();
                    new_def.extends = def.extends.clone();
                    Type::Interface(new_def)
                }
                Type::Alias(alias) => {
                    let new_target = apply_subst(&alias.target, subst);
                    let mut new_alias = TypeAlias::new(&alias.name, new_target);
                    new_alias.generics = alias.generics.clone();
                    Type::Alias(new_alias)
                }
                Type::SumType(variants) => {
                    let new_variants: Vec<Variant> = variants
                        .iter()
                        .map(|v| {
                            let new_fields: Vec<Field> = v
                                .fields
                                .iter()
                                .map(|f| Field::new(&f.name, apply_subst(&f.ty, subst)))
                                .collect();
                            let mut new_variant = Variant::new(&v.name);
                            new_variant.fields = new_fields;
                            new_variant
                        })
                        .collect();
                    Type::SumType(new_variants)
                }
                _ => ty.clone(),
            }
        }

        let new_params: Vec<Type> = self
            .params
            .iter()
            .map(|p| apply_subst(p, &substitution))
            .collect();
        let new_ret = apply_subst(&self.return_ty, &substitution);
        Type::Function(new_params, Box::new(new_ret))
    }
}

/// Core type representation for Hindley-Milner type system
#[derive(Clone, PartialEq, Eq)]
pub enum Type {
    // Base types
    Int,
    Float,
    String,
    Bool,
    Void,
    None,

    // Type variables for HM inference
    TypeVar(TypeVar),

    // Compound types (using Box to handle recursion)
    Array(Box<Type>),
    Function(Vec<Type>, Box<Type>), // (params, return)
    Tuple(Vec<Type>),

    // User-defined types
    Struct(StructDef),
    Interface(InterfaceDef),

    // Generics
    Generic(GenericType),

    // Type aliases
    Alias(TypeAlias),

    // Sum types (Rust-style enums)
    SumType(Vec<Variant>),
}

/// Variant for sum types
#[derive(Clone, PartialEq, Eq)]
pub struct Variant {
    pub name: String,
    pub fields: Vec<Field>,
}

impl Variant {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            fields: Vec::new(),
        }
    }

    pub fn with_fields(&mut self, fields: Vec<Field>) {
        self.fields = fields;
    }
}

impl Type {
    /// Create a new type variable
    pub fn type_var(id: usize, name: &str) -> Self {
        Type::TypeVar(TypeVar::new(id, name))
    }

    /// Create a new array type
    pub fn array(ty: Type) -> Self {
        Type::Array(Box::new(ty))
    }

    /// Create a new function type
    pub fn function(params: Vec<Type>, return_ty: Type) -> Self {
        Type::Function(params, Box::new(return_ty))
    }

    /// Create a new tuple type
    pub fn tuple(tys: Vec<Type>) -> Self {
        Type::Tuple(tys)
    }

    /// Create a new struct type
    pub fn struct_def(name: &str, fields: Vec<Field>) -> Self {
        let struct_def = StructDef::new(name, fields);
        Type::Struct(struct_def)
    }

    /// Create a new interface type
    pub fn interface_def(name: &str, methods: Vec<Method>) -> Self {
        let interface_def = InterfaceDef::new(name, methods);
        Type::Interface(interface_def)
    }

    /// Create a new generic type
    pub fn generic(name: &str, params: Vec<Type>) -> Self {
        Type::Generic(GenericType::new(name, params))
    }

    /// Create a new type alias
    pub fn alias(name: &str, target: Type) -> Self {
        let alias = TypeAlias::new(name, target);
        Type::Alias(alias)
    }

    /// Create a new sum type (enum)
    pub fn sum_type(name: &str, variants: Vec<Variant>) -> Self {
        // For now, we'll wrap variants in a struct-like type
        // This will be refined as we add more enum-specific features
        let _ = name;
        Type::SumType(variants)
    }

    /// Check if this type is a type variable
    pub fn is_type_var(&self) -> bool {
        matches!(self, Type::TypeVar(_))
    }

    /// Check if this type is a base type
    pub fn is_base_type(&self) -> bool {
        matches!(
            self,
            Type::Int | Type::Float | Type::String | Type::Bool | Type::Void | Type::None
        )
    }

    /// Unify this type with another type using the HM algorithm
    pub fn unify(
        &self,
        other: &Type,
        substitution: &mut crate::types::substitution::Substitution,
    ) -> crate::types::unify::UnifyResult {
        crate::types::unify::unify_types(self, other, substitution)
    }

    /// Check if this type is a compound type
    pub fn is_compound_type(&self) -> bool {
        matches!(self, Type::Array(_) | Type::Function(_, _) | Type::Tuple(_))
    }

    /// Get type name for display purposes
    pub fn type_name(&self) -> String {
        match self {
            Type::Int => "int".to_string(),
            Type::Float => "float".to_string(),
            Type::String => "string".to_string(),
            Type::Bool => "bool".to_string(),
            Type::Void => "void".to_string(),
            Type::None => "none".to_string(),
            Type::TypeVar(tv) => tv.name.clone(),
            Type::Array(ty) => format!("array<{}>", ty.type_name()),
            Type::Function(params, ret) => {
                let param_names = params
                    .iter()
                    .map(|p| p.type_name())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("fn({}) -> {}", param_names, ret.type_name())
            }
            Type::Tuple(tys) => {
                let type_names = tys
                    .iter()
                    .map(|t| t.type_name())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({})", type_names)
            }
            Type::Struct(def) => def.name.clone(),
            Type::Interface(def) => def.name.clone(),
            Type::Generic(r#gen) => {
                let param_names = r#gen
                    .params
                    .iter()
                    .map(|p| p.type_name())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}<{}>", r#gen.name, param_names)
            }
            Type::Alias(alias) => {
                format!("{} = {}", alias.name, alias.target.type_name())
            }
            Type::SumType(variants) => {
                let variant_names = variants
                    .iter()
                    .map(|v| v.name.clone())
                    .collect::<Vec<_>>()
                    .join(" | ");
                format!("enum<{}>", variant_names)
            }
        }
    }
}

impl Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.type_name())
    }
}

/// Conversion from String to Type for type name parsing
impl From<String> for Type {
    fn from(value: String) -> Self {
        match value.to_lowercase().as_str() {
            "int" => Type::Int,
            "float" => Type::Float,
            "string" => Type::String,
            "bool" => Type::Bool,
            "array" => Type::Void, // TODO: Handle typed arrays
            "void" => Type::Void,
            "none" => Type::None,
            _ => {
                // For unknown types, create a type variable
                let tv = TypeVar::new(0, &value);
                Type::TypeVar(tv)
            }
        }
    }
}
