//! End-to-end tests for `use` / `mod` module resolution.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use compiler::Pipeline;
use machine::Machine;

// Tests change cwd; serialize with CWD_LOCK when running in parallel.

#[derive(Clone, Default)]
struct SharedBuf {
    inner: Arc<Mutex<Vec<u8>>>,
}

impl SharedBuf {
    fn new() -> Self {
        Self::default()
    }
}

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner
            .lock()
            .map_err(|_| std::io::ErrorKind::Other)?
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Create a temp project and return `(project_root, entry_path)`.
fn build_project(
    test_name: &str,
    manifest: &str,
    files: &[(&str, &str)],
    entry: &str,
) -> (PathBuf, PathBuf) {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("coil_ns_test_{}_{}_{}", test_name, pid, nanos));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp project dir");

    // Write the manifest.
    let manifest_path = tmp.join("coil.toml");
    std::fs::write(&manifest_path, manifest).expect("write coil.toml");

    // Write the source files.
    for (rel, content) in files {
        let full = tmp.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("create parent dir");
        }
        std::fs::write(&full, content).expect("write source file");
    }

    // Return the project root and the entry file's full path.
    let entry_full = tmp.join(entry);
    (tmp, entry_full)
}

fn run_project(project_root: &PathBuf, entry: &PathBuf) -> String {
    with_project_cwd(project_root, || {
        let mut pipeline = Pipeline::new();
        let (bytecode, constants) = match pipeline.compile_src_from_file(entry.to_str().unwrap()) {
            Ok(pair) => pair,
            Err(()) => {
                for msg in pipeline.messages() {
                    eprintln!("PIPELINE ERROR: {}", msg.message());
                }
                panic!("compile failed");
            }
        };
        run_bytecode(bytecode, constants, &pipeline)
    })
}

fn run_bytecode(bytecode: Vec<common::Byte>, constants: Vec<u64>, pipeline: &Pipeline) -> String {
    let shared = SharedBuf::new();
    let mut machine = Machine::<128>::default();
    machine.with_output(shared.clone());
    // Worker threads print via this buffer (not the parent's Write).
    machine.set_shared_print(shared.inner.clone());
    pipeline.wire_vm_ffi(&mut machine, None);
    pipeline.wire_host_natives(&mut machine);
    pipeline.wire_thread_program(&mut machine, &bytecode, &constants, pipeline.strings());
    machine.set_program_debug(pipeline.program_debug());
    machine.run_raw(
        &bytecode,
        &constants,
        pipeline.strings(),
        pipeline.static_slot_count(),
    );
    let _ = machine.restore_output();
    let bytes = shared
        .inner
        .lock()
        .expect("print buffer mutex poisoned")
        .clone();
    String::from_utf8(bytes).expect("captured output should be valid UTF-8")
}

/// Serialize cwd changes, `chdir` into `root`, run `f`, then restore cwd.
fn with_project_cwd<R>(root: &PathBuf, f: impl FnOnce() -> R) -> R {
    let _cwd_lock = CwdLockGuard(CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner()));
    let original_cwd = std::env::current_dir().expect("get cwd");
    std::env::set_current_dir(root).expect("chdir");
    struct CwdGuard(PathBuf);
    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }
    let _guard = CwdGuard(original_cwd);
    f()
}

/// Compile `entry` and assert every `JumpIfMatch` pool index is in range.
fn compile_entry_and_assert_jump_if_match_pool_valid(
    entry: &PathBuf,
) -> (Vec<common::Byte>, Vec<u64>, Pipeline) {
    let mut pipeline = Pipeline::new();
    let (bytecode, constants) = match pipeline.compile_src_from_file(entry.to_str().unwrap()) {
        Ok(pair) => pair,
        Err(()) => {
            for msg in pipeline.messages() {
                eprintln!("MSG: {}", msg.message());
            }
            panic!("compile failed");
        }
    };

    let mut oob = Vec::new();
    for (i, b) in bytecode.iter().enumerate() {
        if matches!(*b.bytecode(), common::Instruction::JumpIfMatch) {
            let idx = (b.operand_u32() & 0xFFFF) as usize;
            if idx >= constants.len() {
                oob.push((i, idx));
            }
        }
    }
    assert!(
        oob.is_empty(),
        "JumpIfMatch pool index out of range after multi-file link: \
         {oob:?} (constants.len() = {})",
        constants.len()
    );
    (bytecode, constants, pipeline)
}

