use std::{borrow::Borrow, collections::HashSet, path::PathBuf};

use ariadne::{Color, Config, IndexType, Label, Report, ReportKind, sources};
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

    fn render_errors(&mut self, filename: String, source: &str, messages: HashSet<Message>) {
        let mut sources = sources([(filename.clone(), source)]);

        for message in messages.iter() {
            Report::build(
                match message.kind() {
                    MessageKind::ERROR => {
                        self.failed = true;
                        ReportKind::Error
                    }
                    MessageKind::WARNING => ReportKind::Warning,
                    MessageKind::INFO => ReportKind::Advice,
                },
                (filename.clone(), message.range().clone()),
            )
            .with_config(
                Config::new()
                    .with_index_type(IndexType::Byte)
                    .with_compact(false),
            )
            .with_message(message.message())
            .with_label(
                Label::new((filename.to_string(), message.range().clone())).with_color(Color::Red),
            )
            .finish()
            .eprint(&mut sources)
            .unwrap()
        }
    }

    fn visit(&mut self, node: &(SimpleSpan, Box<Expression<'_>>)) {
        match node.1.borrow() {
            Expression::Use { path, .. } => {
                let mut p = self.cwd.clone();
                p.push("src");
                path.into_iter().for_each(|segment| p.push(segment));
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
        self.processed.insert(file.clone());

        let src = std::fs::read_to_string(file.clone()).expect("Failed to open file");

        match self.parser.parse(src.as_str()) {
            Ok(ast) => {
                self.visit(&ast);
                match self.compiler.compile(&ast) {
                    Ok(mut bytecode) => {
                        self.bytecode.append(&mut bytecode);
                    }
                    Err(e) => self.render_errors(file, src.as_str(), e),
                }
            }
            Err(e) => self.render_errors(file, src.as_str(), e),
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
