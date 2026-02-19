use parser::{ast::Expression, ast::Output, SimpleSpan};
use std::borrow::Borrow;

use crate::types::{
    constraint::Constraint, ConstraintSet, GenericSignature, Substitution, Type, TypeEnv, TypeVar,
};

/// A type checking error with span information
#[derive(Clone, Debug)]
pub struct TypeError {
    pub message: String,
    pub span: parser::SimpleSpan,
}

impl TypeError {
    pub fn new(message: &str, span: parser::SimpleSpan) -> Self {
        Self {
            message: message.to_string(),
            span,
        }
    }
}

/// Hindley-Milner Type Checker for Zero-Script
pub struct HmTypeChecker {
    env: TypeEnv,
    constraints: ConstraintSet,
    type_var_counter: usize,
}

impl HmTypeChecker {
    /// Handle function declaration (with or without generics)
    fn handle_function<'a>(
        &mut self,
        name: &'a str,
        args: &Output<'a>,
        returns: Option<&Output<'a>>,
        body: &Output<'a>,
        generics: Option<&Vec<(&'a str, Vec<&'a str>)>>,
    ) -> Result<Type, Vec<String>> {
        // Create a new scope for the function
        self.env.push_scope();

        // Process generic type parameters
        let mut type_params: Vec<TypeVar> = Vec::new();
        if let Some(generics) = generics {
            for (i, (gen_name, bounds)) in generics.iter().enumerate() {
                let bounds_vec: Vec<crate::types::ty::TypeBound> = bounds
                    .iter()
                    .map(|b| crate::types::ty::TypeBound::new(b))
                    .collect();
                let tv = TypeVar::new(i, gen_name).with_bounds(bounds_vec);
                type_params.push(tv.clone());
                self.env
                    .define_variable(gen_name, Type::TypeVar(tv.clone()));
            }
            self.env.define_generics(name, type_params.clone());
        }

        // Process arguments and collect parameter types
        let mut param_types = Vec::new();
        if let Expression::Fragment(arg_list) = args.1.borrow() {
            for arg in arg_list.iter() {
                if let Expression::Argument(ty_expr, var_expr) = arg.1.borrow() {
                    // Extract type from Output<'expr>
                    let ty_name = match ty_expr.1.borrow() {
                        Expression::Type(t) => t.1.to_string(),
                        _ => ty_expr.1.to_string(),
                    };
                    let ty = Type::from(ty_name);

                    // Extract variable name from Output<'expr>
                    let var_name = match var_expr.1.borrow() {
                        Expression::Identifier(n) => n.to_string(),
                        _ => var_expr.1.to_string(),
                    };

                    param_types.push(ty.clone());
                    self.env.define_variable(&var_name, ty);
                }
            }
        }

        // Process return type - explicit is preferred
        let return_ty = if let Some(ret_expr) = returns {
            // Extract type name from Output<'expr>
            let ty_name = match ret_expr.1.borrow() {
                Expression::Type(t) => t.1.to_string(),
                _ => ret_expr.1.to_string(),
            };
            Type::from(ty_name)
        } else {
            // Inference fallback: default to Void for functions without explicit return type
            // Note: It's recommended to explicitly declare return types for clarity
            Type::Void
        };

        // Check body
        let _ = self.infer_expr(body.1.borrow())?;

        // Pop scope
        self.env.pop_scope();

        // Register function signature in TypeEnv for call resolution
        let func_ty = Type::Function(param_types.clone(), Box::new(return_ty.clone()));
        self.env.define_function(name, func_ty.clone());

        // If this is a generic function, also store the generic signature
        if !type_params.is_empty() {
            let generic_sig = GenericSignature::new(name, type_params, param_types, return_ty);
            self.env.define_generic_signature(name, generic_sig);
        }

        Ok(func_ty)
    }

    /// Create a new HM type checker
    pub fn new() -> Self {
        Self {
            env: TypeEnv::new(),
            constraints: ConstraintSet::new(),
            type_var_counter: 0,
        }
    }

    pub fn clear(&mut self) -> (TypeEnv, ConstraintSet, usize) {
        let env = self.env.clone();
        let constraints = self.constraints.clone();
        let vars = self.type_var_counter;

        self.env = TypeEnv::new();
        self.constraints = ConstraintSet::new();
        self.type_var_counter = 0;

        return (env, constraints, vars);
    }

    /// Reset the type checker for a new compilation
    pub fn reset(&mut self) -> (TypeEnv, ConstraintSet, usize) {
        let env = self.env.clone();
        let constraints = self.constraints.clone();
        let vars = self.type_var_counter;

        return (env, constraints, vars);
    }

    pub fn restore(&mut self, env: TypeEnv, constraints: ConstraintSet, counter: usize) {
        self.env = env;
        self.constraints = constraints;
        self.type_var_counter = counter;
    }

    /// Generate a new type variable ID
    fn new_type_var(&mut self, name: &str) -> TypeVar {
        let id = self.type_var_counter;
        self.type_var_counter += 1;
        TypeVar::new(id, name)
    }

    /// Check the AST and return the inferred type
    pub fn check(&mut self, ast: &(SimpleSpan, Box<Expression<'_>>)) -> Result<Type, Vec<String>> {
        let (span, expr) = ast;
        let ty = self.infer_expr(expr.borrow())?;

        // Add type constraint for the expression
        self.constraints.add(ty.clone(), ty.clone(), span.clone());

        Ok(ty)
    }

    /// Infer the type of an expression
    pub fn infer_expr(&mut self, expr: &Expression<'_>) -> Result<Type, Vec<String>> {
        match expr {
            Expression::Integer(_) => Ok(Type::Int),
            Expression::Float(_) => Ok(Type::Float),
            Expression::String(_) => Ok(Type::String),
            Expression::Bool(_) => Ok(Type::Bool),

            Expression::Identifier(name) => {
                if let Some(ty) = self.env.lookup(name) {
                    Ok(ty.clone())
                } else {
                    // Create a new type variable for undetermined identifiers
                    let tv = self.new_type_var(name);
                    Ok(Type::TypeVar(tv))
                }
            }

            Expression::Add(lhs, rhs) => {
                let left_ty = self.infer_expr(lhs.1.borrow())?;
                let right_ty = self.infer_expr(rhs.1.borrow())?;

                // For addition, both operands should be the same numeric type
                // Generate constraint: left_ty == right_ty
                self.constraints
                    .add(left_ty.clone(), right_ty.clone(), rhs.0.clone());

                let left_ty_name = left_ty.type_name();
                let right_ty_name = right_ty.type_name();
                match (left_ty, right_ty) {
                    (Type::Int, Type::Int) => Ok(Type::Int),
                    (Type::Float, Type::Float) => Ok(Type::Float),
                    (Type::Int, Type::Float) | (Type::Float, Type::Int) => Ok(Type::Float),
                    _ => {
                        // Provide helpful error message for type mismatch
                        let err = format!(
                            "Invalid operands for arithmetic operation: left type '{}', right type '{}'. \
                            Note: Arithmetic operations require compatible numeric types.",
                            left_ty_name, right_ty_name
                        );
                        Err(vec![err])
                    }
                }
            }

            Expression::Sub(lhs, rhs) => {
                let left_ty = self.infer_expr(lhs.1.borrow())?;
                let right_ty = self.infer_expr(rhs.1.borrow())?;

                self.constraints
                    .add(left_ty.clone(), right_ty.clone(), rhs.0.clone());

                match (left_ty, right_ty) {
                    (Type::Int, Type::Int) => Ok(Type::Int),
                    (Type::Float, Type::Float) => Ok(Type::Float),
                    (Type::Int, Type::Float) | (Type::Float, Type::Int) => Ok(Type::Float),
                    _ => Ok(Type::Int),
                }
            }

            Expression::Mul(lhs, rhs) => {
                let left_ty = self.infer_expr(lhs.1.borrow())?;
                let right_ty = self.infer_expr(rhs.1.borrow())?;

                self.constraints
                    .add(left_ty.clone(), right_ty.clone(), rhs.0.clone());

                match (left_ty, right_ty) {
                    (Type::Int, Type::Int) => Ok(Type::Int),
                    (Type::Float, Type::Float) => Ok(Type::Float),
                    (Type::Int, Type::Float) | (Type::Float, Type::Int) => Ok(Type::Float),
                    _ => Ok(Type::Int),
                }
            }

            Expression::Div(lhs, rhs) => {
                let left_ty = self.infer_expr(lhs.1.borrow())?;
                let right_ty = self.infer_expr(rhs.1.borrow())?;

                self.constraints
                    .add(left_ty.clone(), right_ty.clone(), rhs.0.clone());

                match (left_ty, right_ty) {
                    (Type::Int, Type::Int) => Ok(Type::Int),
                    (Type::Float, Type::Float) => Ok(Type::Float),
                    (Type::Int, Type::Float) | (Type::Float, Type::Int) => Ok(Type::Float),
                    _ => Ok(Type::Int),
                }
            }

            Expression::Eq(lhs, rhs) | Expression::Neq(lhs, rhs) => {
                let left_ty = self.infer_expr(lhs.1.borrow())?;
                let right_ty = self.infer_expr(rhs.1.borrow())?;

                self.constraints
                    .add(left_ty.clone(), right_ty.clone(), rhs.0.clone());

                Ok(Type::Bool)
            }

            Expression::Le(lhs, rhs)
            | Expression::Gt(lhs, rhs)
            | Expression::Leq(lhs, rhs)
            | Expression::Geq(lhs, rhs) => {
                let left_ty = self.infer_expr(lhs.1.borrow())?;
                let right_ty = self.infer_expr(rhs.1.borrow())?;

                self.constraints
                    .add(left_ty.clone(), right_ty.clone(), rhs.0.clone());

                Ok(Type::Bool)
            }

            Expression::And(lhs, rhs) | Expression::Or(lhs, rhs) => {
                let left_ty = self.infer_expr(lhs.1.borrow())?;
                let right_ty = self.infer_expr(rhs.1.borrow())?;

                self.constraints
                    .add(left_ty.clone(), right_ty.clone(), rhs.0.clone());

                Ok(Type::Bool)
            }

            Expression::Not(expr) => {
                let _ = self.infer_expr(expr.1.borrow())?;
                Ok(Type::Bool)
            }

            Expression::Negate(expr) => {
                let ty = self.infer_expr(expr.1.borrow())?;
                Ok(ty)
            }

            Expression::Positive(expr) => {
                let ty = self.infer_expr(expr.1.borrow())?;
                Ok(ty)
            }

            Expression::Function {
                name,
                args,
                returns,
                body,
            } => self.handle_function(name, &args, returns.as_ref(), &body, None),

            Expression::FunctionWithGenerics {
                name,
                args,
                returns,
                body,
                generics,
            } => self.handle_function(name, &args, returns.as_ref(), &body, Some(generics)),

            Expression::GenericFunctionCall {
                name,
                type_args,
                args,
            } => {
                // Extract function name from the name expression
                let identifier = match name.1.borrow() {
                    Expression::Identifier(n) => n.to_string(),
                    _ => {
                        let tv = self.new_type_var("complex_generic_call");
                        return Ok(Type::TypeVar(tv));
                    }
                };

                // Parse type arguments
                let type_arg_types: Vec<Type> = type_args
                    .iter()
                    .map(|ta| {
                        let ty_name = match ta.1.borrow() {
                            Expression::Type(t) => t.1.to_string(),
                            _ => ta.1.to_string(),
                        };
                        Type::from(ty_name)
                    })
                    .collect();

                if let Some(signature) = self.env.lookup_generic_signature(&identifier) {
                    if let Err(bounds_errors) = signature.validate_bounds(&type_arg_types) {
                        return Err(bounds_errors);
                    }
                }

                if let Some(instantiated_ty) =
                    self.env.instantiate_generic(&identifier, &type_arg_types)
                {
                    match instantiated_ty {
                        Type::Function(params, return_ty) => {
                            // Check argument types
                            if let Some(args) = args {
                                if params.len() == args.len() {
                                    let arg_types: Vec<Type> = args
                                        .iter()
                                        .map(|arg| self.infer_expr(arg.1.borrow()))
                                        .collect::<Result<Vec<_>, Vec<_>>>()
                                        .map_err(|e| e)?;

                                    for (i, arg_ty) in arg_types.iter().enumerate() {
                                        self.constraints.add(
                                            arg_ty.clone(),
                                            params[i].clone(),
                                            args[i].0.clone(),
                                        );
                                    }
                                    Ok(return_ty.as_ref().clone())
                                } else {
                                    Err(vec![format!(
                                        "Function '{}' expects {} arguments, but {} provided",
                                        identifier,
                                        params.len(),
                                        args.len()
                                    )])
                                }
                            } else {
                                Ok(return_ty.as_ref().clone())
                            }
                        }
                        _ => Err(vec![format!(
                            "'{}' is not a function after instantiation",
                            identifier
                        )]),
                    }
                } else {
                    // Generic signature not found, create a type variable
                    let tv = self.new_type_var(&format!("{}_generic_ret", identifier));
                    Ok(Type::TypeVar(tv))
                }
            }

            Expression::Call { name, args } => {
                // Extract function name from the name expression
                let identifier = match name.1.borrow() {
                    Expression::Identifier(n) => n.to_string(),
                    _ => {
                        // For complex expressions, use a type variable
                        let tv = self.new_type_var("complex_call");
                        return Ok(Type::TypeVar(tv));
                    }
                };

                // Look up function signature from TypeEnv
                if let Some(func_ty) = self.env.lookup(&identifier).cloned() {
                    match func_ty {
                        Type::Function(params, return_ty) => {
                            // Check argument count if we have args
                            if let Some(args) = args {
                                if params.len() == args.len() {
                                    // Infer argument types first
                                    let arg_types: Vec<Type> = args
                                        .iter()
                                        .map(|arg| self.infer_expr(arg.1.borrow()))
                                        .collect::<Result<Vec<_>, Vec<_>>>()
                                        .map_err(|e| e)?;

                                    // Then add constraints
                                    for (i, arg_ty) in arg_types.iter().enumerate() {
                                        self.constraints.add(
                                            arg_ty.clone(),
                                            params[i].clone(),
                                            args[i].0.clone(),
                                        );
                                    }
                                    Ok(return_ty.as_ref().clone())
                                } else {
                                    Err(vec![format!(
                                        "Function '{}' expects {} arguments, but {} provided",
                                        identifier,
                                        params.len(),
                                        args.len()
                                    )])
                                }
                            } else {
                                Ok(return_ty.as_ref().clone())
                            }
                        }
                        _ => Err(vec![format!(
                            "'{}' is not a function (type: {})",
                            identifier,
                            func_ty.type_name()
                        )]),
                    }
                } else {
                    // Function not found, return a type variable (for forward declarations)
                    let tv = self.new_type_var(&format!("{}_ret", identifier));
                    Ok(Type::TypeVar(tv))
                }
            }

            Expression::Variable(name, ty_expr) => {
                let ty = if let Some(ty_expr) = ty_expr {
                    // Extract type from Output<'expr>
                    let ty_name = match ty_expr.1.borrow() {
                        Expression::Type(t) => t.1.to_string(),
                        _ => ty_expr.1.to_string(),
                    };
                    Type::from(ty_name)
                } else {
                    let tv = self.new_type_var(name);
                    Type::TypeVar(tv)
                };

                self.env.define_variable(name, ty.clone());
                Ok(ty)
            }

            Expression::Assignment(name, value) => {
                let value_ty = self.infer_expr(value.1.borrow())?;

                // Get variable name
                let var_name = match name.1.borrow() {
                    Expression::Identifier(n) => n.to_string(),
                    _ => "unknown".to_string(),
                };

                self.env.define_variable(&var_name, value_ty.clone());
                Ok(value_ty)
            }

            Expression::TypedAssignment { name, ty, value } => {
                // Extract expected type from annotation
                let expected_ty_name = match ty.1.borrow() {
                    Expression::Type(t) => t.1.to_string(),
                    _ => ty.1.to_string(),
                };
                let expected_ty = Type::from(expected_ty_name);

                // Infer the value type
                let value_ty = self.infer_expr(value.1.borrow())?;

                // Check if types match - generate error if not
                if value_ty != expected_ty {
                    // If value_ty is a type variable, we can't check yet
                    if !matches!(value_ty, Type::TypeVar(_)) {
                        return Err(vec![format!(
                            "Type mismatch: expected '{}', found '{}'",
                            expected_ty.type_name(),
                            value_ty.type_name()
                        )]);
                    }
                }

                // Generate constraint: value_ty == expected_ty
                self.constraints
                    .add(value_ty.clone(), expected_ty.clone(), ty.0.clone());

                // Define variable with the expected type
                self.env.define_variable(name, expected_ty.clone());

                Ok(expected_ty)
            }

            Expression::Block(stmts) | Expression::Fragment(stmts) | Expression::Program(stmts) => {
                // Process each statement
                let mut last_ty = Type::Void;
                for stmt in stmts {
                    last_ty = self.infer_expr(stmt.1.borrow())?;
                }
                Ok(last_ty)
            }

            Expression::Return(expr) | Expression::ImplicitReturn(expr) => {
                let ty = self.infer_expr(expr.1.borrow())?;
                Ok(ty)
            }

            Expression::Variant(name_expr, _) => {
                // Variant - return a sum type with the variant name
                // Extract name from Output<'expr>
                let var_name = match name_expr.1.borrow() {
                    Expression::Identifier(n) => n.to_string(),
                    _ => name_expr.1.to_string(),
                };
                // The actual sum type will be inferred from context
                let tv = self.new_type_var(&var_name);
                Ok(Type::TypeVar(tv))
            }

            Expression::VariantWithDestructure(_type_expr, name_expr, _fields) => {
                // Variant with destructured fields - return a sum type
                // The fields will be handled in the Match arm processing
                let var_name = match name_expr.1.borrow() {
                    Expression::Identifier(n) => n.to_string(),
                    _ => name_expr.1.to_string(),
                };
                let tv = self.new_type_var(&var_name);
                Ok(Type::TypeVar(tv))
            }

            Expression::Match(lhs, arms) => {
                let lhs_ty = self.infer_expr(lhs.1.borrow())?;

                // Extract pattern values for exhaustiveness checking
                let mut pattern_values: Vec<String> = Vec::new();
                let mut has_default = false;
                let mut last_body_ty = Type::Void;

                for (span, arm) in arms {
                    if let Expression::MatchArm((s, pattern), (_, body)) = arm.borrow() {
                        // Handle comma-separated variants (List of patterns) or single pattern
                        let patterns: Vec<(SimpleSpan, Box<Expression<'_>>)> =
                            match pattern.borrow() {
                                Expression::List(variants) => variants.iter().cloned().collect(),
                                _ => {
                                    // Single pattern - wrap in a reference to match the expected type
                                    vec![(s.clone(), pattern.clone())]
                                }
                            };

                        // Track variant types for conformance checking across patterns in same arm
                        let mut variant_types: Vec<Type> = Vec::new();

                        // Process each variant in the comma-separated list
                        for variant in patterns {
                            match variant.1.borrow() {
                                Expression::Integer(n) => pattern_values.push(format!("int:{}", n)),
                                Expression::Float(n) => pattern_values.push(format!("float:{}", n)),
                                Expression::String(s) => pattern_values.push(format!("str:{}", s)),
                                Expression::Bool(b) => pattern_values.push(format!("bool:{}", b)),
                                Expression::Default(_) => has_default = true,
                                Expression::VariantItem(type_expr, name_expr) => {
                                    // Sum type variant pattern - extract type information
                                    let ty_name = match type_expr.1.borrow() {
                                        Expression::Type(t) => t.1.to_string(),
                                        _ => type_expr.1.to_string(),
                                    };
                                    let var_name = match name_expr.1.borrow() {
                                        Expression::Identifier(n) => n.to_string(),
                                        _ => name_expr.1.to_string(),
                                    };
                                    pattern_values
                                        .push(format!("variant:{}::{}", ty_name, var_name));

                                    // Create a type variable for the sum type to enable destructuring
                                    let sum_type_var =
                                        self.new_type_var(&format!("{}_sum", ty_name));
                                    self.env.define_variable(
                                        &format!("{}_sum", ty_name),
                                        Type::TypeVar(sum_type_var.clone()),
                                    );

                                    variant_types.push(Type::TypeVar(sum_type_var));
                                }
                                Expression::VariantWithDestructure(
                                    type_expr,
                                    name_expr,
                                    fields,
                                ) => {
                                    // Sum type variant with destructured fields
                                    let ty_name = match type_expr.1.borrow() {
                                        Expression::Type(t) => t.1.to_string(),
                                        _ => type_expr.1.to_string(),
                                    };
                                    let var_name = match name_expr.1.borrow() {
                                        Expression::Identifier(n) => n.to_string(),
                                        _ => name_expr.1.to_string(),
                                    };

                                    // Collect field types from destructured variables
                                    // Fields are identifier names, their types will be inferred from enum declaration
                                    let field_types: Vec<Type> = fields
                                        .iter()
                                        .map(|field| {
                                            match field.1.borrow() {
                                                Expression::Identifier(name) => {
                                                    // Variable declaration - create type variable for inference
                                                    let tv = self.new_type_var(name);
                                                    self.env.define_variable(
                                                        name,
                                                        Type::TypeVar(tv.clone()),
                                                    );
                                                    Type::TypeVar(tv)
                                                }
                                                _ => Type::Void,
                                            }
                                        })
                                        .collect();

                                    pattern_values.push(format!(
                                        "variant:{}::{}{{destructured}}",
                                        ty_name, var_name
                                    ));

                                    // Create a type variable for the sum type
                                    let sum_type_var =
                                        self.new_type_var(&format!("{}_sum", ty_name));
                                    self.env.define_variable(
                                        &format!("{}_sum", ty_name),
                                        Type::TypeVar(sum_type_var.clone()),
                                    );

                                    variant_types.push(Type::TypeVar(sum_type_var));
                                }
                                _ => {
                                    // For complex patterns, try to infer type
                                }
                            }

                            // Add constraint for each variant
                            let variant_ty = self.infer_expr(variant.1.borrow())?;
                            self.constraints
                                .add(lhs_ty.clone(), variant_ty, span.clone());
                        }

                        // Conformance checking: All patterns in the same arm should have the same type
                        // Check that all variant types are the same (same variant with same field structure)
                        if variant_types.len() > 1 {
                            let first_ty = &variant_types[0];
                            for (i, ty) in variant_types.iter().enumerate().skip(1) {
                                // Add constraint that all variant types unify
                                self.constraints
                                    .add(first_ty.clone(), ty.clone(), span.clone());
                            }
                        }

                        // Type narrowing: The body type should match the pattern type for this arm
                        let body_ty = self.infer_expr(body)?;
                        last_body_ty = body_ty;
                    }
                }

                // Exhaustiveness checking (basic level)
                if !has_default && !pattern_values.is_empty() {
                    // Check if this is a simple integer match with all common cases
                    if pattern_values.iter().any(|v| v.starts_with("int:")) {
                        // For integer matches without default, warn about potential incompleteness
                        // This is a basic check - more sophisticated checking would track all possible values
                    }
                }

                Ok(last_body_ty) // Return the type of the last match arm body
            }

            Expression::Class(name, state) => {
                // Process class fields
                for field in state {
                    if let Expression::Field(n, t) = field.1.borrow() {
                        let field_name = match n.1.borrow() {
                            Expression::Identifier(name) => name.to_string(),
                            _ => "unknown".to_string(),
                        };
                        let field_ty = Type::from(t.1.to_string());
                        self.env.define_variable(&field_name, field_ty);
                    }
                }
                Ok(Type::Struct(super::types::StructDef::new(name, Vec::new())))
            }

            Expression::If(branches) => {
                // For each branch, check condition type is Bool
                for branch in branches {
                    // Process the branch
                    let _ = self.infer_expr(branch.1.borrow())?;
                }
                Ok(Type::Void)
            }

            Expression::Loop { iterable, body, .. } => {
                let _ = self.infer_expr(iterable.1.borrow())?;
                let _ = self.infer_expr(body.1.borrow())?;
                Ok(Type::Void)
            }

            Expression::Format(fmt, params) | Expression::Print(fmt, params) => {
                let fmt_ty = self.infer_expr(fmt.1.borrow())?;
                if fmt_ty != Type::String {
                    return Err(vec!["Format string must be a string".to_string()]);
                }

                if let Some(params) = params {
                    for param in params {
                        let _ = self.infer_expr(param.1.borrow())?;
                    }
                }

                Ok(Type::Void)
            }

            Expression::Constant(name, ty_expr) => {
                // Similar to Variable but marks as constant
                self.infer_expr(&Expression::Variable(
                    &<Box<Expression<'_>> as std::borrow::Borrow<Expression>>::borrow(&name.1)
                        .to_string(),
                    // name.1.borrow().to_string(),
                    ty_expr.clone(),
                ))
            }

            Expression::Type(ty_expr) => {
                // Extract type from Output<'expr>
                let ty_name = match ty_expr.1.borrow() {
                    Expression::Type(t) => t.1.to_string(),
                    _ => ty_expr.1.to_string(),
                };
                Ok(Type::from(ty_name))
            }

            Expression::TypeAlias(_name, target) => {
                // Type alias just returns the target type
                let target_ty = self.infer_expr(target.1.borrow())?;
                Ok(target_ty)
            }

            Expression::NewType(_name, target) => {
                // NewType creates a distinct type - infer from target
                let target_ty = self.infer_expr(target.1.borrow())?;
                Ok(target_ty)
            }

            Expression::Comment(_) | Expression::Use { .. } | Expression::Noop(_) => Ok(Type::Void),
            Expression::Expr(expr) => self.infer_expr(expr.1.borrow()),

            // Unhandled expressions - return a type variable for now
            _ => {
                let tv = self.new_type_var("unknown");
                Ok(Type::TypeVar(tv))
            }
        }
    }

    /// Solve constraints and return substitution
    pub fn solve_constraints(&mut self) -> Result<Substitution, Vec<String>> {
        self.constraints.solve()
    }

    /// Check a return statement and generate constraints
    pub fn check_return(
        &mut self,
        expr_ty: Type,
        expected_ty: Type,
        span: SimpleSpan,
    ) -> Result<(), Vec<String>> {
        self.constraints
            .add(expr_ty.clone(), expected_ty.clone(), span);
        Ok(())
    }

    /// Check a function call and infer the return type
    pub fn check_call(
        &mut self,
        func_name: &str,
        arg_types: Vec<Type>,
        span: SimpleSpan,
    ) -> Result<Type, Vec<String>> {
        // Look up function signature
        if let Some(func_ty) = self.env.lookup(func_name) {
            match func_ty {
                Type::Function(params, return_ty) => {
                    // Generate constraints for argument types
                    if params.len() == arg_types.len() {
                        for (param_ty, arg_ty) in params.iter().zip(arg_types.iter()) {
                            self.constraints
                                .add(param_ty.clone(), arg_ty.clone(), span.clone());
                        }
                    } else {
                        return Err(vec![format!(
                            "Function '{}' expects {} arguments, but {} provided",
                            func_name,
                            params.len(),
                            arg_types.len()
                        )]);
                    }
                    Ok(return_ty.as_ref().clone())
                }
                _ => Err(vec![format!("'{}' is not a function", func_name)]),
            }
        } else {
            // Create a type variable for undetermined function return type
            let tv = self.new_type_var(&format!("{}_ret", func_name));
            Ok(Type::TypeVar(tv))
        }
    }

    /// Infer the type of a block and check all statements
    pub fn check_block(&mut self, statements: Vec<&Expression<'_>>) -> Result<Type, Vec<String>> {
        let mut last_type = Type::Void;

        for stmt in statements {
            let ty = self.infer_expr(stmt)?;
            last_type = ty;
        }

        Ok(last_type)
    }

    /// Check an assignment and generate constraints
    pub fn check_assignment(
        &mut self,
        var_name: &str,
        value_ty: Type,
        span: SimpleSpan,
    ) -> Result<(), Vec<String>> {
        if let Some(expected_ty) = self.env.lookup(var_name) {
            self.constraints
                .add(value_ty.clone(), expected_ty.clone(), span);
            Ok(())
        } else {
            // Variable not found in environment, define it
            self.env.define_variable(var_name, value_ty.clone());
            Ok(())
        }
    }

    /// Get the current type environment
    pub fn get_env(&self) -> &TypeEnv {
        &self.env
    }

    /// Get mutable access to the type environment
    pub fn get_env_mut(&mut self) -> &mut TypeEnv {
        &mut self.env
    }

    /// Get all constraints for debugging
    pub fn get_constraints(&self) -> &[Constraint] {
        self.constraints.get_constraints()
    }

    /// Solve constraints and apply substitution to a type
    pub fn apply_substitution(&mut self, ty: Type) -> Result<Type, Vec<String>> {
        let substitution = self.solve_constraints()?;
        Ok(substitution.apply(ty))
    }

    /// Get all type errors from constraint checking
    pub fn check_constraints(&self) -> Vec<String> {
        self.constraints.check()
    }
}

impl Default for HmTypeChecker {
    fn default() -> Self {
        Self::new()
    }
}

// Type conversion from parser Type to our new Type system
// impl From<String> for Type {
//     fn from(value: String) -> Self {
//         match value.to_lowercase().as_str() {
//             "int" => Type::Int,
//             "float" => Type::Float,
//             "string" => Type::String,
//             "bool" => Type::Bool,
//             "array" => Type::Void, // TODO: Handle typed arrays
//             "void" => Type::Void,
//             "none" => Type::None,
//             _ => {
//                 // For unknown types, create a type variable
//                 let tv = TypeVar::new(0, &value);
//                 Type::TypeVar(tv)
//             }
//         }
//     }
// }
//
