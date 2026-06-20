use std::{borrow::Borrow, fmt::Display};

use chumsky::span::SimpleSpan;
pub type Output<'parser> = (SimpleSpan, Box<Expression<'parser>>);

#[derive(Clone, PartialEq, Debug, Copy)]
pub enum Visibility {
    Private,
    Public,
}

impl Default for Visibility {
    fn default() -> Self {
        Visibility::Private
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Expression<'expr> {
    Noop(Output<'expr>),
    Integer(i64),
    Float(f64),
    String(&'expr str),
    Bool(bool),
    Module(String, Output<'expr>),

    Argument(&'expr str, &'expr str),
    Identifier(&'expr str),
    Type(&'expr str),
    Comment(&'expr str),
    Print(Output<'expr>, Option<Vec<Output<'expr>>>),
    Format(Output<'expr>, Option<Vec<Output<'expr>>>),
    Return(Output<'expr>),
    ImplicitReturn(Output<'expr>),
    Yield(Output<'expr>),
    Resume(Output<'expr>, Option<Output<'expr>>),
    Negate(Output<'expr>),
    Not(Output<'expr>),
    Positive(Output<'expr>),
    Default(&'expr str),
    Inc(Output<'expr>),
    Dec(Output<'expr>),
    Add(Output<'expr>, Output<'expr>),
    Sub(Output<'expr>, Output<'expr>),
    Mul(Output<'expr>, Output<'expr>),
    Div(Output<'expr>, Output<'expr>),
    Mod(Output<'expr>, Output<'expr>),
    Pow(Output<'expr>, Output<'expr>),
    Shl(Output<'expr>, Output<'expr>),
    Shr(Output<'expr>, Output<'expr>),
    Xor(Output<'expr>, Output<'expr>),
    And(Output<'expr>, Output<'expr>),
    BitAnd(Output<'expr>, Output<'expr>),
    Or(Output<'expr>, Output<'expr>),
    BitOr(Output<'expr>, Output<'expr>),
    Eq(Output<'expr>, Output<'expr>),
    Neq(Output<'expr>, Output<'expr>),
    Leq(Output<'expr>, Output<'expr>),
    Geq(Output<'expr>, Output<'expr>),
    Le(Output<'expr>, Output<'expr>),
    Gt(Output<'expr>, Output<'expr>),

    List(Vec<Output<'expr>>),
    Expr(Output<'expr>),
    Group(Output<'expr>),
    ExprStatement(Output<'expr>),
    Statement(Output<'expr>),
    Fragment(Vec<Output<'expr>>),
    Block(Vec<Output<'expr>>),
    Program(Vec<Output<'expr>>),
    Defer(Output<'expr>),

    Assignment(Output<'expr>, Output<'expr>),

    Use {
        path: Vec<String>,
        name: String,
        alias: Option<String>,
    },

    Function {
        name: &'expr str,
        args: Output<'expr>,
        returns: Option<&'expr str>,
        body: Output<'expr>,
    },

    Branch(Option<Output<'expr>>, Output<'expr>),

    If(Vec<Output<'expr>>),

    Call {
        name: Output<'expr>,
        args: Option<Vec<Output<'expr>>>,
    },

    Loop {
        identifier: Option<Output<'expr>>,
        iterable: Output<'expr>,
        body: Output<'expr>,
    },

    Variable(&'expr str, Option<Output<'expr>>),
    Constant(Output<'expr>, Option<Output<'expr>>),

    Implementation(&'expr str, &'expr str, Vec<Output<'expr>>),
    Class(&'expr str, Vec<Output<'expr>>),
    Field(Visibility, Output<'expr>, Output<'expr>),
    Method(Visibility, Output<'expr>),
    Member(Output<'expr>),
    Access(Output<'expr>, &'expr str),
    Update(Output<'expr>, Output<'expr>),

    Instantiate(Output<'expr>, Option<Vec<Output<'expr>>>),

    // ---- Phase 15A: sum types and pattern matching ----
    /// `enum Name { Variant1, Variant2(T1, T2), ... }` — a top-level
    /// sum-type declaration. Carries no executable body; the
    /// declaration is registered with the typechecker in 15B and the
    /// code emitter in 15C. In 15A, parsing produces this node so the
    /// AST is stable, but downstream passes stub it out (see
    /// `infer.rs` and `lib.rs`).
    EnumDecl {
        name: &'expr str,
        variants: Vec<Output<'expr>>,
    },
    /// One variant inside an `EnumDecl` payload list. The payload
    /// types are wrapped as `Expression::Type(...)` (matching the
    /// existing `class` field syntax) so the parser can re-use the
    /// same type-name production. Zero-arity variants carry an empty
    /// payload.
    EnumVariant {
        name: &'expr str,
        payload: Vec<Output<'expr>>,
    },
    /// `EnumName::Variant(args...)` — a *qualified* constructor
    /// application. The enum name is required (bare `Variant(...)` is
    /// parsed as a `Call`, not a `Construct`).
    Construct {
        enum_name: &'expr str,
        variant_name: &'expr str,
        args: Vec<Output<'expr>>,
    },
    /// `match scrutinee { pat => body, ... }` — pattern matching. The
    /// pre-walk walks the `scrutinee` and each arm's `body`; pattern
    /// descendants are walked separately by the HM pre-walk helper
    /// (see `compiler::typechecking::id::pre_walk_pattern`).
    Match {
        scrutinee: Output<'expr>,
        arms: Vec<MatchArm<'expr>>,
    },
}

/// One arm inside an `Expression::Match`. The pattern is
/// `Pattern<'expr>`; the body is the standard `Output<'expr>`
/// expression wrapper.
#[derive(Clone, PartialEq, Debug)]
pub struct MatchArm<'expr> {
    pub pattern: Pattern<'expr>,
    pub body: Output<'expr>,
}

/// Patterns matched against the scrutinee in a `match` expression.
///
/// - `Wildcard` matches anything and binds nothing. Produced for both
///   the `_` and `default` source tokens; the literal token is
///   discarded during parsing (Decision C in the Phase 15A plan).
/// - `Binding { name }` matches anything and binds `name` to the
///   scrutinee. (Implemented as a 15B hook; 15A only parses it.)
/// - `Constructor` matches a specific enum variant. The payload list
///   has the same arity as the variant's payload; nested patterns
///   enable tuple destructuring.
#[derive(Clone, PartialEq, Debug)]
pub enum Pattern<'expr> {
    Wildcard,
    Binding { name: &'expr str },
    Constructor {
        enum_name: &'expr str,
        variant_name: &'expr str,
        payload: Vec<Pattern<'expr>>,
    },
}

impl<'a> Display for Pattern<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wildcard => write!(f, "_"),
            Self::Binding { name } => write!(f, "{}", name),
            Self::Constructor {
                enum_name,
                variant_name,
                payload,
            } => {
                write!(f, "{}::{}", enum_name, variant_name)?;
                if !payload.is_empty() {
                    write!(
                        f,
                        "({})",
                        payload
                            .iter()
                            .map(|p| p.to_string())
                            .collect::<Vec<String>>()
                            .join(", ")
                    )?;
                }
                Ok(())
            }
        }
    }
}

impl<'a> Display for Expression<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Integer(n) => write!(f, "{}", n),
            Self::Float(n) => write!(f, "{:.?}", n),
            Self::Identifier(id) => write!(f, "{}", id),
            Self::Not(n) => write!(f, "~{}", n.1),
            Self::Sub(lhs, rhs) => write!(f, "{} - {}", lhs.borrow().1, rhs.borrow().1),
            Self::Add(lhs, rhs) => write!(f, "{} + {}", lhs.borrow().1, rhs.borrow().1),
            Self::Mul(lhs, rhs) => write!(f, "{} * {}", lhs.borrow().1, rhs.borrow().1),
            Self::Div(lhs, rhs) => write!(f, "{} / {}", lhs.borrow().1, rhs.borrow().1),
            Self::Mod(lhs, rhs) => write!(f, "{} % {}", lhs.borrow().1, rhs.borrow().1),
            Self::Shl(lhs, rhs) => write!(f, "{} << {}", lhs.borrow().1, rhs.borrow().1),
            Self::Shr(lhs, rhs) => write!(f, "{} >> {}", lhs.borrow().1, rhs.borrow().1),
            Self::BitOr(lhs, rhs) => write!(f, "{} | {}", lhs.borrow().1, rhs.borrow().1),
            Self::Or(lhs, rhs) => write!(f, "{} || {}", lhs.borrow().1, rhs.borrow().1),
            Self::And(lhs, rhs) => write!(f, "{} && {}", lhs.borrow().1, rhs.borrow().1),
            Self::BitAnd(lhs, rhs) => write!(f, "{} & {}", lhs.borrow().1, rhs.borrow().1),
            Self::Xor(lhs, rhs) => write!(f, "{} ^ {}", lhs.borrow().1, rhs.borrow().1),
            Self::Pow(lhs, rhs) => write!(f, "{} ** {}", lhs.borrow().1, rhs.borrow().1),
            Self::Gt(lhs, rhs) => write!(f, "{} > {}", lhs.borrow().1, rhs.borrow().1),
            Self::Le(lhs, rhs) => write!(f, "{} < {}", lhs.borrow().1, rhs.borrow().1),
            Self::Eq(lhs, rhs) => write!(f, "{} == {}", lhs.borrow().1, rhs.borrow().1),
            Self::Neq(lhs, rhs) => write!(f, "{} != {}", lhs.borrow().1, rhs.borrow().1),
            Self::Inc(n) => write!(f, "{}++", n.borrow().1),
            Self::Dec(n) => write!(f, "{}--", n.borrow().1),
            Self::Negate(n) => write!(f, "-{}", n.borrow().1),
            Self::Positive(n) => write!(f, "+{}", n.borrow().1),
            Self::Expr(e) => write!(f, "{}", e.1),
            Self::ExprStatement(e) => write!(f, "{};", e.1),
            Self::Fragment(list) | Self::Block(list) => write!(
                f,
                "{}",
                list.iter()
                    .map(|e| e.1.to_string())
                    .collect::<Vec<String>>()
                    .join(" ")
            ),
            Self::Group(g) => write!(f, "({})", g.1),
            Self::Statement(s) => write!(f, "{};\n", s.1),
            Self::String(s) => write!(f, "\"{}\"", s),
            Self::Print(fmt, params) => write!(
                f,
                "print {}{}",
                fmt.borrow().1,
                params
                    .clone()
                    .map_or(String::default(), |p: Vec<Output<'a>>| format!(
                        ", {}",
                        p.iter()
                            .map(|p| p.1.to_string())
                            .collect::<Vec<String>>()
                            .join(", ")
                    ))
            ),
            Self::Function {
                name,
                args,
                returns,
                body,
            } => {
                write!(
                    f,
                    "fn {}({}){} {{\n{}}}",
                    name,
                    args.1,
                    returns.map_or(String::default(), |ret| format!(" -> {}", ret)),
                    body.1
                )
            }
            Self::Defer(b) => write!(f, "defer {}", b.1),
            Self::Call { name, args } => {
                write!(
                    f,
                    "{}({})",
                    name.1,
                    args.clone().map_or(String::default(), |p| p
                        .iter()
                        .map(|p| p.1.to_string())
                        .collect::<Vec<String>>()
                        .join(", "))
                )
            }
            Self::Loop {
                identifier,
                iterable,
                body,
            } => {
                write!(
                    f,
                    "{} {{\n{}}}",
                    match identifier {
                        Some(_) => String::new(),
                        None => format!("while {}", iterable.1),
                    },
                    body.1
                )
            }
            Self::Assignment(n, e) => {
                write!(f, "{} = {}", n.1, e.1)
            }
            Self::Noop(n) => write!(f, "@{{ {} }}@", n.1.to_string()),
            Self::EnumDecl { name, variants } => {
                let vs = variants
                    .iter()
                    .map(|v| match v.1.as_ref() {
                        Self::EnumVariant { name, payload } => {
                            if payload.is_empty() {
                                name.to_string()
                            } else {
                                format!(
                                    "{}({})",
                                    name,
                                    payload
                                        .iter()
                                        .map(|p| p.1.to_string())
                                        .collect::<Vec<String>>()
                                        .join(", ")
                                )
                            }
                        }
                        _ => String::from("?"),
                    })
                    .collect::<Vec<String>>()
                    .join(", ");
                write!(f, "enum {} {{ {} }}", name, vs)
            }
            Self::EnumVariant { name, payload } => {
                if payload.is_empty() {
                    write!(f, "{}", name)
                } else {
                    write!(
                        f,
                        "{}({})",
                        name,
                        payload
                            .iter()
                            .map(|p| p.1.to_string())
                            .collect::<Vec<String>>()
                            .join(", ")
                    )
                }
            }
            Self::Construct {
                enum_name,
                variant_name,
                args,
            } => write!(
                f,
                "{}::{}({})",
                enum_name,
                variant_name,
                args.iter()
                    .map(|a| a.1.to_string())
                    .collect::<Vec<String>>()
                    .join(", ")
            ),
            Self::Match { scrutinee, arms } => {
                let as_str = arms
                    .iter()
                    .map(|a| {
                        let pat = a.pattern.to_string();
                        let body = a.body.1.to_string();
                        format!("{} => {}", pat, body)
                    })
                    .collect::<Vec<String>>()
                    .join(", ");
                write!(f, "match {} {{ {} }}", scrutinee.1, as_str)
            }
            e => todo!("Missing rest of nodes: {}", e),
        }
    }
}
