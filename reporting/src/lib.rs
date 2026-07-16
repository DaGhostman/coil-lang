//! Diagnostic reporting abstraction for zero-script.
//!
//! Provides a [`DiagnosticSink`] that can render human-readable ariadne
//! reports ([`ReportFormat::Pretty`]) or tool-consumable SARIF 2.1 JSON
//! ([`ReportFormat::Sarif`]). Producers continue to emit [`common::Message`];
//! convert via [`Diagnostic::from_message`]. Runtime/CLI failures can build
//! span-optional [`Diagnostic`]s directly.
//!
//! Integration into the pipeline/CLI is a follow-up; this crate is the
//! abstraction layer only.

mod ariadne_sink;
mod config;
mod convert;
mod diagnostic;
mod sarif_sink;
mod sink;
mod source;

pub use config::{ReportConfig, ReportFormat};
pub use diagnostic::{Diagnostic, Label, Location, Severity};
pub use sink::{create_sink, emit_all, DiagnosticSink};
pub use source::{SourceId, SourceMap};
