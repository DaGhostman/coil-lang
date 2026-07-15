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

    /// Function argument: `(T name)`. The type `T` is
    /// represented as a full `Output<'expr>` (Phase 24) so
    /// aggregate types like `[int]` and `(int, string)` are
    /// preserved through the parser.
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

    /// `(a, b, c)` — heterogeneous product type literal.
    /// Each element may be a different type. Used in:
    ///   - source: tuple literals in expression position
    ///   - `declare(lib, name, (T1, T2), R)` — the arg-types
    ///     tuple (Phase 23 second point)
    ///   - `invoke(lib, fn_id, (a, b))` — packed args (Phase 23
    ///     third point)
    Tuple(Vec<Output<'expr>>),

    /// `{name: expr, name: expr, ...}` — anonymous record / dict
    /// literal (Phase 25). Structurally typed. Mutable via
    /// the same `Access` field-update path as classes. The
    /// runtime reuses `Object::Instance` for storage.
    Dict(Vec<RecordFieldValue<'expr>>),

    /// `t[i]` or `arr[i]` — element access. The index may be
    /// any expression evaluated to an integer at runtime.
    /// Out-of-bounds indices push `Value::from(-1i64)` (a
    /// sentinel; the typechecker doesn't catch this).
    Index(Output<'expr>, Output<'expr>),

    /// `dload(path)` — userland dynamic library load. Pops a
    /// string path, calls `dlopen`, and pushes the library
    /// handle (an `Object::Library` heap address disguised as
    /// an `int`). Subsequent `declare(...)` / `invoke(...)`
    /// calls take this handle as their first argument.
    Dload(Output<'expr>),

    /// `declare(lib, name, args_tuple, ret_type)` — userland
    /// runtime registration of an FFI signature.
    ///
    /// Phase 23 redesign: `args_tuple` is a single tuple
    /// expression whose elements are the parameter *types*
    /// (each an `FFIType::X` constructor application
    /// evaluated to its enum tag integer). The return type
    /// is the LAST argument, OUTSIDE the tuple. Example:
    ///
    /// ```ignore
    ///     declare(ffi,
    ///             "sum",
    ///             (FFIType::Int, FFIType::Int),
    ///             FFIType::Int)
    /// ```
    ///
    /// Returns a function ID (an `int`) the user passes to
    /// `invoke(...)`. The arity is the tuple's element count
    /// (`args_tuple.1.len()`); the runtime's `DeclareFFI`
    /// opcode reads the tag stack and emits one tag per
    /// tuple position.
    Declare(Vec<Output<'expr>>),

    /// `invoke(lib, fn_id, args_tuple)` — call the function
    /// previously registered by `declare`.
    ///
    /// Phase 23 redesign: the value args are passed as a
    /// single tuple (or array) expression. The VM unpacks
    /// the elements at dispatch time and passes each one to
    /// the C function in source order. Example:
    ///
    /// ```ignore
    ///     invoke(ffi, sum_id, (40, 2))
    /// ```
    ///
    /// The payload of the tuple is also acceptable via
    /// `let args = (40, 2); invoke(ffi, sum_id, args)`.
    Invoke(Vec<Output<'expr>>),

    Use {
        path: Vec<String>,
        name: String,
        alias: Option<String>,
    },

    /// `extern "libname" { fn name(args) -> ret; ... }` — declare
    /// external (FFI) functions loaded from a shared library.
    ///
    /// `library` is the name passed to `dlopen` (e.g. `"c"`,
    /// `"m"`, `"dl"`). The exact filename is resolved by the
    /// platform's dynamic linker (so `"c"` on Linux finds
    /// `libc.so.6` or `libc.so`).
    ///
    /// Each `declarations` entry is a function signature
    /// declared with the `fn name(args) -> ret;` syntax inside
    /// the extern block. The body of an extern function is
    /// `None` (extern functions have no Rust-side body — the
    /// VM resolves the symbol at startup).
    ExternBlock {
        library: String,
        declarations: Vec<ExternFunction<'expr>>,
    },

    Function {
        name: &'expr str,
        args: Output<'expr>,
        /// Phase 24 — return type is now a full type
        /// annotation (was `Option<&'expr str>` in earlier
        /// phases). The typechecker still accepts the
        /// pre-Phase-24 plain-identifier form via the
        /// `Expression::Type(name)` node; array and
        /// tuple type annotations are new.
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

    // ---- Phase 15A: sum types and pattern matching ----
    // ---- Phase 17B: record-shaped variant payloads ----
    // ---- Phase 28: type aliases ----
    /// `type Name = T;` — declares a type alias. Phase 28
    /// additive only; no runtime effect (the alias is
    /// substituted at parse / typecheck time).
    TypeAlias {
        name: &'expr str,
        ty: Box<Output<'expr>>,
    },

    /// `enum Name { Variant1, Variant2(T1, T2), Variant3 { x: T, y: T }, ... }`
    /// — a top-level sum-type declaration. Carries no executable body;
    /// the declaration is registered with the typechecker in 15B and
    /// the code emitter in 15C. In 15A, parsing produces this node so
    /// the AST is stable, but downstream passes stub it out (see
    /// `infer.rs` and `lib.rs`).
    EnumDecl {
        name: &'expr str,
        variants: Vec<Output<'expr>>,
    },
    /// One variant inside an `EnumDecl` payload list. The payload
    /// shape is recorded explicitly as `Unit`, `Tuple`, or `Record` —
    /// the AST carries the shape so neither the typechecker nor the
    /// codegen needs to guess. Phase 17B added `Record` support; the
    /// existing `Vec<Output>` tuple shape was retrofitted into the
    /// `Tuple` variant of `EnumVariantPayload`.
    EnumVariant {
        name: &'expr str,
        payload: EnumVariantPayload<'expr>,
    },
    /// `EnumName::Variant(args...)` — a *qualified* constructor
    /// application. The enum name is required (bare `Variant(...)` is
    /// parsed as a `Call`, not a `Construct`).
    Construct {
        enum_name: &'expr str,
        variant_name: &'expr str,
        fields: EnumConstructPayload<'expr>,
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

/// The payload shape of a single `EnumDecl` variant. Tuple and Record
/// are distinct shapes (they are NOT distinguished by synthetic names
/// in the AST — see 17B's red-team finding #1). The typechecker
/// rejects shape mismatches between declaration and call site /
/// pattern.
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

/// One record field in an `EnumDecl` payload. The `name` is the
/// field's label and `value` is the type expression (wrapped in
/// `Expression::Type` in practice). Mirrors `RecordFieldValue`
/// but uses type expressions.
#[derive(Clone, PartialEq, Debug)]
pub struct RecordFieldDecl<'expr> {
    pub name: &'expr str,
    pub value: Output<'expr>,
}

/// One function declaration inside an
/// [`Expression::ExternBlock`]. The body is always `None` (FFI
/// functions have no Rust-side implementation — the symbol is
/// resolved by the VM's dynamic linker at startup).
#[derive(Clone, PartialEq, Debug)]
pub struct ExternFunction<'expr> {
    pub name: &'expr str,
    pub args: Output<'expr>,
    pub returns: Option<&'expr str>,
}

/// One record field in an `EnumConstructPayload::Record`. The
/// `name` is the field's label and `value` is the expression that
/// produces the field's runtime value. Mirrors `RecordFieldDecl`
/// but uses runtime expressions.
#[derive(Clone, PartialEq, Debug)]
pub struct RecordFieldValue<'expr> {
    pub name: &'expr str,
    pub value: Output<'expr>,
}

/// One field in a record pattern. `pattern` is a `Pattern`; the
/// shorthand `x` desugars at parse time to
/// `PatternField { name: "x", pattern: Pattern::Binding { name: "x" } }`
/// (see the `pattern` parser).
#[derive(Clone, PartialEq, Debug)]
pub struct PatternField<'expr> {
    pub name: &'expr str,
    pub pattern: Pattern<'expr>,
}

/// The payload shape of an `Expression::Construct` (constructor
/// application). Same shape taxonomy as `EnumVariantPayload`.
#[derive(Clone, PartialEq, Debug)]
pub enum EnumConstructPayload<'expr> {
    /// `Foo` (no fields — bare qualified enum value).
    Unit,
    /// `Foo(arg1, arg2, ...)`.
    Tuple(Vec<Output<'expr>>),
    /// `Foo { name: expr, ... }` — fields may be supplied in any
    /// order; the codegen reorders them to declaration order before
    /// emitting `MAKE_ENUM`.
    Record(Vec<RecordFieldValue<'expr>>),
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
/// - `Constructor` matches a specific enum variant. The payload is
///   `PatternPayload`, with the same Unit/Tuple/Record shape
///   taxonomy as `EnumVariantPayload`. Nested patterns enable
///   tuple destructuring; record patterns (`Foo { x, y }`) bind
///   by name in declaration order.
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

/// The payload shape of a `Pattern::Constructor`. Same shape
/// taxonomy as `EnumVariantPayload`.
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
                // `returns` is now `Option<Output>`. Print the
                // inner expression via Display.
                let ret_str = returns
                    .as_ref()
                    .map(|ret| format!(" -> {}", ret.1))
                    .unwrap_or_default();
                // Emit the function body inline. The `name`,
                // `args.1`, `ret_str`, and `body.1` substitutions
                // happen via the runtime format machinery; the
                // surrounding `{` / `}` are literal (escaped via
                // `{{` / `}}` so the format parser doesn't
                // interpret them as `{}` placeholders).
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
            // Dict literal (Phase 25) — `{ name: expr, ... }`.
            // Renders the record as a constructor-shaped string
            // used for round-trip Display tests.
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
            // Field access: `receiver.field` — printed tight (no
            // surrounding whitespace) to match how it parses.
            Self::Access(receiver, field) => {
                write!(f, "{}.{}", receiver.1, field)
            }
            // The following arms are exhaustive but the
            // Display impl previously used `todo!()` for
            // any other variant. We add specific arms for
            // everything we know about; an unknown variant
            // renders as `<unhandled: ...>` so a Display
            // test failure is loud (rather than aborting
            // the whole test process via `todo!`).
            Self::Type(n) => write!(f, "{}", n),
            e => write!(f, "<unhandled: {:?}>", e),
        }
    }
}
