use std::{
    borrow::Borrow,
    collections::HashMap, vec::Drain, 
};

use common::{Label, Message};
use parser::{Expression, SimpleSpan};

pub struct Typechecker {
    functions: HashMap<String, (Vec<Type>, Type)>,
    variables: HashMap<String, (Type, std::ops::Range<usize>)>,

    // --
    messages: Vec<Message>,
}

impl Default for Typechecker {
    fn default() -> Self {
        Self {
            functions: HashMap::with_capacity(16),
            variables: HashMap::with_capacity(16),
            messages: Vec::with_capacity(16),
        }
    }
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

    pub fn get_variable_type(&self, name: &String) -> Option<&Type> {
        self.variables.get(name).map(|(t, _)| t)
    }

    pub fn get_messages(&mut self) -> Drain<'_, Message> {
        self.messages.drain(0..)
    }

    pub fn check<'check>(&mut self, ast: &(SimpleSpan, Box<Expression<'check>>)) -> Type {
        let (span, child) = ast;

        match child.borrow() {
            Expression::Type(t) => t.to_string().into(),
            Expression::Integer(_) => Type::INTEGER,
            Expression::Bool(_) => Type::BOOLEAN,
            Expression::String(_) => Type::STRING,
            Expression::Float(_) => Type::FLOAT,
            Expression::Identifier(n) => {
                if let Some((var, _)) = self.variables.get(&n.to_string()) {
                    var.clone()
                } else {
                    n.to_string().into()
                }
            }
            Expression::Constant(name, ty) | Expression::Variable(name, ty) => {
                let t = if ty.is_some() {
                    ty.as_ref().map(|v| self.check(v)).unwrap_or_default()
                } else {
                    Type::default()
                };

                self.variables
                    .insert(self.resolve_variable(name), (t.clone(), span.into_range()));

                t
            }
            Expression::Function {
                name,
                args,
                returns,
                body,
            } => {
                let variables = self.variables.drain().collect::<Vec<_>>();

                let args = args.iter().map(|arg| self.check(arg)).collect::<Vec<_>>();

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

                self.variables
                    .drain()
                    .filter(|(_, (t, _))| *t == Type::UNKNOWN)
                    .for_each(|(name, (_, s))| {
                            let mut message = Message::info(
                                format!("Variable '{}' has undetermined type", name),
                                span.into_range(),
                            );
                            message.push(Label::new(
                                format!("Variable '{}' has undetermined type", name),
                                     s.clone()));
                            message.with_help("Possibly unused?".to_string());

                        self.messages.push(message);
                    });
                self.variables.extend(variables); 

                returns
                    .clone()
                    .map(|return_| self.check(&return_))
                    .unwrap_or_default()
            }
            Expression::Assignment(name, value) => {
                let name = self.resolve_variable(name);
                let ty = self.check(value);

                if let Some((t, entry_span)) = self.variables.get(&name).cloned() {
                    if t == Type::UNKNOWN {
                        self.variables.entry(name).and_modify(|entry| {
                            *entry = (ty.clone(), entry_span.clone())
                        });

                        ty
                    } else if t != ty {
                            let mut message = Message::error("Assignment type mismatch".to_string(), span.into_range());
                                message.push(Label::new(
                                    format!("Unable to assign value of type '{}' to variable '{}' that is of type '{}'", ty.to_string(), name, t.to_string()),
                                    span.into_range()
                                ));

                        self.messages.push(message);

                        Type::default()
                    } else {
                        t.clone()
                    }
                } else {
                        let mut message = Message::error("Missing variable".to_string(), span.into_range());
                            message.push(Label::new(
                                format!(
                                    "Attempting to assign value to undeclared variable '{}'",
                                    name
                                ),
                                span.into_range(),
                            ));
                    self.messages.push(message);

                    Type::default()
                }
            }
            Expression::Block(stmts) | Expression::Program(stmts) | Expression::Fragment(stmts) => {
                let mut expected = None;
                let mut expected_location = std::ops::Range::default();
                for stmt in stmts {
                    match stmt.1.borrow() {
                        Expression::Return(expr) | Expression::ImplicitReturn(expr) => {
                            let actual = self.check(expr);
                            if expected.is_none() {
                                expected = Some(actual);
                                expected_location = stmt.0.into_range();
                            } else if expected != Some(actual.clone()) {
                                        let mut message = Message::error("Return type mismatch".to_string(), span.into_range());
                                            message.push(Label::new(format!(
                                                    "Here return type is '{}'",
                                                    expected.clone().unwrap().to_string()
                                                ), expected_location.clone())
                                            );
                                message.push(
                                                Label::new(format!(
                                                    "Expected to return value of type '{}', instead returns '{}'",
                                                    expected.clone().unwrap().to_string(),
                                                    actual.to_string()), expr.0.into_range()
                                                )
                                            );
                                    self.messages.push(message);
                            }
                        }
                        _ => {
                            self.check(stmt);
                        }
                    }
                }

                expected.unwrap_or_default()
            }
            Expression::Statement(expr)
            | Expression::ExprStatement(expr)
            | Expression::Expr(expr)
            | Expression::Return(expr) => self.check(expr),
            Expression::ImplicitReturn(expr) => {
                

                self.check(expr)
            }
            Expression::Format(fmt, _) | Expression::Print(fmt, _) => {
                let type_ = self.check(fmt);
                if type_ != Type::STRING {
                        let mut message = Message::error("Invalid format string".to_string(), span.into_range());
                            message.push(Label::new(
                                format!(
                                    "Print format must evaluate to string, '{}' given",
                                    type_.to_string(),
                                ),
                                span.into_range(),
                            ));
                    self.messages.push(message);
                }

                Type::NONE
            }
            Expression::Comment(..) | Expression::Use { .. } => Type::NONE,
            Expression::Call { name, args } => {
                let name = self.resolve_variable(name);
                let call_arity = args.len();
                if let Some(func) = self.functions.get(&name).cloned() {
                    if call_arity != func.0.len() {
                            let mut message = Message::error("Invalid function call".to_string(), span.into_range());
                                message.push(Label::new(
                                    format!(
                                        "Function '{}' expects {} arguments, but called with {}",
                                        name,
                                        func.0.len(),
                                        call_arity
                                    ),
                                    span.into_range(),
                                ));
                        self.messages.push(message);
                    } else {
                        for (idx, arg) in args.iter().enumerate() {
                            let ty = self.check(arg);
                            if func.0[idx] != ty {
                                let mut message = Message::error("Invalid function argument".to_string(), span.into_range());
                                    message.push(
                                    Label::new(
                                    format!(
                                        "Argument #{} of function '{}' is incorrect, expected '{}' but got '{}'",
                                        idx + 1,
                                        name,
                                        func.0[idx].to_string(),
                                        ty.to_string(),
                                    ),
                                    arg.0.into_range(),
                                ));

                                self.messages.push(message);
                            }
                        }
                    }

                    func.1.clone()
                } else {
                        let mut message = Message::error("Unknown function".to_string(), span.into_range());
                        message.push(
                        Label::new(
                            format!(
                                "Unable to check signature, becuase function '{}' does not exist",
                                name
                            ),
                            span.into_range(),
                        ),
                    );

                    self.messages.push(message);

                    Type::default()
                }
            }
            Expression::Instantiate(class) => self.check(class),
            Expression::Negate(lhs) | Expression::Positive(lhs) => {
                let ty = self.check(lhs);

                match &ty {
                    Type::INTEGER | Type::FLOAT => ty,
                    _ => Type::default(),
                }
            }
            Expression::Not(lhs) => {
                let ty = self.check(lhs);

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
                let lhs = self.check(lhs);
                let rhs = self.check(rhs);

                if lhs != rhs || (lhs != Type::INTEGER && lhs != Type::FLOAT) {
                        let mut message = Message::error("Invalid expression".to_string(), span.into_range());
                        message.push(Label::new(
                                "Unable to perform arithmetic operation on non-numeric types."
                                    .to_string(),
                                span.into_range(),
                            ));
                    self.messages.push(message);

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
                let lhs = self.check(lhs);
                let rhs = self.check(rhs);

                // @TODO: Handle `UNKNOWN` that is unable to resolve the function params
                if lhs != rhs || lhs != Type::INTEGER {
                        let mut message =Message::error("Invalid expression".to_string(), span.into_range());
                        message.push(Label::new(
                                "Unable to perform bitwise for non-numeric types.".to_string(),
                                span.into_range(),
                            ));
                    self.messages.push(message);

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
                let lhs = self.check(lhs);
                let rhs = self.check(rhs);

                // @TODO: Handle `UNKNOWN` that is unable to resolve the function params
                if lhs != rhs {
                    let mut message = Message::error("Invalid expression".to_string(), span.into_range());
                        message.push(Label::new(
                                "Unable to perform comparison of non-identical types.".to_string(),
                                span.into_range(),
                            ));
                    self.messages.push(message);

                    Type::default()
                } else {
                    Type::BOOLEAN
                }
            }

            Expression::Branch(condition, body) => {
                if let Some(condition) = condition {
                    let expr = self.check(condition);
                    if !matches!(expr, Type::BOOLEAN) {
                            let mut message = Message::error("Invalid condition".to_string(), span.into_range());
                            message.push(Label::new(
                                    "Conditional expression does not evaluate to true".to_string(),
                                    condition.0.into_range(),
                                ));
                        self.messages.push(message);
                    }

                } 

                self.check(body)
            }
            Expression::If(branches) => {
                branches.iter().for_each(|b| { self.check(b); });

                Type::NONE
            }
            // Expression::If { condition, .. } => {
            //     let expr = self.check(condition);
            //     if !matches!(expr, Type::BOOLEAN) {
            //             let mut message = Message::error("Invalid condition".to_string(), span.into_range());
            //             message.push(Label::new(
            //                     "Conditional expression does not evaluate to true".to_string(),
            //                     condition.0.into_range(),
            //                 ));
            //         self.messages.push(message);
            //     }
            //
            //     expr
            // }
            Expression::Match(lhs, children) => {
                let lhs_ = self.check(lhs);
                let mut expected = None;
                let mut expected_location = std::ops::Range::default();

                for (rhs, body) in children {
                    let current = self.check(rhs);

                    if lhs_ != current {
                            let mut message = Message::error("Invalid comparison".to_string(), span.into_range());
                                message.push(
                                    Label::new(
                                        format!("Expression is of type '{}'", lhs_.to_string()),
                                        lhs.0.into_range(),
                                    )
                                );
                                message.push(Label::new(
                                    format!(
                                        "Found expression to be of type '{}', while expecting '{}'",
                                        current.to_string(),
                                        lhs_.to_string()
                                    ),
                                    rhs.0.into_range(),
                                ));
                        self.messages.push(message);
                    }

                    let body_ = self.check(body);
                    if expected.is_none() {
                        expected = Some(body_.clone());
                        expected_location = body.0.into_range();
                    } else if expected != Some(body_.clone()) {
                        let mut message = 
                        Message::error("Unexpected return value".to_string(), span.into_range());
                        message.push(
                            Label::new(
                                format!("Result of this block is '{}'", body_.to_string()),
                                expected_location.clone(),
                            )
                        );
                        message.push(Label::new(
                            format!(
                                "Expected this block to result in '{}' but found '{}' instead.",
                                expected.clone().unwrap().to_string(),
                                body_.to_string()
                            ),
                            body.0.into_range(),
                        ));
                    self.messages.push(message);
                    }
                }

                expected.unwrap_or_default()
            }
            e => {
                #[cfg(debug_assertions)]
                dbg!(e);
                let mut message = Message::error("Unknown expression".to_string(), span.into_range());
                message.push(Label::new(
                            format!("Unknown expression '{:?}'", e),
                            span.into_range(),
                        ));
                self.messages.push(message);

                Type::default()
            }
        }
    }

}