fn compile_project_errors(project_root: &PathBuf, entry: &PathBuf) -> Vec<String> {
    with_project_cwd(project_root, || {
        let mut pipeline = Pipeline::new();
        let result = pipeline.compile_src_from_file(entry.to_str().unwrap());
        assert!(result.is_err(), "expected compile to fail");
        pipeline
            .messages()
            .iter()
            .map(|m| m.message().to_string())
            .collect()
    })
}

#[test]
fn use_single_segment_resolves_in_src_root() {
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = &[
        ("src/main.hy", "use foo::sadge;\nfn main() { sadge(); }\n"),
        ("src/foo/sadge.hy", "fn sadge() { print \"%x\\n\", 420; }\n"),
    ];
    let (root, entry) = build_project("use_single_segment", manifest, files, "src/main.hy");
    let output = run_project(&root, &entry);
    assert_eq!(output, "1a4\n");
}

#[test]
fn use_with_alias_renames_imported_item() {
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = &[
        ("src/main.hy", "use foo::sadge as f;\nfn main() { f(); }\n"),
        ("src/foo/sadge.hy", "fn sadge() { print \"%i\", 99; }\n"),
    ];
    let (root, entry) = build_project("use_with_alias", manifest, files, "src/main.hy");
    let output = run_project(&root, &entry);
    assert_eq!(output, "99");
}

#[test]
fn use_multi_segment_path_walks_into_nested_directory() {
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = &[
        ("src/main.hy", "use lib::io::read;\nfn main() { read(); }\n"),
        ("src/lib/io/read.hy", "fn read() { print \"%i\", 7; }\n"),
    ];
    let (root, entry) = build_project("use_multi_segment", manifest, files, "src/main.hy");
    let output = run_project(&root, &entry);
    assert_eq!(output, "7");
}

#[test]
fn multiple_roots_search_in_order() {
    let manifest = r#"
[module]
roots = ["./src", "./vendor"]
"#;
    let files = &[
        ("src/main.hy", "use foo::greet;\nfn main() { greet(); }\n"),
        (
            "src/foo/greet.hy",
            "fn greet() { print \"%s\", \"from-src\"; }\n",
        ),
        (
            "vendor/foo/greet.hy",
            "fn greet() { print \"%s\", \"from-vendor\"; }\n",
        ),
    ];
    let (root, entry) = build_project("multiple_roots", manifest, files, "src/main.hy");
    let output = run_project(&root, &entry);
    assert_eq!(output, "from-src");
}

#[test]
fn no_manifest_uses_default_src_root() {
    let files = &[
        ("src/main.hy", "use foo::greet;\nfn main() { greet(); }\n"),
        ("src/foo/greet.hy", "fn greet() { print \"%i\", 42; }\n"),
    ];
    let tmp = std::env::temp_dir().join("coil_ns_test_no_manifest");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    for (rel, content) in files {
        let full = tmp.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("create parent dir");
        }
        std::fs::write(&full, content).expect("write source file");
    }
    let entry_full = tmp.join("src/main.hy");
    let output = run_project(&tmp, &entry_full);
    assert_eq!(output, "42");
}

#[test]
fn use_glob_brings_items_into_scope() {
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = &[
        (
            "src/main.hy",
            "use foo::*;\nfn main() { sadge(); greet(); }\n",
        ),
        (
            "src/foo.hy",
            "fn sadge() { print \"%i\", 100; }\n\
             fn greet() { print \"%i\", 200; }\n",
        ),
    ];
    let (root, entry) = build_project("use_glob", manifest, files, "src/main.hy");
    let output = run_project(&root, &entry);
    assert_eq!(output, "100200");
}

