use ast::{
    EnumConstructPayload, EnumVariantPayload, Expression, MatchArm, Output, Pattern,
    PatternField, PatternPayload, RecordFieldDecl, RecordFieldValue, Visibility,
};
use std::{
    marker::PhantomData,
    num::{ParseFloatError, ParseIntError},
};

pub use chumsky::span::SimpleSpan;
use chumsky::{
    IterParser, Parser,
    error::Rich,
    extra,
    pratt::{infix, left, postfix, prefix, right},
    prelude::{choice, just, none_of, recursive},
    text,
};
use common::{Label, Message};

#[repr(u16)]
enum Precedence {
    None = 0,
    Assign,
    Or,
    Xor,
    And,
    Equal,
    Compare,
    Binary,
    Term,
    Factor,
    Negate,
    Unary,
    Call,
    Primary,
}

macro_rules! op {
    ($operator: literal) => {
        just($operator).padded()
    };
}

macro_rules! keyword {
    ($word: literal) => {
        text::keyword($word).padded()
    };
}

macro_rules! output {
    ($kind: tt) => {
        |v, e| (e.span(), Box::new(Expression::$kind(v)))
    };
    ($kind: tt) => {
        |(lhs, rhs), e| (e.span(), Box::new(Expression::$kind(lhs, rhs)))
    };
}

pub mod ast;

#[derive(Default)]
pub struct Pratt<'pratt> {
    prefix: String,
    _data: PhantomData<&'pratt ()>,
}

impl<'pratt> Pratt<'pratt> {
    fn int(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        text::int(10)
            .to_slice()
            .from_str()
            .validate(|v: Result<i64, ParseIntError>, e, emitter| match v {
                Ok(value) => value,
                Err(msg) => {
                    emitter.emit(Rich::custom(e.span(), msg.to_string()));

                    0_i64
                }
            })
            .labelled("integer")
            .map_with(output!(Integer))
    }

    fn float(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        text::int(10)
            .then(just(".").then(text::int(10)))
            .to_slice()
            .from_str()
            .validate(|v: Result<f64, ParseFloatError>, e, emitter| match v {
                Ok(value) => value,
                Err(msg) => {
                    emitter.emit(Rich::custom(e.span(), msg.to_string()));

                    0_f64
                }
            })
            .labelled("float")
            .map_with(output!(Float))
    }

    fn string(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        just('"')
            .ignore_then(none_of('"').repeated().to_slice())
            .then_ignore(just('"'))
            .map_with(output!(String))
            .labelled("string")
    }
    fn ident(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        text::ident().padded().map_with(output!(Identifier))
    }

