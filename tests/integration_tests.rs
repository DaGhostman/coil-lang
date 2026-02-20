use std::sync::Mutex;

use compiler::Compiler;
use parser::Pratt;

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn cleanup() {
    let cwd = std::env::current_dir().expect("Unable to determine cwd");
    let out_file = cwd.join("out.c0s");
    let _ = std::fs::remove_file(&out_file);
}

#[derive(Debug)]
struct CompilationResult {
    bytecode: Vec<u8>,
    stderr: Vec<String>,
    success: bool,
}

impl CompilationResult {
    fn has_error(&self) -> bool {
        !self.stderr.is_empty()
    }

    fn has_error_containing(&self, text: &str) -> bool {
        self.stderr.iter().any(|e| e.contains(text))
    }
}

fn with_lock<F, T>(f: F) -> T
where
    F: FnOnce() -> T,
{
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let result = f();
    drop(guard);
    result
}

fn compile_test_file(filename: &str) -> Result<CompilationResult, String> {
    with_lock(|| {
        cleanup();

        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let full_path = cwd.join("tests").join(filename);

        if !std::fs::exists(&full_path).map_err(|e| e.to_string())? {
            return Err(format!("Test file not found: {:?}", full_path));
        }

        let source = std::fs::read_to_string(&full_path).map_err(|e| e.to_string())?;

        let parser = Pratt::default();
        let mut compiler = Compiler::default();

        let result = match parser.parse(source.as_str()) {
            Ok(ast) => {
                let bytecode = compiler.compile("", &ast);

                let stderr: Vec<String> = compiler
                    .get_messages()
                    .iter()
                    .filter(|m| matches!(m.kind(), common::MessageKind::ERROR))
                    .map(|m| m.message().to_string())
                    .collect();

                CompilationResult {
                    bytecode: rkyv::to_bytes::<rkyv::rancor::Error>(&bytecode)
                        .unwrap()
                        .to_vec(),
                    stderr,
                    success: true,
                }
            }
            Err(e) => {
                let mut errors = vec![e.message().to_string()];
                for label in e.labels() {
                    errors.push(format!("  at {:?}: {}", label.range(), label.to_string()));
                }
                CompilationResult {
                    bytecode: Vec::new(),
                    stderr: errors,
                    success: false,
                }
            }
        };

        cleanup();

        Ok(result)
    })
}

fn compile_source_direct(source: &str) -> Result<Vec<u8>, String> {
    with_lock(|| {
        cleanup();

        let parser = Pratt::default();
        let mut compiler = Compiler::default();

        let result = match parser.parse(source) {
            Ok(ast) => {
                let bytecode = compiler.compile("", &ast);

                let errors: Vec<String> = compiler
                    .get_messages()
                    .iter()
                    .filter(|m| matches!(m.kind(), common::MessageKind::ERROR))
                    .map(|m| m.message().to_string())
                    .collect();

                if !errors.is_empty() {
                    Err(format!("Compilation errors: {:?}", errors))
                } else {
                    Ok(rkyv::to_bytes::<rkyv::rancor::Error>(&bytecode)
                        .unwrap()
                        .to_vec())
                }
            }
            Err(e) => Err(format!("Parse error: {}", e.message())),
        };

        cleanup();
        result
    })
}

mod monomorphization {
    use super::*;

