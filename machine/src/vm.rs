use common::{ArrayVec, Value, likely, unlikely};

use crate::{
    Byte, Coroutine, Frame, Heap, Instruction, Object,
    garbage::{Collectable, GcSized},
};

pub struct Machine<const S: usize> {
    frames: ArrayVec<Frame<Value>, S>,
    heap: Heap<1024>,
    // handler: ArrayVec<
    //     Option<&'vm dyn Fn(&mut Frame<Value>, &Byte<Value>) -> Option<ExecutionResult>>,
    //     255,
    // >,
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
        }
    }
}

impl<const S: usize> Machine<S> {
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

    // pub fn seek(&mut self, ip: usize) {
    //     self.frames.seek(ip);
    // }

    // pub fn register<F>(&mut self, instruction: Instruction, handler: &'vm F)
    // where
    //     F: Fn(&mut Frame<Value>, &Byte<Value>) -> Option<ExecutionResult>,
    // {
    //     self.handler[instruction as usize] = Some(handler);
    // }

    // fn mark(&mut self) {
    //     // @TODO: Figure out GC in a way that the types are not necessary
    //     // self.frames
    //     //     .iter()
    //     //     .filter(|frame| !frame.is_pending())
    //     //     .for_each(|frame| {
    //     //         frame
    //     //             .stack()
    //     //             .iter()
    //     //             // .filter(|element| matches!(element.get_type(), Type::Object | Type::String))
    //     //             .for_each(|element| {
    //     //                 let mut collectable: Collectable<Object> =
    //     //                     Collectable::from(element.as_ptr());
    //     //
    //     //                 collectable.as_mut().mark(&mut self.pending);
    //     //             });
    //     //     });
    //
    //     // for object in self.pending.iter() {
    //         // object.mark();
    //         // object.mark_reference();
    //     // }
    // }

    fn gc(&mut self) {
        #[cfg(not(debug_assertions))]
        if likely(self.heap.usage() < 0.75) {
            return;
        }

        self.heap.collect();
    }

    fn alloc<T: GcSized, F>(&mut self, value: T, map: F) -> (Object, Collectable<T>)
    where
        F: Fn(Collectable<T>) -> Object,
    {
        self.gc();

        self.heap.alloc(value, map)
    }

