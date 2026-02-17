# Q&A Log - Hindley-Milner Type System Implementation

## Questions for User

### Q1: Type System Design Philosophy
**Date:** 2026-02-13  
**Question:** Should the type system be strict (like Rust) or more permissive (like TypeScript)?

**Considerations:**
- Strict inference requires more annotations but catches more errors
- Permissive inference is easier to use but might hide bugs
- Current language has `let x: Int = 5` syntax, suggesting explicit annotations

**User Input:** 
- **Preference:** Rust-style - catch as much errors at compilation as possible
- **Function parameters:** Must have explicit type annotations (boundary types)
- **Local variables:** Should support type inference, but require explicit types when ambiguous

---

### Q2: Pattern Matching Syntax
**Date:** 2026-02-13  
**Question:** What syntax should pattern matching support?

**User Input:** 
- **Preference:** Mixed Rust+Scala style with `case` prefix for readability
- **Features needed (MUST):** Exhaustiveness checking, type guards, destructuring

---

### Q3: Sum Type Syntax
**Date:** 2026-02-13  
**Question:** How should sum types be declared?

**User Input:** 
- **Preference:** Rust-style enums (known at compile time)
- **Clarification:** `impl` syntax is placeholder for now, not set in stone

---

### Q4: Interface vs Trait Naming
**Date:** 2026-02-13  
**Question:** Should the language use "interface" or "trait" terminology?

**User Input:** 
- **Preference:** Use `interface` terminology
- **Features needed:** Hybrid interface/trait model
  - Default method implementations possible
  - Methods can be overridden in implementations
  - Interfaces can `extend` other interfaces (hierarchy: Circle -> Shape -> Geometry)
  - Classes cannot extend other classes (no inheritance)
  - Classes rely on composition instead

---

### Q5: Generic Syntax
**Date:** 2026-02-13  
**Question:** How should generics be declared?

**User Input:** 
- **Preference:** Rust-style syntax for generics
- **Applies to:** Both functions and classes/enums

---

### Q6: Runtime Type Information
**Date:** 2026-02-13  
**Question:** Does the VM need runtime type information (RTTI)?

**User Input:** 
- **Preference:** Compile-time type determination preferred
- **Approach:** Monomorphization at compile time
  - Different type invocations expanded in bytecode by per-type implementation
  - Appropriate jumps based on type inference at compile time
- **Goal:** Avoid runtime type information to prevent bytecode bloat

---

### Q7: Type Inference Scope
**Date:** 2026-02-13  
**Question:** Which expressions should support type inference?

**User Input:** 
- **Preference:** Inference as much as possible
- **Rules:** 
  - All definitions should be able to be inferred
  - Ambiguous resolution requires explicit type definitions

---

### Q8: Error Recovery Strategy
**Date:** 2026-02-13  
**Question:** How should the type checker recover from errors?

