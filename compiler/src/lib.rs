mod pipeline;
mod typechecker;

use std::{borrow::Borrow, collections::HashMap};

use common::{Byte, Instruction, Interner, Label, Message, Value, likely, unlikely};
use parser::{Expression, SimpleSpan};

pub use pipeline::*;
use regex::Regex;
pub use typechecker::*;

macro_rules! unary {
    ($result: expr, $self: expr, $rhs: expr, $instruction: expr) => {
        $result.append(&mut $self.do_compile($rhs));

        $result.push($instruction);
    };
}
macro_rules! binary {
    ($result: expr, $self: expr, $lhs: expr, $rhs: expr, $instruction: expr) => {
        $result.append(&mut $self.do_compile($lhs));
        $result.append(&mut $self.do_compile($rhs));

        $result.push($instruction);
    };
}

#[derive(Default, Clone)]
struct Context {
    current: Option<String>,
    variables: Interner<String>,
    constants: HashMap<usize, bool>,
    defers: Vec<(usize, usize, Vec<Byte<Value>>)>,

    prev: Option<Box<Self>>,
}

pub struct Compiler {
    bytecode: Vec<Byte<Value>>,
    aliases: HashMap<String, String>,
    functions: HashMap<String, usize>,
    native: HashMap<String, usize>,
    // --
    messages: Vec<Message>,
    context: Context,
    // --
    typechecker: Typechecker,
}

impl Default for Compiler {
    fn default() -> Self {
        let mut bytecode = Vec::with_capacity(1024);
        bytecode.append(&mut vec![
            Byte::new_with_operands(Instruction::CALL, [usize::MAX, 0]),
            Byte::new(Instruction::HALT),
        ]);

        Self {
            bytecode,
            aliases: HashMap::default(),
            functions: HashMap::with_capacity(32),
            native: HashMap::default(),
            // ---
            messages: Vec::default(),
            context: Context::default(),
            // ---
            typechecker: Typechecker::default(),
        }
    }
}

impl<'ctx> Context {
    fn child(&self) -> Self {
        Self {
            current: self.current.clone(),
            defers: Vec::default(),
            constants: self.constants.clone(),
            variables: self.variables.clone(),
            prev: Some(Box::new(self.to_owned())),
        }
    }
}

impl<'ctx> Context {
    pub fn get_prev(&self) -> &Option<Box<Self>> {
        &self.prev
    }
}

impl Compiler {
    pub fn get_function(&self, name: &str) -> usize {
        self.functions[name]
    }

    pub fn get_messages(self) -> Vec<Message> {
        self.messages
    }

    pub fn register(&mut self, name: &str, params: &[Type], returns: Type) -> &mut Self {
        let idx = self.native.len();
        self.native.insert(name.to_string(), idx);
        self.typechecker
            .register_native_function(name, params, returns);

        self
    }

    fn resolve_variable<'compiler>(
        &self,
        variable: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> String {
        match variable.1.borrow() {
            Expression::Identifier(n) => n.to_string(),
            f => {
                dbg!(&f);
                todo!("Function name as expression")
            }
        }
    }

