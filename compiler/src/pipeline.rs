use std::{
    borrow::Borrow,
    collections::HashSet,
    fs::File,
    io::{Read, Write},
    path::PathBuf,
};

use ariadne::{Color, Config, IndexType, Label, LabelAttach, Report, ReportKind, sources};
use common::{ArchivedByte, Byte, Instruction, Interner, Message, MessageKind};
use parser::{Pratt, SimpleSpan, ast::Expression};
use rkyv::{rancor::Error, vec::ArchivedVec};

use crate::Compiler;

pub struct Pipeline {
    failed: bool,
    cwd: PathBuf,
    bytecode: Vec<Byte>,
    processed: HashSet<String>,
    compiler: Compiler,
    interner: Interner<String>,
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Pipeline {
    pub fn register_native_function(&mut self, name: String) {
        self.interner.intern(name);

        todo!("Handle function registration of native functions");
    }

    pub fn new() -> Self {
        let cwd = std::env::current_dir().expect("Unable to determine current working directory");

        Self {
            failed: false,
            cwd,
            bytecode: Vec::with_capacity(16),
            processed: HashSet::default(),
            interner: Interner::default(),
            compiler: Compiler::default(),
        }
    }

    fn render_errors(filename: String, source: &str, message: &Message) {
        let mut sources = sources([(filename.clone(), source)]);

        let mut report = Report::build(
            match message.kind() {
                MessageKind::ERROR => ReportKind::Error,
                MessageKind::WARNING => ReportKind::Warning,
                MessageKind::INFO => ReportKind::Custom("Info", Color::BrightBlue),
            },
            (filename.clone(), message.range().clone()),
        )
        .with_message(message.message())
        .with_config(
            Config::new()
                .with_index_type(IndexType::Byte)
                .with_underlines(true)
                .with_label_attach(LabelAttach::End)
                .with_multiline_arrows(true)
                .with_compact(false),
        );

        for label in message.labels() {
            report = report.with_label(
                Label::new((filename.to_string(), label.range().clone()))
                    .with_message(label.to_string())
                    .with_color(Color::Primary),
            );
        }

        if let Some(tip) = message.help() {
            report = report.with_help(tip);
        }

        report.finish().eprint(&mut sources).unwrap()
    }

    fn visit(&mut self, node: &(SimpleSpan, Box<Expression<'_>>)) {
        match node.1.borrow() {
            Expression::Use { path, .. } => {
                let mut p = self.cwd.clone();
                let mut ns = String::new();
                p.push("src");
                path.iter().for_each(|segment| {
                    p.push(segment);
                    ns.push_str(format!("{}::", segment).as_str());
                });
                p.set_extension("0s");

                if let Some(path) = p.to_str() {
                    self.process(path.to_string(), ns);
                } else {
                    panic!("Unable to handle '{}'", p.display());
                }
            }
            Expression::Program(children)
            | Expression::Block(children)
            | Expression::Fragment(children) => {
                for child in children.iter() {
                    self.visit(child);
                }
            }
            _ => (),
        }
    }

    fn process(&mut self, file: String, ns: String) {
        if self.processed.contains(&file) {
            return;
        }

        let src = std::fs::read_to_string(file.as_str()).expect("Failed to open file");
        self.processed.insert(file.clone());

        let parser = Pratt::default();

        match parser.parse(src.as_str()) {
            Ok(ast) => {
                self.visit(&ast);
                let bytecode = self.compiler.compile(ns.as_str(), &ast);

                self.bytecode = bytecode;

                for message in self.compiler.get_messages() {
                    Self::render_errors(file.clone(), src.as_str(), message);
                }
            }
            Err(e) => Self::render_errors(file, src.as_str(), &e),
        }
    }

    pub fn compile(mut self, filename: String, output: String) {
        self.process(filename, String::default());

        if let Some(byte) = self.bytecode.get_mut(1) {
            *byte = Byte::new(Instruction::JMP)
                .with_operand_u32(self.compiler.get_function("main") as u32);
        }

        let mut out = File::create(output).expect("Unable to open output file");
        out.write(
            rkyv::to_bytes::<rkyv::rancor::Error>(&self.bytecode)
                .unwrap()
                .as_slice(),
        )
        .expect("Unable to write compiled output to file");
    }

    pub fn run(self, filename: String) -> Result<Vec<Byte>, ()> {
        let mut f = File::open(filename).expect("Unable to find file");
        let mut buffer = Vec::with_capacity(1024);
        f.read_to_end(&mut buffer).expect("Unable to read file");

        let _ = rkyv::access::<ArchivedVec<ArchivedByte>, Error>(&buffer)
            .expect("Unable to decode rkyv binary");

        if self.failed {
            return Err(());
        }

        Ok(self.bytecode)
    }
}
