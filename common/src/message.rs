use std::ops::Range;

#[derive(PartialEq, Hash, Eq, Clone, Debug)]
pub enum MessageKind {
    WARNING,
    ERROR,
    INFO,
}

#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub struct Message {
    kind: MessageKind,
    message: String,
    range: Range<usize>,

    related: Option<Box<Self>>,
}

impl Message {
    pub fn new(kind: MessageKind, message: String, range: Range<usize>) -> Self {
        Self {
            kind,
            message,
            range,
            related: None,
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
    pub fn relates(&mut self, other: Self) -> &mut Self {
        self.related = Some(Box::new(other));

        self
    }

    pub fn related(&self) -> &Option<Box<Self>> {
        &self.related
    }

    pub fn message(&self) -> &str {
        self.message.as_str()
    }

    pub fn kind(&self) -> &MessageKind {
        &self.kind
    }

    pub fn range(&self) -> &Range<usize> {
        &self.range
    }
}
