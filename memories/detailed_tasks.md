# Detailed Task Breakdown with Implementation Details

## Phase 1: Foundation (Type System Core)

### Task 1.1: Design Core Type Representation
**Priority:** Critical  
**Dependencies:** None  
**Files to create/modify:**
- `compiler/src/types/type.rs` (new)
- `compiler/src/types/mod.rs` (new)
- `parser/src/ast.rs` (modify)

**Implementation Details:**
```rust
// New type.rs structure
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
    Alias(TypeAlias),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TypeVar {
    pub id: usize,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<Field>,
    pub generics: Vec<TypeVar>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InterfaceDef {
    pub name: String,
    pub methods: Vec<Method>,
    pub generics: Vec<TypeVar>,
}

// Add unification method
impl Type {
    pub fn unify(&self, other: &Type, substitution: &mut Substitution) -> UnifyResult
}
```

**Sub-tasks:**
1. [ ] Create Type enum with all variants
2. [ ] Implement TypeVar struct with unique ID
3. [ ] Implement StructDef and InterfaceDef
4. [ ] Create TypeAlias for type renaming
5. [ ] Implement `unify()` method for HM algorithm
6. [ ] Implement `Display` trait for debugging

### Task 1.2: Type Constraint System
**Priority:** Critical  
**Dependencies:** Task 1.1  
**Files to create:**
- `compiler/src/types/constraint.rs` (new)
- `compiler/src/types/substitution.rs` (new)
- `compiler/src/types/unify.rs` (new)

**Implementation Details:**
```rust
// constraint.rs
#[derive(Clone, Debug)]
pub struct Constraint {
    pub left: Type,
    pub right: Type,
    pub span: SimpleSpan,
}

pub struct ConstraintSet {
    pub constraints: Vec<Constraint>,
}

impl ConstraintSet {
    pub fn add(&mut self, left: Type, right: Type, span: SimpleSpan) {
        self.constraints.push(Constraint::new(left, right, span));
    }
    
    pub fn solve(&self) -> Result<Substitution, Vec<ConstraintError>>
}

// substitution.rs
pub type Substitution = HashMap<TypeVar, Type>;

impl Substitution {
    pub fn compose(&self, other: &Substitution) -> Substitution
    pub fn apply(&self, type: Type) -> Type
    pub fn extend(&mut self, var: TypeVar, ty: Type)
}

// unify.rs
pub fn unify_types(
    t1: &Type,
    t2: &Type,
    subst: &mut Substitution
) -> UnifyResult
```

**Sub-tasks:**
1. [ ] Create Constraint struct with span for error reporting
2. [ ] Implement ConstraintSet with add/solve methods
3. [ ] Create Substitution struct with compose/apply/extend
4. [ ] Implement Occur-check in unification
5. [ ] Implement Path compression for efficiency
6. [ ] Handle type variable instantiation
7. [ ] Add error reporting for failed unification

### Task 1.3: Type Environment
**Priority:** High  
**Dependencies:** Tasks 1.1, 1.2  
**Files to create:**
- `compiler/src/types/env.rs` (new)

**Implementation Details:**
```rust
pub struct TypeEnv {
    // Variable bindings: name -> type
    variables: HashMap<String, Type>,
    
    // Function signatures: name -> FunctionType
    functions: HashMap<String, Type>,
    
    // Type definitions: name -> Struct/Interface/Generic
    types: HashMap<String, Type>,
    
    // Type aliases: name -> Type
    aliases: HashMap<String, Type>,
    
    // Generic type parameters: type_name -> [TypeVar]
    generics: HashMap<String, Vec<TypeVar>>,
    
    // Scope stack for nested environments
    scopes: Vec<HashMap<String, Type>>,
}

impl TypeEnv {
    pub fn new() -> Self
    pub fn push_scope(&mut self)
    pub fn pop_scope(&mut self)
    pub fn define(&mut self, name: &str, ty: Type)
    pub fn lookup(&self, name: &str) -> Option<&Type>
    pub fn resolve_generics(&self, ty: Type) -> Type
}
```

**Sub-tasks:**
1. [ ] Implement basic TypeEnv with variables/functions/types
2. [ ] Add scope management (push/pop)
3. [ ] Implement generic type resolution
4. [ ] Add alias expansion
5. [ ] Support nested environments for function scopes

### Task 1.4: AST Enhancement for HM Features
**Priority:** High  
**Dependencies:** Task 1.1  
**Files to modify:**
- `parser/src/ast.rs` (modify)

