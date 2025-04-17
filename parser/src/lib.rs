use common::opcodes::Metadata;
use common::program::data::Data;
use rand::{Rng, distr::Alphanumeric};
use scanner::buffer::Buffer;
use std::ops::Add;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use rustc_hash::FxHashMap as HashMap;

pub mod precedence;

use common::program::program::Program;
use common::types::{Kind, Type};
use common::{
    Value,
    opcodes::{IR, Operation},
};
use precedence::Precedence;
use scanner::{
    Scanner,
    tokens::{Token, TokenKind},
};

mod typechecker;

use typechecker::TypeChecker;

pub struct Context {
    scanner: Scanner,
    current: Token,
    previous: Option<Token>,

    owner: Option<usize>,
}

const INCLUDE_PATHS: [&str; 5] = ["src/", "lib/", "external/", "vendor/", "deps/"];

impl Context {
    pub fn new(mut scanner: Scanner) -> Self {
        let current = scanner.scan();

        Self {
            scanner,
            current,
            previous: None,
            owner: None,
        }
    }

    #[must_use]
    pub fn current(&self) -> &Token {
        &self.current
    }

    #[must_use]
    pub fn previous(&self) -> Option<Token> {
        self.previous.clone()
    }

    pub fn advance(&mut self) {
        self.previous = Some(self.current.clone());

        self.current = self.scanner.scan();
    }

    #[must_use]
    pub fn tell(&self) -> usize {
        self.scanner.tell()
    }

    pub fn set_owner(&mut self, owner: usize) {
        self.owner = Some(owner);
    }

    pub fn clear_owner(&mut self) {
        self.owner = None;
    }

    pub fn owner(&self) -> Option<usize> {
        self.owner
    }
}

pub struct Parser<'data> {
    data: &'data mut Data,
    file: String,
    typechecker: TypeChecker<64>,

    scanned: Vec<String>,
    aliases: HashMap<String, String>,
    namespace: String,
}

macro_rules! op {
    ($this:expr, $ctx:expr, $op:ident, $value:expr) => {{
        let mut ir = IR::new(Operation::$op, $value);
        if let Some(token) = $ctx.previous() {
            let data = Metadata::new(token.start_line(), token.start_column());
            ir.attach_metadata(data);
        }

        ir
    }};
}

impl<'data> Parser<'data> {
    pub fn new(file: String, data: &'data mut Data) -> Self {
        Self {
            data,
            file,
            typechecker: TypeChecker::default(),
            scanned: Vec::default(),
            aliases: HashMap::default(),
            namespace: String::new(),
        }
    }

    fn name(&mut self, name: String, constant: Option<usize>) -> usize {
        self.data.add_symbol(
            format!("{}::{}", self.namespace, name)
                .trim_start_matches("::")
                .to_string(),
            constant,
        )
    }

