//! Compiler diagnostic messages and source labels.

use std::ops::Range;

#[derive(PartialEq, Hash, Eq, Copy, Clone, Debug)]
pub enum MessageKind {
    WARNING,
    ERROR,
    INFO,
}

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
}

impl ToString for Label {
    fn to_string(&self) -> String {
        self.message.to_string()
    }
}

#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub struct Message {
    kind: MessageKind,
    message: String,
    help: Option<String>,
    labels: Vec<Label>,
    range: Range<usize>,
}

impl Message {
    pub fn new(kind: MessageKind, message: String, range: Range<usize>) -> Self {
        Self {
            kind,
            range,
            help: None,
            message,
            labels: Vec::with_capacity(16),
        }
    }

    pub fn warn(message: String, range: Range<usize>) -> Self {
        Self::new(MessageKind::WARNING, message, range)
    }

    pub fn error(message: String, range: Range<usize>) -> Self {
        Self::new(MessageKind::ERROR, message, range)
    }

    pub fn info(message: String, range: Range<usize>) -> Self {
        Self::new(MessageKind::INFO, message, range)
    }
}

impl Message {
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
