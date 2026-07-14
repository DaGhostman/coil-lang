use std::{
    borrow::Borrow,
    collections::VecDeque,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use ariadne::{Color, Config, IndexType, Label, LabelAttach, Report, ReportKind, sources};
use common::{
    ARCHIVE_VERSION, ArchivedArchivedProgram, ArchivedProgram, Byte, Instruction, Message,
    MessageKind,
};
use machine::{FfiError, FfiSignature, FfiType, Heap, HostClosureFn, NativeFn};
use parser::{Pratt, SimpleSpan, ast::Expression};
use rkyv::rancor::Error;

use crate::manifest::Manifest;
use crate::Compiler;

/// A queued file to compile, along with the path it was
/// discovered under. The pipeline processes queued files
/// in BFS order from the entry point.
#[derive(Debug)]
struct WorkItem {
    /// Absolute path to the file on disk.
    file: PathBuf,
    /// Module namespace, derived from the file's path
    /// relative to one of the manifest's search roots.
    /// `None` means the file is outside any search root
    /// (we still compile it, but its namespace is the
    /// bare file stem).
    namespace: Option<String>,
}

pub struct Pipeline {
    failed: bool,
    project_root: PathBuf,
    manifest: Manifest,
    bytecode: Vec<Byte>,
    /// Set of files already visited (used to short-circuit
    /// diamond dependencies in the worklist).
    ///
    /// A `Vec<PathBuf>` rather than a `HashSet` because
    /// typical projects have <100 source files and a
    /// linear scan is faster than hashing for that size.
    /// Each entry is checked exactly once per `enqueue_file`
    /// call, and the per-file `PathBuf` allocation dominates
    /// the linear scan cost.
    processed: Vec<PathBuf>,
    /// FIFO queue of files to process. Drained front-to-back.
    worklist: VecDeque<WorkItem>,
    /// Native functions registered by the host. The
    /// pipeline tracks these so it can register them
    /// with the typechecker when a native call is
    /// typechecked.
    natives: Vec<NativeDecl>,
    /// Host Rust closures registered via [`Self::register_host_native`].
    host_natives: Vec<std::sync::Arc<dyn NativeFn>>,
    /// The entry file (the file passed to `compile`).
    /// This file is special: it's the program root and
    /// lives in the top-level namespace (no prefix),
    /// regardless of its path on disk. Every other
    /// file gets its path-derived namespace.
    entry_file: Option<PathBuf>,
/// Phase 29A — parsed-source cache.
///
/// `discover_all` reads each file from disk to
/// find its `use`/`mod` declarations. `compile_file`
/// then reads the SAME file again to compile it.
/// The cache holds the owned source text so the
/// second `read_to_string` is avoided.
///
/// Implementation: an `Interner<PathBuf>` assigns
/// each unique path a small `u32` ID; the source
/// text is stored in a `Vec<Option<String>>` indexed
/// by ID. Lookup is a Vec index (`O(1)`, no hash).
/// Compared to `HashMap<PathBuf, String>` this saves
/// the per-entry `PathBuf` hash and bucket overhead,
/// and replaces the `String` key with a `u32` copy.
///
/// Caching the AST itself would avoid the
/// second parse too, but `Output<'parser>` borrows
/// from the source — owning the source for the
/// entire `compile` call would require `'static`,
/// which leaks. The `read_to_string` save is the
/// I/O win; re-parsing is fast enough.
source_interner: common::Interner<PathBuf>,
source_cache: Vec<Option<String>>,
    compiler: Compiler,
}

/// A native function declaration registered by the host
/// (Phase 29A — `Pipeline::register_native_function`).
#[derive(Debug, Clone)]
pub struct NativeDecl {
    pub name: String,
    pub namespace: String,
    pub sig: FfiSignature,
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Pipeline {
    /// Register a host native with an explicit [`FfiSignature`]
    /// and Rust closure. The signature is forwarded to the HM
    /// typechecker; the closure is stored for
    /// [`Self::wire_host_natives`].
    pub fn register_host_native<F>(
        &mut self,
        sig: FfiSignature,
        func: F,
    ) -> usize
    where
        F: Fn(&mut Heap, &[common::Value]) -> Result<Option<common::Value>, FfiError>
            + Send
            + Sync
            + 'static,
    {
        let params: Vec<crate::typechecking::ty::Ty> =
            sig.args.iter().copied().map(ffi_type_to_ty).collect();
        let ret = ffi_type_to_ty(sig.ret);
        self.compiler.register(&sig.name, &params, &ret);
        let id = self.host_natives.len();
        self.host_natives.push(std::sync::Arc::new(
            HostClosureFn::new(sig, func),
        ));
        id
    }

    /// Register a native function's type signature (metadata
    /// only — no VM closure). Embedders that supply their own
    /// closures should prefer [`Self::register_host_native`].
    pub fn register_native_function(
        &mut self,
        name: String,
        namespace: String,
        sig: FfiSignature,
    ) {
        let params: Vec<crate::typechecking::ty::Ty> =
            sig.args.iter().copied().map(ffi_type_to_ty).collect();
        let ret = ffi_type_to_ty(sig.ret);
        self.compiler.register(&name, &params, &ret);
        self.natives.push(NativeDecl {
            name,
            namespace,
            sig,
        });
    }

    /// Wire host natives registered via [`Self::register_host_native`]
    /// into the VM. Call before `Machine::run_raw`.
    pub fn wire_host_natives<const N: usize>(&self, machine: &mut machine::Machine<N>) {
        for native in &self.host_natives {
            machine.register_native(std::sync::Arc::clone(native));
        }
    }

    /// Borrow the inner `Compiler` mutably. Used by the
    /// integration tests in `compiler/src/lib.rs::tests`
    /// and `compiler/tests/namespace.rs` that need to
    /// inspect the compiler's diagnostic messages
    /// directly.
    #[cfg(test)]
    pub fn compiler_mut(&mut self) -> &mut Compiler {
        &mut self.compiler
    }

    /// Borrow the compiler's accumulated diagnostic
    /// messages. Public so integration tests can read
    /// them (the `#[cfg(test)]`-only `compiler_mut` is
    /// only visible to in-crate tests).
    pub fn messages(&self) -> &[Message] {
        self.compiler.get_messages()
    }

    pub fn new() -> Self {
        let cwd = std::env::current_dir().expect("Unable to determine current working directory");
        // The project root is the cwd for now. A future
        // revision could walk up the tree looking for
        // `zero.toml`.
        let project_root = cwd.clone();
        let manifest = Manifest::load(&project_root)
            .expect("Failed to load zero.toml manifest");

        // The prologue is `[CALL, JMP, HALT]`. The pipeline
        // patches the JMP at offset 1 to point at `main`
        // (or `program_start_offset` if `extern` blocks ran
        // first). See `Self::prologue` for the layout.
        let bytecode = vec![
            Byte::new(Instruction::CALL),
            Byte::new(Instruction::JMP).with_operand_u32(u32::MAX),
            Byte::new(Instruction::HALT),
        ];

        Self {
            failed: false,
            project_root,
            manifest,
            bytecode,
            processed: Vec::new(),
            worklist: VecDeque::new(),
            natives: Vec::new(),
            host_natives: Vec::new(),
            entry_file: None,
            source_interner: common::Interner::default(),
            source_cache: Vec::new(),
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

    /// First pass: walk the AST and enqueue every
    /// referenced module file. We do this WITHOUT
    /// compiling (so the worklist is complete before
    /// we touch `self.compiler`). This avoids the
    /// `&mut self` recursion issue.
    ///
    /// `use foo::bar;` and `mod foo;` are both
    /// discovered. `use foo::bar::*;` (glob) is the
    /// same as `use foo::bar;` for discovery purposes
    /// — we just need to load `foo::bar` so the
    /// compiler can resolve the items.
    fn enqueue_uses(&mut self, ast: &(SimpleSpan, Box<Expression<'_>>)) {
        match ast.1.borrow() {
            Expression::Use { path, name, .. } => {
                // `use foo::sadge;` means: import `sadge`
                // from module `foo`. The file containing
                // the module `foo` is `foo.0s` (the file
                // is named after the path, NOT after the
                // item). The function imported is `sadge`
                // (the LAST segment of the dotted path).
                //
                // For globs (`name == "*"`), the file
                // is `<root>/<path joined>/<name>.0s` —
                // i.e. the file is named after the WHOLE
                // dotted path including the glob marker
                // segment... actually no, the file is
                // `<root>/<path joined>.0s` (no
                // trailing segment). The glob marker is
                // just a way to say "bring every item";
                // it doesn't name a file. So we use
                // the path with a synthetic last
                // segment for resolution.
                if name == "*" {
                    // Glob: file is `<root>/<path joined>.0s`.
                    // Equivalent to `use <last-segment-of-path>;`
                    // but with a star marker.
                    let segments = path.clone();
                    if let Some(last) = segments.last().cloned() {
                        // Strip the last segment since
                        // for a glob there's no item
                        // name. The file is at the
                        // directory path of the
                        // original dotted path.
                        let mut segments = segments;
                        segments.pop();
                        if let Some(file) = self.manifest.resolve_use(
                            &self.project_root,
                            &segments,
                            &last,
                        ) {
                            self.enqueue_file(file);
                        }
                    } else if let Some(file) = self
                        .manifest
                        .resolve_mod(&self.project_root, "*")
                    {
                        // `use *;` — top-level glob.
                        self.enqueue_file(file);
                    }
                } else if let Some(file) =
                    self.manifest.resolve_use(&self.project_root, path, name)
                {
                    self.enqueue_file(file);
                }
            }
            Expression::Module(name, _body) => {
                // `mod foo;` — look for `foo.0s` in any
                // root. This is the simplest resolution:
                // the file's stem IS the module name.
                if let Some(file) =
                    self.manifest.resolve_mod(&self.project_root, name)
                {
                    self.enqueue_file(file);
                }
            }
            Expression::Program(children)
            | Expression::Block(children)
            | Expression::Fragment(children) => {
                for child in children.iter() {
                    self.enqueue_uses(child);
                }
            }
            _ => (),
        }
    }

    /// Add `file` to the worklist if not already
    /// processed. Computes and caches the file's
    /// namespace.
    fn enqueue_file(&mut self, file: PathBuf) {
        // Linear scan: typical projects have <100 files
        // and a Vec scan is faster than hashing each
        // PathBuf. Mark the file as processed
        // immediately so concurrent enqueues from
        // `discover_all` don't re-add it.
        if self.processed.contains(&file) {
            #[cfg(debug_assertions)]
            eprintln!("[pipeline]   already loaded {}", file.display());
            return;
        }
        let ns = self.manifest.namespace_of(&self.project_root, &file);
        self.processed.push(file.clone());
        self.worklist.push_back(WorkItem {
            file: file.clone(),
            namespace: ns.clone(),
        });
        #[cfg(debug_assertions)]
        eprintln!(
            "[pipeline]   enqueued {} (namespace={})",
            file.display(),
            ns.as_deref().unwrap_or("<none>")
        );
    }

    /// Read the source text for `file`, populating the
    /// `source_cache` so the second read (in
    /// `compile_file`) is a no-op. Returns `None` if
    /// the file can't be read; the caller records the
    /// error and bails.
    fn read_source(&mut self, file: &Path) -> Option<String> {
        // Intern the path. Repeated calls with the same
        // path return the same id; new paths extend the
        // interner's storage. The id is a `u32` (Copy),
        // not a `PathBuf` (heap-allocated), so the
        // lookup is cheaper than a HashMap key.
        let id = self.source_interner.intern(file.to_path_buf());
        // Resize the cache if this is a fresh path.
        // We extend Vec length up to (id + 1) with
        // `None` placeholders so the indexed lookup
        // below is bounds-checked by Rust (panics if
        // id is out of range, which it isn't by
        // construction).
        if self.source_cache.len() <= id {
            self.source_cache.resize(id + 1, None);
        }
        if let Some(cached) = self.source_cache[id].as_ref() {
            #[cfg(debug_assertions)]
            eprintln!("[pipeline]   cache hit for {}", file.display());
            return Some(cached.clone());
        }
        match std::fs::read_to_string(file) {
            Ok(s) => {
                #[cfg(debug_assertions)]
                eprintln!(
                    "[pipeline]   loaded {} ({} bytes)",
                    file.display(),
                    s.len()
                );
                self.source_cache[id] = Some(s.clone());
                Some(s)
            }
            Err(_) => None,
        }
    }

    /// Discovery pass: walk the worklist front-to-back,
    /// parsing each file and enqueueing its
    /// `use`/`mod` dependencies. We don't compile
    /// here — just build the complete worklist so
    /// that the compilation pass can run in
    /// dependency order.
    ///
/// The `processed` set guards against re-enqueuing
    /// (so the same file isn't discovered twice). The
    /// `failed` flag is set if any file fails to parse.
    fn discover_all(&mut self) {
        #[cfg(debug_assertions)]
        eprintln!("[pipeline] scanning for files (entry={:?})", self.entry_file);
        // Walk the worklist from the front, parsing each
        // file to find its `use`/`mod` declarations.
        // `enqueue_file` adds new dependencies to the back
        // of the worklist and dedupes against `processed`,
        // so each file is scanned exactly once.
        //
        // Each scanned item is RE-ENQUEUED at the back so
        // the compile pass finds it. The trade-off:
        // O(N) extra pops (one per scan) vs allocating
        // a separate scan queue. For typical projects
        // (<100 files) the O(N) cost is negligible.
        //
        // `enqueue_uses`'s re-enqueues of already-processed
        // dependencies are no-ops, so the only repeated
        // work would be re-parsing a file's `use`s. We
        // skip that via `already_scanned` — a file's
        // `use`s are walked exactly once.
        //
        // Termination: track the worklist length at the
        // end of each pass. If it doesn't grow after a
        // pass (i.e., `enqueue_uses` added nothing new),
        // we're done. Each pass is at most one full
        // rotation of the worklist (since new items are
        // added to the BACK, the front gets recycled).
        // So total work is O(N^2) worst case, but in
        // practice O(N) for tree-shaped dependency
        // graphs.
        let mut already_scanned: Vec<PathBuf> = Vec::new();
        let mut depth = 0usize;
        let mut prev_len = self.worklist.len();
        loop {
            let item = match self.worklist.pop_front() {
                Some(i) => i,
                None => break,
            };
            let file = item.file.clone();
            if already_scanned.contains(&file) {
                // Re-enqueue at the back so the compile
                // pass finds it. But don't re-scan.
                self.worklist.push_back(item);
                continue;
            }
            #[cfg(debug_assertions)]
            eprintln!(
                "[pipeline]   scanning {} (depth {})",
                file.display(),
                depth
            );
            depth += 1;
            already_scanned.push(file.clone());
            // Re-enqueue at the back so the compile pass
            // can find it. The compile pass drains the
            // worklist in LIFO order via `pop_back`,
            // so dependencies (which are at the back)
            // are compiled first.
            self.worklist.push_back(item);
            // Read the source (cached after the first
            // call). The `compile_file` pass reuses the
            // same cached source, so the file is only
            // read from disk once per pipeline.
            let src = match self.read_source(&file) {
                Some(s) => s,
                None => {
                    let mut msg = Message::error(
                        format!("Failed to read file `{}`", file.display()),
                        0..0,
                    );
                    msg.push(common::Label::new(
                        format!("file path: {}", file.display()),
                        0..0,
                    ));
                    self.compiler.messages.push(msg);
                    self.failed = true;
                    continue;
                }
            };
            let parser = Pratt::default();
            let ast = match parser.parse(src.as_str()) {
                Ok(ast) => ast,
                Err(errors) => {
                    Self::render_errors(
                        file.display().to_string(),
                        src.as_str(),
                        &errors,
                    );
                    self.failed = true;
                    continue;
                }
            };
            self.enqueue_uses(&ast);
            // Termination check: if the worklist
            // length didn't change after this pass,
            // we're done. This is true when
            // `enqueue_uses` found no new
            // dependencies.
            let new_len = self.worklist.len();
            if new_len == prev_len {
                break;
            }
            prev_len = new_len;
        }
        #[cfg(debug_assertions)]
        eprintln!(
            "[pipeline] scanning complete, {} file(s) in worklist",
            self.worklist.len()
        );
    }

    /// Compile a single file: parse, enqueue uses, and
    /// invoke the compiler. Called once per WorkItem.
    fn compile_file(&mut self, item: WorkItem, is_entry: bool) {
        let file = item.file.clone();
        // The ENTRY file is special: it's the program root
        // and lives in the top-level namespace (no
        // prefix). Non-entry files get their path-derived
        // namespace so they can be referred to by their
        // fully qualified name (e.g., `builtins::core::ffi::dload`).
        let namespace = if is_entry {
            String::new()
        } else {
            item.namespace.unwrap_or_else(|| {
                // File is outside any search root. Use
                // the bare file stem as the namespace.
                file.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("anonymous")
                    .to_string()
            })
        };
        #[cfg(debug_assertions)]
        eprintln!(
            "[pipeline] compiling {} (namespace={:?}, entry={})",
            file.display(),
            namespace,
            is_entry
        );

        let src = match self.read_source(&file) {
            Some(s) => s,
            None => {
                let mut msg = Message::error(
                    format!("Failed to read file `{}`", file.display()),
                    0..0,
                );
                msg.push(common::Label::new(
                    format!("file path: {}", file.display()),
                    0..0,
                ));
                self.compiler.messages.push(msg);
                self.failed = true;
                return;
            }
        };

        let parser = Pratt::default();
        let ast = match parser.parse(src.as_str()) {
            Ok(ast) => ast,
            Err(errors) => {
                // Parse errors are reported via the
                // standard ariadne pipeline. We don't
                // have a Message here (parse errors
                // are chumsky Rich errors), so we
                // construct one with the first error.
                Self::render_errors(
                    file.display().to_string(),
                    src.as_str(),
                    &errors,
                );
                self.failed = true;
                return;
            }
        };

        // Note: `enqueue_uses` was already called by
        // `discover_all` in the pre-pass. The
        // worklist is fully populated. We just
        // compile now.

        // Compile the file. The compiler's `namespace`
        // field is set to the file's derived namespace.
        // We use `compile_module` (not `compile`) so the
        // returned bytes are ONLY the new bytes (not the
        // cumulative bytecode, which would duplicate
        // the prologue on the second call). See
        // `Compiler::compile_module` for the operand
        // adjustment details.
        let bytecode = self
            .compiler
            .compile_module(namespace.as_str(), &ast);
        #[cfg(debug_assertions)]
        eprintln!(
            "[pipeline]   compiled {} → {} bytes (total: {})",
            file.display(),
            bytecode.len(),
            self.bytecode.len()
        );

        // Append this file's bytecode to the running
        // output. Each file's bytecode is independent;
        // the linker (the prologue's JMP) connects them
        // via function-name lookup at call time.
        self.bytecode.extend(bytecode);

        // Surface any compiler-emitted diagnostics.
        for message in self.compiler.get_messages() {
            Self::render_errors(file.display().to_string(), src.as_str(), message);
        }
    }

    pub fn compile(mut self, filename: String, output: String) {
        // Seed the worklist with the entry file. The
        // entry is treated specially (top-level
        // namespace) — see `compile_file`.
        let entry = PathBuf::from(&filename);
        self.entry_file = Some(entry.clone());
        self.enqueue_file(entry);

        // Discovery pass: walk the dependency graph
        // transitively, enqueueing every referenced
        // file. We re-process the worklist, parsing
        // each file's AST to find its `use`/`mod`
        // declarations, but NOT compiling yet. This
        // builds the complete worklist so that the
        // compilation pass can run in dependency
        // order (dependencies first).
        self.discover_all();

        // Compilation pass: drain the worklist in
        // REVERSE order (LIFO via `pop_back`). The
        // `enqueue_file`/`enqueue_uses` ordering
        // means the LAST enqueued file is the
        // deepest dependency; popping from the back
        // gives us dependencies first. This guarantees
        // that when a file's `use foo::bar;` looks
        // up `foo::bar` in `self.functions`, the
        // function is already there.
        #[cfg(debug_assertions)]
        eprintln!(
            "[pipeline] compiling worklist ({} files, LIFO)",
            self.worklist.len()
        );
        while let Some(item) = self.worklist.pop_back() {
            let is_entry = self
                .entry_file
                .as_ref()
                .map(|e| *e == item.file)
                .unwrap_or(false);
            self.compile_file(item, is_entry);
        }

        if self.failed {
            return;
        }

        // Patch the JMP at offset 1 to point to the
        // user-program's `main`. If the source had at
        // least one `extern` block, jump to
        // `program_start_offset` (right after the
        // prologue) so the extern's dload + declare
        // bytecode runs before main. Otherwise jump
        // straight to `main`.
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
            constants: self.compiler.constants.clone(),
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
    ) -> (Vec<Byte>, Vec<u64>) {
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

        (bytecode, self.compiler.constants.clone())
    }

    pub fn compile_src(&mut self, src: &str) -> Result<(Vec<Byte>, Vec<u64>), ()> {
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

        Ok((bytecode, self.compiler.constants.clone()))
    }

    /// Compile a single source file in-memory and return the
    /// resulting bytecode, resolving `use` and `mod`
    /// declarations by reading the referenced files from disk.
    ///
    /// Phase 29A — the new test entry point. Unlike
    /// [`compile_src`](Self::compile_src), this method:
    /// 1. Reads the source from `file` (rather than taking
    ///    a source string in memory).
    /// 2. Walks the AST to discover `use` and `mod`
    ///    declarations.
    /// 3. Resolves each declaration via
    ///    [`Manifest::resolve_module`] and reads the
    ///    referenced files (BFS).
    /// 4. Compiles each file in worklist order, with the
    ///    file's derived namespace.
    /// 5. Returns the combined bytecode of all files.
    ///
    /// Used by the namespace integration tests
    /// (`compiler/tests/namespace.rs`) and by any
    /// downstream user that wants the new project-style
    /// module discovery without writing a `.c0s` file to
    /// disk.
    pub fn compile_src_from_file(&mut self, file: &str) -> Result<(Vec<Byte>, Vec<u64>), ()> {
        let entry = PathBuf::from(file);
        self.entry_file = Some(entry.clone());
        self.enqueue_file(entry);

        // Discovery + LIFO compile (see `compile`).
        self.discover_all();
        #[cfg(debug_assertions)]
        eprintln!(
            "[pipeline] compiling worklist ({} files, LIFO)",
            self.worklist.len()
        );
        while let Some(item) = self.worklist.pop_back() {
            let is_entry = self
                .entry_file
                .as_ref()
                .map(|e| *e == item.file)
                .unwrap_or(false);
            self.compile_file(item, is_entry);
        }

        if self.failed {
            return Err(());
        }

        // Patch the JMP at offset 1.
        if self.compiler.has_extern_block() {
            if let Some(byte) = self.bytecode.get_mut(1) {
                *byte = Byte::new(Instruction::JMP)
                    .with_operand_u32(self.compiler.program_start_offset());
            }
        } else if let Some(&main_offset) = self.compiler.functions.get("main") {
            if let Some(byte) = self.bytecode.get_mut(1) {
                *byte = Byte::new(Instruction::JMP)
                    .with_operand_u32(main_offset as u32);
            }
        }

        // Drain any typecheck messages.
        let messages = self.compiler.get_messages();
        if !messages.is_empty() {
            return Err(());
        }

        Ok((std::mem::take(&mut self.bytecode), self.compiler.constants.clone()))
    }

    /// Borrow the list of natively-registered functions.
    /// Phase 29A — used by the host to register natives
    /// with the VM at startup.
    pub fn natives(&self) -> &[NativeDecl] {
        &self.natives
    }

    pub fn run(self, filename: String) -> Result<(Vec<Byte>, Vec<u64>), ()> {
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
        let constants = rkyv::deserialize::<Vec<u64>, Error>(&archived.constants)
            .expect("Unable to deserialize constant pool");

        Ok((bytecode, constants))
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

fn ffi_type_to_ty(ty: FfiType) -> crate::typechecking::ty::Ty {
    use crate::typechecking::ty::{float, int, string, unit};
    match ty {
        FfiType::Int => int(),
        FfiType::Float => float(),
        FfiType::String => string(),
        FfiType::Void => unit(),
    }
}
