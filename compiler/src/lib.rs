mod hm_typechecker;
mod pipeline;
mod types;

use std::{borrow::Borrow, collections::HashMap};

use common::{Byte, Instruction, Interner, Label, Message, Value, likely, unlikely};
use parser::{SimpleSpan, ast::Expression};

pub use pipeline::*;
pub use crate::types::ty::Type;

use crate::hm_typechecker::HmTypeChecker;

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
    symbols: Interner<String>,
    assignments: HashMap<String, bool>,
    constants: HashMap<usize, bool>,
    defers: Vec<usize>,
    classes: HashMap<String, Vec<(String, usize)>>,
    impementations: HashMap<String, String>,
    methods: HashMap<String, HashMap<String, String>>,

    prev: Option<Box<Self>>,
}

pub struct Compiler {
    namespace: String,
    bytecode: Vec<Byte>,

    aliases: HashMap<String, String>,
    functions: HashMap<String, usize>,
    native: HashMap<String, usize>,
    // --
    messages: Vec<Message>,
    context: Context,
    // HM Type Checker
    hm_typechecker: HmTypeChecker,
}

impl Default for Compiler {
    fn default() -> Self {
        let mut bytecode = Vec::with_capacity(1024);
        bytecode.append(&mut vec![
            Byte::new(Instruction::CALL),
            Byte::new(Instruction::JMP).with_operand_u32(u32::MAX),
            Byte::new(Instruction::HALT),
        ]);

        Self {
            namespace: String::default(),
            bytecode,
            aliases: HashMap::default(),
            functions: HashMap::with_capacity(32),
            native: HashMap::default(),
            messages: Vec::default(),
            context: Context::default(),
            hm_typechecker: HmTypeChecker::new(),
        }
    }
}

