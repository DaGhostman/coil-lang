//! Report format selection and configuration.

/// Output format for the diagnostic sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ReportFormat {
    /// Human-readable ariadne reports.
    #[default]
    Pretty,
    /// Tool-consumable SARIF 2.1 JSON log.
    Sarif,
    /// LSP Diagnostic-shaped NDJSON (one object per line).
    Lsp,
}

/// Configuration for [`crate::create_sink`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReportConfig {
    pub format: ReportFormat,
}

impl ReportConfig {
    pub fn new(format: ReportFormat) -> Self {
        Self { format }
    }

    /// Build config from CLI `--log-json` / `--log-lsp` flags.
    ///
    /// Returns `Err` if both flags are set (mutually exclusive).
    pub fn from_cli_flags(log_json: bool, log_lsp: bool) -> Result<Self, &'static str> {
        match (log_json, log_lsp) {
            (true, true) => Err("--log-json and --log-lsp are mutually exclusive"),
            (true, false) => Ok(Self {
                format: ReportFormat::Sarif,
            }),
            (false, true) => Ok(Self {
                format: ReportFormat::Lsp,
            }),
            (false, false) => Ok(Self {
                format: ReportFormat::Pretty,
            }),
        }
    }

    /// Convenience for SARIF-only selection (tests / thin wrappers).
    pub fn from_log_json_flag(log_json: bool) -> Self {
        Self::from_cli_flags(log_json, false).expect("single flag cannot conflict")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_cli_flags_selects_format() {
        assert_eq!(
            ReportConfig::from_cli_flags(false, false).unwrap().format,
            ReportFormat::Pretty
        );
        assert_eq!(
            ReportConfig::from_cli_flags(true, false).unwrap().format,
            ReportFormat::Sarif
        );
        assert_eq!(
            ReportConfig::from_cli_flags(false, true).unwrap().format,
            ReportFormat::Lsp
        );
        assert!(ReportConfig::from_cli_flags(true, true).is_err());
    }
}
