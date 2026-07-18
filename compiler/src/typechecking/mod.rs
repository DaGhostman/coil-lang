//! Hindley–Milner type inference for zero-script.
//!
//! Runs after parsing and before bytecode emission. Exposes [`Checker`]
//! for inference, native registration, and span-indexed type lookup.

pub mod env;
pub mod generics;
pub mod id;
pub mod infer;
pub mod kind;
pub mod pretty;
pub mod subst;
pub mod ty;
pub mod unify;
pub mod virtual_modules;

pub use infer::{CStructDef, CallbackSigDef, Checker, ForInInfo, ForInKind};
#[allow(unused_imports)] // public API for kind-aware callers / tests
pub use kind::Kind;
pub use ty::Ty;
pub use virtual_modules::{BuiltinExport, FfiBuiltin, IoBuiltin, PreludeFn, VirtualModules};
