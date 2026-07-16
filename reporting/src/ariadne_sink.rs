//! Pretty (ariadne) diagnostic sink.

use std::io::Write;

use ariadne::{
    sources, Color, Config, IndexType, Label as AriadneLabel, LabelAttach, Report, ReportKind,
};

use crate::diagnostic::{Diagnostic, Severity};
use crate::sink::DiagnosticSink;
use crate::source::SourceMap;

/// Renders each diagnostic immediately as an ariadne report.
pub struct AriadneSink {
    sources: SourceMap,
    writer: Box<dyn Write + Send>,
    error_count: usize,
}

impl AriadneSink {
    pub fn new(sources: SourceMap, writer: Box<dyn Write + Send>) -> Self {
        Self {
            sources,
            writer,
            error_count: 0,
        }
    }

    fn write_spanless(&mut self, diag: &Diagnostic) -> std::io::Result<()> {
        let kind = match diag.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
            Severity::Note => "note",
        };
        write!(self.writer, "{kind}: {}", diag.message)?;
        if let Some(help) = &diag.help {
            write!(self.writer, "\nhelp: {help}")?;
        }
        writeln!(self.writer)?;
        Ok(())
    }

    fn write_spanned(&mut self, diag: &Diagnostic) -> std::io::Result<()> {
        let loc = diag
            .location
            .as_ref()
            .expect("write_spanned requires a primary location");

        let (path, _) = self.sources.get(loc.file).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("unknown SourceId {}", loc.file.as_u32()),
            )
        })?;
        let filename = path.display().to_string();

        let kind = match diag.severity {
            Severity::Error => ReportKind::Error,
            Severity::Warning => ReportKind::Warning,
            Severity::Info => ReportKind::Custom("Info", Color::BrightBlue),
            Severity::Note => ReportKind::Custom("Note", Color::BrightBlue),
        };

        let mut report = Report::build(kind, (filename.clone(), loc.range.clone()))
            .with_message(&diag.message)
            .with_config(
                Config::new()
                    .with_index_type(IndexType::Byte)
                    .with_underlines(true)
                    .with_label_attach(LabelAttach::End)
                    .with_multiline_arrows(true)
                    .with_compact(false),
            );

        for label in &diag.labels {
            let label_path = self
                .sources
                .path(label.location.file)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| filename.clone());
            report = report.with_label(
                AriadneLabel::new((label_path, label.location.range.clone()))
                    .with_message(&label.message)
                    .with_color(Color::Primary),
            );
        }

        if let Some(help) = &diag.help {
            report = report.with_help(help);
        }

        let cache_pairs: Vec<(String, String)> = self
            .sources
            .iter()
            .map(|(_, path, text)| (path.display().to_string(), text.to_string()))
            .collect();
        let mut cache = sources(cache_pairs);

        report
            .finish()
            .write(&mut cache, &mut self.writer)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }
}

impl DiagnosticSink for AriadneSink {
    fn emit(&mut self, diag: Diagnostic) {
        if diag.is_error() {
            self.error_count += 1;
        }
        let result = if diag.location.is_some() {
            self.write_spanned(&diag)
        } else {
            self.write_spanless(&diag)
        };
        if let Err(err) = result {
            // Rendering failures must not panic the compiler; surface on stderr.
            let _ = writeln!(std::io::stderr(), "reporting: failed to render diagnostic: {err}");
        }
    }

    fn finish(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }

    fn had_errors(&self) -> bool {
        self.error_count > 0
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::diagnostic::{Label, Location};

    #[derive(Clone, Default)]
    struct SharedBuf {
        inner: Arc<Mutex<Vec<u8>>>,
    }

    impl SharedBuf {
        fn new() -> Self {
            Self::default()
        }

        fn into_string(self) -> String {
            let bytes = self.inner.lock().unwrap().clone();
            String::from_utf8_lossy(&bytes).into_owned()
        }
    }

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.inner.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn pretty_sink_renders_spanned_diagnostic_without_panic() {
        let mut sources = SourceMap::new();
        let file = sources.insert("sample.0s", "let x = \"hi\";\n");
        let shared = SharedBuf::new();
        let mut sink = AriadneSink::new(sources, Box::new(shared.clone()));

        sink.emit(
            Diagnostic::error("Type mismatch")
                .at(file, 8..12)
                .with_label(Label::new(
                    Location::new(file, 8..12),
                    "found string",
                ))
                .with_help("expected int"),
        );
        sink.finish().unwrap();
        assert!(sink.had_errors());
        // Ariadne wrote something (exact layout is not stable across versions).
        assert!(!shared.into_string().is_empty());
    }

    #[test]
    fn pretty_sink_renders_spanless_diagnostic() {
        let shared = SharedBuf::new();
        let mut sink = AriadneSink::new(SourceMap::new(), Box::new(shared.clone()));
        sink.emit(Diagnostic::error("Bytecode archive version mismatch"));
        sink.finish().unwrap();

        let out = shared.into_string();
        assert!(out.contains("error: Bytecode archive version mismatch"));
        assert!(sink.had_errors());
    }
}
