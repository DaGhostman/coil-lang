use common::symbols::SymbolTable;
use rand::{Rng, distr::Alphanumeric, rng};
use std::str::FromStr;

pub mod precedence;

use common::program::Program;
use common::types::Type;
use common::{
    Value, ValueKind,
    interner::Interner,
    opcodes::{IR, Operation},
};
use precedence::Precedence;
use scanner::{
    Scanner,
    tokens::{Token, TokenKind},
};

pub struct Context<'ctx> {
    scanner: &'ctx mut Scanner,
    current: Token,
    previous: Option<Token>,
}

impl<'ctx> Context<'ctx> {
    pub fn new(scanner: &'ctx mut Scanner) -> Self {
        let current = scanner.scan().unwrap_or_default();

        Self {
            scanner,
            current,
            previous: None,
        }
    }

    pub fn current(&self) -> &Token {
        &self.current
    }

    pub fn previous(&self) -> Option<Token> {
        self.previous.clone()
    }

    pub fn advance(&mut self) {
        self.previous = Some(self.current.clone());

        self.current = self.scanner.scan().unwrap_or_default();
    }

    pub fn tell(&self) -> usize {
        self.scanner.tell()
    }
}

pub struct Parser {
    constants: Interner<Value>,
    symbols: SymbolTable,
    strings: Interner<String>,
}

impl Default for Parser {
    fn default() -> Self {
        let mut constants = Interner::default();
        constants.intern(Value::new(ValueKind::NONE));

        Self {
            constants,
            symbols: SymbolTable::new(),
            strings: Interner::default(),
        }
    }
}

impl Parser {
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

    fn patch(&self, tokens: &mut [IR], idx: usize) {
        let length = tokens.len();
        if let Some(token) = tokens.get_mut(idx) {
            // dbg!(&length, &idx);
            *token = match token.code() {
                Operation::ConditionJump => {
                    IR::new(Operation::ConditionJump, Some([length - idx, 0, 0]))
                }
                Operation::Jump => IR::new(Operation::Jump, Some([length - idx, 0, 0])),
                // Operation::Break => IR::new(Operation::Break, Some([tokens.len() - (idx) - 1, 0,0])),
                // Operation::Continue => IR::new(Operation::Continue, Some([tokens.len() - (idx) - 1, 0,0])),
                _ => unreachable!("Should not attempt to jump patch non-jumping instruction"),
            }
        }
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
            TokenKind::True => Value::new(ValueKind::BOOLEAN(true)),
            TokenKind::False => Value::new(ValueKind::BOOLEAN(false)),
            _ => todo!("Fail to build a boolean"),
        };
        let constant = self.constants.intern(value);

