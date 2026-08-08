use std::{collections::HashMap, ffi::c_void};

use coil_jit::{
    F64BinaryOp, HotCounters, I64BinaryOp, I64CompareOp, I64Function, I64Instr, JitConfig,
    JitEngine, JitFunction, JitHelpers, translate_i64_bytecode,
};
#[cfg(test)]
use common::ArchivedInstruction;
use common::{ArchivedByte as Byte, Instruction, Value};

pub struct JitRuntime {
    config: JitConfig,
    counters: HotCounters,
    engine: JitEngine,
    functions: HashMap<u32, JitFunction>,
}

#[repr(C)]
pub struct JitCallContext {
    pub heap: *mut crate::memory::Heap,
}

extern "C" fn jit_array_len(context: *mut c_void, raw: i64) -> i64 {
    let context = unsafe { &*(context.cast::<JitCallContext>()) };
    let heap = unsafe { &*context.heap };
    match heap.find_object_by_addr(raw as u64) {
        Some(crate::memory::Object::Array(gc)) => gc.as_ref().elements.len() as i64,
        Some(crate::memory::Object::Tuple(gc)) => gc.as_ref().elements.len() as i64,
        Some(crate::memory::Object::String(gc)) => gc.as_ref().data.len() as i64,
        Some(crate::memory::Object::Instance(gc)) => gc.as_ref().iter_fields().count() as i64,
        _ => 0,
    }
}

extern "C" fn jit_array_index(context: *mut c_void, raw: i64, index: i64) -> i64 {
    let context = unsafe { &*(context.cast::<JitCallContext>()) };
    let heap = unsafe { &*context.heap };
    let Some(object) = heap.find_object_by_addr(raw as u64) else {
        return -1;
    };
    if index < 0 {
        return -1;
    }
    let index = index as usize;
    match object {
        crate::memory::Object::Array(gc) => {
            let elements = &gc.as_ref().elements;
            if index < elements.len() {
                elements[index].raw() as u64 as i64
            } else {
                -1
            }
        }
        crate::memory::Object::Tuple(gc) => {
            let elements = &gc.as_ref().elements;
            if index < elements.len() {
                elements[index].raw() as u64 as i64
            } else {
                -1
            }
        }
        _ => -1,
    }
}

#[derive(Copy, Clone)]
enum DirectBinaryOp {
    I64(I64BinaryOp),
    F64(F64BinaryOp),
}

impl JitRuntime {
    pub fn new(config: JitConfig) -> Result<Self, String> {
        let engine = JitEngine::new_with_helpers(JitHelpers {
            array_len: jit_array_len as *const u8,
            array_index: jit_array_index as *const u8,
        })
        .map_err(|error| error.to_string())?;
        Ok(Self {
            config,
            counters: HotCounters::default(),
            engine,
            functions: HashMap::new(),
        })
    }

    pub fn reset_program(&mut self) {
        self.counters = HotCounters::default();
        self.functions.clear();
    }

    pub fn compiled_count(&self) -> usize {
        self.functions.len()
    }

    fn run_translated_i64(
        &mut self,
        target: u32,
        function_ir: &I64Function,
        args: &[Value],
        context: Option<*mut c_void>,
    ) -> Option<Value> {
        let function = if let Some(function) = self.functions.get(&target) {
            function
        } else {
            if !self.counters.record_entry(target, &self.config) {
                return None;
            }
            let function = self
                .engine
                .compile_i64(&format!("translated_pc_{target}"), function_ir)
                .ok()?;
            self.counters.mark_compiled(target);
            self.functions.entry(target).or_insert(function)
        };
        let raw = match (args, function_ir.uses_context(), context) {
            ([value], false, _) => function.call1(value.as_int()),
            ([left, right], false, _) => function.call2(left.as_int(), right.as_int()),
            ([value], true, Some(context)) => {
                function.call1_with_context(context, value.raw() as u64 as i64)
            }
            _ => return None,
        };
        Some(Value::from(raw as u64))
    }

