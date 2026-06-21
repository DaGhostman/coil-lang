//! Golden end-to-end tests for the `.0s` example files.
//!
//! Each test:
//! 1. Reads the `.0s` source from disk.
//! 2. Compiles it in-memory via `Pipeline::compile_src` (no
//!    `.c0s` file round-trip).
//! 3. Runs the bytecode through a `Machine` configured to
//!    capture stdout in a `Vec<u8>` (via the 15D.4
//!    `Machine::with_output` sink).
//! 4. Asserts on the exact captured output.
//!
//! The point of these tests is to catch regressions in the
//! full pipeline (parser → typechecker → codegen → VM) for
//! the canonical sum-of-sum / recursive-enum / fib
//! examples. They complement `compiler/tests/diagnostics.rs`
//! (which tests the typechecker's diagnostic messages) and
//! `compiler/src/lib.rs::tests` (which tests bytecode
//! emission in isolation).

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use compiler::Pipeline;
use machine::Machine;

/// Tiny `Write` impl that appends to a shared `Vec<u8>`.
/// Used to capture the VM's `PRINT` output for assertion
/// in the tests below. Shared by `Rc<RefCell<Vec<u8>>>`
/// so the test can recover the captured bytes after
/// `Machine::restore_output` drops the sink.
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

/// Read a `.0s` file from disk, compile it, run it on a
/// `Machine` that captures stdout, and return the captured
/// output as a `String`. The working directory must be the
/// workspace root so the relative `examples/...` paths
/// resolve. We walk up from `CARGO_MANIFEST_DIR` to the
/// workspace root (the compiler's `Cargo.toml` lives in
/// `compiler/`, so the workspace root is the parent).
fn run_example(path: &str) -> String {
    // CARGO_MANIFEST_DIR is `<workspace>/compiler` at
    // compile time. The workspace root is the parent.
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate must have a parent (workspace root)");
    let full = workspace_root.join(path);
    let src = std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", full.display(), e));

    let mut pipeline = Pipeline::new();
    let bytecode = pipeline
        .compile_src(&src)
        .expect("example failed to compile (parse error or type errors)");

    let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
    let shared = SharedBuf(Rc::clone(&buf));
    let mut machine = Machine::<128>::default();
    machine.with_output(shared);

    machine.run_raw(&bytecode);

    // Drop the sink so the test's `Rc` is the only one
    // (then we can move the `Vec` out).
    let _ = machine.restore_output();

    let bytes = Rc::try_unwrap(buf)
        .expect("VM still holds a reference to the buffer")
        .into_inner();
    String::from_utf8(bytes).expect("captured output should be valid UTF-8")
}

#[test]
fn example_option_prints_42() {
    let output = run_example("examples/option.0s");
    assert_eq!(output, "42");
}

#[test]
fn example_result_prints_42_and_neg1() {
    // Two `print "%i"` statements (no trailing newline) →
    // concatenated output: "42" + "-1" = "42-1".
    let output = run_example("examples/result.0s");
    assert_eq!(output, "42-1");
}

#[test]
fn example_tree_prints_6() {
    let output = run_example("examples/tree.0s");
    assert_eq!(output, "6");
}

#[test]
fn example_fib_still_works() {
    // Regression test: the existing `fib.0s` example (added
    // pre-15A) should still produce 13 for `fib(7)`.
    let output = run_example("examples/fib.0s");
    assert_eq!(output, "13");
}
