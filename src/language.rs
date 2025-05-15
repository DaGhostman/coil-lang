use common::{Value, program::data::Data};
use compiler::{
    Compiler,
    passes::{ConstantFolding, DCE, LabelUnrolling, RedundancyRemoval},
};
use machine::{Machine, MachineOptions, NativeLibrary};
use parser::Parser;
use std::collections::HashMap;

#[derive(Default)]
pub struct Language<'lang> {
    data: Data,
    functions: HashMap<usize, (&'lang dyn NativeLibrary, usize)>,
}

impl<'lang> Language<'lang> {
    pub fn load(&mut self, library: &'lang dyn NativeLibrary) {
        for (name, r#type) in library.get_functions(&mut self.data) {
            let symbol = self.data.add_symbol(name.to_string(), None);
            let ty = self.data.add_type(r#type);

            let constant = self.data.add_constant(Value::EXTERNAL(symbol), ty);
            self.data.add_symbol(name.to_string(), Some(constant));
            self.functions.insert(symbol, (library, ty));
        }
    }

    pub fn run(&mut self, file: String) {
        let mut parser = Parser::new(file, &mut self.data);
        for (symbol, (_, ty)) in &self.functions {
            parser.register(*symbol, *ty);
        }

        if let Ok(program) = parser.parse() {
            let mut compiler = Compiler::new(self.data.clone());
            let mut machine = Machine::with_options(MachineOptions::default());

            for (symbol, (func, ty)) in &self.functions {
                compiler.register_function(*symbol, *ty);
                machine.register(*symbol, *func);
            }

            let mut constant_folder = ConstantFolding::default();
            let mut label_conversion = LabelUnrolling::default();
            let mut redundancy_removal = RedundancyRemoval::default();
            let mut dce = DCE::default();

            compiler.attach(&mut constant_folder);
            compiler.attach(&mut redundancy_removal);
            compiler.attach(&mut dce);

            compiler.attach(&mut label_conversion);

            if let Ok((opcodes, data)) = compiler.compile(&program) {
                machine.run(&opcodes, &data);
            }
        } else {
            eprintln!("Parse error");
        }
    }
}
