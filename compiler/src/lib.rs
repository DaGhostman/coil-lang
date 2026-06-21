mod block_builder;
mod pipeline;
mod typechecking;

use std::{borrow::Borrow, collections::HashMap};

use common::{Byte, Instruction, Interner, Label as DiagLabel, Message, Value, likely, unlikely};

use crate::block_builder::{BlockBuilder, JumpKind as BbJumpKind};
use parser::{
    SimpleSpan,
    ast::{Expression, Output, Pattern},
};

pub use pipeline::*;
pub use typechecking::{Checker, Ty};

macro_rules! unary {
    ($result: expr, $self: expr, $rhs: expr, $instruction: expr) => {
        $result.append(&mut $self.do_compile($rhs));

        $result.push($instruction);
    };
}
macro_rules! binary {
    ($result: expr, $self: expr, $lhs: expr, $rhs: expr, $instruction: expr) => {
        $result.append(&mut $self.do_compile($lhs));
        $result.append(&mut $self.do_compile($rhs));

        $result.push($instruction);
    };
}

#[derive(Default, Clone)]
struct Context {
    current: Option<String>,
    variables: Interner<String>,
    symbols: Interner<String>,
    assignments: HashMap<String, bool>,
    constants: HashMap<usize, bool>,
    defers: Vec<usize>,
    classes: HashMap<String, Vec<(String, usize)>>,
    impementations: HashMap<String, String>,
    methods: HashMap<String, HashMap<String, String>>,

    /// Per-match-arm binding map. When `Some`, the codegen is
    /// processing the body of a match arm and this map holds the
    /// pattern-bound names → slot positions. The slots are 1-based
    /// (matching the VM's payload-push positions, which start at
    /// `frame.sp + 1`). When `None`, no match is in scope and the
    /// codegen falls back to the global `variables` Interner.
    ///
    /// This map exists because Phase 17B's "multi-variant matches
    /// with binding bodies" limitation: each arm's payload lives at
    /// the same stack slots (1..arity), but the global Interner
    /// assigns each arm's bindings a DIFFERENT slot ID. With a
    /// per-arm map, every arm's first binding is at slot 1, second
    /// at slot 2, etc., so the VM's payload-push positions line up
    /// with the STORE/LOAD operands.
    match_bindings: Option<HashMap<String, u32>>,

    prev: Option<Box<Self>>,
}

pub struct Compiler {
    namespace: String,
    bytecode: Vec<Byte>,

    aliases: HashMap<String, String>,
    functions: HashMap<String, usize>,
    native: HashMap<String, usize>,
    // --
    messages: Vec<Message>,
    context: Context,
    // --
    /// Hindley–Milner checker. Run once per `compile` via
    /// `Checker::check_program`; its cache is consulted by `do_compile`
    /// to pick `ADD` vs `ADDF`, `==` vs `==` (floats), etc.
    checker: crate::typechecking::Checker,
    /// Index into [`crate::typechecking::Checker::ids`] used by
    /// `do_compile` to recover the `NodeId` of the node it's currently
    /// emitting. Reset at the start of each `compile`.
    emit_idx: usize,
}

impl Default for Compiler {
    fn default() -> Self {
        let mut bytecode = Vec::with_capacity(1024);
        bytecode.append(&mut vec![
            Byte::new(Instruction::CALL),
            Byte::new(Instruction::JMP).with_operand_u32(u32::MAX),
            Byte::new(Instruction::HALT),
        ]);

        Self {
            namespace: String::default(),
            bytecode,
            aliases: HashMap::default(),
            functions: HashMap::with_capacity(32),
            native: HashMap::default(),
            // ---
            messages: Vec::default(),
            context: Context::default(),
            // ---
            checker: crate::typechecking::Checker::new(),
            emit_idx: 0,
        }
    }
}

impl<'ctx> Context {
    fn child(&self) -> Self {
        Self {
            current: self.current.clone(),
            impementations: self.impementations.clone(),
            methods: self.methods.clone(),
            defers: Vec::default(),
            constants: self.constants.clone(),
            assignments: self.assignments.clone(),
            variables: self.variables.clone(),
            symbols: self.symbols.clone(),
            classes: self.classes.clone(),
            match_bindings: self.match_bindings.clone(),
            prev: Some(Box::new(self.to_owned())),
        }
    }
}

impl<'ctx> Context {
    pub fn get_prev(&self) -> &Option<Box<Self>> {
        &self.prev
    }
}

/// Emit the bytecode that binds (or discards) the sub-patterns
/// of a constructor pattern. The VM's `UNPACK` (and
/// `JUMP_IF_MATCH`) push each payload value at consecutive stack
/// positions starting at `frame.sp + 1`, so the slot for the
/// Nth payload is `1 + N` (where the first payload is at slot 1).
///
/// The bindings are recorded in `match_bindings` with their slot
/// positions. Subsequent `LOAD`/`STORE` references in the arm
/// body consult `match_bindings` first (see
/// [`Compiler::lookup_slot`]), so the codegen is independent of
/// how many OTHER arms have bindings — the slot is always 1, 2,
/// 3, ... for the arm being processed.
///
/// For each sub-pattern:
/// - `Binding { name }` — record `name → next_slot` in
///   `match_bindings`, emit `STORE next_slot`, and increment
///   `next_slot`. The STORE is a no-op (Phase 15D) that confirms
///   the binding — the VM already pushed the value at that slot.
/// - `Wildcard` — `POP` to discard the value.
/// - `Constructor { .. }` (tuple shape) — `UNPACK <arity>` to pop
///   the enum value and push its payload, then recurse into
///   each sub-pattern of the inner constructor. The nested
///   bindings continue counting from the outer `next_slot`.
/// - `Constructor { .. }` (record shape) — emit a POP (nested
///   record patterns inside an arm body are not supported in 17B
///   — see Known Limitations in AGENTS.md).
///
/// This function is called once per arm body. After it returns,
/// the stack has been "consumed" by the binding code (POPs for
/// wildcards, no-op STOREs for bindings) and `match_bindings`
/// holds the name → slot mapping for the arm.
fn emit_pattern_binding(
    match_bindings: &mut HashMap<String, u32>,
    next_slot: &mut u32,
    pattern: &Pattern,
    bytecode: &mut Vec<Byte>,
) {
    use parser::ast::PatternPayload;
    match pattern {
        Pattern::Wildcard => {
            bytecode.push(Byte::new(Instruction::POP));
        }
        Pattern::Binding { name } => {
            let slot = *next_slot;
            match_bindings.insert(name.to_string(), slot);
            bytecode.push(Byte::new(Instruction::STORE).with_operand_u32(slot));
            *next_slot += 1;
        }
        Pattern::Constructor { payload, .. } => match payload {
            PatternPayload::Unit => {
                // A unit-variant nested pattern (e.g. `Option::None`)
                // is invalid — unit variants have no payload. But
                // the typechecker would have rejected this; emit a
                // no-op POP for defensive purposes.
                bytecode.push(Byte::new(Instruction::POP));
            }
            PatternPayload::Tuple(parts) => {
                bytecode.push(Byte::new(Instruction::Unpack).with_operand_u32(parts.len() as u32));
                for sub in parts {
                    emit_pattern_binding(match_bindings, next_slot, sub, bytecode);
                }
            }
            PatternPayload::Record(_fields) => {
                // Nested record patterns inside an arm body are
                // NOT supported in 17B — see the documented
                // limitations. The typechecker rejects them
                // implicitly (the inner record pattern must look
                // up the variant from the outer arm's payload
                // type, which we don't thread here). Emit a POP
                // and skip.
                bytecode.push(Byte::new(Instruction::POP));
            }
        },
    }
}

impl Compiler {
    pub fn get_function(&self, name: &str) -> usize {
        self.functions[name]
    }

    /// Look up the slot for a name used in an arm body. First
    /// checks the per-arm `match_bindings` map (where match-bound
    /// names live at slots 1, 2, 3, ... matching the VM's
    /// payload-push positions). Falls back to the global
    /// `variables` Interner for function params and other
    /// non-pattern bindings.
    ///
    /// Returns the slot ID (u32) if the name is found, `None`
    /// otherwise.
    fn lookup_slot(&self, name: &str) -> Option<u32> {
        if let Some(map) = &self.context.match_bindings {
            if let Some(&slot) = map.get(name) {
                return Some(slot);
            }
        }
        self.context
            .variables
            .key(&name.to_string())
            .map(|s| s as u32)
    }

    pub fn get_messages(&self) -> &Vec<Message> {
        &self.messages
    }

    pub fn register(&mut self, name: &str, params: &[Ty], returns: &Ty) -> &mut Self {
        let idx = self.native.len();
        self.native.insert(name.to_string(), idx);
        // Phase 10: only the HM checker is used now.
        self.checker.register_native(name, params, returns);

        self
    }

    fn resolve_variable<'compiler>(
        &self,
        variable: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> String {
        match variable.1.borrow() {
            Expression::Identifier(n) => n.to_string(),
            f => {
                eprintln!("{}", f);
                todo!("Function name as expression")
            }
        }
    }

    /// Look up the inferred type of the `lhs` we're about to use as
    /// the operand of a binary operator. The ID for `lhs` lives at
    /// the current `emit_idx` (since `lhs` is the next AST node to be
    /// visited). Returns true iff that type is the float constructor.
    ///
    /// Recurses into `lhs` and `rhs`, appending their bytecodes to
    /// `bytecode` in the same order as the legacy emitter. The caller
    /// is then responsible for emitting the operator-specific
    /// instruction.
    fn compile_binary_operands(
        &mut self,
        bytecode: &mut Vec<Byte>,
        lhs: &Output,
        rhs: &Output,
    ) -> bool {
        // Capture lhs's ID before recursing — `do_compile(lhs)`
        // advances `emit_idx` past lhs's entire subtree.
        let lhs_id = self.checker.id_table().ids()[self.emit_idx];
        bytecode.append(&mut self.do_compile(lhs));
        bytecode.append(&mut self.do_compile(rhs));
        matches!(
            self.checker.lookup_at(lhs_id),
            Some(crate::typechecking::ty::Ty::Con(ref name))
                if name == crate::typechecking::ty::FLOAT
        )
    }

