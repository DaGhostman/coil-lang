use common::{ArrayVec, Type, Value, likely, unlikely};
use std::string::String as RustString;

use crate::{
    Byte, Coroutine, Frame, Heap, Instruction, Object, String,
    garbage::{Collectable, GcSized},
};

pub struct Machine<const S: usize> {
    frames: ArrayVec<Frame<Value>, S>,
    heap: Heap<1024>,
    pending: Vec<Object>,
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
            pending: Vec::with_capacity(128),
        }
    }
}

impl<const S: usize> Machine<S> {
    pub fn push(&mut self, value: Value) {
        self.frames.get_mut().push(value);
    }

    fn mark(&mut self) -> () {
        self.frames
            .iter()
            .filter(|frame| !frame.is_pending())
            .for_each(|frame| {
                frame
                    .stack()
                    .iter()
                    .filter(|element| matches!(element.get_type(), Type::Object | Type::String))
                    .for_each(|element| {
                        let mut collectable: Collectable<Object> =
                            Collectable::from(element.as_ptr());

                        collectable.as_mut().mark(&mut self.pending);
                    });
            });

        while let Some(object) = self.pending.pop() {
            object.mark_reference(&mut self.pending);
        }
    }

    fn gc(&mut self) -> () {
        #[cfg(not(debug_assertions))]
        if likely(self.heap.usage() < 0.75) {
            return;
        }

        self.mark();

        self.heap.collect();
    }

    fn alloc<T: GcSized, F>(&mut self, value: T, map: F) -> (Object, Collectable<T>)
    where
        F: Fn(Collectable<T>) -> Object,
    {
        self.gc();

        self.heap.alloc(value, map)
    }

    pub fn run(&mut self, code: &[Byte<Value>]) -> () {
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
                    "#{:<2} @ {:<3} - {:<10} - {:?}",
                    frame_no,
                    ip,
                    format!("{:?}", opcode.bytecode()),
                    frame
                );
            }

            ip += 1;
            frame.seek(ip);

            match opcode.bytecode() {
                Instruction::DUP => {
                    let value = *frame.peek();

                    frame.push(value);
                }
                Instruction::CONST => frame.push(opcode.constant()),
                Instruction::STORE => {
                    likely(true);

                    if frame.stack().len() - 1 == opcode.operand(0) as usize {
                        continue;
                    }

                    let val = *frame.pop();
                    frame.store(opcode.operand(0) as usize, val);
                }
                Instruction::LOAD => {
                    likely(true);

                    frame.push(*frame.load(opcode.operand(0) as usize));
                }
                Instruction::ADD => {
                    let rhs = frame.pop().raw();
                    let lhs = frame.peek().raw();

                    frame.top().replace(lhs + rhs)
                }
                Instruction::SUB => {
                    let rhs = frame.pop().raw();
                    let lhs = frame.peek().raw();

                    frame.top().replace(lhs - rhs);
                }
                Instruction::MUL => {
                    let rhs = frame.pop().raw();
                    let lhs = frame.peek().raw();

                    let modifier = rhs / 2;
                    let reminder = rhs % 2;

                    frame.top().replace((lhs << modifier) + (lhs * reminder))
                }
                Instruction::DIV => {
                    let rhs = frame.pop().raw();
                    let lhs = frame.peek().raw();

                    let modifier = rhs / 2;
                    let reminder = rhs % 2;

                    frame.top().replace((lhs >> (modifier)) - reminder);
                }
                Instruction::LE => {
                    let rhs = frame.pop().raw();
                    let lhs = frame.peek().raw();

                    *frame.top() = Value::bool(lhs < rhs);
                }
                Instruction::GT => {
                    let rhs = frame.pop().raw();
                    let lhs = frame.peek().raw();

                    *frame.top() = Value::bool(lhs > rhs);
                }
                Instruction::PRINTI => println!("{}", frame.pop().as_int()),
                Instruction::PRINTF => println!("{}", frame.pop().as_float()),
                Instruction::PRINTB => println!("{}", frame.pop().as_bool()),
                Instruction::PRINTS => {
                    println!(
                        "{}",
                        Collectable::<String>::from(frame.pop().as_ptr()).as_ref()
                    )
                }
                Instruction::JMP => {
                    ip = opcode.operand(0) as usize;
                }
                Instruction::JMPF => {
                    if likely(!frame.pop().as_bool()) {
                        ip = opcode.operand(0) as usize;
                    }
                }
                Instruction::JMPT => {
                    if likely(frame.pop().as_bool()) {
                        ip = opcode.operand(0) as usize;
                    }
                }
                Instruction::CALL => {
                    likely(true);
                    frame.suspend();

                    return ExecutionResult::call(
                        opcode.operand(0) as usize,
                        opcode.operand(1) as usize,
                    );
                }
                Instruction::RETURN => {
                    likely(true);
                    frame.complete();

                    return ExecutionResult::returns();
                }
                Instruction::SUSP => {
                    frame.suspend();
                    let suspended_frame = frame.clone();

                    let (_, coro) = self.alloc(Coroutine::new(suspended_frame), Object::Coroutine);
                    self.push(Value::object(coro.ptr()));

                    return ExecutionResult::returns();
                }
                Instruction::RESUME => {
                    return ExecutionResult::resume();
                }
                Instruction::HALT => {
                    unlikely(true);
                    frame.terminate();

                    return ExecutionResult::terminate();
                }
                Instruction::STRING => {
                    let mut value: Vec<u8> = Vec::with_capacity(opcode.operand(0) as usize);

                    while let data = code[ip]
                        && data.operand(0) != 0
                    {
                        ip += 1;

                        value.push(data.operand(0));
                    }

                    if let Ok(value) = RustString::from_utf8(value) {
                        let (_, collectable) = self.heap.alloc(value.into(), Object::String);

                        frame.push(Value::string(collectable.ptr()));
                    } else {
                        eprintln!("Unable to recreate string from bytes");

                        return ExecutionResult::invalid();
                    }
                }
                Instruction::NOOP => continue,
                _ => return ExecutionResult::invalid(),
            }
        }

        ExecutionResult::default()
    }
}