**Implementation Details:**
Add new expression variants:
```rust
pub enum Expression<'expr> {
    // ... existing variants ...
    
    // Type system features
    TypeVar(&'expr str, usize),     // Type variable for inference
    SumType(Vec<Variant>),          // Union type
    Variant(&'expr str, Type),      // Sum type variant
    
    // Generics
    GenericDecl(Vec<TypeVar>, Output<'expr>),
    GenericCall(&'expr str, Vec<Type>),
    
    // Interfaces & OOP
    InterfaceDecl(&'expr str, Vec<Method>),
    StructDecl(&'expr str, Vec<Field>),
    ImplTrait(&'expr str, &'expr str), // trait implementation
    
    // Pattern matching
    MatchArm(TypePattern, Output<'expr>),
    TypePattern(Vec<FieldPattern>),
    FieldPattern(&'expr str, Option<Type>),
    
    // Custom types
    TypeAlias(&'expr str, Type),
    NewType(&'expr str, Type),
}
```

**Sub-tasks:**
1. [ ] Add TypeVar to Expression enum
2. [ ] Add SumType and Variant for algebraic data types
3. [ ] Add GenericDecl and GenericCall
4. [ ] Add InterfaceDecl and StructDecl
5. [ ] Add MatchArm and TypePattern for pattern matching
6. [ ] Add TypeAlias and NewType

**Parser Changes Required:**
- [ ] Add parser rules for `type X = Y` (alias)
- [ ] Add parser rules for `struct X { ... }`
- [ ] Add parser rules for `interface X { ... }`
- [ ] Add parser rules for `impl Trait for Type`
- [ ] Add parser rules for `match expr with { ... }`

## Phase 2: Type Checker Implementation

### Task 2.1: Core Type Checker
**Priority:** Critical  
**Dependencies:** Tasks 1.1-1.4  
**Files to create:**
- `compiler/src/hm_typechecker.rs` (new)
- `compiler/src/type_checker/mod.rs` (new)

**Implementation Details:**
```rust
pub struct HmTypeChecker {
    env: TypeEnv,
    constraints: ConstraintSet,
    type_vars: Vec<TypeVar>,
}

impl HmTypeChecker {
    pub fn new() -> Self
    pub fn check(&mut self, ast: Output) -> Result<Type, Vec<Message>>
    pub fn infer(&mut self, ast: Output) -> Result<Type, Vec<Message>>
    pub fn solve_constraints(&self) -> Result<Substitution, Vec<Message>>
}
```

**Sub-tasks:**
1. [ ] Initialize TypeEnv and ConstraintSet
2. [ ] Implement `check()` method for statement checking
3. [ ] Implement `infer()` method for expression inference
4. [ ] Implement `solve_constraints()` to run HM algorithm
5. [ ] Return type errors with proper spans

### Task 2.2: Expression Type Inference
**Priority:** High  
**Dependencies:** Task 2.1  
**Implementation Details:**
```rust
impl HmTypeChecker {
    pub fn infer_expr(&mut self, expr: Output) -> Result<Type, Vec<Message>> {
        match expr.borrow() {
            Expression::Integer(_) => Ok(Type::Int),
            Expression::Float(_) => Ok(Type::Float),
            Expression::String(_) => Ok(Type::String),
            Expression::Bool(_) => Ok(Type::Bool),
            
            Expression::Identifier(name) => {
                self.env.lookup(name).cloned()
                    .ok_or(Message::undefined_variable(name, span))
            },
            
            Expression::Add(lhs, rhs) => {
                let t1 = self.infer_expr(lhs)?;
                let t2 = self.infer_expr(rhs)?;
                
                // Generate constraint: t1 == t2 == Int|Float
                self.constraints.add(t1.clone(), t2.clone(), rhs.0);
                
                match t1 {
                    Type::Int | Type::Float => Ok(t1),
                    _ => Err(Message::type_error("numeric", t1, span)),
                }
            },
            
            // ... more cases
        }
    }
}
```

**Sub-tasks:**
1. [ ] Implement literal type inference (Int, Float, String, Bool)
2. [ ] Implement identifier type lookup
3. [ ] Implement arithmetic expression inference
4. [ ] Implement comparison expression inference
5. [ ] Implement logical expression inference
6. [ ] Implement call expression inference

### Task 2.3: Constraint Generation
**Priority:** High  
**Dependencies:** Task 2.2  
**Sub-tasks:**
1. [ ] Generate constraints for assignments
   - Example: `let x: Int = 5` generates constraint `type(x) == Int`
