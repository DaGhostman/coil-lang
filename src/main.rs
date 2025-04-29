use std::path::Path;

use clap::{ArgAction, Parser as Clap, ValueHint};
use common::program::data::Data;
use compiler::{
    Compiler,
    passes::{ConstantFolding, DCE, LabelUnrolling, RedundancyRemoval},
};
use machine::{Machine, MachineOptions};
use parser::Parser;

#[derive(Clap)]
struct Options {
    #[arg(short, long, value_hint = ValueHint::FilePath)]
    /// Path to a configuration file to use to override the defaults
    config: Option<String>,

    #[arg(short, long, action = ArgAction::SetTrue)]
    /// Instruct the program to not print any output
    quiet: bool,

    #[arg(short, long, action = ArgAction::SetTrue)]
    optimize: bool,

    #[arg(long)]
    cwd: Option<String>,

    #[clap(value_name="FILE", value_hint = ValueHint::FilePath)]
    file: String,
}

fn main() {
    // match DynamicLibrary::load("./examples/dynamic_library.so") {
    //     Ok(mut lib) => {
    //         lib.add_function(0, "fib".to_owned())
    //             .returns(Type::String)
    //             .add_argument(Type::Integer)
    //             .returns(Type::Integer);
    //
    //         let mut data = Data::default();
    //
    //         if let Ok(value) = lib.call(0, &[Value::from(38)], &mut data) {
    //             println!("FIB: {}", value.kind());
    //         }
    //     }
    //     Err(e) => {
    //         eprintln!("Unable to load: {}", e);
    //     }
    // }

    let args = Options::parse();

    let mut options = MachineOptions::default();

    // if let Some(location) = args.config {
    //     if let Ok(mut file) = File::open(location) {
    //         let mut buff = vec![];
    //         if let Ok(len) = file.read_to_end(&mut buff) {}
    //     }
    // }

    if let Some(cwd) = args.cwd {
        let directory = Path::new(&cwd);
        if let Err(err) = std::env::set_current_dir(directory) {
            eprintln!("Unable to use '{cwd}' as working directory: {err}");

            return;
        }
    }

    options.set_quiet(args.quiet);

    let mut data = Data::default();

    if let Ok(program) = Parser::new(args.file, &mut data).parse() {
        let mut compiler = Compiler::new(data.clone());

        let mut constant_folder = ConstantFolding::default();
        let mut label_conversion = LabelUnrolling::default();
        let mut redundancy_removal = RedundancyRemoval::default();
        let mut dce = DCE::default();

        // if args.optimize {
        compiler.attach(&mut constant_folder);
        // }

        compiler.attach(&mut redundancy_removal);
        compiler.attach(&mut dce);
        compiler.attach(&mut label_conversion);

        match compiler.compile(&program) {
            Ok((opcodes, data)) => {
                Machine::with_options(options).run(&opcodes, &data);
            }
            Err(e) => {
                dbg!(e);
            }
        }
    }
}
