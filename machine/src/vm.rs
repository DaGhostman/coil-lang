use std::{borrow::Borrow, collections::HashMap, io::Write, ops::Deref};

use common::{ArrayVec, SeekableIterator, Value, promise, unlikely};

use crate::{
    Byte, Coroutine, Frame, Heap, Instruction, Object, ObjectType, String as ObjString,
    garbage::Collectable,
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
                if frame_no == 16 {
                    panic!("Enough!");
                }
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

                        let format_string = Collectable::<ObjString>::from(frame.peek().as_ptr());
                        let format_str = format_string.as_ref().as_str();

                        // Pre-allocate string with estimated capacity
                        let mut message = String::with_capacity(format_str.len() * 2);

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
                                            Collectable::<String>::from(params.pop().as_ptr());
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
                        Heap::free(format_string);
                        let (_, collectable) = Heap::alloc(message.into(), Object::String);
                        *frame.top() = Value::from(collectable.ptr());
                    }
                    // let mut message = String::new();
                    // let mut params = Vec::with_capacity(opcode.operand(1));
                    //
                    // for idx in (0..opcode.operand(0)).rev() {
                    //     params.insert(idx, *frame.pop());
                    // }
                    //
                    // let mut format = Collectable::<ObjString>::from(frame.pop().as_ptr());
                    // let mut byte_format = format.as_ref().as_str().char_indices();
                    //
                    //     while let Some((_, v)) = byte_format.next() {
                    //         match v {
                    //             '%' => match byte_format.next() {
                    //                 Some((_, 'i')) => unsafe {
                    //                     message.push_str(
                    //                         format!("{}", params.pop().unwrap_unchecked().as_int()).as_str(),
                    //                     );
                    //                 }
                    //                 Some((_, 'f')) => unsafe {
                    //                     message.push_str(
                    //                         format!("{:.?}", params.pop().unwrap_unchecked().as_float()).as_str(),
                    //                     );
                    //                 }
                    //                 Some((_, 'b')) => unsafe {
                    //                     message.push_str(
                    //                         format!("{:0b}", params.pop().unwrap_unchecked().raw()).as_str(),
                    //                     );
                    //                 }
                    //                 Some((_, 's')) => unsafe {
                    //                     message.push_str(
                    //                         Collectable::<String>::from(params.pop().unwrap_unchecked().as_ptr())
                    //                             .as_ref()
                    //                             .to_string()
                    //                             .as_str(),
                    //                     );
                    //                 }
                    //                 Some((_, 'x')) => unsafe {
                    //                     message.push_str(
                    //                         format!("{:08x}", params.pop().unwrap_unchecked().raw()).as_str(),
                    //                     );
                    //                 }
                    //                 Some((_, 'z')) => unsafe {
                    //                     message.push_str(if params.pop().unwrap_unchecked().raw() > 0 {
                    //                         "true"
                    //                     } else {
                    //                         "false"
                    //                     });
                    //                 }
                    //                 Some((_, 'u')) => unsafe {
                    //                     message
                    //                         .push_str(format!("{}", params.pop().unwrap_unchecked().raw()).as_str());
                    //                 }
                    //                 Some((_, 'p')) => unsafe {
                    //                     message.push_str(
                    //                         format!(
                    //                             "{:08x}",
                    //                             params.pop().unwrap_unchecked().as_ptr::<bool>().addr()
                    //                         )
                    //                         .as_str(),
                    //                     );
                    //                 }
                    //                 _ => {
                    //                     message.push('%');
                    //                 }
                    //             },
                    //             ch => {
                    //                 message.push(ch);
                    //             }
                    //         }
                    //     }
                    //
                    // Heap::free(&mut format);
                    // let (_, collectable) = Heap::alloc(message.into(), Object::String);
                    //
                    // frame.push(Value::from(collectable.ptr()));
                }
                Instruction::PRINT => {
                    let value = Collectable::<ObjString>::from(frame.pop().as_ptr());
                    print!("{}", value.as_ref().as_str());

                    Heap::free(value);
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

                    let (_, coro) = Heap::alloc(Coroutine::new(suspended_frame), Object::Coroutine);
                    frame.push(Value::from(coro.ptr()));

                    return ExecutionResult::returns();
                }
                Instruction::RESUME => {
                    return ExecutionResult::resume();
                }
                Instruction::ACQUIRE => {
                    match (opcode.operand(0) as u8).into() {
                        ObjectType::String => {
                            Collectable::<ObjString>::from(frame.peek().as_ptr()).inc()
                        }
                        ObjectType::Reference => {
                            Collectable::<crate::memory::Reference>::from(frame.peek().as_ptr())
                                .inc()
                        }
                        ObjectType::Coroutine => {
                            Collectable::<crate::memory::Coroutine<Value>>::from(
                                frame.peek().as_ptr(),
                            )
                            .inc()
                        }
                        _ => 0,
                    };
                }
                Instruction::RELEASE => match (opcode.operand(0) as u8).into() {
                    ObjectType::String => {
                        Heap::free(Collectable::<ObjString>::from(frame.peek().as_ptr()))
                    }
                    ObjectType::Reference => Heap::free(
                        Collectable::<crate::memory::Reference>::from(frame.peek().as_ptr()),
                    ),
                    ObjectType::Coroutine => {
                        Heap::free(Collectable::<crate::memory::Coroutine<Value>>::from(
                            frame.peek().as_ptr(),
                        ))
                    }
                    _ => (),
                },
                Instruction::HALT => {
                    let _ = std::io::stdout().flush();
                    return ExecutionResult::terminate();
                }
                Instruction::STRING => {
                    let mut value: String = String::with_capacity(opcode.operand(0));

                    for _ in 0..opcode.operand(0) {
                        if let Some(data) = code.next() {
                            value.push(char::from_u32(data.operand(0) as u32).unwrap_or_default());
                        }
                    }

                    let (_, collectable) = Heap::alloc(value.into(), Object::String);

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
