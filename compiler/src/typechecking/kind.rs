//! Kind language for higher-kinded type parameters.
//!
//! - [`Kind::Type`] (`*`) — ordinary types (`int`, `Option<int>`, …)
//! - [`Kind::Constraint`] — typeclass predicates.
//! - [`Kind::Arrow`] — type constructors, including `* -> * -> *`,
//!   `* -> Constraint`, and `(* -> *) -> *`.

use std::fmt;

/// A kind in the HKT fragment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum Kind {
    /// `*` — a proper type.
    #[default]
    Type,
    /// `Constraint` — a typeclass predicate.
    Constraint,
    /// `domain -> codomain` — a type constructor kind.
    Arrow(Box<Kind>, Box<Kind>),
}

impl Kind {
    pub fn arrow(domain: Kind, codomain: Kind) -> Self {
        Kind::Arrow(Box::new(domain), Box::new(codomain))
    }

    /// Build a first-order constructor kind with `arity` type arguments.
    pub fn constructor(arity: usize) -> Self {
        (0..arity).fold(Kind::Type, |codomain, _| Kind::arrow(Kind::Type, codomain))
    }

    /// Build a first-order constraint constructor kind with `arity` type arguments.
    pub fn constraint_constructor(arity: usize) -> Self {
        (0..arity).fold(Kind::Constraint, |codomain, _| {
            Kind::arrow(Kind::Type, codomain)
        })
    }

    /// True when this kind classifies a proper type (`*`).
    pub fn is_type(&self) -> bool {
        matches!(self, Kind::Type)
    }

    /// True when this kind classifies a typeclass predicate.
    pub fn is_constraint(&self) -> bool {
        matches!(self, Kind::Constraint)
    }

    /// True when this kind classifies a type constructor.
    pub fn is_arrow(&self) -> bool {
        matches!(self, Kind::Arrow(_, _))
    }

    /// Number of top-level arguments before the kind produces a proper type.
    pub fn arity(&self) -> usize {
        match self {
            Kind::Type | Kind::Constraint => 0,
            Kind::Arrow(_, codomain) => 1 + codomain.arity(),
        }
    }

    pub fn is_constructor_kind(&self) -> bool {
        self.arity() > 0
    }

    /// Result kind after applying every top-level argument.
    pub fn result_kind(&self) -> &Kind {
        let mut current = self;
        while let Kind::Arrow(_, codomain) = current {
            current = codomain.as_ref();
        }
        current
    }

    /// True when this kind is a typeclass predicate constructor, e.g. `* -> Constraint`.
    pub fn is_constraint_constructor_kind(&self) -> bool {
        self.arity() > 0 && self.result_kind().is_constraint()
    }

    /// Top-level argument kinds in source order.
    pub fn argument_kinds(&self) -> Vec<Kind> {
        let mut args = Vec::new();
        let mut current = self;
        while let Kind::Arrow(domain, codomain) = current {
            args.push(domain.as_ref().clone());
            current = codomain.as_ref();
        }
        args
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Kind::Type => write!(f, "*"),
            Kind::Constraint => write!(f, "Constraint"),
            Kind::Arrow(domain, codomain) => {
                match domain.as_ref() {
                    Kind::Arrow(_, _) => write!(f, "({})", domain)?,
                    Kind::Type | Kind::Constraint => write!(f, "{}", domain)?,
                }
                write!(f, " -> {}", codomain)
            }
        }
    }
}

impl From<parser::ast::Kind> for Kind {
    fn from(k: parser::ast::Kind) -> Self {
        match k {
            parser::ast::Kind::Type => Kind::Type,
            parser::ast::Kind::Constraint => Kind::Constraint,
            parser::ast::Kind::Arrow(domain, codomain) => {
                Kind::arrow((*domain).into(), (*codomain).into())
            }
        }
    }
}

impl From<Kind> for parser::ast::Kind {
    fn from(k: Kind) -> Self {
        match k {
            Kind::Type => parser::ast::Kind::Type,
            Kind::Constraint => parser::ast::Kind::Constraint,
            Kind::Arrow(domain, codomain) => {
                parser::ast::Kind::Arrow(Box::new((*domain).into()), Box::new((*codomain).into()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_display_round_trips() {
        assert_eq!(Kind::Type.to_string(), "*");
        assert_eq!(Kind::Constraint.to_string(), "Constraint");
        assert_eq!(Kind::constructor(1).to_string(), "* -> *");
        assert_eq!(
            Kind::constraint_constructor(1).to_string(),
            "* -> Constraint"
        );
        assert_eq!(Kind::constructor(2).to_string(), "* -> * -> *");
        assert_eq!(
            Kind::arrow(Kind::constructor(1), Kind::Type).to_string(),
            "(* -> *) -> *"
        );
    }

    #[test]
    fn kind_argument_kinds_follow_tree_shape() {
        let binary = Kind::constructor(2);
        assert_eq!(binary.arity(), 2);
        assert_eq!(binary.argument_kinds(), vec![Kind::Type, Kind::Type]);
        assert_eq!(binary.result_kind(), &Kind::Type);

        let higher_order = Kind::arrow(Kind::constructor(1), Kind::Type);
        assert_eq!(higher_order.arity(), 1);
        assert_eq!(higher_order.argument_kinds(), vec![Kind::constructor(1)]);

        let predicate = Kind::constraint_constructor(1);
        assert_eq!(predicate.arity(), 1);
        assert_eq!(predicate.argument_kinds(), vec![Kind::Type]);
        assert_eq!(predicate.result_kind(), &Kind::Constraint);
        assert!(predicate.is_constraint_constructor_kind());
    }
}
