//! Hindley–Milner type inference for zero-script.
//!
//! Runs after parsing and before bytecode emission. Exposes [`Checker`]
//! for inference, native registration, and span-indexed type lookup.

pub mod env;
pub mod id;
pub mod infer;
pub mod pretty;
pub mod subst;
pub mod ty;
pub mod unify;

pub use infer::Checker;
pub use ty::Ty;
