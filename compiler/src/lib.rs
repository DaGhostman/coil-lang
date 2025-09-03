use std::{borrow::Borrow, collections::HashMap};

use common::{Byte, Instruction, Interner, Value, likely, unlikely};
use parser::{Expression, SimpleSpan};

#[derive(Default)]
pub struct Compiler {
    bytecode: Vec<Byte<Value>>,
    aliases: HashMap<String, String>,
    functions: HashMap<String, usize>,
    methods: HashMap<String, usize>,
    defers: Vec<(usize, usize, Vec<Byte<Value>>)>,
    variables: Interner<String>,
    constants: HashMap<usize, bool>,
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

        match ast {
            (span, child) => match child.borrow() {
                Expression::Comment(_) => (),
                Expression::Program(children) | Expression::Fragment(children) => {
                    for child in children {
                        bytecode.append(&mut self.do_compile(child));
                    }
                }
                Expression::Block(children) => {
                    let defers = self.defers.clone();
                    self.defers.clear();
                    for child in children {
                        bytecode.append(&mut self.do_compile(child));
                    }
                    self.defers = defers;
                }
                Expression::Function {
                    name,
                    args,
                    returns: _returns,
                    body,
                } => {
                    let variables = self.variables.clone();
                    let constants = self.constants.clone();
                    self.variables = Interner::default();
                    self.constants.clear();

                    self.functions.insert(name.to_string(), self.bytecode.len());

                    for arg in args {
                        let mut a = self.do_compile(&arg);
                        self.bytecode.append(&mut a);
                    }

                    for child in body {
                        let mut c = self.do_compile(&child);
                        self.bytecode.append(&mut c);
                    }

                    for (offset, arity, x) in self.defers.iter() {
                        self.bytecode.append(&mut x.clone());
                        self.bytecode.push(Byte::new_with_operands(
                            Instruction::CALL,
                            [*offset, *arity],
                        ));
                    }

                    self.bytecode
                        .push(Byte::new_with_value(Instruction::CONST, Value::default()));
                    self.bytecode.push(Byte::new(Instruction::RETURN));

                    self.variables = variables;
                    self.constants = constants;
                }
                Expression::Expr(child) | Expression::Statement(child) => {
                    bytecode.append(&mut self.do_compile(child))
                }
                Expression::ExprStatement(child) => {
                    bytecode.append(&mut self.do_compile(child));
                    bytecode.push(Byte::new(Instruction::POP));
                }
                Expression::Print(child) => {
                    bytecode.append(&mut self.do_compile(child));
                    bytecode.push(Byte::new(Instruction::PRINTI));
                }
                Expression::Return(child) | Expression::ImplicitReturn(child) => {
                    for (offset, arity, x) in self.defers.iter() {
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

                    self.defers
                        .push((self.bytecode.len() + body.len(), arity, args));

                    body.append(&mut self.do_compile(child));
                    body.push(Byte::new_with_value(Instruction::CONST, Value::from(0)));
                    body.push(Byte::new(Instruction::RETURN));

                    let total_length = self.bytecode.len();
                    let current_length = body.len() + bytecode.len();
                    body.first_mut().map(|v| {
                        *v = Byte::new_with_operands(
                            Instruction::JMP,
                            [total_length + current_length, 0],
                        );
                    });

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
                    if let Some(symbol) = self.variables.key(&n.to_string()) {
                        bytecode.push(Byte::new_with_operands(Instruction::LOAD, [symbol, 0]));
                    } else {
                        panic!("Unknown variable");
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
                        .map(|v| self.do_compile(&v))
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
                    bytecode.append(&mut self.do_compile(lhs));
                    bytecode.append(&mut self.do_compile(rhs));
                    bytecode.push(Byte::new(Instruction::LE));
                }
                Expression::Add(lhs, rhs) => {
                    bytecode.append(&mut self.do_compile(lhs));
                    bytecode.append(&mut self.do_compile(rhs));
                    bytecode.push(Byte::new(Instruction::ADD));
                }
                Expression::Sub(lhs, rhs) => {
                    bytecode.append(&mut self.do_compile(lhs));
                    bytecode.append(&mut self.do_compile(rhs));
                    bytecode.push(Byte::new(Instruction::SUB));
                }
                Expression::Number(num) => bytecode.push(Byte::new_with_value(
                    Instruction::CONST,
                    Value::from(*num as i64),
                )),
                Expression::Variable(name, _ty) => {
                    let name = self.resolve_variable(name);

                    if unlikely(self.variables.contains(&name)) {
                        panic!("Variable '{}' already declared", name);
                    }

                    self.variables.intern(name);
                }
                Expression::Constant(name, _ty) => {
                    let name = self.resolve_variable(name);
                    if self.variables.contains(&name) {
                        panic!("Constant '{}' already declared", name);
                    }

                    let symbol = self.variables.intern(name.clone());

                    self.constants.insert(symbol, false);
                }
                Expression::Assignment(name, value) => {
                    let name = self.resolve_variable(name);

                    if let Some(symbol) = self.variables.key(&name) {
                        if unlikely(self.constants.contains_key(&symbol)) {
                            let assigned = likely(*self.constants.get(&symbol).unwrap());

                            if !assigned {
                                self.constants.entry(symbol).and_modify(|state| {
                                    *state = true;
                                });
                            } else {
                                panic!(
                                    "Unable to assign to constant {}",
                                    self.variables.resolve(symbol)
                                );
                            }
                        }

                        let mut expr = self.do_compile(value);

                        bytecode.append(&mut expr);
                        bytecode.push(Byte::new_with_operands(Instruction::STORE, [symbol, 0]));
                    } else {
                        panic!("Unable to assign to non-existing variable/constant");
                    }
                }
                expr => {
                    panic!("Unknown expression {:?}", expr);
                }
            },
        }

        bytecode
    }

    pub fn compile<'compiler>(
        &mut self,
        ast: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> Vec<Byte<Value>> {
        self.bytecode = vec![
            Byte::new_with_operands(Instruction::CALL, [usize::MAX, 0]),
            Byte::new(Instruction::HALT),
        ];
        let mut program = self.do_compile(ast);
        self.bytecode.append(&mut program);

        self.bytecode.first_mut().map(|v| {
            *v = Byte::new_with_operands(Instruction::CALL, [self.functions["main"], 0]);
        });

        self.bytecode.clone()
    }
}
