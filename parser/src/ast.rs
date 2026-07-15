use std::{borrow::Borrow, fmt::Display};

use chumsky::span::SimpleSpan;
pub type Output<'parser> = (SimpleSpan, Box<Expression<'parser>>);

#[derive(Clone, PartialEq, Debug, Copy, Default)]
pub enum Visibility {
    #[default]
    Private,
    Public,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Expression<'expr> {
    Noop(Output<'expr>),
    Integer(i64),
    Float(f64),
    String(&'expr str),
    Bool(bool),
    Module(String, Output<'expr>),

    /// Function parameter `(T name)` with a full type annotation.
    Argument(Output<'expr>, &'expr str),
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
    Array(Vec<Output<'expr>>),
    Expr(Output<'expr>),
    Group(Output<'expr>),
    ExprStatement(Output<'expr>),
    Statement(Output<'expr>),
    Fragment(Vec<Output<'expr>>),
    Block(Vec<Output<'expr>>),
    Program(Vec<Output<'expr>>),
    Defer(Output<'expr>),

    Assignment(Output<'expr>, Output<'expr>),

    /// Tuple literal or FFI arg-type / arg-value bundle.
    Tuple(Vec<Output<'expr>>),

    /// Anonymous record literal `{ name: expr, ... }`.
    Dict(Vec<RecordFieldValue<'expr>>),

    /// Element access `target[index]`.
    Index(Output<'expr>, Output<'expr>),

    /// Dynamic library load: `dload(path)`.
    Dload(Output<'expr>),

    /// Runtime FFI registration: `declare(lib, name, args_tuple, ret_type)`.
    Declare(Vec<Output<'expr>>),

    /// Runtime FFI call: `invoke(lib, fn_id, args_tuple)`.
    Invoke(Vec<Output<'expr>>),

    Use {
        path: Vec<String>,
        name: String,
        alias: Option<String>,
    },

    /// `extern "libname" { fn name(args) -> ret; ... }`.
    ExternBlock {
        library: String,
        declarations: Vec<ExternFunction<'expr>>,
    },

    Function {
        name: &'expr str,
        args: Output<'expr>,
        returns: Option<Output<'expr>>,
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

    /// `type Name = T;` — type alias declaration.
    TypeAlias {
        name: &'expr str,
        ty: Box<Output<'expr>>,
    },

    /// Top-level `enum` declaration.
    EnumDecl {
        name: &'expr str,
        variants: Vec<Output<'expr>>,
    },
    /// One variant inside an `enum` body.
    EnumVariant {
        name: &'expr str,
        payload: EnumVariantPayload<'expr>,
    },
    /// Qualified constructor application `EnumName::Variant(...)`.
    Construct {
        enum_name: &'expr str,
        variant_name: &'expr str,
        fields: EnumConstructPayload<'expr>,
    },
    /// Pattern match expression.
    Match {
        scrutinee: Output<'expr>,
        arms: Vec<MatchArm<'expr>>,
    },
}

/// Payload shape for an `enum` variant declaration.
#[derive(Clone, PartialEq, Debug)]
pub enum EnumVariantPayload<'expr> {
    /// No fields: `Foo`.
    Unit,
    /// Tuple of typed expressions (each `Output` is typically an
    /// `Expression::Type`): `Foo(T1, T2, ...)`.
    Tuple(Vec<Output<'expr>>),
    /// Record of named typed fields: `Foo { x: T, y: T }`.
    Record(Vec<RecordFieldDecl<'expr>>),
}

/// One typed field in an `enum` record variant declaration.
#[derive(Clone, PartialEq, Debug)]
pub struct RecordFieldDecl<'expr> {
    pub name: &'expr str,
    pub value: Output<'expr>,
}

/// One function declaration inside an `ExternBlock`.
#[derive(Clone, PartialEq, Debug)]
pub struct ExternFunction<'expr> {
    pub name: &'expr str,
    pub args: Output<'expr>,
    pub returns: Option<&'expr str>,
}

/// One field in a record constructor.
#[derive(Clone, PartialEq, Debug)]
pub struct RecordFieldValue<'expr> {
    pub name: &'expr str,
    pub value: Output<'expr>,
}

/// One field in a record pattern. Shorthand `x` desugars to `x: x`.
#[derive(Clone, PartialEq, Debug)]
pub struct PatternField<'expr> {
    pub name: &'expr str,
    pub pattern: Pattern<'expr>,
}

/// Payload shape for a qualified constructor application.
#[derive(Clone, PartialEq, Debug)]
pub enum EnumConstructPayload<'expr> {
    /// `Foo` (no fields — bare qualified enum value).
    Unit,
    /// `Foo(arg1, arg2, ...)`.
    Tuple(Vec<Output<'expr>>),
    /// `Foo { name: expr, ... }` — fields may appear in any order.
    Record(Vec<RecordFieldValue<'expr>>),
}

/// One arm inside a `match` expression.
#[derive(Clone, PartialEq, Debug)]
pub struct MatchArm<'expr> {
    pub pattern: Pattern<'expr>,
    pub body: Output<'expr>,
}

/// Match pattern: wildcard, binding, or qualified constructor.
#[derive(Clone, PartialEq, Debug)]
pub enum Pattern<'expr> {
    Wildcard,
    Binding {
        name: &'expr str,
    },
    Constructor {
        enum_name: &'expr str,
        variant_name: &'expr str,
        payload: PatternPayload<'expr>,
    },
}

/// Payload shape for a constructor pattern.
#[derive(Clone, PartialEq, Debug)]
pub enum PatternPayload<'expr> {
    /// `Foo` (no sub-patterns — bare qualified enum pattern).
    Unit,
    /// `Foo(p1, p2, ...)`.
    Tuple(Vec<Pattern<'expr>>),
    /// `Foo { name (shorthand) or name: pattern, ... }`.
    /// Shorthand `x` desugars at parse time to
    /// `PatternField { name: "x", pattern: Binding("x") }`.
    Record(Vec<PatternField<'expr>>),
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
                match payload {
                    PatternPayload::Unit => Ok(()),
                    PatternPayload::Tuple(parts) => {
                        if !parts.is_empty() {
                            write!(
                                f,
                                "({})",
                                parts
                                    .iter()
                                    .map(|p| p.to_string())
                                    .collect::<Vec<String>>()
                                    .join(", ")
                            )?;
                        }
                        Ok(())
                    }
                    PatternPayload::Record(fields) => {
                        let parts: Vec<String> = fields
                            .iter()
                            .map(|pf| match &pf.pattern {
                                // Shorthand `x`: render as just `x`.
                                Pattern::Binding { name } if *name == pf.name => name.to_string(),
                                _ => format!("{}: {}", pf.name, pf.pattern),
                            })
                            .collect();
                        write!(f, "{{ {} }}", parts.join(", "))
                    }
                }
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
            Self::Statement(s) => writeln!(f, "{};", s.1),
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
            Self::Dload(path) => write!(f, "dload({})", path.1),
            Self::Tuple(items) => write!(
                f,
                "({})",
                items
                    .iter()
                    .map(|a| a.1.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Array(items) => write!(
                f,
                "[{}]",
                items
                    .iter()
                    .map(|a| a.1.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Index(target, index) => write!(f, "{}[{}]", target.1, index.1),
            Self::Declare(args) | Self::Invoke(args) => {
                let kw = if matches!(self, Self::Declare(_)) {
                    "declare"
                } else {
                    "invoke"
                };
                write!(
                    f,
                    "{}({})",
                    kw,
                    args.iter()
                        .map(|a| a.1.to_string())
                        .collect::<Vec<String>>()
                        .join(", ")
                )
            }
            Self::Function {
                name,
                args,
                returns,
                body,
            } => {
                let ret_str = returns
                    .as_ref()
                    .map(|ret| format!(" -> {}", ret.1))
                    .unwrap_or_default();
                write!(f, "fn {}({}){} {{\n{}}}", name, args.1, ret_str, body.1)
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
            Self::Noop(n) => write!(f, "@{{ {} }}@", n.1),
            Self::TypeAlias { name, ty } => write!(f, "type {} = {};", name, ty.1),
            Self::Dict(items) => {
                let parts: Vec<String> = items
                    .iter()
                    .map(|f| format!("{}: {}", f.name, f.value.1))
                    .collect();
                write!(f, "{{ {} }}", parts.join(", "))
            }
            Self::EnumDecl { name, variants } => {
                let vs = variants
                    .iter()
                    .map(|v| match v.1.as_ref() {
                        Self::EnumVariant { name, payload } => match payload {
                            EnumVariantPayload::Unit => name.to_string(),
                            EnumVariantPayload::Tuple(parts) => {
                                if parts.is_empty() {
                                    name.to_string()
                                } else {
                                    format!(
                                        "{}({})",
                                        name,
                                        parts
                                            .iter()
                                            .map(|p| p.1.to_string())
                                            .collect::<Vec<String>>()
                                            .join(", ")
                                    )
                                }
                            }
                            EnumVariantPayload::Record(fields) => {
                                let parts: Vec<String> = fields
                                    .iter()
                                    .map(|rf| format!("{}: {}", rf.name, rf.value.1))
                                    .collect();
                                format!("{} {{ {} }}", name, parts.join(", "))
                            }
                        },
                        _ => String::from("?"),
                    })
                    .collect::<Vec<String>>()
                    .join(", ");
                write!(f, "enum {} {{ {} }}", name, vs)
            }
            Self::EnumVariant { name, payload } => match payload {
                EnumVariantPayload::Unit => write!(f, "{}", name),
                EnumVariantPayload::Tuple(parts) => {
                    if parts.is_empty() {
                        write!(f, "{}", name)
                    } else {
                        write!(
                            f,
                            "{}({})",
                            name,
                            parts
                                .iter()
                                .map(|p| p.1.to_string())
                                .collect::<Vec<String>>()
                                .join(", ")
                        )
                    }
                }
                EnumVariantPayload::Record(fields) => {
                    let parts: Vec<String> = fields
                        .iter()
                        .map(|rf| format!("{}: {}", rf.name, rf.value.1))
                        .collect();
                    write!(f, "{} {{ {} }}", name, parts.join(", "))
                }
            },
            Self::Construct {
                enum_name,
                variant_name,
                fields,
            } => {
                write!(f, "{}::{}", enum_name, variant_name)?;
                match fields {
                    EnumConstructPayload::Unit => Ok(()),
                    EnumConstructPayload::Tuple(args) => {
                        write!(
                            f,
                            "({})",
                            args.iter()
                                .map(|a| a.1.to_string())
                                .collect::<Vec<String>>()
                                .join(", ")
                        )
                    }
                    EnumConstructPayload::Record(parts) => {
                        let strs: Vec<String> = parts
                            .iter()
                            .map(|rf| format!("{}: {}", rf.name, rf.value.1))
                            .collect();
                        write!(f, "{{ {} }}", strs.join(", "))
                    }
                }
            }
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
            Self::Access(receiver, field) => {
                write!(f, "{}.{}", receiver.1, field)
            }
            Self::Type(n) => write!(f, "{}", n),
            e => write!(f, "<unhandled: {:?}>", e),
        }
    }
}
