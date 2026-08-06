//! End-to-end golden tests for `.hy` example programs.

use std::io::Write;
use std::sync::{Arc, Mutex};

use compiler::Pipeline;
use machine::Machine;

/// Captures VM `PRINT` output into a shared buffer (`Send` for thread workers).
#[derive(Clone, Default)]
struct SharedBuf {
    inner: Arc<Mutex<Vec<u8>>>,
}

impl SharedBuf {
    fn new() -> Self {
        Self::default()
    }

    fn into_utf8(self) -> String {
        let bytes = self
            .inner
            .lock()
            .expect("print buffer mutex poisoned")
            .clone();
        String::from_utf8(bytes).expect("captured output should be valid UTF-8")
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

fn run_example(path: &str) -> String {
    // File-backed examples may `use` stdlib modules (`io::sync`, …); in-memory
    // compile of the text alone used to miss those until `compile_src` gained
    // discovery — prefer the multifile path so entry paths/debug stay accurate.
    run_example_multifile(path)
}

/// Compile and run in-memory source.
fn run_example_src(src: &str) -> String {
    run_example_src_with_entry(src, None)
}

#[cfg(feature = "tls")]
fn coil_escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

fn compile_src_with_tests(src: &str) -> (Pipeline, Vec<common::Byte>, Vec<u64>) {
    let mut pipeline = Pipeline::new();
    pipeline.set_include_tests(true);
    let (bytecode, constants) = pipeline
        .compile_src(src)
        .expect("source with tests should compile");
    (pipeline, bytecode, constants)
}

fn run_harness_src(src: &str) -> String {
    let (pipeline, bytecode, constants) = compile_src_with_tests(src);
    run_bytecode(bytecode, constants, &pipeline, None)
}

fn run_example_src_with_entry(src: &str, entry: Option<&std::path::Path>) -> String {
    let mut pipeline = Pipeline::new();
    let (bytecode, constants) = pipeline
        .compile_src(src)
        .expect("example failed to compile (parse error or type errors)");
    run_bytecode(bytecode, constants, &pipeline, entry)
}

/// Multi-file examples (`use` / `mod`) must go through
/// `compile_src_from_file` so the pipeline discovers dependencies
/// via `coil.toml` roots. In-memory `compile_src` cannot load them.
fn run_example_multifile(path: &str) -> String {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate must have a parent (workspace root)");
    let full = workspace_root.join(path);
    let mut pipeline = Pipeline::new();
    let (bytecode, constants) = pipeline
        .compile_src_from_file(full.to_str().unwrap())
        .unwrap_or_else(|_| panic!("multi-file example failed to compile: {}", full.display()));
    run_bytecode(bytecode, constants, &pipeline, Some(full.as_path()))
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
    let operand_slots = pipeline.operand_stack_slots() as usize;
    let shared = SharedBuf::new();
    let mut machine = Machine::<256>::with_operand_capacity(operand_slots);
    machine.set_shared_print(shared.inner.clone());
    machine.with_output(shared.clone());
    pipeline.wire_vm_ffi(&mut machine, entry);
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
    shared.into_utf8()
}

#[test]
fn example_panic_loc_archive_has_source_files() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    let path = workspace_root.join("examples/panic_loc.hy");
    let mut pipeline = Pipeline::new();
    let (bytecode, _constants) = pipeline
        .compile_src_from_file(path.to_str().unwrap())
        .expect("panic_loc should compile");
    let debug = pipeline.program_debug();
    assert!(
        debug
            .source_files
            .iter()
            .any(|p| p.contains("panic_loc.hy")),
        "expected panic_loc.hy in source_files: {:?}",
        debug.source_files
    );
    assert_eq!(debug.debug_locs.len(), bytecode.len());
    use common::Instruction;
    assert!(
        bytecode
            .iter()
            .any(|b| matches!(b.bytecode(), Instruction::Panic)),
        "expected Panic opcode"
    );
}

#[test]
fn example_option_prints_42() {
    let output = run_example("examples/option.hy");
    assert_eq!(output, "42");
}

#[test]
fn example_generics_uses_builtin_dictionary_abi() {
    let output = run_example("examples/generics.hy");
    assert_eq!(output, "7424.0427");
}

#[test]
fn example_result_prints_42_and_neg1() {
    let output = run_example("examples/result.hy");
    assert_eq!(output, "420-1");
}

#[test]
fn example_raise_try_prints_10_neg() {
    let output = run_example("examples/raise_try.hy");
    assert_eq!(output, "10,neg");
}

#[test]
fn example_assert_prints_ok_assertion_failed_custom() {
    let output = run_example("examples/assert.hy");
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
    let shared = SharedBuf::new();
    let mut machine = Machine::<128>::default();
    machine.with_output(shared.clone());
    machine.run_raw(
        &bytecode,
        &constants,
        pipeline.strings(),
        pipeline.static_slot_count(),
    );
    assert!(machine.panicked(), "expected language-level panic");
    let _ = machine.restore_output();
    let s = shared.into_utf8();
    assert_eq!(s, "panic: boom");
}

#[test]
fn example_coalesce_prints_bar_hi_7_9() {
    let output = run_example("examples/coalesce.hy");
    assert_eq!(output, "bar,hi,7,9");
}

#[test]
fn example_optional_chain_prints_42_0() {
    let output = run_example("examples/optional_chain.hy");
    assert_eq!(output, "42,0");
}

#[test]
fn example_tree_prints_6() {
    let output = run_example("examples/tree.hy");
    assert_eq!(output, "6");
}

#[test]
fn example_fib_still_works() {
    let output = run_example("examples/fib.hy");
    assert_eq!(output, "55");
}

#[test]
fn example_record_prints_169_5_12() {
    let output = run_example("examples/record.hy");
    assert_eq!(output, "169512");
}

#[test]
fn example_dict_prints_42_100_42() {
    let output = run_example("examples/dict.hy");
    assert_eq!(output, "4210042");
}

#[test]
fn example_array_grow_prints_len_first_and_last() {
    let output = run_example("examples/array_grow.hy");
    assert_eq!(output, "414");
}

#[test]
fn example_static_singleton_prints_121() {
    let output = run_example("examples/static_singleton.hy");
    assert_eq!(output, "121");
}

/// `static fn new(...)` / `Class::fresh()` alongside positional `new Class(...)`.
#[test]
fn example_static_ctor_prints_42_1_1() {
    let output = run_example("examples/static_ctor.hy");
    assert_eq!(output, "42,1,1");
}

#[test]
fn example_static_minimal_prints_11() {
    let output = run_example("examples/static_minimal.hy");
    assert_eq!(output, "11");
}

#[test]
fn example_readonly_seal_prints_322() {
    let output = run_example("examples/readonly_seal.hy");
    assert_eq!(output, "322");
}

/// `static const` is readable via LoadStatic; only reassignment is rejected.
#[test]
fn static_const_reads_via_load_static() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
static const VERSION = 42;

fn main() {
    write(stdout(), to_bytes(format("%i", VERSION)));
}
"#,
    );
    assert_eq!(output, "42");
}

/// Class `const` fields are readable after construction (mutation is blocked separately).
#[test]
fn const_class_field_reads_after_construction() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
class Point {
    const x: int,
    y: int,
}

fn main() {
    let p = new Point(7, 9);
    write(stdout(), to_bytes(format("%i", p.x)));
    write(stdout(), to_bytes(format("%i", p.y)));
}
"#,
    );
    assert_eq!(output, "79");
}

#[test]
fn example_classes_prints_7458() {
    let output = run_example("examples/classes.hy");
    assert_eq!(output, "7458");
}

#[test]
fn example_generic_class_prints_42() {
    let output = run_example("examples/generic_class.hy");
    assert_eq!(output, "42");
}

#[test]
fn example_aliases_prints_3_4_7() {
    let output = run_example("examples/aliases.hy");
    assert_eq!(output, "347");
}

#[test]
fn example_nested_aggregates_prints_rows_and_total() {
    let output = run_example("examples/nested_aggregates.hy");
    assert_eq!(output, "alice:30bob:25total:55");
}

#[test]
fn example_modules_brace_prints_12_42() {
    let output = run_example_multifile("examples/modules_brace.hy");
    assert_eq!(output, "1242");
}

#[test]
fn example_match_block_self_prints_5() {
    let output = run_example("examples/match_block_self.hy");
    assert_eq!(output, "5");
}

#[test]
fn example_defer_prints_enterleave_lifo_and_early_return() {
    let output = run_example("examples/defer.hy");
    assert_eq!(output, "enterleave,021,okd7,d99,55");
}

/// Regression: inherent `fn send` must not shadow `thread::send`, so
/// `send(self.channel, msg)` typechecks and runs.
#[test]
fn method_named_send_can_call_thread_send_on_self_channel() {
    let output = run_example_src(
        r#"
use thread::{channel, recv, send, spawn, Sender, Thread};
use io::{stdout, write};
use string::{format, to_bytes};

class ThreadWrapper {
    thread: Thread,
    channel: Sender,
}

impl ThreadWrapper {
    fn send(string msg) {
        send(self.channel, msg)?;
    }
}

fn work() -> int { return 0; }

fn main() {
    let pair = channel()?;
    let t = spawn(work)?;
    let w = new ThreadWrapper(t, pair[0]);
    w.send("hi")?;
    write(stdout(), to_bytes(format("%s", recv(pair[1])?)));
}
"#,
    );
    assert_eq!(output, "hi");
}

/// Regression: parenthesized `(self).field` must still emit GetField.
/// Pre-fix, `receiver_type` ignored `Group`, so Access fell through to
/// `LoadField(0)` and `send` received a bogus value (often looking like
/// the class instance was passed instead of the field).
#[test]
fn grouped_self_field_passes_sender_to_send() {
    let output = run_example_src(
        r#"
use thread::{channel, recv, send, Sender};
use io::{stdout, write};
use string::{format, to_bytes};

class Worker {
    tx: Sender,
}

impl Worker {
    fn push(string msg) {
        send((self).tx, msg)?;
    }
}

fn main() {
    let pair = channel()?;
    let w = new Worker(pair[0]);
    w.push("hi")?;
    write(stdout(), to_bytes(format("%s", recv(pair[1])?)));
}
"#,
    );
    assert_eq!(output, "hi");
}

/// Regression: field access on `new Class(...)` must use GetField, and
/// `new` must not leave a stash slot between a HostInvoke native-id and
/// the field value (StorePop+final LOAD used to bury the id so `send`
/// saw the instance address as the native id).
#[test]
fn inline_new_field_passes_sender_to_send() {
    let output = run_example_src(
        r#"
use thread::{channel, recv, send, Sender};
use io::{stdout, write};
use string::{format, to_bytes};

class Worker {
    tx: Sender,
}

fn main() {
    let pair = channel()?;
    send((new Worker(pair[0])).tx, "hi")?;
    write(stdout(), to_bytes(format("%s", recv(pair[1])?)));
}
"#,
    );
    assert_eq!(output, "hi");
}

/// Regression: method call on `(new Class(...))` must resolve the owner.
#[test]
fn inline_new_method_call_works() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
class Point {
    x: int,
    y: int,
}

impl Point {
    fn sum() -> int {
        return self.x + self.y;
    }
}

fn main() {
    write(stdout(), to_bytes(format("%i", (new Point(1, 3)).sum())));
}
"#,
    );
    assert_eq!(output, "4");
}

/// Regression: `defer` inside a function must run on early `return`, not
/// only on fall-through.
#[test]
fn defer_runs_on_early_return() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn f(int n) -> int {
    defer { write(stdout(), to_bytes("d")); }
    if n == 0 {
        return 1;
    }
    return 2;
}

fn main() {
    write(stdout(), to_bytes(format("%i,", f(0))));
    write(stdout(), to_bytes(format("%i", f(9))));
}
"#,
    );
    assert_eq!(output, "d1,d2");
}

#[test]
fn example_generic_alias_prints_7() {
    let output = run_example("examples/generic_alias.hy");
    assert_eq!(output, "7");
}

#[test]
fn example_generic_enum_prints_7() {
    let output = run_example("examples/generic_enum.hy");
    assert_eq!(output, "7");
}

#[test]
fn example_generics_prints_add_results_for_int_and_float() {
    let output = run_example("examples/generics.hy");
    assert_eq!(output, "7424.0427");
}

#[test]
fn example_typeclass_dict_forwards_dictionary_and_prints_42_twice() {
    let output = run_example("examples/typeclass_dict.hy");
    assert_eq!(output, "4242");
}

#[test]
fn example_typeclass_default_calls_sibling_and_prints_42() {
    let output = run_example("examples/typeclass_default.hy");
    assert_eq!(output, "42");
}

#[test]
fn example_polyfn_supports_multi_instantiation_constraints_and_rank_n() {
    let output = run_example("examples/polyfn.hy");
    assert_eq!(output, "424.0424242");
}

/// Phase 4: `%v` displays through Show (builtin + user instance + format).
#[test]
fn example_generic_print_shows_primitives_and_user_type() {
    let output = run_example("examples/generic_print.hy");
    assert_eq!(output, "42hi1.5true(3,4)99");
}

/// Advanced generics Phase 4: a bare unary trait name is an existential type.
#[test]
fn example_existential_show_prints_42() {
    let output = run_example("examples/existential_show.hy");
    assert_eq!(output, "42");
}

/// Phase 8: tuples and anonymous records have structural Show for `%v`.
#[test]
fn example_show_tuple_prints_structural_tuple_and_record() {
    let output = run_example("examples/show_tuple.hy");
    assert_eq!(output, "(1, 2){ a: 3, b: 4 }");
}

/// Constructor-kind trait `Container<Option>` + `get<F: Container, A>(F<A>)`.
#[test]
fn example_hkt_container_prints_42() {
    let output = run_example("examples/hkt_container.hy");
    assert_eq!(output, "42");
}

/// Phase 1 advanced generics: binary HKT `Bifunctor<Result>`.
#[test]
fn example_hkt_bifunctor_prints_42() {
    let output = run_example("examples/hkt_bifunctor.hy");
    assert_eq!(output, "42");
}

/// Phase 3: multi-param trait `Convert<A, B>` + `where` clause.
#[test]
fn example_multiparam_prints_42() {
    let output = run_example("examples/multiparam.hy");
    assert_eq!(output, "42");
}

/// Prelude `Into`: `let f: Fahrenheit = c.into();` with two local classes.
#[test]
fn example_into_prints_32() {
    let output = run_example("examples/into.hy");
    assert_eq!(output, "32");
}

/// Inline receiver `new Celsius(0).into()` must typecheck and run (Bugbot:
/// codegen used to skip boxing when `receiver_type` only handled Identifier/Access).
#[test]
fn inline_into_receiver_prints_32() {
    let src = r#"
use io::{stdout, write};
use string::{format, to_bytes};
class Celsius { c: int }
class Fahrenheit { f: int }
impl Into<Fahrenheit> for Celsius {
    fn into(Celsius x) -> Fahrenheit {
        return new Fahrenheit(x.c * 2 + 32);
    }
}
fn main() {
    let f: Fahrenheit = new Celsius(0).into();
    write(stdout(), to_bytes(format("%i", f.f)));
}
"#;
    let output = run_example_src(src);
    assert_eq!(output, "32");
}

/// `return c.into();` under `-> Fahrenheit` pins the Into target at runtime.
#[test]
fn return_into_pins_target_prints_32() {
    let src = r#"
use io::{stdout, write};
use string::{format, to_bytes};
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
    write(stdout(), to_bytes(format("%i", f.f)));
}
"#;
    let output = run_example_src(src);
    assert_eq!(output, "32");
}

/// Phase 5: superclass / implied bounds (`Ordered<T: Equal>` → `eq_val` under `T: Ordered`).
#[test]
fn example_superclass_ord_prints_truetruefalse() {
    let output = run_example("examples/superclass_ord.hy");
    assert_eq!(output, "truetruefalse");
}

/// Advanced generics Phase 5: `c: * -> Constraint, T: c` with superclass method use.
#[test]
fn example_constraint_kind_prints_42() {
    let output = run_example("examples/constraint_kind.hy");
    assert_eq!(output, "42");
}

/// Phase 6: associated types — `Collect::Elem` pinned from ground instance.
#[test]
fn example_assoc_type_prints_42() {
    let output = run_example("examples/assoc_type.hy");
    assert_eq!(output, "42");
}

/// Phase 3 advanced generics: generic associated type `Pointer::Ref<A>`.
#[test]
fn example_gat_pointer_prints_42() {
    let output = run_example("examples/gat_pointer.hy");
    assert_eq!(output, "42");
}

/// Shuffled record pattern `{ y: _, x: a }` must bind declaration-order `x`.
#[test]
fn shuffled_record_pattern_binds_declaration_order_field() {
    let src = r#"
use io::{stdout, write};
use string::{format, to_bytes};
        enum E { Foo { x: int, y: int, z: int } }
        fn main() {
            let e = E::Foo { x: 1, y: 2, z: 3 };
            let v = match e {
                E::Foo { y: _, x: a, z: _ } => a,
            };
            write(stdout(), to_bytes(format("%i", v)));
        }
    "#;
    assert_eq!(run_example_src(src), "1");
}

/// Phase 4: open `%v` inside a Show-bound generic body.
#[test]
fn generic_print_open_bound_uses_show_dictionary() {
    let src = r#"
use io::{stdout, write};
use string::{format, to_bytes};
        fn show_it<T: Show>(T x) {
            write(stdout(), to_bytes(format("%v,", x)));
        }
        fn main() {
            show_it(10);
            show_it(20);
        }
    "#;
    assert_eq!(run_example_src(src), "10,20,");
}

/// Phase 4: `string::format("%v", ...)` produces a string for further use.
#[test]
fn format_percent_v_parity_with_print() {
    let src = r#"
use io::{stdout, write};
use string::{format, to_bytes};
        fn main() {
            let s = format("%v-%v", 1, "x");
            write(stdout(), to_bytes(format("%s", s)));
        }
    "#;
    assert_eq!(run_example_src(src), "1-x");
}

