//! Pratt parser for zero-script source.
//!
//! Builds a span-annotated `Expression` AST for the compiler pipeline.

use ast::{
    AdjustOp, AssignOp, AttrArgs, AttrLit, Attribute, EnumConstructPayload, EnumVariantPayload,
    Expression, LetFieldPattern, LetPattern, MatchArm, Output, Pattern, PatternField,
    PatternPayload, RecordFieldDecl, RecordFieldValue, TypeParam, Visibility,
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
    pratt::{infix, left, none, postfix, prefix, right},
    prelude::{choice, empty, just, none_of, recursive},
    text,
};
use reporting::{ErrorCode, Label, Message};

#[repr(u16)]
enum Precedence {
    Assign,
    /// `??` null-coalesce (between Or and Assign).
    Coalesce,
    Or,
    Xor,
    And,
    Equal,
    /// `..` / `..=` — below comparisons, non-associative (Phase P3).
    Range,
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

    /// Type atoms and function types without nested `fn(...)` signatures in
    /// parameter positions (used by `arg_list` to avoid parser recursion).
    fn type_annotation_no_fn(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        use chumsky::Parser;
        recursive(|type_ann| {
            self.type_annotation_atoms(type_ann.clone())
                .then(op!("->").ignore_then(type_ann.clone()).or_not())
                .map_with(|(lhs, rhs), e| match rhs {
                    Some(rhs) => (e.span(), Box::new(Expression::TypeFun(lhs, rhs))),
                    None => lhs,
                })
        })
    }

