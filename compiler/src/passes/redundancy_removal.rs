use common::{
    opcodes::{Byte, Code},
    program::program::Program,
};

use crate::CompilationPass;

#[derive(Default)]
pub struct RedundancyRemoval {}

impl CompilationPass for RedundancyRemoval {
    fn compile(
        &mut self,
        code: &[common::opcodes::Code],
        _: &mut common::program::data::Data,
    ) -> Result<Vec<common::opcodes::Code>, common::error::Error> {
        let mut new_code = Vec::with_capacity(code.len());
        let mut ip = 0;
        while ip < code.len() {
            match code[ip].byte() {
                Byte::Pop => {
                    if code[ip].operand(0) == 0 {
                        ();
                    } else if *code[ip + 1].byte() == Byte::Pop {
                        new_code.push(Code::new_with_operands(
                            Byte::Pop,
                            [code[ip].operand(0) + code[ip + 1].operand(0), 0, 0],
                        ));
                        ip += 1;
                    } else {
                        new_code.push(code[ip]);
                    }
                }
                _ => new_code.push(code[ip]),
            }

            ip += 1;
        }
        Ok(new_code)
    }
}
