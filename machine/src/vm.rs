use std::{collections::HashMap, io::Write};

use common::{ArrayVec, SeekableIterator, Value, unlikely};

use crate::{Allocated, Byte, Coroutine, Frame, Heap, Instruction, Stack};

macro_rules! binary {
    ($stack: expr, $op:tt, $from: ident, $to: ident) => {
        {
            let rhs = $stack.pop().$from();
            let lhs = $stack.peek().$from();

            $stack.top().replace((lhs $op rhs).$to());
        }
    };
    ($stack: expr, $op:tt, $from: ident) => {
        {
            let rhs = $stack.pop().$from();
            let lhs = $stack.peek().$from();

            $stack.top().replace((lhs $op rhs) as _)
        }
    };
}

macro_rules! unary {
    ($stack: expr, $op: tt, $from: ident, $to: ident) => {
        {
        let rhs = $stack.peek().$from();

        $stack.top().replace(($op rhs).$to());
        }
    };
    ($stack: expr, $op: tt, $from: ident) => { {
            let rhs = $stack.peek().$from();

            $stack.top().replace(($op rhs) as _);
        }
    }
}

type External = fn(&[Value]) -> Value;

pub struct Machine<const S: usize> {
    heap: Heap,
    stack: Stack<Value, 2048>,
    frames: ArrayVec<Frame, S>,
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
            heap: Heap::new(),
            stack: Stack::new(),
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
        self.stack.push(value);
    }

    #[cfg(test)]
    pub fn pop(&mut self) -> Value {
        self.stack.pop()
    }

    #[cfg(test)]
    pub fn tell(&self) -> usize {
        self.stack.tell()
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
                    self.frames
                        .current_mut()
                        .set(self.stack.tell() - result.arity());

                    self.frames.consume();
                }
                ExecutionOutcome::RETURN => {
                    let current = self.frames.pop();
                    let v = self.stack.pop();
                    self.stack.seek(current.get());
                    self.stack.push(v);

                    let prev = self.frames.get_mut();

                    code_iter.seek(prev.tell());
                }
                ExecutionOutcome::TERMINATION => {
                    unlikely(true);
                    break;
                }
                ExecutionOutcome::RESUME => {
                    unlikely(true);
                    self.frames.get_mut().seek(code_iter.tell());
                    let coro = Allocated::<Coroutine<Value>>::new(self.stack.pop().as_ptr());

                    self.frames
                        .push((coro.as_ref().ip(), self.stack.tell()).into());
                    code_iter.seek(coro.as_ref().ip());
                    self.stack.append(coro.as_ref().stack());
                }
                _ => (),
            }
        }
    }

    #[inline(always)]
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
                    self.stack.slice(frame.get(), self.stack.tell())
                );
            }

            match opcode.bytecode() {
                Instruction::POP => {
                    self.stack.pop();
                }
                Instruction::CONST => self.stack.push(opcode.constant()),
                Instruction::STORE => {
                    let val = self.stack.peek();
                    self.stack[frame.get() + opcode.operand(0)] = *val;
                }
                Instruction::LOAD => {
                    self.stack.push(self.stack[frame.get() + opcode.operand(0)]);
                }
                Instruction::NOT => unary!(self.stack, !, as_bool),
                Instruction::NEG => unary!(self.stack, -, as_int),
                Instruction::ADD => binary!(self.stack, +, as_int),
                Instruction::SUB => binary!(self.stack, -, as_int),
                Instruction::MUL => binary!(self.stack, *, as_int),
                Instruction::DIV => binary!(self.stack, /, as_int),
                Instruction::MOD => binary!(self.stack, %, as_int),
                Instruction::LE => binary!(self.stack, <, raw),
                Instruction::GT => binary!(self.stack, >, raw),
                Instruction::EQ => binary!(self.stack, ==, raw),
                Instruction::LEF => binary!(self.stack, <=, raw),
                Instruction::GTF => binary!(self.stack, >=, raw),
                Instruction::NEQ => binary!(self.stack, !=, raw),
                Instruction::ADDF => binary!(self.stack, +, as_float, to_bits),
                Instruction::SUBF => binary!(self.stack, -, as_float, to_bits),
                Instruction::MULF => binary!(self.stack, *, as_float, to_bits),
                Instruction::DIVF => binary!(self.stack, /, as_float, to_bits),
                Instruction::MODF => binary!(self.stack, %, as_float, to_bits),
                Instruction::FORMAT => {
                    if opcode.operand(0) != 0 {
                        let mut params = ArrayVec::<Value, 8>::default();

                        for idx in (0..opcode.operand(0)).rev() {
                            params[idx] = self.stack.pop();
                        }

                        let format_string = Allocated::<String>::new(self.stack.peek().as_ptr());
                        let format_str = format_string.as_ref().as_str();

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
                                        message
                                            .push_str(&format!("{:0b}", params.pop().raw().addr()));
                                    }
                                    Some('s') => {
                                        chars.next();
                                        let string_val =
                                            Allocated::<String>::new(params.pop().as_ptr());
                                        message.push_str(string_val.as_ref().as_str());
                                    }
                                    Some('x') => {
                                        chars.next();
                                        message
                                            .push_str(&format!("{:0x}", params.pop().raw().addr()));
                                    }
                                    Some('z') => {
                                        chars.next();
                                        message.push_str(if params.pop().raw() > 0 as _ {
                                            "true"
                                        } else {
                                            "false"
                                        });
                                    }
                                    Some('u') => {
                                        chars.next();
                                        message.push_str(&params.pop().raw().addr().to_string());
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
                        let collectable = self.heap.alloc::<crate::String>(message.into());
                        *self.stack.top() = Value::from(collectable.ptr() as u64);
                    }
                }
                Instruction::PRINT => {
                    print!(
                        "{}",
                        Allocated::<String>::new(self.stack.peek().as_ptr())
                            .as_ref()
                            .as_str()
                    );
                }
                Instruction::JMP => {
                    code.seek(opcode.operand(0));
                }
                Instruction::JMPF => {
                    if !self.stack.pop().as_bool() {
                        code.seek(opcode.operand(0));
                    }
                }
                Instruction::JMPT => {
                    if self.stack.pop().as_bool() {
                        code.seek(opcode.operand(0));
                    }
                }
                Instruction::CALL => {
                    return ExecutionResult::call(opcode.operand(0), opcode.operand(1));
                }
                // Instruction::NATIVE => {
                //     let arity = opcode.operand(1);
                //     let args = (0..arity).map(|_| *frame.pop()).collect::<Vec<_>>();
                //     let result = self.native[&opcode.operand(0)](&args);
                //
                //     frame.push(result);
                // }
                Instruction::RETURN => {
                    return ExecutionResult::returns();
                }
                Instruction::SUSP => {
                    let suspended_frame = (code.tell(), frame.get());

                    let collectable = self.heap.alloc(Coroutine::new(
                        suspended_frame,
                        self.stack.slice(frame.tell(), self.stack.tell() - 1),
                    ));

                    self.stack.push(Value::from(collectable.ptr() as u64));

                    return ExecutionResult::returns();
                }
                Instruction::RESUME => {
                    return ExecutionResult::resume();
                }
                Instruction::HALT => {
                    let _ = std::io::stdout().flush();
                    return ExecutionResult::terminate();
                }
                Instruction::STRING => {
                    let length = opcode.operand(0);
                    let mut value: String = String::with_capacity(length);

                    while length != value.len()
                        && let Some(data) = code.next()
                    {
                        value.push(char::from_u32(data.operand(0) as u32).unwrap_or_default());
                    }

                    let collectable = self.heap.alloc::<crate::String>(value.into());

                    self.stack.push(Value::from(collectable.ptr() as u64));
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