#[test]
fn use_glob_does_not_reach_subdirectory_files() {
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = &[
        ("src/main.hy", "use foo::*;\nfn main() { top_only(); }\n"),
        ("src/foo.hy", "fn top_only() { print \"%s\", \"ok\"; }\n"),
        ("src/foo/bar.hy", "fn bar() { print \"%s\", \"BAD\"; }\n"),
    ];
    let (root, entry) = build_project("use_glob_subdir", manifest, files, "src/main.hy");
    let output = run_project(&root, &entry);
    assert_eq!(output, "ok");
}

#[test]
fn orphan_instance_across_modules_is_rejected() {
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = &[
        (
            "src/main.hy",
            "use iface::*;\n\
             impl Foreign<int> { fn id(int x) -> int { return x; } }\n\
             fn main() { }\n",
        ),
        ("src/iface.hy", "trait Foreign<T> { fn id(T x) -> int; }\n"),
    ];
    let (root, entry) = build_project("orphan_instance_modules", manifest, files, "src/main.hy");
    let msgs = compile_project_errors(&root, &entry);
    assert!(
        msgs.iter()
            .any(|m| m.contains("Orphan instance `Foreign<int>`")),
        "expected orphan-instance diagnostic, got: {:?}",
        msgs
    );
}

/// Phase 1: multi-module link finalizes peephole fusion once, relocating
/// `MakePolyFn` while keeping fused fib-style ops in the entry module.
#[test]
fn two_module_polyfn_and_fib_fuse_and_run() {
    use common::Instruction;

    let manifest = r#"
[module]
roots = ["./src"]
"#;
    // Keep recursive `fib` in the entry namespace (empty prefix) — namespaced
    // modules do not rewrite bare recursive calls to the FQN today.
    let files = &[
        (
            "src/main.hy",
            "use util::inc;\n\
             fn id<T>(T x) -> T { return x; }\n\
             fn fib(int n) -> int {\n\
               if n <= 2 { return 1; }\n\
               return fib(n - 1) + fib(n - 2);\n\
             }\n\
             fn main() {\n\
               let f = id;\n\
               print \"%i\", f(inc(fib(5)));\n\
             }\n",
        ),
        (
            "src/util/inc.hy",
            "fn inc(int x) -> int { return x + 1; }\n",
        ),
    ];
    let (root, entry) = build_project("two_module_polyfn_fib", manifest, files, "src/main.hy");

    let _cwd_lock = CwdLockGuard(CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner()));
    let original_cwd = std::env::current_dir().expect("get cwd");
    std::env::set_current_dir(&root).expect("chdir");
    struct CwdGuard(std::path::PathBuf);
    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }
    let _guard = CwdGuard(original_cwd);

    let mut pipeline = Pipeline::new();
    let (bytecode, constants) = match pipeline.compile_src_from_file(entry.to_str().unwrap()) {
        Ok(pair) => pair,
        Err(()) => {
            for msg in pipeline.messages() {
                eprintln!("PIPELINE ERROR: {}", msg.message());
            }
            panic!("two-module polyfn+fib compile failed");
        }
    };

    assert!(
        bytecode
            .iter()
            .any(|b| matches!(b.bytecode(), Instruction::MakePolyFn)),
        "expected MakePolyFn in linked bytecode"
    );
    let has_fused = bytecode.iter().any(|b| {
        matches!(
            b.bytecode(),
            Instruction::BinSlotImm
                | Instruction::BinSlotImmJmpf
                | Instruction::BinSlotSlot
                | Instruction::CmpJmpf
                | Instruction::BinReturn
                | Instruction::ConstReturnImm
                | Instruction::LoadReturnSlot
        )
    });
    assert!(
        has_fused,
        "final-link fusion should leave fused ops; opcodes: {:?}",
        bytecode.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
    );

    let output = run_bytecode(bytecode, constants, &pipeline);
    // fib(5)=5, inc(5)=6, id(6)=6
    assert_eq!(output, "6");
}

