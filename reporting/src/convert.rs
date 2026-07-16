//! Bridge from [`Message`] to [`Diagnostic`].

use crate::diagnostic::{Diagnostic, Location, RelatedLabel, Severity};
use crate::message::{Message, MessageKind};
use crate::source::SourceId;

impl From<MessageKind> for Severity {
    fn from(kind: MessageKind) -> Self {
        match kind {
            MessageKind::ERROR => Severity::Error,
            MessageKind::WARNING => Severity::Warning,
            MessageKind::INFO => Severity::Info,
        }
    }
}

impl Diagnostic {
    /// Convert a producer-facing [`Message`] into a sink-facing diagnostic,
    /// anchoring all spans to `file`.
    pub fn from_message(msg: &Message, file: SourceId) -> Self {
        let mut diag = Diagnostic::new(Severity::from(*msg.kind()), msg.message())
            .at(file, msg.range().clone());

        if let Some(code) = msg.code() {
            diag = diag.with_code(code);
        }

        if let Some(help) = msg.help() {
            diag = diag.with_help(help.clone());
        }

        for label in msg.labels() {
            diag = diag.with_label(RelatedLabel::new(
                Location::new(file, label.range().clone()),
                label.message(),
            ));
        }

        diag
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codes::ErrorCode;
    use crate::message::Label;
    use crate::source::SourceMap;

    #[test]
    fn from_message_maps_kind_range_help_code_and_labels() {
        let mut map = SourceMap::new();
        let file = map.insert("test.0s", "let x = 1;\n");

        let mut msg = Message::error(ErrorCode::TypeMismatch, "Type mismatch".into(), 4..5);
        msg.with_help("expected int".into());
        msg.push(Label::new("here".into(), 8..9));

        let diag = Diagnostic::from_message(&msg, file);
        assert_eq!(diag.severity, Severity::Error);
        assert_eq!(diag.message, "Type mismatch");
        assert_eq!(diag.code, Some(ErrorCode::TypeMismatch));
        assert_eq!(diag.help.as_deref(), Some("expected int"));
        assert_eq!(
            diag.location.as_ref().map(|l| (l.file, l.range.clone())),
            Some((file, 4..5))
        );
        assert_eq!(diag.labels.len(), 1);
        assert_eq!(diag.labels[0].message, "here");
        assert_eq!(diag.labels[0].location.range, 8..9);
    }
}
