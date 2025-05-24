use common::{
    Value,
    native::{Library, Native},
    opcodes::{Code, IR},
    program::{data::Data, program::Program},
};
use compiler::{
    Compiler,
    passes::{ConstantFolding, DCE, LabelUnrolling, RedundancyRemoval},
};
use machine::{Machine, MachineOptions};
use parser::Parser;
use std::{
    fs::{File, OpenOptions},
    io::{BufReader, Write},
    path::{Path, PathBuf},
    process::exit,
};

#[derive(Default)]
pub struct Language {
    data: Data,
    functions: Vec<Native>,
}

impl Language {
    pub fn load(&mut self, library: &dyn Library) {
        for func in library.get_functions(&mut self.data) {
            let ty = self.data.add_type(func.get_type());

            let constant = self.data.add_constant(Value::NATIVE(func.get_name()), ty);
            let name = func.name(&self.data);
            self.data.add_symbol(name, Some(constant));
            self.functions.push(func);
        }
    }

    pub fn check(&mut self, file: &str) -> Option<Program<IR>> {
        let mut parser = Parser::new(file.to_string(), &mut self.data);
        for func in &self.functions {
            parser.register(func.get_name(), func.get_type());
        }

        match parser.parse() {
            Ok(program) => Some(program),
            Err(e) => {
                for msg in parser.get_messages() {
                    eprintln!("{msg}");
                }
                eprintln!("\n{e}");
                None
            }
        }
    }

    pub fn compile(&mut self, file: &str, output: Option<String>) -> Option<Program<Code>> {
        match self.check(file) {
            Some(code) => {
                let mut compiler = Compiler::new(&mut self.data);
                for func in &self.functions {
                    compiler.register_function(*func);
                }

                let mut constant_folder = ConstantFolding::default();
                let mut label_conversion = LabelUnrolling::default();
                let mut redundancy_removal = RedundancyRemoval::default();
                let mut dce = DCE::default();

                compiler.attach(&mut constant_folder);
                compiler.attach(&mut redundancy_removal);
                compiler.attach(&mut dce);

                compiler.attach(&mut label_conversion);

                if let Ok((opcodes, data)) = compiler.compile(&code) {
                    self.data = data.clone();
                    let output = if let Some(output) = output {
                        output
                    } else {
                        file.to_string()
                    };

                    // if let Some(output) = output {
                    let path = Path::new(&output).with_extension("c0s");

                    // if let Ok(path) = Path::new(&output)
                    // .canonicalize()
                    // .map_err(|e| eprintln!("{e}"))
                    // {
                    let mut opt = OpenOptions::new();
                    opt.create(true);
                    opt.truncate(true);
                    opt.write(true);

                    if let Ok(mut fp) = opt.open(path).map_err(|e| eprintln!("Opening: {e}")) {
                        if let Ok(vals) =
                            rmp_serde::to_vec(&(&opcodes, &data)).map_err(|e| eprintln!("{e}"))
                        {
                            if let Err(e) = fp.write_all(&mut &vals) {
                                panic!("{e}");
                            }
                        }
                    }
                    // }
                    // }
                    Some(opcodes)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn run(&mut self, file: String) {
        if let Ok(path) = PathBuf::from(&file).canonicalize() {
            let mut opt = OpenOptions::new();
            opt.read(true);
            opt.create(false);

            if let Ok(fp) = opt.open(path).map_err(|e| eprintln!("{e}")) {
                let reader = BufReader::new(fp);
                if let Ok((code, data)) =
                    rmp_serde::from_read::<BufReader<File>, (Program<Code>, Data)>(reader)
                {
                    let mut machine = Machine::with_options(MachineOptions::default());

                    for func in &self.functions {
                        machine.register(*func);
                    }
                    machine.run(&code, &data);
                    // if let Ok(vals) = serde_json::to_string(&(&opcodes, &data)) {
                    // if let Err(e) = fp.write_all(&mut vals.as_bytes()) {
                    //     panic!("{e}");
                    // }
                    // if let Err(e) = fp.write_all(vals.as_bytes()) {
                    //     eprintln!("{e}");
                    // }
                    // }
                }
            }
        }
    }

    pub fn interpret(&mut self, file: String) {
        match self.compile(&file, None) {
            Some(code) => {
                let mut machine = Machine::with_options(MachineOptions::default());
                for func in &self.functions {
                    machine.register(*func);
                }
                machine.run(&code, &self.data);
            }
            None => exit(1),
        }
        // let mut parser = Parser::new(file, &mut self.data);
        // for (symbol, (_, ty)) in &self.functions {
        //     parser.register(*symbol, *ty);
        // }
        //
        // match parser.parse() {
        //     Ok(program) => {
        //         let mut compiler = Compiler::new(self.data.clone());
        //         let mut machine = Machine::with_options(MachineOptions::default());
        //
        //         for (symbol, (func, ty)) in &self.functions {
        //             compiler.register_function(*symbol, *ty);
        //             machine.register(*symbol, *func);
        //         }
        //
        //         let mut constant_folder = ConstantFolding::default();
        //         let mut label_conversion = LabelUnrolling::default();
        //         let mut redundancy_removal = RedundancyRemoval::default();
        //         let mut dce = DCE::default();
        //
        //         compiler.attach(&mut constant_folder);
        //         compiler.attach(&mut redundancy_removal);
        //         compiler.attach(&mut dce);
        //
        //         compiler.attach(&mut label_conversion);
        //
        //         if let Ok((opcodes, data)) = compiler.compile(&program) {
        //             machine.run(&opcodes, &data);
        //         }
        //     }
        //     Err(e) => {
        //         eprintln!("{e}");
        //     }
        // }
    }
}
