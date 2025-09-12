use std::collections::HashMap;

#[cfg(not(debug_assertions))]
use common::likely;

use common::{ArrayVec, SeekableIterator, Value, unlikely};

use crate::{
    Byte, Coroutine, Frame, Heap, Instruction, Object, ObjectType, Reference,
    garbage::{Collectable, GcSized},
};

macro_rules! ibinary_op {
    ($frame: expr, $op:tt) => {
        {
            let rhs = $frame.pop().as_int();
            let lhs = $frame.peek().as_int();

            *$frame.top() = Value::from(lhs $op rhs);
        }
    }

}
macro_rules! fbinary_op {
    ($frame: expr, $op:tt) => {
        {
            let rhs = $frame.pop().as_float();
            let lhs = $frame.peek().as_float();

            *$frame.top() = Value::from(lhs $op rhs);
        }
    }

}

macro_rules! binary_op {
    ($frame: expr, $op:tt) => {
        {
            let rhs = $frame.pop().raw();
            let lhs = $frame.peek().raw();

            *$frame.top() = Value::from(lhs $op rhs);
        }
    }

}

macro_rules! unary_op {
    ($frame: expr, $op: tt) => {
        {
            let rhs = $frame.peek().as_int();

            *$frame.top() = Value::from($op rhs);
        }
    }
}


const HEAP: usize = 8192;
type External = fn(&[Value], &mut Heap<HEAP>) -> Value;

pub struct Machine<const S: usize> {
    frames: ArrayVec<Frame<Value>, S>,
    heap: Heap<HEAP>,

    native: HashMap<usize, External>,
}

#[derive(Default, Copy, Clone)]
#[repr(u8)]
enum ExecutionOutcome {
    #[default]
    INVALID,
    CALL,
    RESUME,
    RETURN,
    TERMINATION,
}

#[derive(Default)]
struct ExecutionResult {
    outcome: ExecutionOutcome,
    ip: usize,
    arity: usize,
}
impl ExecutionResult {
    pub fn returns() -> Self {
        Self {
            outcome: ExecutionOutcome::RETURN,
            ip: 0,
            arity: 0,
        }
    }

    pub fn resume() -> Self {
        Self {
            outcome: ExecutionOutcome::RESUME,
            ip: 0,
            arity: 0,
        }
    }

    pub fn call(ip: usize, arity: usize) -> Self {
        Self {
            outcome: ExecutionOutcome::CALL,
            ip,
            arity,
        }
    }

    pub fn terminate() -> Self {
        Self {
            outcome: ExecutionOutcome::TERMINATION,
            ip: 0,
            arity: 0,
        }
    }

    pub fn invalid() -> Self {
        Self {
            outcome: ExecutionOutcome::INVALID,
            ip: usize::MAX,
            arity: usize::MAX,
        }
    }

    pub fn outcome(&self) -> ExecutionOutcome {
        self.outcome
    }

    pub fn arity(&self) -> usize {
        self.arity
    }

    pub fn tell(&self) -> usize {
        self.ip
    }
}

impl<const S: usize> Default for Machine<S> {
    fn default() -> Self {
        let mut frames = ArrayVec::default();
        frames.consume();
        Self {
            frames,
            heap: Heap::default(),
            // handler: ArrayVec::default(),
            native: HashMap::with_capacity(32),
        }
    }
}

impl<const S: usize> Machine<S> {
    pub fn register(&mut self, name: usize, func: External) {
        self.native.insert(name, func);
    }
}

impl<const S: usize> Machine<S> {
    #[cfg(test)]
    pub fn push(&mut self, value: Value) {
        self.frames.get_mut().push(value);
    }

    #[cfg(test)]
    pub fn pop(&mut self) -> Value {
        *self.frames.get_mut().pop()
    }

    #[cfg(test)]
    pub fn tell(&self) -> usize {
        self.frames.get().tell()
    }