2. [ ] Generate constraints for function calls
   - Example: `f(x)` generates `type(x) == param_type(f)`
3. [ ] Generate constraints for return statements
   - Example: `return x` generates `type(x) == return_type(current_fn)`
4. [ ] Handle type variable instantiation
   - Create new TypeVar for undetermined types
   - Track dependencies for better error messages

## Phase 3: Advanced Features

### Task 3.1: Sum Types
**Priority:** High  
**Dependencies:** Tasks 2.1-2.3  
**Sub-tasks:**
1. [ ] Implement AST for sum types (`match` expression)
2. [ ] Add Variant type to Type enum
3. [ ] Implement exhaustiveness checking
   - All variants must be handled in match
   - Default case for unknown variants
4. [ ] Add type narrowing in match arms
5. [ ] Implement pattern matching type inference

### Task 3.2: Generics
**Priority:** High  
**Dependencies:** Tasks 2.1-2.3  
**Sub-tasks:**
1. [ ] Add GenericType to Type enum
2. [ ] Implement generic type instantiation
3. [ ] Add constraint solving for generics
4. [ ] Implement variance checking (covariant/contravariant)
5. [ ] Add generic type bounds

### Task 3.3: Interfaces & OOP
**Priority:** High  
**Dependencies:** Tasks 2.1-2.3  
**Sub-tasks:**
1. [ ] Add Interface type to Type enum
2. [ ] Implement interface conformance checking
   - Check that type implements all required methods
   - Check method signatures match
3. [ ] Add virtual method table generation
4. [ ] Implement trait bounds on generics
5. [ ] Add dynamic dispatch support

### Task 3.4: Custom Types
**Priority:** Medium  
**Dependencies:** Tasks 2.1-2.3  
**Sub-tasks:**
1. [ ] Implement TypeAlias expansion
2. [ ] Add NewType validation
3. [ ] Implement opaque type checking
4. [ ] Add type constraints for custom types

## Phase 4: Integration

### Task 4.1: Compiler Integration
**Priority:** Critical  
**Dependencies:** All previous phases  
**Files to modify:**
- `compiler/src/lib.rs` (modify)

**Implementation Details:**
```rust
pub struct Compiler {
    // ... existing fields ...
    
    // New HM type checker
    hm_checker: HmTypeChecker,
}

impl Compiler {
    fn typecheck<'check>(&mut self, ast: &(SimpleSpan, Box<Expression<'check>>)) -> Type {
        self.hm_checker.infer(ast)
            .unwrap_or(Type::Unknown)
    }
}
```

**Sub-tasks:**
1. [ ] Integrate HmTypeChecker with Compiler
2. [ ] Update bytecode generation to include type info
3. [ ] Add type-based optimizations
4. [ ] Implement runtime type checks

### Task 4.2: Error Reporting
**Priority:** High  
**Dependencies:** Task 4.1  
**Sub-tasks:**
1. [ ] Format constraint violation messages
2. [ ] Generate unification failure reports
3. [ ] Add type inference debugging info
4. [ ] Implement helpful suggestions

### Task 4.3: Testing
**Priority:** Critical  
**Dependencies:** Task 4.2  
**Sub-tasks:**
1. [ ] Unit tests for TypeEnv
2. [ ] Unit tests for constraint solving
3. [ ] Integration tests for examples
4. [ ] Performance benchmarks

## Phase 5: Documentation

### Task 5.1: API Documentation
**Priority:** Medium  
**Sub-tasks:**
1. [ ] Document Type enum variants
2. [ ] Document HmTypeChecker API
3. [ ] Add examples for each feature

### Task 5.2: User Examples
**Priority:** Medium  
**Sub-tasks:**
1. [ ] Sum type examples
2. [ ] Generic function examples
3. [ ] Interface implementation examples
4. [ ] Pattern matching examples

## Estimated Files Structure

```
compiler/
├── src/
│   ├── lib.rs              (modify - integrate HM checker)
│   ├── pipeline.rs         (modify - use new checker)
│   ├── typechecker.rs      (keep for backward compatibility)
│   ├── hm_typechecker.rs   (new - main HM implementation)
│   └── typechecking/
│       ├── mod.rs          (new - module exports)
│       ├── type.rs         (new - Type enum)
│       ├── constraint.rs   (new - constraint system)
│       ├── substitution.rs (new - type substitution)
│       ├── unify.rs        (new - unification algorithm)
│       ├── env.rs          (new - type environment)
│       └── error.rs        (new - error reporting)
```