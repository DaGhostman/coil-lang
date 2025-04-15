use common::opcodes::Metadata;
use common::program::data::Data;
use rand::{Rng, distr::Alphanumeric};
use scanner::buffer::Buffer;
use std::str::FromStr;

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
        }
    }

    // fn op(&mut self, ctx: &mut Context, op: Operation, operands: Option<[usize; 3]>) -> IR {
    //     let mut ir = IR::new(op, operands);
    //     dbg!(ctx.previous());
    //
    //     ir
    // }

    fn get_type(&mut self, ctx: &mut Context) -> Kind {
        match ctx.current().kind() {
            TokenKind::Int => Kind::Integer,
            TokenKind::Float => Kind::Float,
            TokenKind::Str => Kind::String,
            TokenKind::Identifier => {
                ctx.advance();
                Kind::Object(
                    self.data
                        .add_symbol(ctx.current().lexeme().to_string(), None),
                )
            }
            TokenKind::Void => Kind::None,
            _ => {
                eprintln!("Unknown token to be used as value: {:?}", ctx.current());

                Kind::None
            }
        }
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
        let constant = self.data.add_constant(value, Type::new(Kind::Bool));

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

        let constant = self.data.add_constant(value, Type::new(Kind::Integer));

        vec![op!(self, ctx, Const, Some([constant, 0, 0]))]
    }

    fn float(&mut self, ctx: &mut Context) -> Vec<IR> {
        let value = if let Ok(value) = f64::from_str(ctx.current().lexeme()) {
            Value::FLOAT(value)
        } else {
            todo!("Fail to parse number as float");
        };

        ctx.advance();
        let constant = self.data.add_constant(value, Type::new(Kind::Float));

        vec![op!(self, ctx, Const, Some([constant, 0, 0]))]
    }

    fn string(&mut self, ctx: &mut Context) -> Vec<IR> {
        let string = self.data.add_string(ctx.current().lexeme().to_string());
        let constant = self
            .data
            .add_constant(Value::STR(string), Type::new(Kind::String));

        ctx.advance();
        vec![op!(self, ctx, Const, Some([constant, 0, 0]))]
    }

    fn identifier(&mut self, ctx: &mut Context) -> Vec<IR> {
        let symbol = self
            .data
            .add_symbol(ctx.current().lexeme().to_string(), None);
        ctx.advance();

        let mut tokens = vec![];
        if self.consume(ctx, TokenKind::LeftParenthesis) {
            let mut arity = 0;
            while !self.consume(ctx, TokenKind::RightParenthesis) {
                tokens.append(&mut self.expression(ctx));
                self.consume(ctx, TokenKind::Comma);
                arity += 1;
            }

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
            IR::new(
                Operation::Check,
                Some([full_predicate.len(), tokens.len(), 0]),
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
            // if let Some(jump) = has_prev {
            //     dbg!(tokens.get(jump));
            //     self.patch(&mut tokens, jump);
            //     dbg!(tokens.get(jump));
            //     has_prev = None;
            // }
            // has_prev = Some(else_jump);
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
            IR::new(
                Operation::Condition,
                Some([condition_len, body_len, alternative.len()]),
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
                result.push(IR::new(
                    Operation::Iterate,
                    Some([
                        self.data.add_symbol(name.lexeme().to_string(), None),
                        body.len(),
                        0,
                    ]),
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
        result.push(IR::new(
            Operation::Loop,
            Some([condition.len(), body.len(), 0]),
        ));
        result.append(&mut condition);
        // result.push(op!(self, ctx,Rewind, Some([body.len(), 0, 0])));
        result.append(&mut body);

        result
    }

    fn call(&mut self, ctx: &mut Context) -> Vec<IR> {
        self.expect(ctx, TokenKind::Dot, "Expected '.' for call expression");
        let mut tokens = vec![];
        let name = ctx.current().clone();
        let mut arity = 0;

        if self.expect(ctx, TokenKind::Identifier, "Expected function name") {
            if self.consume(ctx, TokenKind::LeftParenthesis) {
                while !self.consume(ctx, TokenKind::RightParenthesis) {
                    arity += 1;
                    tokens.append(&mut self.expression(ctx));

                    self.consume(ctx, TokenKind::Comma);
                }

                // let metadata = Metadata {
                //     line:
                // };

                tokens.push(IR::new(
                    Operation::Invoke,
                    Some([
                        self.data.add_symbol(name.lexeme().to_string(), None),
                        arity,
                        0,
                    ]),
                ));
            } else if self.consume(ctx, TokenKind::Equal) {
                tokens.append(&mut self.expression(ctx));
                tokens.push(IR::new(
                    Operation::PropAssign,
                    Some([self.data.add_symbol(name.lexeme().to_string(), None), 0, 0]),
                ));
            } else {
                tokens.push(IR::new(
                    Operation::PropLoad,
                    Some([self.data.add_symbol(name.lexeme().to_string(), None), 0, 0]),
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
        let mut kind = Kind::None;
        if self.consume(ctx, TokenKind::Colon) {
            kind = self.get_type(ctx);
            ctx.advance();
        }

        let mut tokens = vec![];
        if self.consume(ctx, TokenKind::Equal) {
            tokens.append(&mut self.expression(ctx));
        } else {
            tokens.push(IR::new(
                Operation::Const,
                Some([
                    self.data.add_constant(Value::NONE, Type::new(Kind::None)),
                    0,
                    0,
                ]),
            ));
        }

        let mut declaration = IR::new(
            Operation::Declare,
            Some([self.data.add_symbol(name.lexeme().to_string(), None), 0, 0]),
        );

        declaration.set_type(Type::new(kind));
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
        let mut kind = Kind::None;
        if self.consume(ctx, TokenKind::Colon) {
            kind = self.get_type(ctx);
            ctx.advance();
        }

        let mut tokens = self.expr(ctx);
        tokens.pop();

        let mut declaration = IR::new(
            Operation::Declare,
            Some([self.data.add_symbol(name.lexeme().to_string(), None), 1, 0]),
        );

        declaration.set_type(Type::new(kind));
        tokens.push(declaration);

        tokens
    }

    fn function(&mut self, ctx: &mut Context) -> Vec<IR> {
        let mut tokens = vec![];

        self.consume(ctx, TokenKind::Function);

        let name: String = if self.consume(ctx, TokenKind::Identifier) {
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
            ctx.advance();
            let argument = ctx.current().clone();

            if self.expect(
                ctx,
                TokenKind::Identifier,
                "Expected function argument identifier",
            ) {
                let mut arg = IR::new(
                    Operation::Argument,
                    Some([
                        self.data.add_symbol(argument.lexeme().to_string(), None),
                        arity,
                        0,
                    ]),
                );

                arg.set_type(Type::new(kind));
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
                upvalues.push(IR::new(
                    Operation::Upvalue,
                    Some([self.data.add_symbol(name.lexeme().to_string(), None), 0, 0]),
                ));
                self.consume(ctx, TokenKind::Comma);
            }

            body.append(&mut upvalues);
        }
        let mut kind = Kind::None;
        if self.consume(ctx, TokenKind::SlimArrow) {
            kind = self.get_type(ctx);
            ctx.advance();
        }
        body.append(&mut self.block(ctx));

        let symbol = self.data.add_symbol(name, None);
        let mut func = op!(self, ctx, Function, Some([symbol, arity, body.len()]));
        let mut func_type = Type::new(Kind::Function);
        func_type.set_return(kind);
        for arg in argument_types {
            func_type.add(arg);
        }

        func.set_type(func_type);

        tokens.push(func);
        body.push(IR::new(
            Operation::Const,
            Some([
                self.data.add_constant(Value::NONE, Type::new(Kind::None)),
                0,
                0,
            ]),
        ));
        body.push(op!(self, ctx, Leave, None));
        tokens.append(&mut body);

        tokens
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

        self.expect(
            ctx,
            TokenKind::LeftBracket,
            "Expected '{' denoting class body",
        );
        let mut class = vec![];
        while !self.consume(ctx, TokenKind::RightBracket) {
            let public = self.consume(ctx, TokenKind::Pub);
            if self.consume(ctx, TokenKind::Prop) {
                let prop_name = ctx.current.lexeme().to_string();
                self.consume(ctx, TokenKind::Identifier);

                let mut prop = if self.consume(ctx, TokenKind::SemiColon) {
                    vec![]
                } else {
                    self.expr_statement(ctx)
                };
                prop.push(IR::new(
                    Operation::Prop,
                    Some([
                        owner,
                        self.data.add_symbol(prop_name, None),
                        usize::from(public),
                    ]),
                ));

                prop.append(&mut class);
                class = prop;
            } else if self.matches(ctx, TokenKind::Function) {
                let mut method = self.function(ctx);
                if let Some(code) = method.first_mut() {
                    let operands = code.operands();
                    let mut method =
                        op!(self, ctx, Method, Some([owner, operands[0], operands[2]]));
                    method.set_type(code.kind());
                    *code = method;
                }
                class.append(&mut method);
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
        let owner = self.data.add_symbol(name, None);
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
                    body.push(IR::new(
                        Operation::Argument,
                        Some([
                            self.data.add_symbol(argument.lexeme().to_string(), None),
                            arity,
                            0,
                        ]),
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

            method.insert(
                0,
                IR::new(
                    Operation::Method,
                    Some([owner, self.data.add_symbol(method_name, None), body.len()]),
                ),
            );
            method.append(&mut body);

            interface.append(&mut method);
        }

        interface.insert(0, op!(self, ctx, Begin, None));
        interface.push(op!(self, ctx, End, None));
        interface.insert(
            0,
            IR::new(
                Operation::Interface,
                Some([owner, interface.len(), interface.len()]),
            ),
        );

        interface
    }

    fn class(&mut self, ctx: &mut Context) -> Vec<IR> {
        let name = if self.consume(ctx, TokenKind::Identifier) {
            ctx.previous().unwrap().lexeme().to_string()
        } else {
            "asd".to_string()
        };
        let owner = self.data.add_symbol(name, None);

        self.expect(
            ctx,
            TokenKind::LeftBracket,
            "Expected '{' denoting class body",
        );
        let mut class = vec![];
        if ctx.owner().is_some() {
            panic!("Classes can only be declared outisde any conditional blocks");
        }
        ctx.set_owner(owner);
        while !self.consume(ctx, TokenKind::RightBracket) {
            let public = self.consume(ctx, TokenKind::Pub);
            if self.consume(ctx, TokenKind::Prop) {
                let prop_name = ctx.current.lexeme().to_string();
                self.consume(ctx, TokenKind::Identifier);

                let mut prop = if self.consume(ctx, TokenKind::SemiColon) {
                    vec![]
                } else {
                    self.expr_statement(ctx)
                };
                prop.push(IR::new(
                    Operation::Prop,
                    Some([
                        owner,
                        self.data.add_symbol(prop_name, None),
                        usize::from(public),
                    ]),
                ));

                prop.append(&mut class);
                class = prop;
            } else if self.matches(ctx, TokenKind::Function) {
                let mut method = self.function(ctx);
                if let Some(code) = method.first_mut() {
                    let operands = code.operands();
                    let mut method =
                        op!(self, ctx, Method, Some([owner, operands[0], operands[2]]));
                    method.set_type(code.kind());
                    *code = method;
                }
                class.append(&mut method);
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
        let name = ctx.current().lexeme().to_string();
        if self.expect(ctx, TokenKind::Identifier, "Expecting class name") {
            let mut arity = 0;
            self.expect(ctx, TokenKind::LeftParenthesis, "Expecting '('");
            while !self.consume(ctx, TokenKind::RightParenthesis) {
                result.append(&mut self.expression(ctx));
                arity += 1;
                self.consume(ctx, TokenKind::Comma);
            }

            let symbol = self.data.add_symbol(name, None);
            let mut instance = op!(self, ctx, Instantiate, Some([symbol, arity, 0]));
            instance.set_type(Type::new(Kind::Object(symbol)));

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
        this.set_type(Type::new(Kind::Object(ctx.owner().unwrap())));
        result.insert(0, this);

        result
    }

    fn statement(&mut self, ctx: &mut Context) -> Vec<IR> {
        match ctx.current().kind() {
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

    pub fn parse(&mut self) -> Result<Program<IR>, String> {
        let buffer = if let Ok(buff) = Buffer::new(self.file.as_ref()) {
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
