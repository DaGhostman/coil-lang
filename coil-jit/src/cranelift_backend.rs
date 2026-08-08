use std::{collections::HashMap, ffi::c_void};

use cranelift_codegen::ir::{self, Block, InstBuilder, condcodes::IntCC, types};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module};

use crate::{
    F64BinaryOp, F64Function, F64Instr, I64BinaryOp, I64CompareOp, I64Function, I64Instr, JitError,
    JitHelpers, JitValueKind,
};

pub struct JitEngine {
    module: JITModule,
    builder_context: FunctionBuilderContext,
    next_function: u64,
    array_len_id: Option<FuncId>,
    array_index_id: Option<FuncId>,
}

pub struct JitFunction {
    ptr: *const u8,
    arity: u8,
    kind: JitValueKind,
    has_context: bool,
}

impl JitEngine {
    pub fn new() -> Result<Self, JitError> {
        Self::new_with_helpers(JitHelpers::default())
    }

    pub fn new_with_helpers(helpers: JitHelpers) -> Result<Self, JitError> {
        let mut flag_builder = settings::builder();
        flag_builder
            .set("is_pic", "false")
            .map_err(|error| JitError::Backend(error.to_string()))?;
        let flags = settings::Flags::new(flag_builder);
        let isa = cranelift_native::builder()
            .map_err(|error| JitError::Backend(error.to_string()))?
            .finish(flags)
            .map_err(|error| JitError::Backend(error.to_string()))?;
        let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        if !helpers.array_len.is_null() {
            builder.symbol("coil_jit_array_len", helpers.array_len);
        }
        if !helpers.array_index.is_null() {
            builder.symbol("coil_jit_array_index", helpers.array_index);
        }
        Ok(Self {
            module: JITModule::new(builder),
            builder_context: FunctionBuilderContext::new(),
            next_function: 0,
            array_len_id: None,
            array_index_id: None,
        })
    }

    fn ensure_array_len(&mut self) -> Result<FuncId, JitError> {
        if let Some(id) = self.array_len_id {
            return Ok(id);
        }
        let mut signature = self.module.make_signature();
        signature.params.push(ir::AbiParam::new(types::I64));
        signature.params.push(ir::AbiParam::new(types::I64));
        signature.returns.push(ir::AbiParam::new(types::I64));
        let id = self
            .module
            .declare_function("coil_jit_array_len", Linkage::Import, &signature)
            .map_err(|error| JitError::Backend(error.to_string()))?;
        self.array_len_id = Some(id);
        Ok(id)
    }

    fn ensure_array_index(&mut self) -> Result<FuncId, JitError> {
        if let Some(id) = self.array_index_id {
            return Ok(id);
        }
        let mut signature = self.module.make_signature();
        signature.params.push(ir::AbiParam::new(types::I64));
        signature.params.push(ir::AbiParam::new(types::I64));
        signature.params.push(ir::AbiParam::new(types::I64));
        signature.returns.push(ir::AbiParam::new(types::I64));
        let id = self
            .module
            .declare_function("coil_jit_array_index", Linkage::Import, &signature)
            .map_err(|error| JitError::Backend(error.to_string()))?;
        self.array_index_id = Some(id);
        Ok(id)
    }

