use common::error::{Message, MessageComposer, MessageCreator, MessageKind, MessageOrigin};
use common::opcodes::Metadata;
use common::program::data::Data;
use rand::{Rng, distr::Alphanumeric};
use scanner::buffer::Buffer;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use rustc_hash::{FxHashMap as HashMap, FxHashSet};

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

    messages: Vec<Message>,
}

const INCLUDE_PATHS: [&str; 5] = ["src/", "lib/", "external/", "vendor/", "deps/"];

impl Context {
    #[must_use]
    pub fn new(mut scanner: Scanner) -> Self {
        let current = scanner.scan();

        Self {
            scanner,
            current,
            previous: None,
            owner: None,
            messages: vec![],
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

    #[must_use]
    pub fn has_owner(&self) -> bool {
        self.owner.is_some()
    }

    #[must_use]
    pub fn owner(&self) -> Option<usize> {
        self.owner.or(Some(0))
    }

    pub fn error(&mut self, message: &str) {
        self.messages
            .push(Message::error(MessageOrigin::PARSE, message.to_string()));
    }

    pub fn warn(&mut self, message: &str) {
        self.messages
            .push(Message::warning(MessageOrigin::PARSE, message.to_string()));
    }
}

pub struct Parser<'data> {
    data: &'data mut Data,
    file: String,
    natives: HashMap<usize, Type>,
    typechecker: TypeChecker<64>,

    scanned: Vec<String>,
    aliases: HashMap<String, String>,
    namespace: String,

    messages: FxHashSet<Message>,
}

macro_rules! op {
    ($this:expr, $ctx:expr, $op:ident, $value:expr) => {{
        let mut ir = IR::new(Operation::$op, $value);
        if let Some(token) = $ctx.previous() {
            let data = Metadata::new(token.start_line(), token.start_column());
            ir.with_metadata(data);
        }

        ir
    }};
    ($this:expr, $ctx:expr, $op:ident) => {{
        let mut ir = IR::new(Operation::$op, Default::default());
        if let Some(token) = $ctx.previous() {
            let data = Metadata::new(token.start_line(), token.start_column());
            ir.with_metadata(data);
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
            natives: HashMap::default(),
            namespace: String::new(),

            messages: FxHashSet::default(),
        }
    }

    pub fn register(&mut self, native: usize, type_: Type) {
        let name = self.data.symbol_name(native);

        self.scanned.push(name.to_owned());
        self.aliases.insert(name.to_owned(), name.to_owned());

        self.natives.insert(native, type_);
        self.typechecker.register(native, type_);
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
        let mut r#type = Type::new(if self.consume(ctx, TokenKind::Percent) {
            Kind::Wildcard
        } else if self.consume(ctx, TokenKind::Bool) {
            Kind::Bool
        } else if self.consume(ctx, TokenKind::Int) {
            Kind::Integer
        } else if self.consume(ctx, TokenKind::Float) {
            Kind::Float
        } else if self.consume(ctx, TokenKind::Str) {
            Kind::String
        } else if self.consume(ctx, TokenKind::Void) {
            Kind::None
        } else if self.consume(ctx, TokenKind::Dolar) {
            let symbol = self
                .data
                .add_symbol(ctx.current().lexeme().to_string(), None);
            self.expect(ctx, TokenKind::Identifier, "Expected generic identifier");
            let mut kind = self.data.add_type(Type::any());

            if self.consume(ctx, TokenKind::Equal) {
                kind = self.get_type(ctx);
            }

            Kind::Generic(symbol, kind)
        } else if self.consume(ctx, TokenKind::Coroutine) {
            self.expect(ctx, TokenKind::Less, "Expecting '<' for coroutine subtype");
            let t = self.get_type(ctx);
            self.expect(
                ctx,
                TokenKind::Greater,
                "Expecting '>' for coroutine subtype",
            );

            Kind::Coroutine(t)
        } else if self.matches(ctx, TokenKind::Identifier) {
            let mut name = ctx.current().lexeme().to_string();
            ctx.advance();
            if self.aliases.contains_key(&name) {
                name = self.aliases[&name].to_string();
            }

            let n = self.data.add_symbol(name, None);
            Kind::Object(n)
        } else {
            ctx.error(&format!(
                "Unknown token to be used as type: {:?}",
                ctx.current().lexeme()
            ));

            Kind::default()
        });

        if self.consume(ctx, TokenKind::Less) {
            r#type.add_argument(self.get_type(ctx));
            while self.consume(ctx, TokenKind::Comma) {
                r#type.add_argument(self.get_type(ctx));
            }

            self.expect(
                ctx,
                TokenKind::Greater,
                "Expected '>' to close type parameter list.",
            );
        } else if self.consume(ctx, TokenKind::Ampersand) {
            let mut t = Type::new(Kind::Intersection);
            t.add(self.data.add_type(r#type));
            t.add(self.get_type(ctx));
            while self.consume(ctx, TokenKind::Pipe) || self.matches(ctx, TokenKind::And) {
                t.add(self.get_type(ctx));
            }

            r#type = t;
        } else if self.consume(ctx, TokenKind::Pipe) {
            let mut t = Type::new(Kind::Union);
            t.add(self.data.add_type(r#type));
            t.add(self.get_type(ctx));
            while self.consume(ctx, TokenKind::Pipe) || self.matches(ctx, TokenKind::And) {
                t.add(self.get_type(ctx));
            }

            r#type = t;
        }

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
            ctx.error(&format!(
                "ERROR: {}. Current: {:?}('{}') in {}:{}:{}",
                message,
                ctx.current.kind(),
                ctx.current.lexeme(),
                ctx.current.file(),
                ctx.current.start_line(),
                ctx.current.start_column(),
            ));
        }

        matched
    }

    fn grouping(&mut self, ctx: &mut Context) -> Vec<IR> {
        ctx.advance();

        let mut tokens = vec![];

        while !self.consume(ctx, TokenKind::RightParenthesis) {
            tokens.append(&mut self.expression(ctx));
        }

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
        vec![op!(self, ctx, Const, [constant, 0, 0])]
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

        vec![op!(self, ctx, Const, [constant, 0, 0])]
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

        vec![op!(self, ctx, Const, [constant, 0, 0])]
    }

    fn string(&mut self, ctx: &mut Context) -> Vec<IR> {
        let string = self.data.add_string(ctx.current().lexeme().to_string());
        let constant = self
            .data
            .add_constant(Value::STR(string), self.data.find_type(Type::string()));

        ctx.advance();
        vec![op!(self, ctx, Const, [constant, 0, 0])]
    }

    fn identifier(&mut self, ctx: &mut Context) -> Vec<IR> {
        let mut name = ctx.current().lexeme().to_string();

        let mut symbol = self.data.add_symbol(name.to_string(), None);
        ctx.advance();

        let mut type_ = Type::function();
        if self.consume(ctx, TokenKind::Less) {
            while !self.consume(ctx, TokenKind::Greater) {
                let t = self.get_type(ctx);

                type_.add_argument(t);
            }
        }

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

            if let Some(ty) = self.natives.get(&symbol) {
                type_ = *ty;
            }

            symbol = self.data.add_symbol(name, None);
            let mut call = op!(self, ctx, Call, [symbol, arity, 0]);
            call.set_type(self.data.add_type(type_));

            tokens.push(call);
        } else if self.consume(ctx, TokenKind::Equal) {
            tokens.append(&mut self.expression(ctx));
            tokens.push(op!(self, ctx, Assign, [symbol, 0, 0]));
        } else if self.consume(ctx, TokenKind::PlusPlus) {
            tokens.push(op!(self, ctx, Inc, [symbol, 0, 0]));
        } else if self.consume(ctx, TokenKind::MinusMinus) {
            tokens.push(op!(self, ctx, Dec, [symbol, 0, 0]));
        } else {
            tokens.push(op!(self, ctx, Load, [symbol, 0, 0]));
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

        tokens.push(op!(self, ctx, Array, [arity, 0, 0]));

        tokens
    }

    fn comparison(&mut self, ctx: &mut Context) -> Vec<IR> {
        let operator = ctx.current().clone();
        ctx.advance();
        let mut tokens = self.precedence(ctx, Precedence::get(operator.kind()).next());

        match operator.kind() {
            TokenKind::Less => tokens.push(op!(self, ctx, Less)),
            TokenKind::LessEqual => tokens.push(op!(self, ctx, LessEqual)),
            TokenKind::Greater => tokens.push(op!(self, ctx, Greater)),
            TokenKind::GreaterEqual => tokens.push(op!(self, ctx, GreaterEqual)),
            _ => unreachable!("No other comparison"),
        }

        tokens
    }

    fn equality(&mut self, ctx: &mut Context) -> Vec<IR> {
        let operator = ctx.current().clone();
        ctx.advance();
        let mut tokens = self.precedence(ctx, Precedence::get(operator.kind()).next());

        match operator.kind() {
            TokenKind::EqualEqual => tokens.push(op!(self, ctx, Equal)),
            TokenKind::BangEqual => tokens.push(op!(self, ctx, NotEqual)),
            _ => unreachable!("No other equality"),
        }

        tokens
    }

    fn unary(&mut self, ctx: &mut Context) -> Vec<IR> {
        let operator = ctx.current().clone();
        ctx.advance();

        let mut tokens = self.precedence(ctx, Precedence::Assign);

        match operator.kind() {
            TokenKind::Minus => tokens.push(op!(this, ctx, Negate)),
            TokenKind::Bang => tokens.push(op!(this, ctx, Not)),
            TokenKind::Len => tokens.push(op!(this, ctx, Length)),
            _ => unreachable!("Unexpected prefix operator"),
        }

        tokens
    }

    fn binary(&mut self, ctx: &mut Context, _assignment: bool) -> Vec<IR> {
        let operator = ctx.current().clone();
        ctx.advance();
        let mut tokens = self.precedence(ctx, Precedence::get(operator.kind()).next());

        match operator.kind() {
            TokenKind::Plus => tokens.push(op!(self, ctx, Add)),
            TokenKind::PlusPlus => tokens.push(op!(self, ctx, Inc)),
            TokenKind::Minus => tokens.push(op!(self, ctx, Subtract)),
            TokenKind::MinusMinus => tokens.push(op!(self, ctx, Dec)),
            TokenKind::Star => tokens.push(op!(self, ctx, Multiply)),
            TokenKind::StarStar => tokens.push(op!(self, ctx, Pow)),
            TokenKind::Slash => tokens.push(op!(self, ctx, Divide)),
            TokenKind::Percent => tokens.push(op!(self, ctx, Modulo)),
            TokenKind::Less => tokens.push(op!(self, ctx, Less)),
            TokenKind::LessLess => tokens.push(op!(self, ctx, LeftShift)),
            TokenKind::Greater => tokens.push(op!(self, ctx, Greater)),
            TokenKind::GreaterGreater => tokens.push(op!(self, ctx, RightShift)),
            TokenKind::Caret => tokens.push(op!(self, ctx, BitXor)),
            TokenKind::Pipe => tokens.push(op!(self, ctx, BitOr)),
            TokenKind::Ampersand => tokens.push(op!(self, ctx, BitAnd)),
            TokenKind::Or => tokens.push(op!(self, ctx, Or)),
            TokenKind::And => tokens.push(op!(self, ctx, And)),
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
            TokenKind::DotDot | TokenKind::DotDotEqual => self.range(ctx),
            TokenKind::LeftParenthesis => self.grouping(ctx),
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
                t.push(op!(self, ctx, Leave));

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
            op!(self, ctx, Check, [full_predicate.len(), tokens.len(), 0]),
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
                    tokens.push(op!(self, ctx, Unwrap));
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
                    tokens.push(op!(self, ctx, UnwrapError));
                    tokens.append(&mut self.parse_pattern(ctx, predicate));
                    self.expect(
                        ctx,
                        TokenKind::RightParenthesis,
                        "Expected ')' for sub-pattern",
                    );
                } else {
                    todo!("Handle is_some() and is_err() kind of checks");
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
                tokens.append(&mut self.expression(ctx));
                tokens.push(op!(self, ctx, Store, [symbol, 0, 0]));
            }
            _ => {
                tokens.append(&mut self.expression(ctx));
            }
        }

        if self.matches(ctx, TokenKind::Pipe) {
            ctx.advance();
            tokens.append(&mut self.parse_pattern(ctx, predicate));
            tokens.push(op!(self, ctx, Or));
        }

        tokens.append(&mut predicate.clone());
        tokens.push(op!(self, ctx, Equal));

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
            tokens.append(&mut self.parse_pattern_branch(ctx, &expr));

            self.consume(ctx, TokenKind::Comma);
        }

        tokens.insert(0, op!(self, ctx, Match, [tokens.len(), 0, 0]));

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
                [condition_len, body_len, alternative.len()]
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

        result.insert(0, op!(self, ctx, Loop, [condition_len, body_len, 0]));

        result
    }

    fn for_in(&mut self, ctx: &mut Context) -> Vec<IR> {
        let begin = op!(self, ctx, Begin, [ctx.owner().unwrap(), 0, 0]);
        let end = op!(self, ctx, End);
        let mut result = vec![begin];
        let name = ctx.current().to_owned();
        if self.expect(
            ctx,
            TokenKind::Identifier,
            "Expecting variable name to hold iteration",
        ) && self.expect(ctx, TokenKind::In, "Expecting 'in' for loop")
        {
            result.append(&mut self.expression(ctx));
            let mut body = self.block(ctx);
            result.push(op!(
                self,
                ctx,
                Iterate,
                [
                    self.data.add_symbol(name.lexeme().to_string(), None),
                    body.len(),
                    0,
                ]
            ));
            result.append(&mut body);
        }

        result.push(end);

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

        action.push(op!(self, ctx, Pop, [1, 0, 0]));
        body.append(&mut action);

        result.append(&mut initializer);
        result.push(op!(self, ctx, Loop, [condition.len(), body.len(), 0]));
        result.append(&mut condition);
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
                    [
                        self.data.add_symbol(name, None),
                        arity,
                        ctx.owner().unwrap()
                    ]
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
                    [self.data.add_symbol(name, None), 1, 0]
                ));
            } else {
                tokens.push(op!(
                    self,
                    ctx,
                    Prop,
                    [self.data.add_symbol(name, None), 0, 0]
                ));
            } // TODO: Implement Increment for properties
        }

        tokens
    }

    fn range(&mut self, ctx: &mut Context) -> Vec<IR> {
        let op = ctx.current().kind();
        ctx.advance();

        let mut tokens = self.expression(ctx);
        tokens.push(op!(
            self,
            ctx,
            Range,
            [
                match op {
                    TokenKind::DotDotEqual => 1,
                    _ => 0,
                },
                0,
                0
            ]
        ));

        tokens
    }

    fn prefix(&mut self, ctx: &mut Context, _assignment: bool) -> Vec<IR> {
        match ctx.current().kind() {
            TokenKind::PlusPlus | TokenKind::MinusMinus => {
                let op = ctx.current().clone();
                ctx.advance();

                let name = ctx.current.clone();
                self.expect(ctx, TokenKind::Identifier, "Expected identifier");
                let mut result = vec![];
                match op.kind() {
                    TokenKind::MinusMinus => {
                        result.push(op!(
                            self,
                            ctx,
                            Dec,
                            [self.data.add_symbol(name.lexeme().to_string(), None), 1, 0]
                        ));
                    }
                    TokenKind::PlusPlus => {
                        result.push(op!(
                            self,
                            ctx,
                            Inc,
                            [self.data.add_symbol(name.lexeme().to_string(), None), 1, 0]
                        ));
                    }
                    _ => (),
                }

                result
            }
            TokenKind::Bang | TokenKind::Minus | TokenKind::Len => self.unary(ctx),

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
            TokenKind::Function => self.function_expr(ctx),
            TokenKind::New => self.initialize(ctx),
            TokenKind::LeftBracket => self.block(ctx),
            TokenKind::TypeOf => self.typeof_(ctx),
            TokenKind::Yield => self.yield_(ctx),
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
        self.precedence(ctx, Precedence::Assign)
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

        tokens.push(op!(self, ctx, Pop));

        tokens
    }

    fn block(&mut self, ctx: &mut Context) -> Vec<IR> {
        let mut tokens = vec![op!(self, ctx, Begin, [ctx.owner().unwrap(), 0, 0])];

        if self.expect(
            ctx,
            TokenKind::LeftBracket,
            "Expecting '{' at the start of a block",
        ) {
            while !self.consume(ctx, TokenKind::RightBracket) {
                tokens.append(&mut self.statement(ctx));
            }
        }

        tokens.push(op!(self, ctx, End));

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
        let mut kind = self.data.add_type(Type::any());
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
                [self.data.add_constant(Value::NONE, 0), 0, 0,]
            ));
        }

        let mut declaration = op!(
            self,
            ctx,
            Declare,
            [self.data.add_symbol(name.lexeme().to_string(), None), 0, 0]
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
        let mut kind = self.data.add_type(Type::any());
        if self.consume(ctx, TokenKind::Colon) {
            kind = self.get_type(ctx);
        }

        let mut tokens = vec![];
        let params: [usize; 3] = [self.data.add_symbol(name.lexeme().to_string(), None), 1, 0];

        if self.consume(ctx, TokenKind::Equal) {
            tokens = self.expr(ctx);
            if tokens.len() == 1
                && matches!(
                    tokens.last().map(common::opcodes::IR::code),
                    Some(Operation::Const)
                )
            {
                if let Some(op) = tokens.last() {
                    let [constant, ..] = op.operands();

                    self.data
                        .add_symbol(name.lexeme().to_string(), Some(*constant));
                }
            }
        }

        let mut declaration = op!(self, ctx, Declare, params);

        declaration.set_type(kind);
        tokens.push(declaration);

        tokens
    }

    fn function_expr(&mut self, ctx: &mut Context) -> Vec<IR> {
        let mut result = self.function(ctx);

        if let Some(func) = result.first().copied() {
            let mut expr = IR::new(Operation::Closure, [func.get(0), result.len(), 0]);
            expr.set_type(func.kind());
            result.insert(0, expr);
        }

        result
    }

    fn function(&mut self, ctx: &mut Context) -> Vec<IR> {
        let mut tokens = vec![];

        self.consume(ctx, TokenKind::Function);

        let name: String =
            if self.consume(ctx, TokenKind::Identifier) || self.consume(ctx, TokenKind::New) {
                ctx.previous().unwrap_or_default().lexeme().to_string()
            } else {
                rand::rng()
                    .sample_iter(&Alphanumeric)
                    .take(12)
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
                    [
                        self.data.add_symbol(argument.lexeme().to_string(), None),
                        1,
                        0,
                    ]
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
                    [self.data.add_symbol(name.lexeme().to_string(), None), 0, 0]
                ));
                self.consume(ctx, TokenKind::Comma);
            }

            body.append(&mut upvalues);
        }
        let mut kind = self.data.add_type(Type::any());
        if self.consume(ctx, TokenKind::SlimArrow) {
            kind = self.get_type(ctx);
        }
        if self.consume(ctx, TokenKind::FatArrow) {
            body.append(&mut self.expression(ctx));
        } else {
            body.append(&mut self.block(ctx));
        }

        let symbol = self.name(name, None);
        body.insert(0, op!(self, ctx, Begin, [ctx.owner().unwrap(), 0, 0]));
        body.push(op!(self, ctx, End));

        let mut func = op!(self, ctx, Function, [symbol, arity, body.len()]);
        let mut func_type = Type::function();
        func_type.set_return(kind);
        for arg in argument_types {
            func_type.add(arg);
        }

        func.set_type(self.data.add_type(func_type));

        tokens.push(func);
        tokens.append(&mut body);

        tokens
    }

    fn prop(&mut self, ctx: &mut Context, owner: usize, public: bool) -> Vec<IR> {
        let kind = self.get_type(ctx);

        let prop_name = ctx.current.lexeme().to_string();
        self.consume(ctx, TokenKind::Identifier);

        let mut action = 2;
        let mut prop = vec![];
        if !self.consume(ctx, TokenKind::SemiColon) {
            self.consume(ctx, TokenKind::Equal);
            action = 1;
            prop.append(&mut self.expr(ctx));
        }
        let mut symbol = owner;
        symbol <<= 32;
        symbol |= self.data.add_symbol(prop_name, None);

        let mut property = op!(self, ctx, Prop, [symbol, action, usize::from(public),]);
        property.set_type(kind);
        prop.push(property);

        prop
    }

    fn method(&mut self, ctx: &mut Context, owner: usize, public: bool) -> Vec<IR> {
        let ns = self.namespace.clone();
        self.namespace = String::new();
        let mut method = self.function(ctx);

        method.insert(2, IR::new(Operation::Bind, Default::default()));
        if let Some(func) = method.first() {
            let [name, arity, len] = func.operands().to_owned();
            let mut symbol = owner;
            symbol <<= 16;
            symbol |= name;
            symbol <<= 1;
            symbol |= usize::from(public);

            let mut member = op!(self, ctx, Method, [symbol, arity, len]);
            member.set_type(func.kind());

            method.insert(1, member);
        }
        self.namespace = ns;
        method.drain(0..1);

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

        assert!(
            ctx.owner().is_none(),
            "Classes can only be declared at the top level of a file"
        );

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
            }
        }

        class.insert(0, op!(self, ctx, Begin, [ctx.owner().unwrap(), 0, 0]));
        class.insert(0, op!(self, ctx, Implement, [contract, owner, class.len()]));
        class.push(op!(self, ctx, End, [owner, 0, 0]));

        ctx.clear_owner();

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
        assert!(
            ctx.owner().is_none(),
            "Classes can only be declared at the top level of a file"
        );
        ctx.set_owner(owner);

        let mut iface = op!(self, ctx, Interface, [owner, 0, 0]);

        ctx.advance();
        while !self.consume(ctx, TokenKind::RightBracket) {
            let mut method = vec![];
            if self.consume(ctx, TokenKind::Pub) {
                ctx.error("Interface methods are implicitly public, so 'pub' is not needed here");
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
                let _ = match ctx.current.kind() {
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
                        [
                            self.data.add_symbol(argument.lexeme().to_string(), None),
                            1,
                            0,
                        ]
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
                let _ = match ctx.current.kind() {
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
            symbol <<= 16;
            symbol |= name_symbol;
            symbol <<= 1;
            symbol |= 1;

            method.insert(0, op!(self, ctx, Method, [symbol, arity, body.len()]));
            method.append(&mut body);

            interface.append(&mut method);
        }

        interface.insert(0, op!(self, ctx, Begin, [ctx.owner().unwrap(), 0, 0]));
        interface.push(op!(self, ctx, End, [owner, 0, 0]));
        iface.operands_mut()[1] = interface.len();
        iface.operands_mut()[2] = interface.len();
        interface.insert(0, iface);

        ctx.clear_owner();

        interface
    }

    fn class(&mut self, ctx: &mut Context) -> Vec<IR> {
        let mut class = vec![];
        let ty = self.get_type(ctx);

        assert!(
            !ctx.has_owner(),
            "Classes can only be declared at the top level of a file"
        );

        let name = if let Kind::Object(name) = self.data.get_type(ty).kind() {
            self.data.symbol_name(name).to_string()
        } else {
            unreachable!("Invalid symbol name");
        };

        let owner = self.name(name.clone(), None);
        self.aliases
            .insert(name, self.data.symbol_name(owner).clone());
        let instance = Type::object(owner);

        self.expect(
            ctx,
            TokenKind::LeftBracket,
            "Expected '{' denoting class body",
        );
        ctx.set_owner(owner);
        while !self.consume(ctx, TokenKind::RightBracket) {
            let public = self.consume(ctx, TokenKind::Pub);
            if self.consume(ctx, TokenKind::Prop) {
                class.append(&mut self.prop(ctx, owner, public));
            } else if self.matches(ctx, TokenKind::Function) {
                class.append(&mut self.method(ctx, owner, public));
            }
        }

        class.insert(0, op!(self, ctx, Begin, [ctx.owner().unwrap(), 0, 0]));
        class.push(op!(self, ctx, End, [owner, 0, 0]));
        let mut cls = op!(self, ctx, Class, [owner, class.len(), 0]);
        cls.set_type(self.data.add_type(instance));
        class.insert(1, cls);

        ctx.clear_owner();

        class
    }

    fn initialize(&mut self, ctx: &mut Context) -> Vec<IR> {
        ctx.advance();
        let mut result = vec![];
        let mut symbol = self.name(ctx.current().lexeme().to_string(), None);
        if self.aliases.contains_key(self.data.symbol_name(symbol)) {
            symbol = self
                .data
                .add_symbol(self.aliases[self.data.symbol_name(symbol)].clone(), None);
        }

        // if self.aliases.contains_key(&name) {
        //     name = self.aliases[&name].to_string();
        // }
        // let symbol = self.data.add_symbol(name, None);

        if self.expect(ctx, TokenKind::Identifier, "Expecting class name") {
            let mut ty = Type::object(symbol);
            if self.consume(ctx, TokenKind::Less) {
                while !self.consume(ctx, TokenKind::Greater) {
                    ty.add_argument(self.get_type(ctx));
                    self.consume(ctx, TokenKind::Comma);
                }
            }

            let mut arity = 0;
            let mut arguments = vec![];
            self.expect(ctx, TokenKind::LeftParenthesis, "Expecting '('");
            while !self.consume(ctx, TokenKind::RightParenthesis) {
                arguments.append(&mut self.expression(ctx));
                arity += 1;
                self.consume(ctx, TokenKind::Comma);
            }

            let mut instance = op!(self, ctx, Instantiate, [symbol, arity, 0]);
            instance.set_type(self.data.add_type(ty));

            result.push(instance);
            result.append(&mut arguments);
        }

        result
    }

    fn this(&mut self, ctx: &mut Context) -> Vec<IR> {
        ctx.advance();
        let mut this = op!(self, ctx, This);
        assert!(
            ctx.owner().is_some(),
            "Using 'this' outside of object context"
        );
        this.set_type(self.data.add_type(Type::object(ctx.owner().unwrap())));
        vec![this]
    }

    fn parse_imports(&mut self, ctx: &mut Context, ns: &Vec<String>) -> Vec<Vec<String>> {
        let mut prefix = vec![];
        let mut children = vec![];

        let segment = ctx.current();
        prefix.append(
            &mut segment
                .lexeme()
                .split("::")
                .map(std::string::ToString::to_string)
                .collect(),
        );
        self.expect(ctx, TokenKind::Identifier, "Expected module identifier");

        if self.consume(ctx, TokenKind::LeftBracket) {
            let mut next = ns.clone();
            next.append(&mut prefix.clone());

            loop {
                self.parse_imports(ctx, &next).iter().for_each(|p| {
                    let mut path = prefix.clone();
                    path.append(&mut p.clone());
                    children.push(path);
                });

                if !self.consume(ctx, TokenKind::Comma) {
                    break;
                }
            }

            self.expect(
                ctx,
                TokenKind::RightBracket,
                "Expected '}' to close module group",
            );
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
                            format!(
                                "{}::{}",
                                ns.join("::"),
                                prefix
                                    .clone()
                                    .into_iter()
                                    .filter(|s| !s.is_empty())
                                    .collect::<Vec<_>>()
                                    .join("::")
                            )
                            .trim_start_matches("::")
                            .to_string()
                        });
                }
            }
        } else if let Some(last) = prefix.last() {
            self.aliases.entry(last.to_string()).or_insert_with(|| {
                format!(
                    "{}::{}",
                    ns.iter()
                        .filter(|&v| !v.is_empty())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("::"),
                    prefix
                        .clone()
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join("::")
                )
                .trim_start_matches("::")
                .to_string()
            });

            self.typechecker.alias(
                self.data.add_symbol(last.to_owned(), None),
                self.data
                    .add_symbol(self.aliases.get(last).cloned().unwrap(), None)
                    .to_owned(),
            );
        }

        if children.is_empty() {
            return vec![
                prefix
                    .clone()
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>(),
            ];
        }

        children
            .clone()
            .into_iter()
            .map(|c| c.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>())
            .collect::<Vec<_>>()
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
            for part in &module {
                joined = joined.join(part);
            }

            let mut paths = vec![];
            for p in INCLUDE_PATHS {
                let mut file = PathBuf::from(p).join(&joined);
                file.set_extension("0s");

                if Path::new(&file).exists() {
                    paths.push(file);
                }
            }

            if paths.is_empty() {
                ctx.error(&format!(
                    "Unable to resolve '{fqn}', because no suitable file has been found"
                ));
            } else if paths.len() > 1 {
                ctx.error(&format!(
                    "Unable to resolve '{}', because of multiple possible locations:\n\t{}",
                    fqn,
                    paths
                        .iter()
                        .map(|path| path.to_str().unwrap_or(""))
                        .collect::<Vec<&str>>()
                        .join("\n\t")
                ));
            }

            let ns = self.namespace.clone();
            let aliases = self.aliases.clone();
            let tc_aliases = self.typechecker.aliases().clone();
            let tc_functions = self.typechecker.functions().clone();

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
            self.typechecker.restore(tc_functions, tc_aliases);
        }

        code
    }

    fn typeof_(&mut self, ctx: &mut Context) -> Vec<IR> {
        ctx.advance();
        let op = op!(self, ctx, TypeOf);
        let mut result = self.precedence(ctx, Precedence::Unary);
        result.push(op);

        result
    }

    fn yield_(&mut self, ctx: &mut Context) -> Vec<IR> {
        ctx.advance();
        let op = op!(self, ctx, Yield);
        let mut result = self.precedence(ctx, Precedence::Unary);
        result.push(op);

        result
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
                tokens.push(op!(self, ctx, Print));

                tokens
            }
            TokenKind::PrintLn => {
                ctx.advance();
                let mut tokens = self.expr(ctx);
                tokens.push(op!(self, ctx, Print, [1, 0, 0]));

                tokens
            }
            TokenKind::Return => {
                ctx.advance();
                let mut tokens = if self.consume(ctx, TokenKind::SemiColon) {
                    let constant = self.data.add_constant(Value::NONE, 0);

                    vec![op!(this, ctx, Const, [constant, 0, 0])]
                } else {
                    self.expr(ctx)
                };

                tokens.push(op!(self, ctx, Leave));

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
            return Err(format!("Unable to open file '{}'", &file));
        };

        let mut ctx = Context::new(Scanner::new(buffer, Some(file.clone())));
        let mut code = vec![];
        self.typechecker.set_file(file.clone());

        while ctx.current().kind() != TokenKind::EOF {
            let stmt = self.statement(&mut ctx);

            code.append(&mut self.typechecker.check(&stmt, self.data));
        }

        ctx.messages.iter().for_each(|m| {
            self.messages.insert(m.clone());
        });

        self.typechecker.get_messages().iter().for_each(|m| {
            self.messages.insert(m.clone());
        });

        if !self
            .messages
            .iter()
            .filter(|m| m.kind() == MessageKind::ERROR)
            .collect::<Vec<_>>()
            .is_empty()
        {
            return Err(format!(
                "Encountered {} errors, {} warnings and {} notices during parsing",
                MessageComposer::default().push(
                    &self
                        .messages
                        .iter()
                        .filter(|v| v.kind() == MessageKind::ERROR)
                        .collect::<Vec<_>>()
                        .len()
                        .to_string(),
                    Some("red"),
                    None,
                    Some(&["bold"])
                ),
                MessageComposer::default().push(
                    &self
                        .messages
                        .iter()
                        .filter(|v| v.kind() == MessageKind::WARNING)
                        .collect::<Vec<_>>()
                        .len()
                        .to_string(),
                    Some("yellow"),
                    None,
                    Some(&["bold"]),
                ),
                MessageComposer::default().push(
                    &self
                        .messages
                        .iter()
                        .filter(|v| v.kind() == MessageKind::INFO)
                        .collect::<Vec<_>>()
                        .len()
                        .to_string(),
                    Some("cyan"),
                    None,
                    Some(&["bold"])
                ),
            ));
        }

        Ok(Program::new(code))
    }

    pub fn parse(&mut self) -> Result<Program<IR>, String> {
        self.parse_internal(self.file.clone())
    }

    pub fn get_messages(&self) -> Vec<&Message> {
        Vec::from_iter(self.messages.iter())
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
                            program.get(idx).code(),
                            *token,
                            "Token #{}, '{:?}' does not match token '{:?}'",
                            idx + 1,
                            program.get(idx).code(),
                            *token,
                        )
                    }

                    assert_eq!(tokens.len(), program.len());
                } else {
                    unreachable!("Unable to parse {}", $code);
                }
            } else {
                unreachable!("Unable to build buffer for '{}'", $code);
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
}
