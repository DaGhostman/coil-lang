use common::memory::object::{ObjArray, ObjInstance, Objects};
use common::program::data::Data;
use rustc_hash::FxHashMap as HashMap;
use std::io::{stderr, stdout};

use crate::options::MachineOptions;
use crate::utils::output::Output;
use common::memory::{Heap, Stack};
use common::program::program::Program;
use common::{
    Value,
    opcodes::{Byte, Code},
};

const FRAMES: usize = 2048;
const STACK: usize = 8192;

pub struct Machine {
    stdout: Output,
    stderr: Output,
    halt: bool,

    ip: usize,
    fp: usize,
    // Figure out a way to not use
    call_stack: [(usize, usize); FRAMES],
    stack: Stack<Value, STACK>,
    heap: Heap,
    options: MachineOptions,
}
impl Default for Machine {
    fn default() -> Self {
        Self {
            halt: false,
            ip: 0,
            fp: 1,
            call_stack: [(0, 0); FRAMES],
            stack: Stack::new(),
            heap: Default::default(),
            stdout: Output::new(&MachineOptions::default(), || Box::new(stdout().lock())),
            stderr: Output::new(&MachineOptions::default(), || Box::new(stderr().lock())),
            options: MachineOptions::default(),
        }
    }
}

macro_rules! binary_op {
    ($rhs: expr, $lhs: expr, $op: tt) => {
            $lhs $op $rhs
    };
    ($rhs: expr, $lhs: expr, $op: tt, $kind: ident) => {{
        Value::$kind($lhs $op $rhs)
    }};
}

macro_rules! unary_op {
    ($rhs: expr, $op: tt) => {
            $op $rhs
    };
}

macro_rules! binary_handler {
    ($this: expr, $op: tt) => {{
        let rhs = $this.stack.pop();
        let lhs = $this.stack.pop();

        $this.stack.push(binary_op!(rhs, lhs, $op));
    }};
    ($this: expr, $op: tt, $type: ident) => {{
        let rhs = $this.stack.pop();
        let lhs = $this.stack.pop();

        $this.stack.push(binary_op!(rhs, lhs, $op, $type));
    }};
}

macro_rules! unary_handler {
    ($this: expr, $op: tt) => {{
        let rhs = $this.stack.pop();

        $this.stack.push(unary_op!(rhs, $op));
    }};
}

impl Machine {
    pub fn with_options(options: MachineOptions) -> Self {
        let mut this = Self::default();
        this.options = options;
        this.stdout = Output::new(&this.options, || Box::new(stdout().lock()));
        this.stderr = Output::new(&this.options, || Box::new(stderr().lock()));
        this.heap = Heap::new(this.options.gc_growth(), this.options.gc_threshold());
        this.stack = Stack::new();

        this
    }

