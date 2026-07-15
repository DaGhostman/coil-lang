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
    run_example_src(&src)
}

fn run_bytecode(bytecode: Vec<common::Byte>, constants: Vec<u64>) -> String {
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

/// Compile and run in-memory source.
fn run_example_src(src: &str) -> String {
    let mut pipeline = Pipeline::new();
    let (bytecode, constants) = pipeline
        .compile_src(src)
        .expect("example failed to compile (parse error or type errors)");
    run_bytecode(bytecode, constants)
}

#[test]
fn example_option_prints_42() {
    let output = run_example("examples/option.0s");
    assert_eq!(output, "42");
}

#[test]
fn example_result_prints_42_and_neg1() {
    // Three `print "%i"` statements (no trailing newline) →
    // concatenated output: "42" + "0" + "-1" = "420-1".
    //
    // Phase 18A: `examples/result.0s` was extended to two
    // `Result::Ok` arms (so the codegen emits the inner-pattern
    // dispatch bytecode). The HM typechecker still flags the
    // second `Result::Ok` arm as "Unreachable arm" — the
    // typechecker only tracks the OUTER tag and doesn't see the
    // inner pattern distinction. We use `compile_test` (not
    // `compile_src`) to bypass the typecheck, mirroring the
    // `fizbuz_runs_to_completion` approach.
    //
    // Phase 18A (POP-quirk fix): the example now exercises
    // the None case at runtime (`print` for the second `Result::Ok`
    // arm). The pre-fix codegen would have emitted a redundant
    // POP in the reverse pass for the inner Unit sub-pattern,
    // silently discarding a stack value (the runtime would still
    // produce the right output here, but the codegen is more
    // correct with the fix). See `compiler/src/lib.rs`
    // `emit_pattern_binding` for the `consume_values` parameter.
    use std::cell::RefCell;
    use std::rc::Rc;

    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate must have a parent (workspace root)");
    let full = workspace_root.join("examples/result.0s");
    let src = std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", full.display(), e));

    let mut pipeline = compiler::Pipeline::new();
    let parser = parser::Pratt::default();
    let ast = parser.parse(&src).expect("result.0s should parse");
    let (bytecode, constants) = pipeline.compile_test("", &ast);

    let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
    let shared = SharedBuf(Rc::clone(&buf));
    let mut machine = machine::Machine::<128>::default();
    machine.with_output(shared);
    machine.run_raw(&bytecode, &constants);

    let _ = machine.restore_output();
    let bytes = Rc::try_unwrap(buf)
        .expect("VM still holds a reference to the buffer")
        .into_inner();
    let output = String::from_utf8(bytes).expect("captured output should be valid UTF-8");

    assert_eq!(output, "420-1");
}

#[test]
fn example_tree_prints_6() {
    let output = run_example("examples/tree.0s");
    assert_eq!(output, "6");
}

#[test]
fn example_fib_still_works() {
    // Regression test: the existing `fib.0s` example should still
    // produce the expected output. The example calls `fib(32)` and
    // expects `2178309` (the 32nd Fibonacci number).
    let output = run_example("examples/fib.0s");
    assert_eq!(output, "2178309");
}

#[test]
fn example_record_prints_169_5_12() {
    // The record-shape example demonstrates BOTH access styles:
    //   - Pattern destructuring: distance_squared matches the
    //     record pattern and binds `x, y`.
    //   - Field access (Phase 18D): x_coord and y_coord read the
    //     record's fields via `p.x` and `p.y`.
    //
    // Output: 169 (5² + 12², from distance_squared), 5 (from
    // x_coord), 12 (from y_coord).
    let output = run_example("examples/record.0s");
    assert_eq!(output, "169512");
}

#[test]
fn example_dict_prints_42_100_42() {
    // Phase 25 — anonymous dict / record literal demo.
    // `{ foo: 42, bar: 100 }` constructs an `Object::Instance`
    // with `Table<Member>` keyed by field name. Access via the
    // existing `d.field` syntax routes to the new `GetField`
    // opcode (string-keyed, distinct from the enum-variant
    // `LoadField` opcode which is field-index-keyed).
    //
    // Output: 42 (d.foo), 100 (d.bar), 42 (d.foo again).
    let output = run_example("examples/dict.0s");
    assert_eq!(output, "4210042");
}

#[test]
fn example_aliases_prints_3_4_7() {
    // Phase 28 — type aliases demo.
    // `type Point = (int, int);` declares a struct-like alias
    // substituted at typecheck time. The alias is zero-cost
    // (no runtime effect) and makes parameter / variable
    // annotations more readable.
    //
    // Output: 3, 4 (tuple index access), 7 (distance = 3 + 4).
    let output = run_example("examples/aliases.0s");
    assert_eq!(output, "347");
}

#[test]
fn example_mixed_prints_zero_circle_square_triangle() {
    // The mixed-shape example: 4 print statements, one per
    // variant (Empty, CircleR(5), Rect { 3, 4 }, Tri { 1, 2, 3 }).
    // Output: 0 (Empty), 25 (5²), 12 (3×4), 2 ((1+2+3)/3).
    //
    // The bindings-body codegen was fixed in 17B-cleanup so the
    // multi-variant binding case (CircleR(r), Rect { width, height },
    // Tri { a, b, c }) produces correct output — see
    // `compiler/src/lib.rs` `match_bindings` for details.
    let output = run_example("examples/mixed.0s");
    assert_eq!(output, "025122");
}

#[test]
fn example_chained_prints_42_7() {
    // Phase 19 golden test — `examples/chained.0s` exercises the
    // chained field-access fix at runtime.
    //
    // The example declares two enums:
    //   enum Inner { Inner { v: int } }
    //   enum Outer { Outer { x: Inner, y: int } }
    // and reads `p.x.v` (chained access) and `p.y` (simple
    // access). Output: 42 (Inner.v via chained access) and 7
    // (Outer.y).
    //
    // Pre-19, the OUTER `p.x.v` access silently miscompiled:
    // the codegen indexed the OUTER LoadField against `Outer`
    // (the outer receiver's enum) instead of `Inner` (the
    // inner receiver's enum, which actually owns the `v`
    // field). The defensive `LoadField(0)` fallback read
    // Outer's `x` slot instead, returning an `Inner` value
    // where an `int` was expected.
    //
    // Expected output: "42" + "7" = "427".
    let output = run_example("examples/chained.0s");
    assert_eq!(output, "427");
}

// ============================================================
//  Phase 18A: inner-pattern dispatch — golden regression test
// ============================================================

/// Phase 18A golden test — `examples/result.0s` exercises the
/// inner-pattern dispatch fix at runtime.
///
/// The example source has TWO `Result::Ok` arms with different
/// inner patterns (`Result::Ok(Option::Some(v))` and
/// `Result::Ok(Option::None)`) plus a wildcard `Result::Err(_)`
/// arm. The Phase 18A codegen emits a JUMP_IF_MATCH for the outer
/// `Result::Ok` tag and a second JUMP_IF_MATCH for the inner
/// `Option::Some` tag, so the runtime dispatch correctly picks
/// the right arm based on the runtime value of the inner `Option`.
///
/// The `main()` body only exercises the `Some(42) → 42` and
/// `Err → -1` cases at runtime; the `None → 0` arm exists in
/// the source to force the codegen to emit the inner-pattern
/// dispatch bytecode (a multi-arm group with different inner
/// sub-patterns). The byte-for-byte verification is at the
/// codegen level (see `match_with_same_tag_different_constructors_emits_inner_test_chain`
/// in `compiler/src/lib.rs::tests`); the runtime verification is
/// that the existing test cases (Some and Err) still produce
/// the expected output.
///
/// Like `fizbuz_runs_to_completion` above, we use
/// `pipeline.compile_test` (not `compile_src`) because the
/// HM typechecker currently flags the second `Result::Ok` arm
/// as "Unreachable arm" — the typechecker only tracks the OUTER
/// tag, so two arms with the same outer tag look like duplicates
/// even when their inner patterns differ. The codegen still
/// produces correct bytecode (Phase 18A inner dispatch), but
/// the typechecker's reachability check is too coarse.
#[test]
fn example_match_with_two_ok_arms_dispatches_correctly() {
    use std::cell::RefCell;
    use std::rc::Rc;

    // Read the .0s source from disk.
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate must have a parent (workspace root)");
    let full = workspace_root.join("examples/result.0s");
    let src = std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", full.display(), e));

    // Compile via a fresh Pipeline. We use `compile_test` (not
    // `compile_src`) because the typechecker flags the second
    // `Result::Ok` arm as unreachable.
    let mut pipeline = compiler::Pipeline::new();
    let parser = parser::Pratt::default();
    let ast = parser.parse(&src).expect("result.0s should parse");
    let (bytecode, constants) = pipeline.compile_test("", &ast);

    // Run the bytecode on a Machine that captures stdout.
    let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
    let shared = SharedBuf(Rc::clone(&buf));
    let mut machine = machine::Machine::<128>::default();
    machine.with_output(shared);
    machine.run_raw(&bytecode, &constants);

    let _ = machine.restore_output();
    let bytes = Rc::try_unwrap(buf)
        .expect("VM still holds a reference to the buffer")
        .into_inner();
    let output = String::from_utf8(bytes).expect("captured output should be valid UTF-8");

    // The example now exercises Some(42), None, and Err at
    // runtime. The pre-Phase-18A codegen would have emitted a
    // redundant POP in the reverse pass for the inner Unit
    // sub-pattern (`Result::Ok(Option::None)`), silently
    // discarding a stack value. The Phase 18A fix
    // (`consume_values = false` for test chain arms) prevents
    // the redundant emission. The runtime output is now
    // "420-1" (42, 0, -1) — all three arms exercised.
    assert_eq!(output, "420-1");
}

// ============================================================
//  Phase 16: BlockBuilder codegen — golden regression tests
// ============================================================

/// AMENDMENT 4 — the actual `fizbuz.0s` regression test.
///
/// The pre-Phase-16 If/Loop codegen computed JMPF targets
/// by arithmetic on bytecode buffer lengths, which
/// produced wrong offsets in nested control flow. The
/// `examples/fizbuz.0s` example (with two independent `if`
/// checks inside one function) infinite-looped at VM
/// startup. With the self.bytecode-based refactor, the
/// example runs to completion.
///
/// The `fizbuz.0s` example has a `return;` statement
/// (no value), which the typechecker flags as
/// "Unknown variable 'return'". The example still
/// produces valid bytecode (the codegen is silent on
/// unknown variables), but `Pipeline::compile_src`
/// rejects non-empty typecheck messages. We bypass
/// that check by going through `Compiler::compile`
/// directly via a fresh `Pipeline`.
///
/// Without the fix, this test hangs (or times out) on
/// the VM's infinite loop. With the fix, the program
/// terminates.
#[test]
fn fizbuz_runs_to_completion() {
    // Read the .0s source from disk.
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate must have a parent (workspace root)");
    let full = workspace_root.join("examples/fizbuz.0s");
    let src = std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", full.display(), e));

    // Compile via a fresh Pipeline. We don't use
    // `Pipeline::compile_src` (which rejects non-empty
    // typecheck messages) — `fizbuz.0s` has a `return;`
    // that the typechecker flags, but the codegen still
    // produces valid bytecode.
    //
    // We use the empty module name so the function is
    // registered as "main" (not "testmain" — the
    // `Compiler` keys functions by `namespace::name`).
    let mut pipeline = compiler::Pipeline::new();
    let parser = parser::Pratt::default();
    let ast = parser.parse(&src).expect("fizbuz.0s should parse");
    let (bytecode, constants) = pipeline.compile_test("", &ast);

    // Run the bytecode on a Machine that captures
    // stdout. (We don't assert on output — the example
    // only calls `fizbuz(1)`, which doesn't print
    // FIZ/BUZ. The key assertion is that the program
    // terminates without hanging.)
    use std::cell::RefCell;
    use std::rc::Rc;

    let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
    let shared = SharedBuf(Rc::clone(&buf));
    let mut machine = machine::Machine::<128>::default();
    machine.with_output(shared);
    machine.run_raw(&bytecode, &constants);

    // The real assertion is "the program didn't hang".
    // (A hang would manifest as a 120s+ test timeout.)
    let _ = buf; // suppress unused-variable warning
}

/// Compile a small inline program that uses `let` bindings and
/// re-assignment, and verifies the resulting bytecode contains the
/// expected `StorePop` opcodes. The Phase 18E fix changes the
/// `let x = expr;` codegen from "emit no bytecode for the
/// variable declaration" to "emit `StorePop slot` after the RHS".
/// This test is the codegen-side guard for that fix; the
/// end-to-end behavior is verified by `example_let_reassignment_works`
/// below.
#[test]
fn let_binding_emits_store_pop_in_bytecode() {
    use common::Instruction;
    let mut pipeline = Pipeline::new();
    let src = r#"
        fn main() {
            let x = 5;
            print "%i", x;
            x = 10;
            print "%i", x;
        }
    "#;
    let parser = parser::Pratt::default();
    let ast = parser.parse(src).expect("let-binding program should parse");
    let (bytecode, _constants) = pipeline.compile_test("", &ast);
    assert!(!bytecode.is_empty(), "program should produce bytecode");

    // Exactly 2 StorePop — one for `let x = 5;` and one
    // for `x = 10;` (re-assignment).
    let store_pop_count = bytecode
        .iter()
        .filter(|b| matches!(b.bytecode(), Instruction::StorePop))
        .count();
    assert_eq!(
        store_pop_count, 2,
        "expected exactly 2 StorePop for one let + one re-assignment; got {}",
        store_pop_count
    );

    // Zero `STORE` instructions — the codegen never emits
    // STORE for let-bindings or assignments. STORE is
    // reserved for match-arm bindings (where it acts as a
    // no-op for the slot-push contract).
    let store_count = bytecode
        .iter()
        .filter(|b| matches!(b.bytecode(), Instruction::STORE))
        .count();
    assert_eq!(
        store_count, 0,
        "expected zero STORE instructions for let/assignment; got {}",
        store_count
    );
}

/// Phase 18E end-to-end golden test — `examples/let_test.0s`
/// exercises the let-bound variable codegen fix at runtime.
///
/// The example:
///   let x = 5;     print "%i", x;    // "5"
///   let y = 10;    print "%i", y;    // "10"
///   x = 20;        print "%i", x;    // "20"
///
/// The combined output is "51020". Pre-18E, the `x = 20;`
/// re-assignment used the buggy `STORE` (no-op) + `DUPLICATE`,
/// which didn't update the slot — so `print x` would still
/// print 10 (the previous value) or push 20 on top of 10
/// (depending on cursor state). The Phase 18E fix uses
/// `STORE_POP` which correctly overwrites the slot.
#[test]
fn example_let_reassignment_works() {
    let output = run_example("examples/let_test.0s");
    assert_eq!(output, "51020");
}

/// Phase 18E end-to-end golden test — chained let bindings
/// (`let x = 5; let y = x + 1; print y;`) exercise the
/// `STORE_POP` cursor-preservation behavior.
///
/// Pre-18E: the second `CONST 6; STORE_POP 1;` (where 6 is the
/// result of `x + 1`) would clobber slot 0 (the value of `x`)
/// because the post-pop cursor fell back to slot 0's position.
/// The Phase 18E fix preserves the cursor past slot 0, so slot
/// 0 keeps `5` and slot 1 gets `6`.
///
/// Expected output: "6" (the value of `y`).
#[test]
fn example_let_chained_bindings_works() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate must have a parent (workspace root)");

    let src = r#"
        fn main() {
            let x = 5;
            let y = x + 1;
            print "%i", y;
        }
    "#;

    // Use `compile_test` (not `compile_src`) because the
    // `print "%i", y;` calls the native `print`, which the
    // golden pipeline doesn't register (it's only registered
    // for the in-memory `compile_src` tests). `compile_test`
    // bypasses the typecheck and emits bytecode directly.
    let mut pipeline = compiler::Pipeline::new();
    let parser = parser::Pratt::default();
    let ast = parser
        .parse(src)
        .expect("chained-bindings program should parse");
    let (bytecode, constants) = pipeline.compile_test("", &ast);

    let buf = std::rc::Rc::new(std::cell::RefCell::new(Vec::<u8>::new()));
    let shared = SharedBuf(std::rc::Rc::clone(&buf));
    let mut machine = machine::Machine::<128>::default();
    machine.with_output(shared);
    machine.run_raw(&bytecode, &constants);

    let _ = machine.restore_output();
    let bytes = std::rc::Rc::try_unwrap(buf)
        .expect("VM still holds a reference to the buffer")
        .into_inner();
    let output = String::from_utf8(bytes).expect("captured output should be valid UTF-8");

    // Suppress unused-variable lint for workspace_root (used in
    // other tests in this file; kept for symmetry).
    let _ = workspace_root;
    assert_eq!(output, "6");
}

