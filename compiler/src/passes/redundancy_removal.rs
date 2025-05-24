use common::opcodes::{Byte, Code};

use crate::CompilationPass;

#[derive(Default)]
pub struct RedundancyRemoval {}

impl CompilationPass for RedundancyRemoval {
    fn compile(
        &mut self,
        code: &[common::opcodes::Code],
        _: &mut common::program::data::Data,
    ) -> Result<Vec<common::opcodes::Code>, Vec<common::error::Message>> {
        let mut new_code = Vec::with_capacity(code.len());
        let mut ip = 0;

        // HashMap<label, (position, length)>
        // let labels: HashMap<usize, (usize, usize)> = Default::default();

        // for op in code {
        //     if *op.byte() == Byte::Label {
        //         labels.insert(op.operand(0), ());
        //     }
        // }
        let length = code.len();

        while ip < length {
            match code[ip].byte() {
                Byte::Pop => {
                    if *code[ip + 1].byte() == Byte::Pop {
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
