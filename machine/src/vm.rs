use std::{fmt::{Debug, Display}, ops::{Add, AddAssign, Sub, SubAssign}};
use common::{likely, ArrayVec};

use crate::{Bytecode, Frame, Opcode, Stack};

pub struct Machine<T: Default + Copy + Clone + Debug> {
    halt: bool,
    frames: ArrayVec<Frame<T>, 1024>,
    stack: Stack<T, 1024>,
}

#[derive(Default, Copy, Clone)]
enum ExecutionOutcome {
    #[default]
    INVALID,
    CALL,
    RETURN,
    TERMINATION,
}

struct ExecutionResult {
    outcome: ExecutionOutcome,
    ip: Option<usize>,
    sp: Option<usize>,
}

impl ExecutionResult {
    pub fn returns(result: usize) -> Self {
        Self {
            outcome: ExecutionOutcome::RETURN,
            ip: None,
            sp: Some(result),
        }
    }

    pub fn call(ip: usize, sp: usize) -> Self {
        Self {
            outcome: ExecutionOutcome::CALL,
            ip: Some(ip),
            sp: Some(sp),
        }
    }

    pub fn terminate() -> Self {
        Self {
            outcome: ExecutionOutcome::TERMINATION,
            ip: None,
            sp: None,
        }
    }

    pub fn outcome(&self) -> ExecutionOutcome {
        self.outcome
    }

    pub fn cursor(&self) -> usize {
        if let Some(ip) = self.ip {
            let _ = likely(true);
            ip
        } else {
            0
        }
    }

    pub fn stack(&self) -> usize {
        if let Some(sp) = self.sp {
            let _ = likely(true);
            sp
        } else {
            0
        }
    }
}


impl <T: Default + Copy + Clone + Debug> Default for Machine<T> {
    fn default() -> Self {
        Self {
            halt: false,
            frames: ArrayVec::default(),
            stack: Stack::default(),
        }
    }
}

impl <T: Default + Copy + Clone + Debug + AddAssign + SubAssign + From<u32> + PartialEq + Add<Output = T> + Sub<Output = T> + PartialOrd + Display> Machine<T> {
    pub fn run(&mut self, code: &[Opcode]) -> () {
        self.frames.consume();

        while !self.halt {
            if let Some(result) = self.execute(code) {
                match result.outcome() {
                    ExecutionOutcome::CALL => {
                        self.frames.current_mut().seek(result.cursor());
                        self.frames.current_mut().with(result.stack());

                        self.frames.consume();
                    },
                    ExecutionOutcome::RETURN => {
                        let v = *self.stack.pop();

                        self.frames.pop();
                        self.frames.get_mut().resume();

                        self.stack.seek(result.stack());
                        self.stack.push(v);
                    },
                    ExecutionOutcome::TERMINATION => {
                        break;
                    }
                    _ => (),
                }
            }
        }
    }

    fn execute(&mut self, code: &[Opcode]) -> Option<ExecutionResult> {
        let frame = self.frames.get_mut();
        let mut ip = frame.tell();

        while ip < code.len() {
            let opcode = code[ip];

            // println!("#({:?}) - {} - {:?}", frame.status(), frame.tell(), opcode.bytecode());
            ip += 1;
            frame.seek(ip);

            match opcode.bytecode() {
                Bytecode::CONST => {
                    let c = opcode.constant();

                    self.stack.push(c);
                }
                Bytecode::STORE => {
                    let val = *self.stack.pop();
                    frame.store(opcode.operand(0) as usize, val);
                }
                Bytecode::LOAD => {
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
                Bytecode::PRINT => println!("OUTPUT: {}", frame.load(opcode.operand(0) as usize)),
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
                    frame.suspend();

                    return Some(ExecutionResult::call(opcode.operand(0).into(), self.stack.tell() - opcode.operand(1) as usize))
                }
                Bytecode::RETURN => {
                    frame.complete();

                    self.stack.push(*frame.load(opcode.operand(0) as usize));
                    return Some(ExecutionResult::returns(frame.returns()));
                }
                Bytecode::HALT => {
                    frame.terminate();
                    return Some(ExecutionResult::terminate());
                }
                _ => {
                    unimplemented!("Code execution");
                }
            }
        }

        None
    }
}
