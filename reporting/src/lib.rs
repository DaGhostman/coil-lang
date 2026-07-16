//! Diagnostic reporting for zero-script.
//!
//! Owns producer [`Message`]s (with stable [`ErrorCode`]s) and sink-facing
//! [`Diagnostic`]s. Render via [`DiagnosticSink`]: pretty ariadne,
//! SARIF 2.1 (`--log-json`), or LSP Diagnostic NDJSON (`--log-lsp`).

mod ariadne_sink;
mod codes;
mod config;
mod convert;
mod diagnostic;
mod lsp_sink;
mod message;
mod sarif_sink;
mod sink;
mod source;

pub use codes::ErrorCode;
pub use config::{ReportConfig, ReportFormat};
pub use diagnostic::{Diagnostic, Location, RelatedLabel, Severity};
pub use message::{Label, Message, MessageKind};
pub use sink::{create_sink, emit_all, DiagnosticSink};
pub use source::{SourceId, SourceMap};