    pub fn run(&mut self, code: &[Byte<Value>]) {
        if code.is_empty() {
            return;
        }

        let mut code_iter = code.into();

        loop {
            let result = self.execute(&mut code_iter);

            match result.outcome() {
                ExecutionOutcome::CALL => {
                    self.frames.get_mut().seek(code_iter.tell());
                    self.frames.current_mut().enter(result.tell());
                    code_iter.seek(result.tell());

                    for _ in 0..result.arity() {
                        let value = *self.frames.get_mut().pop();
                        self.frames.current_mut().push(value);
                    }

                    self.frames.consume();
                }
                ExecutionOutcome::RETURN => {
                    let v = *self.frames.get_mut().pop();
                    let allocations = self
                        .frames
                        .pop()
                        .allocations()
                        .map(|v| *v)
                        .collect::<Vec<_>>();
                    let current = self.frames.get_mut();

                    for addr in allocations {
                        if v.raw() == addr {
                            current.alloc(addr);
                        } else {
                            Collectable::<&dyn std::any::Any>::from(
                                std::ptr::NonNull::without_provenance(
                                    core::num::NonZero::new(addr as usize).expect("Invalid porinter address")
                                )
                            )
                                .dec();
                        }
                    }

                    #[cfg(not(debug_assertions))]
                    if likely(self.heap.usage() > 0.75) {
                        self.heap.collect();
                    }
                    #[cfg(debug_assertions)]
                    self.heap.collect();

                    self.frames.get_mut().resume(v);
                    code_iter.seek(self.frames.get().tell());
                }
                ExecutionOutcome::TERMINATION => {
                    unlikely(true);
                    break;
                }
                ExecutionOutcome::RESUME => {
                    unlikely(true);
                    let frame: Collectable<Coroutine<Value>> =
                        Collectable::from(self.frames.get_mut().pop().as_ptr());

                    self.frames.push(frame.as_ref().frame().clone());
                }
                _ => (),
            }
        }
    }

