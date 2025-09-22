use std::{collections::HashMap, io::Write, };

use common::{ArrayVec, SeekableIterator, Value, promise, unlikely};

use crate::{
    ArenaAllocated, Byte, Coroutine, Frame, Instruction, Object, 
};

macro_rules! ibinary_op {
    ($frame: expr, $op:tt) => {
        {
            let rhs = $frame.pop().as_int();
            let lhs = $frame.peek().as_int();

            $frame.top().replace((lhs $op rhs) as u64)
        }
    }

}
macro_rules! fbinary_op {
    ($frame: expr, $op:tt) => {
        {
            let rhs = $frame.pop().as_float();
            let lhs = $frame.peek().as_float();

            $frame.top().replace((lhs $op rhs).to_bits())
        }
    }

}

macro_rules! binary_op {
    ($frame: expr, $op:tt) => {
        {
            let rhs = $frame.pop().raw();
            let lhs = $frame.peek().raw();

            $frame.top().replace((lhs $op rhs) as bool as _);
        }
    }

}

macro_rules! unary_op {
    ($frame: expr, $op: tt) => {
        {
            let rhs = $frame.peek().as_bool();

            $frame.top().replace(($op rhs).into());
        }
    }
}

type External = fn(&[Value]) -> Value;

pub struct Machine<const S: usize> {
    frames: ArrayVec<Frame<Value>, S>,
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
                    code_iter.seek(result.tell());
                    self.frames.current_mut().enter();

                    for _ in 0..result.arity() {
                        let value = *self.frames.get_mut().pop();
                        self.frames.current_mut().push(value);
                    }

                    self.frames.consume();
                }
                ExecutionOutcome::RETURN => {
                    let current = self.frames.pop();
                    let v = *current.peek();

                    let prev = self.frames.get_mut();

                    prev.resume(v);
                    code_iter.seek(prev.tell());
                }
                ExecutionOutcome::TERMINATION => {
                    unlikely(true);
                    break;
                }
                // ExecutionOutcome::RESUME => {
                //     unlikely(true);
                //     let frame: Collectable<Coroutine<Value>> =
                //         Collectable::from(self.frames.get_mut().pop().as_ptr());
                //
                //     self.frames.push(frame.as_ref().frame().clone());
                // }
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
                    if opcode.operand(0) != 0 {
                        let mut params = ArrayVec::<Value, 8>::default();

                        for idx in (0..opcode.operand(0)).rev() {
                            params[idx]= *frame.pop();
                        }

                        let format_string = ArenaAllocated::<Object>::new(frame.peek().as_ptr());
                        let format_str = format_string.as_ref().to_string();

                        frame.free(format_string);

                        // Pre-allocate string with estimated capacity
                        let mut message = String::default();

                        let mut chars = format_str.chars().peekable();
                        while let Some(ch) = chars.next() {
                            if ch == '%' {
                                match chars.peek() {
                                    Some('i') => {
                                        chars.next();
                                        message.push_str(&params.pop().as_int().to_string());
                                    }
                                    Some('f') => {
                                        chars.next();
                                        message
                                            .push_str(&format!("{:.?}", params.pop().as_float()));
                                    }
                                    Some('b') => {
                                        chars.next();
                                        message.push_str(&format!("{:0b}", params.pop().raw()));
                                    }
                                    Some('s') => {
                                        chars.next();
                                        let string_val =
                                            ArenaAllocated::<String>::new(params.pop().as_ptr());
                                        message.push_str(&string_val.as_ref().as_str());
                                    }
                                    Some('x') => {
                                        chars.next();
                                        message.push_str(&format!("{:08x}", params.pop().raw()));
                                    }
                                    Some('z') => {
                                        chars.next();
                                        message.push_str(if params.pop().raw() > 0 {
                                            "true"
                                        } else {
                                            "false"
                                        });
                                    }
                                    Some('u') => {
                                        chars.next();
                                        message.push_str(&params.pop().raw().to_string());
                                    }
                                    Some('p') => {
                                        chars.next();
                                        message.push_str(&format!(
                                            "{:08x}",
                                            params.pop().as_ptr::<bool>().addr()
                                        ));
                                    }
                                    _ => {
                                        message.push('%');
                                    }
                                }
                            } else {
                                message.push(ch);
                            }
                        }
                        let collectable = frame.alloc(Object::String(message.into()));
                        *frame.top() = Value::from(collectable.ptr());
                    }
                }
                Instruction::PRINT => {
                    let value = ArenaAllocated::<Object>::new(frame.pop().as_ptr());

                    match value.as_ref() {
                        Object::String(inner) => print!("{}", inner.as_str()),
                        Object::Coroutine(_) => print!("0x{:016}", value.ptr().addr()),
                        Object::None => print!(""),
                    };

                    frame.free(value);
                }
                Instruction::JMP => {
                    code.seek(opcode.operand(0));
                }
                Instruction::JMPR => {
                    code.add(opcode.operand(0));
                }
                Instruction::JMPF => {
                    if frame.pop().raw() != 1 {
                        code.seek(opcode.operand(0));
                    }
                }
                Instruction::JMPT => {
                    if frame.pop().raw() == 1 {
                        code.seek(opcode.operand(0));
                    }
                }
                Instruction::CALL => {
                    return ExecutionResult::call(opcode.operand(0), opcode.operand(1));
                }
                Instruction::NATIVE => {
                    let arity = opcode.operand(1);
                    let args = (0..arity).map(|_| *frame.pop()).collect::<Vec<_>>();
                    let result = self.native[&opcode.operand(0)](&args);

                    frame.push(result);
                }
                Instruction::RETURN => {
                    return ExecutionResult::returns();
                }
                Instruction::SUSP => {
                    let suspended_frame = frame.clone();

                    let object = frame.alloc(Object::Coroutine(Coroutine::new(suspended_frame)));

                    frame.push(Value::from(object.ptr()));

                    return ExecutionResult::returns();
                }
                Instruction::RESUME => {
                    return ExecutionResult::resume();
                }
                Instruction::RELEASE => {
                    frame.free(ArenaAllocated::<Object>::new(frame.load(opcode.operand(0)).as_ptr()));
                },
                Instruction::ACQUIRE => {
                    ArenaAllocated::<Object>::new(frame.load(opcode.operand(0)).as_ptr()).inc();
                },
                Instruction::HALT => {
                    let _ = std::io::stdout().flush();
                    return ExecutionResult::terminate();
                }
                Instruction::STRING => {
                    let length = opcode.operand(0);
                    let mut value: String = String::with_capacity(length);

                    while length != value.len() && let Some(data) = code.next() {
                        value.push(char::from_u32(data.operand(0) as u32).unwrap_or_default());
                    }

                    let object = frame.alloc(Object::String(value.into()));

                    frame.push(Value::from(object.ptr()));
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
