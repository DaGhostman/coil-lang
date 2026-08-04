//! Stack VM, managed heap, and FFI runtime for coil bytecode.

pub mod char_ord;
#[cfg(feature = "crypto")]
pub mod crypto;
#[cfg(feature = "crypto")]
mod crypto_hasher_state;
#[cfg(any(test, feature = "debugger"))]
pub mod debug;
pub mod env;
mod ffi;
pub mod fs;
pub mod host_natives;
pub mod io;
mod memory;
mod opcode;
pub mod packed_la;
#[cfg(feature = "regex")]
pub mod regex;
#[cfg(feature = "regex")]
mod regex_state;
pub mod thread;
#[cfg(feature = "time")]
pub mod time;
#[cfg(feature = "tls")]
pub mod tls;
mod vm;

#[cfg(feature = "crypto")]
pub use crypto::{CRYPTO_WIRING, CryptoErrorTag};
#[cfg(any(test, feature = "debugger"))]
pub use debug::{DebugController, StepMode, StopReason};
pub use env::ENV_WIRING;
pub use ffi::*;
pub use fs::FS_WIRING;
pub use host_natives::{build_standard_host_natives, wire_standard_host_natives};
pub use memory::*;
pub use opcode::*;
pub use packed_la::{
    PACKED_DOT, PACKED_MATMUL, PACKED_MATRIX_NEG, PACKED_MATRIX_ZIP, packed_dot, packed_matmul,
    packed_matrix_neg, packed_matrix_zip,
};
#[cfg(feature = "regex")]
pub use regex::{REGEX_WIRING, RegexErrorTag};
pub use thread::{
    LiveThreadRegistry, ThreadErrorTag, ThreadProgram, join_undetached_threads,
    new_live_thread_registry,
};
#[cfg(feature = "time")]
pub use time::TIME_WIRING;
pub use vm::*;
