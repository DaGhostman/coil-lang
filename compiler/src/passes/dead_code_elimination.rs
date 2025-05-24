use common::{
    Value,
    error::Message,
    opcodes::{Byte, Code},
    program::data::Data,
    types::Type,
};
use rustc_hash::FxHashMap as HashMap;

use crate::CompilationPass;

#[derive(Default)]
pub struct DCE {}

impl CompilationPass for DCE {
    fn compile(&mut self, code: &[Code], data: &mut Data) -> Result<Vec<Code>, Vec<Message>> {
        let mut modified = Vec::with_capacity(code.len());

        let mut cursor = 0;
        let length = code.len();

        let mut labels: HashMap<usize, usize> = Default::default();
        for (idx, c) in code.iter().enumerate() {
            if *c.byte() == Byte::Label {
                labels.insert(c.operand(0), idx);
            }
        }

        while cursor < length {
            let op = code[cursor];
            cursor += 1;

            match op.byte() {
                Byte::Pop => {
                    if op.operand(0) > 0 {
                        modified.push(op);
                    }
                }
                Byte::Leave => {
                    modified.push(op);
                    let void = data.add_constant(common::Value::NONE, data.find_type(Type::void()));
                    if cursor + 1 < length {
                        let (constant, bytecode) = (code[cursor], code[cursor + 1]);
                        if constant.operand(0) == void && *bytecode.byte() == Byte::Leave {
                            cursor += 2;
                        }
                    }
                }
                Byte::Jump => {
                    if op.operand(1) == 1 {
                        let rhs = modified[modified.len() - 1];

                        if *rhs.byte() == Byte::Push {
                            let constant = data.constant(rhs.operand(0));

                            if let Value::BOOLEAN(state) = constant {
                                // Remove the condition
                                modified.pop();

                                // TODO: Need to handle elimination of both branches and not just the
                                // successful one
                                //
                                // if *constant == Value::BOOLEAN(true) {
                                //     // This condition will always be true, so the else branch needs be
                                //     // eliminated
                                // } else
                                if *state {
                                    modified.push(op);
                                } else {
                                    // This condition will always be false, so the then branch needs be
                                    // eliminated, i.e which is easy since it needs to just compile
                                    // the else part. BUT the handling for `if true` is more
                                    // complicated as it could be just an `else` or alternatively
                                    // `else-if` which will have it's own condition and we
                                    // shouldn't actually skip that, because it will be handled by
                                    // the continuation of this handling.
                                    cursor += labels[&op.operand(0)] - cursor;
                                }
                            } else {
                                modified.push(op);
                            }
                        } else {
                            modified.push(op);
                        }
                    } else {
                        modified.push(op);
                    }
                }
                Byte::Push => {
                    if let Value::NONE = data.constant(op.operand(0)) {
                        if *modified[modified.len() - 1].byte() == Byte::Leave
                            && *code[cursor].byte() == Byte::Leave
                        {
                            cursor += 2;
                        } else {
                            modified.push(op);
                        }
                    } else {
                        modified.push(op);
                    }
                }
                _ => {
                    modified.push(op);
                }
            }
        }

        Ok(modified)
    }
}