    /// Try the first supported native shape: `BinSlotSlot; RETURN` for two
    /// integer arguments in slots zero and one.
    pub fn try_direct_binary(
        &mut self,
        code: &[Byte],
        target: u32,
        left: Value,
        right: Value,
    ) -> Option<Value> {
        if let Some(function) = translate_i64_bytecode(code, target as usize)
            && function.params() == 2
        {
            return self.run_translated_i64(target, &function, &[left, right], None);
        }
        let op = binary_shape(code, target as usize)?;
        let function = if let Some(function) = self.functions.get(&target) {
            function
        } else {
            if !self.counters.record_entry(target, &self.config) {
                return None;
            }
            let function = match op {
                DirectBinaryOp::I64(op) => self
                    .engine
                    .compile_i64_binary(&format!("pc_{target}"), op)
                    .ok()?,
                DirectBinaryOp::F64(op) => self
                    .engine
                    .compile_f64_binary(&format!("pc_{target}"), op)
                    .ok()?,
            };
            self.counters.mark_compiled(target);
            self.functions.entry(target).or_insert(function)
        };
        Some(match op {
            DirectBinaryOp::I64(_) => Value::from(function.call2(left.as_int(), right.as_int())),
            DirectBinaryOp::F64(_) => {
                Value::from(function.call2_f64(left.as_float(), right.as_float()))
            }
        })
    }

    pub fn try_direct_unary(&mut self, code: &[Byte], target: u32, value: Value) -> Option<Value> {
        if let Some(function) = translate_i64_bytecode(code, target as usize)
            && function.params() == 1
            && !function.uses_context()
        {
            return self.run_translated_i64(target, &function, &[value], None);
        }
        let (op, imm) = immediate_shape(code, target as usize)?;
        let function = if let Some(function) = self.functions.get(&target) {
            function
        } else {
            if !self.counters.record_entry(target, &self.config) {
                return None;
            }
            let function = self
                .engine
                .compile_i64_binary_imm(&format!("pc_{target}"), op, imm)
                .ok()?;
            self.counters.mark_compiled(target);
            self.functions.entry(target).or_insert(function)
        };
        Some(Value::from(function.call1(value.as_int())))
    }

    pub fn try_array_len(
        &mut self,
        code: &[Byte],
        target: u32,
        context: *mut c_void,
        value: Value,
    ) -> Option<Value> {
        let function = translate_i64_bytecode(code, target as usize)?;
        if !function.uses_context() || function.params() != 1 {
            return None;
        }
        self.run_translated_i64(target, &function, &[value], Some(context))
    }

    pub fn try_array_index_const(
        &mut self,
        code: &[Byte],
        target: u32,
        context: *mut c_void,
        value: Value,
    ) -> Option<Value> {
        let function = translate_i64_bytecode(code, target as usize)?;
        if !function.uses_context() || function.params() != 1 {
            return None;
        }
        self.run_translated_i64(target, &function, &[value], Some(context))
    }

    pub fn try_recursive_fib(
        &mut self,
        code: &[Byte],
        constants: &[u64],
        target: u32,
        value: Value,
    ) -> Option<Value> {
        let function_ir = recursive_fib_shape(code, constants, target as usize)?;
        let function = if let Some(function) = self.functions.get(&target) {
            function
        } else {
            if !self.counters.record_entry(target, &self.config) {
                return None;
            }
            let function = self
                .engine
                .compile_i64(&format!("fib_pc_{target}"), &function_ir)
                .ok()?;
            self.counters.mark_compiled(target);
            self.functions.entry(target).or_insert(function)
        };
        Some(Value::from(function.call1(value.as_int())))
    }
}

fn binary_shape(code: &[Byte], target: usize) -> Option<DirectBinaryOp> {
    let op = code.get(target)?;
    let ret = code.get(target + 1)?;
    if *op.bytecode() != Instruction::BinSlotSlot || *ret.bytecode() != Instruction::RETURN {
        return None;
    }
    let (raw_op, left, right) = op.bin_slot_slot_parts();
    if left != 0 || right != 1 {
        return None;
    }
    match Instruction::from(raw_op) {
        Instruction::ADD => Some(DirectBinaryOp::I64(I64BinaryOp::Add)),
        Instruction::SUB => Some(DirectBinaryOp::I64(I64BinaryOp::Sub)),
        Instruction::MUL => Some(DirectBinaryOp::I64(I64BinaryOp::Mul)),
        Instruction::DIV => Some(DirectBinaryOp::I64(I64BinaryOp::Div)),
        Instruction::ADDF => Some(DirectBinaryOp::F64(F64BinaryOp::Add)),
        Instruction::SUBF => Some(DirectBinaryOp::F64(F64BinaryOp::Sub)),
        Instruction::MULF => Some(DirectBinaryOp::F64(F64BinaryOp::Mul)),
        Instruction::DIVF => Some(DirectBinaryOp::F64(F64BinaryOp::Div)),
        _ => None,
    }
}