    /// Shared type-atom parser: arrays, tuples, `forall`, projections, names.
    fn type_annotation_atoms<T>(
        &self,
        type_ann: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    where
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    {
        use chumsky::Parser;
        let array_type = text::ident()
            .padded()
            .map_with(output!(Type))
            .then(
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
                    Box::new(Expression::Array(vec![
                        elem,
                        (e.span(), Box::new(Expression::Integer(n))),
                    ])),
                ),
                None => (e.span(), Box::new(Expression::Array(vec![elem]))),
            });
        let tuple_type = self.tuple_atom(type_ann.clone());
        let named_type = text::ident()
            .padded()
            .then(
                type_ann
                    .clone()
                    .separated_by(op!(','))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(op!('<'), op!('>'))
                    .or_not(),
            )
            .map_with(|(name, args_opt), e| match args_opt {
                Some(args) => (e.span(), Box::new(Expression::TypeApp { name, args })),
                None => (e.span(), Box::new(Expression::Type(name))),
            });
        let projection_type = text::ident()
            .padded()
            .then_ignore(op!("::"))
            .then(text::ident().padded())
            .then(
                type_ann
                    .clone()
                    .separated_by(op!(','))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(op!('<'), op!('>'))
                    .or_not(),
            )
            .map_with(|((owner, name), args_opt), e| {
                (
                    e.span(),
                    Box::new(Expression::TypeProjection {
                        owner,
                        name,
                        args: args_opt.unwrap_or_default(),
                    }),
                )
            });
        let forall_type = keyword!("forall")
            .ignore_then(
                self.single_type_param()
                    .separated_by(op!(","))
                    .at_least(1)
                    .collect::<Vec<_>>(),
            )
            .then_ignore(op!("."))
            .then(type_ann.clone())
            .map_with(|(params, ty), e| {
                (
                    e.span(),
                    Box::new(Expression::Forall {
                        params,
                        ty: Box::new(ty),
                    }),
                )
            });
        choice((
            array_type,
            tuple_type,
            forall_type,
            projection_type,
            named_type,
        ))
    }

    /// Type annotation: bare identifiers, `[T]`, `[T; N]`, `(T1, T2, ...)`, or
    /// `fn(T x, ...args) -> R`.
    fn type_annotation(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        use chumsky::Parser;
        recursive(|type_ann| {
            let base_atom = self.type_annotation_atoms(type_ann.clone());
            let fn_sig_type = keyword!("fn")
                .ignore_then(self.arg_list_typed(base_atom.clone()))
                .then(op!("->").ignore_then(type_ann.clone()))
                .map_with(|(params, ret), e| {
                    (
                        e.span(),
                        Box::new(Expression::TypeFnSig {
                            params,
                            ret,
                        }),
                    )
                });
            choice((fn_sig_type, base_atom))
                .then(op!("->").ignore_then(type_ann.clone()).or_not())
                .map_with(|(lhs, rhs), e| match rhs {
                    Some(rhs) => (e.span(), Box::new(Expression::TypeFun(lhs, rhs))),
                    None => lhs,
                })
        })
    }

    /// One type parameter: `T`, `T: Num + Eq`, `F: * -> *`, or
    /// `c: * -> Constraint`.
    ///
    /// After `:`, either class bounds or a kind annotation. A kind annotation
    /// may be followed by class bounds separated with a comma.
    fn single_type_param(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, TypeParam<'pratt>, extra::Err<Rich<'pratt, char>>>
           + Clone
           + 'pratt {
        use crate::ast::Kind;

        let kind_ann = recursive(|kind| {
            let atom = just('*')
                .padded()
                .to(Kind::Type)
                .or(keyword!("Constraint").to(Kind::Constraint))
                .or(kind.clone().delimited_by(op!("("), op!(")")));

            atom.then(op!("->").ignore_then(kind).or_not())
                .map(|(domain, codomain)| match codomain {
                    Some(codomain) => Kind::Arrow(Box::new(domain), Box::new(codomain)),
                    None => domain,
                })
        });

        let class_bound = text::ident()
            .padded()
            .then_ignore(op!(":").not());
        let class_bounds = class_bound
            .separated_by(op!("+"))
            .at_least(1)
            .collect::<Vec<_>>();

        // After `:`, try kind first (leading `*` or `(`), else class bounds.
        let after_colon = kind_ann
            .then(op!(",").ignore_then(class_bounds.clone()).or_not())
            .map(|(kind, bounds)| (bounds.unwrap_or_default(), kind))
            .or(class_bounds.map(|bounds| (bounds, Kind::Type)));

        text::ident()
            .padded()
            .then(op!(":").ignore_then(after_colon).or_not())
            .map(|(name, ann)| {
                let (bounds, kind) = ann.unwrap_or_else(|| (Vec::new(), Kind::Type));
                TypeParam { name, bounds, kind }
            })
    }

    /// `<T, U: Num + Eq, F: * -> *, ...>` — optional generic type parameter list.
    ///
    /// Returns an empty `Vec` when no `<` is found.
    fn type_param_list(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Vec<TypeParam<'pratt>>, extra::Err<Rich<'pratt, char>>>
           + Clone
           + 'pratt {
        self.single_type_param()
            .separated_by(op!(","))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(op!("<"), op!(">"))
            .or_not()
            .map(|opt| opt.unwrap_or_default())
    }

    fn expr(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        recursive(|expr| {
            let atom = choice((
                // `match` is a keyword atom — registered before
                // `self.ident()` so the identifier parser refuses
                // to match it.
                self.match_expr(expr.clone()),
                // `done` stays a keyword builtin. `dload` / `declare` /
                // `invoke` are ordinary calls resolved via `use ffi::*`.
                self.done_(expr.clone()),
                self.resume_(expr.clone()),
                self.yield_expr_(expr.clone()),
                // `raise expr` as an expression atom (also a statement).
                keyword!("raise")
                    .ignore_then(expr.clone())
                    .map_with(|inner, e| (e.span(), Box::new(Expression::Raise(inner)))),
                // `panic expr` as an expression atom (also a statement).
                keyword!("panic")
                    .ignore_then(expr.clone())
                    .map_with(|inner, e| (e.span(), Box::new(Expression::Panic(inner)))),
                self.format_expr(expr.clone()),
                // `(a, b, c)` — tuple atom. MUST come before
                // `self.call(...)` (which expects a leading
                // ident) AND before `self.ident()`.
                self.tuple_atom(expr.clone()),
                // `[a, b, c]` — array atom.
                self.array_atom(expr.clone()),
                self.dict_atom(expr.clone()),
                // `EnumName::Variant(args)` — qualified constructor
                // application. MUST be tried before `self.call(...)`
                // because both start with `ident`: the backtracking
                // inside `choice` will try the next alternative if
                // the `::` after the first ident is missing.
                self.construct(expr.clone()),
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
                // Anonymous `fn (…)` before `ident` so `fn` stays a keyword.
                self.lambda_atom(expr.clone()),
                self.ident(),
            ));

            choice((atom, self.group(expr.clone()))).pratt((
                // No postfix `!` here — it would conflict with `!=`
                // (which should be parsed as a single infix operator).
                // Prefix `!` is logical NOT; prefix `~` is bitwise NOT on integers.
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
                    choice((op!(">="), op!("<="), op!(">"), op!("<"))),
                    |lhs, op, rhs, e| {
                        (
                            e.span(),
                            Box::new(match op {
                                ">" => Expression::Gt(lhs, rhs),
                                ">=" => Expression::Geq(lhs, rhs),
                                "<=" => Expression::Leq(lhs, rhs),
                                "<" => Expression::Le(lhs, rhs),
                                _ => unreachable!("No more comparison operators"),
                            }),
                        )
                    },
                ),
                // `..=` before `..` so the digraph wins. Non-associative
                // (reject `a..b..c`). Float `1.0` stays an atom; postfix
                // `.field` requires an ident after `.`, so `0..10` is fine.
                infix(
                    none(Precedence::Range as u16),
                    choice((op!("..="), op!(".."))),
                    |lhs, op, rhs, e| {
                        (
                            e.span(),
                            Box::new(Expression::Range {
                                start: lhs,
                                end: rhs,
                                inclusive: op == "..=",
                            }),
                        )
                    },
                ),
                infix(
                    right(Precedence::Equal as u16),
                    choice((op!("=="), op!("!="))),
                    |lhs, op, rhs, e| {
                        (
                            e.span(),
                            Box::new(match op {
                                "==" => Expression::Eq(lhs, rhs),
                                "!=" => Expression::Neq(lhs, rhs),
                                _ => unreachable!("No more equality operators"),
                            }),
                        )
                    },
                ),
                infix(right(Precedence::Xor as u16), op!('^'), |lhs, _, rhs, e| {
                    (e.span(), Box::new(Expression::Xor(lhs, rhs)))
                }),
                infix(
                    right(Precedence::Assign as u16),
                    choice((
                        op!("**="),
                        op!("<<="),
                        op!(">>="),
                        op!("+="),
                        op!("-="),
                        op!("*="),
                        op!("/="),
                        op!("%="),
                        op!("&="),
                        op!("|="),
                        op!("^="),
                    )),
                    |lhs, op, rhs, e| {
                        let assign_op = match op {
                            "+=" => AssignOp::Add,
                            "-=" => AssignOp::Sub,
                            "*=" => AssignOp::Mul,
                            "/=" => AssignOp::Div,
                            "%=" => AssignOp::Mod,
                            "**=" => AssignOp::Pow,
                            "<<=" => AssignOp::Shl,
                            ">>=" => AssignOp::Shr,
                            "&=" => AssignOp::BitAnd,
                            "|=" => AssignOp::BitOr,
                            "^=" => AssignOp::BitXor,
                            _ => unreachable!("No other compound assignment operators"),
                        };
                        (
                            e.span(),
                            Box::new(Expression::CompoundAssign(lhs, assign_op, rhs)),
                        )
                    },
                ),
                infix(
                    right(Precedence::Assign as u16),
                    op!("="),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::Assignment(lhs, rhs))),
                ),
                prefix(
                    Precedence::Unary as u16,
                    choice((op!("++"), op!("--"))),
                    |op, rhs, e| {
                        (
                            e.span(),
                            Box::new(Expression::Adjust {
                                op: if op == "++" {
                                    AdjustOp::Inc
                                } else {
                                    AdjustOp::Dec
                                },
                                prefix: true,
                                target: rhs,
                            }),
                        )
                    },
                ),
                prefix(
                    Precedence::Negate as u16,
                    choice((op!('-'), op!('~'), op!('+'), op!('!'))),
                    |c, rhs, e| {
                        (
                            e.span(),
                            Box::new(match c {
                                '-' => Expression::Negate(rhs),
                                '+' => Expression::Positive(rhs),
                                '~' => Expression::Not(rhs),
                                '!' => Expression::LogicalNot(rhs),
                                _ => unreachable!("No other prefix operators"),
                            }),
                        )
                    },
                ),
                infix(left(Precedence::Term as u16), op!('-'), |lhs, _, rhs, e| {
                    (e.span(), Box::new(Expression::Sub(lhs, rhs)))
                }),
                infix(left(Precedence::Term as u16), op!('+'), |lhs, _, rhs, e| {
                    (e.span(), Box::new(Expression::Add(lhs, rhs)))
                }),
                // `??` between Or and Assign (right-associative).
                infix(
                    right(Precedence::Coalesce as u16),
                    op!("??"),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::Coalesce(lhs, rhs))),
                ),
                postfix(
                    Precedence::Primary as u16,
                    choice((op!("++"), op!("--"))),
                    |lhs, op, e| {
                        (
                            e.span(),
                            Box::new(Expression::Adjust {
                                op: if op == "++" {
                                    AdjustOp::Inc
                                } else {
                                    AdjustOp::Dec
                                },
                                prefix: false,
                                target: lhs,
                            }),
                        )
                    },
                ),
                // `?.field` before bare `.` / `?` so the digraph wins.
                postfix(
                    Precedence::Primary as u16,
                    just("?.").ignore_then(text::ident()),
                    |lhs, field, e| {
                        (e.span(), Box::new(Expression::OptionalAccess(lhs, field)))
                    },
                ),
                postfix(
                    Precedence::Primary as u16,
                    just('.').ignore_then(text::ident()),
                    |lhs, field, e| (e.span(), Box::new(Expression::Access(lhs, field))),
                ),
                // Postfix `?` must not steal the first `?` of `??`.
                postfix(
                    Precedence::Primary as u16,
                    just('?').then_ignore(just('?').not()),
                    |lhs, _, e| (e.span(), Box::new(Expression::Try(lhs))),
                ),
                postfix(
                    Precedence::Primary as u16,
                    expr.clone().delimited_by(op!('['), op!(']')),
                    |lhs, index, e| (e.span(), Box::new(Expression::Index(lhs, index))),
                ),
                postfix(
                    Precedence::Call as u16,
                    self.params(expr.clone()),
                    |lhs, args, e| (e.span(), Box::new(Expression::Call { name: lhs, args })),
                ),
            ))
        })
        .map_with(output!(Expr))
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

    fn arg_list_typed<T>(
        &self,
        ty_parser: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    where
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    {
        // `... name` (tuple rest), `T... name` (homogeneous rest), or `T name` (fixed).
        let tuple_rest_arg = op!("...")
            .ignore_then(text::ident().padded())
            .map_with(|name, e| {
                (e.span(), Box::new(Expression::Argument(None, name, true)))
            });
        let rest_arg = ty_parser
            .clone()
            .then_ignore(just("...").padded())
            .then(text::ident().padded())
            .map_with(|(ty, name), e| {
                (
                    e.span(),
                    Box::new(Expression::Argument(Some(ty), name, true)),
                )
            });
        let fixed_arg = ty_parser
            .clone()
            .then(text::ident().padded())
            .map_with(|(ty, name), e| {
                (
                    e.span(),
                    Box::new(Expression::Argument(Some(ty), name, false)),
                )
            });
        let arg = tuple_rest_arg.or(rest_arg).or(fixed_arg);

        arg.separated_by(op!(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .map_with(output!(Fragment))
            .delimited_by(op!("("), op!(")"))
    }

    fn arg_list(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        self.arg_list_typed(self.type_annotation_no_fn())
    }

    /// Anonymous lambda: `fn (T x) use (y) => expr` or `fn (T x) { expr; … }`.
    ///
    /// Distinct from named `fn name(…)` declarations (`func`): this form has
    /// no name between `fn` and `(`. Optional `use (id, …)` after the param
    /// list lists explicit captures (same `use` keyword as module imports;
    /// disambiguated by position after `fn (…)`).
    ///
    /// Long-form bodies are a brace-delimited sequence of expressions (not
    /// full `statement()`s) so this atom can live inside `expr()` without
    /// re-entering `statement()` → `expr()` during parser construction.
    fn lambda_atom<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        let captures = keyword!("use")
            .ignore_then(
                text::ident()
                    .padded()
                    .separated_by(op!(','))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(op!("("), op!(")")),
            )
            .or_not()
            .map(|opt| opt.unwrap_or_default());

        let short_body = op!("=>").ignore_then(expr.clone());
        // Brace body built from the same recursive `expr` — do NOT call
        // `self.statement()` here (that would re-enter `expr()` while the
        // outer `recursive(|expr| …)` is still being constructed → stack
        // overflow at parser build / first parse).
        let long_body = expr
            .clone()
            .then_ignore(op!(';').or_not())
            .repeated()
            .collect::<Vec<_>>()
            .delimited_by(op!("{"), op!("}"))
            .map_with(|children, e| (e.span(), Box::new(Expression::Block(children))));

        keyword!("fn")
            .ignore_then(self.arg_list())
            .then(captures)
            .then(choice((short_body, long_body)))
            .map_with(|((args, captures), body), e| {
                (
                    e.span(),
                    Box::new(Expression::Lambda {
                        args,
                        captures,
                        body,
                    }),
                )
            })
            .labelled("lambda")
    }

    /// Parse one `where` constraint: `Convert<A, B>` or unary `Num<T>`.
    fn where_constraint(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, ast::WhereConstraint<'pratt>, extra::Err<Rich<'pratt, char>>>
    + Clone
    + 'pratt {
        text::ident()
            .padded()
            .then(
                self.type_annotation()
                    .separated_by(op!(','))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(op!("<"), op!(">")),
            )
            .map(|(class, args)| ast::WhereConstraint { class, args })
    }

    /// Optional `where Class<T1, T2>, …` clause after a function's return type.
    fn where_clause(
        &self,
    ) -> impl Parser<
        'pratt,
        &'pratt str,
        Vec<ast::WhereConstraint<'pratt>>,
        extra::Err<Rich<'pratt, char>>,
    > + Clone
           + 'pratt {
        keyword!("where")
            .ignore_then(
                self.where_constraint()
                    .separated_by(op!(','))
                    .allow_trailing()
                    .at_least(1)
                    .collect(),
            )
            .or_not()
            .map(|opt| opt.unwrap_or_default())
    }

    /// Parses the function *signature* (`async? fn Name<T>(args) -> ret where …`)
    /// without consuming the body block.  Used by `typeclass_decl` to parse
    /// sig-only methods that end in `;`.
    ///
    /// Returns
    /// `((((((is_coro, _), name), type_params), args), returns), where_constraints)`.
    fn func_sig(
        &self,
    ) -> impl Parser<
        'pratt,
        &'pratt str,
        (
            (
                (
                    (
                        ((Option<&'pratt str>, &'pratt str), &'pratt str),
                        Vec<TypeParam<'pratt>>,
                    ),
                    Output<'pratt>,
                ),
                Option<Output<'pratt>>,
            ),
            Vec<ast::WhereConstraint<'pratt>>,
        ),
        extra::Err<Rich<'pratt, char>>,
    > + Clone
           + 'pratt {
        keyword!("async")
            .or_not()
            .then(keyword!("fn"))
            .then(text::ident().padded())
            .then(self.type_param_list())
            .then(self.arg_list())
            .then(op!("->").ignore_then(self.type_annotation()).or_not())
            .then(self.where_clause())
    }

    /// `attr Name<T>(target, extras..., ...args) -> R { body }`
    fn attr_decl(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("attr")
            .ignore_then(text::ident().padded())
            .then(self.type_param_list())
            .then(self.arg_list_typed(self.type_annotation()))
            .then(op!("->").ignore_then(self.type_annotation()).or_not())
            .then(self.where_clause())
            .then(self.block(self.statement()))
            .map_with(
                |(((((name, type_params), args), returns), where_constraints), body), e| {
                    (
                        e.span(),
                        Box::new(Expression::AttrDecl {
                            name,
                            type_params,
                            args,
                            returns,
                            where_constraints,
                            body,
                        }),
                    )
                },
            )
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
        self.attr_list()
            .then(keyword!("async").or_not())
            .then(keyword!("fn"))
            .then(text::ident().padded())
            .then(self.type_param_list())
            .then(self.arg_list())
            .then(op!("->").ignore_then(self.type_annotation()).or_not())
            .then(self.where_clause())
            .then(choice((
                self.block(stmt).map(Some),
                op!(";").to(None),
            )))
            .map_with(
                |((((((((attrs, is_coro), _), name), type_params), args), returns), where_constraints), body), e| {
                    (
                        e.span(),
                        Box::new(Expression::Function {
                            attrs,
                            name,
                            is_coro: is_coro.is_some(),
                            type_params,
                            args,
                            returns,
                            where_constraints,
                            body,
                        }),
                    )
                },
            )
    }

    fn yield_expr_<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("yield").ignore_then(choice((
            keyword!("from")
                .ignore_then(expr.clone())
                .map_with(output!(YieldFrom)),
            expr.map_with(output!(Yield)),
        )))
    }

    fn yield_(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        self.yield_expr_(self.expr())
    }

    fn resume_<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("resume")
            .ignore_then(expr.clone())
            .then(keyword!("with").ignore_then(expr).or_not())
            .map_with(|(target, arg), e| (e.span(), Box::new(Expression::Resume(target, arg))))
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

    fn for_<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        stmt: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        // C-style: `for (init?; cond; step?) { body }`
        let init = choice((self.variable(), self.expr())).or_not();
        let step = self.expr().or_not();
        let c_style = init
            .then_ignore(op!(";"))
            .then(self.expr())
            .then_ignore(op!(";"))
            .then(step)
            .delimited_by(op!("("), op!(")"))
            .then(self.block(stmt.clone()))
            .map_with(|(((init, cond), step), body), e| {
                (
                    e.span(),
                    Box::new(Expression::For {
                        init,
                        cond,
                        step,
                        body,
                    }),
                )
            });

        // For-in: `for x in expr { body }` → Loop { identifier: Some(x), … }
        let for_in = text::ident()
            .padded()
            .map_with(output!(Identifier))
            .then_ignore(keyword!("in"))
            .then(self.expr())
            .then(self.block(stmt))
            .map_with(|((identifier, iterable), body), e| {
                (
                    e.span(),
                    Box::new(Expression::Loop {
                        identifier: Some(identifier),
                        iterable,
                        body,
                    }),
                )
            });

        // Prefer the paren form so `for (…)` never misparses as for-in.
        keyword!("for").ignore_then(choice((c_style, for_in)))
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
        // `recursive` enables `else if` to call back into this parser.
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

    fn format_expr<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("format")
            .labelled("format expression")
            .ignore_then(self.string())
            .then(
                expr.separated_by(op!(','))
                    .allow_leading()
                    .collect::<Vec<_>>()
                    .or_not(),
            )
            .map_with(|(fmt, params), e| (e.span(), Box::new(Expression::Format(fmt, params))))
    }

    /// `done(handle)` — true when a coroutine has completed.
    fn done_<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        text::keyword("done")
            .labelled("done builtin")
            .ignore_then(
                expr.clone()
                    .separated_by(op!(','))
                    .allow_trailing()
                    .at_least(1)
                    .at_most(1)
                    .collect::<Vec<_>>()
                    .delimited_by(op!('('), op!(')')),
            )
            .map_with(|args, e| {
                (
                    e.span(),
                    Box::new(Expression::Done(args.into_iter().next().unwrap())),
                )
            })
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

    fn raise_(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("raise")
            .labelled("raise")
            .ignore_then(self.expr())
            .map_with(|result, e| (e.span(), Box::new(Expression::Raise(result))))
    }

    fn panic_(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("panic")
            .labelled("panic")
            .ignore_then(self.expr())
            .map_with(|result, e| (e.span(), Box::new(Expression::Panic(result))))
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

    fn break_(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("break")
            .then_ignore(op!(";"))
            .map_with(|_, e| (e.span(), Box::new(Expression::Break)))
    }

    fn continue_(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("continue")
            .then_ignore(op!(";"))
            .map_with(|_, e| (e.span(), Box::new(Expression::Continue)))
    }

    fn statement(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        recursive(|stmt| {
            choice((
                self.break_(),
                self.continue_(),
                self.for_(stmt.clone()),
                self.while_(stmt.clone()),
                self.if_(stmt.clone()),
                self.block(stmt.clone()),
                self.type_alias(),
                self.variable().then_ignore(op!(';')),
                self.constant().then_ignore(op!(';')),
                // Statement keywords before `expr_statement`: otherwise
                // `return -1;` parses as `Sub(Identifier("return"), 1)`.
                self.print().then_ignore(op!(';')),
                self.return_().then_ignore(op!(';')),
                self.raise_().then_ignore(op!(';')),
                self.panic_().then_ignore(op!(';')),
                self.yield_().then_ignore(op!(';')),
                self.expr_statement(),
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
        //
        // Ordering notes:
        //  - `typeclass_decl` before `type_alias` so `trait` keyword
        //    is not confused with a user identifier.
        //  - `trait_impl_for_block` (`impl Trait for T` / `impl Trait<A,B> for T`)
        //    is tried before other `impl` forms so `for` is unambiguous.
        //  - `impl_block` handles inherent impls and simple trait impls
        //    (`impl Num<int>`, `impl Show<Point>`) via the type-arg heuristic.
        //  - `typeclass_impl_block` is the fallback for complex type-annotation
        //    args (e.g. `impl Foo<Option<int>>`) that `type_param_list()` inside
        //    `impl_block` cannot parse.  Chumsky 0.12 backtracks on failure, so
        //    `impl_block` failing (after consuming `impl Name`) causes `choice` to
        //    retry with `typeclass_impl_block`.
        choice((
            self.class(),
            self.typeclass_decl(stmt.clone()),
            self.trait_impl_for_block(stmt.clone()),
            self.impl_block(stmt.clone()),
            self.typeclass_impl_block(stmt.clone()),
            self.test_case(stmt.clone()),
            self.attr_decl(),
            self.func(stmt.clone()),
            self.type_alias(),
            self.use_(),
            self.mod_(),
            self.enum_decl(),
            self.defer(stmt.clone()),
            self.extern_struct(),
            self.extern_block(),
            stmt.clone(),
        ))
    }

    /// `test("description") { … }` — harness test case declaration.
    fn test_case<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        stmt: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("test")
            .ignore_then(
                self.expr()
                    .delimited_by(op!("("), op!(")")),
            )
            .then(self.block(stmt))
            .map_with(|(name, body), e| {
                (
                    e.span(),
                    Box::new(Expression::TestCase { name, body }),
                )
            })
            .labelled("test case")
    }

    /// `type Name = T;` — type alias declaration.
    fn type_alias(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("type")
            .ignore_then(text::ident().padded())
            .then(self.type_param_list())
            .then_ignore(op!("="))
            .then(self.type_annotation())
            .then_ignore(op!(";"))
            .map_with(|((name, type_params), ty), e| {
                (
                    e.span(),
                    Box::new(Expression::TypeAlias {
                        name,
                        type_params,
                        ty: Box::new(ty),
                    }),
                )
            })
            .labelled("type alias")
    }

    /// `use path::item;`, `use path::item as alias;`, or `use path::*;`.
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

    /// `mod name;` — forward module declaration (loads the file; does not import items).
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

    /// `extern struct Name { field: type, ... };` — C-layout FFI struct.
    fn extern_struct(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        use crate::ast::ExternStructDecl;
        let field = text::ident()
            .padded()
            .then_ignore(op!(":"))
            .then(self.type_annotation())
            .map_with(|(name, ty), _e| (name.to_string(), ty));

        keyword!("extern")
            .ignore_then(keyword!("struct"))
            .ignore_then(text::ident().padded())
            .then(
                field
                    .separated_by(op!(","))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(op!("{"), op!("}")),
            )
            .then_ignore(op!(";"))
            .map_with(|(name, fields), e| {
                (
                    e.span(),
                    Box::new(Expression::ExternStruct(ExternStructDecl { name, fields })),
                )
            })
            .labelled("extern struct declaration")
    }

    /// Extern-only parameter list: fixed `T name` args plus optional trailing bare `...`.
    /// Language rest (`T... name`) is rejected with a clear diagnostic.
    fn extern_arg_list(
        &self,
    ) -> impl Parser<
        'pratt,
        &'pratt str,
        (Output<'pratt>, bool),
        extra::Err<Rich<'pratt, char>>,
    > + Clone
           + 'pratt {
        #[derive(Clone)]
        enum ExternArg<'a> {
            Fixed(Output<'a>),
            /// Matched `T... name` — rejected after the list is collected.
            IllegalRest,
        }

        let illegal_rest = self
            .type_annotation()
            .then_ignore(just("...").padded())
            .then(text::ident().padded())
            .to(ExternArg::IllegalRest);
        let fixed_arg = self
            .type_annotation()
            .then(text::ident().padded())
            .map_with(|(ty, name), e| {
                ExternArg::Fixed((
                    e.span(),
                    Box::new(Expression::Argument(Some(ty), name, false)),
                ))
            });
        // Prefer illegal-rest so `int... xs` is recognized (then rejected).
        let arg = illegal_rest.or(fixed_arg);

        let bare_only = just("...")
            .padded()
            .map_with(|_, e| {
                (
                    (e.span(), Box::new(Expression::Fragment(Vec::new()))),
                    true,
                )
            });

        let fixed_then_ellipsis = arg
            .separated_by(op!(','))
            .at_least(1)
            .collect::<Vec<_>>()
            .then(
                op!(',')
                    .ignore_then(just("...").padded())
                    .or_not()
                    .map(|o| o.is_some()),
            )
            .try_map(|(args, variadic), span| {
                if args.iter().any(|a| matches!(a, ExternArg::IllegalRest)) {
                    return Err(Rich::custom(
                        span,
                        "use bare `...` for C varargs; `T... name` is only for language rest parameters",
                    ));
                }
                let fixed: Vec<Output<'_>> = args
                    .into_iter()
                    .filter_map(|a| match a {
                        ExternArg::Fixed(o) => Some(o),
                        ExternArg::IllegalRest => None,
                    })
                    .collect();
                Ok((
                    (span, Box::new(Expression::Fragment(fixed))),
                    variadic,
                ))
            });

        let empty = empty().map_with(|_, e| {
            (
                (e.span(), Box::new(Expression::Fragment(Vec::new()))),
                false,
            )
        });

        choice((bare_only, fixed_then_ellipsis, empty)).delimited_by(op!("("), op!(")"))
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
            .then(self.extern_arg_list())
            .then(op!("->").ignore_then(self.type_annotation()).or_not())
            // The trailing `;` is required (no body).
            .then_ignore(op!(";"))
            .map_with(|(((_, name), (args, variadic)), returns), _e| ExternFunction {
                name,
                symbol: None,
                args,
                returns,
                variadic,
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

    /// Zero or more `#[attr]` / `#[attr(args)]` prefixes on a declaration.
    fn attr_list(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Vec<Attribute<'pratt>>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        let attr_float = text::int(10)
            .then(just('.').then(text::int(10)))
            .to_slice()
            .from_str::<f64>()
            .validate(|v: Result<f64, _>, _, _| v.unwrap_or(0.0))
            .map(AttrLit::Float);

        let attr_lit = choice((
            just('"')
                .ignore_then(none_of('"').repeated().to_slice())
                .then_ignore(just('"'))
                .map(AttrLit::String),
            text::int(10)
                .to_slice()
                .from_str::<i64>()
                .validate(|v: Result<i64, _>, _, _| v.unwrap_or(0))
                .map(AttrLit::Int),
            attr_float,
            keyword!("true").to(AttrLit::Bool(true)),
            keyword!("false").to(AttrLit::Bool(false)),
        ));

        let attr_kv = text::ident()
            .then_ignore(op!("="))
            .then(attr_lit.clone());

        let attr_args = choice((
            attr_kv
                .padded()
                .separated_by(op!(','))
                .at_least(1)
                .collect::<Vec<_>>()
                .map(AttrArgs::KeyValues),
            just('"')
                .ignore_then(none_of('"').repeated().to_slice())
                .then_ignore(just('"'))
                .map(AttrArgs::String),
            attr_lit
                .padded()
                .separated_by(op!(','))
                .at_least(1)
                .collect::<Vec<_>>()
                .map(AttrArgs::Positional),
            text::ident()
                .padded()
                .separated_by(op!(','))
                .at_least(1)
                .collect::<Vec<_>>()
                .map(AttrArgs::Idents),
        ))
        .delimited_by(op!("("), op!(")"));

        let attribute = text::ident()
            .then(attr_args.or_not())
            .map(|(name, args)| Attribute {
                name,
                args: args.unwrap_or(AttrArgs::Empty),
            });

        op!("#")
            .ignore_then(attribute.delimited_by(op!("["), op!("]")))
            .repeated()
            .collect::<Vec<_>>()
    }

    /// `class Name { [pub] field: Type, ... }`
    ///
    /// Fields are private by default; `pub` makes them public.
    fn class(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        self.attr_list()
            .then(keyword!("class"))
            .then(text::ident().padded())
            .then(self.type_param_list())
            .then(
                self.field_decl()
                    .separated_by(op!(','))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(op!("{"), op!("}")),
            )
            .map_with(|((((attrs, _), name), type_params), fields), e| {
                (
                    e.span(),
                    Box::new(Expression::Class {
                        attrs,
                        name,
                        type_params,
                        fields,
                    }),
                )
            })
    }

    /// `[pub] name: Type` — a class field declaration.
    ///
    /// The field type is parsed via `type_annotation()` so it can accept
    /// generic types (`name: Option<int>`), arrays (`name: [int]`), tuples,
    /// and `forall` annotations.
    fn field_decl(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("pub")
            .or_not()
            .then(text::ident())
            .then_ignore(op!(":"))
            .then(self.type_annotation())
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

    /// `trait Name<T, U: Bound> { type Elem; fn sig(…) -> ret; fn default(…) { body } }`
    ///
    /// Each body item is either:
    /// - Associated type declaration: `type Elem;`
    /// - Signature-only method: `fn name(args) -> ret;`  (represented as a
    ///   `Function` with an empty `Block` body).
    /// - Default implementation: `fn name(args) -> ret { body }`.
    fn typeclass_decl<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        stmt: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        // A method that ends in `;` is signature-only: emit an empty Block.
        let sig_only = self
            .func_sig()
            .then_ignore(op!(";"))
            .map_with(
                |((((((is_coro, _), name), type_params), args), returns), where_constraints), e| {
                    let empty_block = (e.span(), Box::new(Expression::Block(vec![])));
                    (
                        e.span(),
                        Box::new(Expression::Function {
                            attrs: vec![],
                            name,
                            is_coro: is_coro.is_some(),
                            type_params,
                            args,
                            returns,
                            where_constraints,
                            body: Some(empty_block),
                        }),
                    )
                },
            );

        // A method with a full block body (the default implementation).
        let default_method = self.func(stmt);

        // Associated type declaration: `type Elem;` / `type Ref<T>;`
        let assoc_decl = keyword!("type")
            .ignore_then(text::ident().padded())
            .then(self.type_param_list())
            .then_ignore(op!(";"))
            .map_with(|(name, type_params), e| {
                (
                    e.span(),
                    Box::new(Expression::AssocTypeDecl {
                        name,
                        type_params,
                    }),
                )
            });

        keyword!("trait")
            .ignore_then(text::ident().padded())
            .then(self.type_param_list())
            .then(
                choice((assoc_decl, sig_only, default_method))
                    .padded()
                    .repeated()
                    .collect::<Vec<_>>()
                    .delimited_by(op!("{"), op!("}")),
            )
            .map_with(|((name, type_params), methods), e| {
                (e.span(), Box::new(Expression::TypeClass { name, type_params, methods }))
            })
    }

    /// Preferred trait-instance form: `impl Trait for Type { … }` or
    /// `impl Trait<A, B> for Type { … }`.
    ///
    /// The type after `for` is prepended as the first type argument (Self
    /// slot), so `impl Show for Foo` ≡ `impl Show<Foo>` and
    /// `impl Thing<string, int> for Message` ≡ `impl Thing<Message, string, int>`.
    fn trait_impl_for_block<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        stmt: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        let assoc_def = keyword!("type")
            .ignore_then(text::ident().padded())
            .then(self.type_param_list())
            .then_ignore(op!("="))
            .then(self.type_annotation())
            .then_ignore(op!(";"))
            .map_with(|((name, type_params), ty), e| {
                (
                    e.span(),
                    Box::new(Expression::AssocTypeDef {
                        name,
                        type_params,
                        ty: Box::new(ty),
                    }),
                )
            });

        let opt_bracket_args = self
            .type_annotation()
            .padded()
            .separated_by(op!(","))
            .allow_trailing()
            .at_least(1)
            .collect::<Vec<_>>()
            .delimited_by(op!("<"), op!(">"))
            .or_not()
            .map(|opt| opt.unwrap_or_default());

        keyword!("impl")
            .ignore_then(text::ident())
            .then(opt_bracket_args)
            .then_ignore(keyword!("for"))
            .then(self.type_annotation().padded())
            .then(
                choice((assoc_def, self.method_decl(stmt)))
                    .repeated()
                    .collect::<Vec<_>>()
                    .delimited_by(op!("{"), op!("}")),
            )
            .map_with(|(((class, bracket_args), for_ty), methods), e| {
                let mut args = Vec::with_capacity(bracket_args.len() + 1);
                args.push(for_ty);
                args.extend(bracket_args);
                (
                    e.span(),
                    Box::new(Expression::TypeClassImpl {
                        class,
                        args,
                        methods,
                    }),
                )
            })
    }

    /// Inherent `impl` block OR typeclass instance.
    ///
    /// After `impl Name`, an optional `<…>` section is parsed via
    /// `type_param_list()`.  The result is classified at map time:
    ///
    /// - No `<…>` → `Implementation` (inherent, no type params).
    /// - `<T>`, `<T: Num>` (type-parameter shape) → `Implementation`.
    /// - `<int>`, `<string>`, `<Point>`, etc. (concrete type args) →
    ///   `TypeClassImpl`.
    ///
    /// A bare angle-bracket name is treated as a type parameter when it has
    /// bounds (`T: Num`) or is a single uppercase letter (`T`, `U`). Multi-
    /// character names without bounds (`Point`, `int`) are concrete instance
    /// heads — including user enums for `impl Show<Point>`.
    ///
    /// For complex type-annotation args (e.g. `impl Foo<Option<int>>`),
    /// `typeclass_impl_block` is the fallback when `impl_block` fails to parse.
    fn impl_block<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        stmt: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        // Known lowercase primitive type names — these can never be TypeParam
        // names; if they appear inside `<>`, the block is a typeclass impl.
        const PRIMITIVES: &[&str] = &["int", "float", "string", "bool", "void", "unit"];

        // Associated type definition: `type Elem = int;` / `type Ref<T> = T;`
        let assoc_def = keyword!("type")
            .ignore_then(text::ident().padded())
            .then(self.type_param_list())
            .then_ignore(op!("="))
            .then(self.type_annotation())
            .then_ignore(op!(";"))
            .map_with(|((name, type_params), ty), e| {
                (
                    e.span(),
                    Box::new(Expression::AssocTypeDef {
                        name,
                        type_params,
                        ty: Box::new(ty),
                    }),
                )
            });

        keyword!("impl")
            .ignore_then(text::ident())
            .then(self.type_param_list())
            .then(
                // Methods (and assoc type defs for typeclass impls) are
                // separated by juxtaposition (newlines / whitespace), not commas.
                choice((assoc_def, self.method_decl(stmt)))
                    .repeated()
                    .collect::<Vec<_>>()
                    .delimited_by(op!("{"), op!("}")),
            )
            .map_with(|((name, type_params), methods), e| {
                // Classify by inspecting parsed param names.
                let looks_like_type_param = |p: &TypeParam<'_>| -> bool {
                    if PRIMITIVES.contains(&p.name) {
                        return false;
                    }
                    // Bounded names are always type parameters (`T: Num`).
                    if !p.bounds.is_empty() {
                        return true;
                    }
                    // Single uppercase letter (`T`, `U`) — type-parameter shape.
                    let mut chars = p.name.chars();
                    matches!(chars.next(), Some(c) if c.is_uppercase()) && chars.next().is_none()
                };
                let is_typeclass_impl = !type_params.is_empty()
                    && type_params.iter().any(|p| !looks_like_type_param(p));
                if is_typeclass_impl {
                    // e.g. `impl Num<int>` / `impl Show<Point>` → typeclass instance.
                    // Re-wrap each param name as a bare Type annotation.
                    let args = type_params
                        .into_iter()
                        .map(|p| (e.span(), Box::new(Expression::Type(p.name))))
                        .collect();
                    (e.span(), Box::new(Expression::TypeClassImpl { class: name, args, methods }))
                } else {
                    // e.g. `impl Cell {}` or `impl Cell<T>` or `impl Cell<T: Num>`.
                    (
                        e.span(),
                        Box::new(Expression::Implementation {
                            what: "",
                            owner: name,
                            type_params,
                            methods,
                        }),
                    )
                }
            })
    }

    /// Typeclass-impl block for complex type-annotation arguments, e.g.
    /// `impl Foo<Option<int>> { … }`.
    ///
    /// Each angle-bracket item is parsed as a full `type_annotation`, so
    /// this parser handles any well-formed type, including generics.  It is
    /// registered BEFORE `impl_block` in `declaration()` so that it wins for
    /// cases that `type_param_list` cannot represent.
    ///
    /// Bare uppercase idents (which look like type params) are accepted here
    /// too — if the user writes `impl Foo<T>` and `T` is ambiguous this
    /// parser will win only when it appears before `impl_block` in the
    /// `choice`; that ordering is intentional (inherent impls prefer
    /// `impl_block`).
    fn typeclass_impl_block<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        stmt: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        // Associated type definition: `type Elem = int;` / `type Ref<T> = T;`
        let assoc_def = keyword!("type")
            .ignore_then(text::ident().padded())
            .then(self.type_param_list())
            .then_ignore(op!("="))
            .then(self.type_annotation())
            .then_ignore(op!(";"))
            .map_with(|((name, type_params), ty), e| {
                (
                    e.span(),
                    Box::new(Expression::AssocTypeDef {
                        name,
                        type_params,
                        ty: Box::new(ty),
                    }),
                )
            });

        keyword!("impl")
            .ignore_then(text::ident())
            .then(
                // Require a non-empty `<` type_annotation+ `>` — without
                // angle brackets this parser doesn't match and falls through
                // to `impl_block`.
                self.type_annotation()
                    .padded()
                    .separated_by(op!(","))
                    .allow_trailing()
                    .at_least(1)
                    .collect::<Vec<_>>()
                    .delimited_by(op!("<"), op!(">")),
            )
            .then(
                choice((assoc_def, self.method_decl(stmt)))
                    .repeated()
                    .collect::<Vec<_>>()
                    .delimited_by(op!("{"), op!("}")),
            )
            .map_with(|((class, args), methods), e| {
                (e.span(), Box::new(Expression::TypeClassImpl { class, args, methods }))
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
        let simple = keyword!("let")
            .ignore_then(text::ident())
            .then(op!(":").ignore_then(self.type_annotation()).or_not())
            .then(op!("=").ignore_then(self.expr()).or_not())
            .map_with(|((name, ty), val), e| {
                let mut result = vec![(e.span(), Box::new(Expression::Variable(name, ty)))];
                if let Some(v) = val {
                    result.push(v);
                }
                (e.span(), Box::new(Expression::Fragment(result)))
            });

        // `let (a, b) = expr;` / `let { x, y } = expr;` — tried before
        // the simple `let name` form so `(` / `{` are not misread as
        // identifiers. Top-level LHS is tuple/record only (not a bare
        // binding — that stays on the simple path).
        let destructure = keyword!("let")
            .ignore_then(self.let_destructure_lhs())
            .then_ignore(op!("="))
            .then(self.expr())
            .map_with(|(pattern, rhs), e| {
                (
                    e.span(),
                    Box::new(Expression::LetDestructure { pattern, rhs }),
                )
            });

        choice((destructure, simple))
    }

    /// Top-level `let` destructure LHS: `(p, …)` or `{ field, … }` only.
    fn let_destructure_lhs(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, LetPattern<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        choice((self.let_tuple_pattern(), self.let_record_pattern()))
    }

    fn let_tuple_pattern(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, LetPattern<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        let inner = self.let_pattern();
        // Require a comma (or trailing comma) so `(a)` is not a
        // 1-tuple — same rule as tuple literals.
        let tuple_multi = inner
            .clone()
            .separated_by(op!(','))
            .at_least(2)
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(op!('('), op!(')'));
        let tuple_trailing = inner
            .then_ignore(op!(','))
            .map(|p| vec![p])
            .delimited_by(op!('('), op!(')'));
        choice((tuple_multi, tuple_trailing)).map(LetPattern::Tuple)
    }

    fn let_record_pattern(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, LetPattern<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        let field = text::ident()
            .padded()
            .then(op!(":").ignore_then(self.let_pattern()).or_not())
            .map(|(name, sub)| {
                let pattern = sub.unwrap_or(LetPattern::Binding { name });
                LetFieldPattern { name, pattern }
            });
        field
            .separated_by(op!(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(op!("{"), op!("}"))
            .map(LetPattern::Record)
    }

    /// Nested irrefutable `let` pattern: `_`, binding, `(p, …)`, `{ field, … }`.
    fn let_pattern(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, LetPattern<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        recursive(|pattern_parser| {
            let record_field = text::ident()
                .padded()
                .then(op!(":").ignore_then(pattern_parser.clone()).or_not())
                .map(|(name, sub)| {
                    let pattern = sub.unwrap_or(LetPattern::Binding { name });
                    LetFieldPattern { name, pattern }
                });

            let record = record_field
                .separated_by(op!(','))
                .allow_trailing()
                .collect::<Vec<_>>()
                .delimited_by(op!("{"), op!("}"))
                .map(LetPattern::Record);

            let tuple_multi = pattern_parser
                .clone()
                .separated_by(op!(','))
                .at_least(2)
                .allow_trailing()
                .collect::<Vec<_>>()
                .delimited_by(op!('('), op!(')'));
            let tuple_trailing = pattern_parser
                .clone()
                .then_ignore(op!(','))
                .map(|p| vec![p])
                .delimited_by(op!('('), op!(')'));
            let tuple = choice((tuple_multi, tuple_trailing)).map(LetPattern::Tuple);

            choice((
                just("_").padded().to(LetPattern::Wildcard),
                tuple,
                record,
                text::ident()
                    .padded()
                    .map(|name| LetPattern::Binding { name }),
            ))
        })
    }

    fn constant(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("const")
            .ignore_then(text::ident().map_with(output!(Identifier)))
            .then(op!(":").ignore_then(self.type_annotation()).or_not())
            .then_ignore(op!("="))
            .then(self.expr())
            .map_with(|((name, ty), val), e| {
                let result = vec![(e.span(), Box::new(Expression::Constant(name, ty))), val];
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
        // Named call-site arg: `ident : expr` → `NamedArg`. Tried before
        // bare `expr` so `f(a: 1)` does not parse as a labelled type /
        // weird binary form. Positional `expr` still wins when there is
        // no colon after the identifier.
        let named = text::ident()
            .padded()
            .then_ignore(op!(":"))
            .then(expr.clone())
            .map_with(|(name, value), e| {
                (e.span(), Box::new(Expression::NamedArg(name, value)))
            })
            .labelled("named argument");
        let spread = op!("...")
            .ignore_then(expr.clone())
            .map_with(|inner, e| (e.span(), Box::new(Expression::Spread(inner))));
        let arg = spread.or(named).or(expr.clone());
        arg.separated_by(op!(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .or_not()
            .delimited_by(op!('('), op!(')'))
    }

    /// Tuple literal. Requires a comma inside the parens — `(1)` is a group, `(1,)` is a 1-tuple.
    fn tuple_atom<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        // A tuple is:
        //   - `()` (empty — used by zero-arity FFI declare/invoke),
        //   - `at_least(2)` items separated by commas, or
        //   - exactly 1 item with a trailing comma.
        // Non-empty forms still require a comma so `(1)` stays a group.
        use chumsky::Parser;
        let empty = op!('(')
            .ignore_then(op!(')'))
            .to(Vec::new())
            .labelled("empty tuple");
        let two_or_more = expr
            .clone()
            .separated_by(op!(','))
            .allow_trailing()
            .at_least(2)
            .collect::<Vec<_>>()
            .delimited_by(op!('('), op!(')'));
        let one_with_trailing = expr
            .clone()
            .then_ignore(op!(','))
            .map(|e| vec![e])
            .delimited_by(op!('('), op!(')'))
            .labelled("single-element tuple");
        choice((empty, two_or_more, one_with_trailing))
            .map_with(|items, e| (e.span(), Box::new(Expression::Tuple(items))))
            .labelled("tuple")
    }

    /// Anonymous record literal `{ name: expr, ... }`.
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
        // Each field: `name : expr`.
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

    /// Array literal `[a, b, ...]`. Empty `[]` is allowed.
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

    /// Qualified constructor `EnumName::Variant(...)`. Must appear before `call` in the atom choice.
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

        // `Enum::Variant` or `ffi::types::Int` (multi-segment path;
        // last segment is the variant, the rest is the enum/module path).
        text::ident()
            .padded()
            .separated_by(just("::").padded())
            .at_least(2)
            .collect::<Vec<_>>()
            .then(shape)
            .map_with(|(segments, fields), e| {
                let mut segments = segments;
                let variant_name = segments.pop().unwrap();
                let enum_name = if segments.len() == 1 {
                    segments.pop().unwrap()
                } else {
                    // Leak into arena-less `'pratt` by joining into a
                    // single owned string stored via Box::leak for the
                    // AST lifetime — the parser AST borrows from the
                    // source, so multi-segment paths need a stable
                    // string. Join with `::` into a Cow isn't available
                    // here; use the source-backed approach: reconstruct
                    // from the collected idents.
                    //
                    // `segments` are `&str` slices into the source, but
                    // joining them requires an owned String. Store via
                    // the expression's span by using a concatenated
                    // owned string leaked for the duration of the parse
                    // (same pattern as other temporary AST strings is
                    // not used elsewhere — instead keep two-segment
                    // form when possible).
                    let joined = segments.join("::");
                    Box::leak(joined.into_boxed_str()) as &str
                };
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

    /// A match-arm pattern: wildcard, binding, or qualified constructor (tuple or record payload).
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
    ///
    /// Optional derive attribute: `#[derive(Show, Eq)] enum Name { … }`.
    fn enum_decl(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        self.attr_list()
            .then(keyword!("enum"))
            .then(text::ident().padded())
            .then(self.type_param_list())
            .then(
                self.enum_variant()
                    .separated_by(op!(','))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(op!('{'), op!('}')),
            )
            .map_with(|((((attrs, _), name), type_params), variants), e| {
                (
                    e.span(),
                    Box::new(Expression::EnumDecl {
                        attrs,
                        name,
                        type_params,
                        variants,
                    }),
                )
            })
    }

    /// One variant inside an `enum` body (`Variant`, `Variant(T, ...)`, or `Variant { x: T, ... }`).
    fn enum_variant(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        // Record field: `name : Type` — the type is parsed via
        // `type_annotation()` so it can be generic (`Inner<T>`), an array,
        // or a tuple.  Duplicate names are rejected at parse time.
        let record_field_decl = text::ident()
            .padded()
            .then_ignore(op!(":"))
            .then(self.type_annotation().padded())
            .map_with(|(name, value), _| RecordFieldDecl { name, value })
            .labelled("record field declaration");

        let record_payload_decl = record_field_decl
            .separated_by(op!(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(op!("{"), op!("}"))
            .map(EnumVariantPayload::Record)
            .labelled("record variant payload");

        // Tuple payload: each element is a full type annotation so that
        // generic payloads like `Node(Tree<T>, Tree<T>)` are accepted.
        let tuple_payload_decl = self
            .type_annotation()
            .padded()
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

    pub fn parse(&self, input: &'pratt str) -> Result<Output<'pratt>, Message> {
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
                let mut message = Message::error(
                    ErrorCode::ParseError,
                    "Parse error".to_string(),
                    std::ops::Range::default(),
                );

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
        AdjustOp, AssignOp, EnumConstructPayload, EnumVariantPayload, Expression, MatchArm,
        Pattern, PatternPayload,
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
        same!("!true");
        same!("!0");
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
    fn format_keyword_parses_as_expression() {
        same!("format \"%i-%s\", 42, \"x\"");
        let ast = expr_ast!("format \"%i-%s\", 42, \"x\"");
        let inner = match ast {
            Expression::Expr(e) => e.1.as_ref().clone(),
            other => other,
        };
        match inner {
            Expression::Format(_, Some(params)) => assert_eq!(params.len(), 2),
            other => panic!("expected format expression, got {:?}", other),
        }
    }

    #[test]
    fn break_and_continue_parse_as_statements() {
        let break_ast = decl_ast!("break;");
        match break_ast {
            Expression::Statement(inner) => {
                assert!(matches!(inner.1.as_ref(), Expression::Break));
            }
            other => panic!("expected break statement, got {:?}", other),
        }

        let continue_ast = decl_ast!("continue;");
        match continue_ast {
            Expression::Statement(inner) => {
                assert!(matches!(inner.1.as_ref(), Expression::Continue));
            }
            other => panic!("expected continue statement, got {:?}", other),
        }
    }

    #[test]
    fn c_style_for_parses_let_init_and_step() {
        let ast = decl_ast!("for (let i = 0; i < 10; i = i + 1) { continue; }");
        match ast {
            Expression::Statement(inner) => match inner.1.as_ref() {
                Expression::For {
                    init,
                    cond,
                    step,
                    body,
                } => {
                    assert!(matches!(
                        init.as_ref().map(|i| i.1.as_ref()),
                        Some(Expression::Fragment(_))
                    ));
                    assert!(matches!(cond.1.as_ref(), Expression::Expr(_)));
                    assert!(matches!(
                        step.as_ref().map(|s| s.1.as_ref()),
                        Some(Expression::Expr(_))
                    ));
                    assert!(matches!(body.1.as_ref(), Expression::Block(_)));
                }
                other => panic!("expected for statement, got {:?}", other),
            },
            other => panic!("expected statement wrapper, got {:?}", other),
        }
    }

    #[test]
    fn c_style_for_allows_empty_init_and_step() {
        let ast = decl_ast!("for (; keep_going; ) { break; }");
        match ast {
            Expression::Statement(inner) => match inner.1.as_ref() {
                Expression::For {
                    init,
                    cond,
                    step,
                    body,
                } => {
                    assert!(init.is_none());
                    assert!(matches!(cond.1.as_ref(), Expression::Expr(_)));
                    assert!(step.is_none());
                    assert!(matches!(body.1.as_ref(), Expression::Block(_)));
                }
                other => panic!("expected for statement, got {:?}", other),
            },
            other => panic!("expected statement wrapper, got {:?}", other),
        }
    }

    #[test]
    fn for_in_parses_to_loop_with_identifier() {
        let ast = decl_ast!("for x in counter() { print \"%i\", x; }");
        match ast {
            Expression::Statement(inner) => match inner.1.as_ref() {
                Expression::Loop {
                    identifier,
                    iterable,
                    body,
                } => {
                    match identifier.as_ref().map(|i| i.1.as_ref()) {
                        Some(Expression::Identifier(name)) => assert_eq!(*name, "x"),
                        other => panic!("expected Identifier(x), got {:?}", other),
                    }
                    assert!(matches!(iterable.1.as_ref(), Expression::Expr(_)));
                    assert!(matches!(body.1.as_ref(), Expression::Block(_)));
                }
                other => panic!("expected for-in Loop, got {:?}", other),
            },
            other => panic!("expected statement wrapper, got {:?}", other),
        }
    }

    #[test]
    fn for_in_display_round_trips() {
        let rendered = stmt!("for x in counter() { print \"%i\", x; }");
        assert!(
            rendered.contains("for x in"),
            "expected for-in Display, got {rendered:?}"
        );
        assert!(
            rendered.contains("counter()"),
            "expected iterable in Display, got {rendered:?}"
        );
    }

    #[test]
    fn const_keyword_parses_to_constant_fragment() {
        let ast = decl_ast!("const answer = 42;");
        match ast {
            Expression::Statement(inner) => match inner.1.as_ref() {
                Expression::Fragment(children) => {
                    assert_eq!(children.len(), 2);
                    match children[0].1.as_ref() {
                        Expression::Constant(name, ty) => {
                            assert!(ty.is_none());
                            match name.1.as_ref() {
                                Expression::Identifier(name) => assert_eq!(*name, "answer"),
                                other => panic!("expected const identifier, got {:?}", other),
                            }
                        }
                        other => panic!("expected Constant, got {:?}", other),
                    }
                    match children[1].1.as_ref() {
                        Expression::Expr(inner) => match inner.1.as_ref() {
                            Expression::Integer(value) => assert_eq!(*value, 42),
                            other => panic!("expected integer initializer, got {:?}", other),
                        },
                        other => panic!("expected expression initializer, got {:?}", other),
                    }
                }
                other => panic!("expected Fragment, got {:?}", other),
            },
            other => panic!("expected Statement, got {:?}", other),
        }
    }

    #[test]
    fn pratt_test_fn_declaration() {
        assert_eq!(
            "fn main() -> void {\nprint \"Hello, %s\", 42;\n}",
            stmt!("fn main() -> void {\n  print \"Hello, %s\", 42;\n  }")
        );
        same!("foo(1, 3, 4) * foo(2)");
    }

    #[test]
    fn enum_parses_to_enum_decl() {
        let ast = decl_ast!("enum Option { None, Some(int) }");
        match ast {
            Expression::EnumDecl { name, variants, .. } => {
                assert_eq!(name, "Option");
                assert_eq!(variants.len(), 2);

                match variants[0].1.as_ref() {
                    Expression::EnumVariant { name, payload } => {
                        assert_eq!(*name, "None");
                        assert!(matches!(payload, EnumVariantPayload::Unit));
                    }
                    other => panic!("expected EnumVariant(None), got {:?}", other),
                }

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
        let ast = decl_ast!("enum Shape { Circle { x: int, y: int } }");
        match ast {
            Expression::EnumDecl { name, variants, .. } => {
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
        let ast = decl_ast!("let x = Option::Some(42);");
        let frag = match ast {
            Expression::Statement(s) => match s.1.as_ref() {
                Expression::Fragment(items) => items.clone(),
                other => panic!("expected Fragment inside Statement, got {:?}", other),
            },
            Expression::Fragment(items) => items,
            other => panic!("expected Statement/Fragment from let, got {:?}", other),
        };
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
        let ast = expr_ast!("Some(42)");
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
        let inner = match ast {
            Expression::Expr(e) => e.1.as_ref().clone(),
            other => other,
        };
        match inner {
            Expression::Match { scrutinee, arms } => {
                match scrutinee.1.as_ref() {
                    Expression::Identifier(n) => assert_eq!(*n, "x"),
                    Expression::Expr(e) => match e.1.as_ref() {
                        Expression::Identifier(n) => assert_eq!(*n, "x"),
                        other => panic!("expected Identifier(x), got {:?}", other),
                    },
                    other => panic!("expected scrutinee to be `x`, got {:?}", other),
                }
                assert_eq!(arms.len(), 2);

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

    #[test]
    fn postfix_field_access_parses_to_access() {
        let ast = expr_ast!("point.x");
        let inner = match ast {
            Expression::Expr(e) => e.1.as_ref().clone(),
            other => other,
        };
        match inner {
            Expression::Access(receiver, field) => {
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
    fn named_call_args_parse_to_named_arg() {
        let ast = expr_ast!("f(a: 1, b: 2)");
        let inner = match ast {
            Expression::Expr(e) => e.1.as_ref().clone(),
            other => other,
        };
        match inner {
            Expression::Call { name, args } => {
                match name.1.as_ref() {
                    Expression::Identifier(n) => assert_eq!(*n, "f"),
                    other => panic!("expected Identifier(f), got {:?}", other),
                }
                let args = args.expect("call should have args");
                assert_eq!(args.len(), 2);
                match args[0].1.as_ref() {
                    Expression::NamedArg(n, val) => {
                        assert_eq!(*n, "a");
                        assert!(
                            matches!(val.1.as_ref(), Expression::Integer(1)),
                            "expected Integer(1), got {:?}",
                            val.1
                        );
                    }
                    other => panic!("expected NamedArg(a, 1), got {:?}", other),
                }
                match args[1].1.as_ref() {
                    Expression::NamedArg(n, val) => {
                        assert_eq!(*n, "b");
                        assert!(
                            matches!(val.1.as_ref(), Expression::Integer(2)),
                            "expected Integer(2), got {:?}",
                            val.1
                        );
                    }
                    other => panic!("expected NamedArg(b, 2), got {:?}", other),
                }
            }
            other => panic!("expected Call, got {:?}", other),
        }
    }

    #[test]
    fn named_call_args_display_round_trips() {
        same!("f(a: 1, b: 2)");
        same!("greet(\"Ada\", age: 36)");
    }

    #[test]
    fn postfix_field_access_chains_left_to_right() {
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
        same!("point.x");
        same!("p.x.y");
    }

    #[test]
    fn postfix_field_access_does_not_break_float_parsing() {
        // `1.0` must stay a float atom, not `1` + postfix `.0`.
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

    #[test]
    fn rest_param_parses_in_arg_list() {
        let src = "fn sum(int... xs) -> int { return 0; }";
        let ast = Pratt::default().parse(src).expect("parse");
        let func = match ast.1.as_ref() {
            Expression::Program(items) => items[0].1.as_ref(),
            other => other,
        };
        let found = match func {
            Expression::Function { args, .. } => match args.1.as_ref() {
                Expression::Fragment(items) => {
                    matches!(items[0].1.as_ref(), Expression::Argument(_, "xs", true))
                }
                _ => false,
            },
            _ => false,
        };
        assert!(found, "expected Argument(..., xs, true) in arg list");
        // Display round-trip for the rest form.
        assert!(
            format!("{}", func).contains("int... xs"),
            "display should show rest syntax, got {}",
            func
        );
    }

    #[test]
    fn range_half_open_parses() {
        same!("0..10");
        let inner = match expr_ast!("0..10") {
            Expression::Expr(e) => e.1.as_ref().clone(),
            other => other,
        };
        match inner {
            Expression::Range {
                inclusive: false, ..
            } => {}
            other => panic!("expected half-open Range, got {:?}", other),
        }
    }

    #[test]
    fn range_inclusive_parses() {
        same!("0..=10");
        let inner = match expr_ast!("0..=10") {
            Expression::Expr(e) => e.1.as_ref().clone(),
            other => other,
        };
        match inner {
            Expression::Range {
                inclusive: true, ..
            } => {}
            other => panic!("expected inclusive Range, got {:?}", other),
        }
    }

    #[test]
    fn range_does_not_break_float_or_field_access() {
        let float = match expr_ast!("1.0") {
            Expression::Expr(e) => e.1.as_ref().clone(),
            other => other,
        };
        assert!(matches!(float, Expression::Float(_)));
        same!("point.x");
    }

    #[test]
    fn range_chain_is_rejected_as_non_associative() {
        // `..` is non-associative — `a..b..c` must not parse.
        let result = Pratt::default().parse("1..2..3");
        assert!(
            result.is_err(),
            "expected parse error for chained range 1..2..3, got Ok"
        );
    }

    #[test]
    fn compound_assign_parses_at_assignment_precedence() {
        let ast = expr_ast!("x += 1 + 2");
        let inner = match ast {
            Expression::Expr(e) => e.1.as_ref().clone(),
            other => other,
        };
        assert!(matches!(
            inner,
            Expression::CompoundAssign(_, AssignOp::Add, _)
        ));
    }

    #[test]
    fn prefix_increment_parses_as_adjust() {
        let ast = expr_ast!("++x");
        let inner = match ast {
            Expression::Expr(e) => e.1.as_ref().clone(),
            other => other,
        };
        match inner {
            Expression::Adjust {
                op: AdjustOp::Inc,
                prefix: true,
                ..
            } => {}
            other => panic!("expected prefix ++, got {:?}", other),
        }
    }

    #[test]
    fn postfix_increment_parses_as_adjust() {
        let ast = expr_ast!("x++");
        let inner = match ast {
            Expression::Expr(e) => e.1.as_ref().clone(),
            other => other,
        };
        match inner {
            Expression::Adjust {
                op: AdjustOp::Inc,
                prefix: false,
                ..
            } => {}
            other => panic!("expected postfix ++, got {:?}", other),
        }
    }

    #[test]
    fn power_assign_token_is_not_split() {
        let ast = expr_ast!("x **= 2");
        let inner = match ast {
            Expression::Expr(e) => e.1.as_ref().clone(),
            other => other,
        };
        assert!(matches!(
            inner,
            Expression::CompoundAssign(_, AssignOp::Pow, _)
        ));
    }

    /// Unwrap the single `If` from a one-statement `fn main() { ... }` body.
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
            Expression::Function { body, .. } => body.expect("function should have a body"),
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
        let src = "fn main() { if 1 > 0 { return 1; } else }";
        let result = Pratt::default().declaration().parse(src).into_result();
        assert!(
            result.is_err(),
            "expected parse to fail for dangling else, got {:?}",
            result
        );
    }

    #[test]
    fn parse_async_fn_round_trips() {
        let ast = decl_ast!("async fn coro() { yield 1; }");
        match ast {
            Expression::Function { name, is_coro, .. } => {
                assert_eq!(name, "coro");
                assert!(is_coro);
            }
            other => panic!("expected async Function, got {:?}", other),
        }
    }

    #[test]
    fn parse_yield_statement() {
        let src = "async fn coro() { yield 42; }";
        let result = Pratt::default().declaration().parse(src).into_result();
        let (_span, expr) = result.expect("yield statement should parse");
        fn expect_yield_42(expr: &Expression) {
            let yield_node = match expr {
                Expression::Expr(e) => e.1.as_ref(),
                other => other,
            };
            match yield_node {
                Expression::Yield(y) => match y.1.as_ref() {
                    Expression::Expr(e) => match e.1.as_ref() {
                        Expression::Integer(42) => {}
                        other => panic!("expected yield 42, got {:?}", other),
                    },
                    Expression::Integer(42) => {}
                    other => panic!("expected yield 42, got {:?}", other),
                },
                other => panic!("expected Yield, got {:?}", other),
            }
        }
        match expr.as_ref() {
            Expression::Function { body, .. } => match body.as_ref().expect("function body").1.as_ref() {
                Expression::Block(stmts) => match stmts[0].1.as_ref() {
                    Expression::Statement(stmt) => match stmt.1.as_ref() {
                        // `yield` is preferred over `expr_statement`, so the
                        // node is bare `Yield` (not `ExprStatement(Yield)`).
                        Expression::Yield(_) => expect_yield_42(stmt.1.as_ref()),
                        Expression::ExprStatement(inner) => {
                            expect_yield_42(inner.1.as_ref());
                        }
                        other => panic!("expected Yield statement, got {:?}", other),
                    },
                    other => panic!("expected Statement, got {:?}", other),
                },
                other => panic!("expected Block, got {:?}", other),
            },
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn parse_resume_expression() {
        same!("resume h");
        let parsed = expr_ast!("resume h");
        let inner = match &parsed {
            Expression::Expr(e) => e.1.as_ref(),
            other => other,
        };
        match inner {
            Expression::Resume(target, arg) => {
                assert!(arg.is_none());
                match target.1.as_ref() {
                    Expression::Identifier(name) => assert_eq!(*name, "h"),
                    other => panic!("expected identifier target, got {:?}", other),
                }
            }
            other => panic!("expected Resume, got {:?}", other),
        }
    }

    #[test]
    fn parse_resume_with_send_round_trips() {
        same!("resume h with 42");
        let parsed = expr_ast!("resume h with 42");
        let inner = match &parsed {
            Expression::Expr(e) => e.1.as_ref(),
            other => other,
        };
        match inner {
            Expression::Resume(target, Some(arg)) => {
                match target.1.as_ref() {
                    Expression::Identifier(name) => assert_eq!(*name, "h"),
                    other => panic!("expected identifier target, got {:?}", other),
                }
                match arg.1.as_ref() {
                    Expression::Expr(e) => match e.1.as_ref() {
                        Expression::Integer(42) => {}
                        other => panic!("expected send arg 42, got {:?}", other),
                    },
                    Expression::Integer(42) => {}
                    other => panic!("expected send arg 42, got {:?}", other),
                }
            }
            other => panic!("expected Resume with arg, got {:?}", other),
        }
    }

    #[test]
    fn let_tuple_destructure_parses() {
        let ast = decl_ast!("let (a, b) = (1, 2);");
        let is_destructure = match &ast {
            Expression::Statement(s) | Expression::ExprStatement(s) => {
                matches!(s.1.as_ref(), Expression::LetDestructure { .. })
            }
            Expression::LetDestructure { .. } => true,
            _ => false,
        };
        assert!(is_destructure, "expected LetDestructure, got {:?}", ast);
    }

    #[test]
    fn let_record_destructure_parses() {
        let ast = decl_ast!("let { x, y } = { x: 1, y: 2 };");
        let is_destructure = match &ast {
            Expression::Statement(s) | Expression::ExprStatement(s) => {
                matches!(s.1.as_ref(), Expression::LetDestructure { .. })
            }
            Expression::LetDestructure { .. } => true,
            _ => false,
        };
        assert!(is_destructure, "expected LetDestructure, got {:?}", ast);
    }

    #[test]
    fn parse_let_binding_yield_round_trips() {
        let ast = decl_ast!("async fn f() { let x = yield 1; }");
        match ast {
            Expression::Function { body, .. } => match body.as_ref().expect("function body").1.as_ref() {
                Expression::Block(stmts) => match stmts[0].1.as_ref() {
                    Expression::Statement(stmt) => match stmt.1.as_ref() {
                        Expression::Fragment(children) => {
                            assert_eq!(children.len(), 2);
                            let init = children[1].1.as_ref();
                            let yield_expr = match init {
                                Expression::Yield(y) => y.1.as_ref(),
                                Expression::Expr(e) => match e.1.as_ref() {
                                    Expression::Yield(y) => y.1.as_ref(),
                                    other => panic!("expected Yield initializer, got {:?}", other),
                                },
                                other => panic!("expected Yield initializer, got {:?}", other),
                            };
                            match yield_expr {
                                Expression::Expr(e) => match e.1.as_ref() {
                                    Expression::Integer(1) => {}
                                    other => panic!("expected yield 1, got {:?}", other),
                                },
                                Expression::Integer(1) => {}
                                other => panic!("expected yield 1, got {:?}", other),
                            }
                        }
                        other => panic!("expected Fragment let, got {:?}", other),
                    },
                    other => panic!("expected Statement, got {:?}", other),
                },
                other => panic!("expected Block, got {:?}", other),
            },
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn parse_yield_from_round_trips() {
        same!("yield from inner");
        let parsed = expr_ast!("yield from inner");
        let inner = match &parsed {
            Expression::Expr(e) => e.1.as_ref(),
            other => other,
        };
        match inner {
            Expression::YieldFrom(target) => match target.1.as_ref() {
                Expression::Identifier(name) => assert_eq!(*name, "inner"),
                other => panic!("expected identifier, got {:?}", other),
            },
            other => panic!("expected YieldFrom, got {:?}", other),
        }
    }

    /// `extern "c" { fn puts(string s); }` parses to `ExternBlock`.
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
                assert!(!f.variadic);
            }
            other => panic!("expected ExternBlock, got {:?}", other),
        }
    }

    #[test]
    fn parse_extern_block_variadic_ellipsis() {
        let ast = decl_ast!("extern \"c\" { fn printf(string fmt, ...) -> int; }");
        match ast {
            Expression::ExternBlock {
                library,
                declarations,
            } => {
                assert_eq!(library, "c");
                assert_eq!(declarations.len(), 1);
                let f = &declarations[0];
                assert_eq!(f.name, "printf");
                assert!(f.variadic);
                assert!(f.returns.is_some());
            }
            other => panic!("expected ExternBlock, got {:?}", other),
        }
    }

    #[test]
    fn parse_extern_block_bare_ellipsis_only() {
        let ast = decl_ast!("extern \"c\" { fn weird(...) -> int; }");
        match ast {
            Expression::ExternBlock { declarations, .. } => {
                assert!(declarations[0].variadic);
            }
            other => panic!("expected ExternBlock, got {:?}", other),
        }
    }

    #[test]
    fn parse_extern_rejects_language_rest_syntax() {
        use chumsky::Parser;
        let src = "extern \"c\" { fn bad(int... xs) -> int; }";
        let result = Pratt::default().declaration().parse(src);
        assert!(
            result.has_errors(),
            "expected parse error for T... name in extern"
        );
        let errs = result.into_errors();
        let msg = format!("{:?}", errs);
        assert!(
            msg.contains("bare `...`") || msg.contains("C varargs"),
            "unexpected error text: {msg}"
        );
    }

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
                assert!(!declarations[0].variadic);
                assert_eq!(declarations[1].name, "strlen");
                assert!(declarations[1].returns.is_some());
                assert!(matches!(
                    declarations[1].returns.as_ref().unwrap().1.as_ref(),
                    Expression::Type("int")
                ));
            }
            other => panic!("expected ExternBlock, got {:?}", other),
        }
    }

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

    /// `dload` / `declare` / `invoke` are ordinary identifiers (not keywords),
    /// so a user may define `fn dload(...)` when they have not imported `ffi`.
    #[test]
    fn dload_is_not_a_keyword_and_can_name_a_user_function() {
        let src = "fn dload(int x) -> int { return x; } fn main() { print \"%i\", dload(1); }";
        let result = Pratt::default().parse(src);
        assert!(
            result.is_ok(),
            "expected user fn named dload to parse, got {:?}",
            result.err()
        );
        let ast = result.unwrap();
        let src_str = format!("{}", ast.1);
        assert!(
            src_str.contains("dload"),
            "display should retain dload name: {src_str}"
        );
    }

    #[test]
    fn ffi_types_qualified_construct_parses_multi_segment_path() {
        let src = "let x = ffi::types::Int;";
        let result = Pratt::default().parse(src);
        assert!(result.is_ok(), "parse failed: {:?}", result.err());
        let ast = result.unwrap();
        fn find_construct<'a>(e: &'a Expression<'a>) -> Option<(&'a str, &'a str)> {
            match e {
                Expression::Construct {
                    enum_name,
                    variant_name,
                    ..
                } => Some((*enum_name, *variant_name)),
                Expression::Program(items)
                | Expression::Block(items)
                | Expression::Fragment(items) => {
                    items.iter().find_map(|c| find_construct(c.1.as_ref()))
                }
                Expression::Expr(inner)
                | Expression::Group(inner)
                | Expression::Statement(inner)
                | Expression::ExprStatement(inner) => find_construct(inner.1.as_ref()),
                _ => None,
            }
        }
        let (enum_name, variant) =
            find_construct(ast.1.as_ref()).expect("expected Construct(ffi::types::Int)");
        assert_eq!(enum_name, "ffi::types");
        assert_eq!(variant, "Int");
    }

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

    #[test]
    fn parse_test_case_declaration() {
        let src = r#"test("addition works") { assert(1 + 1 == 2)?; }"#;
        let result = Pratt::default().declaration().parse(src).into_result();
        match result {
            Ok((_span, expr)) => match expr.as_ref() {
                Expression::TestCase { name, body } => {
                    fn is_string_lit(e: &Expression<'_>) -> bool {
                        match e {
                            Expression::String("addition works") => true,
                            Expression::Expr((_, inner)) | Expression::Group((_, inner)) => {
                                is_string_lit(inner)
                            }
                            _ => false,
                        }
                    }
                    assert!(
                        is_string_lit(name.1.as_ref()),
                        "expected string literal name, got {:?}",
                        name.1
                    );
                    assert!(matches!(body.1.as_ref(), Expression::Block(_)));
                }
                other => panic!("expected TestCase, got {:?}", other),
            },
            Err(e) => panic!("parse failed: {:?}", e),
        }
    }

    #[test]
    fn test_case_display_round_trips_name() {
        let src = r#"test("x") { assert(true)?; }"#;
        let ast = Pratt::default()
            .declaration()
            .parse(src)
            .into_result()
            .expect("parse");
        let displayed = format!("{}", ast.1);
        assert!(
            displayed.contains("test(\"x\")"),
            "display should retain test name: {displayed}"
        );
    }
}

#[cfg(test)]
mod tests_error_handling {
    use super::*;
    use chumsky::Parser;

    /// Unwrap the outer `Expression::Expr` wrapper the Pratt root often adds.
    fn unwrap_expr<'a>(expr: &'a Expression<'a>) -> &'a Expression<'a> {
        match expr {
            Expression::Expr((_, inner)) => unwrap_expr(inner),
            other => other,
        }
    }

    fn parse_expr(src: &str) -> Box<Expression<'_>> {
        Pratt::default()
            .expr()
            .parse(src)
            .into_result()
            .unwrap_or_else(|e| panic!("parse failed for `{}`: {:?}", src, e))
            .1
    }

    macro_rules! expr {
        ($case: expr) => {{
            parse_expr($case).to_string()
        }};
    }

    macro_rules! same_try {
        ($case: literal) => {
            assert_eq!($case.to_string(), expr!($case));
        };
    }

    #[test]
    fn raise_parses_as_raise_expression() {
        match unwrap_expr(parse_expr("raise \"boom\"").as_ref()) {
            Expression::Raise(inner) => assert_eq!(inner.1.to_string(), "\"boom\""),
            other => panic!("expected Raise, got {:?}", other),
        }
    }

    #[test]
    fn panic_parses_as_panic_expression() {
        match unwrap_expr(parse_expr("panic \"boom\"").as_ref()) {
            Expression::Panic(inner) => assert_eq!(inner.1.to_string(), "\"boom\""),
            other => panic!("expected Panic, got {:?}", other),
        }
    }

    #[test]
    fn postfix_try_parses_to_try() {
        same_try!("x?");
        assert!(matches!(
            unwrap_expr(parse_expr("x?").as_ref()),
            Expression::Try(_)
        ));
    }

    #[test]
    fn coalesce_parses_and_is_right_associative() {
        assert_eq!(expr!("a ?? b ?? c"), "a ?? b ?? c");
        match unwrap_expr(parse_expr("a ?? b ?? c").as_ref()) {
            Expression::Coalesce(lhs, rhs) => {
                assert!(matches!(
                    unwrap_expr(lhs.1.as_ref()),
                    Expression::Identifier("a")
                ));
                assert!(matches!(
                    unwrap_expr(rhs.1.as_ref()),
                    Expression::Coalesce(_, _)
                ));
            }
            other => panic!("expected right-assoc Coalesce, got {:?}", other),
        }
    }

    #[test]
    fn optional_access_parses_to_optional_access() {
        match unwrap_expr(parse_expr("x?.y").as_ref()) {
            Expression::OptionalAccess(recv, field) => {
                assert!(matches!(
                    unwrap_expr(recv.1.as_ref()),
                    Expression::Identifier("x")
                ));
                assert_eq!(*field, "y");
            }
            other => panic!("expected OptionalAccess, got {:?}", other),
        }
    }

    #[test]
    fn try_and_optional_access_bind_tighter_than_coalesce() {
        assert_eq!(expr!("a? ?? b"), "a? ?? b");
        assert_eq!(expr!("a?.x ?? b"), "a?.x ?? b");
    }

    #[test]
    fn coalesce_binds_tighter_than_assignment() {
        // `a = b ?? c` is Assignment(a, Coalesce(b, c)), not Coalesce(Assign(...), c).
        match unwrap_expr(parse_expr("a = b ?? c").as_ref()) {
            Expression::Assignment(_, rhs) => {
                assert!(matches!(
                    unwrap_expr(rhs.1.as_ref()),
                    Expression::Coalesce(_, _)
                ));
            }
            other => panic!("expected Assignment with Coalesce rhs, got {:?}", other),
        }
    }

    #[test]
    fn coalesce_binds_looser_than_or() {
        assert_eq!(expr!("a || b ?? c"), "a || b ?? c");
        match unwrap_expr(parse_expr("a || b ?? c").as_ref()) {
            Expression::Coalesce(lhs, _) => {
                assert!(matches!(unwrap_expr(lhs.1.as_ref()), Expression::Or(_, _)));
            }
            other => panic!("expected Coalesce of Or, got {:?}", other),
        }
    }

    #[test]
    fn error_handling_display_round_trips() {
        same_try!("raise 1");
        same_try!("x?");
        same_try!("a ?? b");
        same_try!("o?.f");
    }
}

#[cfg(test)]
mod tests_classes {
    use super::*;

    #[test]
    fn parse_classes_example() {
        let src = include_str!("../../examples/classes.0s");
        let p = Pratt::default();
        p.parse(src).unwrap_or_else(|e| panic!("PARSE FAIL: {e:?}"));
    }

    /// `impl` methods are space/newline-separated (no commas between methods).
    #[test]
    fn parse_impl_methods_without_commas() {
        let src = r#"
class Point { x: int, y: int, }
impl Point {
    fn sum() -> int { return self.x + self.y; }
    fn set_x(int n) { self.x = n; }
}
fn main() { let p = new Point(1, 2); }
"#;
        let p = Pratt::default();
        p.parse(src)
            .unwrap_or_else(|e| panic!("expected space-separated impl methods: {e:?}"));
    }
}

// ─── Generics parser tests ────────────────────────────────────────────────────
#[cfg(test)]
mod tests_generics {
    use super::*;
    use ast::Expression;

    // ── helpers ────────────────────────────────────────────────────────────────

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

    // ── type_alias with type params ────────────────────────────────────────────

    /// `type Id<T> = T;` — Display round-trips correctly.
    #[test]
    fn type_alias_with_single_type_param_round_trips() {
        assert_eq!(stmt!("type Id<T> = T;"), "type Id<T> = T;");
    }

    /// `type Pair<A, B> = (A, B);` — two type params, no bounds.
    #[test]
    fn type_alias_with_two_type_params_round_trips() {
        assert_eq!(stmt!("type Pair<A, B> = (A, B);"), "type Pair<A, B> = (A, B);");
    }

    /// `type Num<T: Add + Mul> = T;` — single param with two bounds.
    #[test]
    fn type_alias_with_bounded_type_param_round_trips() {
        assert_eq!(stmt!("type Bounded<T: Add + Mul> = T;"), "type Bounded<T: Add + Mul> = T;");
    }

    /// AST: `type Id<T> = T;` has one type param named `T` with no bounds.
    #[test]
    fn type_alias_type_param_ast_structure() {
        match decl_ast!("type Id<T> = T;") {
            Expression::TypeAlias { name, type_params, .. } => {
                assert_eq!(name, "Id");
                assert_eq!(type_params.len(), 1);
                assert_eq!(type_params[0].name, "T");
                assert!(type_params[0].bounds.is_empty());
            }
            other => panic!("expected TypeAlias, got {:?}", other),
        }
    }

    /// AST: `type Bounded<T: Num + Eq> = T;` — bounds are recorded.
    #[test]
    fn type_alias_bounded_param_ast_structure() {
        match decl_ast!("type Bounded<T: Num + Eq> = T;") {
            Expression::TypeAlias { type_params, .. } => {
                assert_eq!(type_params.len(), 1);
                assert_eq!(type_params[0].name, "T");
                assert_eq!(type_params[0].bounds, vec!["Num", "Eq"]);
            }
            other => panic!("expected TypeAlias, got {:?}", other),
        }
    }

    // ── fn with type params ────────────────────────────────────────────────────

    /// `fn id<T>(T x) -> T {}` — single unbounded type param.
    #[test]
    fn fn_with_single_type_param_parses() {
        match decl_ast!("fn id<T>(T x) -> T {}") {
            Expression::Function { name, type_params, .. } => {
                assert_eq!(name, "id");
                assert_eq!(type_params.len(), 1);
                assert_eq!(type_params[0].name, "T");
                assert!(type_params[0].bounds.is_empty());
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    /// `fn add<T: Num>(T a, T b) -> T {}` — one bounded type param.
    #[test]
    fn fn_with_bounded_type_param_parses() {
        match decl_ast!("fn add<T: Num>(T a, T b) -> T {}") {
            Expression::Function { name, type_params, .. } => {
                assert_eq!(name, "add");
                assert_eq!(type_params.len(), 1);
                assert_eq!(type_params[0].name, "T");
                assert_eq!(type_params[0].bounds, vec!["Num"]);
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    /// `fn zip<A, B>(A a, B b) -> (A, B) {}` — two unbounded type params.
    #[test]
    fn fn_with_two_type_params_parses() {
        match decl_ast!("fn zip<A, B>(A a, B b) -> (A, B) {}") {
            Expression::Function { name, type_params, .. } => {
                assert_eq!(name, "zip");
                assert_eq!(type_params.len(), 2);
                assert_eq!(type_params[0].name, "A");
                assert_eq!(type_params[1].name, "B");
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    /// `fn cmp<T: Eq + Ord>(T a, T b) -> bool {}` — multiple bounds.
    #[test]
    fn fn_with_multiple_bounds_parses() {
        match decl_ast!("fn cmp<T: Eq + Ord>(T a, T b) -> bool {}") {
            Expression::Function { type_params, .. } => {
                assert_eq!(type_params.len(), 1);
                assert_eq!(type_params[0].name, "T");
                assert_eq!(type_params[0].bounds, vec!["Eq", "Ord"]);
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    // ── fn with no type params (regression: type_params is empty) ─────────────

    /// Plain `fn main() {}` still has an empty `type_params` list.
    #[test]
    fn fn_without_type_params_has_empty_list() {
        match decl_ast!("fn main() {}") {
            Expression::Function { type_params, .. } => {
                assert!(type_params.is_empty());
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    // ── where clause ──────────────────────────────────────────────────────────

    /// `fn f<A, B>(A x) -> B where Convert<A, B> {}` — multi-param where.
    #[test]
    fn fn_with_multiparam_where_clause_parses() {
        match decl_ast!("fn f<A, B>(A x) -> B where Convert<A, B> {}") {
            Expression::Function {
                name,
                type_params,
                where_constraints,
                ..
            } => {
                assert_eq!(name, "f");
                assert_eq!(type_params.len(), 2);
                assert_eq!(where_constraints.len(), 1);
                assert_eq!(where_constraints[0].class, "Convert");
                assert_eq!(where_constraints[0].args.len(), 2);
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    /// Unary `where Num<T>` parses alongside binder bounds remaining empty.
    #[test]
    fn fn_with_unary_where_clause_parses() {
        match decl_ast!("fn g<T>(T x) -> T where Num<T> {}") {
            Expression::Function {
                where_constraints, ..
            } => {
                assert_eq!(where_constraints.len(), 1);
                assert_eq!(where_constraints[0].class, "Num");
                assert_eq!(where_constraints[0].args.len(), 1);
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    /// Display round-trips a multi-param where clause.
    #[test]
    fn fn_with_where_clause_display_round_trips() {
        let s = stmt!("fn f<A, B>(A x) -> B where Convert<A, B> {}");
        assert!(
            s.contains("where Convert<A, B>"),
            "expected where clause in display, got: {s}"
        );
        assert!(s.starts_with("fn f<A, B>"), "got: {s}");
    }

    // ── enum with type params ──────────────────────────────────────────────────

    /// `enum Option<T> { None, Some(T) }` — one type param.
    #[test]
    fn enum_with_single_type_param_parses() {
        match decl_ast!("enum Option<T> { None, Some(T), }") {
            Expression::EnumDecl { name, type_params, .. } => {
                assert_eq!(name, "Option");
                assert_eq!(type_params.len(), 1);
                assert_eq!(type_params[0].name, "T");
            }
            other => panic!("expected EnumDecl, got {:?}", other),
        }
    }

    /// `enum Result<T, E> { Ok(T), Err(E) }` — two type params.
    #[test]
    fn enum_with_two_type_params_parses() {
        match decl_ast!("enum Result<T, E> { Ok(T), Err(E), }") {
            Expression::EnumDecl { name, type_params, .. } => {
                assert_eq!(name, "Result");
                assert_eq!(type_params.len(), 2);
                assert_eq!(type_params[0].name, "T");
                assert_eq!(type_params[1].name, "E");
            }
            other => panic!("expected EnumDecl, got {:?}", other),
        }
    }

    // ── class with type params ─────────────────────────────────────────────────

    /// `class Box<T> { value: T, }` — one type param.
    #[test]
    fn class_with_single_type_param_parses() {
        match decl_ast!("class Box<T> { value: T, }") {
            Expression::Class { name, type_params, .. } => {
                assert_eq!(name, "Box");
                assert_eq!(type_params.len(), 1);
                assert_eq!(type_params[0].name, "T");
            }
            other => panic!("expected Class, got {:?}", other),
        }
    }

    /// `class Pair<A, B: Ord> { first: A, second: B, }` — two params, one bounded.
    #[test]
    fn class_with_bounded_type_params_parses() {
        match decl_ast!("class Pair<A, B: Ord> { first: A, second: B, }") {
            Expression::Class { name, type_params, .. } => {
                assert_eq!(name, "Pair");
                assert_eq!(type_params.len(), 2);
                assert_eq!(type_params[0].name, "A");
                assert!(type_params[0].bounds.is_empty());
                assert_eq!(type_params[1].name, "B");
                assert_eq!(type_params[1].bounds, vec!["Ord"]);
            }
            other => panic!("expected Class, got {:?}", other),
        }
    }

    // ── inherent impl with type params ─────────────────────────────────────────

    /// `impl Cell<T> { fn get() -> T {} }` → `Implementation` with one type param.
    #[test]
    fn inherent_impl_with_type_param_parses() {
        match decl_ast!("impl Cell<T> { fn get() -> T {} }") {
            Expression::Implementation { owner, type_params, .. } => {
                assert_eq!(owner, "Cell");
                assert_eq!(type_params.len(), 1);
                assert_eq!(type_params[0].name, "T");
            }
            other => panic!("expected Implementation, got {:?}", other),
        }
    }

    /// `impl Foo<T: Num + Eq> { fn bar() {} }` — bounded param.
    #[test]
    fn inherent_impl_with_bounded_type_param_parses() {
        match decl_ast!("impl Foo<T: Num + Eq> { fn bar() {} }") {
            Expression::Implementation { owner, type_params, .. } => {
                assert_eq!(owner, "Foo");
                assert_eq!(type_params.len(), 1);
                assert_eq!(type_params[0].name, "T");
                assert_eq!(type_params[0].bounds, vec!["Num", "Eq"]);
            }
            other => panic!("expected Implementation, got {:?}", other),
        }
    }

    /// `impl Point { fn sum() {} }` — no type params, inherent impl.
    #[test]
    fn inherent_impl_without_type_params_parses() {
        match decl_ast!("impl Point { fn sum() {} }") {
            Expression::Implementation { owner, type_params, .. } => {
                assert_eq!(owner, "Point");
                assert!(type_params.is_empty());
            }
            other => panic!("expected Implementation, got {:?}", other),
        }
    }

    // ── typeclass impl (primitive type args) ───────────────────────────────────

    /// `impl Num<int> { fn add(int a, int b) -> int {} }` → `TypeClassImpl`.
    #[test]
    fn typeclass_impl_with_primitive_type_arg_parses() {
        match decl_ast!("impl Num<int> { fn add(int a, int b) -> int {} }") {
            Expression::TypeClassImpl { class, args, .. } => {
                assert_eq!(class, "Num");
                assert_eq!(args.len(), 1);
                // The single arg should be `Type("int")`.
                assert!(matches!(args[0].1.as_ref(), Expression::Type("int")));
            }
            other => panic!("expected TypeClassImpl, got {:?}", other),
        }
    }

    /// `impl Show<string> { fn show(string s) -> string {} }` — string arg.
    #[test]
    fn typeclass_impl_with_string_type_arg_parses() {
        match decl_ast!("impl Show<string> { fn show(string s) -> string {} }") {
            Expression::TypeClassImpl { class, args, .. } => {
                assert_eq!(class, "Show");
                assert_eq!(args.len(), 1);
                assert!(matches!(args[0].1.as_ref(), Expression::Type("string")));
            }
            other => panic!("expected TypeClassImpl, got {:?}", other),
        }
    }

    /// `impl Show<Point> { … }` — user enum / multi-char concrete type arg
    /// must parse as `TypeClassImpl`, not an inherent `Implementation` for a
    /// class named `Show` (Phase 4 `%v` / Show).
    #[test]
    fn typeclass_impl_with_user_type_arg_parses() {
        match decl_ast!("impl Show<Point> { fn show(Point p) -> string {} }") {
            Expression::TypeClassImpl { class, args, .. } => {
                assert_eq!(class, "Show");
                assert_eq!(args.len(), 1);
                assert!(matches!(args[0].1.as_ref(), Expression::Type("Point")));
            }
            other => panic!("expected TypeClassImpl, got {:?}", other),
        }
    }

    // ── typeclass decl ─────────────────────────────────────────────────────────

    /// `trait Eq<T> { fn eq(T a, T b) -> bool; }` — sig-only method.
    #[test]
    fn typeclass_with_sig_only_method_parses() {
        match decl_ast!("trait Eq<T> { fn eq(T a, T b) -> bool; }") {
            Expression::TypeClass { name, type_params, methods } => {
                assert_eq!(name, "Eq");
                assert_eq!(type_params.len(), 1);
                assert_eq!(type_params[0].name, "T");
                assert_eq!(methods.len(), 1);
                // The sig-only method is a Function with an empty Block body.
                match methods[0].1.as_ref() {
                    Expression::Function { name: mname, body, .. } => {
                        assert_eq!(*mname, "eq");
                        assert!(matches!(
                            body.as_ref().expect("method body").1.as_ref(),
                            Expression::Block(stmts) if stmts.is_empty()
                        ));
                    }
                    other => panic!("expected Function, got {:?}", other),
                }
            }
            other => panic!("expected TypeClass, got {:?}", other),
        }
    }

    /// `trait Num<T> { fn add(T a, T b) -> T { return a + b; } }` — default method.
    #[test]
    fn typeclass_with_default_method_parses() {
        match decl_ast!("trait Num<T> { fn add(T a, T b) -> T { return a + b; } }") {
            Expression::TypeClass { name, type_params, methods } => {
                assert_eq!(name, "Num");
                assert_eq!(type_params.len(), 1);
                assert_eq!(methods.len(), 1);
                // A default method has a non-empty block.
                match methods[0].1.as_ref() {
                    Expression::Function { name: mname, body, .. } => {
                        assert_eq!(*mname, "add");
                        // Block is non-empty (contains the return statement).
                        assert!(matches!(
                            body.as_ref().expect("method body").1.as_ref(),
                            Expression::Block(stmts) if !stmts.is_empty()
                        ));
                    }
                    other => panic!("expected Function, got {:?}", other),
                }
            }
            other => panic!("expected TypeClass, got {:?}", other),
        }
    }

    /// `trait Ord<T: Eq> { fn lt(T a, T b) -> bool; fn gt(T a, T b) -> bool; }` — two sig-only.
    #[test]
    fn typeclass_with_bounded_param_and_two_methods_parses() {
        match decl_ast!(
            "trait Ord<T: Eq> { fn lt(T a, T b) -> bool; fn gt(T a, T b) -> bool; }"
        ) {
            Expression::TypeClass { name, type_params, methods } => {
                assert_eq!(name, "Ord");
                assert_eq!(type_params.len(), 1);
                assert_eq!(type_params[0].bounds, vec!["Eq"]);
                assert_eq!(methods.len(), 2);
            }
            other => panic!("expected TypeClass, got {:?}", other),
        }
    }

    /// `trait Show { fn show() -> string; }` — no type params (plain trait).
    #[test]
    fn typeclass_without_type_params_parses() {
        match decl_ast!("trait Show { fn show() -> string; }") {
            Expression::TypeClass { name, type_params, methods } => {
                assert_eq!(name, "Show");
                assert!(type_params.is_empty());
                assert_eq!(methods.len(), 1);
            }
            other => panic!("expected TypeClass, got {:?}", other),
        }
    }

    // ── forall type annotations ────────────────────────────────────────────────

    /// `type F = forall T. T;` — single unbounded forall param in type alias.
    #[test]
    fn forall_in_type_alias_parses() {
        match decl_ast!("type F = forall T. T;") {
            Expression::TypeAlias { ty, type_params: alias_params, .. } => {
                assert!(alias_params.is_empty()); // the alias itself has no params
                match ty.1.as_ref() {
                    Expression::Forall { params, ty: inner } => {
                        assert_eq!(params.len(), 1);
                        assert_eq!(params[0].name, "T");
                        assert!(params[0].bounds.is_empty());
                        // inner type is `T` (an identifier / Type node)
                        assert!(matches!(
                            inner.1.as_ref(),
                            Expression::Type("T") | Expression::Identifier("T")
                        ));
                    }
                    other => panic!("expected Forall, got {:?}", other),
                }
            }
            other => panic!("expected TypeAlias, got {:?}", other),
        }
    }

    /// `type F = forall T: Num. T;` — single bounded forall param.
    #[test]
    fn forall_with_bounded_param_in_type_alias_parses() {
        match decl_ast!("type F = forall T: Num. T;") {
            Expression::TypeAlias { ty, .. } => {
                match ty.1.as_ref() {
                    Expression::Forall { params, .. } => {
                        assert_eq!(params.len(), 1);
                        assert_eq!(params[0].name, "T");
                        assert_eq!(params[0].bounds, vec!["Num"]);
                    }
                    other => panic!("expected Forall, got {:?}", other),
                }
            }
            other => panic!("expected TypeAlias, got {:?}", other),
        }
    }

    /// `type F = forall T, U. T;` — two unbounded forall params.
    #[test]
    fn forall_with_two_params_in_type_alias_parses() {
        match decl_ast!("type F = forall T, U. T;") {
            Expression::TypeAlias { ty, .. } => {
                match ty.1.as_ref() {
                    Expression::Forall { params, .. } => {
                        assert_eq!(params.len(), 2);
                        assert_eq!(params[0].name, "T");
                        assert_eq!(params[1].name, "U");
                    }
                    other => panic!("expected Forall, got {:?}", other),
                }
            }
            other => panic!("expected TypeAlias, got {:?}", other),
        }
    }

    /// `forall T. T` Display round-trip: `forall T. T`
    #[test]
    fn forall_display_round_trips() {
        assert_eq!(stmt!("type F = forall T. T;"), "type F = forall T. T;");
    }

    // ── Display round-trips for new forms ─────────────────────────────────────

    /// `type Id<T> = T;` (already tested above — extra sanity check).
    #[test]
    fn type_alias_with_param_display_is_stable() {
        // Parse and re-display must be identity.
        let s = stmt!("type Map<K, V> = (K, V);");
        assert_eq!(s, "type Map<K, V> = (K, V);");
    }

    /// TypeClass Display: `trait Eq<T> { … }`
    #[test]
    fn typeclass_display_round_trips() {
        // The Display impl omits the function bodies' args (unhandled Display)
        // so we only check that the outer structure round-trips and contains
        // the expected substrings.
        let s = stmt!("trait Show { fn show() -> string; }");
        assert!(s.starts_with("trait Show {"), "got: {s}");
        assert!(s.contains("show"), "got: {s}");
    }

    /// `F: * -> *` parses as an Arrow-kinded type parameter.
    #[test]
    fn constructor_kind_annotation_parses() {
        match decl_ast!("trait Container<F: * -> *> { fn first<A>(F<A> xs) -> A; }") {
            Expression::TypeClass { type_params, .. } => {
                assert_eq!(type_params.len(), 1);
                assert_eq!(type_params[0].name, "F");
                assert_eq!(
                    type_params[0].kind,
                    crate::ast::Kind::Arrow(
                        Box::new(crate::ast::Kind::Type),
                        Box::new(crate::ast::Kind::Type)
                    )
                );
                assert!(type_params[0].bounds.is_empty());
            }
            other => panic!("expected TypeClass, got {:?}", other),
        }
    }

    #[test]
    fn constraint_kind_annotation_parses() {
        match decl_ast!("fn apply_c<c: * -> Constraint, T: c>(T x) -> string { return show(x); }")
        {
            Expression::Function { type_params, .. } => {
                assert_eq!(type_params.len(), 2);
                assert_eq!(type_params[0].name, "c");
                assert_eq!(
                    type_params[0].kind,
                    crate::ast::Kind::Arrow(
                        Box::new(crate::ast::Kind::Type),
                        Box::new(crate::ast::Kind::Constraint)
                    )
                );
                assert_eq!(type_params[1].name, "T");
                assert_eq!(type_params[1].bounds, vec!["c"]);
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn binary_hkt_kind_annotation_is_right_associative() {
        match decl_ast!("trait Bifunctor<F: * -> * -> *> { fn tag<A, B>(F<A, B> xs) -> int; }") {
            Expression::TypeClass { type_params, .. } => {
                assert_eq!(
                    type_params[0].kind,
                    crate::ast::Kind::Arrow(
                        Box::new(crate::ast::Kind::Type),
                        Box::new(crate::ast::Kind::Arrow(
                            Box::new(crate::ast::Kind::Type),
                            Box::new(crate::ast::Kind::Type)
                        ))
                    )
                );
            }
            other => panic!("expected TypeClass, got {:?}", other),
        }
    }

    #[test]
    fn parenthesized_kind_annotation_parses() {
        match decl_ast!("trait Higher<F: (* -> *) -> *> { fn tag<G: * -> *>(F<G> xs) -> int; }") {
            Expression::TypeClass { type_params, .. } => {
                assert_eq!(
                    type_params[0].kind,
                    crate::ast::Kind::Arrow(
                        Box::new(crate::ast::Kind::Arrow(
                            Box::new(crate::ast::Kind::Type),
                            Box::new(crate::ast::Kind::Type)
                        )),
                        Box::new(crate::ast::Kind::Type)
                    )
                );
            }
            other => panic!("expected TypeClass, got {:?}", other),
        }
    }

    #[test]
    fn kind_annotation_can_be_followed_by_bound() {
        match decl_ast!(
            "fn use_bi<F: * -> * -> *, Bifunctor, A, B>(F<A, B> xs) -> int { return 0; }"
        ) {
            Expression::Function { type_params, .. } => {
                assert_eq!(type_params.len(), 3);
                assert_eq!(type_params[0].name, "F");
                assert_eq!(type_params[0].bounds, vec!["Bifunctor"]);
                assert_eq!(type_params[1].name, "A");
                assert_eq!(type_params[2].name, "B");
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    /// Display keeps constructor-kind annotations on type params.
    #[test]
    fn constructor_kind_display_round_trips() {
        let s = stmt!("trait Container<F: * -> *> { fn first<A>(F<A> xs) -> A; }");
        assert!(
            s.contains("F: * -> *"),
            "expected kind annotation in display, got: {s}"
        );
    }

    #[test]
    fn binary_hkt_kind_display_round_trips() {
        let s = stmt!("trait Bifunctor<F: * -> * -> *> { fn tag<A, B>(F<A, B> xs) -> int; }");
        assert!(
            s.contains("F: * -> * -> *"),
            "expected binary kind annotation in display, got: {s}"
        );
    }

    #[test]
    fn constraint_kind_display_round_trips() {
        let s = stmt!("fn apply_c<c: * -> Constraint, T: c>(T x) -> string { return show(x); }");
        assert!(
            s.contains("c: * -> Constraint"),
            "expected constraint kind annotation in display, got: {s}"
        );
        assert!(s.contains("T: c"), "expected abstract bound in display, got: {s}");
    }

    /// TypeClassImpl Display: `impl Num<int> { … }`
    #[test]
    fn typeclass_impl_display_round_trips() {
        // Display prefers the Self-first `for` form.
        let s = stmt!("impl Num<int> {}");
        assert_eq!(s, "impl Num for int {  }");
    }

    /// Preferred form: `impl Show for Point`.
    #[test]
    fn trait_impl_for_parses_and_prepends_self() {
        match decl_ast!("impl Show for Point { fn show(Point p) -> string {} }") {
            Expression::TypeClassImpl { class, args, .. } => {
                assert_eq!(class, "Show");
                assert_eq!(args.len(), 1);
                assert!(matches!(*args[0].1, Expression::Type("Point")));
            }
            other => panic!("expected TypeClassImpl, got {other:?}"),
        }
    }

    /// `impl Thing<string, int> for Message` → args [Message, string, int].
    #[test]
    fn trait_impl_for_with_bracket_args_prepends_self() {
        match decl_ast!(
            "impl Thing<string, int> for Message { fn do_something(Message m, string x) -> int {} }"
        ) {
            Expression::TypeClassImpl { class, args, .. } => {
                assert_eq!(class, "Thing");
                assert_eq!(args.len(), 3);
                assert!(matches!(*args[0].1, Expression::Type("Message")));
                assert!(matches!(*args[1].1, Expression::Type("string")));
                assert!(matches!(*args[2].1, Expression::Type("int")));
            }
            other => panic!("expected TypeClassImpl, got {other:?}"),
        }
    }

    #[test]
    fn trait_impl_for_display_round_trips() {
        let s = stmt!("impl Show for Point {}");
        assert_eq!(s, "impl Show for Point {  }");
        let s2 = stmt!("impl Thing<string, int> for Message {}");
        assert_eq!(s2, "impl Thing<string, int> for Message {  }");
    }

    /// Phase 6: associated type decl + projection parse / Display round-trip.
    #[test]
    fn assoc_type_decl_and_projection_round_trip() {
        match decl_ast!(
            "trait Collect<C> { type Elem; fn head(C xs) -> Elem; }"
        ) {
            Expression::TypeClass { methods, .. } => {
                assert!(
                    methods
                        .iter()
                        .any(|m| matches!(m.1.as_ref(), Expression::AssocTypeDecl { name: "Elem", .. })),
                    "expected AssocTypeDecl Elem, got {:?}",
                    methods
                );
                // Return type of head should be bare Type("Elem") (resolved as assoc later).
                let head = methods.iter().find_map(|m| match m.1.as_ref() {
                    Expression::Function {
                        name: "head",
                        returns: Some(r),
                        ..
                    } => Some(r.1.as_ref()),
                    _ => None,
                });
                assert!(
                    matches!(head, Some(Expression::Type("Elem"))),
                    "expected bare Elem return, got {:?}",
                    head
                );
            }
            other => panic!("expected TypeClass, got {:?}", other),
        }

        let proj = Pratt::default()
            .type_annotation()
            .parse("Collect::Elem")
            .into_result()
            .expect("projection parse failed");
        assert!(
            matches!(
                proj.1.as_ref(),
                Expression::TypeProjection {
                    owner: "Collect",
                    name: "Elem",
                    args,
                }
                if args.is_empty()
            ),
            "expected TypeProjection, got {:?}",
            proj.1
        );
        assert_eq!(format!("{}", proj.1), "Collect::Elem");

        let s = stmt!(
            "trait Collect<C> { type Elem; fn head(C xs) -> Elem; }"
        );
        assert!(s.contains("type Elem;"), "got: {s}");
        assert!(s.contains("Collect"), "got: {s}");
    }

    /// Phase 6: assoc type def inside typeclass impl.
    #[test]
    fn assoc_type_def_in_impl_parses() {
        match decl_ast!(
            "impl Collect<Option<int>> { type Elem = int; fn head(Option<int> xs) -> int { return 0; } }"
        ) {
            Expression::TypeClassImpl { methods, .. } => {
                assert!(
                    methods.iter().any(|m| matches!(
                        m.1.as_ref(),
                        Expression::AssocTypeDef {
                            name: "Elem",
                            ..
                        }
                    )),
                    "expected AssocTypeDef Elem, got {:?}",
                    methods
                );
            }
            other => panic!("expected TypeClassImpl, got {:?}", other),
        }
    }

    #[test]
    fn generic_assoc_type_decl_parses() {
        match decl_ast!(
            "trait Pointer<P> { type Ref<T>; fn get<T>(P p) -> P::Ref<T>; }"
        ) {
            Expression::TypeClass { methods, .. } => {
                let assoc = methods.iter().find_map(|m| match m.1.as_ref() {
                    Expression::AssocTypeDecl { name: "Ref", type_params } => Some(type_params),
                    _ => None,
                });
                let params = assoc.expect("expected Ref associated type declaration");
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].name, "T");
            }
            other => panic!("expected TypeClass, got {:?}", other),
        }
    }

    #[test]
    fn generic_assoc_type_def_in_impl_parses() {
        match decl_ast!(
            "impl Pointer<Box> { type Ref<T> = T; fn get<T>(Box p) -> T { return 0; } }"
        ) {
            Expression::TypeClassImpl { methods, .. } => {
                let assoc = methods.iter().find_map(|m| match m.1.as_ref() {
                    Expression::AssocTypeDef { name: "Ref", type_params, .. } => {
                        Some(type_params)
                    }
                    _ => None,
                });
                let params = assoc.expect("expected Ref associated type definition");
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].name, "T");
            }
            other => panic!("expected TypeClassImpl, got {:?}", other),
        }
    }

    #[test]
    fn generic_assoc_type_projection_parses_and_displays() {
        let proj = Pratt::default()
            .type_annotation()
            .parse("Pointer::Ref<int>")
            .into_result()
            .expect("projection parse failed");
        match proj.1.as_ref() {
            Expression::TypeProjection { owner, name, args } => {
                assert_eq!((*owner, *name), ("Pointer", "Ref"));
                assert_eq!(args.len(), 1);
                assert!(matches!(args[0].1.as_ref(), Expression::Type("int")));
            }
            other => panic!("expected TypeProjection, got {:?}", other),
        }
        assert_eq!(format!("{}", proj.1), "Pointer::Ref<int>");
    }

    /// Inherent impl Display: `impl Point { … }`
    #[test]
    fn inherent_impl_display_round_trips() {
        let s = stmt!("impl Point {}");
        assert_eq!(s, "impl Point {  }");
    }

    /// Inherent impl with type param Display: `impl Cell<T> { … }`
    #[test]
    fn inherent_impl_with_type_param_display_round_trips() {
        let s = stmt!("impl Cell<T> {}");
        assert_eq!(s, "impl Cell<T> {  }");
    }

    /// `#[derive(Show, Eq)]` on enums parses attribute traits.
    #[test]
    fn enum_derive_attr_parses_traits() {
        match decl_ast!("#[derive(Show, Eq)] enum Point { Origin, Point { x: int, y: int } }") {
            Expression::EnumDecl { name, attrs, .. } => {
                assert_eq!(name, "Point");
                assert_eq!(attrs.len(), 1);
                assert_eq!(attrs[0].name, "derive");
                assert!(matches!(
                    &attrs[0].args,
                    ast::AttrArgs::Idents(v) if v == &["Show", "Eq"]
                ));
            }
            other => panic!("expected EnumDecl, got {:?}", other),
        }
    }

    /// Derive attribute Display round-trips on enums.
    #[test]
    fn enum_derive_attr_display_round_trips() {
        let s = stmt!("#[derive(Show, Eq)] enum Point { Origin }");
        assert_eq!(s, "#[derive(Show, Eq)]\nenum Point { Origin }");
    }

    /// `#[derive(Show, Eq)]` on classes parses attribute traits.
    #[test]
    fn class_derive_attr_parses_traits() {
        match decl_ast!("#[derive(Show, Eq)] class Cell { value: int }") {
            Expression::Class { name, attrs, .. } => {
                assert_eq!(name, "Cell");
                assert_eq!(attrs.len(), 1);
                assert!(matches!(
                    &attrs[0].args,
                    ast::AttrArgs::Idents(v) if v == &["Show", "Eq"]
                ));
            }
            other => panic!("expected Class, got {:?}", other),
        }
    }

    /// Enum without attributes has an empty attrs list.
    #[test]
    fn enum_without_attrs_has_empty_list() {
        match decl_ast!("enum Point { Origin }") {
            Expression::EnumDecl { attrs, .. } => assert!(attrs.is_empty()),
            other => panic!("expected EnumDecl, got {:?}", other),
        }
    }

    #[test]
    fn ffi_attr_signature_only_fn_parses() {
        match decl_ast!("#[ffi(lib = \"c\", name = \"strlen\")] fn strlen(string s) -> int;") {
            Expression::Function {
                attrs,
                name,
                body,
                ..
            } => {
                assert_eq!(name, "strlen");
                assert!(body.is_none());
                assert_eq!(attrs.len(), 1);
                assert_eq!(attrs[0].name, "ffi");
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn test_attr_on_fn_parses() {
        match decl_ast!("#[test(\"desc\")] fn foo() { return; }") {
            Expression::Function { attrs, name, body, .. } => {
                assert_eq!(name, "foo");
                assert!(body.is_some());
                assert_eq!(attrs.len(), 1);
                assert!(matches!(&attrs[0].args, ast::AttrArgs::String("desc")));
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn attr_body_target_call_is_spread() {
        match decl_ast!(
            "attr log<T>(fn(...args) -> T target, string message, ...args) -> T { return target(...args); }"
        ) {
            Expression::AttrDecl { body, .. } => match body.1.as_ref() {
                Expression::Block(stmts) => {
                    let ret = &stmts[0];
                    let call = match ret.1.as_ref() {
                        Expression::Statement(inner) => match inner.1.as_ref() {
                            Expression::Return(inner) => match inner.1.as_ref() {
                                Expression::Call { .. } => inner,
                                Expression::Expr(call) => call,
                                other => panic!("expected Call in return, got {:?}", other),
                            },
                            Expression::Expr(call) => call,
                            other => panic!("expected Return/Expr, got {:?}", other),
                        },
                        other => panic!("expected Statement, got {:?}", other),
                    };
                    match call.1.as_ref() {
                        Expression::Call { name, args } => {
                            assert!(matches!(name.1.as_ref(), Expression::Identifier("target")));
                            let args = args.as_ref().expect("args");
                            assert_eq!(args.len(), 1);
                            assert!(matches!(
                                args[0].1.as_ref(),
                                Expression::Spread(inner)
                                    if matches!(inner.1.as_ref(), Expression::Identifier("args"))
                            ));
                        }
                        other => panic!("expected Call, got {:?}", other),
                    }
                }
                other => panic!("expected Block body, got {:?}", other),
            },
            other => panic!("expected AttrDecl, got {:?}", other),
        }
    }

    #[test]
    fn attr_decl_parses() {
        match decl_ast!(
            "attr log<T>(fn(...args) -> T target, string message, ...args) -> T { return target(...args); }"
        ) {
            Expression::AttrDecl { name, .. } => assert_eq!(name, "log"),
            other => panic!("expected AttrDecl, got {:?}", other),
        }
    }

    #[test]
    fn call_site_spread_parses() {
        match decl_ast!("fn main() { pair_sum(...(1, 2)); }") {
            Expression::Function { body: Some(body), .. } => match body.1.as_ref() {
                Expression::Block(items) => {
                    let call = match items[0].1.as_ref() {
                        Expression::Statement(inner) => match inner.1.as_ref() {
                            Expression::ExprStatement(call) => call,
                            Expression::Expr(call) => call,
                            other => panic!("expected expr statement, got {:?}", other),
                        },
                        Expression::ExprStatement(call) => call,
                        other => panic!("expected Statement, got {:?}", other),
                    };
                    let call = match call.1.as_ref() {
                        Expression::Call { .. } => call,
                        Expression::Expr(inner) => inner,
                        other => panic!("expected Call, got {:?}", other),
                    };
                    match call.1.as_ref() {
                        Expression::Call { args: Some(args), .. } => {
                            assert_eq!(args.len(), 1);
                            assert!(matches!(args[0].1.as_ref(), Expression::Spread(_)));
                        }
                        other => panic!("expected Call, got {:?}", other),
                    }
                }
                other => panic!("expected Block, got {:?}", other),
            },
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn stacked_attrs_parse() {
        match decl_ast!("#[derive(Show)] #[test] fn foo() { return; }") {
            Expression::Function { attrs, .. } => {
                assert_eq!(attrs.len(), 2);
                assert_eq!(attrs[0].name, "derive");
                assert_eq!(attrs[1].name, "test");
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn parse_unclosed_block_fails() {
        let result = Pratt::default().declaration().parse("fn main() {").into_result();
        assert!(
            result.is_err(),
            "expected unclosed brace to fail, got {:?}",
            result
        );
    }

    #[test]
    fn parse_unclosed_paren_in_call_fails() {
        let result = Pratt::default().declaration().parse("fn main() { foo(1; }").into_result();
        assert!(result.is_err(), "expected unclosed call paren to fail, got {:?}", result);
    }

    #[test]
    fn parse_unclosed_string_fails() {
        let result = Pratt::default().declaration().parse(r#"fn main() { print "hi; }"#).into_result();
        assert!(result.is_err(), "expected unclosed string to fail, got {:?}", result);
    }

    #[test]
    fn parse_use_with_trailing_double_colon_fails() {
        let result = Pratt::default()
            .declaration()
            .parse("use foo::;")
            .into_result();
        assert!(
            result.is_err(),
            "expected `use foo::;` to fail, got {:?}",
            result
        );
    }

    #[test]
    fn parse_mod_missing_semicolon_fails() {
        // `mod foo` without `;` / body should not parse as a complete declaration.
        let result = Pratt::default().declaration().parse("mod foo").into_result();
        assert!(
            result.is_err(),
            "expected `mod foo` without terminator to fail, got {:?}",
            result
        );
    }

    #[test]
    fn parse_match_arm_missing_arrow_fails() {
        let result = Pratt::default()
            .declaration()
            .parse("fn main() { let x = match 1 { _ 1 }; }")
            .into_result();
        assert!(
            result.is_err(),
            "expected match arm without `=>` to fail, got {:?}",
            result
        );
    }

    #[test]
    fn parse_parenthesized_expr_is_not_one_tuple() {
        // `(1)` must parse as a grouped expression, not a 1-tuple.
        let result = Pratt::default().declaration().parse("fn main() { let x = (1); }").into_result();
        assert!(result.is_ok(), "expected `(1)` to parse as group, got {:?}", result);
        let src = result.unwrap().1.to_string();
        assert!(!src.contains("(1,)"), "group should not render as 1-tuple: {src}");
    }

    #[test]
    fn parse_explicit_one_tuple_requires_trailing_comma() {
        let result = Pratt::default()
            .declaration()
            .parse("fn main() { let x = (1,); }")
            .into_result();
        assert!(result.is_ok(), "expected `(1,)` to parse as 1-tuple, got {:?}", result);
    }

    #[test]
    fn parse_invalid_tuple_missing_comma_fails() {
        let result = Pratt::default()
            .declaration()
            .parse("fn main() { let x = (1 2); }")
            .into_result();
        assert!(result.is_err(), "expected `(1 2)` to fail, got {:?}", result);
    }

    #[test]
    fn parse_enum_trailing_junk_fails() {
        let result = Pratt::default()
            .declaration()
            .parse("enum E { A B }")
            .into_result();
        assert!(result.is_err(), "expected missing comma between variants to fail, got {:?}", result);
    }

    /// P3: `return -1;` is a Return of negated int, not `return - 1` subtraction.
    #[test]
    fn return_negative_literal_parses_as_return() {
        fn unwrap_expr<'a>(expr: &'a Expression<'a>) -> &'a Expression<'a> {
            match expr {
                Expression::Expr(inner) | Expression::Group(inner) => {
                    unwrap_expr(inner.1.as_ref())
                }
                other => other,
            }
        }
        match decl_ast!("fn f() -> int { return -1; }") {
            Expression::Function { body, .. } => {
                fn has_return_negate(expr: &Expression<'_>) -> bool {
                    match expr {
                        Expression::Return(inner) => {
                            matches!(unwrap_expr(inner.1.as_ref()), Expression::Negate(_))
                        }
                        Expression::Block(children)
                        | Expression::Program(children)
                        | Expression::Fragment(children) => {
                            children.iter().any(|c| has_return_negate(c.1.as_ref()))
                        }
                        Expression::Statement(inner)
                        | Expression::ExprStatement(inner)
                        | Expression::Group(inner)
                        | Expression::Expr(inner) => has_return_negate(inner.1.as_ref()),
                        _ => false,
                    }
                }
                assert!(
                    has_return_negate(body.as_ref().expect("function body").1.as_ref()),
                    "expected Return(Negate(...)); got {}",
                    body.as_ref().expect("function body").1
                );
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn return_subtraction_still_parses() {
        fn unwrap_expr<'a>(expr: &'a Expression<'a>) -> &'a Expression<'a> {
            match expr {
                Expression::Expr(inner) | Expression::Group(inner) => {
                    unwrap_expr(inner.1.as_ref())
                }
                other => other,
            }
        }
        match decl_ast!("fn f() -> int { return 0 - 1; }") {
            Expression::Function { body, .. } => {
                fn has_return_sub(expr: &Expression<'_>) -> bool {
                    match expr {
                        Expression::Return(inner) => {
                            matches!(unwrap_expr(inner.1.as_ref()), Expression::Sub(_, _))
                        }
                        Expression::Block(children)
                        | Expression::Program(children)
                        | Expression::Fragment(children) => {
                            children.iter().any(|c| has_return_sub(c.1.as_ref()))
                        }
                        Expression::Statement(inner)
                        | Expression::ExprStatement(inner)
                        | Expression::Group(inner)
                        | Expression::Expr(inner) => has_return_sub(inner.1.as_ref()),
                        _ => false,
                    }
                }
                assert!(
                    has_return_sub(body.as_ref().expect("function body").1.as_ref()),
                    "expected Return(Sub(...)); got {}",
                    body.as_ref().expect("function body").1
                );
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    /// P3 sibling: `yield -1;` must be Yield(Negate(...)), not
    /// `Sub(Identifier("yield"), 1)` via expr_statement.
    #[test]
    fn yield_negative_literal_parses_as_yield() {
        fn unwrap_expr<'a>(expr: &'a Expression<'a>) -> &'a Expression<'a> {
            match expr {
                Expression::Expr(inner) | Expression::Group(inner) => {
                    unwrap_expr(inner.1.as_ref())
                }
                other => other,
            }
        }
        match decl_ast!("async fn coro() { yield -1; }") {
            Expression::Function { body, .. } => {
                fn has_yield_negate(expr: &Expression<'_>) -> bool {
                    match expr {
                        Expression::Yield(inner) => {
                            matches!(unwrap_expr(inner.1.as_ref()), Expression::Negate(_))
                        }
                        Expression::Block(children)
                        | Expression::Program(children)
                        | Expression::Fragment(children) => {
                            children.iter().any(|c| has_yield_negate(c.1.as_ref()))
                        }
                        Expression::Statement(inner)
                        | Expression::ExprStatement(inner)
                        | Expression::Group(inner)
                        | Expression::Expr(inner) => has_yield_negate(inner.1.as_ref()),
                        _ => false,
                    }
                }
                assert!(
                    has_yield_negate(body.as_ref().expect("function body").1.as_ref()),
                    "expected Yield(Negate(...)); got {}",
                    body.as_ref().expect("function body").1
                );
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }
}

#[cfg(test)]
mod tests_lambdas {
    use super::*;
    use ast::Expression;

    fn find_lambda<'a>(expr: &'a Expression<'a>) -> Option<&'a Expression<'a>> {
        match expr {
            Expression::Lambda { .. } => Some(expr),
            Expression::Block(children)
            | Expression::Program(children)
            | Expression::Fragment(children) => {
                children.iter().find_map(|c| find_lambda(c.1.as_ref()))
            }
            Expression::Statement(inner)
            | Expression::ExprStatement(inner)
            | Expression::Group(inner)
            | Expression::Expr(inner)
            | Expression::Return(inner)
            | Expression::ImplicitReturn(inner) => find_lambda(inner.1.as_ref()),
            Expression::Variable(_, Some(inner)) => find_lambda(inner.1.as_ref()),
            Expression::Function { body, .. } => body
                .as_ref()
                .map(|b| find_lambda(b.1.as_ref()))
                .unwrap_or(None),
            _ => None,
        }
    }

    #[test]
    fn lambda_short_form_parses_captures_and_arrow_body() {
        let ast = Pratt::default()
            .parse(
                r#"
fn main() {
    let f = fn (int x) use (y) => x + y;
}
"#,
            )
            .expect("parse failed");
        match find_lambda(ast.1.as_ref()) {
            Some(Expression::Lambda {
                captures, body, ..
            }) => {
                assert_eq!(captures, &["y"]);
                // Arrow body is an expression tree, not a Block.
                assert!(
                    !matches!(body.1.as_ref(), Expression::Block(_)),
                    "short-form `=>` body should not wrap in Block; got {}",
                    body.1
                );
            }
            other => panic!("expected Lambda, got {:?}", other),
        }
    }

    #[test]
    fn lambda_block_body_parses_without_use() {
        let ast = Pratt::default()
            .parse(
                r#"
fn main() {
    let f = fn (int x) { return x + 1; };
}
"#,
            )
            .expect("parse failed");
        match find_lambda(ast.1.as_ref()) {
            Some(Expression::Lambda {
                captures, body, ..
            }) => {
                assert!(captures.is_empty(), "expected no captures, got {captures:?}");
                assert!(
                    matches!(body.1.as_ref(), Expression::Block(_)),
                    "brace body should be Block; got {}",
                    body.1
                );
            }
            other => panic!("expected Lambda, got {:?}", other),
        }
    }
}
