use rkyv::{Archive, Deserialize, Serialize};

/// Current archive version. Bump this when the bytecode format
/// or `Byte` struct layout changes incompatibly.
///
/// v2: removed unused register-form opcode slots (discriminants
/// 55–85); subsequent opcodes renumbered. Recompile from source.
pub const ARCHIVE_VERSION: u32 = 2;

/// Versioned wrapper for serialized bytecode. Replaces the
/// pre-18C `ArchivedVec<ArchivedByte>` format.
#[derive(Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq))]
pub struct ArchivedProgram {
    pub version: u32,
    pub bytecode: Vec<Byte>,
}

// Re-export `Byte` so users of `ArchivedProgram` can refer to it
// without depending on the opcode module's internal structure.
pub use crate::opcode::Byte;
