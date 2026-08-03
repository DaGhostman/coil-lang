    use super::*;

    #[test]
    fn parse_classes_example() {
        let src = include_str!("../../../examples/classes.hy");
        let p = Pratt::default();
        p.parse(src).unwrap_or_else(|e| panic!("PARSE FAIL: {e:?}"));
    }

    /// `impl` methods are space/newline-separated (no commas between methods).
    #[test]
    fn parse_impl_methods_without_commas() {
        let src = r#"
class Point { x: int, y: int, }
impl Point {
    fn sum() -> int { return self.x + self.y; }
    fn set_x(int n) { self.x = n; }
}
fn main() { let p = new Point(1, 2); }
"#;
        let p = Pratt::default();
        p.parse(src)
            .unwrap_or_else(|e| panic!("expected space-separated impl methods: {e:?}"));
    }
