use crate::CompilationPass;
use common::{Value, opcodes::Byte};
use rustc_hash::FxHashMap as HashMap;

pub struct FunctionInline {
    _functions: HashMap<usize, bool>,
}

impl CompilationPass for FunctionInline {
    fn compile(
        &mut self,
        code: &[common::opcodes::Code],
        data: &mut common::program::data::Data,
    ) -> Result<Vec<common::opcodes::Code>, Vec<common::error::Message>> {
        for item in code {
            if item.byte() == &Byte::Push {
                if let Value::FUNCTION(_arity, _label) = data.constant(item.operand(0)) {
                    // Determine if a function is a good candidate for inlining
                }
            }
        }

        Ok(code.to_vec())
    }
}
