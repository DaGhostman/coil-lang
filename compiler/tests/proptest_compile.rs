//! Property tests: compiling small well-typed programs never panics.
//!
//! Type errors / parse errors are OK (`Err(())`); a Rust panic is not.

use compiler::Pipeline;
use proptest::prelude::*;

fn small_program(a: i32, b: i32, kind: u8) -> String {
    match kind % 4 {
        0 => format!(
            "fn main() {{\n    print \"%i\", {a} + {b};\n}}\n"
        ),
        1 => format!(
            "fn main() {{\n    let x = {a};\n    let y = x + {b};\n    print \"%i\", y;\n}}\n"
        ),
        2 => format!(
            "fn main() {{\n    if {a} < {b} {{\n        print \"%i\", 1;\n    }} else {{\n        print \"%i\", 0;\n    }}\n}}\n"
        ),
        _ => format!(
            "enum Color {{\n    Red,\n    Blue,\n}}\n\
             fn main() {{\n    let c = Color::Red;\n    print \"%z\", c == Color::Red;\n}}\n"
        ),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn compile_small_programs_never_panics(
        a in -20i32..20,
        b in -20i32..20,
        kind in 0u8..4,
    ) {
        let src = small_program(a, b, kind);
        let result = std::panic::catch_unwind(|| {
            let mut pipeline = Pipeline::new();
            let _ = pipeline.compile_src(&src);
        });
        assert!(
            result.is_ok(),
            "compile panicked on source:\n{src}"
        );
    }
}
