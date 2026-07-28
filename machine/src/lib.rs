//! Stack VM, managed heap, and FFI runtime for coil bytecode.

mod ffi;
pub mod crypto;
mod crypto_hasher_state;
pub mod env;
pub mod fs;
pub mod io;
pub mod char_ord;
pub mod regex;
mod regex_state;
pub mod thread;
pub mod time;
mod memory;
mod opcode;
pub mod packed_la;
mod vm;

pub use crypto::{CryptoErrorTag, CRYPTO_WIRING};
pub use regex::{RegexErrorTag, REGEX_WIRING};
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
