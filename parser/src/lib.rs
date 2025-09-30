use std::borrow::Borrow;
use std::marker::PhantomData;
use std::num::{ParseFloatError, ParseIntError};

use chumsky::prelude::*;
use chumsky::{
    IterParser, Parser,
    prelude::{choice, just, recursive},
};

pub use chumsky::span::SimpleSpan;
use common::{Label, Message};

#[derive(Debug, Clone, PartialEq)]
pub enum Expression<'expr> {
    Integer(i64),
    Float(f64),
    String(&'expr str),
    Bool(bool),
    Module(String, Output<'expr>),

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

    Add(Output<'expr>, Output<'expr>),
    Sub(Output<'expr>, Output<'expr>),
    Mul(Output<'expr>, Output<'expr>),
    Div(Output<'expr>, Output<'expr>),
    Mod(Output<'expr>, Output<'expr>),
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
    ExprStatement(Output<'expr>),
    Statement(Output<'expr>),
    Fragment(Vec<Output<'expr>>),
    Block(Vec<Output<'expr>>),
    Program(Vec<Output<'expr>>),
    Defer(Vec<Output<'expr>>, Output<'expr>),

    Assignment(Output<'expr>, Output<'expr>),

    Use {
        path: Vec<String>,
        name: String,
        alias: Option<String>,
    },

    Function {
        name: &'expr str,
        args: Vec<Output<'expr>>,
        returns: Option<Output<'expr>>,
        body: Vec<Output<'expr>>,
    },

    Branch(Option<Output<'expr>>, Output<'expr>),

    // I am writing a parser using the `chumsky` crate in rust, that can be found in #{buffer:parser/src/lib.rs} and #{buffer:compiler/src/lib.rs} and I have encountered the following error:
    // > found Repeated combinator making no progress at parser/src/lib.rs:538:18
    //
    // Can you help me fix it? What I am trying to do is handle inling `if .. else if ... else` chains as a single expression instead of nested ones, i.e the whole tree to be flattened so that in the compiler I have the context of where "outside" the tree is. Currently the `Expression::If` is the one I am trying to replace, while the `Expression::If2` is the new one I am trying to create. Can you suggest the fixes without changing the full parser?
    //
    // I would ask you to be concise and follow my instructions above closely. Do not focus too much on the other aspects of the parser as much as on the `If2` implementation. Also if you have any suggestion on how I could achieve the same with `If` it is also welcomed, but do keep in mind that it is not a priority necessarily.
    //
    If(Vec<Output<'expr>>),

    Call {
        name: Output<'expr>,
        args: Vec<Output<'expr>>,
    },

    Loop {
        idntifier: Option<Output<'expr>>,
        iterable: Output<'expr>,
        body: Output<'expr>,
    },

    Match(Output<'expr>, Vec<(Output<'expr>, Output<'expr>)>),

    Variable(Output<'expr>, Option<Output<'expr>>),
    Constant(Output<'expr>, Option<Output<'expr>>),

    Member(Output<'expr>),

    Instantiate(Output<'expr>),
}

impl PartialOrd for Expression<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Expression::Integer(lhs), Expression::Integer(rhs)) => lhs.partial_cmp(rhs),
            (Expression::Float(lhs), Expression::Float(rhs)) => lhs.partial_cmp(rhs),
            (Expression::Bool(lhs), Expression::Bool(rhs)) => lhs.partial_cmp(rhs),
            (Expression::String(lhs), Expression::String(rhs)) => lhs.partial_cmp(rhs),
            _ => None,
        }
    }
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

pub struct ParserError {
    message: String,
}

impl From<&str> for ParserError {
    fn from(value: &str) -> Self {
        Self {
            message: value.to_string(),
        }
    }
}

impl From<Rich<'_, char>> for ParserError {
    fn from(value: Rich<'_, char>) -> Self {
        Self {
            message: value.to_string(),
        }
    }
}

impl ToString for ParserError {
    fn to_string(&self) -> String {
        self.message.clone()
    }
}

pub struct ParserBuilder<'parser> {
    prefix: String,
    _data: PhantomData<&'parser ()>,
}

impl<'parser> Default for ParserBuilder<'parser> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'parser> ParserBuilder<'parser> {
    pub fn new() -> Self {
        Self { prefix: String::default(), _data: PhantomData }
    }

    fn ident(
        &self,
    ) -> impl Parser<'parser, &'parser str, Output<'parser>, extra::Err<Rich<'parser, char>>>
    + Copy
    + 'parser {
        text::ident().padded().map_with(output!(Identifier))
    }

    fn type_(
        &self,
    ) -> impl Parser<'parser, &'parser str, Output<'parser>, extra::Err<Rich<'parser, char>>>
    + Copy
    + 'parser {
        text::ident().padded().map_with(output!(Type))
    }

    fn comment<
        T: Parser<'parser, &'parser str, Output<'parser>, extra::Err<Rich<'parser, char>>>
            + Clone
            + 'parser,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'parser, &'parser str, Output<'parser>, extra::Err<Rich<'parser, char>>>
    + Clone
    + 'parser {
        let comment = just("//")
            .ignore_then(none_of('\n').repeated().to_slice().padded())
            .then(expr.clone().or_not())
            .map_with(|(comment, value), e| {
                if let Some(value) = value {
                    (
                        e.span(),
                        Box::new(Expression::Fragment(vec![
                            (e.span(), Box::new(Expression::Comment(comment))),
                            value,
                        ])),
                    )
                } else {
                    (e.span(), Box::new(Expression::Comment(comment)))
                }
            })
            .boxed();

        let block_comment = op!("/*")
            .ignore_then(none_of("*/").repeated().to_slice().then_ignore(op!("*/")))
            .then(expr.clone().or_not())
            .map_with(|(comment, value), e| {
                if let Some(value) = value {
                    (
                        e.span(),
                        Box::new(Expression::Fragment(vec![
                            (e.span(), Box::new(Expression::Comment(comment))),
                            value,
                        ])),
                    )
                } else {
                    (e.span(), Box::new(Expression::Comment(comment)))
                }
            })
            .boxed();

        comment.or(block_comment)
    }

    fn params<
        T: Parser<'parser, &'parser str, Output<'parser>, extra::Err<Rich<'parser, char>>>
            + Clone
            + 'parser,
    >(
        &self,
        expr: T,
    ) -> impl Parser<
        'parser,
        &'parser str,
        Option<Vec<Output<'parser>>>,
        extra::Err<Rich<'parser, char>>,
    > + Clone
    + 'parser {
        expr.clone()
            .separated_by(op!(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .or_not()
            .delimited_by(op!('('), op!(')'))
    }

    fn expression(
        &self,
    ) -> impl Parser<'parser, &'parser str, Output<'parser>, extra::Err<Rich<'parser, char>>>
    + Clone
    + 'parser {
        recursive(|expr| {
            let bool = op!("true")
                .or(op!("false"))
                .map_with(|v, e| (e.span(), Box::new(Expression::Bool(v == "true"))));

            let int = text::int(10)
                .to_slice()
                .from_str()
                .validate(|v: Result<i64, ParseIntError>, e, emitter| match v {
                    Ok(value) => value,
                    Err(msg) => {
                        emitter.emit(Rich::custom(e.span(), msg.to_string()));

                        0_i64
                    }
                })
                .map_with(output!(Integer))
                .boxed();

            let float = text::int(10)
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
                .map_with(output!(Float))
                .boxed();
            // @TODO: Handle binary(0b010101011010) as numbers
            // @TODO: Handle hex(0xa970987basd2) as numbers

            let str = just('"')
                .ignore_then(none_of('"').repeated().to_slice())
                .then_ignore(just('"'))
                .map_with(output!(String))
                // .or(just('\'')
                //     .ignore_then(any().repeated().to_slice().or(op!("\\'").not().to_slice()))
                //     .then_ignore(just('\''))
                //     .map_with(output!(String)))
                .boxed();

            let list = expr
                .clone()
                .separated_by(op!(','))
                .allow_trailing()
                .collect()
                .map_with(output!(List))
                .delimited_by(just('['), just(']'))
                .boxed();

            let atom = float
                .clone()
                .or(int)
                .or(bool)
                .or(str.clone())
                .or(self.comment(expr.clone()))
                .or(expr.clone().delimited_by(just('('), just(')')))
                .or(self.ident())
                .boxed();

            let member = op!('.')
                .ignore_then(atom.clone())
                .clone()
                .map_with(output!(Member))
                .boxed();

            let assignment = self
                .ident()
                .then_ignore(op!('='))
                .then(expr.clone())
                .map_with(|(name, value), e| {
                    (e.span(), Box::new(Expression::Assignment(name, value)))
                })
                .boxed();

            let block = expr
                .clone()
                .then(op!(';').or_not())
                .map_with(|(stmt, t), e| {
                    (
                        e.span(),
                        match t {
                            Some(_) => Box::new(Expression::Statement(stmt)),
                            None => Box::new(Expression::ImplicitReturn(stmt)),
                        },
                    )
                })
                .repeated()
                .collect::<Vec<_>>()
                .map_with(output!(Block))
                .boxed();

            let negate = op!('-')
                .repeated()
                .foldr_with(atom.clone(), |_op, rhs, e| {
                    (e.span(), Box::new(Expression::Negate(rhs)))
                })
                .boxed();
            let positive = op!('+')
                .repeated()
                .foldr_with(atom.clone(), |_, rhs, _| rhs)
                .boxed();

            let not = op!('!')
                .repeated()
                .foldr_with(atom.clone().or(bool), |_op, rhs, e| {
                    (e.span(), Box::new(Expression::Not(rhs)))
                })
                .boxed();

            let init = text::keyword("new")
                .padded()
                .ignore_then(self.ident().then_ignore(op!(';').or_not()))
                .map_with(output!(Instantiate))
                .boxed();

            let call = atom
                .clone()
                .then(self.params(expr.clone()))
                .map_with(|(f, args), e| {
                    (
                        e.span(),
                        Box::new(Expression::Call {
                            name: f,
                            args: args.unwrap_or_default(),
                        }),
                    )
                })
                .boxed();
            let pattern = atom.clone().or(str.clone()).or(bool).clone().boxed();

            let arm = pattern
                .then_ignore(op!("=>"))
                .then(
                    block
                        .clone()
                        .delimited_by(op!('{'), op!('}'))
                        .or(expr.clone().map_with(output!(ImplicitReturn))),
                )
                .separated_by(op!(','))
                .allow_trailing()
                .collect::<Vec<_>>()
                .boxed();

            let match_ = text::keyword("match")
                .padded()
                .ignored()
                .then(expr.clone())
                .then(op!('{').ignored())
                .then(arm)
                .then(op!('}').ignored())
                .map_with(|(((((), pattern), _), arms), _), e| {
                    (e.span(), Box::new(Expression::Match(pattern, arms)))
                })
                .boxed();

            let yield_ = text::keyword("yield")
                .ignore_then(expr.clone())
                .map_with(output!(Yield));

            let resume = text::keyword("resume")
                .ignore_then(expr.clone())
                .then(expr.clone().delimited_by(op!('('), op!(')')).or_not())
                .map_with(|(expr, arg), e| {
                    (e.span(), Box::new(Expression::Resume(expr, arg)))
                });

            let unary = match_
                .or(yield_)
                .or(resume)
                .or(init)
                .or(call)
                .or(member)
                .or(assignment)
                .or(negate)
                .or(not)
                .or(positive)
                .boxed();

            let binary = unary
                .clone()
                .foldl_with(
                    choice((
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
                        op!(">=").to(Expression::Geq as fn(_, _) -> _),
                        op!("<=").to(Expression::Leq as fn(_, _) -> _),
                        op!("!=").to(Expression::Neq as fn(_, _) -> _),
                        op!("==").to(Expression::Eq as fn(_, _) -> _),
                        op!("<").to(Expression::Le as fn(_, _) -> _),
                        op!(">").to(Expression::Gt as fn(_, _) -> _),
                    ))
                    .then(expr.clone())
                    .repeated(),
                    |lhs, (op, rhs), e| (e.span(), Box::new(op(lhs, rhs))),
                )
                .boxed();

            let shift = comparison
                .clone()
                .foldl_with(
                    choice((
                        op!("<<").to(Expression::Shl as fn(_, _) -> _),
                        op!(">>").to(Expression::Shr as fn(_, _) -> _),
                    ))
                    .then(expr.clone())
                    .repeated(),
                    |lhs, (op, rhs), e| (e.span(), Box::new(op(lhs, rhs))),
                )
                .boxed();

            //
            let product = shift
                .clone()
                .foldl_with(
                    choice((
                        op!('*').to(Expression::Mul as fn(_, _) -> _),
                        op!('/').to(Expression::Div as fn(_, _) -> _),
                        op!('%').to(Expression::Mod as fn(_, _) -> _),
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

            // @TODO: Investigate complex pattern mattching, for now use it as glorified `if`
            // assignment
            // .or(member)
            str.or(sum).or(list).or(atom)
        })
        .map_with(|value, e| (e.span(), Box::new(Expression::Expr(value))))
        .labelled("expression")
        .boxed()
        .memoized()
    }

    fn expression_stmt(
        &self,
    ) -> impl Parser<'parser, &'parser str, Output<'parser>, extra::Err<Rich<'parser, char>>>
    + Clone
    + 'parser {
        self.expression()
            .then_ignore(op!(';'))
            .map_with(output!(ExprStatement))
            .boxed()
    }

    fn statement(
        &self,
    ) -> impl Parser<'parser, &'parser str, Output<'parser>, extra::Err<Rich<'parser, char>>>
    + Clone
    + 'parser {
        recursive(|stmt| {
            let block = stmt
                .clone()
                .repeated()
                .collect()
                .map_with(output!(Block))
                .delimited_by(op!('{'), op!('}'))
                .boxed();

            let if_ = recursive(|if_| {
                text::keyword("if")
                    .padded()
                    .then(self.expression())
                    .then(block.clone().or(stmt.clone()))
                    .then(
                        op!("else")
                            .ignore_then(if_)
                            .separated_by(op!("else"))
                            .collect::<Vec<_>>()
                            .map_with(output!(Block))
                            .or_not(),
                    )
                    .then(op!("else").ignore_then(stmt.clone()).or_not())
                    .map_with(|((((_, condition), body), branches), alternative), e| {
                        let mut result = vec![(
                            e.span(),
                            Box::new(Expression::Branch(Some(condition), body)),
                        )];

                        if let Some((_, list)) = branches &&
                            let Expression::Block(body) = *list {
                                body.iter().for_each(|(_, branch)| {
                                    if let Expression::If(branches) = branch.borrow() {
                                        result.append(&mut branches.clone());
                                    }
                                })
                        };

                        if let Some((span, alt)) = alternative {
                            result.push((span, Box::new(Expression::Branch(None, (span, alt)))));
                        }

                        (e.span(), Box::new(Expression::If(result)))
                    })
                    .boxed()
            })
            .boxed();

            let for_ = text::keyword("for")
                .padded()
                .ignore_then(self.ident())
                .then(text::keyword("in").padded().ignored())
                .then(self.expression())
                .then(block.clone())
                .map_with(|(((item, _), iterable), body), e| {
                    (
                        e.span(),
                        Box::new(Expression::Loop {
                            idntifier: Some(item),
                            iterable,
                            body,
                        }),
                    )
                })
                .boxed();

            let while_ = text::keyword("while")
                .ignore_then(self.expression())
                .then(block.clone())
                .map_with(|(iterable, body), e| {
                    (
                        e.span(),
                        Box::new(Expression::Loop {
                            idntifier: None,
                            iterable,
                            body,
                        }),
                    )
                })
                .boxed();

            let loop_ = for_.or(while_).boxed();

            let declaration = text::keyword("let")
                .padded()
                .ignore_then(self.ident())
                .then(op!(':').ignore_then(self.type_()).or_not())
                .then(op!('=').ignore_then(self.expression().clone()).or_not())
                .map_with(|((name, type_), assignment), e| {
                    let mut vals = vec![];
                    vals.push((
                        e.span(),
                        Box::new(Expression::Variable(name.clone(), type_)),
                    ));
                    if let Some(assignment) = assignment {
                        vals.push((e.span(), Box::new(Expression::Assignment(name, assignment))))
                    }

                    (e.span(), Box::new(Expression::Fragment(vals)))
                })
                .then_ignore(op!(';'))
                .boxed();

            let constant = text::keyword("const")
                .padded()
                .ignore_then(self.ident())
                .then(op!(':').ignore_then(self.type_()).or_not())
                .then(op!('=').ignore_then(self.expression().clone()).or_not())
                .map_with(|((name, type_), assignment), e| {
                    let mut vals = vec![];
                    vals.push((
                        e.span(),
                        Box::new(Expression::Constant(name.clone(), type_)),
                    ));

                    if let Some(assignment) = assignment {
                        vals.push((e.span(), Box::new(Expression::Assignment(name, assignment))))
                    }

                    (e.span(), Box::new(Expression::Fragment(vals)))
                })
                .then_ignore(op!(';'))
                .boxed();

            let print = text::keyword("print")
                .padded()
                .ignore_then(
                    self.expression()
                        .then(
                            op!(',')
                                .ignore_then(self.expression())
                                .repeated()
                                .collect::<Vec<_>>()
                                .or_not(),
                        )
                        .then_ignore(op!(';')),
                )
                .map_with(|(format, params), e| {
                    (e.span(), Box::new(Expression::Print(format, params)))
                })
                .boxed();
            let format = text::keyword("fmt")
                .padded()
                .ignore_then(
                    self.expression()
                        .then(
                            op!(',')
                                .ignore_then(self.expression())
                                .repeated()
                                .collect::<Vec<_>>()
                                .or_not(),
                        )
                        .then_ignore(op!(';')),
                )
                .map_with(|(format, params), e| {
                    (e.span(), Box::new(Expression::Format(format, params)))
                })
                .boxed();
            let deferred = text::keyword("defer")
                .padded()
                .ignore_then(
                    text::keyword("use")
                        .ignore_then(self.params(self.expression()).clone())
                        .or_not(),
                )
                .then(block.clone().or(self.expression().then_ignore(op!(';'))))
                .map_with(|(imports, body), e| {
                    (
                        e.span(),
                        Box::new(Expression::Defer(
                            imports.map(|v| v.unwrap_or_default()).unwrap_or_default(),
                            body,
                        )),
                    )
                })
                .boxed();

            let return_ = text::keyword("return")
                .padded()
                .ignore_then(self.expression().then_ignore(op!(';')))
                .map_with(output!(Return))
                .boxed();
            let yield_ = text::keyword("yield")
                .padded()
                .ignore_then(self.expression().then_ignore(op!(';')))
                .map_with(output!(Yield))
                .boxed();

            choice((
                deferred,
                print,
                format,
                yield_,
                return_,
                if_,
                loop_,
                declaration,
                constant,
                self.expression_stmt(),
                self.comment(self.expression()),
                block,
            ))
            .map_with(|value, e| (e.span(), Box::new(Expression::Statement(value))))
            .boxed()
        })
    }

    fn block(
        &self,
    ) -> impl Parser<'parser, &'parser str, Output<'parser>, extra::Err<Rich<'parser, char>>>
    + Clone
    + 'parser {
        self.statement()
            .repeated()
            .collect()
            .map_with(output!(Block))
            .delimited_by(op!('{'), op!('}'))
            .boxed()
    }

    fn args(
        &self,
    ) -> impl Parser<
        'parser,
        &'parser str,
        Option<Vec<(Output<'parser>, Output<'parser>)>>,
        extra::Err<Rich<'parser, char>>,
    > + Clone
    + 'parser {
        self.type_()
            .then(self.ident())
            .separated_by(op!(','))
            .at_least(0)
            .allow_trailing()
            .collect::<Vec<_>>()
            .or_not()
            .delimited_by(op!('('), op!(')'))
            .boxed()
    }

    fn func(
        &self,
    ) -> impl Parser<'parser, &'parser str, Output<'parser>, extra::Err<Rich<'parser, char>>>
    + Clone
    + 'parser {
        text::keyword("fn")
            .padded()
            .ignore_then(text::ident().or_not())
            .then(self.args())
            .then(op!("->").ignore_then(self.ident()).or_not())
            .then(self.block())
            .map_with(|(((name, args), ret), body), e| {
                (
                    e.span(),
                    Box::new(Expression::Function {
                        name: name.unwrap_or("@"),
                        args: args
                            .unwrap_or_default()
                            .iter()
                            .map(|(ty, name)| {
                                (
                                    e.span(),
                                    Box::new(Expression::Constant(name.clone(), Some(ty.clone()))),
                                )
                            })
                            .collect(),
                        returns: ret,
                        body: match body.1.borrow() {
                            Expression::Block(items) => items.clone(),
                            _ => vec![],
                        },
                    }),
                )
            })
            .boxed()
    }

    fn use_(
        &self,
    ) -> impl Parser<'parser, &'parser str, Output<'parser>, extra::Err<Rich<'parser, char>>>
    + Clone
    + 'parser {
        let submodules = text::ident()
            .padded()
            .then(
                text::keyword("as")
                    .padded()
                    .ignore_then(text::ident().padded())
                    .padded()
                    .or_not(),
            )
            .then_ignore(op!(",").or_not())
            .repeated()
            .collect::<Vec<_>>()
            .delimited_by(op!('{'), op!('}'))
            .boxed();

        text::keyword("use")
            .padded()
            .ignore_then(
                text::ident()
                    .then_ignore(just("::").or_not())
                    .repeated()
                    .collect::<Vec<_>>(),
            )
            .then(submodules.or_not())
            .then_ignore(just(';'))
            .map_with(|(mut path, children), e| {
                let mut imports: Vec<Expression<'parser>> = vec![];

                match children {
                    Some(children) => children.iter().for_each(
                        |(name, alias): &(&'parser str, Option<&'parser str>)| {
                            imports.push(Expression::Use {
                                path: path.iter().map(ToString::to_string).collect(),
                                name: name.to_string(),
                                alias: alias.map(ToString::to_string),
                            });
                        },
                    ),
                    None => {
                        let name: Option<&'parser str> = path.pop();

                        imports.push(Expression::Use {
                            name: name.expect("Unable to get name").to_string(),
                            path: path.iter().map(ToString::to_string).collect(),
                            alias: None,
                        })
                    }
                }

                (
                    e.span(),
                    Box::new(Expression::Fragment(
                        imports
                            .iter()
                            .map(|item| (e.span(), Box::new(item.clone())))
                            .collect::<Vec<Output<'parser>>>(),
                    )),
                )
            })
    }

    fn build(
        &self,
    ) -> impl Parser<'parser, &'parser str, Output<'parser>, extra::Err<Rich<'parser, char>>> + 'parser
    {
        self.func()
            .or(self.use_())
            .or(self.comment(self.expression()))
            .repeated()
            .collect()
            .map_with(output!(Program))
            .padded()
            .memoized()
    }

    pub fn parse(&self, src: &'parser str) -> Result<Output<'parser>, common::Message> {
        match self.build().parse(src).into_result() {
            Ok(ast) => Ok(ast),
            Err(errs) => {
                let mut message =
                    Message::error("Parse error".to_string(), std::ops::Range::default());
                errs.iter().for_each(|err| {
                    message.push(Label::new(err.to_string(), err.span().into_range()));
                });

                Err(message)
            }
        }
    }
}

