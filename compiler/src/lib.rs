mod hm_typechecker;
mod pipeline;
mod types;

use std::{any::Any, borrow::Borrow, collections::HashMap};

use common::{likely, unlikely, Byte, Instruction, Interner, Label, Message, Value};
use parser::{ast::Expression, SimpleSpan};

pub use crate::types::ty::Type;
pub use pipeline::*;

use crate::hm_typechecker::HmTypeChecker;

macro_rules! unary {
    ($result: expr, $self: expr, $rhs: expr, $instruction: expr) => {
        $result.append(&mut $self.do_compile($rhs));

        $result.push($instruction);
    };
}
macro_rules! binary {
    ($result: expr, $self: expr, $lhs: expr, $rhs: expr, $instruction: expr) => {
        $result.append(&mut $self.do_compile($lhs));
        $result.append(&mut $self.do_compile($rhs));

        $result.push($instruction);
    };
}

#[derive(Default, Clone)]
struct Context {
    current: Option<String>,
    variables: Interner<String>,
    symbols: Interner<String>,
    assignments: HashMap<String, bool>,
    constants: HashMap<usize, bool>,
    defers: Vec<usize>,
    classes: HashMap<String, Vec<(String, usize)>>,
    impementations: HashMap<String, String>,
    methods: HashMap<String, HashMap<String, String>>,

    prev: Option<Box<Self>>,
}

pub struct Compiler {
    namespace: String,
    bytecode: Vec<Byte>,

    aliases: HashMap<String, String>,
    functions: HashMap<String, usize>,
    native: HashMap<String, usize>,
    // --
    messages: Vec<Message>,
    context: Context,
    // HM Type Checker
    hm_typechecker: HmTypeChecker,
    // Variant discriminants: type_name -> (variant_name -> discriminant_value)
    variant_discriminants: HashMap<String, HashMap<String, i64>>,
}

impl Default for Compiler {
    fn default() -> Self {
        let mut bytecode = Vec::with_capacity(1024);
        bytecode.append(&mut vec![
            Byte::new(Instruction::CALL),
            Byte::new(Instruction::JMP).with_operand_u32(u32::MAX),
            Byte::new(Instruction::HALT),
        ]);

        Self {
            namespace: String::default(),
            bytecode,
            aliases: HashMap::default(),
            functions: HashMap::with_capacity(32),
            native: HashMap::default(),
            messages: Vec::default(),
            context: Context::default(),
            hm_typechecker: HmTypeChecker::new(),
            variant_discriminants: HashMap::default(),
        }
    }
}

impl<'ctx> Context {
    fn child(&self) -> Self {
        Self {
            current: self.current.clone(),
            impementations: self.impementations.clone(),
            methods: self.methods.clone(),
            defers: Vec::default(),
            constants: self.constants.clone(),
            assignments: self.assignments.clone(),
            variables: self.variables.clone(),
            symbols: self.symbols.clone(),
            classes: self.classes.clone(),
            prev: Some(Box::new(self.to_owned())),
        }
    }

    fn clear(&mut self) {
        self.defers.clear();
        self.constants.clear();
        self.variables = Default::default();
        self.assignments.clear();
    }
}

impl<'ctx> Context {
    pub fn get_prev(&self) -> &Option<Box<Self>> {
        &self.prev
    }
}

impl Compiler {
    pub fn get_function(&self, name: &str) -> usize {
        self.functions[name]
    }

    pub fn get_messages(&self) -> &Vec<Message> {
        &self.messages
    }

    pub fn register(&mut self, name: &str, params: &[Type], returns: Type) -> &mut Self {
        let idx = self.native.len();
        self.native.insert(name.to_string(), idx);
        // Register function signature with HM typechecker for type inference
        let func_ty = Type::Function(params.to_vec(), Box::new(returns));
        self.hm_typechecker
            .get_env_mut()
            .define_function(name, func_ty);
        self
    }

    fn resolve_variable<'compiler>(
        &self,
        variable: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> String {
        match variable.1.borrow() {
            Expression::Identifier(n) => n.to_string(),
            f => {
                eprintln!("{}", f);
                todo!("Function name as expression")
            }
        }
    }

