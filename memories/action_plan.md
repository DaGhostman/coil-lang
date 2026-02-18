# Hindley-Milner Type System Implementation Plan

## Overview
This plan breaks down the implementation of a complete Hindley-Milner type system for the Zero-Script language, supporting sum types, generics, interfaces, and OOP features.

## Design Decisions (from Q&A)
- **Type system:** Rust-style strict with comprehensive inference
- **Syntax:** Rust-style generics (`<T>`), mixed Rust+Scala pattern matching (`case` prefix)
- **Sum types:** Rust-style enums (known at compile time)
- **Interfaces:** Hybrid interface/trait model with default implementations, interface hierarchy
- **Classes:** No inheritance, rely on composition
- **Error handling:** Collect all errors, support warnings for better UX
- **RTTI:** Compile-time monomorphization, no runtime type information
- **Inference:** As much as possible, explicit types required for ambiguity

## Phase 1: Foundation (Type System Core)

### Task 1.1: Design Core Type Representation
**Date:** __/__/
- [ ] Create `Type` enum with:
  - Basic types: `Int`, `Float`, `String`, `Bool`, `Void`, `None`
  - Type variables: `TypeVar(String, Id)` for inference
  - Compound types: `Array(Type)`, `Function(Vec<Type>, Type)`
  - User-defined: `Struct(String, Vec<Field>)`, `Interface(String, Vec<Method>)`
- [ ] Implement `Type::unify()` for Hindley-Milner unification
- [ ] Add type alias support: `TypeAlias(String, Type)`
- [ ] Implement `Display` trait for human-readable type names

### Task 1.2: Type Constraint System
**Date:** __/__/
- [ ] Create `Constraint` struct: `(Type, Type, Span)`
- [ ] Implement `ConstraintSet` with methods:
  - `add(constraint)`
  - `solve()` - returns substitution map
  - `check()` - validates all constraints
- [ ] Implement Hindley-Milner unification algorithm:
  - Variable elimination
  - Occur-check
  - Path compression

### Task 1.3: Type Environment
**Date:** __/__/
- [ ] Create `TypeEnv` struct:
  - Variable bindings: `HashMap<String, Type>`
  - Function signatures: `HashMap<String, FunctionType>`
  - Type definitions: `HashMap<String, Type>`
  - Generic parameters: `HashMap<String, Vec<TypeVar>>`
- [ ] Implement scope management (nested environments)
- [ ] Add trait/environment support for interfaces

### Task 1.4: AST Enhancement for HM Features
**Date:** __/__/
- [ ] Add `TypeVar` to Expression enum
- [ ] Add `SumType`, `Variant` for algebraic data types
- [ ] Add `GenericDecl`, `GenericCall`
- [ ] Add `InterfaceDecl`, `StructDecl`
- [ ] Add `MatchArm`, `TypePattern` for pattern matching
- [ ] Add `TypeAlias`, `NewType`

## Phase 2: Type Checker Implementation

### Task 2.1: Core Type Checker
**Date:** __/__/
- [ ] Implement `TypeChecker::infer_expr()` for expression type inference
- [ ] Implement `TypeChecker::check_function()` for function type checking
- [ ] Implement `TypeChecker::check_return()` for return type validation
- [ ] Implement `TypeChecker::check_call()` for function call validation

### Task 2.2: Expression Type Inference
**Date:** __/__/
- [ ] Arithmetic expressions: infer numeric type
- [ ] Logical expressions: infer Bool
- [ ] Comparison expressions: infer Bool
- [ ] Call expressions: infer from function signature
- [ ] Variable expressions: infer from environment
- [ ] Lambda expressions: infer from parameter types

### Task 2.3: Constraint Generation
**Date:** __/__/
- [ ] Generate constraints for assignments
- [ ] Generate constraints for function calls
- [ ] Generate constraints for return statements
- [ ] Handle type variable instantiation

## Phase 3: Advanced Features