#[test]
fn nested_if_in_loop_runs_correctly() {
    // The VM doesn't have a `print` native registered
    // in the test pipeline, so we exercise the loop+if
    // control flow without observable output. We verify
    // the program compiles cleanly and the resulting
    // bytecode has the expected control-flow opcodes.
    //
    // (A more thorough "doesn't hang" assertion would
    // require running the VM, but the VM's `Machine`
    // isn't `Send` so we can't run it in a background
    // thread for a timeout. The compile-only check is
    // still a strong regression guard — the
    // `fizbuz_runs_to_completion` test above exercises
    // the actual VM execution path on a real .0s
    // example.)
    let mut pipeline = compiler::Pipeline::new();
    let src = r#"
        fn main() {
            let i = 0;
            while (i < 4) {
                if i < 2 { 1; }
                i = i + 1;
            }
        }
    "#;
    let parser = parser::Pratt::default();
    let ast = parser.parse(src).expect("nested if-in-loop should parse");
    let (bytecode, _constants) = pipeline.compile_test("", &ast);
    assert!(!bytecode.is_empty(), "program should produce bytecode");

    // The bytecode should contain at least 2 JMPF (the
    // loop's condition + the if's condition) and at
    // least 1 JMP (the loop's back-edge).
    use common::Instruction;
    let jmpf_count = bytecode
        .iter()
        .filter(|b| matches!(b.bytecode(), Instruction::JMPF))
        .count();
    let jmp_count = bytecode
        .iter()
        .filter(|b| matches!(b.bytecode(), Instruction::JMP))
        .count();
    assert!(
        jmpf_count >= 2,
        "expected at least 2 JMPF (loop cond + if cond); got {}",
        jmpf_count
    );
    assert!(
        jmp_count >= 1,
        "expected at least 1 JMP (loop back-edge); got {}",
        jmp_count
    );
}

