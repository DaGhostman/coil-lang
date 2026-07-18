use std::{borrow::Borrow, fmt::Display};

use chumsky::span::SimpleSpan;
pub type Output<'parser> = (SimpleSpan, Box<Expression<'parser>>);

/// Kind of a type parameter.
///
/// - [`Kind::Type`] (`*`) — ordinary type parameter (`T`, `A`)
/// - [`Kind::Constraint`] — typeclass predicates
/// - [`Kind::Arrow`] — type constructor arrows such as `* -> * -> *`
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum Kind {
    /// `*` — a proper type.
    #[default]
    Type,
    /// `Constraint` — a typeclass predicate.
    Constraint,
    /// `domain -> codomain` — a type constructor kind.
    Arrow(Box<Kind>, Box<Kind>),
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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

/// A generic type parameter with optional bounds and/or an explicit kind.
///
/// `T` → `TypeParam { name: "T", bounds: [], kind: Type }`
/// `T: Num + Eq` → `TypeParam { name: "T", bounds: ["Num", "Eq"], kind: Type }`
/// `F: * -> *` → `TypeParam { name: "F", bounds: [], kind: Arrow(Type, Type) }`
///
/// After `:`, a parameter takes class bounds (`Num + Eq`), a kind annotation
/// (`* -> *`), or a kind annotation followed by class bounds
/// (`F: * -> *, Container`).
#[derive(Clone, PartialEq, Debug)]
pub struct TypeParam<'expr> {
    pub name: &'expr str,
    /// Bound class names, e.g. `["Num", "Eq"]` for `T: Num + Eq`.
    pub bounds: Vec<&'expr str>,
    /// Explicit kind; defaults to [`Kind::Type`].
    pub kind: Kind,
}

/// A `where` clause constraint: `Convert<A, B>` or unary `Num<T>`.
#[derive(Clone, PartialEq, Debug)]
pub struct WhereConstraint<'expr> {
    pub class: &'expr str,
    pub args: Vec<Output<'expr>>,
}

impl<'a> Display for WhereConstraint<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.class)?;
        if !self.args.is_empty() {
            write!(f, "<")?;
            for (i, arg) in self.args.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{}", arg.1)?;
            }
            write!(f, ">")?;
        }
        Ok(())
    }
}

/// Compound assignment operator (`+=`, `-=`, …).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AssignOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Shl,
    Shr,
    BitAnd,
    BitOr,
    BitXor,
}