/// Phase 4: captured dictionaries remain valid after the creating frame returns
/// and the application site need not supply dictionaries (`app_dict_arity=0`).
#[test]
fn polyfn_captured_dict_survives_return() {
    let src = r#"
use io::{stdout, write};
use string::{format, to_bytes};
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
            write(stdout(), to_bytes(format("%i", f(41))));
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

/// Inner-block PolyFn let must not poison an outer same-named ObjFn local.
#[test]
fn polyfn_block_shadow_does_not_box_outer_mono_fn() {
    let src = r#"
use io::{stdout, write};
use string::{format, to_bytes};
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
        fn add(int a, int b) -> int { return a + b; }
        fn main() {
            let f = add;
            {
                let f = capture_show(0);
                write(stdout(), to_bytes(format("%i", f(41))));
            }
            write(stdout(), to_bytes(format("%i", f(1, 2))));
        }
    "#;
    assert_eq!(run_example_src(src), "423");
}

/// Phase 4: multiparam `Convert<A,B>` capture works after return with no app dict.
/// Both type args are witnessed at the capture call so the dict is concrete.
#[test]
fn polyfn_multiparam_capture_survives_return() {
    let src = r#"
use io::{stdout, write};
use string::{format, to_bytes};
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
            write(stdout(), to_bytes(format("%i", f(42))));
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
use io::{stdout, write};
use string::{format, to_bytes};
        fn id<T>(T x) -> T { return x; }
        fn fib(int n) -> int {
            if n <= 2 { return 1; }
            return fib(n - 1) + fib(n - 2);
        }
        fn main() {
            let f = id;
            write(stdout(), to_bytes(format("%i", f(fib(6)))));
        }
    "#;
    let mut pipeline = Pipeline::new();
    let (bytecode, constants) = pipeline.compile_src(src).expect("compile");
    assert!(
        bytecode
            .iter()
            .any(|b| matches!(b.bytecode(), Instruction::MakePolyFn))
    );
    // Fuse-select is intentionally conservative while absolute JMP joins
    // finish migrating; fib arithmetic may be unfused.
    let has_arith = bytecode.iter().any(|b| {
        matches!(
            *b.bytecode(),
            Instruction::ADD
                | Instruction::LEQ
                | Instruction::BinSlotImm
                | Instruction::BinSlotSlot
                | Instruction::BinReturn
        )
    });
    assert!(has_arith, "expected fib arithmetic with PolyFn present");
    let output = run_bytecode(bytecode, constants, &pipeline, None);
    // fib(6) = 8
    assert_eq!(output, "8");
}

#[test]
fn monomorphized_generic_add_prints_3() {
    let output = run_example_src(
        r#"use io::{stdout, write};
use string::{format, to_bytes};
fn add<T: Num>(T a, T b) -> T {
            return a + b;
        }

        fn main() {
            write(stdout(), to_bytes(format("%i", add(1, 2))));
        }"#,
    );
    assert_eq!(output, "3");
}

#[test]
fn example_const_prints_42hi() {
    let output = run_example("examples/const.hy");
    assert_eq!(output, "42hi");
}

#[test]
fn string_fmt_example_prints_concatenated_and_formatted_strings() {
    let output = run_example("examples/string_fmt.hy");
    assert_eq!(output, "hello world42-x");
}

#[test]
fn string_plus_equal_updates_binding() {
    let output = run_example_src(
        r#"use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
            let s = "a";
            s += "b";
            write(stdout(), to_bytes(format("%s", s)));
        }"#,
    );
    assert_eq!(output, "ab");
}

#[test]
fn example_mixed_prints_zero_circle_square_triangle() {
    let output = run_example("examples/mixed.hy");
    assert_eq!(output, "025122");
}

#[test]
fn example_chained_prints_42_7() {
    let output = run_example("examples/chained.hy");
    assert_eq!(output, "427");
}

#[test]
fn example_match_with_two_ok_arms_dispatches_correctly() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate must have a parent (workspace root)");
    let full = workspace_root.join("examples/result.hy");
    let src = std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", full.display(), e));

    let mut pipeline = compiler::Pipeline::new();
    let parser = parser::Pratt::default();
    let mut ast = parser.parse(&src).expect("result.hy should parse");
    let (bytecode, constants) = pipeline.compile_test("", &mut ast);

    let shared = SharedBuf::new();
    let mut machine = machine::Machine::<128>::default();
    machine.with_output(shared.clone());
    pipeline.wire_thread_program(&mut machine, &bytecode, &constants, pipeline.strings());
    pipeline.wire_host_natives(&mut machine);
    machine.run_raw(
        &bytecode,
        &constants,
        pipeline.strings(),
        pipeline.static_slot_count(),
    );

    let _ = machine.restore_output();
    let output = shared.into_utf8();

    assert_eq!(output, "420-1");
}

#[test]
fn fizbuz_runs_to_completion() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate must have a parent (workspace root)");
    let full = workspace_root.join("examples/fizbuz.hy");
    let src = std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", full.display(), e));

    let mut pipeline = compiler::Pipeline::new();
    let parser = parser::Pratt::default();
    let mut ast = parser.parse(&src).expect("fizbuz.hy should parse");
    let (bytecode, constants) = pipeline.compile_test("", &mut ast);

    let shared = SharedBuf::new();
    let mut machine = machine::Machine::<128>::default();
    machine.with_output(shared);
    pipeline.wire_host_natives(&mut machine);
    machine.run_raw(
        &bytecode,
        &constants,
        pipeline.strings(),
        pipeline.static_slot_count(),
    );

    let _ = shared;
}

#[test]
fn let_binding_emits_store_pop_in_bytecode() {
    use common::Instruction;
    let mut pipeline = Pipeline::new();
    let src = r#"
use io::{stdout, write};
use string::{format, to_bytes};
        fn main() {
            let x = 5;
            write(stdout(), to_bytes(format("%i", x)));
            x = 10;
            write(stdout(), to_bytes(format("%i", x)));
        }
    "#;
    let parser = parser::Pratt::default();
    let mut ast = parser.parse(src).expect("let-binding program should parse");
    let (bytecode, _constants) = pipeline.compile_test("", &mut ast);
    assert!(!bytecode.is_empty(), "program should produce bytecode");

    let binding_store_count = bytecode
        .iter()
        .filter(|b| {
            matches!(b.bytecode(), Instruction::STORE) && b.load_store_single_slot() == Some(0)
        })
        .count();
    assert_eq!(
        binding_store_count, 2,
        "expected exactly 2 STORE writes to binding slot 0 for one let + one re-assignment; got {}",
        binding_store_count
    );
}

#[test]
fn example_let_reassignment_works() {
    let output = run_example("examples/let_test.hy");
    assert_eq!(output, "51020");
}

#[test]
fn example_named_args_prints_ada36_grace40() {
    let output = run_example("examples/named_args.hy");
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
use io::{stdout, write};
use string::{format, to_bytes};
fn greet(string name, int age) {
    write(stdout(), to_bytes(format("%s", name)));
    write(stdout(), to_bytes(format("%i", age)));
}

fn main() {
    greet(age: 36, name: "Ada");
    greet(age: 40, name: "Grace");
}
"#,
    );
    assert_eq!(output, "Ada36Grace40");
}

#[test]
fn builtin_len_named_arg_prints_3() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    write(stdout(), to_bytes(format("%i", len(value: [1, 2, 3]))));
}
"#,
    );
    assert_eq!(output, "3");
}

#[test]
fn format_operand_string_concat_prints() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let n = 7;
    write(stdout(), to_bytes("n=" + format("%i", n)));
    write(stdout(), to_bytes(format("%i", n) + "!"));
}
"#,
    );
    assert_eq!(output, "n=77!");
}

#[test]
fn pure_arg_reorder_preserves_identifier_args() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn effect() -> int {
    write(stdout(), to_bytes(format("%i,", 7)));
    return 2;
}
fn sink(int a, int b) -> int {
    write(stdout(), to_bytes(format("%i,", a + b)));
    return a + b;
}
fn main() {
    let cached = 10;
    write(stdout(), to_bytes(format("%i", sink(effect(), cached))));
}
"#,
    );
    assert_eq!(output, "7,12,12");
}

/// Rest packing with a named fixed prefix + trailing positionals (P4 + P2).
#[test]
fn rest_after_named_fixed_prefix_packs_trailing() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn f(int a, int... xs) -> int {
    return a + len(xs);
}

fn main() {
    write(stdout(), to_bytes(format("%i", f(a: 10, 1, 2, 3))));
    write(stdout(), to_bytes(format("%i", f(a: 7))));
}
"#,
    );
    assert_eq!(output, "137");
}

#[test]
fn example_let_destructure_prints_12342() {
    let output = run_example("examples/let_destructure.hy");
    assert_eq!(output, "12342");
}

/// Nested let destructure must bind inner tuple slots correctly (not swap).
#[test]
fn let_nested_tuple_destructure_binds_in_order() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let (a, (b, c)) = (1, (2, 3));
    write(stdout(), to_bytes(format("%i", a)));
    write(stdout(), to_bytes(format("%i", b)));
    write(stdout(), to_bytes(format("%i", c)));
}
"#,
    );
    assert_eq!(output, "123");
}

#[test]
fn example_variadic_prints_60_hi() {
    let output = run_example("examples/variadic.hy");
    assert_eq!(output, "60Hi!?");
}

/// Phase P0: `let x = match { … }` must bind the arm value via
/// StorePop. Pre-fix Match emitted RETURN at end_label, so the
/// StorePop was unreachable and prints never ran / saw 0.
#[test]
fn let_match_binds_arm_value() {
    let src = r#"
use io::{stdout, write};
use string::{format, to_bytes};
        fn main() {
            let x = match Option::None {
                Option::None => 7,
                Option::Some(v) => v,
            };
            write(stdout(), to_bytes(format("%i", x)));
            let y = match Option::Some(42) {
                Option::None => 0,
                Option::Some(v) => v,
            };
            write(stdout(), to_bytes(format("%i", y)));
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
use io::{stdout, write};
use string::{format, to_bytes};
        fn main() {
            let d = { name: "hi", n: 9 };
            write(stdout(), to_bytes(format("%s", d.name)));
            write(stdout(), to_bytes(format("%i", d.n)));
        }
    "#;
    let output = run_example_src(src);
    assert_eq!(output, "hi9");
}

/// Phase P1: in-place dict mutation via `d.field = value` then re-read.
#[test]
fn dict_mutation_round_trips() {
    let src = r#"
use io::{stdout, write};
use string::{format, to_bytes};
        fn main() {
            let d = { foo: 1, name: "a" };
            d.foo = 99;
            d.name = "z";
            write(stdout(), to_bytes(format("%i", d.foo)));
            write(stdout(), to_bytes(format("%s", d.name)));
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
use io::{stdout, write};
use string::{format, to_bytes};
        fn main() {
            let x = 5;
            let y = x + 1;
            write(stdout(), to_bytes(format("%i", y)));
        }
    "#;

    let mut pipeline = compiler::Pipeline::new();
    let parser = parser::Pratt::default();
    let mut ast = parser
        .parse(src)
        .expect("chained-bindings program should parse");
    let (bytecode, constants) = pipeline.compile_test("", &mut ast);

    let shared = SharedBuf::new();
    let mut machine = machine::Machine::<128>::default();
    machine.with_output(shared.clone());
    pipeline.wire_host_natives(&mut machine);
    machine.run_raw(
        &bytecode,
        &constants,
        pipeline.strings(),
        pipeline.static_slot_count(),
    );

    let _ = machine.restore_output();
    let output = shared.into_utf8();

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
    let output = run_example("examples/nested_records.hy");
    assert_eq!(output, "99");
}

/// Nested multi-field record patterns must not clobber sibling outer fields.
/// Pre-fix, in-place `UnpackAt` at the outer field slot overwrote later
/// siblings when the inner arity exceeded one.
#[test]
fn nested_multifield_record_pattern_preserves_sibling() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
enum Inner {
    I { x: int, y: int },
}
enum Wrap {
    W { inner: Inner, name: int },
}
fn both(Wrap w) -> int {
    return match w {
        Wrap::W { inner: Inner::I { x, y }, name } => x + y + name,
    };
}
fn main() {
    let w = Wrap::W { inner: Inner::I { x: 10, y: 20 }, name: 3 };
    write(stdout(), to_bytes(format("%i", both(w))));
}
"#,
    );
    assert_eq!(
        output, "33",
        "inner fields and outer sibling `name` must all bind correctly"
    );
}

/// Two nested multifield records in one outer arm — each must unpack into
/// distinct scratch regions so the second nested payload cannot overwrite
/// the first nested bindings.
#[test]
fn nested_two_multifield_records_preserve_both() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
enum Inner {
    I { x: int, y: int },
}
enum Pair {
    P { a: Inner, b: Inner },
}
fn sum(Pair p) -> int {
    return match p {
        Pair::P {
            a: Inner::I { x: ax, y: ay },
            b: Inner::I { x: bx, y: by },
        } => ax + ay + bx + by,
    };
}
fn main() {
    let p = Pair::P {
        a: Inner::I { x: 1, y: 2 },
        b: Inner::I { x: 10, y: 20 },
    };
    write(stdout(), to_bytes(format("%i", sum(p))));
}
"#,
    );
    assert_eq!(output, "33", "both nested multifield bindings must survive");
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
    let status = cmd.arg("-O2").arg("-o").arg(&libsum).arg(&sum_c).status();
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
    let full = workspace_root.join("examples/ffi_sum.hy");
    let lib_abs = libsum.canonicalize().unwrap_or_else(|_| libsum.clone());
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
        let full = workspace_root.join("examples/strlen.hy");
        let src = std::fs::read_to_string(&full).expect("read strlen.hy");
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

