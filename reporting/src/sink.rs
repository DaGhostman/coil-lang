//! Diagnostic sink trait and factory.

use std::io::Write;
use std::path::Path;

use crate::ariadne_sink::AriadneSink;
use crate::config::{ReportConfig, ReportFormat};
use crate::diagnostic::Diagnostic;
use crate::lsp_sink::LspSink;
use crate::sarif_sink::SarifSink;
use crate::source::{SourceId, SourceMap};

/// Sink that consumes [`Diagnostic`]s and renders them.
pub trait DiagnosticSink {
    fn emit(&mut self, diag: Diagnostic);

    /// Register (or update) a source file for span resolution.
    fn register_source(&mut self, path: &Path, text: &str) -> SourceId;

    /// Flush buffered output (SARIF/LSP write here; pretty flushes).
    fn finish(&mut self) -> std::io::Result<()>;

    fn had_errors(&self) -> bool;
}

/// Emit every diagnostic in `diags` into `sink`.
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
        ReportFormat::Lsp => Box::new(LspSink::new(sources, writer)),
    }
}
