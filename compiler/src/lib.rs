mod attrs;
mod block_builder;
mod const_fold;
#[cfg(any(test, feature = "dissect"))]
mod dissect;
mod il;
mod manifest;
mod monomorphize;
mod pipeline;
mod strip_tests;
mod typechecking;
#[macro_use]
mod codegen;

#[cfg(any(test, feature = "dissect"))]
pub use dissect::{
    DissectArtifacts, FnSym, IlSnapshot, filter_symbols, format_bytecode, format_bytecode_section,
    format_il, format_symbol_index, matches_fn_pat,
};
pub use pipeline::*;
pub use reporting::{ErrorCode, Label, Message, MessageKind};
pub use typechecking::{
    BuiltinExport, CStructDef, CallbackSigDef, Checker, FfiBuiltin, ForInInfo, ForInKind, Ty,
    VirtualModules,
};

pub use codegen::{Compiler, PROLOGUE_BYTECODE_LEN, unescape_coil_string};
