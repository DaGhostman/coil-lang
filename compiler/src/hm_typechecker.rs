use std::collections::HashMap;

use parser::{SimpleSpan, ast::Expression};

use crate::types::{
    Type, TypeVar, TypeEnv, ConstraintSet, Substitution, unify::unify_types,
};

/// Hindley-Milner Type Checker for Zero-Script
pub struct HmTypeChecker {
    env: TypeEnv,
    constraints: ConstraintSet,
    type_var_counter: usize,
}

impl HmTypeChecker {
    /// Create a new HM type checker
    pub fn new() -> Self {
        Self {
            env: TypeEnv::new(),
            constraints: ConstraintSet::new(),
            type_var_counter: 0,
        }
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
        self.constraints.add(ty, ty.clone(), span.clone());

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
                self.constraints.add(left_ty.clone(), right_ty.clone(), rhs.0.clone());

                match (left_ty, right_ty) {
                    (Type::Int, Type::Int) => Ok(Type::Int),
                    (Type::Float, Type::Float) => Ok(Type::Float),
                    (Type::Int, Type::Float) | (Type::Float, Type::Int) => Ok(Type::Float),
                    _ => Ok(Type::Int), // Fallback for now
                }
            }

            Expression::Sub(lhs, rhs) => {
                let left_ty = self.infer_expr(lhs.1.borrow())?;
                let right_ty = self.infer_expr(rhs.1.borrow())?;

                self.constraints.add(left_ty.clone(), right_ty.clone(), rhs.0.clone());

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

                self.constraints.add(left_ty.clone(), right_ty.clone(), rhs.0.clone());

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

                self.constraints.add(left_ty.clone(), right_ty.clone(), rhs.0.clone());

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

                self.constraints.add(left_ty.clone(), right_ty.clone(), rhs.0.clone());

                Ok(Type::Bool)
            }

            Expression::Le(lhs, rhs)
            | Expression::Gt(lhs, rhs)
            | Expression::Leq(lhs, rhs)
            | Expression::Geq(lhs, rhs) => {
                let left_ty = self.infer_expr(lhs.1.borrow())?;
                let right_ty = self.infer_expr(rhs.1.borrow())?;

                self.constraints.add(left_ty.clone(), right_ty.clone(), rhs.0.clone());

                Ok(Type::Bool)
            }

            Expression::And(lhs, rhs) | Expression::Or(lhs, rhs) => {
                let left_ty = self.infer_expr(lhs.1.borrow())?;
                let right_ty = self.infer_expr(rhs.1.borrow())?;

                self.constraints.add(left_ty.clone(), right_ty.clone(), rhs.0.clone());

                Ok(Type::Bool)
            }

            Expression::Not(expr) => {
                let ty = self.infer_expr(expr.1.borrow())?;
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
            } => {
                // Create a new scope for the function
                self.env.push_scope();

                // Process arguments
                if let Expression::Fragment(arg_list) = args.1.borrow() {
                    for arg in arg_list.iter() {
                        if let Expression::Argument(ty_name, var_name) = arg.1.borrow() {
                            let ty = Type::from(ty_name.to_string());
                            self.env.define_variable(var_name, ty.clone());
                        }
                    }
                }

                // Process return type
                let return_ty = if let Some(ret) = returns {
                    Type::from(ret.to_string())
                } else {
                    Type::Void
                };

                // Check body
                let body_ty = self.infer_expr(body.1.borrow())?;

                // Pop scope
                self.env.pop_scope();

                // Create function type
                Ok(Type::Function(Vec::new(), return_ty))
            }

            Expression::Call { name, args } => {
                // For now, create a type variable for the call result
                let tv = self.new_type_var("call_result");
                Ok(Type::TypeVar(tv))
            }

            Expression::Variable(name, ty_expr) => {
                let ty = if let Some(ty_expr) = ty_expr {
                    let expr_ty = self.infer_expr(ty_expr.1.borrow())?;
                    Type::from(expr_ty.type_name())
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

            Expression::Match(lhs, arms) => {
                let lhs_ty = self.infer_expr(lhs.1.borrow())?;

                // For each arm, check the pattern type matches lhs type
                for (pattern, body) in arms {
                    let pattern_ty = self.infer_expr(pattern.1.borrow())?;
                    self.constraints.add(lhs_ty.clone(), pattern_ty, pattern.0.clone());

                    // Infer body type
                    let _ = self.infer_expr(body.1.borrow())?;
                }

                Ok(lhs_ty) // Return the type of the match expression
            }

            Expression::Class(name, state) => {
                // Process class fields
                for field in state {
                    if let Expression::Field(n, t) = field.1.borrow() {
                        let field_name = match n.1.borrow() {
                            Expression::Identifier(name) => name.to_string(),
                            _ => "unknown".to_string(),
                        };
                        let field_ty = Type::from(t.1.borrow().to_string());
                        self.env.define_variable(&field_name, field_ty);
                    }
                }
                Ok(Type::Struct(super::types::StructDef::new(name, Vec::new(), name.into())))
            }

            Expression::If(branches) => {
                // For each branch, check condition type is Bool
                for branch in branches {
                    // Process the branch
                    let _ = self.infer_expr(branch.1.borrow())?;
                }
                Ok(Type::Void)
            }

            Expression::Loop { iterable, body } => {
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
                self.infer_expr(&(name.0.clone(), Box::new(Expression::Variable(name.1.borrow().to_string(), ty_expr.clone()))))
            }

            Expression::Type(ty_name) => {
                Ok(Type::from(ty_name.to_string()))
            }

            Expression::Comment(_) | Expression::Use { .. } | Expression::Noop(_) | Expression::Expr(_) => {
                Ok(Type::Void)
            }

            // Unhandled expressions - return a type variable for now
            _ => {
                let tv = self.new_type_var("unknown");
                Ok(Type::TypeVar(tv))
            }
        }
    }

    /// Solve constraints and return substitution
    pub fn solve_constraints(&mut self) -> Result<Substitution, Vec<String>> {
        // For now, return empty substitution
        // This will be implemented with full HM constraint solving
        Ok(Substitution::new())
    }
}

impl Default for HmTypeChecker {
    fn default() -> Self {
        Self::new()
    }
}

// Type conversion from parser Type to our new Type system
impl From<String> for Type {
    fn from(value: String) -> Self {
        match value.to_lowercase().as_str() {
            "int" => Type::Int,
            "float" => Type::Float,
            "string" => Type::String,
            "bool" => Type::Bool,
            "array" => Type::Void, // TODO: Handle typed arrays
            "void" => Type::Void,
            "none" => Type::None,
            _ => {
                // For unknown types, create a type variable
                let tv = TypeVar::new(0, &value);
                Type::TypeVar(tv)
            }
        }
    }
}