    #[allow(clippy::too_many_lines)]
    fn execute(&mut self, code: &Program<Code>, data: &Data) {
        self.ip = 0;
        // for (let i = 0; i < code.len(); i++) {
        while let Some(op) = code.get(self.ip) {
            #[cfg(feature = "trace")]
            {
                eprintln!(
                    "({:0>2})#{:0>8} {:?}\t{:?}",
                    self.fp,
                    self.ip,
                    op.byte(),
                    self.stack.iter().collect::<Vec<Value>>()
                );
            }
            #[cfg(feature = "trace")]
            {
                if self.fp > 32 {
                    break;
                }
            }
            match op.byte() {
                Byte::Call => {
                    if let Value::FUNCTION(arity, position) = self.stack.pop() {
                        if op.operand(1) == 1 {
                            self.fp = 0;
                        }

                        self.enter(arity);
                        self.ip = position;
                        continue;
                    }
                }
                Byte::Leave => {
                    self.leave();
                    // continue;
                }
                Byte::Push => {
                    let constant = data.constant(op.operand(0));
                    self.stack.push(*constant);
                }
                Byte::Pop => {
                    self.stack.npop(op.operand(0));
                }
                Byte::Duplicate => {
                    self.stack.push(self.stack.peek(0));
                }
                Byte::Store => self
                    .stack
                    .pop_to(self.call_stack[self.fp - 1].1 + op.operand(0)),
                Byte::Load => {
                    let pos = self.call_stack[self.fp - 1].1 + op.operand(0);
                    if let Value::OBJECT(_) = self.stack.peek_at(pos) {
                        self.stack.push(Value::REFERENCE(pos));
                    } else {
                        self.stack.copy_to_top(pos)
                    }
                }
                Byte::Not => unary_handler!(self, !),
                Byte::Negate => unary_handler!(self, -),
                Byte::Less => binary_handler!(self, <, BOOLEAN),
                Byte::LessEqual => binary_handler!(self, <=, BOOLEAN),
                Byte::Greater => binary_handler!(self, >, BOOLEAN),
                Byte::GreaterEqual => binary_handler!(self, >=, BOOLEAN),
                Byte::Equal => binary_handler!(self, ==, BOOLEAN),
                Byte::Add => binary_handler!(self, +),
                Byte::Sub => binary_handler!(self, -),
                Byte::Mul => binary_handler!(self, *),
                Byte::Div => binary_handler!(self, /),
                Byte::Mod => binary_handler!(self, %),
                Byte::LShift => binary_handler!(self, <<),
                Byte::RShift => binary_handler!(self, >>),
                Byte::Xor => binary_handler!(self, ^),
                Byte::And => binary_handler!(self, &),
                Byte::Or => binary_handler!(self, |),
                Byte::Print => {
                    self.stdout.write(&match self.stack.pop() {
                        Value::STR(idx) => data.string(idx).to_string(),
                        value => value.to_string(),
                    });

                    if op.operand(0) == 1 {
                        self.stdout.write("\n");
                    }
                }
                Byte::Jump => {
                    self.ip = op.operand(0);
                    continue;
                }
                Byte::Jumpz => {
                    let value = self.stack.pop();

                    match value {
                        Value::NONE => {
                            self.ip = op.operand(0);
                            continue;
                        }
                        Value::BOOLEAN(byte) => {
                            if !byte {
                                self.ip = op.operand(0);
                                continue;
                            }
                        }
                        _ => (),
                    };
                }
                Byte::Range => {
                    let last = self.stack.pop();
                    let first = self.stack.pop();

                    if let (Value::INTEGER(start), Value::INTEGER(end)) = (first, last) {
                        let items: Vec<Value> = (start..end).map(Value::from).collect();

                        self.gc();
                        let (obj_array, _) = self.heap.alloc(ObjArray::from(items), Objects::Array);

                        self.stack.push(Value::OBJECT(obj_array));
                    }
                }
                Byte::Array => {
                    // likely(true);
                    let len = op.operand(0);
                    let mut items = Vec::with_capacity(len);
                    items.copy_from_slice(self.stack.npop(len));

                    self.gc();
                    let (obj_array, _) = self.heap.alloc(ObjArray::from(items), Objects::Array);
                    self.stack.push(Value::OBJECT(obj_array));
                }
                Byte::Iterate => {
                    let iter = self.stack.pop();
                    let arr = self.stack.peek_at(self.stack.tell(1));
                    if let (Value::ITERATOR(n), Value::OBJECT(Objects::Array(arr))) = (iter, arr) {
                        if n >= arr.as_ref().len() {
                            self.ip = op.operand(0);
                            continue;
                        }

                        self.stack.push(Value::ITERATOR(n + 1));
                        self.stack.pop_to(op.operand(1));
                        self.stack.push(*arr.as_ref().item(n));
                    } else {
                        todo!(
                            "Handle cases where value is not an array, but object that actually implements iterator interface or error if it is not a valid iterable"
                        );
                    }
                }
                Byte::Instantiate => {
                    let owner = op.operand(0);
                    let object = ObjInstance::new(owner);
                    // if self.properties.contains_key(&owner) {
                    //     for (idx, field) in self.properties[&owner].iter().enumerate() {
                    //         if idx == 0 {
                    //             continue;
                    //         }
                    //         object.update(*field, Default::default());
                    //     }
                    // }
                    self.gc();
                    let (instance, _) = self.heap.alloc(object, Objects::Object);

                    self.stack.push(Value::OBJECT(instance));
                }
                Byte::Invoke => {
                    let [ip, arity, _] = op.operands();
                    // let operands = op.operands();
                    // let arity = operands[1];

                    if let Value::OBJECT(Objects::Object(_)) = self.peek_obj(*arity) {
                        self.enter(arity + 1);
                        self.ip = *ip;
                        continue;
                    } else {
                        eprintln!("Attempting to call method on non-object");
                    }
                }
                Byte::Prop => {
                    let [name, action, ..] = op.operands();

                    match action {
                        0 => {
                            if let Value::OBJECT(Objects::Object(object)) = self.peek_obj(0) {
                                self.stack.pop();
                                self.stack
                                    .push(if let Some(value) = object.as_ref().get(*name) {
                                        *value
                                    } else {
                                        Value::default()
                                    });
                            }
                        }
                        1 => {
                            if let Value::OBJECT(Objects::Object(mut object)) = self.peek_obj(1) {
                                object.as_mut().update(*name, self.stack.pop());
                                self.stack.pop();
                            }
                        }
                        _ => (),
                    }
                }
                Byte::Upvalue => {
                    let [frame, _, upvalue] = op.operands();
                    // let frame = op.operand(0);
                    // let upvalue = op.operand(2);

                    self.stack.copy_to_top(self.call_stack[*frame].1 + *upvalue);
                }
                Byte::Halt => {
                    self.halt = true;
                }
                _ => (),
            }

            #[cfg(feature = "stress")]
            {
                self.gc()
            }

            if self.halt || self.fp == 0 {
                break;
            }

            self.ip += 1;
        }
    }