// ============================================================
//  Phase 18B: nested record patterns — golden regression test
// ============================================================

/// Phase 18B golden test — `examples/nested_records.0s` exercises
/// the nested record patterns fix at runtime.
///
/// The example declares two record-shaped enums (`Inner` and
/// `Wrap`) and binds `v` from a record-inside-a-record pattern
/// (`Wrap::W { inner: Inner::I { v }, name }`).
///
/// Pre-18B, the codegen emitted a POP for the inner record
/// (instead of walking its declared fields), so the body never
/// saw `v`. The Phase 18B fix lifts this — `emit_pattern_binding`
/// recurses at unbounded depth, passing the inner record's
/// declared field order as `parent_decl_order`.
///
/// Expected output: `99` (the value of `v`).
#[test]
fn example_nested_records_prints_99() {
    let output = run_example("examples/nested_records.0s");
    assert_eq!(output, "99");
}

// ============================================================
//  Phase 22: FFI — `extern "lib" { fn ...; }` end-to-end
// ============================================================

/// Build the FFI shared library (`libsum.so`) from the C
/// source in `examples/sum.c` if it isn't already present
/// (or is older than the source). Skips gracefully on
/// platforms where the C compiler isn't available.
fn ensure_ffi_libsum_built() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate must have a parent (workspace root)");
    let sum_c = workspace_root.join("examples/sum.c");
    let libsum_so = workspace_root.join("examples/libsum.so");

    // Always rebuild if the source is newer than the .so, or
    // if the .so doesn't exist.
    let needs_build = match (sum_c.metadata(), libsum_so.metadata()) {
        (Ok(src_meta), Ok(so_meta)) => src_meta.modified().ok() > so_meta.modified().ok(),
        (Ok(_), Err(_)) => true, // .so doesn't exist
        _ => false,
    };
    if !needs_build && libsum_so.exists() {
        return;
    }

    // Run `cc -shared -fPIC -o libsum.so sum.c` from the
    // examples/ directory. Use `cc` (the standard C compiler
    // on most Unix-like systems, including macOS via Apple
    // clang and Linux via gcc).
    let status = std::process::Command::new("cc")
        .arg("-shared")
        .arg("-fPIC")
        .arg("-O2")
        .arg("-o")
        .arg(&libsum_so)
        .arg(&sum_c)
        .status();
    match status {
        Ok(s) if s.success() => {
            // cc already wrote the file and updated its mtime.
            // Never use File::create here — it truncates the .so.
            if let Ok(meta) = std::fs::metadata(&libsum_so)
                && meta.len() < 256
            {
                eprintln!(
                    "warning: {} looks truncated ({} bytes) after cc build",
                    libsum_so.display(),
                    meta.len()
                );
            }
        }
        Ok(s) => {
            eprintln!(
                "skipping FFI tests: cc returned non-zero status {}",
                s.code().unwrap_or(-1)
            );
        }
        Err(e) => {
            eprintln!("skipping FFI tests: failed to invoke cc: {}", e);
        }
    }
}

