//! Kind language for unary higher-kinded type parameters (Phase 5).
//!
//! Only two kinds are supported:
//! - [`Kind::Type`] (`*`) — ordinary types (`int`, `Option<int>`, …)
//! - [`Kind::Arrow`] (`* -> *`) — unary type constructors (`Option`, `F` in `F<A>`)
//!
//! Higher arities and kind variables are intentionally out of scope.

use std::fmt;

/// A kind in the unary HKT fragment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Kind {
    /// `*` — a proper type.
    #[default]
    Type,
    /// `* -> *` — a unary type constructor.
    Arrow,
}

impl Kind {
    /// True when this kind classifies a proper type (`*`).
    pub fn is_type(self) -> bool {
        matches!(self, Kind::Type)
    }

    /// True when this kind classifies a unary constructor (`* -> *`).
    pub fn is_arrow(self) -> bool {
        matches!(self, Kind::Arrow)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Kind::Type => write!(f, "*"),
            Kind::Arrow => write!(f, "* -> *"),
        }
    }
}

impl From<parser::ast::Kind> for Kind {
    fn from(k: parser::ast::Kind) -> Self {
        match k {
            parser::ast::Kind::Type => Kind::Type,
            parser::ast::Kind::Arrow => Kind::Arrow,
        }
    }
}

impl From<Kind> for parser::ast::Kind {
    fn from(k: Kind) -> Self {
        match k {
            Kind::Type => parser::ast::Kind::Type,
            Kind::Arrow => parser::ast::Kind::Arrow,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_display_round_trips() {
        assert_eq!(Kind::Type.to_string(), "*");
        assert_eq!(Kind::Arrow.to_string(), "* -> *");
    }
}