    pub fn compile_i64(
        &mut self,
        name: &str,
        function: &I64Function,
    ) -> Result<JitFunction, JitError> {
        function.validate()?;
        let needs_context = function.uses_context();
        let function_name = format!("coil_jit_{name}_{}", self.next_function);
        self.next_function += 1;

        let mut signature = self.module.make_signature();
        if needs_context {
            signature.params.push(ir::AbiParam::new(types::I64));
        }
        for _ in 0..function.params() {
            signature.params.push(ir::AbiParam::new(types::I64));
        }
        signature.returns.push(ir::AbiParam::new(types::I64));
        let function_id = self
            .module
            .declare_function(&function_name, Linkage::Local, &signature)
            .map_err(|error| JitError::Backend(error.to_string()))?;

        let mut context = self.module.make_context();
        context.func.signature = signature;
        let array_len_ref = if function
            .instructions()
            .iter()
            .any(|instruction| matches!(instruction, I64Instr::ArrayLen { .. }))
        {
            let id = self.ensure_array_len()?;
            Some(self.module.declare_func_in_func(id, &mut context.func))
        } else {
            None
        };
        let array_index_ref = if function
            .instructions()
            .iter()
            .any(|instruction| matches!(instruction, I64Instr::ArrayIndex { .. }))
        {
            let id = self.ensure_array_index()?;
            Some(self.module.declare_func_in_func(id, &mut context.func))
        } else {
            None
        };
        let self_ref = self
            .module
            .declare_func_in_func(function_id, &mut context.func);
        let mut builder = FunctionBuilder::new(&mut context.func, &mut self.builder_context);
        let entry_block = builder.create_block();
        let mut blocks: HashMap<u32, Block> = HashMap::new();
        blocks.insert(0, entry_block);
        for instruction in function.instructions() {
            if let I64Instr::Label { block } = instruction
                && *block != 0
            {
                blocks.insert(*block, builder.create_block());
            }
        }
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);

        let params = builder.block_params(entry_block).to_vec();
        let context_param = needs_context.then(|| params[0]);
        let mut values: HashMap<(u32, u8), ir::Value> = HashMap::new();
        let mut current_block_id = 0u32;
        for instruction in function.instructions() {
            match instruction {
                I64Instr::Label { block } => {
                    current_block_id = *block;
                    let target = blocks
                        .get(block)
                        .copied()
                        .ok_or_else(|| JitError::InvalidIr(format!("unknown block {block}")))?;
                    builder.switch_to_block(target);
                }
                I64Instr::LoadParam { dst, param } => {
                    let offset = usize::from(needs_context);
                    values.insert((current_block_id, *dst), params[*param as usize + offset]);
                }
                I64Instr::Const { dst, value } => {
                    let block = current_block_id;
                    values.insert((block, *dst), builder.ins().iconst(types::I64, *value));
                }
                I64Instr::Binary { dst, lhs, rhs, op } => {
                    let block = current_block_id;
                    let left = lookup_value(&values, block, *lhs)?;
                    let right = lookup_value(&values, block, *rhs)?;
                    let result = match op {
                        I64BinaryOp::Add => builder.ins().iadd(left, right),
                        I64BinaryOp::Sub => builder.ins().isub(left, right),
                        I64BinaryOp::Mul => builder.ins().imul(left, right),
                        I64BinaryOp::Div => builder.ins().sdiv(left, right),
                    };
                    values.insert((block, *dst), result);
                }
                I64Instr::Compare { dst, lhs, rhs, op } => {
                    let block = current_block_id;
                    let left = lookup_value(&values, block, *lhs)?;
                    let right = lookup_value(&values, block, *rhs)?;
                    let result = builder.ins().icmp(compare_condition(*op), left, right);
                    values.insert((block, *dst), result);
                }
                I64Instr::Jump { target } => {
                    let block = blocks
                        .get(target)
                        .copied()
                        .ok_or_else(|| JitError::InvalidIr(format!("unknown block {target}")))?;
                    builder.ins().jump(block, &[]);
                }
                I64Instr::Branch {
                    cond,
                    then_block,
                    else_block,
                } => {
                    let condition = lookup_value(&values, current_block_id, *cond)?;
                    let then_block = blocks.get(then_block).copied().ok_or_else(|| {
                        JitError::InvalidIr(format!("unknown block {then_block}"))
                    })?;
                    let else_block = blocks.get(else_block).copied().ok_or_else(|| {
                        JitError::InvalidIr(format!("unknown block {else_block}"))
                    })?;
                    builder
                        .ins()
                        .brif(condition, then_block, &[], else_block, &[]);
                }
                I64Instr::CallSelf { dst, args } => {
                    let block = current_block_id;
                    let arguments: Vec<_> = args
                        .iter()
                        .map(|register| lookup_value(&values, block, *register))
                        .collect::<Result<_, _>>()?;
                    let call = builder.ins().call(self_ref, &arguments);
                    let result = builder.inst_results(call)[0];
                    values.insert((block, *dst), result);
                }
                I64Instr::ArrayLen { dst, value } => {
                    let value = lookup_value(&values, current_block_id, *value)?;
                    let call = builder.ins().call(
                        array_len_ref.ok_or_else(|| {
                            JitError::Backend("array length helper is unavailable".into())
                        })?,
                        &[
                            context_param.ok_or_else(|| {
                                JitError::Backend("JIT context is unavailable".into())
                            })?,
                            value,
                        ],
                    );
                    let result = builder.inst_results(call)[0];
                    values.insert((current_block_id, *dst), result);
                }
                I64Instr::ArrayIndex { dst, array, index } => {
                    let array = lookup_value(&values, current_block_id, *array)?;
                    let index = lookup_value(&values, current_block_id, *index)?;
                    let call = builder.ins().call(
                        array_index_ref.ok_or_else(|| {
                            JitError::Backend("array index helper is unavailable".into())
                        })?,
                        &[
                            context_param.ok_or_else(|| {
                                JitError::Backend("JIT context is unavailable".into())
                            })?,
                            array,
                            index,
                        ],
                    );
                    let result = builder.inst_results(call)[0];
                    values.insert((current_block_id, *dst), result);
                }
                I64Instr::Return { value } => {
                    let result = lookup_value(&values, current_block_id, *value)?;
                    builder.ins().return_(&[result]);
                }
            }
        }
        for block in blocks.values().copied() {
            builder.seal_block(block);
        }
        builder.finalize(self.module.target_config());

