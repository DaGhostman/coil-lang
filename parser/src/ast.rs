use std::{borrow::Borrow, fmt::Display};

use chumsky::span::SimpleSpan;
pub type Output<'parser> = (SimpleSpan, Box<Expression<'parser>>);

#[derive(Clone, PartialEq)]
pub enum Expression<'expr> {
    Noop(Output<'expr>),
    Integer(i64),
    Float(f64),
    String(&'expr str),
    Bool(bool),
    Module(String, Output<'expr>),

    Argument(Output<'expr>, Output<'expr>),
    Identifier(&'expr str),
    Type(Output<'expr>),
    Comment(&'expr str),
    Print(Output<'expr>, Option<Vec<Output<'expr>>>),
    Format(Output<'expr>, Option<Vec<Output<'expr>>>),
    Return(Output<'expr>),
    ImplicitReturn(Output<'expr>),
    Yield(Output<'expr>),
    Resume(Output<'expr>, Option<Output<'expr>>),
    Negate(Output<'expr>),
    Not(Output<'expr>),
    Flip(Output<'expr>),
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
        returns: Option<Output<'expr>>,
        body: Output<'expr>,
    },

    GenericParam {
        name: &'expr str,
        bounds: Vec<&'expr str>,
    },

    FunctionWithGenerics {
        generics: Vec<(&'expr str, Vec<&'expr str>)>, // (name, bounds)
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

    Match(Output<'expr>, Vec<Output<'expr>>),

    // HM Type System Features
    TypeVar(&'expr str, usize), // Type variable for inference
    SumType(Output<'expr>, Vec<&'expr str>, Vec<Output<'expr>>), // Sum type (enum) with optional type params: enum Result<T, E> { Ok(T), Err(E) }
    Variant(Output<'expr>, Option<Vec<Output<'expr>>>),          // Variant for sum type
    VariantItem(Output<'expr>, Output<'expr>), // Simple variant (e.g., Color::Red)
    VariantWithDestructure(Output<'expr>, Output<'expr>, Vec<Output<'expr>>), // Variant with type, name, and destructured fields (e.g., Color::Rgb(255, 127, 0))
    GenericDecl(Vec<&'expr str>, Output<'expr>), // Generic type declaration
    GenericCall(&'expr str, Vec<Output<'expr>>), // Generic type call
    GenericFunctionCall {
        name: Output<'expr>,
        type_args: Vec<Output<'expr>>,
        args: Option<Vec<Output<'expr>>>,
    }, // Generic function call with type arguments: foo::<i32>(42)
    InterfaceDecl(&'expr str, Vec<Output<'expr>>), // Interface definition
    StructDecl(&'expr str, Vec<Output<'expr>>),  // Struct definition
    ImplTrait(Output<'expr>, Output<'expr>),     // Trait implementation
    MatchArm(Output<'expr>, Output<'expr>),      // Match arm with pattern
    TypePattern(Vec<Expression<'expr>>),         // Pattern matching type
    FieldPattern(&'expr str, Option<Output<'expr>>), // Field pattern
    TypeAlias(&'expr str, Output<'expr>),        // Type alias
    NewType(&'expr str, Output<'expr>),          // New type declaration

    Variable(&'expr str, Option<Output<'expr>>),
    Constant(Output<'expr>, Option<Output<'expr>>),

    TypedAssignment {
        name: &'expr str,
        ty: Output<'expr>,
        value: Output<'expr>,
    }, // Typed assignment: let x: int = value

    Implementation(&'expr str, &'expr str, Vec<Output<'expr>>),
    Class(&'expr str, Vec<Output<'expr>>),
    Field(Output<'expr>, Output<'expr>),
    Method(bool, Output<'expr>),
    Member(Output<'expr>),
    Access(&'expr str),
    Update(&'expr str, Output<'expr>),

    Instantiate(Output<'expr>, Option<Vec<Output<'expr>>>),
}

impl<'a> Display for Expression<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Integer(n) => write!(f, "{}", n),
            Self::Float(n) => write!(f, "{:.?}", n),
            Self::Identifier(id) => write!(f, "{}", id),
            Self::Not(n) => write!(f, "!{}", n.1),
            Self::Flip(n) => write!(f, "~{}", n.1),
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
            Self::List(items) => {
                let items_str = items
                    .iter()
                    .map(|i| i.1.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "[{}]", items_str)
            }
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
                    returns
                        .clone()
                        .map_or(String::default(), |ret| format!(" -> {}", &ret.1)),
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
            Self::TypedAssignment { name, ty, value } => {
                write!(f, "{}: {} = {}", name, ty.1, value.1)
            }
            Self::Noop(n) => write!(f, "@{{ {} }}@", n.1.to_string()),
            Self::TypeVar(name, id) => write!(f, "T<{}:{}>", id, name),
            Self::SumType(name, type_params, variants) => {
                let names = variants
                    .iter()
                    .map(|v| v.1.to_string())
                    .collect::<Vec<_>>()
                    .join(" | ");
                if type_params.is_empty() {
                    write!(f, "enum<{}>", names)
                } else {
                    let params = type_params.join(", ");
                    write!(f, "enum{}<{}> {{{}}}", name.1, params, names)
                }
            }
            Self::Variant(name, fields) => {
                write!(
                    f,
                    "{}{}",
                    name.1,
                    fields
                        .clone()
                        .map(|f| f
                            .iter()
                            .map(|f| f.1.to_string())
                            .collect::<Vec<String>>()
                            .join(", "))
                        .unwrap_or(String::new())
                )
            }
            Self::VariantItem(ty, name) => write!(f, "{}::{}", ty.1, name.1),
            Self::VariantWithDestructure(ty, name, fields) => {
                let fields_str = fields
                    .iter()
                    .map(|f| f.1.to_string())
                    .collect::<Vec<String>>()
                    .join(", ");
                write!(
                    f,
                    "{}::{}{}",
                    ty.1,
                    name.1,
                    if fields_str.is_empty() {
                        String::new()
                    } else {
                        format!("({})", fields_str)
                    }
                )
            }
            Self::GenericDecl(params, body) => {
                let p = params
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "gen<{}> {{ {} }}", p, body.1)
            }
            Self::GenericCall(name, args) => {
                let a = args
                    .iter()
                    .map(|a| a.1.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "{}<{}>", name, a)
            }
            Self::GenericFunctionCall {
                name,
                type_args,
                args,
            } => {
                let type_str = type_args
                    .iter()
                    .map(|a| a.1.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                let args_str = args
                    .as_ref()
                    .map(|a| {
                        a.iter()
                            .map(|a| a.1.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                write!(f, "{}::<{}>({})", name.1, type_str, args_str)
            }
            Self::InterfaceDecl(name, _methods) => {
                write!(f, "interface {} {{ ... }}", name)
            }
            Self::StructDecl(name, _fields) => {
                write!(f, "struct {} {{ ... }}", name)
            }
            Self::ImplTrait(r#trait, r#type) => {
                write!(f, "impl {} for {}", r#trait.1, r#type.1)
            }
            Self::MatchArm((_, pattern), (_, body)) => {
                write!(f, "case {} => {}", pattern, body)
            }
            Self::TypePattern(_) => write!(f, "type pattern {{ ... }}"),
            Self::FieldPattern(name, _) => write!(f, "{}", name),
            Self::TypeAlias(name, target) => {
                write!(f, "type {} = {}", name, target.1)
            }
            Self::NewType(name, target) => {
                write!(f, "newtype {} = {}", name, target.1)
            }
            Self::Match(expr, arms) => {
                write!(f, "match {} {{", expr.1)?;
                for arm in arms {
                    write!(f, "\n  {}", arm.1)?;
                }
                write!(f, "\n}}")
            }
            Self::Type(t) => {
                write!(f, "{}", t.1)
            }
            Self::Argument(ty, name) => {
                write!(f, "{} {}", ty.1, name.1)
            }
            Self::Program(nodes) => {
                write!(
                    f,
                    "{}",
                    nodes
                        .iter()
                        .map(|n| n.1.to_string())
                        .collect::<Vec<String>>()
                        .join("\n")
                )
            }
            Self::Format(lhs, items) => {
                write!(
                    f,
                    "{}{}",
                    lhs.1,
                    items
                        .as_ref()
                        .map(|i| {
                            let mut r = i.iter().map(|i| i.1.to_string()).collect::<Vec<String>>();
                            r.push("".to_string());
                            r.join(", ")
                        })
                        .unwrap_or_default()
                )
            }
            Self::Bool(state) => {
                write!(f, "{}", if *state { "true" } else { "false" })
            }
            Self::Return(expr) => {
                write!(f, "return {}", expr.1)
            }
            Self::Comment(c) => {
                write!(f, "// {}", c)
            }
            Self::ImplicitReturn(r) => {
                write!(f, "{}", r.1)
            }
            Self::Leq(lhs, rhs) => {
                write!(f, "{} <= {}", lhs.1, rhs.1)
            }
            Self::Geq(lhs, rhs) => {
                write!(f, "{} >= {}", lhs.1, rhs.1)
            }
            e => {
                eprintln!("Missing rest of nodes: '{}'", e);
                todo!("Missing rest of nodes: {}", e)
            }
        }
    }
}
