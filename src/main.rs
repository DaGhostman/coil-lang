mod language;
mod stdlib;

use std::path::Path;

use clap::{ArgAction, Parser as Clap, Subcommand, ValueHint};
use language::Language;
use machine::MachineOptions;
use stdlib::{Basic, Common, Coroutine};

#[derive(Clap, Debug)]
#[command(version, about, long_about = None)]
#[command(propagate_version = true)]
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

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Check the defined program for type-errors
    Check {
        #[clap(value_name="FILE", value_hint = ValueHint::FilePath)]
        file: String,
    },
    // Lint {},
    // Debug {},
    /// Compile the current program into a binary executable
    Compile {
        /// Entrypoint source file from which the compilation is to begin
        /// scanning the source code
        #[clap(value_name="input", value_hint = ValueHint::FilePath)]
        file: String,
        /// The target file in which the binary should be output
        #[clap(value_name="output", value_hint = ValueHint::FilePath)]
        output: Option<String>,
    },
    /// Execute an already compiled binary
    Run {
        /// Execute an already compiled version of the file
        #[clap(value_name="FILE", value_hint = ValueHint::FilePath)]
        file: String,
    },
    /// Check, Compile & Execute the provided file, without leaving any artifacts behind
    Interpret {
        #[clap(value_name="FILE", value_hint = ValueHint::FilePath)]
        file: String,
    },
    #[command(external_subcommand)]
    FILE(Vec<String>),
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

    if let Some(cwd) = &args.cwd {
        let directory = Path::new(&cwd);
        if let Err(err) = std::env::set_current_dir(directory) {
            eprintln!("Unable to use '{cwd}' as working directory: {err}");

            return;
        }
    }

    options.set_quiet(args.quiet);
    let mut language = Language::default();
    let coroutine = Coroutine::default();
    let common = Common::default();
    let string = Basic::default();
    //
    language.load(&common);
    language.load(&coroutine);
    language.load(&string);

    match args.command {
        Some(Command::Check { file }) => {
            language.check(&file);
        }
        Some(Command::Compile { file, output, .. }) => {
            language.compile(&file, output.or(Some(file.to_string())));
        }
        Some(Command::Run { file }) => language.run(file),
        Some(Command::Interpret { file }) => language.interpret(file),
        Some(Command::FILE(files)) => {
            for file in &files {
                language.interpret(file.to_string());
            }
        }
        None => {
            panic!("{:?}", &args);
        }
    }

    // let mut data = Data::default();
    //
    // if let Ok(program) = Parser::new(args.file, &mut data).parse() {
    //     let mut compiler = Compiler::new(data.clone());
    //
    //     let mut constant_folder = ConstantFolding::default();
    //     let mut label_conversion = LabelUnrolling::default();
    //     let mut redundancy_removal = RedundancyRemoval::default();
    //     let mut dce = DCE::default();
    //
    //     // if args.optimize {
    //     compiler.attach(&mut constant_folder);
    //     // }
    //
    //     compiler.attach(&mut redundancy_removal);
    //     compiler.attach(&mut dce);
    //     compiler.attach(&mut label_conversion);
    //
    //     match compiler.compile(&program) {
    //         Ok((opcodes, data)) => {
    //             Machine::with_options(options).run(&opcodes, &data);
    //         }
    //         Err(e) => {
    //             dbg!(e);
    //         }
    //     }
    // }
}
