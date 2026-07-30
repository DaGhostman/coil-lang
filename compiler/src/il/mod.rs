//! Compile-time stack IL with symbolic labels.
//!
//! Codegen emits [`IlOp`]s (including [`IlOp::Label`] bind points and
//! label-targeted jumps). [`lower`] assigns PCs once, selecting fused
//! encodings along the way — no post-shrink jump relocation.

mod builder;
mod codebuf;
mod emit_buf;
mod lower;
mod op;
mod opt;

pub use builder::{IlBuilder, IlError};
pub use codebuf::CodeBuf;
pub use emit_buf::EmitBuf;
pub use lower::{Lowered, lower};
pub use op::{EntryKind, IlJumpKind, IlOp, Label};
pub use opt::{OptimizeOptions, optimize};
