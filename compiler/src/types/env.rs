use std::collections::HashMap;

use parser::SimpleSpan;

use crate::types::ty::{
    Field, GenericSignature, InterfaceDef, Method, StructDef, Type, TypeVar, Variant,
};

/// Type environment for Hindley-Milner type checking
#[derive(Clone)]
pub struct TypeEnv {
    /// Variable bindings: name -> type
    variables: HashMap<String, Type>,

    /// Function signatures: name -> type
    functions: HashMap<String, Type>,

    /// Type definitions: name -> type
    types: HashMap<String, Type>,

    /// Type aliases: name -> type
    aliases: HashMap<String, Type>,

    /// Generic type parameters: type_name -> [TypeVar]
    generics: HashMap<String, Vec<TypeVar>>,

    /// Generic function signatures: name -> GenericSignature
    /// Used for monomorphisation of generic functions
    generic_signatures: HashMap<String, GenericSignature>,

    /// Instantiated generic functions: (name, args_string) -> instantiated_type
    /// Cache for monomorphised functions
    instantiated_generics: HashMap<String, Type>,

    /// Interface implementations: type_name -> [interface_name]
    implementations: HashMap<String, Vec<String>>,

    /// Scope stack for nested environments
    scopes: Vec<HashMap<String, Type>>,
}

impl TypeEnv {
    /// Create a new empty type environment
    pub fn new() -> Self {
        Self {
            variables: HashMap::new(),
            functions: HashMap::new(),
            types: HashMap::new(),
            aliases: HashMap::new(),
            generics: HashMap::new(),
            generic_signatures: HashMap::new(),
            instantiated_generics: HashMap::new(),
            implementations: HashMap::new(),
            scopes: Vec::new(),
        }
    }

    /// Push a new scope
    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    /// Pop the current scope
    pub fn pop_scope(&mut self) -> Option<HashMap<String, Type>> {
        self.scopes.pop()
    }

    /// Define a variable with a type
    pub fn define_variable(&mut self, name: &str, ty: Type) {
        // Check current scope first
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), ty);
        } else {
            self.variables.insert(name.to_string(), ty);
        }
    }

    /// Define a function with a type
    pub fn define_function(&mut self, name: &str, ty: Type) {
        self.functions.insert(name.to_string(), ty);
    }

    /// Define a type (struct, interface, etc.)
    pub fn define_type(&mut self, name: &str, ty: Type) {
        self.types.insert(name.to_string(), ty);
    }

    /// Define an alias
    pub fn define_alias(&mut self, name: &str, ty: Type) {
        self.aliases.insert(name.to_string(), ty);
    }

    /// Define generic parameters for a type
    pub fn define_generics(&mut self, name: &str, params: Vec<TypeVar>) {
        self.generics.insert(name.to_string(), params);
    }

    /// Define a generic function signature
    pub fn define_generic_signature(&mut self, name: &str, signature: GenericSignature) {
        self.generic_signatures.insert(name.to_string(), signature);
    }

    /// Lookup a generic function signature
    pub fn lookup_generic_signature(&self, name: &str) -> Option<&GenericSignature> {
        self.generic_signatures.get(name)
    }

    /// Get or instantiate a generic function with concrete type arguments
    pub fn instantiate_generic(&mut self, name: &str, args: &[Type]) -> Option<Type> {
        // Create a string key from name and type arguments
        let type_args_str: Vec<String> = args.iter().map(|t| t.type_name()).collect();
        let key = format!("{}<{}>", name, type_args_str.join(", "));

        // Check cache first
        if let Some(cached) = self.instantiated_generics.get(&key) {
            return Some(cached.clone());
        }

        // Get the generic signature and instantiate
        if let Some(signature) = self.generic_signatures.get(name) {
            let instantiated = signature.instantiate(args);
            self.instantiated_generics.insert(key, instantiated.clone());
            return Some(instantiated);
        }

        None
    }

    /// Clear the instantiated generics cache (for fresh compilation)
    pub fn clear_instantiated_generics(&mut self) {
        self.instantiated_generics.clear();
    }

    /// Define an implementation (trait/trait-like)
    pub fn define_implementation(&mut self, type_name: &str, interface_name: &str) {
        self.implementations
            .entry(type_name.to_string())
            .or_insert(Vec::new())
            .push(interface_name.to_string());
    }

    /// Lookup a type by name
    pub fn lookup(&self, name: &str) -> Option<&Type> {
        // Check current scope first
        if let Some(scope) = self.scopes.last() {
            if let Some(ty) = scope.get(name) {
                return Some(ty);
            }
        }

        // Check variables
        if let Some(ty) = self.variables.get(name) {
            return Some(ty);
        }

        // Check functions
        if let Some(ty) = self.functions.get(name) {
            return Some(ty);
        }

        // Check types
        if let Some(ty) = self.types.get(name) {
            return Some(ty);
        }

        // Check aliases
        if let Some(ty) = self.aliases.get(name) {
            return Some(ty);
        }

        None
    }

    /// Resolve generic types
    pub fn resolve_generics(&self, ty: Type) -> Type {
        // For now, just return the type
        // This will be implemented with full generic resolution
        ty
    }

    /// Get all variable bindings
    pub fn variables(&self) -> &HashMap<String, Type> {
        &self.variables
    }

    pub fn scope(&self) -> Option<&HashMap<String, Type>> {
        self.scopes.last()
    }

    /// Get all function signatures
    pub fn functions(&self) -> &HashMap<String, Type> {
        &self.functions
    }

    /// Get all type definitions
    pub fn types(&self) -> &HashMap<String, Type> {
        &self.types
    }

    /// Get all generic parameters
    pub fn generics(&self) -> &HashMap<String, Vec<TypeVar>> {
        &self.generics
    }

    /// Get all generic signatures
    pub fn generic_signatures(&self) -> &HashMap<String, GenericSignature> {
        &self.generic_signatures
    }

    /// Get all implementations
    pub fn implementations(&self) -> &HashMap<String, Vec<String>> {
        &self.implementations
    }
}