    fn get_type(&mut self, ctx: &mut Context) -> usize {
        // ctx.advance();
        let mut r#type = Type::new(if self.consume(ctx, TokenKind::Int) {
            Kind::Integer
        } else if self.consume(ctx, TokenKind::Float) {
            Kind::Float
        } else if self.consume(ctx, TokenKind::Str) {
            Kind::String
        } else if self.consume(ctx, TokenKind::Void) {
            Kind::None
        } else if self.consume(ctx, TokenKind::Result) {
            Kind::Result
        } else if self.consume(ctx, TokenKind::Dolar) {
            let symbol = self
                .data
                .add_symbol(ctx.current().lexeme().to_string(), None);
            self.expect(ctx, TokenKind::Identifier, "Expected generic identifier");
            let mut kind = self.data.add_type(Type::void());

            if self.consume(ctx, TokenKind::Equal) {
                kind = self.get_type(ctx);
            }

            Kind::Generic(symbol, kind)
        } else if self.consume(ctx, TokenKind::Identifier) {
            let mut name = ctx.current().lexeme().to_string();
            if self.aliases.contains_key(&name) {
                name = self.aliases[&name].to_string();
            }

            let n = self.data.add_symbol(name, None);

            Kind::Object(n)
        } else {
            eprintln!(
                "Unknown token to be used as type: {:?}",
                ctx.previous().unwrap().lexeme()
            );

            Kind::None
        });
        // ctx.advance();

        // let mut r#type = Type::new(match ctx.previous().map(|p| p.kind()) {
        //     Some(TokenKind::Int) => Kind::Integer,
        //     Some(TokenKind::Float) => Kind::Float,
        //     Some(TokenKind::Str) => Kind::String,
        //     // TokenKind::Identifier => {
        //     //     ctx.advance();
        //     //     Kind::Object(self.name(ctx.current().lexeme().to_string(), None))
        //     // }
        //     Some(TokenKind::Void) => Kind::None,
        //     Some(TokenKind::Result) => Kind::Result,
        //     Some(TokenKind::Identifier) => {
        //         let mut name = ctx.current().lexeme().to_string();
        //         if self.aliases.contains_key(&name) {
        //             name = self.aliases[&name].to_string();
        //         }
        //
        //         let n = self.data.add_symbol(name, None);
        //
        //         Kind::Object(n)
        //     }
        //     _ => {
        //         eprintln!("Unknown token to be used as value: {:?}", ctx.current());
        //
        //         Kind::None
        //     }
        // });
        // ctx.advance();

        if self.consume(ctx, TokenKind::Less) {
            r#type.add(self.get_type(ctx));
            while self.consume(ctx, TokenKind::Comma) {
                r#type.add(self.get_type(ctx));
            }

            self.expect(
                ctx,
                TokenKind::Greater,
                "Expected '>' to close type parameter list.",
            );
        }
        // else if self.consume(ctx, TokenKind::And) {
        //     let mut t = Type::new(Kind::Intersection);
        //     t.add(self.data.add_type(r#type));
        //     t.add(self.get_type(ctx));
        //     while self.consume(ctx, TokenKind::Pipe) || self.matches(ctx, TokenKind::And) {
        //         t.add(self.get_type(ctx));
        //     }
        //
        //     r#type = t;
        // } else if self.consume(ctx, TokenKind::Pipe) {
        //     let mut t = Type::new(Kind::Union);
        //     t.add(self.data.add_type(r#type));
        //     t.add(self.get_type(ctx));
        //     while self.consume(ctx, TokenKind::Pipe) || self.matches(ctx, TokenKind::And) {
        //         t.add(self.get_type(ctx));
        //     }
        //
        //     r#type = t;
        // }
        // else if self.consume(ctx, TokenKind::LeftParenthesis) {
        //     while !self.consume(ctx, TokenKind::RightParenthesis) {
        //         let mut t = r#type.add(kind);
        //         t.add(
        //     }
        // }

        // eprintln!(
        //     "TYPE: {}: {}",
        //     r#type.output(self.data),
        //     ctx.current().lexeme()
        // );

        self.data.add_type(r#type)
    }

    fn matches(&self, ctx: &Context, token: TokenKind) -> bool {
        ctx.current.kind() == token || ctx.current.kind() == TokenKind::EOF
    }

    fn consume(&self, ctx: &mut Context, token: TokenKind) -> bool {
        if self.matches(ctx, token) {
            ctx.advance();

            true
        } else {
            false
        }
    }

    fn expect(&self, ctx: &mut Context, token: TokenKind, message: &str) -> bool {
        let matched = self.consume(ctx, token);

        if !matched {
            eprintln!(
                "ERROR: {}. Current: {:?}('{}')",
                message,
                ctx.current.kind(),
                ctx.current.lexeme()
            );
            panic!("SADGE");
        }

        matched
    }

    fn grouping(&mut self, ctx: &mut Context) -> Vec<IR> {
        ctx.advance();

        let tokens = self.expression(ctx);

        self.expect(
            ctx,
            TokenKind::RightParenthesis,
            "Expected ')' to close group",
        );

        tokens
    }

    fn boolean(&mut self, ctx: &mut Context) -> Vec<IR> {
        let value = match ctx.current().kind() {
            TokenKind::True => Value::BOOLEAN(true),
            TokenKind::False => Value::BOOLEAN(false),
            _ => todo!("Fail to build a boolean"),
        };
        let ty = self.data.add_type(Type::bool());
        let constant = self.data.add_constant(value, ty);

        ctx.advance();
        vec![op!(self, ctx, Const, Some([constant, 0, 0]))]
    }

    fn number(&mut self, ctx: &mut Context) -> Vec<IR> {
        let value = match ctx.current().lexeme().as_bytes().get(1).map(|c| *c as char) {
            Some('o') => {
                if let Ok(int) = i64::from_str_radix(ctx.current().lexeme(), 8) {
                    Value::INTEGER(int)
                } else {
                    todo!("Fail to parse number as octal");
                }
            }
            Some('x') => {
                if let Ok(int) = i64::from_str_radix(ctx.current().lexeme(), 16) {
                    Value::INTEGER(int)
                } else {
                    todo!("Fail to parse number as hexadecimal");
                }
            }
            Some('b') => {
                if let Ok(int) = i64::from_str_radix(ctx.current().lexeme(), 2) {
                    Value::INTEGER(int)
                } else {
                    todo!("Fail to parse number as binary");
                }
            }
            _ => {
                if let Ok(int) = ctx.current().lexeme().parse::<i64>() {
                    Value::INTEGER(int)
                } else {
                    todo!("Fail to parse '{}' as decimal", ctx.current().lexeme());
                }
            }
        };
        ctx.advance();

        let ty = self.data.add_type(Type::integer());
        let constant = self.data.add_constant(value, ty);

        vec![op!(self, ctx, Const, Some([constant, 0, 0]))]
    }

    fn float(&mut self, ctx: &mut Context) -> Vec<IR> {
        let value = if let Ok(value) = f64::from_str(ctx.current().lexeme()) {
            Value::FLOAT(value)
        } else {
            todo!("Fail to parse number as float");
        };

        ctx.advance();
        let constant = self
            .data
            .add_constant(value, self.data.find_type(Type::float()));

        vec![op!(self, ctx, Const, Some([constant, 0, 0]))]
    }

    fn string(&mut self, ctx: &mut Context) -> Vec<IR> {
        let string = self.data.add_string(ctx.current().lexeme().to_string());
        let constant = self
            .data
            .add_constant(Value::STR(string), self.data.find_type(Type::string()));

        ctx.advance();
        vec![op!(self, ctx, Const, Some([constant, 0, 0]))]
    }

    fn identifier(&mut self, ctx: &mut Context) -> Vec<IR> {
        let mut name = ctx.current().lexeme().to_string();

        let mut symbol = self.data.add_symbol(name.to_string(), None);
        ctx.advance();

        let mut tokens = vec![];
        if self.consume(ctx, TokenKind::LeftParenthesis) {
            let mut arity = 0;
            while !self.consume(ctx, TokenKind::RightParenthesis) {
                tokens.append(&mut self.expression(ctx));
                self.consume(ctx, TokenKind::Comma);
                arity += 1;
            }
            if self.aliases.contains_key(&name) {
                name = self.aliases[&name].clone();
            }

            symbol = self.data.add_symbol(name, None);

            tokens.push(op!(self, ctx, Call, Some([symbol, arity, 0])));
        } else if self.consume(ctx, TokenKind::Equal) {
            tokens.append(&mut self.expression(ctx));
            tokens.push(op!(self, ctx, Assign, Some([symbol, 0, 0])));
        } else {
            // tokens.append(&mut self.expression(ctx));
            tokens.push(op!(self, ctx, Load, Some([symbol, 0, 0])));
        } // TODO: handle other tokens on identifiers, like increment, decrement, etc.

        tokens
    }

    fn array(&mut self, ctx: &mut Context) -> Vec<IR> {
        let mut tokens = vec![];
        let mut arity = 0;
        if self.consume(ctx, TokenKind::LeftBrace) {
            while !self.consume(ctx, TokenKind::RightBrace) {
                tokens.append(&mut self.expression(ctx));
                self.consume(ctx, TokenKind::Comma);
                arity += 1;
            }
        }

        tokens.push(op!(self, ctx, Array, Some([arity, 0, 0])));

        tokens
    }

    fn comparison(&mut self, ctx: &mut Context) -> Vec<IR> {
        let operator = ctx.current().clone();
        ctx.advance();
        let mut tokens = self.precedence(ctx, Precedence::get(operator.kind()).next());

        match operator.kind() {
            TokenKind::Less => tokens.push(op!(self, ctx, Less, None)),
            TokenKind::LessEqual => tokens.push(op!(self, ctx, LessEqual, None)),
            TokenKind::Greater => tokens.push(op!(self, ctx, Greater, None)),
            TokenKind::GreaterEqual => tokens.push(op!(self, ctx, GreaterEqual, None)),
            _ => unreachable!("No other comparison"),
        }

        tokens
    }

    fn equality(&mut self, ctx: &mut Context) -> Vec<IR> {
        let operator = ctx.current().clone();
        ctx.advance();
        let mut tokens = self.precedence(ctx, Precedence::get(operator.kind()).next());

        match operator.kind() {
            TokenKind::EqualEqual => tokens.push(op!(self, ctx, Equal, None)),
            TokenKind::BangEqual => tokens.push(op!(self, ctx, NotEqual, None)),
            _ => unreachable!("No other equality"),
        }

        tokens
    }

    fn binary(&mut self, ctx: &mut Context, _assignment: bool) -> Vec<IR> {
        let operator = ctx.current().clone();
        ctx.advance();
        let mut tokens = self.precedence(ctx, Precedence::get(operator.kind()).next());

        match operator.kind() {
            TokenKind::Plus => tokens.push(op!(self, ctx, Add, None)),
            TokenKind::Minus => tokens.push(op!(self, ctx, Subtract, None)),
            TokenKind::Star => tokens.push(op!(self, ctx, Multiply, None)),
            TokenKind::StarStar => tokens.push(op!(self, ctx, Pow, None)),
            TokenKind::Slash => tokens.push(op!(self, ctx, Divide, None)),
            TokenKind::Percent => tokens.push(op!(self, ctx, Modulo, None)),
            TokenKind::Less => tokens.push(op!(self, ctx, Less, None)),
            TokenKind::LessLess => tokens.push(op!(self, ctx, LeftShift, None)),
            TokenKind::Greater => tokens.push(op!(self, ctx, Greater, None)),
            TokenKind::GreaterGreater => tokens.push(op!(self, ctx, RightShift, None)),
            TokenKind::Caret => tokens.push(op!(self, ctx, BitXor, None)),
            TokenKind::Pipe => tokens.push(op!(self, ctx, BitOr, None)),
            TokenKind::Ampersand => tokens.push(op!(self, ctx, BitAnd, None)),
            TokenKind::Or => tokens.push(op!(self, ctx, Or, None)),
            TokenKind::And => tokens.push(op!(self, ctx, And, None)),
            _ => unreachable!("Unknown binary operator '{}'", operator.lexeme()),
        }

        tokens
    }

    fn infix(&mut self, ctx: &mut Context, assignment: bool) -> Vec<IR> {
        match ctx.current().kind() {
            TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::StarStar
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::Pipe
            | TokenKind::Ampersand
            | TokenKind::Caret
            | TokenKind::LessLess
            | TokenKind::GreaterGreater
            | TokenKind::And
            | TokenKind::Or => self.binary(ctx, assignment),
            TokenKind::EqualEqual | TokenKind::BangEqual => self.equality(ctx),
            TokenKind::Less
            | TokenKind::LessEqual
            | TokenKind::Greater
            | TokenKind::GreaterEqual => self.comparison(ctx),
            TokenKind::Dot => self.call(ctx),
            TokenKind::DotDot => self.range(ctx),
            _ => todo!("Unexpected token '{}'", ctx.current.lexeme()),
        }
    }

    fn parse_pattern_branch(&mut self, ctx: &mut Context, predicate: &Vec<IR>) -> Vec<IR> {
        let mut tokens = vec![];

        let mut full_predicate = predicate.clone();
        full_predicate.append(&mut self.parse_pattern(ctx, predicate));

        tokens.append(&mut if self.expect(
            ctx,
            TokenKind::FatArrow,
            "Expected '=>' to denote match arm body",
        ) {
            if self.matches(ctx, TokenKind::LeftBracket) {
                self.block(ctx)
            } else {
                let mut t = self.expression(ctx);
                t.push(op!(self, ctx, Leave, None));

                t
            }
        } else {
            todo!("Unable to handle invalid match expression");
        });

        let mut result = vec![];
        result.append(&mut full_predicate);
        result.append(&mut tokens);
        result.insert(
            0,
            op!(
                self,
                ctx,
                Check,
                Some([full_predicate.len(), tokens.len(), 0])
            ),
        );

        result
    }

    fn parse_pattern(&mut self, ctx: &mut Context, predicate: &Vec<IR>) -> Vec<IR> {
        let mut tokens = vec![];

        match ctx.current().kind() {
            TokenKind::Some => {
                ctx.advance();
                if self.matches(ctx, TokenKind::LeftParenthesis) {
                    self.expect(
                        ctx,
                        TokenKind::LeftParenthesis,
                        "Expected '(' for sub-pattern",
                    );
                    tokens.push(op!(self, ctx, Unwrap, None));
                    tokens.append(&mut self.parse_pattern(ctx, predicate));
                    self.expect(
                        ctx,
                        TokenKind::RightParenthesis,
                        "Expected ')' for sub-pattern",
                    );
                } else {
                    todo!("Handle is_some() kind of checks");
                }
            }
            TokenKind::Err => {
                ctx.advance();
                if self.matches(ctx, TokenKind::LeftParenthesis) {
                    self.expect(
                        ctx,
                        TokenKind::LeftParenthesis,
                        "Expected '(' for sub-pattern",
                    );
                    tokens.push(op!(self, ctx, UnwrapError, None));
                    tokens.append(&mut self.parse_pattern(ctx, predicate));
                    self.expect(
                        ctx,
                        TokenKind::RightParenthesis,
                        "Expected ')' for sub-pattern",
                    );
                } else {
                    todo!("Handle is_some() kind of checks");
                    todo!("Handle is_err kind of checks");
                }
            }
            TokenKind::Default => {
                ctx.advance();
                tokens.append(&mut predicate.clone());
            }
            TokenKind::LeftBrace => {
                tokens.append(&mut self.parse_pattern(ctx, predicate));
                while self.matches(ctx, TokenKind::Comma) {
                    self.expect(
                        ctx,
                        TokenKind::Comma,
                        "Expecting comma, but didn't get one.",
                    );
                    tokens.append(&mut self.parse_pattern(ctx, predicate));
                }
            }
            TokenKind::Identifier => {
                let symbol = self
                    .data
                    .add_symbol(ctx.current().lexeme().to_string(), None);
                // ctx.advance();
                tokens.append(&mut self.expression(ctx));
                tokens.push(op!(self, ctx, Store, Some([symbol, 0, 0])));
            }
            _ => {
                tokens.append(&mut self.expression(ctx));
                // ctx.advance();
                // tokens.append(&mut self.expression(ctx));
            } // _ => {
              //     ctx.advance();
              //     tokens.append(&mut self.expression(ctx));
              // }
        }

        if self.matches(ctx, TokenKind::Pipe) {
            ctx.advance();
            tokens.append(&mut self.parse_pattern(ctx, predicate));
            tokens.push(op!(self, ctx, Or, None));
        }

        tokens.append(&mut predicate.clone());
        tokens.push(op!(self, ctx, Equal, None));
        // tokens.push(op!(self, ctx,ConditionJump, None));

        tokens
    }

    fn match_expression2(&mut self, ctx: &mut Context) -> Vec<IR> {
        self.expect(ctx, TokenKind::Match, "Expected 'match' keyword'");
        let expr = self.expression(ctx);

        let mut tokens: Vec<IR> = vec![];

        self.expect(
            ctx,
            TokenKind::LeftBracket,
            "Expecting '{' denoting the match body",
        );

        while !self.consume(ctx, TokenKind::RightBracket) {
            // self.parse_pattern_branch(ctx, &expr);
            tokens.append(&mut self.parse_pattern_branch(ctx, &expr));

            self.consume(ctx, TokenKind::Comma);
        }

        tokens.insert(0, op!(self, ctx, Match, Some([tokens.len(), 0, 0])));

        tokens
    }

    fn match_expression(&mut self, ctx: &mut Context) -> Vec<IR> {
        self.expect(ctx, TokenKind::Match, "Expected a 'match' keyword");
        let mut tokens = vec![];
        let predicate = self.expression(ctx);

        self.expect(ctx, TokenKind::LeftBracket, "Expected '{' for match body");

        // let mut has_prev = None;
        let mut jumps = vec![];
        while !self.matches(ctx, TokenKind::RightBracket) {
            tokens.append(&mut self.parse_pattern_branch(ctx, &predicate));
            jumps.push(tokens.len() - 1);

            if !self.matches(ctx, TokenKind::Comma) {
                break;
            }

            self.expect(ctx, TokenKind::Comma, "Expected comma");
        }

        {
            // Handles the removal of trailing jump which is obsolete as the execution
            // should continue normally onwards
            jumps.pop();
            tokens.pop();
        }

        // jumps.iter().for_each(|jump| self.patch(&mut tokens, *jump));

        self.expect(ctx, TokenKind::RightBracket, "Unterminated match body");

        tokens
    }

    fn if_expression(&mut self, ctx: &mut Context) -> Vec<IR> {
        self.consume(ctx, TokenKind::If);

        let mut condition = self.expression(ctx);
        let mut body = if self.matches(ctx, TokenKind::LeftBracket) {
            self.block(ctx)
        } else {
            self.expression(ctx)
        };

        let mut alternative = vec![];
        if self.matches(ctx, TokenKind::Else) {
            alternative = self.else_expression(ctx);
        }

        let mut tokens = vec![];
        let condition_len = condition.len();
        let body_len = body.len();
        tokens.append(&mut condition);
        tokens.append(&mut body);
        tokens.insert(
            0,
            op!(
                self,
                ctx,
                Condition,
                Some([condition_len, body_len, alternative.len()])
            ),
        );
        tokens.append(&mut alternative);

        tokens
    }

    fn else_expression(&mut self, ctx: &mut Context) -> Vec<IR> {
        self.consume(ctx, TokenKind::Else);

        if self.matches(ctx, TokenKind::LeftBracket) {
            self.block(ctx)
        } else {
            self.expression(ctx)
        }
    }

    fn while_(&mut self, ctx: &mut Context) -> Vec<IR> {
        self.consume(ctx, TokenKind::While);
        let mut result = self.expression(ctx);
        let condition_len = result.len();
        result.append(&mut self.block(ctx));
        let body_len = result.len() - condition_len;

        result.insert(0, op!(self, ctx, Loop, Some([condition_len, body_len, 0])));

        result
    }

    fn for_in(&mut self, ctx: &mut Context) -> Vec<IR> {
        let mut result = vec![];
        let name = ctx.current().to_owned();
        if self.expect(
            ctx,
            TokenKind::Identifier,
            "Expecting variable name to hold iteration",
        ) {
            if self.expect(ctx, TokenKind::In, "Expecting 'in' for loop") {
                result.append(&mut self.expression(ctx));
                let mut body = self.block(ctx);
                result.push(op!(
                    self,
                    ctx,
                    Iterate,
                    Some([
                        self.data.add_symbol(name.lexeme().to_string(), None),
                        body.len(),
                        0,
                    ])
                ));
                result.append(&mut body);
            }
        }

        result
    }

    fn boomer_loop(&mut self, ctx: &mut Context) -> Vec<IR> {
        self.consume(ctx, TokenKind::For);
        let mut result = vec![];

        self.expect(
            ctx,
            TokenKind::LeftParenthesis,
            "Expecting '(' for block loop header",
        );
        let mut initializer = self.expression(ctx);
        self.expect(ctx, TokenKind::SemiColon, "Expecting ';' after initializer");
        let mut condition = self.expression(ctx);
        self.expect(ctx, TokenKind::SemiColon, "Expecting ';' after condition");
        let mut action = self.expression(ctx);
        self.expect(
            ctx,
            TokenKind::RightParenthesis,
            "Expecting ')' for block loop header",
        );

        let mut body = if self.matches(ctx, TokenKind::LeftBracket) {
            self.block(ctx)
        } else {
            self.expr_statement(ctx)
        };

        action.push(op!(self, ctx, Pop, Some([1, 0, 0])));
        body.append(&mut action);

        result.append(&mut initializer);
        result.push(op!(self, ctx, Loop, Some([condition.len(), body.len(), 0])));
        result.append(&mut condition);
        // result.push(op!(self, ctx,Rewind, Some([body.len(), 0, 0])));
        result.append(&mut body);

        result
    }

    fn call(&mut self, ctx: &mut Context) -> Vec<IR> {
        self.expect(ctx, TokenKind::Dot, "Expected '.' for call expression");
        let mut tokens = vec![];
        let name = ctx.current().lexeme().to_string();

        let mut arity = 0;

        if self.expect(ctx, TokenKind::Identifier, "Expected function name") {
            if self.consume(ctx, TokenKind::LeftParenthesis) {
                let mut invoke = op!(
                    self,
                    ctx,
                    Invoke,
                    Some([self.data.add_symbol(name, None), arity, 0])
                );

                while !self.consume(ctx, TokenKind::RightParenthesis) {
                    arity += 1;
                    tokens.append(&mut self.expression(ctx));

                    self.consume(ctx, TokenKind::Comma);
                }
                invoke.operands_mut()[1] = arity;

                tokens.push(invoke);
            } else if self.consume(ctx, TokenKind::Equal) {
                tokens.append(&mut self.expression(ctx));
                tokens.push(op!(
                    self,
                    ctx,
                    Prop,
                    Some([self.data.add_symbol(name, None), 1, 0])
                ));
            } else {
                tokens.push(op!(
                    self,
                    ctx,
                    Prop,
                    Some([self.data.add_symbol(name, None), 0, 0])
                ));
                // todo!("Handle remainder of cases");
            } // TODO: Implement Increment for properties
        }

        tokens
    }

    fn range(&mut self, ctx: &mut Context) -> Vec<IR> {
        self.expect(ctx, TokenKind::DotDot, "Expected range symbol");
        let mut tokens = self.expression(ctx);
        tokens.push(op!(self, ctx, Range, None));

        tokens
    }

    fn prefix(&mut self, ctx: &mut Context, _assignment: bool) -> Vec<IR> {
        match ctx.current().kind() {
            TokenKind::LeftParenthesis => self.grouping(ctx),
            TokenKind::True | TokenKind::False => self.boolean(ctx),
            TokenKind::Number => self.number(ctx),
            TokenKind::Double => self.float(ctx),
            TokenKind::String => self.string(ctx),
            TokenKind::Dot => self.call(ctx),
            TokenKind::Identifier => self.identifier(ctx),
            TokenKind::This => self.this(ctx),
            TokenKind::LeftBrace => self.array(ctx),
            TokenKind::Match => self.match_expression2(ctx),
            TokenKind::If => self.if_expression(ctx),
            TokenKind::Let => self.variable(ctx),
            TokenKind::Function => self.function(ctx),
            TokenKind::New => self.initialize(ctx),
            TokenKind::LeftBracket => self.block(ctx),
            _ => todo!("Unimplemented token '{:?}'", ctx.current()),
        }
    }

    fn precedence(&mut self, ctx: &mut Context, precedence: Precedence) -> Vec<IR> {
        let mut tokens = vec![];
        let assignment = precedence <= Precedence::Assign;

        tokens.append(&mut self.prefix(ctx, assignment));

        while precedence <= Precedence::get(ctx.current().kind()) {
            tokens.append(&mut self.infix(ctx, assignment));
        }

        tokens
    }

    fn expression(&mut self, ctx: &mut Context) -> Vec<IR> {
        // if self.matches(ctx, TokenKind::Match) {
        //     self.expect(
        //         ctx,
        //         TokenKind::Match,
        //         "Something went wrong with parsing match",
        //     );
        //     self.match_expression(ctx)
        // } else if self.matches(ctx, TokenKind::If) {
        //     self.if_expression(ctx)
        // } else {
        self.precedence(ctx, Precedence::Assign)

        // }
    }

    fn expr(&mut self, ctx: &mut Context) -> Vec<IR> {
        let tokens = self.expression(ctx);
        self.expect(
            ctx,
            TokenKind::SemiColon,
            "Expected ';' at end of expression.",
        );

        tokens
    }

    fn expr_statement(&mut self, ctx: &mut Context) -> Vec<IR> {
        let mut tokens = self.expr(ctx);

        tokens.push(op!(self, ctx, Pop, None));

        tokens
    }

    fn block(&mut self, ctx: &mut Context) -> Vec<IR> {
        let mut tokens = vec![op!(self, ctx, Begin, None)];

        if self.expect(
            ctx,
            TokenKind::LeftBracket,
            "Expecting '{' at the start of a block",
        ) {
            while !self.consume(ctx, TokenKind::RightBracket) {
                tokens.append(&mut self.statement(ctx));
            }
        }

        tokens.push(op!(self, ctx, End, None));

        tokens
    }

    fn variable(&mut self, ctx: &mut Context) -> Vec<IR> {
        self.consume(ctx, TokenKind::Let);
        let name = ctx.current().clone();
        self.expect(
            ctx,
            TokenKind::Identifier,
            "Expected identifier for variable name",
        );
        let mut kind = self.data.add_type(Type::void());
        if self.consume(ctx, TokenKind::Colon) {
            kind = self.get_type(ctx);
        }

        let mut tokens = vec![];
        if self.consume(ctx, TokenKind::Equal) {
            tokens.append(&mut self.expression(ctx));
        } else {
            tokens.push(op!(
                self,
                ctx,
                Const,
                Some([self.data.add_constant(Value::NONE, 0), 0, 0,])
            ));
        }

        let mut declaration = op!(
            self,
            ctx,
            Declare,
            Some([self.data.add_symbol(name.lexeme().to_string(), None), 0, 0])
        );

        declaration.set_type(kind);
        tokens.push(declaration);

        tokens
    }
    fn constant(&mut self, ctx: &mut Context) -> Vec<IR> {
        self.consume(ctx, TokenKind::Const);
        let name = ctx.current().clone();
        self.expect(
            ctx,
            TokenKind::Identifier,
            "Expected identifier for variable name",
        );
        let mut kind = self.data.add_type(Type::void());
        if self.consume(ctx, TokenKind::Colon) {
            kind = self.get_type(ctx);
        }

        let mut tokens = self.expr(ctx);
        tokens.pop();

        let mut declaration = op!(
            self,
            ctx,
            Declare,
            Some([self.data.add_symbol(name.lexeme().to_string(), None), 1, 0])
        );

        declaration.set_type(kind);
        tokens.push(declaration);

        tokens
    }

    fn function(&mut self, ctx: &mut Context) -> Vec<IR> {
        let mut tokens = vec![];

        self.consume(ctx, TokenKind::Function);

        let mut name: String = if self.consume(ctx, TokenKind::Identifier) {
            ctx.previous().unwrap_or_default().lexeme().to_string()
        } else {
            rand::rng()
                .sample_iter(&Alphanumeric)
                .take(8)
                .map(char::from)
                .collect()
        };

        let mut body = vec![];
        self.expect(
            ctx,
            TokenKind::LeftParenthesis,
            "Missing '(' for argument list",
        );

        let mut arity: usize = 0;
        let mut argument_types = vec![];
        while !self.matches(ctx, TokenKind::RightParenthesis) {
            // ctx.advance();
            let kind = self.get_type(ctx);
            let argument = ctx.current().clone();

            if self.expect(
                ctx,
                TokenKind::Identifier,
                "Expected function argument identifier",
            ) {
                let mut arg = op!(
                    self,
                    ctx,
                    Argument,
                    Some([
                        self.data.add_symbol(argument.lexeme().to_string(), None),
                        1,
                        0,
                    ])
                );

                arg.set_type(kind);
                argument_types.push(kind);
                body.push(arg);
            }
            arity += 1;

            if !self.consume(ctx, TokenKind::Comma) {
                break;
            }
        }
        self.expect(
            ctx,
            TokenKind::RightParenthesis,
            "Expected ')' to close off argument list.",
        );
        if self.consume(ctx, TokenKind::Use) {
            self.consume(ctx, TokenKind::LeftParenthesis);

            let mut upvalues = vec![];
            while !self.consume(ctx, TokenKind::RightParenthesis) {
                let name = ctx.current().clone();
                self.expect(ctx, TokenKind::Identifier, "Expected variable name");
                upvalues.push(op!(
                    self,
                    ctx,
                    Upvalue,
                    Some([self.data.add_symbol(name.lexeme().to_string(), None), 0, 0])
                ));
                self.consume(ctx, TokenKind::Comma);
            }

            body.append(&mut upvalues);
        }
        let mut kind = self.data.add_type(Type::void()); // Kind::None;
        if self.consume(ctx, TokenKind::SlimArrow) {
            kind = self.get_type(ctx);
        }
        body.append(&mut self.block(ctx));

        let symbol = self.name(name, None);

        let mut func = op!(self, ctx, Function, Some([symbol, arity, body.len()]));
        let mut func_type = Type::function();
        func_type.set_return(kind);
        for arg in argument_types {
            func_type.add(arg);
        }

        func.set_type(self.data.add_type(func_type));

        tokens.push(func);
        body.push(op!(
            self,
            ctx,
            Const,
            Some([self.data.add_constant(Value::NONE, 0), 0, 0,])
        ));
        body.push(op!(self, ctx, Leave, None));
        tokens.append(&mut body);

        tokens
    }

    fn prop(&mut self, ctx: &mut Context, owner: usize, public: bool) -> Vec<IR> {
        let mut kind = self.data.add_type(Type::void());

        if !self.matches(ctx, TokenKind::Identifier) {
            kind = self.get_type(ctx);
        }

        let prop_name = ctx.current.lexeme().to_string();
        self.consume(ctx, TokenKind::Identifier);

        let mut prop = if self.consume(ctx, TokenKind::SemiColon) {
            vec![op!(
                self,
                ctx,
                Const,
                Some([self.data.add_constant(Value::NONE, 0), 0, 0])
            )]
        } else {
            self.expr_statement(ctx)
        };
        let mut symbol = owner;
        symbol = symbol << 32;
        symbol |= self.data.add_symbol(prop_name, None);

        let mut property = op!(self, ctx, Prop, Some([symbol, 2, usize::from(public),]));
        property.set_type(kind);
        prop.push(property);

        prop
    }

    fn method(&mut self, ctx: &mut Context, owner: usize, public: bool) -> Vec<IR> {
        let ns = self.namespace.clone();
        self.namespace = String::new();
        let mut method = self.function(ctx);

        method.insert(1, IR::new(Operation::This, None));
        if let Some(code) = method.first_mut() {
            let operands = code.operands();
            let mut symbol = owner;
            symbol = symbol << 16;
            symbol |= operands[0];
            symbol = symbol << 1;
            symbol |= usize::from(public);

            let mut method = op!(self, ctx, Method, Some([symbol, operands[1], operands[2]]));
            method.set_type(code.kind());
            // method.attach_metadata(*code.metadata().unwrap());
            *code = method;
        }
        self.namespace = ns;

        method
    }

    fn implement(&mut self, ctx: &mut Context) -> Vec<IR> {
        self.consume(ctx, TokenKind::Identifier);
        let interface = ctx.previous().unwrap().lexeme().to_string();
        self.expect(
            ctx,
            TokenKind::For,
            "Expected target for the contract implementation",
        );
        let contract = self.data.add_symbol(interface, None);
        self.consume(ctx, TokenKind::Identifier);
        let name = ctx.previous().unwrap().lexeme().to_string();
        let owner = self.data.add_symbol(name, None);

        if ctx.owner().is_some() {
            panic!("Classes can only be declared outisde any conditional blocks");
        }

        ctx.set_owner(owner);
        self.expect(
            ctx,
            TokenKind::LeftBracket,
            "Expected '{' denoting class body",
        );
        let mut class = vec![];
        while !self.consume(ctx, TokenKind::RightBracket) {
            let public = self.consume(ctx, TokenKind::Pub);

            if self.consume(ctx, TokenKind::Prop) {
                class.append(&mut self.prop(ctx, owner, public));
            } else if self.matches(ctx, TokenKind::Function) {
                class.append(&mut self.method(ctx, owner, public));
                // let mut method = self.function(ctx);
                // if let Some(code) = method.first_mut() {
                //     let operands = code.operands();
                //     let mut method =
                //         op!(self, ctx, Method, Some([owner, operands[0], operands[2]]));
                //     method.set_type(code.kind());
                //     *code = method;
                // }
                // class.append(&mut method);
            }
        }

        class.insert(0, op!(self, ctx, Begin, None));
        class.insert(
            0,
            op!(self, ctx, Implement, Some([contract, owner, class.len()])),
        );
        class.push(op!(self, ctx, End, None));

        class
    }

    fn interface(&mut self, ctx: &mut Context) -> Vec<IR> {
        let mut interface = vec![];
        let name = if self.consume(ctx, TokenKind::Identifier) {
            ctx.previous().unwrap().lexeme().to_string()
        } else {
            "sad".to_string()
        };
        let owner = self.name(name, None);
        if ctx.owner().is_some() {
            panic!("Classes can only be declared outisde any conditional blocks");
        }
        ctx.set_owner(owner);

        let mut iface = op!(self, ctx, Interface, Some([owner, 0, 0]));

        ctx.advance();
        while !self.consume(ctx, TokenKind::RightBracket) {
            let mut method = vec![];
            if self.consume(ctx, TokenKind::Pub) {
                eprintln!("Interface methods are implicitly public, so 'pub' is not needed here");
            }
            self.expect(
                ctx,
                TokenKind::Function,
                "Interfaces could define only functions",
            );

            self.expect(ctx, TokenKind::Identifier, "Expected method name");
            let method_name = ctx.previous().unwrap().lexeme().to_string();
            self.expect(
                ctx,
                TokenKind::LeftParenthesis,
                "Expected '(' for argument list",
            );
            let mut body = vec![];
            let mut arity = 0;
            while !self.matches(ctx, TokenKind::RightParenthesis) {
                let type_ = match ctx.current.kind() {
                    TokenKind::Int => Kind::Integer,
                    TokenKind::Str => Kind::String,
                    TokenKind::Bool => Kind::Bool,
                    TokenKind::Float => Kind::Float,
                    _ => todo!(
                        "Unknown type: {}. Investigate support for additional types.",
                        ctx.current().lexeme()
                    ),
                };
                ctx.advance();
                let argument = ctx.current().clone();

                if self.expect(
                    ctx,
                    TokenKind::Identifier,
                    "Expected argument name identifier",
                ) {
                    body.push(op!(
                        self,
                        ctx,
                        Argument,
                        Some([
                            self.data.add_symbol(argument.lexeme().to_string(), None),
                            1,
                            0,
                        ])
                    ));
                    arity += 1;
                    if !self.consume(ctx, TokenKind::Comma) {
                        break;
                    }
                }
            }

            self.expect(
                ctx,
                TokenKind::RightParenthesis,
                "Expected ')' to close argument list",
            );

            if self.consume(ctx, TokenKind::SlimArrow) {
                let type_ = match ctx.current.kind() {
                    TokenKind::Int => Kind::Integer,
                    TokenKind::Str => Kind::String,
                    TokenKind::Bool => Kind::Bool,
                    TokenKind::Float => Kind::Float,
                    _ => todo!(
                        "Unknown type: {}. Investigate support for additional types.",
                        ctx.current().lexeme()
                    ),
                };
                ctx.advance();
            }
            if self.matches(ctx, TokenKind::LeftBracket) {
                body.append(&mut self.block(ctx));
            } else {
                self.expect(
                    ctx,
                    TokenKind::SemiColon,
                    "Expecting ';' at end of method declaration",
                );
            }

            let name_symbol = self.data.add_symbol(method_name, None);
            let mut symbol = owner;
            symbol = symbol << 16;
            symbol |= name_symbol;
            symbol = symbol << 1;
            symbol |= 1;

            method.insert(0, op!(self, ctx, Method, Some([symbol, arity, body.len()])));
            method.append(&mut body);

            interface.append(&mut method);
        }

        interface.insert(0, op!(self, ctx, Begin, None));
        interface.push(op!(self, ctx, End, None));
        iface.operands_mut()[1] = interface.len();
        iface.operands_mut()[2] = interface.len();
        interface.insert(0, iface);

        interface
    }

    fn class(&mut self, ctx: &mut Context) -> Vec<IR> {
        let mut class = vec![];
        let name = if self.consume(ctx, TokenKind::Identifier) {
            ctx.previous().unwrap().lexeme().to_string()
        } else {
            "asd".to_string()
        };

        let owner = self.name(name, None);

        if self.consume(ctx, TokenKind::Less) {
            while !self.consume(ctx, TokenKind::Greater) {
                self.expect(
                    ctx,
                    TokenKind::Dolar,
                    "Expecting '$' infront of generic type arguments",
                );

                let identifier = ctx.current().lexeme().to_string();
                self.expect(
                    ctx,
                    TokenKind::Identifier,
                    "Expected a valid identifier for type parameter",
                );

                let type_param = self.data.add_symbol(identifier, None);
                let mut param = op!(self, ctx, ClassParam, Some([owner, type_param, 0]));
                if self.consume(ctx, TokenKind::Colon) {
                    param.set_type(self.get_type(ctx));
                }
                self.consume(ctx, TokenKind::Comma);

                class.insert(0, param);
            }
        }

        self.expect(
            ctx,
            TokenKind::LeftBracket,
            "Expected '{' denoting class body",
        );
        if ctx.owner().is_some() {
            panic!("Classes can only be declared outisde any conditional blocks");
        }
        ctx.set_owner(owner);
        while !self.consume(ctx, TokenKind::RightBracket) {
            let public = self.consume(ctx, TokenKind::Pub);
            if self.consume(ctx, TokenKind::Prop) {
                class.append(&mut self.prop(ctx, owner, public));
            } else if self.matches(ctx, TokenKind::Function) {
                class.append(&mut self.method(ctx, owner, public));
            }
        }
        ctx.clear_owner();

        class.insert(0, op!(self, ctx, Begin, None));
        class.insert(0, op!(self, ctx, Class, Some([owner, class.len(), 0])));
        class.push(op!(self, ctx, End, None));

        class
    }

    fn initialize(&mut self, ctx: &mut Context) -> Vec<IR> {
        ctx.advance();
        let mut result = vec![];
        let mut name = ctx.current().lexeme().to_string();
        if self.aliases.contains_key(&name) {
            name = self.aliases[&name].to_string();
        }
        let symbol = self.data.add_symbol(name, None);

        if self.expect(ctx, TokenKind::Identifier, "Expecting class name") {
            let mut ty = Type::object(symbol);
            let mut type_arity = 0;
            if self.consume(ctx, TokenKind::Less) {
                while !self.consume(ctx, TokenKind::Greater) {
                    // self.consume(ctx, TokenKind::Dolar);
                    // let arg = self
                    //     .data
                    //     .add_symbol(ctx.current().lexeme().to_string(), None);
                    //
                    // let param_ty = self.get_type(ctx);
                    // let param = self
                    //     .data
                    //     .add_type(Type::new(Kind::Generic(type_arity, param_ty)));

                    ty.add_argument(self.get_type(ctx));
                    self.consume(ctx, TokenKind::Comma);
                    type_arity += 1;
                }
            }

            let mut arity = 0;
            self.expect(ctx, TokenKind::LeftParenthesis, "Expecting '('");
            while !self.consume(ctx, TokenKind::RightParenthesis) {
                result.append(&mut self.expression(ctx));
                arity += 1;
                self.consume(ctx, TokenKind::Comma);
            }

            let mut instance = op!(self, ctx, Instantiate, Some([symbol, arity, 0]));

            instance.set_type(self.data.add_type(ty));

            result.push(instance);
        }

        result
    }

    fn this(&mut self, ctx: &mut Context) -> Vec<IR> {
        ctx.advance();
        let mut result = self.expression(ctx);
        let mut this = op!(self, ctx, This, None);
        if ctx.owner().is_none() {
            panic!("Using 'this' outside of object context");
        }
        this.set_type(self.data.add_type(Type::object(ctx.owner().unwrap())));
        result.insert(0, this);

        result
    }

    fn parse_imports(&mut self, ctx: &mut Context, ns: &Vec<String>) -> Vec<Vec<String>> {
        let mut prefix = vec![];
        let mut children = vec![];

        self.expect(ctx, TokenKind::Identifier, "Expected module identifier");
        if let Some(segment) = ctx.previous() {
            prefix.push(segment.lexeme().to_string());
        }

        if self.consume(ctx, TokenKind::ColonColon) {
            if self.consume(ctx, TokenKind::LeftBracket) {
                let mut next = ns.clone();
                next.append(&mut prefix.clone());

                loop {
                    self.parse_imports(ctx, &next).iter().for_each(|p| {
                        let mut path = prefix.clone();
                        path.append(&mut p.clone());
                        children.push(path);
                    });

                    if !self.matches(ctx, TokenKind::Comma) {
                        break;
                    }
                }

                self.expect(
                    ctx,
                    TokenKind::RightBracket,
                    "Expected '}' to close module group",
                );
            } else {
                let mut next = ns.clone();
                next.append(&mut prefix.clone());
                self.parse_imports(ctx, &next).iter().for_each(|p| {
                    let mut path = prefix.clone();
                    path.append(&mut p.clone());
                    children.push(path);
                });
            }
        }

        if self.consume(ctx, TokenKind::As) {
            if self.expect(
                ctx,
                TokenKind::Identifier,
                "Expected a valid identifier for import alias",
            ) {
                if let Some(prev) = ctx.previous() {
                    self.aliases
                        .entry(prev.lexeme().to_string())
                        .or_insert_with(|| {
                            format!("{}::{}", ns.join("::"), prefix.join("::"))
                                .trim_start_matches("::")
                                .to_string()
                        });
                }
            }
        } else if let Some(last) = prefix.last() {
            self.aliases.entry(last.to_string()).or_insert_with(|| {
                format!("{}::{}", ns.join("::"), prefix.join("::"))
                    .trim_start_matches("::")
                    .to_string()
            });
        }

        if children.is_empty() {
            return vec![prefix];
        }

        children.into_iter().filter(|s| !s.is_empty()).collect()
    }

    fn import(&mut self, ctx: &mut Context) -> Vec<IR> {
        let files = self.parse_imports(ctx, &vec![]);

        self.expect(
            ctx,
            TokenKind::SemiColon,
            "Expecting ';' at end of import statement",
        );

        let mut code = vec![];

        for mut module in files {
            let fqn = module.join("::");

            if self.scanned.contains(&fqn) {
                continue;
            }

            self.scanned.push(fqn.clone());
            module.pop();

            let mut joined = PathBuf::from("");
            module.iter().for_each(|part| {
                joined = joined.join(part);
            });

            let mut paths = vec![];
            for p in INCLUDE_PATHS {
                let mut file = PathBuf::from(p).join(&joined);
                file.set_extension("0s");

                if Path::new(&file).exists() {
                    paths.push(file);
                }
            }

            if paths.is_empty() {
                panic!(
                    "Unable to resolve '{}', because no suitable file has been found",
                    fqn
                );
            } else if paths.len() > 1 {
                panic!(
                    "Unable to resolve '{}', because of multiple possible locations:\n\t{}",
                    fqn,
                    paths
                        .iter()
                        .map(|path| path.to_str().unwrap_or(""))
                        .collect::<Vec<&str>>()
                        .join("\n\t")
                );
            }

            let ns = self.namespace.clone();
            let aliases = self.aliases.clone();
            self.aliases.clear();
            self.namespace = module.join("::");

            if let Some(path) = paths.first().map(|p| p.to_str()).unwrap() {
                if let Ok(program) = self.parse_internal(path.to_string()) {
                    self.typechecker.set_file(path.to_string());
                    code.append(&mut program.code().to_vec());
                    self.typechecker.set_file(self.file.to_string());
                }
            }
            self.namespace = ns;
            self.aliases = aliases;
        }

        code
    }

    fn statement(&mut self, ctx: &mut Context) -> Vec<IR> {
        match ctx.current().kind() {
            TokenKind::Use => {
                ctx.advance();

                self.import(ctx)
            }
            TokenKind::Let => {
                ctx.advance();
                let result = self.variable(ctx);
                self.consume(ctx, TokenKind::SemiColon);
                result
            }
            TokenKind::While => {
                ctx.advance();
                self.while_(ctx)
            }
            TokenKind::Const => {
                ctx.advance();
                self.constant(ctx)
            }
            TokenKind::Print => {
                ctx.advance();
                let mut tokens = self.expr(ctx);
                tokens.push(op!(self, ctx, Print, None));

                tokens
            }
            TokenKind::PrintLn => {
                ctx.advance();
                let mut tokens = self.expr(ctx);
                tokens.push(op!(self, ctx, Print, Some([1, 0, 0])));

                tokens
            }
            TokenKind::Return => {
                ctx.advance();
                let mut tokens = self.expr(ctx);
                tokens.push(op!(self, ctx, Leave, None));

                tokens
            }
            TokenKind::Function => {
                ctx.advance();
                self.function(ctx)
            }
            TokenKind::Interface => {
                ctx.advance();
                self.interface(ctx)
            }
            TokenKind::Class => {
                ctx.advance();
                self.class(ctx)
            }
            TokenKind::Implement => {
                ctx.advance();
                self.implement(ctx)
            }
            TokenKind::If => {
                ctx.advance();
                self.if_expression(ctx)
            }
            TokenKind::For => {
                ctx.advance();
                if self.matches(ctx, TokenKind::LeftParenthesis) {
                    self.boomer_loop(ctx)
                } else {
                    self.for_in(ctx)
                }
            }
            _ => self.expr_statement(ctx),
        }
    }

    fn parse_internal(&mut self, file: String) -> Result<Program<IR>, String> {
        let buffer = if let Ok(buff) = Buffer::new(file.as_ref()) {
            buff
        } else {
            return Err(format!("Unable to open file '{}'", &self.file));
        };

        let mut ctx = Context::new(Scanner::new(buffer, Some(self.file.clone())));
        let mut code = vec![];
        self.typechecker.set_file(self.file.clone());

        while ctx.current().kind() != TokenKind::EOF {
            let stmt = self.statement(&mut ctx);

            code.append(&mut self.typechecker.check(&stmt, self.data));
        }
        if !self.typechecker.get_errors().is_empty() {
            for err in self.typechecker.get_errors() {
                eprintln!("{}", err);
            }

            return Err("Encountered errors during parsing".to_string());
        }

        Ok(Program::new(code))
    }

    pub fn parse(&mut self) -> Result<Program<IR>, String> {
        self.parse_internal(self.file.clone())
    }
}

