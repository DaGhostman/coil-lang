//! Per-instruction debug locations shipped in `.c0s` archives.

use rkyv::{Archive, Deserialize, Serialize};

/// Sentinel `file` index: no source location (synthetic / unknown).
pub const DEBUG_FILE_UNKNOWN: u32 = u32::MAX;

/// One entry per bytecode slot (same index as VM `ip`).
#[derive(Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize, Debug, Default)]
#[rkyv(compare(PartialEq))]
pub struct DebugLoc {
    /// Index into [`ProgramDebug::source_files`].
    pub file: u32,
    pub start_byte: u32,
    pub end_byte: u32,
}

impl DebugLoc {
    pub const fn unknown() -> Self {
        Self {
            file: DEBUG_FILE_UNKNOWN,
            start_byte: 0,
            end_byte: 0,
        }
    }

    pub fn is_known(self) -> bool {
        self.file != DEBUG_FILE_UNKNOWN && self.start_byte < self.end_byte
    }
}

/// Debug sections loaded with bytecode (not the reporting `SourceMap`).
#[derive(Clone, PartialEq, Eq, Archive, Serialize, Deserialize, Debug, Default)]
#[rkyv(compare(PartialEq))]
pub struct ProgramDebug {
    pub source_files: Vec<String>,
    pub debug_locs: Vec<DebugLoc>,
}

impl ProgramDebug {
    pub fn empty_for_bytecode_len(len: usize) -> Self {
        Self {
            source_files: Vec::new(),
            debug_locs: vec![DebugLoc::unknown(); len],
        }
    }
}
