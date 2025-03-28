use common::program::data::Data;

use crate::CompilationPass;

pub struct LabelUnrolling {}

impl CompilationPass for LabelUnrolling {
    fn compile<'compilation>(
        &mut self,
        code: &'compilation [common::opcodes::Code],
        data: &mut Data,
    ) -> Result<Vec<common::opcodes::Code>, common::error::Error> {
        todo!()
    }
}
