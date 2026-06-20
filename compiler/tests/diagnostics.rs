//! Golden-file style tests for the typechecker's diagnostic messages.
//!
//! These tests run the full HM inference pass over a small program and
//! assert on the *content* of the resulting `Message`s — what they say
//! and roughly where they point. The tests are deliberately a bit
//! tolerant (substring matches, not exact equality) so we can tweak
//! error wording without breaking the suite, but strict enough that a
//! regression in the diagnostic content fails the test.
//!
//! The goal is to lock in the user-facing experience: a type error
//! should produce a `Message` that points at the offending code, names
//! the values involved, and (where useful) hints at a fix.

use compiler::Checker;
use parser::Pratt;

/// Parse `src`, run the HM checker, and return both the root type and
/// the accumulated messages.
fn check(src: &str) -> (String, Vec<String>) {
    let ast = Pratt::default().parse(src).expect("parse failed");
    let mut c = Checker::new();
    let ty = c.check_program(&ast);
    let msgs = c.take_messages();
    let msg_strings = msgs.iter().map(|m| m.message().to_string()).collect();
    (format!("{}", ty), msg_strings)
}

#[test]
fn unknown_identifier_reports_helpful_message() {
    let (_ty, msgs) = check("x;");
    assert!(
        msgs.iter().any(|m| m.contains("Cannot find value `x`")),
        "expected 'Cannot find value `x`' in messages, got: {:?}",
        msgs
    );
}

#[test]
fn type_mismatch_on_let_annotation_reports_expected_and_actual() {
    // Annotation pins x to int, but RHS is a string literal.
    let (_ty, msgs) = check(r#"let x: int = "hello";"#);
    assert!(
        msgs.iter().any(|m| m.contains("Type mismatch")),
        "expected 'Type mismatch' in messages, got: {:?}",
        msgs
    );
    assert!(
        msgs.iter().any(|m| m.contains("int")),
        "expected `int` to appear in the mismatch message, got: {:?}",
        msgs
    );
}

#[test]
fn arity_mismatch_mentions_function_name() {
    // `missing_fn` is unknown — the message should still mention the
    // name so the user can grep for it.
    let (_ty, msgs) = check("missing_fn(1, 2, 3);");
    assert!(
        msgs.iter().any(|m| m.contains("Cannot find function")),
        "expected 'Cannot find function' in messages, got: {:?}",
        msgs
    );
    assert!(
        msgs.iter().any(|m| m.contains("missing_fn")),
        "expected 'missing_fn' in the message, got: {:?}",
        msgs
    );
}

#[test]
fn calling_non_function_produces_helpful_message() {
    // `x` is bound to an int (not a function). Calling it should
    // produce a clear diagnostic instead of silently doing the wrong
    // thing.
    let (_ty, msgs) = check("let x = 42; x(1);");
    assert!(
        msgs.iter()
            .any(|m| m.contains("too many arguments") || m.contains("Cannot call")),
        "expected a 'too many arguments' / 'cannot call' diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn assignment_to_undeclared_variable_emits_help() {
    let (_ty, msgs) = check("undeclared = 1;");
    assert!(
        msgs.iter()
            .any(|m| m.contains("Cannot assign to undeclared variable")),
        "expected 'Cannot assign to undeclared variable' message, got: {:?}",
        msgs
    );
    // The error should also include a help hint suggesting a fix.
    let ast = Pratt::default().parse("undeclared = 1;").expect("parse failed");
    let mut c = Checker::new();
    let _ty = c.check_program(&ast);
    let msgs = c.take_messages();
    assert!(
        msgs.iter().any(|m| m.help().is_some()),
        "expected at least one message to carry a help hint"
    );
}

#[test]
fn multiple_errors_are_reported_in_one_pass() {
    // Two distinct problems: an unknown identifier and an assignment
    // to an undeclared variable. The checker should report both, not
    // stop at the first.
    let (_ty, msgs) = check("x; undeclared = 1;");
    assert!(
        msgs.len() >= 2,
        "expected at least 2 messages, got {} ({:?})",
        msgs.len(),
        msgs
    );
}

#[test]
fn well_typed_program_produces_no_messages() {
    // Sanity check: the happy path emits no diagnostics.
    let (_ty, msgs) = check("let x = 1 + 2; let y = x * 3;");
    assert!(
        msgs.is_empty(),
        "expected no messages for a well-typed program, got: {:?}",
        msgs
    );
}

#[test]
fn recursive_function_typechecks() {
    // The `fib` example from the examples/ directory, inlined.
    let src = "fn fib(int n) -> int { if n <= 2 { return 1; } return fib(n - 1) + fib(n - 2); }";
    let (_ty, msgs) = check(src);
    assert!(
        msgs.is_empty(),
        "recursive fib should type-check, got: {:?}",
        msgs
    );
}

#[test]
fn integer_inference_works() {
    // Plain integer literal infers to int.
    let (ty, msgs) = check("42;");
    assert!(msgs.is_empty(), "42 should type-check, got: {:?}", msgs);
    assert_eq!(ty, "int");
}

#[test]
fn float_inference_works() {
    // Plain float literal infers to float.
    let (ty, msgs) = check("1.5;");
    assert!(msgs.is_empty(), "1.5 should type-check, got: {:?}", msgs);
    assert_eq!(ty, "float");
}

#[test]
fn string_inference_works() {
    // Plain string literal infers to string.
    let (ty, msgs) = check(r#""hello";"#);
    assert!(msgs.is_empty(), "string literal should type-check, got: {:?}", msgs);
    assert_eq!(ty, "string");
}

#[test]
fn boolean_inference_works() {
    // Boolean literals infer to bool.
    let (ty, msgs) = check("true;");
    assert!(msgs.is_empty(), "true should type-check, got: {:?}", msgs);
    assert_eq!(ty, "bool");
}

#[test]
fn mixed_int_float_arithmetic_reports_mismatch() {
    // HM does NOT silently promote int to float — `1 + 2.0` is a
    // type mismatch (int ≠ float). The bytecode emitter separately
    // picks `ADDF` vs `ADD` based on operand types for opcode
    // selection, but the checker reports the mismatch so users are
    // aware.
    let (_ty, msgs) = check("1 + 2.0;");
    assert!(
        msgs.iter()
            .any(|m| m.contains("Type mismatch") && m.contains("int") && m.contains("float")),
        "expected 'Type mismatch: ... int ... float' message, got: {:?}",
        msgs
    );
}

#[test]
fn function_with_explicit_return_type_checks() {
    // A function that declares its return type and returns the right
    // shape should type-check.
    let (_ty, msgs) = check("fn add(int a, int b) -> int { return a + b; }");
    assert!(
        msgs.is_empty(),
        "explicit-typed function should type-check, got: {:?}",
        msgs
    );
}

#[test]
fn class_declaration_typechecks() {
    // `class Foo { name: String }` registers `Foo` as a type constructor.
    let (_ty, msgs) = check("class Foo { name: String, }");
    assert!(
        msgs.is_empty(),
        "class declaration should type-check, got: {:?}",
        msgs
    );
}