#[cfg(test)]
mod tests {
    use common::opcodes::Operation;
    use common::program::data::Data;
    use scanner::{Scanner, buffer::Buffer};

    use crate::Parser;

    macro_rules! assert_parsed_expression {
        ($code:expr, $($token:expr),+) => {
            if let Ok(buffer) = Buffer::try_from($code) {
                let mut scanner = Scanner::new(buffer, None);
                let mut data = Data::default();

                if let Ok(program) = Parser::new(&mut data).parse(&mut scanner) {
                    let tokens = [$($token),+];

                    let diff = (tokens.len()) + (program.code().len() - tokens.len());
                    for i in diff..program.code().len() {
                        eprintln!("'{}'\t- Assertion is missing '{:?}' in expectation", $code, program.get(i));
                    }

                    for (idx, token) in tokens.iter().enumerate() {
                        assert_eq!(
                            program.get(idx).map(|t| t.code()),
                            Some(*token),
                            "Token #{}, '{:?}' does not match token '{:?}'",
                            idx + 1,
                            program.get(idx).map(|t| t.code()).unwrap_or_default(),
                            (*token),
                        )
                    }

                    assert_eq!(tokens.len(), program.len());
                } else {
                    assert!(false, "Unable to parse {}", $code);
                }
            } else {
                assert!(false, "Unable to build buffer for '{}'", $code);
            }
        };
    }

