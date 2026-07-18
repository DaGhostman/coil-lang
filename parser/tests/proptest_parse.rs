//! Property tests: parser never panics on generated small programs.
//!
//! Failures may return `Err(Message)`; that is fine. A Rust panic is not.

use parser::Pratt;
use proptest::prelude::*;

/// Build a small well-formed-ish source string from constrained components.
fn program_from_parts(
    a: i32,
    b: i32,
    use_if: bool,
    use_let: bool,
    ident_idx: u8,
) -> String {
    let names = ["x", "y", "n", "tmp", "acc"];
    let name = names[(ident_idx as usize) % names.len()];
    let mut body = String::new();
    if use_let {
        body.push_str(&format!("    let {name} = {a};\n"));
        body.push_str(&format!("    {name} = {name} + {b};\n"));
        body.push_str(&format!("    print \"%i\", {name};\n"));
    } else {
        body.push_str(&format!("    print \"%i\", {a} + {b};\n"));
    }
    if use_if {
        body.push_str(&format!(
            "    if {a} < {b} {{ print \"%i\", 1; }} else {{ print \"%i\", 0; }}\n"
        ));
    }
    format!("fn main() {{\n{body}}}\n")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn parse_small_programs_never_panics(
        a in -50i32..50,
        b in -50i32..50,
        use_if in any::<bool>(),
        use_let in any::<bool>(),
        ident_idx in 0u8..5,
    ) {
        let src = program_from_parts(a, b, use_if, use_let, ident_idx);
        let result = std::panic::catch_unwind(|| {
            let _ = Pratt::default().parse(&src);
        });
        assert!(
            result.is_ok(),
            "parser panicked on source:\n{src}"
        );
    }

    #[test]
    fn parse_random_bytes_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..64)) {
        // Lossy UTF-8 is fine — we only care that parse does not abort.
        let src = String::from_utf8_lossy(&bytes).into_owned();
        let result = std::panic::catch_unwind(|| {
            let _ = Pratt::default().parse(&src);
        });
        assert!(
            result.is_ok(),
            "parser panicked on random input ({src:?})"
        );
    }
}
