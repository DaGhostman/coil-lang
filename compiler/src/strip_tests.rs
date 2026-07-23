//! Remove harness-only declarations from the AST when compiling for production.

use parser::{
    ast::{Expression, Output},
};

/// Drop top-level `test("…") { … }` blocks and `#[test]` functions.
pub fn strip_test_declarations(ast: &mut Output<'_>) {
    let Expression::Program(children) = ast.1.as_mut() else {
        return;
    };
    children.retain(|child| !is_test_top_level_decl(child));
}

fn is_test_top_level_decl(node: &Output<'_>) -> bool {
    match node.1.as_ref() {
        Expression::TestCase { .. } => true,
        Expression::Function { attrs, body, .. } => {
            body.is_some() && attrs.iter().any(|a| a.name == "test")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parser::Pratt;

    #[test]
    fn strip_removes_test_blocks_and_attributed_functions() {
        let mut ast = Pratt::default()
            .parse(
                r#"
#[test]
fn hidden() { assert(true)?; }
test("block") { assert(true)?; }
fn main() { }
"#,
            )
            .expect("parse");
        strip_test_declarations(&mut ast);
        let Expression::Program(children) = ast.1.as_ref() else {
            panic!("expected program");
        };
        assert_eq!(children.len(), 1);
        assert!(matches!(children[0].1.as_ref(), Expression::Function { name, .. } if *name == "main"));
    }
}
