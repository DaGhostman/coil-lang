use std::{
    borrow::Borrow,
    collections::{HashMap, HashSet},
};

use common::Message;
use parser::{Expression, SimpleSpan};

#[derive(Default)]
pub struct Typechecker {
    functions: HashMap<String, (Vec<Type>, Type)>,
    variables: HashMap<String, Type>,

    // --
    messages: HashSet<Message>,
}

#[derive(Default, Clone, Debug, PartialEq)]
pub enum Type {
    #[default]
    UNKNOWN,
    NONE,
    INTEGER,
    FLOAT,
    STRING,
    BOOLEAN,
    LIST,
    OBJECT(String),
}

impl From<String> for Type {
    fn from(value: String) -> Self {
        match value.to_lowercase().as_str() {
            "int" => Self::INTEGER,
            "float" => Self::FLOAT,
            "string" => Self::STRING,
            "bool" => Self::BOOLEAN,
            "array" => Self::LIST, // TODO: Handle typed arrays
            "void" => Self::NONE,
            _ => Self::OBJECT(value), // TODO: Handle generic params
        }
    }
}

impl ToString for Type {
    fn to_string(&self) -> String {
        match self {
            Type::INTEGER => "int",
            Type::FLOAT => "float",
            Type::BOOLEAN => "bool",
            Type::STRING => "string",
            Type::LIST => "array",
            Type::NONE => "void",
            Type::OBJECT(s) => s,
            Type::UNKNOWN => "%",
        }
        .to_string()
    }
}

impl Typechecker {
    fn resolve_variable<'compiler>(
        &self,
        variable: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> String {
        match variable.1.borrow() {
            Expression::Identifier(n) | Expression::Type(n) => n.to_string(),
            f => {
                dbg!(&f);
                todo!("Function name as expression")
            }
        }
    }

    pub fn register_native_function(&mut self, name: &str, params: &[Type], returns: Type) {
        self.functions
            .insert(name.to_string(), (params.to_vec(), returns));
    }

