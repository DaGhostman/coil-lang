use std::{borrow::Borrow, collections::HashMap};

use common::{Byte, Instruction, Value};
use parser::{Expression, SimpleSpan};

#[derive(Default)]
pub struct Compiler {
    bytecode: Vec<Byte<Value>>,
    aliases: HashMap<String, String>,
    functions: HashMap<String, usize>,
    methods: HashMap<String, usize>,
    defers: Vec<(usize, usize, Vec<Byte<Value>>)>,
}

impl Compiler {
    pub fn do_compile<'compiler>(
        &mut self,
        ast: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> Vec<Byte<Value>> {
        let mut bytecode = vec![];

        match ast {
            (span, child) => match child.borrow() {
                Expression::Program(children) => {
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
                    returns,
                    body,
                } => {
                    self.functions.insert(name.to_string(), self.bytecode.len());

                    for arg in args {
                        let mut a = self.do_compile(&arg.1);
                        self.bytecode.append(&mut a);
                    }

                    for child in body {
                        let mut c = self.do_compile(&child);
                        self.bytecode.append(&mut c);
                    }

                    for (offset, arity, x) in self.defers.iter() {
                        self.bytecode.append(&mut x.clone());
                        self.bytecode
                            .push(Byte::new(Instruction::CALL, [*offset, *arity]));
                    }

                    self.bytecode.push(Byte::new_with(
                        Instruction::CONST,
                        [0, 0],
                        Value::default(),
                    ));
                    self.bytecode.push(Byte::new(Instruction::RETURN, [0, 0]));
                }
                Expression::Expr(child) | Expression::Statement(child) => {
                    bytecode.append(&mut self.do_compile(child))
                }
                Expression::ExprStatement(child) => {
                    bytecode.append(&mut self.do_compile(child));
                    bytecode.push(Byte::new(Instruction::POP, [0, 0]));
                }
                Expression::Print(child) => {
                    bytecode.append(&mut self.do_compile(child));
                    bytecode.push(Byte::new(Instruction::PRINTI, [0, 0]));
                }
                Expression::Return(child) | Expression::ImplicitReturn(child) => {
                    for (offset, arity, x) in self.defers.iter() {
                        self.bytecode.append(&mut x.clone());
                        self.bytecode
                            .push(Byte::new(Instruction::CALL, [*offset, *arity]));
                    }
                    bytecode.append(&mut self.do_compile(child));
                    bytecode.push(Byte::new(Instruction::RETURN, [0, 0]));
                }
                Expression::Yield(child) => {
                    bytecode.append(&mut self.do_compile(child));
                    bytecode.push(Byte::new(Instruction::SUSP, [0, 0]));
                }
                Expression::Defer(deps, child) => {
                    let mut body = vec![Byte::new(Instruction::JMP, [usize::MAX, 0])];
                    let mut args = vec![];
                    let mut arity = 0;
                    for arg in deps.iter() {
                        args.append(&mut self.do_compile(arg));
                        arity += 1;
                    }

                    self.defers
                        .push((self.bytecode.len() + body.len(), arity, args));

                    body.append(&mut self.do_compile(child));
                    body.push(Byte::new_with(Instruction::CONST, [0, 0], Value::from(0)));
                    body.push(Byte::new(Instruction::RETURN, [0, 0]));

                    let total_length = self.bytecode.len();
                    let current_length = body.len() + bytecode.len();
                    body.first_mut().map(|v| {
                        *v = Byte::new(Instruction::JMP, [total_length + current_length, 0]);
                    });

                    bytecode.append(&mut body);
                }
                Expression::Call { name, args } => {
                    let n = match name.1.borrow() {
                        Expression::Identifier(n) => n.to_string(),
                        f => {
                            dbg!(&f);
                            todo!("Function name as expression")
                        }
                    };

                    let offset = self.functions[&n];
                    for arg in args {
                        bytecode.append(&mut self.do_compile(arg));
                    }
                    bytecode.push(Byte::new(Instruction::CALL, [offset, args.len()]))
                }
                Expression::Identifier(n) => {
                    // TODO: Handle argument internment
                    bytecode.push(Byte::new(Instruction::LOAD, [0, 0]));
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

                    bytecode.push(Byte::new(
                        Instruction::JMPF,
                        [self.bytecode.len() + current_len + 1, 0],
                    ));
                    bytecode.append(&mut body);
                    bytecode.append(&mut alternative);
                }
                Expression::Le(lhs, rhs) => {
                    bytecode.append(&mut self.do_compile(lhs));
                    bytecode.append(&mut self.do_compile(rhs));
                    bytecode.push(Byte::new(Instruction::LE, [0, 0]));
                }
                Expression::Add(lhs, rhs) => {
                    bytecode.append(&mut self.do_compile(lhs));
                    bytecode.append(&mut self.do_compile(rhs));
                    bytecode.push(Byte::new(Instruction::ADD, [0, 0]));
                }
                Expression::Sub(lhs, rhs) => {
                    bytecode.append(&mut self.do_compile(lhs));
                    bytecode.append(&mut self.do_compile(rhs));
                    bytecode.push(Byte::new(Instruction::SUB, [0, 0]));
                }
                Expression::Number(num) => bytecode.push(Byte::new_with(
                    Instruction::CONST,
                    [0, 0],
                    Value::from(*num as i64),
                )),
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
            Byte::new(Instruction::CALL, [usize::MAX, 0]),
            Byte::new(Instruction::HALT, [0, 0]),
        ];
        let mut program = self.do_compile(ast);
        self.bytecode.append(&mut program);

        self.bytecode.first_mut().map(|v| {
            *v = Byte::new(Instruction::CALL, [self.functions["main"], 0]);
        });

        self.bytecode.clone()
    }
}
