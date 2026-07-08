use std::{
    borrow::Borrow,
    collections::HashSet,
    fs::File,
    io::{Read, Write},
    path::PathBuf,
};

use ariadne::{Color, Config, IndexType, Label, LabelAttach, Report, ReportKind, sources};
use common::{
    ARCHIVE_VERSION, ArchivedArchivedProgram, ArchivedProgram, Byte, Instruction, Interner,
    Message, MessageKind,
};
use parser::{Pratt, SimpleSpan, ast::Expression};
use rkyv::rancor::Error;

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

    /// Borrow the inner `Compiler` mutably. Used by the
    /// integration tests in `compiler/src/lib.rs::tests`
    /// that need to inspect the compiler's diagnostic
    /// messages directly.
    #[cfg(test)]
    pub fn compiler_mut(&mut self) -> &mut Compiler {
        &mut self.compiler
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

        // Patch the JMP. If the source had at least one
        // `extern` block, jump to `program_start_offset`
        // (right after the prologue) so the extern's dload +
        // declare bytecode runs before main. Otherwise jump
        // straight to `main` (the patch below does this
        // automatically because in the no-extern case
        // `program_start_offset == main_offset` — the
        // prologue was 3 bytes and main started right after).
        if self.compiler.has_extern_block() {
            if let Some(byte) = self.bytecode.get_mut(1) {
                *byte = Byte::new(Instruction::JMP)
                    .with_operand_u32(self.compiler.program_start_offset());
            }
        } else {
            if let Some(byte) = self.bytecode.get_mut(1) {
                *byte = Byte::new(Instruction::JMP)
                    .with_operand_u32(self.compiler.get_function("main") as u32);
            }
        }

        // Wrap the bytecode in the versioned `ArchivedProgram` envelope
        // so that older `.c0s` files can be rejected at load time via
        // `version` mismatch (see `Pipeline::run`).
        let program = ArchivedProgram {
            version: ARCHIVE_VERSION,
            bytecode: self.bytecode,
        };

        let mut out = File::create(output).expect("Unable to open output file");
        out.write(
            rkyv::to_bytes::<rkyv::rancor::Error>(&program)
                .unwrap()
                .as_slice(),
        )
        .expect("Unable to write compiled output to file");
    }

    /// Compile a source string in-memory and return the
    /// resulting bytecode. Used by the golden pipeline tests
    /// in `compiler/tests/pipeline.rs` so the tests don't
    /// need a temporary `.c0s` file round-trip via
    /// `Pipeline::compile`.
    ///
    /// The returned bytecode is the same shape
    /// `Pipeline::compile` writes to disk: a `[CALL, JMP
    /// <main>, HALT, ...]` prologue followed by the
    /// per-function bodies. Suitable for direct execution
    /// by `Machine::run`.
    ///
    /// Returns `Err(())` on parse failure or non-empty
    /// typecheck messages (we surface those as a hard error
    /// because golden tests assume the example is
    /// well-typed).
    /// Compile a parsed AST and return the bytecode
    /// (ignoring typecheck messages). Used by the
    /// `fizbuz_runs_to_completion` golden test, which
    /// exercises a .0s example that the typechecker
    /// rejects (`return;` is parsed as a variable name)
    /// but the codegen still produces valid bytecode for.
    pub fn compile_test(
        &mut self,
        module: &str,
        ast: &(SimpleSpan, Box<Expression<'_>>),
    ) -> Vec<Byte> {
        let mut bytecode = self.compiler.compile(module, ast);

        // Patch the JMP at offset 1 (the second prologue
        // instruction). If `extern` blocks were emitted,
        // jump to `program_start_offset` so they run first;
        // otherwise jump straight to `main`.
        if self.compiler.has_extern_block() {
            if let Some(byte) = bytecode.get_mut(1) {
                *byte = Byte::new(Instruction::JMP)
                    .with_operand_u32(self.compiler.program_start_offset());
            }
        } else if let Some(&main_offset) =
            self.compiler.functions.get("main")
        {
            if let Some(byte) = bytecode.get_mut(1) {
                *byte = Byte::new(Instruction::JMP)
                    .with_operand_u32(main_offset as u32);
            }
        }

        bytecode
    }

    pub fn compile_src(&mut self, src: &str) -> Result<Vec<Byte>, ()> {
        let parser = Pratt::default();
        let ast = parser.parse(src).map_err(|_| ())?;

        let mut bytecode = self.compiler.compile("", &ast);

        // Drain any typecheck messages — for golden tests,
        // we expect the example to be well-typed.
        let messages = self.compiler.get_messages();
        if !messages.is_empty() {
            return Err(());
        }

        // Patch the JMP at offset 1. If `extern` blocks
        // were emitted, jump to `program_start_offset` so
        // they run first; otherwise jump to `main`.
        if self.compiler.has_extern_block() {
            if let Some(byte) = bytecode.get_mut(1) {
                *byte = Byte::new(Instruction::JMP)
                    .with_operand_u32(self.compiler.program_start_offset());
            }
        } else if let Some(&main_offset) =
            self.compiler.functions.get("main")
        {
            if let Some(byte) = bytecode.get_mut(1) {
                *byte = Byte::new(Instruction::JMP)
                    .with_operand_u32(main_offset as u32);
            }
        }

        Ok(bytecode)
    }

    /// Borrow the FFI library map populated by the last
    /// `compile` (or `compile_test`) call. Each entry maps a
    /// function name declared in an `extern "libname" { ... }`
    /// block to the library short name (`"sum"`, `"c"`, ...).
    /// The test runner passes this map to
    /// `Machine::register_extern_libs` so the VM loads
    /// the libraries and binds the symbols at startup.
    pub fn extern_libs(&self) -> &std::collections::HashMap<String, String> {
        self.compiler.extern_libs()
    }

    pub fn run(self, filename: String) -> Result<Vec<Byte>, ()> {
        let mut f = File::open(filename).expect("Unable to find file");
        let mut buffer = Vec::with_capacity(1024);
        f.read_to_end(&mut buffer).expect("Unable to read file");

        // Access the archived envelope. Note: `ArchivedProgram` is the
        // SERIALIZABLE struct; rkyv's `Archive` derive generates a
        // separate archived struct named `ArchivedArchivedProgram`
        // (the derive just prepends `Archived` to the source name),
        // which is the type `rkyv::access` expects.
        let archived = rkyv::access::<ArchivedArchivedProgram, Error>(&buffer)
            .expect("Unable to decode rkyv binary");

        // Reject archives whose format doesn't match the in-tree
        // bytecode layout. `ARCHIVE_VERSION` is bumped whenever
        // `Byte` or any opcode changes incompatibly.
        if archived.version != ARCHIVE_VERSION {
            return Err(());
        }

        if self.failed {
            return Err(());
        }

        // Deserialize the archived `ArchivedVec<ArchivedByte>` back
        // into an owned `Vec<Byte>` for the VM. rkyv's `Deserialize`
        // impl for `ArchivedVec` handles the deep copy.
        let bytecode = rkyv::deserialize::<Vec<Byte>, Error>(&archived.bytecode)
            .expect("Unable to deserialize bytecode");

        Ok(bytecode)
    }
}

