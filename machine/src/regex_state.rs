//! Compiled PCRE2 pattern handle for host `Regex` values.

use pcre2::bytes::Regex;

use crate::memory::GcSized;

/// Opaque compiled regex on the VM heap.
pub struct ObjRegex {
    pub re: Regex,
}

impl ObjRegex {
    pub fn new(re: Regex) -> Self {
        Self { re }
    }
}

impl GcSized for ObjRegex {
    fn size(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}
