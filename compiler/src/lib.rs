pub mod passes;
pub mod types;

use std::collections::HashMap;

use common::{
    error::Error,
    opcodes::{Byte, Code, Operation, IR},
    ValueKind,
};
use parser::Program;
use types::Type;

pub enum Opcode {
    PUSH,
    POP,
    IADD,
    FADD,
    CONCAT,
}

pub trait CompilationPass {
    fn compile(&mut self, code: Program<IR>) -> Result<Program<IR>, Error>;
}

#[derive(Default)]
pub struct Compiler<'compilation> {
    pipeline: Vec<&'compilation mut dyn CompilationPass>,
    functions: HashMap<usize, (Type, Vec<Type>)>,

    types: Vec<Type>,
}

impl<'compilation> Compiler<'compilation> {
    pub fn attach(&mut self, pass: &'compilation mut dyn CompilationPass) {
        self.pipeline.push(pass);
    }

    pub fn compile(&mut self, code: Program<IR>) -> Result<Program<Code>, Error> {
        let mut program = code.clone();

        for compiler in &mut self.pipeline {
            match compiler.compile(program) {
                Ok(result) => {
                    program = result;
                }
                Err(err) => {
                    return Err(err);
                }
            }
        }

        let mut bytecode = vec![];
        let mut ip = 0;
        let mut skips = 0;

        for op in program.code() {
            ip += 1;
            if skips > 0 {
                skips -= 1;
                continue;
            }

            bytecode.append(&mut match op.code() {
                Operation::Noop => continue,
                Operation::Pop => vec![Code::new(Byte::Pop)],
                Operation::Push => {
                    match op.get(0).cloned().map(|key| {
                        code.constant(key)
                            .cloned()
                            .unwrap_or_default()
                            .kind()
                            .clone()
                    }) {
                        Some(ValueKind::NONE) => self.types.push(Type::None),
                        Some(ValueKind::FLOAT(_)) => {
                            self.types.push(Type::Float);
                        }
                        Some(ValueKind::INTEGER(_)) => {
                            self.types.push(Type::Integer);
                        }
                        Some(ValueKind::STRING(_)) => {
                            self.types.push(Type::String);
                        }
                        _ => todo!("Unhandled literal"),
                    }
                    let mut code = Code::new(Byte::Push);
                    if let Some(op) = op.get(0) {
                        code.with_operands(vec![*op]);
                    }

                    vec![code]
                }
                Operation::Array => {
                    let mut code = Code::new(Byte::Array);
                    code.with_operands(vec![op.get(0).copied().unwrap_or_default()]);

                    vec![code]
                }
                Operation::Equal => {
                    let byte = match (self.types.pop(), self.types.pop()) {
                        (Some(Type::Integer), Some(Type::Integer)) => {
                            self.types.push(Type::Integer);

                            Byte::Equal
                        }
                        _ => Byte::Pop,
                    };

                    vec![Code::new(byte)]
                }
                Operation::Add => {
                    let byte = match (self.types.pop(), self.types.pop()) {
                        (Some(Type::Integer), Some(Type::Integer)) => {
                            self.types.push(Type::Integer);
                            Byte::AddInteger
                        }
                        (Some(Type::Float), Some(Type::Float)) => {
                            self.types.push(Type::Float);
                            Byte::AddFloat
                        }
                        (Some(Type::String), Some(Type::String)) => {
                            self.types.push(Type::String);

                            Byte::ConcatString
                        }
                        _ => todo!("Unable to add incompatible types"),
                    };
                    vec![Code::new(byte)]
                }
                Operation::Match => {
                    if let Some(len) = op.get(0) {
                        skips += len;
                        self.compile_match(&program.code()[ip..ip + len])
                    } else {
                        vec![]
                    }
                }
                // Operation::Check => {
                //     vec![Code::new(Byte::Scope)]
                // }
                Operation::Leave => vec![Code::new(Byte::Leave)],
                Operation::Jump => {
                    let mut b = Code::new(Byte::Jump);
                    b.with_operands(op.operands().to_vec());

                    vec![b]
                }
                Operation::ConditionJump => {
                    let mut b = Code::new(Byte::Jumpz);
                    b.with_operands(op.operands().to_vec());

                    vec![b]
                }
                Operation::Load => vec![Code::new(Byte::Load)],
                Operation::Store => {
                    let mut code = Code::new(Byte::Store);
                    code.with_operands(op.operands().to_vec());

                    vec![code]
                }
                Operation::Print => {
                    let mut code = Code::new(Byte::Print);
                    code.with_operands(vec![op.get(0).is_some().into()]);

                    vec![code]
                }
                Operation::Range => vec![Code::new(Byte::Range)],
                Operation::Invoke => vec![Code::new(Byte::Call)],
                _ => todo!("Unable to compile {:?}", op.code()),
            });
        }

        Ok(Program::new(
            bytecode,
            program.constants(),
            code.strings(),
            code.symbols(),
        ))
    }

    fn compile_match(&mut self, code: &[IR]) -> Vec<Code> {
        let mut tokens = vec![Code::new(Byte::Halt)];
        let mut ip = 0;

        while let Some(op) = code.get(ip) {
            if op.code() != Operation::Check {
                unreachable!("Malformed match expression");
            }

            dbg!(op.operands());

            ip += 1;
        }

        tokens
    }
}