    fn expr(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        // Note: `case` is listed in the Phase 15A plan as a reserved
        // keyword for 15D polish, but it is not actively parsed in
        // 15A. Registering it would require either including a
        // no-op `keyword!("case")` in a `choice` (changing the
        // output type) or a typed `text::keyword::<...>` call that
        // leaks chumsky internals. Defer the registration to 15D
        // when the keyword gains a real parser.

        recursive(|expr| {
            let atom = choice((
                // `match` is a keyword atom — registered before
                // `self.ident()` so the identifier parser refuses
                // to match it.
                self.match_expr(expr.clone()),
                // `EnumName::Variant(args)` — qualified constructor
                // application. MUST be tried before `self.call(...)`
                // because both start with `ident`: the backtracking
                // inside `choice` will try the next alternative if
                // the `::` after the first ident is missing.
                self.construct(expr.clone()),
                self.call(expr.clone()),
                self.instantiate(expr.clone()),
                // float comes before int so that `1.0` is parsed as a
                // float, not an `int` `1` followed by a stray `.0`.
                self.float(),
                self.int(),
                self.string(),
                // Keyword atoms come before self.ident() so they're
                // registered in chumsky's KEYWORDS set before the
                // identifier parser is built (which then refuses to
                // match them).
                keyword!("true")
                    .map_with(|state, e| (e.span(), Box::new(Expression::Bool(state == "true"))))
                    .labelled("boolean"),
                keyword!("false")
                    .map_with(|state, e| (e.span(), Box::new(Expression::Bool(state == "true"))))
                    .labelled("boolean"),
                keyword!("new")
                    .ignore_then(text::ident())
                    .map_with(|class, e| {
                        let class_output = (e.span(), Box::new(Expression::Identifier(class)));
                        (
                            e.span(),
                            Box::new(Expression::Instantiate(class_output, None)),
                        )
                    })
                    .labelled("new"),
                self.ident(),
            ));

            choice((atom, self.group(expr.clone()))).pratt((
                // No postfix `!` here — it would conflict with `!=`
                // (which should be parsed as a single infix operator).
                // Bitwise/logical negation is the prefix `~` operator,
                // listed further down.
                infix(
                    right(Precedence::Binary as u16),
                    op!("<<"),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::Shl(lhs, rhs))),
                ),
                infix(
                    right(Precedence::Binary as u16),
                    op!(">>"),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::Shr(lhs, rhs))),
                ),
                infix(
                    right(Precedence::Binary as u16),
                    op!('&'),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::BitAnd(lhs, rhs))),
                ),
                infix(
                    right(Precedence::And as u16),
                    op!("&&"),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::And(lhs, rhs))),
                ),
                infix(
                    right(Precedence::Binary as u16),
                    op!('|'),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::BitOr(lhs, rhs))),
                ),
                infix(right(Precedence::Or as u16), op!("||"), |lhs, _, rhs, e| {
                    (e.span(), Box::new(Expression::Or(lhs, rhs)))
                }),
                infix(
                    right(Precedence::Binary as u16),
                    op!('^'),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::Xor(lhs, rhs))),
                ),
                infix(
                    right(Precedence::Factor as u16),
                    choice((op!("**"), op!("*"), op!("/"), op!("%"))),
                    |lhs, op, rhs, e| {
                        (
                            e.span(),
                            Box::new(match op {
                                "**" => Expression::Pow(lhs, rhs),
                                "*" => Expression::Mul(lhs, rhs),
                                "/" => Expression::Div(lhs, rhs),
                                "%" => Expression::Mod(lhs, rhs),
                                _ => unreachable!("No other operators"),
                            }),
                        )
                    },
                ),
                infix(
                    right(Precedence::Compare as u16),
                    // Multi-character operators come first so that
                    // `>=` is matched as `>=` rather than as `>`
                    // followed by `=`.
                    choice((
                        op!("=="),
                        op!("!="),
                        op!(">="),
                        op!("<="),
                        op!(">"),
                        op!("<"),
                    )),
                    |lhs, op, rhs, e| {
                        (
                            e.span(),
                            Box::new(match op {
                                "==" => Expression::Eq(lhs, rhs),
                                "!=" => Expression::Neq(lhs, rhs),
                                ">" => Expression::Gt(lhs, rhs),
                                ">=" => Expression::Geq(lhs, rhs),
                                "<=" => Expression::Leq(lhs, rhs),
                                "<" => Expression::Le(lhs, rhs),
                                _ => unreachable!("No more comparison operators"),
                            }),
                        )
                    },
                ),
                // infix(
                //     right(Precedence::Compare as u16),
                //     op!("!="),
                //     |lhs, _, rhs, e| (e.span(), Box::new(Expression::Neq(lhs, rhs))),
                // ),
                // infix(
                //     right(Precedence::Compare as u16),
                //     op!('>'),
                //     |lhs, _, rhs, e| (e.span(), Box::new(Expression::Gt(lhs, rhs))),
                // ),
                // infix(
                //     right(Precedence::Compare as u16),
                //     op!(">="),
                //     |lhs, _, rhs, e| (e.span(), Box::new(Expression::Geq(lhs, rhs))),
                // ),
                // infix(
                //     right(Precedence::Compare as u16),
                //     op!('<'),
                //     |lhs, _, rhs, e| (e.span(), Box::new(Expression::Le(lhs, rhs))),
                // ),
                // infix(
                //     right(Precedence::Compare as u16),
                //     op!("<="),
                //     |lhs, _, rhs, e| (e.span(), Box::new(Expression::Leq(lhs, rhs))),
                // ),
                prefix(
                    Precedence::Negate as u16,
                    choice((op!('-'), op!('~'), op!('+'))),
                    |c, rhs, e| {
                        (
                            e.span(),
                            Box::new(match c {
                                '-' => Expression::Negate(rhs),
                                '+' => Expression::Positive(rhs),
                                '~' => Expression::Not(rhs),
                                _ => unreachable!("No other prefix operators"),
                            }),
                        )
                    },
                ),
                infix(
                    right(Precedence::Assign as u16),
                    op!("="),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::Assignment(lhs, rhs))),
                ),
                infix(left(Precedence::Term as u16), op!('-'), |lhs, _, rhs, e| {
                    (e.span(), Box::new(Expression::Sub(lhs, rhs)))
                }),
                infix(left(Precedence::Term as u16), op!('+'), |lhs, _, rhs, e| {
                    (e.span(), Box::new(Expression::Add(lhs, rhs)))
                }),
                // infix(
                //     right(Precedence::None as u16),
                //     op!('='),
                //     |lhs, _, rhs, e| (e.span(), Box::new(Expression::Assignment(lhs, rhs))),
                // ),
                postfix(
                    Precedence::Primary as u16,
                    choice((op!("++"), op!("--"))),
                    |lhs, op, e| {
                        (
                            e.span(),
                            Box::new(match op {
                                "++" => Expression::Inc(lhs),
                                "--" => Expression::Dec(lhs),
                                _ => unreachable!("no other inc/dec operators"),
                            }),
                        )
                    },
                ),
            ))
        })
        .map_with(output!(Expr))
    }

    fn inc(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        self.ident().then_ignore(just("++")).map_with(output!(Inc))
    }

    fn dec(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        self.ident().then_ignore(just("--")).map_with(output!(Inc))
    }

    fn group<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        expr.repeated()
            .at_least(0)
            .collect()
            .map_with(output!(Fragment))
            .delimited_by(op!('('), op!(')'))
            .map_with(output!(Group))
    }

    fn block<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        stmt: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        stmt.repeated()
            .at_least(0)
            .collect()
            .map_with(output!(Block))
            .delimited_by(op!('{'), op!('}'))
    }

    fn arg_list(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        let arg = text::ident()
            .padded()
            .then(text::ident().padded())
            .map_with(|(ty, name), e| (e.span(), Box::new(Expression::Argument(ty, name))));

        arg.separated_by(op!(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .map_with(output!(Fragment))
            .delimited_by(op!("("), op!(")"))
    }

    fn func<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        stmt: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("fn")
            .then(text::ident().padded())
            .then(self.arg_list())
            .then(op!("->").ignore_then(text::ident().padded()).or_not())
            .then(self.block(stmt))
            .map_with(|((((_, name), args), returns), body), e| {
                (
                    e.span(),
                    Box::new(Expression::Function {
                        name,
                        args,
                        returns,
                        body,
                    }),
                )
            })
    }

    fn defer<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        stmt: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("defer")
            .ignore_then(self.block(stmt))
            .map_with(output!(Defer))
    }

    fn while_<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        stmt: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("while")
            .ignore_then(self.expr())
            .then(self.block(stmt))
            .map_with(|(iterable, body), e| {
                (
                    e.span(),
                    Box::new(Expression::Loop {
                        identifier: None,
                        iterable,
                        body,
                    }),
                )
            })
    }

    fn if_<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        stmt: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("if")
            .ignore_then(self.expr())
            .then(self.block(stmt))
            .map_with(|(cond, body), e| {
                let branch: Output = (
                    e.span(),
                    Box::new(Expression::Branch(Some(cond), body)),
                );
                (e.span(), Box::new(Expression::If(vec![branch])))
            })
    }

    fn print(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("print")
            .labelled("print statement")
            .ignore_then(self.string())
            .then(
                self.expr()
                    .separated_by(op!(','))
                    .allow_leading()
                    .collect::<Vec<_>>()
                    .or_not(),
            )
            .map_with(|(fmt, params), e| (e.span(), Box::new(Expression::Print(fmt, params))))
    }
    fn return_(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("return")
            .labelled("return")
            .ignore_then(self.expr())
            .map_with(|result, e| (e.span(), Box::new(Expression::Return(result))))
    }

    fn comment(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        op!("//")
            .ignore_then(none_of('\n').repeated().to_slice().padded())
            .map_with(output!(Comment))
    }

    fn expr_statement(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        self.expr()
            .then_ignore(op!(';'))
            .map_with(output!(ExprStatement))
    }

    fn statement(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        recursive(|stmt| {
            choice((
                self.while_(stmt.clone()),
                self.if_(stmt.clone()),
                self.block(stmt.clone()),
                self.variable().then_ignore(op!(';')),
                self.expr_statement(),
                self.print().then_ignore(op!(';')),
                self.return_().then_ignore(op!(';')),
                self.comment(),
            ))
        })
        .map_with(output!(Statement))
    }

    fn declaration(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        let stmt = self.statement();

        // `enum_decl` is registered before `variable()` (which lives
        // inside `stmt`) so a leading `enum` keyword is not
        // mis-parsed as `let`.
        choice((
            self.class(),
            self.impl_block(stmt.clone()),
            self.func(stmt.clone()),
            self.enum_decl(),
            self.defer(stmt.clone()),
            stmt.clone(),
        ))
    }

    /// `class Name { [pub] field: Type, ... }`
    ///
    /// Fields are private by default; `pub` makes them public.
    fn class(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("class")
            .ignore_then(text::ident())
            .then(
                self.field_decl()
                    .separated_by(op!(','))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(op!("{"), op!("}")),
            )
            .map_with(|(name, fields), e| (e.span(), Box::new(Expression::Class(name, fields))))
    }

    /// `[pub] name: Type` — a class field declaration.
    fn field_decl(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("pub")
            .or_not()
            .then(text::ident())
            .then_ignore(op!(":"))
            .then(text::ident().map_with(output!(Type)))
            .map_with(|((vis, name), ty), e| {
                let visibility = if vis.is_some() {
                    Visibility::Public
                } else {
                    Visibility::Private
                };
                let name_output: Output = (e.span(), Box::new(Expression::Identifier(name)));
                (
                    e.span(),
                    Box::new(Expression::Field(visibility, name_output, ty)),
                )
            })
    }

    /// `impl Owner { [pub] fn ... , ... }`
    ///
    /// Methods are private by default; `pub` makes them public.
    /// `what` (the trait name) is not yet used — implementations are
    /// always for the owner class.
    fn impl_block<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        stmt: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("impl")
            .ignore_then(text::ident())
            .then(
                self.method_decl(stmt)
                    .separated_by(op!(','))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(op!("{"), op!("}")),
            )
            .map_with(|(owner, methods), e| {
                // Empty `what` — trait implementations are not yet supported.
                (
                    e.span(),
                    Box::new(Expression::Implementation("", owner, methods)),
                )
            })
    }

    /// `[pub] fn name(...) -> ret { body }` — a method declaration
    /// inside an `impl` block.
    fn method_decl<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        stmt: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("pub")
            .or_not()
            .then(self.func(stmt))
            .map_with(|(vis, func), e| {
                let visibility = if vis.is_some() {
                    Visibility::Public
                } else {
                    Visibility::Private
                };
                (e.span(), Box::new(Expression::Method(visibility, func)))
            })
    }

    /// `new ClassName(args)` — instantiation.
    ///
    /// Constructed as an atom-style parser so it can be embedded in
    /// expressions. Returns `(ClassName, args)` via `Instantiate`.
    fn instantiate<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("new")
            .ignore_then(text::ident())
            .then(self.params(expr))
            .map_with(|(class, args), e| {
                let class_output = (e.span(), Box::new(Expression::Identifier(class)));
                (
                    e.span(),
                    Box::new(Expression::Instantiate(class_output, args)),
                )
            })
    }

    fn variable(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("let")
            .ignore_then(text::ident())
            .then(op!(":").ignore_then(self.ident()).or_not())
            .then(op!("=").ignore_then(self.expr()).or_not())
            .map_with(|((name, ty), val), e| {
                let mut result = vec![(e.span(), Box::new(Expression::Variable(name, ty)))];
                if let Some(v) = val {
                    result.push(v);
                }
                (e.span(), Box::new(Expression::Fragment(result)))
            })
    }

    fn params<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<
        'pratt,
        &'pratt str,
        Option<Vec<Output<'pratt>>>,
        extra::Err<Rich<'pratt, char>>,
    > + Clone
    + 'pratt {
        expr.clone()
            .separated_by(op!(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .or_not()
            .delimited_by(op!('('), op!(')'))
    }

    fn call<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        self.ident()
            .then(self.params(expr))
            .map_with(|(name, args), e| (e.span(), Box::new(Expression::Call { name, args })))
    }

    // ============================================================
    //  Phase 15A: sum types, qualified constructors, match
    // ============================================================

    /// `EnumName::Variant(args)` — a *qualified* constructor
    /// application. The `::` is mandatory: a bare `Variant(args)` is
    /// parsed as a `Call` (and the typechecker will report
    /// 'Cannot find function' for bare constructors — users must
    /// qualify with `EnumName::VariantName`).
    ///
    /// Phase 17B: the constructor can also use record syntax
    /// `EnumName::Variant { name: expr, ... }`. Tuple syntax
    /// `EnumName::Variant(a, b, ...)` is unchanged. Both forms
    /// share the `::` prefix; the difference is the delimiter
    /// (`(` vs `{`).
    ///
    /// Tried before `call` in the atom choice so the `::` is matched
    /// before the `(`.
    fn construct<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        // Record field: `name : expr` — duplicate field names are
        // rejected by the parser (emit a chumsky error).
        let record_field = text::ident()
            .padded()
            .then_ignore(op!(":"))
            .then(expr.clone())
            .map_with(|(name, value), e| {
                (
                    e.span(),
                    RecordFieldValue {
                        name,
                        value,
                    },
                )
            })
            .labelled("record field");

        let record_payload = record_field
            .separated_by(op!(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(op!("{"), op!("}"))
            .map(|fields| {
                // Duplicate field names pass through to the
                // typechecker, which reports them with a
                // source-anchored "Duplicate field `x`" message.
                // The parser intentionally does NOT reject them
                // — emitting a chumsky error here from inside
                // `map_with` would require a borrowed emitter and
                // duplicate the diagnostic machinery.
                EnumConstructPayload::Record(
                    fields.into_iter().map(|(_, f)| f).collect(),
                )
            })
            .labelled("record payload");

        // Tuple payload: `(arg1, arg2, ...)` — `None` means Unit.
        // Empty parens `()` are also treated as Unit (so users can
        // write `Option::None()` instead of `Option::None`).
        let tuple_payload = self.params(expr.clone()).map(|opt| {
            match opt {
                Some(args) if args.is_empty() => EnumConstructPayload::Unit,
                Some(args) => EnumConstructPayload::Tuple(args),
                None => EnumConstructPayload::Unit,
            }
        });

        // Shape selector: tuple or record. Both are optional
        // (Unit is the default when nothing matches).
        let shape = choice((tuple_payload, record_payload)).or_not();

        text::ident()
            .padded()
            .then_ignore(just("::").padded())
            .then(text::ident().padded())
            .then(shape)
            .map_with(|((enum_name, variant_name), fields), e| {
                (
                    e.span(),
                    Box::new(Expression::Construct {
                        enum_name,
                        variant_name,
                        fields: fields.unwrap_or(EnumConstructPayload::Unit),
                    }),
                )
            })
    }

    /// `match scrutinee { pat => body, ... }` — a pattern-match
    /// expression. The body of each arm is a full `expr` (so it can
    /// be a block, a literal, another match, etc.). Patterns are
    /// parsed by [`Self::pattern`].
    ///
    /// Takes the recursive `expr` parser as a parameter (rather than
    /// calling `self.expr()`) so nested match expressions share the
    /// outer `recursive` group instead of spawning a fresh one on
    /// every call — which would overflow the stack at construction
    /// time.
    fn match_expr<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("match")
            .ignore_then(expr.clone())
            .then(
                self.arm(expr.clone())
                    .separated_by(op!(','))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(op!('{'), op!('}')),
            )
            .map_with(|(scrutinee, arms), e| {
                (
                    e.span(),
                    Box::new(Expression::Match { scrutinee, arms }),
                )
            })
    }

    /// `pattern => expr` — one arm inside a `match` block.
    ///
    /// Returns a [`MatchArm`] directly (not an `Output`) because
    /// patterns are not expressions and don't carry a span.
    fn arm<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'pratt, &'pratt str, MatchArm<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        self.pattern()
            .then_ignore(op!("=>"))
            .then(expr)
            .map_with(|(pattern, body), _| MatchArm { pattern, body })
    }

    /// A match-arm pattern: `_`/`default` (wildcard), `name` (binding),
    /// or `Enum::Variant(p1, p2, ...)` (constructor with nested
    /// patterns). Phase 17B: constructor patterns also accept record
    /// syntax `Enum::Variant { name (shorthand) or name: pattern, ... }`.
    /// Wrapped in `recursive` so constructor payloads can
    /// themselves contain nested patterns.
    fn pattern(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Pattern<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        recursive(|pattern_parser| {
            // Record-pattern field. Two shapes:
            //   - `name`            → shorthand: desugars to
            //                         `PatternField { name, pattern: Binding(name) }`
            //   - `name : pattern`  → explicit binding / sub-pattern.
            //
            // Duplicate field names are deduplicated silently (the
            // first wins). The typechecker reports the rest as
            // "extra field".
            let record_pattern_field = text::ident()
                .padded()
                .then(
                    op!(":").ignore_then(pattern_parser.clone()).or_not(),
                )
                .map_with(|(name, sub_pat), _| {
                    let pattern = match sub_pat {
                        Some(p) => p,
                        None => Pattern::Binding { name },
                    };
                    PatternField { name, pattern }
                })
                .labelled("record pattern field");

            let record_pattern_payload = record_pattern_field
                .separated_by(op!(','))
                .allow_trailing()
                .collect::<Vec<_>>()
                .delimited_by(op!("{"), op!("}"))
                .map(|fields| PatternPayload::Record(fields))
                .labelled("record pattern payload");

            // `Enum::Variant(p1, p2, ...)` — the first ident must be
            // followed by `::`, otherwise this alternative fails and
            // the choice falls through to the binding alternative.
            //
            // Shape selector: nothing (Unit), tuple `(p1, p2)`,
            // record `{ name, name: pat, ... }`. Empty parens `()`
            // are treated as Unit (so `Option::None()` is
            // equivalent to `Option::None`).
            let tuple_payload = pattern_parser
                .clone()
                .separated_by(op!(','))
                .allow_trailing()
                .collect::<Vec<_>>()
                .delimited_by(op!('('), op!(')'))
                .map(|parts| {
                    if parts.is_empty() {
                        PatternPayload::Unit
                    } else {
                        PatternPayload::Tuple(parts)
                    }
                });

            let payload_choice = tuple_payload
                .or(record_pattern_payload)
                .or_not()
                .map(|opt| opt.unwrap_or(PatternPayload::Unit));

            let constructor = text::ident()
                .padded()
                .then_ignore(just("::").padded())
                .then(text::ident().padded())
                .then(payload_choice)
                .map_with(|((enum_name, variant_name), payload), _| {
                    Pattern::Constructor {
                        enum_name,
                        variant_name,
                        payload,
                    }
                });

            choice((
                // `_` and `default` both parse to the same wildcard
                // node — the literal token is discarded (Decision C).
                just("_").padded().to(Pattern::Wildcard),
                keyword!("default").to(Pattern::Wildcard),
                constructor,
                // Bare identifier — binds the scrutinee. Tried after
                // `constructor` so a name followed by `::` is taken
                // as a constructor, not a binding.
                text::ident()
                    .padded()
                    .map_with(|name, _| Pattern::Binding { name }),
            ))
        })
    }

    /// `enum Name { Variant1, Variant2(T1, T2), ... }` — a top-level
    /// sum-type declaration. Registered in [`Self::declaration`]
    /// before `variable()` so a leading `enum` keyword isn't
    /// mis-parsed as `let`.
    fn enum_decl(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("enum")
            .ignore_then(text::ident().padded())
            .then(
                self.enum_variant()
                    .separated_by(op!(','))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(op!('{'), op!('}')),
            )
            .map_with(|(name, variants), e| {
                (
                    e.span(),
                    Box::new(Expression::EnumDecl { name, variants }),
                )
            })
    }

    /// `Variant`, `Variant(T1, T2, ...)`, or `Variant { x: T, y: T }`
    /// — one entry inside an `enum` body. The payload is a list of
    /// *type names* (not expressions) wrapped in `Expression::Type(...)`
    /// to match the `class` field syntax. Phase 17B adds record-shape
    /// declarations. Shape is recorded explicitly in the AST
    /// (`EnumVariantPayload`) so neither the typechecker nor the
    /// codegen needs to guess.
    fn enum_variant(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        // Record field: `name : Type` — types are bare identifiers
        // wrapped in `Expression::Type(...)`. Duplicate names are
        // rejected at parse time.
        let record_field_decl = text::ident()
            .padded()
            .then_ignore(op!(":"))
            .then(text::ident().padded().map_with(output!(Type)))
            .map_with(|(name, value), _| RecordFieldDecl { name, value })
            .labelled("record field declaration");

        let record_payload_decl = record_field_decl
            .separated_by(op!(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(op!("{"), op!("}"))
            .map(|fields| EnumVariantPayload::Record(fields))
            .labelled("record variant payload");

        let tuple_payload_decl = text::ident()
            .padded()
            .map_with(output!(Type))
            .separated_by(op!(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(op!('('), op!(')'))
            .map(EnumVariantPayload::Tuple);

        let payload_choice = tuple_payload_decl
            .or(record_payload_decl)
            .or_not()
            .map(|opt| opt.unwrap_or(EnumVariantPayload::Unit));

        text::ident()
            .padded()
            .then(payload_choice)
            .map_with(|(name, payload), e| {
                (
                    e.span(),
                    Box::new(Expression::EnumVariant { name, payload }),
                )
            })
    }

    pub fn parse(&self, input: &'pratt str) -> Result<Output<'pratt>, common::Message> {
        match self
            .declaration()
            .repeated()
            .collect()
            .map_with(output!(Program))
            .or(self.comment())
            .parse(input)
            .into_result()
        {
            Err(errs) => {
                let mut message =
                    Message::error("Parse error".to_string(), std::ops::Range::default());

                errs.iter().for_each(|err| {
                    message.push(Label::new(err.to_string(), err.span().into_range()));
                });

                Err(message)
            }
            Ok(ast) => Ok(ast),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ast::{
        EnumConstructPayload, EnumVariantPayload, Expression, MatchArm, Pattern, PatternPayload,
    };
    use crate::Pratt;
    use chumsky::Parser;

    macro_rules! expr {
        ($case: literal) => {
            Pratt::default()
                .expr()
                .parse($case)
                .into_result()
                .unwrap()
                .1
                .to_string()
        };
    }

    macro_rules! stmt {
        ($case: literal) => {
            Pratt::default()
                .declaration()
                .parse($case)
                .into_result()
                .unwrap()
                .1
                .to_string()
        };
    }

    /// Parse a top-level declaration, returning the inner
    /// `Expression` for structural assertions (no `Display` round-trip).
    macro_rules! decl_ast {
        ($case: literal) => {
            Pratt::default()
                .declaration()
                .parse($case)
                .into_result()
                .expect("parse failed")
                .1
                .as_ref()
                .clone()
        };
    }

    /// Parse an expression, returning the inner `Expression`.
    macro_rules! expr_ast {
        ($case: literal) => {
            Pratt::default()
                .expr()
                .parse($case)
                .into_result()
                .expect("parse failed")
                .1
                .as_ref()
                .clone()
        };
    }

    macro_rules! same {
        ($case: literal) => {
            assert_eq!($case.to_string(), expr!($case));
        };
    }

    #[test]
    fn pratt_test_precedence() {
        same!("~1");
        same!("-1");
        same!("+1");
        same!("1 + 2");
        same!("1 - 2");
        same!("1 * 2");
        same!("1 / 2");
        same!("1 % 2");
        same!("1 ^ 2");
        same!("1 & 2");
        same!("1 | 2");
        same!("1 << 2");
        same!("1 >> 2");
        same!("1 || 2");
        same!("1 && 2");
        same!("1 << 2 > 3 >> 1");
        same!("2 << 2 + 2");
        same!("((2 + 2) * 2) + -3");
        same!("2 * 2 + 3 + -3");
        same!("2 * ((2 * 2) + 2)");
        same!("2 + 2 - 1 / 5 % 3");
        same!("foo()");
    }

    #[test]
    fn pratt_test_statements() {
        stmt!("print \"%i\", 42;");
        stmt!("print \"Hello, World!\";");
        stmt!("defer { print \"%i\", 42; }");
        stmt!("while x < 10 { x = x + 1; }");
    }

    #[test]
    fn pratt_test_fn_declaration() {
        assert_eq!(
            "fn main() -> void {\nprint \"Hello, %s\", 42;\n}",
            stmt!("fn main() -> void {\n  print \"Hello, %s\", 42;\n  }")
        );
        same!("foo(1, 3, 4) * foo(2)");
    }

    // ============================================================
    //  Phase 15A tests
    // ============================================================

    #[test]
    fn enum_parses_to_enum_decl() {
        let ast = decl_ast!("enum Option { None, Some(int) }");
        match ast {
            Expression::EnumDecl { name, variants } => {
                assert_eq!(name, "Option");
                assert_eq!(variants.len(), 2);

                // First variant: `None` — zero-arity.
                match variants[0].1.as_ref() {
                    Expression::EnumVariant { name, payload } => {
                        assert_eq!(*name, "None");
                        assert!(matches!(payload, EnumVariantPayload::Unit));
                    }
                    other => panic!("expected EnumVariant(None), got {:?}", other),
                }

                // Second variant: `Some(int)` — single Type payload.
                match variants[1].1.as_ref() {
                    Expression::EnumVariant { name, payload } => {
                        assert_eq!(*name, "Some");
                        match payload {
                            EnumVariantPayload::Tuple(parts) => {
                                assert_eq!(parts.len(), 1);
                                match parts[0].1.as_ref() {
                                    Expression::Type(t) => assert_eq!(*t, "int"),
                                    other => panic!("expected Type(\"int\"), got {:?}", other),
                                }
                            }
                            other => panic!("expected Tuple payload, got {:?}", other),
                        }
                    }
                    other => panic!("expected EnumVariant(Some), got {:?}", other),
                }
            }
            other => panic!("expected EnumDecl, got {:?}", other),
        }
    }

    #[test]
    fn enum_with_record_variant_parses() {
        // Phase 17B: record-shape variants parse to
        // `EnumVariantPayload::Record`.
        let ast = decl_ast!("enum Shape { Circle { x: int, y: int } }");
        match ast {
            Expression::EnumDecl { name, variants } => {
                assert_eq!(name, "Shape");
                assert_eq!(variants.len(), 1);
                match variants[0].1.as_ref() {
                    Expression::EnumVariant { name, payload } => {
                        assert_eq!(*name, "Circle");
                        match payload {
                            EnumVariantPayload::Record(fields) => {
                                assert_eq!(fields.len(), 2);
                                assert_eq!(fields[0].name, "x");
                                assert_eq!(fields[1].name, "y");
                                match fields[0].value.1.as_ref() {
                                    Expression::Type(t) => assert_eq!(*t, "int"),
                                    other => panic!("expected Type(\"int\"), got {:?}", other),
                                }
                                match fields[1].value.1.as_ref() {
                                    Expression::Type(t) => assert_eq!(*t, "int"),
                                    other => panic!("expected Type(\"int\"), got {:?}", other),
                                }
                            }
                            other => panic!("expected Record payload, got {:?}", other),
                        }
                    }
                    other => panic!("expected EnumVariant(Circle), got {:?}", other),
                }
            }
            other => panic!("expected EnumDecl, got {:?}", other),
        }
    }

    #[test]
    fn qualified_construct_parses_to_construct() {
        // `let x = Option::Some(42);` — the RHS of the assignment is
        // a Construct. The `let` returns a Fragment of
        // [Variable(x), Assignment(x, Construct(...))] or similar;
        // the test walks down to the Construct node.
        let ast = decl_ast!("let x = Option::Some(42);");
        // Top-level is a `Statement` wrapping the Fragment produced
        // by `variable()`.
        let frag = match ast {
            Expression::Statement(s) => match s.1.as_ref() {
                Expression::Fragment(items) => items.clone(),
                other => panic!("expected Fragment inside Statement, got {:?}", other),
            },
            Expression::Fragment(items) => items,
            other => panic!("expected Statement/Fragment from let, got {:?}", other),
        };
        // Second item is the `Option::Some(42)` expression (wrapped
        // in Expr by the recursive expr() rule).
        let construct = match frag[1].1.as_ref() {
            Expression::Expr(e) => match e.1.as_ref() {
                Expression::Construct {
                    enum_name,
                    variant_name,
                    fields,
                } => (
                    *enum_name,
                    *variant_name,
                    fields.clone(),
                ),
                other => panic!("expected Construct inside Expr, got {:?}", other),
            },
            other => panic!("expected Expr, got {:?}", other),
        };
        assert_eq!(construct.0, "Option");
        assert_eq!(construct.1, "Some");
        match construct.2 {
            EnumConstructPayload::Tuple(args) => {
                assert_eq!(args.len(), 1);
                match args[0].1.as_ref() {
                    Expression::Integer(n) => assert_eq!(*n, 42),
                    other => panic!("expected Integer(42), got {:?}", other),
                }
            }
            other => panic!("expected Tuple payload, got {:?}", other),
        }
    }

    #[test]
    fn record_construct_parses_to_record_payload() {
        // `E::Foo { x: 1, y: 2 }` parses to a `Construct` with
        // `fields = Record([{ x, 1 }, { y, 2 }])`.
        let ast = decl_ast!("let p = E::Foo { x: 1, y: 2 };");
        let frag = match ast {
            Expression::Statement(s) => match s.1.as_ref() {
                Expression::Fragment(items) => items.clone(),
                other => panic!("expected Fragment, got {:?}", other),
            },
            Expression::Fragment(items) => items,
            other => panic!("expected Fragment, got {:?}", other),
        };
        let construct = match frag[1].1.as_ref() {
            Expression::Expr(e) => match e.1.as_ref() {
                Expression::Construct {
                    enum_name,
                    variant_name,
                    fields,
                } => (*enum_name, *variant_name, fields.clone()),
                other => panic!("expected Construct, got {:?}", other),
            },
            other => panic!("expected Expr, got {:?}", other),
        };
        assert_eq!(construct.0, "E");
        assert_eq!(construct.1, "Foo");
        match construct.2 {
            EnumConstructPayload::Record(parts) => {
                assert_eq!(parts.len(), 2);
                assert_eq!(parts[0].name, "x");
                assert_eq!(parts[1].name, "y");
                match parts[0].value.1.as_ref() {
                    Expression::Integer(n) => assert_eq!(*n, 1),
                    other => panic!("expected Integer(1), got {:?}", other),
                }
                match parts[1].value.1.as_ref() {
                    Expression::Integer(n) => assert_eq!(*n, 2),
                    other => panic!("expected Integer(2), got {:?}", other),
                }
            }
            other => panic!("expected Record payload, got {:?}", other),
        }
    }

    #[test]
    fn bare_construct_is_a_call_not_a_construct() {
        // `Some(42)` (no `::`) is parsed as a `Call` — the typechecker
        // reports an error ('Cannot find function `Some`') for bare
        // constructors — see 15B for details.
        let ast = expr_ast!("Some(42)");
        // The recursive expr() rule wraps the result in `Expr`.
        let inner = match ast {
            Expression::Expr(e) => e.1.as_ref().clone(),
            other => other,
        };
        match inner {
            Expression::Call { name, args } => {
                match name.1.as_ref() {
                    Expression::Identifier(n) => assert_eq!(*n, "Some"),
                    other => panic!("expected Identifier(\"Some\"), got {:?}", other),
                }
                let args = args.expect("Some(42) must have args");
                assert_eq!(args.len(), 1);
                match args[0].1.as_ref() {
                    Expression::Integer(n) => assert_eq!(*n, 42),
                    other => panic!("expected Integer(42), got {:?}", other),
                }
            }
            other => panic!("expected Call (NOT Construct), got {:?}", other),
        }
    }

    #[test]
    fn match_with_constructor_patterns() {
        let ast = expr_ast!("match x { Option::None => 0, Option::Some(v) => v }");
        // The recursive expr() rule wraps the result in `Expr`.
        let inner = match ast {
            Expression::Expr(e) => e.1.as_ref().clone(),
            other => other,
        };
        match inner {
            Expression::Match { scrutinee, arms } => {
                // Scrutinee is `x` (Identifier, possibly wrapped in
                // Expr by the recursive expr() rule).
                match scrutinee.1.as_ref() {
                    Expression::Identifier(n) => assert_eq!(*n, "x"),
                    Expression::Expr(e) => match e.1.as_ref() {
                        Expression::Identifier(n) => assert_eq!(*n, "x"),
                        other => panic!("expected Identifier(x), got {:?}", other),
                    },
                    other => panic!("expected scrutinee to be `x`, got {:?}", other),
                }
                assert_eq!(arms.len(), 2);

                // First arm: `Option::None => 0`
                let MatchArm { pattern, body } = &arms[0];
                match pattern {
                    Pattern::Constructor {
                        enum_name,
                        variant_name,
                        payload,
                    } => {
                        assert_eq!(*enum_name, "Option");
                        assert_eq!(*variant_name, "None");
                        assert!(matches!(payload, PatternPayload::Unit));
                    }
                    other => panic!("expected Constructor(Option::None), got {:?}", other),
                }
                match body.1.as_ref() {
                    Expression::Integer(n) => assert_eq!(*n, 0),
                    other => panic!("expected Integer(0), got {:?}", other),
                }

                // Second arm: `Option::Some(v) => v`
                let MatchArm { pattern, body } = &arms[1];
                match pattern {
                    Pattern::Constructor {
                        enum_name,
                        variant_name,
                        payload,
                    } => {
                        assert_eq!(*enum_name, "Option");
                        assert_eq!(*variant_name, "Some");
                        match payload {
                            PatternPayload::Tuple(parts) => {
                                assert_eq!(parts.len(), 1);
                                match &parts[0] {
                                    Pattern::Binding { name } => assert_eq!(*name, "v"),
                                    other => panic!("expected Binding(v), got {:?}", other),
                                }
                            }
                            other => panic!("expected Tuple payload, got {:?}", other),
                        }
                    }
                    other => panic!("expected Constructor(Option::Some(v)), got {:?}", other),
                }
                match body.1.as_ref() {
                    Expression::Identifier(n) => assert_eq!(*n, "v"),
                    other => panic!("expected Identifier(v), got {:?}", other),
                }
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn record_pattern_shorthand_desugars() {
        // Phase 17B: `Foo { x, y }` (no `: pattern`) desugars at
        // parse time to `Foo { x: Binding("x"), y: Binding("y") }`.
        let ast = expr_ast!("match p { E::Foo { x, y } => x + y }");
        let inner = match ast {
            Expression::Expr(e) => e.1.as_ref().clone(),
            other => other,
        };
        match inner {
            Expression::Match { arms, .. } => {
                match &arms[0].pattern {
                    Pattern::Constructor {
                        enum_name,
                        variant_name,
                        payload,
                    } => {
                        assert_eq!(*enum_name, "E");
                        assert_eq!(*variant_name, "Foo");
                        match payload {
                            PatternPayload::Record(fields) => {
                                assert_eq!(fields.len(), 2);
                                assert_eq!(fields[0].name, "x");
                                assert_eq!(fields[1].name, "y");
                                match &fields[0].pattern {
                                    Pattern::Binding { name } => assert_eq!(*name, "x"),
                                    other => panic!("expected Binding(x), got {:?}", other),
                                }
                                match &fields[1].pattern {
                                    Pattern::Binding { name } => assert_eq!(*name, "y"),
                                    other => panic!("expected Binding(y), got {:?}", other),
                                }
                            }
                            other => panic!("expected Record payload, got {:?}", other),
                        }
                    }
                    other => panic!("expected Constructor(E::Foo), got {:?}", other),
                }
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn wildcard_and_default_both_parse_to_wildcard() {
        // `_` and `default` both parse to the same `Pattern::Wildcard`
        // node — the literal token is discarded (Decision C).
        let ast1 = expr_ast!("match x { _ => 0 }");
        let inner1 = match ast1 {
            Expression::Expr(e) => e.1.as_ref().clone(),
            other => other,
        };
        match inner1 {
            Expression::Match { arms, .. } => {
                assert_eq!(arms.len(), 1);
                assert!(matches!(arms[0].pattern, Pattern::Wildcard));
            }
            other => panic!("expected Match, got {:?}", other),
        }

        let ast2 = expr_ast!("match x { default => 0 }");
        let inner2 = match ast2 {
            Expression::Expr(e) => e.1.as_ref().clone(),
            other => other,
        };
        match inner2 {
            Expression::Match { arms, .. } => {
                assert_eq!(arms.len(), 1);
                assert!(matches!(arms[0].pattern, Pattern::Wildcard));
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn nested_constructor_pattern() {
        let ast = expr_ast!("match x { Option::Some(Option::Some(v)) => v }");
        let inner = match ast {
            Expression::Expr(e) => e.1.as_ref().clone(),
            other => other,
        };
        match inner {
            Expression::Match { arms, .. } => {
                assert_eq!(arms.len(), 1);
                match &arms[0].pattern {
                    Pattern::Constructor {
                        enum_name,
                        variant_name,
                        payload,
                    } => {
                        assert_eq!(*enum_name, "Option");
                        assert_eq!(*variant_name, "Some");
                        match payload {
                            PatternPayload::Tuple(parts) => {
                                assert_eq!(parts.len(), 1);
                                // Nested pattern is itself a Constructor.
                                match &parts[0] {
                                    Pattern::Constructor {
                                        enum_name: inner_enum,
                                        variant_name: inner_variant,
                                        payload: inner_payload,
                                    } => {
                                        assert_eq!(*inner_enum, "Option");
                                        assert_eq!(*inner_variant, "Some");
                                        match inner_payload {
                                            PatternPayload::Tuple(inner_parts) => {
                                                assert_eq!(inner_parts.len(), 1);
                                                match &inner_parts[0] {
                                                    Pattern::Binding { name } => {
                                                        assert_eq!(*name, "v")
                                                    }
                                                    other => panic!(
                                                        "expected Binding(v), got {:?}",
                                                        other
                                                    ),
                                                }
                                            }
                                            other => panic!(
                                                "expected inner Tuple payload, got {:?}",
                                                other
                                            ),
                                        }
                                    }
                                    other => panic!(
                                        "expected nested Constructor(Option::Some(v)), got {:?}",
                                        other
                                    ),
                                }
                            }
                            other => panic!("expected Tuple payload, got {:?}", other),
                        }
                    }
                    other => panic!("expected outer Constructor(Option::Some(...)), got {:?}", other),
                }
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }
}