/// P2a: two glob imports must both resolve after discovery scans every dep.
#[test]
fn two_glob_imports_both_resolve() {
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = &[
        (
            "src/main.hy",
            "use a::*;\nuse b::*;\nfn main() { from_a(); from_b(); }\n",
        ),
        ("src/a.hy", "fn from_a() { print \"%i\", 1; }\n"),
        ("src/b.hy", "fn from_b() { print \"%i\", 2; }\n"),
    ];
    let (root, entry) = build_project("two_glob_imports", manifest, files, "src/main.hy");
    let output = run_project(&root, &entry);
    assert_eq!(output, "12");
}

/// P2b: bare sibling calls inside a non-entry module resolve to `ns::name`.
#[test]
fn sibling_bare_call_in_namespaced_module() {
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = &[
        (
            "src/main.hy",
            "use util::*;\nfn main() { print \"%i\", public_fn(); }\n",
        ),
        (
            "src/util.hy",
            "fn helper() -> int { return 7; }\n\
             fn public_fn() -> int { return helper(); }\n",
        ),
    ];
    let (root, entry) = build_project("sibling_bare_call", manifest, files, "src/main.hy");
    let output = run_project(&root, &entry);
    assert_eq!(output, "7");
}

#[test]
fn cross_module_static_slot_init_and_mutation() {
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = &[
        ("src/counter.hy", "static let n = 0;\n"),
        (
            "src/main.hy",
            "mod counter;\nfn main() {\n    counter::n = counter::n + 5;\n    print \"%i\", counter::n;\n}\n",
        ),
    ];
    let (root, entry) = build_project("cross_module_static", manifest, files, "src/main.hy");
    let output = run_project(&root, &entry);
    assert_eq!(output, "5");
}

#[test]
fn use_brace_group_imports_from_module_file() {
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = &[
        (
            "src/main.hy",
            "use math::{add, mul};\nfn main() { print \"%i\", add(2, 3); print \"%i\", mul(4, 5); }\n",
        ),
        (
            "src/math.hy",
            "fn add(int a, int b) -> int { return a + b; }\n\
             fn mul(int a, int b) -> int { return a * b; }\n",
        ),
    ];
    let (root, entry) = build_project("use_brace_group", manifest, files, "src/main.hy");
    let output = run_project(&root, &entry);
    assert_eq!(output, "520");
}

#[test]
fn use_brace_group_as_alias_imports_from_module_file() {
    // Parser covers `as` AST shape; this locks the desugar → alias map → call path.
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = &[
        (
            "src/main.hy",
            "use math::{add as plus};\nfn main() { print \"%i\", plus(2, 3); }\n",
        ),
        (
            "src/math.hy",
            "fn add(int a, int b) -> int { return a + b; }\n",
        ),
    ];
    let (root, entry) = build_project("use_brace_as", manifest, files, "src/main.hy");
    let output = run_project(&root, &entry);
    assert_eq!(output, "5");
}

#[test]
fn parse_fail_dependency_emits_single_diagnostic() {
    // discover_all emits parse errors and must not re-enqueue the bad file
    // for compile_file (which would duplicate the same diagnostic).
    use reporting::ReportConfig;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Capture {
        inner: Arc<Mutex<Vec<u8>>>,
    }
    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.inner.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = &[
        ("src/main.hy", "use bad::*;\nfn main() {}\n"),
        ("src/bad.hy", "@@@ not valid coil\n"),
    ];
    let (root, entry) = build_project("parse_fail_dep", manifest, files, "src/main.hy");

    let _cwd_lock = CwdLockGuard(CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner()));
    let original_cwd = std::env::current_dir().expect("get cwd");
    std::env::set_current_dir(&root).expect("chdir");
    struct CwdGuard(PathBuf);
    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }
    let _guard = CwdGuard(original_cwd);

    let capture = Capture::default();
    let mut pipeline = Pipeline::with_reporter(ReportConfig::default(), Box::new(capture.clone()));
    let result = pipeline.compile_src_from_file(entry.to_str().unwrap());
    assert!(
        result.is_err(),
        "expected compile to fail on bad dependency"
    );
    let _ = pipeline.finish_reporting();
    let out = String::from_utf8_lossy(&capture.inner.lock().unwrap()).into_owned();
    assert!(
        out.contains("Parse error") || out.contains("E0001"),
        "expected parse diagnostic in sink output, got: {out:?}"
    );
    let parse_hits = out.matches("Parse error").count() + out.matches("E0001").count();
    // Pretty sink prints both the code and "Parse error" once per diagnostic.
    assert!(
        parse_hits <= 2,
        "parse diagnostic appears duplicated (discover+compile): hit_count={parse_hits}, out={out:?}"
    );
    assert!(
        out.contains("bad.hy"),
        "diagnostic should name the unparseable dependency: {out:?}"
    );
}

