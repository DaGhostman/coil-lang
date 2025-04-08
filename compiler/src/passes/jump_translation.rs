use common::{
    Value,
    opcodes::{Byte, Code},
    program::{data::Data, program::Program},
    vec_array::VecArray,
};

use crate::CompilationPass;

#[derive(Default)]
pub struct LabelUnrolling {}

impl CompilationPass for LabelUnrolling {
    fn compile(
        &mut self,
        code: &[common::opcodes::Code],
        data: &mut Data,
    ) -> Result<Vec<common::opcodes::Code>, common::error::Error> {
        let mut bytecode = code.to_owned();
        let mut labels: VecArray<usize, 64> = VecArray::default();
        for (idx, label) in code.iter().enumerate().filter_map(|(idx, code)| {
            if Byte::Label == *code.byte() {
                Some((idx, code.operand(0)))
            } else {
                None
            }
        }) {
            labels.insert(label, idx);
        }

        for bytecode in bytecode.iter_mut() {
            *bytecode = match bytecode.byte() {
                Byte::Jump => {
                    Code::new_with_operands(Byte::Jump, [labels.get(bytecode.operand(0)) + 1, 0, 0])
                }
                Byte::Jumpz => Code::new_with_operands(
                    Byte::Jumpz,
                    [
                        labels.get(bytecode.operand(0)) + 1,
                        labels.get(bytecode.operand(1)) + 1,
                        0,
                    ],
                ),
                Byte::Label => Code::new_with_operands(Byte::Jumpr, [bytecode.operand(1), 0, 0]),
                Byte::Push => {
                    if let Value::FUNCTION(arity, label) = data.constant(bytecode.operand(0)) {
                        data.replace_constant(
                            bytecode.operand(0),
                            Value::FUNCTION(*arity, labels.get(*label) + 1),
                        );
                    }

                    *bytecode
                }
                _ => *bytecode,
            }
        }

        Ok(bytecode.to_vec())
        // todo!()
    }
}
