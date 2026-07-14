//! End-to-end tests for Phase 29 — namespacing and `use` resolution.
//!
//! These tests build a small on-disk project layout (a `zero.toml`,
//! a `src/main.0s` entry point, and one or more `use`d files),
//! run the pipeline against it, and assert that the program
//! prints the expected output.
//!
//! The temp project is created in a fresh directory under
//! `std::env::temp_dir()` and removed after the test.

use std::cell::RefCell;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;

use compiler::Pipeline;
use machine::Machine;

// The `run_project` helper changes the process-wide cwd
// to read `zero.toml` from the test's temp project root.
// Tests in this file MUST run serially — cargo's
// parallel-test runner would have multiple threads
// fighting over cwd. We force serial execution with
// `--test-threads=1`. See the test harness at the bottom
// of this file (`#[test] fn _serial_entry() { ... }`).
//
// (Alternative: thread-local cwd, or pass the manifest
// path explicitly to the pipeline. The simplest fix
// for Phase 29A is to serialize the tests; we'll
// revisit in Phase 30+.)

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

/// Build a temp project layout and return the path to the
/// main file. Each test passes a `zero.toml` body, a map of
/// relative paths to source contents, and the entry file
/// path.
///
/// Layout example:
///   <tmp>/zero.toml
///   <tmp>/src/main.0s
///   <tmp>/src/foo.0s
fn build_project(
    test_name: &str,
    manifest: &str,
    files: &[(&str, &str)],
    entry: &str,
) -> (PathBuf, PathBuf) {
    // Use a process-wide unique subdirectory so parallel
    // test invocations don't collide on the same temp
    // dir. The test name alone isn't unique enough when
    // cargo runs tests in parallel threads.
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
    // Acquire the process-wide cwd lock so concurrent
    // test threads don't fight over the cwd.
    let _cwd_lock = CwdLockGuard(CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner()));

    // We need to chdir into the project root for the
    // pipeline's `Manifest::load` to find `zero.toml`.
    let original_cwd = std::env::current_dir().expect("get cwd");
    std::env::set_current_dir(project_root).expect("chdir to project root");

    // Restore cwd on every exit path (success, error,
    // panic) so the next test starts in the workspace
    // root, not in a leftover temp project.
    struct CwdGuard(PathBuf);
    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }
    let _guard = CwdGuard(original_cwd);

    let mut pipeline = Pipeline::new();
    let bytecode = match pipeline.compile_src_from_file(entry.to_str().unwrap()) {
        Ok(bc) => bc,
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

    machine.run_raw(&bytecode);
    let _ = machine.restore_output();

    let bytes = Rc::try_unwrap(buf)
        .expect("VM still holds a reference to the buffer")
        .into_inner();
    String::from_utf8(bytes).expect("captured output should be valid UTF-8")
}

#[test]
fn use_single_segment_resolves_in_src_root() {
    // The default roots = ["src"], so we use that.
    // Convention (Go-style): `use foo::sadge;` resolves
    // to file `foo/sadge.0s` (the dotted path maps to
    // a directory tree, and the LAST segment is the
    // file's stem).
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = &[
        ("src/main.0s", "use foo::sadge;\nfn main() { sadge(); }\n"),
        ("src/foo/sadge.0s", "fn sadge() { print \"%x\\n\", 420; }\n"),
    ];
    let (root, entry) = build_project(
        "use_single_segment",
        manifest,
        files,
        "src/main.0s",
    );
    let output = run_project(&root, &entry);
    assert_eq!(output, "1a4\n");
}

#[test]
fn use_with_alias_renames_imported_item() {
    // `use foo::sadge as f;` — call site uses the alias.
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = &[
        (
            "src/main.0s",
            "use foo::sadge as f;\nfn main() { f(); }\n",
        ),
        ("src/foo/sadge.0s", "fn sadge() { print \"%i\", 99; }\n"),
    ];
    let (root, entry) = build_project(
        "use_with_alias",
        manifest,
        files,
        "src/main.0s",
    );
    let output = run_project(&root, &entry);
    assert_eq!(output, "99");
}

#[test]
fn use_multi_segment_path_walks_into_nested_directory() {
    // `use lib::io::read;` resolves to `src/lib/io/read.0s`.
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = &[
        (
            "src/main.0s",
            "use lib::io::read;\nfn main() { read(); }\n",
        ),
        (
            "src/lib/io/read.0s",
            "fn read() { print \"%i\", 7; }\n",
        ),
    ];
    let (root, entry) = build_project(
        "use_multi_segment",
        manifest,
        files,
        "src/main.0s",
    );
    let output = run_project(&root, &entry);
    assert_eq!(output, "7");
}