        self.module
            .define_function(function_id, &mut context)
            .map_err(|error| JitError::Backend(error.to_string()))?;
        self.module.clear_context(&mut context);
        self.module
            .finalize_definitions()
            .map_err(|error| JitError::Backend(error.to_string()))?;
        let ptr = self.module.get_finalized_function(function_id);

        Ok(JitFunction {
            ptr,
            arity: function.params(),
            kind: JitValueKind::I64,
            has_context: needs_context,
        })
    }

    pub fn compile_i64_binary(
        &mut self,
        name: &str,
        op: I64BinaryOp,
    ) -> Result<JitFunction, JitError> {
        self.compile_i64(name, &I64Function::binary(op))
    }

    pub fn compile_f64(
        &mut self,
        name: &str,
        function: &F64Function,
    ) -> Result<JitFunction, JitError> {
        function.validate()?;
        let function_name = format!("coil_jit_{name}_{}", self.next_function);
        self.next_function += 1;

        let mut signature = self.module.make_signature();
        for _ in 0..function.params() {
            signature.params.push(ir::AbiParam::new(types::F64));
        }
        signature.returns.push(ir::AbiParam::new(types::F64));
        let function_id = self
            .module
            .declare_function(&function_name, Linkage::Local, &signature)
            .map_err(|error| JitError::Backend(error.to_string()))?;

        let mut context = self.module.make_context();
        context.func.signature = signature;
        let mut builder = FunctionBuilder::new(&mut context.func, &mut self.builder_context);
        let block = builder.create_block();
        builder.append_block_params_for_function_params(block);
        builder.switch_to_block(block);
        builder.seal_block(block);

        let params = builder.block_params(block).to_vec();
        let mut values = HashMap::new();
        for instruction in function.instructions() {
            match instruction {
                F64Instr::LoadParam { dst, param } => {
                    values.insert(*dst, params[*param as usize]);
                }
                F64Instr::Const { dst, value } => {
                    values.insert(*dst, builder.ins().f64const(*value));
                }
                F64Instr::Binary { dst, lhs, rhs, op } => {
                    let left = values[lhs];
                    let right = values[rhs];
                    let result = match op {
                        F64BinaryOp::Add => builder.ins().fadd(left, right),
                        F64BinaryOp::Sub => builder.ins().fsub(left, right),
                        F64BinaryOp::Mul => builder.ins().fmul(left, right),
                        F64BinaryOp::Div => builder.ins().fdiv(left, right),
                    };
                    values.insert(*dst, result);
                }
                F64Instr::Return { value } => {
                    builder.ins().return_(&[values[value]]);
                }
            }
        }
        builder.finalize(self.module.target_config());

        self.module
            .define_function(function_id, &mut context)
            .map_err(|error| JitError::Backend(error.to_string()))?;
        self.module.clear_context(&mut context);
        self.module
            .finalize_definitions()
            .map_err(|error| JitError::Backend(error.to_string()))?;
        let ptr = self.module.get_finalized_function(function_id);
        Ok(JitFunction {
            ptr,
            arity: function.params(),
            kind: JitValueKind::F64,
            has_context: false,
        })
    }

    pub fn compile_f64_binary(
        &mut self,
        name: &str,
        op: F64BinaryOp,
    ) -> Result<JitFunction, JitError> {
        self.compile_f64(name, &F64Function::binary(op))
    }

    pub fn compile_i64_binary_imm(
        &mut self,
        name: &str,
        op: I64BinaryOp,
        value: i64,
    ) -> Result<JitFunction, JitError> {
        self.compile_i64(name, &I64Function::binary_imm(op, value))
    }
}

