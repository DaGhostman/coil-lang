# Project Context: Zero-Script Language

## Current Status (as of 2026-02-13)
- Language: Rust-based scripting language with a Pratt parser
- Directory structure:
  - `parser/` - PRATT parser handling code -> AST generation
  - `compiler/` - Bytecode compiler with a prototype type checker
  - `common/` - Shared utilities (Value, Message, Opcode, etc.)
  - `machine/` - VM implementation
  - `src/` - Main entry point

## Current AST Support (parser/src/ast.rs)
- Basic types: Integer, Float, String, Bool
- Expressions: Arithmetic, Comparison, Logical, Bitwise
- Control flow: If, Loop, Match
- Functions: Declaration, Calls
- OOP: Class, Implementation, Field, Method, Instantiate
- Variables: Variable, Constant, Assignment

## Current Type System (compiler/src/typechecker.rs)
- Basic types: UNKNOWN, NONE, INTEGER, FLOAT, STRING, BOOLEAN, LIST, OBJECT
- Function registration and checking
- Variable type inference
- Expression type checking
- Basic return type checking

## Target Features for HM Type System
1. Pattern matching & sum types
2. Generics
3. OOP enhancements:
   - Structs
   - Interfaces (method contracts)
4. Custom types

## Implementation Status
- Initial prototype exists but needs significant enhancement for full HM type system
- Missing: Algebraic data types, type inference algorithms, trait/interface support, generic type checking