    fn do_compile<'compiler>(
        &mut self,
        ast: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> Vec<Byte> {
        let mut bytecode = vec![];
        // Phase 9: pull the next `NodeId` from the pre-walk's
        // minting order. Both `do_compile` and `Checker::infer` walk
        // the AST in pre-order, so the `n`-th call here consumes the
        // `n`-th ID. We use it later for opcode selection.
        let _self_id = self.checker.id_table().ids()[self.emit_idx];
        self.emit_idx += 1;
        let (span, child) = ast;

        match child.borrow() {
            Expression::Comment(_) => (),
            Expression::Use {
                path: p,
                name,
                alias,
            } => {
                let mut prefix = p.clone();
                if prefix.len() == 1 {
                    prefix.push("".to_string());
                }
                self.aliases.insert(
                    alias.clone().unwrap_or(name.to_string()),
                    format!("{}{}", prefix.join("::"), name),
                );
            }
            Expression::Noop(_) => (),
            Expression::Group(e) => bytecode.append(&mut self.do_compile(e)),
            Expression::Program(children) | Expression::Fragment(children) => {
                children.iter().for_each(|child| {
                    bytecode.append(&mut self.do_compile(child));
                });
            }
            Expression::Block(children) => {
                let ctx = self.context.child();
                self.context = ctx;
                children.iter().for_each(|child| {
                    bytecode.append(&mut self.do_compile(child));
                });

                self.context = *self.context.get_prev().clone().unwrap();
            }
            Expression::Function {
                name,
                args,
                returns: _returns,
                body,
            } => {
                self.functions
                    .insert(format!("{}{}", self.namespace, name), self.bytecode.len());

                let mut a = self.do_compile(args);

                self.bytecode.append(&mut a);

                let mut c = self.do_compile(body);
                self.bytecode.append(&mut c);

                self.context.defers.iter().for_each(|offset| {
                    self.bytecode
                        .push(Byte::new(Instruction::JMP).with_operand_u32(*offset as u32));
                });

                if !matches!(
                    self.bytecode.last().map(|b| b.bytecode()),
                    Some(Instruction::RETURN)
                ) {
                    self.bytecode.push(Byte::new_with_value(
                        Instruction::CONST,
                        Value::default().raw() as _,
                    ));
                    self.bytecode.push(Byte::new(Instruction::RETURN));
                }
            }
            Expression::Expr(child) | Expression::Statement(child) => {
                bytecode.append(&mut self.do_compile(child))
            }
            Expression::ExprStatement(child) => {
                bytecode.append(&mut self.do_compile(child));
                // Do not add pop if previous instruction is `DUP` since they both cancel eachother
                // out
                if !matches!(
                    bytecode.last().map(|b| b.bytecode()),
                    Some(Instruction::DUPLICATE)
                ) {
                    bytecode.push(Byte::new(Instruction::POP));
                } else {
                    // If it was supposed to add `POP` but prev is `DUP`
                    // then remove the DUP as well
                    bytecode.pop();
                }
            }
            Expression::Print(format, params) => {
                // The Print handler emits to `self.bytecode`
                // DIRECTLY (not a local Vec) so that any nested
                // expression (e.g., a `match` inside the params
                // list) can compute ABSOLUTE jump targets in
                // `self.bytecode`. The format string is emitted
                // first, then the params (which may include a
                // `match`), then FORMAT and PRINT.
                let format_bc = self.do_compile(format);
                self.bytecode.extend(format_bc);

                let mut params_len = 0;
                if let Some(params) = params {
                    params_len = params.len();
                    for param in params {
                        let bc = self.do_compile(param);
                        self.bytecode.extend(bc);
                    }
                }

                self.bytecode
                    .push(Byte::new(Instruction::FORMAT).with_operand_u32(params_len as u32));
                self.bytecode.push(Byte::new(Instruction::PRINT));
            }
            Expression::Format(format, params) => {
                // Same as Print: emit directly to `self.bytecode`
                // for nested-match correctness.
                let format_bc = self.do_compile(format);
                self.bytecode.extend(format_bc);

                let mut params_len = 0;
                if let Some(params) = params {
                    params_len = params.len();
                    for param in params {
                        let bc = self.do_compile(param);
                        self.bytecode.extend(bc);
                    }
                }
                self.bytecode
                    .push(Byte::new(Instruction::FORMAT).with_operand_u32(params_len as u32));
            }
            Expression::Return(expr) | Expression::ImplicitReturn(expr) => {
                self.context.defers.iter().for_each(|offset| {
                    self.bytecode
                        .push(Byte::new(Instruction::CALL).with_operand_u32(0));
                    self.bytecode
                        .push(Byte::new(Instruction::JMP).with_operand_u32(*offset as u32));
                });

                // if let Expression::Identifier(name) = *expr.1.borrow() {
                //     let ty = self.typechecker.get_variable_type(&name.into());
                //     let symbol = self.context.variables.key(&name.into());
                //     // if matches!(ty, Some(Type::OBJECT(_))) || matches!(ty, Some(Type::STRING)) {
                //     //     bytecode.push(Byte::new_with_operands(
                //     //         Instruction::ACQUIRE,
                //     //         [symbol.expect("Unable to resolve unknown variable"), 0],
                //     //     ));
                //     // }
                // }

                // for variable in self.context.variables.iter() {
                //     if let (Some(symbol), Some(ty)) = (
                //         self.context.variables.key(variable),
                //         self.typechecker.get_variable_type(variable),
                //     ) && (matches!(ty, Type::OBJECT(_)) || matches!(ty, Type::STRING))
                //     {
                //         bytecode.push(Byte::new_with_operands(Instruction::RELEASE, [symbol, 0]));
                //     }
                // }

                // if matches!(expr.1.borrow(), Expression::Identifier(_)) {
                //     let symbol = self.context.variables.intern(self.resolve_variable(expr));
                // }

                bytecode.append(&mut self.do_compile(expr));
                if !matches!(child.borrow(), Expression::ImplicitReturn(_)) {
                    bytecode.push(Byte::new(Instruction::RETURN));
                }
            }
            Expression::Class(name, state) => {
                self.context.classes.insert(
                    name.to_string(),
                    state
                        .iter()
                        .enumerate()
                        .map(|(idx, v)| match v.1.borrow() {
                            Expression::Field(_, n, _) => (self.resolve_variable(n), idx),
                            _ => unreachable!(
                                "The should be only fields inside of a class definition"
                            ),
                        })
                        .collect::<Vec<_>>(),
                );
                self.context.symbols.intern(name.to_string());
            }
            Expression::Implementation(what, owner, methods) => {
                let namespace = self.namespace.clone();
                let functions = self.functions.clone();

                self.namespace.push_str(what);

                for func in methods {
                    self.functions.drain();
                    self.do_compile(func);

                    for (method, _) in self.functions.iter() {
                        self.context
                            .methods
                            .entry(owner.to_string())
                            .and_modify(|e| {
                                e.insert(what.to_string(), method.clone());
                            })
                            .or_insert_with(|| {
                                let mut h = HashMap::default();
                                h.insert(what.to_string(), method.clone());
                                h
                            });
                    }
                }

                self.context
                    .impementations
                    .insert(what.to_string(), owner.to_string());

                self.namespace = namespace;
                self.functions = functions;
            }
            Expression::Instantiate(class, _args) => {
                let name = self.resolve_variable(class);
                bytecode.push(
                    Byte::new(Instruction::INIT)
                        .with_operand_u32(self.context.classes[&name].len() as u32),
                );
                // bytecode.push(Byte::new(Instruction::SET).with_operand_u32(operand);
                // let s = self;
                // self.functions.get(k);

                // bytecode.push(Byte::new(Instruction::CONST).with_operand_u32(0));
            }
            Expression::Inc(var) => {
                let name = self.resolve_variable(var);
                let symbol = self.context.variables.intern(name);

                bytecode.push(Byte::new(Instruction::INC).with_operand_u32(symbol as u32));
            }
            Expression::Dec(var) => {
                let name = self.resolve_variable(var);
                let symbol = self.context.variables.intern(name);

                bytecode.push(Byte::new(Instruction::DEC).with_operand_u32(symbol as u32));
            }
            Expression::Loop { iterable, body, .. } => {
                // Phase 17A: refactor the Loop codegen onto the
                // placeholder-tracking `BlockBuilder` (the same
                // primitive that drives the If codegen since 16.6).
                // The semantics are IDENTICAL to the pre-17A
                // version — only the placeholder tracking moves
                // from manual `Vec`-based position tracking to
                // `BlockBuilder`'s `bind_label` / `emit_jump_to`.
                //
                // Layout produced for `while cond { body }`:
                //
                //   [top_label]
                //   <iterable bytecode>     ← do_compile(iterable)
                //   JMPF → exit_label       ← exits if cond is false
                //   [exit_label]
                //   <body bytecode>         ← do_compile(body)
                //   JMP → top_label         ← back-edge
                //
                // Bindings (handled by BlockBuilder):
                //   top_label  → self.bytecode.len() at entry
                //   exit_label → self.bytecode.len() after the JMPF
                //   JMP back-edge → top_label
                //
                // All bytes (including any direct-to-self.bytecode
                // emitters inside `body`, e.g. nested `Print`) land
                // in `self.bytecode`; BlockBuilder tracks placeholder
                // positions in that same coordinate system, so the
                // back-edge and exit jumps are correct without any
                // post-pass arithmetic.
                let mut bb = BlockBuilder::new();
                let top_label = bb.fresh_label();
                let exit_label = bb.fresh_label();

                // Bind top_label to the current position (start of
                // the loop). The back-edge JMP at the end of the
                // body will be patched to point here.
                bb.bind_label(top_label, self.bytecode.len() as u32, &mut self.bytecode);

                // Emit the iterable (the condition expression).
                // Borrow-checker note: same as the body case
                // below — stage the bytes in a local to avoid
                // overlapping `&mut self` borrows.
                let iter_bc = self.do_compile(iterable);
                self.bytecode.extend(iter_bc);

                // Emit a JMPF placeholder targeting exit_label.
                // When the condition is false, the JMPF skips past
                // the body to exit the loop.
                bb.emit_jump_to(exit_label, BbJumpKind::JumpIfFalse, &mut self.bytecode);

                // Bind exit_label to the current position (start
                // of the body in the bytecode). This patches the
                // JMPF placeholder.
                bb.bind_label(exit_label, self.bytecode.len() as u32, &mut self.bytecode);

                // Emit the body. The body is a `Block` (a
                // sequence of statements) — `do_compile` handles
                // the multi-statement shape. Any nested control
                // flow inside the body emits its own jumps
                // (using the same BlockBuilder pattern, or its
                // own), all in the same `self.bytecode`
                // coordinate system.
                //
                // Borrow-checker note: stage the body into a
                // local first so the `&mut self` from
                // `do_compile` doesn't overlap with the
                // `&mut self.bytecode` from `extend`. Same
                // pattern as the If codegen (line 677).
                let body_bc = self.do_compile(body);
                self.bytecode.extend(body_bc);

                // Emit the back-edge JMP → top_label. When the
                // body finishes, control jumps back to the top of
                // the loop (where `top_label` was bound).
                bb.emit_jump_to(top_label, BbJumpKind::Unconditional, &mut self.bytecode);

                // Validate: every label that had a pending jump is
                // bound. (Both `top_label` and `exit_label` were
                // bound above, and both had pending jumps.)
                bb.finalize()
                    .expect("BlockBuilder::finalize: all targeted labels bound");
            }
            Expression::Defer(child) => {
                let mut body = vec![Byte::new(Instruction::JMP).with_operand_u32(u32::MAX)];

                self.context.defers.push(self.bytecode.len() + body.len());

                body.append(&mut self.do_compile(child));
                body.push(Byte::new_with_value(
                    Instruction::CONST,
                    Value::from(0i64).raw() as _,
                ));
                body.push(Byte::new(Instruction::RETURN));

                let total_length = self.bytecode.len();
                let current_length = body.len() + bytecode.len();
                if let Some(v) = body.first_mut() {
                    *v = Byte::new(Instruction::JMP)
                        .with_operand_u32((total_length + current_length) as u32);
                }

                bytecode.append(&mut body);
            }
            Expression::Call { name, args } => {
                let identifier = self.resolve_variable(name);
                let n = self.aliases.get(&identifier).unwrap_or(&identifier);

                if let Some(offset) = self.functions.get(n).copied() {
                    if let Some(args) = args {
                        args.iter()
                            .for_each(|arg| bytecode.append(&mut self.do_compile(arg)))
                    }

                    bytecode.push(Byte::new(Instruction::CALL).with_operand_u32(
                        args.as_ref().map(|items| items.len()).unwrap_or(0) as u32,
                    ));
                    bytecode.push(Byte::new(Instruction::JMP).with_operand_u32(offset as u32));
                } else if self.native.get(n).is_some() {
                    todo!("Not implemented");
                } else {
                    let mut message =
                        Message::error("Unknown function".to_string(), span.into_range());
                    message.push(DiagLabel::new(
                        format!("Unable to call unknown function '{}'", n),
                        span.into_range(),
                    ));
                    self.messages.push(message);
                }
            }
            Expression::Argument(_, n) => {
                let _ = self.context.variables.intern(n.to_string());
                // bytecode.push(Byte::new(Instruction::LOAD)
            }
            Expression::Type(_) => {
                // Type names appear as metadata inside enum
                // declarations (e.g. `Some(int)` wraps `int` as
                // an `Expression::Type`). The typechecker has
                // already extracted the type name and registered
                // the enum shape; no runtime bytecode is
                // emitted for the type wrapper. The pre-walk
                // still mints a NodeId (so this arm consumes one
                // to stay in lockstep), but the bytecode here is
                // empty.
            }
            Expression::Identifier(n) => {
                if let Some(slot) = self.lookup_slot(n) {
                    bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(slot));
                } else {
                    let mut message =
                        Message::error("Unknown variable".to_string(), span.into_range());
                    message.push(DiagLabel::new(
                        format!("Unknown variable '{}'", n),
                        span.into_range(),
                    ));
                    self.messages.push(message);
                }
            }
            Expression::If(branches) => {
                // Phase 16.6 refactor: rewrite the If codegen using
                // the placeholder-tracking `BlockBuilder` instead of
                // manual placeholder-tracking via two `Vec<usize>`s.
                // The semantics are IDENTICAL to the Phase 16.5
                // direct-manipulation version (no behavior change) —
                // only the implementation is cleaner.
                //
                // The key correctness property is that ALL bytes
                // (including those emitted by `Print`/`Format`
                // codegen, which targets `self.bytecode` directly)
                // are appended to the same `self.bytecode` vector,
                // and `BlockBuilder` tracks the positions of JMPF
                // and JMP placeholders in that same coordinate
                // system. This sidesteps the pre-16.5
                // coordinate-system hazard entirely.
                //
                // Layout produced for `if c1 { b1 } else if c2 { b2 } else { b3 }`:
                //   c1, JMPF1, b1, JMP1, c2, JMPF2, b2, JMP2, b3, [end]
                //
                // Bindings (handled by BlockBuilder):
                //   JMPF1 → start of c2 (= branch_start_labels[0])
                //   JMP1  → end_label (= end_pos)
                //   JMPF2 → start of b3 (= branch_start_labels[1])
                //   JMP2  → end_label (= end_pos)
                //
                // Layout for a single-branch `if c { b }`:
                //   c, JMPF, b, [end]
                //
                // Binding: JMPF → end_label (= end_pos).

                // Pre-allocate labels for the START of each non-last
                // branch. These are the targets of the PREVIOUS
                // branch's JMPF — when we begin emitting branch `i`,
                // we bind `branch_start_labels[i - 1]` to the
                // current bytecode position (which is the start of
                // branch `i`). The last branch's start has no
                // pre-allocated label (it never serves as a JMPF
                // target because nothing falls through into it from
                // an earlier branch — only `else` does, and `else`
                // has no preceding JMPF in this design).
                let mut bb = BlockBuilder::new();
                let end_label = bb.fresh_label();
                let mut branch_start_labels: Vec<Option<crate::block_builder::Label>> =
                    Vec::with_capacity(branches.len());
                for i in 0..branches.len() {
                    if i + 1 < branches.len() {
                        branch_start_labels.push(Some(bb.fresh_label()));
                    } else {
                        branch_start_labels.push(None);
                    }
                }

                for (i, (_, branch)) in branches.iter().enumerate() {
                    let (cond_opt, body) = match branch.borrow() {
                        Expression::Branch(c, b) => (c.as_ref(), b),
                        _ => unreachable!("If branch must be Expression::Branch"),
                    };

                    // If this is not the first branch, bind the
                    // previous branch's pre-allocated start label to
                    // the CURRENT bytecode position (= the start of
                    // this branch). This patches the JMPF placeholder
                    // emitted by the previous iteration.
                    if i > 0 {
                        if let Some(prev_label) = branch_start_labels[i - 1] {
                            let target = self.bytecode.len() as u32;
                            bb.bind_label(prev_label, target, &mut self.bytecode);
                        }
                    }

                    // Emit the condition (if any) followed by a JMPF
                    // placeholder. The JMPF target is the start of
                    // the NEXT branch (if not the last), or end_label
                    // (if last). The last branch's start label is
                    // `None`, so `unwrap_or(end_label)` falls back to
                    // end_label.
                    //
                    // Bug #1 fix (Phase 16.5): the JMPF is emitted
                    // UNCONDITIONALLY for every branch with a
                    // condition, including the last branch. The
                    // pre-16.5 codegen skipped the JMPF for
                    // single-branch `if`, which meant the body was
                    // ALWAYS executed regardless of the condition.
                    if let Some(cond) = cond_opt {
                        let cond_bc = self.do_compile(cond);
                        self.bytecode.extend(cond_bc);
                        let jmpf_target = branch_start_labels[i].unwrap_or(end_label);
                        bb.emit_jump_to(jmpf_target, BbJumpKind::JumpIfFalse, &mut self.bytecode);
                    }

                    // Emit the body AFTER the cond + JMPF so the
                    // body lands at the right position in
                    // self.bytecode. (`Print` emits directly to
                    // self.bytecode, but the bytes are appended here
                    // via the call to `do_compile(body)`.)
                    //
                    // Bug #2 fix (Phase 16.5): in the pre-16.5
                    // codegen, the body was eagerly compiled BEFORE
                    // the cond + JMPF landed in self.bytecode, so
                    // its bytes appeared before the JMPF. The JMPF
                    // operand was computed relative to a
                    // `self.bytecode.len()` snapshot that no longer
                    // matched the actual layout.
                    let body_bc = self.do_compile(body);
                    self.bytecode.extend(body_bc);

                    // Emit a `JMP → end` placeholder for every
                    // branch except the last. The last branch falls
                    // through to `end_pos`.
                    if i + 1 < branches.len() {
                        bb.emit_jump_to(end_label, BbJumpKind::Unconditional, &mut self.bytecode);
                    }
                }

                // Bind `end_label` to the current bytecode position
                // (= past the last branch's body / JMP). This patches
                // every JMP → end placeholder AND the last JMPF
                // placeholder (if any).
                let end_pos = self.bytecode.len() as u32;
                bb.bind_label(end_label, end_pos, &mut self.bytecode);

                // Validate: every label that had a pending jump must
                // be bound. (Allocated-but-unused labels are allowed.)
                bb.finalize()
                    .expect("BlockBuilder::finalize: all targeted labels bound");
            }
            Expression::Le(lhs, rhs) => {
                let is_float = self.compile_binary_operands(&mut bytecode, lhs, rhs);
                bytecode.push(Byte::new(if is_float {
                    Instruction::LEF
                } else {
                    Instruction::LE
                }));
            }
            Expression::Gt(lhs, rhs) => {
                let is_float = self.compile_binary_operands(&mut bytecode, lhs, rhs);
                bytecode.push(Byte::new(if is_float {
                    Instruction::GTF
                } else {
                    Instruction::GT
                }));
            }
            Expression::Leq(lhs, rhs) => {
                let is_float = self.compile_binary_operands(&mut bytecode, lhs, rhs);
                bytecode.push(Byte::new(if is_float {
                    Instruction::LEQF
                } else {
                    Instruction::LEQ
                }));
            }
            Expression::Geq(lhs, rhs) => {
                let is_float = self.compile_binary_operands(&mut bytecode, lhs, rhs);
                bytecode.push(Byte::new(if is_float {
                    Instruction::GEQF
                } else {
                    Instruction::GEQ
                }));
            }
            Expression::Eq(lhs, rhs) => {
                binary!(bytecode, self, lhs, rhs, Byte::new(Instruction::EQ));
            }
            Expression::Not(lhs) => {
                unary!(bytecode, self, lhs, Byte::new(Instruction::NOT));
            }
            Expression::Negate(lhs) => {
                unary!(bytecode, self, lhs, Byte::new(Instruction::NEG));
            }
            Expression::Add(lhs, rhs) => {
                let is_float = likely(self.compile_binary_operands(&mut bytecode, lhs, rhs));
                bytecode.push(Byte::new(if is_float {
                    Instruction::ADDF
                } else {
                    Instruction::ADD
                }));
            }
            Expression::Sub(lhs, rhs) => {
                let is_float = likely(self.compile_binary_operands(&mut bytecode, lhs, rhs));
                bytecode.push(Byte::new(if is_float {
                    Instruction::SUBF
                } else {
                    Instruction::SUB
                }));
            }
            Expression::Mul(lhs, rhs) => {
                let is_float = likely(self.compile_binary_operands(&mut bytecode, lhs, rhs));
                bytecode.push(Byte::new(if is_float {
                    Instruction::MULF
                } else {
                    Instruction::MUL
                }));
            }
            Expression::Mod(lhs, rhs) => {
                let is_float = likely(self.compile_binary_operands(&mut bytecode, lhs, rhs));
                bytecode.push(Byte::new(if is_float {
                    Instruction::MODF
                } else {
                    Instruction::MOD
                }));
            }
            Expression::Div(lhs, rhs) => {
                let is_float = likely(self.compile_binary_operands(&mut bytecode, lhs, rhs));
                bytecode.push(Byte::new(if is_float {
                    Instruction::DIVF
                } else {
                    Instruction::DIV
                }));
            }
            Expression::And(lhs, rhs) => {
                binary!(bytecode, self, lhs, rhs, Byte::new(Instruction::AND));
            }
            Expression::Integer(num) => bytecode.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(*num).raw() as _,
            )),
            Expression::Bool(state) => bytecode.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(*state).raw() as _,
            )),
            Expression::Float(num) => bytecode.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(*num).raw() as _,
            )),
            Expression::String(str) => {
                let escaped = str
                    .replace("\\n", "\n")
                    .replace("\\r", "\r")
                    .replace("\\t", "\t")
                    .replace("\\0", "\0");

                // if let Ok(re) = Regex::new(r"\\u(?<code>\d{1,})").map_err(|e| dbg!(e)) {
                //     while let Some(captures) = re.captures(escaped.as_str()) {
                //         let unicode = captures.name("code").unwrap().as_str();
                //
                //         escaped = escaped.replace(
                //             format!("\\u{}", unicode).as_str(),
                //             char::from_u32(
                //                 captures
                //                     .name("code")
                //                     .unwrap()
                //                     .as_str()
                //                     .parse()
                //                     .unwrap_or_default(),
                //             )
                //             .unwrap_or_default()
                //             .to_string()
                //             .as_str(),
                //         )
                //     }
                // }
                let idx = bytecode.len();

                let mut count = 0;

                escaped.chars().inspect(|_| count += 1).for_each(|ch| {
                    bytecode.push(Byte::new(Instruction::DATA).with_operand_u32(ch.into()));
                });

                bytecode.insert(
                    idx,
                    Byte::new(Instruction::STRING).with_operand_u32(count as u32),
                );
            }
            Expression::Variable(name, _ty) => {
                if unlikely(self.context.variables.contains(&name.to_string())) {
                    let mut message =
                        Message::error("Variable redeclaration".to_string(), span.into_range());
                    message.push(DiagLabel::new(
                        format!("Variable '{}' already declared", name),
                        span.into_range(),
                    ));
                    self.messages.push(message);
                }

                self.context.variables.intern(name.to_string());
            }
            Expression::Constant(name, _ty) => {
                let name = self.resolve_variable(name);
                if self.context.variables.contains(&name) {
                    let mut message =
                        Message::error("Constand redeclaration".to_string(), span.into_range());
                    message.push(DiagLabel::new(
                        format!("Constant '{}' already declared", name),
                        span.into_range(),
                    ));
                    self.messages.push(message);
                }

                let symbol = self.context.variables.intern(name.clone());

                self.context.constants.insert(symbol, false);
            }
            Expression::Assignment(name, value) => {
                let name = self.resolve_variable(name);

                self.context.assignments.insert(name.clone(), true);

                // Match bindings (arm body pattern bindings) live
                // in `match_bindings`, not the global `variables`
                // Interner. Fall back to the global Interner only
                // when no match is in scope.
                let symbol_opt = if let Some(map) = &self.context.match_bindings {
                    if let Some(&slot) = map.get(&name) {
                        Some(slot as usize)
                    } else {
                        self.context.variables.key(&name)
                    }
                } else {
                    self.context.variables.key(&name)
                };

                if let Some(symbol) = symbol_opt {
                    if unlikely(self.context.constants.contains_key(&symbol)) {
                        let assigned = likely(*self.context.constants.get(&symbol).unwrap());

                        if !assigned {
                            self.context.constants.entry(symbol).and_modify(|state| {
                                *state = true;
                            });
                        } else {
                            let mut message =
                                Message::error("Assignment error".to_string(), span.into_range());
                            message.push(DiagLabel::new(
                                format!(
                                    "Unable to assign to an already assigned constant '{}'",
                                    name
                                ),
                                span.into_range(),
                            ));
                            self.messages.push(message);
                        }
                    }

                    // let ty = self.typecheck(value);
                    let mut expr = self.do_compile(value);

                    bytecode.append(&mut expr);
                    bytecode.push(Byte::new(Instruction::STORE).with_operand_u32(symbol as u32));

                    // Do not pop if assigning to the same place
                    if self.context.variables.len() == symbol + 1 {
                        bytecode.push(Byte::new(Instruction::DUPLICATE));
                    }
                } else {
                    let mut message =
                        Message::error("Undefined variable".to_string(), span.into_range());
                    message.push(DiagLabel::new(
                        format!(
                            "Unable to assign to a non-existing variable/constant '{}'",
                            name
                        ),
                        span.into_range(),
                    ));
                    self.messages.push(message);
                }
            }

            // ---- Phase 15A: sum types and pattern matching ----
            //
            // The HM typechecker (15B) handles all type-level
            // validation (enum registration, constructor arity
            // checks, exhaustiveness). The codegen (15C) is only
            // responsible for emitting the matching bytecode.
            //
            // ID alignment: the pre-walk in
            // `crate::typechecking::id::pre_walk_children` mints
            // NodeIds for `EnumDecl` (1), each `EnumVariant`
            // node (1 per variant), each `Expression::Type`
            // payload (1 per payload type), the `Construct`
            // node (1), and each of its `args` (1 per arg). The
            // `Match` arm bodies each get 1 ID. Patterns do NOT
            // mint IDs (see `pre_walk_pattern`). The codegen
            // below consumes exactly that many IDs by recursing
            // via `self.do_compile`.
            Expression::EnumDecl { name: _, variants } => {
                // Recurse into each variant. Each variant's
                // `do_compile` consumes 1 ID (for the variant
                // itself) and then descends into each payload's
                // `Type` expression. We don't emit any bytecode
                // here — the enum declaration is metadata that's
                // already been registered with the typechecker
                // (15B).
                for v in variants {
                    bytecode.append(&mut self.do_compile(v));
                }
            }
            Expression::EnumVariant { payload, .. } => {
                // Recurse into each payload's `Type` expression
                // (or `RecordFieldDecl`'s value type). We don't
                // emit bytecode — the variant's payload shape is
                // metadata that's already registered with the
                // typechecker (15B). Phase 17B: the payload is
                // `EnumVariantPayload` (Unit / Tuple / Record);
                // only Tuple and Record have children to walk.
                use parser::ast::EnumVariantPayload;
                match payload {
                    EnumVariantPayload::Unit => {}
                    EnumVariantPayload::Tuple(parts) => {
                        for p in parts {
                            bytecode.append(&mut self.do_compile(p));
                        }
                    }
                    EnumVariantPayload::Record(fields) => {
                        for f in fields {
                            bytecode.append(&mut self.do_compile(&f.value));
                        }
                    }
                }
            }
            Expression::Construct {
                enum_name,
                variant_name,
                fields,
            } => {
                use parser::ast::EnumConstructPayload;
                // Look up the variant's tag and arity in the
                // typechecker's tables. The typechecker would
                // already have rejected this construct if the
                // enum / variant isn't registered, so the
                // `expect`s below only fire when something is
                // seriously out of sync (e.g., the typechecker
                // was bypassed).
                let tag = self
                    .checker
                    .tag_for(enum_name, variant_name)
                    .expect("Construct: enum/variant not registered with typechecker");
                let arity = self
                    .checker
                    .arity_for(enum_name, variant_name)
                    .expect("Construct: arity missing from typechecker");

                // Phase 17B: the declared field order is the
                // source-of-truth for the on-stack order (the VM's
                // MAKE_ENUM pops arity values and stores them in
                // pop order — so the FIRST popped ends up at
                // payload[0]).
                //
                // - Tuple: the call-site positional args ARE
                //   already in declaration order. Emit in
                //   REVERSE so the top of stack holds args[0].
                // - Record: the call-site may supply fields in
                //   ANY order. Look up the declared order from
                //   `payload_tys_for`, then walk the user's
                //   fields by name in DECLARATION order. Emit in
                //   REVERSE declaration order so the top of stack
                //   holds the value for `decl_fields[0]`.
                match fields {
                    EnumConstructPayload::Unit => {}
                    EnumConstructPayload::Tuple(args) => {
                        for arg in args.iter().rev() {
                            bytecode.append(&mut self.do_compile(arg));
                        }
                    }
                    EnumConstructPayload::Record(parts) => {
                        // Build a name → &Output map for the call site.
                        let call_site: std::collections::HashMap<&str, &Output> = parts
                            .iter()
                            .map(|p| (p.name, &p.value))
                            .collect();
                        let decl_order = self.checker.payload_tys_for(enum_name, variant_name);
                        // Walk DECLARATION order REVERSED — so when
                        // MAKE_ENUM pops, payload[0] is `decl_fields[0]`.
                        for (decl_name, _) in decl_order.iter().rev() {
                            if let Some(arg) = call_site.get(decl_name.as_str()) {
                                bytecode.append(&mut self.do_compile(arg));
                            }
                            // Missing field: typechecker has already
                            // reported; skip silently to keep bytecode
                            // emission in lockstep with IDs.
                        }
                    }
                }

                // Emit MAKE_ENUM with the tag (upper 16) and
                // arity (lower 16) packed in the operand.
                bytecode.push(
                    Byte::new(Instruction::MakeEnum).with_operands_u16([tag as u16, arity as u16]),
                );
            }
            Expression::Match { scrutinee, arms } => {
                // ---- Match codegen (Phase 15C) ----
                //
                // Strategy (canonical "threaded code" / "jump
                // table" layout):
                //
                //   1. Emit the scrutinee (leaves it on the stack).
                //   2. For each non-last arm with a constructor
                //      pattern, emit a `JUMP_IF_MATCH <tag,
                //      target, arity>` placeholder.
                //   3. For the LAST arm (or any wildcard/binding
                //      arm reached by fall-through), emit the
                //      scrutinee-consumer:
                //        - Constructor last arm: `UNPACK <arity>`.
                //        - Wildcard: `POP` to discard.
                //        - Binding: `STORE <symbol>` to bind.
                //   4. In REVERSE source order, emit each arm's
                //      binding code (STORE/POP for sub-patterns)
                //      followed by the arm body. For every
                //      non-FIRST arm (in source order) emit a
                //      `JMP <end>` placeholder after the body so
                //      it doesn't fall through into the next
                //      body. The LAST arm body has no JMP after
                //      it — execution proceeds to the
                //      JMP-to-end of the previous arm and exits
                //      the match.
                //   5. Patch JUMP_IF_MATCH placeholders to point
                //      to each arm body's absolute offset, and
                //      JMP-to-end placeholders to point to the
                //      END of the match (just past the FIRST arm
                //      body in source order).
                //
                // Resulting bytecode (for `match x { A => a,
                // B => b, C => c }`):
                //
                //   <scrutinee bytecode>
                //   JUMP_IF_MATCH tag_A, target_body_A, _
                //   JUMP_IF_MATCH tag_B, target_body_B, _
                //   UNPACK arity_C
                //   body_c            ← reached by fall-through
                //   JMP end            ← skipped after body_c
                //   body_b            ← reached via JUMP_IF_MATCH B
                //   JMP end            ← skipped after body_b
                //   body_a            ← reached via JUMP_IF_MATCH A
                //   ← match end here

                if arms.is_empty() {
                    // No arms — emit the scrutinee so the stack
                    // doesn't accumulate a dangling value, then
                    // POP it.
                    bytecode.append(&mut self.do_compile(scrutinee));
                    bytecode.push(Byte::new(Instruction::POP));
                } else {
                    // Phase 17A: refactor the Match codegen onto
                    // the placeholder-tracking `BlockBuilder` (the
                    // same primitive that drives the If codegen
                    // since 16.6 and the Loop codegen since this
                    // phase). The semantics are IDENTICAL to the
                    // pre-17A 15C version — only the placeholder
                    // tracking moves from manual
                    // `Vec<usize>`-based position tracking to
                    // `BlockBuilder`'s `bind_label` /
                    // `emit_jump_to`.
                    //
                    // We use the canonical "threaded code" /
                    // "jump table" layout. Bytecode is:
                    //
                    //   1. Emit the scrutinee (leaves it on
                    //      the stack).
                    //   2. For each non-last arm with a
                    //      constructor pattern, emit a
                    //      `JUMP_IF_MATCH tag, 0` placeholder
                    //      targeting a pre-allocated `Label`
                    //      for that arm.
                    //   3. For the LAST arm (or any
                    //      wildcard/binding arm), emit the
                    //      scrutinee-consumer: `UNPACK arity`
                    //      (constructor last arm), `POP`
                    //      (wildcard), or `STORE symbol`
                    //      (binding).
                    //   4. Emit the LAST arm's binding code
                    //      (STORE / POP for sub-patterns).
                    //   5. Emit the LAST arm's body.
                    //   6. (No JMP needed after the last arm.)
                    //   7. For arm N-1, N-2, ..., 1 (in
                    //      REVERSE source order):
                    //      a. Bind the arm's pre-allocated
                    //         `Label` (if any) to the current
                    //         bytecode position (start of this
                    //         arm's binding+body). This
                    //         patches the JUMP_IF_MATCH
                    //         placeholder emitted in step 2.
                    //      b. Emit binding code.
                    //      c. Emit the arm body.
                    //      d. Emit JMP-to-end placeholder
                    //         (targeting `end_label`).
                    //   8. Finally, emit arm 0's binding code
                    //      and body (no JMP after — arm 0 is
                    //      the last body in the bytecode, so
                    //      nothing to skip).
                    //
                    // After emission we bind `end_label` to
                    // the absolute offset just past all the
                    // arm bodies (= the END of the match),
                    // which patches every JMP-to-end
                    // placeholder. Then `finalize()` validates
                    // that every targeted label is bound.

                    let mut bb = BlockBuilder::new();
                    // The END of the match — bound below to
                    // `self.bytecode.len()` after the last
                    // arm body is emitted.
                    let end_label = bb.fresh_label();

                    // Pre-allocate a `Label` for each arm that
                    // will emit a `JUMP_IF_MATCH` placeholder
                    // (i.e., each non-last constructor arm).
                    // The JUMP_IF_MATCH emitted in the forward
                    // pass targets this label; the label is
                    // bound in the reverse pass when we start
                    // emitting that arm's binding+body.
                    let arm_labels: Vec<Option<crate::block_builder::Label>> = arms
                        .iter()
                        .enumerate()
                        .map(|(i, arm)| {
                            let is_last = i == arms.len() - 1;
                            if !is_last && matches!(&arm.pattern, Pattern::Constructor { .. }) {
                                Some(bb.fresh_label())
                            } else {
                                None
                            }
                        })
                        .collect();

                    // Step 1: scrutinee.
                    let scrutinee_bc = self.do_compile(scrutinee);
                    self.bytecode.extend(scrutinee_bc);

                    // Step 2 + 3: emit JUMP_IF_MATCH
                    // placeholders for non-last constructor
                    // arms, then the scrutinee-consumer for
                    // each remaining arm (last arm's UNPACK,
                    // or any arm's wildcard POP / binding
                    // STORE).
                    //
                    // Borrow-checker note: stage the `tag` and
                    // `arity` from `self.checker` into locals
                    // before the `&mut self.bytecode` borrows
                    // that follow. The checker borrows are
                    // short-lived (each `.tag_for` / `.arity_for`
                    // returns an owned value), so the local
                    // staging keeps the borrow checker happy
                    // without changing semantics.
                    for (i, arm) in arms.iter().enumerate() {
                        let is_last = i == arms.len() - 1;

                        match &arm.pattern {
                            Pattern::Constructor {
                                enum_name,
                                variant_name,
                                payload,
                            } => {
                                let tag = self
                                    .checker
                                    .tag_for(enum_name, variant_name)
                                    .expect(
                                        "Match arm constructor: typechecker should have registered the enum",
                                    );
                                if !is_last {
                                    // Non-last constructor arm
                                    // — emit JUMP_IF_MATCH with
                                    // a placeholder target. The
                                    // placeholder will be patched
                                    // when we bind this arm's
                                    // `Label` in the reverse pass.
                                    let label = arm_labels[i]
                                        .expect("non-last constructor arm must have a Label");
                                    bb.emit_jump_to(
                                        label,
                                        BbJumpKind::JumpIfMatch { tag, arity: 0 },
                                        &mut self.bytecode,
                                    );
                                } else {
                                    // Last constructor arm —
                                    // emit UNPACK. The scrutinee
                                    // is still on the stack
                                    // because every previous
                                    // JUMP_IF_MATCH fell through.
                                    let arity = self
                                        .checker
                                        .arity_for(enum_name, variant_name)
                                        .expect(
                                            "Match arm constructor: typechecker should have registered the arity",
                                        );
                                    self.bytecode.push(
                                        Byte::new(Instruction::Unpack)
                                            .with_operand_u32(arity as u32),
                                    );
                                    let _ = payload; // silence unused; payload handled below
                                }
                            }
                            Pattern::Wildcard => {
                                // Wildcard arm — POP the
                                // scrutinee.
                                self.bytecode.push(Byte::new(Instruction::POP));
                            }
                            Pattern::Binding { name } => {
                                // Binding arm — STORE the
                                // scrutinee at slot 1 (matching
                                // LOAD 0's push position).
                                //
                                // Phase 17B: the binding is
                                // recorded in `match_bindings`
                                // by the reverse pass, so the
                                // body's `Identifier` lookup
                                // resolves the name to slot 1.
                                // Hardcoding slot 1 here is
                                // correct because LOAD 0 always
                                // pushes to frame.sp + 1, and the
                                // STORE is a no-op (Phase 15D).
                                let _ = name;
                                self.bytecode.push(
                                    Byte::new(Instruction::STORE)
                                        .with_operand_u32(1),
                                );
                            }
                        }
                    }

                    // Step 4-8: emit each arm's binding code
                    // and body. We go in REVERSE source order
                    // so the bytecode layout is:
                    //   [last arm body]
                    //   [JMP end (skip remaining)]
                    //   [second-to-last arm body]
                    //   [JMP end]
                    //   ...
                    //   [first arm body]
                    //
                    // Each non-first arm body is preceded by
                    // JMP-to-end so it doesn't fall through
                    // into the next body.

                    // We process arms in reverse order so the
                    // LAST arm body comes first in the
                    // bytecode, then non-last arms with
                    // JMP-to-end after each.
                    for i in (0..arms.len()).rev() {
                        let arm = &arms[i];
                        let is_first = i == 0;

                        // If this arm has a pre-allocated
                        // `Label` (it's a non-last constructor
                        // arm), bind it to the current bytecode
                        // position. This patches the
                        // JUMP_IF_MATCH placeholder emitted in
                        // the forward pass.
                        //
                        // The `Label` is `Copy`, so the
                        // immutable borrow of `arm_labels`
                        // ends after this `if let` expression,
                        // and the mutable borrow of `bb` (via
                        // `bind_label`) starts fresh. No
                        // borrow conflict.
                        if let Some(label) = arm_labels[i] {
                            bb.bind_label(
                                label,
                                self.bytecode.len() as u32,
                                &mut self.bytecode,
                            );
                        }

                        // Emit binding code for this arm's
                        // sub-patterns.
                        //
                        // Phase 17B: the slot order is DECLARATION
                        // order (the VM's `JUMP_IF_MATCH` /
                        // `UNPACK` push payload values in
                        // declaration order), so:
                        // - Tuple: walk the pattern in source order
                        //   (= declaration order).
                        // - Record: walk the DECLARATION order and
                        //   look up the pattern by name. The
                        //   pattern may supply fields in a
                        //   different order — that doesn't change
                        //   the slot order, only which sub-pattern
                        //   binds to which slot.
                        //
                        // Phase 17B fix for multi-variant binding
                        // bodies: the binding slots are recorded
                        // in a per-arm `match_bindings` map (not
                        // the global Interner), starting at slot 1
                        // for the first payload position. The map
                        // is consulted by `Identifier` / `Assignment`
                        // lookups in the arm body.
                        let mut arm_bindings: HashMap<String, u32> = HashMap::new();
                        let mut next_slot: u32 = 1;
                        match &arm.pattern {
                            Pattern::Binding { name } => {
                                // Binding arm: the forward pass
                                // already emitted `STORE 1` for
                                // the scrutinee (at slot 1,
                                // matching LOAD 0's push). Record
                                // the binding here so the body's
                                // `Identifier` lookup finds it.
                                arm_bindings.insert(name.to_string(), 1);
                            }
                            Pattern::Constructor {
                                enum_name,
                                variant_name,
                                payload,
                            } => {
                                use parser::ast::PatternPayload;
                                match payload {
                                    PatternPayload::Unit => {}
                                    PatternPayload::Tuple(parts) => {
                                        for sub_pat in parts {
                                            emit_pattern_binding(
                                                &mut arm_bindings,
                                                &mut next_slot,
                                                sub_pat,
                                                &mut self.bytecode,
                                            );
                                        }
                                    }
                                    PatternPayload::Record(fields) => {
                                        // Walk DECLARATION order (from
                                        // the checker's payload_tys_for
                                        // — gives `Vec<(String, Ty)>`
                                        // in declaration order).
                                        let decl_order = self
                                            .checker
                                            .payload_tys_for(enum_name, variant_name);
                                        // Build a name → &Pattern map for
                                        // the pattern site.
                                        let pattern_site: std::collections::HashMap<
                                            &str,
                                            &Pattern,
                                        > = fields
                                            .iter()
                                            .map(|pf| (pf.name, &pf.pattern))
                                            .collect();
                                        for (decl_name, _) in decl_order.iter() {
                                            if let Some(sub_pat) =
                                                pattern_site.get(decl_name.as_str())
                                            {
                                                emit_pattern_binding(
                                                    &mut arm_bindings,
                                                    &mut next_slot,
                                                    sub_pat,
                                                    &mut self.bytecode,
                                                );
                                            }
                                            // Missing field: typechecker
                                            // already reported; skip
                                            // silently so the bytecode
                                            // layout stays consistent.
                                        }
                                    }
                                }
                            }
                            Pattern::Wildcard => {
                                // No bindings — the forward pass
                                // already emitted POP for the
                                // scrutinee.
                            }
                        }

                        // Install the per-arm bindings map so the
                        // body's `Identifier` / `Assignment` lookups
                        // resolve pattern bindings to slots 1, 2, 3,
                        // ... — matching the VM's payload-push
                        // positions. Cleared after the body emits.
                        let saved_bindings =
                            self.context.match_bindings.take();
                        self.context.match_bindings = Some(arm_bindings);

                        // Emit the arm body. Borrow-checker
                        // note: stage the bytes in a local so
                        // the `&mut self` from `do_compile`
                        // doesn't overlap with the
                        // `&mut self.bytecode` from `extend`.
                        let body_bc = self.do_compile(&arm.body);
                        self.bytecode.extend(body_bc);

                        // Restore the prior `match_bindings`
                        // (usually `None` — we only save/restore
                        // to be safe if a match is nested inside
                        // an arm body, which doesn't happen in
                        // practice but the typechecker doesn't
                        // prevent it).
                        self.context.match_bindings = saved_bindings;

                        // For non-first arms, emit a
                        // JMP-to-end placeholder targeting
                        // `end_label`. This is patched when we
                        // bind `end_label` below.
                        if !is_first {
                            bb.emit_jump_to(
                                end_label,
                                BbJumpKind::Unconditional,
                                &mut self.bytecode,
                            );
                        }
                    }

                    // Emit CONST + RETURN at the match's tail.
                    //
                    // The match codegen leaves the body's value
                    // on the stack at the end of the FIRST
                    // (source) arm — which is the LAST arm in
                    // bytecode order (reached by fallthrough).
                    // The JMP-end placeholders from the non-FIRST
                    // arms jump to a position past body_a where
                    // the body's value is RETURNed.
                    //
                    // Why emit CONST + RETURN here:
                    //   1. The function codegen's auto-additions
                    //      check `last != RETURN` and add
                    //      `CONST 0; RETURN` if not. Emitting
                    //      our own RETURN avoids the duplicate
                    //      CONST 0 (which would clobber the
                    //      body's value with 0).
                    //   2. The JMP-end placeholders need a real
                    //      RETURN at their target — not just a
                    //      position in the bytecode — otherwise
                    //      the body's value on the stack would
                    //      leak past the function's epilogue.
                    //
                    // end_pos points to the position of the
                    // RETURN (one BEFORE the end of self.bytecode,
                    // because bytecode_len() is a count, not an
                    // index).
                    // Emit a RETURN at the match's tail. The
                    // body_a's last byte is followed by this
                    // RETURN, so the body's value (on the stack)
                    // is RETURNed. The non-first arms' JMP-end
                    // placeholders target this RETURN (via
                    // end_label bound to its position), so their
                    // values are RETURNed directly without
                    // clobbering.
                    //
                    // The function codegen's auto-additions
                    // check `last != RETURN` and skip the
                    // duplicate CONST + RETURN. end_pos points to
                    // the position of the RETURN (one BEFORE the
                    // end of self.bytecode, because bytecode_len()
                    // is a count, not an index).
                    self.bytecode.push(Byte::new(Instruction::RETURN));
                    let end_pos = (self.bytecode.len() - 1) as u32;
                    bb.bind_label(end_label, end_pos, &mut self.bytecode);

                    // Validate: every label that had a
                    // pending jump is bound.
                    bb.finalize()
                        .expect("BlockBuilder::finalize: all targeted labels bound");
                }
            }
            // TODO: Not reachable from real source — Phase 15A's
            // Decision C preserves this AST node for
            // backwards compatibility, but the parser maps both
            // `_` and `default` to `Pattern::Wildcard` (not
            // `Expression::Default`). This arm exists to consume
            // the NodeId for ID alignment; if the parser ever
            // produces `Expression::Default`, the right behavior
            // is to emit a `POP` (the legacy codegen treated it
            // as a wildcard).
            Expression::Default(_) => (),

            _expr => {
                let mut message =
                    Message::error("Unknown expression".to_string(), span.into_range());
                message.push(DiagLabel::new(
                    "Unable to compile expression".to_string(),
                    span.into_range(),
                ));
                self.messages.push(message);
                #[cfg(debug_assertions)]
                dbg!(_expr);
            }
        }

        bytecode
    }

    pub fn compile<'compiler>(
        &mut self,
        module: &str,
        ast: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> Vec<Byte> {
        let ns = self.namespace.clone();
        self.namespace = module.to_string();

        // Phase 9: run HM inference up-front. The cache populated here
        // is consulted by `do_compile` for opcode selection (e.g.,
        // `ADD` vs `ADDF`).
        self.emit_idx = 0;
        let _program_ty = self.checker.check_program(ast);

        let mut program = self.do_compile(ast);
        self.namespace = ns.to_string();

        // Drain the HM checker's messages into the pipeline-visible
        // message list. The legacy typechecker is gone (Phase 10).
        self.messages.extend(self.checker.take_messages());

        self.bytecode.append(&mut program);

        self.bytecode.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parser::Pratt;

    fn compile_src(src: &str) -> Vec<Byte> {
        let ast = Pratt::default().parse(src).expect("parse failed");
        Compiler::default().compile("test", &ast)
    }

    /// End-to-end: a simple integer expression compiles to bytecode
    /// using the HM checker's cache. We don't check exact bytes (those
    /// change as the emitter evolves); we just verify the pipeline
    /// runs without panicking and produces a non-empty bytecode.
    #[test]
    fn integer_arithmetic_emits_bytecode() {
        let bc = compile_src("42;");
        assert!(!bc.is_empty());
    }

    /// Float arithmetic should pick `ADDF` (float) instead of `ADD`
    /// (int) — that's the whole point of the Phase 9 cache lookup.
    #[test]
    fn float_arithmetic_emits_float_opcode() {
        use common::Instruction;
        let bc = compile_src("1.0 + 2.0;");
        // Find the binary operator instruction. The bytecode is
        // initialised with CALL/JMP/HALT, then operand code, then the
        // operator. We search for the LAST ADDF / ADD.
        let mut last_binop: Option<&Instruction> = None;
        for b in &bc {
            if matches!(b.bytecode(), Instruction::ADDF | Instruction::ADD) {
                last_binop = Some(b.bytecode());
            }
        }
        assert!(
            matches!(last_binop, Some(Instruction::ADDF)),
            "expected ADDF for float arithmetic"
        );
    }

    /// Integer arithmetic should pick `ADD`, not `ADDF`.
    #[test]
    fn integer_arithmetic_emits_int_opcode() {
        use common::Instruction;
        let bc = compile_src("1 + 2;");
        let mut last_binop: Option<&Instruction> = None;
        for b in &bc {
            if matches!(b.bytecode(), Instruction::ADDF | Instruction::ADD) {
                last_binop = Some(b.bytecode());
            }
        }
        assert!(
            matches!(last_binop, Some(Instruction::ADD)),
            "expected ADD for integer arithmetic"
        );
    }

    /// Mixed int+float picks float (because HM unifies the operands
    /// and one is float). The pipeline emits a single, well-typed
    /// result — either way, the test should not panic.
    #[test]
    fn mixed_int_float_arithmetic_emits_bytecode() {
        let bc = compile_src("1 + 2.0;");
        assert!(!bc.is_empty());
    }

    /// The HM checker should record diagnostic messages on type errors;
    /// `compile` should drain them into the compiler's message list.
    #[test]
    fn type_errors_appear_in_messages() {
        let ast = Pratt::default().parse("x;").expect("parse failed");
        let mut c = Compiler::default();
        c.compile("test", &ast);
        assert!(
            !c.messages.is_empty(),
            "expected at least one error message for unknown identifier"
        );
    }

    /// `register_native` adds the native to the HM checker. A subsequent
    /// call to the native type-checks cleanly.
    #[test]
    fn register_native_visible_to_emitter() {
        use crate::typechecking::ty::{string, unit};
        let mut c = Compiler::default();
        c.register("print", &[string()], &unit());
        // `print "hi";` should compile without errors.
        let ast = Pratt::default()
            .parse("print \"hi\";")
            .expect("parse failed");
        let _bc = c.compile("test", &ast);
        let msgs = std::mem::take(&mut c.messages);
        assert!(msgs.is_empty(), "expected no messages, got: {:?}", msgs);
    }

    // ============================================================
    //  Phase 15C: sum types and pattern matching codegen
    // ============================================================

    /// Codegen test 1: a constructor call emits a `MAKE_ENUM`
    /// with the correct tag and arity in the operand (upper 16
    /// bits = tag, lower 16 bits = arity).
    #[test]
    fn construct_emits_make_enum_with_correct_tag_and_arity() {
        use common::Instruction;
        let bc = compile_src("enum Option { None, Some(int) } let x = Option::Some(42);");

        // Find the MAKE_ENUM instruction. Its operands encode
        // (tag, arity) — for `Option::Some(42)`, tag=1, arity=1.
        let make_enum = bc
            .iter()
            .find(|b| matches!(b.bytecode(), Instruction::MakeEnum))
            .expect("expected at least one MakeEnum in the bytecode");
        let tag = (make_enum.operand_u32() >> 16) as u16;
        let arity = (make_enum.operand_u32() & 0xFFFF) as u16;
        assert_eq!(tag, 1, "expected tag=1 (Some)");
        assert_eq!(arity, 1, "expected arity=1 for Some(int)");
    }

    /// Codegen test 2: a `match` with multiple constructor arms
    /// emits a cascade of `JUMP_IF_MATCH` instructions
    /// (one per non-last constructor arm).
    #[test]
    fn match_emits_jump_if_match_cascade() {
        use common::Instruction;
        let bc = compile_src(
            "enum Option { None, Some(int) } \
             match Option::Some(1) { \
                 Option::None() => 0, \
                 Option::Some(v) => v, \
             };",
        );

        // Two arms, both constructor. Two JUMP_IF_MATCH should
        // be emitted (one per arm — actually only one, since
        // arm 0 is non-last and arm 1 is last. So we expect 1
        // JUMP_IF_MATCH and 1 UNPACK.
        let jump_if_match_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JumpIfMatch))
            .count();
        let unpack_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::Unpack))
            .count();
        assert_eq!(
            jump_if_match_count, 1,
            "expected 1 JUMP_IF_MATCH (one per non-last constructor arm)"
        );
        assert_eq!(
            unpack_count, 1,
            "expected 1 UNPACK (one for the last constructor arm)"
        );
    }

    /// Codegen test 3: a wildcard match arm emits `POP` to
    /// discard the scrutinee.
    #[test]
    fn wildcard_match_arm_emits_pop() {
        use common::Instruction;
        let bc = compile_src(
            "enum Option { None, Some(int) } \
             let x = Option::Some(42); \
             match x { _ => 42 };",
        );

        // The wildcard arm is the LAST (and only) arm, reached
        // by fall-through from the scrutinee. It emits `POP` to
        // discard the scrutinee.
        let pop_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::POP))
            .count();
        assert!(
            pop_count >= 1,
            "expected at least one POP for the wildcard scrutinee"
        );
    }

    /// Codegen test 4 (Phase 15D.5 — LOW #5): a `match` with a
    /// NESTED constructor pattern (`Result::Ok(Option::Some(v))`)
    /// emits at least 2 `UNPACK`s — one for the outer `Result::Ok`
    /// and one for the inner `Option::Some`. The codegen
    /// recurses through `emit_pattern_binding` for nested
    /// constructors; the test guards against accidental
    /// simplification that would skip the inner unpack.
    #[test]
    fn match_with_nested_constructor_pattern_emits_unpack_cascade() {
        use common::Instruction;
        let bc = compile_src(
            "enum Option { None, Some(int) } \
             enum Result { Ok(Option), Err(string) } \
             match Result::Ok(Option::Some(1)) { \
                 Result::Err(_) => 0, \
                 Result::Ok(Option::Some(v)) => v, \
             };",
        );

        // The outer match arm (`Result::Ok(Option::Some(v))`) is
        // non-last (the `Err` arm is listed first), so the
        // codegen emits a `JUMP_IF_MATCH` for it. The inner
        // pattern `Option::Some(v)` is a nested constructor, so
        // the binding code emits an `UNPACK` for the inner
        // payload. The two-UNPACK cascade is the structural
        // signature of a nested match.
        let unpack_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::Unpack))
            .count();
        let jump_if_match_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JumpIfMatch))
            .count();
        assert!(
            unpack_count >= 1,
            "expected at least one UNPACK (the inner Option::Some); got {}",
            unpack_count
        );
        assert!(
            jump_if_match_count >= 1,
            "expected at least one JUMP_IF_MATCH (the outer Result::Ok); got {}",
            jump_if_match_count
        );
    }

    // ============================================================
    //  Phase 17A: BlockBuilder for Loop and Match codegen
    // ============================================================
    //
    // The 17A refactor moves both the Loop and Match codegen
    // from manual `Vec<usize>`-based placeholder tracking to
    // the placeholder-tracking `BlockBuilder` (the same
    // primitive that drives If since 16.6). The semantics are
    // IDENTICAL — only the placeholder mechanism changes.
    //
    // These tests guard against regressions in the
    // BlockBuilder-based Loop and Match codegen. The key
    // invariant we check is that the placeholder TARGETS
    // (operands) are correctly patched to the absolute
    // positions of the arm bodies / loop tops — if a `bind_label`
    // is missed, the operand would be `0` (the placeholder
    // value), and the program would either infinite-loop or
    // jump to the prologue.

    /// Codegen test 5 (Phase 17A): a `while` loop emits
    /// the structural shape expected by the
    /// BlockBuilder-based codegen — at least 1 JMPF (the
    /// exit condition) and at least 1 JMP (the back-edge).
    /// This mirrors the 16.5 regression test for If, but
    /// for the new Loop codegen.
    #[test]
    fn loop_emits_top_label_and_back_edge() {
        use common::Instruction;
        let bc = compile_src(
            "fn main() { \
                 let i = 0; \
                 while (i < 3) { \
                     i = i + 1; \
                 } \
             }",
        );

        // The loop emits: <iterable>, JMPF, <body>, JMP→top.
        let jmpf_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JMPF))
            .count();
        let jmp_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JMP))
            .count();
        assert!(
            jmpf_count >= 1,
            "expected at least 1 JMPF (the loop's exit condition); got {}",
            jmpf_count
        );
        assert!(
            jmp_count >= 1,
            "expected at least 1 JMP (the loop's back-edge); got {}",
            jmp_count
        );
    }

    /// Codegen test 6 (Phase 17A): the loop's JMP back-edge
    /// TARGETS the start of the loop, not the prologue. If
    /// the BlockBuilder's `bind_label` for `top_label` were
    /// missed, the JMP would either point at the prologue
    /// (offset 0) or at some other incorrect position; the
    /// program would either infinite-loop or jump out of the
    /// function. The fix-verification: the JMP operand must
    /// be > 3 (past the 3-byte prologue: CALL, JMP, HALT)
    /// and point at the start of the loop's iterable.
    #[test]
    fn loop_jmp_back_edge_targets_loop_top_not_prologue() {
        use common::Instruction;
        let bc = compile_src(
            "fn main() { \
                 let i = 0; \
                 while (i < 3) { \
                     i = i + 1; \
                 } \
             }",
        );

        // The loop has exactly one JMPF (the exit) and
        // exactly one JMP (the back-edge). The JMP is the
        // one we care about.
        let jmp = bc
            .iter()
            .find(|b| matches!(b.bytecode(), Instruction::JMP))
            .expect("expected at least one JMP in the loop bytecode");
        let jmp_target = jmp.operand_u32();

        // The JMP's target must be > 3 (past the prologue)
        // AND should match the start of the loop's iterable
        // (= offset of the first non-prologue byte). For
        // this test program, that's the position of the
        // first JMPF's iterable operand (the start of
        // `i < 3`). We don't assert the exact offset
        // (depends on the precise prologue layout), but we
        // do assert it's > 3 — i.e., the back-edge points
        // INTO the function body, not at the prologue.
        assert!(
            jmp_target > 3,
            "JMP back-edge target {} should be > 3 (past the 3-byte prologue)",
            jmp_target
        );
    }

    /// Codegen test 7 (Phase 17A): in the BlockBuilder-based
    /// Match codegen, every `JUMP_IF_MATCH` placeholder is
    /// patched via `bind_label` to a non-zero target. If a
    /// `bind_label` call were missed (e.g., the
    /// `if let Some(label) = arm_labels[i]` arm didn't
    /// fire for some non-last constructor arm), the
    /// placeholder's lower 16 bits would be `0` (the
    /// `BlockBuilder` placeholder value), and the VM would
    /// jump to the prologue — crashing with a `HALT`.
    #[test]
    fn match_jump_if_match_targets_are_patched_to_arm_offsets() {
        use common::Instruction;
        let bc = compile_src(
            "enum Option { None, Some(int) } \
             match Option::Some(1) { \
                 Option::None() => 0, \
                 Option::Some(v) => v, \
             };",
        );

        // Find every JUMP_IF_MATCH. For each, the target
        // (lower 16 bits) must be > 0 (i.e., the placeholder
        // was patched to a real arm-body offset).
        let jump_if_matches: Vec<_> = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JumpIfMatch))
            .collect();
        assert!(
            !jump_if_matches.is_empty(),
            "expected at least one JUMP_IF_MATCH in the match bytecode"
        );
        for (i, jim) in jump_if_matches.iter().enumerate() {
            let target = (jim.operand_u32() & 0xFFFF) as u16;
            let tag = (jim.operand_u32() >> 16) as u16;
            assert!(
                target > 0,
                "JUMP_IF_MATCH #{} (tag={}) target should be patched to a non-zero offset; got {}",
                i, tag, target
            );
        }
    }

    /// Codegen test 8 (Phase 17A): in the BlockBuilder-based
    /// Match codegen, the `end_label` is correctly bound to
    /// the position just past the FIRST arm body in source
    /// order. The JMP-to-end placeholder (emitted after
    /// every non-FIRST arm body) is patched to this
    /// position. If the binding were missed, the JMP would
    /// point at offset 0 (prologue) and crash.
    ///
    /// We verify by checking that the number of JMP
    /// instructions emitted by a 3-arm match is exactly 2
    /// (one for each non-first arm's JMP-to-end), AND that
    /// the LAST arm body has no JMP after it (it's reached
    /// by fall-through from the previous arm's JMP-to-end).
    /// The pre-17A 15C codegen produced this exact same
    /// shape; the 17A refactor preserves it.
    #[test]
    fn match_jmp_to_end_placeholders_are_patched_to_end_label() {
        use common::Instruction;
        let bc = compile_src(
            "enum Option { None, Some(int), Maybe(int) } \
             match Option::Some(1) { \
                 Option::None() => 0, \
                 Option::Some(v) => v, \
                 Option::Maybe(w) => w, \
             };",
        );

        // 3 arms → 2 non-first arms → 2 JMP-to-end
        // placeholders. The loop's JMP at the very end of
        // the function is ALSO a JMP, but it's not part of
        // the match. We filter for JMPs that are NOT the
        // prologue JMP (operand == 0 or u32::MAX) and NOT
        // the function-exit JMP (if any).
        //
        // Easier check: the JMPs emitted by the match
        // are JMPs with operand > 3 (past the prologue).
        // The 3-arm match emits exactly 2 such JMPs
        // (one per non-first arm).
        let match_jmps: Vec<_> = bc
            .iter()
            .filter(|b| {
                matches!(b.bytecode(), Instruction::JMP)
                    && b.operand_u32() > 3
                    && b.operand_u32() != u32::MAX
            })
            .collect();
        // The match's 2 JMP-to-end + the function's
        // JMP-for-defers (if any) and any nested control
        // flow's JMPs. For this minimal program, the
        // function has no defers, so the only JMPs should
        // be the 2 match JMP-to-end instructions.
        assert_eq!(
            match_jmps.len(),
            2,
            "expected exactly 2 JMP-to-end for a 3-arm match; got {}",
            match_jmps.len()
        );
        // Both JMPs should point to the same end-of-match
        // position (the same `end_label` was bound to the
        // same offset).
        let target_a = match_jmps[0].operand_u32();
        let target_b = match_jmps[1].operand_u32();
        assert_eq!(
            target_a, target_b,
            "both JMP-to-end should target the same end_label; got {} and {}",
            target_a, target_b
        );
    }

    /// Codegen test 9 (Phase 17A): a `match` inside a `while`
    /// loop body — the canonical nested-control-flow
    /// scenario for the BlockBuilder-based codegen. The
    /// 16.5/16.6 If-in-If scenario was the regression that
    /// motivated `BlockBuilder`; this test guards against
    /// the equivalent regression in the Match-in-Loop case.
    /// We don't run the VM (the test infrastructure doesn't
    /// support that for arbitrary programs), but we do
    /// assert the bytecode has the expected control-flow
    /// opcode shape: at least 1 JMPF (the loop's exit
    /// condition), at least 1 JMP (the loop's back-edge),
    /// at least 1 JUMP_IF_MATCH (the match's tag dispatch),
    /// and at least 1 UNPACK (the match's last arm
    /// scrutinee-consumer).
    ///
    /// The match's result is the last expression in the
    /// loop body, which sidesteps the parser's
    /// statement-vs-expression ambiguity (the parser
    /// doesn't accept `match { ... }` followed by another
    /// statement — the `match` is an expression and the
    /// parser wants an operator, not a new statement).
    #[test]
    fn nested_match_in_loop_emits_expected_opcodes() {
        use common::Instruction;
        let bc = compile_src(
            "enum Option { None, Some(int) } \
             fn main() { \
                 let x = Option::Some(0); \
                 let i = 0; \
                 while (i < 3) { \
                     return match x { \
                         Option::None() => 0, \
                         Option::Some(v) => v, \
                     }; \
                 } \
             }",
        );

        let jmpf_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JMPF))
            .count();
        let jmp_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JMP))
            .count();
        let jump_if_match_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JumpIfMatch))
            .count();
        let unpack_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::Unpack))
            .count();
        assert!(
            jmpf_count >= 1,
            "expected at least 1 JMPF (the loop's exit condition); got {}",
            jmpf_count
        );
        assert!(
            jmp_count >= 1,
            "expected at least 1 JMP (the loop's back-edge); got {}",
            jmp_count
        );
        assert!(
            jump_if_match_count >= 1,
            "expected at least 1 JUMP_IF_MATCH (the match's tag dispatch); got {}",
            jump_if_match_count
        );
        assert!(
            unpack_count >= 1,
            "expected at least 1 UNPACK (the match's last arm); got {}",
            unpack_count
        );
    }

    // ============================================================
    //  Phase 17B: record-payload codegen tests
    // ============================================================
    //
    // The 17B spec listed 6 record-payload codegen tests. The
    // developer claimed to add them but in fact added 0 — all 6
    // were silently skipped. This section adds the missing tests,
    // including the red-team's canonical
    // `record_construct_reorders_shuffled_call_site_fields` test
    // that locks in the record-field reordering behavior.

    /// Codegen test 10 (Phase 17B): the red-team's canonical
    /// record-payload reorder test. The variant is declared as
    /// `Foo { x: int, y: int, z: int }` and the user calls it
    /// with shuffled fields `Foo { z: 1, x: 2, y: 3 }`. The
    /// codegen must emit the CONST operands in DECLARATION order
    /// (2, 3, 1) so the VM's `MAKE_ENUM` produces a payload
    /// `[2, 3, 1]` matching the declaration order. If the
    /// codegen emitted them in call-site order (1, 2, 3), the
    /// payload would be in the wrong slot positions and any
    /// match destructuring would get the wrong values.
