use std::{fs::File, io::Read, path::Path};

use clap::{ArgAction, Parser as Clap, ValueHint};
use compiler::{
    passes::{constant_folding::ConstantFolding, typechecker::TypeChecker},
    Compiler,
};
use machine::{options::MachineOptions, Machine};
use parser::Parser;
use scanner::{buffer::Buffer, Scanner};

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
    let args = Options::parse();

    let mut options = MachineOptions::default();

    if let Some(location) = args.config {
        if let Ok(mut file) = File::open(location) {
            let mut buff = vec![];
            if let Ok(len) = file.read_to_end(&mut buff) {
                if len > 0 {
                    options = toml::from_str::<MachineOptions>(
                        &String::from_utf8(buff).unwrap_or_default(),
                    )
                    .unwrap();
                }
            }
        }
    }

    if let Some(cwd) = args.cwd {
        let directory = Path::new(&cwd);
        if let Err(err) = std::env::set_current_dir(directory) {
            eprintln!("Unable to use '{}' as working directory: {}", cwd, err);

            return;
        }
    }

    options.set_quiet(args.quiet);

    if let Ok(buffer) = Buffer::new(&args.file) {
        // if let Ok(buffer) = Buffer::try_from("match true { Some(Some(val)) => { print 'Some'; } };") {
        // if let Ok(buffer) = Buffer::try_from("print 1 + 2 + 3 + 4 + 5;") {
        // if let Ok(buffer) = Buffer::try_from("print 1..10;") {
        // if let Ok(buffer) = Buffer::try_from("print foo.say('hello').bar();") {
        // if let Ok(buffer) = Buffer::try_from("print [5, 4, 3, 2, 1];") {
        let mut scanner = Scanner::new(buffer, Some(args.file));
        let mut compiler = Compiler::default();

        let mut typechecker = TypeChecker::default();
        let mut constant_folder = ConstantFolding::default();

        compiler.attach(&mut typechecker);

        if args.optimize {
            compiler.attach(&mut constant_folder);
        }

        if let Ok(program) = Parser::default().parse(&mut scanner) {
            dbg!(&program.code());
            if let Ok(opcodes) = compiler.compile(program) {
                // let mut bytes = vec![];
                // for p in opcodes.code().clone() {
                // bytes.append(&mut p.bits());
                // println!(
                //     "{:?} ",
                //     p.bits()
                //         .iter()
                //         .map(|b| b.to_string())
                //         .collect::<Vec<String>>()
                //         .join("|")
                // );
                // }

                // println!("{:?}", bytes);
                if let Err(err) = Machine::with_options(options).run(opcodes) {
                    eprintln!("{}", err);
                }
            }
        }
    } else {
        eprintln!("Missing file");
    }
}