    #[test]
    fn test_literals() {
        assert_parsed_expression!("42;", Operation::Const, Operation::Pop);
        assert_parsed_expression!("1.2;", Operation::Const, Operation::Pop);
        assert_parsed_expression!("'Hello, World';", Operation::Const, Operation::Pop);
        assert_parsed_expression!("\"Hello, World\";", Operation::Const, Operation::Pop);
    }

    #[test]
    fn test_simple_expressions() {
        assert_parsed_expression!(
            "0.1 + 0.2;",
            Operation::Const,
            Operation::Const,
            Operation::Add,
            Operation::Pop
        );
        assert_parsed_expression!(
            "42 + 69;",
            Operation::Const,
            Operation::Const,
            Operation::Add,
            Operation::Pop
        );
        assert_parsed_expression!(
            "42.1 + 69.3;",
            Operation::Const,
            Operation::Const,
            Operation::Add,
            Operation::Pop
        );
        assert_parsed_expression!(
            "42 - 69;",
            Operation::Const,
            Operation::Const,
            Operation::Subtract,
            Operation::Pop
        );
        assert_parsed_expression!(
            "42 * 69;",
            Operation::Const,
            Operation::Const,
            Operation::Multiply,
            Operation::Pop
        );
        assert_parsed_expression!(
            "42 / 69;",
            Operation::Const,
            Operation::Const,
            Operation::Divide,
            Operation::Pop
        );
        assert_parsed_expression!(
            "42 % 69;",
            Operation::Const,
            Operation::Const,
            Operation::Modulo,
            Operation::Pop
        );
        assert_parsed_expression!(
            "42 ** 69;",
            Operation::Const,
            Operation::Const,
            Operation::Pow,
            Operation::Pop
        );
        assert_parsed_expression!(
            "42 << 69;",
            Operation::Const,
            Operation::Const,
            Operation::LeftShift,
            Operation::Pop
        );
        assert_parsed_expression!(
            "42 >> 69;",
            Operation::Const,
            Operation::Const,
            Operation::RightShift,
            Operation::Pop
        );
        assert_parsed_expression!(
            "42 ^ 69;",
            Operation::Const,
            Operation::Const,
            Operation::BitXor,
            Operation::Pop
        );
        assert_parsed_expression!(
            "42 | 69;",
            Operation::Const,
            Operation::Const,
            Operation::BitOr,
            Operation::Pop
        );
        assert_parsed_expression!(
            "42 & 69;",
            Operation::Const,
            Operation::Const,
            Operation::BitAnd,
            Operation::Pop
        );
        assert_parsed_expression!(
            "42 == 69;",
            Operation::Const,
            Operation::Const,
            Operation::Equal,
            Operation::Pop
        );
        assert_parsed_expression!(
            "42 != 69;",
            Operation::Const,
            Operation::Const,
            Operation::NotEqual,
            Operation::Pop
        );
        assert_parsed_expression!(
            "42 < 69;",
            Operation::Const,
            Operation::Const,
            Operation::Less,
            Operation::Pop
        );
        assert_parsed_expression!(
            "42 <= 69;",
            Operation::Const,
            Operation::Const,
            Operation::LessEqual,
            Operation::Pop
        );
        assert_parsed_expression!(
            "42 > 69;",
            Operation::Const,
            Operation::Const,
            Operation::Greater,
            Operation::Pop
        );
        assert_parsed_expression!(
            "42 >= 69;",
            Operation::Const,
            Operation::Const,
            Operation::GreaterEqual,
            Operation::Pop
        );
        assert_parsed_expression!(
            "true or true;",
            Operation::Const,
            Operation::Const,
            Operation::Or,
            Operation::Pop
        );
        assert_parsed_expression!(
            "true and true;",
            Operation::Const,
            Operation::Const,
            Operation::And,
            Operation::Pop
        );
    }

