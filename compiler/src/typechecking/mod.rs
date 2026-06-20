//! Hindley–Milner type inference for zero-script.
//!
//! This module replaces the legacy structural typechecker at
//! `compiler/src/typechecker.rs`. It runs as a single pass after parsing
//! and before bytecode emission (see `HM_TYPECHECKER_PLAN.md`).
//!
//! ## Phase status
//!
//! - **Phase 1 (this commit)**: type representation (`Ty`, `Scheme`,
//!   `TyVarId`), substitution (`Subst`, `apply`, `compose`, `union`,
//!   `ftv`), and pretty-printing.
//! - Phase 2: unification (Robinson + occurs check).
//! - Phase 3: environments and generalization (let-polymorphism).
//! - Phase 4: inference over the AST (Algorithm W).
//! - Phase 5: recursion, classes, `impl`, `self`.
//! - Phase 6: span-indexed cache for the bytecode emitter.
//! - Phase 7: native-function registration.
//! - Phase 8: diagnostics.
//! - Phase 9: wire-up to `Compiler`.
//! - Phase 10: removal of the legacy typechecker.

pub mod env;
pub mod id;
pub mod infer;
pub mod pretty;
pub mod subst;
pub mod ty;
pub mod unify;

// Re-export the small public surface used by later phases and by tests.
// The `Display` impls on `Ty` and `Scheme` live in `pretty` but are pulled
// in automatically when callers `use` the types.
pub use infer::Checker;
pub use ty::Ty;