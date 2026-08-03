use std::path::Path;

use crate::{
    Diagnostic, DiagnosticSink, SourceId, SourceMap,
};

/// In-memory diagnostic sink for language tooling and embedding clients.
#[derive(Default)]
pub struct DiagnosticCollector {
    sources: SourceMap,
    diagnostics: Vec<Diagnostic>,
}

impl DiagnosticCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    pub fn sources(&self) -> &SourceMap {
        &self.sources
    }
}

impl DiagnosticSink for DiagnosticCollector {
    fn emit(&mut self, diag: Diagnostic) {
        self.diagnostics.push(diag);
    }

    fn register_source(&mut self, path: &Path, text: &str) -> SourceId {
        self.sources.insert(path, text)
    }

    fn finish(&mut self) -> std::io::Result<()> {
        Ok(())
    }

    fn had_errors(&self) -> bool {
        self.diagnostics.iter().any(Diagnostic::is_error)
    }
}
