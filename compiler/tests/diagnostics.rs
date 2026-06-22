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

// ---- Phase 15B: sum types and pattern matching ----

#[test]
fn enum_decl_no_messages() {
    // A bare `enum` declaration produces no diagnostic.
    let (_ty, msgs) = check("enum Option { None, Some(int) }");
    assert!(
        msgs.is_empty(),
        "enum declaration should produce no messages, got: {:?}",
        msgs
    );
}

#[test]
fn match_with_all_variants_no_messages() {
    // All variants covered → no diagnostic.
    let src = "let x = Option::Some(1); match x { Option::None() => 0, Option::Some(v) => v }; enum Option { None, Some(int) }";
    let (_ty, msgs) = check(src);
    assert!(
        msgs.is_empty(),
        "match with all variants should produce no messages, got: {:?}",
        msgs
    );
}

#[test]
fn non_exhaustive_match_emits_diagnostic() {
    // One arm missing the `Some` variant → "Non-exhaustive" error.
    let src = "let x = Option::None(); match x { Option::None() => 0 }; enum Option { None, Some(int) }";
    let (_ty, msgs) = check(src);
    assert!(
        msgs.iter().any(|m| m.contains("Non-exhaustive match")),
        "expected non-exhaustive diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn unreachable_arm_emits_diagnostic() {
    // Two arms covering the same tag → second is unreachable.
    let src = "let x = Option::None(); match x { Option::None() => 0, Option::None() => 1 }; enum Option { None, Some(int) }";
    let (_ty, msgs) = check(src);
    assert!(
        msgs.iter().any(|m| m.contains("Unreachable arm")),
        "expected unreachable-arm diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn unknown_constructor_in_pattern_errors() {
    // A pattern that references an unknown constructor (an
    // enum/variant pair that was never declared). The typechecker
    // emits "Pattern references unknown constructor".
    let src = "let x = NoSuch::Missing(1); enum Real { Bar(int) }";
    let (_ty, msgs) = check(src);
    // The constructor call `NoSuch::Missing(1)` is the unknown
    // one. The error path: `infer_construct` → "Cannot find
    // enum `NoSuch`".
    assert!(
        msgs.iter()
            .any(|m| m.contains("Cannot find enum") || m.contains("unknown constructor")),
        "expected unknown-enum / unknown-constructor diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn constructor_wrong_arity_errors() {
    let src = "Option::Some(1, 2); enum Option { None, Some(int) }";
    let (_ty, msgs) = check(src);
    assert!(
        msgs.iter()
            .any(|m| m.contains("expects 1 arguments") || m.contains("wrong")),
        "expected wrong-arity diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn format_string_type_mismatch_errors() {
    // %s requires a string; passing int is a type error.
    let (_ty, msgs) = check("print \"%s\", 42;");
    assert!(
        msgs.iter().any(|m| m.contains("requires string")),
        "expected format-string type error, got: {:?}",
        msgs
    );
}

#[test]
fn format_string_percent_z_accepts_bool() {
    // `%z` is the bool specifier. `print "%z", true` should
    // type-check with no diagnostics.
    let (_ty, msgs) = check("print \"%z\", true;");
    assert!(
        msgs.is_empty(),
        "expected `%z` to accept bool, got: {:?}",
        msgs
    );
}

#[test]
fn format_string_percent_z_rejects_int() {
    // `%z` requires a bool; passing an int is a type error.
    let (_ty, msgs) = check("print \"%z\", 42;");
    assert!(
        msgs.iter().any(|m| m.contains("requires bool")),
        "expected 'requires bool' error for `%z` with int, got: {:?}",
        msgs
    );
}

// ---- Phase 17B: record-shape diagnostics ----

#[test]
fn record_construct_missing_field_diagnostic() {
    // Variant declared with two fields; constructor supplies
    // only one. Typechecker should emit a "missing field" error.
    let (_ty, msgs) = check(
        "enum E { Foo { x: int, y: int } } \
         fn main() { E::Foo { x: 1 }; }",
    );
    assert!(
        msgs.iter().any(|m| m.contains("Missing field `y`")),
        "expected 'Missing field `y`' diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn record_construct_extra_field_diagnostic() {
    // Variant declared with two fields; constructor supplies
    // an unknown third. Typechecker should emit an "unknown
    // field" / "no field `z`" error.
    let (_ty, msgs) = check(
        "enum E { Foo { x: int, y: int } } \
         fn main() { E::Foo { x: 1, y: 2, z: 3 }; }",
    );
    assert!(
        msgs.iter().any(|m| m.contains("Unknown field `z`") || m.contains("no field `z`")),
        "expected 'Unknown field `z`' / 'no field `z`' diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn record_pattern_unknown_field_diagnostic() {
    // Pattern references a field that doesn't exist in the
    // variant's declaration. The pattern may use either
    // `z: v` (explicit binding) or `{ z }` (shorthand).
    let (_ty, msgs) = check(
        "enum E { Foo { x: int, y: int } } \
         fn main() { \
             let e = E::Foo { x: 1, y: 2 }; \
             match e { E::Foo { z: v, x: _, y: _ } => v }; \
         }",
    );
    assert!(
        msgs.iter().any(|m| m.contains("Unknown field `z`") || m.contains("missing field `z`")),
        "expected 'Unknown field `z`' / 'missing field `z`' diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn record_construct_shape_mismatch_diagnostic() {
    // The variant is declared as a record (`{ x, y }`) but
    // the user calls it with tuple syntax `(a, b)`. This is
    // a shape mismatch.
    let (_ty, msgs) = check(
        "enum E { Foo { x: int, y: int } } \
         fn main() { E::Foo(1, 2); }",
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("shape mismatch") || m.contains("uses tuple syntax")),
        "expected 'shape mismatch' / 'uses tuple syntax' diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn record_construct_duplicate_field_diagnostic() {
    // The user supplies the same field twice in a record
    // constructor. The typechecker should reject this.
    let (_ty, msgs) = check(
        "enum E { Foo { x: int, y: int } } \
         fn main() { E::Foo { x: 1, x: 2 }; }",
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("Duplicate field `x`") || m.contains("duplicate")),
        "expected 'Duplicate field `x`' diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn mixed_shape_enum_with_match_uses_correct_shape() {
    // Regression test: a match across all three variant
    // shapes (Unit, Tuple, Record) compiles cleanly without
    // any diagnostics.
    let (_ty, msgs) = check(
        "enum E { A, B(int), C { x: int } } \
         fn classify(E e) -> int { \
             return match e { \
                 E::A => 0, \
                 E::B(v) => v, \
                 E::C { x: v } => v, \
             }; \
         }",
    );
    assert!(
        msgs.is_empty(),
        "mixed-shape match with all shapes should type-check, got: {:?}",
        msgs
    );
}

// ---- Phase 18D: field-access diagnostics ----

#[test]
fn access_field_on_non_record_produces_helpful_message() {
    // `1.x` — the receiver is `int`, not a sum. The diagnostic
    // should mention the field and explain what types support
    // field access.
    let (_ty, msgs) = check("1.x;");
    assert!(
        msgs.iter().any(|m| m.contains("Cannot access field")),
        "expected 'Cannot access field' in messages, got: {:?}",
        msgs
    );
    assert!(
        msgs.iter().any(|m| m.contains("`x`")),
        "expected the field name `x` in messages, got: {:?}",
        msgs
    );
}

#[test]
fn access_unknown_field_lists_known_fields_in_help() {
    // `p.z` where Point's only record-shaped variant declares
    // `x` and `y`. The diagnostic should list `x` and `y` in
    // its help text so the user sees what's available.
    //
    // The diagnostics golden-test helper returns message
    // strings (not the full `Message` struct), so we drive the
    // typechecker manually to inspect both the message and its
    // help hint.
    let src = "enum Point { Origin, Point { x: int, y: int } } \
               let p = Point::Point { x: 1, y: 2 }; \
               p.z;";
    let ast = Pratt::default().parse(src).expect("parse failed");
    let mut c = Checker::new();
    let _ty = c.check_program(&ast);
    let msgs = c.take_messages();
    let no_field = msgs
        .iter()
        .find(|m| m.message().contains("no field `z`"))
        .expect("expected 'no field `z`' diagnostic");
    let help = no_field
        .help()
        .as_ref()
        .expect("expected help hint on no-field diagnostic");
    assert!(
        help.contains("`x`") && help.contains("`y`"),
        "expected help to list `x` and `y`, got: {:?}",
        help
    );
}

#[test]
fn access_field_ambiguous_across_variants_suggests_match() {
    // Two record-shaped variants both declare `x`. When the
    // receiver is annotated as the bare enum name (a function
    // parameter here), the typechecker resolves it through the
    // enum registry to a `Ty::Sum` — and the field is
    // ambiguous because TWO variants carry it. The diagnostic
    // must tell the user to narrow with a `match`.
    let (_ty, msgs) = check(
        "enum Two { A { x: int, y: int }, B { x: string, z: int } } \
         fn get_x(Two p) -> int { return p.x; }",
    );
    assert!(
        msgs.iter().any(|m| m.contains("narrow with match first")),
        "expected 'narrow with match first' diagnostic, got: {:?}",
        msgs
    );
}