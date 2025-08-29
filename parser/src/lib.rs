use chumsky::{prelude::any, text, Parser};

#[derive(Copy, Clone)]
pub enum TokenKind {
    FUNCTION,
    INTEGER,
    FLOAT,
    STRING,
}

#[derive(Copy, Clone)]
pub struct Token<'token>(TokenKind, &'token str, usize, usize);

impl<'token> Token<'token> {
    pub fn new(kind: TokenKind, lexeme: &'token str) -> Self {
        Self(kind, lexeme, 0, 0)
    }

    pub fn with_info(&mut self, line: usize, column: usize) {
        self.2 = line;
        self.3 = column;
    }

    pub fn lexeme(&self) -> &'token str {
        self.1
    }

    pub fn kind(&self) -> TokenKind {
        self.0
    }

    pub fn line(&self) -> usize {
        self.2
    }

    pub fn column(&self) -> usize {
        self.3
    }
}

#[derive(Debug)]
enum Expression<'expr> {
    Int(i64),
    Float(f64),

    Variable(&'expr str),

    Add(Box<Expression<'expr>>, Box<Expression<'expr>>),
    Sub(Box<Expression<'expr>>, Box<Expression<'expr>>),
    Mul(Box<Expression<'expr>>, Box<Expression<'expr>>),
    Div(Box<Expression<'expr>>, Box<Expression<'expr>>),

    Eq(Box<Expression<'expr>>, Box<Expression<'expr>>),
    Le(Box<Expression<'expr>>, Box<Expression<'expr>>),
    Gt(Box<Expression<'expr>>, Box<Expression<'expr>>),

    Call(&'expr str, Vec<Expression<'expr>>),

    Let {
        name: &'expr str,
        rhs: Box<Expression<'expr>>,
        then: Box<Expression<'expr>>,
    },
    Fn {
        name: &'expr str,
        args: Vec<&'expr str>,
        body: Box<Expression<'expr>>,
        then: Box<Expression<'expr>>,
    },

}


fn parser<'parser>() -> impl Parser<'parser, &'parser str, Expression<'parser>> {
    let int = text::int(10)
        .map(|s: &str| Expression::Int(s.parse().unwrap())).padded();
    // any()
    //     .filter(|c: &char| c.is_ascii_digit())
    //     .map(|c| Expression::Int(c.to_digit(10).expect("Valid integer") as i64))
    //     .padded_by(any().filter(|c: &char| c.is_whitespace()).repeated())
    int
}

fn eval<'eval>(expr: &'eval Expression<'eval>) -> Result<i64, String> {
    match expr {
        Expression::Int(x) => Ok(*x),
        Expression::Add(a, b) => Ok(eval(a)? + eval(b)?),
        Expression::Sub(a, b) => Ok(eval(a)? - eval(b)?),
        Expression::Mul(a, b) => Ok(eval(a)? * eval(b)?),
        Expression::Div(a, b) => Ok(eval(a)? / eval(b)?),
        _ => todo!(),
    }
}


#[cfg(test)]
mod test {
    use chumsky::Parser;

    use crate::{eval, parser};

    #[test]
    fn test_simple_parsing() {
        let p = std::env::current_dir().unwrap().with_file_name("test.0s");

        dbg!(&p);

        let src = std::fs::read_to_string(p.canonicalize().unwrap()).unwrap();

        match parser().parse(&src).into_result() {
            Ok(ast) => match eval(&ast) {
                Ok(output) => println!("Result: {}", output),
                Err(err) => println!("Evaluation Error: {}", err),
            }
            Err(parse_err) => parse_err.into_iter().for_each(|e| eprintln!("Parse Error: {}", e)),
        }

    }
}
