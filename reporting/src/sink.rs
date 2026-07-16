//! Diagnostic sink trait and factory.

use std::io::Write;

use crate::ariadne_sink::AriadneSink;
use crate::config::{ReportConfig, ReportFormat};
use crate::diagnostic::Diagnostic;
use crate::sarif_sink::SarifSink;
use crate::source::SourceMap;

/// Sink that consumes [`Diagnostic`]s and renders them.
pub trait DiagnosticSink {
    fn emit(&mut self, diag: Diagnostic);

    /// Flush buffered output (SARIF writes the log here; pretty is a no-op).
    fn finish(&mut self) -> std::io::Result<()>;

    fn had_errors(&self) -> bool;
}

/// Emit every diagnostic in `diags` into `sink`.
///
/// Kept outside the trait so [`DiagnosticSink`] stays dyn-compatible.
pub fn emit_all(sink: &mut dyn DiagnosticSink, diags: impl IntoIterator<Item = Diagnostic>) {
    for diag in diags {
        sink.emit(diag);
    }
}

/// Build a sink for `config.format`, owning `sources` and writing to `writer`.
pub fn create_sink(
    config: &ReportConfig,
    sources: SourceMap,
    writer: Box<dyn Write + Send>,
) -> Box<dyn DiagnosticSink> {
    match config.format {
        ReportFormat::Pretty => Box::new(AriadneSink::new(sources, writer)),
        ReportFormat::Sarif => Box::new(SarifSink::new(sources, writer)),
    }
}