    #[inline]
    fn execute<'iter>(
        &mut self,
        code: &mut SeekableIterator<'iter, Byte<Value>>,
    ) -> ExecutionResult {
        #[cfg(debug_assertions)]
        let frame_no = self.frames.len();

        let frame = self.frames.get_mut();

        macro_rules! alloc {
            ($value: expr, $map: expr) => {
                {
                let (object, collectable) = self.heap.alloc($value, $map);
                frame.alloc(Value::from(collectable.ptr()).raw());

                (object, collectable)
                }
            };
        }

        while let Some(opcode) = code.next() {
            #[cfg(debug_assertions)]
            {
                eprintln!(
                    "#{:<2} @ {:0>4} - {:>8}[{:0>4}, {:0>4}] - {:?}",
                    frame_no,
                    code.tell(),
                    format!("{:?}", opcode.bytecode()),
                    opcode.operand(0),
                    opcode.operand(1),
                    frame
                );
            }

            match opcode.bytecode() {
                Instruction::POP => {
                    frame.pop();
                }
                Instruction::CONST => frame.push(opcode.constant()),
                Instruction::STORE => {
                    let val = *frame.peek();
                    frame.store(opcode.operand(0), val);
                }
                Instruction::LOAD => {
                    frame.push(*frame.load(opcode.operand(0)));
                }
                Instruction::NOT => unary_op!(frame, !),
                Instruction::NEG => unary_op!(frame, !),
                Instruction::ADD => ibinary_op!(frame, +),
                Instruction::SUB => ibinary_op!(frame, -),
                Instruction::MUL => ibinary_op!(frame, *),
                Instruction::DIV => ibinary_op!(frame, /),
                Instruction::MOD => ibinary_op!(frame, %),
                Instruction::LE => binary_op!(frame, <),
                Instruction::GT => binary_op!(frame, >),
                Instruction::EQ => binary_op!(frame, ==),
                Instruction::LEF => binary_op!(frame, <=),
                Instruction::GTF => binary_op!(frame, >=),
                Instruction::NEQ => binary_op!(frame, !=),
                Instruction::ADDF => fbinary_op!(frame, +),
                Instruction::SUBF => fbinary_op!(frame, -),
                Instruction::MULF => fbinary_op!(frame, *),
                Instruction::DIVF => fbinary_op!(frame, /),
                Instruction::MODF => fbinary_op!(frame, %),
                Instruction::FORMAT => {
                    let mut message = String::new();
                    let num_params = opcode.operand(0);
                    if num_params == 0 {
                        message = Collectable::<String>::from(frame.pop().as_ptr())
                            .as_ref()
                            .to_string();
                    } else {
                        let mut params = vec![];
                        for _ in 0..opcode.operand(0) {
                            params.push(*frame.pop());
                        }
                        let format = Collectable::<String>::from(frame.pop().as_ptr())
                            .as_ref()
                            .to_string();

                        let byte_format = format.chars().collect::<Vec<char>>();

                        let mut n = 0;
                        let mut param = 0;
                        while n < byte_format.len() {
                            if '%' == byte_format[n] && (n == 0 || '\\' != byte_format[n - 1]) {
                                if params.len() <= param {
                                    todo!("Handle within typechecker");
                                }
                                n += 1;
                                match byte_format[n] {
                                    'i' => {
                                        n += 1;

                                        message.push_str(
                                            format!("{}", params[param].as_int()).as_str(),
                                        );
                                        param += 1;
                                    }
                                    'f' => {
                                        n += 1;
                                        message.push_str(
                                            format!("{:.?}", params[param].as_float()).as_str(),
                                        );
                                        param += 1;
                                    }
                                    'b' => {
                                        n += 1;
                                        message.push_str(
                                            format!("{:0b}", params[param].raw()).as_str(),
                                        );
                                        param += 1;
                                    }
                                    's' => {
                                        n += 1;
                                        message.push_str(
                                            Collectable::<String>::from(params[param].as_ptr())
                                                .as_ref()
                                                .to_string()
                                                .as_str(),
                                        );
                                        param += 1;
                                    }
                                    'x' => {
                                        n += 1;
                                        message.push_str(
                                            format!("{:08x}", params[param].raw()).as_str(),
                                        );
                                        param += 1;
                                    }
                                    'z' => {
                                        n += 1;
                                        message.push_str(if params[param].raw() > 0 {
                                            "true"
                                        } else {
                                            "false"
                                        });
                                        param += 1;
                                    }
                                    'u' => {
                                        n += 1;
                                        message
                                            .push_str(format!("{}", params[param].raw()).as_str());
                                        param += 1;
                                    }
                                    'p' => {
                                        n += 1;
                                        message.push_str(
                                            format!(
                                                "{:08x}",
                                                params[param].as_ptr::<bool>().addr()
                                            )
                                            .as_str(),
                                        );
                                        param += 1;
                                    }
                                    _ => {
                                        message.push(byte_format[n]);
                                    }
                                }
                                continue;
                            }

                            message.push(byte_format[n]);
                            n += 1;
                        }
                    }

                    let (_, collectable) = alloc!(message.into(), Object::String);

                    frame.push(Value::from(collectable.ptr()));
                }
                Instruction::PRINT => {
                    print!(
                        "{}",
                        Collectable::<String>::from(frame.pop().as_ptr()).as_ref()
                    );
                }
                Instruction::JMP => {
                    code.seek(opcode.operand(0));
                }
                Instruction::JMPF => {
                    if !frame.pop().as_bool() {
                        code.seek(opcode.operand(0));
                    }
                }
                Instruction::JMPT => {
                    if frame.pop().as_bool() {
                        code.seek(opcode.operand(0));
                    }
                }
                Instruction::CALL => {
                    return ExecutionResult::call(opcode.operand(0), opcode.operand(1));
                }
                Instruction::NATIVE => {
                    let arity = opcode.operand(1);
                    let args = (0..arity).map(|_| *frame.pop()).collect::<Vec<_>>();
                    let result = self.native[&opcode.operand(0)](&args, &mut self.heap);

                    frame.push(result);
                }
                Instruction::RETURN => {
                    return ExecutionResult::returns();
                }
                Instruction::SUSP => {
                    let suspended_frame = frame.clone();

                    let (_, coro) = self
                        .heap
                        .alloc(Coroutine::new(suspended_frame), Object::Coroutine);
                    frame.push(Value::from(coro.ptr()));

                    return ExecutionResult::returns();
                }
                Instruction::RESUME => {
                    return ExecutionResult::resume();
                }
                Instruction::ACQUIRE => {
                    Collectable::<&dyn std::any::Any>::from(frame.load(opcode.operand(0)).as_ptr())
                        .inc();
                }
                Instruction::RELEASE => {
                    Collectable::<&dyn std::any::Any>::from(frame.load(opcode.operand(0)).as_ptr())
                        .dec();
                }
                Instruction::HALT => {
                    return ExecutionResult::terminate();
                }
                Instruction::STRING => {
                    let mut value: String = String::with_capacity(opcode.operand(0));

                    for _ in 0..opcode.operand(0) {
                        if let Some(data) = code.next() {
                            value.push(char::from_u32(data.operand(0) as u32).unwrap_or_default());
                        }
                    }

                    let (_, collectable) = alloc!(value.into(), Object::String);

                    frame.push(Value::from(collectable.ptr()));
                }
                Instruction::NOOP => continue,
                _ => return ExecutionResult::invalid(),
            }
        }

        ExecutionResult::terminate()
    }
}
#[cfg(test)]
mod tests {
    use common::Value;

