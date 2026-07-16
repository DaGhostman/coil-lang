//! Report format selection and configuration.

/// Output format for the diagnostic sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ReportFormat {
    /// Human-readable ariadne reports.
    #[default]
    Pretty,
    /// Tool-consumable SARIF 2.1 JSON log.
    Sarif,
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

    /// Build config from the future CLI `--log-json` flag.
    pub fn from_log_json_flag(log_json: bool) -> Self {
        Self {
            format: if log_json {
                ReportFormat::Sarif
            } else {
                ReportFormat::Pretty
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_log_json_flag_selects_format() {
        assert_eq!(
            ReportConfig::from_log_json_flag(false).format,
            ReportFormat::Pretty
        );
        assert_eq!(
            ReportConfig::from_log_json_flag(true).format,
            ReportFormat::Sarif
        );
    }
}