/// Prefix/postfix increment or decrement.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AdjustOp {
    Inc,
    Dec,
}

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
    /// Generic type application in annotations: `Option<int>`, `Result<int, string>`.
    TypeApp {
        name: &'expr str,
        args: Vec<Output<'expr>>,
    },
    /// Associated-type projection in annotations: `Collect::Elem`, `C::Elem`.
    TypeProjection {
        owner: &'expr str,
        name: &'expr str,
        args: Vec<Output<'expr>>,
    },
    /// Function type annotation `A -> B`.
    TypeFun(Output<'expr>, Output<'expr>),
    Comment(&'expr str),
    Print(Output<'expr>, Option<Vec<Output<'expr>>>),
    Format(Output<'expr>, Option<Vec<Output<'expr>>>),
    Return(Output<'expr>),
    ImplicitReturn(Output<'expr>),
    /// `raise expr` — early-return `Err(expr)` from a Result-mode function.
    Raise(Output<'expr>),
    Yield(Output<'expr>),
    YieldFrom(Output<'expr>),
    Resume(Output<'expr>, Option<Output<'expr>>),
    /// Postfix `expr?` — propagate `Err`/`None` from Result/Option.
    Try(Output<'expr>),
    /// `lhs ?? rhs` — coalesce: Some/Ok unwrap, None/Err → rhs.
    Coalesce(Output<'expr>, Output<'expr>),
    /// `expr?.field` — optional field access on `Option`.
    OptionalAccess(Output<'expr>, &'expr str),
    Negate(Output<'expr>),
    Not(Output<'expr>),
    LogicalNot(Output<'expr>),
    Positive(Output<'expr>),
    Default(&'expr str),
    /// `target += rhs` and related compound assignments.
    CompoundAssign(Output<'expr>, AssignOp, Output<'expr>),
    /// Prefix/postfix `++` / `--`.
    Adjust {
        op: AdjustOp,
        prefix: bool,
        target: Output<'expr>,
    },
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

    /// Coroutine completion check: `done(handle)`.
    Done(Output<'expr>),

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
        is_coro: bool,
        type_params: Vec<TypeParam<'expr>>,
        args: Output<'expr>,
        returns: Option<Output<'expr>>,
        /// Constraints from a trailing `where` clause (after returns).
        where_constraints: Vec<WhereConstraint<'expr>>,
        body: Output<'expr>,
    },

    Branch(Option<Output<'expr>>, Output<'expr>),

    If(Vec<Output<'expr>>),

    Call {
        name: Output<'expr>,
        args: Option<Vec<Output<'expr>>>,
    },

    Break,
    Continue,
    For {
        init: Option<Output<'expr>>,
        cond: Output<'expr>,
        step: Option<Output<'expr>>,
        body: Output<'expr>,
    },

    Loop {
        identifier: Option<Output<'expr>>,
        iterable: Output<'expr>,
        body: Output<'expr>,
    },

    Variable(&'expr str, Option<Output<'expr>>),
    Constant(Output<'expr>, Option<Output<'expr>>),

    Class {
        name: &'expr str,
        type_params: Vec<TypeParam<'expr>>,
        fields: Vec<Output<'expr>>,
    },
    Implementation {
        /// Unused trait slot (`""` for inherent impls).
        what: &'expr str,
        owner: &'expr str,
        type_params: Vec<TypeParam<'expr>>,
        methods: Vec<Output<'expr>>,
    },
    Field(Visibility, Output<'expr>, Output<'expr>),
    Method(Visibility, Output<'expr>),
    Member(Output<'expr>),
    Access(Output<'expr>, &'expr str),

    Instantiate(Output<'expr>, Option<Vec<Output<'expr>>>),

    /// `type Name = T;` — type alias declaration.
    TypeAlias {
        name: &'expr str,
        type_params: Vec<TypeParam<'expr>>,
        ty: Box<Output<'expr>>,
    },

    /// Top-level `enum` declaration.
    EnumDecl {
        name: &'expr str,
        type_params: Vec<TypeParam<'expr>>,
        variants: Vec<Output<'expr>>,
    },

    /// `extern struct Name { field: type, ... };` — C-layout FFI struct.
    ExternStruct(ExternStructDecl<'expr>),

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

    /// `forall T. T` / `forall T: Num + Eq, U. (T, U)` in type annotations.
    Forall {
        params: Vec<TypeParam<'expr>>,
        ty: Box<Output<'expr>>,
    },

    /// `trait Name<T> { type Elem; fn ...; fn ... { default } }`
    TypeClass {
        name: &'expr str,
        type_params: Vec<TypeParam<'expr>>,
        /// Body items: `AssocTypeDecl`, and `Function` nodes (empty `Block` = sig-only).
        methods: Vec<Output<'expr>>,
    },

    /// `impl Num<int> { … }` or `impl Show for Point { … }` — trait instance.
    ///
    /// For the `impl Trait<A, B> for T` form, `args` is `[T, A, B]` (Self first).
    TypeClassImpl {
        class: &'expr str,
        /// Type annotations for the class type arguments, e.g. `[int]`.
        args: Vec<Output<'expr>>,
        /// Body items: `AssocTypeDef` and method `Function`/`Method` nodes.
        methods: Vec<Output<'expr>>,
    },

    /// Associated type declaration inside a trait: `type Elem;` or `type Ref<T>;`.
    AssocTypeDecl {
        name: &'expr str,
        type_params: Vec<TypeParam<'expr>>,
    },

    /// Associated type definition inside an impl: `type Elem = int;` or `type Ref<T> = T;`.
    AssocTypeDef {
        name: &'expr str,
        type_params: Vec<TypeParam<'expr>>,
        ty: Box<Output<'expr>>,
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
    pub returns: Option<Output<'expr>>,
}

/// C-layout struct for FFI: `extern struct Name { field: type, ... }`.
#[derive(Clone, PartialEq, Debug)]
pub struct ExternStructDecl<'expr> {
    pub name: &'expr str,
    pub fields: Vec<(String, Output<'expr>)>,
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

/// Format a `Vec<TypeParam>` as `<T, U: Num + Eq, F: * -> *>`.
fn fmt_type_params(params: &[TypeParam<'_>]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let inner: Vec<String> = params.iter().map(|p| p.to_string()).collect();
    format!("<{}>", inner.join(", "))
}

impl<'a> Display for TypeParam<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.kind != Kind::Type {
            write!(f, "{}: {}", self.name, self.kind)?;
            if !self.bounds.is_empty() {
                write!(f, ", {}", self.bounds.join(" + "))?;
            }
            Ok(())
        } else if !self.bounds.is_empty() {
            write!(f, "{}: {}", self.name, self.bounds.join(" + "))
        } else {
            write!(f, "{}", self.name)
        }
    }
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
            Self::Bool(b) => write!(f, "{}", if *b { "true" } else { "false" }),
            Self::Identifier(id) => write!(f, "{}", id),
            Self::Not(n) => write!(f, "~{}", n.1),
            Self::LogicalNot(n) => write!(f, "!{}", n.1),
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
            Self::CompoundAssign(lhs, op, rhs) => {
                let sym = match op {
                    AssignOp::Add => "+=",
                    AssignOp::Sub => "-=",
                    AssignOp::Mul => "*=",
                    AssignOp::Div => "/=",
                    AssignOp::Mod => "%=",
                    AssignOp::Pow => "**=",
                    AssignOp::Shl => "<<=",
                    AssignOp::Shr => ">>=",
                    AssignOp::BitAnd => "&=",
                    AssignOp::BitOr => "|=",
                    AssignOp::BitXor => "^=",
                };
                write!(f, "{} {} {}", lhs.borrow().1, sym, rhs.borrow().1)
            }
            Self::Adjust { op, prefix, target } => {
                let sym = match op {
                    AdjustOp::Inc => "++",
                    AdjustOp::Dec => "--",
                };
                if *prefix {
                    write!(f, "{}{}", sym, target.borrow().1)
                } else {
                    write!(f, "{}{}", target.borrow().1, sym)
                }
            }
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
            Self::Format(fmt, params) => write!(
                f,
                "format {}{}",
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
            Self::Done(handle) => write!(f, "done({})", handle.1),
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
                is_coro,
                type_params,
                args,
                returns,
                where_constraints,
                body,
            } => {
                let async_kw = if *is_coro { "async " } else { "" };
                let tp = if type_params.is_empty() {
                    String::new()
                } else {
                    fmt_type_params(type_params)
                };
                let ret_str = returns
                    .as_ref()
                    .map(|ret| format!(" -> {}", ret.1))
                    .unwrap_or_default();
                let where_str = if where_constraints.is_empty() {
                    String::new()
                } else {
                    format!(
                        " where {}",
                        where_constraints
                            .iter()
                            .map(|c| c.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                write!(
                    f,
                    "{}fn {}{}({}){}{} {{\n{}}}",
                    async_kw, name, tp, args.1, ret_str, where_str, body.1
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
            Self::Break => write!(f, "break"),
            Self::Continue => write!(f, "continue"),
            Self::For {
                init,
                cond,
                step,
                body,
            } => {
                let init = init.as_ref().map(|i| i.1.to_string()).unwrap_or_default();
                let step = step.as_ref().map(|s| s.1.to_string()).unwrap_or_default();
                write!(f, "for ({}; {}; {}) {{\n{}}}", init, cond.1, step, body.1)
            }
            Self::Assignment(n, e) => {
                write!(f, "{} = {}", n.1, e.1)
            }
            Self::Noop(n) => write!(f, "@{{ {} }}@", n.1),
            Self::TypeAlias {
                name,
                type_params,
                ty,
            } => {
                if type_params.is_empty() {
                    write!(f, "type {} = {};", name, ty.1)
                } else {
                    write!(
                        f,
                        "type {}{} = {};",
                        name,
                        fmt_type_params(type_params),
                        ty.1
                    )
                }
            }
            Self::Dict(items) => {
                let parts: Vec<String> = items
                    .iter()
                    .map(|f| format!("{}: {}", f.name, f.value.1))
                    .collect();
                write!(f, "{{ {} }}", parts.join(", "))
            }
            Self::EnumDecl {
                name,
                type_params,
                variants,
            } => {
                let tp = if type_params.is_empty() {
                    String::new()
                } else {
                    fmt_type_params(type_params)
                };
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
                write!(f, "enum {}{} {{ {} }}", name, tp, vs)
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
            Self::OptionalAccess(receiver, field) => {
                write!(f, "{}?.{}", receiver.1, field)
            }
            Self::Try(inner) => write!(f, "{}?", inner.1),
            Self::Coalesce(lhs, rhs) => write!(f, "{} ?? {}", lhs.1, rhs.1),
            Self::Raise(inner) => write!(f, "raise {}", inner.1),
            Self::Yield(inner) => write!(f, "yield {}", inner.1),
            Self::YieldFrom(inner) => write!(f, "yield from {}", inner.1),
            Self::Resume(target, None) => write!(f, "resume {}", target.1),
            Self::Resume(target, Some(arg)) => write!(f, "resume {} with {}", target.1, arg.1),
            Self::Type(n) => write!(f, "{}", n),
            Self::TypeApp { name, args } => {
                let args_s = args
                    .iter()
                    .map(|a| a.1.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "{}<{}>", name, args_s)
            }
            Self::TypeProjection { owner, name, args } => {
                if args.is_empty() {
                    write!(f, "{}::{}", owner, name)
                } else {
                    let args_s = args
                        .iter()
                        .map(|a| a.1.to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    write!(f, "{}::{}<{}>", owner, name, args_s)
                }
            }
            Self::TypeFun(arg, ret) => write!(f, "{} -> {}", arg.1, ret.1),
            Self::AssocTypeDecl { name, type_params } => {
                write!(f, "type {}{};", name, fmt_type_params(type_params))
            }
            Self::AssocTypeDef {
                name,
                type_params,
                ty,
            } => write!(f, "type {}{} = {};", name, fmt_type_params(type_params), ty.1),
            Self::Class {
                name,
                type_params,
                fields,
            } => {
                let tp = if type_params.is_empty() {
                    String::new()
                } else {
                    fmt_type_params(type_params)
                };
                let fs: Vec<String> = fields.iter().map(|f| f.1.to_string()).collect();
                write!(f, "class {}{} {{ {} }}", name, tp, fs.join(", "))
            }
            Self::Implementation {
                what,
                owner,
                type_params,
                methods,
            } => {
                let tp = if type_params.is_empty() {
                    String::new()
                } else {
                    fmt_type_params(type_params)
                };
                let ms: Vec<String> = methods.iter().map(|m| m.1.to_string()).collect();
                if what.is_empty() {
                    write!(f, "impl {}{} {{ {} }}", owner, tp, ms.join(" "))
                } else {
                    write!(
                        f,
                        "impl {} for {}{} {{ {} }}",
                        what,
                        owner,
                        tp,
                        ms.join(" ")
                    )
                }
            }
            Self::Forall { params, ty } => {
                write!(
                    f,
                    "forall {}. {}",
                    params
                        .iter()
                        .map(|p| p.to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                    ty.1
                )
            }
            Self::TypeClass {
                name,
                type_params,
                methods,
            } => {
                let tp = if type_params.is_empty() {
                    String::new()
                } else {
                    fmt_type_params(type_params)
                };
                let ms: Vec<String> = methods.iter().map(|m| m.1.to_string()).collect();
                write!(f, "trait {}{} {{ {} }}", name, tp, ms.join(" "))
            }
            Self::TypeClassImpl {
                class,
                args,
                methods,
            } => {
                // Prefer `impl Trait for T` / `impl Trait<A, B> for T` when
                // there is at least one type argument (Self-first convention).
                let ms: Vec<String> = methods.iter().map(|m| m.1.to_string()).collect();
                if let Some((for_ty, rest)) = args.split_first() {
                    let for_s = for_ty.1.to_string();
                    if rest.is_empty() {
                        write!(f, "impl {} for {} {{ {} }}", class, for_s, ms.join(" "))
                    } else {
                        let rest_s: Vec<String> = rest.iter().map(|a| a.1.to_string()).collect();
                        write!(
                            f,
                            "impl {}<{}> for {} {{ {} }}",
                            class,
                            rest_s.join(", "),
                            for_s,
                            ms.join(" ")
                        )
                    }
                } else {
                    write!(f, "impl {} {{ {} }}", class, ms.join(" "))
                }
            }
            e => write!(f, "<unhandled: {:?}>", e),
        }
    }
}
