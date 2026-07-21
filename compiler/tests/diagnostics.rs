//! Golden tests for typechecker diagnostic messages.

use compiler::{Checker, ErrorCode, Message};
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

fn check_messages(src: &str) -> Vec<Message> {
    let ast = Pratt::default().parse(src).expect("parse failed");
    let mut c = Checker::new();
    let _ = c.check_program(&ast);
    c.take_messages()
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
fn unknown_identifier_has_stable_error_code() {
    let msgs = check_messages("x;");
    assert!(
        msgs.iter()
            .any(|m| m.code() == Some(ErrorCode::UnknownValue)),
        "expected ErrorCode::UnknownValue (E0100), got: {:?}",
        msgs.iter().map(|m| m.code()).collect::<Vec<_>>()
    );
}

#[test]
fn type_mismatch_has_stable_error_code() {
    let msgs = check_messages(r#"let x: int = "hello";"#);
    assert!(
        msgs.iter()
            .any(|m| m.code() == Some(ErrorCode::TypeMismatch)),
        "expected ErrorCode::TypeMismatch (E0102), got: {:?}",
        msgs.iter().map(|m| m.code()).collect::<Vec<_>>()
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
fn positional_after_named_argument_is_rejected() {
    let (_ty, msgs) = check(
        r#"
fn greet(string name, int age) {}
fn main() {
    greet(age: 36, "Ada");
}
"#,
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("Positional argument after named argument")),
        "expected positional-after-named diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn unknown_named_argument_is_rejected() {
    let (_ty, msgs) = check(
        r#"
fn greet(string name, int age) {}
fn main() {
    greet(name: "Ada", years: 36);
}
"#,
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("Unknown named argument `years`")),
        "expected unknown named argument diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn duplicate_named_argument_is_rejected() {
    let (_ty, msgs) = check(
        r#"
fn greet(string name, int age) {}
fn main() {
    greet(name: "Ada", name: "Grace");
}
"#,
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("Duplicate named argument `name`")),
        "expected duplicate named argument diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn duplicate_let_pattern_binder_is_rejected() {
    let (_ty, msgs) = check(
        r#"
fn main() {
    let (x, x) = (1, 2);
}
"#,
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("Duplicate binder `x` in let pattern")),
        "expected duplicate binder diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn named_call_under_applied_is_rejected() {
    // Named calls must supply every remaining parameter (no partial
    // application once any arg is named).
    let (_ty, msgs) = check(
        r#"
fn greet(string name, int age) {}
fn main() {
    greet(name: "Ada");
}
"#,
    );
    assert!(
        msgs.iter().any(|m| {
            m.contains("under-applied") || m.contains("Missing argument `age`")
        }),
        "expected under-applied / missing named-arg diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn let_tuple_destructure_arity_mismatch_is_rejected() {
    let (_ty, msgs) = check(
        r#"
fn main() {
    let (a, b) = (1, 2, 3);
}
"#,
    );
    assert!(
        msgs.iter().any(|m| {
            m.contains("tuple pattern has")
                || m.contains("Type mismatch")
                || m.contains("let tuple destructure")
        }),
        "expected tuple destructure arity diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn let_tuple_destructure_on_non_tuple_is_rejected() {
    let (_ty, msgs) = check(
        r#"
fn main() {
    let (a, b) = 42;
}
"#,
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("cannot destructure type") && m.contains("tuple pattern")),
        "expected cannot-destructure-with-tuple diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn let_record_duplicate_field_is_rejected() {
    // Distinct binders so this stays a *field* diagnostic (not binder).
    let (_ty, msgs) = check(
        r#"
fn main() {
    let { x: a, x: b } = { x: 1, y: 2 };
}
"#,
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("Duplicate field `x` in record pattern")),
        "expected duplicate field in let record pattern, got: {:?}",
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
    let ast = Pratt::default()
        .parse("undeclared = 1;")
        .expect("parse failed");
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
    assert!(
        msgs.is_empty(),
        "string literal should type-check, got: {:?}",
        msgs
    );
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

// ---- Sum types and pattern matching ----

#[test]
fn enum_decl_no_messages() {
    // A bare `enum` declaration produces no diagnostic.
    let (_ty, msgs) = check("enum Color { Red, Green(int) }");
    assert!(
        msgs.is_empty(),
        "enum declaration should produce no messages, got: {:?}",
        msgs
    );
}

#[test]
fn match_with_all_variants_no_messages() {
    // All variants covered → no diagnostic.
    let src = "let x = Option::Some(1); match x { Option::None() => 0, Option::Some(v) => v };";
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
    let src = "let x = Option::None(); match x { Option::None() => 0 };";
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
    let src = "let x = Option::None(); match x { Option::None() => 0, Option::None() => 1 };";
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
    let src = "Option::Some(1, 2);";
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

#[test]
fn format_percent_i_rejects_open_type_suggests_percent_v() {
    let msgs = check_messages(
        "fn bad<T>(T x) { print \"%i\", x; } \
         fn main() { bad(1); }",
    );
    assert!(
        msgs.iter().any(|m| {
            m.message().contains("open type") && m.help().as_ref().is_some_and(|h| h.contains("%v"))
        }),
        "expected open-type `%i` diagnostic suggesting `%v`, got: {:?}",
        msgs
    );
}

#[test]
fn format_percent_v_requires_show_bound() {
    let (_ty, msgs) = check(
        "fn bad<T>(T x) { print \"%v\", x; } \
         fn main() { bad(1); }",
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("`Show`") || m.contains("Show")),
        "expected `%v` without Show bound to error, got: {:?}",
        msgs
    );
}

#[test]
fn format_percent_v_requires_show_bound_inside_structural_tuple() {
    let (_ty, msgs) = check(
        "fn bad<T>(T x) { print \"%v\", (x, 1); } \
         fn main() { bad(1); }",
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("`Show`") || m.contains("Show")),
        "expected structural `%v` with open T to require Show, got: {:?}",
        msgs
    );
}

#[test]
fn format_percent_v_accepts_show_bound() {
    let (_ty, msgs) = check(
        "fn ok<T: Show>(T x) { print \"%v\", x; } \
         fn main() { ok(1); }",
    );
    assert!(
        msgs.is_empty(),
        "expected `%v` with Show bound to typecheck, got: {:?}",
        msgs
    );
}

#[test]
fn existential_pack_without_instance_reports_missing_instance() {
    let (_ty, msgs) = check(
        "trait Printable<T> { fn printable(T x) -> int; } \
         fn take(Printable x) { } \
         fn main() { take(42); }",
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("No instance for `Printable<int>`")),
        "expected missing existential instance diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn multiparam_typeclass_cannot_be_bare_existential_type() {
    let (_ty, msgs) = check(
        "trait Convert<A, B> { fn cast(A x) -> B; } \
         fn take(Convert x) { }",
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("Typeclass `Convert` cannot be used as a bare value type")),
        "expected bare multi-param trait diagnostic, got: {:?}",
        msgs
    );
}

// ---- Record-shape diagnostics ----

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
        msgs.iter()
            .any(|m| m.contains("Unknown field `z`") || m.contains("no field `z`")),
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
        msgs.iter()
            .any(|m| m.contains("Unknown field `z`") || m.contains("missing field `z`")),
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

// ---- Field-access diagnostics ----

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

#[test]
fn yield_outside_async_fn_reports_diagnostic() {
    let (_ty, msgs) = check("fn main() { yield 1; }");
    assert!(
        msgs.iter().any(|m| m.contains("yield outside async")),
        "expected yield-outside-async diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn binding_yield_outside_async_fn_reports_diagnostic() {
    let (_ty, msgs) = check("fn main() { let x = yield 1; }");
    assert!(
        msgs.iter().any(|m| m.contains("yield outside async")),
        "expected yield-outside-async diagnostic for binding yield, got: {:?}",
        msgs
    );
}

#[test]
fn try_on_int_has_stable_invalid_try_code() {
    let msgs = check_messages("fn f() -> int { let x = 1; return x?; }");
    assert!(
        msgs.iter().any(|m| m.code() == Some(ErrorCode::InvalidTry)),
        "expected ErrorCode::InvalidTry (E0114), got: {:?}",
        msgs.iter().map(|m| m.code()).collect::<Vec<_>>()
    );
}

#[test]
fn coalesce_on_int_has_stable_invalid_coalesce_code() {
    let msgs = check_messages("fn main() { let x = 1 ?? 2; }");
    assert!(
        msgs.iter()
            .any(|m| m.code() == Some(ErrorCode::InvalidCoalesce)),
        "expected ErrorCode::InvalidCoalesce (E0115), got: {:?}",
        msgs.iter().map(|m| m.code()).collect::<Vec<_>>()
    );
}

#[test]
fn optional_access_on_result_has_stable_code() {
    let msgs = check_messages("fn main() { let r = Result::Ok({ v: 1 }); let _x = r?.v; }");
    assert!(
        msgs.iter()
            .any(|m| m.code() == Some(ErrorCode::InvalidOptionalAccess)),
        "expected ErrorCode::InvalidOptionalAccess (E0116), got: {:?}",
        msgs.iter().map(|m| m.code()).collect::<Vec<_>>()
    );
}

/// Phase 5: HKT class instances must take a bare constructor, not an application.
#[test]
fn hkt_instance_rejects_applied_type_argument() {
    let (_ty, msgs) = check(
        r#"
        trait Container<F: * -> *> {
            fn first<A>(F<A> xs) -> A;
        }
        impl Container<Option<int>> {
            fn first<A>(Option<A> xs) -> A {
                return match xs {
                    Option::Some(v) => v,
                    Option::None => 0,
                };
            }
        }
        "#,
    );
    assert!(
        msgs.iter().any(|m| m.contains("constructor-kinded class")
            && m.contains("type constructor")
            && m.contains("* -> *")),
        "expected HKT instance kind diagnostic, got: {:?}",
        msgs
    );
}

/// Phase 5: a `* -> *` variable cannot be used where a proper type is required.
#[test]
fn hkt_var_rejected_as_type_argument() {
    let (_ty, msgs) = check(
        r#"
        trait Container<F: * -> *> {
            fn first<A>(F<A> xs) -> A;
        }
        fn bad<F: Container, A>(F<F> xs) -> A {
            return first(xs);
        }
        "#,
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("kind `* -> *`") && m.contains("expected `*`")),
        "expected kind-mismatch diagnostic for F used as type arg, got: {:?}",
        msgs
    );
}

#[test]
fn constraint_bound_rejects_non_constraint_kind_parameter() {
    let (_ty, msgs) = check(
        r#"
        fn bad<c: Constraint, T: c>(T x) -> T {
            return x;
        }
        "#,
    );
    assert!(
        msgs.iter().any(|m| {
            m.contains("Constraint parameter `c` has kind `Constraint`")
                && m.contains("expected `* -> Constraint`")
        }),
        "expected ill-kinded constraint parameter diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn abstract_constraint_bound_requires_concrete_method_selection() {
    let (_ty, msgs) = check(
        r#"
        fn bad<c: * -> Constraint, T: c>(T x) -> T {
            return x;
        }
        "#,
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("Cannot satisfy abstract constraint")),
        "expected unsatisfied abstract constraint diagnostic, got: {:?}",
        msgs
    );
}

/// Phase 5: impl of a subclass requires the superclass instance.
#[test]
fn superclass_impl_requires_superclass_instance() {
    let (_ty, msgs) = check(
        r#"
        trait Equal<T> { fn eq_val(T a, T b) -> bool; }
        trait Ordered<T: Equal> { fn lt_val(T a, T b) -> bool; }
        impl Ordered<int> {
            fn lt_val(int a, int b) -> bool { return a < b; }
        }
        "#,
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("requires superclass instance")
                && m.contains("Equal")
                && m.contains("Ordered")),
        "expected missing Equal superclass diagnostic, got: {:?}",
        msgs
    );
}

/// Phase 6: impl must define every associated type declared by the class.
#[test]
fn assoc_type_missing_in_impl_errors() {
    let (_ty, msgs) = check(
        r#"
        trait Collect<C> {
            type Elem;
            fn head(C xs) -> Elem;
        }
        impl Collect<int> {
            fn head(int xs) -> int { return xs; }
        }
        "#,
    );
    assert!(
        msgs.iter().any(|m| m.contains("missing associated type")
            && m.contains("Elem")
            && m.contains("Collect")),
        "expected missing associated type diagnostic, got: {:?}",
        msgs
    );
}

/// Phase 6: impl cannot define an unknown associated type.
#[test]
fn assoc_type_unknown_in_impl_errors() {
    let (_ty, msgs) = check(
        r#"
        trait Collect<C> {
            type Elem;
            fn head(C xs) -> Elem;
        }
        impl Collect<int> {
            type Elem = int;
            type Extra = int;
            fn head(int xs) -> int { return xs; }
        }
        "#,
    );
    assert!(
        msgs.iter().any(|m| m.contains("Unknown associated type")
            && m.contains("Extra")
            && m.contains("Collect")),
        "expected unknown associated type diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn gat_impl_wrong_number_of_params_errors() {
    let (_ty, msgs) = check(
        r#"
        trait Pointer<P: * -> *> {
            type Ref<T>;
            fn deref<T>(P<T> ptr) -> Ref<T>;
        }
        impl Pointer<Option> {
            type Ref = int;
            fn deref<T>(Option<T> ptr) -> T { return 0; }
        }
        "#,
    );
    assert!(
        msgs.iter().any(|m| m.contains(
            "Associated type `Ref` in instance of `Pointer` expects 1 type parameter, got 0"
        )),
        "expected GAT impl arity diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn gat_projection_wrong_number_of_args_errors() {
    let (_ty, msgs) = check(
        r#"
        trait Pointer<P: * -> *> {
            type Ref<T>;
            fn deref<T>(P<T> ptr) -> Ref<T>;
        }
        fn bad<P: * -> *, Pointer, A>(P<A> ptr) -> P::Ref<A, int> {
            return deref(ptr);
        }
        "#,
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("Associated type `Pointer::Ref` expects 1 type argument, got 2")),
        "expected GAT projection arity diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn duplicate_typeclass_errors() {
    let (_ty, msgs) = check(
        r#"
        trait Tiny<T> { fn id(T x) -> T; }
        trait Tiny<T> { fn id(T x) -> T; }
        "#,
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("Duplicate trait `Tiny`")),
        "expected duplicate trait diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn orphan_instance_for_foreign_class_and_structural_type_errors() {
    let (_ty, msgs) = check(
        r#"
        impl Show<(int, int)> {
            fn show((int, int) x) -> string { return ""; }
        }
        "#,
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("Orphan instance `Show<(int, int)>`")),
        "expected orphan-instance diagnostic, got: {:?}",
        msgs
    );
}

/// Builtin source type is rejected: every non-variable instance arg must
/// be a local nominal head (strict orphan rule).
#[test]
fn into_impl_for_builtin_source_is_orphan() {
    let (_ty, msgs) = check(
        r#"
        class Wrapper { v: int }
        impl Into<Wrapper> for int {
            fn into(int x) -> Wrapper { return new Wrapper(x); }
        }
        "#,
    );
    assert!(
        msgs.iter().any(|m| m.contains("Orphan instance")),
        "expected orphan diagnostic for Into<Wrapper> for int, got: {:?}",
        msgs
    );
}

#[test]
fn overlapping_typeclass_instance_names_new_and_existing_instances() {
    let (_ty, msgs) = check(
        r#"
        trait Tiny<T> { fn id(T x) -> T; }
        impl Tiny<int> {
            fn id(int x) -> int { return x; }
        }
        impl Tiny<int> {
            fn id(int x) -> int { return x; }
        }
        "#,
    );
    assert!(
        msgs.iter().any(|m| {
            m.contains("Overlapping instance `Tiny<int>`") && m.contains("existing `Tiny<int>`")
        }),
        "expected overlapping-instance diagnostic, got: {:?}",
        msgs
    );
}

/// Compile through the full compiler so derive expansion diagnostics fire
/// (Checker alone never sees the header `derive` clause).
fn compile_messages(src: &str) -> Vec<String> {
    let mut ast = Pratt::default().parse(src).expect("parse failed");
    let mut c = compiler::Compiler::default();
    let _ = c.compile("", &mut ast);
    c.get_messages()
        .iter()
        .map(|m| m.message().to_string())
        .collect()
}

#[test]
fn derive_unknown_trait_reports_diagnostic() {
    let msgs = compile_messages("enum Color derive Clone { Red } fn main() {}");
    assert!(
        msgs.iter()
            .any(|m| m.contains("Cannot derive unknown or non-derivable trait `Clone`")),
        "expected unknown-trait derive diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn derive_non_derivable_trait_reports_diagnostic() {
    let msgs = compile_messages("enum Color derive Num { Red } fn main() {}");
    assert!(
        msgs.iter()
            .any(|m| m.contains("Cannot derive unknown or non-derivable trait `Num`")),
        "expected non-derivable trait diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn derive_generic_enum_reports_diagnostic() {
    let msgs = compile_messages("enum Box<T> derive Show { Box(T) } fn main() {}");
    assert!(
        msgs.iter().any(|m| {
            m.contains("Cannot derive traits for generic enum `Box`")
                && m.contains("write an explicit `impl`")
        }),
        "expected generic-enum derive diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn derive_generic_class_reports_diagnostic() {
    let msgs = compile_messages("class Cell<T> derive Eq { value: T } fn main() {}");
    assert!(
        msgs.iter().any(|m| {
            m.contains("Cannot derive traits for generic class `Cell`")
                && m.contains("write an explicit `impl`")
        }),
        "expected generic-class derive diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn ffi_dload_without_use_errors() {
    let (_ty, msgs) = check(r#"fn main() { let lib = dload("x.so"); }"#);
    assert!(
        msgs.iter().any(|m| m.contains("Cannot find value `dload`") || m.contains("Cannot find function `dload`")),
        "expected missing dload without `use ffi`, got: {:?}",
        msgs
    );
}

#[test]
fn declare_wrong_arity_errors() {
    let msgs = check_messages(
        r#"
        use ffi::*;
        fn main() {
            let lib = match dload("x.so") {
                Result::Ok(h) => h,
                Result::Err(_) => 0,
            };
            declare(lib, "f", Int);
        }
        "#,
    );
    assert!(
        msgs.iter().any(|m| m.code() == Some(ErrorCode::DeclareArity)
            || m.message().contains("declare")
            || m.message().contains("Declare")),
        "expected DeclareArity diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn invoke_wrong_arity_errors() {
    let msgs = check_messages(
        r#"
        use ffi::*;
        fn main() {
            let lib = match dload("x.so") {
                Result::Ok(h) => h,
                Result::Err(_) => 0,
            };
            let id = 0;
            invoke(lib, id);
        }
        "#,
    );
    assert!(
        msgs.iter().any(|m| m.code() == Some(ErrorCode::InvokeArity)
            || m.message().to_lowercase().contains("invoke")),
        "expected InvokeArity diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn io_stdin_without_import_errors() {
    let (_ty, msgs) = check(r#"fn main() { let s = stdin(); }"#);
    assert!(
        msgs.iter().any(|m| m.contains("stdin")),
        "expected unknown stdin without `use io`, got: {:?}",
        msgs
    );
}

#[test]
fn array_literal_index_oob_errors() {
    let msgs = check_messages(
        r#"
        fn main() {
            let a = [0, 1, 2];
            let x = a[3];
        }
        "#,
    );
    assert!(
        msgs.iter().any(|m| m.code() == Some(ErrorCode::IndexOutOfBounds)
            || m.message().contains("out of bounds")),
        "expected IndexOutOfBounds, got: {:?}",
        msgs
    );
}

#[test]
fn tuple_literal_index_oob_errors() {
    let msgs = check_messages(
        r#"
        fn main() {
            let t = (1, 2);
            let x = t[5];
        }
        "#,
    );
    assert!(
        msgs.iter().any(|m| m.code() == Some(ErrorCode::IndexOutOfBounds)
            || m.message().contains("out of bounds")),
        "expected tuple IndexOutOfBounds, got: {:?}",
        msgs
    );
}

#[test]
fn index_on_int_errors() {
    let msgs = check_messages(
        r#"
        fn main() {
            let x = 5;
            let y = x[0];
        }
        "#,
    );
    assert!(
        msgs.iter().any(|m| m.code() == Some(ErrorCode::CannotIndex)
            || m.message().to_lowercase().contains("index")),
        "expected CannotIndex, got: {:?}",
        msgs
    );
}

#[test]
fn record_pattern_missing_field_errors() {
    let (_ty, msgs) = check(
        r#"
        enum P { P { x: int, y: int } }
        fn f(P p) -> int {
            return match p {
                P::P { x } => x,
            };
        }
        "#,
    );
    assert!(
        msgs.iter().any(|m| m.contains("Missing field") || m.contains("missing field") || m.contains("`y`")),
        "expected missing field in record pattern, got: {:?}",
        msgs
    );
}

#[test]
fn record_pattern_duplicate_field_errors() {
    let (_ty, msgs) = check(
        r#"
        enum P { P { x: int, y: int } }
        fn f(P p) -> int {
            return match p {
                P::P { x, x } => x,
            };
        }
        "#,
    );
    assert!(
        msgs.iter().any(|m| m.contains("Duplicate field") || m.contains("duplicate field")),
        "expected duplicate field in record pattern, got: {:?}",
        msgs
    );
}

#[test]
fn record_pattern_shape_mismatch_errors() {
    let (_ty, msgs) = check(
        r#"
        enum P { P { x: int, y: int } }
        fn f(P p) -> int {
            return match p {
                P::P(x, y) => x + y,
            };
        }
        "#,
    );
    assert!(
        msgs.iter().any(|m| m.contains("shape") || m.contains("payload") || m.contains("Missing field") || m.contains("Constructor")),
        "expected record/tuple shape mismatch, got: {:?}",
        msgs
    );
}

#[test]
fn const_reassignment_errors() {
    let (_ty, msgs) = check(
        r#"
        fn main() {
            const x = 1;
            x = 2;
        }
        "#,
    );
    assert!(
        msgs.iter().any(|m| m.contains("Cannot assign to constant `x`") || m.contains("constant")),
        "expected const reassignment diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn panic_non_string_errors() {
    let (_ty, msgs) = check(r#"fn main() { panic 1; }"#);
    assert!(
        !msgs.is_empty(),
        "expected diagnostic for non-string panic, got: {:?}",
        msgs
    );
    assert!(
        msgs.iter().any(|m| m.contains("Type mismatch") || m.contains("string")),
        "expected string/type mismatch for panic, got: {:?}",
        msgs
    );
}

#[test]
fn array_element_type_mismatch_errors() {
    let msgs = check_messages(r#"fn main() { let a = [1, "x"]; }"#);
    assert!(
        msgs.iter().any(|m| m.code() == Some(ErrorCode::ArrayElementMismatch)
            || m.message().contains("array element")),
        "expected ArrayElementMismatch, got: {:?}",
        msgs
    );
}