        ctx.advance();
        vec![IR::new(Operation::Const, Some([constant, 0, 0]))]
    }

    fn number(&mut self, ctx: &mut Context) -> Vec<IR> {
        let value = match ctx.current().lexeme().as_bytes().get(1).map(|c| *c as char) {
            Some('o') => {
                if let Ok(int) = i64::from_str_radix(ctx.current().lexeme(), 8) {
                    Value::new(ValueKind::INTEGER(int))
                } else {
                    todo!("Fail to parse number as octal");
                }
            }
            Some('x') => {
                if let Ok(int) = i64::from_str_radix(ctx.current().lexeme(), 16) {
                    Value::new(ValueKind::INTEGER(int))
                } else {
                    todo!("Fail to parse number as hexadecimal");
                }
            }
            Some('b') => {
                if let Ok(int) = i64::from_str_radix(ctx.current().lexeme(), 2) {
                    Value::new(ValueKind::INTEGER(int))
                } else {
                    todo!("Fail to parse number as binary");
                }
            }
            _ => {
                if let Ok(int) = ctx.current().lexeme().parse::<i64>() {
                    Value::new(ValueKind::INTEGER(int))
                } else {
                    todo!("Fail to parse '{}' as decimal", ctx.current().lexeme());
                }
            }
        };
        ctx.advance();

        let constant = self.constants.intern(value);

        vec![IR::new(Operation::Const, Some([constant, 0, 0]))]
    }

    fn float(&mut self, ctx: &mut Context) -> Vec<IR> {
        let value = if let Ok(value) = f64::from_str(ctx.current().lexeme()) {
            Value::new(ValueKind::FLOAT(value))
        } else {
            todo!("Fail to parse number as float");
        };

        ctx.advance();
        let constant = self.constants.intern(value);

        vec![IR::new(Operation::Const, Some([constant, 0, 0]))]
    }

    fn string(&mut self, ctx: &mut Context) -> Vec<IR> {
        let string = self.strings.intern(ctx.current().lexeme().to_string());
        let constant = self.constants.intern(Value::new(ValueKind::STRING(string)));

        ctx.advance();
        vec![IR::new(Operation::Const, Some([constant, 0, 0]))]
    }

    fn identifier(&mut self, ctx: &mut Context) -> Vec<IR> {
        let symbol = self
            .symbols
            .insert(ctx.current().lexeme().to_string(), None);
        ctx.advance();

        let mut tokens = vec![];
        if self.consume(ctx, TokenKind::LeftParenthesis) {
            let mut arity = 0;
            while !self.consume(ctx, TokenKind::RightParenthesis) {
                tokens.append(&mut self.expression(ctx));
                self.consume(ctx, TokenKind::Comma);
                arity += 1;
            }

            tokens.push(IR::new(Operation::Call, Some([symbol, arity, 0])));
        } else {
            // tokens.append(&mut self.expression(ctx));
            tokens.push(IR::new(Operation::Load, Some([symbol, 0, 0])));
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

        tokens.push(IR::new(Operation::Array, Some([arity, 0, 0])));

        tokens
    }

    fn comparison(&mut self, ctx: &mut Context) -> Vec<IR> {
        let operator = ctx.current().clone();
        ctx.advance();
        let mut tokens = self.precedence(ctx, Precedence::get(operator.kind()).next());

        match operator.kind() {
            TokenKind::Less => tokens.push(IR::new(Operation::Less, None)),
            TokenKind::LessEqual => tokens.push(IR::new(Operation::LessEqual, None)),
            TokenKind::Greater => tokens.push(IR::new(Operation::Greater, None)),
            TokenKind::GreaterEqual => tokens.push(IR::new(Operation::GreaterEqual, None)),
            _ => unreachable!("No other comparison"),
        }

        tokens
    }

    fn equality(&mut self, ctx: &mut Context) -> Vec<IR> {
        let operator = ctx.current().clone();
        ctx.advance();
        let mut tokens = self.precedence(ctx, Precedence::get(operator.kind()).next());

        match operator.kind() {
            TokenKind::EqualEqual => tokens.push(IR::new(Operation::Equal, None)),
            TokenKind::BangEqual => tokens.push(IR::new(Operation::NotEqual, None)),
            _ => unreachable!("No other equality"),
        }

        tokens
    }

    fn binary(&mut self, ctx: &mut Context, _assignment: bool) -> Vec<IR> {
        let operator = ctx.current().clone();
        ctx.advance();
        let mut tokens = self.precedence(ctx, Precedence::get(operator.kind()).next());

        match operator.kind() {
            TokenKind::Plus => tokens.push(IR::new(Operation::Add, None)),
            TokenKind::Minus => tokens.push(IR::new(Operation::Subtract, None)),
            TokenKind::Star => tokens.push(IR::new(Operation::Multiply, None)),
            TokenKind::StarStar => tokens.push(IR::new(Operation::Pow, None)),
            TokenKind::Slash => tokens.push(IR::new(Operation::Divide, None)),
            TokenKind::Percent => tokens.push(IR::new(Operation::Modulo, None)),
            TokenKind::Less => tokens.push(IR::new(Operation::Less, None)),
            TokenKind::LessLess => tokens.push(IR::new(Operation::LeftShift, None)),
            TokenKind::Greater => tokens.push(IR::new(Operation::Greater, None)),
            TokenKind::GreaterGreater => tokens.push(IR::new(Operation::RightShift, None)),
            TokenKind::Caret => tokens.push(IR::new(Operation::BitXor, None)),
            TokenKind::Pipe => tokens.push(IR::new(Operation::BitOr, None)),
            TokenKind::Ampersand => tokens.push(IR::new(Operation::BitAnd, None)),
            TokenKind::Or => tokens.push(IR::new(Operation::Or, None)),
            TokenKind::And => tokens.push(IR::new(Operation::And, None)),
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
                t.push(IR::new(Operation::Leave, None));

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
                    tokens.push(IR::new(Operation::Unwrap, None));
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
                    tokens.push(IR::new(Operation::UnwrapError, None));
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
                    .symbols
                    .insert(ctx.current().lexeme().to_string(), None);
                tokens.push(IR::new(Operation::Store, Some([symbol, 0, 0])));
                // ctx.advance();
                tokens.append(&mut self.expression(ctx));
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
            tokens.push(IR::new(Operation::Or, None));
        }

        tokens.append(&mut predicate.clone());
        tokens.push(IR::new(Operation::Equal, None));
        // tokens.push(IR::new(Operation::ConditionJump, None));

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

        tokens.insert(0, IR::new(Operation::Match, Some([tokens.len(), 0, 0])));

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

                tokens.push(IR::new(
                    Operation::Invoke,
                    Some([
                        self.symbols.insert(name.lexeme().to_string(), None),
                        arity,
                        0,
                    ]),
                ));
            } else {
                todo!("Handle remainder of cases");
            } // TODO: Implement Increment for properties
        }

        tokens
    }

    fn range(&mut self, ctx: &mut Context) -> Vec<IR> {
        self.expect(ctx, TokenKind::DotDot, "Expected range symbol");
        let mut tokens = self.expression(ctx);
        tokens.push(IR::new(Operation::Range, None));

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
            TokenKind::LeftBrace => self.array(ctx),
            TokenKind::Match => self.match_expression2(ctx),
            TokenKind::If => self.if_expression(ctx),
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
        tokens.push(IR::new(Operation::Pop, None));

        tokens
    }

    fn block(&mut self, ctx: &mut Context) -> Vec<IR> {
        let mut tokens = vec![];
        if self.expect(
            ctx,
            TokenKind::LeftBracket,
            "Expecting '{' at the start of a block",
        ) {
            while !self.consume(ctx, TokenKind::RightBracket) {
                tokens.append(&mut self.statement(ctx));
            }
        }

        tokens
    }

    fn function(&mut self, ctx: &mut Context) -> Vec<IR> {
        let mut tokens = vec![];

        let name: String = if self.expect(ctx, TokenKind::Identifier, "Expected function name") {
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
        while !self.consume(ctx, TokenKind::RightParenthesis) {
            let type_ = match ctx.current.kind() {
                TokenKind::Int => Type::Integer,
                TokenKind::Str => Type::String,
                TokenKind::Bool => Type::Bool,
                TokenKind::Float => Type::Float,
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
                "Expected function argument identifier",
            ) {
                body.insert(
                    0,
                    IR::new(
                        Operation::Argument,
                        Some([
                            self.symbols.insert(argument.lexeme().to_string(), None),
                            type_.into(),
                            arity,
                        ]),
                    ),
                );
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
        body.append(&mut self.block(ctx));

        let symbol = self.symbols.insert(name, None);
        tokens.push(IR::new(
            Operation::Function,
            Some([symbol, arity, body.len()]),
        ));
        tokens.append(&mut body);

        tokens
    }

    fn statement(&mut self, ctx: &mut Context) -> Vec<IR> {
        match ctx.current().kind() {
            TokenKind::Print => {
                ctx.advance();
                let mut tokens = self.expr(ctx);
                tokens.push(IR::new(Operation::Print, None));

                tokens
            }
            TokenKind::PrintLn => {
                ctx.advance();
                let mut tokens = self.expr(ctx);
                tokens.push(IR::new(Operation::Print, Some([1, 0, 0])));

                tokens
            }
            TokenKind::Return => {
                ctx.advance();
                let mut tokens = self.expr(ctx);
                tokens.push(IR::new(Operation::Leave, None));

                tokens
            }
            TokenKind::Function => {
                ctx.advance();
                self.function(ctx)
            }
            TokenKind::If => {
                ctx.advance();
                self.if_expression(ctx)
            }
            _ => self.expr_statement(ctx),
        }
    }

    pub fn parse(&mut self, scanner: &mut Scanner) -> Result<Program<IR>, String> {
        let mut ctx = Context::new(scanner);
        let mut code = vec![];

        while ctx.current().kind() != TokenKind::EOF {
            code.append(&mut self.statement(&mut ctx));
        }

        let mut program = Program::new(code);
        program.with_constants(self.constants.dump());
        program.with_symbols(self.symbols.clone());

        Ok(program)
    }
}

#[cfg(test)]
mod tests {
    use common::opcodes::Operation;
    use scanner::{Scanner, buffer::Buffer};

    use crate::Parser;

    macro_rules! assert_parsed_expression {
        ($code:expr, $($token:expr),+) => {
            if let Ok(buffer) = Buffer::try_from($code) {
                let mut scanner = Scanner::new(buffer, None);

                if let Ok(program) = Parser::default().parse(&mut scanner) {
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
        // assert_parsed_expression!(
        //     "42 - 69;",
        //     Operation::Push,
        //     Operation::Push,
        //     Operation::Subtract,
        //     Operation::Pop
        // );
        // assert_parsed_expression!(
        //     "42 * 69;",
        //     Operation::Push,
        //     Operation::Push,
        //     Operation::Multiply,
        //     Operation::Pop
        // );
        // assert_parsed_expression!(
        //     "42 / 69;",
        //     Operation::Push,
        //     Operation::Push,
        //     Operation::Divide,
        //     Operation::Pop
        // );
        // assert_parsed_expression!(
        //     "42 % 69;",
        //     Operation::Push,
        //     Operation::Push,
        //     Operation::Modulo,
        //     Operation::Pop
        // );
        // assert_parsed_expression!(
        //     "42 ** 69;",
        //     Operation::Push,
        //     Operation::Push,
        //     Operation::Pow,
        //     Operation::Pop
        // );
        // assert_parsed_expression!(
        //     "42 << 69;",
        //     Operation::Push,
        //     Operation::Push,
        //     Operation::LeftShift,
        //     Operation::Pop
        // );
        // assert_parsed_expression!(
        //     "42 >> 69;",
        //     Operation::Push,
        //     Operation::Push,
        //     Operation::RightShift,
        //     Operation::Pop
        // );
        // assert_parsed_expression!(
        //     "42 ^ 69;",
        //     Operation::Push,
        //     Operation::Push,
        //     Operation::BitXor,
        //     Operation::Pop
        // );
        // assert_parsed_expression!(
        //     "42 | 69;",
        //     Operation::Push,
        //     Operation::Push,
        //     Operation::BitOr,
        //     Operation::Pop
        // );
        // assert_parsed_expression!(
        //     "42 & 69;",
        //     Operation::Push,
        //     Operation::Push,
        //     Operation::BitAnd,
        //     Operation::Pop
        // );
        // assert_parsed_expression!(
        //     "42 == 69;",
        //     Operation::Push,
        //     Operation::Push,
        //     Operation::Equal,
        //     Operation::Pop
        // );
        // assert_parsed_expression!(
        //     "42 != 69;",
        //     Operation::Push,
        //     Operation::Push,
        //     Operation::NotEqual,
        //     Operation::Pop
        // );
        // assert_parsed_expression!(
        //     "42 < 69;",
        //     Operation::Push,
        //     Operation::Push,
        //     Operation::Less,
        //     Operation::Pop
        // );
        // assert_parsed_expression!(
        //     "42 <= 69;",
        //     Operation::Push,
        //     Operation::Push,
        //     Operation::LessEqual,
        //     Operation::Pop
        // );
        // assert_parsed_expression!(
        //     "42 > 69;",
        //     Operation::Push,
        //     Operation::Push,
        //     Operation::Greater,
        //     Operation::Pop
        // );
        // assert_parsed_expression!(
        //     "42 >= 69;",
        //     Operation::Push,
        //     Operation::Push,
        //     Operation::GreaterEqual,
        //     Operation::Pop
        // );
        // assert_parsed_expression!(
        //     "true or true;",
        //     Operation::Push,
        //     Operation::Push,
        //     Operation::Or
        // );
        // assert_parsed_expression!(
        //     "true and true;",
        //     Operation::Push,
        //     Operation::Push,
        //     Operation::And,
        //     Operation::Pop
        // );
    }

    // #[test]
    // fn test_precedence() {
    //     assert_parsed_expression!(
    //         "3 * 4 + 2",
    //         Operation::Push,
    //         Operation::Push,
    //         Operation::Multiply,
    //         Operation::Push,
    //         Operation::Add,
    //         Operation::Pop
    //     );
    //
    //     assert_parsed_expression!(
    //         "3 * (4 + 2)",
    //         Operation::Push,
    //         Operation::Push,
    //         Operation::Push,
    //         Operation::Add,
    //         Operation::Multiply,
    //         Operation::Pop
    //     );
    //
    //     assert_parsed_expression!(
    //         "3 * (4 + 2) + 5;",
    //         Operation::Push,
    //         Operation::Push,
    //         Operation::Push,
    //         Operation::Add,
    //         Operation::Multiply,
    //         Operation::Push,
    //         Operation::Add,
    //         Operation::Pop
    //     );
    //
    //     assert_parsed_expression!(
    //         "3 * ((4 + 2) + 5);",
    //         Operation::Push,
    //         Operation::Push,
    //         Operation::Push,
    //         Operation::Add,
    //         Operation::Push,
    //         Operation::Add,
    //         Operation::Multiply,
    //         Operation::Pop
    //     );
    // }
    //
    // #[test]
    // fn test_template_expression() {
    //     assert_parsed_expression!(
    //         "`Hello, ${name}`;",
    //         Operation::Push,
    //         Operation::Load,
    //         Operation::Add,
    //         Operation::Push,
    //         Operation::Add,
    //         Operation::Pop
    //     );
    // }
    //
    // #[test]
    // fn test_match_expression() {
    //     assert_parsed_expression!(
    //         "match true { true => 'sadge' };",
    //         Operation::Push,  // Left
    //         Operation::Push,  // Right
    //         Operation::Equal, // Equals
    //         Operation::Match, // conditional jump
    //         Operation::Push,  // the match arm body
    //         Operation::Jump   // jump out of the match expr
    //     );
    // }
}
