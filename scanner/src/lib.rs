use buffer::Buffer;
use common::error::Error;
use tokens::{Token, TokenKind};

pub mod buffer;
pub mod tokens;

pub struct Scanner {
    buffer: Buffer,
    line: usize,
    column: usize,
    file: String,
    // ----
    in_template: bool,
    in_template_expr: bool,
}

impl Scanner {
    #[must_use]
    pub fn tell(&self) -> usize {
        self.buffer.tell()
    }

    #[must_use]
    pub fn new(buffer: Buffer, file: Option<String>) -> Self {
        Scanner {
            buffer,
            column: 0,
            line: 0,
            file: file.unwrap_or_default(),

            in_template: false,
            in_template_expr: false,
        }
    }

    fn advance(&mut self) {
        self.buffer.next();
        self.column += 1;

        if self.buffer.is_consumed() {
            return;
        }

        if let Some(char) = self.buffer.current() {
            if char == '\n' {
                self.line += 1;
                self.column = 0;
            }
        }
    }

    fn advance_by(&mut self, steps: usize) {
        for _ in 0..steps {
            self.advance();
        }
    }

    fn current(&mut self) -> Option<char> {
        while self.buffer.current() == Some('\n') {
            self.advance();
        }

        self.buffer.current()
    }

    fn peek(&mut self, offset: usize) -> Option<char> {
        if self.buffer.is_consumed() {
            return None;
        }

        self.buffer.peek(offset)
    }

    fn matches(&mut self, lexeme: &str) -> bool {
        for position in 0..lexeme.len() {
            if self.peek(position).map(|c| c.to_ascii_lowercase())
                != lexeme.chars().nth(position).map(|c| c.to_ascii_lowercase())
            {
                return false;
            }
        }

        // if self
        //     .peek(lexeme.len())
        //     .map(|v| v.is_ascii_alphanumeric() || v == '_')
        //     .or(Some(false))
        //     .unwrap()
        // {
        //     return false;
        // }

        true
    }

    fn string(&mut self) -> Token {
        let separator = self.current().unwrap();
        self.advance();
        let start = self.buffer.tell();

        let mut token = Token::begin(TokenKind::String, self.line, self.column, "file-name");
        while self.current() != Some(separator) && !self.buffer.is_consumed() {
            if let (Some('\\'), Some(ch)) = (self.current(), self.peek(1)) {
                if ch == separator {
                    self.advance();
                }
            }

            self.advance();
        }
        let end = self.buffer.tell();
        self.advance();

        token.end(
            TokenKind::String,
            self.buffer
                .string_at_range(start, end)
                .unwrap_or_default()
                .replace("\\\\", "\\")
                .replace("\\n", "\n")
                .replace("\\r", "\r")
                .replace("\\t", "\t")
                .replace(
                    format!("\\{separator}").as_str(),
                    format!("{separator}").as_str(),
                ),
            self.line,
            self.column,
        );

        token
    }

    fn digit(&mut self) -> Token {
        let start = self.buffer.tell();
        let mut token = Token::begin(TokenKind::EOF, self.line, self.column, "file-name");
        let mut is_float = None;

        let mut hex = false;
        while let Some(ch) = self.current() {
            if is_float.is_none() && ch == '.' {
                match (self.current(), self.peek(1)) {
                    (Some('.'), Some('0'..='9')) => {
                        is_float = Some(true);
                    }
                    _ => {
                        break;
                    }
                }
                self.advance();
                continue;
            }
            // if let Some(ch) = self.current() {
            match ch {
                '0'..='9' | '_' => self.advance(),
                'x' | 'o' | 'b' => {
                    if (self.buffer.tell() - start) == 1 {
                        if self.current() == Some('x') {
                            hex = true;
                        }

                        self.advance();
                    } else {
                        todo!("Error unexpected token");
                    }
                }
                'a'..='z' | 'A'..='Z' => {
                    if !hex {
                        todo!("Error unexpected token");
                    }

                    self.advance();
                }
                _ => break,
            }
            // }
        }

        token.end(
            if is_float.is_some() {
                TokenKind::Double
            } else {
                TokenKind::Number
            },
            self.buffer
                .string_at_range(start, self.buffer.tell())
                .unwrap_or_default(), // .replace("_", "")
            self.line,
            self.column,
        );

        token
    }

