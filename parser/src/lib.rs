use ast::{Expression, Output};
use std::{
    marker::PhantomData,
    num::{ParseFloatError, ParseIntError},
};

pub use chumsky::span::SimpleSpan;
use chumsky::{
    error::Rich,
    extra,
    pratt::{infix, left, postfix, prefix, right},
    prelude::{choice, just, none_of, recursive},
    text, IterParser, Parser,
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
        recursive(|expr| {
            let atom = choice((
                self.variant(),
                self.call(expr.clone()),
                self.float(),
                self.int(),
                self.ident(),
                op!("true")
                    .map_with(|state, e| (e.span(), Box::new(Expression::Bool(state == "true"))))
                    .labelled("boolean"),
                op!("false")
                    .map_with(|state, e| (e.span(), Box::new(Expression::Bool(state == "true"))))
                    .labelled("boolean"),
            ));

            choice((atom, self.group(expr.clone()))).pratt((
                postfix(Precedence::Unary as u16, op!('!'), |lhs, _, e| {
                    (e.span(), Box::new(Expression::Not(lhs)))
                }),
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
                    choice((
                        op!("=="),
                        op!("!="),
                        op!(">"),
                        op!(">="),
                        op!("<="),
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
        let arg = self
            .ident()
            .padded()
            .then(self.ident())
            .map_with(|(ty, name), e| (e.span(), Box::new(Expression::Argument(ty, name))));

        arg.separated_by(op!(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .map_with(output!(Fragment))
            .delimited_by(op!("("), op!(")"))
    }

    fn generic_params(
        &self,
    ) -> impl Parser<
        'pratt,
        &'pratt str,
        Vec<(&'pratt str, Vec<&'pratt str>)>,
        extra::Err<Rich<'pratt, char>>,
    > + Clone
           + 'pratt {
        // Parse generic parameters with optional bounds: T or T: Copy + Clone
        let generic_param = text::ident()
            .padded()
            .then(
                op!(":")
                    .ignore_then(
                        text::ident()
                            .separated_by(op!("+"))
                            .at_least(1)
                            .collect::<Vec<_>>(),
                    )
                    .or_not(),
            )
            .map(|(name, bounds)| (name, bounds.unwrap_or_default()));

        generic_param
            .separated_by(op!(","))
            .at_least(1)
            .collect()
            .delimited_by(op!("<"), op!(">"))
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
        // Create a closure that builds the function expression
        let build_func = |name: &'pratt str,
                          args: Output<'pratt>,
                          returns: Option<Output<'pratt>>,
                          body: Output<'pratt>,
                          span| {
            (
                span,
                Box::new(Expression::Function {
                    name,
                    args,
                    returns,
                    body,
                }),
            )
        };

        // Create a closure that builds the function with generics expression
        let build_func_with_generics = |generics: Vec<(&'pratt str, Vec<&'pratt str>)>,
                                        name: &'pratt str,
                                        args: Output<'pratt>,
                                        returns: Option<Output<'pratt>>,
                                        body: Output<'pratt>,
                                        span| {
            (
                span,
                Box::new(Expression::FunctionWithGenerics {
                    generics,
                    name,
                    args,
                    returns,
                    body,
                }),
            )
        };

        // Try with generics first, fall back to without
        self.generic_params()
            .then(keyword!("fn"))
            .then(text::ident().padded())
            .then(self.arg_list())
            .then(op!("->").ignore_then(self.ident()).or_not())
            .then(self.block(stmt.clone()))
            .map_with(move |(((((generics, _), name), args), returns), body), e| {
                build_func_with_generics(generics, name, args, returns, body, e.span())
            })
            .or(keyword!("fn")
                .then(text::ident().padded())
                .then(self.arg_list())
                .then(op!("->").ignore_then(self.ident()).or_not())
                .then(self.block(stmt))
                .map_with(move |((((_, name), args), returns), body), e| {
                    build_func(name, args, returns, body, e.span())
                }))
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
                self.match_(stmt.clone()),
                self.block(stmt.clone()),
                self.variable().then_ignore(op!(";")),
                self.type_alias().then_ignore(op!(";")),
                self.new_type().then_ignore(op!(";")),
                self.expr_statement(),
                self.print().then_ignore(op!(";")),
                self.return_().then_ignore(op!(";")),
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

        choice((
            self.func(stmt.clone()),
            self.defer(stmt.clone()),
            self.sum_type(),
            self.struct_(stmt.clone()),
            self.interface_(stmt.clone()),
            self.impl_trait(stmt.clone()),
            self.generic_decl(stmt.clone()),
            stmt.clone(),
        ))
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
                if let Some(v) = val {
                    // Check if there's a type annotation
                    if let Some(ty_expr) = ty {
                        // Typed assignment: let x: int = value
                        (
                            e.span(),
                            Box::new(Expression::TypedAssignment {
                                name,
                                ty: ty_expr,
                                value: v,
                            }),
                        )
                    } else {
                        // Regular Assignment for let x = value
                        (
                            e.span(),
                            Box::new(Expression::Assignment(
                                (e.span(), Box::new(Expression::Identifier(name))),
                                v,
                            )),
                        )
                    }
                } else {
                    // Just declare variable without value: let x: int
                    (e.span(), Box::new(Expression::Variable(name, ty)))
                }
            })
    }

    fn type_alias(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("type")
            .ignore_then(text::ident())
            .then(op!("=").ignore_then(self.ident()))
            .map_with(|(name, target), e| {
                (
                    e.span(),
                    Box::new(Expression::TypeAlias(
                        name,
                        (e.span(), Box::new(Expression::Type(target))),
                    )),
                )
            })
    }

    fn new_type(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("newtype")
            .ignore_then(text::ident())
            .then(op!("=").ignore_then(self.ident()))
            .map_with(|(name, target), e| {
                (
                    e.span(),
                    Box::new(Expression::NewType(
                        name,
                        (e.span(), Box::new(Expression::Type(target))),
                    )),
                )
            })
    }

    fn variant_item(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        // Parse Rust-style variant: NONE or SOME(int)
        //

        self.ident()
            .then(
                op!('(')
                    .ignore_then(self.ident().separated_by(op!(",")).collect::<Vec<_>>())
                    .then_ignore(op!(')'))
                    .or_not(), // op!("(")
                               //     .ignore_then(self.ident().separated_by(op!(",")).repeated().collect())
                               //     .then_ignore(op!(")"))
                               //     .or_not(),
            )
            .map_with(|(name, field), e| {
                // Create Variant for enum declaration with optional fields
                // let fields = if let Some(fname) = field {
                //     // Variant with field: SOME(int)
                //     vec![(
                //         e.span(),
                //         Box::new(Expression::Argument(
                //             (e.span(), Box::new(Expression::Type(fname))),
                //             fname,
                //         )),
                //     )]
                // } else {
                //     Vec::new()
                // };
                (e.span(), Box::new(Expression::Variant(name, field)))
            })
    }

    fn sum_type(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("enum")
            .ignore_then(self.ident())
            .then(
                self.variant_item()
                    .separated_by(op!(","))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .or_not()
                    .delimited_by(op!("{"), op!("}")),
            )
            .map_with(|(name, variants), e| {
                (
                    e.span(),
                    Box::new(Expression::SumType(name, variants.unwrap_or_default())),
                )
            })
    }

    fn variant(
        &self,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        // Parse Type::Variant syntax
        // Syntax: Color::Red (simple) or Result::Ok(int x) (with destructuring)
        // For match patterns, we use variable names for destructuring
        self.ident()
            .then_ignore(op!("::"))
            .then(self.ident())
            .then(
                // Destructured fields must be identifiers (variable names) for match patterns
                // Syntax: Result::Ok(x) where 'x' is a variable to bind
                self.ident()
                    .separated_by(op!(","))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(op!("("), op!(")"))
                    .or_not(),
            )
            .map_with(|((type_name, variant_name), destructured), e| {
                match destructured {
                    Some(fields) => {
                        // Variant with destructured fields for match patterns: Result::Ok(x)
                        (
                            e.span(),
                            Box::new(Expression::VariantWithDestructure(
                                type_name,
                                variant_name,
                                fields,
                            )),
                        )
                    }
                    None => {
                        // Simple variant: Color::Red
                        (
                            e.span(),
                            Box::new(Expression::VariantItem(type_name, variant_name)),
                        )
                    }
                }
            })
    }

    fn struct_<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        stmt: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("struct")
            .ignore_then(text::ident())
            .then(
                self.ident()
                    .then(op!(":").ignore_then(self.ident()))
                    .separated_by(op!(","))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .or_not()
                    .delimited_by(op!("{"), op!("}")),
            )
            .map_with(|(name, fields), e| {
                let field_exprs = fields
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(fname, ftype)| {
                        (
                            e.span(),
                            Box::new(Expression::Field(
                                fname,
                                (e.span(), Box::new(Expression::Type(ftype))),
                            )),
                        )
                    })
                    .collect();
                (
                    e.span(),
                    Box::new(Expression::StructDecl(name, field_exprs)),
                )
            })
    }

    fn interface_<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        stmt: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("interface")
            .ignore_then(text::ident())
            .then(
                self.ident()
                    .then(self.arg_list())
                    .then(op!("->").ignore_then(self.ident()))
                    .separated_by(op!(","))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .or_not()
                    .delimited_by(op!("{"), op!("}")),
            )
            .map_with(|(name, methods), e| {
                let method_exprs = methods
                    .unwrap_or_default()
                    .into_iter()
                    .map(|((mname, args), ret)| {
                        (
                            e.span(),
                            Box::new(Expression::Method(
                                false,
                                (e.span(), Box::new(Expression::Argument(ret, mname))),
                            )),
                        )
                    })
                    .collect();
                (
                    e.span(),
                    Box::new(Expression::InterfaceDecl(name, method_exprs)),
                )
            })
    }

    fn impl_trait<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        stmt: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("impl")
            .ignore_then(self.ident())
            .then_ignore(keyword!("for"))
            .then(self.ident())
            .map_with(|(r#trait, r#type), e| {
                (e.span(), Box::new(Expression::ImplTrait(r#trait, r#type)))
            })
    }

    fn generic_decl<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        stmt: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        // Parse a single generic parameter with optional bounds: T or T: Copy + Clone
        let generic_param = text::ident()
            .then(
                op!(":")
                    .ignore_then(
                        text::ident()
                            .separated_by(op!("+"))
                            .at_least(1)
                            .collect::<Vec<_>>(),
                    )
                    .or_not(),
            )
            .map(|(name, bounds)| (name, bounds.unwrap_or_default()));

        keyword!("fn")
            .ignore_then(text::ident())
            .then(
                op!("<").ignore_then(
                    generic_param
                        .separated_by(op!(","))
                        .allow_trailing()
                        .collect::<Vec<_>>()
                        .then_ignore(op!(">")),
                ),
            )
            .then(self.arg_list())
            .then(op!("->").ignore_then(self.ident()).or_not())
            .then(self.block(stmt))
            .map_with(|((((name, generics), args), returns), body), e| {
                (
                    e.span(),
                    Box::new(Expression::FunctionWithGenerics {
                        generics,
                        name,
                        args,
                        returns,
                        body,
                    }),
                )
            })
    }

    fn match_<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        stmt: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        keyword!("match")
            .ignore_then(self.expr())
            .then(
                self.match_arm(stmt.clone())
                    .repeated()
                    .collect::<Vec<_>>()
                    .then_ignore(op!(",").or_not())
                    .or_not()
                    .delimited_by(op!("{"), op!("}")),
            )
            .map_with(|(lhs, arms), e| {
                (
                    e.span(),
                    Box::new(Expression::Match(lhs, arms.unwrap_or_default())),
                )
            })
    }

    fn match_arm<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        stmt: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        // Parse match arm with Rust-style sum type destructuring
        // Syntax: case Type::Variant1, Type::Variant2 => { ... }
        // Also supports literals: case 1, 2, 3 => { ... }
        // The parser supports comma-separated variants in the same arm

        keyword!("case")
            .ignore_then(
                // Parse pattern - can be identifier (for variants), integer, float, string, bool
                // Variants: Color::Red, Result::Ok
                // Literals: 1, 2.5, "hello", true
                choice((
                    // First try to parse variant (Type::Name)
                    self.variant(),
                    // Then try other expressions (literals, identifiers)
                    self.expr(),
                ))
                .separated_by(op!(","))
                .collect::<Vec<_>>(),
            )
            .then_ignore(op!("=>"))
            .then(self.block(stmt))
            .map_with(|(patterns, body), e| {
                // Wrap patterns in a List expression for multiple patterns
                let patterns_expr = if patterns.len() == 1 {
                    patterns.into_iter().next().unwrap()
                } else {
                    (e.span(), Box::new(Expression::List(patterns)))
                };
                (
                    e.span(),
                    Box::new(Expression::MatchArm(patterns_expr, body)),
                )
            })
    }

    fn params<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'pratt, &'pratt str, Option<Vec<Output<'pratt>>>, extra::Err<Rich<'pratt, char>>>
           + Clone
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
        // Generic function call with turbofish syntax: foo::<Type>(args)
        // Or angle bracket syntax: foo<Type>(args)
        let turbofish = op!("::")
            .ignore_then(op!("<"))
            .ignore_then(
                self.ident()
                    .separated_by(op!(","))
                    .allow_trailing()
                    .collect::<Vec<_>>(),
            )
            .then_ignore(op!(">"));

        let angle_bracket = op!("<")
            .then(
                self.ident()
                    .separated_by(op!(","))
                    .allow_trailing()
                    .collect::<Vec<_>>(),
            )
            .then_ignore(op!(">"))
            .map(|(_, types)| types);

        self.ident()
            .then(turbofish.or(angle_bracket).or_not())
            .then(self.params(expr))
            .map_with(|((name, type_args), args), e| {
                if let Some(type_args) = type_args {
                    // Generic function call: foo::<Type>(args) or foo<Type>(args)
                    (
                        e.span(),
                        Box::new(Expression::GenericFunctionCall {
                            name,
                            type_args,
                            args,
                        }),
                    )
                } else {
                    // Regular function call: foo(args)
                    (e.span(), Box::new(Expression::Call { name, args }))
                }
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
}
