//! Stack VM, managed heap, and FFI runtime for coil bytecode.

mod ffi;
#[cfg(feature = "crypto")]
pub mod crypto;
#[cfg(feature = "crypto")]
mod crypto_hasher_state;
pub mod env;
pub mod fs;
pub mod io;
pub mod char_ord;
#[cfg(feature = "regex")]
pub mod regex;
#[cfg(feature = "regex")]
mod regex_state;
pub mod thread;
#[cfg(feature = "time")]
pub mod time;
#[cfg(feature = "tls")]
pub mod tls;
mod memory;
mod opcode;
pub mod packed_la;
mod vm;

#[cfg(feature = "crypto")]
pub use crypto::{CryptoErrorTag, CRYPTO_WIRING};
pub use env::ENV_WIRING;
pub use fs::FS_WIRING;
#[cfg(feature = "regex")]
pub use regex::{RegexErrorTag, REGEX_WIRING};
#[cfg(feature = "time")]
pub use time::TIME_WIRING;
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