fn immediate_shape(code: &[Byte], target: usize) -> Option<(I64BinaryOp, i64)> {
    let op = code.get(target)?;
    let ret = code.get(target + 1)?;
    if *op.bytecode() != Instruction::BinSlotImm || *ret.bytecode() != Instruction::RETURN {
        return None;
    }
    let (raw_op, slot, imm) = op.bin_slot_imm_parts();
    if slot != 0 {
        return None;
    }
    let op = match Instruction::from(raw_op) {
        Instruction::ADD => I64BinaryOp::Add,
        Instruction::SUB => I64BinaryOp::Sub,
        Instruction::MUL => I64BinaryOp::Mul,
        Instruction::DIV => I64BinaryOp::Div,
        _ => return None,
    };
    Some((op, imm as i64))
}

fn recursive_fib_shape(code: &[Byte], constants: &[u64], target: usize) -> Option<I64Function> {
    let branch = code.get(target)?;
    if *branch.bytecode() != Instruction::BinSlotImmJmpf {
        return None;
    }
    let (raw_op, slot, pool_idx) = branch.bin_slot_imm_jmpf_parts();
    if Instruction::from(raw_op) != Instruction::LEQ || slot != 0 {
        return None;
    }
    let packed = *constants.get(pool_idx)?;
    let immediate = packed as u32 as i32 as i64;
    let false_target = (packed >> 32) as usize;
    if immediate != 2 || false_target != target.checked_add(2)? {
        return None;
    }
    if code.get(target + 1)?.bytecode() != &Instruction::ConstReturnImm
        || code.get(target + 1)?.operand_u32() != 1
    {
        return None;
    }
    if !bin_slot_imm_at(code, target + 2, Instruction::SUB, 0, 1)
        || !call_at(code, target + 3, 1, target)
        || !store_at(code, target + 4, 1)
        || !bin_slot_imm_at(code, target + 5, Instruction::SUB, 0, 2)
        || !call_at(code, target + 6, 1, target)
        || !store_at(code, target + 7, 2)
        || !bin_slot_slot_at(code, target + 8, Instruction::ADD, 1, 2)
        || code.get(target + 9)?.bytecode() != &Instruction::RETURN
    {
        return None;
    }

    Some(I64Function::new(
        1,
        vec![
            I64Instr::LoadParam { dst: 0, param: 0 },
            I64Instr::Const { dst: 1, value: 1 },
            I64Instr::Compare {
                dst: 2,
                lhs: 0,
                rhs: 1,
                op: I64CompareOp::LessEqual,
            },
            I64Instr::Branch {
                cond: 2,
                then_block: 1,
                else_block: 2,
            },
            I64Instr::Label { block: 1 },
            I64Instr::Const { dst: 3, value: 1 },
            I64Instr::Return { value: 3 },
            I64Instr::Label { block: 2 },
            I64Instr::Const { dst: 4, value: 1 },
            I64Instr::Binary {
                dst: 5,
                lhs: 0,
                rhs: 4,
                op: I64BinaryOp::Sub,
            },
            I64Instr::CallSelf {
                dst: 6,
                args: vec![5],
            },
            I64Instr::Const { dst: 7, value: 2 },
            I64Instr::Binary {
                dst: 8,
                lhs: 0,
                rhs: 7,
                op: I64BinaryOp::Sub,
            },
            I64Instr::CallSelf {
                dst: 9,
                args: vec![8],
            },
            I64Instr::Binary {
                dst: 10,
                lhs: 6,
                rhs: 9,
                op: I64BinaryOp::Add,
            },
            I64Instr::Return { value: 10 },
        ],
    ))
}

fn bin_slot_imm_at(
    code: &[Byte],
    index: usize,
    expected_op: Instruction,
    expected_slot: usize,
    expected_imm: i64,
) -> bool {
    let Some(byte) = code.get(index) else {
        return false;
    };
    if *byte.bytecode() != Instruction::BinSlotImm {
        return false;
    }
    let (raw_op, slot, imm) = byte.bin_slot_imm_parts();
    Instruction::from(raw_op) == expected_op && slot == expected_slot && imm as i64 == expected_imm
}

fn bin_slot_slot_at(
    code: &[Byte],
    index: usize,
    expected_op: Instruction,
    expected_left: usize,
    expected_right: usize,
) -> bool {
    let Some(byte) = code.get(index) else {
        return false;
    };
    if *byte.bytecode() != Instruction::BinSlotSlot {
        return false;
    }
    let (raw_op, left, right) = byte.bin_slot_slot_parts();
    Instruction::from(raw_op) == expected_op && left == expected_left && right == expected_right
}