**User Input:** 
- **Preference:** Continue and report all errors
- **Features:**
  - Support for warnings (don't block processing)
  - Collect all errors before termination
  - Better UX - user sees all issues at once

---

## Decisions Made

### D1: Type System Architecture
**Date:** 2026-02-13  
**Decision:** Two-tier type checker system

**Rationale:**
- Keep current `typechecker.rs` for simple cases (backward compatibility)
- Add new `hm_typechecker.rs` for complex inference
- Gradual migration path

### D2: Type Variable Management
**Date:** 2026-02-13  
**Decision:** Use unique ID + name for TypeVar

**Rationale:**
- Unique ID ensures proper unification
- Name for readable error messages
- Thread-safe with proper ID generation

### D3: Constraint Solving Strategy
**Date:** 2026-02-13  
**Decision:** Bottom-up constraint generation with top-down solving

**Rationale:**
- Bottom-up: Generate constraints while traversing AST
- Top-down: Solve constraints from root type variables
- Separates concerns and improves error reporting

### D4: AST Modification Approach
**Date:** 2026-02-13  
**Decision:** Extend existing Expression enum

**Rationale:**
- Minimal parser changes
- Reuse existing span/position tracking
- Keep type information in AST for later processing

### D5: Memory Management
**Date:** 2026-02-13  
**Decision:** Use Arc<Type> for shared types

**Rationale:**
- Type checking may duplicate types
- Shared types improve memory usage
- Clone-on-write for mutable operations

### D6: Error Reporting Format
**Date:** 2026-02-13  
**Decision:** Use existing Message/Label system

**Rationale:**
- Consistent with parser errors
- Supports span-based reporting
- Already integrated with compiler

### D7: Generic Implementation Strategy
**Date:** 2026-02-13  
**Decision:** Monomorphization at compile time

**Rationale:**
- No runtime generic cost
- Type safe with no type erasure
- Similar to Rust's approach

### D8: Design Preferences Summary
**Date:** 2026-02-13  
**Decision:** Comprehensive design decisions based on user requirements

**Summary:**
- Rust-style strict type system with comprehensive inference
- Function parameters: explicit type annotations (boundary types)
- Local variables: type inference with explicit types required for ambiguity
- Pattern matching: mixed Rust+Scala style with `case` prefix
- Sum types: Rust-style enums (known at compile time)
- Interface terminology with hybrid trait features
- Default method implementations, override capability, interface hierarchy
- Classes cannot extend other classes (no inheritance), rely on composition
- Generic syntax: Rust-style `<T>` for both functions and classes/enums
- Runtime type information: compile-time monomorphization preferred
- Type inference: as much as possible, explicit types for ambiguity
- Error recovery: collect all errors, support warnings, better UX

### D9: Recursive Type Handling
**Date:** 2026-02-13  
**Decision:** Use Box<Type> for recursive type fields

**Rationale:**
- Rust cannot have infinitely sized recursive types without indirection
- `Box<Type>` provides heap allocation and fixed-size pointer
- Allows type inference to work with nested/recursive type structures

### D10: Type Module Naming
**Date:** 2026-02-13  
**Decision:** Rename type.rs to ty.rs

**Rationale:**
- `type` is Rust reserved keyword, cannot be used as module name
- `ty` is short, descriptive, and avoids conflicts
- Minimal changes to existing code structure

### D11: Substitution Implementation
**Date:** 2026-02-13  
**Decision:** Convert Substitution from type alias to struct

**Rationale:**
- Type aliases in Rust don't support impl blocks
- Struct wrapper provides proper encapsulation and method implementation
- More idiomatic Rust pattern for custom collection types

## Notes

- All decisions should be preserved in this file
- Add timestamps for traceability
- Include user input when available
- Document rationale for each decision

## Implementation Notes - 2026-02-13

### Q9: Recursive Type Handling
**Date:** 2026-02-13  
**Question:** How should recursive types be handled in Rust without causing infinite size errors?

**User Input:** 
- **Preference:** Use `Box<Type>` for recursive type fields

**Decision:** 
- Use `Box<Type>` for recursive type fields in `Array`, `Function.return_ty`, `Field.ty`, `GenericType.params`, `InterfaceDef.methods`, `TypeAlias.target`
- Convert `Substitution` from type alias to struct for proper method implementation

**Rationale:**
- Rust cannot have infinitely sized recursive types without indirection
- `Box<Type>` provides heap allocation and fixed-size pointer
- Allows type inference to work with nested/recursive type structures

### Q10: Type Module Name Conflict
**Date:** 2026-02-13  
**Question:** `type` is a reserved keyword in Rust, how to handle the type.rs module?

**User Input:** 
- **Preference:** Rename module to `ty.rs`

**Decision:** 
- Rename `compiler/src/types/type.rs` to `compiler/src/types/ty.rs`
- Update all references from `crate::types::type` to `crate::types::ty`
- Update `mod.rs` to use `pub mod ty`

**Rationale:**
- `type` is Rust reserved keyword, cannot be used as module name
- `ty` is short, descriptive, and avoids conflicts
- Minimal changes to existing code structure

### Q11: Substitution Type Alias Issue
**Date:** 2026-02-13  
**Question:** Rust type aliases don't support impl blocks, how to add methods to HashMap?

**User Input:** 
- **Preference:** Use struct wrapper around HashMap

**Decision:** 
- Convert `pub type Substitution = HashMap<TypeVar, Type>` to `pub struct Substitution { inner: HashMap<TypeVar, Type> }`
- Implement all methods on the struct

**Rationale:**
- Type aliases in Rust don't inherit impl blocks from underlying type
- Struct wrapper provides proper encapsulation and method implementation
- More idiomatic Rust pattern for custom collection types
### Q12-Q13: Function Call Type Resolution Strategy
**Date:** 2026-02-16  
**Issue:** The HM typechecker creates type variables for function calls but cannot resolve them to concrete types.

**User Input:**
- **Q12 Preference:** Option 1 - Store function return types in TypeEnv during compilation, look them up during call inference (for now, future improvements possible)
- **Q13 Preference:** HM typechecker maintains its own separate function registry (parallel to Compiler's registry) to track return types for type inference, avoiding confusion with bytecode jump targets

**Rationale:**
- TypeEnv is already has scope management and variable/function storage
- Separate registry keeps type checking concerns isolated from bytecode generation
- Simplest implementation for now, can evolve later if needed

### Q14: Function Body Variable Scope Management
**Date:** 2026-02-16  
**Issue:** Variables defined in function body go into function scope, then get lost when scope is popped.

**User Input:**
- Keep the scope mechanism (good for nested scopes, closures, etc.)
- Don't move variables to parent level (risk of overlaps)
- Store parameter types and return type in Function type in TypeEnv for later validation
- The Function type should contain: (params: Vec<Type>, return_ty: Type)
- Don't need to preserve local variables from function body after type checking

**Rationale:**
- Scope mechanism is correct for nested structures
- Function signatures only need parameter types and return type
- Local variables are transient and don't need to persist after type checking
- Store function signatures in TypeEnv for call resolution

**Solution:**
- Function handler in HM typechecker keeps scope for body processing
- After body is processed, extract parameter types and return type
- Store function signature in TypeEnv.functions with full type info
- Don't copy local variables to parent level
- Scope is popped after processing

### Q15: Function Return Type Inference
**Date:** 2026-02-16
**Question:** Should functions without explicit return type default to void or infer the type?

**Considerations:**
1. Default to void: Simpler implementation, explicit contract, catches missing return type annotations
2. Auto-inference: More convenient, less boilerplate, consistent with local variable inference

**User Input:**
- Check function signature to ensure return types match
- Inference should be implemented but with signature verification
- Function signature should be primary source of truth
- If no return type, auto-inference can be done

**Decision:**
- Function return type from declaration is primary source
- Infer from return statements and validate against declared type
- If no declaration, infer from return statements
- For `fn fib(int n) { ... }`, infer return type from `return` statements

**Implementation Approach:**
1. First pass: process function body, collect return types from return statements
2. If explicit return type exists, validate against inferred type
3. If no explicit return type, use inferred type
4. Handle multiple return statements with type unification

### Q16: Function Call Type Resolution (Current Session)
**Date:** 2026-02-17
**Issue:** Function calls with inferred return types don't resolve to concrete types

**User Input:**
- Use TypeEnv-based function registry for call resolution
- Function signatures stored as Type::Function(params: Vec<Type>, return_ty: Type)

**Rationale:**
- TypeEnv already has scope management and variable/function storage
- Separate registry keeps type checking concerns isolated from bytecode generation
- Simplest implementation for now, can evolve later if needed

**Decision:**
- Store function return types in TypeEnv during compilation
- Look them up during call inference
- Ensure Call expression resolves return types from TypeEnv

**Next Steps:**
1. Review how function signatures are registered in TypeEnv
2. Fix Call expression handling to resolve return types from TypeEnv
3. Test with various function signature patterns

**Session Complete (2026-02-17):**
- test.0s compiles and runs successfully
- Function call resolution working for functions with explicit return types
- fib(32) returns 2178309 correctly
- fizbuz(3/5/15) outputs fiz/buz/fizbuz correctly

### Q17: Match Exhaustiveness Checking
**Date:** 2026-02-17
**Decision:** Implement basic exhaustiveness checking for match expressions

### Q18: Sum Types Support
**Date:** 2026-02-17
**Decision:** Implement sum types with variant discriminants using `Type::Variant` syntax

**Implementation:**
- Variant syntax: `Color::Red` instead of just `Red`
- Sequential numeric discriminants assigned (Red=0, Green=1, Blue=2)
- HM typechecker handles variants with type variable generation
- Compiler tracks variant discriminants per sum type

**Rationale:**
- Clean, explicit syntax for variant access
- Type-safe variant dispatch via discriminant comparison
- Consistent with Rust's enum variant syntax

**Session Complete (2026-02-17 Session 2):**
- test.0s compiles and runs successfully with sum types
- Variant discriminants working correctly
- Match expressions working with numeric discriminants

**Implementation:**
- Pattern value extraction for integer, float, string, bool literals
- Default case detection for match expressions
- Basic exhaustiveness warnings for integer matches without default
- Type narrowing in match arms

**Rationale:**
- Early detection of incomplete matches
- Helps prevent runtime errors
- Consistent with Rust's match exhaustiveness checking

**Session 2 (2026-02-17):**
- Added exhaustiveness checking for match expressions in HmTypeChecker
- Pattern value extraction implemented for common literal types
- Default case detection implemented
- Basic warnings for integer matches without default
- Type narrowing implemented in match arms
- HM typechecker integrated and functional
