//! Stack VM, managed heap, and FFI runtime for zero-script bytecode.

mod ffi;
pub mod io;
mod memory;
mod opcode;
mod vm;

pub use ffi::*;
pub use memory::*;
pub use opcode::*;
pub use vm::*;