fn call_at(code: &[Byte], index: usize, expected_arity: usize, expected_target: usize) -> bool {
    let Some(byte) = code.get(index) else {
        return false;
    };
    if *byte.bytecode() != Instruction::CALL {
        return false;
    }
    let (arity, target) = byte.call_parts();
    arity == expected_arity && target == expected_target
}

fn store_at(code: &[Byte], index: usize, expected_slot: u32) -> bool {
    let Some(byte) = code.get(index) else {
        return false;
    };
    *byte.bytecode() == Instruction::STORE && byte.load_store_single_slot() == Some(expected_slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_program() -> Vec<Byte> {
        vec![
            Byte::new(ArchivedInstruction::BinSlotSlot).with_bin_slot_slot(
                Instruction::ADD as u8,
                0,
                1,
            ),
            Byte::new(ArchivedInstruction::RETURN),
        ]
    }

    #[test]
    fn compiles_supported_direct_binary_shape() {
        let mut runtime = JitRuntime::new(JitConfig {
            function_threshold: 1,
            ..JitConfig::default()
        })
        .expect("native target");
        let result = runtime
            .try_direct_binary(&add_program(), 0, Value::from(20_i64), Value::from(22_i64))
            .expect("hot function should compile");
        assert_eq!(result.as_int(), 42);
        assert_eq!(runtime.compiled_count(), 1);
    }

    #[test]
    fn compiles_supported_float_binary_shape() {
        let mut runtime = JitRuntime::new(JitConfig {
            function_threshold: 1,
            ..JitConfig::default()
        })
        .expect("native target");
        let code = vec![
            Byte::new(ArchivedInstruction::BinSlotSlot).with_bin_slot_slot(
                Instruction::ADDF as u8,
                0,
                1,
            ),
            Byte::new(ArchivedInstruction::RETURN),
        ];
        let result = runtime
            .try_direct_binary(&code, 0, Value::from(20.5_f64), Value::from(21.5_f64))
            .expect("hot float function should compile");
        assert_eq!(result.as_float(), 42.0);
    }

    #[test]
    fn waits_for_hotness_threshold_before_compiling() {
        let mut runtime = JitRuntime::new(JitConfig {
            function_threshold: 2,
            ..JitConfig::default()
        })
        .expect("native target");
        let code = add_program();
        assert!(
            runtime
                .try_direct_binary(&code, 0, Value::from(20_i64), Value::from(22_i64))
                .is_none()
        );
        assert_eq!(
            runtime
                .try_direct_binary(&code, 0, Value::from(20_i64), Value::from(22_i64))
                .expect("second call should compile")
                .as_int(),
            42
        );
    }

    #[test]
    fn refuses_unsupported_binary_shapes() {
        let mut runtime = JitRuntime::new(JitConfig {
            function_threshold: 1,
            ..JitConfig::default()
        })
        .expect("native target");

        let wrong_slots = vec![
            Byte::new(ArchivedInstruction::BinSlotSlot).with_bin_slot_slot(
                Instruction::ADD as u8,
                1,
                0,
            ),
            Byte::new(ArchivedInstruction::RETURN),
        ];
        assert!(
            runtime
                .try_direct_binary(&wrong_slots, 0, Value::from(1_i64), Value::from(2_i64))
                .is_none()
        );

        let compare = vec![
            Byte::new(ArchivedInstruction::BinSlotSlot).with_bin_slot_slot(
                Instruction::LEQ as u8,
                0,
                1,
            ),
            Byte::new(ArchivedInstruction::RETURN),
        ];
        assert!(
            runtime
                .try_direct_binary(&compare, 0, Value::from(1_i64), Value::from(2_i64))
                .is_none()
        );
        assert_eq!(runtime.compiled_count(), 0);
    }

    #[test]
    fn compiles_direct_unary_immediate() {
        let mut runtime = JitRuntime::new(JitConfig {
            function_threshold: 1,
            ..JitConfig::default()
        })
        .expect("native target");
        let imm = vec![
            Byte::new(ArchivedInstruction::BinSlotImm).with_bin_slot_imm(
                Instruction::ADD as u8,
                0,
                1,
            ),
            Byte::new(ArchivedInstruction::RETURN),
        ];
        assert_eq!(
            runtime
                .try_direct_unary(&imm, 0, Value::from(41_i64))
                .expect("unary imm")
                .as_int(),
            42
        );
    }

    #[test]
    fn compiles_direct_division_shape() {
        let mut runtime = JitRuntime::new(JitConfig {
            function_threshold: 1,
            ..JitConfig::default()
        })
        .expect("native target");
        let div = vec![
            Byte::new(ArchivedInstruction::BinSlotSlot).with_bin_slot_slot(
                Instruction::DIV as u8,
                0,
                1,
            ),
            Byte::new(ArchivedInstruction::RETURN),
        ];
        assert_eq!(
            runtime
                .try_direct_binary(&div, 0, Value::from(84_i64), Value::from(2_i64))
                .expect("div")
                .as_int(),
            42
        );
    }

    fn fib_program() -> (Vec<Byte>, Vec<u64>) {
        // false_target = entry + 2 = 2 for an entry PC of zero.
        let pool = vec![(2_u64 << 32) | 2];
        let code = vec![
            Byte::new(ArchivedInstruction::BinSlotImmJmpf).with_bin_slot_imm_jmpf(
                Instruction::LEQ as u8,
                0,
                0,
            ),
            Byte::new(ArchivedInstruction::ConstReturnImm).with_operand_u32(1),
            Byte::new(ArchivedInstruction::BinSlotImm).with_bin_slot_imm(
                Instruction::SUB as u8,
                0,
                1,
            ),
            Byte::new(ArchivedInstruction::CALL).with_call_packed(1, 0),
            Byte::new(ArchivedInstruction::STORE).with_load_store_slot(1),
            Byte::new(ArchivedInstruction::BinSlotImm).with_bin_slot_imm(
                Instruction::SUB as u8,
                0,
                2,
            ),
            Byte::new(ArchivedInstruction::CALL).with_call_packed(1, 0),
            Byte::new(ArchivedInstruction::STORE).with_load_store_slot(2),
            Byte::new(ArchivedInstruction::BinSlotSlot).with_bin_slot_slot(
                Instruction::ADD as u8,
                1,
                2,
            ),
            Byte::new(ArchivedInstruction::RETURN),
        ];
        (code, pool)
    }

    #[test]
    fn recognizes_recursive_fib_and_refuses_near_misses() {
        let (code, pool) = fib_program();
        assert!(recursive_fib_shape(&code, &pool, 0).is_some());

        let bad_immediate = vec![(5_u64 << 32) | 3];
        assert!(recursive_fib_shape(&code, &bad_immediate, 0).is_none());

        let bad_false_target = vec![(6_u64 << 32) | 2];
        assert!(recursive_fib_shape(&code, &bad_false_target, 0).is_none());

        let mut near_miss = fib_program().0;
        near_miss[1] = Byte::new(ArchivedInstruction::ConstReturnImm).with_operand_u32(0);
        assert!(recursive_fib_shape(&near_miss, &pool, 0).is_none());
    }

    #[test]
    fn compiles_recursive_fib_shape() {
        let mut runtime = JitRuntime::new(JitConfig {
            function_threshold: 1,
            ..JitConfig::default()
        })
        .expect("native target");
        let (code, pool) = fib_program();
        assert_eq!(
            runtime
                .try_recursive_fib(&code, &pool, 0, Value::from(10_i64))
                .expect("fib")
                .as_int(),
            89
        );
    }

    #[test]
    fn reset_program_clears_compiled_cache() {
        let mut runtime = JitRuntime::new(JitConfig {
            function_threshold: 1,
            ..JitConfig::default()
        })
        .expect("native target");
        let code = add_program();
        assert!(
            runtime
                .try_direct_binary(&code, 0, Value::from(1_i64), Value::from(2_i64))
                .is_some()
        );
        assert_eq!(runtime.compiled_count(), 1);
        runtime.reset_program();
        assert_eq!(runtime.compiled_count(), 0);
        assert!(
            runtime
                .try_direct_binary(&code, 0, Value::from(1_i64), Value::from(2_i64))
                .is_some()
        );
        assert_eq!(runtime.compiled_count(), 1);
    }

    #[test]
    fn array_helpers_match_interpreter_bounds_semantics() {
        let mut heap = crate::memory::Heap::default();
        let (object, _) = heap.alloc(
            crate::memory::ObjArray {
                elements: vec![Value::from(10_i64), Value::from(20_i64)],
            },
            crate::memory::Object::Array,
        );
        let addr = object.addr() as i64;
        let mut context = JitCallContext {
            heap: &mut heap,
        };
        let ctx = (&mut context as *mut JitCallContext).cast();

        assert_eq!(jit_array_len(ctx, addr), 2);
        assert_eq!(jit_array_index(ctx, addr, 1), 20);
        assert_eq!(jit_array_index(ctx, addr, 2), -1);
        assert_eq!(jit_array_index(ctx, addr, -1), -1);
        assert_eq!(jit_array_index(ctx, 0, 0), -1);
        assert_eq!(jit_array_len(ctx, 0), 0);
    }
}
