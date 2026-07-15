use ast::{
    EnumConstructPayload, EnumVariantPayload, Expression, MatchArm, Output, Pattern, PatternField,
    PatternPayload, RecordFieldDecl, RecordFieldValue, Visibility,
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

    /// Phase 24 — type-annotation parser. Accepts the bare
    /// identifier form (`int`, `string`, `Foo`) AND the
    /// aggregate forms `[T]` / `[T; N]` and `(T1, T2, ...)`
    /// that are needed for array/tuple types in
    /// `let x: [int; 3] = ...`, `fn f(int a) -> [int]`, etc.
    ///
    /// The parser-level representation is a `Tuple`/`Array`
    /// expression node containing nested `Type(name)` nodes
    /// for the element types; the typechecker's `parse_type_name`
    /// helper understands the shape.
    fn type_annotation(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        use chumsky::Parser;
        // `[T]` or `[T; N]` — array form. `T` is the
        // element type; `N` (optional) is a non-negative
        // integer for static-length arrays.
        let array_type = text::ident()
            .padded()
            .map_with(output!(Type))
            .then(
                // Optional `; N` for static length.
                op!(";")
                    .ignore_then(
                        text::int(10)
                            .to_slice()
                            .from_str::<i64>()
                            .validate(|v: Result<i64, _>, _, _| v.unwrap_or(0)),
                    )
                    .or_not(),
            )
            .delimited_by(op!('['), op!(']'))
            .map_with(|(elem, n_opt), e| match n_opt {
                Some(n) => (
                    e.span(),
                    Box::new(Expression::Array(vec![(
                        e.span(),
                        Box::new(Expression::Integer(n)),
                    )])),
                ),
                None => (e.span(), Box::new(Expression::Array(vec![elem]))),
            });
        // `(T1, T2, ...)` — tuple form. Reuses the
        // existing tuple_atom machinery.
        let tuple_type = self.tuple_atom(
            // Inner element: an identifier-or-int (for the
            // `; N` shape inherited from array). For Phase
            // 24 we don't yet support nested aggregate
            // types as type annotations (so a tuple element
            // is just an identifier, not another tuple).
            text::ident().padded().map_with(output!(Type)),
        );
        choice((
            array_type,
            tuple_type,
            text::ident().padded().map_with(output!(Type)),
        ))
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
                // Userland FFI builtins — must come BEFORE
                // `self.call(...)` because `dload(args)` /
                // `declare(args)` / `invoke(args)` would
                // otherwise match `self.call(...)` as ordinary
                // function calls to non-existent functions.
                self.dload(expr.clone()),
                self.declare(expr.clone()),
                self.invoke_(expr.clone()),
                // `(a, b, c)` — tuple atom. MUST come before
                // `self.call(...)` (which expects a leading
                // ident) AND before `self.ident()`.
                self.tuple_atom(expr.clone()),
                // `[a, b, c]` — array atom.
                self.array_atom(expr.clone()),
                // `{ name: expr, ... }` — dict atom (Phase 25).
                // Tried before `self.ident()` only because the
                // backtracking inside `choice` will otherwise
                // try the ident form first (which would fail
                // because `{` isn't an ident). The `Block`
                // parser is statement-level and never sees
                // expression context, so `{` here is unambiguous.
                self.dict_atom(expr.clone()),
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
                // Field access: `expr.identifier` — a postfix
                // operator at Primary precedence. The operator is
                // `.ident` with NO surrounding whitespace required
                // (mirroring Rust/C-style languages); the ident
                // immediately follows the dot.
                //
                // Note: `1.0` is still parsed as a float by the
                // atom-level float parser (which requires an int
                // after the dot), so `1.x` falls through to
                // int + postfix `.x`. This is acceptable for the
                // 18D feature surface; only record-shaped enum
                // payloads need field access.
                postfix(
                    Precedence::Primary as u16,
                    just('.').ignore_then(text::ident()),
                    |lhs, field, e| (e.span(), Box::new(Expression::Access(lhs, field))),
                ),
                // `target[index]` — postfix indexing at Primary
                // precedence (the same level as field access, so
                // `t[i].x` parses as `Access(Index(t, i), x)`).
                // The operator is `[expr]` where `expr` is itself
                // a full expression (so `t[i+1]`, `t[f()]` etc.
                // all parse naturally).
                postfix(
                    Precedence::Primary as u16,
                    expr.clone().delimited_by(op!('['), op!(']')),
                    |lhs, index, e| (e.span(), Box::new(Expression::Index(lhs, index))),
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
        let arg = self
            .type_annotation()
            .then(text::ident().padded())
            .map_with(|(ty, name), e| {
                // For the typechecker the `ty` is an
                // `Expression::Type(name)` (or `Array`/`Tuple`
                // containing `Type`s). Wrap in `Argument` —
                // Phase 24 carries the full type annotation
                // instead of just the source name.
                (e.span(), Box::new(Expression::Argument(ty, name)))
            });

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
            .then(op!("->").ignore_then(self.type_annotation()).or_not())
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
        // Phase 1 (else support): wrap in `recursive` so the
        // `else if` branch can call back into the if-parser itself.
        // The `else` clause is optional:
        //
        //   - absent      → single-branch If
        //   - `else { .. }` → two-branch If (then-branch + else-branch
        //                    with `cond = None`)
        //   - `else if ..` → flatten the inner If's branches into ours,
        //                    so `if c1 {b1} else if c2 {b2} else {b3}`
        //                    becomes a 3-branch If.
        recursive(|if_parser| {
            keyword!("if")
                .ignore_then(self.expr())
                .then(self.block(stmt.clone()))
                .then(
                    keyword!("else")
                        .ignore_then(choice((
                            // `else { body }` — a Block.
                            self.block(stmt.clone()),
                            // `else if ...` — recurse into the if-parser.
                            if_parser,
                        )))
                        .or_not(),
                )
                .map_with(|((cond, body), else_clause), e| {
                    let then_branch: Output =
                        (e.span(), Box::new(Expression::Branch(Some(cond), body)));
                    let mut branches: Vec<Output> = vec![then_branch];
                    if let Some(else_output) = else_clause {
                        match else_output.1.as_ref() {
                            // `else if c2 {b2} [else {b3} ...]` — the
                            // inner `if_parser` returned a fully-
                            // formed If whose branches we flatten
                            // into ours.
                            Expression::If(more_branches) => {
                                branches.extend(more_branches.iter().cloned());
                            }
                            // `else { body }` — a Block. Wrap as the
                            // terminal Branch(None, body).
                            _ => {
                                branches.push((
                                    e.span(),
                                    Box::new(Expression::Branch(None, else_output)),
                                ));
                            }
                        }
                    }
                    (e.span(), Box::new(Expression::If(branches)))
                })
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

    // ============================================================
    //  Userland FFI builtins — `dload`, `declare`, `invoke`
    //
    // These are expression-level (not statement-level like
    // `print`) because they return values. They're parsed
    // inside the atom choice so the keyword is registered with
    // chumsky before `self.ident()` (which would otherwise
    // match them as identifiers).
    // ============================================================

    fn dload<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        text::keyword("dload")
            .labelled("dload builtin")
            .ignore_then(
                expr.clone()
                    .separated_by(op!(','))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(op!('('), op!(')')),
            )
            .map_with(|args, e| {
                (
                    e.span(),
                    Box::new(Expression::Dload(args.into_iter().next().unwrap())),
                )
            })
    }

    fn declare<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        text::keyword("declare")
            .labelled("declare builtin")
            .ignore_then(
                expr.clone()
                    .separated_by(op!(','))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(op!('('), op!(')')),
            )
            .map_with(|args, e| (e.span(), Box::new(Expression::Declare(args))))
    }

    fn invoke_<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        text::keyword("invoke")
            .labelled("invoke builtin")
            .ignore_then(
                expr.clone()
                    .separated_by(op!(','))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(op!('('), op!(')')),
            )
            .map_with(|args, e| (e.span(), Box::new(Expression::Invoke(args))))
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
            self.type_alias(),
            // Phase 29: `use` and `mod` are top-level
            // declarations, registered before the catch-all
            // `stmt` so their leading keywords (`use` / `mod`)
            // aren't mis-parsed as identifiers.
            self.use_(),
            self.mod_(),
            self.enum_decl(),
            self.defer(stmt.clone()),
            self.extern_block(),
            stmt.clone(),
        ))
    }

    /// `type Name = T;` — type alias (Phase 28). The right-hand
    /// side is a full type annotation (`int`, `[int; 5]`,
    /// `(int, string)`, a class name, etc.). The alias is
    /// registered with the typechecker (which substitutes
    /// `Name` with `T` at lookup time). The parser emits a
    /// single `Expression::TypeAlias` node; the codegen
    /// emits nothing (zero-cost).
    fn type_alias(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("type")
            .ignore_then(text::ident().padded())
            .then_ignore(op!("="))
            .then(self.type_annotation())
            .then_ignore(op!(";"))
            .map_with(|(name, ty), e| {
                (
                    e.span(),
                    Box::new(Expression::TypeAlias {
                        name,
                        ty: Box::new(ty),
                    }),
                )
            })
            .labelled("type alias")
    }

    /// `use foo::bar::baz;` — import a single item from a
    /// module path. Phase 29 — replaces the previously
    /// dead `Expression::Use` AST node (which existed but
    /// was never produced by a parser). The codegen +
    /// pipeline populate the typechecker's env and the
    /// compiler's `aliases` map from the parsed `Use` node.
    ///
    /// Forms:
    /// - `use foo::bar;` → `{ path: ["foo"], name: "bar", alias: None }`
    /// - `use foo::bar::baz;` → `{ path: ["foo","bar"], name: "baz", alias: None }`
    /// - `use foo::bar as x;` → `{ path: ["foo"], name: "bar", alias: Some("x") }`
    /// - `use foo::bar::*;` (glob) → encoded as `{ path, name: "*", .. }`.
    ///
    /// The path is built iteratively via `.then()` because
    /// chumsky's `.separated_by` requires the parser to
    /// yield owned values, but `text::ident()` yields a
    /// borrowed slice — the type system can't infer the
    /// conversion. Using `.then_ignore(op!("::"))` repeated
    /// 0..N times gives us a `Vec<&'pratt str>` directly.
    fn use_(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        // A path segment is one ident. The path is
        // `ident (:: ident)* (:: *)?`. Each `::` is
        // followed by either another ident or a `*`
        // (glob marker).
        let segment = text::ident().padded();

        // Each `::`-prefixed piece is either an ident or
        // a `*`. We represent both as `Option<String>`:
        // `Some(name)` for an ident, `None` for the glob
        // marker. The first ident is consumed outside the
        // loop (so we have at least one segment before
        // any `::` is seen).
        let path_tail = op!("::")
            .ignore_then(
                choice((
                    text::ident().padded().map(|s: &str| Some(s.to_string())),
                    just('*').padded().to(None),
                ))
                .map_with(|opt, e| (e.span(), opt)),
            )
            .repeated()
            .collect::<Vec<_>>();

        // Path = first ident + zero or more `::` pieces.
        keyword!("use")
            .ignore_then(segment.then(path_tail))
            .map(
                |(first, rest): (&'pratt str, Vec<(SimpleSpan, Option<String>)>)| {
                    let mut out: Vec<Option<String>> = Vec::with_capacity(1 + rest.len());
                    out.push(Some(first.to_string()));
                    for (_span, opt) in rest {
                        out.push(opt);
                    }
                    out
                },
            )
            // After the path: either `as alias` (alias form)
            // or nothing (concrete form). The glob marker
            // (`*`) was consumed inside the path_tail above.
            // The trailing `;` is consumed by the outer
            // `.then_ignore(op!(";"))` below — we must NOT
            // consume it here.
            .then(
                keyword!("as")
                    .ignore_then(text::ident().padded())
                    .map(|s: &str| s.to_string())
                    .or_not(),
            )
            .then_ignore(op!(";"))
            .map_with(|(segments, alias), e| {
                // Walk the segments. The last segment is
                // either:
                //   - Some(name) — concrete import.
                //   - None — glob (`use foo::bar::*;`).
                // All earlier segments form the path.
                let mut segs = segments;
                let last = segs
                    .pop()
                    .expect("at least one segment from the leading ident");
                let path: Vec<String> = segs
                    .into_iter()
                    .map(|opt| opt.expect("only the LAST segment may be a glob"))
                    .collect();

                if let Some(name) = last {
                    // Concrete import. Alias is whatever
                    // the `as` clause produced.
                    (e.span(), Box::new(Expression::Use { path, name, alias }))
                } else {
                    // Glob import. `alias` is always None
                    // (a glob can't be aliased — the
                    // alias would have nothing to bind
                    // to, since glob imports are resolved
                    // by the pipeline at compile time).
                    (
                        e.span(),
                        Box::new(Expression::Use {
                            path,
                            name: "*".to_string(),
                            alias: None,
                        }),
                    )
                }
            })
            .labelled("use statement")
    }

    /// `mod foo;` — forward declaration of a module.
    /// Phase 29 — registers `foo` as a module the pipeline
    /// should load (if not already loaded). Useful for
    /// forward references and for making module usage
    /// explicit at the top of a file.
    ///
    /// `mod foo;` triggers the pipeline to load `foo.0s`
    /// (via `Manifest::resolve_module`), but does NOT pull
    /// its items into the current scope. Use a separate
    /// `use foo::bar;` to refer to a specific item.
    ///
    /// The parser reuses the existing
    /// `Expression::Module(String, Output)` variant with a
    /// `Noop` body. The pipeline only consumes the name;
    /// the body is meaningless.
    fn mod_(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("mod")
            .ignore_then(text::ident().padded().map(|s: &str| s))
            .then_ignore(op!(";"))
            .map_with(|name: &'pratt str, e| {
                let noop_span = e.span();
                // Noop wraps an Output, which wraps a Box<Expression>.
                // We use a leaf Integer(0) as the inner expression.
                // The pipeline doesn't traverse the body; the
                // name is all that matters.
                let inner: Output = (noop_span, Box::new(Expression::Integer(0)));
                let body: Output = (noop_span, Box::new(Expression::Noop(inner)));
                (
                    e.span(),
                    Box::new(Expression::Module(name.to_string(), body)),
                )
            })
            .labelled("mod declaration")
    }

    /// `extern "libname" { fn name(args) -> ret; ... }` — declare
    /// external (FFI) functions from a shared library.
    ///
    /// The block contains a list of zero-or-more function
    /// declarations with a trailing semicolon (no body). Each
    /// `fn name(args) -> ret;` inside the `{ ... }` produces an
    /// `ExternFunction` (a separate struct, not an `Expression`
    /// variant — extern functions are metadata, not runtime
    /// expressions). The whole block produces an
    /// `Expression::ExternBlock` carrying the library name and
    /// the list of declared functions.
    fn extern_block(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        use crate::ast::ExternFunction;
        // The `fn name(args) -> ret;` sub-parser produces
        // `ExternFunction` directly (not an `Output`). The
        // declaration chain accepts parsers of any output type
        // as long as the final `map_with` produces an `Output`.
        let extern_function_decl = keyword!("fn")
            .then(text::ident().padded())
            .then(self.arg_list())
            .then(op!("->").ignore_then(text::ident().padded()).or_not())
            // The trailing `;` is required (no body).
            .then_ignore(op!(";"))
            .map_with(|(((_, name), args), returns), _e| ExternFunction {
                name,
                args,
                returns,
            });

        // Inline string-literal parser for the library name.
        // We don't use `self.string()` because it returns an
        // `Output` (wrapping the value in an `Expression`),
        // but we just need the raw `String` for the library
        // name (it's metadata, not a runtime expression).
        let library_name = just('"')
            .ignore_then(none_of('"').repeated().to_slice())
            .then_ignore(just('"'))
            .map(|s: &'pratt str| s.to_string());

        keyword!("extern")
            .ignore_then(library_name)
            .then(
                extern_function_decl
                    .repeated()
                    .collect::<Vec<_>>()
                    .delimited_by(op!("{"), op!("}")),
            )
            .map_with(|(library, declarations), e| {
                (
                    e.span(),
                    Box::new(Expression::ExternBlock {
                        library,
                        declarations,
                    }),
                )
            })
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
            .then(op!(":").ignore_then(self.type_annotation()).or_not())
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

    /// `(a, b, c)` — tuple literal atom.
    ///
    /// Phase 24 fix: a tuple literal REQUIRES a comma inside
    /// the parens. Without a comma, the parens are a Group
    /// (parenthesised expression), NOT a tuple. This matches
    /// the user's spec:
    ///   - `(1, 2)`   → tuple (two elements, comma-separated)
    ///   - `(1,)`     → tuple (one element, trailing comma)
    ///   - `(1)`      → group (one element, no comma)
    ///   - `(1 + 2)`  → group (single expression, no comma)
    ///
    /// The previous implementation matched any `at_least(1)`
    /// parenthesised list as a tuple, which broke `f((1+2)*3)`
    /// arithmetic by mis-parsing the `(1+2)` grouping as a
    /// 1-tuple.
    fn tuple_atom<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        // A tuple is `at_least(2)` items separated by commas,
        // OR exactly 1 item with a trailing comma. Either way,
        // a comma must be present inside the parens.
        use chumsky::Parser;
        let two_or_more = expr
            .clone()
            .separated_by(op!(','))
            .allow_trailing()
            .at_least(2)
            .collect::<Vec<_>>();
        let one_with_trailing = expr
            .clone()
            .then_ignore(op!(','))
            .map(|e| vec![e])
            .labelled("single-element tuple");
        choice((two_or_more, one_with_trailing))
            .delimited_by(op!('('), op!(')'))
            .map_with(|items, e| (e.span(), Box::new(Expression::Tuple(items))))
            .labelled("tuple")
    }

    /// `{name: expr, name: expr, ...}` — anonymous record /
    /// dict literal (Phase 25). Parsed in the atom choice,
    /// BEFORE the `tuple_atom` and the `Block` parser (the
    /// latter matches `{ stmt; stmt; }` in statement
    /// position, which the atom choice never sees).
    ///
    /// Bare `{` is unambiguous with `Block` because the
    /// `Block` parser lives at statement level (not in the
    /// expression atom), and bare `{` followed by a field
    /// shape (`ident : expr`) can never start a block (a
    /// valid expression after `{` would have to be an
    /// identifier — but our parser knows that an
    /// identifier alone is a value, not a statement, so the
    /// dispatch picks the `Dict` arm).
    fn dict_atom<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        use crate::ast::RecordFieldValue;
        use chumsky::Parser;
        // Each field: `name : expr`. Bare name (without the
        // `:`) isn't supported here (the typechecker and
        // codegen always have the explicit form, and the
        // pre-Phase-25 enum record constructor uses the same
        // explicit form).
        let field = text::ident()
            .padded()
            .then_ignore(op!(":"))
            .then(expr)
            .map_with(|(name, value), e| (e.span(), RecordFieldValue { name, value }))
            .labelled("dict field");
        field
            .separated_by(op!(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(op!("{"), op!("}"))
            .map_with(|fields, e| {
                let fs: Vec<RecordFieldValue<'pratt>> =
                    fields.into_iter().map(|(_, f)| f).collect();
                (e.span(), Box::new(Expression::Dict(fs)))
            })
            .labelled("dict")
    }
    ///
    /// Same shape as the tuple atom but with `[]` brackets.
    /// Empty `[]` IS allowed (the parser distinguishes by
    /// bracket shape; an empty array is fine).
    fn array_atom<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        use chumsky::Parser;
        let inner = expr
            .clone()
            .separated_by(op!(','))
            .allow_trailing()
            .collect::<Vec<_>>();
        inner
            .delimited_by(op!('['), op!(']'))
            .map_with(|items, e| (e.span(), Box::new(Expression::Array(items))))
            .labelled("array")
    }

    /// `target[index]` postfix indexing helper. The actual
    /// wiring lives at the `pratt` call site below; this
    /// method is unused and reserved for future expansion
    /// (e.g., for explicit `slice(i, j)` syntax).
    #[allow(dead_code)]
    fn index_postfix_disabled<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        _expr: T,
    ) {
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
            .map_with(|(name, value), e| (e.span(), RecordFieldValue { name, value }))
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
                EnumConstructPayload::Record(fields.into_iter().map(|(_, f)| f).collect())
            })
            .labelled("record payload");

        // Tuple payload: `(arg1, arg2, ...)` — `None` means Unit.
        // Empty parens `()` are also treated as Unit (so users can
        // write `Option::None()` instead of `Option::None`).
        let tuple_payload = self.params(expr.clone()).map(|opt| match opt {
            Some(args) if args.is_empty() => EnumConstructPayload::Unit,
            Some(args) => EnumConstructPayload::Tuple(args),
            None => EnumConstructPayload::Unit,
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
                (e.span(), Box::new(Expression::Match { scrutinee, arms }))
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
    ) -> impl Parser<'pratt, &'pratt str, MatchArm<'pratt>, extra::Err<Rich<'pratt, char>>>
    + Clone
    + 'pratt {
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
                .then(op!(":").ignore_then(pattern_parser.clone()).or_not())
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
                .map(PatternPayload::Record)
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
                .map_with(
                    |((enum_name, variant_name), payload), _| Pattern::Constructor {
                        enum_name,
                        variant_name,
                        payload,
                    },
                );

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
                (e.span(), Box::new(Expression::EnumDecl { name, variants }))
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
            .map(EnumVariantPayload::Record)
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
    use crate::Pratt;
    use crate::ast::{
        EnumConstructPayload, EnumVariantPayload, Expression, MatchArm, Pattern, PatternPayload,
    };
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
                } => (*enum_name, *variant_name, fields.clone()),
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
            Expression::Match { arms, .. } => match &arms[0].pattern {
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
            },
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
                    other => panic!(
                        "expected outer Constructor(Option::Some(...)), got {:?}",
                        other
                    ),
                }
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    // ============================================================
    //  Phase 18D tests — postfix field access
    // ============================================================

    #[test]
    fn postfix_field_access_parses_to_access() {
        // `point.x` should parse to `Access(point, "x")` at the
        // top level (modulo the `Expression::Expr` wrapper that
        // the recursive expr() rule always inserts).
        let ast = expr_ast!("point.x");
        let inner = match ast {
            Expression::Expr(e) => e.1.as_ref().clone(),
            other => other,
        };
        match inner {
            Expression::Access(receiver, field) => {
                // Receiver is `point` (possibly wrapped in Expr
                // by the recursive rule — strip it for the
                // assertion).
                let recv_inner = match receiver.1.as_ref() {
                    Expression::Expr(e) => e.1.as_ref(),
                    other => other,
                };
                match recv_inner {
                    Expression::Identifier(n) => assert_eq!(*n, "point"),
                    other => panic!("expected Identifier(point), got {:?}", other),
                }
                assert_eq!(field, "x");
            }
            other => panic!("expected Access, got {:?}", other),
        }
    }

    #[test]
    fn postfix_field_access_chains_left_to_right() {
        // `p.x.y` should parse as `Access(Access(p, "x"), "y")` —
        // the outermost access is the LAST `.y`, with `.x` as the
        // inner receiver. Postfix combinators bind left-to-right
        // in a Pratt parser.
        let ast = expr_ast!("p.x.y");
        let inner = match ast {
            Expression::Expr(e) => e.1.as_ref().clone(),
            other => other,
        };
        match inner {
            Expression::Access(outer_receiver, outer_field) => {
                assert_eq!(outer_field, "y");
                match outer_receiver.1.as_ref() {
                    Expression::Access(inner_receiver, inner_field) => {
                        assert_eq!(*inner_field, "x");
                        // Innermost receiver: `p` (Identifier,
                        // possibly wrapped in Expr).
                        let recv_inner = match inner_receiver.1.as_ref() {
                            Expression::Expr(e) => e.1.as_ref(),
                            other => other,
                        };
                        match recv_inner {
                            Expression::Identifier(n) => assert_eq!(*n, "p"),
                            other => panic!("expected Identifier(p), got {:?}", other),
                        }
                    }
                    other => panic!("expected Access(p, x) as outer receiver, got {:?}", other),
                }
            }
            other => panic!("expected Access(p.x, y), got {:?}", other),
        }
    }

    #[test]
    fn postfix_field_access_display_round_trips() {
        // The Display arm for `Access` produces `receiver.field`,
        // which should round-trip via the `same!` macro.
        same!("point.x");
        same!("p.x.y");
    }

    #[test]
    fn postfix_field_access_does_not_break_float_parsing() {
        // Regression: `1.0` must still parse as a Float atom, not
        // as `int(1) + postfix('.0')`. The float parser requires
        // an int after the dot, which the field-access postfix
        // operator (which requires an ident) cannot satisfy.
        let ast = expr_ast!("1.0");
        let inner = match ast {
            Expression::Expr(e) => e.1.as_ref().clone(),
            other => other,
        };
        assert!(
            matches!(inner, Expression::Float(_)),
            "expected Float(1.0), got {:?}",
            inner
        );
    }

    // ============================================================
    //  Phase 1 tests — `else` keyword support for if expressions
    // ============================================================

    /// Walk an if-block body (a `Block(Vec<Statement>)`) out of the
    /// standard `Function { body: Block([Statement(If(...))]) }`
    /// wrapper produced by parsing a `fn main() { ... }` decl.
    /// Returns the inner `If` AST.
    ///
    /// Inlined (rather than calling `decl_ast!`) because that macro
    /// requires a literal token-tree for its `$case:literal` matcher
    /// — passing a `&str` variable is rejected.
    fn unwrap_fn_if(src: &str) -> Expression<'_> {
        let ast = Pratt::default()
            .declaration()
            .parse(src)
            .into_result()
            .expect("parse failed")
            .1
            .as_ref()
            .clone();
        // Top: Function { ..., body: Block([...]) }
        let fn_body = match ast {
            Expression::Function { body, .. } => body,
            other => panic!("expected Function decl, got {:?}", other),
        };
        let stmts = match fn_body.1.as_ref() {
            Expression::Block(stmts) => stmts.clone(),
            other => panic!("expected Block body, got {:?}", other),
        };
        assert_eq!(stmts.len(), 1, "expected exactly one stmt in body");
        let inner = stmts.into_iter().next().unwrap();
        let inner_stmt = match inner.1.as_ref() {
            Expression::Statement(s) => s.1.as_ref().clone(),
            other => other.clone(),
        };

        match inner_stmt {
            Expression::If(branches) => Expression::If(branches),
            other => panic!("expected If, got {:?}", other),
        }
    }

    #[test]
    fn parse_if_without_else_still_works() {
        // Regression: pre-Phase-1 single-branch if must still parse
        // to exactly one Branch(Some(cond), body) inside the If.
        let src = "fn main() { if 1 > 0 { return 1; } }";
        match unwrap_fn_if(src) {
            Expression::If(branches) => {
                assert_eq!(branches.len(), 1, "single-branch if has 1 branch");
                let (cond_opt, _) = match branches[0].1.as_ref() {
                    Expression::Branch(c, b) => (c.clone(), b.clone()),
                    other => panic!("expected Branch, got {:?}", other),
                };
                assert!(
                    cond_opt.is_some(),
                    "the lone if-branch's cond must be Some(_), not None"
                );
            }
            other => panic!("expected If, got {:?}", other),
        }
    }

    #[test]
    fn parse_if_with_else_single_branch() {
        // `if c { b } else { eb }` → two branches; the second has
        // `cond = None` (terminal else).
        let src = "fn main() { if 1 > 0 { return 1; } else { return 0; } }";
        match unwrap_fn_if(src) {
            Expression::If(branches) => {
                assert_eq!(branches.len(), 2, "if/else has 2 branches");
                // First branch: Some(cond)
                let (cond_opt, _) = match branches[0].1.as_ref() {
                    Expression::Branch(c, b) => (c.clone(), b.clone()),
                    other => panic!("expected Branch at index 0, got {:?}", other),
                };
                assert!(cond_opt.is_some(), "first if-branch's cond must be Some(_)");
                // Second branch: None (the terminal else)
                let (cond_opt, _) = match branches[1].1.as_ref() {
                    Expression::Branch(c, b) => (c.clone(), b.clone()),
                    other => panic!("expected Branch at index 1, got {:?}", other),
                };
                assert!(cond_opt.is_none(), "else-branch's cond must be None");
            }
            other => panic!("expected If, got {:?}", other),
        }
    }

    #[test]
    fn parse_if_else_if_no_final_else() {
        // `if c1 { b1 } else if c2 { b2 }` → two branches, both with
        // Some(cond). The else-if gets flattened into the outer If.
        let src = "fn main() { if 1 > 0 { return 1; } else if 1 < 0 { return 2; } }";
        match unwrap_fn_if(src) {
            Expression::If(branches) => {
                assert_eq!(branches.len(), 2, "if/else-if has 2 branches");
                for (i, branch) in branches.iter().enumerate() {
                    let (cond_opt, _) = match branch.1.as_ref() {
                        Expression::Branch(c, b) => (c.clone(), b.clone()),
                        other => panic!("expected Branch at index {}, got {:?}", i, other),
                    };
                    assert!(
                        cond_opt.is_some(),
                        "branch #{} cond must be Some(_) (no terminal else)",
                        i
                    );
                }
            }
            other => panic!("expected If, got {:?}", other),
        }
    }

    #[test]
    fn parse_if_else_if_single_else() {
        // `if c1 { b1 } else if c2 { b2 } else { b3 }` → three branches.
        // Branch 0: Some(c1), Branch 1: Some(c2), Branch 2: None.
        let src =
            "fn main() { if 1 > 0 { return 1; } else if 1 < 0 { return 2; } else { return 3; } }";
        match unwrap_fn_if(src) {
            Expression::If(branches) => {
                assert_eq!(branches.len(), 3, "if/else-if/else has 3 branches");
                // First two: Some(cond)
                for i in 0..2 {
                    let (cond_opt, _) = match branches[i].1.as_ref() {
                        Expression::Branch(c, b) => (c.clone(), b.clone()),
                        other => panic!("expected Branch at index {}, got {:?}", i, other),
                    };
                    assert!(cond_opt.is_some(), "branch #{} cond must be Some(_)", i);
                }
                // Last: None (terminal else)
                let (cond_opt, _) = match branches[2].1.as_ref() {
                    Expression::Branch(c, b) => (c.clone(), b.clone()),
                    other => panic!("expected Branch at index 2, got {:?}", other),
                };
                assert!(
                    cond_opt.is_none(),
                    "terminal else-branch's cond must be None"
                );
            }
            other => panic!("expected If, got {:?}", other),
        }
    }

    #[test]
    fn parse_if_else_chain_deep() {
        // `if c1 else if c2 else if c3 else` — four branches, all
        // `Some` except the last. Verifies the recursive flattening
        // works at depth ≥ 2.
        let src = "fn main() { if 1 > 0 { return 1; } else if 1 < 0 { return 2; } else if 1 == 0 { return 3; } else { return 4; } }";
        match unwrap_fn_if(src) {
            Expression::If(branches) => {
                assert_eq!(branches.len(), 4, "if/else-if/else-if/else has 4 branches");
                for i in 0..3 {
                    let (cond_opt, _) = match branches[i].1.as_ref() {
                        Expression::Branch(c, b) => (c.clone(), b.clone()),
                        other => panic!("expected Branch at index {}, got {:?}", i, other),
                    };
                    assert!(cond_opt.is_some(), "branch #{} cond must be Some(_)", i);
                }
                let (cond_opt, _) = match branches[3].1.as_ref() {
                    Expression::Branch(c, b) => (c.clone(), b.clone()),
                    other => panic!("expected Branch at index 3, got {:?}", other),
                };
                assert!(
                    cond_opt.is_none(),
                    "terminal else-branch's cond must be None"
                );
            }
            other => panic!("expected If, got {:?}", other),
        }
    }

    #[test]
    fn parse_if_with_dangling_else_fails() {
        // Regression: `else` with no following body must NOT silently
        // parse. The `.or_not()` only catches the case where the
        // whole `else` keyword is absent — once `else` is committed,
        // the choice must match either a Block or another if-parser.
        let src = "fn main() { if 1 > 0 { return 1; } else }";
        let result = Pratt::default().declaration().parse(src).into_result();
        assert!(
            result.is_err(),
            "expected parse to fail for dangling else, got {:?}",
            result
        );
    }

    // ============================================================
    //  Phase 22: FFI — `extern "libname" { fn ...; }` blocks
    // ============================================================

    /// `extern "c" { fn puts(string s); }` parses to an
    /// `Expression::ExternBlock` with the library name "c" and
    /// a single function declaration `puts` (arity 1, returns
    /// `None`).
    #[test]
    fn parse_extern_block_single_function() {
        let ast = decl_ast!("extern \"c\" { fn puts(string s); }");
        match ast {
            Expression::ExternBlock {
                library,
                declarations,
            } => {
                assert_eq!(library, "c");
                assert_eq!(declarations.len(), 1);
                let f = &declarations[0];
                assert_eq!(f.name, "puts");
                // Returns: none
                assert!(f.returns.is_none());
            }
            other => panic!("expected ExternBlock, got {:?}", other),
        }
    }

    /// Multiple functions in one extern block all parse, in
    /// source order.
    #[test]
    fn parse_extern_block_multiple_functions() {
        let ast = decl_ast!("extern \"c\" { fn puts(string s); fn strlen(string s) -> int; }");
        match ast {
            Expression::ExternBlock {
                library,
                declarations,
            } => {
                assert_eq!(library, "c");
                assert_eq!(declarations.len(), 2);
                assert_eq!(declarations[0].name, "puts");
                assert!(declarations[0].returns.is_none());
                assert_eq!(declarations[1].name, "strlen");
                assert_eq!(declarations[1].returns, Some("int"));
            }
            other => panic!("expected ExternBlock, got {:?}", other),
        }
    }

    /// An empty extern block (no declarations) is valid
    /// syntax — the `dlopen` call happens, but no symbols
    /// are bound. Useful for side-effect-only libraries.
    #[test]
    fn parse_extern_block_empty_body() {
        let ast = decl_ast!("extern \"m\" {}");
        match ast {
            Expression::ExternBlock {
                library,
                declarations,
            } => {
                assert_eq!(library, "m");
                assert!(declarations.is_empty());
            }
            other => panic!("expected ExternBlock, got {:?}", other),
        }
    }

    /// A trailing semicolon is required — `fn foo(string s)` (no
    /// `;`) should fail to parse, so users can't accidentally
    /// write a function body that won't be executed.
    #[test]
    fn parse_extern_function_requires_trailing_semicolon() {
        let src = "extern \"c\" { fn puts(string s) }"; // missing ';'
        let result = Pratt::default().declaration().parse(src).into_result();
        assert!(
            result.is_err(),
            "expected parse to fail for missing trailing ';' in extern fn, got {:?}",
            result
        );
    }

    // ============================================================
    //  Phase 29: `use` and `mod` parsers
    // ============================================================

    /// `use foo::bar;` — single-segment path. Path is `["foo"]`,
    /// name is `"bar"`, no alias.
    #[test]
    fn parse_use_single_segment() {
        let src = "use foo::bar;";
        let result = Pratt::default().declaration().parse(src).into_result();
        match result {
            Ok((_span, expr)) => match expr.as_ref() {
                Expression::Use { path, name, alias } => {
                    assert_eq!(path, &["foo".to_string()]);
                    assert_eq!(name, "bar");
                    assert!(alias.is_none());
                }
                other => panic!("expected Use, got {:?}", other),
            },
            Err(e) => panic!("parse failed: {:?}", e),
        }
    }

    /// `use foo::bar::baz;` — multi-segment path. Path is
    /// `["foo", "bar"]`, name is `"baz"`.
    #[test]
    fn parse_use_multi_segment() {
        let src = "use foo::bar::baz;";
        let result = Pratt::default().declaration().parse(src).into_result();
        match result {
            Ok((_span, expr)) => match expr.as_ref() {
                Expression::Use { path, name, alias } => {
                    assert_eq!(path, &["foo".to_string(), "bar".to_string()]);
                    assert_eq!(name, "baz");
                    assert!(alias.is_none());
                }
                other => panic!("expected Use, got {:?}", other),
            },
            Err(e) => panic!("parse failed: {:?}", e),
        }
    }

    /// `use foo::bar as x;` — alias. Path is `["foo"]`, name is
    /// `"bar"`, alias is `Some("x")`.
    #[test]
    fn parse_use_with_alias() {
        let src = "use foo::bar as x;";
        let result = Pratt::default().declaration().parse(src).into_result();
        match result {
            Ok((_span, expr)) => match expr.as_ref() {
                Expression::Use { path, name, alias } => {
                    assert_eq!(path, &["foo".to_string()]);
                    assert_eq!(name, "bar");
                    assert_eq!(alias.as_deref(), Some("x"));
                }
                other => panic!("expected Use, got {:?}", other),
            },
            Err(e) => panic!("parse failed: {:?}", e),
        }
    }

    /// `use foo::bar::*;` — glob. Path is `["foo", "bar"]`,
    /// name is `"*"` (sentinel for glob).
    #[test]
    fn parse_use_glob() {
        let src = "use foo::bar::*;";
        let result = Pratt::default().declaration().parse(src).into_result();
        match result {
            Ok((_span, expr)) => match expr.as_ref() {
                Expression::Use { path, name, alias } => {
                    assert_eq!(path, &["foo".to_string(), "bar".to_string()]);
                    assert_eq!(name, "*");
                    assert!(alias.is_none());
                }
                other => panic!("expected Use, got {:?}", other),
            },
            Err(e) => panic!("parse failed: {:?}", e),
        }
    }

    /// `mod foo;` — forward declaration. Body is a Noop;
    /// pipeline only consumes the name.
    #[test]
    fn parse_mod_forward_declaration() {
        let src = "mod foo;";
        let result = Pratt::default().declaration().parse(src).into_result();
        match result {
            Ok((_span, expr)) => match expr.as_ref() {
                Expression::Module(name, _body) => {
                    assert_eq!(name, "foo");
                }
                other => panic!("expected Module, got {:?}", other),
            },
            Err(e) => panic!("parse failed: {:?}", e),
        }
    }
}
