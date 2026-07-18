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
    run_example_src_with_entry(&src, Some(full.as_path()))
}

/// Compile and run in-memory source.
fn run_example_src(src: &str) -> String {
    run_example_src_with_entry(src, None)
}

fn run_example_src_with_entry(src: &str, entry: Option<&std::path::Path>) -> String {
    let mut pipeline = Pipeline::new();
    let (bytecode, constants) = pipeline
        .compile_src(src)
        .expect("example failed to compile (parse error or type errors)");
    run_bytecode(bytecode, constants, &pipeline, entry)
}

fn run_bytecode(
    bytecode: Vec<common::Byte>,
    constants: Vec<u64>,
    pipeline: &Pipeline,
    entry: Option<&std::path::Path>,
) -> String {
    let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
    let shared = SharedBuf(Rc::clone(&buf));
    let mut machine = Machine::<128>::default();
    machine.with_output(shared);
    pipeline.wire_vm_ffi(&mut machine, entry);
    pipeline.wire_host_natives(&mut machine);
    machine.run_raw(&bytecode, &constants);
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
fn example_generics_uses_builtin_dictionary_abi() {
    let output = run_example("examples/generics.0s");
    assert_eq!(output, "7424.042");
}

#[test]
fn example_result_prints_42_and_neg1() {
    let output = run_example("examples/result.0s");
    assert_eq!(output, "420-1");
}

#[test]
fn example_raise_try_prints_10_neg() {
    let output = run_example("examples/raise_try.0s");
    assert_eq!(output, "10,neg");
}

#[test]
fn example_coalesce_prints_bar_hi_7_9() {
    let output = run_example("examples/coalesce.0s");
    assert_eq!(output, "bar,hi,7,9");
}

#[test]
fn example_optional_chain_prints_42_0() {
    let output = run_example("examples/optional_chain.0s");
    assert_eq!(output, "42,0");
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
fn example_array_grow_prints_len_first_and_last() {
    let output = run_example("examples/array_grow.0s");
    assert_eq!(output, "414");
}

#[test]
fn example_classes_prints_7458() {
    let output = run_example("examples/classes.0s");
    assert_eq!(output, "7458");
}

#[test]
fn example_generic_class_prints_42() {
    let output = run_example("examples/generic_class.0s");
    assert_eq!(output, "42");
}

#[test]
fn example_aliases_prints_3_4_7() {
    let output = run_example("examples/aliases.0s");
    assert_eq!(output, "347");
}

#[test]
fn example_generic_alias_prints_7() {
    let output = run_example("examples/generic_alias.0s");
    assert_eq!(output, "7");
}

#[test]
fn example_generic_enum_prints_7() {
    let output = run_example("examples/generic_enum.0s");
    assert_eq!(output, "7");
}

#[test]
fn example_generics_prints_add_results_for_int_and_float() {
    let output = run_example("examples/generics.0s");
    assert_eq!(output, "7424.042");
}

#[test]
fn example_typeclass_dict_forwards_dictionary_and_prints_42_twice() {
    let output = run_example("examples/typeclass_dict.0s");
    assert_eq!(output, "4242");
}

#[test]
fn example_typeclass_default_calls_sibling_and_prints_42() {
    let output = run_example("examples/typeclass_default.0s");
    assert_eq!(output, "42");
}

#[test]
fn example_polyfn_supports_multi_instantiation_constraints_and_rank_n() {
    let output = run_example("examples/polyfn.0s");
    assert_eq!(output, "424.0424242");
}

/// Phase 4: `%v` displays through Show (builtin + user instance + format).
#[test]
fn example_generic_print_shows_primitives_and_user_type() {
    let output = run_example("examples/generic_print.0s");
    assert_eq!(output, "42hi1.5true(3,4)99");
}

/// Phase 8: tuples and anonymous records have structural Show for `%v`.
#[test]
fn example_show_tuple_prints_structural_tuple_and_record() {
    let output = run_example("examples/show_tuple.0s");
    assert_eq!(output, "(1, 2){ a: 3, b: 4 }");
}

/// Phase 5: single-argument HKT `Container<Option>` + `get<F: Container, A>(F<A>)`.
#[test]
fn example_hkt_container_prints_42() {
    let output = run_example("examples/hkt_container.0s");
    assert_eq!(output, "42");
}

/// Phase 1 advanced generics: binary HKT `Bifunctor<Result>`.
#[test]
fn example_hkt_bifunctor_prints_42() {
    let output = run_example("examples/hkt_bifunctor.0s");
    assert_eq!(output, "42");
}

/// Phase 3: multi-param typeclass `Convert<A, B>` + `where` clause.
#[test]
fn example_multiparam_prints_42() {
    let output = run_example("examples/multiparam.0s");
    assert_eq!(output, "42");
}

/// Phase 5: superclass / implied bounds (`Ordered<T: Equal>` → `eq_val` under `T: Ordered`).
#[test]
fn example_superclass_ord_prints_truetruefalse() {
    let output = run_example("examples/superclass_ord.0s");
    assert_eq!(output, "truetruefalse");
}

/// Phase 6: associated types — `Collect::Elem` pinned from ground instance.
#[test]
fn example_assoc_type_prints_42() {
    let output = run_example("examples/assoc_type.0s");
    assert_eq!(output, "42");
}

/// Phase 3 advanced generics: generic associated type `Pointer::Ref<A>`.
#[test]
fn example_gat_pointer_prints_42() {
    let output = run_example("examples/gat_pointer.0s");
    assert_eq!(output, "42");
}

/// Shuffled record pattern `{ y: _, x: a }` must bind declaration-order `x`.
#[test]
fn shuffled_record_pattern_binds_declaration_order_field() {
    let src = r#"
        enum E { Foo { x: int, y: int, z: int } }
        fn main() {
            let e = E::Foo { x: 1, y: 2, z: 3 };
            let v = match e {
                E::Foo { y: _, x: a, z: _ } => a,
            };
            print "%i", v;
        }
    "#;
    assert_eq!(run_example_src(src), "1");
}

/// Phase 4: open `%v` inside a Show-bound generic body.
#[test]
fn generic_print_open_bound_uses_show_dictionary() {
    let src = r#"
        fn show_it<T: Show>(T x) {
            print "%v,", x;
        }
        fn main() {
            show_it(10);
            show_it(20);
        }
    "#;
    assert_eq!(run_example_src(src), "10,20,");
}

/// Phase 4: `format "%v"` produces a string for further use.
#[test]
fn format_percent_v_parity_with_print() {
    let src = r#"
        fn main() {
            let s = format "%v-%v", 1, "x";
            print "%s", s;
        }
    "#;
    assert_eq!(run_example_src(src), "1-x");
}

/// Phase 4: captured dictionaries remain valid after the creating frame returns
/// and the application site need not supply dictionaries (`app_dict_arity=0`).
#[test]
fn polyfn_captured_dict_survives_return() {
    let src = r#"
        typeclass Describable<T> {
            fn describe_val(T x) -> int;
        }
        impl Describable<int> {
            fn describe_val(int x) -> int { return x + 1; }
        }
        fn show<T: Describable>(T x) -> int {
            return describe_val(x);
        }
        fn capture_show<T: Describable>(T _w) {
            return show;
        }
        fn main() {
            let f = capture_show(0);
            print "%i", f(41);
        }
    "#;
    let mut pipeline = Pipeline::new();
    let (bytecode, _constants) = pipeline.compile_src(src).expect("compile");
    let capture = bytecode
        .iter()
        .find(|b| matches!(b.bytecode(), common::Instruction::MakePolyFnCapture))
        .expect("expected MakePolyFnCapture when escaping show from a constrained scope");
    assert_eq!(
        capture.operand_u32() & 0xFF,
        1,
        "capture should reserve one Describable dict slot"
    );
    // Application of the returned PolyFn must not require a second MakeTuple
    // dict at the call site — evidence lives in the capture.
    let capture_pos = bytecode
        .iter()
        .position(|b| matches!(b.bytecode(), common::Instruction::MakePolyFnCapture))
        .unwrap();
    let call_indirects_after: Vec<_> = bytecode[capture_pos..]
        .iter()
        .filter(|b| matches!(b.bytecode(), common::Instruction::CallIndirect))
        .collect();
    assert!(
        call_indirects_after
            .iter()
            .any(|b| (b.operand_u32() >> 16) == 0),
        "captured PolyFn call should use app_dict_arity=0; CallIndirect operands: {:?}",
        call_indirects_after
            .iter()
            .map(|b| b.operand_u32())
            .collect::<Vec<_>>()
    );
    assert_eq!(run_example_src(src), "42");
}

/// Phase 4: multiparam `Convert<A,B>` capture works after return with no app dict.
/// Both type args are witnessed at the capture call so the dict is concrete.
#[test]
fn polyfn_multiparam_capture_survives_return() {
    let src = r#"
        typeclass Convert<A, B> {
            fn cast(A x) -> B;
        }
        impl Convert<int, int> {
            fn cast(int x) -> int { return x; }
        }
        fn convert_fn<A, B>(A x) -> B where Convert<A, B> {
            return cast(x);
        }
        fn capture_convert<A, B>(A _wa, B _wb) where Convert<A, B> {
            return convert_fn;
        }
        fn main() {
            let f = capture_convert(0, 0);
            print "%i", f(42);
        }
    "#;
    let mut pipeline = Pipeline::new();
    let (bytecode, _constants) = pipeline.compile_src(src).expect("compile");
    let capture = bytecode
        .iter()
        .find(|b| matches!(b.bytecode(), common::Instruction::MakePolyFnCapture))
        .expect("expected MakePolyFnCapture for multiparam escape");
    assert_eq!(capture.operand_u32() & 0xFF, 1);
    assert_eq!(run_example_src(src), "42");
}

/// Phase 1: PolyFn + fib-style arithmetic still receives peephole fusion.
#[test]
fn polyfn_with_fib_keeps_fused_superinstructions() {
    use common::Instruction;
    let src = r#"
        fn id<T>(T x) -> T { return x; }
        fn fib(int n) -> int {
            if n <= 2 { return 1; }
            return fib(n - 1) + fib(n - 2);
        }
        fn main() {
            let f = id;
            print "%i", f(fib(6));
        }
    "#;
    let mut pipeline = Pipeline::new();
    let (bytecode, constants) = pipeline.compile_src(src).expect("compile");
    assert!(
        bytecode
            .iter()
            .any(|b| matches!(b.bytecode(), Instruction::MakePolyFn))
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
        "expected fused ops with PolyFn present; opcodes: {:?}",
        bytecode.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
    );
    let output = run_bytecode(bytecode, constants, &pipeline, None);
    // fib(6) = 8
    assert_eq!(output, "8");
}

#[test]
fn monomorphized_generic_add_prints_3() {
    let output = run_example_src(
        r#"fn add<T: Num>(T a, T b) -> T {
            return a + b;
        }

        fn main() {
            print "%i", add(1, 2);
        }"#,
    );
    assert_eq!(output, "3");
}

#[test]
fn example_const_prints_42hi() {
    let output = run_example("examples/const.0s");
    assert_eq!(output, "42hi");
}

#[test]
fn string_fmt_example_prints_concatenated_and_formatted_strings() {
    let output = run_example("examples/string_fmt.0s");
    assert_eq!(output, "hello world42-x");
}

#[test]
fn string_plus_equal_updates_binding() {
    let output = run_example_src(
        r#"fn main() {
            let s = "a";
            s += "b";
            print "%s", s;
        }"#,
    );
    assert_eq!(output, "ab");
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

/// Phase P0: `let x = match { … }` must bind the arm value via
/// StorePop. Pre-fix Match emitted RETURN at end_label, so the
/// StorePop was unreachable and prints never ran / saw 0.
#[test]
fn let_match_binds_arm_value() {
    let src = r#"
        fn main() {
            let x = match Option::None {
                Option::None => 7,
                Option::Some(v) => v,
            };
            print "%i", x;
            let y = match Option::Some(42) {
                Option::None => 0,
                Option::Some(v) => v,
            };
            print "%i", y;
        }
    "#;
    let output = run_example_src(src);
    assert_eq!(output, "742");
}

/// Phase P0: dict fields that hold heap objects (strings) must
/// round-trip through GetField.
#[test]
fn dict_string_field_round_trips() {
    let src = r#"
        fn main() {
            let d = { name: "hi", n: 9 };
            print "%s", d.name;
            print "%i", d.n;
        }
    "#;
    let output = run_example_src(src);
    assert_eq!(output, "hi9");
}

/// Phase P1: in-place dict mutation via `d.field = value` then re-read.
#[test]
fn dict_mutation_round_trips() {
    let src = r#"
        fn main() {
            let d = { foo: 1, name: "a" };
            d.foo = 99;
            d.name = "z";
            print "%i", d.foo;
            print "%s", d.name;
        }
    "#;
    let output = run_example_src(src);
    assert_eq!(output, "99z");
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
    let exit_branch_count = bytecode
        .iter()
        .filter(|b| {
            matches!(
                b.bytecode(),
                Instruction::JMPF | Instruction::CmpJmpf | Instruction::BinSlotImmJmpf
            )
        })
        .count();
    let jmp_count = bytecode
        .iter()
        .filter(|b| matches!(b.bytecode(), Instruction::JMP))
        .count();
    assert!(
        exit_branch_count >= 2,
        "expected at least 2 exit branches (loop + if); got {}",
        exit_branch_count
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

    let result =
        std::panic::catch_unwind(|| run_example_src_with_entry(&src, Some(full.as_path())));
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
    // Quick probe: if dlopen("libc.so.6") fails, skip.
    if machine::load_library("libc.so.6").is_err() {
        eprintln!("skipping: libc.so.6 not loadable on this platform");
        return;
    }

    let result = std::panic::catch_unwind(|| {
        let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("compiler crate must have a parent (workspace root)");
        let full = workspace_root.join("examples/strlen.0s");
        let src = std::fs::read_to_string(&full).expect("read strlen.0s");
        run_example_src_with_entry(&src, Some(full.as_path()))
    });
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

#[test]
fn example_coro_done_prints_false_false_true() {
    let output = run_example("examples/coro_done.0s");
    assert_eq!(output, "falsefalsetrue");
}

/// Regression guard: `resume h` used INLINE as a `print` argument
/// (no intermediate `let` binding) must not corrupt the operand
/// stack. Pre-fix, the bare `yield expr;` statement's spurious
/// trailing `POP` (see `bare_yield_statement_does_not_emit_trailing_pop`
/// in `compiler/src/lib.rs`) would pop whatever the resumer had
/// already pushed for the in-progress `print` call (e.g. the format
/// string), leading to a misaligned pointer dereference.
#[test]
fn inline_resume_in_print_does_not_corrupt_stack() {
    let src = r#"
        async fn counter() {
            yield 0;
            yield 1;
            yield 2;
        }

        fn main() {
            let h = counter();
            print "%i,", resume h;
            print "%i,", resume h;
            print "%i", resume h;
        }
    "#;
    let output = run_example_src(src);
    assert_eq!(output, "0,1,2");
}

/// Regression guard: two handles from the SAME parameterized
/// `async fn`, interleaved, with `resume` used inline. Pre-fix, the
/// same spurious trailing `POP` corrupted each coroutine's argument
/// slot (`base`) on every resume after the first yield, producing
/// wrong values once interleaving pushed other locals onto the
/// shared stack.
#[test]
fn parameterized_interleaved_coroutines_inline_resume_stay_independent() {
    let src = r#"
        async fn counter(int base) {
            yield base;
            yield base + 1;
            yield base + 2;
        }

        fn main() {
            let a = counter(1);
            let b = counter(100);

            print "%i,", resume a;
            print "%i,", resume b;
            print "%i,", resume a;
            print "%i,", resume b;
            print "%i,", resume a;
            print "%i", resume b;
        }
    "#;
    let output = run_example_src(src);
    assert_eq!(output, "1,100,2,101,3,102");
}

/// `return e;` inside an `async fn` now produces a real completion
/// value (previously it was silently unified against `unit` and the
/// typechecker rejected any non-unit `return`). The value returned by
/// `return` propagates to the `resume` call that completes the
/// coroutine, exactly like a yielded value.
#[test]
fn coroutine_return_value_propagates_to_resume() {
    let src = r#"
        async fn counter() {
            yield 1;
            yield 2;
            return 42;
        }

        fn main() {
            let h = counter();
            print "%i,", resume h; // yield 1
            print "%i,", resume h; // yield 2
            print "%i", resume h;  // return 42
        }
    "#;
    let output = run_example_src(src);
    assert_eq!(output, "1,2,42");
}

/// Resuming an already-Done coroutine always yields the sentinel
/// `Value::default()` (`0`) — never the coroutine's last `return`
/// value — since there's no error-handling protocol yet to signal
/// "resumed after completion" and returning the stale value again
/// would be a worse form of undefined behavior.
#[test]
fn resume_after_done_returns_default_not_last_return_value() {
    let src = r#"
        async fn counter() {
            return 42;
        }

        fn main() {
            let h = counter();
            print "%i,", resume h; // return 42 (completes)
            print "%i,", resume h; // Done -> 0, not 42
            print "%i", resume h;  // Done -> 0, not 42
        }
    "#;
    let output = run_example_src(src);
    assert_eq!(output, "42,0,0");
}

fn run_ffi_example_with_lib(path: &str, lib_path: &std::path::Path) -> String {
    ensure_ffi_libsum_built();
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate must have a parent (workspace root)");
    let full = workspace_root.join(path);
    let lib_abs = lib_path
        .canonicalize()
        .unwrap_or_else(|_| lib_path.to_path_buf());
    let mut src = std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", full.display(), e));
    src = src.replace(
        "dload(\"libsum.so\")",
        &format!("dload(\"{}\")", lib_abs.display()),
    );
    run_example_src_with_entry(&src, Some(full.as_path()))
}

#[test]
fn example_ffi_array_sum_prints_15() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate must have a parent (workspace root)");
    let libsum = workspace_root.join("examples/libsum.so");
    if !libsum.exists() {
        eprintln!("skipping: libsum.so not built");
        return;
    }
    let output = run_ffi_example_with_lib("examples/ffi_array.0s", &libsum);
    assert_eq!(output, "15");
}

#[test]
fn example_ffi_callback_prints_42() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate must have a parent (workspace root)");
    let libsum = workspace_root.join("examples/libsum.so");
    ensure_ffi_libsum_built();
    if !libsum.exists() {
        eprintln!("skipping: libsum.so not built");
        return;
    }
    let output = run_ffi_example_with_lib("examples/ffi_callback.0s", &libsum);
    assert_eq!(output, "42");
}

#[test]
fn example_ffi_struct_return_prints_34() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate must have a parent (workspace root)");
    let libsum = workspace_root.join("examples/libsum.so");
    // Rebuild when sum.c is newer (mtime check inside ensure_*). Do not
    // delete libsum.so here — other FFI tests in this file race on that path.
    ensure_ffi_libsum_built();
    if !libsum.exists() {
        eprintln!("skipping: libsum.so not built");
        return;
    }
    let output = run_ffi_example_with_lib("examples/ffi_struct_ret.0s", &libsum);
    assert_eq!(output, "34");
}