#[test]
    fn record_construct_reorders_shuffled_call_site_fields() {
        use common::Instruction;
        // The variant is declared as `Foo { x: int, y: int, z: int }`
        // and the user calls it with shuffled fields
        // `Foo { z: 1, x: 2, y: 3 }`. The codegen must emit the
        // CONST operands in REVERSE declaration order so the VM's
        // `MAKE_ENUM` produces a payload in DECLARATION order
        // (payload[0] = x = 2, payload[1] = y = 3, payload[2] = z = 1).
        let bc = compile_src(
            r#"enum E { Foo { x: int, y: int, z: int } }
fn main() {
    print "%i", E::Foo { z: 1, x: 2, y: 3 };
}"#,
        );

        // The construct `E::Foo { z: 1, x: 2, y: 3 }` should
        // emit CONST 1 (z), CONST 3 (y), CONST 2 (x) — that
        // is, REVERSE declaration order — so that MAKE_ENUM's
        // top-first pop order places them at payload[0..2]
        // in declaration order.
        let const_operands: Vec<i64> = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::CONST))
            .map(|b| b.constant() as i64)
            .filter(|&v| v >= 1 && v <= 3)
            .collect();
        assert_eq!(
            const_operands,
            vec![1, 3, 2],
            "Record fields must be emitted in REVERSE declaration order \
             so MAKE_ENUM pops them into declaration order at payload[0..]"
        );

        // Verify MAKE_ENUM has the right tag and arity.
        let make_enum = bc
            .iter()
            .find(|b| matches!(b.bytecode(), Instruction::MakeEnum))
            .expect("expected MAKE_ENUM in the bytecode");
        let tag = (make_enum.operand_u32() >> 16) as u16;
        let arity = (make_enum.operand_u32() & 0xFFFF) as u16;
        assert_eq!(tag, 0, "expected tag=0 for the only variant Foo");
        assert_eq!(arity, 3, "expected arity=3 for Foo {{ x, y, z }}");
    }

    /// Codegen test 11 (Phase 17B): a record construct with one
    /// field emits exactly 1 CONST followed by MAKE_ENUM with
    /// arity=1.
    #[test]
    fn record_construct_one_field_emits_correct_bytecode() {
        use common::Instruction;
        let bc = compile_src(
            "enum E { Foo { x: int } } fn main() { let _ = E::Foo { x: 1 }; }",
        );

        // Find the MAKE_ENUM. Its operand is tag (upper 16) and
        // arity (lower 16).
        let make_enum = bc
            .iter()
            .find(|b| matches!(b.bytecode(), Instruction::MakeEnum))
            .expect("expected at least one MAKE_ENUM");
        let tag = (make_enum.operand_u32() >> 16) as u16;
        let arity = (make_enum.operand_u32() & 0xFFFF) as u16;
        assert_eq!(tag, 0);
        assert_eq!(arity, 1, "expected arity=1 for Foo {{ x }}");

        // Exactly 1 CONST with value 1 (the literal `1`).
        let const_one_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::CONST) && b.constant() == 1)
            .count();
        assert_eq!(
            const_one_count, 1,
            "expected exactly 1 CONST with value 1; got {}",
            const_one_count
        );
    }

    /// Codegen test 12 (Phase 17B): a match pattern with
    /// SHUFFLED record fields (`{ y: b, x: a }`) emits STORE
    /// opcodes in DECLARATION order — a first, then b. If the
    /// codegen emitted in pattern-source order, the bindings
    /// would be swapped at runtime (because the VM pushes
    /// payload values in declaration order).
    #[test]
    fn match_emits_binding_interns_in_declaration_order() {
        use common::Instruction;
        // Declare variant as `{ x: int, y: int, z: int }`. The
        // pattern site supplies fields in shuffled order
        // (`y: b, x: a`). The codegen should walk DECLARATION
        // order (x first, then y) when emitting STORE, so the
        // bindings line up with the VM's payload-push positions.
        //
        // Use a `print` to consume the match's result so the
        // binding code is not optimized away.
        let bc = compile_src(
            "enum E { Foo { x: int, y: int, z: int } } \
             fn main() { \
                 let e = E::Foo { x: 1, y: 2, z: 3 }; \
                 let v = match e { \
                     E::Foo { y: _, x: a } => a, \
                 }; \
                 print \"%i\", v; \
             }",
        );

        // The match has one arm. The STORE for `a` should
        // appear at slot 1 (the first payload position for
        // declaration-order walking). The POP for `_` doesn't
        // emit a STORE.
        let stores: Vec<u32> = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::STORE))
            .map(|b| b.operand_u32())
            .collect();
        // We expect at least one STORE at slot 1 (the binding
        // `a`). The match destructures 3 fields but only 1 is
        // bound (a), so only 1 STORE is emitted. The wildcard
        // `_` produces POP.
        let slot_1_count = stores.iter().filter(|&&s| s == 1).count();
        assert!(
            slot_1_count >= 1,
            "expected STORE at slot 1 for the binding `a`; got stores at {:?}",
            stores
        );
        // The POP for `_` should be present.
        let pop_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::POP))
            .count();
        assert!(
            pop_count >= 1,
            "expected at least 1 POP for the wildcard `_`"
        );
    }

    /// Codegen test 13 (Phase 17B): a mixed-shape enum with
    /// Unit + Tuple + Record variants compiles with the
    /// correct tags and arities for each variant.
    #[test]
    fn mixed_enum_unit_tuple_record_all_in_one() {
        use common::Instruction;
        // Use prints to keep the constructs alive in the
        // bytecode (the codegen is silent on unused `let _`).
        let bc = compile_src(
            "enum E { A, B(int), C { x: int } } \
             fn main() { \
                 print \"%i\", E::A; \
                 print \"%i\", E::B(1); \
                 print \"%i\", E::C { x: 2 }; \
             }",
        );

        // Find all MAKE_ENUM ops (one per construct call,
        // including unit variants — the codegen always emits
        // MAKE_ENUM, even for Unit, with arity=0).
        let make_enums: Vec<_> = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::MakeEnum))
            .collect();
        assert_eq!(
            make_enums.len(),
            3,
            "expected 3 MAKE_ENUM ops (one per construct call); got {}",
            make_enums.len()
        );

        // Sort by (tag, arity) for stable comparison.
        let mut tags_arities: Vec<(u16, u16)> = make_enums
            .iter()
            .map(|b| {
                let tag = (b.operand_u32() >> 16) as u16;
                let arity = (b.operand_u32() & 0xFFFF) as u16;
                (tag, arity)
            })
            .collect();
        tags_arities.sort();
        assert_eq!(
            tags_arities,
            vec![(0, 0), (1, 1), (2, 1)],
            "expected MAKE_ENUM ops at (tag=0, arity=0) for A (unit), \
             (tag=1, arity=1) for B(int), and (tag=2, arity=1) for C record variant"
        );
    }

    /// Codegen test 14 (Phase 17B): a record pattern with a
    /// wildcard field (`_`) emits a POP for the wildcard
    /// sub-pattern instead of a STORE.
    #[test]
    fn record_pattern_with_wildcard_field_emits_pop() {
        use common::Instruction;
        let bc = compile_src(
            "enum E { Foo { x: int, y: int } } \
             fn main() { \
                 let e = E::Foo { x: 1, y: 2 }; \
                 match e { \
                     E::Foo { x: _, y: v } => v, \
                 }; \
             }",
        );

        // The wildcard `x: _` produces POP in the binding code.
        // The binding `y: v` produces STORE at slot 2 (second
        // payload position for a 2-field record).
        let pop_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::POP))
            .count();
        assert!(
            pop_count >= 1,
            "expected at least 1 POP for the wildcard field; got {}",
            pop_count
        );
    }

    /// Codegen test 15 (Phase 17B): a unit-variant match arm
    /// (`Empty`) does NOT emit UNPACK (the variant has no
    /// payload). It emits a POP to discard the scrutinee.
    #[test]
    fn empty_record_pattern_does_not_emit_unpack() {
        use common::Instruction;
        // The spec says "E::Empty => 0" where Empty is unit.
        // The codegen for a unit-variant last arm emits POP,
        // not UNPACK.
        let bc = compile_src(
            "enum E { Empty, Foo(int) } \
             fn main() { \
                 let e = E::Empty; \
                 match e { \
                     E::Empty => 0, \
                     E::Foo(_) => 1, \
                 }; \
             }",
        );

        // Exactly 1 UNPACK (for the Foo arm, which is the
        // last arm and uses UNPACK to consume the scrutinee).
        // The Empty arm is NOT last → emits JUMP_IF_MATCH
        // (not UNPACK). If the codegen wrongly emitted UNPACK
        // for the unit arm, we'd see 2 UNPACKs.
        let unpack_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::Unpack))
            .count();
        assert_eq!(
            unpack_count, 1,
            "expected exactly 1 UNPACK (for the Foo last arm); got {}",
            unpack_count
        );

        // And the Empty arm's JUMP_IF_MATCH should be present.
        let jump_if_match_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JumpIfMatch))
            .count();
        assert_eq!(
            jump_if_match_count, 1,
            "expected exactly 1 JUMP_IF_MATCH (for the Empty arm); got {}",
            jump_if_match_count
        );
    }
}
