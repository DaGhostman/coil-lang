//! Bridge from [`common::Message`] to [`Diagnostic`].

use common::{Message, MessageKind};

use crate::diagnostic::{Diagnostic, Label, Location, Severity};
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

        if let Some(help) = msg.help() {
            diag = diag.with_help(help.clone());
        }

        for label in msg.labels() {
            diag = diag.with_label(Label::new(
                Location::new(file, label.range().clone()),
                label.to_string(),
            ));
        }

        diag
    }
}

#[cfg(test)]
mod tests {
    use common::{Label as MsgLabel, Message};

    use super::*;
    use crate::source::SourceMap;

    #[test]
    fn from_message_maps_kind_range_help_and_labels() {
        let mut map = SourceMap::new();
        let file = map.insert("test.0s", "let x = 1;\n");

        let mut msg = Message::error("Type mismatch".into(), 4..5);
        msg.with_help("expected int".into());
        msg.push(MsgLabel::new("here".into(), 8..9));

        let diag = Diagnostic::from_message(&msg, file);
        assert_eq!(diag.severity, Severity::Error);
        assert_eq!(diag.message, "Type mismatch");
        assert_eq!(diag.help.as_deref(), Some("expected int"));
        assert_eq!(
            diag.location.as_ref().map(|l| (l.file, l.range.clone())),
            Some((file, 4..5))
        );
        assert_eq!(diag.labels.len(), 1);
        assert_eq!(diag.labels[0].message, "here");
        assert_eq!(diag.labels[0].location.range, 8..9);
        assert_eq!(diag.labels[0].location.file, file);
    }
}
