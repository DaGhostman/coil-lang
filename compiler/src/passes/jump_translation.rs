use common::{
    Value,
    opcodes::{Byte, Code},
    program::data::Data,
    vec_array::VecArray,
};

use crate::CompilationPass;

#[derive(Default)]
pub struct LabelUnrolling {
    jumps: VecArray<usize, 8>,
}

impl LabelUnrolling {
    fn jump_at(&mut self, label: usize) -> usize {
        self.jumps.get(label).checked_sub(1).or(Some(0)).unwrap()
    }

    fn reduce_jumps(&mut self, index: usize) {
        let offset = self.jumps[index];
        for (l, position) in self.jumps.clone().into_iter().enumerate() {
            if offset <= position {
                self.jumps.insert(l, position - 1);
            }
        }
    }

    fn insert(&mut self, label: usize, index: usize) {
        self.jumps.insert(label, index);
    }
}

impl CompilationPass for LabelUnrolling {
    fn compile(
        &mut self,
        code: &[common::opcodes::Code],
        data: &mut Data,
    ) -> Result<Vec<common::opcodes::Code>, Vec<common::error::Message>> {
        let labels: Vec<(usize, usize)> = code
            .iter()
            .enumerate()
            .filter_map(|(idx, code)| {
                if Byte::Label == *code.byte() {
                    Some((idx + 1, code.operand(0)))
                } else {
                    None
                }
            })
            .collect();

        for (idx, label) in &labels {
            self.insert(*label, *idx);
        }

        for (_, label) in &labels {
            self.reduce_jumps(*label);
        }

        let mut funcs = rustc_hash::FxHashSet::default();

        let mut bytecode = Vec::with_capacity(code.len());
        for code in code {
            bytecode.push(match code.byte() {
                Byte::Jumpz | Byte::Jump => {
                    Code::new_with_operands(*code.byte(), [(self.jump_at(code.operand(0))), 0, 0])
                }
                Byte::Label => {
                    continue;
                }
                Byte::Iterate => Code::new_with_operands(
                    Byte::Iterate,
                    [
                        self.jump_at(code.operand(0)),
                        code.operand(1),
                        code.operand(2),
                    ],
                ),
                Byte::Method => Code::new_with_operands(
                    Byte::Method,
                    [
                        code.operand(0),
                        code.operand(1),
                        self.jump_at(code.operand(2)),
                    ],
                ),
                Byte::Invoke => Code::new_with_operands(
                    Byte::Invoke,
                    [self.jump_at(code.operand(0)), code.operand(1), 0],
                ),
                Byte::Push => {
                    if let Value::FUNCTION(arity, label) = data.constant(code.operand(0)) {
                        if !funcs.contains(&code.operand(0)) {
                            data.replace_constant(
                                code.operand(0),
                                Value::FUNCTION(*arity, self.jump_at(*label)),
                                code.get_type(),
                            );
                            funcs.insert(code.operand(0));
                        }
                    }

                    *code
                }
                _ => *code,
            });
        }

        Ok(bytecode)
    }
}