    fn identifier(&mut self) -> Token {
        let start = self.buffer.tell();
        let mut token = Token::begin(TokenKind::Identifier, self.line, self.column, "file-name");
        match self.current().map(|c| c.to_ascii_lowercase()) {
            Some('a'..='z' | '_') => self.advance(),
            Some(i) => unreachable!("Invalid identifier: '{}'", i),
            _ => (),
        }

        while matches!(self.current(), Some('a'..='z' | '_' | '0'..='9'))
            && !self.buffer.is_consumed()
        {
            self.advance();
        }

        token.end(
            TokenKind::Identifier,
            self.buffer
                .string_at_range(start, self.buffer.tell())
                .unwrap_or_default(),
            self.line,
            self.column,
        );

        token
    }

    fn make_token(&mut self, kind: TokenKind, lexeme: &str) -> Token {
        let start = self.buffer.tell();
        let mut token = Token::begin(kind, self.line, self.column, "file-name");

        if !self.matches(lexeme) {
            unreachable!("Dafuq happened with '{}'", lexeme);
        }

        self.advance_by(lexeme.len());

        token.end(
            kind,
            self.buffer
                .string_at_range(start, self.buffer.tell())
                .unwrap_or_default(),
            self.line,
            self.column,
        );

        token
    }

    fn keyword_or_identifier(&mut self, kind: TokenKind, lexeme: &str) -> Token {
        if self.matches(lexeme) {
            if !self
                .peek(lexeme.len())
                .is_none_or(|c| c.is_whitespace() || c.is_ascii_punctuation())
            {
                return self.identifier();
            }

            let start = self.buffer.tell();
            let mut token = Token::begin(kind, self.line, self.column, self.file.as_str());

            self.advance_by(lexeme.len());

            token.end(
                kind,
                self.buffer
                    .string_at_range(start, self.buffer.tell())
                    .unwrap_or_default(),
                self.line,
                self.column - lexeme.len(),
            );

            token
        } else {
            self.identifier()
        }
    }

    fn skip_comment(&mut self) -> Token {
        let line = self.line;

        while line == self.line && self.current().is_some() {
            self.advance();
        }
        if self.current() == Some('\n') {
            self.advance();
        }

        self.scan()
    }

    fn make_template(&mut self) -> Token {
        if self.in_template {
            self.column += 1;
            // We are in template parsing, which means this is a closing so we return an
            // empty string to complete the template building
            self.in_template = false;
            self.advance();
            return self.make_token(TokenKind::String, "");
        }

        // move one forward
        self.advance();
        let start = self.buffer.tell();

        if self.in_template {
            self.advance();
            self.in_template = false;
            return self.scan();
        }

        self.in_template = true;
        if self.current() == Some('$') && self.peek(1) == Some('{') {
            return self.make_token(TokenKind::String, "");
        }

        let mut token = Token::begin(TokenKind::String, self.line, self.column, "file-name");

        while self.current() != Some('`') {
            self.advance();

            if self.current() == Some('$') && self.peek(1) == Some('{') {
                break;
            }
        }

        token.end(
            TokenKind::String,
            self.buffer
                .string_at_range(start, self.buffer.tell())
                .unwrap_or_default(),
            self.line,
            self.column,
        );

        token
    }

