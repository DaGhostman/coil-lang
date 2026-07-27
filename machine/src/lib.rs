//! Stack VM, managed heap, and FFI runtime for coil bytecode.

mod ffi;
pub mod io;
pub mod thread;
mod memory;
mod opcode;
pub mod packed_la;
mod vm;

pub use ffi::*;
pub use memory::*;
pub use opcode::*;
pub use packed_la::{
    PACKED_DOT, PACKED_MATMUL, PACKED_MATRIX_NEG, PACKED_MATRIX_ZIP, packed_dot, packed_matmul,
    packed_matrix_neg, packed_matrix_zip,
};
pub use thread::{
    join_undetached_threads, new_live_thread_registry, LiveThreadRegistry, ThreadErrorTag,
    ThreadProgram,
};
pub use vm::*;