type Output<'parser> = (SimpleSpan, Box<Expression<'parser>>);

#[cfg(test)]
mod tests {
    use chumsky::Parser;

    use crate::ParserBuilder;

    macro_rules! parse {
        ($case: literal) => {
            ParserBuilder::new().build().parse($case).into_result()
        };
    }

    #[test]
    fn test_operators() {
        // @NOTE: Commented out cases, should be made to pass :)
        assert!(parse!("42").is_err());
        assert!(parse!("42;").is_err());
        assert!(parse!("return 42;").is_err());
        assert!(parse!("use foo;").is_ok());
        assert!(parse!("use foo::bar::baz;").is_ok());
        assert!(parse!("use foo::bar::{ baz };").is_ok());
        // assert!(parse!("use foo::bar::{ baz::foobar };").is_ok());
        assert!(parse!("use foo::bar::{ baz as foobar };").is_ok());
        assert!(parse!("fn foo() { }").is_ok());
        assert!(parse!("fn foo() { 10 }").is_err());
        assert!(parse!("fn foo() { 10; }").is_ok());
        assert!(parse!("fn foo() { (10); }").is_ok());
        assert!(parse!("fn foo() { return 10 }").is_err());
        assert!(parse!("fn foo() { return 10; }").is_ok());
        assert!(parse!("fn foo() { return 0.0; }").is_ok());
        assert!(parse!("fn foo() { return b01; }").is_ok());
        assert!(parse!("fn foo() { return x01; }").is_ok());
        assert!(parse!("fn foo() { 1 + 1; }").is_ok());
        assert!(parse!("fn foo() { 1 - 1; }").is_ok());
        assert!(parse!("fn foo() { 1 * 2; }").is_ok());
        assert!(parse!("fn foo() { 2 / 2; }").is_ok());
        assert!(parse!("fn foo() { 2 % 2; }").is_ok());
        assert!(parse!("fn foo() { 10 < 10; }").is_ok());
        // assert!(parse!("fn foo() { 10 <= 10; }").is_ok());
        assert!(parse!("fn foo() { 10 == 10; }").is_ok());
        assert!(parse!("fn foo() { 10 != 10; }").is_ok());
        assert!(parse!("fn foo() { 10 > 10; }").is_ok());
        assert!(parse!("fn foo() { 10 >= 10; }").is_ok());
        assert!(parse!("fn foo() { 10 << 10; }").is_ok());
        assert!(parse!("fn foo() { 10 >> 10; }").is_ok());
        assert!(parse!("fn foo() { 10 & 10; }").is_ok());
        assert!(parse!("fn foo() { 10 | 10; }").is_ok());
        assert!(parse!("fn foo() { 10 ^ 10; }").is_ok());
        assert!(parse!("fn foo() { -10; }").is_ok());
        assert!(parse!("fn foo() { -----10; }").is_ok());
        assert!(parse!("fn foo() { !10; }").is_ok());
        assert!(parse!("fn foo() { !true; }").is_ok());
        assert!(parse!("fn foo() { !!false; }").is_ok());
        assert!(parse!("fn foo() { +10; }").is_ok());
    }