    pub fn run(&mut self, code: &[Byte<Value>]) {
        loop {
            let result = self.execute(code);

            match result.outcome() {
                ExecutionOutcome::CALL => {
                    likely(true);
                    self.frames.current_mut().enter(result.tell());

                    for _ in 0..result.arity() {
                        let value = *self.frames.get_mut().pop();
                        self.frames.current_mut().push(value);
                    }

                    self.frames.consume();
                }
                ExecutionOutcome::RETURN => {
                    likely(true);
                    let v = *self.frames.get_mut().pop();

                    self.frames.pop();
                    self.frames.get_mut().resume(v);
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

        self.heap.collect();
    }

    #[inline]
    fn execute(&mut self, code: &[Byte<Value>]) -> ExecutionResult {
        #[cfg(debug_assertions)]
        let frame_no = self.frames.len();

        let frame = self.frames.get_mut();
        let mut ip = frame.tell();

        while likely(ip < code.len()) {
            let opcode = &code[ip];

            #[cfg(debug_assertions)]
            {
                eprintln!(
                    "#{:<2} @ {:0>4} - {:>8}[{:0>4}, {:0>4}] - {:?}",
                    frame_no,
                    ip,
                    format!("{:?}", opcode.bytecode()),
                    opcode.operand(0),
                    opcode.operand(1),
                    frame
                );
            }

            ip += 1;
            frame.seek(ip);

            // if let Some(handler) = self.handler[(*opcode.bytecode() as u8) as usize] {
            //     let result = handler(frame, opcode);
            //     ip = frame.tell();
            //
            //     if let Some(result) = result {
            //         return result;
            //     } else {
            //         unlikely(true);
            //     }
            // } else {
            //     unlikely(true);
            // }

            match opcode.bytecode() {
                // Instruction::DUP => {
                //     let value = *frame.peek();
                //
                //     frame.push(value);
                // }
                Instruction::POP => {
                    frame.pop();
                }
                Instruction::CONST => frame.push(opcode.constant()),
                Instruction::STORE => {
                    likely(true);

                    let val = *frame.peek();
                    frame.store(opcode.operand(0), val);
                }
                Instruction::LOAD => {
                    likely(true);

                    frame.push(*frame.load(opcode.operand(0)));
                }
                Instruction::ADD => ibinary_op!(frame, +),
                Instruction::SUB => ibinary_op!(frame, -),
                Instruction::MUL => ibinary_op!(frame, *),
                Instruction::DIV => ibinary_op!(frame, /),
                // Instruction::ADDF => fbinary_op!(frame, +),
                // Instruction::SUBF => fbinary_op!(frame, -),
                // Instruction::MULF => fbinary_op!(frame, *),
                // Instruction::DIVF => fbinary_op!(frame, /),
                Instruction::LE => binary_op!(frame, <),
                // Instruction::GT => binary_op!(frame, >),
                // Instruction::EQ => binary_op!(frame, ==),
                // Instruction::LEQ => binary_op!(frame, <=),
                // Instruction::GEQ => binary_op!(frame, >=),
                // Instruction::NEQ => binary_op!(frame, !=),
                Instruction::PRINTI => println!("{}", frame.pop().as_int()),
                // Instruction::PRINTF => println!("{:.?}", frame.pop().as_float()),
                // Instruction::PRINTB => println!("{}", frame.pop().as_bool()),
                // Instruction::PRINTS => {
                //     println!(
                //         "{}",
                //         Collectable::<String>::from(frame.pop().as_ptr()).as_ref()
                //     )
                // }
                Instruction::JMP => {
                    ip = opcode.operand(0);
                }
                // Instruction::JLE => {
                //     let rhs = frame.pop().raw();
                //     let lhs = frame.pop().raw();
                //
                //     if lhs < rhs {
                //         ip = opcode.operand(0) ;
                //     }
                // }
                // Instruction::JGT => {
                //     let rhs = frame.pop().raw();
                //     let lhs = frame.pop().raw();
                //
                //     if lhs > rhs {
                //         ip = opcode.operand(0) ;
                //     }
                // }
                // Instruction::JEQ => {
                //     let rhs = frame.pop().raw();
                //     let lhs = frame.pop().raw();
                //
                //     if lhs == rhs {
                //         ip = opcode.operand(0) ;
                //     }
                // }
                Instruction::JMPF => {
                    if likely(!frame.pop().as_bool()) {
                        ip = opcode.operand(0);
                    }
                }
                // Instruction::JMPT => {
                //     if likely(frame.pop().as_bool()) {
                //         ip = opcode.operand(0);
                //     }
                // }
                Instruction::CALL => {
                    likely(true);
                    frame.suspend();

                    return ExecutionResult::call(opcode.operand(0), opcode.operand(1));
                }
                Instruction::RETURN => {
                    likely(true);
                    frame.complete();

                    return ExecutionResult::returns();
                }
                // Instruction::SUSP => {
                //     frame.suspend();
                //     let suspended_frame = frame.clone();
                //
                //     let (_, coro) = self.alloc(Coroutine::new(suspended_frame), Object::Coroutine);
                //     self.push(Value::from(coro.ptr()));
                //
                //     return ExecutionResult::returns();
                // }
                // Instruction::RESUME => {
                //     return ExecutionResult::resume();
                // }
                // Instruction::FREE => {
                //     unlikely(true);
                //
                //     match ObjectType::from_u8(opcode.operand(0) as u8) {
                //         ObjectType::String => Collectable::<String>::from(frame.pop().as_ptr()).release(),
                //         ObjectType::Coroutine => Collectable::<Coroutine<Value>>::from(frame.pop().as_ptr()).release(),
                //         ObjectType::Reference => Collectable::<Reference>::from(frame.pop().as_ptr()).release(),
                //         _ => unreachable!("Should not attempt to free null-object"),
                //     }
                // }
                Instruction::HALT => {
                    unlikely(true);
                    frame.terminate();

                    return ExecutionResult::terminate();
                }
                // Instruction::STRING => {
                //     let mut value: Vec<u8> = Vec::with_capacity(opcode.operand(0));
                //
                //     while let data = code[ip]
                //         && data.operand(0) != 0
                //     {
                //         ip += 1;
                //
                //         value.push(
                //             data.operand(0)
                //                 .try_into()
                //                 .expect("Unable to reconstruct character"),
                //         );
                //     }
                //
                //     let (_, collectable) = self.heap.alloc(
                //         if let Ok(value) = RustString::from_utf8(value) {
                //             value.into()
                //         } else {
                //             unreachable!("Unable to recreate string from bytes");
                //         },
                //         Object::String,
                //     );
                //
                //     frame.push(Value::from(collectable.ptr()));
                // }
                Instruction::NOOP => continue,
                _ => return ExecutionResult::invalid(),
            }
        }

        ExecutionResult::default()
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
            // (
            //     Byte::new_with(Instruction::CONST, [0, 0], Value::from(2.0)),
            //     Value::from(4.0),
            //     Instruction::ADDF,
            // ),
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
            // (
            //     Byte::new_with(Instruction::CONST, [0, 0], Value::from(2.0)),
            //     Value::from(0.0),
            //     Instruction::SUBF,
            // ),
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
            // (
            //     Byte::new_with(Instruction::CONST, [0, 0], Value::from(2.0)),
            //     Value::from(4.0),
            //     Instruction::MULF,
            // ),
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
            // (
            //     Byte::new_with(Instruction::CONST, [0, 0], Value::from(2.0)),
            //     Value::from(1.0),
            //     Instruction::DIVF,
            // ),
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
            // (
            //     Byte::new_with(Instruction::CONST, [0, 0], Value::from(2.0)),
            //     Value::from(1.0),
            //     Instruction::DIVF,
            // ),
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