    #[test]
    fn test_precedence() {
        assert_parsed_expression!(
            "3 * 4 + 2",
            Operation::Const,
            Operation::Const,
            Operation::Multiply,
            Operation::Const,
            Operation::Add,
            Operation::Pop
        );

        assert_parsed_expression!(
            "3 * (4 + 2)",
            Operation::Const,
            Operation::Const,
            Operation::Const,
            Operation::Add,
            Operation::Multiply,
            Operation::Pop
        );

        assert_parsed_expression!(
            "3 * (4 + 2) + 5;",
            Operation::Const,
            Operation::Const,
            Operation::Const,
            Operation::Add,
            Operation::Multiply,
            Operation::Const,
            Operation::Add,
            Operation::Pop
        );

        assert_parsed_expression!(
            "3 * ((4 + 2) + 5);",
            Operation::Const,
            Operation::Const,
            Operation::Const,
            Operation::Add,
            Operation::Const,
            Operation::Add,
            Operation::Multiply,
            Operation::Pop
        );
    }

    #[test]
    fn test_template_expression() {
        assert_parsed_expression!(
            "`Hello, ${name}`;",
            Operation::Const,
            Operation::Load,
            Operation::Add,
            Operation::Const,
            Operation::Add,
            Operation::Pop
        );
    }

    // #[test]
    // fn test_match_expression() {
    //     assert_parsed_expression!(
    //         "match true { true => 'sadge' };",
    //         Operation::Const, // Left
    //         Operation::Const, // Right
    //         Operation::Equal, // Equals
    //         Operation::Match, // conditional jump
    //         Operation::Const, // the match arm body
    //         Operation::Jump   // jump out of the match expr
    //     );
    // }
}