fn compare_condition(op: I64CompareOp) -> IntCC {
    match op {
        I64CompareOp::Less => IntCC::SignedLessThan,
        I64CompareOp::LessEqual => IntCC::SignedLessThanOrEqual,
        I64CompareOp::Equal => IntCC::Equal,
    }
}

fn lookup_value(
    values: &HashMap<(u32, u8), ir::Value>,
    block: u32,
    register: u8,
) -> Result<ir::Value, JitError> {
    values
        .get(&(block, register))
        .or_else(|| values.get(&(0, register)))
        .copied()
        .ok_or_else(|| {
            JitError::InvalidIr(format!(
                "register r{register} is unavailable in block {block}"
            ))
        })
}

impl JitFunction {
    pub fn call1(&self, value: i64) -> i64 {
        assert_eq!(self.arity, 1, "call1 requires a one-parameter JIT function");
        assert_eq!(
            self.kind,
            JitValueKind::I64,
            "call1 requires an i64 function"
        );
        assert!(!self.has_context, "call1 requires a context-free function");
        let function: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(self.ptr) };
        function(value)
    }

    pub fn call1_with_context(&self, context: *mut c_void, value: i64) -> i64 {
        assert_eq!(
            self.arity, 1,
            "call1_with_context requires one value argument"
        );
        assert_eq!(
            self.kind,
            JitValueKind::I64,
            "call1_with_context requires i64"
        );
        assert!(
            self.has_context,
            "call1_with_context requires a context function"
        );
        let function: extern "C" fn(*mut c_void, i64) -> i64 =
            unsafe { std::mem::transmute(self.ptr) };
        function(context, value)
    }

    pub fn call2(&self, left: i64, right: i64) -> i64 {
        assert_eq!(self.arity, 2, "call2 requires a two-parameter JIT function");
        assert_eq!(
            self.kind,
            JitValueKind::I64,
            "call2 requires an i64 function"
        );
        assert!(!self.has_context, "call2 requires a context-free function");
        // The signature is fixed by `compile_i64`; callers must keep the
        // engine that produced this handle alive while invoking it.
        let function: extern "C" fn(i64, i64) -> i64 = unsafe { std::mem::transmute(self.ptr) };
        function(left, right)
    }

    pub fn call2_with_context(&self, context: *mut c_void, left: i64, right: i64) -> i64 {
        assert_eq!(
            self.arity, 2,
            "call2_with_context requires two value arguments"
        );
        assert_eq!(
            self.kind,
            JitValueKind::I64,
            "call2_with_context requires i64"
        );
        assert!(
            self.has_context,
            "call2_with_context requires a context function"
        );
        let function: extern "C" fn(*mut c_void, i64, i64) -> i64 =
            unsafe { std::mem::transmute(self.ptr) };
        function(context, left, right)
    }

    pub fn call2_f64(&self, left: f64, right: f64) -> f64 {
        assert_eq!(self.arity, 2, "call2_f64 requires a two-parameter function");
        assert_eq!(
            self.kind,
            JitValueKind::F64,
            "call2_f64 requires an f64 function"
        );
        assert!(
            !self.has_context,
            "call2_f64 requires a context-free function"
        );
        let function: extern "C" fn(f64, f64) -> f64 = unsafe { std::mem::transmute(self.ptr) };
        function(left, right)
    }
}
