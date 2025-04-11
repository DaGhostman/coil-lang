use std::path::Path;

use clap::{ArgAction, Parser as Clap, ValueHint};
use compiler::{
    Compiler,
    passes::{
        constant_folding::ConstantFolding, jump_translation::LabelUnrolling,
        typechecker::TypeChecker,
    },
};
use machine::{options::MachineOptions, stack::Machine};
use parser::Parser;
use scanner::{Scanner, buffer::Buffer};

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

    if let Ok(buffer) = Buffer::new(&args.file) {
        let mut scanner = Scanner::new(buffer, Some(args.file));
        let mut compiler = Compiler::default();

        let mut typechecker = TypeChecker::default();
        let mut constant_folder = ConstantFolding::default();
        let mut label_conversion = LabelUnrolling::default();

        compiler.attach(&mut typechecker);

        if args.optimize {
            compiler.attach(&mut constant_folder);
        }

        compiler.attach(&mut label_conversion);

        if let Ok(program) = Parser::default().parse(&mut scanner) {
            match compiler.compile(&program) {
                Ok(opcodes) => {
                    Machine::with_options(options).run(&opcodes);
                }
                Err(e) => {
                    dbg!(e);
                }
            }
        }
    } else {
        eprintln!("Missing file");
    }
}
