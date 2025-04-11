#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(u8)]
pub enum TokenKind {
    #[default]
    EOF,
    Identifier,
    Number,
    Double,
    String,
    True,
    False,
    None,

    /// Symbols
    LeftParenthesis,
    RightParenthesis,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,

    Minus,
    MinusMinus,
    Plus,
    PlusPlus,
    SlimArrow,
    Star,
    StarStar,
    Slash,
    Percent,
    Ampersand,
    AmpersandAmpersand,
    Pipe,
    PipePipe,
    PipeGreater,
    Caret,
    Tilde,
    Comma,
    SemiColon,

    Dot,
    DotDot,
    DotDotDot,
    FatArrow,
    Less,
    LessEqual,
    LessLess,
    Greater,
    GreaterEqual,
    GreaterGreater,

    Bang,
    BangEqual,
    Equal,
    EqualEqual,

    ///
    And,
    As,
    Bool,
    Const,
    Continue,
    Class,
    Default,
    Else,
    Float,
    For,
    Function,
    If,
    Int,
    Len,
    Let,
    Match,
    New,
    Or,
    Print,
    PrintLn,
    Prop,
    Pub,
    Raise,
    Return,
    Some,
    Str,
    Err,
    Trait,
    This,
    Use,
    While,
    Yield,
}

#[derive(Debug, Clone, Default)]
pub struct TokenPosition {
    line: usize,
    column: usize,
}

impl TokenPosition {
    #[must_use] pub fn line(&self) -> usize {
        self.line
    }

    #[must_use] pub fn column(&self) -> usize {
        self.column
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    start: TokenPosition,
    end: TokenPosition,
    file: String,
    lexeme: String,
    kind: TokenKind,
}

impl Default for Token {
    fn default() -> Self {
        Token {
            start: TokenPosition { line: 0, column: 0 },
            end: TokenPosition { line: 0, column: 0 },
            lexeme: String::from("\0"),
            file: String::new(),
            kind: TokenKind::EOF,
        }
    }
}

impl Token {
    #[must_use] pub fn begin(kind: TokenKind, line: usize, column: usize, file: &str) -> Self {
        Token {
            kind,
            start: TokenPosition { line, column },
            end: TokenPosition { line, column },
            file: file.to_string(),
            lexeme: String::new(),
        }
    }

    pub fn end(&mut self, kind: TokenKind, lexeme: String, line: usize, column: usize) {
        self.kind = kind;
        self.end = TokenPosition { line, column };
        self.lexeme = lexeme;
    }

    #[must_use] pub fn start_line(&self) -> usize {
        self.start.line
    }

    #[must_use] pub fn start_column(&self) -> usize {
        self.start.column
    }

    #[must_use] pub fn end_line(&self) -> usize {
        self.end.line
    }

    #[must_use] pub fn end_column(&self) -> usize {
        self.end.column
    }

    #[must_use] pub fn kind(&self) -> TokenKind {
        self.kind
    }

    #[must_use] pub fn lexeme(&self) -> &str {
        self.lexeme.as_ref()
    }
}
