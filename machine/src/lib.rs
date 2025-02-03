mod frame;
pub mod options;
mod utils;

use std::cmp::min;
use std::io::{stderr, stdout};

use common::memory::{Array, Memory, Object};
use common::Value;
use common::{
    error::{Error, ErrorOrigin},
    opcodes::{Byte, Code},
    ValueKind,
};
use frame::Frame;
use options::MachineOptions;
use parser::Program;
use utils::output::Output;

struct Suspension {
    program: Program<Byte>,
    memory: Memory,
}

// #[derive(PartialEq, Debug)]
// pub enum ExecutionStatus {
//     Complete,
//     Halt,
//     Pause,
// }

pub struct Machine {
    stdout: Output,
    stderr: Output,
    halt: bool,

    memento: Option<Program<Code>>,

    ip: usize,
    memory: Memory,
    frame: Frame,

    options: MachineOptions,
}

impl Default for Machine {
    fn default() -> Self {
        Self {
            halt: false,
            memento: None,
            ip: 0,
            memory: Memory::new(1024, 32),
            frame: Frame::default(),
            options: MachineOptions::default(),
            stdout: Output::new(&MachineOptions::default(), || Box::new(stdout().lock())),
            stderr: Output::new(&MachineOptions::default(), || Box::new(stderr().lock())),
        }
    }
}

impl Machine {
    pub fn with_options(options: MachineOptions) -> Self {
        let mut this = Self::default();
        this.options = options;
        this.stdout = Output::new(&this.options, || Box::new(stdout().lock()));
        this.stderr = Output::new(&this.options, || Box::new(stderr().lock()));

        this
    }

    fn execute(&mut self, code: Program<Code>) -> Result<ValueKind, Error> {
        let mut threads = vec![];

        while let Some(op) = code.get(self.ip) {
            eprintln!("#{:0>8} {:?}\t{:?}", self.ip, op.byte(), self.memory);
            match op.byte() {
                Byte::Halt => {
                    self.halt = true;
                }
                Byte::Push => {
                    if let Some(constant) = op.operand(0) {
                        if let Some(val) = code.constant(constant) {
                            self.memory.push(*val);
                        } else {
                            return Err(Error::new(
                                ErrorOrigin::RUNTIME,
                                format!("Unable to resolve constant {}", constant),
                            ));
                        }
                    } else {
                        return Err(Error::new(
                            ErrorOrigin::RUNTIME,
                            String::from("Opcode doesn't have operand"),
                        ));
                    }
                }
                Byte::Pop => {
                    self.memory.pop();
                }
                Byte::Array => {
                    if let Some(size) = op.operand(0) {
                        let mut items = Vec::with_capacity(size);
                        for _ in 0..size {
                            if let Some(item) = self.memory.pop() {
                                items.insert(0, *item.kind());
                            }
                        }

                        let arr = Array::with_items(items);

                        let key = self.memory.alloc(common::memory::Object::Array(arr));
                        self.memory.push(Value::new(ValueKind::ARRAY(key)));
                    } else {
                        unreachable!("Missing array size");
                    }
                }
                Byte::AddInteger => match (
                    self.memory.pop().map(|v| *v.kind()),
                    self.memory.pop().map(|v| *v.kind()),
                ) {
                    (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                        self.memory.push(Value::new(ValueKind::INTEGER(lhs + rhs)));
                    }
                    _ => {
                        return Err(Error::new(
                            ErrorOrigin::RUNTIME,
                            String::from("Operands do not match expected type 'int'"),
                        ))
                    }
                },
                Byte::AddFloat => match (
                    self.memory.pop().map(|v| *v.kind()),
                    self.memory.pop().map(|v| *v.kind()),
                ) {
                    (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
                        self.memory.push(Value::new(ValueKind::FLOAT(lhs + rhs)))
                    }
                    _ => {
                        return Err(Error::new(
                            ErrorOrigin::RUNTIME,
                            String::from("Operands do not match expected type 'float'"),
                        ))
                    }
                },
                Byte::Range => match (
                    self.memory.pop().map(|v| *v.kind()),
                    self.memory.pop().map(|v| *v.kind()),
                ) {
                    (Some(ValueKind::INTEGER(end)), Some(ValueKind::INTEGER(start))) => {
                        self.memory.push(Value::new(ValueKind::RANGE(start, end)));
                    }
                    _ => unreachable!("Range bounds must be an integer"),
                },
                Byte::Print => {
                    self.stdout
                        .write(match self.memory.pop().map(|v| *v.kind()) {
                            Some(ValueKind::NONE) => String::new(),
                            Some(ValueKind::BOOLEAN(bit)) => format!("{}", bit),
                            Some(ValueKind::INTEGER(num)) => format!("{}", num),
                            Some(ValueKind::FLOAT(num)) => format!("{:.?}", num),
                            Some(ValueKind::STRING(num)) => {
                                if let Some(str) = code.string(num) {
                                    format!("{}", str)
                                } else {
                                    String::new()
                                }
                            }
                            Some(ValueKind::RANGE(start, end)) => format!("{}..{}", start, end),
                            Some(ValueKind::ARRAY(key)) => {
                                if let Some(Object::Array(arr)) = self.memory.lookup(key) {
                                    let mut formatted = "[".to_string();
                                    let count = min(3, arr.len());
                                    for i in 0..count {
                                        if let Some(item) = arr.item(count - (count - i)) {
                                            formatted = format!("{}{}, ", formatted, item);
                                        }
                                    }
                                    if count < arr.len() {
                                        formatted = format!("{}...", formatted);
                                    }

                                    formatted = format!("{}]", formatted);

                                    formatted
                                } else {
                                    String::new()
                                }
                            }
                            _ => String::new(),
                        });

                    if op.operand(0).is_some() {
                        self.stdout.write("\n".to_string());
                    }
                }

                Byte::Pause => {
                    self.memento = Some(code);

                    break;
                }
                Byte::Spawn => {
                    let program = code.clone();

                    threads.push(Some(std::thread::spawn(|| {
                        let mut vm = Machine::default();
                        vm.ip = 1024;

                        vm.run(program).unwrap_or_default()
                    })));
                }
                Byte::Join => {
                    if let Some(thread) = threads[1024].take() {
                        match thread.join() {
                            Ok(value) => {
                                threads[1024] = None;
                                // todo!("Handle properly VM return values");

                                self.memory.push(Value::new(value));
                            }
                            Err(_) => {
                                self.halt = true;
                                panic!("Thread error handle remainder");
                            }
                        }
                    }
                }
                Byte::Jump => {
                    if let Some(offset) = op.operand(0) {
                        self.ip += offset - 1
                    }
                }
                Byte::Jumpz => {
                    if let Some(offset) = op.operand(0) {
                        if let Some(val) = self.memory.pop() {
                            if *val.kind() != ValueKind::BOOLEAN(true) {
                                self.ip += offset;
                            }
                        } else {
                            unreachable!("No value on stack");
                        }
                    } else {
                        unreachable!("No offset for conditional jump?");
                    }
                }
                Byte::Leave => {
                    self.leave();
                }
                Byte::Enter => {
                    self.enter();
                }
                Byte::Scope => {
                    self.frame = self.frame.scope();
                }
                Byte::Equal => match (self.memory.pop(), self.memory.pop()) {
                    (Some(rhs), Some(lhs)) => {
                        self.memory.push(Value::new(ValueKind::BOOLEAN(lhs == rhs)));
                    }
                    _ => self.memory.push(Value::new(ValueKind::BOOLEAN(false))),
                },
                _ => (),
            }

            if self.halt {
                break;
            }

            // if self.memory.size() > self.options.memory().limit() {
            //     return Err(Error::new(
            //         ErrorOrigin::RUNTIME,
            //         format!(
            //             "Ran out of memory, current usage is {} bytes out of maximum allowed {} bytes",
            //             self.memory.size(),
            //             self.options.memory().limit(),
            //         ),
            //     ));
            // }

            self.ip += 1;
        }

        if self.halt || self.memento.is_some() {}

        if self.halt {
            return Ok(ValueKind::NONE);
        }
        if self.memento.is_some() {
            return Ok(ValueKind::NONE);
        }

        let result = if !self.halt && self.memento.is_none() {
            if self.memory.len() == 0 {
                ValueKind::default()
            } else {
                *self.memory.pop().unwrap_or_default().kind()
            }
        } else {
            ValueKind::NONE
        };

        Ok(result)
    }

