use std::{borrow::Borrow, collections::HashSet, path::PathBuf};

use ariadne::{Color, Config, IndexType, Label, LabelAttach, Report, ReportKind, sources};
use common::{Byte, Instruction, Interner, Message, MessageKind, Value};
use parser::{Expression, ParserBuilder, SimpleSpan};

use crate::Compiler;

pub struct Pipeline<'pipeline> {
    failed: bool,
    cwd: PathBuf,
    bytecode: Vec<Byte<Value>>,
    processed: HashSet<String>,
    compiler: Compiler,
    parser: ParserBuilder<'pipeline>,
    interner: Interner<String>,
}

impl<'pipeline> Default for Pipeline<'pipeline> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'pipeline> Pipeline<'pipeline> {
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
            parser: ParserBuilder::new(),
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
                p.push("src");
                path.iter().for_each(|segment| p.push(segment));
                p.set_extension("0s");

                if let Some(path) = p.to_str() {
                    self.process(path.to_string());
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

    fn process(&mut self, file: String) {
        if self.processed.contains(&file) {
            return;
        }

        let src = std::fs::read_to_string(file.as_str()).expect("Failed to open file");
        self.processed.insert(file.clone());

        match self.parser.parse(src.as_str()) {
            Ok(ast) => {
                self.visit(&ast);
                self.bytecode.append(&mut self.compiler.compile(&ast));

                // for message in self.compiler.get_messages() {
                //     Self::render_errors(file.clone(), src.as_str(), &message);
                // }
            }
            Err(e) => Self::render_errors(file, src.as_str(), &e),
        }
    }

    pub fn run(mut self, filename: String) -> Result<Vec<Byte<Value>>, ()> {
        self.process(filename);

        if let Some(byte) = self.bytecode.first_mut() {
            *byte =
                Byte::new_with_operands(Instruction::CALL, [self.compiler.get_function("main"), 0]);
        }

        if self.failed {
            return Err(());
        }

        Ok(self.bytecode)
    }
}