    fn typecheck<'check>(&mut self, ast: &(SimpleSpan, Box<Expression<'check>>)) -> Type {
        self.typechecker.check(ast)
    }

    fn do_compile<'compiler>(
        &mut self,
        ast: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> Vec<Byte<Value>> {
        let mut bytecode = vec![];
        let _ = self.typecheck(ast);
        let (span, child) = ast;

        match child.borrow() {
            Expression::Comment(_) => (),
            Expression::Use {
                path: _,
                name,
                alias,
            } => {
                self.aliases
                    .insert(alias.clone().unwrap_or(name.to_string()), name.to_string());
            }
            Expression::Program(children) | Expression::Fragment(children) => {
                for child in children {
                    bytecode.append(&mut self.do_compile(child));
                }
            }
            Expression::Block(children) => {
                let ctx = self.context.child();
                self.context = ctx;
                for child in children {
                    bytecode.append(&mut self.do_compile(child));
                }

                self.context = *self.context.get_prev().clone().unwrap();
            }
            Expression::Function {
                name,
                args,
                returns: _returns,
                body,
            } => {
                self.functions.insert(name.to_string(), self.bytecode.len());

                for arg in args {
                    let ty = self.typecheck(arg);
                    let mut a = self.do_compile(arg);

                    if matches!(ty, Type::OBJECT(_) | Type::STRING) {
                        self.bytecode.push(Byte::new_with_operands(
                            Instruction::ACQUIRE,
                            [
                                match ty {
                                    Type::STRING => 1,
                                    _ => 0,
                                },
                                0,
                            ],
                        ));
                    }
                    self.bytecode.append(&mut a);
                }

                for child in body {
                    let mut c = self.do_compile(child);
                    self.bytecode.append(&mut c);
                }

                for (offset, arity, x) in self.context.defers.iter() {
                    self.bytecode.append(&mut x.clone());
                    self.bytecode.push(Byte::new_with_operands(
                        Instruction::CALL,
                        [*offset, *arity],
                    ));
                }

                for variable in self.context.variables.iter() {
                    if let (Some(symbol), Some(ty)) = (
                        self.context.variables.key(variable),
                        self.typechecker.get_variable_type(variable),
                    ) {
                        if matches!(ty, Type::OBJECT(_) | Type::STRING) {
                            self.bytecode
                                .push(Byte::new_with_operands(Instruction::LOAD, [symbol, 0]));
                            self.bytecode.push(Byte::new_with_operands(
                                Instruction::RELEASE,
                                [
                                    match ty {
                                        Type::STRING => 1,
                                        _ => 0,
                                    },
                                    0,
                                ],
                            ));
                        }
                    }
                }
                self.bytecode
                    .push(Byte::new_with_value(Instruction::CONST, Value::default()));
                self.bytecode.push(Byte::new(Instruction::RETURN));
            }
            Expression::Expr(child) | Expression::Statement(child) => {
                bytecode.append(&mut self.do_compile(child))
            }
            Expression::ExprStatement(child) => {
                bytecode.append(&mut self.do_compile(child));
                bytecode.push(Byte::new(Instruction::POP));
            }
            Expression::Print(format, params) => {
                bytecode.append(&mut self.do_compile(format));
                let mut params_len = 0;
                if let Some(params) = params {
                    params_len = params.len();
                    params.iter().for_each(|param| {
                        bytecode.append(&mut self.do_compile(param));
                    });
                }
                bytecode.push(Byte::new_with_operands(
                    Instruction::FORMAT,
                    [params_len, 0],
                ));
                bytecode.push(Byte::new(Instruction::PRINT));
            }
            Expression::Format(format, params) => {
                bytecode.append(&mut self.do_compile(format));
                let mut params_len = 0;
                if let Some(params) = params {
                    params_len = params.len();
                    params.iter().for_each(|param| {
                        bytecode.append(&mut self.do_compile(param));
                    });
                }
                bytecode.push(Byte::new_with_operands(
                    Instruction::FORMAT,
                    [params_len, 0],
                ));
            }
            Expression::Return(expr) | Expression::ImplicitReturn(expr) => {
                for (offset, arity, x) in self.context.defers.iter() {
                    self.bytecode.append(&mut x.clone());
                    self.bytecode.push(Byte::new_with_operands(
                        Instruction::CALL,
                        [*offset, *arity],
                    ));
                }

                for variable in self.context.variables.iter() {
                    if let (Some(symbol), Some(ty)) = (
                        self.context.variables.key(variable),
                        self.typechecker.get_variable_type(variable),
                    ) {
                        if matches!(ty, Type::OBJECT(_)) || matches!(ty, Type::STRING) {
                            bytecode.push(Byte::new_with_operands(Instruction::LOAD, [symbol, 0]));
                            bytecode.push(Byte::new(Instruction::RELEASE));
                        }
                    }
                }
                bytecode.append(&mut self.do_compile(expr));
                if !matches!(child.borrow(), Expression::ImplicitReturn(_)) {
                    bytecode.push(Byte::new(Instruction::RETURN));
                }
            }
            Expression::Yield(child) => {
                bytecode.append(&mut self.do_compile(child));
                bytecode.push(Byte::new(Instruction::SUSP));
            }
            Expression::Defer(deps, child) => {
                let mut body = vec![Byte::new_with_operands(Instruction::JMP, [usize::MAX, 0])];
                let mut args = vec![];
                let mut arity = 0;
                for arg in deps.iter() {
                    args.append(&mut self.do_compile(arg));
                    arity += 1;
                }

                self.context
                    .defers
                    .push((self.bytecode.len() + body.len(), arity, args));

                body.append(&mut self.do_compile(child));
                body.push(Byte::new_with_value(Instruction::CONST, Value::from(0i64)));
                body.push(Byte::new(Instruction::RETURN));

                let total_length = self.bytecode.len();
                let current_length = body.len() + bytecode.len();
                if let Some(v) = body.first_mut() {
                    *v = Byte::new_with_operands(
                        Instruction::JMP,
                        [total_length + current_length, 0],
                    );
                }

                bytecode.append(&mut body);
            }
            Expression::Call { name, args } => {
                let identifier = self.resolve_variable(name);
                let n = self.aliases.get(&identifier).unwrap_or(&identifier);

                if let Some(offset) = self.functions.get(n).copied() {
                    for arg in args {
                        bytecode.append(&mut self.do_compile(arg));
                    }

                    bytecode.push(Byte::new_with_operands(
                        Instruction::CALL,
                        [offset, args.len()],
                    ));
                } else if let Some(offset) = self.native.get(n) {
                    bytecode.push(Byte::new_with_operands(
                        Instruction::NATIVE,
                        [*offset, args.len()],
                    ))
                } else {
                    let mut message =
                        Message::error("Unknown function".to_string(), span.into_range());
                    message.push(Label::new(
                        format!("Unable to call unknown function '{}'", n),
                        span.into_range(),
                    ));
                    self.messages.push(message);
                }
            }
            Expression::Identifier(n) => {
                if let Some(symbol) = self.context.variables.key(&n.to_string()) {
                    bytecode.push(Byte::new_with_operands(Instruction::LOAD, [symbol, 0]));
                } else {
                    let mut message =
                        Message::error("Unknown variable".to_string(), span.into_range());
                    message.push(Label::new(
                        format!("Unknown variable '{}'", n),
                        span.into_range(),
                    ));
                    self.messages.push(message);
                }
            }
            Expression::If {
                condition,
                body,
                alternative,
            } => {
                let mut condition = self.do_compile(condition);
                let mut body = self.do_compile(body);

                let mut alternative = alternative
                    .as_ref()
                    .map(|v| self.do_compile(v))
                    .unwrap_or_default();

                if !alternative.is_empty() {
                    body.push(Byte::new_with_operands(
                        Instruction::JMPR,
                        [
                            // self.bytecode.len()
                                // + condition.len()
                                // + body.len()
                                2 // This instruction + the JMPF bellow
                                + alternative.len(),
                            0,
                        ],
                    ));
                }

                let current_len = body.len() + condition.len();
                bytecode.append(&mut condition);

                bytecode.push(Byte::new_with_operands(
                    Instruction::JMPF,
                    [self.bytecode.len() + current_len + 1, 0],
                ));
                bytecode.append(&mut body);
                bytecode.append(&mut alternative);
            }
            Expression::Le(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(if self.typecheck(lhs) == Type::FLOAT {
                        Instruction::LEF
                    } else {
                        Instruction::LE
                    },)
                );
            }
            Expression::Gt(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(if self.typecheck(lhs) == Type::FLOAT {
                        Instruction::GTF
                    } else {
                        Instruction::GT
                    },)
                );
            }
            Expression::Leq(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(if self.typecheck(lhs) == Type::FLOAT {
                        Instruction::GTF
                    } else {
                        Instruction::GT
                    },)
                );

                bytecode.push(Byte::new(Instruction::NOT))
            }
            Expression::Geq(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(if self.typecheck(lhs) == Type::FLOAT {
                        Instruction::LEF
                    } else {
                        Instruction::LE
                    },)
                );

                bytecode.push(Byte::new(Instruction::NOT))
            }
            Expression::Eq(lhs, rhs) => {
                binary!(bytecode, self, lhs, rhs, Byte::new(Instruction::EQ));

                // bytecode.push(Byte::new(Instruction::NOT))
            }
            Expression::Not(lhs) => {
                unary!(bytecode, self, lhs, Byte::new(Instruction::NOT));
            }
            Expression::Negate(lhs) => {
                unary!(bytecode, self, lhs, Byte::new(Instruction::NEG));
            }
            Expression::Add(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(if likely(self.typecheck(lhs) == Type::FLOAT) {
                        Instruction::ADDF
                    } else {
                        Instruction::ADD
                    },)
                );
            }
            Expression::Sub(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(if likely(self.typecheck(lhs) == Type::FLOAT) {
                        Instruction::SUBF
                    } else {
                        Instruction::SUB
                    },)
                );
            }
            Expression::Mul(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(if likely(self.typecheck(lhs) == Type::FLOAT) {
                        Instruction::MULF
                    } else {
                        Instruction::MUL
                    },)
                );
            }
            Expression::Mod(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(if likely(self.typecheck(lhs) == Type::FLOAT) {
                        Instruction::MODF
                    } else {
                        Instruction::MOD
                    },)
                );
            }
            Expression::Div(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(if likely(self.typecheck(lhs) == Type::FLOAT) {
                        Instruction::DIVF
                    } else {
                        Instruction::DIV
                    },)
                );
            }
            Expression::And(lhs, rhs) => {
                binary!(bytecode, self, lhs, rhs, Byte::new(Instruction::AND));
            }
            Expression::Integer(num) => {
                bytecode.push(Byte::new_with_value(Instruction::CONST, Value::from(*num)))
            }
            Expression::Bool(state) => bytecode.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(*state),
            )),
            Expression::Float(num) => {
                bytecode.push(Byte::new_with_value(Instruction::CONST, Value::from(*num)))
            }
            Expression::String(str) => {
                let mut escaped = str
                    .replace("\\n", "\n")
                    .replace("\\r", "\r")
                    .replace("\\t", "\t")
                    .replace("\\0", "\0");

                if let Ok(re) = Regex::new(r"\\u(?<code>\d{1,})").map_err(|e| dbg!(e)) {
                    while let Some(captures) = re.captures(escaped.as_str()) {
                        let unicode = captures.name("code").unwrap().as_str();

                        escaped = escaped.replace(
                            format!("\\u{}", unicode).as_str(),
                            char::from_u32(
                                captures
                                    .name("code")
                                    .unwrap()
                                    .as_str()
                                    .parse()
                                    .unwrap_or_default(),
                            )
                            .unwrap_or_default()
                            .to_string()
                            .as_str(),
                        )
                    }
                }
                let idx = bytecode.len();

                let mut count = 0;

                for ch in escaped.chars() {
                    count += 1;
                    bytecode.push(Byte::new_with_operands(Instruction::DATA, [ch as usize, 0]));
                }

                bytecode.insert(
                    idx,
                    Byte::new_with_operands(Instruction::STRING, [count, 0]),
                );
            }
            Expression::Variable(name, _ty) => {
                let name = self.resolve_variable(name);

                if unlikely(self.context.variables.contains(&name)) {
                    let mut message =
                        Message::error("Variable redeclaration".to_string(), span.into_range());
                    message.push(Label::new(
                        format!("Variable '{}' already declared", name),
                        span.into_range(),
                    ));
                    self.messages.push(message);
                }

                self.context.variables.intern(name);
            }
            Expression::Constant(name, _ty) => {
                let name = self.resolve_variable(name);
                if self.context.variables.contains(&name) {
                    let mut message =
                        Message::error("Constand redeclaration".to_string(), span.into_range());
                    message.push(Label::new(
                        format!("Constant '{}' already declared", name),
                        span.into_range(),
                    ));
                    self.messages.push(message);
                }

                let symbol = self.context.variables.intern(name.clone());

                self.context.constants.insert(symbol, false);
            }
            Expression::Assignment(name, value) => {
                let name = self.resolve_variable(name);

                if let Some(symbol) = self.context.variables.key(&name) {
                    if unlikely(self.context.constants.contains_key(&symbol)) {
                        let assigned = likely(*self.context.constants.get(&symbol).unwrap());

                        if !assigned {
                            self.context.constants.entry(symbol).and_modify(|state| {
                                *state = true;
                            });
                        } else {
                            let mut message =
                                Message::error("Assignment error".to_string(), span.into_range());
                            message.push(Label::new(
                                format!(
                                    "Unable to assign to an already assigned constant '{}'",
                                    name
                                ),
                                span.into_range(),
                            ));
                            self.messages.push(message);
                        }
                    }

                    let ty = self.typecheck(value);
                    let mut expr = self.do_compile(value);

                    bytecode.append(&mut expr);
                    bytecode.push(Byte::new_with_operands(Instruction::STORE, [symbol, 0]));

                    if matches!(ty, Type::OBJECT(_) | Type::STRING) {
                        bytecode.push(Byte::new_with_operands(
                            Instruction::ACQUIRE,
                            [
                                match ty {
                                    Type::STRING => 1,
                                    _ => 0,
                                },
                                0,
                            ],
                        ));
                    }
                } else {
                    let mut message =
                        Message::error("Undefined variable".to_string(), span.into_range());
                    message.push(Label::new(
                        format!(
                            "Unable to assign to a non-existing variable/constant '{}'",
                            name
                        ),
                        span.into_range(),
                    ));
                    self.messages.push(message);
                }
            }
            Expression::Match(lhs, children) => {
                let lhs = self.do_compile(lhs);

                let mut jumps: Vec<usize> = Vec::with_capacity(children.len());

                for (rhs, body) in children {
                    bytecode.append(&mut lhs.clone());
                    bytecode.append(&mut self.do_compile(rhs));
                    bytecode.push(Byte::new(Instruction::EQ));

                    let mut body = self.do_compile(body);
                    bytecode.push(Byte::new_with_operands(
                        Instruction::JMPF,
                        [self.bytecode.len() + bytecode.len() + body.len() + 1, 0],
                    ));
                    bytecode.append(&mut body);
                    jumps.push(bytecode.len());
                    bytecode.push(Byte::new_with_operands(Instruction::JMP, [usize::MAX, 0]));
                }

                let len = bytecode.len();
                for jump in jumps {
                    if let Some(instruction) = bytecode.get_mut(jump) {
                        *instruction = Byte::new_with_operands(
                            Instruction::JMP,
                            [self.bytecode.len() + len, 0],
                        );
                    }
                }
            }
            _expr => {
                let mut message =
                    Message::error("Unknown expression".to_string(), span.into_range());
                message.push(Label::new(
                    "Unable to compile expression".to_string(),
                    span.into_range(),
                ));
                self.messages.push(message);
                #[cfg(debug_assertions)]
                dbg!(_expr);
            }
        }

        bytecode
    }

    pub fn compile<'compiler>(
        &mut self,
        ast: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> Vec<Byte<Value>> {
        let mut program = self.do_compile(ast);

        self.messages
            .append(&mut self.typechecker.get_messages().collect());
        // let messages = self.typechecker.get_messages();
        // self.messages.reserve(messages.len());
        // for message in messages {
        //     self.messages.push(message.clone());
        // }

        self.bytecode.append(&mut program);

        self.bytecode.clone()
    }
}