    pub fn resume(&mut self) -> Result<ValueKind, Error> {
        if let Some(program) = self.memento.take() {
            self.memento = None;
            self.execute(program)
        } else {
            Err(Error::new(
                ErrorOrigin::RUNTIME,
                "Attempting to resume a non-suspended program".to_string(),
            ))
        }
    }

    pub fn run(&mut self, code: Program<Code>) -> Result<ValueKind, Error> {
        self.execute(code)
    }

    pub fn enter(&mut self) {
        let frame = self.frame.clone();
        self.frame = Frame::new(self.ip, self.memory.len());
        self.frame.with_parent(frame);
    }

    pub fn leave(&mut self) {
        let f = self.frame.parent();
        let val = self.memory.pop();

        self.memory.truncate(self.frame.stack());
        self.memory.push(val.unwrap_or_default());

        if !self.frame.is_scoped() {
            self.ip = self.frame.tell();
        }
        self.frame = f.cloned().unwrap_or_default();
    }
}

#[cfg(test)]
mod tests {
    use common::{
        interner::Interner,
        opcodes::{Byte, Code},
        Value, ValueKind,
    };
    use parser::Program;

    use crate::Machine;

    #[test]
    fn test_vm_execution() {
        let mut values = Interner::default();
        let num = values.intern(Value::new(ValueKind::INTEGER(2)));
        let mut constant = Code::new(Byte::Push);
        constant.with_operands(vec![num]);

        let program = Program::new(
            vec![
                constant.clone(),
                constant.clone(),
                Code::new(Byte::AddInteger),
                Code::new(Byte::Print),
            ],
            values,
            Interner::default(),
            Interner::default(),
        );
        let result = Machine::default().run(program);

        assert!(result.is_ok());
        assert_eq!(result, Ok(ValueKind::NONE));
    }
}
