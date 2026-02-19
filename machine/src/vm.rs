use std::{fmt::Write, io::Write as _};

use common::{
    ArchivedByte as Byte, ArchivedInstruction as Instruction, ArrayVec, SeekableIterator, Value,
    unlikely,
};

use crate::{Frame, Heap, ObjInstance, ObjString, Object, Stack};

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

// type External = fn(&[Value]) -> Value;

pub struct Machine<const S: usize> {
    heap: Heap,
    stack: Stack<Value, 8192>,
    frames: ArrayVec<Frame, S>,
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
    arity: usize,
}
impl ExecutionResult {
    pub fn returns() -> Self {
        Self {
            outcome: ExecutionOutcome::RETURN,
            arity: 0,
        }
    }

    pub fn call(arity: usize) -> Self {
        Self {
            outcome: ExecutionOutcome::CALL,
            arity,
        }
    }

    pub fn terminate() -> Self {
        Self {
            outcome: ExecutionOutcome::TERMINATION,
            arity: 0,
        }
    }

    pub fn invalid() -> Self {
        Self {
            outcome: ExecutionOutcome::INVALID,
            arity: usize::MAX,
        }
    }

    #[inline]
    pub fn outcome(&self) -> ExecutionOutcome {
        self.outcome
    }

    #[inline]
    pub fn arity(&self) -> usize {
        self.arity
    }
}

impl<const S: usize> Default for Machine<S> {
    fn default() -> Self {
        let mut frames = ArrayVec::default();
        frames.consume();
        Self {
            frames,
            heap: Heap::default(),
            stack: Stack::new(),
        }
    }
}

impl<const S: usize> Machine<S> {
    // pub fn register(&mut self, name: usize, func: External) {
    //     self.native.insert(name, func);
    // }
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