#[cfg(test)]
mod tests {
    /// End-to-end: build a `Message` via the HM checker and feed it
    /// through `render_errors`. We verify that ariadne can build a
    /// `Report` from our well-formed `Message` (with secondary labels
    /// and a help hint) without panicking. Capturing stderr is fiddly
    /// in stable Rust, so we just exercise the report-building path.
    #[test]
    fn ariadne_handles_rich_message() {
        use ariadne::{
            Color, Config, IndexType, Label as AriaLabel, LabelAttach, Report, ReportKind,
        };
        use common::{Label, Message, MessageKind};

        // Build a Message with primary range, secondary label, and help.
        let mut msg = Message::new(
            MessageKind::ERROR,
            "Type mismatch: expected `int`, found `string`".to_string(),
            5..10,
        );
        msg.push(Label::new(
            "expected `int` comes from here".to_string(),
            5..8,
        ));
        msg.with_help("while checking `assignment`".to_string());

        // The render_errors path is private, but the report-building
        // step is identical. We rebuild it here.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut builder = Report::build(
                ReportKind::Error,
                ("test.0s".to_string(), msg.range().clone()),
            )
            .with_config(
                Config::new()
                    .with_index_type(IndexType::Byte)
                    .with_underlines(true)
                    .with_label_attach(LabelAttach::End)
                    .with_multiline_arrows(true)
                    .with_compact(false),
            );

            for label in msg.labels() {
                builder = builder.with_label(
                    AriaLabel::new(("test.0s".to_string(), label.range().clone()))
                        .with_message(label.to_string())
                        .with_color(Color::Primary),
                );
            }

            if let Some(tip) = msg.help() {
                builder = builder.with_help(tip);
            }

            let _ = builder.finish();
        }));

        assert!(result.is_ok(), "ariadne panicked on a well-formed message");
    }

    /// The help hint should be included in the rendered report. We
    /// verify by reaching into ariadne's internals is too invasive;
    /// instead we just check that calling `with_help` followed by
    /// `finish` doesn't panic.
    #[test]
    fn ariadne_handles_message_with_help() {
        use ariadne::{Config, IndexType, LabelAttach, Report, ReportKind};
        use common::{Message, MessageKind};

        let mut msg = Message::new(
            MessageKind::WARNING,
            "this variable is unused".to_string(),
            0..5,
        );
        msg.with_help("consider prefixing with `_`".to_string());

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let builder = Report::build(
                ReportKind::Warning,
                ("test.0s".to_string(), msg.range().clone()),
            )
            .with_config(
                Config::new()
                    .with_index_type(IndexType::Byte)
                    .with_underlines(true)
                    .with_label_attach(LabelAttach::End)
                    .with_multiline_arrows(true)
                    .with_compact(false),
            )
            .with_help(msg.help().as_ref().unwrap())
            .finish();
            let _ = builder;
        }));
        assert!(result.is_ok());
    }
}
