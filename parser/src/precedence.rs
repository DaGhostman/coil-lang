use scanner::tokens::TokenKind;

#[derive(Debug, Default, PartialOrd, PartialEq, Hash, Copy, Clone)]
#[repr(u8)]
pub enum Precedence {
    #[default]
    None = 0,
    Assign,
    Or,
    Xor,
    And,
    Equal,
    Compare,
    Term,
    Factor,
    Unary,
    Call,
    Primary,
}

impl From<Precedence> for u8 {
    fn from(value: Precedence) -> Self {
        value as u8
    }
}

impl From<u8> for Precedence {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::None,
            1 => Self::Assign,
            2 => Self::Or,
            3 => Self::Xor,
            4 => Self::And,
            5 => Self::Equal,
            6 => Self::Compare,
            7 => Self::Term,
            8 => Self::Factor,
            9 => Self::Unary,
            10 => Self::Call,
            11 => Self::Primary,
            _ => Self::None,
        }
    }
}

impl Precedence {
    pub fn get(code: TokenKind) -> Precedence {
        match code {
            TokenKind::LeftParenthesis => Precedence::Call,
            TokenKind::Plus => Precedence::Term,
            TokenKind::Star
            | TokenKind::StarStar
            | TokenKind::Minus
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::LessLess
            | TokenKind::GreaterGreater => Precedence::Factor,
            TokenKind::EqualEqual | TokenKind::BangEqual => Precedence::Equal,
            TokenKind::Less
            | TokenKind::LessEqual
            | TokenKind::Greater
            | TokenKind::GreaterEqual => Precedence::Compare,
            TokenKind::Ampersand | TokenKind::And => Precedence::And,
            TokenKind::Pipe | TokenKind::Or => Precedence::Or,
            TokenKind::Caret => Precedence::Xor,
            TokenKind::PlusPlus
            | TokenKind::MinusMinus
            | TokenKind::Bang
            | TokenKind::Tilde
            | TokenKind::As => Precedence::Unary,
            TokenKind::Equal => Precedence::Assign,
            TokenKind::DotDot => Precedence::Unary,
            TokenKind::Dot => Precedence::Call,
            TokenKind::Match | TokenKind::If => Precedence::Primary,
            _ => Precedence::None,
        }
    }

    pub fn next(&self) -> Precedence {
        ((*self as u8) + 1 as u8).into()
    }
}

#[cfg(test)]
mod tests {
    use crate::precedence::Precedence;

    #[test]
    fn test_comparison() {
        assert!(Precedence::None < Precedence::Assign);
    }
}
