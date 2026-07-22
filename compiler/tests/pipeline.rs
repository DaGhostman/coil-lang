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

/// Soft-skip an FFI-dependent test outside CI. In CI (`CI` env set), skip is a
/// hard failure so missing `cc` / libffi never silently greens the suite.
fn ffi_soft_skip(reason: &str) {
    if std::env::var_os("CI").is_some() {
        panic!("FFI soft-skip forbidden in CI: {reason}");
    }
    eprintln!("skipping: {reason}");
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
    assert_eq!(output, "7424.0427");
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
fn example_assert_prints_ok_assertion_failed_custom() {
    let output = run_example("examples/assert.0s");
    assert_eq!(output, "ok,assertion failed,custom");
}

#[test]
fn panic_aborts_and_writes_message() {
    let mut pipeline = Pipeline::new();
    let (bytecode, constants) = pipeline
        .compile_src(
            r#"
fn main() {
    panic "boom";
}
"#,
        )
        .expect("panic example should compile");
    let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
    let shared = SharedBuf(Rc::clone(&buf));
    let mut machine = Machine::<128>::default();
    machine.with_output(shared);
    machine.run_raw(&bytecode, &constants);
    assert!(machine.panicked(), "expected language-level panic");
    let _ = machine.restore_output();
    let bytes = Rc::try_unwrap(buf)
        .expect("VM still holds a reference to the buffer")
        .into_inner();
    let s = String::from_utf8(bytes).expect("captured output should be valid UTF-8");
    assert_eq!(s, "panic: boom");
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
    assert_eq!(output, "55");
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
    assert_eq!(output, "7424.0427");
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

/// Advanced generics Phase 4: a bare unary trait name is an existential type.
#[test]
fn example_existential_show_prints_42() {
    let output = run_example("examples/existential_show.0s");
    assert_eq!(output, "42");
}

/// Phase 8: tuples and anonymous records have structural Show for `%v`.
#[test]
fn example_show_tuple_prints_structural_tuple_and_record() {
    let output = run_example("examples/show_tuple.0s");
    assert_eq!(output, "(1, 2){ a: 3, b: 4 }");
}

/// Constructor-kind trait `Container<Option>` + `get<F: Container, A>(F<A>)`.
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

/// Phase 3: multi-param trait `Convert<A, B>` + `where` clause.
#[test]
fn example_multiparam_prints_42() {
    let output = run_example("examples/multiparam.0s");
    assert_eq!(output, "42");
}

/// Prelude `Into`: `let f: Fahrenheit = c.into();` with two local classes.
#[test]
fn example_into_prints_32() {
    let output = run_example("examples/into.0s");
    assert_eq!(output, "32");
}

/// Inline receiver `new Celsius(0).into()` must typecheck and run (Bugbot:
/// codegen used to skip boxing when `receiver_type` only handled Identifier/Access).
#[test]
fn inline_into_receiver_prints_32() {
    let src = r#"
class Celsius { c: int }
class Fahrenheit { f: int }
impl Into<Fahrenheit> for Celsius {
    fn into(Celsius x) -> Fahrenheit {
        return new Fahrenheit(x.c * 2 + 32);
    }
}
fn main() {
    let f: Fahrenheit = new Celsius(0).into();
    print "%i", f.f;
}
"#;
    let output = run_example_src(src);
    assert_eq!(output, "32");
}

/// `return c.into();` under `-> Fahrenheit` pins the Into target at runtime.
#[test]
fn return_into_pins_target_prints_32() {
    let src = r#"
class Celsius { c: int }
class Fahrenheit { f: int }
class Kelvin { k: int }
impl Into<Fahrenheit> for Celsius {
    fn into(Celsius x) -> Fahrenheit {
        return new Fahrenheit(x.c * 2 + 32);
    }
}
impl Into<Kelvin> for Celsius {
    fn into(Celsius x) -> Kelvin {
        return new Kelvin(x.c);
    }
}
fn to_f(Celsius c) -> Fahrenheit {
    return c.into();
}
fn main() {
    let f = to_f(new Celsius(0));
    print "%i", f.f;
}
"#;
    let output = run_example_src(src);
    assert_eq!(output, "32");
}

/// Phase 5: superclass / implied bounds (`Ordered<T: Equal>` → `eq_val` under `T: Ordered`).
#[test]
fn example_superclass_ord_prints_truetruefalse() {
    let output = run_example("examples/superclass_ord.0s");
    assert_eq!(output, "truetruefalse");
}

/// Advanced generics Phase 5: `c: * -> Constraint, T: c` with superclass method use.
#[test]
fn example_constraint_kind_prints_42() {
    let output = run_example("examples/constraint_kind.0s");
    assert_eq!(output, "42");
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
        trait Describable<T> {
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
        trait Convert<A, B> {
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
    let mut ast = parser.parse(&src).expect("result.0s should parse");
    let (bytecode, constants) = pipeline.compile_test("", &mut ast);

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
    let mut ast = parser.parse(&src).expect("fizbuz.0s should parse");
    let (bytecode, constants) = pipeline.compile_test("", &mut ast);

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
    let mut ast = parser.parse(src).expect("let-binding program should parse");
    let (bytecode, _constants) = pipeline.compile_test("", &mut ast);
    assert!(!bytecode.is_empty(), "program should produce bytecode");

    let binding_store_pop_count = bytecode
        .iter()
        .filter(|b| matches!(b.bytecode(), Instruction::StorePop) && b.operand_u32() == 0)
        .count();
    assert_eq!(
        binding_store_pop_count, 2,
        "expected exactly 2 StorePop writes to binding slot 0 for one let + one re-assignment; got {}",
        binding_store_pop_count
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
fn example_named_args_prints_ada36_grace40() {
    let output = run_example("examples/named_args.0s");
    assert_eq!(output, "Ada36Grace40");
}

/// Critical regression: shuffled named args must reorder to declaration
/// order at runtime. Happy-path goldens only exercise source-order and
/// positional-prefix forms; a missing codegen reorder would still typecheck
/// here (string then int) but print the wrong values.
#[test]
fn named_args_shuffled_order_prints_correct_values() {
    let output = run_example_src(
        r#"
fn greet(string name, int age) {
    print "%s", name;
    print "%i", age;
}

fn main() {
    greet(age: 36, name: "Ada");
    greet(age: 40, name: "Grace");
}
"#,
    );
    assert_eq!(output, "Ada36Grace40");
}

/// Rest packing with a named fixed prefix + trailing positionals (P4 + P2).
#[test]
fn rest_after_named_fixed_prefix_packs_trailing() {
    let output = run_example_src(
        r#"
fn f(int a, int... xs) -> int {
    return a + len(xs);
}

fn main() {
    print "%i", f(a: 10, 1, 2, 3);
    print "%i", f(a: 7);
}
"#,
    );
    assert_eq!(output, "137");
}

#[test]
fn example_let_destructure_prints_12342() {
    let output = run_example("examples/let_destructure.0s");
    assert_eq!(output, "12342");
}

/// Nested let destructure must bind inner tuple slots correctly (not swap).
#[test]
fn let_nested_tuple_destructure_binds_in_order() {
    let output = run_example_src(
        r#"
fn main() {
    let (a, (b, c)) = (1, (2, 3));
    print "%i", a;
    print "%i", b;
    print "%i", c;
}
"#,
    );
    assert_eq!(output, "123");
}

#[test]
fn example_variadic_prints_60_hi() {
    let output = run_example("examples/variadic.0s");
    assert_eq!(output, "60Hi!?");
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
    let mut ast = parser
        .parse(src)
        .expect("chained-bindings program should parse");
    let (bytecode, constants) = pipeline.compile_test("", &mut ast);

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
    let mut ast = parser.parse(src).expect("nested if-in-loop should parse");
    let (bytecode, _constants) = pipeline.compile_test("", &mut ast);
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

fn ensure_ffi_libsum_built() -> std::path::PathBuf {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate must have a parent (workspace root)");
    let sum_c = workspace_root.join("examples/sum.c");
    let lib_name = machine::platform_shared_lib_filename("sum");
    let libsum = workspace_root.join("examples").join(&lib_name);

    // Always rebuild if the source is newer than the shared lib, or
    // if the shared lib doesn't exist.
    let needs_build = match (sum_c.metadata(), libsum.metadata()) {
        (Ok(src_meta), Ok(so_meta)) => src_meta.modified().ok() > so_meta.modified().ok(),
        (Ok(_), Err(_)) => true,
        _ => false,
    };
    if !needs_build && libsum.exists() {
        return libsum;
    }

    let mut cmd = std::process::Command::new("cc");
    #[cfg(target_os = "macos")]
    {
        cmd.arg("-dynamiclib");
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        cmd.arg("-shared").arg("-fPIC");
    }
    #[cfg(target_os = "windows")]
    {
        cmd.arg("-shared");
    }
    let status = cmd
        .arg("-O2")
        .arg("-o")
        .arg(&libsum)
        .arg(&sum_c)
        .status();
    match status {
        Ok(s) if s.success() => {
            if let Ok(meta) = std::fs::metadata(&libsum)
                && meta.len() < 256
            {
                eprintln!(
                    "warning: {} looks truncated ({} bytes) after cc build",
                    libsum.display(),
                    meta.len()
                );
            }
        }
        Ok(s) => {
            ffi_soft_skip(&format!(
                "FFI tests: cc returned non-zero status {}",
                s.code().unwrap_or(-1)
            ));
        }
        Err(e) => {
            ffi_soft_skip(&format!("FFI tests: failed to invoke cc: {e}"));
        }
    }
    libsum
}

#[test]
fn example_ffi_sum_via_dlopen_prints_42() {
    let libsum = ensure_ffi_libsum_built();
    if !libsum.exists() {
        ffi_soft_skip(&format!("{} not built (no C compiler?)", libsum.display()));
        return;
    }

    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate must have a parent (workspace root)");

    // Absolute dload path avoids cwd races in parallel tests.
    let full = workspace_root.join("examples/ffi_sum.0s");
    let lib_abs = libsum
        .canonicalize()
        .unwrap_or_else(|_| libsum.clone());
    let mut src = std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", full.display(), e));
    src = src.replace(
        "dload(\"sum\")",
        &format!("dload(\"{}\")", lib_abs.display()),
    );

    let result =
        std::panic::catch_unwind(|| run_example_src_with_entry(&src, Some(full.as_path())));
    let output = match result {
        Ok(s) => s,
        Err(_) => {
            ffi_soft_skip("FFI test panicked (dlopen failure?)");
            return;
        }
    };
    assert_eq!(output, "42", "sum(40, 2) via userland FFI should print 42");
}

#[test]
fn example_strlen_prints_5() {
    // Quick probe: if the portable `c` alias fails, skip outside CI.
    if machine::resolve_library("c", None, &[]).is_err() {
        ffi_soft_skip("C library not loadable on this platform via resolve_library(\"c\")");
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
            ffi_soft_skip("strlen test panicked (dlopen failure?)");
            return;
        }
    };
    assert_eq!(output, "5", "strlen(\"hello\") should print 5");
}

/// Serialize fd-1 redirection: parallel tests + libtest status lines share
/// process stdout, so nested `dup2` would corrupt capture.
#[cfg(unix)]
static OS_STDOUT_CAPTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Restores process stdout (fd 1) on drop — including panics inside `f`.
#[cfg(unix)]
struct StdoutFdGuard {
    old_stdout: i32,
}

#[cfg(unix)]
impl Drop for StdoutFdGuard {
    fn drop(&mut self) {
        if self.old_stdout < 0 {
            return;
        }
        unsafe {
            libc::fflush(std::ptr::null_mut());
            let _ = libc::dup2(self.old_stdout, 1);
            libc::close(self.old_stdout);
        }
        self.old_stdout = -1;
    }
}

/// Capture bytes written to OS stdout (fd 1) while `f` runs.
/// Needed for libc `printf`, which bypasses the VM's `PRINT` sink.
#[cfg(unix)]
fn with_captured_os_stdout<R>(f: impl FnOnce() -> R) -> (R, String) {
    use std::io::Read;
    use std::os::fd::FromRawFd;

    let _lock = OS_STDOUT_CAPTURE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let (read_fd, guard) = unsafe {
        let mut pipefd = [0i32; 2];
        assert_eq!(libc::pipe(pipefd.as_mut_ptr()), 0);
        let read_fd = pipefd[0];
        let write_fd = pipefd[1];
        let old_stdout = libc::dup(1);
        assert!(old_stdout >= 0);
        assert_eq!(libc::dup2(write_fd, 1), 1);
        libc::close(write_fd);
        (read_fd, StdoutFdGuard { old_stdout })
    };

    // Catch panics so we can restore fd 1 (via `guard`) before rethrowing —
    // otherwise later tests inherit a broken stdout.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    drop(guard);

    let mut file = unsafe { std::fs::File::from_raw_fd(read_fd) };
    let mut buf = Vec::new();
    let _ = file.read_to_end(&mut buf);
    drop(file);

    let s = String::from_utf8_lossy(&buf).into_owned();
    match result {
        Ok(r) => (r, s),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// Drop noise that lands in a process-wide stdout pipe under `cargo test`
/// parallelism: debug heap traces and libtest harness status lines.
#[cfg(unix)]
fn clean_captured_os_stdout(output: &str) -> String {
    output
        .lines()
        .filter(|l| {
            let t = l.trim();
            if t.is_empty() {
                return false;
            }
            // Debug heap: `0x… alloc …` / `0x… free …`
            if t.contains(" alloc ") || t.contains(" free ") {
                return false;
            }
            // libtest: `test foo::bar ... ok` (other threads finish mid-capture)
            if t.starts_with("test ") && t.contains(" ... ") {
                return false;
            }
            true
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn example_ffi_printf_prints_hello_42() {
    if machine::resolve_library("c", None, &[]).is_err() {
        ffi_soft_skip("C library not loadable on this platform via resolve_library(\"c\")");
        return;
    }

    #[cfg(not(unix))]
    {
        ffi_soft_skip("ffi_printf OS-stdout capture is unix-only");
        return;
    }

    #[cfg(unix)]
    {
        let result = std::panic::catch_unwind(|| {
            let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("compiler crate must have a parent (workspace root)");
            let full = workspace_root.join("examples/ffi_printf.0s");
            let src = std::fs::read_to_string(&full).expect("read ffi_printf.0s");
            let ((), os_out) = with_captured_os_stdout(|| {
                let _vm_out = run_example_src_with_entry(&src, Some(full.as_path()));
            });
            os_out
        });
        let output = match result {
            Ok(s) => s,
            Err(_) => {
                ffi_soft_skip("ffi_printf test panicked (dlopen failure?)");
                return;
            }
        };
        let cleaned = clean_captured_os_stdout(&output);
        assert_eq!(
            cleaned.trim(),
            "hello 42",
            "printf(\"hello %lld\", 42) should write to OS stdout (raw={output:?})"
        );
    }
}

#[test]
fn extern_missing_library_panics_with_message() {
    let src = r#"
extern "this_library_definitely_does_not_exist_xyzzy" {
    fn noop() -> int;
}
fn main() {
    print "%i", noop();
}
"#;
    let mut pipeline = Pipeline::new();
    let (bytecode, constants) = pipeline
        .compile_src(src)
        .expect("should compile (load failure is runtime)");
    let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
    let shared = SharedBuf(Rc::clone(&buf));
    let mut machine = Machine::<128>::default();
    machine.with_output(shared);
    pipeline.wire_vm_ffi(&mut machine, None);
    pipeline.wire_host_natives(&mut machine);
    machine.run_raw(&bytecode, &constants);
    assert!(
        machine.panicked(),
        "missing library should panic, not segfault"
    );
    let _ = machine.restore_output();
    let bytes = Rc::try_unwrap(buf)
        .expect("VM still holds a reference to the buffer")
        .into_inner();
    let output = String::from_utf8(bytes).expect("utf-8");
    assert!(
        output.contains("panic:") && output.contains("not found"),
        "expected panic message about missing library, got: {output:?}"
    );
}

#[test]
fn userland_dload_missing_library_returns_err() {
    let src = r#"
use ffi::*;
fn main() {
    let r = dload("this_library_definitely_does_not_exist_xyzzy");
    let msg = match r {
        Result::Ok(_) => "ok",
        Result::Err(e) => match e.kind {
            ErrorKind::LibraryNotFound => "missing",
            _ => "other",
        },
    };
    print "%s", msg;
}
"#;
    let output = run_example_src(src);
    assert_eq!(output, "missing");
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

#[test]
fn example_for_in_coro_prints_012_and_breaks() {
    // counter yields 0,1,2 then returns 99 — completion must NOT print.
    // early yields 10,20,30 — break on 20 prints only 10.
    let output = run_example("examples/for_in_coro.0s");
    assert_eq!(output, "01210");
}

#[test]
fn example_for_in_array_prints_123() {
    let output = run_example("examples/for_in_array.0s");
    assert_eq!(output, "123");
}

#[test]
fn example_for_in_tuple_prints_123() {
    let output = run_example("examples/for_in_tuple.0s");
    assert_eq!(output, "123");
}

#[test]
fn example_for_in_dict_prints_12() {
    let output = run_example("examples/for_in_dict.0s");
    assert_eq!(output, "12");
}

#[test]
fn example_for_in_custom_prints_012() {
    let output = run_example("examples/for_in_custom.0s");
    assert_eq!(output, "012");
}

#[test]
fn example_range_prints_01234012356() {
    // 0..5 → 01234; 0..=3 → 0123; 10..0 empty; byte 5..=6 → 56;
    // float 1.0..4.0 → 1.02.03.0
    let output = run_example("examples/range.0s");
    assert_eq!(output, "012340123561.02.03.0");
}

/// Inner-block destructure must not clobber an outer binding's slot.
#[test]
fn let_destructure_block_shadow_preserves_outer_binding() {
    let output = run_example_src(
        r#"
enum A { A { z: int, x: int }, }
enum B { B { y: int }, }
fn main() {
  let outer = { a: A::A { z: 10, x: 42 } };
  let { a } = outer;
  { let inner = { a: B::B { y: 7 } }; let { a } = inner; }
  print "%i", a.x;
}
"#,
    );
    assert_eq!(output, "42", "outer `a` must survive inner-block shadow");
}

/// Rest-only generic with a typeclass constraint needs a call-site dict.
#[test]
fn generic_rest_only_show_call_emits_dict_and_prints() {
    let output = run_example_src(
        r#"
fn show_all<T: Show>(T... xs) {
    print "%v", xs[0];
}
fn main() {
    show_all(1);
}
"#,
    );
    assert_eq!(output, "1");
}

/// Rest-only Num generic must monomorphize (not print a boxed heap pointer).
#[test]
fn generic_rest_only_num_call_monomorphizes_and_prints() {
    let output = run_example_src(
        r#"
fn twice_first<T: Num>(T... xs) -> T { return xs[0] + xs[0]; }
fn main() { print "%i", twice_first(21); }
"#,
    );
    assert_eq!(output, "42");
}

/// Shadowing a function parameter inside a block must restore Access typing.
#[test]
fn block_shadow_of_param_restores_access_field_type() {
    let output = run_example_src(
        r#"
enum A { A { z: int, x: int }, }
enum B { B { y: int }, }
fn foo(A a) {
  { let a = B::B { y: 7 }; }
  print "%i", a.x;
}
fn main() { foo(A::A { z: 10, x: 42 }); }
"#,
    );
    assert_eq!(output, "42");
}

/// Half-open vs inclusive endpoints: `0..1` yields only 0; `0..=1` yields 0,1.
#[test]
fn range_half_open_excludes_end_inclusive_includes_end() {
    let output = run_example_src(
        r#"
fn main() {
    for x in 0..1 {
        print "%i", x;
    }
    print ",";
    for x in 0..=1 {
        print "%i", x;
    }
}
"#,
    );
    assert_eq!(output, "0,01");
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
        "dload(\"sum\")",
        &format!("dload(\"{}\")", lib_abs.display()),
    );
    run_example_src_with_entry(&src, Some(full.as_path()))
}

#[test]
fn example_ffi_array_sum_prints_15() {
    let libsum = ensure_ffi_libsum_built();
    if !libsum.exists() {
        ffi_soft_skip(&format!("{} not built", libsum.display()));
        return;
    }
    let output = run_ffi_example_with_lib("examples/ffi_array.0s", &libsum);
    assert_eq!(output, "15");
}

#[test]
fn example_ffi_callback_prints_42() {
    let libsum = ensure_ffi_libsum_built();
    if !libsum.exists() {
        ffi_soft_skip(&format!("{} not built", libsum.display()));
        return;
    }
    let output = run_ffi_example_with_lib("examples/ffi_callback.0s", &libsum);
    assert_eq!(output, "42");
}

#[test]
fn example_ffi_struct_return_prints_34() {
    let libsum = ensure_ffi_libsum_built();
    if !libsum.exists() {
        ffi_soft_skip(&format!("{} not built", libsum.display()));
        return;
    }
    let output = run_ffi_example_with_lib("examples/ffi_struct_ret.0s", &libsum);
    assert_eq!(output, "34");
}

#[test]
fn example_ffi_callback_return_prints_1() {
    let libsum = ensure_ffi_libsum_built();
    if !libsum.exists() {
        ffi_soft_skip(&format!("{} not built", libsum.display()));
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
fn example_derive_show_eq_prints_expected() {
    let output = run_example("examples/derive_show_eq.0s");
    assert_eq!(
        output,
        "Color::Red,true,false,true,Point::Point { x: 5, y: 12 },true,false,Cell { value: 42 },true,false"
    );
}

/// Regression: concrete `<`/`>` codegen must look up `Lt`/`Gt` (not empty
/// `Ord`), otherwise unit-enum compares fall back to raw heap-pointer `LE`
/// and become ASLR-flaky (`Red < Blue` randomly false).
#[test]
fn derive_ord_unit_variants_compare_by_declaration_order() {
    let src = r#"
enum Color derive Ord {
    Red,
    Blue,
}

fn main() {
    print "%z,", Color::Red < Color::Blue;
    print "%z,", Color::Blue < Color::Red;
    print "%z,", Color::Red < Color::Red;
    print "%z,", Color::Red <= Color::Red;
    print "%z", Color::Blue > Color::Red;
}
"#;
    for _ in 0..8 {
        let output = run_example_src(src);
        assert_eq!(
            output, "true,false,false,true,true",
            "unit-enum Ord must be tag-order stable (not pointer order)"
        );
    }
}

/// Regression: `derive Ord` must emit Lt/Le/Gt/Ge + empty Ord (PR #14 layout)
/// and lexicographic field compare must use strict `<` so equal prefixes fall
/// through (a Leq-primary fold would short-circuit on equal leading fields).
#[test]
fn derive_ord_record_payload_lexicographic_compare() {
    let src = r#"
enum Pair derive Ord {
    Pair { x: int, y: int },
}

fn main() {
    let a = Pair::Pair { x: 1, y: 2 };
    let b = Pair::Pair { x: 1, y: 3 };
    let c = Pair::Pair { x: 2, y: 0 };
    print "%z,", a < b;
    print "%z,", a < c;
    print "%z,", b < a;
    print "%z,", a <= a;
    print "%z", a < a;
}
"#;
    let output = run_example_src(src);
    assert_eq!(output, "true,true,false,true,false");
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

#[test]
fn example_io_bytes_prints_25532() {
    let output = run_example("examples/io_bytes.0s");
    assert_eq!(output, "25532");
}

#[test]
fn example_io_file_prints_2() {
    let output = run_example("examples/io_file.0s");
    assert_eq!(output, "2");
}

#[test]
fn example_io_eof_prints_eof() {
    let output = run_example("examples/io_eof.0s");
    assert_eq!(output, "eof");
}

#[test]
fn example_io_text_prints_hello2() {
    let output = run_example("examples/io_text.0s");
    assert_eq!(output, "hello2");
}

#[test]
fn example_io_udp_prints_2() {
    let output = run_example("examples/io_udp.0s");
    assert_eq!(output, "2");
}

/// Nested IO HostInvoke (`read_to_end(open(...)?)`) must leave the stream on
/// the stack as the MakeTuple element, not the outer native id.
#[test]
fn example_io_nested_host_prints_3() {
    let output = run_example("examples/io_nested_host.0s");
    assert_eq!(output, "3");
}

/// Nested IO as the first of two HostInvoke args (`write_all(open(...), buf)`).
/// Outer arity > 1 — MakeTuple must pack the stream, not the outer native id.
#[test]
fn example_io_nested_write_prints_2() {
    let output = run_example("examples/io_nested_write.0s");
    assert_eq!(output, "2");
}

/// Standalone virtual `main` for a green `test("…")` suite exits cleanly.
#[test]
fn harness_virtual_main_passes_when_all_asserts_ok() {
    let output = run_example_src(
        r#"
test("ok") {
    assert(true)?;
}
"#,
    );
    assert_eq!(output, "");
}

/// Soft-fail path prints `> Test "…" failed` and aborts via Panic.
#[test]
fn harness_virtual_main_prints_failure_and_panics() {
    let mut pipeline = Pipeline::new();
    let (bytecode, constants) = pipeline
        .compile_src(
            r#"
test("broken") {
    assert(false)?;
}
"#,
        )
        .expect("compile");
    let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
    let shared = SharedBuf(Rc::clone(&buf));
    let mut machine = Machine::<128>::default();
    machine.with_output(shared);
    pipeline.wire_host_natives(&mut machine);
    machine.run_raw(&bytecode, &constants);
    let _ = machine.restore_output();
    assert!(
        machine.panicked(),
        "virtual main must panic when a case fails"
    );
    let bytes = Rc::try_unwrap(buf)
        .expect("VM still holds a reference to the buffer")
        .into_inner();
    let output = String::from_utf8(bytes).expect("utf-8");
    assert!(
        output.contains("> Test \"broken\" failed"),
        "expected failure banner, got {output:?}"
    );
}

/// CLI-style isolation: each case is `call_function`'d independently so a
/// soft failure does not prevent later cases from running.
#[test]
fn harness_isolated_call_function_continues_after_soft_fail() {
    let mut pipeline = Pipeline::new();
    let (bytecode, constants) = pipeline
        .compile_src(
            r#"
test("a") { assert(true)?; }
test("b") { assert(false)?; }
test("c") { assert(1 + 1 == 2)?; }
"#,
        )
        .expect("compile");
    let cases = pipeline.test_cases().to_vec();
    assert_eq!(cases.len(), 3);
    assert_eq!(
        cases.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["a", "b", "c"]
    );

    let mut results = Vec::new();
    for (name, offset) in &cases {
        let mut machine = Machine::<128>::default();
        pipeline.wire_host_natives(&mut machine);
        machine.load_program(&bytecode, &constants);
        let ret = machine.call_function(*offset, &[]);
        let ok = !machine.panicked() && machine.result_is_ok(ret);
        results.push((name.as_str(), ok));
    }
    assert_eq!(results, [("a", true), ("b", false), ("c", true)]);
}

/// Hard-`panic` path: each case is still isolated (fresh VM + unwind fence),
/// matching `run_test_case` in the CLI so a VM abort does not fail-fast.
#[test]
fn harness_isolated_call_function_continues_after_hard_panic() {
    let mut pipeline = Pipeline::new();
    let (bytecode, constants) = pipeline
        .compile_src(
            r#"
test("boom") { panic "x"; }
test("after") { assert(true)?; }
"#,
        )
        .expect("compile");
    let cases = pipeline.test_cases().to_vec();
    assert_eq!(
        cases.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["boom", "after"]
    );

    let mut results = Vec::new();
    for (name, offset) in &cases {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut machine = Machine::<128>::default();
            pipeline.wire_host_natives(&mut machine);
            machine.load_program(&bytecode, &constants);
            let ret = machine.call_function(*offset, &[]);
            !machine.panicked() && machine.result_is_ok(ret)
        }));
        let ok = match outcome {
            Ok(ok) => ok,
            Err(_) => false,
        };
        results.push((name.as_str(), ok));
    }
    assert_eq!(results, [("boom", false), ("after", true)]);
}

/// Match arms that reuse a binding name with different payload types
/// must resolve field access against *that arm's* type. A flat
/// `codegen_var_types` side-table last-wins would make `p.y` emit
/// `LoadField(0)` (against Rect) and return `x` instead of `y`.
#[test]
fn match_arm_reused_binding_name_field_access_uses_arm_type() {
    let output = run_example_src(
        r#"
enum Point {
    Point { x: int, y: int },
}

enum Rect {
    Rect { w: int, h: int },
}

enum Shape {
    Pt(Point),
    Rc(Rect),
}

fn get(Shape s) -> int {
    return match s {
        Shape::Pt(p) => p.y,
        Shape::Rc(p) => p.h,
    };
}

fn main() {
    print "%i", get(Shape::Pt(Point::Point { x: 1, y: 2 }));
    print "%i", get(Shape::Rc(Rect::Rect { w: 3, h: 4 }));
}
"#,
    );
    assert_eq!(output, "24");
}

/// Polymorphic payloads (`Option<Point>`, `Box<T>`) must not push
/// registry schema placeholders (`Con("T")`) onto the per-arm override
/// stack — that shadows the instantiated side-table type and makes
/// `p.y` emit `LoadField(0)` (returns `x` / `1` instead of `y` / `2`).
#[test]
fn match_poly_payload_field_access_uses_instantiated_type() {
    let output = run_example_src(
        r#"
enum Point {
    Point { x: int, y: int },
}

enum Box<T> {
    Full(T),
}

fn from_option(Option<Point> o) -> int {
    return match o {
        Option::None => 0,
        Option::Some(p) => p.y,
    };
}

fn from_box(Box<Point> b) -> int {
    return match b {
        Box::Full(p) => p.y,
    };
}

fn main() {
    print "%i", from_option(Option::Some(Point::Point { x: 1, y: 2 }));
    print "%i", from_box(Box::Full(Point::Point { x: 1, y: 2 }));
}
"#,
    );
    // Broken override → "11"; correct → "22".
    assert_eq!(output, "22");
}

/// P0: early-loop flag reassignment sticks while later locals stay live.
#[test]
fn store_pop_early_flag_sticks_with_later_locals() {
    let output = run_example_src(
        r#"
fn main() {
    let got = 0;
    let a = 1;
    let b = 2;
    let c = 3;
    let i = 0;
    while i < 3 {
        got = 1;
        i = i + 1;
    }
    print "%i", got;
    print "%i", a + b + c;
}
"#,
    );
    assert_eq!(output, "16");
}

/// P1: empty array + push + index round-trip (and under GC pressure).
#[test]
fn empty_array_push_and_index_round_trip() {
    let output = run_example_src(
        r#"
fn main() {
    let arr: [int] = [];
    push(arr, 4);
    push(arr, 1);
    push(arr, 4);
    print "%i", len(arr);
    print "%i", arr[0];
    print "%i", arr[2];
    let i = 0;
    while i < 80 {
        push(arr, i);
        i = i + 1;
    }
    print "%i", arr[0];
}
"#,
    );
    // len=3, arr[0]=4, arr[2]=4, arr[0]=4 after growth
    assert_eq!(output, "3444");
}

/// P1: `arr[i] = x` then read-back.
#[test]
fn array_index_store_round_trip() {
    let output = run_example_src(
        r#"
fn main() {
    let arr = [0, 0, 0];
    arr[1] = 42;
    print "%i", arr[0];
    print "%i", arr[1];
    print "%i", arr[2];
}
"#,
    );
    assert_eq!(output, "0420");
}

/// P3: `return -1;` compiles and runs.
#[test]
fn return_negative_one_works() {
    let output = run_example_src(
        r#"
fn neg() -> int { return -1; }
fn main() {
    print "%i", neg();
    print "%i", 0 - 1;
}
"#,
    );
    assert_eq!(output, "-1-1");
}

/// P4: natural Ok/Ok/Err arm order (Err last) must not panic at codegen.
#[test]
fn nested_match_ok_arms_before_err_dispatches() {
    let output = run_example_src(
        r#"
fn unwrap_result(Result r) -> int {
    return match r {
        Result::Ok(Option::Some(v)) => v,
        Result::Ok(Option::None) => 0,
        Result::Err(_) => -1,
    };
}
fn main() {
    print "%i", unwrap_result(Result::Ok(Option::Some(42)));
    print "%i", unwrap_result(Result::Ok(Option::None));
    print "%i", unwrap_result(Result::Err("oops"));
}
"#,
    );
    assert_eq!(output, "420-1");
}

/// P6: class field holding an enum round-trips via GetField.
#[test]
fn class_enum_field_access_round_trip() {
    let output = run_example_src(
        r#"
enum Status {
    Ready,
    Done(int),
}

class Box {
    status: Status,
}

impl Box {
    fn get() -> Status {
        return self.status;
    }
}

fn main() {
    let b = new Box(Status::Done(9));
    let s = b.get();
    print "%i", match s {
        Status::Ready => 0,
        Status::Done(v) => v,
    };
    print "%i", match b.status {
        Status::Ready => 0,
        Status::Done(v) => v,
    };
}
"#,
    );
    assert_eq!(output, "99");
}

/// P6: match-bound constructor payloads must land in `codegen_var_types`
/// so field access uses the payload enum's LoadField index — not the
/// defensive `LoadField(0)` fallback (which silently returns the wrong
/// field when the target is not field 0).
#[test]
fn match_bound_enum_field_access_uses_correct_index() {
    let output = run_example_src(
        r#"
enum Info {
    Info { kind: int, code: int },
}

enum Wrap {
    Empty,
    Full(Info),
}

fn read_code(Wrap w) -> int {
    return match w {
        Wrap::Empty => 0,
        Wrap::Full(e) => e.code,
    };
}

fn main() {
    let w = Wrap::Full(Info::Info { kind: 1, code: 42 });
    print "%i", read_code(w);
    print "%i", match w {
        Wrap::Empty => 0,
        Wrap::Full(e) => e.kind,
    };
}
"#,
    );
    // Pre-fix: e.code → LoadField(0) → kind (1), not code (42).
    assert_eq!(output, "421");
}


#[test]
fn example_overload_prints_15() {
    let output = run_example("examples/overload.0s");
    assert_eq!(output, "15");
}

#[test]
fn example_fn_value_prints_423() {
    let output = run_example("examples/fn_value.0s");
    assert_eq!(output, "423");
}

#[test]
fn example_lambda_prints_42() {
    let output = run_example("examples/lambda.0s");
    assert_eq!(output, "42");
}

#[test]
fn example_method_overload_prints_1116() {
    let output = run_example("examples/method_overload.0s");
    assert_eq!(output, "1116");
}
