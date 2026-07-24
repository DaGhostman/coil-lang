//! Self-contained executable packaging (append `.hyc` archive + trailer).

use crate::opcode::{Byte, Instruction};

/// Magic at the end of a packaged `coil` binary.
pub const PACKAGE_MAGIC: &[u8; 8] = b"COILAPP\0";

/// Trailer size in bytes (little-endian fields).
pub const PACKAGE_TRAILER_SIZE: usize = 32;

/// [`PackageTrailer::flags`]: program bytecode uses dynamic FFI (`dload` / `extern`).
pub const PACKAGE_FLAG_USES_FFI: u32 = 1;

/// Metadata stored in the last 32 bytes of a packaged executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackageTrailer {
    pub archive_offset: u64,
    pub archive_len: u64,
    pub flags: u32,
    pub archive_version: u32,
}

impl PackageTrailer {
    pub fn uses_ffi(self) -> bool {
        self.flags & PACKAGE_FLAG_USES_FFI != 0
    }

    pub fn encode(self) -> [u8; PACKAGE_TRAILER_SIZE] {
        let mut buf = [0u8; PACKAGE_TRAILER_SIZE];
        buf[..8].copy_from_slice(PACKAGE_MAGIC);
        buf[8..16].copy_from_slice(&self.archive_offset.to_le_bytes());
        buf[16..24].copy_from_slice(&self.archive_len.to_le_bytes());
        buf[24..28].copy_from_slice(&self.flags.to_le_bytes());
        buf[28..32].copy_from_slice(&self.archive_version.to_le_bytes());
        buf
    }

    pub fn decode(trailer: &[u8; PACKAGE_TRAILER_SIZE]) -> Option<Self> {
        if &trailer[..8] != PACKAGE_MAGIC {
            return None;
        }
        Some(Self {
            archive_offset: u64::from_le_bytes(trailer[8..16].try_into().ok()?),
            archive_len: u64::from_le_bytes(trailer[16..24].try_into().ok()?),
            flags: u32::from_le_bytes(trailer[24..28].try_into().ok()?),
            archive_version: u32::from_le_bytes(trailer[28..32].try_into().ok()?),
        })
    }
}

/// Read trailer from the end of `data`, if present.
pub fn read_package_trailer(data: &[u8]) -> Option<PackageTrailer> {
    if data.len() < PACKAGE_TRAILER_SIZE {
        return None;
    }
    let start = data.len() - PACKAGE_TRAILER_SIZE;
    let trailer: &[u8; PACKAGE_TRAILER_SIZE] = data[start..].try_into().ok()?;
    PackageTrailer::decode(trailer)
}

/// Slice of embedded archive bytes inside a packaged executable.
pub fn embedded_archive_slice(data: &[u8], trailer: PackageTrailer) -> Option<&[u8]> {
    let off = usize::try_from(trailer.archive_offset).ok()?;
    let len = usize::try_from(trailer.archive_len).ok()?;
    let end = off.checked_add(len)?;
    if end + PACKAGE_TRAILER_SIZE != data.len() {
        return None;
    }
    data.get(off..end)
}

/// Whether `data` already ends with a package trailer (template is already packaged).
pub fn is_packaged_executable(data: &[u8]) -> bool {
    read_package_trailer(data).is_some()
}

/// Append `archive` and trailer to `runner_bytes`, producing a packaged executable.
pub fn append_package_payload(
    runner_bytes: &[u8],
    archive: &[u8],
    flags: u32,
    archive_version: u32,
) -> Vec<u8> {
    let offset = u64::try_from(runner_bytes.len()).expect("runner too large");
    let len = u64::try_from(archive.len()).expect("archive too large");
    let mut out = runner_bytes.to_vec();
    out.extend_from_slice(archive);
    let trailer = PackageTrailer {
        archive_offset: offset,
        archive_len: len,
        flags,
        archive_version,
    };
    out.extend_from_slice(&trailer.encode());
    out
}

fn is_ffi_opcode(op: Instruction) -> bool {
    matches!(
        op,
        Instruction::FfiLoad | Instruction::FfiInvoke | Instruction::DeclareFFI | Instruction::NATIVE
    )
}

/// True when bytecode may call into shared libraries at runtime.
pub fn bytecode_uses_ffi(bytecode: &[Byte]) -> bool {
    bytecode.iter().any(|b| is_ffi_opcode(*b.bytecode()))
}

/// Decode a `STRING` + `DATA` literal starting at `i`. Returns `(text, index_after)`.
fn decode_string_literal(bytecode: &[Byte], i: usize) -> Option<(String, usize)> {
    let b = bytecode.get(i)?;
    if *b.bytecode() != Instruction::STRING {
        return None;
    }
    let count = b.operand_u32() as usize;
    let mut chars = String::with_capacity(count);
    let mut j = i + 1;
    for _ in 0..count {
        let data = bytecode.get(j)?;
        if *data.bytecode() != Instruction::DATA {
            return None;
        }
        let ch = char::from_u32(data.operand_u32())?;
        chars.push(ch);
        j += 1;
    }
    Some((chars, j))
}

/// Library names passed to `FfiLoad` (STRING literal immediately before each `FfiLoad`).
pub fn ffi_library_names_from_bytecode(bytecode: &[Byte]) -> Vec<String> {
    let mut names = Vec::new();
    let mut i = 0;
    while i < bytecode.len() {
        if let Some((name, end)) = decode_string_literal(bytecode, i) {
            if bytecode
                .get(end)
                .is_some_and(|b| *b.bytecode() == Instruction::FfiLoad)
                && !names.iter().any(|n| n == &name)
            {
                names.push(name);
            }
            i = end;
        } else {
            i += 1;
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opcode::Byte;

    #[test]
    fn trailer_round_trip() {
        let t = PackageTrailer {
            archive_offset: 1_234,
            archive_len: 56_789,
            flags: PACKAGE_FLAG_USES_FFI,
            archive_version: 26,
        };
        let enc = t.encode();
        assert_eq!(PackageTrailer::decode(&enc), Some(t));
    }

    #[test]
    fn append_and_read_embedded_archive() {
        let runner = b"#!/bin/fake\nELF...";
        let archive = b"rkyv-bytes-here";
        let out = append_package_payload(runner, archive, 0, 26);
        let trailer = read_package_trailer(&out).expect("trailer");
        assert_eq!(trailer.archive_offset, runner.len() as u64);
        assert_eq!(trailer.archive_len, archive.len() as u64);
        assert_eq!(
            embedded_archive_slice(&out, trailer),
            Some(archive.as_slice())
        );
    }

    #[test]
    fn detects_ffi_opcodes() {
        let bc = vec![
            Byte::new(Instruction::HALT),
            Byte::new(Instruction::FfiLoad),
        ];
        assert!(bytecode_uses_ffi(&bc));
        assert!(!bytecode_uses_ffi(&[Byte::new(Instruction::HALT)]));
    }

    #[test]
    fn extracts_dload_library_name_before_ffi_load() {
        let mut bc = Vec::new();
        bc.push(Byte::new(Instruction::STRING).with_operand_u32(3));
        for ch in "sum".chars() {
            bc.push(Byte::new(Instruction::DATA).with_operand_u32(ch as u32));
        }
        bc.push(Byte::new(Instruction::FfiLoad));
        assert_eq!(ffi_library_names_from_bytecode(&bc), vec!["sum".to_string()]);
    }
}
