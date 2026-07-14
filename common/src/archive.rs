use rkyv::{Archive, Deserialize, Serialize};

/// Current archive version. Bump this when the bytecode format
/// or `Byte` struct layout changes incompatibly.
///
/// v4: 8-byte `Byte` (dropped `value` field); wide immediates
/// live in the constant pool. Recompile from source.
pub const ARCHIVE_VERSION: u32 = 4;

/// Versioned wrapper for serialized bytecode. Replaces the
/// pre-18C `ArchivedVec<ArchivedByte>` format.
#[derive(Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq))]
pub struct ArchivedProgram {
    pub version: u32,
    /// Wide immediates: float bits, large ints, `JumpIfMatch`
    /// targets, etc. Referenced from `Byte.operands` via pool
    /// index or `Byte::POOL_FLAG`.
    pub constants: Vec<u64>,
    pub bytecode: Vec<Byte>,
}

// Re-export `Byte` so users of `ArchivedProgram` can refer to it
// without depending on the opcode module's internal structure.
pub use crate::opcode::Byte;
