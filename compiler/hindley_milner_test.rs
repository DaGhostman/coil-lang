// hindley_milner_test.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unify() {
        let t1 = Type::Int;
        let t2 = Type::Int;
        assert!(unify(&t1, &t2).is_ok());
    }

    #[test]
    fn test_infer_expr() {
        let mut env = TypeEnv::new();
        let expr = Expr::Lit(42);
        let result = infer_expr(&expr, &mut env);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), Type::Int));
    }

    // Add more tests as needed
}