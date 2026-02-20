mod hm_typechecker;
mod pipeline;
mod types;

use std::{borrow::Borrow, collections::HashMap};

pub use crate::types::ty::Type;
use common::{likely, unlikely, Byte, Instruction, Interner, Label, Message, Value};
use parser::{ast::Expression, SimpleSpan};
pub use pipeline::*;

use crate::hm_typechecker::HmTypeChecker;
use crate::types::ty::TypeVar;

/// Patch point in bytecode template - marks locations that need type-dependent modification
#[derive(Clone, Debug)]
pub struct PatchPoint {
    pub offset: usize,
    pub patch_type: PatchType,
}

#[derive(Clone, Debug)]
pub enum PatchType {
    ArithmeticOp {
        type_var_id: usize,
    },
    VariantTag {
        type_name: String,
        variant_name: String,
    },
    MatchBranch {
        type_name: String,
        variant_name: String,
    },
    CallArity {
        param_index: usize,
        type_var_ids: Vec<usize>,
    },
    NestedGenericCall {
        func_name: String,
        type_arg_type_var_ids: Vec<usize>,
    },
}

#[derive(Clone)]
pub struct BytecodeTemplate {
    pub name: String,
    pub type_params: Vec<TypeVar>,
    pub param_types: Vec<Type>,
    pub return_ty: Type,
    pub bytecode: Vec<Byte>,
    pub patch_points: Vec<PatchPoint>,
}

