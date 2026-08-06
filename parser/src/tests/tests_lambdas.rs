    use super::*;
    use ast::Expression;

    fn find_lambda<'a>(expr: &'a Expression<'a>) -> Option<&'a Expression<'a>> {
        match expr {
            Expression::Lambda { .. } => Some(expr),
            Expression::Block(children)
            | Expression::Program(children)
            | Expression::Fragment(children) => {
                children.iter().find_map(|c| find_lambda(c.1.as_ref()))
            }
            Expression::Statement(inner)
            | Expression::ExprStatement(inner)
            | Expression::Group(inner)
            | Expression::Expr(inner)
            | Expression::Return(inner)
            | Expression::ImplicitReturn(inner) => find_lambda(inner.1.as_ref()),
            Expression::Variable(_, Some(inner)) => find_lambda(inner.1.as_ref()),
            Expression::Function { body, .. } => body
                .as_ref()
                .map(|b| find_lambda(b.1.as_ref()))
                .unwrap_or(None),
            _ => None,
        }
    }

    #[test]
    fn lambda_short_form_parses_captures_and_arrow_body() {
        let ast = Pratt::default()
            .parse(
                r#"
fn main() {
    let f = fn (int x) use (y) => x + y;
}
"#,
            )
            .expect("parse failed");
        match find_lambda(ast.1.as_ref()) {
            Some(Expression::Lambda { captures, body, .. }) => {
                assert_eq!(captures, &["y"]);
                // Arrow body is an expression tree, not a Block.
                assert!(
                    !matches!(body.1.as_ref(), Expression::Block(_)),
                    "short-form `=>` body should not wrap in Block; got {}",
                    body.1
                );
            }
            other => panic!("expected Lambda, got {:?}", other),
        }
    }

    #[test]
    fn lambda_block_body_parses_without_use() {
        let ast = Pratt::default()
            .parse(
                r#"
fn main() {
    let f = fn (int x) { return x + 1; };
}
"#,
            )
            .expect("parse failed");
        match find_lambda(ast.1.as_ref()) {
            Some(Expression::Lambda { captures, body, .. }) => {
                assert!(
                    captures.is_empty(),
                    "expected no captures, got {captures:?}"
                );
                assert!(
                    matches!(body.1.as_ref(), Expression::Block(_)),
                    "brace body should be Block; got {}",
                    body.1
                );
            }
            other => panic!("expected Lambda, got {:?}", other),
        }
    }

    #[test]
    fn lambda_block_body_allows_let_and_return_statements() {
        let ast = Pratt::default()
            .parse(
                r#"
fn main() {
    let f = fn (int x) { let y = x + 1; return y; };
}
"#,
            )
            .expect("parse failed");
        match find_lambda(ast.1.as_ref()) {
            Some(Expression::Lambda { body, .. }) => match body.1.as_ref() {
                Expression::Block(children) => {
                    assert_eq!(children.len(), 2);
                    assert!(matches!(
                        children[0].1.as_ref(),
                        Expression::Statement(inner)
                            if matches!(inner.1.as_ref(), Expression::Fragment(_))
                    ));
                    assert!(matches!(
                        children[1].1.as_ref(),
                        Expression::Statement(inner)
                            if matches!(inner.1.as_ref(), Expression::Return(_))
                    ));
                }
                other => panic!("expected lambda Block, got {:?}", other),
            },
            other => panic!("expected Lambda, got {:?}", other),
        }
    }
