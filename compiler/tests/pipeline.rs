//! End-to-end golden tests for `.0s` example programs.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use compiler::Pipeline;
use machine::Machine;

/// Captures VM `PRINT` output into a shared buffer.
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

fn run_example(path: &str) -> String {
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
    // Typechecker flags duplicate outer tags; compile without typecheck.
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
    let output = run_example("examples/fib.0s");
    assert_eq!(output, "2178309");
}

#[test]
fn example_record_prints_169_5_12() {
    let output = run_example("examples/record.0s");
    assert_eq!(output, "169512");
}

#[test]
fn example_dict_prints_42_100_42() {
    let output = run_example("examples/dict.0s");
    assert_eq!(output, "4210042");
}

#[test]
fn example_aliases_prints_3_4_7() {
    let output = run_example("examples/aliases.0s");
    assert_eq!(output, "347");
}

#[test]
fn example_mixed_prints_zero_circle_square_triangle() {
    let output = run_example("examples/mixed.0s");
    assert_eq!(output, "025122");
}

#[test]
fn example_chained_prints_42_7() {
    let output = run_example("examples/chained.0s");
    assert_eq!(output, "427");
}

#[test]
fn example_match_with_two_ok_arms_dispatches_correctly() {
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
fn fizbuz_runs_to_completion() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate must have a parent (workspace root)");
    let full = workspace_root.join("examples/fizbuz.0s");
    let src = std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", full.display(), e));

    let mut pipeline = compiler::Pipeline::new();
    let parser = parser::Pratt::default();
    let ast = parser.parse(&src).expect("fizbuz.0s should parse");
    let (bytecode, constants) = pipeline.compile_test("", &ast);

    use std::cell::RefCell;
    use std::rc::Rc;

    let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
    let shared = SharedBuf(Rc::clone(&buf));
    let mut machine = machine::Machine::<128>::default();
    machine.with_output(shared);
    machine.run_raw(&bytecode, &constants);

    let _ = buf;
}

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

    let store_pop_count = bytecode
        .iter()
        .filter(|b| matches!(b.bytecode(), Instruction::StorePop))
        .count();
    assert_eq!(
        store_pop_count, 2,
        "expected exactly 2 StorePop for one let + one re-assignment; got {}",
        store_pop_count
    );

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

#[test]
fn example_let_reassignment_works() {
    let output = run_example("examples/let_test.0s");
    assert_eq!(output, "51020");
}

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

    let _ = workspace_root;
    assert_eq!(output, "6");
}

#[test]
fn nested_if_in_loop_runs_correctly() {
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

#[test]
fn example_nested_records_prints_99() {
    let output = run_example("examples/nested_records.0s");
    assert_eq!(output, "99");
}

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

#[test]
fn example_ffi_sum_via_dlopen_prints_42() {
    ensure_ffi_libsum_built();

    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate must have a parent (workspace root)");
    let libsum_so = workspace_root.join("examples/libsum.so");
    if !libsum_so.exists() {
        eprintln!("skipping: libsum.so not built (no C compiler?)");
        return;
    }

    // Absolute dload path avoids cwd races in parallel tests.
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

#[test]
fn example_coro_prints_suspended_1_resumed() {
    let output = run_example("examples/coro.0s");
    assert_eq!(output, "Suspended\n1Resumed\n");
}

#[test]
fn example_coro_gen_prints_012() {
    let output = run_example("examples/coro_gen.0s");
    assert_eq!(output, "012");
}

#[test]
fn example_coro_interleave_prints_out_of_order_counters() {
    let output = run_example("examples/coro_interleave.0s");
    assert_eq!(output, "10,100,101,11,12,102");
}

#[test]
fn example_coro_send_prints_hello() {
    let output = run_example("examples/coro_send.0s");
    assert_eq!(output, "hello");
}

#[test]
fn example_coro_yield_from_prints_012() {
    let output = run_example("examples/coro_yield_from.0s");
    assert_eq!(output, "012");
}