### Task 3.1: Sum Types
**Date:** 2026-02-13 (Started) / 2026-02-17 (Updated)
- [x] Add `Expression::Variant(Name, Type)` to AST - ✅ (Added to parser/src/ast.rs)
- [x] Add `Expression::VariantItem(Type, Variant)` - ✅ (For Type::Variant syntax)
- [x] Add `Expression::MatchBranch(TypePattern, Body)` - ✅ (MatchArm implemented)
- [x] Implement `TypeChecker::check_sum_type()` - ✅ (Variant discriminant handling in compiler)
- [x] Implement `TypeChecker::check_match()` - ✅ (With type narrowing)
- [x] Add exhaustiveness checking for match expressions - ✅ (Basic level implemented)
- [x] Type narrowing in match arms - ✅ (Implemented)
- [x] Add `Expression::VariantWithDestructure(Type, Name, Vec<Field>)` - ✅ (For match patterns)
- [x] Comma-separated variants in same arm - ✅ (Parser support)
- [x] Destructuring pattern binding - ✅ (Type inference implemented)
- [ ] Fix variant construction with values (`Result::Ok(42)`) - In Progress
- [ ] Implement runtime sum type representation - In Progress
- [ ] Implement heap-based variant storage for variants with fields - In Progress

### Task 3.2: Generics
**Date:** __/__/
- [ ] Add `Expression::GenericDecl(TypeVar, Body)`
- [ ] Add `Expression::GenericCall(Name, Vec<Type>)`
- [ ] Implement generic type instantiation
- [ ] Add constraint solving for generics
- [ ] Implement `TypeChecker::check_generic()`

### Task 3.3: Interfaces & OOP
**Date:** __/__/
- [ ] Add `Expression::InterfaceDecl(Name, Vec<Method>)`
- [ ] Add `Expression::StructDecl(Name, Vec<Field>)`
- [ ] Implement interface conformance checking
- [ ] Add `TypeChecker::check_impl()`
- [ ] Add virtual table generation for interface dispatch

### Task 3.4: Custom Types
**Date:** __/__/
- [ ] Add `Expression::TypeAlias(Name, Type)`
- [ ] Add `Expression::NewType(Name, Type)`
- [ ] Implement type alias expansion
- [ ] Add custom type validation

## Phase 4: Integration

### Task 4.1: Compiler Integration
**Date:** 2026-02-13 (Started) / 2026-02-17 (Updated)
- [x] Integrate HmTypeChecker with Compiler - ✅ (Added hm_typechecker field to Compiler)
- [ ] Fix Call expression type resolution for inferred return types - In Progress
- [ ] Update bytecode generation to include type information
- [ ] Implement type-based optimizations
- [ ] Add runtime type checks for dynamic operations

### Task 4.2: Error Reporting
**Date:** __/__/
- [ ] Implement detailed type error messages
- [ ] Add constraint violation reporting
- [ ] Implement type inference failure diagnostics
- [ ] Add suggestions for type mismatches

### Task 4.3: Testing
**Date:** __/__/
- [ ] Create unit tests for each component
- [ ] Create integration tests for example programs
- [ ] Test all target features (sum types, generics, interfaces)
- [ ] Performance testing for type checking

## Phase 5: Documentation & Examples

### Task 5.1: Documentation
**Date:** __/__/
- [ ] Write API documentation for type system
- [ ] Create user guide for new features
- [ ] Add examples for each feature

### Task 5.2: Examples
**Date:** __/__/
- [ ] Create sum type examples
- [ ] Create generic examples
- [ ] Create interface examples
- [ ] Create OOP examples

## Implementation Notes

### Constraints for HM Algorithm
1. Type variables must be unique (use IDs)
2. Occur-check prevents infinite types (T = List<T>)
3. Unification must be bottom-up (from leaves to root)
4. Constraint generation must be single-pass

### Performance Considerations
1. Use `Arc<Type>` for shared types
2. Memoize type unification results
3. Use incremental type checking for IDE support
4. Consider type inference caching

### Backward Compatibility
1. Keep current `Typechecker` for simple cases
2. Gradually migrate to new HM implementation
3. Support implicit type annotations as before
4. Maintain existing error message format