impl<'ctx> Context {
    fn child(&self) -> Self {
        Self {
            current: self.current.clone(),
            impementations: self.impementations.clone(),
            methods: self.methods.clone(),
            defers: Vec::default(),
            constants: self.constants.clone(),
            assignments: self.assignments.clone(),
            variables: self.variables.clone(),
            symbols: self.symbols.clone(),
            classes: self.classes.clone(),
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

    pub fn get_messages(&self) -> &Vec<Message> {
        &self.messages
    }

    pub fn register(&mut self, name: &str, params: &[Type], returns: Type) -> &mut Self {
        let idx = self.native.len();
        self.native.insert(name.to_string(), idx);
        // HM typechecker handles function registration
        let _ = params;
        let _ = returns;
        self
    }

    fn resolve_variable<'compiler>(
        &self,
        variable: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> String {
        match variable.1.borrow() {
            Expression::Identifier(n) => n.to_string(),
            f => {
                eprintln!("{}", f);
                todo!("Function name as expression")
            }
        }
    }

    fn typecheck<'check>(&mut self, ast: &(SimpleSpan, Box<Expression<'check>>)) -> Type {
        // Reset HM typechecker before each typecheck
        self.hm_typechecker.reset();
        
        // Use the HM typechecker for type inference
        match self.hm_typechecker.check(ast) {
            Ok(ty) => ty,
            Err(errors) => {
                // Report type errors
                for error in errors {
                    let message = Message::error(error, ast.0.clone().into_range());
                    self.messages.push(message);
                }
                Type::Void
            }
        }
    }

    fn do_compile<'compiler>(
        &mut self,
        ast: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> Vec<Byte> {
        let mut bytecode = vec![];
        let _type = self.typecheck(ast);
        let (span, child) = ast;

        match child.borrow() {
            Expression::Comment(_) => (),
            Expression::Use {
                path: p,
                name,
                alias,
            } => {
                let mut prefix = p.clone();
                if prefix.len() == 1 {
                    prefix.push("".to_string());
                }
                self.aliases.insert(
                    alias.clone().unwrap_or(name.to_string()),
                    format!("{}{}", prefix.join("::"), name),
                );
            }
            Expression::Noop(_) => (),
            Expression::Group(e) => bytecode.append(&mut self.do_compile(e)),
            Expression::Program(children) | Expression::Fragment(children) => {
                children.iter().for_each(|child| {
                    bytecode.append(&mut self.do_compile(child));
                });
            }
            Expression::Block(children) => {
                let ctx = self.context.child();
                self.context = ctx;
                children.iter().for_each(|child| {
                    bytecode.append(&mut self.do_compile(child));
                });

                self.context = *self.context.get_prev().clone().unwrap();
            }
            Expression::Function {
                name,
                args,
                returns: _returns,
                body,
            } => {
                self.functions
                    .insert(format!("{}{}", self.namespace, name), self.bytecode.len());

                let mut a = self.do_compile(args);

                self.bytecode.append(&mut a);

                let mut c = self.do_compile(body);
                self.bytecode.append(&mut c);

                self.context.defers.iter().for_each(|offset| {
                    self.bytecode
                        .push(Byte::new(Instruction::JMP).with_operand_u32(*offset as u32));
                });

                if !matches!(
                    self.bytecode.last().map(|b| b.bytecode()),
                    Some(Instruction::RETURN)
                ) {
                    self.bytecode.push(Byte::new_with_value(
                        Instruction::CONST,
                        Value::default().raw() as _,
                    ));
                    self.bytecode.push(Byte::new(Instruction::RETURN));
                }
            }
            Expression::Expr(child) | Expression::Statement(child) => {
                bytecode.append(&mut self.do_compile(child))
            }
            Expression::ExprStatement(child) => {
                bytecode.append(&mut self.do_compile(child));
                // Do not add pop if previous instruction is `DUP` since they both cancel eachother
                // out
                if !matches!(
                    bytecode.last().map(|b| b.bytecode()),
                    Some(Instruction::DUPLICATE)
                ) {
                    bytecode.push(Byte::new(Instruction::POP));
                } else {
                    // If it was supposed to add `POP` but prev is `DUP`
                    // then remove the DUP as well
                    bytecode.pop();
                }
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
                bytecode.push(Byte::new(Instruction::FORMAT).with_operand_u32(params_len as u32));
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
                bytecode.push(Byte::new(Instruction::FORMAT).with_operand_u32(params_len as u32));
            }
            Expression::Return(expr) | Expression::ImplicitReturn(expr) => {
                self.context.defers.iter().for_each(|offset| {
                    self.bytecode
                        .push(Byte::new(Instruction::CALL).with_operand_u32(0));
                    self.bytecode
                        .push(Byte::new(Instruction::JMP).with_operand_u32(*offset as u32));
                });

                // if let Expression::Identifier(name) = *expr.1.borrow() {
                //     let ty = self.typechecker.get_variable_type(&name.into());
                //     let symbol = self.context.variables.key(&name.into());
                //     // if matches!(ty, Some(Type::OBJECT(_))) || matches!(ty, Some(Type::STRING)) {
                //     //     bytecode.push(Byte::new_with_operands(
                //     //         Instruction::ACQUIRE,
                //     //         [symbol.expect("Unable to resolve unknown variable"), 0],
                //     //     ));
                //     // }
                // }

                // for variable in self.context.variables.iter() {
                //     if let (Some(symbol), Some(ty)) = (
                //         self.context.variables.key(variable),
                //         self.typechecker.get_variable_type(variable),
                //     ) && (matches!(ty, Type::OBJECT(_)) || matches!(ty, Type::STRING))
                //     {
                //         bytecode.push(Byte::new_with_operands(Instruction::RELEASE, [symbol, 0]));
                //     }
                // }

                // if matches!(expr.1.borrow(), Expression::Identifier(_)) {
                //     let symbol = self.context.variables.intern(self.resolve_variable(expr));
                // }

                bytecode.append(&mut self.do_compile(expr));
                if !matches!(child.borrow(), Expression::ImplicitReturn(_)) {
                    bytecode.push(Byte::new(Instruction::RETURN));
                }
            }
            Expression::Class(name, state) => {
                self.context.classes.insert(
                    name.to_string(),
                    state
                        .iter()
                        .enumerate()
                        .map(|(idx, v)| match v.1.borrow() {
                            Expression::Field(n, _) => (self.resolve_variable(n), idx),
                            _ => unreachable!(
                                "The should be only fields inside of a class definition"
                            ),
                        })
                        .collect::<Vec<_>>(),
                );
                self.context.symbols.intern(name.to_string());
            }
            Expression::Implementation(what, owner, methods) => {
                let namespace = self.namespace.clone();
                let functions = self.functions.clone();

                self.namespace.push_str(what);

                for func in methods {
                    self.functions.drain();
                    self.do_compile(func);

                    for (method, _) in self.functions.iter() {
                        self.context
                            .methods
                            .entry(owner.to_string())
                            .and_modify(|e| {
                                e.insert(what.to_string(), method.clone());
                            })
                            .or_insert_with(|| {
                                let mut h = HashMap::default();
                                h.insert(what.to_string(), method.clone());
                                h
                            });
                    }
                }

                self.context
                    .impementations
                    .insert(what.to_string(), owner.to_string());

                self.namespace = namespace;
                self.functions = functions;
            }
            Expression::Instantiate(class, _args) => {
                let name = self.resolve_variable(class);
                bytecode.push(
                    Byte::new(Instruction::INIT)
                        .with_operand_u32(self.context.classes[&name].len() as u32),
                );
                // bytecode.push(Byte::new(Instruction::SET).with_operand_u32(operand);
                // let s = self;
                // self.functions.get(k);

                // bytecode.push(Byte::new(Instruction::CONST).with_operand_u32(0));
            }
            Expression::Inc(var) => {
                let name = self.resolve_variable(var);
                let symbol = self.context.variables.intern(name);

                bytecode.push(Byte::new(Instruction::INC).with_operand_u32(symbol as u32));
            }
            Expression::Dec(var) => {
                let name = self.resolve_variable(var);
                let symbol = self.context.variables.intern(name);

                bytecode.push(Byte::new(Instruction::DEC).with_operand_u32(symbol as u32));
            }
            Expression::Loop { iterable, body, .. } => {
                let mut loop_ = self.do_compile(iterable);
                let exit = loop_.len();
                loop_.push(Byte::new(Instruction::JMPF));
                loop_.append(&mut self.do_compile(body));
                loop_
                    .push(Byte::new(Instruction::JMP).with_operand_u32(self.bytecode.len() as u32));

                let len = loop_.len();
                loop_[exit] = Byte::new(Instruction::JMPF)
                    .with_operand_u32((self.bytecode.len() + len) as u32);

                self.bytecode.append(&mut loop_);
            }
            Expression::Defer(child) => {
                let mut body = vec![Byte::new(Instruction::JMP).with_operand_u32(u32::MAX)];

                self.context.defers.push(self.bytecode.len() + body.len());

                body.append(&mut self.do_compile(child));
                body.push(Byte::new_with_value(
                    Instruction::CONST,
                    Value::from(0i64).raw() as _,
                ));
                body.push(Byte::new(Instruction::RETURN));

                let total_length = self.bytecode.len();
                let current_length = body.len() + bytecode.len();
                if let Some(v) = body.first_mut() {
                    *v = Byte::new(Instruction::JMP)
                        .with_operand_u32((total_length + current_length) as u32);
                }

                bytecode.append(&mut body);
            }
            Expression::Call { name, args } => {
                let identifier = self.resolve_variable(name);
                let n = self.aliases.get(&identifier).unwrap_or(&identifier);

                if let Some(offset) = self.functions.get(n).copied() {
                    if let Some(args) = args {
                        args.iter()
                            .for_each(|arg| bytecode.append(&mut self.do_compile(arg)))
                    }

                    bytecode.push(Byte::new(Instruction::CALL).with_operand_u32(
                        args.as_ref().map(|items| items.len()).unwrap_or(0) as u32,
                    ));
                    bytecode.push(Byte::new(Instruction::JMP).with_operand_u32(offset as u32));
                } else if self.native.get(n).is_some() {
                    todo!("Not implemented");
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
            Expression::Argument(_, n) => {
                let _ = self.context.variables.intern(n.to_string());
                // bytecode.push(Byte::new(Instruction::LOAD)
            }
            Expression::Identifier(n) => {
                if let Some(symbol) = self.context.variables.key(&n.to_string()) {
                    bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(symbol as u32));
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
            Expression::If(branches) => {
                let mut compiled = branches
                    .iter()
                    .map(|(_, branch)| {
                        if let Expression::Branch(condition, body) = branch.borrow() {
                            (
                                condition.as_ref().map(|c| self.do_compile(c)),
                                self.do_compile(body),
                            )
                        } else {
                            unreachable!("Unable to handle");
                        }
                    })
                    .collect::<Vec<_>>();

                let compiled_lenght = compiled
                    .iter()
                    .map(|(condition, body)| {
                        if !condition.is_none() {
                            condition.as_ref().map(|c| c.len()).unwrap_or(0) + body.len() + 2
                        } else {
                            0
                        }
                    })
                    .sum::<usize>()
                    + self.bytecode.len()
                    + bytecode.len();

                let branchless = branches.len() == 1;
                compiled.iter_mut().for_each(|(condition, body)| {
                    if let Some(condition) = condition {
                        bytecode.append(condition);
                        bytecode.push(Byte::new(Instruction::JMPF).with_operand_u32(
                            (bytecode.len()
                                + self.bytecode.len()
                                + body.len()
                                + 1
                                + ((!branchless) as usize)) as u32,
                        ));
                    }

                    if !branchless {
                        body.push(
                            Byte::new(Instruction::JMP).with_operand_u32(compiled_lenght as u32),
                        );
                    }
                    bytecode.append(body);
                });
            }
            Expression::Le(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(if self.typecheck(lhs) == Type::Float {
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
                    Byte::new(if self.typecheck(lhs) == Type::Float {
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
                    Byte::new(if self.typecheck(lhs) == Type::Float {
                        Instruction::LEQF
                    } else {
                        Instruction::LEQ
                    })
                );
            }
            Expression::Geq(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(if self.typecheck(lhs) == Type::Float {
                        Instruction::GEQF
                    } else {
                        Instruction::GEQ
                    })
                );
            }
            Expression::Eq(lhs, rhs) => {
                binary!(bytecode, self, lhs, rhs, Byte::new(Instruction::EQ));
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
                    Byte::new(if likely(self.typecheck(lhs) == Type::Float) {
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
                    Byte::new(if likely(self.typecheck(lhs) == Type::Float) {
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
                    Byte::new(if likely(self.typecheck(lhs) == Type::Float) {
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
                    Byte::new(if likely(self.typecheck(lhs) == Type::Float) {
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
                    Byte::new(if likely(self.typecheck(lhs) == Type::Float) {
                        Instruction::DIVF
                    } else {
                        Instruction::DIV
                    },)
                );
            }
            Expression::And(lhs, rhs) => {
                binary!(bytecode, self, lhs, rhs, Byte::new(Instruction::AND));
            }
            Expression::Integer(num) => bytecode.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(*num).raw() as _,
            )),
            Expression::Bool(state) => bytecode.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(*state).raw() as _,
            )),
            Expression::Float(num) => bytecode.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(*num).raw() as _,
            )),
            Expression::String(str) => {
                let escaped = str
                    .replace("\\n", "\n")
                    .replace("\\r", "\r")
                    .replace("\\t", "\t")
                    .replace("\\0", "\0");

                // if let Ok(re) = Regex::new(r"\\u(?<code>\d{1,})").map_err(|e| dbg!(e)) {
                //     while let Some(captures) = re.captures(escaped.as_str()) {
                //         let unicode = captures.name("code").unwrap().as_str();
                //
                //         escaped = escaped.replace(
                //             format!("\\u{}", unicode).as_str(),
                //             char::from_u32(
                //                 captures
                //                     .name("code")
                //                     .unwrap()
                //                     .as_str()
                //                     .parse()
                //                     .unwrap_or_default(),
                //             )
                //             .unwrap_or_default()
                //             .to_string()
                //             .as_str(),
                //         )
                //     }
                // }
                let idx = bytecode.len();

                let mut count = 0;

                escaped.chars().inspect(|_| count += 1).for_each(|ch| {
                    bytecode.push(Byte::new(Instruction::DATA).with_operand_u32(ch.into()));
                });

                bytecode.insert(
                    idx,
                    Byte::new(Instruction::STRING).with_operand_u32(count as u32),
                );
            }
            Expression::Variable(name, _ty) => {
                if unlikely(self.context.variables.contains(&name.to_string())) {
                    let mut message =
                        Message::error("Variable redeclaration".to_string(), span.into_range());
                    message.push(Label::new(
                        format!("Variable '{}' already declared", name),
                        span.into_range(),
                    ));
                    self.messages.push(message);
                }

                self.context.variables.intern(name.to_string());
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

                self.context.assignments.insert(name.clone(), true);

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

                    // let ty = self.typecheck(value);
                    let mut expr = self.do_compile(value);

                    bytecode.append(&mut expr);
                    bytecode.push(Byte::new(Instruction::STORE).with_operand_u32(symbol as u32));

                    // Do not pop if assigning to the same place
                    if self.context.variables.len() == symbol + 1 {
                        bytecode.push(Byte::new(Instruction::DUPLICATE));
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
                let mut lhs_code = self.do_compile(lhs);
                bytecode.append(&mut lhs_code);

                let mut jumps: Vec<usize> = Vec::with_capacity(children.len());
                let last_idx = children.len() - 1;

                for child in children.iter() {
                    let expr = child.1.as_ref();
                    match expr {
                        Expression::MatchArm(pattern, body) => {
                            let is_condition =
                                !matches!(pattern.1.as_ref(), Expression::Default(_));

                            if !is_condition && jumps.len() != last_idx {
                                let mut message = Message::warn(
                                    "`default` branch should be at the end of expression"
                                        .to_string(),
                                    child.0.clone().into_range(),
                                );
                                message.push(Label::new(
                                    "Code after this block is not reachable".to_string(),
                                    child.0.clone().into_range(),
                                ));
                                message.with_help(
                                    "Maybe you need to move this to the bottom of the list?"
                                        .to_string(),
                                );
                                self.messages.push(message);
                            }

                            let mut pattern_code = self.do_compile(&pattern);
                            bytecode.append(&mut pattern_code);
                            bytecode.push(Byte::new(Instruction::EQ));

                            let mut body_code = self.do_compile(&body);
                            if is_condition {
                                bytecode.push(Byte::new(Instruction::JMPF).with_operand_u32(
                                    (self.bytecode.len() + bytecode.len() + body_code.len() + 2)
                                        as u32,
                                ));
                            }
                            bytecode.append(&mut body_code);
                            jumps.push(bytecode.len());
                            bytecode.push(Byte::new(Instruction::JMP).with_operand_u32(u32::MAX));
                        }
                        _ => {
                            let mut message = Message::error(
                                "Invalid match arm".to_string(),
                                child.0.clone().into_range(),
                            );
                            message.push(Label::new(
                                "Match arm must be a case expression".to_string(),
                                child.0.clone().into_range(),
                            ));
                            self.messages.push(message);
                        }
                    }
                }

                let len = bytecode.len();
                jumps.iter().for_each(|jump| {
                    if let Some(instruction) = bytecode.get_mut(*jump) {
                        *instruction = Byte::new(Instruction::JMP)
                            .with_operand_u32((self.bytecode.len() + len) as u32);
                    }
                });
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
                eprintln!("{}", _expr);
            }
        }

        bytecode
    }

    pub fn compile<'compiler>(
        &mut self,
        module: &str,
        ast: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> Vec<Byte> {
        let ns = self.namespace.clone();
        self.namespace = module.to_string();
        let mut program = self.do_compile(ast);
        self.namespace = ns.to_string();

        // HM typechecker messages are already collected in typecheck()

        self.bytecode.append(&mut program);

        self.bytecode.clone()
    }
}