    pub fn run(&mut self, code: &Program<Code>, data: &Data) {
        self.execute(code, data);
    }

    fn gc(&mut self) {
        #[cfg(not(feature = "stress"))]
        if self.heap.size() <= self.heap.threshold() {
            return;
        }

        self.mark_sweep();
    }

    // #[inline]
    fn mark_sweep(&mut self) {
        let mut grey_objects = Vec::with_capacity(32);
        self.mark_roots(&mut grey_objects);
        while let Some(object) = grey_objects.pop() {
            object.mark_references(&mut grey_objects);
        }

        self.heap.sweep();
    }

    // #[inline]
    fn mark_roots(&mut self, grey_objects: &mut Vec<Objects>) {
        grey_objects.clear();
        for value in &self.stack {
            match value {
                Value::OBJECT(mut o) | Value::STRING(mut o) => o.mark(grey_objects),
                _ => (),
            }
        }
    }

    // #[inline]
    fn enter(&mut self, arity: usize) {
        self.call_stack[self.fp] = (self.ip, self.stack.tell(arity));
        self.fp += 1;
    }

    // #[inline]
    fn leave(&mut self) {
        self.fp -= 1;

        // let (ip, stack) = self.call_stack.get(self.fp);
        let (ip, stack) = self.call_stack[self.fp];

        self.ip = ip;
        self.stack.restore(stack);
    }

    // // #[inline]
    // fn lookup(&mut self, name: usize) -> usize {
    //     *self.variables[self.fp].get(name)
    // }
    //
    // fn lookup_upvalue(&mut self, frame: usize, name: usize) -> usize {
    //     *self.variables[frame].get(name)
    // }
    //
    // // #[inline]
    // fn reassign(&mut self, symbol: usize, position: usize) {
    //     self.variables[self.fp].insert(symbol, position);
    // }

    // #[inline]
    fn peek_obj(&self, position: usize) -> Value {
        match self.stack.peek(position) {
            Value::REFERENCE(n) => self.stack.peek_at(n),
            v => v,
        }
    }
}

#[cfg(test)]
mod tests {
    use common::program::data::Data;
    use common::program::program::Program;
    use common::types::Type;
    use common::{
        Value,
        opcodes::{Byte, Code},
    };

    use crate::stack::Machine;

    #[test]
    fn test_integer_addition() {
        let mut values = Data::default();
        let num = values.add_constant(Value::INTEGER(2), Type::integer());
        let mut constant = Code::new(Byte::Push);
        constant.with_operands([num, 0, 0]);

        let mut program = Program::new(vec![constant, constant, Code::new(Byte::Add)]);
        Machine::default().run(&program, &values);
    }

    #[test]
    fn test_float_addition() {
        let mut values = Data::default();
        let a = Code::new_with_operands(
            Byte::Push,
            [values.add_constant(Value::FLOAT(0.8), Type::float()), 0, 0],
        );
        let b = Code::new_with_operands(
            Byte::Push,
            [values.add_constant(Value::FLOAT(0.1), Type::float()), 0, 0],
        );

        let mut program = Program::new(vec![a, b, Code::new(Byte::Add)]);

        Machine::default().run(&program, &values);
    }

    #[test]
    fn test_int_float_addition() {
        let mut values = Data::default();
        let a = Code::new_with_operands(
            Byte::Push,
            [
                values.add_constant(Value::INTEGER(8), Type::integer()),
                0,
                0,
            ],
        );
        let b = Code::new_with_operands(
            Byte::Push,
            [values.add_constant(Value::FLOAT(0.1), Type::float()), 0, 0],
        );

        let mut program = Program::new(vec![a, b, Code::new(Byte::Add)]);

        Machine::default().run(&program, &values);
    }

    #[test]
    fn test_float_int_addition() {
        let mut values = Data::default();
        let a = Code::new_with_operands(
            Byte::Push,
            [values.add_constant(Value::FLOAT(0.8), Type::float()), 0, 0],
        );
        let b = Code::new_with_operands(
            Byte::Push,
            [
                values.add_constant(Value::INTEGER(1), Type::integer()),
                0,
                0,
            ],
        );

        let mut program = Program::new(vec![a, b, Code::new(Byte::Add)]);

        Machine::default().run(&program, &values);
    }
}
