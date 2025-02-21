pub mod passes;

use common::program::Program;
use common::{
    error::Error,
    opcodes::{Byte, Code, Operation, IR},
};

pub trait CompilationPass {
    fn compile<'compilation>(
        &mut self,
        code: &'compilation mut Program<IR>,
    ) -> Result<&'compilation mut Program<IR>, Error>;
}

#[derive(Default)]
pub struct Compiler<'compilation> {
    pipeline: Vec<&'compilation mut dyn CompilationPass>,
}

impl<'compilation> Compiler<'compilation> {
    pub fn attach(&mut self, pass: &'compilation mut dyn CompilationPass) {
        self.pipeline.push(pass);
    }

    pub fn compile(&mut self, code: &mut Program<IR>) -> Result<Program<Code>, Error> {
        let mut program = Program::new(vec![]);
        program.with_constants(code.get_constants());

        let mut intermediary = code;

        for compiler in &mut self.pipeline {
            match compiler.compile(intermediary) {
                Ok(result) => {
                    program.with_constants(result.get_constants());
                    intermediary = result;
                }
                Err(err) => {
                    return Err(err);
                }
            }
        }

        let mut bytecode = vec![];
        let mut skips = 0;

        for op in intermediary.code() {
            if skips > 0 {
                skips -= 1;
                continue;
            }

            bytecode.append(&mut match op.code() {
                Operation::Noop => continue,
                Operation::Pop => vec![Code::new(Byte::Pop)],
                Operation::Const => {
                    vec![Code::new_with_operands(Byte::Push, op.operands().to_vec())]
                }
                Operation::Add => vec![Code::new(Byte::Add)],
                Operation::Print => vec![Code::new(Byte::Print)],
                _ => todo!("Unable to compile {:?}", op.code()),
            });
        }

        program.with_code(bytecode);

        Ok(program)
    }
}