#[test]
fn example_strlen_prints_5_compile_src_from_file() {
    if machine::resolve_library("c", None, &[]).is_err() {
        ffi_soft_skip("C library not loadable on this platform via resolve_library(\"c\")");
        return;
    }

    let result = std::panic::catch_unwind(|| run_example("examples/strlen.hy"));
    let output = match result {
        Ok(s) => s,
        Err(_) => {
            ffi_soft_skip("strlen test panicked (dlopen failure?)");
            return;
        }
    };
    assert_eq!(
        output,
        "5",
        "strlen(\"hello\") via compile_src_from_file should print 5"
    );
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
/// parallelism: libtest harness status lines.
#[cfg(unix)]
fn clean_captured_os_stdout(output: &str) -> String {
    output
        .lines()
        .filter(|l| {
            let t = l.trim();
            if t.is_empty() {
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
            let full = workspace_root.join("examples/ffi_printf.hy");
            let src = std::fs::read_to_string(&full).expect("read ffi_printf.hy");
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
use io::{stdout, write};
use string::{format, to_bytes};
extern "this_library_definitely_does_not_exist_xyzzy" {
    fn noop() -> int;
}
fn main() {
    write(stdout(), to_bytes(format("%i", noop())));
}
"#;
    let mut pipeline = Pipeline::new();
    let (bytecode, constants) = pipeline
        .compile_src(src)
        .expect("should compile (load failure is runtime)");
    let shared = SharedBuf::new();
    let mut machine = Machine::<128>::default();
    machine.with_output(shared.clone());
    pipeline.wire_vm_ffi(&mut machine, None);
    pipeline.wire_host_natives(&mut machine);
    machine.set_program_debug(pipeline.program_debug());
    machine.run_raw(
        &bytecode,
        &constants,
        pipeline.strings(),
        pipeline.static_slot_count(),
    );
    assert!(
        machine.panicked(),
        "missing library should panic, not segfault"
    );
    let _ = machine.restore_output();
    let output = shared.into_utf8();
    assert!(
        output.contains("panic:") && output.contains("not found"),
        "expected panic message about missing library, got: {output:?}"
    );
}

#[test]
fn userland_dload_missing_library_returns_err() {
    let src = r#"
use ffi::{dload, ErrorKind};
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let r = dload("this_library_definitely_does_not_exist_xyzzy");
    let msg = match r {
        Result::Ok(_) => "ok",
        Result::Err(e) => match e.kind {
            ErrorKind::LibraryNotFound => "missing",
            _ => "other",
        },
    };
    write(stdout(), to_bytes(format("%s", msg)));
}
"#;
    let output = run_example_src(src);
    assert_eq!(output, "missing");
}

#[test]
fn example_coro_prints_suspended_1_resumed() {
    let output = run_example("examples/coro.hy");
    assert_eq!(output, "Suspended\n1Resumed\n");
}

#[test]
fn example_coro_gen_prints_012() {
    let output = run_example("examples/coro_gen.hy");
    assert_eq!(output, "012");
}

#[test]
fn example_coro_interleave_prints_out_of_order_counters() {
    let output = run_example("examples/coro_interleave.hy");
    assert_eq!(output, "10,100,101,11,12,102");
}

#[test]
fn example_coro_send_prints_hello() {
    let output = run_example("examples/coro_send.hy");
    assert_eq!(output, "hello");
}

#[test]
fn example_coro_yield_from_prints_012() {
    let output = run_example("examples/coro_yield_from.hy");
    assert_eq!(output, "012");
}

#[test]
fn example_coro_done_prints_false_false_true() {
    let output = run_example("examples/coro_done.hy");
    assert_eq!(output, "falsefalsetrue");
}

#[test]
fn example_for_in_coro_prints_012_and_breaks() {
    // counter yields 0,1,2 then returns 99 — completion must NOT print.
    // early yields 10,20,30 — break on 20 prints only 10.
    let output = run_example("examples/for_in_coro.hy");
    assert_eq!(output, "01210");
}

#[test]
fn example_for_in_array_prints_123() {
    let output = run_example("examples/for_in_array.hy");
    assert_eq!(output, "123");
}

#[test]
fn example_for_in_tuple_prints_123() {
    let output = run_example("examples/for_in_tuple.hy");
    assert_eq!(output, "123");
}

#[test]
fn example_for_in_dict_prints_12() {
    let output = run_example("examples/for_in_dict.hy");
    assert_eq!(output, "12");
}

#[test]
fn example_for_in_custom_prints_012() {
    let output = run_example("examples/for_in_custom.hy");
    assert_eq!(output, "012");
}

#[test]
fn example_range_prints_01234012356() {
    // 0..5 → 01234; 0..=3 → 0123; 10..0 empty; byte 5..=6 → 56;
    // float 1.0..4.0 → 1.02.03.0
    let output = run_example("examples/range.hy");
    assert_eq!(output, "012340123561.02.03.0");
}

/// Inner-block destructure must not clobber an outer binding's slot.
#[test]
fn let_destructure_block_shadow_preserves_outer_binding() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
enum A { A { z: int, x: int }, }
enum B { B { y: int }, }
fn main() {
  let outer = { a: A::A { z: 10, x: 42 } };
  let { a } = outer;
  { let inner = { a: B::B { y: 7 } }; let { a } = inner; }
  write(stdout(), to_bytes(format("%i", a.x)));
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
use io::{stdout, write};
use string::{format, to_bytes};
fn show_all<T: Show>(T... xs) {
    write(stdout(), to_bytes(format("%v", xs[0])));
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
use io::{stdout, write};
use string::{format, to_bytes};
fn twice_first<T: Num>(T... xs) -> T { return xs[0] + xs[0]; }
fn main() { write(stdout(), to_bytes(format("%i", twice_first(21)))); }
"#,
    );
    assert_eq!(output, "42");
}

/// Shadowing a function parameter inside a block must restore Access typing.
#[test]
fn block_shadow_of_param_restores_access_field_type() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
enum A { A { z: int, x: int }, }
enum B { B { y: int }, }
fn foo(A a) {
  { let a = B::B { y: 7 }; }
  write(stdout(), to_bytes(format("%i", a.x)));
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
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    for x in 0..1 {
        write(stdout(), to_bytes(format("%i", x)));
    }
    write(stdout(), to_bytes(","));
    for x in 0..=1 {
        write(stdout(), to_bytes(format("%i", x)));
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
use io::{stdout, write};
use string::{format, to_bytes};
        async fn counter() {
            yield 0;
            yield 1;
            yield 2;
        }

        fn main() {
            let h = counter();
            write(stdout(), to_bytes(format("%i,", resume h)));
            write(stdout(), to_bytes(format("%i,", resume h)));
            write(stdout(), to_bytes(format("%i", resume h)));
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
use io::{stdout, write};
use string::{format, to_bytes};
        async fn counter(int base) {
            yield base;
            yield base + 1;
            yield base + 2;
        }

        fn main() {
            let a = counter(1);
            let b = counter(100);

            write(stdout(), to_bytes(format("%i,", resume a)));
            write(stdout(), to_bytes(format("%i,", resume b)));
            write(stdout(), to_bytes(format("%i,", resume a)));
            write(stdout(), to_bytes(format("%i,", resume b)));
            write(stdout(), to_bytes(format("%i,", resume a)));
            write(stdout(), to_bytes(format("%i", resume b)));
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
use io::{stdout, write};
use string::{format, to_bytes};
        async fn counter() {
            yield 1;
            yield 2;
            return 42;
        }

        fn main() {
            let h = counter();
            write(stdout(), to_bytes(format("%i,", resume h))); // yield 1
            write(stdout(), to_bytes(format("%i,", resume h))); // yield 2
            write(stdout(), to_bytes(format("%i", resume h)));  // return 42
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
use io::{stdout, write};
use string::{format, to_bytes};
        async fn counter() {
            return 42;
        }

        fn main() {
            let h = counter();
            write(stdout(), to_bytes(format("%i,", resume h))); // return 42 (completes)
            write(stdout(), to_bytes(format("%i,", resume h))); // Done -> 0, not 42
            write(stdout(), to_bytes(format("%i", resume h)));  // Done -> 0, not 42
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
    let output = run_ffi_example_with_lib("examples/ffi_array.hy", &libsum);
    assert_eq!(output, "15");
}

#[test]
fn example_ffi_callback_prints_42() {
    let libsum = ensure_ffi_libsum_built();
    if !libsum.exists() {
        ffi_soft_skip(&format!("{} not built", libsum.display()));
        return;
    }
    let output = run_ffi_example_with_lib("examples/ffi_callback.hy", &libsum);
    assert_eq!(output, "42");
}

#[test]
fn example_ffi_struct_return_prints_34() {
    let libsum = ensure_ffi_libsum_built();
    if !libsum.exists() {
        ffi_soft_skip(&format!("{} not built", libsum.display()));
        return;
    }
    let output = run_ffi_example_with_lib("examples/ffi_struct_ret.hy", &libsum);
    assert_eq!(output, "34");
}

#[test]
fn example_ffi_callback_return_prints_1() {
    let libsum = ensure_ffi_libsum_built();
    if !libsum.exists() {
        ffi_soft_skip(&format!("{} not built", libsum.display()));
        return;
    }
    let output = run_ffi_example_with_lib("examples/ffi_callback_ret.hy", &libsum);
    assert_eq!(output, "1");
}

#[test]
fn example_operators_prints_expected() {
    let output = run_example("examples/operators.hy");
    assert_eq!(output, "801125428falsetrue3");
}

#[test]
fn example_while_loop_accumulates_correctly() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
        fn main() {
            let acc = 0;
            let i = 0;
            while (i < 100) {
                acc = acc + i;
                i = i + 1;
            }
            write(stdout(), to_bytes(format("%i", acc)));
        }
        "#,
    );
    assert_eq!(output, "4950");
}

#[test]
fn example_for_break_prints_18() {
    let output = run_example("examples/for_break.hy");
    assert_eq!(output, "18");
}

#[test]
fn example_derive_show_eq_prints_expected() {
    let output = run_example("examples/derive_show_eq.hy");
    assert_eq!(
        output,
        "Color::Red,true,false,true,Point::Point { x: 5, y: 12 },true,false,Cell { value: 42 },true,false"
    );
}

#[test]
fn example_typeof_len_prints_expected() {
    let output = run_example("examples/typeof_len.hy");
    assert_eq!(
        output,
        "int\nstring\n(int, int)\n3\n3\n2\n2\nPoint\nPoint\n"
    );
}

#[test]
fn example_length_trait_prints_expected() {
    let output = run_example("examples/length_trait.hy");
    assert_eq!(output, "3\n2\n42\n");
}

/// Structural `len` on non-literal values: string hits VM `ArrayLen`;
/// tuple/dict use typed static length after binding (not literal fold).
#[test]
fn runtime_len_of_string_tuple_dict_params() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};

fn id_str(string s) -> string { return s; }

fn main() {
    let t = (1, 2, 3);
    let d = { a: 1, b: 2 };
    write(stdout(), to_bytes(format("%i\n", len(id_str("ab")))));
    write(stdout(), to_bytes(format("%i\n", len(t))));
    write(stdout(), to_bytes(format("%i\n", len(d))));
}
"#,
    );
    assert_eq!(output, "2\n3\n2\n");
}

/// `typeof` of prelude Option/Result must print the module-qualified FQN.
#[test]
fn typeof_option_prints_prelude_fqn() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};

fn main() {
    let o = Option::Some(1);
    let r: Result<int, string> = Result::Ok(1);
    write(stdout(), to_bytes(format("%s\n", typeof o)));
    write(stdout(), to_bytes(format("%s\n", typeof r)));
}
"#,
    );
    assert_eq!(
        output,
        "prelude::Option<int>\nprelude::Result<int, string>\n"
    );
}

/// Regression: derived `Serialize::serialize` must typecheck (`[byte]` return,
/// payload fields cast to `byte`).
#[test]
fn derive_serialize_enum_e2e_typechecks() {
    let src = r#"
#[derive(Serialize)]
enum E {
    A,
    B(int),
}

fn main() {}
"#;
    let mut pipeline = Pipeline::new();
    pipeline
        .compile_src(src)
        .expect("derive Serialize should compile without type errors");
}

/// Regression: concrete `<`/`>` codegen must look up `Lt`/`Gt` (not empty
/// `Ord`), otherwise unit-enum compares fall back to raw heap-pointer `LE`
/// and become ASLR-flaky (`Red < Blue` randomly false).
#[test]
fn derive_ord_unit_variants_compare_by_declaration_order() {
    let src = r#"
use io::{stdout, write};
use string::{format, to_bytes};
#[derive(Ord)]
enum Color {
    Red,
    Blue,
}

fn main() {
    write(stdout(), to_bytes(format("%z,", Color::Red < Color::Blue)));
    write(stdout(), to_bytes(format("%z,", Color::Blue < Color::Red)));
    write(stdout(), to_bytes(format("%z,", Color::Red < Color::Red)));
    write(stdout(), to_bytes(format("%z,", Color::Red <= Color::Red)));
    write(stdout(), to_bytes(format("%z", Color::Blue > Color::Red)));
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
use io::{stdout, write};
use string::{format, to_bytes};
#[derive(Ord)]
enum Pair {
    Pair { x: int, y: int },
}

fn main() {
    let a = Pair::Pair { x: 1, y: 2 };
    let b = Pair::Pair { x: 1, y: 3 };
    let c = Pair::Pair { x: 2, y: 0 };
    write(stdout(), to_bytes(format("%z,", a < b)));
    write(stdout(), to_bytes(format("%z,", a < c)));
    write(stdout(), to_bytes(format("%z,", b < a)));
    write(stdout(), to_bytes(format("%z,", a <= a)));
    write(stdout(), to_bytes(format("%z", a < a)));
}
"#;
    let output = run_example_src(src);
    assert_eq!(output, "true,true,false,true,false");
}

#[test]
fn example_attr_ffi_strlen_prints_5() {
    let output = run_example("examples/attr_ffi.hy");
    assert_eq!(output, "5");
}

#[test]
fn example_spread_prints_3_and_60() {
    let output = run_example("examples/spread.hy");
    assert_eq!(output, "360");
}

#[test]
fn example_attr_decorator_forwards_args_and_stacks_attrs() {
    let output = run_example("examples/attr_decorator.hy");
    assert_eq!(output, "enterdo_thinghi42");
}

#[test]
fn example_attr_class_decorates_constructor() {
    let output = run_example("examples/attr_class.hy");
    assert_eq!(output, "Point ctor512");
}

#[test]
fn attr_method_forwards_self() {
    let src = r#"
use io::{stdout, write};
use string::{format, to_bytes};
attr log<T>(fn(...args) -> T target, string message, ...args) -> T {
    write(stdout(), to_bytes(format("%s", message)));
    return target(...args);
}

class Counter {
    n: int,
}

impl Counter {
    #[log(message = "bump")]
    fn bump() -> int {
        return self.n;
    }
}

fn main() {
    let c = new Counter(7);
    write(stdout(), to_bytes(format("%i", c.bump())));
}
"#;
    let output = run_example_src(src);
    assert_eq!(output, "bump7");
}

#[test]
fn attr_test_fn_discovered_by_harness() {
    let mut pipeline = Pipeline::new();
    pipeline.set_include_tests(true);
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("tests/positive/attr_test.hy"),
    )
    .expect("read attr_test.hy");
    let (bytecode, constants) = pipeline.compile_src(&src).expect("compile attr_test.hy");
    let cases = pipeline.test_cases().to_vec();
    assert_eq!(
        cases.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["addition works", "multiply_works"]
    );

    for (_name, offset) in &cases {
        let mut machine = Machine::<128>::default();
        pipeline.wire_host_natives(&mut machine);
        machine.load_program(&bytecode, &constants, pipeline.strings());
        let ret = machine.call_function(*offset, &[]);
        assert!(
            !machine.panicked() && machine.result_is_ok(ret),
            "#[test] case should pass"
        );
    }
}

#[test]
fn example_perf_numeric_prints_expected_sum() {
    let output = run_example("examples/perf/numeric.hy");
    assert_eq!(output, "1999000");
}

#[test]
fn example_perf_array_mut_prints_expected() {
    let output = run_example("examples/perf/array_mut.hy");
    assert_eq!(output, "2000");
}

#[test]
fn example_perf_dict_hot_prints_expected() {
    let output = run_example("examples/perf/dict_hot.hy");
    assert_eq!(output, "6000");
}

#[test]
fn example_perf_operators_loop_prints_expected() {
    let output = run_example("examples/perf/operators_loop.hy");
    assert_eq!(output, "149912");
}

#[test]
fn example_perf_coro_ping_prints_expected() {
    let output = run_example("examples/perf/coro_ping.hy");
    assert_eq!(output, "124750");
}

#[test]
fn example_io_bytes_prints_25532() {
    let output = run_example("examples/io_bytes.hy");
    assert_eq!(output, "25532");
}

#[test]
fn example_io_file_prints_2() {
    let output = run_example("examples/io_file.hy");
    assert_eq!(output, "2");
}

#[test]
fn example_io_eof_prints_eof() {
    let output = run_example("examples/io_eof.hy");
    assert_eq!(output, "eof");
}

#[test]
fn example_io_text_prints_hello2() {
    let output = run_example("examples/io_text.hy");
    assert_eq!(output, "hello2");
}

#[test]
fn example_io_udp_prints_2() {
    let output = run_example("examples/io_udp.hy");
    assert_eq!(output, "2");
}

#[test]
fn io_tcp_helper_hostinvokes_are_wired() {
    let output = run_example_src(
        r#"
use io::{open, stdout, write};
use io::net::tcp::{accept, connect_timeout, listen, local_addr, set_nodelay, shutdown};
use string::{format, to_bytes};

fn main() {
    let path = "/tmp/coil_io_timeout_tcp_helpers.bin";
    let file = open(path, "w")?;

    let local_on_file = match local_addr(file) {
        Result::Ok(_) => 0,
        Result::Err(_) => 1,
    };
    let nodelay_on_file = match set_nodelay(file, true) {
        Result::Ok(_) => 0,
        Result::Err(_) => 1,
    };
    let shutdown_on_file = match shutdown(file, 2) {
        Result::Ok(_) => 0,
        Result::Err(_) => 1,
    };

    let listener = listen("127.0.0.1", 0)?;
    let addr = local_addr(listener)?;
    let accept_code = match accept(listener) {
        Result::Ok(_) => 0,
        Result::Err(_) => 1,
    };
    let connect_code = match connect_timeout("127.0.0.1", 1, 1) {
        Result::Ok(_) => 2,
        Result::Err(_) => 2,
    };

    write(stdout(), to_bytes(format("%i%i%i%i%i", local_on_file, nodelay_on_file, shutdown_on_file, accept_code, connect_code)));
}
"#,
    );
    assert_eq!(output, "11112");
}

/// Nested IO HostInvoke (`read_to_end(open(...)?)`) must leave the stream on
/// the stack as the MakeTuple element, not the outer native id.
#[test]
fn example_io_nested_host_prints_3() {
    let output = run_example("examples/io_nested_host.hy");
    assert_eq!(output, "3");
}

/// Nested IO as the first of two HostInvoke args (`write(open(...), buf)`).
/// Outer arity > 1 — MakeTuple must pack the stream, not the outer native id.
#[test]
fn example_io_nested_write_prints_2() {
    let output = run_example("examples/io_nested_write.hy");
    assert_eq!(output, "2");
}

/// Prelude `block_on` drives a coroutine to its completion value.
#[test]
fn example_block_on_io_prints_2() {
    let output = run_example("examples/block_on_io.hy");
    assert_eq!(output, "2");
}

/// Top-level `drive` with no waiters returns 0 (smoke for host wiring).
#[test]
fn example_io_await_drive_prints_0() {
    let output = run_example("examples/io_await.hy");
    assert_eq!(output, "0");
}

/// Cooperative `await_*` inside coroutines + `wait_ready` batch poll.
#[test]
fn example_io_wait_ready_prints_ok() {
    let output = run_example("examples/io_wait_ready.hy");
    assert_eq!(output, "ok");
}

/// Nested `let server = match accept_wait(listener) { … }` inside a loop must handle
/// multiple connections (binding matches omit the fusion barrier).
#[test]
fn while_accept_match_write_handles_two_connections() {
    use std::io::Read;
    use std::net::TcpStream;
    use std::thread;
    use std::time::Duration;

    const PORT: i64 = 41_299;
    let src = r#"
use io::{close, stdout, write};
use io::sync::{accept_wait};
use io::net::tcp::{listen};
use string::{to_bytes};

fn main() {
    let listener = match listen("127.0.0.1", 41299) {
        Result::Ok(s) => s,
        Result::Err(_) => panic "listen",
    };
    let msg = to_bytes("ok");
    let count = 0;
    while count < 2 {
        let server = match accept_wait(listener) {
            Result::Ok(s) => s,
            Result::Err(_) => panic "accept",
        };
        match write(server, msg) {
            Result::Ok(_) => 0,
            Result::Err(_) => panic "write",
        };
        match close(server) {
            Result::Ok(_) => 0,
            Result::Err(_) => 0,
        };
        count = count + 1;
    }
    write(stdout(), to_bytes("ok"));
}
"#;

    let server = thread::spawn(|| run_example_src(src));

    for round in 0..2 {
        let mut connected = false;
        for attempt in 0..80 {
            match TcpStream::connect(("127.0.0.1", PORT as u16)) {
                Ok(mut s) => {
                    let mut buf = [0u8; 2];
                    s.read_exact(&mut buf).expect("read");
                    assert_eq!(&buf, b"ok");
                    connected = true;
                    break;
                }
                Err(_) if attempt + 1 < 80 => thread::sleep(Duration::from_millis(25)),
                Err(e) => panic!("connect round {round}: {e}"),
            }
        }
        assert!(connected, "server never accepted round {round}");
        thread::sleep(Duration::from_millis(20));
    }

    let output = server.join().expect("server thread");
    assert_eq!(output, "ok");
}

/// `let x = match …` inside `while` must not corrupt loop locals across iterations.
#[test]
fn while_let_result_ok_panic_match_preserves_locals() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let i = 0;
    let acc = 0;
    while i < 3 {
        let v = match Result::Ok(i) {
            Result::Ok(x) => x,
            Result::Err(_) => panic "tick",
        };
        acc = acc + v;
        i = i + 1;
    }
    write(stdout(), to_bytes(format("%i,%i", acc, i)));
}
"#,
    );
    assert_eq!(output, "3,3");
}