    pub fn check<'check>(
        &mut self,
        ast: &(SimpleSpan, Box<Expression<'check>>),
    ) -> Result<Type, HashSet<Message>> {
        let (span, child) = ast;

        let result = match child.borrow() {
            Expression::Type(t) => t.to_string().into(),
            Expression::Integer(_) => Type::INTEGER,
            Expression::Bool(_) => Type::BOOLEAN,
            Expression::String(_) => Type::STRING,
            Expression::Float(_) => Type::FLOAT,
            Expression::Identifier(n) => {
                if let Some(var) = self.variables.get(&n.to_string()) {
                    var.clone()
                } else {
                    n.to_string().into()
                }
            }
            Expression::Constant(name, ty) | Expression::Variable(name, ty) => {
                let t = if ty.is_some() {
                    ty.as_ref()
                        .map(|v| self.check(v).unwrap_or_default())
                        .unwrap_or_default()
                } else {
                    Type::default()
                };

                self.variables
                    .insert(self.resolve_variable(name), t.clone());

                t
            }
            Expression::Function {
                name,
                args,
                returns,
                body,
            } => {
                let variables = self.variables.clone();
                self.variables.clear();

                let args = args
                    .iter()
                    .map(|arg| self.check(arg).unwrap_or_default())
                    .collect::<Vec<_>>();

                self.functions.insert(
                    name.to_string(),
                    (
                        args.clone(),
                        returns
                            .clone()
                            .map(|v| self.resolve_variable(&v).into())
                            .unwrap_or_default(),
                    ),
                );

                body.iter().for_each(|stmt| {
                    let _ = self.check(stmt);
                });

                self.variables = variables;

                returns
                    .clone()
                    .map(|return_| self.check(&return_).unwrap_or_default())
                    .unwrap_or_default()
            }
            Expression::Assignment(name, value) => {
                let name = self.resolve_variable(name);
                let ty = self.check(value).unwrap_or_default();

                if self.variables.contains_key(&name) {
                    if let Some(t) = self.variables.get(&name) {
                        if t == &Type::UNKNOWN {
                            self.variables.entry(name).and_modify(|entry| {
                                *entry = ty.clone();
                            });

                            ty
                        } else if t != &ty {
                            self.messages.insert(
                                Message::error(
                                    format!("Unable to assign value of type '{}' to variable '{}' that is of type '{}'", ty.to_string(), name, t.to_string()),
                                    span.into_range()
                                )
                            );
                            Type::default()
                        } else {
                            Type::default()
                        }
                    } else {
                        Type::default()
                    }
                } else {
                    Type::default()
                }
            }
            Expression::Block(stmts) | Expression::Program(stmts) | Expression::Fragment(stmts) => {
                for stmt in stmts {
                    let _ = self.check(stmt);
                }

                Type::default()
            }
            Expression::Statement(expr)
            | Expression::ExprStatement(expr)
            | Expression::Expr(expr)
            | Expression::Return(expr) => self.check(expr)?,
            Expression::Format(fmt, _) | Expression::Print(fmt, _) => {
                let type_ = self.check(fmt).unwrap_or_default();
                if type_ != Type::STRING {
                    self.messages.insert(Message::error(
                        format!(
                            "Print format must evaluate to string, '{}' given",
                            type_.to_string(),
                        ),
                        span.into_range(),
                    ));
                }

                Type::NONE
            }
            Expression::Comment(..) | Expression::Use { .. } => Type::NONE,
            Expression::Call { name, args } => {
                let name = self.resolve_variable(name);
                let call_arity = args.len();
                if let Some(func) = self.functions.get(&name).cloned() {
                    if call_arity != func.0.len() {
                        self.messages.insert(Message::error(
                            format!(
                                "Function '{}' expects {} arguments, but called with {}",
                                name,
                                func.0.len(),
                                call_arity
                            ),
                            span.into_range(),
                        ));
                    } else {
                        for (idx, arg) in args.iter().enumerate() {
                            if let Ok(ty) = self.check(arg) {
                                if func.0[idx] != ty {
                                    self.messages.insert(Message::error(
                                format!(
                                    "Argument #{} of function '{}' is incorrect, expected '{}' but got '{}'",
                                    idx + 1,
                                    name,
                                    func.0[idx].to_string(),
                                    ty.to_string(),
                                ),
                                arg.0.into_range(),
                            ));
                                }
                            };
                        }
                    }

                    func.1.clone()
                } else {
                    self.messages.insert(Message::error(
                        format!(
                            "Unable to check signature, becuase function '{}' does not exist",
                            name
                        ),
                        span.into_range(),
                    ));

                    Type::default()
                }
            }
            Expression::Instantiate(class) => self.check(class).unwrap_or_default(),
            Expression::Negate(lhs) | Expression::Positive(lhs) => {
                let ty = self.check(lhs).unwrap_or_default();

                match &ty {
                    Type::INTEGER | Type::FLOAT => ty,
                    _ => Type::default(),
                }
            }
            Expression::Not(lhs) => {
                let ty = self.check(lhs).unwrap_or_default();

                match ty {
                    Type::INTEGER | Type::FLOAT | Type::BOOLEAN => Type::BOOLEAN,
                    _ => Type::default(),
                }
            }
            Expression::Add(lhs, rhs)
            | Expression::Sub(lhs, rhs)
            | Expression::Mul(lhs, rhs)
            | Expression::Div(lhs, rhs)
            | Expression::Mod(lhs, rhs) => {
                let lhs = self.check(lhs).unwrap_or_default();
                let rhs = self.check(rhs).unwrap_or_default();

                if lhs != rhs || (lhs != Type::INTEGER && lhs != Type::FLOAT) {
                    self.messages.insert(Message::error(
                        "Unable to perform arithmetic operation on non-numeric types.".to_string(),
                        span.into_range(),
                    ));

                    Type::default()
                } else {
                    lhs
                }
            }
            Expression::Shl(lhs, rhs)
            | Expression::Shr(lhs, rhs)
            | Expression::And(lhs, rhs)
            | Expression::Or(lhs, rhs)
            | Expression::Xor(lhs, rhs) => {
                let lhs = self.check(lhs).unwrap_or_default();
                let rhs = self.check(rhs).unwrap_or_default();

                // @TODO: Handle `UNKNOWN` that is unable to resolve the function params
                if lhs != rhs || lhs != Type::INTEGER {
                    self.messages.insert(Message::error(
                        "Unable to perform bitwise for non-numeric types.".to_string(),
                        span.into_range(),
                    ));

                    Type::default()
                } else {
                    lhs
                }
            }
            Expression::Eq(lhs, rhs)
            | Expression::Neq(lhs, rhs)
            | Expression::Le(lhs, rhs)
            | Expression::Gt(lhs, rhs)
            | Expression::Leq(lhs, rhs)
            | Expression::Geq(lhs, rhs) => {
                let lhs = self.check(lhs).unwrap_or_default();
                let rhs = self.check(rhs).unwrap_or_default();

                // @TODO: Handle `UNKNOWN` that is unable to resolve the function params
                if lhs != rhs {
                    self.messages.insert(Message::error(
                        "Unable to perform comparison of non-identical types.".to_string(),
                        span.into_range(),
                    ));

                    Type::default()
                } else {
                    Type::BOOLEAN
                }
            }

            Expression::If { condition, .. } => self.check(condition).unwrap_or_default(),
            e => {
                #[cfg(debug_assertions)]
                dbg!(e);
                self.messages.insert(Message::error(
                    format!("Unknown expression '{:?}'", e),
                    span.into_range(),
                ));

                Type::default()
            }
        };

        if !self.messages.is_empty() {
            Err(self.messages.clone())
        } else {
            Ok(result)
        }
    }
}
