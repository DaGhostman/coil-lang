use std::borrow::Borrow;
use std::num::ParseFloatError;
use std::path::Path;

use ariadne::{Color, Config, IndexType, Label, Report, ReportKind, sources};
use chumsky::prelude::*;
use chumsky::{
    IterParser, Parser,
    prelude::{choice, just, recursive},
};

#[derive(Debug, Clone)]
enum Expression<'expr> {
    Number(f64),
    // Integer(i64),
    // Float(u64),
    String(&'expr str),

    Identifier(&'expr str),
    Type(&'expr str),
    Return(Output<'expr>),
    ImplicitReturn(Output<'expr>),

    Negate(Output<'expr>),
    Not(Output<'expr>),
    Positive(Output<'expr>),

    Add(Output<'expr>, Output<'expr>),
    Sub(Output<'expr>, Output<'expr>),
    Mul(Output<'expr>, Output<'expr>),
    Div(Output<'expr>, Output<'expr>),
    Shl(Output<'expr>, Output<'expr>),
    Shr(Output<'expr>, Output<'expr>),
    Xor(Output<'expr>, Output<'expr>),
    And(Output<'expr>, Output<'expr>),
    Or(Output<'expr>, Output<'expr>),
    Eq(Output<'expr>, Output<'expr>),
    Neq(Output<'expr>, Output<'expr>),
    Leq(Output<'expr>, Output<'expr>),
    Geq(Output<'expr>, Output<'expr>),
    Le(Output<'expr>, Output<'expr>),
    Gt(Output<'expr>, Output<'expr>),

    List(Vec<Output<'expr>>),
    Expr(Output<'expr>),
    Statement(Output<'expr>),
    Block(Vec<Output<'expr>>),
    Program(Vec<Output<'expr>>),

    Function {
        name: &'expr str,
        args: Vec<(Output<'expr>, Output<'expr>)>,
        body: Vec<Output<'expr>>,
    },

    If {
        condition: Output<'expr>,
        body: Output<'expr>,
        alternative: Option<Output<'expr>>,
    },

    Call {
        name: &'expr str,
        args: Vec<Output<'expr>>,
    },
}

macro_rules! op {
    ($operator: literal) => {
        just($operator).padded()
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

type Output<'parser> = (SimpleSpan, Box<Expression<'parser>>);

fn ident<'parser>()
-> impl Parser<'parser, &'parser str, Output<'parser>, extra::Err<Rich<'parser, char>>>
+ Clone
+ Copy
+ 'parser {
    text::ident().padded().map_with(output!(Identifier))
}

fn type_<'parser>()
-> impl Parser<'parser, &'parser str, Output<'parser>, extra::Err<Rich<'parser, char>>>
+ Clone
+ Copy
+ 'parser {
    text::ascii::ident().padded().map_with(output!(Type))
}

fn args<'parser>() -> impl Parser<
    'parser,
    &'parser str,
    Option<Vec<(Output<'parser>, Output<'parser>)>>,
    extra::Err<Rich<'parser, char>>,
> + Clone
+ 'parser {
    type_()
        .then(ident())
        .separated_by(op!(','))
        .at_least(0)
        .allow_trailing()
        .collect::<Vec<_>>()
        .or_not()
        .delimited_by(op!('('), op!(')'))
        .labelled("arguments")
        .boxed()
}

fn func<'parser>()
-> impl Parser<'parser, &'parser str, Output<'parser>, extra::Err<Rich<'parser, char>>> + Clone + 'parser
{
    text::keyword("fn")
        .padded()
        .ignored()
        .then(text::ident())
        .then(args())
        .then(block())
        .map_with(|(((_, name), args), body), e| {
            (
                e.span(),
                Box::new(Expression::Function {
                    name,
                    args: args.unwrap_or_default(),
                    body: match body.1.borrow() {
                        Expression::Block(items) => items.clone(),
                        _ => vec![],
                    },
                }),
            )
        })
        .boxed()
}

fn block<'parser>()
-> impl Parser<'parser, &'parser str, Output<'parser>, extra::Err<Rich<'parser, char>>> + Clone + 'parser
{
    statement()
        .or(expression().map_with(output!(ImplicitReturn)))
        .repeated()
        .collect()
        .map_with(output!(Block))
        .delimited_by(op!('{'), op!('}'))
        .boxed()
        .validate(|t, _, emitter| {
            match &t.1.borrow() {
                Expression::Block(body) => {
                    for (idx, (s, expr)) in body.iter().enumerate() {
                        if idx < (body.len() - 1)
                            && matches!(
                                expr.borrow(),
                                Expression::Return(_) | Expression::ImplicitReturn(_)
                            )
                        {
                            emitter.emit(Rich::custom(*s, "All code after this is unreachable"));
                            break;
                        }
                    }
                }
                _ => emitter.emit(Rich::custom(
                    t.0,
                    "Unexpected expression/statement encountered",
                )),
            }

            t
        })
}

fn statement<'parser>()
-> impl Parser<'parser, &'parser str, Output<'parser>, extra::Err<Rich<'parser, char>>> + Clone + 'parser
{
    recursive(|stmt| {
        let block = stmt
            .clone()
            .repeated()
            .collect()
            .map_with(output!(Block))
            .delimited_by(op!('{'), op!('}'));

        let else_ = |if_| {
            op!("else")
                .ignore_then(choice((if_, stmt)))
                .boxed()
                .or_not()
        };

        let if_ = recursive(|if_| {
            text::keyword("if")
                .padded()
                .then(expression())
                .then(block.clone())
                .then(else_(if_))
                .map_with(|(((_, condition), body), alternative), e| {
                    (
                        e.span(),
                        Box::new(Expression::If {
                            condition,
                            body,
                            alternative,
                        }),
                    )
                })
            // .validate(|t, _, emitter| {
            //     match &t.1 {
            //         Expression::Block(body) => {
            //             if body.len() > 1 {
            //                 let last = body.len() - 1;
            //                 let second_last = body.len() - 2;
            //
            //                 if matches!(body[second_last].1, Expression::ImplicitReturn(_)) {
            //                     if matches!(body[last].1, Expression::ImplicitReturn(_)) {
            //                         emitter.emit(Rich::custom(
            //                             body[second_last].0,
            //                             "possibly missing ';'",
            //                         ))
            //                     }
            //                     emitter.emit(Rich::custom(body[last].0, "Unreachable code"))
            //                 }
            //             }
            //         }
            //         _ => emitter.emit(Rich::custom(
            //             t.0,
            //             "Unexpected expression/statement encountered",
            //         )),
            //     }
            //
            //     t
            // })
        })
        .boxed();

        let return_ = text::keyword("return")
            .padded()
            .then(expression_stmt())
            .map_with(|(_, rhs), e| (e.span(), Box::new(Expression::Return(rhs))))
            .boxed();

        choice((return_, if_, expression_stmt(), block))
            .map_with(|value, e| (e.span(), Box::new(Expression::Statement(value))))
            .boxed()
    })
}

fn expression_stmt<'parser>()
-> impl Parser<'parser, &'parser str, Output<'parser>, extra::Err<Rich<'parser, char>>> + Clone + 'parser
{
    expression().then_ignore(op!(';').ignored()).boxed()
}

fn expression<'parser>()
-> impl Parser<'parser, &'parser str, Output<'parser>, extra::Err<Rich<'parser, char>>> + Clone + 'parser
{
    recursive(|expr| {
        let int = text::int(10)
            .then(just(".").then(text::int(10)).or_not())
            .to_slice()
            .from_str()
            .validate(|v: Result<f64, ParseFloatError>, e, emitter| match v {
                Ok(value) => value,
                Err(msg) => {
                    emitter.emit(Rich::custom(e.span(), msg.to_string()));

                    0_f64
                }
            })
            .map_with(output!(Number)) // |v, e| (e.span(), Expression::Number(v)))
            .boxed();

        let str = int
            .clone()
            .or(just('"')
                .ignore_then(none_of('"').repeated().to_slice())
                .then_ignore(just('"'))
                .map_with(output!(String)))
            .boxed();

        let list = expr
            .clone()
            .separated_by(just(',').padded())
            .allow_trailing()
            .collect()
            .map_with(output!(List)) // |v, e| (e.span(), Expression::List(v)))
            .delimited_by(just('['), just(']'))
            .boxed();

        let atom = int
            .clone()
            .or(expr.clone().delimited_by(just('('), just(')')))
            .or(ident())
            .boxed();

        let negate = op!('-')
            .repeated()
            .foldr_with(atom.clone(), |_op, rhs, e| {
                (e.span(), Box::new(Expression::Negate(rhs)))
            })
            .boxed();

        let not = op!('!')
            .repeated()
            .foldr_with(atom.clone(), |_op, rhs, e| {
                (e.span(), Box::new(Expression::Not(rhs)))
            })
            .boxed();

        let unary = negate.or(not).boxed();

        let binary = unary
            .clone()
            .foldl_with(
                choice((
                    op!("<<").to(Expression::Shl as fn(_, _) -> _),
                    op!(">>").to(Expression::Shr as fn(_, _) -> _),
                    op!("^").or(op!("xor")).to(Expression::Xor as fn(_, _) -> _),
                    op!("&").or(op!("and")).to(Expression::And as fn(_, _) -> _),
                    op!("|").or(op!("or")).to(Expression::Or as fn(_, _) -> _),
                ))
                .then(expr.clone())
                .repeated(),
                |lhs, (op, rhs), e| (e.span(), Box::new(op(lhs, rhs))),
            )
            .boxed();

        let comparison = binary
            .clone()
            .foldl_with(
                choice((
                    op!("==").to(Expression::Eq as fn(_, _) -> _),
                    op!("!=").to(Expression::Neq as fn(_, _) -> _),
                    op!(">=").to(Expression::Leq as fn(_, _) -> _),
                    op!("<=.").to(Expression::Geq as fn(_, _) -> _),
                    op!("<").to(Expression::Le as fn(_, _) -> _),
                    op!(">").to(Expression::Gt as fn(_, _) -> _),
                ))
                .then(expr.clone())
                .repeated(),
                |lhs, (op, rhs), e| (e.span(), Box::new(op(lhs, rhs))),
            )
            .boxed();
        //
        let product = comparison
            .clone()
            .foldl_with(
                choice((
                    op!('*').to(Expression::Mul as fn(_, _) -> _),
                    op!('/').to(Expression::Div as fn(_, _) -> _),
                ))
                .then(expr.clone())
                .repeated(),
                |lhs, (op, rhs), e| (e.span(), Box::new(op(lhs, rhs))),
            )
            .boxed();

        let sum = product
            .clone()
            .foldl_with(
                choice((
                    op!('+').to(Expression::Add as fn(_, _) -> _),
                    op!('-').to(Expression::Sub as fn(_, _) -> _),
                ))
                .then(expr.clone())
                .repeated(),
                |lhs, (op, rhs), e| (e.span(), Box::new(op(lhs, rhs))),
            )
            .boxed();

        sum.or(str).or(list).or(ident())
    })
    .map_with(|value, e| (e.span(), Box::new(Expression::Expr(value))))
    .labelled("expression")
    .boxed()
}

fn parser<'parser>()
-> impl Parser<'parser, &'parser str, Output<'parser>, extra::Err<Rich<'parser, char>>> {
    func()
        .repeated()
        .collect()
        .map_with(output!(Program))
        .padded()
        .memoized()
}

fn eval<'eval>(expr: &'eval Expression<'eval>) -> Result<(), String> {
    dbg!(&expr);
    match expr {
        _ => todo!(),
    }
}

fn main() {
    let p = std::env::current_dir().unwrap().with_file_name("test.0s");
    let src = std::fs::read_to_string(p.canonicalize().unwrap()).unwrap();
    let file = p.to_str().expect("Unable to convert file path").to_string();

    match parser().parse(&src).into_result() {
        Ok((_, ast)) => match eval(&ast) {
            Ok(_) => (),
            Err(err) => println!("Evaluation Error: {}", err),
        },
        Err(errs) => {
            errs.into_iter().for_each(|e: Rich<'_, char>| {
                Report::build(ReportKind::Error, (file.clone(), e.span().into_range()))
                    .with_config(
                        Config::new()
                            .with_index_type(IndexType::Byte)
                            .with_compact(true),
                    )
                    // .with_message(e.to_string())
                    .with_label(
                        Label::new((file.clone(), e.span().into_range()))
                            .with_message(e.reason())
                            .with_color(Color::Red),
                    )
                    // .with_label(
                    //     Label::new((file.clone(), e.span().into_range()))
                    //         .with_message(e.to_string())
                    //         .with_color(Color::Yellow), // })
                    // )
                    .finish()
                    .eprint(sources([(file.clone(), src.clone())]))
                    .unwrap()
            });
        }
    }
}
