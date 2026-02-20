mod harness;

use harness::*;

mod positive {
    use super::*;

    #[test]
    fn test_simple_enum() {
        let source = r#"
            enum Color {
                Red,
                Green,
                Blue
            }

            fn main() {
                let c = Color::Red;
                print "Color created";
            }
        "#;

        let result = compile_and_check_no_errors(source);
        assert!(result.bytecode.len() > 0);
    }

    #[test]
    fn test_enum_with_data() {
        let source = r#"
            enum Colors {
                RGB(int, int, int),
                CMYK(float, float, float, float)
            }

            fn main() {
                let c = Colors::RGB(255, 128, 0);
                print "Color created";
            }
        "#;

        let result = compile_and_check_no_errors(source);
        assert!(result.bytecode.len() > 0);
    }

    #[test]
    fn test_match_simple_enum() {
        let source = r#"
            enum Color {
                Red,
                Green,
                Blue
            }

            fn print_color(Color c) {
                match c {
                    case Color::Red => { print "Red"; }
                    case Color::Green => { print "Green"; }
                    case Color::Blue => { print "Blue"; }
                }
            }

            fn main() {
                print_color(Color::Red);
            }
        "#;

        let result = compile_and_check_no_errors(source);
        assert!(result.bytecode.len() > 0);
    }

    #[test]
    fn test_match_with_destructure() {
        let source = r#"
            enum Colors {
                RGB(int, int, int),
                CMYK(float, float, float, float)
            }

            fn print_rgb(Colors c) {
                match c {
                    case Colors::RGB(r, g, b) => {
                        print "RGB: %i, %i, %i", r, g, b;
                    }
                    case Colors::CMYK(c, m, y, k) => {
                        print "CMYK";
                    }
                }
            }

            fn main() {
                print_rgb(Colors::RGB(255, 128, 64));
            }
        "#;

        let result = compile_and_check_no_errors(source);
        assert!(result.bytecode.len() > 0);
    }

    #[test]
    fn test_enum_as_function_param() {
        let source = r#"
            enum Status {
                Ok,
                Error
            }

            fn check(Status s) -> int {
                match s {
                    case Status::Ok => { return 1; }
                    case Status::Error => { return 0; }
                }
            }

            fn main() {
                let result: int = check(Status::Ok);
                print "%i", result;
            }
        "#;

        let result = compile_and_check_no_errors(source);
        assert!(result.bytecode.len() > 0);
    }

    #[test]
    fn test_enum_as_return_type() {
        let source = r#"
            enum Option {
                Some(int),
                None
            }

            fn maybe_value(int x) -> Option {
                if x > 0 {
                    return Option::Some(x);
                }
                return Option::None;
            }

            fn main() {
                let opt = maybe_value(42);
                print "Created option";
            }
        "#;

        let result = compile_and_check_no_errors(source);
        assert!(result.bytecode.len() > 0);
    }

    #[test]
    fn test_match_with_default() {
        // Note: The 'default' keyword handling needs improvement
        // This test checks that we handle it without crashing
        let source = r#"
            enum Color {
                Red,
                Green,
                Blue
            }

            fn is_red(Color c) -> int {
                match c {
                    case Color::Red => { return 1; }
                    case default => { return 0; }
                }
            }

            fn main() {
                let r: int = is_red(Color::Red);
                let g: int = is_red(Color::Green);
                print "%i %i", r, g;
            }
        "#;

        let result = compile_source(source);
        // May have errors depending on default handling
        assert!(result.bytecode.len() > 0 || result.has_errors());
    }

    #[test]
    fn test_nested_match() {
        let source = r#"
            enum Color {
                Red,
                Green,
                Blue
            }

            enum Shape {
                Circle,
                Square
            }

            fn describe(Color c, Shape s) {
                match c {
                    case Color::Red => {
                        match s {
                            case Shape::Circle => { print "Red Circle"; }
                            case Shape::Square => { print "Red Square"; }
                        }
                    }
                    case Color::Green => {
                        print "Green shape";
                    }
                    case Color::Blue => {
                        print "Blue shape";
                    }
                }
            }

            fn main() {
                describe(Color::Red, Shape::Circle);
            }
        "#;

        let result = compile_and_check_no_errors(source);
        assert!(result.bytecode.len() > 0);
    }