#[test]
fn manifest_entry_path_joins_project_root() {
    let manifest = r#"
[module]
roots = ["./src"]

[entry]
file = "./src/main.hy"
"#;
    let files = &[("src/main.hy", "fn main() { print \"%i\", 42; }\n")];
    let (root, _entry) = build_project("manifest_entry", manifest, files, "src/main.hy");

    let _cwd_lock = CwdLockGuard(CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner()));
    let original_cwd = std::env::current_dir().expect("get cwd");
    std::env::set_current_dir(&root).expect("chdir");
    struct CwdGuard(PathBuf);
    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }
    let _guard = CwdGuard(original_cwd);

    let mut pipeline = Pipeline::new();
    let entry = pipeline
        .manifest_entry_path()
        .expect("manifest should declare [entry].file");
    assert!(
        entry.ends_with("src/main.hy"),
        "expected project-root-joined entry, got {}",
        entry.display()
    );
    let (bytecode, constants) = pipeline
        .compile_src_from_file(entry.to_str().unwrap())
        .expect("manifest entry should compile");
    let output = run_bytecode(bytecode, constants, &pipeline);
    assert_eq!(output, "42");
}

#[test]
fn use_item_from_module_file_without_subdir() {
    // Concrete `use math::add` must fall back to math.hy when math/add.hy
    // does not exist (the "modules in roots don't get imported" gap).
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = &[
        (
            "src/main.hy",
            "use math::add;\nfn main() { print \"%i\", add(10, 32); }\n",
        ),
        (
            "src/math.hy",
            "fn add(int a, int b) -> int { return a + b; }\n",
        ),
    ];
    let (root, entry) = build_project("use_module_file_item", manifest, files, "src/main.hy");
    let output = run_project(&root, &entry);
    assert_eq!(output, "42");
}

/// Multi-file programs that use `?` (JumpIfMatch → constant pool) in a
/// dependency must keep a single shared pool across `compile_module`
/// calls. Clearing the pool between files left worker threads panicking
/// at `jump_if_match_target` (pool index OOB) under `spawn`.
#[test]
fn multi_file_try_operator_pool_survives_module_link() {
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = [
        (
            "src/main.hy",
            r#"
use thread::*;
use pool::worker::run_jobs;

class Worker {
    thread: Thread,
    tx: Sender,
}

impl Worker {
    fn submit(string job) {
        send(self.tx, job)?;
    }

    fn join() {
        join(self.thread)?;
    }
}

fn main() {
    let pair = channel()?;
    let t = spawn(run_jobs, pair[1])?;
    let w = new Worker(t, pair[0]);
    w.submit("a")?;
    w.submit("b")?;
    w.submit("stop")?;
    w.join()?;
}
"#,
        ),
        (
            "src/pool/worker.hy",
            r#"
use thread::*;

fn run_jobs(Receiver rx) -> Result<int, ThreadError> {
    while true {
        let job = recv(rx)?;
        if job == "stop" {
            break;
        }
        print "%s,", job;
    }
    return 0;
}
"#,
        ),
    ];
    let (root, entry) = build_project("try_pool", manifest, &files, "src/main.hy");

    let output = with_project_cwd(&root, || {
        let (bytecode, constants, pipeline) =
            compile_entry_and_assert_jump_if_match_pool_valid(&entry);
        run_bytecode(bytecode, constants, &pipeline)
    });
    assert_eq!(output, "a,b,");
}

