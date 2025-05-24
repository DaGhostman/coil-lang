use std::fmt::Display;

use colored::{ColoredString, Colorize, Style};

pub struct MessageCreator {}

impl MessageCreator {}

#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum MessageKind {
    #[default]
    INFO,
    WARNING,
    ERROR,
}

impl Display for MessageKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::INFO => "INFO",
                Self::WARNING => "WARN",
                Self::ERROR => "ERROR",
            }
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageOrigin {
    SCAN,
    PARSE,
    LEX,
    COMPILE,
    RUNTIME,
    TYPE,
    FFI,
    CUSTOM,
}

impl Display for MessageOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", <MessageOrigin as Into<&str>>::into(*self))
    }
}

impl From<MessageOrigin> for &str {
    fn from(value: MessageOrigin) -> &'static str {
        match value {
            MessageOrigin::SCAN => "SCANNER",
            MessageOrigin::PARSE => "PARSER",
            MessageOrigin::LEX => "LEXER",
            MessageOrigin::TYPE => "TYPE",
            MessageOrigin::COMPILE => "COMPILER",
            MessageOrigin::RUNTIME => "RUNTIME",
            MessageOrigin::FFI => "FFI RUNTIME",
            MessageOrigin::CUSTOM => "CUSTOM",
        }
    }
}

#[derive(Default)]
pub struct MessageComposer {
    content: String,
}

impl MessageComposer {
    pub fn push(
        &mut self,
        text: &str,
        fg: Option<&str>,
        bg: Option<&str>,
        style: Option<&[&str]>,
    ) -> &mut Self {
        let mut text = ColoredString::from(text);

        let styles = Style::default();
        if let Some(style) = style {
            for &s in style {
                match s {
                    "dimmed" => styles.dimmed(),
                    "strikethrough" | "strike" => styles.strikethrough(),
                    "bold" => styles.bold(),
                    "italic" => styles.italic(),
                    "underline" => styles.underline(),
                    "blink" => styles.blink(),
                    _ => Style::default(),
                };
            }
        }
        text.style = styles;

        if let Some(fg) = fg {
            text = text.color(fg);
        }

        if let Some(bg) = bg {
            text = text.color(bg);
        }

        self.content = format!("{}{}", self.content, text);

        self
    }
}

impl Display for MessageComposer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.content)
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Message {
    kind: MessageKind,
    origin: MessageOrigin,
    message: String,
    location: Option<String>,
    context: Option<String>,
}

impl Message {
    #[must_use]
    pub fn new(kind: MessageKind, origin: MessageOrigin, message: String) -> Self {
        Self {
            kind,
            origin,
            message,
            location: None,
            context: None,
        }
    }

    pub fn set_context(&mut self, data: String) {
        self.context = Some(data)
    }

    pub fn get_context(&mut self) -> &Option<String> {
        &self.context
    }

    pub fn set_location(&mut self, location: String) {
        self.location = Some(location);
    }

    pub fn get_location(&self) -> &Option<String> {
        &self.location
    }

    pub fn kind(&self) -> MessageKind {
        self.kind
    }

    #[must_use]
    pub fn message(&self) -> &str {
        self.message.as_str()
    }

    #[must_use]
    pub fn origin(&self) -> MessageOrigin {
        self.origin
    }

    pub fn error(origin: MessageOrigin, message: String) -> Message {
        Message::new(MessageKind::ERROR, origin, message)
    }

    pub fn warning(origin: MessageOrigin, message: String) -> Message {
        Message::new(MessageKind::WARNING, origin, message)
    }

    pub fn info(origin: MessageOrigin, message: String) -> Message {
        Message::new(MessageKind::INFO, origin, message)
    }

    pub fn compose() -> MessageComposer {
        MessageComposer::default()
    }
}

impl Display for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut message;
        match self.kind {
            MessageKind::ERROR => {
                message = format!(
                    "{} {}",
                    self.origin.to_string().red().bold(),
                    self.kind.to_string().red().bold(),
                );
            }
            MessageKind::WARNING => {
                message = format!(
                    "{} {}",
                    self.origin.to_string().yellow().bold(),
                    self.kind.to_string().yellow().bold(),
                );
            }
            MessageKind::INFO => {
                message = format!(
                    "{} {}",
                    self.origin.to_string().cyan().bold(),
                    self.kind.to_string().cyan().bold(),
                );
            }
        }

        message = format!("[{message}] {}", self.message.yellow().italic());

        if let Some(location) = &self.location {
            message = format!("{message} in {}", location.cyan());
        }

        write!(f, "{message}")
    }
}
