//! Canonical sink-facing diagnostic model.

use std::ops::Range;

use crate::codes::ErrorCode;
use crate::source::SourceId;

/// Severity of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    Error,
    Warning,
    Info,
    /// Reserved for future help-as-related / note annotations.
    Note,
}

/// A byte-span location within a registered source file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Location {
    pub file: SourceId,
    pub range: Range<usize>,
}

impl Location {
    pub fn new(file: SourceId, range: Range<usize>) -> Self {
        Self { file, range }
    }
}

/// Secondary span annotation attached to a [`Diagnostic`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RelatedLabel {
    pub location: Location,
    pub message: String,
}

impl RelatedLabel {
    pub fn new(location: Location, message: impl Into<String>) -> Self {
        Self {
            location,
            message: message.into(),
        }
    }
}

/// A single diagnostic ready for a [`crate::DiagnosticSink`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub location: Option<Location>,
    pub labels: Vec<RelatedLabel>,
    pub help: Option<String>,
    pub code: Option<ErrorCode>,
}

impl Diagnostic {
    pub fn new(severity: Severity, message: impl Into<String>) -> Self {
        Self {
            severity,
            message: message.into(),
            location: None,
            labels: Vec::new(),
            help: None,
            code: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::new(Severity::Error, message)
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, message)
    }

    pub fn info(message: impl Into<String>) -> Self {
        Self::new(Severity::Info, message)
    }

    pub fn note(message: impl Into<String>) -> Self {
        Self::new(Severity::Note, message)
    }

    pub fn at(mut self, file: SourceId, range: Range<usize>) -> Self {
        self.location = Some(Location::new(file, range));
        self
    }

    pub fn without_location(mut self) -> Self {
        self.location = None;
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn with_code(mut self, code: ErrorCode) -> Self {
        self.code = Some(code);
        self
    }

    pub fn with_label(mut self, label: RelatedLabel) -> Self {
        self.labels.push(label);
        self
    }

    pub fn is_error(&self) -> bool {
        matches!(self.severity, Severity::Error)
    }
}
