//! Versioned bytecode archive format.

use rkyv::{Archive, Deserialize, Serialize};

/// Bump when bytecode encoding or `Byte` layout changes incompatibly.
pub const ARCHIVE_VERSION: u32 = 25;

/// Serialized program with constant pool and bytecode.
#[derive(Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq))]
pub struct ArchivedProgram {
    pub version: u32,
    /// Number of global static slots (`LoadStatic` / `StoreStatic`).
    pub static_slot_count: u32,
    /// Wide immediates (floats, large ints, jump targets, …).
    /// Referenced from `Byte.operands` via pool index or `Byte::POOL_FLAG`.
    pub constants: Vec<u64>,
    pub bytecode: Vec<Byte>,
}

pub use crate::opcode::Byte;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opcode::{Byte, Instruction};
    use rkyv::rancor::Error;

    #[test]
    fn byte_layout_is_eight_bytes() {
        use std::mem::{align_of, size_of};
        assert_eq!(size_of::<Byte>(), 8, "Byte must be 8 bytes for archive layout");
        assert_eq!(align_of::<Byte>(), 4);
        assert_eq!(size_of::<Instruction>(), 1);
    }

    #[test]
    fn archive_round_trip_preserves_bytecode_and_constants() {
        let program = ArchivedProgram {
            version: ARCHIVE_VERSION,
            static_slot_count: 0,
            constants: vec![1.5f64.to_bits(), 42],
            bytecode: vec![
                Byte::new(Instruction::CONST).with_const_inline(7),
                Byte::new(Instruction::HALT),
            ],
        };
        let bytes = rkyv::to_bytes::<Error>(&program).expect("serialize");
        let archived = rkyv::access::<ArchivedArchivedProgram, Error>(bytes.as_slice())
            .expect("access");
        assert_eq!(u32::from(archived.version), ARCHIVE_VERSION);
        let back: ArchivedProgram =
            rkyv::deserialize::<ArchivedProgram, Error>(archived).expect("deserialize");
        assert!(back == program);
        assert_eq!(back.version, program.version);
        assert_eq!(back.constants, program.constants);
        assert_eq!(back.bytecode.len(), program.bytecode.len());
    }

    #[test]
    fn archive_version_constant_is_positive() {
        assert!(ARCHIVE_VERSION >= 1);
    }
}
