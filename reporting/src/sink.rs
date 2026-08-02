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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codes::ErrorCode;
    use crate::config::{ReportConfig, ReportFormat};
    use crate::diagnostic::Diagnostic;
    use crate::source::SourceMap;
    use std::io::Cursor;

    #[test]
    fn create_sink_builds_pretty_sarif_and_lsp() {
        for format in [ReportFormat::Pretty, ReportFormat::Sarif, ReportFormat::Lsp] {
            let config = ReportConfig { format };
            let mut sink =
                create_sink(&config, SourceMap::new(), Box::new(Cursor::new(Vec::new())));
            assert!(!sink.had_errors());
            sink.emit(Diagnostic::error("boom").with_code(ErrorCode::ParseError));
            assert!(sink.had_errors());
            sink.finish().expect("finish");
        }
    }

    #[test]
    fn emit_all_forwards_every_diagnostic() {
        let config = ReportConfig {
            format: ReportFormat::Lsp,
        };
        let buf = Cursor::new(Vec::new());
        let mut sink = create_sink(&config, SourceMap::new(), Box::new(buf));
        emit_all(
            &mut *sink,
            [
                Diagnostic::warning("one"),
                Diagnostic::error("two").with_code(ErrorCode::TypeMismatch),
            ],
        );
        assert!(sink.had_errors());
        sink.finish().expect("finish");
    }
}
