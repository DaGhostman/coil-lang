// hindley_milner.rs

// Define type variables and type constructors
pub enum Type {
    Var(String),
    Int,
    Bool,
    Func(Box<Type>, Box<Type>),
    // Add other type constructors as needed
}

// Type environment mapping variables to types
pub type TypeEnv = std::collections::HashMap<String, Type>;

// Trait for type inference
pub trait TypeInference {
    fn infer_type(&self, env: &mut TypeEnv) -> Result<Type, String>;
}

// Unification of types
pub fn unify(t1: &Type, t2: &Type) -> Result<(), String> {
    match (t1, t2) {
        (Type::Var(a), Type::Var(b)) if a == b => Ok(()),
        (Type::Var(a), t) => {
            // Check if `a` is already bound to a type
            // If not, bind it to `t`
            Ok(())
        },
        (Type::Int, Type::Int) => Ok(()),
        (Type::Bool, Type::Bool) => Ok(()),
        (Type::Func(l1, r1), Type::Func(l2, r2)) => {
            unify(l1, l2)?;
            unify(r1, r2)
        },
        _ => Err(format!("Type mismatch: {:?} vs {:?}", t1, t2)),
    }
}

// Main type inference function
pub fn infer_expr(expr: &Expr, env: &mut TypeEnv) -> Result<Type, String> {
    match expr {
        Expr::Lit(_) => Ok(Type::Int),
        Expr::Bool(_) => Ok(Type::Bool),
        Expr::Var(x) => {
            // Look up the type of `x` in the environment
            env.get(x).cloned().ok_or_else(|| format!("Variable not found: {}", x))
        },
        Expr::App(f, arg) => {
            let f_type = infer_expr(f, env)?;
            let arg_type = infer_expr(arg, env)?;
            match f_type {
                Type::Func(l, r) => {
                    unify(&l, &arg_type)?;
                    Ok(*r)
                },
                _ => Err("Function type expected".to_string()),
            }
        },
        Expr::Lam(param, body) => {
            // Generate a fresh type variable for the parameter
            let param_type = Type::Var(param.clone());
            // Extend the environment with the parameter type
            env.insert(param.clone(), param_type.clone());
            let body_type = infer_expr(body, env)?;
            Ok(Type::Func(Box::new(param_type), Box::new(body_type)))
        },
    }
}

// Define a simple expression type for demonstration
pub enum Expr {
    Var(String),
    Lit(i32),
    Bool(bool),
    App(Box<Expr>, Box<Expr>),
    Lam(String, Box<Expr>),
    // Add other expression forms as needed
}