    fn typecheck<'check>(&mut self, ast: &(SimpleSpan, Box<Expression<'check>>)) -> Type {
        // Reset HM typechecker before each typecheck

        // Use the HM typechecker for type inference
        match self.hm_typechecker.check(ast) {
            Ok(ty) => ty,
            Err(errors) => {
                // Report type errors
                for error in errors {
                    let message = Message::error(error, ast.0.clone().into_range());
                    self.messages.push(message);
                }
                Type::Void
            }
        }
    }

    fn do_compile<'compiler>(
        &mut self,
        ast: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> Vec<Byte> {
        let mut bytecode = vec![];
        let _type = self.typecheck(ast);
        let (span, child) = ast;

        match child.borrow() {
            Expression::Comment(_) => (),
            Expression::Use {
                path: p,
                name,
                alias,
            } => {
                let mut prefix = p.clone();
                if prefix.len() == 1 {
                    prefix.push("".to_string());
                }
                self.aliases.insert(
                    alias.clone().unwrap_or(name.to_string()),
                    format!("{}{}", prefix.join("::"), name),
                );
            }
            Expression::Noop(_) => (),
            Expression::Group(e) => bytecode.append(&mut self.do_compile(e)),
            Expression::Program(children) | Expression::Fragment(children) => {
                children.iter().for_each(|child| {
                    bytecode.append(&mut self.do_compile(child));
                });
            }
            Expression::Block(children) => {
                let ctx = self.context.child();
                self.context = ctx;
                children.iter().for_each(|child| {
                    bytecode.append(&mut self.do_compile(child));
                });

                self.context = *self.context.get_prev().clone().unwrap();
            }
            Expression::Function {
                name,
                args,
                returns: _returns,
                body,
            } => {
                let (env, constraints, counter) = self.hm_typechecker.reset();
                self.context = self.context.child();
                self.context.clear();
                // Register function signature with HM typechecker for type inference
                self.hm_typechecker.check(ast).ok();

                self.functions
                    .insert(format!("{}{}", self.namespace, name), self.bytecode.len());

                let mut a = self.do_compile(args);

                self.bytecode.append(&mut a);

                let mut c = self.do_compile(body);
                self.bytecode.append(&mut c);

                self.context.defers.iter().for_each(|offset| {
                    self.bytecode
                        .push(Byte::new(Instruction::JMP).with_operand_u32(*offset as u32));
                });

                if !matches!(
                    self.bytecode.last().map(|b| b.bytecode()),
                    Some(Instruction::RETURN)
                ) {
                    self.bytecode.push(Byte::new_with_value(
                        Instruction::CONST,
                        Value::default().raw() as _,
                    ));
                    self.bytecode.push(Byte::new(Instruction::RETURN));
                }
                // Keep the updated environment with generic signatures instead of restoring
                // self.hm_typechecker.restore(env, constraints, counter);
                self.hm_typechecker
                    .get_env_mut()
                    .define_function(name, _type);
                if let Some(ctx) = self.context.get_prev().clone() {
                    self.context = *ctx;
                }
            }
            Expression::FunctionWithGenerics {
                name,
                args,
                returns: _returns,
                body,
                generics: _,
            } => {
                // Handle generic functions similarly to regular functions
                let (env, constraints, counter) = self.hm_typechecker.reset();
                self.context = self.context.child();
                self.context.clear();
                // Register function signature with HM typechecker for type inference
                self.hm_typechecker.check(ast).ok();

                self.functions
                    .insert(format!("{}{}", self.namespace, name), self.bytecode.len());

                let mut a = self.do_compile(args);

                self.bytecode.append(&mut a);

                let mut c = self.do_compile(body);
                self.bytecode.append(&mut c);

                self.context.defers.iter().for_each(|offset| {
                    self.bytecode
                        .push(Byte::new(Instruction::JMP).with_operand_u32(*offset as u32));
                });

                if !matches!(
                    self.bytecode.last().map(|b| b.bytecode()),
                    Some(Instruction::RETURN)
                ) {
                    self.bytecode.push(Byte::new_with_value(
                        Instruction::CONST,
                        Value::default().raw() as _,
                    ));
                    self.bytecode.push(Byte::new(Instruction::RETURN));
                }
                // Keep the updated environment with generic signatures
                // self.hm_typechecker.restore(env, constraints, counter);
                self.hm_typechecker
                    .get_env_mut()
                    .define_function(name, _type);
                if let Some(ctx) = self.context.get_prev().clone() {
                    self.context = *ctx;
                }
            }
            Expression::Expr(child) | Expression::Statement(child) => {
                bytecode.append(&mut self.do_compile(child))
            }
            Expression::ExprStatement(child) => {
                bytecode.append(&mut self.do_compile(child));
                // Do not add pop if previous instruction is `DUP` since they both cancel eachother
                // out
                if !matches!(
                    bytecode.last().map(|b| b.bytecode()),
                    Some(Instruction::DUPLICATE)
                ) {
                    bytecode.push(Byte::new(Instruction::POP));
                } else {
                    // If it was supposed to add `POP` but prev is `DUP`
                    // then remove the DUP as well
                    bytecode.pop();
                }
            }
            Expression::Print(format, params) => {
                bytecode.append(&mut self.do_compile(format));
                let mut params_len = 0;
                if let Some(params) = params {
                    params_len = params.len();
                    params.iter().for_each(|param| {
                        bytecode.append(&mut self.do_compile(param));
                    });
                }
                bytecode.push(Byte::new(Instruction::FORMAT).with_operand_u32(params_len as u32));
                bytecode.push(Byte::new(Instruction::PRINT));
            }
            Expression::Format(format, params) => {
                bytecode.append(&mut self.do_compile(format));
                let mut params_len = 0;
                if let Some(params) = params {
                    params_len = params.len();
                    params.iter().for_each(|param| {
                        bytecode.append(&mut self.do_compile(param));
                    });
                }
                bytecode.push(Byte::new(Instruction::FORMAT).with_operand_u32(params_len as u32));
            }
            Expression::Return(expr) | Expression::ImplicitReturn(expr) => {
                self.context.defers.iter().for_each(|offset| {
                    self.bytecode
                        .push(Byte::new(Instruction::CALL).with_operand_u32(0));
                    self.bytecode
                        .push(Byte::new(Instruction::JMP).with_operand_u32(*offset as u32));
                });

                // if let Expression::Identifier(name) = *expr.1.borrow() {
                //     let ty = self.typechecker.get_variable_type(&name.into());
                //     let symbol = self.context.variables.key(&name.into());
                //     // if matches!(ty, Some(Type::OBJECT(_))) || matches!(ty, Some(Type::STRING)) {
                //     //     bytecode.push(Byte::new_with_operands(
                //     //         Instruction::ACQUIRE,
                //     //         [symbol.expect("Unable to resolve unknown variable"), 0],
                //     //     ));
                //     // }
                // }

                // for variable in self.context.variables.iter() {
                //     if let (Some(symbol), Some(ty)) = (
                //         self.context.variables.key(variable),
                //         self.typechecker.get_variable_type(variable),
                //     ) && (matches!(ty, Type::OBJECT(_)) || matches!(ty, Type::STRING))
                //     {
                //         bytecode.push(Byte::new_with_operands(Instruction::RELEASE, [symbol, 0]));
                //     }
                // }

                // if matches!(expr.1.borrow(), Expression::Identifier(_)) {
                //     let symbol = self.context.variables.intern(self.resolve_variable(expr));
                // }

                bytecode.append(&mut self.do_compile(expr));
                if !matches!(child.borrow(), Expression::ImplicitReturn(_)) {
                    bytecode.push(Byte::new(Instruction::RETURN));
                }
            }
            Expression::Class(name, state) => {
                self.context.classes.insert(
                    name.to_string(),
                    state
                        .iter()
                        .enumerate()
                        .map(|(idx, v)| match v.1.borrow() {
                            Expression::Field(n, _) => (self.resolve_variable(n), idx),
                            _ => unreachable!(
                                "The should be only fields inside of a class definition"
                            ),
                        })
                        .collect::<Vec<_>>(),
                );
                self.context.symbols.intern(name.to_string());
            }
            Expression::SumType(name, variants) => {
                // Register variant discriminants for the sum type
                // For now, we use a placeholder type name since the enum name isn't captured
                let mut discriminants = HashMap::new();
                for (idx, variant) in variants.iter().enumerate() {
                    let var_name = match variant.1.borrow() {
                        Expression::VariantItem(_n, name_expr) => {
                            // Extract variant name from Type::Name
                            match name_expr.1.borrow() {
                                Expression::Identifier(n) => n.to_string(),
                                _ => variant.1.to_string(),
                            }
                        }
                        Expression::VariantWithDestructure(_ty, name_expr, _fields) => {
                            // Extract variant name from variant with destructured fields
                            match name_expr.1.borrow() {
                                Expression::Identifier(n) => n.to_string(),
                                _ => variant.1.to_string(),
                            }
                        }
                        Expression::Variant(name_expr, _n) => {
                            // Legacy variant syntax
                            match name_expr.1.borrow() {
                                Expression::Identifier(n) => n.to_string(),
                                _ => variant.1.to_string(),
                            }
                        }
                        _ => unreachable!(
                            "Variant should be VariantItem, VariantWithDestructure, or Variant"
                        ),
                    };
                    discriminants.insert(var_name, idx as i64);
                }

                let name = self.resolve_variable(name);

                // Use a placeholder type name
                self.variant_discriminants.insert(name, discriminants);
            }
            Expression::Implementation(what, owner, methods) => {
                let namespace = self.namespace.clone();
                let functions = self.functions.clone();

                self.namespace.push_str(what);

                for func in methods {
                    self.functions.drain();
                    self.do_compile(func);

                    for (method, _) in self.functions.iter() {
                        self.context
                            .methods
                            .entry(owner.to_string())
                            .and_modify(|e| {
                                e.insert(what.to_string(), method.clone());
                            })
                            .or_insert_with(|| {
                                let mut h = HashMap::default();
                                h.insert(what.to_string(), method.clone());
                                h
                            });
                    }
                }

                self.context
                    .impementations
                    .insert(what.to_string(), owner.to_string());

                self.namespace = namespace;
                self.functions = functions;
            }
            Expression::Instantiate(class, _args) => {
                let name = self.resolve_variable(class);
                bytecode.push(
                    Byte::new(Instruction::INIT)
                        .with_operand_u32(self.context.classes[&name].len() as u32),
                );
                // bytecode.push(Byte::new(Instruction::SET).with_operand_u32(operand);
                // let s = self;
                // self.functions.get(k);

                // bytecode.push(Byte::new(Instruction::CONST).with_operand_u32(0));
            }
            Expression::Inc(var) => {
                let name = self.resolve_variable(var);
                let symbol = self.context.variables.intern(name);

                bytecode.push(Byte::new(Instruction::INC).with_operand_u32(symbol as u32));
            }
            Expression::Dec(var) => {
                let name = self.resolve_variable(var);
                let symbol = self.context.variables.intern(name);

                bytecode.push(Byte::new(Instruction::DEC).with_operand_u32(symbol as u32));
            }
            Expression::Loop { iterable, body, .. } => {
                let mut loop_ = self.do_compile(iterable);
                let exit = loop_.len();
                loop_.push(Byte::new(Instruction::JMPF));
                loop_.append(&mut self.do_compile(body));
                loop_
                    .push(Byte::new(Instruction::JMP).with_operand_u32(self.bytecode.len() as u32));

                let len = loop_.len();
                loop_[exit] = Byte::new(Instruction::JMPF)
                    .with_operand_u32((self.bytecode.len() + len) as u32);

                self.bytecode.append(&mut loop_);
            }
            Expression::Defer(child) => {
                let mut body = vec![Byte::new(Instruction::JMP).with_operand_u32(u32::MAX)];

                self.context.defers.push(self.bytecode.len() + body.len());

                body.append(&mut self.do_compile(child));
                body.push(Byte::new_with_value(
                    Instruction::CONST,
                    Value::from(0i64).raw() as _,
                ));
                body.push(Byte::new(Instruction::RETURN));

                let total_length = self.bytecode.len();
                let current_length = body.len() + bytecode.len();
                if let Some(v) = body.first_mut() {
                    *v = Byte::new(Instruction::JMP)
                        .with_operand_u32((total_length + current_length) as u32);
                }

                bytecode.append(&mut body);
            }
            Expression::Call { name, args } => {
                let identifier = self.resolve_variable(name);
                let n = self.aliases.get(&identifier).unwrap_or(&identifier);

                if let Some(offset) = self.functions.get(n).copied() {
                    if let Some(args) = args {
                        args.iter()
                            .for_each(|arg| bytecode.append(&mut self.do_compile(arg)))
                    }

                    bytecode.push(Byte::new(Instruction::CALL).with_operand_u32(
                        args.as_ref().map(|items| items.len()).unwrap_or(0) as u32,
                    ));
                    bytecode.push(Byte::new(Instruction::JMP).with_operand_u32(offset as u32));
                } else if self.native.get(n).is_some() {
                    todo!("Not implemented");
                } else {
                    let mut message =
                        Message::error("Unknown function".to_string(), span.into_range());
                    message.push(Label::new(
                        format!("Unable to call unknown function '{}'", n),
                        span.into_range(),
                    ));
                    self.messages.push(message);
                }
            }
            Expression::GenericFunctionCall {
                name,
                type_args: _,
                args,
            } => {
                // Generic function call - treat as regular function call for now
                // The type arguments are handled by the HM typechecker for type inference
                let identifier = self.resolve_variable(name);
                let n = self.aliases.get(&identifier).unwrap_or(&identifier);

                if let Some(offset) = self.functions.get(n).copied() {
                    if let Some(args) = args {
                        args.iter()
                            .for_each(|arg| bytecode.append(&mut self.do_compile(arg)))
                    }

                    bytecode.push(Byte::new(Instruction::CALL).with_operand_u32(
                        args.as_ref().map(|items| items.len()).unwrap_or(0) as u32,
                    ));
                    bytecode.push(Byte::new(Instruction::JMP).with_operand_u32(offset as u32));
                } else if self.native.get(n).is_some() {
                    todo!("Not implemented");
                } else {
                    let mut message =
                        Message::error("Unknown generic function".to_string(), span.into_range());
                    message.push(Label::new(
                        format!("Unable to call unknown generic function '{}'", n),
                        span.into_range(),
                    ));
                    self.messages.push(message);
                }
            }
            Expression::VariantItem(ty_expr, name_expr) => {
                // Variant item (Type::Variant) - emit the discriminant value
                // Extract type name from Output<'expr>
                let type_name = match ty_expr.1.borrow() {
                    Expression::Type(t) => t.1.to_string(),
                    _ => ty_expr.1.to_string(),
                };

                // Extract variant name from Output<'expr>
                let var_name = match name_expr.1.borrow() {
                    Expression::Identifier(n) => n.to_string(),
                    _ => name_expr.1.to_string(),
                };

                // Look up discriminant value
                if let Some(discriminants) = self.variant_discriminants.get(&type_name) {
                    if let Some(discriminant) = discriminants.get(&var_name) {
                        bytecode.push(Byte::new_with_value(
                            Instruction::CONST,
                            Value::from(*discriminant).raw() as _,
                        ));
                    } else {
                        let mut message = Message::error(
                            format!("Unknown variant '{}::{}'", type_name, var_name),
                            span.into_range(),
                        );
                        self.messages.push(message);
                    }
                } else {
                    let mut message =
                        Message::error(format!("Unknown type '{}'", type_name), span.into_range());
                    self.messages.push(message);
                }
            }
            Expression::VariantWithDestructure(_ty, _name, fields) => {
                // Variant with destructured fields - emit variant discriminant + push fields on stack
                // For match patterns like: Result::Ok(value)
                // We need to push the discriminant and then the field values
                let type_name = match _ty.1.borrow() {
                    Expression::Type(t) => t.1.to_string(),
                    _ => _ty.1.to_string(),
                };

                let var_name = match _name.1.borrow() {
                    Expression::Identifier(n) => n.to_string(),
                    _ => _name.1.to_string(),
                };

                if let Some(discriminants) = self.variant_discriminants.get(&type_name) {
                    if let Some(discriminant) = discriminants.get(&var_name) {
                        // Clone discriminant to avoid borrow issues
                        let discrim_val = *discriminant;
                        // Push discriminant
                        bytecode.push(Byte::new_with_value(
                            Instruction::CONST,
                            Value::from(discrim_val).raw() as _,
                        ));

                        // Push field values on stack (for pattern matching)
                        for field in fields {
                            bytecode.append(&mut self.do_compile(field));
                        }

                        // Emit variant set instruction: tag + field_count
                        bytecode.push(
                            Byte::new(Instruction::VARIANT_SET)
                                .with_operands_u16([discrim_val as u16, fields.len() as u16]),
                        );
                    } else {
                        let mut message = Message::error(
                            format!("Unknown variant '{}::{}'", type_name, var_name),
                            span.into_range(),
                        );
                        self.messages.push(message);
                    }
                } else {
                    let mut message =
                        Message::error(format!("Unknown type '{}'", type_name), span.into_range());
                    self.messages.push(message);
                }
            }
            // Expression::Variant(name_expr, fields) => {
            //     // Variant for sum type - emit the discriminant value
            //     // Note: This is for legacy variant syntax in enum declarations
            //     let var_name = match name_expr.1.borrow() {
            //         Expression::Identifier(n) => n.to_string(),
            //         _ => name_expr.1.to_string(),
            //     };
            //     // For enum declaration variants, we don't have discriminants yet
            //     // Emit as 0 (will be updated when SumType is processed)
            //     bytecode.push(Byte::new_with_value(
            //         Instruction::CONST,
            //         Value::from(0).raw() as _,
            //     ));
            // }
            Expression::Argument(ty_expr, n_expr) => {
                // Extract type name from Output<'expr>
                let ty_name = match ty_expr.1.borrow() {
                    Expression::Type(t) => t.1.to_string(),
                    _ => ty_expr.1.to_string(),
                };
                // Extract variable name from Output<'expr>
                let var_name = match n_expr.1.borrow() {
                    Expression::Identifier(n) => n.to_string(),
                    _ => n_expr.1.to_string(),
                };

                let _ = self.context.variables.intern(var_name.clone());
                self.hm_typechecker
                    .get_env_mut()
                    .define_variable(&var_name, Type::from(ty_name));
                // bytecode.push(Byte::new(Instruction::LOAD)
            }
            Expression::Identifier(n) => {
                if let Some(symbol) = self.context.variables.key(&n.to_string()) {
                    bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(symbol as u32));
                } else {
                    let mut message =
                        Message::error("Unknown variable".to_string(), span.into_range());
                    message.push(Label::new(
                        format!("Unknown variable '{}'", n),
                        span.into_range(),
                    ));
                    self.messages.push(message);
                }
            }
            Expression::If(branches) => {
                let mut compiled = branches
                    .iter()
                    .map(|(_, branch)| {
                        if let Expression::Branch(condition, body) = branch.borrow() {
                            (
                                condition.as_ref().map(|c| self.do_compile(c)),
                                self.do_compile(body),
                            )
                        } else {
                            unreachable!("Unable to handle");
                        }
                    })
                    .collect::<Vec<_>>();

                let compiled_lenght = compiled
                    .iter()
                    .map(|(condition, body)| {
                        if !condition.is_none() {
                            condition.as_ref().map(|c| c.len()).unwrap_or(0) + body.len() + 2
                        } else {
                            0
                        }
                    })
                    .sum::<usize>()
                    + self.bytecode.len()
                    + bytecode.len();

                let branchless = branches.len() == 1;
                compiled.iter_mut().for_each(|(condition, body)| {
                    if let Some(condition) = condition {
                        bytecode.append(condition);
                        bytecode.push(Byte::new(Instruction::JMPF).with_operand_u32(
                            (bytecode.len()
                                + self.bytecode.len()
                                + body.len()
                                + 1
                                + ((!branchless) as usize)) as u32,
                        ));
                    }

                    if !branchless {
                        body.push(
                            Byte::new(Instruction::JMP).with_operand_u32(compiled_lenght as u32),
                        );
                    }
                    bytecode.append(body);
                });
            }
            Expression::Le(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(if self.typecheck(lhs) == Type::Float {
                        Instruction::LEF
                    } else {
                        Instruction::LE
                    },)
                );
            }
            Expression::Gt(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(if self.typecheck(lhs) == Type::Float {
                        Instruction::GTF
                    } else {
                        Instruction::GT
                    },)
                );
            }
            Expression::Leq(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(if self.typecheck(lhs) == Type::Float {
                        Instruction::LEQF
                    } else {
                        Instruction::LEQ
                    })
                );
            }
            Expression::Geq(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(if self.typecheck(lhs) == Type::Float {
                        Instruction::GEQF
                    } else {
                        Instruction::GEQ
                    })
                );
            }
            Expression::Eq(lhs, rhs) => {
                binary!(bytecode, self, lhs, rhs, Byte::new(Instruction::EQ));
            }
            Expression::Not(lhs) => {
                unary!(bytecode, self, lhs, Byte::new(Instruction::NOT));
            }
            Expression::Negate(lhs) => {
                unary!(bytecode, self, lhs, Byte::new(Instruction::NEG));
            }
            Expression::Add(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(
                        //if likely(self.typecheck(lhs) == Type::Float) {
                        // Instruction::ADDF
                        // } else {
                        Instruction::ADD // },
                    )
                );
            }
            Expression::Sub(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(if likely(self.typecheck(lhs) == Type::Float) {
                        Instruction::SUBF
                    } else {
                        Instruction::SUB
                    },)
                );
            }
            Expression::Mul(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(if likely(self.typecheck(lhs) == Type::Float) {
                        Instruction::MULF
                    } else {
                        Instruction::MUL
                    },)
                );
            }
            Expression::Mod(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(if likely(self.typecheck(lhs) == Type::Float) {
                        Instruction::MODF
                    } else {
                        Instruction::MOD
                    },)
                );
            }
            Expression::Div(lhs, rhs) => {
                binary!(
                    bytecode,
                    self,
                    lhs,
                    rhs,
                    Byte::new(if likely(self.typecheck(lhs) == Type::Float) {
                        Instruction::DIVF
                    } else {
                        Instruction::DIV
                    },)
                );
            }
            Expression::And(lhs, rhs) => {
                binary!(bytecode, self, lhs, rhs, Byte::new(Instruction::AND));
            }
            Expression::Integer(num) => bytecode.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(*num).raw() as _,
            )),
            Expression::Bool(state) => bytecode.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(*state).raw() as _,
            )),
            Expression::Float(num) => bytecode.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(*num).raw() as _,
            )),
            Expression::String(str) => {
                let escaped = str
                    .replace("\\n", "\n")
                    .replace("\\r", "\r")
                    .replace("\\t", "\t")
                    .replace("\\0", "\0");

                // if let Ok(re) = Regex::new(r"\\u(?<code>\d{1,})").map_err(|e| dbg!(e)) {
                //     while let Some(captures) = re.captures(escaped.as_str()) {
                //         let unicode = captures.name("code").unwrap().as_str();
                //
                //         escaped = escaped.replace(
                //             format!("\\u{}", unicode).as_str(),
                //             char::from_u32(
                //                 captures
                //                     .name("code")
                //                     .unwrap()
                //                     .as_str()
                //                     .parse()
                //                     .unwrap_or_default(),
                //             )
                //             .unwrap_or_default()
                //             .to_string()
                //             .as_str(),
                //         )
                //     }
                // }
                let idx = bytecode.len();

                let mut count = 0;

                escaped.chars().inspect(|_| count += 1).for_each(|ch| {
                    bytecode.push(Byte::new(Instruction::DATA).with_operand_u32(ch.into()));
                });

                bytecode.insert(
                    idx,
                    Byte::new(Instruction::STRING).with_operand_u32(count as u32),
                );
            }
            Expression::Variable(name, ty_expr) => {
                // Register variable with HM typechecker for type inference
                if let Some(ty_expr) = ty_expr {
                    // Extract type from Output<'expr>
                    let ty_name = match ty_expr.1.borrow() {
                        Expression::Type(t) => t.1.to_string(),
                        _ => ty_expr.1.to_string(),
                    };
                    let t = Type::from(ty_name);
                    self.hm_typechecker.get_env_mut().define_variable(name, t);
                }

                if unlikely(self.context.variables.contains(&name.to_string())) {
                    let mut message =
                        Message::error("Variable redeclaration".to_string(), span.into_range());
                    message.push(Label::new(
                        format!("Variable '{}' already declared", name),
                        span.into_range(),
                    ));
                    self.messages.push(message);
                }

                self.context.variables.intern(name.to_string());
            }
            Expression::Constant(name, _ty) => {
                let name = self.resolve_variable(name);
                if self.context.variables.contains(&name) {
                    let mut message =
                        Message::error("Constand redeclaration".to_string(), span.into_range());
                    message.push(Label::new(
                        format!("Constant '{}' already declared", name),
                        span.into_range(),
                    ));
                    self.messages.push(message);
                }

                let symbol = self.context.variables.intern(name.clone());

                self.context.constants.insert(symbol, false);
            }
            Expression::Assignment(name, value) => {
                let name = self.resolve_variable(name);

                self.context.assignments.insert(name.clone(), true);

                // Typecheck the value to infer its type and register with HM typechecker
                let value_ty = self.typecheck(&(span.clone(), value.1.clone()));

                // @TODO: investigate the usage of `check_assignment` for type checking
                // self.hm_typechecker
                //     .check_assignment(&name, value_ty.clone(), span.clone())

                // Register variable with inferred type in HM typechecker
                self.hm_typechecker
                    .get_env_mut()
                    .define_variable(&name, value_ty.clone());

                // Register variable in context if not exists (for let statements)
                let symbol = if let Some(sym) = self.context.variables.key(&name) {
                    sym
                } else {
                    self.context.variables.intern(name.clone())
                };

                if unlikely(self.context.constants.contains_key(&symbol)) {
                    let assigned = likely(*self.context.constants.get(&symbol).unwrap());

                    if !assigned {
                        self.context.constants.entry(symbol).and_modify(|state| {
                            *state = true;
                        });
                    } else {
                        let mut message =
                            Message::error("Assignment error".to_string(), span.into_range());
                        message.push(Label::new(
                            format!(
                                "Unable to assign to an already assigned constant '{}'",
                                name
                            ),
                            span.into_range(),
                        ));
                        self.messages.push(message);
                    }
                }

                let mut expr = self.do_compile(value);

                bytecode.append(&mut expr);
                bytecode.push(Byte::new(Instruction::STORE).with_operand_u32(symbol as u32));

                // Do not pop if assigning to the same place
                if self.context.variables.len() == symbol + 1 {
                    bytecode.push(Byte::new(Instruction::DUPLICATE));
                }
            }
            Expression::TypedAssignment { name, ty, value } => {
                let name =
                    self.resolve_variable(&(span.clone(), Box::new(Expression::Identifier(name))));

                self.context.assignments.insert(name.clone(), true);

                // Typecheck the value to infer its type and register with HM typechecker
                let value_ty = self.typecheck(&(span.clone(), value.1.clone()));

                // Define variable with the expected type from annotation
                let expected_ty_name = match ty.1.borrow() {
                    parser::ast::Expression::Type(t) => t.1.to_string(),
                    _ => ty.1.to_string(),
                };
                let expected_ty = crate::types::Type::from(expected_ty_name);

                // Register variable with expected type in HM typechecker
                self.hm_typechecker
                    .get_env_mut()
                    .define_variable(&name, expected_ty);

                // Register variable in context if not exists
                let symbol = if let Some(sym) = self.context.variables.key(&name) {
                    sym
                } else {
                    self.context.variables.intern(name.clone())
                };

                let mut expr = self.do_compile(value);

                bytecode.append(&mut expr);
                bytecode.push(Byte::new(Instruction::STORE).with_operand_u32(symbol as u32));

                if self.context.variables.len() == symbol + 1 {
                    bytecode.push(Byte::new(Instruction::DUPLICATE));
                }
            }
            Expression::Match(lhs, children) => {
                let mut lhs_code = self.do_compile(lhs);
                bytecode.append(&mut lhs_code);

                let mut jumps: Vec<usize> = Vec::with_capacity(children.len());
                let last_idx = children.len() - 1;

                for child in children.iter() {
                    let expr = child.1.as_ref();
                    match expr {
                        Expression::MatchArm(pattern, body) => {
                            let is_condition =
                                !matches!(pattern.1.as_ref(), Expression::Default(_));

                            if !is_condition && jumps.len() != last_idx {
                                let mut message = Message::warn(
                                    "`default` branch should be at the end of expression"
                                        .to_string(),
                                    child.0.clone().into_range(),
                                );
                                message.push(Label::new(
                                    "Code after this block is not reachable".to_string(),
                                    child.0.clone().into_range(),
                                ));
                                message.with_help(
                                    "Maybe you need to move this to the bottom of the list?"
                                        .to_string(),
                                );
                                self.messages.push(message);
                            }

                            bytecode.push(Byte::new(Instruction::DUPLICATE));
                            let mut compiled_patterns = vec![];
                            if let Expression::List(patterns) = pattern.1.borrow() {
                                compiled_patterns =
                                    patterns.iter().map(|p| self.do_compile(p)).collect();
                            } else {
                                compiled_patterns.push(self.do_compile(pattern));
                            }

                            // Handle VariantWithDestructure patterns - bind field names as variables
                            // This is needed for match arms like: case Result::Ok(value) => { ... }
                            if let Expression::VariantWithDestructure(_ty, _name, fields) =
                                pattern.1.borrow()
                            {
                                // Bind each field as a variable in the current scope
                                for field in fields {
                                    if let Expression::Identifier(name) = field.1.borrow() {
                                        let var_name = name.to_string();
                                        self.context.variables.intern(var_name.clone());
                                        // Pop the variant from stack, keeping the field
                                        bytecode.push(Byte::new(Instruction::VARIANT_POP));
                                        // Store the field value into the variable
                                        let symbol = self
                                            .context
                                            .variables
                                            .key(&var_name)
                                            .expect("Field should be interned");
                                        bytecode.push(
                                            Byte::new(Instruction::STORE)
                                                .with_operand_u32(symbol as u32),
                                        );
                                    }
                                }
                            }

                            compiled_patterns.iter_mut().for_each(|mut pattern_code| {
                                bytecode.append(&mut pattern_code);
                                bytecode.push(Byte::new(Instruction::EQ));

                                let mut body_code = self.do_compile(&body);
                                if is_condition {
                                    bytecode.push(Byte::new(Instruction::JMPF).with_operand_u32(
                                        (self.bytecode.len() + bytecode.len() + body_code.len() + 2)
                                            as u32,
                                    ));
                                }
                                bytecode.append(&mut body_code);
                                jumps.push(bytecode.len());
                                bytecode
                                    .push(Byte::new(Instruction::JMP).with_operand_u32(u32::MAX));
                            });
                        }
                        _ => {
                            let mut message = Message::error(
                                "Invalid match arm".to_string(),
                                child.0.clone().into_range(),
                            );
                            message.push(Label::new(
                                "Match arm must be a case expression".to_string(),
                                child.0.clone().into_range(),
                            ));
                            self.messages.push(message);
                        }
                    }
                }

                let len = bytecode.len();
                jumps.iter().for_each(|jump| {
                    if let Some(instruction) = bytecode.get_mut(*jump) {
                        *instruction = Byte::new(Instruction::JMP)
                            .with_operand_u32((self.bytecode.len() + len) as u32);
                    }
                });
            }
            _expr => {
                let mut message =
                    Message::error("Unknown expression".to_string(), span.into_range());
                message.push(Label::new(
                    "Unable to compile expression".to_string(),
                    span.into_range(),
                ));
                self.messages.push(message);
                #[cfg(debug_assertions)]
                eprintln!("{}", _expr);
            }
        }

        bytecode
    }

    pub fn compile<'compiler>(
        &mut self,
        module: &str,
        ast: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> Vec<Byte> {
        let ns = self.namespace.clone();
        self.namespace = module.to_string();
        let mut program = self.do_compile(ast);
        self.namespace = ns.to_string();

        // HM typechecker messages are already collected in typecheck()

        self.bytecode.append(&mut program);

        self.bytecode.clone()
    }
}