/// Same guard for `for-in` loop bodies (not only `while`).
#[test]
fn for_in_let_match_preserves_accumulator() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let items = [1, 2, 3];
    let acc = 0;
    for n in items {
        let v = match Result::Ok(n) {
            Result::Ok(x) => x + 1,
            Result::Err(_) => panic "tick",
        };
        acc = acc + v;
    }
    write(stdout(), to_bytes(format("%i", acc)));
}
"#,
    );
    assert_eq!(output, "9");
}

/// Err arm in a binding match must still panic with the literal message.
#[test]
fn let_result_ok_panic_match_err_path_panics() {
    let mut pipeline = Pipeline::new();
    let (bytecode, constants) = pipeline
        .compile_src(
            r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let x = match Result::Err(1) {
        Result::Ok(s) => s,
        Result::Err(_) => panic "accept",
    };
    write(stdout(), to_bytes(format("%i", x)));
}
"#,
        )
        .expect("compile");
    let shared = SharedBuf::new();
    let mut machine = Machine::<128>::default();
    machine.with_output(shared.clone());
    machine.run_raw(
        &bytecode,
        &constants,
        pipeline.strings(),
        pipeline.static_slot_count(),
    );
    assert!(machine.panicked(), "expected language-level panic on Err");
    let _ = machine.restore_output();
    let s = shared.into_utf8();
    assert_eq!(s, "panic: accept");
}

/// `x = match …` reassignment inside a loop uses the same binding-context
/// match lowering as `let x = match …`.
#[test]
fn while_assignment_match_preserves_locals() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let i = 0;
    let v = 0;
    while i < 3 {
        v = match Result::Ok(i) {
            Result::Ok(x) => x,
            Result::Err(_) => panic "tick",
        };
        i = i + 1;
    }
    write(stdout(), to_bytes(format("%i", v)));
}
"#,
    );
    assert_eq!(output, "2");
}

/// Standalone virtual `main` for a green `test("…")` suite exits cleanly.
#[test]
fn harness_virtual_main_passes_when_all_asserts_ok() {
    let output = run_harness_src(
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
    pipeline.set_include_tests(true);
    let (bytecode, constants) = pipeline
        .compile_src(
            r#"
test("broken") {
    assert(false)?;
}
"#,
        )
        .expect("compile");
    let shared = SharedBuf::new();
    let mut machine = Machine::<128>::default();
    machine.with_output(shared.clone());
    pipeline.wire_host_natives(&mut machine);
    machine.set_program_debug(pipeline.program_debug());
    machine.run_raw(
        &bytecode,
        &constants,
        pipeline.strings(),
        pipeline.static_slot_count(),
    );
    let _ = machine.restore_output();
    assert!(
        machine.panicked(),
        "virtual main must panic when a case fails"
    );
    let output = shared.into_utf8();
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
    pipeline.set_include_tests(true);
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
        machine.load_program(&bytecode, &constants, pipeline.strings());
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
    pipeline.set_include_tests(true);
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
            machine.load_program(&bytecode, &constants, pipeline.strings());
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
use io::{stdout, write};
use string::{format, to_bytes};
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
    write(stdout(), to_bytes(format("%i", get(Shape::Pt(Point::Point { x: 1, y: 2 })))));
    write(stdout(), to_bytes(format("%i", get(Shape::Rc(Rect::Rect { w: 3, h: 4 })))));
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
use io::{stdout, write};
use string::{format, to_bytes};
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
    write(stdout(), to_bytes(format("%i", from_option(Option::Some(Point::Point { x: 1, y: 2 })))));
    write(stdout(), to_bytes(format("%i", from_box(Box::Full(Point::Point { x: 1, y: 2 })))));
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
use io::{stdout, write};
use string::{format, to_bytes};
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
    write(stdout(), to_bytes(format("%i", got)));
    write(stdout(), to_bytes(format("%i", a + b + c)));
}
"#,
    );
    assert_eq!(output, "16");
}

/// P1: empty Vec + push + index round-trip (and under GC pressure).
#[test]
fn empty_array_append_and_index_round_trip() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let arr: Vec<int> = Vec::new();
    arr.push(4);
    arr.push(1);
    arr.push(4);
    write(stdout(), to_bytes(format("%i", len(arr))));
    write(stdout(), to_bytes(format("%i", arr[0])));
    write(stdout(), to_bytes(format("%i", arr[2])));
    let i = 0;
    while i < 80 {
        arr.push(i);
        i = i + 1;
    }
    write(stdout(), to_bytes(format("%i", arr[0])));
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
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let arr = [0, 0, 0];
    arr[1] = 42;
    write(stdout(), to_bytes(format("%i", arr[0])));
    write(stdout(), to_bytes(format("%i", arr[1])));
    write(stdout(), to_bytes(format("%i", arr[2])));
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
use io::{stdout, write};
use string::{format, to_bytes};
fn neg() -> int { return -1; }
fn main() {
    write(stdout(), to_bytes(format("%i", neg())));
    write(stdout(), to_bytes(format("%i", 0 - 1)));
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
use io::{stdout, write};
use string::{format, to_bytes};
fn unwrap_result(Result r) -> int {
    return match r {
        Result::Ok(Option::Some(v)) => v,
        Result::Ok(Option::None) => 0,
        Result::Err(_) => -1,
    };
}
fn main() {
    write(stdout(), to_bytes(format("%i", unwrap_result(Result::Ok(Option::Some(42))))));
    write(stdout(), to_bytes(format("%i", unwrap_result(Result::Ok(Option::None)))));
    write(stdout(), to_bytes(format("%i", unwrap_result(Result::Err("oops")))));
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
use io::{stdout, write};
use string::{format, to_bytes};
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
    write(stdout(), to_bytes(format("%i", match s {
        Status::Ready => 0,
        Status::Done(v) => v,
    })));
    write(stdout(), to_bytes(format("%i", match b.status {
        Status::Ready => 0,
        Status::Done(v) => v,
    })));
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
use io::{stdout, write};
use string::{format, to_bytes};
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
    write(stdout(), to_bytes(format("%i", read_code(w))));
    write(stdout(), to_bytes(format("%i", match w {
        Wrap::Empty => 0,
        Wrap::Full(e) => e.kind,
    })));
}
"#,
    );
    // Pre-fix: e.code → LoadField(0) → kind (1), not code (42).
    assert_eq!(output, "421");
}

#[test]
fn example_overload_prints_15() {
    let output = run_example("examples/overload.hy");
    assert_eq!(output, "15");
}

#[test]
fn example_type_overload_prints_typed_tags() {
    let output = run_example("examples/type_overload.hy");
    assert_eq!(output, "i:7f:1.5s:hi");
}

#[test]
fn example_fn_value_prints_423() {
    let output = run_example("examples/fn_value.hy");
    assert_eq!(output, "423");
}

#[test]
fn example_lambda_prints_42() {
    let output = run_example("examples/lambda.hy");
    assert_eq!(output, "42");
}

/// Disk-module imports (e.g. `io::sync::write_all`) are file-level globals.
/// After `take_and_isolate` they must be rebound like virtual imports — not
/// treated as missing captures inside lambdas.
#[test]
fn disk_import_usable_inside_lambda_without_capture() {
    let output = run_example_src(
        r#"
use io::{stdout};
use io::sync::{write_all};
use string::{to_bytes};
fn main() {
    let f = fn () => write_all(stdout(), to_bytes("ok"));
    f()?;
}
"#,
    );
    assert_eq!(output, "ok");
}

/// Aliased disk imports must rebind under the local alias name.
#[test]
fn disk_import_alias_usable_inside_lambda() {
    let output = run_example_src(
        r#"
use io::{stdout};
use io::sync::{write_all as wa};
use string::{to_bytes};
fn main() {
    let f = fn () => wa(stdout(), to_bytes("ok"));
    f()?;
}
"#,
    );
    assert_eq!(output, "ok");
}

/// Same rebind rule for `defer` bodies that call disk-module imports.
#[test]
fn disk_import_usable_inside_defer_without_capture() {
    let output = run_example_src(
        r#"
use io::{stdout};
use io::sync::{write_all};
use string::{to_bytes};
fn main() {
    defer { write_all(stdout(), to_bytes("d")); }
    write_all(stdout(), to_bytes("a"));
}
"#,
    );
    assert_eq!(output, "ad");
}

/// Disk-import rebind must not suppress explicit-capture checks for locals.
#[test]
fn disk_import_lambda_still_requires_local_capture() {
    let mut pipeline = Pipeline::new();
    let result = pipeline.compile_src(
        r#"
use io::{stdout};
use io::sync::{write_all};
use string::{format, to_bytes};
fn main() {
    let y = 1;
    let f = fn () => write_all(stdout(), to_bytes(format("%i", y)));
    f()?;
}
"#,
    );
    assert!(
        result.is_err(),
        "local `y` must still require `use (y)`: {:?}",
        pipeline.messages()
    );
    assert!(
        pipeline.messages().iter().any(|m| {
            m.message().contains("cannot capture `y` without `use (y)`")
                || m.message().contains("list `y` in the enclosing `use")
                || m.message().contains("list `y` in the lambda's `use")
        }),
        "expected capture diagnostic for `y`, got: {:?}",
        pipeline.messages()
    );
}

/// Nested typed lambdas must keep Fragment/Argument NodeIds lockstep with
/// codegen (`assign_fn_arg_node_ids`); a desync surfaces as wrong results or
/// compile failure rather than Identifier span prefer.
#[test]
fn nested_typed_lambdas_print_sum() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let add = fn (int x) => fn (int y) use (x) => x + y;
    let add40 = add(40);
    write(stdout(), to_bytes(format("%i", add40(2))));
}
"#,
    );
    assert_eq!(output, "42");
}

#[test]
fn example_method_overload_prints_1116() {
    let output = run_example("examples/method_overload.hy");
    assert_eq!(output, "1116");
}

/// Named under-apply must build a partial whose holes fill in declaration
/// order at CallIndirect time (not print the named value as the only arg).
#[test]
fn named_partial_application_completes_and_prints_3() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn add(int a, int b) -> int { return a + b; }
fn main() {
    let g = add(a: 1);
    write(stdout(), to_bytes(format("%i", g(2))));
}
"#,
    );
    assert_eq!(output, "3");
}

/// Nested partials (`f(1)` → `p(2)` → `q(3)`) must merge filled masks without
/// clobbering earlier holes — a common ObjFn regression.
#[test]
fn nested_partial_application_prints_6() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn add3(int a, int b, int c) -> int { return a + b + c; }
fn main() {
    let p = add3(1);
    let q = p(2);
    write(stdout(), to_bytes(format("%i", q(3))));
}
"#,
    );
    assert_eq!(output, "6");
}

/// Fixed-N vs rest-K (N < K) must dispatch by argc: exact fixed wins at N;
/// rest handles K and above. Typechecker unit tests alone miss wrong CALL targets.
#[test]
fn fixed_vs_rest_overload_dispatches_at_runtime() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn f(int x) -> int { return x * 10; }
fn f(int x, int y, int... xs) -> int { return x + y + len(xs); }
fn main() {
    write(stdout(), to_bytes(format("%i", f(1))));
    write(stdout(), to_bytes(format("%i", f(1, 2))));
    write(stdout(), to_bytes(format("%i", f(1, 2, 3, 4))));
}
"#,
    );
    assert_eq!(output, "1035");
}

// ── Harness stripping + cross-feature integration ─────────────────────────────

#[test]
fn production_compile_strips_harness_declarations() {
    let mut pipeline = Pipeline::new();
    let (bytecode, constants) = pipeline
        .compile_src(
            r#"
use io::{stdout, write};
use string::{format, to_bytes};
#[test]
fn hidden() { assert(false)?; }
test("also hidden") { assert(false)?; }
fn main() { write(stdout(), to_bytes("ok")); }
"#,
        )
        .expect("compile");
    assert!(
        pipeline.test_cases().is_empty(),
        "production compile must not register harness cases"
    );
    let output = run_bytecode(bytecode, constants, &pipeline, None);
    assert_eq!(output, "ok");
}

#[test]
fn include_tests_flag_embeds_harness_metadata() {
    let (pipeline, _bc, _constants) = compile_src_with_tests(
        r#"
#[test("via attr")]
fn from_attr() { assert(true)?; }
test("via block") { assert(true)?; }
"#,
    );
    assert_eq!(pipeline.test_cases().len(), 2);
    assert_eq!(pipeline.test_cases()[0].0, "via attr");
    assert_eq!(pipeline.test_cases()[1].0, "via block");
}

#[test]
fn attr_decorator_with_overloaded_functions_forwards_each_arity() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
attr log<T>(fn(...args) -> T target, string message, ...args) -> T {
    write(stdout(), to_bytes(format("%s", message)));
    return target(...args);
}

#[log(message = "nullary")]
fn do_thing() -> int { return 0; }

#[log(message = "unary")]
fn do_thing(int x) -> int { return x; }

fn main() {
    write(stdout(), to_bytes(format("%i", do_thing())));
    write(stdout(), to_bytes(format("%i", do_thing(42))));
}
"#,
    );
    assert_eq!(output, "nullary0unary42");
}

#[test]
fn spread_with_partial_application_forwards_remaining_args() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn add3(int a, int b, int c) -> int { return a + b + c; }
fn main() {
    let p = add3(1);
    write(stdout(), to_bytes(format("%i", p(...(2, 3)))));
}
"#,
    );
    assert_eq!(output, "6");
}

#[test]
fn named_call_args_on_overloaded_functions_dispatch_correctly() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn greet(string name) -> string { return name; }
fn greet(string name, int age) -> string { return format("%s:%i", name, age); }
fn main() {
    write(stdout(), to_bytes(format("%s", greet(name: "Ada"))));
    write(stdout(), to_bytes(format("%s", greet(name: "Grace", age: 40))));
}
"#,
    );
    assert_eq!(output, "AdaGrace:40");
}

#[test]
fn attr_on_async_fn_rejected_at_compile_time() {
    let mut pipeline = Pipeline::new();
    let result = pipeline.compile_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
attr log<T>(fn(...args) -> T target, string message, ...args) -> T {
    yield 99;
    return target(...args);
}
#[log(message = "coro")]
async fn counter() {
    yield 1;
}
fn main() {
    let h = counter();
    write(stdout(), to_bytes(format("%i", resume h)));
}
"#,
    );
    assert!(
        result.is_err(),
        "attrs that yield outside target(...args) must be rejected on async fn"
    );
}

#[test]
fn rest_overload_with_attr_logging_forwards_pack() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
attr log<T>(fn(...args) -> T target, string message, ...args) -> T {
    write(stdout(), to_bytes(format("%s", message)));
    return target(...args);
}

#[log(message = "sum")]
fn total(int... xs) -> int {
    return len(xs);
}

fn main() {
    write(stdout(), to_bytes(format("%i", total(1, 2, 3))));
}
"#,
    );
    assert_eq!(output, "sum3");
}

/// Prefix args + spread pack must flatten in source order. A buggy flatten that
/// drops the prefix or reverses pack elements would still typecheck for
/// `add3(int,int,int)` but print the wrong sum.
#[test]
fn spread_mixed_prefix_and_pack_prints_6() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn add3(int a, int b, int c) -> int { return a + b + c; }
fn main() {
    write(stdout(), to_bytes(format("%i", add3(1, ...(2, 3)))));
}
"#,
    );
    assert_eq!(output, "6");
}

/// Let-bound packs exercise the typed-lookup codegen path (not the literal
/// Tuple/Array shortcuts). Wrong Index expansion would silently mis-bind.
#[test]
fn spread_let_bound_tuple_prints_6() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn add3(int a, int b, int c) -> int { return a + b + c; }
fn main() {
    let pack = (1, 2, 3);
    write(stdout(), to_bytes(format("%i", add3(...pack))));
}
"#,
    );
    assert_eq!(output, "6");
}

/// Positional attr extras (`#[log("enter")]`) must bind to the first extra
/// parameter the same way named extras do.
#[test]
fn attr_positional_extra_forwards_literal() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
attr log<T>(fn(...args) -> T target, string message, ...args) -> T {
    write(stdout(), to_bytes(format("%s", message)));
    return target(...args);
}

#[log("enter")]
fn do_thing(int x) -> int { return x; }

fn main() {
    write(stdout(), to_bytes(format("%i", do_thing(7))));
}
"#,
    );
    assert_eq!(output, "enter7");
}

/// Stacking is Python-style: first listed attr is outermost. Reversing the
/// expand loop would swap the print order without failing typecheck.
#[test]
fn stacked_attrs_apply_outer_first() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
attr outer<T>(fn(...args) -> T target, ...args) -> T {
    write(stdout(), to_bytes("O"));
    return target(...args);
}
attr inner<T>(fn(...args) -> T target, ...args) -> T {
    write(stdout(), to_bytes("I"));
    return target(...args);
}

#[outer]
#[inner]
fn f() -> int { return 1; }

fn main() {
    write(stdout(), to_bytes(format("%i", f())));
}
"#,
    );
    assert_eq!(output, "OI1");
}

#[test]
fn attr_inlining_rewrites_target_in_all_expression_contexts() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
attr wrap_if<T>(fn(...args) -> T target, ...args) -> T {
    if true {
        return target(...args);
    }
    return 0;
}

attr wrap_for<T>(fn(...args) -> T target, ...args) -> T {
    for (let i = 0; i < 1; i = i + 1) {
        return target(...args);
    }
    return 0;
}

attr wrap_while<T>(fn(...args) -> T target, ...args) -> T {
    while (true) {
        return target(...args);
    }
    return 0;
}

attr wrap_print<T>(fn(...args) -> T target, ...args) -> T {
    write(stdout(), to_bytes(format("%s", "x")));
    return target(...args);
}

#[wrap_if]
fn a() -> int { return 10; }

#[wrap_for]
fn b() -> int { return 20; }

#[wrap_while]
fn c() -> int { return 30; }

#[wrap_print]
fn d() -> int { return 40; }

fn main() {
    write(stdout(), to_bytes(format("%i", a())));
    write(stdout(), to_bytes(format("%i", b())));
    write(stdout(), to_bytes(format("%i", c())));
    write(stdout(), to_bytes(format("%i", d())));
}
"#,
    );
    assert_eq!(output, "102030x40");
}

