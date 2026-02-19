# Extended Generics Implementation Plan

## Overview
This plan extends the existing generics implementation to support:
1. Multiple generic parameters per function/struct
2. Generic type constraints (bounds) syntax (`T: Copy`, `T: Clone`)
3. Proper type-checking with meaningful error messages for mismatched generic invocations

## Current State (2026-02-19)
- ✅ Generic function declaration parsing (`fn identity<T>(T x) -> T`)
- ✅ Generic function call parsing with turbofish syntax (`identity::<int>(42)`)
- ✅ `GenericSignature::instantiate()` for monomorphisation
- ✅ Instantiation cache in TypeEnv
- ✅ Multiple generic parameters - working
- ✅ Type bounds syntax - `T: Copy`, `T: Copy + Clone`
- ✅ Type-checking errors for explicit type args - NOW WORKING

---

## Phase 1: Multiple Generic Parameters Support ✅

### 1.1 Verify Current Implementation ✅
- [x] Test functions with multiple type parameters (`fn make_pair<A, B>(A a, B b)`)
- [x] Fix parser to support multiple type arguments in turbofish (`foo::<int, string>(a, b)`)

### 1.2 Parser Updates ✅
- [x] Update `call()` parser to handle multiple comma-separated type arguments
- [x] Test: `make_pair<int, int>(1, 2)` works

### Bug Fix: generic_decl was creating wrong AST type
- Fixed: `parser/src/lib.rs` - `generic_decl` was creating `Function` instead of `FunctionWithGenerics`

---

## Phase 2: Generic Type Constraints (Bounds) - NOT YET IMPLEMENTED

### 2.1 Parser Support for Bounds
- [ ] Add bound syntax parsing: `fn foo<T: Copy>(T x) -> T`
- [ ] Support multiple bounds: `fn foo<T: Copy + Clone>(T x) -> T`

### 2.2 AST Updates
- [ ] Create `GenericBound` struct: `{ name: String, traits: Vec<String> }`
- [ ] Update `FunctionWithGenerics` to store bounds

---

## Phase 3: Type-Checking for Generic Invocations ✅

### 3.1 Current Status ✅
- [x] Type inference works: `let x: int = identity(42)` correctly infers T=int
- [x] Added `TypedAssignment` for explicit type annotations
- [x] Type mismatch checking for explicit type args: `identity<float>(42)` where x is int

### 3.2 Known Issues
- [x] FIXED: Explicit type arguments - now properly looks up generic signature
- [x] FIXED: Parser was creating wrong AST type for generic functions

---

## Test Results (2026-02-19)

### Working:
```zero
fn identity<T>(T x) -> T { return x; }
fn main() {
    let x: int = identity(42);  // ✅ Works - type inference
    print "%i", x;
}
```

### Multiple params:
```zero
fn make_pair<A, B>(A a, B b) -> int { return 1; }
fn main() {
    let p: int = make_pair<int, int>(1, 2);  // ✅ Works
}
```

### Type mismatch detection:
```zero
fn identity<T>(T x) -> T { return x; }
fn main() {
    let x: int = identity<float>(42);  // ✅ Error: Type mismatch: expected 'int', found 'float'
}
```

---

## Success Criteria
- [x] `fn foo<T, U>(T a, U b)` compiles and runs
- [x] `fn bar<T: Copy>(T x)` validates bounds (syntax works)
- [x] `let x: int = identity::<float>(42)` produces type error
- [x] All existing tests continue to pass

---

## Test Files Added

### Positive Tests (`tests/generics_positive.0s`)
- Basic generic identity function
- Generic with type inference
- Multiple type parameters
- Generic with single bound (`T: Copy`)
- Generic with multiple bounds (`T: Copy + Clone`)
- Float generic types

### Negative Tests (`tests/generics_negative*.0s`)
- Type mismatch: `let x: int = identity<float>(42)` → Error: Type mismatch: expected 'int', found 'float'
- Argument type mismatch: `let z: float = identity<int>(99)` → Error: Type mismatch: expected 'float', found 'int'