#[derive(Clone, Debug)]
pub struct FunctionBounds {
    pub name: String,
    pub start: usize,
    pub end: usize,
    pub is_generic: bool,
}

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
macro_rules! binary {
    ($result: expr, $self: expr, $lhs: expr, $rhs: expr, $instruction: expr) => {
        $result.append(&mut $self.do_compile($lhs));
        $result.append(&mut $self.do_compile($rhs));

        $result.push($instruction);
    };
}
macro_rules! binary_with_patch {
    ($result: expr, $self: expr, $lhs: expr, $rhs: expr, $instruction: expr, $type_var_id: expr) => {
        $result.append(&mut $self.do_compile($lhs));
        $result.append(&mut $self.do_compile($rhs));

        let offset = $result.len();
        $result.push($instruction);

        if let Some(tv_id) = $type_var_id {
            $self
                .context
                .add_patch_point(offset, PatchType::ArithmeticOp { type_var_id: tv_id });
        }
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
    type_params: Vec<TypeVar>,
    patch_points: Vec<PatchPoint>,
    compiling_template: bool,
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
    // Monomorphization support
    generic_templates: HashMap<String, BytecodeTemplate>,
    function_bounds: Vec<FunctionBounds>,
    instantiations: HashMap<String, usize>,
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
            generic_templates: HashMap::default(),
            function_bounds: Vec::default(),
            instantiations: HashMap::default(),
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
            type_params: self.type_params.clone(),
            patch_points: Vec::default(),
            compiling_template: self.compiling_template,
        }
    }

    fn clear(&mut self) {
        self.defers.clear();
        self.constants.clear();
        self.variables = Default::default();
        self.assignments.clear();
    }

    fn add_patch_point(&mut self, offset: usize, patch_type: PatchType) {
        if self.compiling_template {
            self.patch_points.push(PatchPoint { offset, patch_type });
        }
    }

    fn find_type_var(&self, name: &str) -> Option<&TypeVar> {
        self.type_params.iter().find(|tv| tv.name == name)
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

    fn extract_type_var_id(&self, ty: &Type) -> Option<usize> {
        match ty {
            Type::TypeVar(tv) => Some(tv.id),
            _ => None,
        }
    }

    fn emit_arithmetic_op<'a>(
        &mut self,
        bytecode: &mut Vec<Byte>,
        lhs: &'a (SimpleSpan, Box<Expression<'a>>),
        rhs: &'a (SimpleSpan, Box<Expression<'a>>),
        int_op: Instruction,
        float_op: Instruction,
    ) {
        bytecode.append(&mut self.do_compile(lhs));
        bytecode.append(&mut self.do_compile(rhs));

        let lhs_ty = self.typecheck(lhs);
        let is_float = lhs_ty == Type::Float;
        let op = if is_float { float_op } else { int_op };

        let offset = bytecode.len();
        bytecode.push(Byte::new(op));

        if let Some(tv) = self.extract_type_var_id(&lhs_ty) {
            self.context
                .add_patch_point(offset, PatchType::ArithmeticOp { type_var_id: tv });
        }
    }

    fn extract_param_types<'a>(&self, args: &(SimpleSpan, Box<Expression<'a>>)) -> Vec<Type> {
        let mut param_types = Vec::new();
        if let Expression::Fragment(arg_list) = args.1.borrow() {
            for arg in arg_list.iter() {
                if let Expression::Argument(ty_expr, _var_expr) = arg.1.borrow() {
                    let ty_name = match ty_expr.1.borrow() {
                        Expression::Type(t) => t.1.to_string(),
                        _ => ty_expr.1.to_string(),
                    };
                    param_types.push(Type::from(ty_name));
                }
            }
        }
        param_types
    }

    fn instantiate_generic(&mut self, name: &str, type_args: &[Type]) -> Option<usize> {
        let type_args_str: Vec<String> = type_args.iter().map(|t| t.type_name()).collect();
        let key = format!("{}<{}>", name, type_args_str.join(", "));

        if let Some(&offset) = self.instantiations.get(&key) {
            return Some(offset);
        }

        let template = self.generic_templates.get(name)?.clone();

        if template.type_params.len() != type_args.len() {
            return None;
        }

        let subst: HashMap<usize, Type> = template
            .type_params
            .iter()
            .zip(type_args.iter())
            .map(|(tv, ty)| (tv.id, ty.clone()))
            .collect();

        let start = self.bytecode.len();

        let mut patched_bytecode = template.bytecode.clone();
        let patch_points = template.patch_points.clone();

        self.apply_type_patches(
            &mut patched_bytecode,
            &subst,
            &template.param_types,
            &patch_points,
        );

        self.resolve_nested_generics(&mut patched_bytecode, &subst, &patch_points);

        self.bytecode.extend(patched_bytecode);

        let end = self.bytecode.len();

        self.function_bounds.push(FunctionBounds {
            name: key.clone(),
            start,
            end,
            is_generic: true,
        });

        self.instantiations.insert(key, start);
        Some(start)
    }

    fn type_stack_size(&self, ty: &Type) -> usize {
        match ty {
            Type::SumType { variants, .. } => {
                1 + variants.iter().map(|v| v.fields.len()).max().unwrap_or(0)
            }
            Type::TypeVar(_) => 1,
            _ => 1,
        }
    }

    fn calculate_param_arity(&self, param_types: &[Type]) -> usize {
        param_types.iter().map(|t| self.type_stack_size(t)).sum()
    }

    fn apply_type_patches(
        &mut self,
        bytecode: &mut [Byte],
        subst: &HashMap<usize, Type>,
        param_types: &[Type],
        patch_points: &[PatchPoint],
    ) {
        let concrete_param_types: Vec<Type> = param_types
            .iter()
            .map(|ty| self.substitute_type(ty, subst))
            .collect();

        for patch in patch_points {
            if patch.offset >= bytecode.len() {
                continue;
            }

            let byte = &mut bytecode[patch.offset];

            match &patch.patch_type {
                PatchType::ArithmeticOp { type_var_id } => {
                    if let Some(concrete_ty) = subst.get(type_var_id) {
                        let current_op = byte.bytecode().clone();
                        let new_op = match (current_op, concrete_ty) {
                            (Instruction::ADD, Type::Float) => Instruction::ADDF,
                            (Instruction::SUB, Type::Float) => Instruction::SUBF,
                            (Instruction::MUL, Type::Float) => Instruction::MULF,
                            (Instruction::DIV, Type::Float) => Instruction::DIVF,
                            (Instruction::MOD, Type::Float) => Instruction::MODF,
                            (Instruction::ADDF, Type::Int) => Instruction::ADD,
                            (Instruction::SUBF, Type::Int) => Instruction::SUB,
                            (Instruction::MULF, Type::Int) => Instruction::MUL,
                            (Instruction::DIVF, Type::Int) => Instruction::DIV,
                            (Instruction::MODF, Type::Int) => Instruction::MOD,
                            _ => current_op,
                        };
                        *byte = Byte::new(new_op).with_operand_u32(byte.operand_u32());
                    }
                }
                PatchType::CallArity {
                    param_index,
                    type_var_ids,
                } => {
                    let mut extra_arity = 0usize;
                    for type_var_id in type_var_ids {
                        if let Some(concrete_ty) = subst.get(type_var_id) {
                            extra_arity += self.type_stack_size(concrete_ty).saturating_sub(1);
                        }
                    }

                    if extra_arity > 0 {
                        let current_arity = byte.operand_u32() as usize;
                        let new_arity = current_arity + extra_arity;
                        *byte = Byte::new(Instruction::CALL).with_operand_u32(new_arity as u32);
                    }
                }
                PatchType::VariantTag {
                    type_name,
                    variant_name,
                } => {
                    if let Some(discriminants) = self.variant_discriminants.get(type_name) {
                        if let Some(&discriminant) = discriminants.get(variant_name) {
                            *byte = Byte::new_with_value(
                                Instruction::CONST,
                                Value::from(discriminant).raw() as _,
                            );
                        }
                    }
                }
                PatchType::MatchBranch {
                    type_name,
                    variant_name,
                } => {
                    if let Some(discriminants) = self.variant_discriminants.get(type_name) {
                        if let Some(&discriminant) = discriminants.get(variant_name) {
                            let offset = byte.operand_u32();
                            *byte = Byte::new(Instruction::MATCH_BRANCH)
                                .with_operands_u16([discriminant as u16, (offset >> 16) as u16]);
                        }
                    }
                }
                PatchType::NestedGenericCall {
                    func_name,
                    type_arg_type_var_ids,
                } => {
                    // Note: Nested generic calls are handled in a second pass
                    // by resolve_nested_generics() after initial patching
                    // Here we just mark that this needs resolution
                    // The actual patching happens after instantiate_generic completes
                    let _ = (func_name, type_arg_type_var_ids);
                }
            }
        }
    }

    fn resolve_nested_generics(
        &mut self,
        bytecode: &mut [Byte],
        subst: &HashMap<usize, Type>,
        patch_points: &[PatchPoint],
    ) {
        for patch in patch_points {
            if patch.offset >= bytecode.len() {
                continue;
            }

            if let PatchType::NestedGenericCall {
                func_name,
                type_arg_type_var_ids,
            } = &patch.patch_type
            {
                let concrete_type_args: Vec<Type> = type_arg_type_var_ids
                    .iter()
                    .filter_map(|tv_id| subst.get(tv_id).cloned())
                    .collect();

                if concrete_type_args.len() == type_arg_type_var_ids.len() {
                    if let Some(nested_offset) =
                        self.instantiate_generic(func_name, &concrete_type_args)
                    {
                        let byte = &mut bytecode[patch.offset];
                        *byte = Byte::new(Instruction::JMP).with_operand_u32(nested_offset as u32);
                    }
                }
            }
        }
    }

    fn substitute_type(&self, ty: &Type, subst: &HashMap<usize, Type>) -> Type {
        match ty {
            Type::TypeVar(tv) => subst.get(&tv.id).cloned().unwrap_or_else(|| ty.clone()),
            Type::Function(params, ret) => Type::Function(
                params
                    .iter()
                    .map(|p| self.substitute_type(p, subst))
                    .collect(),
                Box::new(self.substitute_type(ret, subst)),
            ),
            Type::Array(inner) => Type::Array(Box::new(self.substitute_type(inner, subst))),
            Type::Tuple(types) => Type::Tuple(
                types
                    .iter()
                    .map(|t| self.substitute_type(t, subst))
                    .collect(),
            ),
            Type::SumType {
                name,
                type_params,
                variants,
            } => {
                let new_type_params: Vec<TypeVar> = type_params
                    .iter()
                    .map(|tp| {
                        if let Some(Type::TypeVar(new_tv)) = subst.get(&tp.id) {
                            new_tv.clone()
                        } else {
                            tp.clone()
                        }
                    })
                    .collect();
                let new_variants: Vec<crate::types::ty::Variant> = variants
                    .iter()
                    .map(|v| {
                        let mut new_variant = v.clone();
                        new_variant.fields = v
                            .fields
                            .iter()
                            .map(|f| {
                                crate::types::ty::Field::new(
                                    &f.name,
                                    self.substitute_type(&f.ty, subst),
                                )
                            })
                            .collect();
                        new_variant
                    })
                    .collect();
                Type::SumType {
                    name: name.clone(),
                    type_params: new_type_params,
                    variants: new_variants,
                }
            }
            _ => ty.clone(),
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
                self.hm_typechecker.check(ast).ok();

                let func_name = format!("{}{}", self.namespace, name);
                let start = self.bytecode.len();

                self.functions.insert(func_name.clone(), start);

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

                let end = self.bytecode.len();

                self.function_bounds.push(FunctionBounds {
                    name: func_name,
                    start,
                    end,
                    is_generic: false,
                });

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
                generics,
            } => {
                let (_env, _constraints, _counter) = self.hm_typechecker.reset();
                self.context = self.context.child();
                self.context.clear();
                self.hm_typechecker.check(ast).ok();

                let type_params: Vec<TypeVar> = generics
                    .iter()
                    .enumerate()
                    .map(|(i, (gen_name, bounds))| {
                        let bounds_vec: Vec<crate::types::ty::TypeBound> = bounds
                            .iter()
                            .map(|b| crate::types::ty::TypeBound::new(b))
                            .collect();
                        TypeVar::new(i, gen_name).with_bounds(bounds_vec)
                    })
                    .collect();

                self.context.type_params = type_params.clone();
                self.context.compiling_template = true;

                let param_types = self.extract_param_types(args);
                let return_ty = _returns
                    .as_ref()
                    .map(|r| self.typecheck(r))
                    .unwrap_or(Type::Void);

                let start = self.bytecode.len();

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

                let end = self.bytecode.len();

                let template_bytecode = self.bytecode[start..end].to_vec();
                let patch_points = self.context.patch_points.clone();

                self.bytecode.truncate(start);

                let template = BytecodeTemplate {
                    name: name.to_string(),
                    type_params: type_params.clone(),
                    param_types: param_types.clone(),
                    return_ty: return_ty.clone(),
                    bytecode: template_bytecode,
                    patch_points,
                };

                self.generic_templates
                    .insert(format!("{}{}", self.namespace, name), template);

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
            Expression::SumType(name, _type_params, variants) => {
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

                    bytecode.push(
                        Byte::new(Instruction::CALL).with_operand_u32(
                            args.as_ref()
                                .map(|items| {
                                    items.len()
                                        + items
                                            .iter()
                                            .map(|i| {
                                                if let Expression::VariantWithDestructure(
                                                    _,
                                                    _,
                                                    variants,
                                                ) = i.1.borrow()
                                                {
                                                    variants.len()
                                                } else {
                                                    0
                                                }
                                            })
                                            .sum::<usize>()
                                })
                                .unwrap_or(0) as u32,
                        ),
                    );
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
                type_args,
                args,
            } => {
                let identifier = self.resolve_variable(name);
                let full_name = format!("{}{}", self.namespace, identifier);
                let n = self
                    .aliases
                    .get(&identifier)
                    .cloned()
                    .unwrap_or_else(|| identifier.clone());
                let template_name = self
                    .aliases
                    .get(&identifier)
                    .cloned()
                    .unwrap_or_else(|| full_name.clone());

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

                let type_var_ids: Vec<usize> = type_arg_types
                    .iter()
                    .filter_map(|ty| self.extract_type_var_id(ty))
                    .collect();

                let is_nested_generic = self.context.compiling_template
                    && self.generic_templates.contains_key(&template_name);

                if is_nested_generic && !type_var_ids.is_empty() {
                    if let Some(args) = args {
                        for arg in args {
                            bytecode.append(&mut self.do_compile(arg));
                        }
                    }

                    let arity = args.as_ref().map(|items| items.len()).unwrap_or(0);

                    let call_offset = bytecode.len();
                    bytecode.push(Byte::new(Instruction::CALL).with_operand_u32(arity as u32));

                    let jmp_offset = bytecode.len();
                    bytecode.push(Byte::new(Instruction::JMP).with_operand_u32(0));

                    self.context.add_patch_point(
                        jmp_offset,
                        PatchType::NestedGenericCall {
                            func_name: template_name,
                            type_arg_type_var_ids: type_var_ids,
                        },
                    );
                } else if let Some(offset) =
                    self.instantiate_generic(&template_name, &type_arg_types)
                {
                    if let Some(args) = args {
                        for arg in args {
                            bytecode.append(&mut self.do_compile(arg));
                        }
                    }

                    let arity = args
                        .as_ref()
                        .map(|items| {
                            items.len()
                                + items
                                    .iter()
                                    .map(|i| {
                                        if let Expression::VariantWithDestructure(_, _, variants) =
                                            i.1.borrow()
                                        {
                                            variants.len()
                                        } else {
                                            0
                                        }
                                    })
                                    .sum::<usize>()
                        })
                        .unwrap_or(0);

                    bytecode.push(Byte::new(Instruction::CALL).with_operand_u32(arity as u32));
                    bytecode.push(Byte::new(Instruction::JMP).with_operand_u32(offset as u32));
                } else if self.functions.get(&n).is_some() {
                    if let Some(args) = args {
                        for arg in args {
                            bytecode.append(&mut self.do_compile(arg));
                        }
                    }

                    let offset = self.functions[&n];
                    bytecode.push(Byte::new(Instruction::CALL).with_operand_u32(
                        args.as_ref().map(|items| items.len()).unwrap_or(0) as u32,
                    ));
                    bytecode.push(Byte::new(Instruction::JMP).with_operand_u32(offset as u32));
                } else if self.native.get(&n).is_some() {
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
                self.emit_arithmetic_op(
                    &mut bytecode,
                    lhs,
                    rhs,
                    Instruction::ADD,
                    Instruction::ADDF,
                );
            }
            Expression::Sub(lhs, rhs) => {
                self.emit_arithmetic_op(
                    &mut bytecode,
                    lhs,
                    rhs,
                    Instruction::SUB,
                    Instruction::SUBF,
                );
            }
            Expression::Mul(lhs, rhs) => {
                self.emit_arithmetic_op(
                    &mut bytecode,
                    lhs,
                    rhs,
                    Instruction::MUL,
                    Instruction::MULF,
                );
            }
            Expression::Mod(lhs, rhs) => {
                self.emit_arithmetic_op(
                    &mut bytecode,
                    lhs,
                    rhs,
                    Instruction::MOD,
                    Instruction::MODF,
                );
            }
            Expression::Div(lhs, rhs) => {
                self.emit_arithmetic_op(
                    &mut bytecode,
                    lhs,
                    rhs,
                    Instruction::DIV,
                    Instruction::DIVF,
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

                self.bytecode.append(&mut expr);
                self.bytecode
                    .push(Byte::new(Instruction::STORE).with_operand_u32(symbol as u32));

                // Do not pop if assigning to the same place
                // if self.context.variables.len() == symbol + 1 {
                //     self.bytecode.push(Byte::new(Instruction::DUPLICATE));
                // }
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

                self.bytecode.append(&mut expr);
                self.bytecode
                    .push(Byte::new(Instruction::STORE).with_operand_u32(symbol as u32));

                // if self.context.variables.len() == symbol + 1 {
                //     bytecode.push(Byte::new(Instruction::DUPLICATE));
                // }
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
                            let is_default = matches!(pattern.1.as_ref(), Expression::Default(_));

                            if is_default && jumps.len() != last_idx {
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

                            if let Expression::VariantWithDestructure(_ty, _name, fields) =
                                pattern.1.borrow()
                            {
                                let type_name = match _ty.1.borrow() {
                                    Expression::Type(t) => t.1.to_string(),
                                    _ => _ty.1.to_string(),
                                };
                                let var_name = match _name.1.borrow() {
                                    Expression::Identifier(n) => n.to_string(),
                                    _ => _name.1.to_string(),
                                };

                                if let Some(discriminants) =
                                    self.variant_discriminants.get(&type_name)
                                {
                                    if let Some(discriminant) = discriminants.get(&var_name) {
                                        for field in fields.iter() {
                                            if let Expression::Identifier(name) = field.1.borrow() {
                                                let var_name = name.to_string();
                                                self.context.variables.intern(var_name.clone());
                                            }
                                        }

                                        bytecode.push(Byte::new(Instruction::DUPLICATE));
                                        bytecode.push(Byte::new_with_value(
                                            Instruction::CONST,
                                            Value::from(*discriminant).raw() as _,
                                        ));
                                        bytecode.push(Byte::new(Instruction::EQ));

                                        let mut body_code = self.do_compile(&body);
                                        let jmpf_target = self.bytecode.len()
                                            + bytecode.len()
                                            + body_code.len()
                                            + 2
                                            + 2; // field_extraction_count;

                                        bytecode.push(
                                            Byte::new(Instruction::JMPF)
                                                .with_operand_u32(jmpf_target as u32),
                                        );

                                        bytecode.append(&mut body_code);
                                        bytecode.push(Byte::new(Instruction::POP));
                                        jumps.push(bytecode.len());
                                        bytecode.push(
                                            Byte::new(Instruction::JMP).with_operand_u32(u32::MAX),
                                        );
                                    } else {
                                        let mut message = Message::error(
                                            format!(
                                                "Unknown variant '{}::{}'",
                                                type_name, var_name
                                            ),
                                            child.0.clone().into_range(),
                                        );
                                        self.messages.push(message);
                                    }
                                } else {
                                    let mut message = Message::error(
                                        format!("Unknown type '{}'", type_name),
                                        child.0.clone().into_range(),
                                    );
                                    self.messages.push(message);
                                }
                            } else {
                                let mut compiled_patterns = vec![];
                                if let Expression::List(patterns) = pattern.1.borrow() {
                                    compiled_patterns =
                                        patterns.iter().map(|p| self.do_compile(p)).collect();
                                } else {
                                    compiled_patterns.push(self.do_compile(pattern));
                                }

                                compiled_patterns.iter_mut().for_each(|mut pattern_code| {
                                    bytecode.push(Byte::new(Instruction::DUPLICATE));
                                    bytecode.append(&mut pattern_code);
                                    bytecode.push(Byte::new(Instruction::EQ));

                                    let mut body_code = self.do_compile(&body);
                                    // The magic 2, accounting for the JMPF & JMP being inserted
                                    // after the count has been taken
                                    let jmpf_target =
                                        self.bytecode.len() + bytecode.len() + body_code.len() + 3;
                                    if !is_default {
                                        bytecode.push(
                                            Byte::new(Instruction::JMPF)
                                                .with_operand_u32(jmpf_target as _),
                                        );
                                    }
                                    bytecode.append(&mut body_code);
                                    bytecode.push(Byte::new(Instruction::POP));
                                    jumps.push(bytecode.len());
                                    bytecode.push(
                                        Byte::new(Instruction::JMP).with_operand_u32(u32::MAX),
                                    );
                                });
                            }
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

                bytecode.push(Byte::new(Instruction::POP));
                // dbg!(&jumps);
                jumps.iter().for_each(|jump| {
                    let len = bytecode.len();
                    if let Some(instruction) = bytecode.get_mut(*jump) {
                        *instruction = Byte::new(Instruction::JMP)
                            .with_operand_u32((self.bytecode.len() + len) as u32);
                    }
                });
                self.bytecode.append(&mut bytecode);
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
