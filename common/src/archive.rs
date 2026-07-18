//! Versioned bytecode archive format.

use rkyv::{Archive, Deserialize, Serialize};

/// Bump when bytecode encoding or `Byte` layout changes incompatibly.
pub const ARCHIVE_VERSION: u32 = 18;

/// Serialized program with constant pool and bytecode.
#[derive(Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq))]
pub struct ArchivedProgram {
    pub version: u32,
    /// Wide immediates (floats, large ints, jump targets, …).
    /// Referenced from `Byte.operands` via pool index or `Byte::POOL_FLAG`.
    pub constants: Vec<u64>,
    pub bytecode: Vec<Byte>,
}

pub use crate::opcode::Byte;