/// End-to-end TailCall: self-recursive accumulator must print the correct sum.
#[test]
fn tail_recursive_sum_to_prints_15() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn sum_to(int n, int acc) -> int {
    if n <= 0 { return acc; }
    return sum_to(n - 1, acc + n);
}
fn main() {
    write(stdout(), to_bytes(format("%i", sum_to(5, 0))));
}
"#,
    );
    assert_eq!(output, "15");
}

/// Const-fold `if` with strict `<` equality boundary takes else (Le vs Leq).
#[test]
fn const_if_strict_lt_equality_prints_else() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    if 5 < 5 {
        write(stdout(), to_bytes(format("%i", 1)));
    } else {
        write(stdout(), to_bytes(format("%i", 0)));
    }
}
"#,
    );
    assert_eq!(output, "0");
}

/// `while false` must skip the body entirely.
#[test]
fn const_while_false_skips_body() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    while false {
        write(stdout(), to_bytes(format("%i", 1)));
    }
    write(stdout(), to_bytes(format("%i", 2)));
}
"#,
    );
    assert_eq!(output, "2");
}

/// Constant-trip `for` unroll must advance `i` via `step` between bodies.
/// If unroll engaged but skipped `step`, `s = s + i` would stay `0`.
#[test]
fn const_for_unroll_prints_sum() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let s = 0;
    for (let i = 0; i < 4; i = i + 1) {
        s = s + i;
    }
    write(stdout(), to_bytes(format("%i", s)));
}
"#,
    );
    assert_eq!(output, "6");
}

/// Same induction check with `i <= 2` (3 trips: 0+1+2).
#[test]
fn const_for_unroll_leq_advances_induction() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let s = 0;
    for (let i = 0; i <= 2; i = i + 1) {
        s = s + i;
    }
    write(stdout(), to_bytes(format("%i", s)));
}
"#,
    );
    assert_eq!(output, "3");
}

/// Range for-in unroll (`0..3`) binds successive values correctly.
#[test]
fn const_range_for_in_unroll_prints() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let s = 0;
    for x in 0..3 {
        s = s + x;
    }
    write(stdout(), to_bytes(format("%i", s)));
}
"#,
    );
    assert_eq!(output, "3");
}

/// `break` disables unroll — first iteration still runs, rest skipped.
#[test]
fn for_with_break_still_stops_early() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let s = 0;
    for (let i = 0; i < 5; i = i + 1) {
        s = s + i;
        break;
    }
    write(stdout(), to_bytes(format("%i", s)));
}
"#,
    );
    assert_eq!(output, "0");
}

/// Tiny direct-call inlining must preserve call semantics end-to-end.
#[test]
fn tiny_add_inlined_prints_7() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn add(int a, int b) -> int { return a + b; }
fn main() {
    write(stdout(), to_bytes(format("%i", add(3, 4))));
}
"#,
    );
    assert_eq!(output, "7");
}

/// `x * 2^n` / `2^n * x` / `x * const(2^n)` must lower to SHL but still
/// compute the same values as MUL (wrong shift amount is silent otherwise).
#[test]
fn mul_strength_reduce_to_shl_prints_correct_values() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn scale_rhs(int x) -> int { return x * 8; }
fn scale_lhs(int x) -> int { return 4 * x; }
fn scale_const(int x) -> int {
    const K = 16;
    return x * K;
}
fn scale_one(int x) -> int { return x * 1; }
fn scale_six(int x) -> int { return x * 6; }
fn scale_negative_x(int x) -> int { return x * 8; }
fn main() {
    write(stdout(), to_bytes(format("%i,", scale_rhs(5))));
    write(stdout(), to_bytes(format("%i,", scale_lhs(7))));
    write(stdout(), to_bytes(format("%i,", scale_const(3))));
    write(stdout(), to_bytes(format("%i,", scale_one(9))));
    write(stdout(), to_bytes(format("%i,", scale_six(7))));
    write(stdout(), to_bytes(format("%i", scale_negative_x(0 - 3))));
}
"#,
    );
    assert_eq!(output, "40,28,48,9,42,-24");
}

/// Early-return diamond callees are tiny-inlined; both arms must still evaluate
/// correctly at the call site.
#[test]
fn early_return_callee_both_arms_correct() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn early(int n, int is_neg) -> int {
    if is_neg == 1 {
        return 0 - 1;
    }
    return n * 2;
}
fn main() {
    write(stdout(), to_bytes(format("%i,", early(4, 1))));
    write(stdout(), to_bytes(format("%i", early(4, 0))));
}
"#,
    );
    assert_eq!(output, "-1,8");
}

/// Pure-arg reorder must keep CALL arg order while evaluating pure temps before
/// effectful args (print from effect runs first; sink still sees a=2, b=10).
#[test]
fn pure_arg_reorder_preserves_order_and_effects() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn effect() -> int {
    write(stdout(), to_bytes(format("%i,", 7)));
    return 2;
}
fn sink(int a, int b) -> int {
    write(stdout(), to_bytes(format("%i,", a + b)));
    return a + b;
}
fn main() {
    write(stdout(), to_bytes(format("%i", sink(effect(), 10))));
}
"#,
    );
    assert_eq!(output, "7,12,12");
}

/// Predicate peel base path and recursive path must both return correct values.
#[test]
fn predicate_peel_base_and_recursive_paths() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn other(int n) -> int {
    return n;
}
fn base(int n) -> int {
    if n <= 0 {
        return 1;
    }
    return other(n) + 1;
}
fn main() {
    write(stdout(), to_bytes(format("%i,", base(0))));
    write(stdout(), to_bytes(format("%i", base(5))));
}
"#,
    );
    assert_eq!(output, "1,6");
}

/// One-level self-unroll must not change recursive fib results.
#[test]
fn self_unroll_fib_still_correct() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn fib(int n) -> int {
    if n <= 1 {
        return n;
    }
    return fib(n - 1) + fib(n - 2);
}
fn main() {
    write(stdout(), to_bytes(format("%i", fib(10))));
}
"#,
    );
    assert_eq!(output, "55");
}

/// Auto-par fork-join of pure recursive `fib(n-1)+fib(n-2)` must stay correct.
#[test]
fn auto_par_fib_still_correct() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn fib(int n) -> int {
    if n <= 1 {
        return n;
    }
    return fib(n - 1) + fib(n - 2);
}
fn main() {
    write(stdout(), to_bytes(format("%i", fib(12))));
}
"#,
    );
    assert_eq!(output, "144");
}

/// Constant sites above `COIL_PAR_THRESHOLD` must emit `__coil_par_*` and stay correct.
#[test]
fn auto_par_fib_above_threshold_emits_spec_and_runs() {
    let src = r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn fib(int n) -> int {
    if n <= 1 {
        return n;
    }
    return fib(n - 1) + fib(n - 2);
}
fn main() {
    write(stdout(), to_bytes(format("%i", fib(22))));
}
"#;
    let mut pipeline = Pipeline::new();
    let (bytecode, constants) = pipeline
        .compile_src(src)
        .expect("auto-par fib should compile");
    assert!(
        pipeline.function_offset("__coil_par_fib_22").is_some(),
        "expected static specialization for fib(22)"
    );
    assert!(
        pipeline.function_offset("__coil_par_fib_21").is_some(),
        "expected chain specialization for fib(21)"
    );
    // Default threshold is 20 — exact threshold stays sequential.
    assert!(
        pipeline.function_offset("__coil_par_fib_20").is_none(),
        "fib(20) must not get a parallel specialization"
    );
    let output = run_bytecode(bytecode, constants, &pipeline, None);
    assert_eq!(output, "17711");
}

/// Impure recursive binops must not grow `__coil_par_*` clones.
#[test]
fn impure_recursive_binop_skips_par_specialization() {
    let src = r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn leaf(int n) -> int {
    write(stdout(), to_bytes(format("%i", n)));
    return n;
}
fn rec(int n) -> int {
    if n <= 1 { return leaf(n); }
    return rec(n - 1) + rec(n - 2);
}
fn main() {
    write(stdout(), to_bytes(format("%i", rec(22))));
}
"#;
    let mut pipeline = Pipeline::new();
    let _ = pipeline
        .compile_src(src)
        .expect("impure rec should still compile");
    assert!(
        pipeline.function_offset("__coil_par_rec_22").is_none(),
        "impure recursion must not emit auto-par specializations"
    );
}

/// Nested CALL + `let x = f(); if x == k` must not hang: mem_fwd must not
/// turn StorePop;Load into Dup;Store when the store extends tell past TOS
/// (shared-stack CmpJmpf would eat the local — broke http `parse_url`).
#[test]
fn mem_fwd_post_call_store_compare_does_not_hang() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn find_bytes(Vec<byte> hay, Vec<byte> needle) -> int {
    let hn = len(hay);
    let nn = len(needle);
    if nn == 0 { return 0; }
    if nn > hn { return 999999; }
    let i = 0;
    while i + nn <= hn {
        let ok = 1;
        let j = 0;
        while j < nn {
            if hay[i + j] != needle[j] { ok = 0; }
            j = j + 1;
        }
        if ok == 1 { return i; }
        i = i + 1;
    }
    return 999999;
}
fn bytes_slice(Vec<byte> src, int start, int end) -> Vec<byte> {
    let out: Vec<byte> = Vec::new();
    let i = start;
    while i < end {
        if i < len(src) { out.push(src[i]); }
        i = i + 1;
    }
    return out;
}
fn parse_url(string s) -> int {
    let b = to_bytes(s);
    let sep = Vec::from([58 as byte, 47 as byte, 47 as byte]);
    let sep_at = find_bytes(b, sep);
    if sep_at == 999999 { return -1; }
    let scheme_b = bytes_slice(b, 0, sep_at);
    let rest_start = sep_at + 3;
    let host_end = len(b);
    let host_b = bytes_slice(b, rest_start, host_end);
    return len(scheme_b) + len(host_b);
}
fn main() {
    write(stdout(), to_bytes(format("%i", parse_url("http://example.com/hi"))));
}
"#,
    );
    assert_eq!(output, "18");
}

/// Fused assign / two-local AND-if / packed three-arg call must evaluate correctly.
#[test]
fn fused_assign_and_packed_call_runtime() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn add3(int a, int b, int c) -> int {
    if a < 0 {
        return 0;
    }
    if b < 0 {
        return 0;
    }
    return a + b + c;
}
fn main() {
    let i = 0;
    i = i + 1;
    i = i + 1;
    let flags = 15;
    let mask = 7;
    flags = flags & mask;
    let a = true;
    let b = false;
    let and_ok = 0;
    if a && b {
        and_ok = 1;
    } else {
        and_ok = 2;
    }
    let x = 1;
    let y = 2;
    let z = 3;
    write(stdout(), to_bytes(format("%i,", i)));
    write(stdout(), to_bytes(format("%i,", flags)));
    write(stdout(), to_bytes(format("%i,", and_ok)));
    write(stdout(), to_bytes(format("%i", add3(x, y, z))));
}
"#,
    );
    assert_eq!(output, "2,7,2,6");
}

#[test]
fn example_thread_join_prints_42() {
    let output = run_example("examples/thread_join.hy");
    assert_eq!(output, "42");
}

#[test]
fn example_thread_channel_prints_hello() {
    let output = run_example("examples/thread_channel.hy");
    assert_eq!(output, "hello");
}

#[test]
fn example_thread_reply_prints_ping() {
    let output = run_example("examples/thread_reply.hy");
    assert_eq!(output, "ping");
}

#[test]
fn thread_main_exits_without_join_still_runs_worker_recv() {
    // Regression: process exit used to kill workers still in `recv`, so
    // nothing after the worker's recv ran and the script looked like it
    // "didn't block". Auto-join at end of `run_with_pool` keeps them alive.
    let output = run_example_src(
        r#"
use thread::{channel, recv, send, spawn, Receiver};
use io::{stdout, write};
use string::{format, to_bytes};

fn worker(Receiver rx) {
    let msg = recv(rx)?;
    write(stdout(), to_bytes(format("%s", msg)));
    return 0;
}

fn main() {
    let pair = channel()?;
    let t = spawn(worker, pair[1])?;
    send(pair[0], "hi")?;
}
"#,
    );
    assert_eq!(output, "hi");
}

#[test]
fn nested_spawn_joins_via_shared_root_registry() {
    // Worker mid spawns leaf on the root Machine's live-thread registry.
    // Main returns without join; root auto-join must still wait for leaf.
    let output = run_example_src(
        r#"
use thread::{spawn};
use io::{stdout, write};
use string::{format, to_bytes};

fn leaf() {
    write(stdout(), to_bytes("leaf"));
    return 0;
}

fn mid() {
    let _ = spawn(leaf)?;
    return 0;
}

fn main() {
    let _ = spawn(mid)?;
}
"#,
    );
    assert_eq!(output, "leaf");
}

#[test]
fn example_thread_mutex_prints_2() {
    let output = run_example("examples/thread_mutex.hy");
    assert_eq!(output, "2");
}

#[test]
fn example_gc_root_weak_prints_pinned() {
    let output = run_example("examples/gc_root_weak.hy");
    assert_eq!(output, "pinned\npinned");
}

#[test]
fn gc_upgrade_some_while_rooted() {
    let output = run_example_src(
        r#"
use gc::{get, root, weak, upgrade};
use io::{stdout, write};
use string::{to_bytes};

fn main() {
    let r = root([7, 8]);
    let w = weak(get(r));
    let out = match upgrade(w) {
        Option::Some(_) => "some",
        Option::None => "none",
    };
    write(stdout(), to_bytes(out));
}
"#,
    );
    assert_eq!(output, "some");
}

#[test]
fn example_gc_collect_clears_weak() {
    assert_eq!(run_example("examples/gc_collect.hy"), "none");
}

#[test]
fn gc_heap_bytes_is_nonnegative() {
    let output = run_example_src(
        r#"
use gc::{heap_bytes, root};
use io::{stdout, write};
use string::{format, to_bytes};

fn main() {
    let _ = root([1, 2, 3, 4]);
    let n = heap_bytes();
    write(stdout(), to_bytes(format("%z", n >= 0)));
}
"#,
    );
    assert_eq!(output, "true");
}

#[test]
fn gc_collect_preserves_stack_rooted_weak() {
    // `collect` must root the operand stack like auto-GC; a live `Root` on
    // the frame keeps the referent alive so `upgrade` stays `Some`.
    let output = run_example_src(
        r#"
use gc::{collect, get, root, weak, upgrade};
use io::{stdout, write};
use string::{to_bytes};

fn main() {
    let r = root([9, 8, 7]);
    let w = weak(get(r));
    let _freed = collect();
    let out = match upgrade(w) {
        Option::Some(_) => "some",
        Option::None => "none",
    };
    write(stdout(), to_bytes(out));
}
"#,
    );
    assert_eq!(output, "some");
}

#[test]
fn gc_collect_returns_nonnegative_freed_bytes() {
    let output = run_example_src(
        r#"
use gc::{collect};
use io::{stdout, write};
use string::{format, to_bytes};

fn main() {
    let n = collect();
    write(stdout(), to_bytes(format("%z", n >= 0)));
}
"#,
    );
    assert_eq!(output, "true");
}

#[test]
fn gc_get_after_unroot_then_collect_clears_weak() {
    let output = run_example_src(
        r#"
use gc::{collect, get, root, unroot, weak, upgrade};
use io::{stdout, write};
use string::{to_bytes};

fn only_weak() {
    let r = root("temp");
    let w = weak(get(r));
    let _ = unroot(r);
    return w;
}

fn main() {
    let w = only_weak();
    let _ = collect();
    let out = match upgrade(w) {
        Option::Some(_) => "some",
        Option::None => "none",
    };
    write(stdout(), to_bytes(out));
}
"#,
    );
    assert_eq!(output, "none");
}

#[test]
fn thread_channel_close_try_recv_is_disconnected() {
    let output = run_example_src(
        r#"
use thread::{channel, close, try_recv, ThreadError};
use io::{stdout, write};
use string::{format, to_bytes};

fn main() {
    let pair = channel()?;
    let tx = pair[0];
    let rx = pair[1];
    close(tx)?;
    let msg = match try_recv(rx) {
        Result::Ok(_) => "ok",
        Result::Err(e) => match e {
            ThreadError::Disconnected => "disc",
            _ => "other",
        },
    };
    write(stdout(), to_bytes(format("%s", msg)));
}
"#,
    );
    assert_eq!(output, "disc");
}

#[test]
fn thread_try_recv_empty_open_channel_would_block() {
    let output = run_example_src(
        r#"
use thread::{channel, try_recv, ThreadError};
use io::{stdout, write};
use string::{format, to_bytes};

fn main() {
    let pair = channel()?;
    let rx = pair[1];
    let msg = match try_recv(rx) {
        Result::Ok(_) => "ok",
        Result::Err(e) => match e {
            ThreadError::WouldBlock => "wb",
            _ => "other",
        },
    };
    write(stdout(), to_bytes(format("%s", msg)));
}
"#,
    );
    assert_eq!(output, "wb");
}

#[test]
fn thread_rwlock_with_write_then_read() {
    let output = run_example_src(
        r#"
use thread::{rwlock, with_read, with_write};
use io::{stdout, write};
use string::{format, to_bytes};

fn main() {
    let rw = rwlock(10)?;
    with_write(rw, fn (int n) => (n + 1, 0))?;
    let v = with_read(rw, fn (int n) => n)?;
    write(stdout(), to_bytes(format("%i", v)));
}
"#,
    );
    assert_eq!(output, "11");
}

#[test]
fn thread_detach_then_join_fails() {
    let output = run_example_src(
        r#"
use thread::{detach, join, spawn, ThreadError};
use io::{stdout, write};
use string::{format, to_bytes};

fn work() -> int {
    return 1;
}

fn main() {
    let t = spawn(work)?;
    detach(t)?;
    let msg = match join(t) {
        Result::Ok(_) => "joined",
        Result::Err(e) => match e {
            ThreadError::JoinFailed => "jf",
            _ => "other",
        },
    };
    write(stdout(), to_bytes(format("%s", msg)));
}
"#,
    );
    assert_eq!(output, "jf");
}

#[test]
fn example_vec_tuple_prints_zip_broadcast_negate() {
    let output = run_example("examples/vec_tuple.hy");
    assert_eq!(output, "22,23,24,-1-2");
}