/// Dependency modules that call IO HostInvoke + `?` must share the same
/// constant pool as the entry (same class of bug as
/// `multi_file_try_operator_pool_survives_module_link`, but with real Stream
/// IO rather than thread channels).
#[test]
fn multi_file_io_hostinvoke_try_in_dependency() {
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = [
        (
            "src/main.hy",
            r#"
use io::*;
use helper::write_greeting;

fn main() {
    write_greeting()?;
    print "ok";
}
"#,
        ),
        (
            "src/helper.hy",
            r#"
use io::*;

fn write_greeting() -> Result<(), IoError> {
    let path = "greeting.bin";
    let h: byte = 104;
    let i: byte = 105;
    let nl: byte = 10;
    let payload: [byte] = [h, i, nl];
    let w = open(path, "w")?;
    write_all(w, payload)?;
    close(w)?;
    return ();
}
"#,
        ),
    ];
    let (root, entry) = build_project("io_host_try", &manifest, &files, "src/main.hy");

    let output = with_project_cwd(&root, || {
        let (bytecode, constants, pipeline) =
            compile_entry_and_assert_jump_if_match_pool_valid(&entry);
        run_bytecode(bytecode, constants, &pipeline)
    });
    assert_eq!(output, "ok");

    let written = std::fs::read(root.join("greeting.bin")).expect("dependency wrote greeting.bin");
    assert_eq!(written, b"hi\n");
}

/// Nested library layout (entry → facade → transport): deepest module owns
/// Stream IO + `?`. Models stdlib/http calling a transport helper while the
/// shared constant pool must survive every `compile_module` hop.
#[test]
fn multi_file_io_hostinvoke_try_in_nested_dependency() {
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = [
        (
            "src/main.hy",
            r#"
use facade::roundtrip;

fn main() {
    roundtrip()?;
    print "ok";
}
"#,
        ),
        (
            "src/facade.hy",
            r#"
use io::*;
use transport::write_then_read;

fn roundtrip() -> Result<(), IoError> {
    write_then_read("nested.bin")?;
    return ();
}
"#,
        ),
        (
            "src/transport.hy",
            r#"
use io::*;

fn write_then_read(string path) -> Result<(), IoError> {
    let a: byte = 65;
    let b: byte = 66;
    let payload: [byte] = [a, b];
    let w = open(path, "w")?;
    write_all(w, payload)?;
    close(w)?;

    let r = open(path, "r")?;
    let out: [byte] = [0, 0];
    read_exact(r, out)?;
    close(r)?;
    return ();
}
"#,
        ),
    ];
    let (root, entry) = build_project("io_host_nested", &manifest, &files, "src/main.hy");

    let output = with_project_cwd(&root, || {
        let (bytecode, constants, pipeline) =
            compile_entry_and_assert_jump_if_match_pool_valid(&entry);
        run_bytecode(bytecode, constants, &pipeline)
    });
    assert_eq!(output, "ok");

    let written = std::fs::read(root.join("nested.bin")).expect("transport wrote nested.bin");
    assert_eq!(written, b"AB");
}

/// Non-entry modules may `use` a sibling module's free functions (the old
/// echo NOTES caveat about `payload_eq` from `server.hy`). Locks the pattern
/// this PR's docs now treat as supported.
#[test]
fn multi_file_sibling_use_free_fn_from_dependency() {
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = [
        (
            "src/main.hy",
            r#"
use server::check;

fn main() {
    print "%i", check();
}
"#,
        ),
        (
            "src/protocol.hy",
            r#"
fn payload_eq(int a, int b) -> int {
    if a == b {
        return 1;
    }
    return 0;
}
"#,
        ),
        (
            "src/server.hy",
            r#"
use protocol::*;

fn check() -> int {
    return payload_eq(7, 7);
}
"#,
        ),
    ];
    let (root, entry) = build_project("sibling_use_dep", &manifest, &files, "src/main.hy");
    let output = run_project(&root, &entry);
    assert_eq!(output, "1");
}

static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct CwdLockGuard(std::sync::MutexGuard<'static, ()>);
impl Drop for CwdLockGuard {
    fn drop(&mut self) {}
}