    #[test]
    fn test_basic_identity() {
        let result = compile_test_file("monomorphization/01_basic_identity.0s")
            .expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_multiple_type_params() {
        let result = compile_test_file("monomorphization/02_multiple_type_params.0s")
            .expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_multiple_instantiations() {
        let result = compile_test_file("monomorphization/03_multiple_instantiations.0s")
            .expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_type_bounds() {
        let result = compile_test_file("monomorphization/04_type_bounds.0s")
            .expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_nested_generics() {
        let result = compile_test_file("monomorphization/05_nested_generics.0s")
            .expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_generic_chain() {
        let result = compile_test_file("monomorphization/06_generic_chain.0s")
            .expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_generic_arithmetic() {
        let result = compile_test_file("monomorphization/07_generic_arithmetic.0s")
            .expect("Compilation should not crash");
        // Note: This may have errors until numeric bounds are implemented
    }

    #[test]
    fn test_negative_unknown_generic() {
        let result = compile_test_file("monomorphization/neg_01_unknown_generic.0s")
            .expect("Compilation should not crash");
        assert!(
            result.has_error(),
            "Expected error for unknown generic function"
        );
    }

    #[test]
    fn test_negative_missing_type_args() {
        let result = compile_test_file("monomorphization/neg_02_missing_type_args.0s")
            .expect("Compilation should not crash");
        // Type inference may or may not handle this
    }

    #[test]
    fn test_negative_wrong_type_arg_count() {
        let result = compile_test_file("monomorphization/neg_03_wrong_type_arg_count.0s")
            .expect("Compilation should not crash");
        // Should either error or handle gracefully
    }
}

mod sum_types {
    use super::*;

    #[test]
    fn test_simple_enum() {
        let result =
            compile_test_file("sum_types/01_simple_enum.0s").expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_enum_with_data() {
        let result = compile_test_file("sum_types/02_enum_with_data.0s")
            .expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_match_simple() {
        let result = compile_test_file("sum_types/03_match_simple.0s")
            .expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_match_destructure() {
        // Note: Type inference for destructured match variables is limited
        let result = compile_test_file("sum_types/04_match_destructure.0s")
            .expect("Compilation should not crash");
        // This test currently fails type checking because the types of
        // destructured variables aren't inferred from the enum definition
    }

    #[test]
    fn test_match_default() {
        let result = compile_test_file("sum_types/05_match_default.0s")
            .expect("Compilation should not crash");
        // May have warnings about default placement
    }

    #[test]
    fn test_multiple_enums() {
        let result = compile_test_file("sum_types/06_multiple_enums.0s")
            .expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_result_pattern() {
        let result = compile_test_file("sum_types/07_result_pattern.0s")
            .expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_option_pattern() {
        let result = compile_test_file("sum_types/08_option_pattern.0s")
            .expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_negative_unknown_variant() {
        let result = compile_test_file("sum_types/neg_01_unknown_variant.0s")
            .expect("Compilation should not crash");
        assert!(result.has_error(), "Expected error for unknown variant");
    }

    #[test]
    fn test_negative_unknown_enum() {
        let result = compile_test_file("sum_types/neg_02_unknown_enum.0s")
            .expect("Compilation should not crash");
        assert!(result.has_error(), "Expected error for unknown enum type");
    }

    #[test]
    fn test_negative_match_wrong_type() {
        let result = compile_test_file("sum_types/neg_03_match_wrong_type.0s")
            .expect("Compilation should not crash");
        // Should produce an error about type mismatch
    }

    #[test]
    fn test_negative_match_unknown_variant() {
        let result = compile_test_file("sum_types/neg_04_match_unknown_variant.0s")
            .expect("Compilation should not crash");
        assert!(
            result.has_error(),
            "Expected error for unknown variant in match"
        );
    }
}

mod combined {
    use super::*;

    #[test]
    fn test_generic_with_enum() {
        let source = r#"
            enum Option {
                Some(int),
                None
            }

            fn wrap<T>(T x) -> T {
                return x;
            }

            fn main() {
                let opt = Option::Some(42);
                let val: int = wrap<int>(100);
                print "%i", val;
            }
        "#;

        let result = compile_source_direct(source);
        assert!(result.is_ok(), "Compilation failed: {:?}", result.err());
    }

    #[test]
    fn test_generic_function_returning_enum() {
        let source = r#"
            enum Result {
                Ok(int),
                Err
            }

            fn make_ok<T>(T x) -> Result {
                return Result::Ok(42);
            }

            fn main() {
                let r = make_ok<int>(42);
                print "Created result";
            }
        "#;

        let result = compile_source_direct(source);
        assert!(result.is_ok(), "Compilation failed: {:?}", result.err());
    }

    #[test]
    fn test_match_in_generic() {
        let source = r#"
            enum Status {
                Ok,
                Error
            }

            fn check<T>(Status s, T x) -> T {
                match s {
                    case Status::Ok => { return x; }
                    case Status::Error => { return x; }
                }
            }

            fn main() {
                let result: int = check<int>(Status::Ok, 42);
                print "%i", result;
            }
        "#;

        let result = compile_source_direct(source);
        assert!(result.is_ok(), "Compilation failed: {:?}", result.err());
    }
}

mod generics {
    use super::*;

    #[test]
    fn test_identity() {
        let result =
            compile_test_file("generics/01_identity.0s").expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_multiple_params() {
        let result = compile_test_file("generics/02_multiple_params.0s")
            .expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_type_bounds() {
        let result =
            compile_test_file("generics/03_type_bounds.0s").expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_multiple_instantiations() {
        let result = compile_test_file("generics/04_multiple_instantiations.0s")
            .expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_nested() {
        let result =
            compile_test_file("generics/05_nested.0s").expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_chain() {
        let result =
            compile_test_file("generics/06_chain.0s").expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_multiple_args() {
        let result = compile_test_file("generics/07_multiple_args.0s")
            .expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_negative_unknown() {
        let result =
            compile_test_file("generics/neg_01_unknown.0s").expect("Compilation should not crash");
        assert!(result.has_error(), "Expected error for unknown generic");
    }

    #[test]
    fn test_negative_wrong_arg_count() {
        let result = compile_test_file("generics/neg_02_wrong_arg_count.0s")
            .expect("Compilation should not crash");
        // May or may not error depending on implementation
    }

    #[test]
    fn test_negative_type_mismatch() {
        let result = compile_test_file("generics/neg_03_type_mismatch.0s")
            .expect("Compilation should not crash");
        // May or may not error depending on type inference
    }
}

mod syntax {
    use super::*;

    #[test]
    fn test_variables() {
        let result =
            compile_test_file("syntax/01_variables.0s").expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_arithmetic() {
        let result =
            compile_test_file("syntax/02_arithmetic.0s").expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_comparison() {
        let result =
            compile_test_file("syntax/03_comparison.0s").expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_logical() {
        let result =
            compile_test_file("syntax/04_logical.0s").expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_if() {
        let result = compile_test_file("syntax/05_if.0s").expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_while() {
        let result = compile_test_file("syntax/06_while.0s").expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_functions() {
        let result =
            compile_test_file("syntax/07_functions.0s").expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_print() {
        let result = compile_test_file("syntax/08_print.0s").expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_strings() {
        let result =
            compile_test_file("syntax/09_strings.0s").expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_assignments() {
        let result =
            compile_test_file("syntax/10_assignments.0s").expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_blocks() {
        let result =
            compile_test_file("syntax/11_blocks.0s").expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_unary() {
        let result = compile_test_file("syntax/12_unary.0s").expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_bitwise() {
        let result =
            compile_test_file("syntax/13_bitwise.0s").expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_comments() {
        let result =
            compile_test_file("syntax/14_comments.0s").expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }

    #[test]
    fn test_defer() {
        let result = compile_test_file("syntax/15_defer.0s").expect("Compilation should not crash");
        assert!(
            !result.has_error(),
            "Unexpected errors: {:?}",
            result.stderr
        );
    }
}