    #[test]
    fn test_statements() {
        assert!(parse!("fn foo() { var = 10; }").is_ok());
        assert!(parse!("fn foo() { return 10; }").is_ok());
        assert!(parse!("fn foo() { print 10; }").is_ok());
        assert!(parse!("fn foo() { print foo; }").is_ok());
        assert!(parse!("fn foo() { print false; }").is_ok());
        assert!(parse!("fn foo() { print true; }").is_ok());
        assert!(parse!("fn foo() { print 0.0; }").is_ok());
        assert!(parse!("fn foo() { print b01; }").is_ok());
        assert!(parse!("fn foo() { print x01; }").is_ok());
        assert!(parse!("fn foo() { print bar(); }").is_ok());
        // assert!(parse!("fn foo() { this.foo = 11; }").is_ok());
        assert!(parse!("fn foo() { bar(); }").is_ok());
        assert!(parse!("fn foo() { bar(10); }").is_ok());
        assert!(parse!("fn foo() { bar(10, 10,); }").is_ok());
        assert!(parse!("fn x(int z) { }").is_ok());
        assert!(parse!("fn (int z, string foo) -> int { }").is_ok());

        assert!(parse!("fn foo() { bar() + baz(); }").is_ok());
        assert!(parse!("fn foo() { bar() - baz(); }").is_ok());
        assert!(parse!("fn foo() { bar() * baz(); }").is_ok());
        assert!(parse!("fn foo() { bar() / baz(); }").is_ok());
        assert!(parse!("fn foo() { bar() % baz(); }").is_ok());
        assert!(parse!("fn foo() { defer { baz(); } }").is_ok());
    }
}