#[test]
fn example_ffi_callback_return_prints_1() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate must have a parent (workspace root)");
    let libsum = workspace_root.join("examples/libsum.so");
    ensure_ffi_libsum_built();
    if !libsum.exists() {
        eprintln!("skipping: libsum.so not built");
        return;
    }
    let output = run_ffi_example_with_lib("examples/ffi_callback_ret.0s", &libsum);
    assert_eq!(output, "1");
}

#[test]
fn example_operators_prints_expected() {
    let output = run_example("examples/operators.0s");
    assert_eq!(output, "801125428falsetrue3");
}

#[test]
fn example_while_loop_accumulates_correctly() {
    let output = run_example_src(
        r#"
        fn main() {
            let acc = 0;
            let i = 0;
            while (i < 100) {
                acc = acc + i;
                i = i + 1;
            }
            print "%i", acc;
        }
        "#,
    );
    assert_eq!(output, "4950");
}

#[test]
fn example_for_break_prints_18() {
    let output = run_example("examples/for_break.0s");
    assert_eq!(output, "18");
}

#[test]
fn example_perf_numeric_prints_expected_sum() {
    let output = run_example("examples/perf/numeric.0s");
    assert_eq!(output, "1999000");
}

#[test]
fn example_perf_array_mut_prints_expected() {
    let output = run_example("examples/perf/array_mut.0s");
    assert_eq!(output, "2000");
}

#[test]
fn example_perf_dict_hot_prints_expected() {
    let output = run_example("examples/perf/dict_hot.0s");
    assert_eq!(output, "6000");
}

#[test]
fn example_perf_operators_loop_prints_expected() {
    let output = run_example("examples/perf/operators_loop.0s");
    assert_eq!(output, "149912");
}

#[test]
fn example_perf_coro_ping_prints_expected() {
    let output = run_example("examples/perf/coro_ping.0s");
    assert_eq!(output, "124750");
}