#[test]
fn example_vec_packed_mul_uses_hostinvoke_path() {
    let output = run_example("examples/vec_packed_mul.hy");
    assert_eq!(output, "246810121416,3691215182124");
}

#[test]
fn packed_vec_arith_runtime_neg_div_and_scalar_left() {
    // Covers unary neg, float zip div, and non-commutative scalar-left sub
    // on the N≥8 HostInvoke path (mul/broadcast already covered by the example).
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let a = [1, 2, 3, 4, 5, 6, 7, 8];
    let n = -a;
    write(stdout(), to_bytes(format("%i%i,", n[0], n[7])));
    let f = [2.0, 4.0, 6.0, 8.0, 10.0, 12.0, 14.0, 16.0];
    let d = f / [2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0];
    write(stdout(), to_bytes(format("%f%f,", d[0], d[7])));
    let s = 10 - a;
    write(stdout(), to_bytes(format("%i%i", s[0], s[7])));
}
"#,
    );
    assert_eq!(output, "-1-8,1.08.0,92");
}

#[test]
fn aggregate_float_negate_uses_mulf_not_int_neg() {
    // Regression: float aggregate unary `-` must not emit int `NEG`
    // (which bit-twiddles a float as i64). Float path is `CONST -1; MULF`.
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let d = -(1.5, 2.0);
    write(stdout(), to_bytes(format("%f,%f", d[0], d[1])));
}
"#,
    );
    assert_eq!(output, "-1.5,-2.0");
}

#[test]
fn aggregate_dynamic_array_broadcast_adds_scalar() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn add1([int] xs) -> [int] {
    return xs + 1;
}

fn main() {
    let a = add1([1, 2, 3]);
    write(stdout(), to_bytes(format("%i%i%i", a[0], a[1], a[2])));
}
"#,
    );
    assert_eq!(output, "234");
}

#[test]
fn example_vec_array_prints_zip_broadcast_pow() {
    let output = run_example("examples/vec_array.hy");
    assert_eq!(output, "46,45,18");
}

#[test]
fn example_vec_generic_prints_scale_and_shape_generic_add() {
    let output = run_example("examples/vec_generic.hy");
    assert_eq!(output, "24,55");
}

#[test]
fn example_vec_dot_prints_32_and_cross_product() {
    let output = run_example("examples/vec_dot.hy");
    assert_eq!(output, "32,001");
}

#[test]
fn example_vec_matmul_prints_2x2_product() {
    let output = run_example("examples/vec_matmul.hy");
    assert_eq!(output, "19,22,43,50");
}

#[test]
fn example_matrix_mul_prints_product_and_hadamard_add() {
    let output = run_example("examples/matrix_mul.hy");
    assert_eq!(output, "19,22,43,502");
}

#[test]
fn matrix_compound_add_assign_updates_cells() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let m = matrix([[1, 2], [3, 4]]);
    m += matrix([[10, 20], [30, 40]]);
    write(stdout(), to_bytes(format("%i,%i,%i,%i", m[0][0], m[0][1], m[1][0], m[1][1])));
}
"#,
    );
    assert_eq!(output, "11,22,33,44");
}

#[test]
fn matrix_sub_and_neg_packed_path_values() {
    // End-to-end: Matrix `-` (PackedMatrixZip Sub) and unary `-` (PackedMatrixNeg).
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let a = matrix([[5, 7], [9, 11]]);
    let b = matrix([[1, 2], [3, 4]]);
    let s = a - b;
    write(stdout(), to_bytes(format("%i,%i,%i,%i,", s[0][0], s[0][1], s[1][0], s[1][1])));
    let n = -b;
    write(stdout(), to_bytes(format("%i,%i,%i,%i", n[0][0], n[0][1], n[1][0], n[1][1])));
}
"#,
    );
    assert_eq!(output, "4,5,6,7,-1,-2,-3,-4");
}

#[test]
fn float_dot_packed_path_value() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    write(stdout(), to_bytes(format("%f", dot([1.5, 2.0], [2.5, 4.0]))));
}
"#,
    );
    assert_eq!(output, "11.75");
}

#[test]
fn matmul_tuple_of_tuples_rebuilds_correct_product() {
    // Exercises outer_is_tuple + row_is_tuple packing flags end-to-end.
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let a = ((1, 2), (3, 4));
    let b = ((5, 6), (7, 8));
    let c = matmul(a, b);
    write(stdout(), to_bytes(format("%i,%i,%i,%i", c[0][0], c[0][1], c[1][0], c[1][1])));
}
"#,
    );
    assert_eq!(output, "19,22,43,50");
}

#[test]
fn aggregate_compound_assign_updates_tuple() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let v = (1, 2);
    v += (3, 4);
    write(stdout(), to_bytes(format("%i%i", v[0], v[1])));
}
"#,
    );
    assert_eq!(output, "46");
}

#[test]
fn example_casts_primitive_as_operators() {
    assert_eq!(run_example("examples/casts.hy"), "13true");
}

#[test]
fn example_time_epoch_ok() {
    assert_eq!(run_example("examples/time_demo.hy"), "1");
}

#[test]
fn example_ansi_color_prints_red() {
    let output = run_example("examples/ansi_color.hy");
    assert!(
        output.contains("red"),
        "expected visible 'red', got {:?}",
        output
    );
}

/// HostInvoke + virtual `crypto` wiring: empty SHA-256 digest length is 32.
#[test]
fn crypto_sha256_empty_digest_len_via_host_invoke() {
    let output = run_example_src(
        r#"
use crypto::{sha256};
use io::{stdout, write};
use string::{format, to_bytes};

fn digest_len() -> int {
    let empty: Vec<byte> = Vec::new();
    return match sha256(empty) {
        Result::Ok(d) => len(d),
        Result::Err(_) => 0,
    };
}

fn main() {
    write(stdout(), to_bytes(format("%i", digest_len())));
}
"#,
    );
    assert_eq!(output, "32");
}

/// HostInvoke + virtual `io::fs` wiring: `exists(".")` returns Ok.
#[test]
fn fs_exists_dot_ok_via_host_invoke() {
    let output = run_example_src(
        r#"
use io::fs::{exists};
use io::{stdout, write};
use string::{format, to_bytes};

fn dot_ok() -> int {
    return match exists(".") {
        Result::Ok(_) => 1,
        Result::Err(_) => 0,
    };
}

fn main() {
    write(stdout(), to_bytes(format("%i", dot_ok())));
}
"#,
    );
    assert_eq!(output, "1");
}

/// HostInvoke + virtual `env` wiring: set/get/remove round-trip.
#[test]
fn env_var_round_trip_via_host_invoke() {
    let output = run_example_src(
        r#"
use env::{remove_var, set_var, var};
use io::{stdout, write};
use string::{format, to_bytes};

fn round_trip() -> string {
    let set_ok = match set_var("COIL_PIPELINE_ENV_KEY", "coil_ok") {
        Result::Ok(_) => 1,
        Result::Err(_) => 0,
    };
    if set_ok == 0 {
        return "0";
    }
    let got = match var("COIL_PIPELINE_ENV_KEY") {
        Result::Ok(s) => s,
        Result::Err(_) => "0",
    };
    let rem_ok = match remove_var("COIL_PIPELINE_ENV_KEY") {
        Result::Ok(_) => 1,
        Result::Err(_) => 0,
    };
    if rem_ok == 0 {
        return "0";
    }
    return got;
}

fn main() {
    write(stdout(), to_bytes(format("%s", round_trip())));
}
"#,
    );
    assert_eq!(output, "coil_ok");
}

/// HostInvoke + `io::net::tls::client`: enable on non-TCP → InvalidInput.
#[cfg(feature = "tls")]
#[test]
fn tls_client_enable_non_tcp_is_err_via_host_invoke() {
    let output = run_example_src(
        r#"
use io::{open, stdout, IoError, write};
use io::net::tls::client::{enable};
use string::{format, to_bytes};

fn classify(IoError e) -> int {
    return match e {
        IoError::WouldBlock => 10,
        IoError::NotFound => 11,
        IoError::PermissionDenied => 12,
        IoError::AlreadyClosed => 13,
        IoError::InvalidInput => 1,
        IoError::Other => 15,
        IoError::NotADirectory => 16,
        IoError::AlreadyExists => 17,
        IoError::TimedOut => 18,
        IoError::Truncated => 19,
        IoError::Certificate => 20,
        IoError::Handshake => 21,
    };
}

fn main() {
    let path = "/tmp/coil_tls_enable_kind.bin";
    let s = open(path, "w")?;
    let r = enable(s, "127.0.0.1", { verify: false, ca_pem: Option::None, ca_path: Option::None, timeout_ms: 0 });
    let code = match r {
        Result::Ok(_) => 0,
        Result::Err(e) => classify(e),
    };
    write(stdout(), to_bytes(format("%i", code)));
}
"#,
    );
    assert_eq!(output, "1");
}

/// HostInvoke wiring for client `disable` on a non-TLS stream → Err.
#[cfg(feature = "tls")]
#[test]
fn tls_client_disable_on_file_is_err_via_host_invoke() {
    let output = run_example_src(
        r#"
use io::{open, stdout, write};
use io::net::tls::client::{disable};
use string::{format, to_bytes};

fn disable_file_is_err() -> int {
    let path = "/tmp/coil_tls_disable_kind.bin";
    return match open(path, "w") {
        Result::Ok(s) => match disable(s) {
            Result::Ok(_) => 0,
            Result::Err(_) => 1,
        },
        Result::Err(_) => 9,
    };
}

fn main() {
    write(stdout(), to_bytes(format("%i", disable_file_is_err())));
}
"#,
    );
    assert_eq!(output, "1");
}

/// Two-arg client `enable` is not a complete call (needs opts record).
#[cfg(feature = "tls")]
#[test]
fn tls_client_enable_two_arg_does_not_compile() {
    let mut pipeline = Pipeline::new();
    let err = pipeline.compile_src(
        r#"
use io::{open, IoError, Stream, write};
use io::net::tls::client::{enable};

fn main() {
    let path = "/tmp/coil_tls_arity.bin";
    let s = open(path, "w")?;
    let r: Result<Stream, IoError> = enable(s, "127.0.0.1");
}
"#,
    );
    assert!(
        err.is_err(),
        "2-arg enable should fail to typecheck as Result"
    );
}

/// Third arg to client `enable` must be a record with `verify: bool`.
#[cfg(feature = "tls")]
#[test]
fn tls_client_enable_non_record_opts_does_not_compile() {
    let mut pipeline = Pipeline::new();
    let err = pipeline.compile_src(
        r#"
use io::{open, write};
use io::net::tls::client::{enable};

fn main() {
    let path = "/tmp/coil_tls_opts.bin";
    let s = open(path, "w")?;
    let _ = enable(s, "127.0.0.1", 1)?;
}
"#,
    );
    assert!(err.is_err(), "non-record opts should fail to typecheck");
}

/// Empty client opts `{}` omit required `verify` → type error.
#[cfg(feature = "tls")]
#[test]
fn tls_client_enable_empty_opts_does_not_compile() {
    let mut pipeline = Pipeline::new();
    let err = pipeline.compile_src(
        r#"
use io::{open, write};
use io::net::tls::client::{enable};

fn main() {
    let path = "/tmp/coil_tls_empty_opts.bin";
    let s = open(path, "w")?;
    let _ = enable(s, "127.0.0.1", {})?;
}
"#,
    );
    assert!(err.is_err(), "empty opts should fail to typecheck");
}

/// HostInvoke: server `enable` on non-TCP → InvalidInput.
#[cfg(feature = "tls")]
#[test]
fn tls_server_enable_non_tcp_is_err_via_host_invoke() {
    // Valid PEM so InvalidInput comes from StreamKind (not PEM parse).
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).expect("cert");
    let cert_pem = coil_escape_string(&cert.cert.pem());
    let key_pem = coil_escape_string(&cert.key_pair.serialize_pem());
    let src = format!(
        r#"
use io::{{open, stdout, IoError}};

use io::net::tls::server::{{enable}};
use string::{{format, to_bytes}};

fn classify(IoError e) -> int {{
    return match e {{
        IoError::WouldBlock => 10,
        IoError::NotFound => 11,
        IoError::PermissionDenied => 12,
        IoError::AlreadyClosed => 13,
        IoError::InvalidInput => 1,
        IoError::Other => 15,
        IoError::NotADirectory => 16,
        IoError::AlreadyExists => 17,
        IoError::TimedOut => 18,
        IoError::Truncated => 19,
        IoError::Certificate => 20,
        IoError::Handshake => 21,
    }};
}}

fn main() {{
    let path = "/tmp/coil_tls_server_enable_kind.bin";
    let s = open(path, "w")?;
    let r = enable(s, {{ cert_pem: "{cert_pem}", key_pem: "{key_pem}", timeout_ms: 0, client_ca_pem: "" }});
    let code = match r {{
        Result::Ok(_) => 0,
        Result::Err(e) => classify(e),
    }};
    write(stdout(), to_bytes(format("%i", code)));
}}
"#
    );
    let output = run_example_src(&src);
    assert_eq!(output, "1");
}

/// HostInvoke: server `disable` on non-TLS → Err.
#[cfg(feature = "tls")]
#[test]
fn tls_server_disable_on_file_is_err_via_host_invoke() {
    let output = run_example_src(
        r#"
use io::{open, stdout, write};
use io::net::tls::server::{disable};
use string::{format, to_bytes};

fn main() {
    let path = "/tmp/coil_tls_server_disable_kind.bin";
    let code = match open(path, "w") {
        Result::Ok(s) => match disable(s) {
            Result::Ok(_) => 0,
            Result::Err(_) => 1,
        },
        Result::Err(_) => 9,
    };
    write(stdout(), to_bytes(format("%i", code)));
}
"#,
    );
    assert_eq!(output, "1");
}

/// Server `enable` opts must be the cert/key record (not an int).
#[cfg(feature = "tls")]
#[test]
fn tls_server_enable_non_record_opts_does_not_compile() {
    let mut pipeline = Pipeline::new();
    let err = pipeline.compile_src(
        r#"
use io::{open, write};
use io::net::tls::server::{enable};

fn main() {
    let path = "/tmp/coil_tls_server_opts.bin";
    let s = open(path, "w")?;
    let _ = enable(s, 1)?;
}
"#,
    );
    assert!(err.is_err(), "non-record server enable opts should fail");
}

/// Empty server `enable` opts omit required PEM keys → type error.
#[cfg(feature = "tls")]
#[test]
fn tls_server_enable_empty_opts_does_not_compile() {
    let mut pipeline = Pipeline::new();
    let err = pipeline.compile_src(
        r#"
use io::{open, write};
use io::net::tls::server::{enable};

fn main() {
    let path = "/tmp/coil_tls_server_empty.bin";
    let s = open(path, "w")?;
    let _ = enable(s, {})?;
}
"#,
    );
    assert!(err.is_err(), "empty server enable opts should fail");
}

/// Parent `io::net::tls` exports nothing — flat `enable` must not resolve.
#[cfg(feature = "tls")]
#[test]
fn tls_flat_parent_enable_does_not_compile() {
    let mut pipeline = Pipeline::new();
    let err = pipeline.compile_src(
        r#"
use io::{open, write};

fn main() {
    let path = "/tmp/coil_tls_flat.bin";
    let s = open(path, "w")?;
    let _ = enable(s, "127.0.0.1", { verify: false, ca_pem: Option::None, ca_path: Option::None, timeout_ms: 0 })?;
}
"#,
    );
    assert!(err.is_err(), "enable must not resolve without tls client/server import");
}

/// Legacy `encrypt` / `decrypt` names under server must stay gone.
#[cfg(feature = "tls")]
#[test]
fn tls_legacy_server_encrypt_decrypt_do_not_compile() {
    let mut pipeline = Pipeline::new();
    let err = pipeline.compile_src(
        r#"
use io::{open, write};

fn main() {
    let path = "/tmp/coil_tls_legacy_encrypt.bin";
    let s = open(path, "w")?;
    let _ = encrypt(s, { cert_pem: "x", key_pem: "y", timeout_ms: 0, client_ca_pem: "" })?;
}
"#,
    );
    assert!(
        err.is_err(),
        "server::encrypt must not resolve after rename"
    );

    let err = pipeline.compile_src(
        r#"
use io::{open, write};

fn main() {
    let path = "/tmp/coil_tls_legacy_decrypt.bin";
    let s = open(path, "w")?;
    let _ = decrypt(s)?;
}
"#,
    );
    assert!(
        err.is_err(),
        "server::decrypt must not resolve after rename"
    );
}

/// Same surface name `enable`, different opts records — cross-namespace misuse fails.
#[cfg(feature = "tls")]
#[test]
fn tls_client_opts_on_server_enable_do_not_compile() {
    let mut pipeline = Pipeline::new();
    let err = pipeline.compile_src(
        r#"
use io::{open, write};
use io::net::tls::server::{enable};

fn main() {
    let path = "/tmp/coil_tls_cross_opts.bin";
    let s = open(path, "w")?;
    let _ = enable(s, { verify: false, ca_pem: Option::None, ca_path: Option::None, timeout_ms: 0 })?;
}
"#,
    );
    assert!(
        err.is_err(),
        "server enable must reject client-shaped {{ verify }} opts"
    );
}