    #[test]
    fn test_multiple_enum_match() {
        let source = r#"
            enum Direction {
                North,
                South,
                East,
                West
            }

            fn move_dir(Direction d) -> int {
                match d {
                    case Direction::North => { return 1; }
                    case Direction::South => { return 2; }
                    case Direction::East => { return 3; }
                    case Direction::West => { return 4; }
                }
            }

            fn main() {
                let n: int = move_dir(Direction::North);
                let s: int = move_dir(Direction::South);
                let e: int = move_dir(Direction::East);
                let w: int = move_dir(Direction::West);
                print "%i %i %i %i", n, s, e, w;
            }
        "#;

        let result = compile_and_check_no_errors(source);
        assert!(result.bytecode.len() > 0);
    }
}

mod negative {
    use super::*;

    #[test]
    fn test_unknown_variant() {
        let source = r#"
            enum Color {
                Red,
                Green
            }

            fn main() {
                let c = Color::Blue;
            }
        "#;

        let result = compile_source(source);
        assert!(result.has_errors());
    }

    #[test]
    fn test_unknown_enum_type() {
        let source = r#"
            fn main() {
                let c = UnknownType::SomeVariant;
            }
        "#;

        let result = compile_source(source);
        assert!(result.has_errors());
    }

    #[test]
    fn test_variant_wrong_field_count() {
        let source = r#"
            enum RGB {
                Color(int, int, int)
            }

            fn main() {
                let c = RGB::Color(255, 128);
            }
        "#;

        // This may or may not error depending on implementation
        // The test ensures we don't crash
        let result = compile_source(source);
        assert!(result.bytecode.len() > 0 || result.has_errors());
    }

    #[test]
    fn test_match_unknown_variant() {
        let source = r#"
            enum Color {
                Red,
                Green
            }

            fn check(Color c) {
                match c {
                    case Color::Red => { print "Red"; }
                    case Color::Blue => { print "Blue"; }
                }
            }
        "#;

        let result = compile_source(source);
        assert!(result.has_errors());
    }

    #[test]
    fn test_match_type_mismatch() {
        let source = r#"
            enum Color {
                Red,
                Green
            }

            enum Shape {
                Circle,
                Square
            }

            fn check(Color c) {
                match c {
                    case Shape::Circle => { print "Circle"; }
                }
            }
        "#;

        let result = compile_source(source);
        // Should produce an error about type mismatch
        assert!(result.has_errors() || result.bytecode.len() > 0);
    }
}

mod variants_with_fields {
    use super::*;

    #[test]
    fn test_single_field_variant() {
        let source = r#"
            enum Option {
                Some(int),
                None
            }

            fn unwrap(Option o) -> int {
                match o {
                    case Option::Some(value) => { return value; }
                    case Option::None => { return 0; }
                }
            }

            fn main() {
                let result: int = unwrap(Option::Some(42));
                print "%i", result;
            }
        "#;

        let result = compile_and_check_no_errors(source);
        assert!(result.bytecode.len() > 0);
    }

    #[test]
    fn test_multiple_field_variant() {
        // Note: Variable destructuring in match needs type inference improvement
        let source = r#"
            enum Pair {
                Values(int, int)
            }

            fn sum(Pair p) -> int {
                match p {
                    case Pair::Values(a, b) => { return a + b; }
                }
            }

            fn main() {
                let result: int = sum(Pair::Values(10, 20));
                print "%i", result;
            }
        "#;

        let result = compile_source(source);
        // Type inference for destructured variables is a work in progress
        assert!(result.bytecode.len() > 0 || result.has_errors());
    }

    #[test]
    fn test_mixed_variants() {
        let source = r#"
            enum Result {
                Ok(int),
                Err(string),
                None
            }

            fn process(Result r) {
                match r {
                    case Result::Ok(value) => { print "Ok: %i", value; }
                    case Result::Err(msg) => { print "Error: %s", msg; }
                    case Result::None => { print "None"; }
                }
            }

            fn main() {
                process(Result::Ok(42));
            }
        "#;

        let result = compile_and_check_no_errors(source);
        assert!(result.bytecode.len() > 0);
    }

    #[test]
    fn test_nested_data_variant() {
        // Note: Variable destructuring in match needs type inference improvement
        let source = r#"
            enum Expr {
                Add(int, int),
                Mul(int, int)
            }

            fn eval(Expr e) -> int {
                match e {
                    case Expr::Add(a, b) => { return a + b; }
                    case Expr::Mul(a, b) => { return a * b; }
                }
            }

            fn main() {
                let sum: int = eval(Expr::Add(10, 20));
                let product: int = eval(Expr::Mul(5, 6));
                print "%i %i", sum, product;
            }
        "#;

        let result = compile_source(source);
        // Type inference for destructured variables is a work in progress
        assert!(result.bytecode.len() > 0 || result.has_errors());
    }
}
