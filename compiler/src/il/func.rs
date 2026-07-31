//! Lightweight per-function IL metadata.
//!
//! Recorded at function finalize on the flat [`super::CodeBuf`]. At lower time
//! [`super::IlModule::from_flat`] takes ownership of each body's ops for scoped
//! opts / CFG GVN; emitting spans are the split keys until then.

use super::Label;

/// Span of one compiled function inside the shared IL buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IlFunc {
    /// Fully-qualified / overload key used by the compiler.
    pub name: String,
    /// Entry label bound at the function start (if recorded).
    pub entry: Option<Label>,
    /// Inclusive-exclusive emitting-op indices in [`super::CodeBuf`].
    pub code_start: usize,
    pub code_end: usize,
}

impl IlFunc {
    pub fn new(name: impl Into<String>, entry: Option<Label>, code_start: usize, code_end: usize) -> Self {
        Self {
            name: name.into(),
            entry,
            code_start,
            code_end,
        }
    }
}