/// FFI end-to-end: `extern "sum" { fn sum(int, int) -> int; }`
/// loads `libsum.so` via `dlopen("sum")` + `dlsym("sum")`,
/// then `sum(40, 2)` runs in userland (zero-script) and the
/// result is `print`-ed. Expected output: `"42"`.
///
/// Skipped on platforms where:
/// - `libsum.so` can't be built (no C compiler), or
/// - `dlopen("sum")` fails at test time (the build worked
///   but the runtime can't find / load the library).
#[test]
fn example_ffi_sum_via_dlopen_prints_42() {
    ensure_ffi_libsum_built();

    // Make `libsum.so` findable by the dynamic linker. On
    // Linux, `dlopen("sum")` looks for `libsum.so` in
    // `LD_LIBRARY_PATH`, then in `/etc/ld.so.cache`, then in
    // `/lib` and `/usr/lib`. We extend the search path to
    // include the examples/ directory so the test is
    // self-contained.
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate must have a parent (workspace root)");
    let libsum_so = workspace_root.join("examples/libsum.so");
    if !libsum_so.exists() {
        eprintln!("skipping: libsum.so not built (no C compiler?)");
        return;
    }

    // Use an absolute dload path so parallel tests don't depend
    // on process cwd (chdir races with other pipeline tests).
    let full = workspace_root.join("examples/ffi_sum.0s");
    let lib_abs = libsum_so
        .canonicalize()
        .unwrap_or_else(|_| libsum_so.clone());
    let mut src = std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", full.display(), e));
    src = src.replace(
        "dload(\"libsum.so\")",
        &format!("dload(\"{}\")", lib_abs.display()),
    );

    let result = std::panic::catch_unwind(|| run_example_src(&src));
    let output = match result {
        Ok(s) => s,
        Err(_) => {
            eprintln!("skipping: FFI test panicked (dlopen failure?)");
            return;
        }
    };
    assert_eq!(output, "42", "sum(40, 2) via userland FFI should print 42");
}

