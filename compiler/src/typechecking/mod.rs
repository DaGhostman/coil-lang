//! Hindley–Milner type inference for coil.
//!
//! Runs after parsing and before bytecode emission. Exposes [`Checker`]
//! for inference, native registration, and span-indexed type lookup.

pub mod aggregate_arith;
pub mod const_eval;
pub mod control_flow;
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

pub use aggregate_arith::{
    AggregateArithInfo, AggregateArithKind, AggregateOp, LinearAlgebraKind, ScalarSide,
};
#[allow(unused_imports)] // public API for Matrix helpers
pub use aggregate_arith::{is_matrix_ty, unwrap_matrix_ty, wrap_matrix_ty};
pub use infer::{CStructDef, CallbackSigDef, Checker, ForInInfo, ForInKind};
#[allow(unused_imports)] // public API for kind-aware callers / tests
pub use kind::Kind;
pub use ty::Ty;
pub use virtual_modules::{
    BuiltinExport, FfiBuiltin, IoBuiltin, PreludeFn, StringBuiltin, ThreadBuiltin, VirtualModules,
};
