//! End-to-end tests for `use` / `mod` module resolution.

use std::cell::RefCell;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;

use compiler::Pipeline;
use machine::Machine;

// Tests change cwd; serialize with CWD_LOCK when running in parallel.

#[derive(Clone)]
struct SharedBuf(Rc<RefCell<Vec<u8>>>);

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
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
    let tmp = std::env::temp_dir().join(format!(
        "zero_script_ns_test_{}_{}_{}",
        test_name, pid, nanos
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp project dir");

    // Write the manifest.
    let manifest_path = tmp.join("zero.toml");
    std::fs::write(&manifest_path, manifest).expect("write zero.toml");

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
    let _cwd_lock = CwdLockGuard(CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner()));

    let original_cwd = std::env::current_dir().expect("get cwd");
    std::env::set_current_dir(project_root).expect("chdir to project root");

    struct CwdGuard(PathBuf);
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
            panic!("compile failed");
        }
    };

    let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
    let shared = SharedBuf(Rc::clone(&buf));
    let mut machine = Machine::<128>::default();
    machine.with_output(shared);

    machine.run_raw(&bytecode, &constants);
    let _ = machine.restore_output();

    let bytes = Rc::try_unwrap(buf)
        .expect("VM still holds a reference to the buffer")
        .into_inner();
    String::from_utf8(bytes).expect("captured output should be valid UTF-8")
}

fn compile_project_errors(project_root: &PathBuf, entry: &PathBuf) -> Vec<String> {
    let _cwd_lock = CwdLockGuard(CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner()));

    let original_cwd = std::env::current_dir().expect("get cwd");
    std::env::set_current_dir(project_root).expect("chdir to project root");

    struct CwdGuard(PathBuf);
    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }
    let _guard = CwdGuard(original_cwd);

    let mut pipeline = Pipeline::new();
    let result = pipeline.compile_src_from_file(entry.to_str().unwrap());
    assert!(result.is_err(), "expected compile to fail");
    pipeline
        .messages()
        .iter()
        .map(|m| m.message().to_string())
        .collect()
}

#[test]
fn use_single_segment_resolves_in_src_root() {
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = &[
        ("src/main.0s", "use foo::sadge;\nfn main() { sadge(); }\n"),
        ("src/foo/sadge.0s", "fn sadge() { print \"%x\\n\", 420; }\n"),
    ];
    let (root, entry) = build_project("use_single_segment", manifest, files, "src/main.0s");
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
        ("src/main.0s", "use foo::sadge as f;\nfn main() { f(); }\n"),
        ("src/foo/sadge.0s", "fn sadge() { print \"%i\", 99; }\n"),
    ];
    let (root, entry) = build_project("use_with_alias", manifest, files, "src/main.0s");
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
        ("src/main.0s", "use lib::io::read;\nfn main() { read(); }\n"),
        ("src/lib/io/read.0s", "fn read() { print \"%i\", 7; }\n"),
    ];
    let (root, entry) = build_project("use_multi_segment", manifest, files, "src/main.0s");
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
        ("src/main.0s", "use foo::greet;\nfn main() { greet(); }\n"),
        (
            "src/foo/greet.0s",
            "fn greet() { print \"%s\", \"from-src\"; }\n",
        ),
        (
            "vendor/foo/greet.0s",
            "fn greet() { print \"%s\", \"from-vendor\"; }\n",
        ),
    ];
    let (root, entry) = build_project("multiple_roots", manifest, files, "src/main.0s");
    let output = run_project(&root, &entry);
    assert_eq!(output, "from-src");
}

#[test]
fn no_manifest_uses_default_src_root() {
    let files = &[
        ("src/main.0s", "use foo::greet;\nfn main() { greet(); }\n"),
        ("src/foo/greet.0s", "fn greet() { print \"%i\", 42; }\n"),
    ];
    let tmp = std::env::temp_dir().join("zero_script_ns_test_no_manifest");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    for (rel, content) in files {
        let full = tmp.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("create parent dir");
        }
        std::fs::write(&full, content).expect("write source file");
    }
    let entry_full = tmp.join("src/main.0s");
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
            "src/main.0s",
            "use foo::*;\nfn main() { sadge(); greet(); }\n",
        ),
        (
            "src/foo.0s",
            "fn sadge() { print \"%i\", 100; }\n\
             fn greet() { print \"%i\", 200; }\n",
        ),
    ];
    let (root, entry) = build_project("use_glob", manifest, files, "src/main.0s");
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
        ("src/main.0s", "use foo::*;\nfn main() { top_only(); }\n"),
        ("src/foo.0s", "fn top_only() { print \"%s\", \"ok\"; }\n"),
        ("src/foo/bar.0s", "fn bar() { print \"%s\", \"BAD\"; }\n"),
    ];
    let (root, entry) = build_project("use_glob_subdir", manifest, files, "src/main.0s");
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
            "src/main.0s",
            "use iface::*;\n\
             impl Foreign<int> { fn id(int x) -> int { return x; } }\n\
             fn main() { }\n",
        ),
        (
            "src/iface.0s",
            "typeclass Foreign<T> { fn id(T x) -> int; }\n",
        ),
    ];
    let (root, entry) = build_project("orphan_instance_modules", manifest, files, "src/main.0s");
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
            "src/main.0s",
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
            "src/util/inc.0s",
            "fn inc(int x) -> int { return x + 1; }\n",
        ),
    ];
    let (root, entry) = build_project("two_module_polyfn_fib", manifest, files, "src/main.0s");

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
        bytecode
            .iter()
            .map(|b| b.bytecode())
            .collect::<Vec<_>>()
    );

    let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
    let shared = SharedBuf(Rc::clone(&buf));
    let mut machine = Machine::<128>::default();
    machine.with_output(shared);
    machine.run_raw(&bytecode, &constants);
    let _ = machine.restore_output();
    let output = String::from_utf8(
        Rc::try_unwrap(buf)
            .expect("VM still holds buffer")
            .into_inner(),
    )
    .expect("utf8");
    // fib(5)=5, inc(5)=6, id(6)=6
    assert_eq!(output, "6");
}

static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct CwdLockGuard(std::sync::MutexGuard<'static, ()>);
impl Drop for CwdLockGuard {
    fn drop(&mut self) {}
}