    pub fn run(&mut self, code: &[Byte]) {
        if code.is_empty() {
            return;
        }

        let mut code_iter = SeekableIterator::new(code);

        loop {
            let result = self.execute(&mut code_iter);

            match result.outcome() {
                ExecutionOutcome::CALL => {
                    self.frames.get_mut().seek(code_iter.tell() + 1);
                    // code_iter.seek(result.tell());
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
                _ => (),
            }
        }
    }

    #[inline(always)]
    fn execute(&mut self, code: &mut SeekableIterator<'_, Byte>) -> ExecutionResult {
        let frame_no = self.frames.len();

        let frame = self.frames.get_mut();

        while let Some(opcode) = code.next() {
            #[cfg(debug_assertions)]
            eprintln!(
                "#{:<2} @ {:0>4} - {:?}({:?}, {:?}) - {:?}",
                frame_no,
                code.tell(),
                opcode.bytecode(),
                opcode.operand_u16(0),
                opcode.operand_u16(1),
                self.stack.as_slice()
            );

            // #[cfg(debug_assertions)]
            // {
            //     println!("Performing GC trace");
            //     self.heap.trace(
            //         &self
            //             .stack
            //             .as_slice()
            //             .iter()
            //             .map(|v| v.raw() as u64)
            //             .collect::<Vec<u64>>(),
            //     );
            //
            //     println!("Performing GC collection");
            //     unsafe { self.heap.sweep() };
            // }

            match opcode.bytecode() {
                Instruction::POP => {
                    self.stack.pop();
                }
                Instruction::DUPLICATE => {
                    self.stack.push(*self.stack.peek());
                }
                Instruction::CONST => self.stack.push(Value::from(opcode.constant())),
                Instruction::STORE => {
                    let val = if self.stack.current() == opcode.operand_u32() as _ {
                        self.stack.peek()
                    } else {
                        &self.stack.pop()
                    };

                    self.stack[frame.get() + opcode.operand_u32() as usize] = *val;
                }
                Instruction::LOAD => {
                    self.stack
                        .push(self.stack[frame.get() + opcode.operand_u32() as usize]);
                }
                Instruction::INC => {
                    let lhs = *self.stack[frame.get() + opcode.operand_u32() as usize].inc();
                    self.stack.push(lhs);
                }
                Instruction::DEC => {
                    let lhs = *self.stack[frame.get() + opcode.operand_u32() as usize].dec();
                    self.stack.push(lhs);
                }
                Instruction::NOT => unary!(self.stack, !, as_bool),
                Instruction::NEG => unary!(self.stack, -, as_int),
                Instruction::ADD => binary!(self.stack, +, as_int),
                Instruction::SUB => binary!(self.stack, -, as_int),
                Instruction::MUL => binary!(self.stack, *, as_int),
                Instruction::DIV => binary!(self.stack, /, as_int),
                Instruction::MOD => binary!(self.stack, %, as_int),
                Instruction::LE => binary!(self.stack, <, raw),
                Instruction::LEQ => binary!(self.stack, <=, raw),
                Instruction::GT => binary!(self.stack, >, raw),
                Instruction::GEQ => binary!(self.stack, >=, raw),
                Instruction::EQ => binary!(self.stack, ==, raw),
                Instruction::NEQ => binary!(self.stack, !=, raw),
                Instruction::ADDF => binary!(self.stack, +, as_float, to_bits),
                Instruction::SUBF => binary!(self.stack, -, as_float, to_bits),
                Instruction::MULF => binary!(self.stack, *, as_float, to_bits),
                Instruction::DIVF => binary!(self.stack, /, as_float, to_bits),
                Instruction::MODF => binary!(self.stack, %, as_float, to_bits),
                Instruction::LEF => binary!(self.stack, <, as_float),
                Instruction::LEQF => binary!(self.stack, <=, as_float),
                Instruction::GTF => binary!(self.stack, >, as_float),
                Instruction::GEQF => binary!(self.stack, >=, as_float),
                Instruction::FORMAT => {
                    let params_count = opcode.operand_u32();
                    if params_count != 0 {
                        let mut params = ArrayVec::<Value, 8>::default();

                        for idx in (0..params_count as usize).rev() {
                            params[idx] = self.stack.pop();
                        }

                        let ptr = self.stack.pop().as_ptr::<ObjString>();
                        let format_string = (unsafe { &*ptr }).data.as_str();

                        let mut message = String::default();

                        let mut chars = format_string.chars().peekable();
                        while let Some(ch) = chars.next() {
                            if ch == '%' {
                                match chars.peek() {
                                    Some('i') => {
                                        chars.next();
                                        message.push_str(&params.pop().as_int().to_string());
                                    }
                                    Some('f') => {
                                        chars.next();
                                        // message
                                        //     .push_str(&format!("{:.?}", params.pop().as_float()));
                                        let _ =
                                            write!(&mut message, "{:.?}", params.pop().as_float());
                                    }
                                    Some('b') => {
                                        chars.next();
                                        let _ = write!(
                                            &mut message,
                                            "{:0b}",
                                            params.pop().raw().addr()
                                        );
                                    }
                                    Some('s') => {
                                        chars.next();
                                        let string_val =
                                            (unsafe { &*params.pop().as_ptr::<ObjString>() })
                                                .data
                                                .as_str();
                                        // Allocated::<crate::String>::new(params.pop().as_ptr());
                                        message.push_str(string_val);
                                    }
                                    Some('x') => {
                                        chars.next();
                                        let _ = write!(
                                            &mut message,
                                            "{:0x}",
                                            params.pop().raw().addr()
                                        );
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
                                        let _ = write!(
                                            &mut message,
                                            "{:08x}",
                                            params.pop().as_ptr::<bool>().addr()
                                        );
                                    }
                                    _ => {
                                        message.push('%');
                                    }
                                }
                            } else {
                                message.push(ch);
                            }
                        }

                        let (obj, _) = self
                            .heap
                            .alloc(ObjString::from(message.as_str()), Object::String);

                        self.stack.push(Value::from(obj.addr()));
                    }
                }
                Instruction::PRINT => {
                    let ptr = self.stack.pop().as_ptr::<ObjString>();
                    print!("{}", unsafe { &*ptr });
                }
                Instruction::JMP => {
                    code.seek(opcode.operand_u32() as usize);
                }
                Instruction::JMPF => {
                    if !self.stack.pop().as_bool() {
                        code.seek(opcode.operand_u32() as usize);
                    }
                }
                Instruction::JMPT => {
                    if self.stack.pop().as_bool() {
                        code.seek(opcode.operand_u32() as usize);
                    }
                }
                Instruction::CALL => {
                    return ExecutionResult::call(opcode.operand_u32() as usize);
                }
                Instruction::INIT => {
                    let (_, mut r) = self.heap.alloc(ObjInstance::default(), Object::Instance);
                    let _ = r.as_mut();

                    self.stack.push(Value::from(r.as_ptr().addr() as u64));
                }
                Instruction::RETURN => {
                    return ExecutionResult::returns();
                }
                Instruction::HALT => {
                    let _ = std::io::stdout().flush();
                    return ExecutionResult::terminate();
                }
                Instruction::STRING => {
                    let length = opcode.operand_u32() as usize;
                    let mut value: String = String::with_capacity(length);

                    while length != value.len()
                        && let Some(data) = code.next()
                    {
                        value.push(char::from_u32(data.operand_u32()).unwrap_or_default());
                    }

                    let (object, _) = self
                        .heap
                        .alloc(ObjString::from(value.as_str()), Object::String);

                    self.stack.push(Value::from(object.addr()));
                }
                Instruction::NOOP => continue,
                Instruction::VARIANT_SET => {
                    let field_count = opcode.operand_u16(1) as usize;
                    let tag = opcode.operand_u16(0) as usize;

                    let mut fields = Vec::with_capacity(field_count);
                    for _ in 0..field_count {
                        fields.push(self.stack.pop());
                    }

                    for field in fields {
                        self.stack.push(field);
                    }
                    self.stack.push(Value::from(tag as i64));
                }
                Instruction::MATCH_BRANCH => {
                    // Match_branch { tag: usize, offset: usize },
                    let tag = opcode.operand_u16(0) as usize;
                    let offset = opcode.operand_u32() as usize;

                    // Peek the top value (discriminant)
                    let discriminant = self.stack.peek().as_int() as usize;

                    if discriminant == tag {
                        code.seek(offset);
                    }
                }
                Instruction::MATCH_DEFAULT => {
                    // Match_default { offset: usize },
                    let offset = opcode.operand_u32() as usize;
                    code.seek(offset);
                }
                Instruction::VARIANT_POP => {
                    // Pop the variant value and its fields from stack
                    // The stack has: discriminant, field1, field2, ..., fieldN
                    // We need to pop the discriminant and keep the fields
                    // But we don't know field_count at runtime, so we'll pop just 1 for now
                    // and handle fields separately in match
                    self.stack.pop();
                }
                _ => return ExecutionResult::invalid(),
            }
        }

        ExecutionResult::terminate()
    }
}

// #[cfg(test)]
// mod tests {
//     use common::{ArchivedByte, Value};
//
//     use crate::{Byte, Instruction, Machine};
//
//     #[test]
//     fn test_constants() {
//         let mut vm = Machine::<1>::default();
//         let v = Value::from(42);
//         vm.run(&[
//             ArchivedByte::ew_with_value(Instruction::CONST, v.raw() as _),
//             Byte::new(Instruction::HALT),
//         ]);
//
//         assert_eq!(v.raw(), vm.pop().raw());
//     }
//
//     #[test]
//     fn test_addition() {
//         let cases = [
//             (
//                 Byte::new_with_value(Instruction::CONST, Value::from(2.0).raw() as _),
//                 Value::from(4.0),
//                 Instruction::ADDF,
//             ),
//             (
//                 Byte::new_with_value(Instruction::CONST, Value::from(2).raw() as _),
//                 Value::from(4),
//                 Instruction::ADD,
//             ),
//         ];
//
//         for (v, e, i) in cases {
//             let mut vm = Machine::<2>::default();
//             vm.run(&[v, v, Byte::new(i), Byte::new(Instruction::HALT)]);
//
//             assert_eq!(e.raw(), vm.pop().raw());
//         }
//     }
//
//     #[test]
//     fn test_subtraction() {
//         let cases = [
//             (
//                 Byte::new_with_value(Instruction::CONST, Value::from(2.0).raw() as _),
//                 Value::from(0.0),
//                 Instruction::SUBF,
//             ),
//             (
//                 Byte::new_with_value(Instruction::CONST, Value::from(2).raw() as _),
//                 Value::from(0),
//                 Instruction::SUB,
//             ),
//         ];
//
//         for (v, e, i) in cases {
//             let mut vm = Machine::<2>::default();
//             vm.run(&[v, v, Byte::new(i), Byte::new(Instruction::HALT)]);
//
//             assert_eq!(e.raw(), vm.pop().raw());
//         }
//     }
//
//     #[test]
//     fn test_multiplication() {
//         let cases = [
//             (
//                 Byte::new_with_value(Instruction::CONST, Value::from(2.0).raw() as _),
//                 Value::from(4.0),
//                 Instruction::MULF,
//             ),
//             (
//                 Byte::new_with_value(Instruction::CONST, Value::from(2).raw() as _),
//                 Value::from(4),
//                 Instruction::MUL,
//             ),
//         ];
//
//         for (v, e, i) in cases {
//             let mut vm = Machine::<2>::default();
//             vm.run(&[v, v, Byte::new(i), Byte::new(Instruction::HALT)]);
//
//             assert_eq!(e.raw(), vm.pop().raw());
//         }
//     }
//
//     #[test]
//     fn test_division() {
//         let cases = [
//             (
//                 Byte::new_with_value(Instruction::CONST, Value::from(2.0).raw() as _),
//                 Value::from(1.0),
//                 Instruction::DIVF,
//             ),
//             (
//                 Byte::new_with_value(Instruction::CONST, Value::from(2).raw() as _),
//                 Value::from(1),
//                 Instruction::DIV,
//             ),
//         ];
//
//         for (v, e, i) in cases {
//             let mut vm = Machine::<2>::default();
//             vm.run(&[v, v, Byte::new(i), Byte::new(Instruction::HALT)]);
//
//             assert_eq!(e.raw(), vm.pop().raw());
//         }
//     }
//
//     #[test]
//     fn test_jumps() {
//         let cases = [
//             (
//                 Byte::new_with_value(Instruction::CONST, Value::from(2.0).raw() as _),
//                 Value::from(1.0),
//                 Instruction::DIVF,
//             ),
//             (
//                 Byte::new_with_value(Instruction::CONST, Value::from(2).raw() as _),
//                 Value::from(1),
//                 Instruction::DIV,
//             ),
//         ];
//
//         for (v, e, i) in cases {
//             let mut vm = Machine::<2>::default();
//             vm.run(&[v, v, Byte::new(i), Byte::new(Instruction::HALT)]);
//
//             assert_eq!(e.raw(), vm.pop().raw());
//         }
//     }
// }