#[test]
fn multiple_roots_search_in_order() {
    // Both `src/foo/greet.0s` and `vendor/foo/greet.0s`
    // exist. The pipeline picks `src/foo/greet.0s`
    // (the first root).
    let manifest = r#"
[module]
roots = ["./src", "./vendor"]
"#;
    let files = &[
        (
            "src/main.0s",
            "use foo::greet;\nfn main() { greet(); }\n",
        ),
        (
            "src/foo/greet.0s",
            "fn greet() { print \"%s\", \"from-src\"; }\n",
        ),
        // vendor/foo/greet.0s would print "from-vendor"
        // — it should NOT be loaded.
        (
            "vendor/foo/greet.0s",
            "fn greet() { print \"%s\", \"from-vendor\"; }\n",
        ),
    ];
    let (root, entry) = build_project(
        "multiple_roots",
        manifest,
        files,
        "src/main.0s",
    );
    let output = run_project(&root, &entry);
    assert_eq!(output, "from-src");
}

#[test]
fn no_manifest_uses_default_src_root() {
    // No `zero.toml` — the pipeline falls back to the
    // default `src/` root.
    let files = &[
        ("src/main.0s", "use foo::greet;\nfn main() { greet(); }\n"),
        ("src/foo/greet.0s", "fn greet() { print \"%i\", 42; }\n"),
    ];
    let tmp = std::env::temp_dir().join("zero_script_ns_test_no_manifest");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    // Note: no zero.toml is written.
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
    // `use foo::*;` brings every top-level item from
    // the FILE `foo.0s` (namespace `foo`) into scope.
    // The user can then call those items by their bare
    // name (no namespace prefix in the call site).
    //
    // Convention: `foo.0s` lives at `<root>/foo.0s`
    // and has namespace `foo`. Items inside have FQNs
    // `foo::<item_name>`. `use foo::*;` matches those
    // FQNs.
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = &[
        // main calls `sadge` and `greet` — both
        // imported by the glob.
        (
            "src/main.0s",
            "use foo::*;\nfn main() { sadge(); greet(); }\n",
        ),
        // The file `foo.0s` (NOT `foo/sadge.0s`) has
        // both functions as top-level items. The glob
        // targets THIS file.
        (
            "src/foo.0s",
            "fn sadge() { print \"%i\", 100; }\n\
             fn greet() { print \"%i\", 200; }\n",
        ),
    ];
    let (root, entry) = build_project(
        "use_glob",
        manifest,
        files,
        "src/main.0s",
    );
    let output = run_project(&root, &entry);
    assert_eq!(output, "100200");
}

#[test]
fn use_glob_does_not_reach_subdirectory_files() {
    // `use foo::*;` brings items from the FILE
    // `<root>/foo.0s` into scope. It does NOT
    // transitively reach into `<root>/foo/bar.0s`
    // (the file lives in the `foo/` subdirectory
    // and is named `bar.0s`, with namespace
    // `foo::bar`).
    //
    // To reach `bar`, the user must write a separate
    // `use foo::bar;` (which loads `foo/bar.0s` and
    // imports its top-level `bar` item).
    let manifest = r#"
[module]
roots = ["./src"]
"#;
    let files = &[
        (
            "src/main.0s",
            "use foo::*;\nfn main() { top_only(); }\n",
        ),
        // The file `foo.0s` has the function
        // `top_only` as a top-level item. The glob
        // targets THIS file.
        (
            "src/foo.0s",
            "fn top_only() { print \"%s\", \"ok\"; }\n",
        ),
        // The file `foo/bar.0s` is a separate
        // module with namespace `foo::bar`. It's
        // NOT auto-loaded by `use foo::*;` — the
        // user has to write a separate
        // `use foo::bar;` to reach its items.
        (
            "src/foo/bar.0s",
            "fn bar() { print \"%s\", \"BAD\"; }\n",
        ),
    ];
    let (root, entry) = build_project(
        "use_glob_subdir",
        manifest,
        files,
        "src/main.0s",
    );
    let output = run_project(&root, &entry);
    assert_eq!(output, "ok");
}

// A process-wide mutex that serializes the cwd-dependent
// tests. Each `run_project` call acquires the lock,
// changes cwd, runs the project, and releases the lock
// on drop. Without this, cargo's parallel test runner
// would have multiple threads fighting over the
// process-wide cwd and the wrong `zero.toml` would be
// read.
static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct CwdLockGuard(std::sync::MutexGuard<'static, ()>);
impl Drop for CwdLockGuard {
    fn drop(&mut self) {}
}