    use crate::{Byte, Instruction, Machine};

    #[test]
    fn test_constants() {
        let mut vm = Machine::<1>::default();
        let v = Value::from(42);
        vm.run(&[
            Byte::new_with_value(Instruction::CONST, v),
            Byte::new(Instruction::HALT),
        ]);

        assert_eq!(v.raw(), vm.pop().raw());
    }

    #[test]
    fn test_addition() {
        let cases = [
            (
                Byte::new_with_value(Instruction::CONST, Value::from(2.0)),
                Value::from(4.0),
                Instruction::ADDF,
            ),
            (
                Byte::new_with_value(Instruction::CONST, Value::from(2)),
                Value::from(4),
                Instruction::ADD,
            ),
        ];

        for (v, e, i) in cases {
            let mut vm = Machine::<2>::default();
            vm.run(&[v, v, Byte::new(i), Byte::new(Instruction::HALT)]);

            assert_eq!(e.raw(), vm.pop().raw());
        }
    }

    #[test]
    fn test_subtraction() {
        let cases = [
            (
                Byte::new_with_value(Instruction::CONST, Value::from(2.0)),
                Value::from(0.0),
                Instruction::SUBF,
            ),
            (
                Byte::new_with_value(Instruction::CONST, Value::from(2)),
                Value::from(0),
                Instruction::SUB,
            ),
        ];

        for (v, e, i) in cases {
            let mut vm = Machine::<2>::default();
            vm.run(&[v, v, Byte::new(i), Byte::new(Instruction::HALT)]);

            assert_eq!(e.raw(), vm.pop().raw());
        }
    }

    #[test]
    fn test_multiplication() {
        let cases = [
            (
                Byte::new_with_value(Instruction::CONST, Value::from(2.0)),
                Value::from(4.0),
                Instruction::MULF,
            ),
            (
                Byte::new_with_value(Instruction::CONST, Value::from(2)),
                Value::from(4),
                Instruction::MUL,
            ),
        ];

        for (v, e, i) in cases {
            let mut vm = Machine::<2>::default();
            vm.run(&[v, v, Byte::new(i), Byte::new(Instruction::HALT)]);

            assert_eq!(e.raw(), vm.pop().raw());
        }
    }

    #[test]
    fn test_division() {
        let cases = [
            (
                Byte::new_with_value(Instruction::CONST, Value::from(2.0)),
                Value::from(1.0),
                Instruction::DIVF,
            ),
            (
                Byte::new_with_value(Instruction::CONST, Value::from(2)),
                Value::from(1),
                Instruction::DIV,
            ),
        ];

        for (v, e, i) in cases {
            let mut vm = Machine::<2>::default();
            vm.run(&[v, v, Byte::new(i), Byte::new(Instruction::HALT)]);

            assert_eq!(e.raw(), vm.pop().raw());
        }
    }

    #[test]
    fn test_jumps() {
        let cases = [
            (
                Byte::new_with_value(Instruction::CONST, Value::from(2.0)),
                Value::from(1.0),
                Instruction::DIVF,
            ),
            (
                Byte::new_with_value(Instruction::CONST, Value::from(2)),
                Value::from(1),
                Instruction::DIV,
            ),
        ];

        for (v, e, i) in cases {
            let mut vm = Machine::<2>::default();
            vm.run(&[v, v, Byte::new(i), Byte::new(Instruction::HALT)]);

            assert_eq!(e.raw(), vm.pop().raw());
        }
    }
}
