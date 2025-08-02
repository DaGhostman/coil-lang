use std::{fmt::{Debug, Display}, ops::{Add, AddAssign, BitXor, Sub, SubAssign}};
use common::{likely, unlikely, ArrayVec};

use crate::{Bytecode, Frame, Opcode, Stack};

pub struct Machine<T: Default + Copy + Clone + Debug> {
    frames: ArrayVec<Frame<T>, 128>,
    stack: Stack<T, 512>,
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
    stack: usize,
}

impl ExecutionResult {
    pub fn returns(result: usize) -> Self {
        Self {
            outcome: ExecutionOutcome::RETURN,
            ip: 0,
            stack: result,
        }
    }

    pub fn call(ip: usize, sp: usize) -> Self {
        Self {
            outcome: ExecutionOutcome::CALL,
            ip,
            stack: sp,
        }
    }

    pub fn terminate() -> Self {
        Self {
            outcome: ExecutionOutcome::TERMINATION,
            ip: 0,
            stack: 0,
        }
    }

    pub fn outcome(&self) -> ExecutionOutcome {
        self.outcome
    }

    pub fn stack(&self) -> usize {
        self.stack
    }

    pub fn tell(&self) -> usize {
        self.ip
    }
}


impl <T: Default + Copy + Clone + Debug> Default for Machine<T> {
    fn default() -> Self {
        Self {
            frames: ArrayVec::default(),
            stack: Stack::default(),
        }
    }
}

impl <T: Default + Copy + Clone + Debug + AddAssign + SubAssign + From<u32> + PartialEq + Add<Output = T> + Sub<Output = T> + PartialOrd + Display + BitXor<Output = T>> Machine<T> {
    pub fn run(&mut self, code: &[Opcode]) -> () {
        self.frames.consume();

        loop {
            let result = self.execute(code);
                match result.outcome() {
                    ExecutionOutcome::CALL => {
                        likely(true);

                        self.frames.current_mut()
                            .seek_with_stack(result.tell(), result.stack());

                        self.frames.consume();
                    },
                    ExecutionOutcome::RETURN => {
                        likely(true);
                        let v = *self.stack.pop();

                        self.frames.pop();
                        self.frames.get_mut().resume();

                        self.stack.seek(result.stack());
                        self.stack.push(v);
                    },
                    ExecutionOutcome::TERMINATION => {
                        unlikely(true);
                        break;
                    }
                    _ => (),
                }
        }
    }

    #[inline]
    fn execute(&mut self, code: &[Opcode]) -> ExecutionResult {
        // let frame_no = self.frames.len();
        let frame = self.frames.get_mut();
        let mut ip = frame.tell();

        while likely(ip < code.len()) {
            let opcode = code[ip];

            // println!("#{}({:?}) - {} - {:?}", frame_no, frame.status(), ip, opcode.bytecode());
            ip += 1;
            frame.seek(ip);

            match opcode.bytecode() {
                Bytecode::CONST => {
                    let c = opcode.constant();

                    self.stack.push(c);
                }
                Bytecode::STORE => {
                    likely(true);
                    let val = *self.stack.pop();
                    frame.store(opcode.operand(0) as usize, val);
                }
                Bytecode::LOAD => {
                    likely(true);

                    self.stack.push(
                        *frame.load(opcode.operand(0) as usize)
                    );
                }
                Bytecode::ADD => {
                    frame.store(opcode.operand(2) as usize,
                        *frame.load(opcode.operand(0) as usize) +
                        *frame.load(opcode.operand(1) as usize)
                    );
                }
                Bytecode::SUB => {
                    frame.store(
                        opcode.operand(2) as usize,
                        *frame.load(opcode.operand(0) as usize) - *frame.load(opcode.operand(1) as usize)
                    );
                }
                Bytecode::LE => {
                    frame.store(
                        opcode.operand(2) as usize,
                        T::from((
                            frame.get(opcode.operand(0) as usize) < frame.get(opcode.operand(1) as usize
                        )) as u32),
                    );
                }
                Bytecode::GT => {
                    let rhs = *self.stack.pop();
                    let lhs = *self.stack.pop();

                    self.stack.push(T::from((lhs > rhs) as u32));
                }
                Bytecode::PRINT => println!("{}", frame.load(opcode.operand(0) as usize)),
                Bytecode::JMP => {
                    ip = opcode.operand(0) as usize;
                }
                Bytecode::JMPF => {
                    if likely(*frame.get(opcode.operand(1) as usize) == T::from(0 as u32)) {
                        ip = opcode.operand(0) as usize;
                    } 
                }
                Bytecode::JMPT => {
                    if likely(*self.stack.pop() == T::from(1 as u32)) {
                        ip = opcode.operand(0) as usize;
                    }
                }
                Bytecode::INC => frame.inc(opcode.operand(0) as usize),
                Bytecode::DEC => frame.dec(opcode.operand(0) as usize),
                Bytecode::CALL => {
                    likely(true);
                    frame.suspend();

                    return ExecutionResult::call(opcode.operand(0) as usize, self.stack.tell() - opcode.operand(1) as usize)
                }
                Bytecode::RETURN => {
                    likely(true);
                    frame.complete();

                    self.stack.push(*frame.load(opcode.operand(0) as usize));
                    return ExecutionResult::returns(frame.returns());
                }
                Bytecode::HALT => {
                    unlikely(true);
                    frame.terminate();
                    return ExecutionResult::terminate();
                }
                _ => {
                    unimplemented!("Code execution");
                }
            }
        }

        ExecutionResult::default()
    }
}
