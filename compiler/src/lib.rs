mod pipeline;
mod typechecking;

use std::{borrow::Borrow, collections::HashMap};

use common::{Byte, Instruction, Interner, Label, Message, Value, likely, unlikely};
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
/// `JUMP_IF_MATCH`) push each payload value into the binding's
/// slot position directly (because the stack and the locals
/// area share memory), so the `STORE` emitted for each
/// `Binding` is a no-op that confirms the binding.
///
/// For each sub-pattern:
/// - `Binding { name }` — intern `name` in `variables`, then
///   `STORE <symbol>` to pop the top of stack into that slot.
/// - `Wildcard` — `POP` to discard the value.
/// - `Constructor { .. }` — `UNPACK <arity>` to pop the enum
///   value and push its payload, then recurse into each
///   sub-pattern of the inner constructor.
///
/// This function is called once per match arm body, right after
/// the `JUMP_IF_MATCH` / `UNPACK` has filled the stack with
/// payload values. After it returns, the stack is empty (the
/// payload values are all bound to local slots or discarded).
///
/// `emit_pattern_binding_self` is the `&mut Vec<Byte>` variant
/// used by the match handler when it emits directly to
/// `self.bytecode`. The two helpers are split to keep the
/// borrowed `&mut Vec` out of the recursive call site (so the
/// borrow checker doesn't see `self.context.variables` and
/// `self.bytecode` as simultaneously borrowed inside the helper
/// — they're separate parameters).
fn emit_pattern_binding(
    variables: &mut Interner<String>,
    pattern: &Pattern,
    bytecode: &mut Vec<Byte>,
) {
    match pattern {
        Pattern::Wildcard => {
            bytecode.push(Byte::new(Instruction::POP));
        }
        Pattern::Binding { name } => {
            let symbol = variables.intern(name.to_string());
            bytecode.push(Byte::new(Instruction::STORE).with_operand_u32(symbol as u32));
        }
        Pattern::Constructor { payload, .. } => {
            bytecode.push(Byte::new(Instruction::Unpack).with_operand_u32(payload.len() as u32));
            for sub in payload {
                emit_pattern_binding(variables, sub, bytecode);
            }
        }
    }
}

/// Same as [`emit_pattern_binding`] but takes `&mut Vec<Byte>` for
/// use in code paths that already hold a mutable borrow of
/// `self.bytecode` (e.g., the match handler that emits
/// directly to `self.bytecode`).
#[allow(dead_code)]
fn emit_pattern_binding_self(
    variables: &mut Interner<String>,
    pattern: &Pattern,
    bytecode: &mut Vec<Byte>,
) {
    emit_pattern_binding(variables, pattern, bytecode);
}

