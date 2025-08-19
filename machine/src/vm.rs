use common::{ArrayVec, Type, Value, likely, unlikely};
use std::string::String as RustString;
const FRAME_COUNT: usize = 256;

use crate::{Byte, Frame, Heap, Instruction, Object, String, garbage::Collectable};

pub struct Machine {
    frames: ArrayVec<Frame<Value>, FRAME_COUNT>,
    heap: Heap<1024>,
}

#[derive(Default, Copy, Clone)]
#[repr(u8)]
enum ExecutionOutcome {
    #[default]
    INVALID,
    CALL,
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

impl Default for Machine {
    fn default() -> Self {
        let mut frames = ArrayVec::default();
        frames.consume();
        Self {
            frames,
            heap: Heap::default(),
        }
    }
}

impl Machine {
    pub fn push(&mut self, value: Value) {
        self.frames.get_mut().push(value);
    }

    fn mark(&mut self, fp: usize) -> () {
        let mut grey: Vec<Object> = Vec::with_capacity(1024);

        for n in 0..fp {
            let stack = self.frames[n].stack_mut();
            let max = stack.len();

            for x in 0..max {
                match stack[x].get_type() {
                    Type::Object | Type::String => {
                        let mut collectable: Collectable<Object> =
                            Collectable::from(stack[x].as_ptr());

                        collectable.as_mut().mark(&mut grey);
                    }
                    _ => (),
                }
            }
        }

        while let Some(object) = grey.pop() {
            object.mark_reference(&mut grey);
        }
    }

    fn gc(&mut self, fp: usize) -> () {
        self.mark(fp);

        #[cfg(not(debug_assertions))]
        if self.heap.usage() < 0.75 {
            return;
        }

        self.heap.collect();
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

                    self.heap.collect();
                }
                ExecutionOutcome::TERMINATION => {
                    unlikely(true);
                    break;
                }
                _ => (),
            }
        }
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
                    format!("{}", opcode.bytecode()),
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
                    let val = *frame.pop();
                    frame.store(opcode.operand(0) as usize, val);
                }
                Instruction::LOAD => {
                    likely(true);

                    frame.push(*frame.load(opcode.operand(0) as usize));
                }
                Instruction::ADD => {
                    let rhs = frame.pop().raw();
                    let lhs = frame.top();

                    lhs.replace(lhs.raw() + rhs);
                }
                Instruction::SUB => {
                    let rhs = frame.pop().raw();
                    let lhs = frame.top();

                    lhs.replace(lhs.raw() - rhs);
                }
                Instruction::LE => {
                    let rhs = frame.pop().raw();
                    let lhs = frame.top();

                    *lhs = Value::bool(lhs.raw() < rhs);
                }
                Instruction::GT => {
                    let rhs = frame.pop().raw();
                    let lhs = frame.top();

                    *lhs = Value::bool(lhs.raw() > rhs);
                }
                Instruction::PRINT => {
                    let value = frame.pop();
                    println!(
                        "{}",
                        match value.get_type() {
                            Type::Bool | Type::Integer => format!("{}", value.as_int()),
                            Type::Float => format!("{:.?}", value.as_float()),
                            Type::String => {
                                let value: Collectable<String> = Collectable::from(value.as_ptr());

                                format!("{}", (*value).as_ref())
                            }
                            _ => RustString::new(),
                        }
                    )
                }
                Instruction::JMP => {
                    ip = opcode.operand(0) as usize;
                }
                Instruction::JMPF => {
                    if likely(frame.pop().as_int() == 0) {
                        ip = opcode.operand(0) as usize;
                    }
                }
                Instruction::JMPT => {
                    if likely(frame.pop().as_int() == 1) {
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