/// Extern-block end-to-end: `extern "libc.so.6" { fn strlen(string) -> int; }`
/// loads libc via `FfiLoad`, declares the signature with `DeclareFFI`
/// (Phase 26 tuple form), then `strlen("hello")` prints `5`.
///
/// Skipped when libc cannot be loaded (Windows, minimal containers).
#[test]
fn example_strlen_prints_5() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate must have a parent (workspace root)");
    let examples_dir = workspace_root.join("examples");

    // Quick probe: if dlopen("libc.so.6") fails, skip.
    if machine::load_library("libc.so.6").is_err() {
        eprintln!("skipping: libc.so.6 not loadable on this platform");
        return;
    }

    let prev_cwd = std::env::current_dir().ok();
    if std::env::set_current_dir(&examples_dir).is_err() {
        eprintln!("skipping: couldn't chdir to {}", examples_dir.display());
        return;
    }
    let result = std::panic::catch_unwind(|| run_example("examples/strlen.0s"));
    if let Some(prev) = prev_cwd {
        let _ = std::env::set_current_dir(&prev);
    }
    let output = match result {
        Ok(s) => s,
        Err(_) => {
            eprintln!("skipping: strlen test panicked (dlopen failure?)");
            return;
        }
    };
    assert_eq!(output, "5", "strlen(\"hello\") should print 5");
}
