use std::fmt::Display;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ErrorOrigin {
    SCAN,
    PARSE,
    LEX,
    COMPILE,
    RUNTIME,
    FFI,
    CUSTOM,
}

impl From<ErrorOrigin> for &str {
    fn from(value: ErrorOrigin) -> &'static str {
        match value {
            ErrorOrigin::SCAN => "SCANNER",
            ErrorOrigin::PARSE => "PARSER",
            ErrorOrigin::LEX => "LEXER",
            ErrorOrigin::COMPILE => "COMPILER",
            ErrorOrigin::RUNTIME => "RUNTIME",
            ErrorOrigin::FFI => "FFI RUNTIME",
            ErrorOrigin::CUSTOM => "CUSTOM",
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Error {
    origin: ErrorOrigin,
    message: String,
}

impl Error {
    #[must_use]
    pub fn new(origin: ErrorOrigin, message: String) -> Self {
        Self { origin, message }
    }

    #[must_use]
    pub fn message(&self) -> &str {
        self.message.as_str()
    }

    #[must_use]
    pub fn origin(&self) -> ErrorOrigin {
        self.origin
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{} ERROR] {}",
            <ErrorOrigin as Into<&str>>::into(self.origin),
            self.message
        )
    }
}