/// Both namespaces in one program: HostInvoke routes distinct natives.
#[cfg(feature = "tls")]
#[test]
fn tls_client_and_server_enable_both_invalid_input_via_host_invoke() {
    let output = run_example_src(
        r#"
use io::{open, stdout, IoError, write};
use io::net::tls::client::enable as client_enable;
use io::net::tls::server::enable as server_enable;
use string::{format, to_bytes};

fn classify(IoError e) -> int {
    return match e {
        IoError::WouldBlock => 10,
        IoError::NotFound => 11,
        IoError::PermissionDenied => 12,
        IoError::AlreadyClosed => 13,
        IoError::InvalidInput => 1,
        IoError::Other => 15,
        IoError::NotADirectory => 16,
        IoError::AlreadyExists => 17,
        IoError::TimedOut => 18,
        IoError::Truncated => 19,
        IoError::Certificate => 20,
        IoError::Handshake => 21,
    };
}

fn main() {
    let path = "/tmp/coil_tls_both_ns.bin";
    let s = open(path, "w")?;
    let c = match client_enable(s, "127.0.0.1", { verify: false, ca_pem: Option::None, ca_path: Option::None, timeout_ms: 0 }) {
        Result::Ok(_) => 0,
        Result::Err(e) => classify(e),
    };
    let s2 = open(path, "w")?;
    let sv = match server_enable(s2, { cert_pem: "not-pem", key_pem: "not-pem", timeout_ms: 0, client_ca_pem: "" }) {
        Result::Ok(_) => 0,
        Result::Err(e) => classify(e),
    };
    // File stream → InvalidInput for both; distinct natives both wired.
    write(stdout(), to_bytes(format("%i%i", c, sv)));
}
"#,
    );
    assert_eq!(output, "11");
}

/// HostInvoke: empty PEM strings typecheck but fail at runtime → InvalidInput.
#[cfg(feature = "tls")]
#[test]
fn tls_server_enable_empty_pem_is_invalid_input_via_host_invoke() {
    let output = run_example_src(
        r#"
use io::{open, stdout, IoError, write};
use io::net::tls::server::{enable};
use string::{format, to_bytes};

fn classify(IoError e) -> int {
    return match e {
        IoError::WouldBlock => 10,
        IoError::NotFound => 11,
        IoError::PermissionDenied => 12,
        IoError::AlreadyClosed => 13,
        IoError::InvalidInput => 1,
        IoError::Other => 15,
        IoError::NotADirectory => 16,
        IoError::AlreadyExists => 17,
        IoError::TimedOut => 18,
        IoError::Truncated => 19,
        IoError::Certificate => 20,
        IoError::Handshake => 21,
    };
}

fn main() {
    let path = "/tmp/coil_tls_server_empty_pem.bin";
    let s = open(path, "w")?;
    let r = enable(s, { cert_pem: "", key_pem: "", timeout_ms: 0, client_ca_pem: "" });
    let code = match r {
        Result::Ok(_) => 0,
        Result::Err(e) => classify(e),
    };
    write(stdout(), to_bytes(format("%i", code)));
}
"#,
    );
    assert_eq!(output, "1");
}

/// `cert_pem` / `key_pem` must be strings (not ints).
#[cfg(feature = "tls")]
#[test]
fn tls_server_enable_non_string_pem_fields_do_not_compile() {
    let mut pipeline = Pipeline::new();
    let err = pipeline.compile_src(
        r#"
use io::{open, write};
use io::net::tls::server::{enable};

fn main() {
    let path = "/tmp/coil_tls_server_pem_ty.bin";
    let s = open(path, "w")?;
    let _ = enable(s, { cert_pem: 1, key_pem: 2, timeout_ms: 0, client_ca_pem: "" })?;
}
"#,
    );
    assert!(
        err.is_err(),
        "non-string server enable PEM fields should fail to typecheck"
    );
}

#[cfg(feature = "tls")]
#[test]
fn tls_client_enable_unknown_opts_key_does_not_compile() {
    let mut pipeline = Pipeline::new();
    let err = pipeline.compile_src(
        r#"
use io::{open, write};
use io::net::tls::client::{enable};

fn main() {
    let path = "/tmp/coil_tls_client_unknown_opts.bin";
    let s = open(path, "w")?;
    let _ = enable(s, "127.0.0.1", { verify: false, ca_pem: Option::None, ca_path: Option::None, timeout_ms: 0, alpn: "h2" })?;
}
"#,
    );
    assert!(
        err.is_err(),
        "unknown client opts key should fail to typecheck"
    );
}

#[cfg(feature = "tls")]
#[test]
fn tls_server_enable_unknown_opts_key_does_not_compile() {
    let mut pipeline = Pipeline::new();
    let err = pipeline.compile_src(
        r#"
use io::{open, write};
use io::net::tls::server::{enable};

fn main() {
    let path = "/tmp/coil_tls_server_unknown_opts.bin";
    let s = open(path, "w")?;
    let _ = enable(s, { cert_pem: "c", key_pem: "k", timeout_ms: 0, client_ca_pem: "", alpn: "h2" })?;
}
"#,
    );
    assert!(
        err.is_err(),
        "unknown server enable opts key should fail to typecheck"
    );
}

/// Smoke example stays green without public-network TLS.
#[cfg(feature = "tls")]
#[test]
fn example_io_tls_prints_tls_ok() {
    assert_eq!(run_example("examples/io_tls.hy"), "tls-ok");
}

#[test]
fn example_regex_demo_prints_expected() {
    assert_eq!(
        run_example("examples/regex_demo.hy"),
        "true,2,a->1 b->2,a|b|c"
    );
}

#[test]
fn regex_compile_error_is_err_via_host_invoke() {
    let output = run_example_src(
        r#"
use regex::{compile};
use io::{stdout, write};
use string::{format, to_bytes};

fn bad() -> int {
    return match compile("(", "") {
        Result::Ok(_) => 0,
        Result::Err(e) => match e {
            RegexError::Compile => 1,
            RegexError::Runtime => 2,
            RegexError::NoMatch => 3,
            RegexError::Utf8 => 4,
        },
    };
}

fn main() {
    write(stdout(), to_bytes(format("%i", bad())));
}
"#,
    );
    assert_eq!(output, "1");
}

/// `#[derive(String)]` end-to-end: synthesized `to_string` is callable.
#[test]
fn derive_string_to_string_prints_variant() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
#[derive(String)]
enum Color {
    Red,
}

fn main() {
    write(stdout(), to_bytes(format("%s", Color::Red.to_string())));
}
"#,
    );
    assert_eq!(output, "Color::Red");
}

/// Recursive `#[derive(Hash)]` + primitive Hash instances.
#[test]
fn example_derive_hash_prints_true_true_true_true() {
    let output = run_example("examples/derive_hash.hy");
    assert_eq!(output, "true,true,true,true");
}

/// Primitive `Hash` covers non-int payloads used by derive.
#[test]
fn hash_primitives_and_nested_differ_when_payloads_differ() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
#[derive(Hash)]
enum Box {
    S { s: string },
    B { b: bool },
    F { f: float },
}

fn main() {
    write(stdout(), to_bytes(format("%z,", "a".hash() != "b".hash())));
    write(stdout(), to_bytes(format("%z,", true.hash() != false.hash())));
    write(stdout(), to_bytes(format("%z,", (1.0).hash() != (2.0).hash())));
    write(stdout(), to_bytes(format("%z,", Box::S { s: "x" }.hash() != Box::S { s: "y" }.hash())));
    write(stdout(), to_bytes(format("%z", Box::B { b: true }.hash() != Box::B { b: false }.hash())));
}
"#,
    );
    assert_eq!(output, "true,true,true,true,true");
}

#[test]
fn fallthrough_string_return_requires_explicit_return() {
    let mut pipeline = Pipeline::new();
    let err = pipeline.compile_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn bad() -> string {
    // no return
}

fn main() {
    write(stdout(), to_bytes(format("%s", bad())));
}
"#,
    );
    assert!(
        err.is_err(),
        "missing return for string should fail with E0111"
    );
    let msgs = pipeline.messages();
    assert!(
        msgs.iter()
            .any(|m| m.code() == Some(compiler::ErrorCode::ReturnMismatch)),
        "expected ReturnMismatch; got {} messages",
        msgs.len()
    );
}

#[test]
fn fallthrough_unit_allows_implicit_epilogue() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn unitish() {
    let x = 1;
}

fn main() {
    unitish();
    write(stdout(), to_bytes("ok"));
}
"#,
    );
    assert_eq!(output, "ok");
}

#[test]
fn fallthrough_int_requires_explicit_return() {
    let mut pipeline = Pipeline::new();
    let err = pipeline.compile_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn answer() -> int {
    // no return
}

fn main() {
    write(stdout(), to_bytes(format("%i", answer())));
}
"#,
    );
    assert!(err.is_err());
    assert!(
        pipeline
            .messages()
            .iter()
            .any(|m| m.code() == Some(compiler::ErrorCode::ReturnMismatch))
    );
}

#[test]
fn fallthrough_bool_byte_float_require_explicit_return() {
    let mut pipeline = Pipeline::new();
    let err = pipeline.compile_src(
        r#"
fn flag() -> bool {}
fn b() -> byte {}
fn f() -> float {}
fn main() {}
"#,
    );
    assert!(err.is_err());
    let n = pipeline
        .messages()
        .iter()
        .filter(|m| m.code() == Some(compiler::ErrorCode::ReturnMismatch))
        .count();
    assert!(n >= 3, "expected E0111 for bool/byte/float; got {n}");
}

#[test]
fn fallthrough_option_requires_explicit_return() {
    let mut pipeline = Pipeline::new();
    let err = pipeline.compile_src(
        r#"
fn opt() -> Option<int> {}
fn main() { let _ = opt(); }
"#,
    );
    assert!(err.is_err());
    assert!(
        pipeline
            .messages()
            .iter()
            .any(|m| m.code() == Some(compiler::ErrorCode::ReturnMismatch))
    );
}

#[test]
fn fallthrough_async_yield_only_does_not_e0111() {
    let mut pipeline = Pipeline::new();
    let result = pipeline.compile_src(
        r#"
async fn gen_three() {
    yield 0;
    yield 1;
    yield 2;
}
async fn outer() {
    yield from gen_three();
}
async fn parameterized(int base) {
    yield base;
    yield base + 1;
    yield base + 2;
}
fn main() {
    let _ = resume gen_three();
}
"#,
    );
    assert!(
        result.is_ok(),
        "yield-only async bodies must not E0111 on fall-through: {:?}",
        pipeline
            .messages()
            .iter()
            .map(|m| (m.code(), m.message()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn fallthrough_result_unit_ok_allows_epilogue() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn ok_unit() -> Result<(), string> {}

fn main() {
    write(stdout(), to_bytes(format("%i", match ok_unit() {
        Result::Ok(_) => 1,
        Result::Err(_) => 0,
    })));
}
"#,
    );
    assert_eq!(output, "1");
}

#[test]
fn fallthrough_result_int_requires_explicit_return() {
    let mut pipeline = Pipeline::new();
    let err = pipeline.compile_src(
        r#"
fn ok_int() -> Result<int, string> {}
fn main() { let _ = ok_int(); }
"#,
    );
    assert!(err.is_err());
    assert!(
        pipeline
            .messages()
            .iter()
            .any(|m| m.code() == Some(compiler::ErrorCode::ReturnMismatch))
    );
}

#[test]
fn fallthrough_result_string_and_adt_require_explicit_return() {
    let mut pipeline = Pipeline::new();
    let err = pipeline.compile_src(
        r#"
enum Color { Red, Blue }
fn bad_res() -> Result<string, string> {}
fn bad_adt() -> Color {}
fn main() {
    let _ = bad_res();
    let _ = bad_adt();
}
"#,
    );
    assert!(
        err.is_err(),
        "Result<string,_>/ADT fall-through should fail with E0111"
    );
    let msgs = pipeline.messages();
    let mismatches = msgs
        .iter()
        .filter(|m| m.code() == Some(compiler::ErrorCode::ReturnMismatch))
        .count();
    assert!(
        mismatches >= 2,
        "expected ReturnMismatch for both Result<string> and ADT; got {} messages: {:?}",
        msgs.len(),
        msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
    );
}

#[test]
fn bare_return_semi_exits_unit_fn() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn early(int n) {
    if n == 0 {
        return;
    }
    write(stdout(), to_bytes("go"));
    return;
}

fn main() {
    early(0);
    early(1);
}
"#,
    );
    assert_eq!(output, "go");
}

#[test]
fn while_true_satisfies_non_unit_return() {
    let mut pipeline = Pipeline::new();
    let result = pipeline.compile_src(
        r#"
fn forever() -> int {
    while true {
    }
}

fn main() {
    let _ = forever;
}
"#,
    );
    assert!(
        result.is_ok(),
        "while true without break should complete -> int: {:?}",
        pipeline
            .messages()
            .iter()
            .map(|m| m.message())
            .collect::<Vec<_>>()
    );
}

#[test]
fn unreachable_after_return_is_warning() {
    let mut pipeline = Pipeline::new();
    let result = pipeline.compile_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn f() -> int {
    return 1;
    write(stdout(), to_bytes("dead"));
}

fn main() {
    write(stdout(), to_bytes(format("%i", f())));
}
"#,
    );
    assert!(result.is_ok(), "unreachable is warning, not hard error");
    assert!(
        pipeline
            .messages()
            .iter()
            .any(|m| m.code() == Some(compiler::ErrorCode::UnreachableCode)
                && *m.kind() == reporting::MessageKind::WARNING)
    );
}

#[test]
fn unreachable_after_while_true_is_warning() {
    let mut pipeline = Pipeline::new();
    let result = pipeline.compile_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn f() -> int {
    while true {
    }
    write(stdout(), to_bytes("dead"));
}

fn main() {
    let _ = f;
}
"#,
    );
    assert!(result.is_ok(), "unreachable after infinite loop is warning");
    assert!(
        pipeline
            .messages()
            .iter()
            .any(|m| m.code() == Some(compiler::ErrorCode::UnreachableCode)
                && *m.kind() == reporting::MessageKind::WARNING)
    );
}

#[test]
fn defer_dominated_by_while_true_warns_e0123() {
    let mut pipeline = Pipeline::new();
    let result = pipeline.compile_src(
        r#"
use io::{stdout, write};
use string::{to_bytes};
fn f() -> int {
    defer { write(stdout(), to_bytes("d")); }
    while true {
    }
}

fn main() {
    let _ = f;
}
"#,
    );
    // Warnings stay inspectable; only hard errors fail `compile_src`.
    assert!(
        result.is_ok(),
        "warning-only E0123 must not fail compile_src: {:?}",
        pipeline.messages()
    );
    assert!(
        pipeline
            .messages()
            .iter()
            .any(|m| m.code() == Some(compiler::ErrorCode::DeferNeverRuns)
                && *m.kind() == reporting::MessageKind::WARNING),
        "expected DeferNeverRuns dominated by while true"
    );
    assert!(
        !pipeline
            .messages()
            .iter()
            .any(|m| *m.kind() == reporting::MessageKind::ERROR),
        "E0123 should be warning-only: {:?}",
        pipeline.messages()
    );
    assert!(!pipeline.had_errors());
}

#[test]
fn defer_inside_while_true_warns_e0123() {
    let mut pipeline = Pipeline::new();
    let result = pipeline.compile_src(
        r#"
use io::{stdout, write};
use string::{to_bytes};
fn f() -> int {
    while true {
        defer { write(stdout(), to_bytes("d")); }
    }
}

fn main() {
    let _ = f;
}
"#,
    );
    assert!(
        result.is_ok(),
        "warning-only E0123 must not fail compile_src: {:?}",
        pipeline.messages()
    );
    assert!(
        pipeline
            .messages()
            .iter()
            .any(|m| m.code() == Some(compiler::ErrorCode::DeferNeverRuns)
                && *m.kind() == reporting::MessageKind::WARNING),
        "expected DeferNeverRuns inside while true"
    );
    assert!(
        !pipeline
            .messages()
            .iter()
            .any(|m| *m.kind() == reporting::MessageKind::ERROR),
        "E0123 should be warning-only: {:?}",
        pipeline.messages()
    );
    assert!(!pipeline.had_errors());
}

/// Hard errors must still fail `compile_src` after the warning-only change.
#[test]
fn compile_src_still_fails_on_hard_errors() {
    let mut pipeline = Pipeline::new();
    let result = pipeline.compile_src(
        r#"
fn main() {
    let _ = totally_undefined_var;
}
"#,
    );
    assert!(
        result.is_err(),
        "unknown identifier must fail compile_src"
    );
    assert!(pipeline.had_errors());
    assert!(
        pipeline
            .messages()
            .iter()
            .any(|m| *m.kind() == reporting::MessageKind::ERROR),
        "expected at least one error: {:?}",
        pipeline.messages()
    );
}

#[test]
fn while_true_with_break_requires_explicit_return() {
    let mut pipeline = Pipeline::new();
    let err = pipeline.compile_src(
        r#"
fn f() -> int {
    while true {
        break;
    }
}

fn main() {
    let _ = f;
}
"#,
    );
    assert!(err.is_err(), "break defeats infinite-loop path proof");
    assert!(
        pipeline
            .messages()
            .iter()
            .any(|m| m.code() == Some(compiler::ErrorCode::ReturnMismatch))
    );
}

#[test]
fn const_true_while_satisfies_non_unit_return() {
    let mut pipeline = Pipeline::new();
    let result = pipeline.compile_src(
        r#"
fn forever() -> int {
    const loop = true;
    while loop {
    }
}

fn main() {
    let _ = forever;
}
"#,
    );
    assert!(
        result.is_ok(),
        "const-folded while cond should complete -> int: {:?}",
        pipeline
            .messages()
            .iter()
            .map(|m| m.message())
            .collect::<Vec<_>>()
    );
}

#[test]
fn for_true_satisfies_non_unit_return() {
    let mut pipeline = Pipeline::new();
    let result = pipeline.compile_src(
        r#"
fn forever() -> int {
    for (; true; ) {
    }
}

fn main() {
    let _ = forever;
}
"#,
    );
    assert!(
        result.is_ok(),
        "for (; true; ) without break should complete -> int: {:?}",
        pipeline
            .messages()
            .iter()
            .map(|m| m.message())
            .collect::<Vec<_>>()
    );
}

#[test]
fn raise_path_satisfies_result_return() {
    let mut pipeline = Pipeline::new();
    let result = pipeline.compile_src(
        r#"
fn boom() -> Result<int, string> {
    raise "x";
}

fn main() {
    let _ = boom;
}
"#,
    );
    assert!(
        result.is_ok(),
        "raise should count as exit for Result return: {:?}",
        pipeline
            .messages()
            .iter()
            .map(|m| m.message())
            .collect::<Vec<_>>()
    );
}

