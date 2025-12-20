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
    prelude::{choice, just, recursive},
    select_ref, text,
};

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

type Output<'parser> = (SimpleSpan, Box<Expression<'parser>>);

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

#[derive(Debug, Clone, PartialEq)]
pub enum Expression<'expr> {
    Noop(&'expr Expression<'expr>),
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

    If(Vec<Output<'expr>>),

    Call {
        name: Output<'expr>,
        args: Vec<Output<'expr>>,
    },

    Loop {
        identifier: Option<Output<'expr>>,
        iterable: Output<'expr>,
        body: Output<'expr>,
    },

    Match(Output<'expr>, Vec<(Output<'expr>, Output<'expr>)>),

    Variable(Output<'expr>, Option<Output<'expr>>),
    Constant(Output<'expr>, Option<Output<'expr>>),

    Implementation(&'expr str, &'expr str, Vec<Output<'expr>>),
    Class(&'expr str, Vec<Output<'expr>>),
    Field(Output<'expr>, Output<'expr>),
    Method(bool, Output<'expr>),
    Member(Output<'expr>),
    Access(&'expr str),
    Update(&'expr str, Output<'expr>),

    Instantiate(Output<'expr>, Option<Vec<Output<'expr>>>),
}

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
            .map_with(output!(Float))
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
                self.int(),
                self.float(),
                self.ident(),
                op!("true")
                    .map_with(|state, e| (e.span(), Box::new(Expression::Bool(state == "true")))),
                op!("false")
                    .map_with(|state, e| (e.span(), Box::new(Expression::Bool(state == "true")))),
            ));

            choice((atom, self.group(expr.clone()))).pratt((
                postfix(Precedence::Unary as u16, op!('!'), |lhs, _, e| {
                    (e.span(), Box::new(Expression::Not(lhs)))
                }),
                (infix(
                    right(Precedence::Binary as u16),
                    op!("<<"),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::Shl(lhs, rhs))),
                )),
                (infix(
                    right(Precedence::Binary as u16),
                    op!(">>"),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::Shr(lhs, rhs))),
                )),
                (infix(
                    right(Precedence::Binary as u16),
                    op!('&'),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::BitAnd(lhs, rhs))),
                )),
                (infix(
                    right(Precedence::And as u16),
                    op!("&&"),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::And(lhs, rhs))),
                )),
                (infix(
                    right(Precedence::Binary as u16),
                    op!('|'),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::BitOr(lhs, rhs))),
                )),
                (infix(right(Precedence::Or as u16), op!("||"), |lhs, _, rhs, e| {
                    (e.span(), Box::new(Expression::Or(lhs, rhs)))
                })),
                (infix(
                    right(Precedence::Binary as u16),
                    op!('^'),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::Xor(lhs, rhs))),
                )),
                (infix(
                    right(Precedence::Factor as u16),
                    op!("**"),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::Pow(lhs, rhs))),
                )),
                (infix(
                    right(Precedence::Factor as u16),
                    op!('*'),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::Mul(lhs, rhs))),
                )),
                (infix(
                    right(Precedence::Factor as u16),
                    op!('/'),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::Div(lhs, rhs))),
                )),
                (infix(
                    right(Precedence::Factor as u16),
                    op!('%'),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::Mod(lhs, rhs))),
                )),
                (infix(
                    right(Precedence::Compare as u16),
                    op!("=="),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::Eq(lhs, rhs))),
                )),
                (infix(
                    right(Precedence::Compare as u16),
                    op!("!="),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::Neq(lhs, rhs))),
                )),
                (infix(
                    right(Precedence::Compare as u16),
                    op!('>'),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::Gt(lhs, rhs))),
                )),
                (infix(
                    right(Precedence::Compare as u16),
                    op!(">="),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::Geq(lhs, rhs))),
                )),
                (infix(
                    right(Precedence::Compare as u16),
                    op!('<'),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::Le(lhs, rhs))),
                )),
                (infix(
                    right(Precedence::Compare as u16),
                    op!("<="),
                    |lhs, _, rhs, e| (e.span(), Box::new(Expression::Leq(lhs, rhs))),
                )),
                (prefix(Precedence::Negate as u16, op!('-'), |_, rhs, e| {
                    (e.span(), Box::new(Expression::Negate(rhs)))
                })),
                (prefix(Precedence::Negate as u16, op!('~'), |_, rhs, e| {
                    (e.span(), Box::new(Expression::Not(rhs)))
                })),
                (prefix(Precedence::Negate as u16, op!('+'), |_, rhs, e| {
                    (e.span(), Box::new(Expression::Positive(rhs)))
                })),
                infix(left(Precedence::Term as u16), op!('-'), |lhs, _, rhs, e| {
                    (e.span(), Box::new(Expression::Sub(lhs, rhs)))
                }),
                infix(left(Precedence::Term as u16), op!('+'), |lhs, _, rhs, e| {
                    (e.span(), Box::new(Expression::Add(lhs, rhs)))
                }),
                postfix(Precedence::None as u16, self.inc(), |_, rhs, e| {
                    // @TODO handle precedence of
                    // inc/dec
                    (e.span(), Box::new(Expression::Inc(rhs)))
                }),
                postfix(Precedence::None as u16, self.dec(), |_, rhs, e| {
                    // @TODO handle precedence of
                    // inc/dec
                    (e.span(), Box::new(Expression::Dec(rhs)))
                }),
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

    fn expr_statement<
        T: Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>>
            + Clone
            + 'pratt,
    >(
        &self,
        expr: T,
    ) -> impl Parser<'pratt, &'pratt str, Output<'pratt>, extra::Err<Rich<'pratt, char>>> + Clone + 'pratt
    {
        expr.then_ignore(op!(';')).map_with(output!(ExprStatement))
    }
}

#[cfg(test)]
mod tests {
    use std::{borrow::Borrow, fmt::Display};

    use crate::{Pratt, pratt::Expression};
    use chumsky::Parser;

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
                Self::Fragment(list) => write!(
                    f,
                    "{}",
                    list.iter()
                        .map(|e| e.1.to_string())
                        .collect::<Vec<String>>()
                        .join(" ")
                ),
                Self::Group(g) => write!(f, "({})", g.1),
                e => todo!("Missing rest of nodes: {:?}", e),
            }
        }
    }

    macro_rules! parse {
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

    macro_rules! same {
        ($case: literal) => {
            assert_eq!($case.to_string(), parse!($case));
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
    }
}
