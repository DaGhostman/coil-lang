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