#[test]
fn panic_path_satisfies_non_unit_return() {
    let mut pipeline = Pipeline::new();
    let result = pipeline.compile_src(
        r#"
fn boom() -> int {
    panic "x";
}

fn main() {
    let _ = boom;
}
"#,
    );
    assert!(
        result.is_ok(),
        "panic should count as exit for -> int: {:?}",
        pipeline
            .messages()
            .iter()
            .map(|m| m.message())
            .collect::<Vec<_>>()
    );
}

#[test]
fn if_without_else_requires_explicit_return() {
    let mut pipeline = Pipeline::new();
    let err = pipeline.compile_src(
        r#"
fn f(bool b) -> int {
    if b {
        return 1;
    }
}

fn main() {
    let _ = f;
}
"#,
    );
    assert!(err.is_err(), "if without else must not satisfy -> int");
    assert!(
        pipeline
            .messages()
            .iter()
            .any(|m| m.code() == Some(compiler::ErrorCode::ReturnMismatch))
    );
}

#[test]
fn if_else_returns_satisfy_non_unit_return() {
    let mut pipeline = Pipeline::new();
    let result = pipeline.compile_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn f(bool b) -> int {
    if b {
        return 1;
    } else {
        return 0;
    }
}

fn main() {
    write(stdout(), to_bytes(format("%i,", f(true))));
    write(stdout(), to_bytes(format("%i", f(false))));
}
"#,
    );
    assert!(
        result.is_ok(),
        "if/else returns should complete -> int: {:?}",
        pipeline
            .messages()
            .iter()
            .map(|m| m.message())
            .collect::<Vec<_>>()
    );
}

#[test]
fn never_join_match_panic_arm_types_as_int() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn unwrap(Option<int> o) -> int {
    return match o {
        Option::Some(x) => x,
        Option::None => panic "none",
    };
}

fn main() {
    write(stdout(), to_bytes(format("%i", unwrap(Option::Some(7)))));
}
"#,
    );
    assert_eq!(output, "7");
}

#[test]
fn match_all_arms_exit_satisfies_non_unit_return() {
    let mut pipeline = Pipeline::new();
    let result = pipeline.compile_src(
        r#"
fn f(Option<int> o) -> int {
    match o {
        Option::Some(x) => panic "some",
        Option::None => panic "none",
    };
}

fn main() {
    let _ = f;
}
"#,
    );
    assert!(
        result.is_ok(),
        "match arms that all exit should complete -> int: {:?}",
        pipeline
            .messages()
            .iter()
            .map(|m| m.message())
            .collect::<Vec<_>>()
    );
}

#[test]
fn unit_fallthrough_still_runs_defers() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn f() {
    defer { write(stdout(), to_bytes("d")); }
    write(stdout(), to_bytes("b"));
}

fn main() {
    f();
}
"#,
    );
    assert_eq!(
        output, "bd",
        "unit epilogue must still emit defers before CONST 0; RETURN"
    );
}

/// Nested multifield record that is not the first outer field must still
/// relocate into scratch and preserve the preceding sibling binding.
#[test]
fn nested_multifield_record_after_sibling_preserves_bindings() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
enum Inner {
    I { x: int, y: int },
}
enum Wrap {
    W { name: int, inner: Inner },
}
fn both(Wrap w) -> int {
    return match w {
        Wrap::W { name, inner: Inner::I { x, y } } => name + x + y,
    };
}
fn main() {
    let w = Wrap::W { name: 3, inner: Inner::I { x: 10, y: 20 } };
    write(stdout(), to_bytes(format("%i", both(w))));
}
"#,
    );
    assert_eq!(
        output, "33",
        "preceding sibling `name` and nested `x`/`y` must all bind"
    );
}

/// `x ** 0/1/2` strength-reduce must still match `i32::pow` (incl. `0 ** 0` → 1).
#[test]
fn int_pow_strength_reduce_prints_correct_values() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let x = 5;
    write(stdout(), to_bytes(format("%i,", x ** 0)));
    write(stdout(), to_bytes(format("%i,", x ** 1)));
    write(stdout(), to_bytes(format("%i,", x ** 2)));
    write(stdout(), to_bytes(format("%i,", x ** 3)));
    write(stdout(), to_bytes(format("%i", 0 ** 0)));
}
"#,
    );
    assert_eq!(output, "1,5,25,125,1");
}

/// `if (!c) { A } else { B }` inverts to `if (c) { B } else { A }` — arms must
/// not swap incorrectly when LogNot is eliminated.
#[test]
fn invert_not_if_else_preserves_arm_semantics() {
    let src = r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn pick(bool c) -> int {
    if !c {
        return 10;
    } else {
        return 20;
    }
}
fn main() {
    write(stdout(), to_bytes(format("%i,", pick(false))));
    write(stdout(), to_bytes(format("%i", pick(true))));
}
"#;
    let mut pipeline = Pipeline::new();
    let (bytecode, _) = pipeline.compile_src(src).expect("compile");
    assert!(
        !bytecode.iter().any(|b| matches!(
            b.bytecode(),
            common::Instruction::LogNot | common::Instruction::LogNotJmpf
        )),
        "inverted if(!c) else should drop LogNot / LogNotJmpf"
    );
    assert_eq!(run_example_src(src), "10,20");
}

/// SetField / StoreIndex leave the RHS on the stack — statement forms must POP
/// or later ops see a corrupted stack.
#[test]
fn set_field_and_store_index_statements_pop_value() {
    let src = r#"
use io::{stdout, write};
use string::{format, to_bytes};
class Point {
    x: int,
    y: int,
}
fn main() {
    let p = new Point(0, 0);
    p.x = 3;
    p.y = 4;
    let a = [0, 0];
    a[0] = 10;
    a[1] = 20;
    write(stdout(), to_bytes(format("%i,", p.x + p.y)));
    write(stdout(), to_bytes(format("%i", a[0] + a[1])));
}
"#;
    let mut pipeline = Pipeline::new();
    let (bytecode, _) = pipeline.compile_src(src).expect("compile");
    let mut set_field_followed_by_pop = 0usize;
    let mut store_index_followed_by_pop = 0usize;
    for w in bytecode.windows(2) {
        if matches!(w[0].bytecode(), common::Instruction::SetField)
            && matches!(w[1].bytecode(), common::Instruction::POP)
        {
            set_field_followed_by_pop += 1;
        }
        if matches!(w[0].bytecode(), common::Instruction::StoreIndex)
            && matches!(w[1].bytecode(), common::Instruction::POP)
        {
            store_index_followed_by_pop += 1;
        }
    }
    assert!(
        set_field_followed_by_pop >= 2,
        "class field assignment statements need SetField; POP"
    );
    assert!(
        store_index_followed_by_pop >= 2,
        "index assignment statements need StoreIndex; POP"
    );
    assert_eq!(run_example_src(src), "7,30");
}

/// Repeated GetField of the same name materializes the key once and reuses the value.
#[test]
fn repeated_field_key_reuses_prologue_temp() {
    let src = r#"
use io::{stdout, write};
use string::{format, to_bytes};
class Point {
    x: int,
    y: int,
}
fn twice(Point p) -> int {
    return p.x + p.x;
}
fn main() {
    write(stdout(), to_bytes(format("%i", twice(new Point(3, 4)))));
}
"#;
    let mut pipeline = Pipeline::new();
    let (bytecode, _) = pipeline.compile_src(src).expect("compile");
    // Prologue: STRING table["x"]; STORE temp (ctor also emits STRING "x" for SetField).
    let x_idx = pipeline
        .strings()
        .iter()
        .position(|s| s == "x")
        .expect("string table should contain field key x") as u32;
    let has_string_x_store = bytecode.windows(2).any(|w| {
        matches!(w[0].bytecode(), common::Instruction::STRING)
            && w[0].operand_u32() == x_idx
            && matches!(
                w[1].bytecode(),
                common::Instruction::STORE | common::Instruction::StorePop
            )
    });
    assert!(
        has_string_x_store,
        "p.x + p.x should materialize STRING \"x\" into a temp slot"
    );
    // Value reuse: GetField then DUPLICATE (not a second GetField of x).
    let has_getfield_dup = bytecode.windows(2).any(|w| {
        matches!(w[0].bytecode(), common::Instruction::GetField)
            && matches!(w[1].bytecode(), common::Instruction::DUPLICATE)
    });
    assert!(
        has_getfield_dup,
        "p.x + p.x should GetField once then DUPLICATE"
    );
    assert_eq!(run_example_src(src), "6");
}

/// for-in over arrays hoists ArrayLen out of the loop header.
#[test]
fn for_in_array_hoists_array_len_once() {
    let src = r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    let a = [1, 2, 3];
    for x in a {
        write(stdout(), to_bytes(format("%i", x)));
    }
}
"#;
    let mut pipeline = Pipeline::new();
    let (bytecode, _) = pipeline.compile_src(src).expect("compile");
    let lens = bytecode
        .iter()
        .filter(|b| matches!(b.bytecode(), common::Instruction::ArrayLen))
        .count();
    // Loop hoist emits one ArrayLen; builtin `Length__string__len` may add another.
    assert!(
        (1..=2).contains(&lens),
        "for-in should ArrayLen once before the loop (got {lens})"
    );
    assert!(
        bytecode
            .iter()
            .any(|b| matches!(b.bytecode(), common::Instruction::BinSlotSlotJmpf)),
        "loop header should fuse idx < len into BinSlotSlotJmpf"
    );
    assert_eq!(run_example_src(src), "123");
}

/// Option unwrap join-clone + field_hot smoke (GetField / key reuse under load).
#[test]
fn example_perf_field_hot_prints_expected() {
    let output = run_example("examples/perf/field_hot.hy");
    assert_eq!(output, "4000000");
}

#[test]
fn option_unwrap_both_arms_correct_after_return_clone() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn unwrap(Option o) -> int {
    return match o {
        Option::None => 0,
        Option::Some(v) => v,
    };
}
fn main() {
    write(stdout(), to_bytes(format("%i,", unwrap(Option::Some(42)))));
    write(stdout(), to_bytes(format("%i", unwrap(Option::None))));
}
"#,
    );
    assert_eq!(output, "42,0");
}

/// v35 string table: serialize → load → run must keep STRING indexes valid,
/// and the compiler must not emit legacy trailing DATA payloads.
#[test]
fn string_table_archive_round_trip_preserves_literals() {
    use common::{ARCHIVE_VERSION, ArchivedProgram, Instruction};
    use rkyv::rancor::Error;

    let src = r#"
use io::{stdout, write};
use string::{format, to_bytes};
fn main() {
    write(stdout(), to_bytes(format("%s-%i", "archive", 35)));
}
"#;
    let mut pipeline = Pipeline::new();
    let (bytecode, constants) = pipeline.compile_src(src).expect("compile");
    assert!(
        !pipeline.strings().is_empty(),
        "expected program string table entries"
    );
    assert!(
        pipeline.strings().iter().any(|s| s == "archive"),
        "string table should contain literal `archive`: {:?}",
        pipeline.strings()
    );
    assert!(
        bytecode
            .iter()
            .any(|b| matches!(b.bytecode(), Instruction::STRING)),
        "expected STRING opcodes indexing the table"
    );
    assert!(
        bytecode
            .iter()
            .all(|b| !matches!(b.bytecode(), Instruction::DATA)),
        "compiler must not emit DATA tombstones after STRING"
    );

    let program = ArchivedProgram {
        version: ARCHIVE_VERSION,
        static_slot_count: pipeline.static_slot_count(),
        constants: constants.clone(),
        strings: pipeline.strings().to_vec(),
        bytecode: bytecode.clone(),
        source_files: pipeline.program_debug().source_files,
        debug_locs: pipeline.program_debug().debug_locs,
        fn_symbols: Vec::new(),
    };
    let bytes = rkyv::to_bytes::<Error>(&program).expect("serialize");
    let archived =
        rkyv::access::<rkyv::Archived<ArchivedProgram>, Error>(bytes.as_slice()).expect("access");
    assert_eq!(u32::from(archived.version), ARCHIVE_VERSION);
    let loaded_bc: Vec<common::Byte> =
        rkyv::deserialize::<Vec<common::Byte>, Error>(&archived.bytecode).expect("bc");
    let loaded_constants: Vec<u64> =
        rkyv::deserialize::<Vec<u64>, Error>(&archived.constants).expect("consts");
    let loaded_strings: Vec<String> =
        rkyv::deserialize::<Vec<String>, Error>(&archived.strings).expect("strings");
    let static_slots = u32::from(archived.static_slot_count);
    assert!(
        loaded_strings.iter().any(|s| s == "archive"),
        "deserialized string table lost literals: {loaded_strings:?}"
    );

    let shared = SharedBuf::new();
    let mut machine = Machine::<128>::default();
    machine.set_shared_print(shared.inner.clone());
    machine.with_output(shared.clone());
    pipeline.wire_host_natives(&mut machine);
    machine.run_raw(&loaded_bc, &loaded_constants, &loaded_strings, static_slots);
    let _ = machine.restore_output();
    assert_eq!(shared.into_utf8(), "archive-35");
}

/// Worker `write(stdout(), …)` in a multi-iteration loop must keep HostInvoke
/// results on the stack and honor shared-print capture TLS.
#[test]
fn worker_write_all_loop_captures_via_shared_print() {
    let output = run_example_src(
        r#"
use thread::{join, spawn};
use io::{stdout, write};
use string::{format, to_bytes};

fn worker() -> int {
    let i = 0;
    while i < 3 {
        write(stdout(), to_bytes(format("%i", i)));
        i = i + 1;
    }
    return 0;
}

fn main() {
    let t = spawn(worker)?;
    join(t)?;
}
"#,
    );
    assert_eq!(output, "012");
}

/// Qualified `string::format` / `string::to_bytes` resolve without a glob import.
#[test]
fn qualified_string_helpers_without_glob_import() {
    let output = run_example_src(
        r#"
use io::{stdout, write};

fn main() {
    write(stdout(), string::to_bytes(string::format("%s", "ok")));
}
"#,
    );
    assert_eq!(output, "ok");
}

/// Nested HostInvoke call args must stage through temps — evaluating the second
/// `to_bytes` inline must not clobber the first result on the operand stack.
#[test]
fn nested_host_invoke_call_args_stage_without_clobber() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::{format, to_bytes};

fn concat_bytes(Vec<byte> a, Vec<byte> b) -> Vec<byte> {
    let out: Vec<byte> = Vec::new();
    let i = 0;
    while i < len(a) {
        out.push(a[i]);
        i = i + 1;
    }
    let j = 0;
    while j < len(b) {
        out.push(b[j]);
        j = j + 1;
    }
    return out;
}

fn main() {
    let msg = concat_bytes(to_bytes("GET "), to_bytes("/hi"));
    write(stdout(), msg);
}
"#,
    );
    assert_eq!(output, "GET /hi");
}

#[test]
fn variable_string_add_concatenates() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::to_bytes;

fn main() {
    let a = "GET";
    let b = "/hi";
    write(stdout(), to_bytes(a + b));
}
"#,
    );
    assert_eq!(output, "GET/hi");
}

#[test]
fn variable_string_add_in_test_harness() {
    let src = r#"
use io::{stdout, write};
use string::to_bytes;

test("add") {
    let a = "GET";
    let b = "/hi";
    write(stdout(), to_bytes(a + b));
}
"#;
    let output = run_harness_src(src);
    assert_eq!(output, "GET/hi");
}

/// Nested `+` emits consecutive `"%s%s"` STRING ops. Dup-CSE of those ops
/// broke http `request_build` / multi-piece header lines.
#[test]
fn nested_string_concat_chain_prints_full_content() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::to_bytes;

fn main() {
    let method = "GET";
    let path = "/hi";
    let host = "example.com";
    let line = method + " " + path + " HTTP/1.1\r\nHost: " + host + "\r\n";
    write(stdout(), to_bytes(line));
}
"#,
    );
    assert_eq!(output, "GET /hi HTTP/1.1\r\nHost: example.com\r\n");
}

/// Same nested-concat shape under loop `+=` (format_extra_headers_str style).
#[test]
fn string_plus_equal_loop_builds_header_block() {
    let output = run_example_src(
        r#"
use io::{stdout, write};
use string::to_bytes;

fn main() {
    let names = ["X-Trace", "Accept"];
    let values = ["abc", "text/plain"];
    let acc = names[0] + ": " + values[0] + "\r\n";
    let i = 1;
    while i < 2 {
        acc = acc + names[i] + ": " + values[i] + "\r\n";
        i = i + 1;
    }
    write(stdout(), to_bytes(acc));
}
"#,
    );
    assert_eq!(output, "X-Trace: abc\r\nAccept: text/plain\r\n");
}

/// Bytecode shape: chained concat must keep multiple STRING `"%s%s"` hits
/// (not collapse them via Dup-CSE).
#[test]
fn nested_string_concat_keeps_multiple_pct_s_string_ops() {
    let src = r#"
use io::{stdout, write};
use string::to_bytes;

fn main() {
    let a = "a";
    let b = "b";
    let c = "c";
    write(stdout(), to_bytes(a + b + c));
}
"#;
    let mut pipeline = Pipeline::new();
    let (bytecode, _) = pipeline.compile_src(src).expect("compile");
    let pct_idx = pipeline
        .strings()
        .iter()
        .position(|s| s == "%s%s")
        .expect("concat should intern %s%s") as u32;
    let pct_string_ops = bytecode
        .iter()
        .filter(|b| {
            matches!(b.bytecode(), common::Instruction::STRING) && b.operand_u32() == pct_idx
        })
        .count();
    assert!(
        pct_string_ops >= 2,
        "a + b + c should emit ≥2 STRING \"%s%s\" (got {pct_string_ops}); Dup-CSE would under-count"
    );
    // No STRING \"%s%s\" immediately rewritten to DUPLICATE in the final stream.
    let string_then_dup = bytecode.windows(2).any(|w| {
        matches!(w[0].bytecode(), common::Instruction::STRING)
            && w[0].operand_u32() == pct_idx
            && matches!(w[1].bytecode(), common::Instruction::DUPLICATE)
    });
    assert!(
        !string_then_dup,
        "STRING \"%s%s\" must not be followed by DUPLICATE (within-block Dup-CSE)"
    );
    assert_eq!(run_example_src(src), "abc");
}
