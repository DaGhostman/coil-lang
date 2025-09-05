mod typechecker;

use std::{borrow::Borrow, collections::HashMap};

use common::{Byte, Instruction, Interner, Value, likely, unlikely};
use parser::{Expression, SimpleSpan};

use regex::Regex;
pub use typechecker::*;

macro_rules! unary {
    ($result: expr, $self: expr, $rhs: expr, $instruction: expr) => {
        $result.append(&mut $self.do_compile($rhs));

        bytecode.push($instruction);
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

#[derive(Default)]
pub struct Compiler {
    bytecode: Vec<Byte<Value>>,
    _aliases: HashMap<String, String>,
    functions: HashMap<String, usize>,
    _methods: HashMap<String, usize>,
    // defers: Vec<(usize, usize, Vec<Byte<Value>>)>,
    // variables: Interner<String>,
    // constants: HashMap<usize, bool>,
    // --
    messages: Vec<(SimpleSpan, String)>,
    context: Context,

    typechecker: Typechecker,
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

    pub fn do_compile<'compiler>(
        &mut self,
        ast: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> Vec<Byte<Value>> {
        let mut bytecode = vec![];
        let type_ = self.typechecker.check(ast).unwrap_or_default();
        let (span, child) = ast;

        match child.borrow() {
            Expression::Comment(_) => (),
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
                // let variables = self.variables.clone();
                // let constants = self.constants.clone();
                // self.variables = Interner::default();
                // self.constants.clear();

                self.functions.insert(name.to_string(), self.bytecode.len());

                for arg in args {
                    let mut a = self.do_compile(arg);
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

                self.bytecode
                    .push(Byte::new_with_value(Instruction::CONST, Value::default()));
                self.bytecode.push(Byte::new(Instruction::RETURN));

                // self.variables = variables;
                // self.constants = constants;
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
                bytecode.push(Byte::new_with_operands(Instruction::PRINT, [params_len, 0]));
            }
            Expression::Return(child) | Expression::ImplicitReturn(child) => {
                for (offset, arity, x) in self.context.defers.iter() {
                    self.bytecode.append(&mut x.clone());
                    self.bytecode.push(Byte::new_with_operands(
                        Instruction::CALL,
                        [*offset, *arity],
                    ));
                }
                bytecode.append(&mut self.do_compile(child));
                bytecode.push(Byte::new(Instruction::RETURN));
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
                let n = self.resolve_variable(name);

                let offset = self.functions[&n];
                for arg in args {
                    bytecode.append(&mut self.do_compile(arg));
                }
                bytecode.push(Byte::new_with_operands(
                    Instruction::CALL,
                    [offset, args.len()],
                ))
            }
            Expression::Identifier(n) => {
                if let Some(symbol) = self.context.variables.key(&n.to_string()) {
                    bytecode.push(Byte::new_with_operands(Instruction::LOAD, [symbol, 0]));
                } else {
                    self.messages
                        .push((*span, format!("Unknown variable '{}'", n)));
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
                    Byte::new(
                        if self.typechecker.check(lhs).unwrap_or_default() == Type::FLOAT {
                            Instruction::LEF
                        } else {
                            Instruction::LE
                        },
                    )
                );
            }
            Expression::Gt(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(
                        if self.typechecker.check(lhs).unwrap_or_default() == Type::FLOAT {
                            Instruction::GTF
                        } else {
                            Instruction::GT
                        },
                    )
                );
            }
            Expression::Leq(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(
                        if self.typechecker.check(lhs).unwrap_or_default() == Type::FLOAT {
                            Instruction::GTF
                        } else {
                            Instruction::GT
                        },
                    )
                );

                bytecode.push(Byte::new(Instruction::NOT))
            }
            Expression::Geq(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(
                        if self.typechecker.check(lhs).unwrap_or_default() == Type::FLOAT {
                            Instruction::LEF
                        } else {
                            Instruction::LE
                        },
                    )
                );

                bytecode.push(Byte::new(Instruction::NOT))
            }
            Expression::Eq(lhs, rhs) => {
                binary!(bytecode, self, lhs, rhs, Byte::new(Instruction::EQ));

                bytecode.push(Byte::new(Instruction::NOT))
            }
            Expression::Add(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(
                        if self.typechecker.check(lhs).unwrap_or_default() == Type::FLOAT {
                            Instruction::ADDF
                        } else {
                            Instruction::ADD
                        },
                    )
                );
            }
            Expression::Sub(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(
                        if self.typechecker.check(lhs).unwrap_or_default() == Type::FLOAT {
                            Instruction::SUBF
                        } else {
                            Instruction::SUB
                        },
                    )
                );
            }
            Expression::Mul(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(
                        if self.typechecker.check(lhs).unwrap_or_default() == Type::FLOAT {
                            Instruction::MULF
                        } else {
                            Instruction::MUL
                        },
                    )
                );
            }
            Expression::Div(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(
                        if self.typechecker.check(lhs).unwrap_or_default() == Type::FLOAT {
                            Instruction::DIVF
                        } else {
                            Instruction::DIV
                        },
                    )
                );
            }
            Expression::Integer(num) => bytecode.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(*num as i64),
            )),
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
                    self.messages
                        .push((*span, format!("Variable '{}' already declared", name)));
                }

                self.context.variables.intern(name);
            }
            Expression::Constant(name, _ty) => {
                let name = self.resolve_variable(name);
                if self.context.variables.contains(&name) {
                    self.messages
                        .push((*span, format!("Constant '{}' already declared", name)));
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
                            self.messages.push((
                                *span,
                                format!(
                                    "Unable to assign to an already assigned constant '{}'",
                                    name
                                ),
                            ));
                        }
                    }

                    let mut expr = self.do_compile(value);

                    bytecode.append(&mut expr);
                    bytecode.push(Byte::new_with_operands(Instruction::STORE, [symbol, 0]));
                } else {
                    self.messages.push((
                        *span,
                        format!(
                            "Unable to assign to a non-existing variable/constant '{}'",
                            name
                        ),
                    ))
                }
            }
            _expr => {
                self.messages
                    .push((*span, format!("Unable to compile expression")));
                #[cfg(debug_assertions)]
                dbg!(_expr);
            }
        }

        bytecode
    }

    pub fn compile<'compiler>(
        &mut self,
        ast: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> Result<Vec<Byte<Value>>, Vec<(SimpleSpan, String)>> {
        self.bytecode = vec![
            Byte::new_with_operands(Instruction::CALL, [usize::MAX, 0]),
            Byte::new(Instruction::HALT),
        ];
        let mut program = self.do_compile(ast);
        self.bytecode.append(&mut program);

        if let Some(v) = self.bytecode.first_mut() {
            *v = Byte::new_with_operands(Instruction::CALL, [self.functions["main"], 0]);
        }

        if self.messages.len() > 0 {
            Err(self.messages.clone())
        } else {
            Ok(self.bytecode.clone())
        }
    }
}
