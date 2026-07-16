//! Producer-facing diagnostic messages (formerly `common::message`).

use std::ops::Range;

use crate::codes::ErrorCode;

#[derive(PartialEq, Hash, Eq, Copy, Clone, Debug)]
pub enum MessageKind {
    WARNING,
    ERROR,
    INFO,
}

/// Secondary span annotation on a [`Message`] (byte range in the same file).
#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub struct Label {
    message: String,
    range: Range<usize>,
}

impl Label {
    pub fn new(message: String, range: Range<usize>) -> Self {
        Self { message, range }
    }

    pub fn range(&self) -> &Range<usize> {
        &self.range
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub struct Message {
    kind: MessageKind,
    message: String,
    help: Option<String>,
    labels: Vec<Label>,
    range: Range<usize>,
    code: Option<ErrorCode>,
}

impl Message {
    pub fn new(kind: MessageKind, message: String, range: Range<usize>) -> Self {
        Self {
            kind,
            range,
            help: None,
            message,
            labels: Vec::with_capacity(16),
            code: None,
        }
    }

    pub fn with_code(mut self, code: ErrorCode) -> Self {
        self.code = Some(code);
        self
    }

    pub fn set_code(&mut self, code: ErrorCode) {
        self.code = Some(code);
    }

    pub fn code(&self) -> Option<ErrorCode> {
        self.code
    }

    pub fn warn(code: ErrorCode, message: String, range: Range<usize>) -> Self {
        Self::new(MessageKind::WARNING, message, range).with_code(code)
    }

    pub fn error(code: ErrorCode, message: String, range: Range<usize>) -> Self {
        Self::new(MessageKind::ERROR, message, range).with_code(code)
    }

    pub fn info(code: ErrorCode, message: String, range: Range<usize>) -> Self {
        Self::new(MessageKind::INFO, message, range).with_code(code)
    }

    pub fn with_help(&mut self, msg: String) {
        self.help = Some(msg);
    }

    pub fn help(&self) -> &Option<String> {
        &self.help
    }

    pub fn message(&self) -> &str {
        self.message.as_str()
    }

    pub fn kind(&self) -> &MessageKind {
        &self.kind
    }

    pub fn labels(&self) -> &Vec<Label> {
        &self.labels
    }

    pub fn range(&self) -> &Range<usize> {
        &self.range
    }

    pub fn push(&mut self, label: Label) {
        self.labels.push(label);
    }
}