    pub fn scan(&mut self) -> Token {
        match self.current().map(|c| c.to_ascii_lowercase()) {
            // Some('\n') => {
            //     self.advance();
            //     let token = self.scan();
            //
            //     token
            // }
            Some('\'' | '"') => self.string(),
            Some('`') => self.make_template(),
            Some('#') => self.skip_comment(),
            Some('$') => {
                if self.peek(1) == Some('{') {
                    self.in_template_expr = true;
                    self.make_token(TokenKind::Plus, "${")
                } else {
                    todo!("Handle unexpected token");
                }
            }
            Some('(') => self.make_token(TokenKind::LeftParenthesis, "("),
            Some(')') => self.make_token(TokenKind::RightParenthesis, ")"),
            Some('[') => self.make_token(TokenKind::LeftBrace, "["),
            Some(']') => self.make_token(TokenKind::RightBrace, "]"),
            Some('{') => self.make_token(TokenKind::LeftBracket, "{"),
            Some('}') => {
                if self.in_template_expr {
                    self.in_template_expr = false;
                    self.make_token(TokenKind::Plus, "}")
                } else {
                    self.make_token(TokenKind::RightBracket, "}")
                }
            }
            Some('-') => match self.peek(1) {
                Some('-') => self.make_token(TokenKind::MinusMinus, "--"),
                Some('>') => self.make_token(TokenKind::SlimArrow, "->"),
                _ => self.make_token(TokenKind::Minus, "-"),
            },
            Some('+') => match self.peek(1) {
                Some('+') => self.make_token(TokenKind::PlusPlus, "++"),
                _ => self.make_token(TokenKind::Plus, "+"),
            },
            Some('*') => match self.peek(1) {
                Some('*') => self.make_token(TokenKind::StarStar, "**"),
                _ => self.make_token(TokenKind::Star, "*"),
            },
            Some('/') => {
                if self.peek(1) == Some('/') {
                    self.skip_comment()
                } else {
                    self.make_token(TokenKind::Slash, "/")
                }
            }
            Some('%') => self.make_token(TokenKind::Percent, "%"),
            Some('&') => match self.peek(1) {
                Some('&') => self.make_token(TokenKind::AmpersandAmpersand, "&&"),
                _ => self.make_token(TokenKind::Ampersand, "&"),
            },
            Some('|') => match self.peek(1) {
                Some('|') => self.make_token(TokenKind::PipePipe, "||"),
                Some('>') => self.make_token(TokenKind::PipeGreater, "|>"),
                _ => self.make_token(TokenKind::Pipe, "|"),
            },
            Some('^') => self.make_token(TokenKind::Caret, "^"),
            Some('~') => self.make_token(TokenKind::Tilde, "~"),
            Some('.') => match self.peek(1) {
                Some('.') => match self.peek(2) {
                    Some('.') => self.make_token(TokenKind::DotDotDot, "..."),
                    _ => self.make_token(TokenKind::DotDot, ".."),
                },
                _ => self.make_token(TokenKind::Dot, "."),
            },
            Some(',') => self.make_token(TokenKind::Comma, ","),
            Some('<') => match self.peek(1) {
                Some('=') => self.make_token(TokenKind::LessEqual, "<="),
                Some('<') => self.make_token(TokenKind::LessLess, "<<"),
                _ => self.make_token(TokenKind::Less, "<"),
            },
            Some('>') => match self.peek(1) {
                Some('=') => self.make_token(TokenKind::GreaterEqual, ">="),
                Some('>') => self.make_token(TokenKind::GreaterGreater, ">>"),
                _ => self.make_token(TokenKind::Greater, ">"),
            },
            Some('!') => match self.peek(1) {
                Some('=') => self.make_token(TokenKind::BangEqual, "!="),
                _ => self.make_token(TokenKind::Bang, "!"),
            },
            Some('=') => match self.peek(1) {
                Some('=') => self.make_token(TokenKind::EqualEqual, "=="),
                Some('>') => self.make_token(TokenKind::FatArrow, "=>"),
                _ => self.make_token(TokenKind::Equal, "="),
            },
            Some(';') => self.make_token(TokenKind::SemiColon, ";"),
            Some('0'..='9') => self.digit(),
            Some('a') => match self.peek(1) {
                Some('s') => self.keyword_or_identifier(TokenKind::As, "as"),
                Some('n') => self.keyword_or_identifier(TokenKind::And, "and"),
                _ => self.identifier(),
            },
            Some('b') => self.keyword_or_identifier(TokenKind::Bool, "bool"),
            Some('c') => match self.peek(1) {
                Some('l') => self.keyword_or_identifier(TokenKind::Class, "class"),
                Some('o') => match self.peek(3) {
                    Some('s') => self.keyword_or_identifier(TokenKind::Const, "const"),
                    Some('t') => self.keyword_or_identifier(TokenKind::Continue, "continue"),
                    _ => self.identifier(),
                },
                _ => self.identifier(),
            },
            Some('d') => self.keyword_or_identifier(TokenKind::Default, "default"),
            Some('e') => self.keyword_or_identifier(TokenKind::Else, "else"),
            Some('f') => match self.peek(1) {
                Some('a') => self.keyword_or_identifier(TokenKind::False, "false"),
                Some('o') => self.keyword_or_identifier(TokenKind::For, "for"),
                Some('n') => self.keyword_or_identifier(TokenKind::Function, "fn"),
                Some('l') => self.keyword_or_identifier(TokenKind::Float, "float"),
                _ => self.identifier(),
            },
            Some('i') => match self.peek(1) {
                Some('f') => self.keyword_or_identifier(TokenKind::If, "if"),
                Some('n') => self.keyword_or_identifier(TokenKind::Int, "int"),
                _ => self.identifier(),
            },
            Some('l') => match self.peek(2) {
                Some('n') => self.keyword_or_identifier(TokenKind::Len, "len"),
                Some('t') => self.keyword_or_identifier(TokenKind::Let, "let"),
                _ => self.identifier(),
            },
            Some('m') => self.keyword_or_identifier(TokenKind::Match, "match"),
            Some('n') => match self.peek(1) {
                Some('e') => self.keyword_or_identifier(TokenKind::New, "new"),
                Some('o') => self.keyword_or_identifier(TokenKind::None, "none"),
                _ => self.identifier(),
            },
            Some('o') => self.keyword_or_identifier(TokenKind::Or, "or"),
            Some('p') => match self.peek(1) {
                Some('u') => self.keyword_or_identifier(TokenKind::Pub, "pub"),
                Some('r') => match self.peek(2) {
                    Some('o') => self.keyword_or_identifier(TokenKind::Prop, "prop"),
                    _ => match self.peek(5) {
                        Some('l') => self.keyword_or_identifier(TokenKind::PrintLn, "println"),
                        _ => self.keyword_or_identifier(TokenKind::Print, "print"),
                    },
                },
                _ => self.identifier(),
            },
            Some('r') => match self.peek(1) {
                Some('a') => self.keyword_or_identifier(TokenKind::Raise, "raise"),
                Some('e') => self.keyword_or_identifier(TokenKind::Return, "return"),
                _ => self.identifier(),
            },
            Some('s') => self.keyword_or_identifier(TokenKind::Some, "some"),
            Some('t') => match self.peek(2) {
                Some('u') => self.keyword_or_identifier(TokenKind::True, "true"),
                Some('a') => self.keyword_or_identifier(TokenKind::Trait, "trait"),
                Some('i') => self.keyword_or_identifier(TokenKind::This, "this"),
                _ => self.identifier(),
            },
            Some('u') => self.keyword_or_identifier(TokenKind::Use, "use"),
            Some('w') => self.keyword_or_identifier(TokenKind::While, "while"),
            Some('y') => self.keyword_or_identifier(TokenKind::Yield, "yield"),
            Some(' ') => {
                self.advance();
                self.scan()
            }
            Some(_) => self.identifier(),
            None => self.make_token(TokenKind::EOF, ""),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Scanner, buffer::Buffer, tokens::TokenKind};

    macro_rules! assert_scanned_tokens {
        ($code:expr, $expected:expr, $($token:expr),+) => {
            if let Ok(buffer) = Buffer::try_from($code) {
                let mut scanner = Scanner::new(buffer, None);

                let tokens = [$($token),+];
                let mut output = String::new();

                for t in tokens {
                    let token = scanner.scan();
                    assert_eq!(
                        token.kind(),
                        t,
                        "Unable to match expected token '{:?}', found '{:?}' {}",
                        t,
                        token,
                        $code,
                    );
                    output = format!("{}{}", output, token.lexeme());
                }

                assert_eq!(
                    output,
                    $expected,
                    "Unable to match expected lexeme to '{}'",
                    $expected
                );
            } else {
                assert!(false, "Unable to build buffer for '{}'", $code);
            }
        };
    }

    #[test]
    fn test_literals() {
        assert_scanned_tokens!("", "", TokenKind::EOF);
        assert_scanned_tokens!("0", "0", TokenKind::Number);
        assert_scanned_tokens!("1", "1", TokenKind::Number);
        assert_scanned_tokens!("2", "2", TokenKind::Number);
        assert_scanned_tokens!("3", "3", TokenKind::Number);
        assert_scanned_tokens!("4", "4", TokenKind::Number);
        assert_scanned_tokens!("5", "5", TokenKind::Number);
        assert_scanned_tokens!("6", "6", TokenKind::Number);
        assert_scanned_tokens!("7", "7", TokenKind::Number);
        assert_scanned_tokens!("8", "8", TokenKind::Number);
        assert_scanned_tokens!("9", "9", TokenKind::Number);
        assert_scanned_tokens!("42", "42", TokenKind::Number);
        assert_scanned_tokens!("1_000_000", "1_000_000", TokenKind::Number);
        assert_scanned_tokens!("0xff", "0xff", TokenKind::Number);
        assert_scanned_tokens!("0o88", "0o88", TokenKind::Number);
        assert_scanned_tokens!("1.2", "1.2", TokenKind::Double);
        assert_scanned_tokens!(
            "1..2",
            "1..2",
            TokenKind::Number,
            TokenKind::DotDot,
            TokenKind::Number
        );
        assert_scanned_tokens!("'hello, world!'", "hello, world!", TokenKind::String);
        assert_scanned_tokens!(
            "'\\'hello, world!\\''",
            "'hello, world!'",
            TokenKind::String
        );
        assert_scanned_tokens!("true", "true", TokenKind::True);
        assert_scanned_tokens!("false", "false", TokenKind::False);
        assert_scanned_tokens!("none", "none", TokenKind::None);
    }

    #[test]
    fn test_literal_template() {
        assert_scanned_tokens!("`hello`", "hello", TokenKind::String);
        assert_scanned_tokens!(
            "`hello, ${name}`",
            "hello, ${name}",
            TokenKind::String,
            TokenKind::Plus,
            TokenKind::Identifier,
            TokenKind::Plus // TokenKind::String,
                            // TokenKind::Plus
        );
        assert_scanned_tokens!("`${name}`", "", TokenKind::String);

        if let Ok(buffer) = Buffer::try_from("`hello, ${name}`") {
            let mut scanner = Scanner::new(buffer, None);

            if let token = scanner.scan() {
                assert_eq!(token.kind(), TokenKind::String);
                assert_eq!(token.lexeme(), "hello, ");
            }

            if let token = scanner.scan() {
                assert_eq!(token.kind(), TokenKind::Plus);
                assert_eq!(token.lexeme(), "${");
            }
            if let token = scanner.scan() {
                assert_eq!(token.kind(), TokenKind::Identifier);
                assert_eq!(token.lexeme(), "name");
            }
            if let token = scanner.scan() {
                assert_eq!(token.kind(), TokenKind::Plus);
                assert_eq!(token.lexeme(), "}");
            }
            if let token = scanner.scan() {
                assert_eq!(token.kind(), TokenKind::String);
                assert_eq!(token.lexeme(), "");
            }
            if let token = scanner.scan() {
                assert_eq!(token.kind(), TokenKind::EOF);
            }
        }
    }

    #[test]
    fn test_symbols() {
        assert_scanned_tokens!("(", "(", TokenKind::LeftParenthesis);
        assert_scanned_tokens!(")", ")", TokenKind::RightParenthesis);
        assert_scanned_tokens!("[", "[", TokenKind::LeftBrace);
        assert_scanned_tokens!("]", "]", TokenKind::RightBrace);
        assert_scanned_tokens!("{", "{", TokenKind::LeftBracket);
        assert_scanned_tokens!("}", "}", TokenKind::RightBracket);

        assert_scanned_tokens!("-", "-", TokenKind::Minus);
        assert_scanned_tokens!("--", "--", TokenKind::MinusMinus);
        assert_scanned_tokens!("+", "+", TokenKind::Plus);
        assert_scanned_tokens!("++", "++", TokenKind::PlusPlus);
        assert_scanned_tokens!("*", "*", TokenKind::Star);
        assert_scanned_tokens!("**", "**", TokenKind::StarStar);
        assert_scanned_tokens!("/", "/", TokenKind::Slash);
        assert_scanned_tokens!("%", "%", TokenKind::Percent);
        assert_scanned_tokens!("^", "^", TokenKind::Caret);
        assert_scanned_tokens!("~", "~", TokenKind::Tilde);

        assert_scanned_tokens!(".", ".", TokenKind::Dot);
        assert_scanned_tokens!("..", "..", TokenKind::DotDot);
        assert_scanned_tokens!("...", "...", TokenKind::DotDotDot);
    }

    #[test]
    fn test_comments() {
        assert_scanned_tokens!("# hello\n42", "42", TokenKind::Number);
        assert_scanned_tokens!("// hello\n// world\n42", "42", TokenKind::Number);
    }

    #[test]
    fn test_keyword_parsing() {
        assert_scanned_tokens!("as", "as", TokenKind::As);
        assert_scanned_tokens!("asd", "asd", TokenKind::Identifier);
        assert_scanned_tokens!("bool", "bool", TokenKind::Bool);
        assert_scanned_tokens!("boolean", "boolean", TokenKind::Identifier);
        assert_scanned_tokens!("const", "const", TokenKind::Const);
        assert_scanned_tokens!("constant", "constant", TokenKind::Identifier);
        assert_scanned_tokens!("continue", "continue", TokenKind::Continue);
        assert_scanned_tokens!("continues", "continues", TokenKind::Identifier);
        assert_scanned_tokens!("default", "default", TokenKind::Default);
        assert_scanned_tokens!("defaults", "defaults", TokenKind::Identifier);
        assert_scanned_tokens!("else", "else", TokenKind::Else);
        assert_scanned_tokens!("elses", "elses", TokenKind::Identifier);
        assert_scanned_tokens!("for", "for", TokenKind::For);
        assert_scanned_tokens!("force", "force", TokenKind::Identifier);
        assert_scanned_tokens!("fn", "fn", TokenKind::Function);
        assert_scanned_tokens!("fnaf", "fnaf", TokenKind::Identifier);
        assert_scanned_tokens!("if", "if", TokenKind::If);
        assert_scanned_tokens!("ifs", "ifs", TokenKind::Identifier);
        assert_scanned_tokens!("len", "len", TokenKind::Len);
        assert_scanned_tokens!("lens", "lens", TokenKind::Identifier);
        assert_scanned_tokens!("let", "let", TokenKind::Let);
        assert_scanned_tokens!("lets", "lets", TokenKind::Identifier);
        assert_scanned_tokens!("match", "match", TokenKind::Match);
        assert_scanned_tokens!("matches", "matches", TokenKind::Identifier);
        assert_scanned_tokens!("print", "print", TokenKind::Print);
        assert_scanned_tokens!("prints", "prints", TokenKind::Identifier);
        assert_scanned_tokens!("println", "println", TokenKind::PrintLn);
        assert_scanned_tokens!("printlns", "printlns", TokenKind::Identifier);
        assert_scanned_tokens!("prop", "prop", TokenKind::Prop);
        assert_scanned_tokens!("props", "props", TokenKind::Identifier);
        assert_scanned_tokens!("pub", "pub", TokenKind::Pub);
        assert_scanned_tokens!("pubs", "pubs", TokenKind::Identifier);
        assert_scanned_tokens!("return", "return", TokenKind::Return);
        assert_scanned_tokens!("returns", "returns", TokenKind::Identifier);
        assert_scanned_tokens!("trait", "trait", TokenKind::Trait);
        assert_scanned_tokens!("traits", "traits", TokenKind::Identifier);
        assert_scanned_tokens!("this", "this", TokenKind::This);
        assert_scanned_tokens!("thisis", "thisis", TokenKind::Identifier);
        assert_scanned_tokens!("use", "use", TokenKind::Use);
        assert_scanned_tokens!("uses", "uses", TokenKind::Identifier);
        assert_scanned_tokens!("while", "while", TokenKind::While);
        assert_scanned_tokens!("whiles", "whiles", TokenKind::Identifier);
        assert_scanned_tokens!("yield", "yield", TokenKind::Yield);
        assert_scanned_tokens!("yields", "yields", TokenKind::Identifier);
    }
}