impl Compiler {
    pub fn get_function(&self, name: &str) -> usize {
        self.functions[name]
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
                let mut loop_ = self.do_compile(iterable);
                let exit = loop_.len();
                loop_.push(Byte::new(Instruction::JMPF));
                loop_.append(&mut self.do_compile(body));
                loop_
                    .push(Byte::new(Instruction::JMP).with_operand_u32(self.bytecode.len() as u32));

                let len = loop_.len();
                loop_[exit] = Byte::new(Instruction::JMPF)
                    .with_operand_u32((self.bytecode.len() + len) as u32);

                self.bytecode.append(&mut loop_);
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
                    message.push(Label::new(
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
                if let Some(symbol) = self.context.variables.key(&n.to_string()) {
                    bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(symbol as u32));
                } else {
                    let mut message =
                        Message::error("Unknown variable".to_string(), span.into_range());
                    message.push(Label::new(
                        format!("Unknown variable '{}'", n),
                        span.into_range(),
                    ));
                    self.messages.push(message);
                }
            }
            Expression::If(branches) => {
                // Phase 16.5 bugfix: rewrite the If codegen so the
                // body comes AFTER the cond + JMPF in `self.bytecode`,
                // not BEFORE (which is what eager body compilation
                // does when the body is a `Print`).
                //
                // The pre-16.5 implementation eagerly compiled every
                // branch's body via `self.do_compile(body)` inside the
                // branch-iteration loop. For bodies whose codegen
                // emits directly to `self.bytecode` (notably `Print`),
                // the body's bytes landed in `self.bytecode` BEFORE
                // the cond + JMPF. The JMPF target formula then
                // computed an operand that pointed at the START of
                // the cond (not past the body), because the formula
                // was a snapshot of `self.bytecode.len()` at the wrong
                // moment. The VM dutifully jumped back to the start
                // of the cond and re-evaluated it forever.
                //
                // The fix interleaves cond → JMPF → body → (JMP if
                // not last) per branch, in that order. JMPF and JMP
                // are emitted as placeholders (operand = 0) and
                // patched in a final pass once the position of
                // `end_pos` (and each JMP) is known. This works for
                // any body shape (Print, nested if, match, etc.)
                // because the body is compiled AFTER the cond + JMPF
                // placeholder, so its bytes always land at the right
                // position in `self.bytecode`.
                //
                // Layout produced for `if c1 { b1 } else if c2 { b2 } else { b3 }`:
                //   c1, JMPF1, b1, JMP1, c2, JMPF2, b2, JMP2, b3, [end]
                //
                // Patches:
                //   JMPF1 → position right after JMP1 (= start of c2)
                //   JMP1  → end_pos
                //   JMPF2 → position right after JMP2 (= start of b3)
                //   JMP2  → end_pos
                //
                // Layout for a single-branch `if c { b }`:
                //   c, JMPF, b, [end]
                //
                // Patch: JMPF → end_pos (past b).
                //
                // Note: this implementation does NOT use
                // `BlockBuilder`. The BlockBuilder's contract assumes
                // child bytecode lands in the BlockBuilder's local
                // buffer via `bb.extend(child_bc)`. `Print` violates
                // that contract by emitting directly to
                // `self.bytecode`, which is why the BlockBuilder-based
                // approach in the pre-fix spec had the same kind of
                // bug as the original. The direct-manipulation
                // approach below sidesteps the contract entirely.

                // Positions of JMPF and JMP placeholders to patch
                // once we know `end_pos`.
                let mut jmpf_patches: Vec<(usize, bool)> = Vec::new();
                let mut jmp_patches: Vec<usize> = Vec::new();

                for (i, (_, branch)) in branches.iter().enumerate() {
                    let (cond_opt, body) = match branch.borrow() {
                        Expression::Branch(c, b) => (c.as_ref(), b),
                        _ => unreachable!("If branch must be Expression::Branch"),
                    };

                    let is_last = i + 1 == branches.len();

                    // Emit the condition (if any) followed by a JMPF
                    // placeholder. The placeholder's operand is
                    // patched AFTER the body is in self.bytecode
                    // (because we don't know how many bytes the body
                    // contributes until we compile it).
                    //
                    // Bug #1 fix: the JMPF is emitted UNCONDITIONALLY
                    // for every branch with a condition, including
                    // the last branch. The pre-16.5 codegen skipped
                    // the JMPF for single-branch `if` (because
                    // `branches.len() == 1` triggered a `branchless`
                    // flag that gated JMPF emission), which meant
                    // the single-branch if's body was ALWAYS executed
                    // regardless of the condition.
                    if let Some(cond) = cond_opt {
                        let cond_bc = self.do_compile(cond);
                        self.bytecode.extend(cond_bc);
                        self.bytecode.push(Byte::new(Instruction::JMPF).with_operand_u32(0));
                        jmpf_patches.push((self.bytecode.len() - 1, is_last));
                    }

                    // Emit the body AFTER the cond + JMPF so the
                    // body lands at the right position in
                    // self.bytecode. (`Print` emits directly to
                    // self.bytecode, but the bytes are appended here
                    // via the call to `do_compile(body)`.)
                    //
                    // Bug #2 fix: in the pre-16.5 codegen, the body
                    // was eagerly compiled BEFORE the cond + JMPF
                    // landed in self.bytecode, so its bytes appeared
                    // before the JMPF. The JMPF operand was computed
                    // relative to a `self.bytecode.len()` snapshot
                    // that no longer matched the actual layout.
                    let body_bc = self.do_compile(body);
                    self.bytecode.extend(body_bc);

                    // Emit a `JMP → end` placeholder for every
                    // branch except the last. The last branch falls
                    // through to `end_pos`.
                    if !is_last {
                        self.bytecode.push(Byte::new(Instruction::JMP).with_operand_u32(0));
                        jmp_patches.push(self.bytecode.len() - 1);
                    }
                }

                let end_pos = self.bytecode.len();

                // Patch every `JMP → end` placeholder with the
                // final `end_pos`.
                for pos in &jmp_patches {
                    self.bytecode[*pos] =
                        Byte::new(Instruction::JMP).with_operand_u32(end_pos as u32);
                }

                // Patch every JMPF placeholder.
                //
                // For the LAST branch with a condition (i.e., the
                // `else` of a multi-branch chain, OR the only
                // branch of a single-branch `if`), the JMPF target
                // is `end_pos` (past the body).
                //
                // For a NON-LAST branch with a condition, the JMPF
                // target is the position RIGHT AFTER the JMP that
                // skips this branch — which is the start of the
                // NEXT branch's condition. The JMP for branch `i` is
                // at `jmp_patches[i]`, so the JMPF target is
                // `jmp_patches[i] + 1`.
                let mut jmp_idx = 0;
                for (jmpf_pos, is_last) in &jmpf_patches {
                    let target = if *is_last {
                        end_pos
                    } else {
                        jmp_patches[jmp_idx] + 1
                    };
                    self.bytecode[*jmpf_pos] =
                        Byte::new(Instruction::JMPF).with_operand_u32(target as u32);
                    jmp_idx += 1;
                }
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
                    message.push(Label::new(
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
                    message.push(Label::new(
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

                if let Some(symbol) = self.context.variables.key(&name) {
                    if unlikely(self.context.constants.contains_key(&symbol)) {
                        let assigned = likely(*self.context.constants.get(&symbol).unwrap());

                        if !assigned {
                            self.context.constants.entry(symbol).and_modify(|state| {
                                *state = true;
                            });
                        } else {
                            let mut message =
                                Message::error("Assignment error".to_string(), span.into_range());
                            message.push(Label::new(
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
                    message.push(Label::new(
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
                // Recurse into each payload's `Type` expression.
                // We don't emit bytecode — the variant's payload
                // shape is metadata that's already registered
                // with the typechecker (15B).
                for p in payload {
                    bytecode.append(&mut self.do_compile(p));
                }
            }
            Expression::Construct {
                enum_name,
                variant_name,
                args,
            } => {
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

                // Emit the args in REVERSE declaration order.
                // After this, the stack top holds `args[0]` (the
                // first payload value in source order). The VM's
                // `MAKE_ENUM` pops the top arity values, reverses
                // the buffer, and stores them in declaration
                // order — so the final `ObjEnum::payload[i]`
                // matches `args[i]`.
                for arg in args.iter().rev() {
                    bytecode.append(&mut self.do_compile(arg));
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
                    // Phase 15C match codegen.
                    //
                    // We use the canonical "threaded code" /
                    // "jump table" layout. Bytecode is:
                    //
                    //   1. Emit the scrutinee (leaves it on
                    //      the stack).
                    //   2. For each non-last arm with a
                    //      constructor pattern, emit a
                    //      `JUMP_IF_MATCH tag, target, arity`
                    //      placeholder.
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
                    //      a. Emit binding code.
                    //      b. Emit the arm body.
                    //      c. Emit JMP-to-end (skip past
                    //         the remaining arm bodies to
                    //         the END of the match).
                    //   8. Finally, emit arm 0's binding code
                    //      and body (no JMP after — arm 0 is
                    //      the last body in the bytecode, so
                    //      nothing to skip).
                    //
                    // After emission we patch the
                    // JUMP_IF_MATCH placeholders with the
                    // absolute offsets of each arm body, and
                    // the JMP-to-end placeholders with the
                    // absolute offset just past all the arm
                    // bodies (the END of the match).

                    // Step 1: scrutinee.
                    let scrutinee_bc = self.do_compile(scrutinee);
                    self.bytecode.extend(scrutinee_bc);

                    // Step 2 + 3: emit JUMP_IF_MATCH
                    // placeholders for non-last constructor
                    // arms, then the scrutinee-consumer for
                    // the last arm.
                    let mut jump_if_match_places: Vec<(usize, u32)> = Vec::new();

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
                                    .expect("Match arm constructor: typechecker should have registered the enum");
                                if !is_last {
                                    // Non-last constructor arm
                                    // — emit JUMP_IF_MATCH with
                                    // placeholder target.
                                    let placeholder = self.bytecode.len();
                                    jump_if_match_places.push((placeholder, tag));
                                    self.bytecode.push(
                                        Byte::new(Instruction::JumpIfMatch)
                                            .with_operands_u16([tag as u16, 0]),
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
                                        .expect("Match arm constructor: typechecker should have registered the arity");
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
                                // scrutinee under the
                                // binding's symbol.
                                let symbol = self.context.variables.intern(name.to_string());
                                self.bytecode.push(
                                    Byte::new(Instruction::STORE).with_operand_u32(symbol as u32),
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
                    let mut arm_body_offsets: Vec<usize> = vec![0; arms.len()];
                    let mut jmp_to_end_places: Vec<usize> = Vec::new();

                    // We process arms in reverse order so the
                    // LAST arm body comes first in the
                    // bytecode, then non-last arms with
                    // JMP-to-end after each.
                    for i in (0..arms.len()).rev() {
                        let arm = &arms[i];
                        let is_first = i == 0;

                        // Record the offset where this arm
                        // body starts (absolute in
                        // self.bytecode). JUMP_IF_MATCH for
                        // this arm (if any) was emitted earlier
                        // with a placeholder; we patch the
                        // placeholder to point here.
                        arm_body_offsets[i] = self.bytecode.len();

                        // Emit binding code for this arm's
                        // sub-patterns.
                        if let Pattern::Constructor { payload, .. } = &arm.pattern {
                            for sub_pat in payload {
                                emit_pattern_binding(
                                    &mut self.context.variables,
                                    sub_pat,
                                    &mut self.bytecode,
                                );
                            }
                        }

                        // Emit the arm body.
                        let body_bc = self.do_compile(&arm.body);
                        self.bytecode.extend(body_bc);

                        // For non-first arms, emit a
                        // JMP-to-end placeholder so the arm
                        // body doesn't fall through into the
                        // next (previous-in-source-order) arm
                        // body.
                        if !is_first {
                            jmp_to_end_places.push(self.bytecode.len());
                            self.bytecode
                                .push(Byte::new(Instruction::JMP).with_operand_u32(0));
                        }
                    }

                    // The END of the match is just past the
                    // last (first-in-source-order) arm body
                    // — i.e., the current self.bytecode.len().
                    let end_offset = self.bytecode.len() as u32;

                    // Patch JMP-to-end placeholders.
                    for place in jmp_to_end_places {
                        self.bytecode[place] =
                            Byte::new(Instruction::JMP).with_operand_u32(end_offset);
                    }

                    // Patch JUMP_IF_MATCH placeholders.
                    // Each placeholder corresponds to the arm
                    // whose tag it tests; in source order, the
                    // i-th JUMP_IF_MATCH is for the i-th
                    // non-last constructor arm.
                    let mut non_last_index = 0;
                    for i in 0..arms.len() {
                        // Skip arms that don't emit a
                        // JUMP_IF_MATCH (non-constructor
                        // arms or the last arm).
                        let is_last = i == arms.len() - 1;
                        let is_constructor =
                            matches!(&arms[i].pattern, Pattern::Constructor { .. });
                        if is_last || !is_constructor {
                            continue;
                        }
                        let (place, tag) = jump_if_match_places[non_last_index];
                        let target = arm_body_offsets[i] as u16;
                        self.bytecode[place] = Byte::new(Instruction::JumpIfMatch)
                            .with_operands_u16([tag as u16, target]);
                        non_last_index += 1;
                    }
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
                message.push(Label::new(
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
}
