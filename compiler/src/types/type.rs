use std::fmt::Display;

use parser::SimpleSpan;
use common::Byte;

/// Type variable for Hindley-Milner type inference
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TypeVar {
    pub id: usize,
    pub name: String,
}

impl TypeVar {
    pub fn new(id: usize, name: &str) -> Self {
        Self {
            id,
            name: name.to_string(),
        }
    }
}

impl Display for TypeVar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// Field definition for structs
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub ty: Type,
    pub span: SimpleSpan,
}

impl Field {
    pub fn new(name: &str, ty: Type, span: SimpleSpan) -> Self {
        Self {
            name: name.to_string(),
            ty,
            span,
        }
    }
}

/// Method definition for interfaces
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Method {
    pub name: String,
    pub params: Vec<Type>,
    pub return_ty: Type,
    pub default_impl: Option<Vec<Byte>>,
    pub span: SimpleSpan,
}

impl Method {
    pub fn new(
        name: &str,
        params: Vec<Type>,
        return_ty: Type,
        span: SimpleSpan,
    ) -> Self {
        Self {
            name: name.to_string(),
            params,
            return_ty,
            default_impl: None,
            span,
        }
    }

    pub fn with_default_impl(&mut self, impl_bytes: Vec<Byte>) {
        self.default_impl = Some(impl_bytes);
    }
}

/// Structure definition for custom types
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<Field>,
    pub generics: Vec<TypeVar>,
    pub span: SimpleSpan,
}

impl StructDef {
    pub fn new(name: &str, fields: Vec<Field>, span: SimpleSpan) -> Self {
        Self {
            name: name.to_string(),
            fields,
            generics: Vec::new(),
            span,
        }
    }

    pub fn with_generics(&mut self, generics: Vec<TypeVar>) {
        self.generics = generics;
    }
}

/// Interface definition for contracts
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterfaceDef {
    pub name: String,
    pub methods: Vec<Method>,
    pub generics: Vec<TypeVar>,
    pub extends: Vec<String>,
    pub span: SimpleSpan,
}

impl InterfaceDef {
    pub fn new(name: &str, methods: Vec<Method>, span: SimpleSpan) -> Self {
        Self {
            name: name.to_string(),
            methods,
            generics: Vec::new(),
            extends: Vec::new(),
            span,
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeAlias {
    pub name: String,
    pub target: Type,
    pub generics: Vec<TypeVar>,
    pub span: SimpleSpan,
}

impl TypeAlias {
    pub fn new(name: &str, target: Type, span: SimpleSpan) -> Self {
        Self {
            name: name.to_string(),
            target,
            generics: Vec::new(),
            span,
        }
    }

    pub fn with_generics(&mut self, generics: Vec<TypeVar>) {
        self.generics = generics;
    }
}

/// Generic type for parameterized types
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenericType {
    pub name: String,
    pub params: Vec<Type>,
    pub span: SimpleSpan,
}

impl GenericType {
    pub fn new(name: &str, params: Vec<Type>, span: SimpleSpan) -> Self {
        Self {
            name: name.to_string(),
            params,
            span,
        }
    }
}

/// Core type representation for Hindley-Milner type system
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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

    // Compound types
    Array(Type),
    Function(Vec<Type>, Type), // (params, return)
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variant {
    pub name: String,
    pub fields: Vec<Field>,
    pub span: SimpleSpan,
}

impl Variant {
    pub fn new(name: &str, span: SimpleSpan) -> Self {
        Self {
            name: name.to_string(),
            fields: Vec::new(),
            span,
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
        Type::Array(ty)
    }

    /// Create a new function type
    pub fn function(params: Vec<Type>, return_ty: Type) -> Self {
        Type::Function(params, return_ty)
    }

    /// Create a new tuple type
    pub fn tuple(tys: Vec<Type>) -> Self {
        Type::Tuple(tys)
    }

    /// Create a new struct type
    pub fn struct_def(name: &str, fields: Vec<Field>, span: SimpleSpan) -> Self {
        let mut struct_def = StructDef::new(name, fields, span);
        Type::Struct(struct_def)
    }

    /// Create a new interface type
    pub fn interface_def(name: &str, methods: Vec<Method>, span: SimpleSpan) -> Self {
        let mut interface_def = InterfaceDef::new(name, methods, span);
        Type::Interface(interface_def)
    }

    /// Create a new generic type
    pub fn generic(name: &str, params: Vec<Type>, span: SimpleSpan) -> Self {
        Type::Generic(GenericType::new(name, params, span))
    }

    /// Create a new type alias
    pub fn alias(name: &str, target: Type, span: SimpleSpan) -> Self {
        let mut alias = TypeAlias::new(name, target, span);
        Type::Alias(alias)
    }

    /// Create a new sum type (enum)
    pub fn sum_type(name: &str, variants: Vec<Variant>, span: SimpleSpan) -> Self {
        // For now, we'll wrap variants in a struct-like type
        // This will be refined as we add more enum-specific features
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

    /// Check if this type is a compound type
    pub fn is_compound_type(&self) -> bool {
        matches!(
            self,
            Type::Array(_) | Type::Function(_, _) | Type::Tuple(_)
        )
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
            Type::Generic(gen) => {
                let param_names = gen
                    .params
                    .iter()
                    .map(|p| p.type_name())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}<{}>", gen.name, param_names)
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