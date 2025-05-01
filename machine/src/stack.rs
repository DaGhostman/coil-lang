use common::memory::object::{
    ObjArray, ObjCoroutine, ObjInstance, ObjIterator, ObjString, Objects,
};
use common::program::data::Data;
use common::types::Type;
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
const STACK: usize = i16::MAX as usize;

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
    #[must_use]
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
                if self.fp > 32 || self.stack.len() > 16 {
                    break;
                }
            }
            match op.byte() {
                Byte::Call => {
                    let [arity, is_entrypoint, ..] = op.operands();
                    // if let Value::FUNCTION(arity, position) = self.stack.pop() {
                    //     if *is_entrypoint == 1 {
                    //         self.fp = 0;
                    //     }
                    //
                    //     self.enter(arity);
                    //     self.ip = position;
                    //     continue;
                    // }

                    let value = self.peek_obj(0);
                    self.stack.pop();
                    // if let Value::REFERENCE(idx) = value {
                    //     value = self.stack.peek_at(idx);
                    // }

                    match value {
                        Value::FUNCTION(arity, position) => {
                            if *is_entrypoint == 1 {
                                self.fp = 0;
                            }

                            self.enter(arity);
                            self.ip = position;
                            continue;
                        }
                        Value::OBJECT(Objects::Coroutine(coro)) => {
                            if *arity > 1 {
                                unreachable!(
                                    "Resuming a suspended coroutine with multiple values is not allowed"
                                );
                            }
                            let value = self.stack.pop();
                            let (ip, stack) = coro.as_ref().resume();
                            self.enter(0);
                            self.ip = ip;
                            for item in stack {
                                self.stack.push(*item);
                            }
                            self.stack.push(value);
                        }
                        value => unreachable!("Value '{value}' is not callable"),
                    }
                }
                Byte::Leave => {
                    self.leave();
                }
                Byte::Push => {
                    let constant = data.constant(op.operand(0));
                    self.stack.push(*constant);
                }
                Byte::Pop => {
                    self.stack.npop(op.operand(0).min(self.stack.len()));
                }
                Byte::Duplicate => {
                    self.stack.push(self.stack.peek(0));
                }
                Byte::Store => {
                    let source = self.stack.tell(1);
                    let pos = self.call_stack[self.fp - 1].1 + op.operand(0);

                    self.stack.copy(source, pos);
                }
                Byte::Load => {
                    let pos = self.call_stack[self.fp - 1].1 + op.operand(0);
                    // let pos = op.operand(0);
                    if let Value::OBJECT(_) = self.stack.peek_at(pos) {
                        self.stack.push(Value::REFERENCE(pos));
                    } else {
                        self.stack.copy_to_top(pos);
                    }
                }
                Byte::Not => unary_handler!(self, !),
                Byte::Negate => unary_handler!(self, -),
                Byte::Less => binary_handler!(self, <, BOOLEAN),
                Byte::LessEqual => binary_handler!(self, <=, BOOLEAN),
                Byte::Greater => binary_handler!(self, >, BOOLEAN),
                Byte::GreaterEqual => binary_handler!(self, >=, BOOLEAN),
                Byte::Equal => binary_handler!(self, ==, BOOLEAN),
                Byte::Yield => {
                    let result = self.stack.pop();
                    let stack = self
                        .stack
                        .npop(self.stack.tell(0) - self.call_stack[self.fp - 1].1);
                    let ip = self.ip;

                    let (obj, mut coro) =
                        self.heap.alloc(ObjCoroutine::default(), Objects::Coroutine);
                    coro.as_mut().suspend(ip, stack.to_vec());
                    coro.as_mut().set(result);

                    self.stack.push(Value::OBJECT(obj));
                    self.leave();
                }
                Byte::Add => match (self.stack.peek(1), self.stack.peek(0)) {
                    (Value::STRING(Objects::String(lhs)), Value::STRING(Objects::String(rhs))) => {
                        if rhs.as_ref().len() == 0 {
                            let _ = self.stack.npop(2)[1];
                        } else if lhs.as_ref().len() == 0 {
                            let l = self.stack.npop(2)[0];

                            self.stack.push(l);
                        } else {
                            let (obj_string, _) = self.heap.alloc(
                                ObjString::from(format!("{}{}", lhs.as_ref(), rhs.as_ref())),
                                Objects::String,
                            );

                            self.stack.npop(2);
                            self.stack.push(Value::STRING(obj_string));
                        }
                    }
                    (Value::STRING(Objects::String(lhs)), Value::STR(idx)) => {
                        let rhs = data.string(idx);
                        let (obj_string, _) = self.heap.alloc(
                            ObjString::from(format!("{}{}", lhs.as_ref(), rhs)),
                            Objects::String,
                        );

                        self.stack.npop(2);
                        self.stack.push(Value::STRING(obj_string));
                    }
                    (Value::STR(idx), Value::STRING(Objects::String(rhs))) => {
                        let lhs = data.string(idx);
                        let (obj_string, _) = self.heap.alloc(
                            ObjString::from(format!("{}{}", lhs, rhs.as_ref())),
                            Objects::String,
                        );

                        self.stack.npop(2);
                        self.stack.push(Value::STRING(obj_string));
                    }
                    (Value::STR(lhs), Value::STR(rhs)) => {
                        let lhs = data.string(lhs);
                        let rhs = data.string(rhs);

                        let (obj_string, _) = self
                            .heap
                            .alloc(ObjString::from(format!("{lhs}{rhs}")), Objects::String);

                        self.stack.npop(2);
                        self.stack.push(Value::STRING(obj_string));
                    }
                    (Value::STR(lhs), rhs) => {
                        let lhs = data.string(lhs);

                        let (obj_string, _) = self
                            .heap
                            .alloc(ObjString::from(format!("{lhs}{rhs}")), Objects::String);

                        self.stack.npop(2);
                        self.stack.push(Value::STRING(obj_string));
                    }
                    _ => binary_handler!(self, +),
                },
                Byte::Sub => binary_handler!(self, -),
                Byte::Mul => binary_handler!(self, *),
                Byte::Div => binary_handler!(self, /),
                Byte::Mod => binary_handler!(self, %),
                Byte::Pow => {
                    let rhs = self.stack.pop();
                    let lhs = self.stack.pop();

                    match (lhs, rhs) {
                        (Value::INTEGER(lhs), Value::INTEGER(rhs)) => {
                            self.stack.push(Value::INTEGER(lhs.pow(rhs as u32)));
                        }
                        (Value::FLOAT(lhs), Value::INTEGER(rhs)) => {
                            self.stack.push(Value::FLOAT(lhs.powf(rhs as f64)));
                        }
                        (Value::INTEGER(lhs), Value::FLOAT(rhs)) => {
                            self.stack.push(Value::INTEGER(lhs.pow(rhs as u32)));
                        }
                        (Value::FLOAT(lhs), Value::FLOAT(rhs)) => {
                            self.stack.push(Value::FLOAT(lhs.powf(rhs)));
                        }
                        _ => (),
                    }
                }
                Byte::LShift => binary_handler!(self, <<),
                Byte::RShift => binary_handler!(self, >>),
                Byte::Xor => binary_handler!(self, ^),
                Byte::And => binary_handler!(self, &),
                Byte::Or => binary_handler!(self, |),
                Byte::Print => {
                    let value = self.stack.pop();
                    // match value {
                    //     Value::REFERENCE(idx) => value = self.stack.peek_at(idx),
                    //     Value::OBJECT(Objects::C) => value = self.stack.peek_at(idx),
                    // }
                    self.stdout.write(&match self.resolve(value) {
                        Value::STR(idx) => data.string(idx).to_string(),
                        Value::STRING(Objects::String(str)) => str.as_ref().to_string(),
                        Value::TYPE(ty) => data.get_type(ty).output(data),
                        Value::OBJECT(Objects::Coroutine(coro)) => {
                            format!("{}", coro.as_ref().get())
                        }
                        Value::OBJECT(Objects::Object(obj)) => {
                            format!("obj({})", data.symbol_name(obj.as_ref().name()).to_owned())
                        }
                        Value::OBJECT(Objects::Array(arr)) => {
                            let length = arr.as_ref().len();

                            let items: Vec<String> = arr
                                .as_ref()
                                .clone()
                                .into_iter()
                                .take(3)
                                .map(|v| v.to_string())
                                .collect();
                            format!(
                                "[{}{}]",
                                items.join(", "),
                                if length > 3 {
                                    format!(", .., {}", arr.as_ref().item(length - 1))
                                } else {
                                    String::new()
                                }
                            )
                        }
                        value => value.to_string(),
                    });

                    if op.operand(0) == 1 {
                        self.stdout.write("\n");
                    }
                }
                Byte::TypeOf => {
                    let result = Value::TYPE(data.find_type(Type::new(self.stack.pop().into())));

                    self.stack.push(result);
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
                        a => {
                            dbg!(a);
                        }
                    }
                }
                Byte::Range => {
                    let [inclusive, ..] = op.operands();

                    let last = self.stack.pop();
                    let first = self.stack.pop();

                    if let (Value::INTEGER(start), Value::INTEGER(end)) = (first, last) {
                        let items: Vec<Value> = if *inclusive == 1 {
                            (start..=end).map(Value::from).collect()
                        } else {
                            (start..end).map(Value::from).collect()
                        };

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
                Byte::Iterator => {
                    let [val, iterator, ..] = op.operands();
                    let frame = self.call_stack[self.fp - 1].1;

                    let iter = self.peek_obj(0);

                    if let Value::OBJECT(obj) = iter {
                        self.gc();
                        self.stack.pop();
                        let (iter, _) = self
                            .heap
                            .alloc(ObjIterator::new(Value::OBJECT(obj)), Objects::Iterator);

                        match obj {
                            Objects::Array(arr) => {
                                self.stack.insert(frame + *iterator, Value::OBJECT(iter));
                                self.stack.insert(frame + *val, arr.as_ref().item(0));
                            }
                            _ => todo!("Handle the rest of the objects as iterables"),
                        }
                        self.gc();
                    } else {
                        dbg!(iter);
                    }
                }
                Byte::Iterate => {
                    let frame = self.call_stack[self.fp - 1].1;

                    let [position, var, iterator] = op.operands();
                    let arr = self.rev_peek_obj(*iterator);
                    if let Value::OBJECT(Objects::Iterator(mut obj)) = arr {
                        // let iter_cursor = obj.as_ref().valid();

                        if !obj.as_ref().valid() {
                            self.ip = *position;
                            continue;
                        }

                        let value = obj.as_ref().get();
                        obj.as_mut().next();

                        self.stack.insert(frame + *var, value);
                    } else {
                        todo!(
                            "Handle cases where value is not an array, but object that actually implements iterator interface or error if it is not a valid iterable"
                        );
                    }
                }
                Byte::Instantiate => {
                    let [owner, arity, _] = op.operands();
                    let object = ObjInstance::new(*owner);
                    self.gc();
                    let (instance, _) = self.heap.alloc(object, Objects::Object);

                    if *arity > 0 {
                        let params = self.stack.npop(*arity).to_vec();

                        let position = self.stack.len();
                        self.stack.push(Value::OBJECT(instance));
                        for param in params.iter().rev() {
                            self.stack.push(*param);
                        }
                        self.stack.push(Value::REFERENCE(position));
                        params.iter().rev().for_each(|v| {
                            self.stack.push(*v);
                        });
                    } else {
                        self.stack.push(Value::OBJECT(instance));
                    }
                }
                Byte::This => {
                    let idx = self.call_stack[self.fp - 1].1 - 1;
                    self.stack.push(self.stack.peek_at(idx));
                }
                Byte::Invoke => {
                    let [ip, arity, _] = op.operands();

                    let instance = self.peek_obj(*arity);
                    match instance {
                        Value::OBJECT(Objects::Object(_)) => {
                            // let args = self.stack.npop(*arity - 1).to_vec();
                            // self.stack.pop();
                            // for arg in args.iter().rev() {
                            //     self.stack.push(*arg);
                            // }
                            // self.stack.push(instance);
                            self.enter(*arity);
                            self.ip = *ip;
                            continue;
                        }
                        Value::OBJECT(Objects::Coroutine(coro)) => {
                            todo!("Handling coroutine method");
                        }
                        _ => unreachable!("This is not callable (yet?)"),
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
                            } else {
                                dbg!("No obj");
                            }
                        }
                        1 => {
                            if let Value::OBJECT(Objects::Object(mut object)) = self.rev_peek_obj(0)
                            {
                                object.as_mut().update(*name, self.stack.pop());
                                self.stack.pop();
                            } else {
                                dbg!("No obj2");
                            }
                        }
                        _ => (),
                    }
                }
                Byte::Upvalue => {
                    let [.., upvalue] = op.operands();
                    self.stack.copy_to_top(*upvalue);
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

    fn resolve(&self, value: Value) -> Value {
        match value {
            Value::REFERENCE(n) => self.stack.peek_at(n),
            val => val,
        }
    }

    // #[inline]
    fn peek_obj(&self, position: usize) -> Value {
        match self.stack.peek(position) {
            Value::REFERENCE(n) => self.stack.peek_at(n),
            v => v,
        }
    }
    fn peek_obj_at(&self, position: usize) -> Value {
        match self.stack.peek_at(position) {
            Value::REFERENCE(n) => self.stack.peek_at(n),
            v => v,
        }
    }
    fn rev_peek_obj(&self, position: usize) -> Value {
        match self
            .stack
            .peek_at(self.call_stack[self.fp - 1].1 + position)
        {
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