impl Default for TypeEnv {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper struct for struct definitions
pub struct StructBuilder {
    name: String,
    fields: Vec<Field>,
    generics: Vec<TypeVar>,
    span: SimpleSpan,
}

impl StructBuilder {
    pub fn new(name: &str, span: SimpleSpan) -> Self {
        Self {
            name: name.to_string(),
            fields: Vec::new(),
            generics: Vec::new(),
            span,
        }
    }

    pub fn field(&mut self, name: &str, ty: Type) -> &mut Self {
        self.fields.push(Field::new(name, ty));
        self
    }

    pub fn with_generics(&mut self, generics: Vec<TypeVar>) -> &mut Self {
        self.generics = generics;
        self
    }

    pub fn build(&self) -> StructDef {
        let mut def = StructDef::new(&self.name, self.fields.clone());
        def.with_generics(self.generics.clone());
        def
    }
}

/// Helper struct for interface definitions
pub struct InterfaceBuilder {
    name: String,
    methods: Vec<Method>,
    generics: Vec<TypeVar>,
    extends: Vec<String>,
    span: SimpleSpan,
}

impl InterfaceBuilder {
    pub fn new(name: &str, span: SimpleSpan) -> Self {
        Self {
            name: name.to_string(),
            methods: Vec::new(),
            generics: Vec::new(),
            extends: Vec::new(),
            span,
        }
    }

    pub fn method(&mut self, name: &str, params: Vec<Type>, return_ty: Type) -> &mut Self {
        self.methods.push(Method::new(name, params, return_ty));
        self
    }

    pub fn with_generics(&mut self, generics: Vec<TypeVar>) -> &mut Self {
        self.generics = generics;
        self
    }

    pub fn extends(&mut self, interfaces: Vec<&str>) -> &mut Self {
        self.extends = interfaces.iter().map(|s| s.to_string()).collect();
        self
    }

    pub fn build(&self) -> InterfaceDef {
        let mut def = InterfaceDef::new(&self.name, self.methods.clone());
        def.with_generics(self.generics.clone());
        def.extends(self.extends.iter().map(|s| s.as_str()).collect());
        def
    }
}

/// Helper struct for variant definitions (for sum types)
pub struct VariantBuilder {
    name: String,
    fields: Vec<Field>,
    span: SimpleSpan,
}

impl VariantBuilder {
    pub fn new(name: &str, span: SimpleSpan) -> Self {
        Self {
            name: name.to_string(),
            fields: Vec::new(),
            span,
        }
    }

    pub fn field(&mut self, name: &str, ty: Type) -> &mut Self {
        self.fields.push(Field::new(name, ty));
        self
    }

    pub fn build(&self) -> Variant {
        let mut variant = Variant::new(&self.name);
        variant.with_fields(self.fields.clone());
        variant
    }
}
