use super::*;
use crate::typechecking::{CStructDef, ForInKind};
use reporting::{ErrorCode, Message};

#[cfg(any(test, feature = "dissect"))]
type FinalizeIlOut = Option<crate::dissect::IlSnapshot>;
#[cfg(not(any(test, feature = "dissect")))]
type FinalizeIlOut = ();

impl Compiler {
    /// Expose inferred state to language tooling after a module is checked.
    pub fn checker(&self) -> &crate::typechecking::Checker {
        &self.checker
    }

    pub fn checker_mut(&mut self) -> &mut crate::typechecking::Checker {
        &mut self.checker
    }

    pub fn aliases(&self) -> &HashMap<String, String> {
        &self.aliases
    }

    pub fn module_items(&self) -> &HashMap<String, Vec<String>> {
        &self.module_items
    }

    /// Run HM inference for a module without emitting bytecode.
    pub fn typecheck_module<'compiler>(
        &mut self,
        module: &str,
        ast: &(SimpleSpan, Box<Expression<'compiler>>),
    ) {
        self.checker.set_current_module(module);
        let _ = self.checker.check_program(ast);
        self.messages.extend(self.checker.take_messages());
    }

    pub fn constants(&self) -> &[u64] {
        &self.constants
    }

    pub fn strings(&self) -> &[String] {
        &self.strings
    }

    pub fn set_source_file(&mut self, path: impl Into<std::path::PathBuf>) {
        self.current_source_file = Some(path.into());
    }

    pub fn source_files_list(&self) -> Vec<String> {
        self.source_file_list.clone()
    }

    pub fn debug_locs(&self) -> &[DebugLoc] {
        &self.debug_locs
    }

    fn pad_debug_locs(&mut self) {
        while self.debug_locs.len() < self.bytecode.len() {
            self.debug_locs.push(DebugLoc::unknown());
        }
        if self.debug_locs.len() > self.bytecode.len() {
            self.debug_locs.truncate(self.bytecode.len());
        }
    }

    /// Run registered `defer` thunks in LIFO order.
    ///
    /// For each thunk: LOAD `use (…)` captures from the enclosing frame, then
    /// `CALL` the thunk entry with that arity (push return IP + new frame whose
    /// slots 0..N-1 are the captures). The thunk ends in `RETURN`, which
    /// resumes at the next op. A following `POP` discards the thunk's sentinel
    /// return value so a pending function return value stays on top.
    fn emit_run_defers(&mut self) {
        let defers = self.fn_defers.clone();
        for (label, captures) in defers.iter().rev() {
            for cap in captures {
                if let Some(slot) = self.lookup_slot(cap) {
                    self.bytecode.push_load(slot);
                } else {
                    // Typecheck should have rejected unknown captures; emit a
                    // zero so the CALL arity still matches.
                    debug_assert!(
                        false,
                        "defer capture `{cap}` missing from enclosing frame at codegen"
                    );
                    self.bytecode.push(Byte::new_with_value(
                        Instruction::CONST,
                        Value::default().raw() as _,
                    ));
                }
            }
            self.bytecode
                .emit_entry(EntryKind::Call, captures.len() as u32, *label);
            self.bytecode.push_pop();
        }
    }

    fn loc_for_span(&mut self, span: SimpleSpan) -> DebugLoc {
        let file = self.intern_source_file();
        if file == DEBUG_FILE_UNKNOWN {
            return DebugLoc::unknown();
        }
        let start = span.start as u32;
        let end = span.end.max(span.start + 1) as u32;
        DebugLoc {
            file,
            start_byte: start,
            end_byte: end,
        }
    }

    fn intern_source_file(&mut self) -> u32 {
        let Some(ref path) = self.current_source_file else {
            return DEBUG_FILE_UNKNOWN;
        };
        let key = path.to_string_lossy().into_owned();
        if let Some(&id) = self.source_file_indices.get(&key) {
            return id;
        }
        let id = self.source_file_list.len() as u32;
        self.source_file_list.push(key.clone());
        self.source_file_indices.insert(key, id);
        id
    }

    fn emit_byte(&mut self, span: SimpleSpan, b: Byte) {
        self.pad_debug_locs();
        let loc = self.loc_for_span(span);
        self.bytecode.push(b);
        self.debug_locs.push(loc);
    }

    fn emit_bytes(&mut self, span: SimpleSpan, bytes: &[Byte]) {
        if bytes.is_empty() {
            return;
        }
        self.pad_debug_locs();
        let loc = self.loc_for_span(span);
        for &b in bytes {
            self.bytecode.push(b);
            self.debug_locs.push(loc);
        }
    }

    /// Number of global static slots for the VM table.
    pub fn static_slot_count(&self) -> u32 {
        self.checker.static_slot_count()
    }

    /// Prologue `JMP` target: static initializers and/or `extern` setup
    /// run at `program_start_offset`; otherwise jump straight to `main`.
    pub fn prologue_jmp_target(&self) -> u32 {
        if self.static_slot_count() > 0 {
            self.setup_entry_offset
        } else if self.has_extern_block() {
            self.program_start_offset
        } else {
            self.functions
                .get("main")
                .copied()
                .unwrap_or(self.program_start_offset as usize) as u32
        }
    }

    /// Harness test cases emitted this compile: `(description, fn offset)`.
    pub fn test_cases(&self) -> &[(String, u32)] {
        &self.test_cases
    }

    /// Include harness `test("…")` / `#[test]` declarations in the compile unit.
    pub fn set_include_tests(&mut self, include: bool) {
        self.include_tests = include;
    }

    pub fn include_tests(&self) -> bool {
        self.include_tests
    }

    pub fn intern_constant(&mut self, value: u64) -> u32 {
        let idx = self.constants.len() as u32;
        self.constants.push(value);
        idx
    }

    pub fn intern_string(&mut self, value: impl AsRef<str>) -> u32 {
        let value = value.as_ref();
        if let Some(&idx) = self.string_indices.get(value) {
            return idx;
        }
        let idx = self.strings.len() as u32;
        self.strings.push(value.to_string());
        self.string_indices.insert(value.to_string(), idx);
        idx
    }

    fn push_string_literal(&mut self, bytecode: &mut impl EmitBuf, value: impl AsRef<str>) {
        let idx = self.intern_string(value);
        bytecode.push_string(idx);
    }

    fn const_env(&self) -> &HashMap<String, ConstValue> {
        self.const_env_stack
            .last()
            .expect("const_env_stack initialized in compile_unfused")
    }

    fn const_env_mut(&mut self) -> &mut HashMap<String, ConstValue> {
        self.const_env_stack
            .last_mut()
            .expect("const_env_stack must be non-empty during codegen")
    }

    fn push_const_env(&mut self) {
        let parent = self.const_env().clone();
        self.const_env_stack.push(parent);
    }

    fn pop_const_env(&mut self) {
        self.const_env_stack.pop();
    }

    fn emit_const_value(&mut self, v: &ConstValue, bytecode: &mut Vec<Byte>) {
        match v {
            ConstValue::Int(n) => {
                if (0..=i32::MAX as i64).contains(n) {
                    bytecode.push_const(*n as i32);
                } else {
                    let bits = Value::from(*n).raw() as u64;
                    let idx = self.intern_constant(bits);
                    bytecode.push_const_pool(idx);
                }
            }
            ConstValue::Float(n) => {
                let bits = Value::from(*n).raw() as u64;
                let idx = self.intern_constant(bits);
                bytecode.push_const_pool(idx);
            }
            ConstValue::Bool(b) => {
                bytecode.push(Byte::new_with_value(
                    Instruction::CONST,
                    Value::from(*b).raw() as _,
                ));
            }
            ConstValue::Str(s) => {
                self.push_string_literal(bytecode, s);
            }
        }
    }

    /// If `ast` folds to a scalar, emit it and return true.
    ///
    /// When `allow_mul_shl` is false, skip `x * 2^n` → `SHL` so trait/`Mul`
    /// dictionary calls (`bound_operator_call`) still dispatch through
    /// `emit_bound_operator_call` for non-primitive `T * 2^n`.
    fn try_emit_folded_expr(
        &mut self,
        ast: &(SimpleSpan, Box<Expression<'_>>),
        bytecode: &mut Vec<Byte>,
        allow_mul_shl: bool,
    ) -> bool {
        if let Some(v) = crate::const_fold::eval_expr(ast, self.const_env()) {
            self.emit_const_value(&v, bytecode);
            true
        } else if let Some(inner) = crate::const_fold::strength_reduced_inner(ast) {
            let mut inner_bc = self.do_compile(inner);
            bytecode.append(&mut inner_bc);
            true
        } else if allow_mul_shl
            && let Some((inner, shift)) = crate::const_fold::strength_mul_to_shl(ast, self.const_env())
        {
            // Defense-in-depth: only emit int SHL when the non-const operand
            // is a known integer-like immediate (`int` or `byte` — extend
            // this match if more int-like primitives are added). VM `SHL`
            // uses `as_int`; `float * k` is rejected at typecheck. Unknown
            // types fall through to MUL / dictionary dispatch.
            use crate::typechecking::subst::apply_ty_prune;
            use crate::typechecking::ty::{BYTE, INT};
            let inner_is_int_like = self.codegen_expr_ty(inner).is_some_and(|ty| {
                matches!(
                    apply_ty_prune(self.checker.subst(), &ty),
                    Ty::Con(ref n) if n == INT || n == BYTE
                )
            });
            if !inner_is_int_like {
                return false;
            }
            let mut inner_bc = self.do_compile(inner);
            bytecode.append(&mut inner_bc);
            bytecode.push_const(shift as i32);
            bytecode.push(Byte::new(Instruction::SHL));
            true
        } else if let Some((base, kind)) = crate::const_fold::strength_pow_int(ast, self.const_env()) {
            use crate::typechecking::subst::apply_ty_prune;
            use crate::typechecking::ty::{BYTE, INT};
            let base_is_int_like = self.codegen_expr_ty(base).is_some_and(|ty| {
                matches!(
                    apply_ty_prune(self.checker.subst(), &ty),
                    Ty::Con(ref n) if n == INT || n == BYTE
                )
            });
            if !base_is_int_like {
                return false;
            }
            match kind {
                crate::const_fold::StrengthPow::ConstOne => {
                    // Still walk `base` for NodeId alignment, then push 1.
                    self.discard_compile(base);
                    bytecode.push_const(1);
                    true
                }
                crate::const_fold::StrengthPow::Square => {
                    // Dup-safe bases only: Identifier or pure (no Call / IO).
                    if !(matches!(
                        unwrap_expr_output(base).1.as_ref(),
                        Expression::Identifier(_)
                    ) || Self::call_arg_is_pure(base))
                    {
                        return false;
                    }
                    let mut base_bc = self.do_compile(base);
                    bytecode.append(&mut base_bc);
                    bytecode.push(Byte::new(Instruction::DUPLICATE));
                    bytecode.push(Byte::new(Instruction::MUL));
                    true
                }
            }
        } else {
            false
        }
    }

    fn discard_compile(&mut self, ast: &(SimpleSpan, Box<Expression<'_>>)) {
        // Walk for NodeId alignment / side tables, but drop any bytes that
        // direct-to-`self.bytecode` emitters (Print/Format/control flow) wrote.
        let bc_len = self.bytecode.len();
        let dbg_len = self.debug_locs.len();
        let _ = self.do_compile(ast);
        self.bytecode.truncate(bc_len);
        self.debug_locs.truncate(dbg_len);
    }

    fn discard_if_branch(&mut self, branch: &Output<'_>) {
        if let Expression::Branch(cond, body) = branch.1.as_ref() {
            if let Some(c) = cond {
                self.discard_compile(c);
            }
            self.discard_compile(body);
        }
    }

    /// Rewrite `if (!c) { A } else { B }` as `if (c) { B } else { A }` so the
    /// condition can fuse into `BinSlot*Jmpf` / `CmpJmpf` without `LogNotJmpf`.
    fn try_invert_not_if_else<'a>(branches: &'a [Output<'a>]) -> Option<Vec<Output<'a>>> {
        if branches.len() != 2 {
            return None;
        }
        let Expression::Branch(Some(cond), then_body) = branches[0].1.as_ref() else {
            return None;
        };
        let Expression::Branch(else_cond, else_body) = branches[1].1.as_ref() else {
            return None;
        };
        if else_cond.is_some() {
            return None;
        }
        let cond = unwrap_expr_output(cond);
        let Expression::LogicalNot(inner) = cond.1.as_ref() else {
            return None;
        };
        let inner = unwrap_expr_output(inner).clone();
        Some(vec![
            (
                branches[0].0,
                Box::new(Expression::Branch(Some(inner), else_body.clone())),
            ),
            (
                branches[1].0,
                Box::new(Expression::Branch(None, then_body.clone())),
            ),
        ])
    }

    /// Constant-folded `if` / `else if` / `else`. Returns true when handled.
    fn try_compile_const_if(&mut self, branches: &[Output<'_>]) -> bool {
        let mut i = 0usize;
        while i < branches.len() {
            let Expression::Branch(cond, body) = branches[i].1.as_ref() else {
                return false;
            };
            match cond {
                Some(c) => match crate::const_fold::eval_expr(c, self.const_env()) {
                    Some(ConstValue::Bool(true)) => {
                        for j in 0..i {
                            self.discard_if_branch(&branches[j]);
                        }
                        self.discard_compile(c);
                        let mut body_bc = self.do_compile(body);
                        self.bytecode.append(&mut body_bc);
                        for j in (i + 1)..branches.len() {
                            self.discard_if_branch(&branches[j]);
                        }
                        return true;
                    }
                    Some(ConstValue::Bool(false)) => {
                        self.discard_compile(c);
                        self.discard_compile(body);
                        i += 1;
                    }
                    _ => return false,
                },
                None => {
                    for j in 0..i {
                        self.discard_if_branch(&branches[j]);
                    }
                    let mut body_bc = self.do_compile(body);
                    self.bytecode.append(&mut body_bc);
                    for j in (i + 1)..branches.len() {
                        self.discard_if_branch(&branches[j]);
                    }
                    return true;
                }
            }
        }
        for b in branches {
            self.discard_if_branch(b);
        }
        true
    }

    /// `return self(...)` tail-call when eligible.
    fn try_emit_tail_call_expr(&mut self, expr: &Output<'_>, bytecode: &mut Vec<Byte>) -> bool {
        if !self.fn_defers.is_empty() {
            return false;
        }
        let Some(cur) = self.current_function_table_key.clone() else {
            return false;
        };
        let call_expr = match expr.1.as_ref() {
            Expression::Call { .. } => expr,
            Expression::Expr(inner) | Expression::Group(inner) => {
                if matches!(inner.1.as_ref(), Expression::Call { .. }) {
                    inner
                } else {
                    return false;
                }
            }
            _ => return false,
        };
        let Expression::Call { name, args } = call_expr.1.as_ref() else {
            return false;
        };
        let Expression::Identifier(fname) = name.1.as_ref() else {
            return false;
        };
        let mut call_key = self
            .aliases
            .get(*fname)
            .cloned()
            .unwrap_or_else(|| fname.to_string());
        if let Some((fa, is_rest)) = self
            .checker
            .selected_overload_at(call_expr.0.start, call_expr.0.end)
        {
            let keyed = overload_fn_key(&call_key, fa, is_rest);
            if self.functions.contains_key(&keyed) {
                call_key = keyed;
            } else {
                let simple = call_key
                    .rsplit("::")
                    .next()
                    .unwrap_or(&call_key)
                    .to_string();
                let keyed_simple = overload_fn_key(&simple, fa, is_rest);
                if self.functions.contains_key(&keyed_simple) {
                    call_key = keyed_simple;
                }
            }
        } else if !self.functions.contains_key(&call_key) {
            if let Some(q) = self.current_function_qualified.as_ref() {
                if call_key == *q || call_key == strip_overload_key(&cur) {
                    call_key = cur.clone();
                }
            }
        }
        if call_key != cur {
            return false;
        }
        let qualified = self.current_function_qualified.as_deref().unwrap_or("");
        if self.coroutine_fns.contains(qualified) || self.coroutine_fns.contains(&call_key) {
            return false;
        }
        let arg_slice = args.as_deref().unwrap_or(&[]);
        let lookup = strip_overload_key(&cur).to_string();
        let arity = self.emit_call_args_with_rest(&lookup, arg_slice, bytecode, false);
        let Some(&target) = self.functions.get(&cur) else {
            return false;
        };
        // Packed abs PC; CodeBuf::push rewrites to IlOp::Entry via entry_at_offset.
        bytecode
            .push(Byte::new(Instruction::TailCall).with_call_packed(arity as u32, target as u32));
        true
    }

    /// Max emitting ops for a compare+branch tiny-inline diamond.
    const TINY_INLINE_DIAMOND_MAX_OPS: usize = 24;
    /// Max emitting ops for a one-level self-unroll peel.
    const SELF_UNROLL_MAX_OPS: usize = 48;

    fn is_tiny_inline_il(ops: &[IlOp]) -> bool {
        if ops.is_empty() || ops.len() > 64 {
            return false;
        }
        if Self::is_tiny_inline_diamond_il(ops) {
            return true;
        }
        if ops.iter().any(|op| op.is_control()) {
            return false;
        }
        // Sole fused return: expand to producer at the call site (no RETURN).
        if ops.len() == 1 {
            match &ops[0] {
                IlOp::LoadReturnSlot { .. }
                | IlOp::ConstReturnImm { .. }
                | IlOp::BinReturn { .. } => return true,
                _ => {
                    if let Some(b) = ops[0].as_plain_byte()
                        && matches!(
                            *b.bytecode(),
                            Instruction::LoadReturnSlot
                                | Instruction::ConstReturnImm
                                | Instruction::BinReturn
                        )
                    {
                        return true;
                    }
                }
            }
        }
        // Pure micro-body: ≤3 compute ops + terminal Return / fused *Return.
        if Self::is_pure_micro_inline_il(ops) {
            return true;
        }
        // Inliner copies opcodes until the first `RETURN` and leaves that
        // value on the stack. Early-return / branched bodies therefore
        // truncate (else-arm dropped). Only allow a single terminal RETURN
        // and no control-flow jumps.
        let return_idxs: Vec<usize> = ops
            .iter()
            .enumerate()
            .filter(|(_, op)| op.is_plain_return())
            .map(|(i, _)| i)
            .collect();
        if return_idxs.len() != 1 || return_idxs[0] != ops.len() - 1 {
            return false;
        }
        !ops.iter().any(|op| Self::inline_forbidden_op(op))
    }

    /// One compare+branch diamond: `if cond { return A; } return B;` (no calls).
    ///
    /// Emitting shape (labels omitted by [`CodeBuf::code_slice_ops`]):
    /// `cond…; JumpIfFalse; then…; Return; else…; Return`.
    fn is_tiny_inline_diamond_il(ops: &[IlOp]) -> bool {
        if ops.is_empty() || ops.len() > Self::TINY_INLINE_DIAMOND_MAX_OPS {
            return false;
        }
        if ops.iter().any(|op| matches!(op, IlOp::Entry { .. })) {
            return false;
        }
        let jump_idxs: Vec<usize> = ops
            .iter()
            .enumerate()
            .filter(|(_, op)| matches!(op, IlOp::Jump { .. }))
            .map(|(i, _)| i)
            .collect();
        if jump_idxs.len() != 1 {
            return false;
        }
        let j = jump_idxs[0];
        let IlOp::Jump {
            kind: IlJumpKind::JumpIfFalse,
            ..
        } = &ops[j]
        else {
            return false;
        };
        if j == 0 || j + 1 >= ops.len() {
            return false;
        }
        // Cond / arms must not contain nested control or forbidden ops.
        if ops[..j].iter().any(|op| {
            op.is_control() || Self::inline_forbidden_op(op) || Self::inline_is_return(op)
        }) {
            return false;
        }
        let Some(then_end) = Self::diamond_arm_end(ops, j + 1) else {
            return false;
        };
        if then_end + 1 >= ops.len() {
            return false;
        }
        let else_start = then_end + 1;
        let Some(else_end) = Self::diamond_arm_end(ops, else_start) else {
            return false;
        };
        if else_end != ops.len() - 1 {
            return false;
        }
        let then_arm = &ops[j + 1..=then_end];
        let else_arm = &ops[else_start..=else_end];
        Self::diamond_arm_ok(then_arm) && Self::diamond_arm_ok(else_arm)
    }

    fn inline_is_return(op: &IlOp) -> bool {
        op.is_plain_return()
            || matches!(
                op,
                IlOp::LoadReturnSlot { .. } | IlOp::ConstReturnImm { .. } | IlOp::BinReturn { .. }
            )
            || matches!(
                op.as_plain_byte(),
                Some(b) if matches!(
                    *b.bytecode(),
                    Instruction::RETURN
                        | Instruction::LoadReturnSlot
                        | Instruction::ConstReturnImm
                        | Instruction::BinReturn
                )
            )
    }

    /// Index of the last op of an arm starting at `start` (inclusive).
    fn diamond_arm_end(ops: &[IlOp], start: usize) -> Option<usize> {
        if start >= ops.len() {
            return None;
        }
        // Sole fused *Return arm.
        if Self::inline_is_fused_return(&ops[start]) {
            return Some(start);
        }
        for i in start..ops.len() {
            if ops[i].is_control() {
                return None;
            }
            if ops[i].is_plain_return() {
                return Some(i);
            }
        }
        None
    }

    fn inline_is_fused_return(op: &IlOp) -> bool {
        matches!(
            op,
            IlOp::LoadReturnSlot { .. } | IlOp::ConstReturnImm { .. } | IlOp::BinReturn { .. }
        ) || matches!(
            op.as_plain_byte(),
            Some(b) if matches!(
                *b.bytecode(),
                Instruction::LoadReturnSlot
                    | Instruction::ConstReturnImm
                    | Instruction::BinReturn
            )
        )
    }

    fn diamond_arm_ok(arm: &[IlOp]) -> bool {
        if arm.is_empty() {
            return false;
        }
        if Self::inline_is_fused_return(&arm[0]) {
            return arm.len() == 1;
        }
        if arm
            .iter()
            .any(|op| op.is_control() || Self::inline_forbidden_op(op))
        {
            return false;
        }
        arm.last().is_some_and(|op| op.is_plain_return())
            && arm[..arm.len() - 1]
                .iter()
                .all(|op| !Self::inline_is_return(op))
    }

    /// Body eligible for one-level self-unroll at a call site to `self_entry`.
    fn is_self_unroll_il(ops: &[IlOp], self_entry: Option<IlLabel>) -> bool {
        if ops.is_empty() || ops.len() > Self::SELF_UNROLL_MAX_OPS {
            return false;
        }
        let Some(self_entry) = self_entry else {
            return false;
        };
        let mut saw_self_call = false;
        for op in ops {
            match op {
                IlOp::Entry {
                    kind: EntryKind::TailCall,
                    ..
                } => {
                    // Tail-call bodies leave dead fallthrough and rely on
                    // post-emit opts for arg order — unsafe to peel pre-opt.
                    return false;
                }
                IlOp::Entry {
                    kind: EntryKind::Call,
                    target,
                    ..
                } => {
                    if *target == self_entry {
                        saw_self_call = true;
                    }
                }
                IlOp::Entry { .. } | IlOp::PrologueJmp { .. } => return false,
                IlOp::HostInvoke { .. } => return false,
                IlOp::Jump {
                    kind: IlJumpKind::JumpIfMatch { .. },
                    ..
                } => return false,
                _ => {
                    if let Some(b) = op.as_plain_byte() {
                        match *b.bytecode() {
                            Instruction::TailCall => return false,
                            Instruction::CALL => {}
                            Instruction::MakeCoro
                            | Instruction::YieldCoro
                            | Instruction::YieldFromCoro
                            | Instruction::HostInvoke
                            | Instruction::FfiInvoke
                            | Instruction::JumpIfMatch => return false,
                            _ => {}
                        }
                    }
                }
            }
        }
        saw_self_call
            && !ops.iter().any(|op| {
                matches!(
                    op,
                    IlOp::Print { .. }
                        | IlOp::GetField { .. }
                        | IlOp::SetField { .. }
                        | IlOp::MakeTuple { .. }
                        | IlOp::MakeArray { .. }
                        | IlOp::MakeEnum { .. }
                )
            })
    }

    /// ≤3 pure producers + terminal Return / fused *Return (Load/Const/Bin/…).
    fn is_pure_micro_inline_il(ops: &[IlOp]) -> bool {
        if ops.is_empty() || ops.len() > 4 {
            return false;
        }
        let last = ops.last().unwrap();
        let terminal_ok = last.is_plain_return()
            || matches!(
                last,
                IlOp::LoadReturnSlot { .. } | IlOp::ConstReturnImm { .. } | IlOp::BinReturn { .. }
            )
            || matches!(
                last.as_plain_byte(),
                Some(b) if matches!(
                    *b.bytecode(),
                    Instruction::LoadReturnSlot
                        | Instruction::ConstReturnImm
                        | Instruction::BinReturn
                        | Instruction::RETURN
                )
            );
        if !terminal_ok {
            return false;
        }
        let compute = &ops[..ops.len() - 1];
        if compute.len() > 3 {
            return false;
        }
        compute.iter().all(|op| {
            matches!(
                op,
                IlOp::Load { .. }
                    | IlOp::Const { .. }
                    | IlOp::ConstPool { .. }
                    | IlOp::String { .. }
                    | IlOp::Dup { .. }
                    | IlOp::Bin { .. }
                    | IlOp::BinSlotImm { .. }
                    | IlOp::BinSlotSlot { .. }
            ) || matches!(
                op.as_plain_byte(),
                Some(b) if matches!(
                    *b.bytecode(),
                    Instruction::LOAD
                        | Instruction::CONST
                        | Instruction::STRING
                        | Instruction::DUPLICATE
                        | Instruction::ADD
                        | Instruction::SUB
                        | Instruction::MUL
                        | Instruction::DIV
                        | Instruction::MOD
                        | Instruction::BinSlotImm
                        | Instruction::BinSlotSlot
                        | Instruction::EQ
                        | Instruction::NEQ
                        | Instruction::LE
                        | Instruction::LEQ
                        | Instruction::GT
                        | Instruction::GEQ
                )
            )
        })
    }

    fn inline_forbidden_op(op: &IlOp) -> bool {
        matches!(
            op,
            IlOp::HostInvoke { .. }
                | IlOp::Print { .. }
                | IlOp::GetField { .. }
                | IlOp::SetField { .. }
                | IlOp::LoadField { .. }
                | IlOp::MakeTuple { .. }
                | IlOp::MakeArray { .. }
                | IlOp::MakeEnum { .. }
        ) || match op.as_plain_byte() {
            None => true,
            Some(b) => matches!(
                *b.bytecode(),
                Instruction::CALL
                    | Instruction::TailCall
                    | Instruction::MakeCoro
                    | Instruction::CallIndirect
                    | Instruction::YieldCoro
                    | Instruction::YieldFromCoro
                    | Instruction::LoadField
                    | Instruction::MakeEnum
                    | Instruction::MakeArray
                    | Instruction::MakeTuple
                    | Instruction::JumpIfMatch
                    | Instruction::HostInvoke
                    | Instruction::FfiInvoke
                    | Instruction::PRINT
                    | Instruction::GetField
                    | Instruction::SetField
                    | Instruction::JMP
                    | Instruction::JMPF
                    | Instruction::JMPT
                    | Instruction::BinReturn
                    | Instruction::CmpJmpf
                    | Instruction::BinSlotImmJmpf
                    | Instruction::BinSlotSlotJmpf
                    | Instruction::LogNotJmpf
                    | Instruction::LoadReturnSlot
                    | Instruction::ConstReturnImm
            ),
        }
    }

    /// Expand a fused `*Return` byte into the producer left on the caller's stack.
    fn expand_fused_return_for_inline(byte: &Byte, temps: &[u32]) -> Option<Byte> {
        match *byte.bytecode() {
            Instruction::ConstReturnImm => {
                Some(Byte::new(Instruction::CONST).with_const_inline(byte.operand_u32() as i32))
            }
            Instruction::LoadReturnSlot => {
                let slot = byte.operand_u32() as usize;
                let &tmp = temps.get(slot)?;
                Some(Byte::new(Instruction::LOAD).with_load_store_slot(tmp))
            }
            _ => None,
        }
    }

    /// Expand sole `BinReturn` at a call site: reload caller temps then the plain op.
    fn expand_bin_return_for_inline(byte: &Byte, temps: &[u32], out: &mut Vec<Byte>) -> bool {
        if *byte.bytecode() != Instruction::BinReturn {
            return false;
        }
        let op: Instruction = byte.bin_return_op().into();
        for &tmp in temps {
            out.push_load(tmp);
        }
        out.push(Byte::new(op));
        true
    }

    /// Remap callee-frame slots in fused `BinSlot*` to caller temps.
    ///
    /// Returns `None` if any slot is out of arity or the remapped index exceeds
    /// the `u8` packing used by these opcodes.
    fn remap_bin_slot_for_inline(byte: &Byte, temps: &[u32]) -> Option<Byte> {
        match *byte.bytecode() {
            Instruction::BinSlotImm => {
                let (op, slot, imm) = byte.bin_slot_imm_parts();
                let &tmp = temps.get(slot)?;
                if tmp > u8::MAX as u32 {
                    return None;
                }
                Some(
                    Byte::new(Instruction::BinSlotImm).with_bin_slot_imm(op, tmp as u8, imm as i16),
                )
            }
            Instruction::BinSlotSlot => {
                let (op, a, b) = byte.bin_slot_slot_parts();
                let &ta = temps.get(a)?;
                let &tb = temps.get(b)?;
                if ta > u8::MAX as u32 || tb > u8::MAX as u32 {
                    return None;
                }
                Some(Byte::new(Instruction::BinSlotSlot).with_bin_slot_slot(op, ta as u8, tb as u8))
            }
            _ => None,
        }
    }

    fn try_emit_inline_direct_call(
        &mut self,
        fqn: &str,
        args: Option<&[Output<'_>]>,
        bytecode: &mut Vec<Byte>,
    ) -> bool {
        let Some((start, end)) = self.fn_bytecode_spans.get(fqn).copied() else {
            return false;
        };
        let lookup = strip_overload_key(fqn).to_string();
        if self.checker.fn_has_rest(&lookup) {
            return false;
        }
        let ops = self.bytecode.code_slice_ops(start, end);
        if !Self::is_tiny_inline_il(&ops) {
            return false;
        }
        let arg_slice = args.unwrap_or(&[]);
        let mut temps = Vec::new();
        let flat = self.flatten_call_args_for_emit(arg_slice);
        for arg in &flat {
            let value = match arg.1.as_ref() {
                Expression::NamedArg(_, v) => v,
                _ => arg,
            };
            bytecode.append(&mut self.do_compile(value));
            let tmp = self.alloc_temp_slot();
            bytecode.push_store_pop(tmp);
            temps.push(tmp);
        }
        // Compare+branch diamond: emit CFG into `self.bytecode`, stash the
        // result in a temp, and leave a LOAD in `bytecode` so parents that
        // accumulate into a local Vec keep program order.
        //
        // On emit failure, roll back and clear arg prep so peel/call can
        // re-emit cleanly — a partial diamond leaves `JMP end_label` unbound
        // (resolves to PC 0) and poisons later fallbacks.
        if Self::is_tiny_inline_diamond_il(&ops) {
            let raw = self.bytecode.code_slice_raw_ops(start, end);
            let rollback = self.bytecode.len();
            self.bytecode.append(bytecode);
            if !self.emit_cfg_inline_body(&raw, &temps, /*allow_calls=*/ false) {
                self.bytecode.truncate(rollback);
                bytecode.clear();
                return false;
            }
            let result = self.alloc_temp_slot();
            self.bytecode.push_store_pop(result);
            bytecode.push_load(result);
            return true;
        }
        let slice = self.bytecode.code_slice_bytes(start, end);
        if slice.len() == 1
            && let Some(expanded) = Self::expand_fused_return_for_inline(&slice[0], &temps)
        {
            bytecode.push(expanded);
            return true;
        }
        if slice.len() == 1 && Self::expand_bin_return_for_inline(&slice[0], &temps, bytecode) {
            return true;
        }
        for byte in &slice {
            if matches!(byte.bytecode(), Instruction::RETURN) {
                break;
            }
            if matches!(byte.bytecode(), Instruction::LOAD) {
                let Some(slot) = byte.load_store_single_slot() else {
                    return false;
                };
                let Some(&tmp) = temps.get(slot as usize) else {
                    return false;
                };
                bytecode.push_load(tmp);
            } else if matches!(
                byte.bytecode(),
                Instruction::BinSlotImm | Instruction::BinSlotSlot
            ) {
                let Some(remapped) = Self::remap_bin_slot_for_inline(byte, &temps) else {
                    return false;
                };
                bytecode.push(remapped);
            } else {
                bytecode.push(*byte);
            }
        }
        true
    }

    /// One-level self-unroll: peel callee body once at a self-`CALL` site.
    /// Nested self-calls remain `CALL`/`Entry`. Emits into `self.bytecode`.
    fn try_emit_self_unroll_call(
        &mut self,
        fqn: &str,
        args: Option<&[Output<'_>]>,
        bytecode: &mut Vec<Byte>,
    ) -> bool {
        let Some((start, end)) = self.fn_bytecode_spans.get(fqn).copied() else {
            return false;
        };
        let lookup = strip_overload_key(fqn).to_string();
        if self.checker.fn_has_rest(&lookup) {
            return false;
        }
        if self.coroutine_fns.contains(fqn) || self.coroutine_fns.contains(&lookup) {
            return false;
        }
        // Skip callees that use defer (body would miss deferred side effects).
        // `fn_defers` is only populated while compiling the callee; once its
        // body is finished the stack is empty — refuse bodies that contain
        // MakeCoro / Yield (already gated) and nested `fn` defs (not in span).
        let ops = self.bytecode.code_slice_ops(start, end);
        let self_entry = self.fn_entry_labels.get(fqn).copied();
        if !Self::is_self_unroll_il(&ops, self_entry) {
            return false;
        }
        // Refuse locals beyond arity (temps only cover args).
        let arity = self.flatten_call_args_for_emit(args.unwrap_or(&[])).len();
        if Self::body_uses_slot_past(&ops, arity) {
            return false;
        }
        let arg_slice = args.unwrap_or(&[]);
        let mut temps = Vec::new();
        let flat = self.flatten_call_args_for_emit(arg_slice);
        for arg in &flat {
            let value = match arg.1.as_ref() {
                Expression::NamedArg(_, v) => v,
                _ => arg,
            };
            bytecode.append(&mut self.do_compile(value));
            let tmp = self.alloc_temp_slot();
            bytecode.push_store_pop(tmp);
            temps.push(tmp);
        }
        let raw = self.bytecode.code_slice_raw_ops(start, end);
        let rollback = self.bytecode.len();
        self.bytecode.append(bytecode);
        if !self.emit_cfg_inline_body(&raw, &temps, /*allow_calls=*/ true) {
            self.bytecode.truncate(rollback);
            bytecode.clear();
            return false;
        }
        let result = self.alloc_temp_slot();
        self.bytecode.push_store_pop(result);
        bytecode.push_load(result);
        true
    }

    /// Caller-side predicate peel (2B): when callee opens with compare+JMPF and an
    /// immediate/slot base return, evaluate that check before `CALL` so base cases
    /// skip the frame. Nested/false path still `CALL`s.
    fn try_emit_predicate_peel_call(
        &mut self,
        fqn: &str,
        args: Option<&[Output<'_>]>,
        bytecode: &mut Vec<Byte>,
        target_offset: u32,
        is_indirect: bool,
    ) -> bool {
        let Some((start, end)) = self.fn_bytecode_spans.get(fqn).copied() else {
            return false;
        };
        let lookup = strip_overload_key(fqn).to_string();
        if self.checker.fn_has_rest(&lookup) {
            return false;
        }
        if self.coroutine_fns.contains(fqn) || self.coroutine_fns.contains(&lookup) {
            return false;
        }
        if self.checker.is_generic_fn(&lookup) {
            return false;
        }
        let ops = self.bytecode.code_slice_raw_ops(start, end);
        let Some(peel) = Self::match_predicate_peel_shape(&ops) else {
            return false;
        };
        drop(ops);
        let arg_slice = args.unwrap_or(&[]);
        let flat = self.flatten_call_args_for_emit(arg_slice);
        if flat.len() < peel.arity_hint {
            return false;
        }
        // Pre-check slot remapping against arity (temps will be 1:1 with flat).
        let fake_temps: Vec<u32> = (0..flat.len() as u32).collect();
        if !Self::remap_peel_ops_ok(&peel, &fake_temps) {
            return false;
        }
        // Evaluate args into temps (reuse pure-first when mixed).
        let mut temps = Vec::with_capacity(flat.len());
        if Self::should_reorder_pure_call_args(&flat) {
            let mut slots = vec![0u32; flat.len()];
            for (i, arg) in flat.iter().enumerate() {
                if !Self::call_arg_is_pure(arg) {
                    continue;
                }
                let value = match arg.1.as_ref() {
                    Expression::NamedArg(_, v) => v,
                    _ => arg,
                };
                bytecode.append(&mut self.do_compile(value));
                let tmp = self.alloc_temp_slot();
                bytecode.push_store_pop(tmp);
                slots[i] = tmp;
            }
            for (i, arg) in flat.iter().enumerate() {
                if Self::call_arg_is_pure(arg) {
                    continue;
                }
                let value = match arg.1.as_ref() {
                    Expression::NamedArg(_, v) => v,
                    _ => arg,
                };
                if self.arg_emits_on_self_bytecode(value) {
                    self.stage_call_arg_to_temp(value, false, &mut slots[i]);
                } else {
                    bytecode.append(&mut self.do_compile(value));
                    let tmp = self.alloc_temp_slot();
                    bytecode.push_store_pop(tmp);
                    slots[i] = tmp;
                }
            }
            temps = slots;
        } else {
            for arg in &flat {
                let value = match arg.1.as_ref() {
                    Expression::NamedArg(_, v) => v,
                    _ => arg,
                };
                if self.arg_emits_on_self_bytecode(value) {
                    let mut slot = 0u32;
                    self.stage_call_arg_to_temp(value, false, &mut slot);
                    temps.push(slot);
                } else {
                    bytecode.append(&mut self.do_compile(value));
                    let tmp = self.alloc_temp_slot();
                    bytecode.push_store_pop(tmp);
                    temps.push(tmp);
                }
            }
        }

        // Emit into self.bytecode then leave a LOAD of the result in `bytecode`
        // so parents that accumulate into a local Vec keep program order.
        self.bytecode.append(bytecode);
        let do_call = self.bytecode.fresh_label();
        let join = self.bytecode.fresh_label();

        // Remapped condition; JMPF → do_call (false path continues into CALL).
        for op in &peel.cond {
            if !self.emit_peel_remapped_op(op, &temps) {
                return false;
            }
        }
        self.bytecode.push_op(IlOp::Jump {
            kind: IlJumpKind::JumpIfFalse,
            target: do_call,
            loc: DebugLoc::unknown(),
        });
        // Base-case then-arm value.
        if !self.emit_peel_remapped_op(&peel.then_value, &temps) {
            return false;
        }
        self.bytecode.push_op(IlOp::Jump {
            kind: IlJumpKind::Unconditional,
            target: join,
            loc: DebugLoc::unknown(),
        });
        self.bytecode.bind_label(do_call);
        for &tmp in &temps {
            self.bytecode.push_load(tmp);
        }
        let arity = temps.len() as u32;
        if is_indirect {
            Self::emit_call_indirect(&mut self.bytecode, target_offset, arity);
        } else {
            self.bytecode
                .push(Byte::new(Instruction::CALL).with_call_packed(arity, target_offset));
        }
        self.bytecode.bind_label(join);
        let result = self.alloc_temp_slot();
        self.bytecode.push_store_pop(result);
        bytecode.push_load(result);
        true
    }

    /// Opening shape: `cond…; JumpIfFalse; (Const|Load) [; Return]; Label? …`
    /// with an imm/slot base return. `arity_hint` is 1 + max slot referenced.
    fn match_predicate_peel_shape(ops: &[IlOp]) -> Option<PredicatePeel> {
        // Skip leading labels.
        let mut i = 0usize;
        while i < ops.len() && matches!(ops[i], IlOp::Label(_)) {
            i += 1;
        }
        if i >= ops.len() {
            return None;
        }
        let jump_idx = (i..ops.len()).find(|&j| {
            matches!(
                ops[j],
                IlOp::Jump {
                    kind: IlJumpKind::JumpIfFalse,
                    ..
                }
            )
        })?;
        if jump_idx == i {
            return None;
        }
        let cond = &ops[i..jump_idx];
        // Cond must be pure producers only (no control / calls / effects).
        if cond.iter().any(|op| {
            op.is_control()
                || matches!(
                    op,
                    IlOp::HostInvoke { .. }
                        | IlOp::Print { .. }
                        | IlOp::Entry { .. }
                        | IlOp::SetField { .. }
                        | IlOp::GetField { .. }
                )
                || matches!(
                    op.as_plain_byte(),
                    Some(b) if matches!(
                        *b.bytecode(),
                        Instruction::CALL
                            | Instruction::TailCall
                            | Instruction::HostInvoke
                            | Instruction::PRINT
                            | Instruction::FfiInvoke
                    )
                )
        }) {
            return None;
        }
        // Pure cond ops: Load/Const/Bin/BinSlot*/Dup/ConstPool.
        if !cond.iter().all(|op| {
            matches!(
                op,
                IlOp::Load { .. }
                    | IlOp::Const { .. }
                    | IlOp::ConstPool { .. }
                    | IlOp::Dup { .. }
                    | IlOp::Bin { .. }
                    | IlOp::BinSlotImm { .. }
                    | IlOp::BinSlotSlot { .. }
            ) || matches!(
                op.as_plain_byte(),
                Some(b) if matches!(
                    *b.bytecode(),
                    Instruction::LOAD
                        | Instruction::CONST
                        | Instruction::DUPLICATE
                        | Instruction::ADD
                        | Instruction::SUB
                        | Instruction::MUL
                        | Instruction::DIV
                        | Instruction::MOD
                        | Instruction::EQ
                        | Instruction::NEQ
                        | Instruction::LE
                        | Instruction::LEQ
                        | Instruction::GT
                        | Instruction::GEQ
                        | Instruction::AND
                        | Instruction::OR
                        | Instruction::BITAND
                        | Instruction::BITOR
                        | Instruction::SHL
                        | Instruction::SHR
                        | Instruction::XOR
                        | Instruction::BinSlotImm
                        | Instruction::BinSlotSlot
                )
            )
        }) {
            return None;
        }
        let then_start = jump_idx + 1;
        if then_start >= ops.len() {
            return None;
        }
        // Then: fused *Return, or Const/Load + Return, or sole Const/Load before label.
        let (then_value, then_end) = if Self::inline_is_fused_return(&ops[then_start]) {
            match &ops[then_start] {
                IlOp::ConstReturnImm { imm, loc } => (
                    IlOp::Const {
                        imm: *imm as i32,
                        loc: *loc,
                    },
                    then_start,
                ),
                IlOp::LoadReturnSlot { slot, loc } => (
                    IlOp::Load {
                        slot: *slot,
                        loc: *loc,
                    },
                    then_start,
                ),
                _ => return None, // BinReturn not an imm/slot base
            }
        } else {
            let v = match &ops[then_start] {
                IlOp::Const { .. } | IlOp::Load { .. } => ops[then_start].clone(),
                other => {
                    if let Some(b) = other.as_plain_byte() {
                        match *b.bytecode() {
                            Instruction::CONST => IlOp::Const {
                                imm: b.operand_u32() as i32,
                                loc: DebugLoc::unknown(),
                            },
                            Instruction::LOAD => {
                                let slot = b.load_store_single_slot()?;
                                IlOp::Load {
                                    slot,
                                    loc: DebugLoc::unknown(),
                                }
                            }
                            _ => return None,
                        }
                    } else {
                        return None;
                    }
                }
            };
            let mut end = then_start;
            if then_start + 1 < ops.len() && ops[then_start + 1].is_plain_return() {
                end = then_start + 1;
            } else if then_start + 1 < ops.len() && matches!(ops[then_start + 1], IlOp::Label(_)) {
                // fall through after value (unusual); still ok if JMPF target
            } else if then_start + 1 < ops.len()
                && !matches!(ops[then_start + 1], IlOp::Label(_))
                && !ops[then_start + 1].is_plain_return()
            {
                return None;
            }
            (v, end)
        };
        // After then-arm there should be more body (otherwise tiny-inline diamond
        // would have taken it). Require at least one emitting op past the peel.
        let after = then_end + 1;
        let has_rest = ops[after..].iter().any(|op| !matches!(op, IlOp::Label(_)));
        if !has_rest {
            return None;
        }
        let mut arity_hint = 0usize;
        let bump_slot = |slot: u32, hint: &mut usize| {
            *hint = (*hint).max(slot as usize + 1);
        };
        for op in cond {
            match op {
                IlOp::Load { slot, .. } => bump_slot(*slot, &mut arity_hint),
                IlOp::BinSlotImm { slot, .. } => bump_slot(*slot as u32, &mut arity_hint),
                IlOp::BinSlotSlot { a, b, .. } => {
                    bump_slot(*a as u32, &mut arity_hint);
                    bump_slot(*b as u32, &mut arity_hint);
                }
                _ => {}
            }
        }
        if let IlOp::Load { slot, .. } = &then_value {
            bump_slot(*slot, &mut arity_hint);
        }
        Some(PredicatePeel {
            cond: cond.to_vec(),
            then_value,
            arity_hint,
        })
    }

    fn remap_peel_ops_ok(peel: &PredicatePeel, temps: &[u32]) -> bool {
        let ok_slot = |s: u32| (s as usize) < temps.len();
        for op in &peel.cond {
            match op {
                IlOp::Load { slot, .. } if !ok_slot(*slot) => return false,
                IlOp::BinSlotImm { slot, .. } if !ok_slot(*slot as u32) => return false,
                IlOp::BinSlotSlot { a, b, .. } if !ok_slot(*a as u32) || !ok_slot(*b as u32) => {
                    return false;
                }
                _ => {}
            }
        }
        match &peel.then_value {
            IlOp::Load { slot, .. } => ok_slot(*slot),
            IlOp::Const { .. } => true,
            _ => false,
        }
    }

    fn emit_peel_remapped_op(&mut self, op: &IlOp, temps: &[u32]) -> bool {
        match op {
            IlOp::Load { slot, loc } => {
                let Some(&tmp) = temps.get(*slot as usize) else {
                    return false;
                };
                self.bytecode.push_op(IlOp::Load {
                    slot: tmp,
                    loc: *loc,
                });
                true
            }
            IlOp::Const { imm, loc } => {
                self.bytecode.push_op(IlOp::Const {
                    imm: *imm,
                    loc: *loc,
                });
                true
            }
            IlOp::ConstPool { idx, loc } => {
                self.bytecode.push_op(IlOp::ConstPool {
                    idx: *idx,
                    loc: *loc,
                });
                true
            }
            IlOp::String { idx, loc } => {
                self.bytecode.push_op(IlOp::String {
                    idx: *idx,
                    loc: *loc,
                });
                true
            }
            IlOp::Dup { loc } => {
                self.bytecode.push_op(IlOp::Dup { loc: *loc });
                true
            }
            IlOp::Bin { op: bin, loc } => {
                self.bytecode.push_op(IlOp::Bin {
                    op: *bin,
                    loc: *loc,
                });
                true
            }
            IlOp::BinSlotImm {
                op: bin,
                slot,
                imm,
                loc,
            } => {
                let Some(&tmp) = temps.get(*slot as usize) else {
                    return false;
                };
                if tmp > u8::MAX as u32 {
                    return false;
                }
                self.bytecode.push_op(IlOp::BinSlotImm {
                    op: *bin,
                    slot: tmp as u8,
                    imm: *imm,
                    loc: *loc,
                });
                true
            }
            IlOp::BinSlotSlot { op: bin, a, b, loc } => {
                let Some(&ta) = temps.get(*a as usize) else {
                    return false;
                };
                let Some(&tb) = temps.get(*b as usize) else {
                    return false;
                };
                if ta > u8::MAX as u32 || tb > u8::MAX as u32 {
                    return false;
                }
                self.bytecode.push_op(IlOp::BinSlotSlot {
                    op: *bin,
                    a: ta as u8,
                    b: tb as u8,
                    loc: *loc,
                });
                true
            }
            other => {
                if let Some(b) = other.as_plain_byte() {
                    self.bytecode.push(b);
                    true
                } else {
                    false
                }
            }
        }
    }

    fn body_uses_slot_past(ops: &[IlOp], arity: usize) -> bool {
        for op in ops {
            match op {
                IlOp::Load { slot, .. } | IlOp::StorePop { slot, .. } => {
                    if *slot as usize >= arity {
                        return true;
                    }
                }
                IlOp::LoadReturnSlot { slot, .. } => {
                    if *slot as usize >= arity {
                        return true;
                    }
                }
                IlOp::BinSlotImm { slot, .. } => {
                    if *slot as usize >= arity {
                        return true;
                    }
                }
                IlOp::BinSlotSlot { a, b, .. } => {
                    if *a as usize >= arity || *b as usize >= arity {
                        return true;
                    }
                }
                _ => {
                    if let Some(b) = op.as_plain_byte() {
                        match *b.bytecode() {
                            Instruction::LOAD | Instruction::STORE => {
                                if b.load_store_single_slot()
                                    .is_some_and(|s| s as usize >= arity)
                                {
                                    return true;
                                }
                            }
                            Instruction::LoadReturnSlot => {
                                if b.operand_u32() as usize >= arity {
                                    return true;
                                }
                            }
                            Instruction::BinSlotImm => {
                                if b.bin_slot_imm_parts().1 >= arity {
                                    return true;
                                }
                            }
                            Instruction::BinSlotSlot => {
                                let (_, a, bslot) = b.bin_slot_slot_parts();
                                if a >= arity || bslot >= arity {
                                    return true;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        false
    }

    /// Copy a CFG-bearing callee body into `self.bytecode`, remapping slots to
    /// `temps`. Strips `RETURN` / fused `*Return` so the value stays on stack.
    /// When `allow_calls` is set, `Entry`/`CALL` are preserved (self-unroll).
    fn emit_cfg_inline_body(&mut self, ops: &[IlOp], temps: &[u32], allow_calls: bool) -> bool {
        use std::collections::HashMap;
        let mut label_map: HashMap<u32, IlLabel> = HashMap::new();
        let mut ensure_label = |id: u32, bc: &mut CodeBuf| -> IlLabel {
            *label_map.entry(id).or_insert_with(|| bc.fresh_label())
        };
        // Pre-allocate labels referenced by jumps.
        for op in ops {
            if let IlOp::Jump { target, .. } = op {
                let _ = ensure_label(target.0, &mut self.bytecode);
            }
            if let IlOp::Label(l) = op {
                let _ = ensure_label(l.0, &mut self.bytecode);
            }
        }
        let end_label = self.bytecode.fresh_label();
        let mut saw_value = false;
        for op in ops {
            match op {
                IlOp::Label(l) => {
                    let mapped = ensure_label(l.0, &mut self.bytecode);
                    self.bytecode.bind_label(mapped);
                }
                IlOp::Jump { kind, target, loc } => {
                    let mapped = ensure_label(target.0, &mut self.bytecode);
                    self.bytecode.push_op(IlOp::Jump {
                        kind: *kind,
                        target: mapped,
                        loc: *loc,
                    });
                }
                IlOp::Entry {
                    kind,
                    arity,
                    target,
                    loc,
                } => {
                    if !allow_calls {
                        return false;
                    }
                    // Peel is an expression context: TailCall would replace the
                    // caller's frame and never yield a value back. Demote to Call.
                    let kind = match kind {
                        EntryKind::TailCall => EntryKind::Call,
                        other => *other,
                    };
                    self.bytecode.push_op(IlOp::Entry {
                        kind,
                        arity: *arity,
                        target: *target,
                        loc: *loc,
                    });
                    saw_value = true;
                }
                IlOp::Return { .. } => {
                    // Arm/function return → jump to join with value on stack.
                    self.bytecode.push_op(IlOp::Jump {
                        kind: IlJumpKind::Unconditional,
                        target: end_label,
                        loc: DebugLoc::unknown(),
                    });
                    saw_value = true;
                }
                IlOp::ConstReturnImm { imm, loc } => {
                    self.bytecode.push_op(IlOp::Const {
                        imm: *imm as i32,
                        loc: *loc,
                    });
                    self.bytecode.push_op(IlOp::Jump {
                        kind: IlJumpKind::Unconditional,
                        target: end_label,
                        loc: DebugLoc::unknown(),
                    });
                    saw_value = true;
                }
                IlOp::LoadReturnSlot { slot, loc } => {
                    let Some(&tmp) = temps.get(*slot as usize) else {
                        return false;
                    };
                    self.bytecode.push_op(IlOp::Load {
                        slot: tmp,
                        loc: *loc,
                    });
                    self.bytecode.push_op(IlOp::Jump {
                        kind: IlJumpKind::Unconditional,
                        target: end_label,
                        loc: DebugLoc::unknown(),
                    });
                    saw_value = true;
                }
                IlOp::BinReturn { op: bin_op, loc } => {
                    for &tmp in temps {
                        self.bytecode.push_op(IlOp::Load {
                            slot: tmp,
                            loc: *loc,
                        });
                    }
                    self.bytecode.push_op(IlOp::Bin {
                        op: *bin_op,
                        loc: *loc,
                    });
                    self.bytecode.push_op(IlOp::Jump {
                        kind: IlJumpKind::Unconditional,
                        target: end_label,
                        loc: DebugLoc::unknown(),
                    });
                    saw_value = true;
                }
                IlOp::Load { slot, loc } => {
                    let Some(&tmp) = temps.get(*slot as usize) else {
                        return false;
                    };
                    self.bytecode.push_op(IlOp::Load {
                        slot: tmp,
                        loc: *loc,
                    });
                }
                IlOp::StorePop { slot, loc } => {
                    let Some(&tmp) = temps.get(*slot as usize) else {
                        return false;
                    };
                    self.bytecode.push_op(IlOp::StorePop {
                        slot: tmp,
                        loc: *loc,
                    });
                }
                IlOp::BinSlotImm {
                    op: bin_op,
                    slot,
                    imm,
                    loc,
                } => {
                    let Some(&tmp) = temps.get(*slot as usize) else {
                        return false;
                    };
                    if tmp > u8::MAX as u32 {
                        return false;
                    }
                    self.bytecode.push_op(IlOp::BinSlotImm {
                        op: *bin_op,
                        slot: tmp as u8,
                        imm: *imm,
                        loc: *loc,
                    });
                }
                IlOp::BinSlotSlot {
                    op: bin_op,
                    a,
                    b,
                    loc,
                } => {
                    let Some(&ta) = temps.get(*a as usize) else {
                        return false;
                    };
                    let Some(&tb) = temps.get(*b as usize) else {
                        return false;
                    };
                    if ta > u8::MAX as u32 || tb > u8::MAX as u32 {
                        return false;
                    }
                    self.bytecode.push_op(IlOp::BinSlotSlot {
                        op: *bin_op,
                        a: ta as u8,
                        b: tb as u8,
                        loc: *loc,
                    });
                }
                other => {
                    // Plain producers / residual bytes — remap LOAD/STORE/BinSlot*.
                    if let Some(b) = other.as_plain_byte() {
                        match *b.bytecode() {
                            Instruction::RETURN => {
                                self.bytecode.push_op(IlOp::Jump {
                                    kind: IlJumpKind::Unconditional,
                                    target: end_label,
                                    loc: DebugLoc::unknown(),
                                });
                                saw_value = true;
                            }
                            Instruction::ConstReturnImm => {
                                self.bytecode.push_const(b.operand_u32() as i32);
                                self.bytecode.push_op(IlOp::Jump {
                                    kind: IlJumpKind::Unconditional,
                                    target: end_label,
                                    loc: DebugLoc::unknown(),
                                });
                                saw_value = true;
                            }
                            Instruction::LoadReturnSlot => {
                                let Some(&tmp) = temps.get(b.operand_u32() as usize) else {
                                    return false;
                                };
                                self.bytecode.push_load(tmp);
                                self.bytecode.push_op(IlOp::Jump {
                                    kind: IlJumpKind::Unconditional,
                                    target: end_label,
                                    loc: DebugLoc::unknown(),
                                });
                                saw_value = true;
                            }
                            Instruction::BinReturn => {
                                let op: Instruction = b.bin_return_op().into();
                                for &tmp in temps {
                                    self.bytecode.push_load(tmp);
                                }
                                self.bytecode.push(Byte::new(op));
                                self.bytecode.push_op(IlOp::Jump {
                                    kind: IlJumpKind::Unconditional,
                                    target: end_label,
                                    loc: DebugLoc::unknown(),
                                });
                                saw_value = true;
                            }
                            Instruction::LOAD => {
                                let Some(slot) = b.load_store_single_slot() else {
                                    return false;
                                };
                                let Some(&tmp) = temps.get(slot as usize) else {
                                    return false;
                                };
                                self.bytecode.push_load(tmp);
                            }
                            Instruction::STORE => {
                                let Some(slot) = b.load_store_single_slot() else {
                                    return false;
                                };
                                let Some(&tmp) = temps.get(slot as usize) else {
                                    return false;
                                };
                                self.bytecode.push_store_pop(tmp);
                            }
                            Instruction::BinSlotImm | Instruction::BinSlotSlot => {
                                let Some(remapped) = Self::remap_bin_slot_for_inline(&b, temps)
                                else {
                                    return false;
                                };
                                self.bytecode.push(remapped);
                            }
                            Instruction::CALL | Instruction::TailCall => {
                                if !allow_calls {
                                    return false;
                                }
                                // Expression peel: TailCall → CALL so the value returns.
                                let (arity, target) = b.call_parts();
                                self.bytecode.push(
                                    Byte::new(Instruction::CALL)
                                        .with_call_packed(arity as u32, target as u32),
                                );
                                saw_value = true;
                            }
                            _ => {
                                if Self::inline_forbidden_op(other) && !allow_calls {
                                    return false;
                                }
                                self.bytecode.push_op(other.clone());
                            }
                        }
                    } else {
                        self.bytecode.push_op(other.clone());
                    }
                }
            }
        }
        self.bytecode.bind_label(end_label);
        saw_value
    }
}

impl Compiler {
    pub fn get_function(&self, name: &str) -> usize {
        self.functions[name]
    }

    pub fn function_offset(&self, name: &str) -> Option<usize> {
        self.functions.get(name).copied()
    }

    /// Bind a fresh entry label at the current PC and register `name`.
    fn bind_function_entry(&mut self, name: String) -> (usize, IlLabel) {
        let offset = self.bytecode.len();
        let label = self.bytecode.bind_fresh_entry();
        self.functions.insert(name.clone(), offset);
        self.fn_entry_labels.insert(name, label);
        (offset, label)
    }

    /// Entry label for a registered function, if bound.
    #[allow(dead_code)] // call-site Entry emit / step-5 assert helpers
    fn fn_entry_label(&self, name: &str) -> Option<IlLabel> {
        self.fn_entry_labels.get(name).copied()
    }

    /// Bytecode offset WHERE the prologue (CALL+JMP+HALT)
    /// ENDS and user-program code BEGINS. Used by the runtime
    /// pipeline to patch the prologue's JMP operand so that
    /// any module-level `extern` block bytes (appended to
    /// `self.bytecode` before `main`) execute before main.
    /// Without this, the prologue would skip past the extern
    /// block entirely (because `main_offset` lands past it).
    pub fn program_start_offset(&self) -> u32 {
        self.program_start_offset
    }

    /// True iff at least one `extern` block was emitted in
    /// the last `compile`. The pipeline uses this to decide
    /// whether to JMP to `program_start_offset` (which would
    /// execute `extern` block bytes first) or directly to
    /// `main` (which is correct when no extern was used).
    pub fn has_extern_block(&self) -> bool {
        !self.extern_runtime_functions.is_empty()
    }

    /// Record a user-visible local/param for `coil debug` / dissect.
    ///
    /// Skips synthetic `__pad*` / `__dict*` names. `__shadow_name_N` is stored
    /// under the user-facing `name`.
    fn record_debug_local(&mut self, name: &str, slot: u32) {
        if name.starts_with("__pad") || name.starts_with("__dict") {
            return;
        }
        let display = if let Some(rest) = name.strip_prefix("__shadow_") {
            rest.rsplit_once('_').map(|(n, _)| n).unwrap_or(rest)
        } else {
            name
        };
        let Some(key) = self.current_function_table_key.clone() else {
            return;
        };
        self.fn_debug_locals
            .entry(key)
            .or_default()
            .insert(display.to_string(), slot);
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
        if let Some(map) = &self.context.match_bindings
            && let Some(&slot) = map.get(name)
        {
            return Some(slot);
        }
        // Innermost block overlay first (walk `prev` for nested blocks).
        let mut ctx = Some(&self.context);
        while let Some(c) = ctx {
            if let Some(map) = &c.block_bindings
                && let Some(&slot) = map.get(name)
            {
                return Some(slot);
            }
            ctx = c.prev.as_deref();
        }
        self.context
            .variables
            .key(&name.to_string())
            .map(|s| s as u32)
    }

    /// Allocate a locals slot for a `let` / destructure binder.
    ///
    /// Inside a block (`block_bindings = Some`), re-binding a name that is
    /// already visible in an outer scope gets a **fresh** slot so the outer
    /// value is not overwritten.
    fn alloc_binding_slot(&mut self, name: &str) -> u32 {
        if let Some(map) = &self.context.block_bindings
            && let Some(&slot) = map.get(name)
        {
            self.record_debug_local(name, slot);
            return slot;
        }
        if self.context.block_bindings.is_none() {
            let slot = self.context.variables.intern(name.to_string()) as u32;
            self.record_debug_local(name, slot);
            return slot;
        }
        let shadows_outer = {
            let in_vars = self.context.variables.key(&name.to_string()).is_some();
            let mut in_ancestor = false;
            let mut ctx = self.context.prev.as_deref();
            while let Some(c) = ctx {
                if let Some(map) = &c.block_bindings
                    && map.contains_key(name)
                {
                    in_ancestor = true;
                    break;
                }
                ctx = c.prev.as_deref();
            }
            in_vars || in_ancestor
        };
        if shadows_outer {
            self.temp_counter += 1;
            let synthetic = format!("__shadow_{}_{}", name, self.temp_counter);
            let slot = self.context.variables.intern(synthetic) as u32;
            self.context
                .block_bindings
                .as_mut()
                .expect("block_bindings checked above")
                .insert(name.to_string(), slot);
            self.record_debug_local(name, slot);
            slot
        } else {
            let slot = self.context.variables.intern(name.to_string()) as u32;
            self.record_debug_local(name, slot);
            slot
        }
    }

    /// Flatten `...expr` spread nodes for codegen using inferred types.
    fn flatten_call_args_for_emit<'a>(&self, args: &[Output<'a>]) -> Vec<Output<'a>> {
        use crate::typechecking::subst::apply_ty_prune;
        use crate::typechecking::ty::{ArrayLength, Ty};
        let mut out = Vec::new();
        for arg in args {
            if let Expression::Spread(inner) = arg.1.as_ref() {
                if let Expression::Array(items) = inner.1.as_ref() {
                    for i in 0..items.len() {
                        let span = inner.0;
                        out.push((
                            span,
                            Box::new(Expression::Index(
                                inner.clone(),
                                Some((span, Box::new(Expression::Integer(i as i64)))),
                            )),
                        ));
                    }
                    continue;
                }
                if let Expression::Tuple(items) = inner.1.as_ref() {
                    for i in 0..items.len() {
                        let span = inner.0;
                        out.push((
                            span,
                            Box::new(Expression::Index(
                                inner.clone(),
                                Some((span, Box::new(Expression::Integer(i as i64)))),
                            )),
                        ));
                    }
                    continue;
                }
                let ty = self.codegen_expr_ty(inner).or_else(|| {
                    self.checker
                        .lookup_for_codegen_span(inner.0.start, inner.0.end)
                        .map(|t| apply_ty_prune(self.checker.subst(), &t))
                });
                let Some(ty) = ty else {
                    out.push(arg.clone());
                    continue;
                };
                let resolved = apply_ty_prune(self.checker.subst(), &ty);
                match resolved {
                    Ty::Tuple(elems) => {
                        for i in 0..elems.len() {
                            let span = inner.0;
                            out.push((
                                span,
                                Box::new(Expression::Index(
                                    inner.clone(),
                                    Some((span, Box::new(Expression::Integer(i as i64)))),
                                )),
                            ));
                        }
                    }
                    Ty::Array {
                        length: ArrayLength::Static(n),
                        ..
                    } => {
                        for i in 0..n {
                            let span = inner.0;
                            out.push((
                                span,
                                Box::new(Expression::Index(
                                    inner.clone(),
                                    Some((span, Box::new(Expression::Integer(i as i64)))),
                                )),
                            ));
                        }
                    }
                    _ => out.push(arg.clone()),
                }
            } else {
                out.push(arg.clone());
            }
        }
        out
    }

    /// Split call args into fixed formals + rest elements (P4).
    ///
    /// Returns `(fixed, rest_elems, pack_rest)`. When `pack_rest` is true,
    /// codegen must emit `MakeArray` even if `rest_elems` is empty.
    fn split_call_args_for_rest<'a>(
        &self,
        fn_name: &str,
        args: &[Output<'a>],
    ) -> (Vec<Output<'a>>, Vec<Output<'a>>, bool) {
        let args = self.flatten_call_args_for_emit(args);
        let fn_name = strip_overload_key(fn_name);
        let has_named = args
            .iter()
            .any(|a| matches!(a.1.as_ref(), Expression::NamedArg(..)));
        let has_rest = self.checker.fn_has_rest(fn_name);
        if !has_named && !has_rest {
            return (args, Vec::new(), false);
        }
        let Some(param_names) = self.checker.fn_param_names(fn_name) else {
            return (args, Vec::new(), false);
        };
        let fixed_count = if has_rest {
            param_names.len().saturating_sub(1)
        } else {
            param_names.len()
        };
        let rest_name = if has_rest {
            param_names.get(fixed_count).map(|s| s.as_str())
        } else {
            None
        };
        let mut slots: Vec<Option<Output<'a>>> = vec![None; fixed_count];
        let mut rest = Vec::new();
        let mut next_pos = 0usize;
        for arg in &args {
            match arg.1.as_ref() {
                Expression::NamedArg(name, value) => {
                    if rest_name == Some(*name) {
                        rest.push(value.clone());
                        continue;
                    }
                    if let Some(idx) = param_names[..fixed_count].iter().position(|p| p == *name) {
                        slots[idx] = Some(value.clone());
                    }
                }
                _ => {
                    while next_pos < fixed_count && slots[next_pos].is_some() {
                        next_pos += 1;
                    }
                    if next_pos < fixed_count {
                        slots[next_pos] = Some(arg.clone());
                        next_pos += 1;
                    } else if has_rest {
                        rest.push(arg.clone());
                        next_pos += 1;
                    } else {
                        next_pos += 1;
                    }
                }
            }
        }
        let pack_rest = has_rest
            && (has_named
                || next_pos >= fixed_count
                || args.len() >= fixed_count
                || fixed_count == 0);
        let fixed: Vec<_> = slots.into_iter().flatten().collect();
        if pack_rest {
            (fixed, rest, true)
        } else {
            (fixed, Vec::new(), false)
        }
    }

    /// Consume pre-walk IDs for `Spread` nodes (flattened at call sites).
    fn consume_spread_emit_ids(&mut self, args: &[Output<'_>]) {
        for arg in args {
            if matches!(arg.1.as_ref(), Expression::Spread(_)) {
                let _ = self.next_emit_id();
            }
        }
    }

    /// Emit value args for a call, packing rest into `MakeArray` when needed.
    /// Returns the CALL arity (fixed + 1 if rest packed).
    ///
    /// When args mix pure and effectful expressions, evaluates pure args into
    /// temps first, then effectful args, then restores original CALL order via
    /// `LOAD`s (2A pure-arg reorder).
    fn emit_call_args_with_rest(
        &mut self,
        fn_name: &str,
        args: &[Output<'_>],
        bytecode: &mut Vec<Byte>,
        box_generic: bool,
    ) -> u32 {
        self.consume_spread_emit_ids(args);
        let (fixed, rest, pack_rest) = self.split_call_args_for_rest(fn_name, args);

        if !pack_rest && Self::should_reorder_pure_call_args(&fixed) {
            return self.emit_call_args_pure_first(&fixed, bytecode, box_generic);
        }
        // Two+ HostInvoke/format/match args leave values on the shared
        // operand/local stack; the next self-bytecode emit clobbers the prior
        // result. Stage those onto temps; leave identifiers in the Call vec.
        if !pack_rest
            && fixed
                .iter()
                .filter(|a| self.arg_emits_on_self_bytecode(a))
                .count()
                >= 2
        {
            return self.emit_call_args_stage_self_bc(&fixed, bytecode, box_generic);
        }

        for arg in &fixed {
            self.append_with_existential_pack(bytecode, arg);
            if box_generic {
                if let Some(arg_ty) = self.codegen_expr_ty(arg) {
                    Self::emit_box_if_needed(bytecode, &arg_ty);
                }
            }
        }
        if pack_rest {
            for arg in &rest {
                self.append_with_existential_pack(bytecode, arg);
                if box_generic {
                    if let Some(arg_ty) = self.codegen_expr_ty(arg) {
                        Self::emit_box_if_needed(bytecode, &arg_ty);
                    }
                }
            }
            if self.checker.fn_tuple_rest(fn_name) {
                bytecode.push_make_tuple(rest.len() as u32);
            } else {
                bytecode.push_make_array(rest.len() as u32);
            }
            return (fixed.len() + 1) as u32;
        }
        fixed.len() as u32
    }

    /// True when multi-arg calls must evaluate into temps before the CALL.
    ///
    /// Mixed pure/effectful args need reordering (2A pure-first). Two or more
    /// HostInvoke/`format`/`match` args are handled via
    /// [`Self::emit_call_args_stage_self_bc`]. Bare identifiers stay unstaged
    /// (see [`Self::call_arg_is_pure`]).
    fn should_reorder_pure_call_args(args: &[Output<'_>]) -> bool {
        if args.len() < 2 {
            return false;
        }
        let mut saw_pure = false;
        let mut saw_effect = false;
        for arg in args {
            if Self::call_arg_is_pure(arg) {
                saw_pure = true;
            } else {
                saw_effect = true;
            }
            if saw_pure && saw_effect {
                return true;
            }
        }
        false
    }

    /// True when compiling `expr` writes into [`Self::bytecode`] (HostInvoke,
    /// `string::format`, `match`, …) rather than only returning a local `Vec`.
    fn arg_emits_on_self_bytecode(&self, expr: &Output<'_>) -> bool {
        match expr.1.as_ref() {
            Expression::NamedArg(_, v) | Expression::Group(v) | Expression::Expr(v) => {
                self.arg_emits_on_self_bytecode(v)
            }
            Expression::Match { .. } => true,
            Expression::Call { name, .. } => {
                if let Expression::Identifier(fname) = name.1.as_ref() {
                    if self.string_builtin_for_call(fname).is_some() {
                        return true;
                    }
                    if self.checker.io_fn_in_scope(fname).is_some() {
                        return true;
                    }
                    if self.checker.thread_fn_in_scope(fname).is_some() {
                        return true;
                    }
                    if self.checker.host_fn_in_scope(fname).is_some() {
                        return true;
                    }
                    if let Some(kind) = self.checker.prelude_fn_in_scope(fname) {
                        return matches!(
                            kind,
                            crate::typechecking::PreludeFn::Ord
                                | crate::typechecking::PreludeFn::Char
                                | crate::typechecking::PreludeFn::Assert
                        );
                    }
                    if self.checker.ffi_fn_in_scope(fname).is_some() {
                        return true;
                    }
                } else if let Expression::QualifiedAccess { owner, member } = name.1.as_ref() {
                    let fqn = format!("{}::{}", owner, member);
                    if self.string_builtin_for_call(&fqn).is_some() {
                        return true;
                    }
                }
                false
            }
            _ => false,
        }
    }

    /// Pure call arg: literals and pure arith/cmp/logic — no Call / HostInvoke /
    /// IO / mutation / control side effects.
    ///
    /// Bare [`Expression::Identifier`] is intentionally **not** pure here: copying
    /// locals through temps on the shared operand/local stack (STORE extends
    /// `tell`) before effectful args corrupted frames in large functions
    /// (`parse_url` → Url field SEGV). Literals still reorder ahead of effects.
    fn call_arg_is_pure(expr: &Output<'_>) -> bool {
        match expr.1.as_ref() {
            Expression::NamedArg(_, v) | Expression::Group(v) | Expression::Expr(v) => {
                Self::call_arg_is_pure(v)
            }
            Expression::Integer(_)
            | Expression::Float(_)
            | Expression::String(_)
            | Expression::Bool(_)
            | Expression::Default(_)
            | Expression::TypeOf(_) => true,
            Expression::Identifier(_) => false,
            Expression::Negate(e)
            | Expression::Not(e)
            | Expression::LogicalNot(e)
            | Expression::Positive(e)
            | Expression::Cast(e, _) => Self::call_arg_is_pure(e),
            Expression::Add(a, b)
            | Expression::Sub(a, b)
            | Expression::Mul(a, b)
            | Expression::Div(a, b)
            | Expression::Mod(a, b)
            | Expression::Pow(a, b)
            | Expression::Shl(a, b)
            | Expression::Shr(a, b)
            | Expression::Xor(a, b)
            | Expression::And(a, b)
            | Expression::BitAnd(a, b)
            | Expression::Or(a, b)
            | Expression::BitOr(a, b)
            | Expression::Eq(a, b)
            | Expression::Neq(a, b)
            | Expression::Leq(a, b)
            | Expression::Geq(a, b)
            | Expression::Le(a, b)
            | Expression::Gt(a, b) => Self::call_arg_is_pure(a) && Self::call_arg_is_pure(b),
            Expression::Array(items) | Expression::Tuple(items) | Expression::List(items) => {
                items.iter().all(Self::call_arg_is_pure)
            }
            _ => false,
        }
    }

    /// Evaluate pure args into temps, then effectful args, then `LOAD` in order.
    fn emit_call_args_pure_first(
        &mut self,
        args: &[Output<'_>],
        bytecode: &mut Vec<Byte>,
        box_generic: bool,
    ) -> u32 {
        let mut temps = vec![0u32; args.len()];
        for (i, arg) in args.iter().enumerate() {
            if !Self::call_arg_is_pure(arg) {
                continue;
            }
            self.append_with_existential_pack(bytecode, arg);
            if box_generic {
                if let Some(arg_ty) = self.codegen_expr_ty(arg) {
                    Self::emit_box_if_needed(bytecode, &arg_ty);
                }
            }
            let tmp = self.alloc_temp_slot();
            bytecode.push_store_pop(tmp);
            temps[i] = tmp;
        }
        for (i, arg) in args.iter().enumerate() {
            if Self::call_arg_is_pure(arg) {
                continue;
            }
            // HostInvoke/format/match emit onto self.bytecode — StorePop must
            // follow immediately there, not in the Call local vec.
            if self.arg_emits_on_self_bytecode(arg) {
                self.stage_call_arg_to_temp(arg, box_generic, &mut temps[i]);
            } else {
                self.append_with_existential_pack(bytecode, arg);
                if box_generic {
                    if let Some(arg_ty) = self.codegen_expr_ty(arg) {
                        Self::emit_box_if_needed(bytecode, &arg_ty);
                    }
                }
                let tmp = self.alloc_temp_slot();
                bytecode.push_store_pop(tmp);
                temps[i] = tmp;
            }
        }
        for &tmp in &temps {
            bytecode.push_load(tmp);
        }
        args.len() as u32
    }

    /// Stage HostInvoke/`format`/`match` args on [`Self::bytecode`]; emit other
    /// args (identifiers, etc.) into the Call local vec in original order.
    fn emit_call_args_stage_self_bc(
        &mut self,
        args: &[Output<'_>],
        bytecode: &mut Vec<Byte>,
        box_generic: bool,
    ) -> u32 {
        let mut temps: Vec<Option<u32>> = vec![None; args.len()];
        for (i, arg) in args.iter().enumerate() {
            if !self.arg_emits_on_self_bytecode(arg) {
                continue;
            }
            let mut slot = 0u32;
            self.stage_call_arg_to_temp(arg, box_generic, &mut slot);
            temps[i] = Some(slot);
        }
        for (i, arg) in args.iter().enumerate() {
            if let Some(tmp) = temps[i] {
                bytecode.push_load(tmp);
            } else {
                self.append_with_existential_pack(bytecode, arg);
                if box_generic {
                    if let Some(arg_ty) = self.codegen_expr_ty(arg) {
                        Self::emit_box_if_needed(bytecode, &arg_ty);
                    }
                }
            }
        }
        args.len() as u32
    }

    /// Compile `arg` onto [`Self::bytecode`] and `StorePop` into a fresh temp.
    fn stage_call_arg_to_temp(&mut self, arg: &Output<'_>, box_generic: bool, tmp_out: &mut u32) {
        let mut staged = Vec::new();
        self.append_with_existential_pack(&mut staged, arg);
        self.bytecode.append(&mut staged);
        if box_generic {
            if let Some(arg_ty) = self.codegen_expr_ty(arg) {
                Self::emit_box_if_needed(&mut self.bytecode, &arg_ty);
            }
        }
        let tmp = self.alloc_temp_slot();
        self.bytecode.push_store_pop(tmp);
        *tmp_out = tmp;
    }

    fn next_emit_id(&mut self) -> Option<crate::typechecking::id::NodeId> {
        let id = self.checker.id_table().ids().get(self.emit_idx).copied();
        if id.is_some() {
            self.emit_idx += 1;
        }
        id
    }

    fn codegen_var_type_for(&self, name: &str) -> Option<Ty> {
        for frame in self.mono_codegen_var_types.iter().rev() {
            if let Some(ty) = frame.get(name) {
                return Some(ty.clone());
            }
        }
        self.checker.codegen_var_type(name).cloned()
    }

    /// Identifier type for codegen: mono arm overrides, then span cache, then
    /// the flat name map. Preferring span avoids later functions' `let x`
    /// overwriting earlier `x` entries used by `static_len_of` / arith.
    fn codegen_ident_ty(&self, node: &Output) -> Option<Ty> {
        use crate::typechecking::subst::apply_ty_prune;
        let Expression::Identifier(name) = node.1.as_ref() else {
            return None;
        };
        for frame in self.mono_codegen_var_types.iter().rev() {
            if let Some(ty) = frame.get(*name) {
                return Some(apply_ty_prune(self.checker.subst(), ty));
            }
        }
        if let Some(ty) = self
            .checker
            .lookup_for_codegen_span(node.0.start, node.0.end)
        {
            return Some(ty);
        }
        self.checker
            .codegen_var_type(name)
            .map(|t| apply_ty_prune(self.checker.subst(), t))
    }

    fn mono_ty_from_name(name: &str) -> Ty {
        if let Some(ty) = parse_mono_ty_name(name) {
            return ty;
        }
        match name {
            crate::typechecking::ty::INT => Ty::Con(crate::typechecking::ty::INT.into()),
            crate::typechecking::ty::FLOAT => Ty::Con(crate::typechecking::ty::FLOAT.into()),
            crate::typechecking::ty::STRING => Ty::Con(crate::typechecking::ty::STRING.into()),
            crate::typechecking::ty::BOOL => Ty::Con(crate::typechecking::ty::BOOL.into()),
            crate::typechecking::ty::UNIT => Ty::Con(crate::typechecking::ty::UNIT.into()),
            other => Ty::Con(other.to_string()),
        }
    }

    fn discard_statement_value(bytecode: &mut Vec<Byte>) {
        if matches!(
            bytecode.last().map(|b| b.bytecode()),
            Some(Instruction::DUPLICATE)
        ) {
            // If it was supposed to add `POP` but prev is `DUP`
            // then remove the DUP as well
            bytecode.pop();
        } else if matches!(
            bytecode.last().map(|b| b.bytecode()),
            Some(
                Instruction::STORE
                    | Instruction::StorePop
                    | Instruction::StoreStatic
                    | Instruction::POP
            )
        ) {
            // Slot/static stores consume the RHS; a prior POP already discarded.
            // SetField / StoreIndex push the value back — still need POP when
            // they are last (handled by the final branch).
        } else if !matches!(
            bytecode.last().map(|b| b.bytecode()),
            Some(Instruction::YieldCoro | Instruction::YieldFromCoro)
        ) {
            bytecode.push_pop();
        }
    }

    fn emit_loop_jump(
        &mut self,
        target: Option<BbLabel>,
        keyword: &str,
        range: std::ops::Range<usize>,
    ) {
        if let (Some(label), Some(bb)) = (target, self.loop_bbs.last_mut()) {
            bb.emit_jump_to(label, BbJumpKind::Unconditional, self.bytecode.il_mut());
        } else {
            let mut message = Message::error(
                ErrorCode::GenericTypeError,
                format!("{} outside of loop", keyword),
                range.clone(),
            );
            message.push(DiagLabel::new(
                format!("`{}` can only be used inside a loop", keyword),
                range,
            ));
            self.messages.push(message);
        }
    }

    fn emit_call_indirect(bytecode: &mut impl EmitBuf, target_offset: u32, arity: u32) {
        bytecode.push(Byte::new(Instruction::CodePtr).with_operand_u32(target_offset));
        bytecode.push(Byte::new(Instruction::CallIndirect).with_operand_u32(arity));
    }

    /// Lower an operator selected by HM inference to the uniform dictionary
    /// calling convention: two boxed values, the hidden trailing dictionary,
    /// then its method entry loaded from the dictionary tuple.
    fn emit_bound_operator_call(
        &mut self,
        bytecode: &mut Vec<Byte>,
        lhs: &Output,
        rhs: &Output,
        dict_index: usize,
        method_slot: usize,
    ) -> bool {
        let dict_name = format!("__dict{}", dict_index);
        let Some(dict_slot) = self.lookup_slot(&dict_name) else {
            return false;
        };
        bytecode.append(&mut self.do_compile(lhs));
        bytecode.append(&mut self.do_compile(rhs));
        bytecode.push_load(dict_slot);
        bytecode.push_load(dict_slot);
        bytecode.push_const(method_slot as i32);
        bytecode.push_index();
        bytecode.push(Byte::new(Instruction::CallIndirect).with_operand_u32(3));
        true
    }

    /// Emit element-wise / broadcast aggregate arithmetic when the typechecker
    /// recorded an [`AggregateArithInfo`] for this node (or we can recover the
    /// shape from mono/codegen var types).
    fn try_emit_aggregate_arith(
        &mut self,
        bytecode: &mut Vec<Byte>,
        self_id: Option<crate::typechecking::id::NodeId>,
        span_start: usize,
        span_end: usize,
        lhs: &Output,
        rhs: Option<&Output>,
        fallback_op: crate::typechecking::AggregateOp,
    ) -> bool {
        use crate::typechecking::{AggregateArithKind, AggregateOp, ScalarSide};

        let info = self_id
            .and_then(|id| self.checker.aggregate_arith_at(id))
            .or_else(|| self.checker.aggregate_arith_for_span(span_start, span_end))
            .cloned()
            .or_else(|| self.recover_aggregate_arith(lhs, rhs, fallback_op));
        let Some(info) = info else {
            return false;
        };

        if self.try_emit_packed_aggregate_arith(bytecode, &info, lhs, rhs) {
            return true;
        }

        let scalar_instr = |op: AggregateOp, is_float: bool| -> Instruction {
            match (op, is_float) {
                (AggregateOp::Add, false) => Instruction::ADD,
                (AggregateOp::Add, true) => Instruction::ADDF,
                (AggregateOp::Sub, false) => Instruction::SUB,
                (AggregateOp::Sub, true) => Instruction::SUBF,
                (AggregateOp::Mul, false) => Instruction::MUL,
                (AggregateOp::Mul, true) => Instruction::MULF,
                (AggregateOp::Div, false) => Instruction::DIV,
                (AggregateOp::Div, true) => Instruction::DIVF,
                (AggregateOp::Mod, false) => Instruction::MOD,
                (AggregateOp::Mod, true) => Instruction::MODF,
                (AggregateOp::Pow, false) => Instruction::Pow,
                (AggregateOp::Pow, true) => Instruction::PowF,
                // Neg is handled by `emit_neg_tos` (float uses MULF −1; no NEGF).
                (AggregateOp::Neg, _) => Instruction::NEG,
            }
        };

        match info.kind {
            AggregateArithKind::NegTuple {
                arity,
                elem_is_float,
            } => {
                let t0 = self.alloc_temp_slot();
                bytecode.append(&mut self.do_compile(lhs));
                bytecode.push_store_pop(t0);
                self.emit_zip_loop(
                    bytecode,
                    arity,
                    |c, bc, i| {
                        bc.push_load(t0);
                        bc.push_const(i as i32);
                        bc.push_index();
                        c.emit_neg_tos(bc, elem_is_float);
                    },
                    true,
                );
                true
            }
            AggregateArithKind::NegArray {
                length,
                elem_is_float,
            } => {
                let t0 = self.alloc_temp_slot();
                bytecode.append(&mut self.do_compile(lhs));
                bytecode.push_store_pop(t0);
                match length {
                    Some(n) => {
                        self.emit_zip_loop(
                            bytecode,
                            n,
                            |c, bc, i| {
                                bc.push_load(t0);
                                bc.push_const(i as i32);
                                bc.push_index();
                                c.emit_neg_tos(bc, elem_is_float);
                            },
                            false,
                        );
                    }
                    None => {
                        // Flush setup into CodeBuf so loop labels join the main IL.
                        self.bytecode.append(bytecode);
                        self.emit_dynamic_unary_array(t0, elem_is_float);
                    }
                }
                true
            }
            AggregateArithKind::ZipTuple {
                arity,
                elem_is_float,
            } => {
                let Some(rhs) = rhs else {
                    return false;
                };
                let t0 = self.alloc_temp_slot();
                let t1 = self.alloc_temp_slot();
                bytecode.append(&mut self.do_compile(lhs));
                bytecode.push_store_pop(t0);
                bytecode.append(&mut self.do_compile(rhs));
                bytecode.push_store_pop(t1);
                let op = info.op;
                self.emit_zip_loop(
                    bytecode,
                    arity,
                    |_c, bc, i| {
                        bc.push_load(t0);
                        bc.push_const(i as i32);
                        bc.push_index();
                        bc.push_load(t1);
                        bc.push_const(i as i32);
                        bc.push_index();
                        bc.push(Byte::new(scalar_instr(op, elem_is_float)));
                    },
                    true,
                );
                true
            }
            AggregateArithKind::ZipArray {
                length,
                elem_is_float,
            } => {
                let Some(rhs) = rhs else {
                    return false;
                };
                let t0 = self.alloc_temp_slot();
                let t1 = self.alloc_temp_slot();
                bytecode.append(&mut self.do_compile(lhs));
                bytecode.push_store_pop(t0);
                bytecode.append(&mut self.do_compile(rhs));
                bytecode.push_store_pop(t1);
                let op = info.op;
                self.emit_zip_loop(
                    bytecode,
                    length,
                    |_c, bc, i| {
                        bc.push_load(t0);
                        bc.push_const(i as i32);
                        bc.push_index();
                        bc.push_load(t1);
                        bc.push_const(i as i32);
                        bc.push_index();
                        bc.push(Byte::new(scalar_instr(op, elem_is_float)));
                    },
                    false,
                );
                true
            }
            AggregateArithKind::BroadcastTuple {
                arity,
                scalar_on,
                elem_is_float,
            } => {
                let Some(rhs) = rhs else {
                    return false;
                };
                let t_vec = self.alloc_temp_slot();
                let t_sc = self.alloc_temp_slot();
                match scalar_on {
                    ScalarSide::Right => {
                        bytecode.append(&mut self.do_compile(lhs));
                        bytecode.push_store_pop(t_vec);
                        bytecode.append(&mut self.do_compile(rhs));
                        bytecode.push_store_pop(t_sc);
                    }
                    ScalarSide::Left => {
                        bytecode.append(&mut self.do_compile(lhs));
                        bytecode.push_store_pop(t_sc);
                        bytecode.append(&mut self.do_compile(rhs));
                        bytecode.push_store_pop(t_vec);
                    }
                }
                let op = info.op;
                self.emit_zip_loop(
                    bytecode,
                    arity,
                    |_c, bc, i| {
                        match scalar_on {
                            ScalarSide::Right => {
                                bc.push_load(t_vec);
                                bc.push_const(i as i32);
                                bc.push_index();
                                bc.push_load(t_sc);
                            }
                            ScalarSide::Left => {
                                bc.push_load(t_sc);
                                bc.push_load(t_vec);
                                bc.push_const(i as i32);
                                bc.push_index();
                            }
                        }
                        bc.push(Byte::new(scalar_instr(op, elem_is_float)));
                    },
                    true,
                );
                true
            }
            AggregateArithKind::BroadcastArray {
                length,
                scalar_on,
                elem_is_float,
            } => {
                let Some(rhs) = rhs else {
                    return false;
                };
                let t_vec = self.alloc_temp_slot();
                let t_sc = self.alloc_temp_slot();
                match scalar_on {
                    ScalarSide::Right => {
                        bytecode.append(&mut self.do_compile(lhs));
                        bytecode.push_store_pop(t_vec);
                        bytecode.append(&mut self.do_compile(rhs));
                        bytecode.push_store_pop(t_sc);
                    }
                    ScalarSide::Left => {
                        bytecode.append(&mut self.do_compile(lhs));
                        bytecode.push_store_pop(t_sc);
                        bytecode.append(&mut self.do_compile(rhs));
                        bytecode.push_store_pop(t_vec);
                    }
                }
                let op = info.op;
                match length {
                    Some(n) => {
                        self.emit_zip_loop(
                            bytecode,
                            n,
                            |_c, bc, i| {
                                match scalar_on {
                                    ScalarSide::Right => {
                                        bc.push_load(t_vec);
                                        bc.push_const(i as i32);
                                        bc.push_index();
                                        bc.push_load(t_sc);
                                    }
                                    ScalarSide::Left => {
                                        bc.push_load(t_sc);
                                        bc.push_load(t_vec);
                                        bc.push_const(i as i32);
                                        bc.push_index();
                                    }
                                }
                                bc.push(Byte::new(scalar_instr(op, elem_is_float)));
                            },
                            false,
                        );
                    }
                    None => {
                        // Flush setup into CodeBuf so loop labels join the main IL.
                        self.bytecode.append(bytecode);
                        self.emit_dynamic_broadcast_array(
                            t_vec,
                            t_sc,
                            scalar_on,
                            op,
                            elem_is_float,
                        );
                    }
                }
                true
            }
        }
    }

    /// HostInvoke packed path for 1-D aggregate zip / broadcast / neg.
    ///
    /// Used when static length ≥ 8 so SIMD kernels amortize HostInvoke cost;
    /// smaller shapes keep the existing scalar unroll.
    fn try_emit_packed_aggregate_arith(
        &mut self,
        bytecode: &mut Vec<Byte>,
        info: &crate::typechecking::AggregateArithInfo,
        lhs: &Output,
        rhs: Option<&Output>,
    ) -> bool {
        use crate::typechecking::{AggregateArithKind, AggregateOp, ScalarSide};

        const MIN_PACKED: usize = 8;

        let op_code: u32 = match info.op {
            AggregateOp::Add => 0,
            AggregateOp::Sub => 1,
            AggregateOp::Mul => 2,
            AggregateOp::Div => 3,
            AggregateOp::Neg => 4,
            AggregateOp::Mod | AggregateOp::Pow => return false,
        };

        let (len, is_tuple, elem_is_float, broadcast, scalar_left) = match &info.kind {
            AggregateArithKind::ZipTuple {
                arity,
                elem_is_float,
            } => (*arity, true, *elem_is_float, false, false),
            AggregateArithKind::ZipArray {
                length,
                elem_is_float,
            } => (*length, false, *elem_is_float, false, false),
            AggregateArithKind::BroadcastTuple {
                arity,
                scalar_on,
                elem_is_float,
            } => (
                *arity,
                true,
                *elem_is_float,
                true,
                matches!(scalar_on, ScalarSide::Left),
            ),
            AggregateArithKind::BroadcastArray {
                length: Some(n),
                scalar_on,
                elem_is_float,
            } => (
                *n,
                false,
                *elem_is_float,
                true,
                matches!(scalar_on, ScalarSide::Left),
            ),
            AggregateArithKind::NegTuple {
                arity,
                elem_is_float,
            } => (*arity, true, *elem_is_float, false, false),
            AggregateArithKind::NegArray {
                length: Some(n),
                elem_is_float,
            } => (*n, false, *elem_is_float, false, false),
            AggregateArithKind::BroadcastArray { length: None, .. }
            | AggregateArithKind::NegArray { length: None, .. } => return false,
        };

        if len < MIN_PACKED || len > u16::MAX as usize {
            return false;
        }

        let is_neg = matches!(info.op, AggregateOp::Neg);
        if !is_neg && rhs.is_none() {
            return false;
        }

        let Some(native_id) = self.native_id(machine::PACKED_VEC_ARITH) else {
            return false;
        };

        let mut meta = (len as u32) & 0xFFFF;
        meta |= op_code << 16;
        if elem_is_float {
            meta |= 1 << 24;
        }
        if is_tuple {
            meta |= 1 << 25;
        }
        if broadcast {
            meta |= 1 << 26;
        }
        if scalar_left {
            meta |= 1 << 27;
        }

        let depth_on_entry = self.expr_depth;
        bytecode.push(Byte::new(Instruction::CONST).with_operand_u32(native_id as u32));
        self.expr_depth = depth_on_entry + 1;
        bytecode.append(&mut self.do_compile(lhs));
        self.expr_depth += 1;
        let arity = if is_neg {
            2 // vec + meta
        } else {
            bytecode.append(&mut self.do_compile(rhs.unwrap()));
            self.expr_depth += 1;
            3 // lhs + rhs + meta
        };
        bytecode.push(Byte::new(Instruction::CONST).with_operand_u32(meta));
        self.expr_depth += 1;
        bytecode.push_make_tuple(arity as u32);
        bytecode.push_host_invoke(arity as u32);
        self.expr_depth = depth_on_entry;
        true
    }

    /// Negate TOS: int via `NEG`; float via `MULF` by −1 (no `NEGF` opcode).
    fn emit_neg_tos(&mut self, bytecode: &mut Vec<Byte>, is_float: bool) {
        if is_float {
            let bits = Value::from(-1.0f64).raw() as u64;
            let idx = self.intern_constant(bits);
            bytecode.push_const_pool(idx);
            bytecode.push(Byte::new(Instruction::MULF));
        } else {
            bytecode.push(Byte::new(Instruction::NEG));
        }
    }

    /// Always unrolls static arities at compile time in v1 (including N > 4).
    fn emit_zip_loop<F>(
        &mut self,
        bytecode: &mut Vec<Byte>,
        n: usize,
        mut emit_elem: F,
        as_tuple: bool,
    ) where
        F: FnMut(&mut Self, &mut Vec<Byte>, usize),
    {
        for i in 0..n {
            emit_elem(self, bytecode, i);
        }
        if as_tuple {
            bytecode.push_make_tuple(n as u32);
        } else {
            bytecode.push_make_array(n as u32);
        }
    }

    fn emit_dynamic_unary_array(&mut self, src: u32, elem_is_float: bool) {
        let len_slot = self.alloc_temp_slot();
        let idx = self.alloc_temp_slot();
        let out = self.alloc_temp_slot();
        self.bytecode.push_load(src);
        self.bytecode.push(Byte::new(Instruction::ArrayLen));
        self.bytecode.push_store_pop(len_slot);
        self.bytecode.push_make_array(0);
        self.bytecode.push_store_pop(out);
        self.bytecode.push_const(0);
        self.bytecode.push_store_pop(idx);

        let mut bb = BlockBuilder::new();
        let loop_top = bb.fresh_label(self.bytecode.il_mut());
        let end = bb.fresh_label(self.bytecode.il_mut());
        bb.bind_label(loop_top, self.bytecode.il_mut());

        self.bytecode.push_load(idx);
        self.bytecode.push_load(len_slot);
        self.bytecode.push(Byte::new(Instruction::LE));
        bb.emit_jump_to(end, BbJumpKind::JumpIfFalse, self.bytecode.il_mut());

        self.bytecode.push_load(out);
        self.bytecode.push_load(src);
        self.bytecode.push_load(idx);
        self.bytecode.push_index();
        {
            let mut neg_bc = Vec::new();
            self.emit_neg_tos(&mut neg_bc, elem_is_float);
            self.bytecode.extend(neg_bc);
        }
        self.bytecode.push(Byte::new(Instruction::ArrayPush));
        self.bytecode.push_store_pop(out);
        self.bytecode.push_load(idx);
        self.bytecode.push_const(1);
        self.bytecode.push(Byte::new(Instruction::ADD));
        self.bytecode.push_store_pop(idx);

        bb.emit_jump_to(loop_top, BbJumpKind::Unconditional, self.bytecode.il_mut());
        bb.bind_label(end, self.bytecode.il_mut());
        self.bytecode.push_load(out);
        bb.finalize()
            .expect("BlockBuilder::finalize: dynamic unary array labels bound");
    }

    fn emit_dynamic_broadcast_array(
        &mut self,
        t_vec: u32,
        t_sc: u32,
        scalar_on: crate::typechecking::ScalarSide,
        op: crate::typechecking::AggregateOp,
        elem_is_float: bool,
    ) {
        use crate::typechecking::{AggregateOp, ScalarSide};
        let scalar_instr = match (op, elem_is_float) {
            (AggregateOp::Add, false) => Instruction::ADD,
            (AggregateOp::Add, true) => Instruction::ADDF,
            (AggregateOp::Sub, false) => Instruction::SUB,
            (AggregateOp::Sub, true) => Instruction::SUBF,
            (AggregateOp::Mul, false) => Instruction::MUL,
            (AggregateOp::Mul, true) => Instruction::MULF,
            (AggregateOp::Div, false) => Instruction::DIV,
            (AggregateOp::Div, true) => Instruction::DIVF,
            (AggregateOp::Mod, false) => Instruction::MOD,
            (AggregateOp::Mod, true) => Instruction::MODF,
            (AggregateOp::Pow, false) => Instruction::Pow,
            (AggregateOp::Pow, true) => Instruction::PowF,
            (AggregateOp::Neg, _) => Instruction::NEG, // unused
        };
        let len_slot = self.alloc_temp_slot();
        let idx = self.alloc_temp_slot();
        let out = self.alloc_temp_slot();
        self.bytecode.push_load(t_vec);
        self.bytecode.push(Byte::new(Instruction::ArrayLen));
        self.bytecode.push_store_pop(len_slot);
        self.bytecode.push_make_array(0);
        self.bytecode.push_store_pop(out);
        self.bytecode.push_const(0);
        self.bytecode.push_store_pop(idx);

        let mut bb = BlockBuilder::new();
        let loop_top = bb.fresh_label(self.bytecode.il_mut());
        let end = bb.fresh_label(self.bytecode.il_mut());
        bb.bind_label(loop_top, self.bytecode.il_mut());

        self.bytecode.push_load(idx);
        self.bytecode.push_load(len_slot);
        self.bytecode.push(Byte::new(Instruction::LE));
        bb.emit_jump_to(end, BbJumpKind::JumpIfFalse, self.bytecode.il_mut());

        self.bytecode.push_load(out);
        match scalar_on {
            ScalarSide::Right => {
                self.bytecode.push_load(t_vec);
                self.bytecode.push_load(idx);
                self.bytecode.push_index();
                self.bytecode.push_load(t_sc);
            }
            ScalarSide::Left => {
                self.bytecode.push_load(t_sc);
                self.bytecode.push_load(t_vec);
                self.bytecode.push_load(idx);
                self.bytecode.push_index();
            }
        }
        self.bytecode.push(Byte::new(scalar_instr));
        self.bytecode.push(Byte::new(Instruction::ArrayPush));
        self.bytecode.push_store_pop(out);
        self.bytecode.push_load(idx);
        self.bytecode.push_const(1);
        self.bytecode.push(Byte::new(Instruction::ADD));
        self.bytecode.push_store_pop(idx);

        bb.emit_jump_to(loop_top, BbJumpKind::Unconditional, self.bytecode.il_mut());
        bb.bind_label(end, self.bytecode.il_mut());
        self.bytecode.push_load(out);
        bb.finalize()
            .expect("BlockBuilder::finalize: dynamic broadcast array labels bound");
    }

    /// Recover aggregate arith info from mono/codegen var types when the
    /// side-table miss (specialized clones).
    ///
    /// Requires the same homogeneous-element rule as the typechecker: mixed
    /// element types (e.g. `(int, float)`) must not recover as zip candidates.
    fn recover_aggregate_arith(
        &self,
        lhs: &Output,
        rhs: Option<&Output>,
        op: crate::typechecking::AggregateOp,
    ) -> Option<crate::typechecking::AggregateArithInfo> {
        use crate::typechecking::aggregate_arith::{elem_is_float, homogeneous_aggregate_elem};
        use crate::typechecking::subst::apply_ty_prune;
        use crate::typechecking::ty::{ArrayLength, Ty};
        use crate::typechecking::{
            AggregateArithInfo, AggregateArithKind, AggregateOp, ScalarSide,
        };

        let lty = self.expr_codegen_ty(lhs)?;
        let lty = apply_ty_prune(self.checker.subst(), &lty);
        match (op, rhs) {
            (AggregateOp::Neg, None) => {
                let elem = homogeneous_aggregate_elem(&lty)?;
                let float = elem_is_float(&elem);
                match lty {
                    Ty::Tuple(elems) => Some(AggregateArithInfo {
                        kind: AggregateArithKind::NegTuple {
                            arity: elems.len(),
                            elem_is_float: float,
                        },
                        op,
                    }),
                    Ty::Array { length, .. } => Some(AggregateArithInfo {
                        kind: AggregateArithKind::NegArray {
                            length: match length {
                                ArrayLength::Static(n) => Some(n),
                                ArrayLength::Dynamic => None,
                            },
                            elem_is_float: float,
                        },
                        op,
                    }),
                    _ => None,
                }
            }
            (_, Some(rhs)) => {
                let rty = self.expr_codegen_ty(rhs)?;
                let rty = apply_ty_prune(self.checker.subst(), &rty);
                use crate::typechecking::aggregate_arith::is_numeric_elem;
                match (&lty, &rty) {
                    (Ty::Tuple(a), Ty::Tuple(b)) if a.len() == b.len() && !a.is_empty() => {
                        let le = homogeneous_aggregate_elem(&lty)?;
                        let re = homogeneous_aggregate_elem(&rty)?;
                        if le != re {
                            return None;
                        }
                        Some(AggregateArithInfo {
                            kind: AggregateArithKind::ZipTuple {
                                arity: a.len(),
                                elem_is_float: elem_is_float(&le),
                            },
                            op,
                        })
                    }
                    (
                        Ty::Array {
                            element,
                            length: ArrayLength::Static(n),
                        },
                        Ty::Array {
                            length: ArrayLength::Static(m),
                            ..
                        },
                    ) if n == m => Some(AggregateArithInfo {
                        kind: AggregateArithKind::ZipArray {
                            length: *n,
                            elem_is_float: elem_is_float(element),
                        },
                        op,
                    }),
                    (Ty::Tuple(a), r) if !a.is_empty() && is_numeric_elem(r) => {
                        let elem = homogeneous_aggregate_elem(&lty)?;
                        Some(AggregateArithInfo {
                            kind: AggregateArithKind::BroadcastTuple {
                                arity: a.len(),
                                scalar_on: ScalarSide::Right,
                                elem_is_float: elem_is_float(&elem),
                            },
                            op,
                        })
                    }
                    (l, Ty::Tuple(b)) if !b.is_empty() && is_numeric_elem(l) => {
                        let elem = homogeneous_aggregate_elem(&rty)?;
                        Some(AggregateArithInfo {
                            kind: AggregateArithKind::BroadcastTuple {
                                arity: b.len(),
                                scalar_on: ScalarSide::Left,
                                elem_is_float: elem_is_float(&elem),
                            },
                            op,
                        })
                    }
                    (Ty::Array { element, length }, r) if is_numeric_elem(r) => {
                        Some(AggregateArithInfo {
                            kind: AggregateArithKind::BroadcastArray {
                                length: match length {
                                    ArrayLength::Static(n) => Some(*n),
                                    ArrayLength::Dynamic => None,
                                },
                                scalar_on: ScalarSide::Right,
                                elem_is_float: elem_is_float(element),
                            },
                            op,
                        })
                    }
                    (l, Ty::Array { element, length }) if is_numeric_elem(l) => {
                        Some(AggregateArithInfo {
                            kind: AggregateArithKind::BroadcastArray {
                                length: match length {
                                    ArrayLength::Static(n) => Some(*n),
                                    ArrayLength::Dynamic => None,
                                },
                                scalar_on: ScalarSide::Left,
                                elem_is_float: elem_is_float(element),
                            },
                            op,
                        })
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn expr_codegen_ty(&self, expr: &Output) -> Option<Ty> {
        match expr.1.as_ref() {
            Expression::Identifier(_) => self.codegen_ident_ty(expr),
            Expression::Integer(_) => Some(Ty::Con("int".into())),
            Expression::Float(_) => Some(Ty::Con("float".into())),
            Expression::Tuple(items) => {
                let mut tys = Vec::with_capacity(items.len());
                for it in items {
                    tys.push(self.expr_codegen_ty(it)?);
                }
                Some(Ty::Tuple(tys))
            }
            Expression::Array(items) => {
                if items.is_empty() {
                    return None;
                }
                let elem = self.expr_codegen_ty(&items[0])?;
                Some(crate::typechecking::ty::array_fixed(elem, items.len()))
            }
            Expression::Group(inner) | Expression::Expr(inner) | Expression::Statement(inner) => {
                self.expr_codegen_ty(inner)
            }
            _ => None,
        }
    }

    /// Emit a direct call to a concrete `Eq` / `Ord` instance method when
    /// the operands are a user type with a registered instance.
    ///
    /// Primitive `int`/`float`/`string`/`bool` keep the hardwired opcode
    /// path (caller falls through when this returns `false`).
    fn emit_concrete_operator_call(
        &mut self,
        bytecode: &mut Vec<Byte>,
        lhs: &Output,
        rhs: &Output,
        class: &str,
        method: &str,
    ) -> bool {
        let arg_ty = self
            .codegen_expr_ty(lhs)
            .or_else(|| self.codegen_expr_ty(rhs));
        let Some(ty) = arg_ty else {
            return false;
        };
        let resolved = crate::typechecking::subst::apply_ty_prune(self.checker.subst(), &ty);
        let lookup_ty = Self::show_lookup_ty_for_instance(&resolved);
        // Only dispatch for nominal *user* enums/classes. Open `Ty::Var`s
        // unify against the first builtin `Eq`/`Ord` instance under
        // `find_instance_relaxed`, which would incorrectly replace
        // hardwired `EQ`/`LT` opcodes (and box immediates into garbage).
        let nominal = match &lookup_ty {
            Ty::Con(name) => Some(name.as_str()),
            Ty::App(head, _) => match head.as_ref() {
                Ty::Con(name) => Some(name.as_str()),
                _ => None,
            },
            _ => None,
        };
        let Some(name) = nominal else {
            return false;
        };
        if matches!(
            name,
            "int" | "float" | "string" | "bool" | "unit" | "Option" | "Result"
        ) {
            return false;
        }
        if self.checker.enum_variants(name).is_none() && !self.checker.is_class(name) {
            return false;
        }
        let Some(instance) = self
            .checker
            .generics()
            .find_instance_relaxed(class, std::slice::from_ref(&lookup_ty))
            .cloned()
        else {
            return false;
        };
        let Some(fqn) = instance.method_fqns.get(method).cloned() else {
            return false;
        };
        let Some(&offset) = self.functions.get(&fqn) else {
            return false;
        };
        // Instance methods use the dictionary ABI: value args are boxed at
        // the call site and unboxed in the method prologue (see
        // `instance_method_unbox_tys` + `compile_function_output_with_name`).
        // Without boxing here, `UnboxValue` on a raw enum/class pointer
        // yields `Value::default()` and comparisons always fail.
        //
        // Stash each boxed operand in a temp before compiling the other
        // side: `new Class(...)` (Instantiate) uses `StorePop` into temps
        // and would otherwise steal a pending boxed arg off the operand
        // stack mid-call.
        bytecode.append(&mut self.do_compile(lhs));
        Self::emit_box_if_needed(bytecode, &lookup_ty);
        let lhs_slot = self.alloc_temp_slot();
        bytecode.push_store_pop(lhs_slot);
        bytecode.append(&mut self.do_compile(rhs));
        Self::emit_box_if_needed(bytecode, &lookup_ty);
        let rhs_slot = self.alloc_temp_slot();
        bytecode.push_store_pop(rhs_slot);
        bytecode.push_load(lhs_slot);
        bytecode.push_load(rhs_slot);
        Self::emit_call_indirect(bytecode, offset as u32, 2);
        true
    }

    /// Emit a string literal as a table-indexed `STRING` byte into `self.bytecode`.
    /// Applies the same escape processing as `Expression::String` codegen.
    fn emit_string_literal(&mut self, s: &str) {
        let escaped = unescape_coil_string(s);
        let idx = self.intern_string(&escaped);
        self.bytecode.push_string(idx);
    }

    /// Rewrite `%v` → `%s` in a format literal (leave `%%` alone).
    fn rewrite_format_v_to_s(fmt: &str) -> String {
        let mut out = String::with_capacity(fmt.len());
        let mut chars = fmt.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '%' {
                match chars.next() {
                    Some('%') => {
                        out.push('%');
                        out.push('%');
                    }
                    Some('v') => {
                        out.push('%');
                        out.push('s');
                    }
                    Some(other) => {
                        out.push('%');
                        out.push(other);
                    }
                    None => out.push('%'),
                }
            } else {
                out.push(ch);
            }
        }
        out
    }

    /// Consuming format specifiers in source order (`%%` skipped).
    fn format_consuming_specs(fmt: &str) -> Vec<char> {
        let mut specs = Vec::new();
        let mut chars = fmt.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '%' {
                match chars.next() {
                    Some('%') => {}
                    Some(spec) => specs.push(spec),
                    None => break,
                }
            }
        }
        specs
    }

    /// Emit `string::format` body: format string, then args (with `%v`
    /// lowered through `Show`), then `FORMAT`.
    fn emit_format_expression(&mut self, format: &Output, params: Option<&Vec<Output>>) {
        let fmt_lit = match format.1.as_ref() {
            Expression::String(s) => Some(s.to_string()),
            _ => None,
        };

        if let (Some(fmt), Some(params)) = (fmt_lit.as_deref(), params) {
            let rewritten = Self::rewrite_format_v_to_s(fmt);
            let specs = Self::format_consuming_specs(fmt);
            // Evaluate args into temps first. Emitting the format string
            // before args leaves it under CALL/STORE frames (self-unroll /
            // nested calls) and corrupts the shared stack.
            let mut arg_slots = Vec::with_capacity(params.len());
            let mut emitted = 0usize;
            for (param, spec) in params.iter().zip(specs.iter()) {
                if *spec == 'v' {
                    self.emit_show_for_format_arg(param);
                } else {
                    let bc = self.do_compile(param);
                    self.bytecode.extend(bc);
                }
                let slot = self.alloc_temp_slot();
                self.bytecode.push_store_pop(slot);
                arg_slots.push(slot);
                emitted += 1;
            }
            for param in params.iter().skip(emitted) {
                let bc = self.do_compile(param);
                self.bytecode.extend(bc);
                let slot = self.alloc_temp_slot();
                self.bytecode.push_store_pop(slot);
                arg_slots.push(slot);
            }
            self.emit_string_literal(&rewritten);
            for slot in arg_slots {
                self.bytecode.push_load(slot);
            }
            self.bytecode
                .push(Byte::new(Instruction::FORMAT).with_operand_u32(params.len() as u32));
        } else {
            let format_bc = self.do_compile(format);
            self.bytecode.extend(format_bc);
            let mut params_len = 0u32;
            if let Some(params) = params {
                params_len = params.len() as u32;
                for param in params {
                    let bc = self.do_compile(param);
                    self.bytecode.extend(bc);
                }
            }
            self.bytecode
                .push(Byte::new(Instruction::FORMAT).with_operand_u32(params_len));
        }
    }

    fn string_builtin_for_call(&self, ident: &str) -> Option<crate::typechecking::StringBuiltin> {
        self.checker.string_fn_in_scope(ident).or_else(|| {
            ident
                .strip_prefix("string::")
                .and_then(crate::typechecking::StringBuiltin::from_name)
        })
    }

    fn show_format_arg_ty(&self, arg: &Output) -> Option<Ty> {
        let span = arg.0.into_range();
        let span_ty = self
            .checker
            .lookup_for_codegen_span(span.start, span.end)
            .map(|t| crate::typechecking::subst::apply_ty_prune(self.checker.subst(), &t));
        match span_ty {
            Some(Ty::Var(_)) | None => self.codegen_expr_ty(arg),
            Some(other) => Some(other),
        }
    }

    fn emit_ffi_declare(&mut self, span: SimpleSpan, args: &[Output]) {
        if args.len() != 4 && args.len() != 5 {
            let mut m = Message::error(
                ErrorCode::DeclareArity,
                "declare requires arguments as a tuple in position 3 (use (T1, T2, ...) syntax)"
                    .to_string(),
                span.into_range(),
            );
            m.push(DiagLabel::new(
                format!(
                    "expected 4 or 5 arguments (lib, name, args_tuple, ret_type[, variadic]); got {}",
                    args.len()
                ),
                span.into_range(),
            ));
            self.messages.push(m);
            self.bytecode
                .push(Byte::new(Instruction::DeclareFFI).with_operand_u32(0));
            return;
        }
        let lib = &args[0];
        let name = &args[1];
        let args_tuple = &args[2];
        let ret_type = &args[3];
        let variadic = if args.len() == 5 {
            match args[4].1.as_ref() {
                Expression::Bool(b) => *b,
                _ => {
                    let mut m = Message::error(
                        ErrorCode::DeclareArity,
                        "declare(...) 5th argument (variadic) must be a bool literal".to_string(),
                        args[4].0.into_range(),
                    );
                    m.push(DiagLabel::new(
                        "use `true` or `false`".to_string(),
                        args[4].0.into_range(),
                    ));
                    self.messages.push(m);
                    false
                }
            }
        } else {
            false
        };

        let tuple_elements: Vec<_> = match args_tuple.1.as_ref() {
            Expression::Tuple(items) => items.to_vec(),
            _ => {
                let mut m = Message::error(
                    ErrorCode::DeclareArity,
                    "declare(...) arguments tuple must be (T1, T2, ...) syntax".to_string(),
                    args_tuple.0.into_range(),
                );
                m.push(DiagLabel::new(
                    "wrap the arg types in parentheses — (Int, Float) after `use ffi::types::*;`"
                        .to_string(),
                    args_tuple.0.into_range(),
                ));
                self.messages.push(m);
                Vec::new()
            }
        };

        let lib_bc = self.do_compile(lib);
        self.bytecode.extend(lib_bc);
        let name_bc = self.do_compile(name);
        self.bytecode.extend(name_bc);

        for elem in &tuple_elements {
            if let Some((tag, aux)) = ffi_type_tag_from_output(&self.checker, elem) {
                emit_ffi_type_const(&mut self.bytecode, tag, aux);
            } else {
                let bc = self.do_compile(elem);
                self.bytecode.extend(bc);
            }
        }
        let arity = tuple_elements.len() as u32;
        self.bytecode.push_make_tuple(arity);

        if let Some((tag, aux)) = ffi_type_tag_from_output(&self.checker, ret_type) {
            emit_ffi_type_const(&mut self.bytecode, tag, aux);
        } else {
            let ret_bc = self.do_compile(ret_type);
            self.bytecode.extend(ret_bc);
        }

        let mut operand = arity & 0xFFFF;
        if variadic {
            operand |= 1 << 16;
        }
        self.bytecode
            .push(Byte::new(Instruction::DeclareFFI).with_operand_u32(operand));
    }

    fn emit_ffi_invoke(&mut self, span: SimpleSpan, args: &[Output]) {
        if args.len() != 3 {
            let mut m = Message::error(
                ErrorCode::InvokeArity,
                "invoke requires arguments as a tuple in position 3 (use (a, b, ...) syntax)"
                    .to_string(),
                span.into_range(),
            );
            m.push(DiagLabel::new(
                format!(
                    "expected 3 arguments (lib, fn_id, args_tuple); got {}",
                    args.len()
                ),
                span.into_range(),
            ));
            self.messages.push(m);
            self.bytecode
                .push(Byte::new(Instruction::FfiInvoke).with_operand_u32(0));
            return;
        }
        let lib = &args[0];
        let fn_id = &args[1];
        let args_tuple = &args[2];

        let variadic = match fn_id.1.as_ref() {
            Expression::Identifier(name) => self.checker.is_ffi_declare_variadic(name),
            _ => false,
        };

        let tuple_elements: Vec<_> = match args_tuple.1.as_ref() {
            Expression::Tuple(items) => items.to_vec(),
            _ => {
                let mut m = Message::error(
                    ErrorCode::InvokeArity,
                    "invoke(...) arguments must be a tuple in position 3".to_string(),
                    args_tuple.0.into_range(),
                );
                m.push(DiagLabel::new(
                    "wrap the arg values in parentheses — (40, 2)".to_string(),
                    args_tuple.0.into_range(),
                ));
                self.messages.push(m);
                Vec::new()
            }
        };

        let lib_bc = self.do_compile(lib);
        self.bytecode.extend(lib_bc);
        let fn_bc = self.do_compile(fn_id);
        self.bytecode.extend(fn_bc);

        for elem in &tuple_elements {
            if let Expression::Identifier(name) = elem.1.as_ref()
                && let Some(&offset) = self.functions.get(*name)
            {
                self.bytecode
                    .push(Byte::new(Instruction::CodePtr).with_operand_u32(offset as u32));
                continue;
            }
            let bc = self.do_compile(elem);
            self.bytecode.extend(bc);
        }
        let arity = tuple_elements.len() as u32;
        self.bytecode.push_make_tuple(arity);

        let mut operand = arity & 0xFFFF;
        if variadic {
            let args: Vec<_> = tuple_elements.iter().collect();
            if let Some(tags) = resolve_variadic_ffi_tags(
                &self.checker,
                (span.start, span.end),
                &args,
                &mut self.messages,
            ) {
                for &(tag, aux) in &tags {
                    emit_ffi_type_const(&mut self.bytecode, tag, aux);
                }
                self.bytecode.push_make_tuple(tags.len() as u32);
                operand |= 1 << 16;
            }
        }

        self.bytecode
            .push(Byte::new(Instruction::FfiInvoke).with_operand_u32(operand));
    }

    /// Unwrap a `Result` on top of the stack: on `Ok`, leave the payload;
    /// on `Err(ffi::Error)`, panic with the error's `message` string.
    /// Used by `extern` lowering so failed `dload`/`declare`/`invoke`
    /// never reach unsafe FFI calls.
    fn emit_result_unwrap_or_panic(&mut self) {
        // Result::Ok = tag 0 (arity 1), Result::Err = tag 1 (arity 1).
        let mut bb = BlockBuilder::new();
        let success = bb.fresh_label(self.bytecode.il_mut());
        bb.emit_jump_to(
            success,
            BbJumpKind::JumpIfMatch { tag: 0, arity: 1 },
            self.bytecode.il_mut(),
        );
        // Miss: Err still on stack — unpack `ffi::Error`, then LoadField
        // message (field index 1: kind=0, message=1) and Panic.
        self.bytecode
            .push(Byte::new(Instruction::Unpack).with_operand_u32(1));
        self.bytecode.push_load_field(1);
        self.bytecode.push(Byte::new(Instruction::Panic));
        bb.bind_label(success, self.bytecode.il_mut());
        bb.finalize()
            .expect("BlockBuilder::finalize: FFI Result unwrap success label bound");
    }

    /// True when `expr` is a `match` (after peeling `Expr` wrappers).
    fn rhs_is_match_expr(expr: &Output<'_>) -> bool {
        matches!(
            unwrap_expr_output(expr).1.as_ref(),
            Expression::Match { .. }
        )
    }

    /// True when `body` is just the sole pattern binding (e.g. `Ok(x) => x`).
    fn match_arm_body_is_identity_binding<'a>(
        pattern: &parser::ast::Pattern<'a>,
        body: &Output<'a>,
    ) -> bool {
        use parser::ast::{Expression, Pattern, PatternPayload};
        let body_name = match unwrap_expr_output(body).1.as_ref() {
            Expression::Identifier(n) | Expression::Variable(n, _) => *n,
            _ => return false,
        };
        fn sole_binding<'a>(pattern: &Pattern<'a>) -> Option<&'a str> {
            match pattern {
                Pattern::Binding { name } => Some(name),
                Pattern::Constructor { payload, .. } => match payload {
                    PatternPayload::Tuple(items) if items.len() == 1 => sole_binding(&items[0]),
                    _ => None,
                },
                _ => None,
            }
        }
        sole_binding(pattern) == Some(body_name)
    }

    /// Lower one `%v` argument to a string via the `Show` dictionary /
    /// concrete instance method, leaving an `ObjString` on the stack.
    fn emit_show_for_format_arg(&mut self, arg: &Output) {
        let span = arg.0.into_range();
        if let Some((dict_index, method_slot)) = self
            .checker
            .bound_display_call_for_span(span.start, span.end)
            .map(|h| (h.dict_index, h.method_slot))
        {
            let dict_name = format!("__dict{}", dict_index);
            if let Some(dict_slot) = self.lookup_slot(&dict_name) {
                let mut arg_bc = self.do_compile(arg);
                self.bytecode.append(&mut arg_bc);
                self.bytecode.push_load(dict_slot);
                self.bytecode.push_load(dict_slot);
                self.bytecode.push_const(method_slot as i32);
                self.bytecode.push_index();
                self.bytecode
                    .push(Byte::new(Instruction::CallIndirect).with_operand_u32(2));
                return;
            }
        }

        // Concrete Show instance at the call site.
        // Prefer a fully resolved type: span cache from a shared generic body
        // may still be an open `Ty::Var` even when mono/codegen side-tables
        // know the ground type (or when the arg is a literal / construct).
        let arg_ty = self.show_format_arg_ty(arg);

        if let Some(ty) = arg_ty.as_ref() {
            let resolved = crate::typechecking::subst::apply_ty_prune(self.checker.subst(), ty);
            if matches!(resolved, Ty::Tuple(_) | Ty::Record { .. }) {
                let mut arg_bc = self.do_compile(arg);
                self.bytecode.append(&mut arg_bc);
                self.emit_show_for_stack_value(&resolved);
                return;
            }

            // Instance heads use `Ty::Con("Point")`; construct sites often
            // produce `Constructor` / `Sum` — peel to the enum name.
            let lookup_ty = Self::show_lookup_ty_for_instance(&resolved);
            if let Some(instance) = self
                .checker
                .generics()
                .find_instance("Show", std::slice::from_ref(&lookup_ty))
                .cloned()
                && let Some(fqn) = instance.method_fqns.get("show").cloned()
                && let Some(&offset) = self.functions.get(&fqn)
            {
                let mut arg_bc = self.do_compile(arg);
                self.bytecode.append(&mut arg_bc);
                // Box using the lookup head so enum Constructs get Enum tag.
                Self::emit_box_if_needed(&mut self.bytecode, &lookup_ty);
                Self::emit_call_indirect(&mut self.bytecode, offset as u32, 1);
                return;
            }
        }

        // Typechecker should have rejected; keep bytecode well-formed.
        let mut arg_bc = self.do_compile(arg);
        self.bytecode.append(&mut arg_bc);
        self.bytecode.push(Byte::new(Instruction::STRINGIFY));
    }

    fn show_lookup_ty_for_instance(ty: &Ty) -> Ty {
        match ty {
            Ty::Sum { name, .. } => Ty::Con(name.clone()),
            Ty::Constructor { owner, .. } => Self::show_lookup_ty_for_instance(owner),
            other => other.clone(),
        }
    }

    fn tuple_show_format(len: usize) -> String {
        match len {
            0 => "()".to_string(),
            1 => "(%s,)".to_string(),
            _ => format!("({})", vec!["%s"; len].join(", ")),
        }
    }

    fn record_show_format(fields: &[(String, Ty)]) -> String {
        if fields.is_empty() {
            return "{}".to_string();
        }
        let parts = fields
            .iter()
            .map(|(name, _)| format!("{name}: %s"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{{ {parts} }}")
    }

    fn emit_show_for_stack_value(&mut self, ty: &Ty) {
        let resolved = crate::typechecking::subst::apply_ty_prune(self.checker.subst(), ty);
        match resolved {
            Ty::Tuple(items) => self.emit_tuple_show_for_stack_value(&items),
            Ty::Record { fields } => self.emit_record_show_for_stack_value(&fields),
            other => {
                let lookup_ty = Self::show_lookup_ty_for_instance(&other);
                if let Some(instance) = self
                    .checker
                    .generics()
                    .find_instance("Show", std::slice::from_ref(&lookup_ty))
                    .cloned()
                    && let Some(fqn) = instance.method_fqns.get("show").cloned()
                    && let Some(&offset) = self.functions.get(&fqn)
                {
                    Self::emit_box_if_needed(&mut self.bytecode, &lookup_ty);
                    Self::emit_call_indirect(&mut self.bytecode, offset as u32, 1);
                } else {
                    self.bytecode.push(Byte::new(Instruction::STRINGIFY));
                }
            }
        }
    }

    fn emit_tuple_show_for_stack_value(&mut self, items: &[Ty]) {
        let tuple_slot = self.alloc_temp_slot();
        self.bytecode.push_store_pop(tuple_slot);

        let mut element_slots = Vec::with_capacity(items.len());
        for (idx, item_ty) in items.iter().enumerate() {
            self.bytecode.push_load(tuple_slot);
            self.bytecode.push_const(idx as i32);
            self.bytecode.push_index();
            self.emit_show_for_stack_value(item_ty);
            let slot = self.alloc_temp_slot();
            self.bytecode.push_store_pop(slot);
            element_slots.push(slot);
        }

        self.emit_string_literal(&Self::tuple_show_format(items.len()));
        for slot in element_slots {
            self.bytecode.push_load(slot);
        }
        self.bytecode
            .push(Byte::new(Instruction::FORMAT).with_operand_u32(items.len() as u32));
    }

    fn emit_record_show_for_stack_value(&mut self, fields: &[(String, Ty)]) {
        let record_slot = self.alloc_temp_slot();
        self.bytecode.push_store_pop(record_slot);

        let mut field_slots = Vec::with_capacity(fields.len());
        for (name, field_ty) in fields {
            self.bytecode.push_load(record_slot);
            let idx = self.intern_string(name);
            self.bytecode.push_string(idx);
            self.bytecode.push_get_field();
            self.emit_show_for_stack_value(field_ty);
            let slot = self.alloc_temp_slot();
            self.bytecode.push_store_pop(slot);
            field_slots.push(slot);
        }

        self.emit_string_literal(&Self::record_show_format(fields));
        for slot in field_slots {
            self.bytecode.push_load(slot);
        }
        self.bytecode
            .push(Byte::new(Instruction::FORMAT).with_operand_u32(fields.len() as u32));
    }

    /// Structurally bind scheme pattern variables to concrete call-site types.
    ///
    /// Used by [`emit_call_site_dicts`] so `F<A>` against `Option<int>` records
    /// both `F = Option` and `A = int` (Phase 5).
    fn bind_scheme_vars(
        pattern: &Ty,
        concrete: &Ty,
        map: &mut HashMap<crate::typechecking::ty::TyVarId, Ty>,
    ) {
        use crate::typechecking::ty::{option_inner, result_ok_err};

        match (pattern, concrete) {
            (Ty::Var(v), c) => {
                map.entry(*v).or_insert_with(|| c.clone());
            }
            (Ty::App(h1, a1), Ty::App(h2, a2)) if a1.len() == a2.len() => {
                Self::bind_scheme_vars(h1, h2, map);
                for (p, c) in a1.iter().zip(a2.iter()) {
                    Self::bind_scheme_vars(p, c, map);
                }
            }
            // `F<A>` vs builtin Option/Result constructor or structural sum:
            // bind `F` to the constructor constant and recurse into payloads.
            (Ty::App(head, args), other)
                if matches!(head.as_ref(), Ty::Var(_))
                    && (option_inner(other).is_some() || result_ok_err(other).is_some()) =>
            {
                if let Some(inner) = option_inner(other) {
                    if args.len() == 1 {
                        Self::bind_scheme_vars(
                            head,
                            &Ty::Con(common::BUILTIN_OPTION_ENUM.into()),
                            map,
                        );
                        Self::bind_scheme_vars(&args[0], &inner, map);
                    }
                } else if let Some((ok, err)) = result_ok_err(other) {
                    if args.len() == 2 {
                        Self::bind_scheme_vars(
                            head,
                            &Ty::Con(common::BUILTIN_RESULT_ENUM.into()),
                            map,
                        );
                        Self::bind_scheme_vars(&args[0], &ok, map);
                        Self::bind_scheme_vars(&args[1], &err, map);
                    }
                }
            }
            (Ty::App(h1, a1), Ty::Constructor { owner, .. }) => {
                Self::bind_scheme_vars(&Ty::App(h1.clone(), a1.clone()), owner.as_ref(), map);
            }
            (Ty::Fun(a1, r1), Ty::Fun(a2, r2)) => {
                Self::bind_scheme_vars(a1, a2, map);
                Self::bind_scheme_vars(r1, r2, map);
            }
            (Ty::Tuple(t1), Ty::Tuple(t2)) if t1.len() == t2.len() => {
                for (p, c) in t1.iter().zip(t2.iter()) {
                    Self::bind_scheme_vars(p, c, map);
                }
            }
            // Rest packs are `[T]` / `[T; N]` — bind `T` from the element.
            (Ty::Array { element: e1, .. }, Ty::Array { element: e2, .. }) => {
                Self::bind_scheme_vars(e1, e2, map);
            }
            _ => {}
        }
    }

    /// Emit one instance dictionary (`CodePtr`s + `MakeTuple`) for a
    /// trait constraint whose type arguments have already been resolved
    /// to concrete lookup types. Returns `true` when a dict was pushed.
    ///
    /// Layout (Phase 5): subclass methods first, then each superclass’s
    /// methods in declaration order (flattened). Superclass slots are filled
    /// from the matching superclass instance for the same type arguments.
    fn emit_instance_dict(
        bytecode: &mut Vec<Byte>,
        class: &str,
        lookup: &[crate::typechecking::Ty],
        checker: &Checker,
        functions: &HashMap<String, usize>,
    ) -> bool {
        let Some(instance) = checker.generics().find_instance_relaxed(class, lookup) else {
            return false;
        };
        let Some(class_def) = checker.generics().typeclass(&instance.class) else {
            return false;
        };
        let flat = class_def.flattened_methods(checker.generics());
        let n_methods = flat.len() as u32;
        for (owner_class, method_def) in &flat {
            let fqn = if *owner_class == instance.class.as_str() {
                instance.method_fqns.get(&method_def.name).cloned()
            } else {
                checker
                    .generics()
                    .find_instance_relaxed(owner_class, lookup)
                    .and_then(|super_inst| super_inst.method_fqns.get(&method_def.name).cloned())
            };
            let offset = fqn
                .and_then(|name| functions.get(&name).copied())
                .unwrap_or(0);
            bytecode.push(Byte::new(Instruction::CodePtr).with_operand_u32(offset as u32));
        }
        bytecode.push_make_tuple(n_methods);
        true
    }

    fn emit_existential_pack_recipe(
        bytecode: &mut Vec<Byte>,
        pack: &crate::typechecking::infer::ExistentialPack,
        checker: &Checker,
        functions: &HashMap<String, usize>,
    ) {
        Self::emit_box_if_needed(bytecode, &pack.value_ty);
        if Self::emit_instance_dict(
            bytecode,
            &pack.class,
            std::slice::from_ref(&pack.value_ty),
            checker,
            functions,
        ) {
            bytecode.push_make_tuple(2);
        }
    }

    fn append_with_existential_pack(&mut self, bytecode: &mut Vec<Byte>, expr: &Output) {
        let pack = self
            .checker
            .existential_pack_for_span(expr.0.start, expr.0.end)
            .cloned();
        bytecode.append(&mut self.do_compile(expr));
        if let Some(pack) = pack {
            Self::emit_existential_pack_recipe(bytecode, &pack, &self.checker, &self.functions);
        }
    }

    /// Compile `expr` into [`Self::bytecode`] for an immediate store (`let` / `=`).
    fn emit_binding_rhs(&mut self, expr: &Output) {
        let prev = self.suppress_match_fusion_barrier;
        self.suppress_match_fusion_barrier = true;
        let pack = self
            .checker
            .existential_pack_for_span(expr.0.start, expr.0.end)
            .cloned();
        let mut expr_bc = self.do_compile(expr);
        self.bytecode.append(&mut expr_bc);
        if let Some(pack) = pack {
            let mut pack_bc = Vec::new();
            Self::emit_existential_pack_recipe(&mut pack_bc, &pack, &self.checker, &self.functions);
            self.bytecode.append(&mut pack_bc);
        }
        self.suppress_match_fusion_barrier = prev;
    }

    /// Like [`Self::append_with_existential_pack`], but match expressions skip
    /// the join `DUPLICATE; POP` barrier because the value is stored immediately.
    fn append_binding_rhs(&mut self, bytecode: &mut Vec<Byte>, expr: &Output) {
        let prev = self.suppress_match_fusion_barrier;
        self.suppress_match_fusion_barrier = true;
        self.append_with_existential_pack(bytecode, expr);
        self.suppress_match_fusion_barrier = prev;
    }

    fn load_tuple_field(bytecode: &mut Vec<Byte>, tuple_slot: u32, index: i32) {
        bytecode.push_load(tuple_slot);
        bytecode.push_const(index);
        bytecode.push_index();
    }

    fn emit_existential_method_call(
        &mut self,
        bytecode: &mut Vec<Byte>,
        name: &Output,
        args: Option<&Vec<Output>>,
        hint: &crate::typechecking::infer::ExistentialMethodCall,
    ) -> bool {
        let (pack_expr, extra_args): (&Output, &[Output]) = if hint.has_receiver {
            let Expression::Access(recv, _) = name.1.as_ref() else {
                return false;
            };
            (recv, args.map(Vec::as_slice).unwrap_or(&[]))
        } else {
            let Some(items) = args else {
                return false;
            };
            let Some((first, rest)) = items.split_first() else {
                return false;
            };
            (first, rest)
        };

        let pack_slot = self.alloc_temp_slot();
        bytecode.append(&mut self.do_compile(pack_expr));
        bytecode.push_store_pop(pack_slot);

        // Pack layout: tuple[0] = boxed value, tuple[1] = dictionary tuple.
        Self::load_tuple_field(bytecode, pack_slot, 0);
        for arg in extra_args {
            self.append_with_existential_pack(bytecode, arg);
        }
        Self::load_tuple_field(bytecode, pack_slot, 1);
        Self::load_tuple_field(bytecode, pack_slot, 1);
        bytecode.push_const(hint.method_slot as i32);
        bytecode.push_index();
        bytecode.push(Byte::new(Instruction::CallIndirect).with_operand_u32(hint.arity as u32 + 1));
        true
    }

    /// Resolve a constraint's type arguments through `var_to_ty`, returning
    /// concrete lookup types when every argument is ground. `None` means at
    /// least one argument is still open (cannot synthesize yet).
    fn resolve_constraint_lookup(
        constraint: &crate::typechecking::ty::Constraint,
        var_to_ty: &HashMap<crate::typechecking::ty::TyVarId, crate::typechecking::Ty>,
        checker: &Checker,
    ) -> Option<Vec<crate::typechecking::Ty>> {
        use crate::typechecking::Ty;
        use crate::typechecking::subst::apply_ty_prune;
        use crate::typechecking::ty::ftv_ty;

        let mut resolved = Vec::with_capacity(constraint.args.len());
        for arg in &constraint.args {
            let concrete = match arg {
                Ty::Var(v) => apply_ty_prune(checker.subst(), var_to_ty.get(v)?),
                other => apply_ty_prune(checker.subst(), other),
            };
            if !ftv_ty(&concrete).is_empty() {
                return None;
            }
            resolved.push(concrete);
        }
        // Constructor-kinded class params look up by constructor head
        // (`Option`, `Result`), not applied types.
        let lookup = if let Some(class_def) = checker.generics().typeclass(&constraint.class) {
            resolved
                .iter()
                .enumerate()
                .map(|(i, concrete)| {
                    if class_def.is_constructor_kind_at(i) {
                        match concrete {
                            Ty::App(head, _) => head.as_ref().clone(),
                            other => other.clone(),
                        }
                    } else {
                        concrete.clone()
                    }
                })
                .collect()
        } else {
            resolved
        };
        Some(lookup)
    }

    /// Emit dictionary tuples for a non-monomorphized generic call site.
    ///
    /// Convention: after value args, one `MakeTuple` per typeclass
    /// constraint. Compiler-provided and source-provided instances use the
    /// same dictionary layout.
    /// Each tuple holds method entry offsets in flattened declaration order
    /// (subclass methods, then superclass methods — Phase 5)
    /// (`CodePtr <offset>`, or `CodePtr 0` if the FQN is not compiled yet).
    ///
    /// Instances are resolved from the callee's scheme + concrete argument
    /// types (not `NodeId`), because the pre-walk / infer ID table can be
    /// misaligned inside function bodies.
    ///
    /// Returns the number of dict tuples pushed (used to bump CALL arity).
    fn emit_call_site_dicts(
        bytecode: &mut Vec<Byte>,
        fn_name: &str,
        arg_tys: &[crate::typechecking::Ty],
        ret_ty: Option<&crate::typechecking::Ty>,
        checker: &Checker,
        functions: &HashMap<String, usize>,
    ) -> usize {
        use crate::typechecking::Ty;

        let Some(scheme) = checker.env().lookup(fn_name).cloned() else {
            return 0;
        };
        // Map quantified vars → concrete arg types by structurally matching
        // the curried function type against the call's argument types.
        // Phase 5: `F<A>` vs `Option<int>` binds both `F = Option` and `A = int`.
        // Do NOT apply the global subst to the scheme — those vars may have
        // been reused/unified later in the program.
        let mut var_to_ty: HashMap<crate::typechecking::ty::TyVarId, crate::typechecking::Ty> =
            HashMap::new();
        let mut fun = &scheme.ty;
        let mut arg_idx = 0usize;
        while let Ty::Fun(param, ret) = fun {
            if arg_idx >= arg_tys.len() {
                break;
            }
            Self::bind_scheme_vars(param.as_ref(), &arg_tys[arg_idx], &mut var_to_ty);
            fun = ret.as_ref();
            arg_idx += 1;
        }
        // Multi-param constraints often mention return-type vars
        // (`Convert<A, B>` with `A -> B`). Bind those from the call's result type.
        if let Some(ret_ty) = ret_ty {
            Self::bind_scheme_vars(fun, ret_ty, &mut var_to_ty);
        }

        let mut dict_count = 0;
        for constraint in &scheme.constraints {
            let Some(lookup) = Self::resolve_constraint_lookup(constraint, &var_to_ty, checker)
            else {
                continue;
            };
            if Self::emit_instance_dict(bytecode, &constraint.class, &lookup, checker, functions) {
                dict_count += 1;
            }
        }
        dict_count
    }

    /// Phase 4: push dictionary evidence for every constraint slot when a
    /// generic function escapes into a `PolyFn` value.
    ///
    /// Slot fill order per constraint index:
    /// 1. in-scope `__dictN` (open bound forwarded from the enclosing frame)
    /// 2. concrete instance synthesis when constraint args are ground
    /// 3. null sentinel (`CONST 0` → `None` in `MakePolyFnCapture`) when
    ///    evidence is truly unavailable (e.g. top-level `let f = show`)
    ///
    /// Returns the dict arity (number of stack slots pushed). Caller always
    /// emits `MakePolyFnCapture` when this is non-zero.
    fn emit_polyfn_escape_dicts(
        &self,
        bytecode: &mut Vec<Byte>,
        fn_name: &str,
        escape_ty: Option<&crate::typechecking::Ty>,
    ) -> usize {
        let dict_arity = self.checker.dict_arity_for(fn_name);
        if dict_arity == 0 {
            return 0;
        }

        let scheme = self.checker.env().lookup(fn_name).cloned();
        let mut var_to_ty: HashMap<crate::typechecking::ty::TyVarId, crate::typechecking::Ty> =
            HashMap::new();
        if let (Some(scheme), Some(escape_ty)) = (scheme.as_ref(), escape_ty) {
            // Bind scheme vars from the escape site's instantiated type so
            // ground specializations can synthesize instance dictionaries.
            Self::bind_scheme_vars(&scheme.ty, escape_ty, &mut var_to_ty);
        }

        for dict_index in 0..dict_arity {
            if let Some(slot) = self.lookup_slot(&format!("__dict{}", dict_index)) {
                bytecode.push_load(slot);
                continue;
            }

            let synthesized = scheme.as_ref().and_then(|s| {
                let constraint = s.constraints.get(dict_index)?;
                let lookup =
                    Self::resolve_constraint_lookup(constraint, &var_to_ty, &self.checker)?;
                Self::emit_instance_dict(
                    bytecode,
                    &constraint.class,
                    &lookup,
                    &self.checker,
                    &self.functions,
                )
                .then_some(())
            });
            if synthesized.is_none() {
                // Unresolved sentinel — CallIndirect fills from app evidence.
                bytecode.push_const(0);
            }
        }
        dict_arity
    }

    /// Emit `HostInvoke` for a virtual `io` free function.
    ///
    /// Nested IO calls (e.g. `read_to_end(stdin())`) also write directly to
    /// `self.bytecode` via this helper. Emit the native-id `CONST` **before**
    /// compiling arguments so the runtime stack is `[id, arg0, …]` — the order
    /// `HostInvoke` expects. Compiling args into a side buffer first left nested
    /// invokes *above* the id and `MakeTuple` packed the wrong values (piped
    /// stdin then looked empty).
    ///
    /// Always targets [`Self::bytecode`] so nested `format` / `match` (same
    /// buffer) stay contiguous with HostInvoke staging.
    fn emit_io_host_invoke(&mut self, kind: crate::typechecking::IoBuiltin, args: &[Output]) {
        self.emit_host_native_invoke(kind.native_name(), args);
    }

    fn emit_thread_host_invoke(
        &mut self,
        kind: crate::typechecking::ThreadBuiltin,
        args: &[Output],
    ) {
        self.emit_host_native_invoke(kind.native_name(), args);
    }

    /// Match `f(arg)` where `f` is a unary recursive-pure function.
    fn match_unary_recursive_pure_call<'a>(
        &self,
        expr: &'a Output<'a>,
    ) -> Option<(String, &'a Output<'a>)> {
        if self.recursive_pure.is_empty() {
            return None;
        }
        let expr = unwrap_expr_output(expr);
        let Expression::Call {
            name,
            args: Some(args),
        } = expr.1.as_ref()
        else {
            return None;
        };
        if args.len() != 1 {
            return None;
        }
        let callee = match unwrap_expr_output(name).1.as_ref() {
            Expression::Identifier(n) => *n,
            _ => return None,
        };
        let resolved = self
            .aliases
            .get(callee)
            .cloned()
            .unwrap_or_else(|| callee.to_string());
        let short = strip_overload_key(&resolved);
        if !self.recursive_pure.contains(callee)
            && !self.recursive_pure.contains(short)
            && !self.recursive_pure.contains(&resolved)
        {
            return None;
        }
        if let Some(ty) = self.codegen_expr_ty(&args[0]) {
            let pruned = crate::typechecking::subst::apply_ty_prune(self.checker.subst(), &ty);
            if !self.checker.is_thread_sendable_ty(&pruned) {
                return None;
            }
        }
        Some((resolved, &args[0]))
    }

    /// Auto fork-join: `f(a) ⊕ f(b)` for recursive-pure unary `f`.
    ///
    /// Tries `thread_spawn(f, a)`; on `Ok` runs `f(b)` inline then `join`;
    /// on `WouldBlock` / other Err falls back to sequential `f(a) ⊕ f(b)`.
    /// Writes directly to [`Self::bytecode`] (control-flow labels).
    fn try_emit_auto_par_binop(
        &mut self,
        lhs: &Output<'_>,
        rhs: &Output<'_>,
        int_op: Instruction,
        float_op: Instruction,
    ) -> bool {
        let Some((fname, arg_l)) = self.match_unary_recursive_pure_call(lhs) else {
            return false;
        };
        let Some((fname_r, _arg_r)) = self.match_unary_recursive_pure_call(rhs) else {
            return false;
        };
        if fname != fname_r {
            return false;
        }
        let entry_key = self
            .fn_arities
            .keys()
            .find(|k| strip_overload_key(k) == strip_overload_key(&fname) || *k == &fname)
            .cloned()
            .unwrap_or_else(|| fname.clone());
        let (fa, is_rest) = self
            .fn_arities
            .get(&entry_key)
            .or_else(|| self.fn_arities.get(&fname))
            .copied()
            .unwrap_or((1, false));
        if fa != 1 || is_rest {
            return false;
        }
        let Some(&entry_offset) = self
            .functions
            .get(&entry_key)
            .or_else(|| self.functions.get(&fname))
        else {
            return false;
        };
        let Some(spawn_id) = self.native_id("thread_spawn") else {
            return false;
        };
        let Some(join_id) = self.native_id("thread_join") else {
            return false;
        };

        let is_float = self
            .codegen_expr_ty(lhs)
            .map(|t| {
                let pruned = crate::typechecking::subst::apply_ty_prune(self.checker.subst(), &t);
                matches!(
                    pruned,
                    crate::typechecking::Ty::Con(ref n) if n == "float"
                )
            })
            .unwrap_or(false);
        let bin_op = if is_float { float_op } else { int_op };

        // MakeFn for the recursive callee.
        self.bytecode.push_const(0);
        self.bytecode.push(
            Byte::new(Instruction::CodePtr).with_operand_u32(entry_offset as u32),
        );
        self.bytecode.push(
            Byte::new(Instruction::MakeFn).with_operand_u32(make_fn_operand(0, 0, 1, false)),
        );
        let fn_tmp = self.alloc_temp_slot();
        self.bytecode.push_store_pop(fn_tmp);

        let mut arg_l_bc = self.do_compile(arg_l);
        self.bytecode.append(&mut arg_l_bc);
        let arg_l_tmp = self.alloc_temp_slot();
        self.bytecode.push_store_pop(arg_l_tmp);

        // thread_spawn(fn, arg_l) → Result<Thread, Error>
        self.bytecode
            .push(Byte::new(Instruction::CONST).with_value_u32(spawn_id as u32));
        self.bytecode.push_load(fn_tmp);
        self.bytecode.push_load(arg_l_tmp);
        self.bytecode.push_make_tuple(2);
        self.bytecode.push_host_invoke(2);

        let mut bb = BlockBuilder::new();
        let have_handle = bb.fresh_label(self.bytecode.il_mut());
        let done = bb.fresh_label(self.bytecode.il_mut());
        bb.emit_jump_to(
            have_handle,
            BbJumpKind::JumpIfMatch { tag: 0, arity: 1 },
            self.bytecode.il_mut(),
        );

        // Capacity / spawn failure: discard Err, run both calls sequentially.
        self.bytecode.push_pop();
        let mut lhs_bc = self.do_compile(lhs);
        self.bytecode.append(&mut lhs_bc);
        let mut rhs_bc = self.do_compile(rhs);
        self.bytecode.append(&mut rhs_bc);
        self.bytecode.push(Byte::new(bin_op));
        bb.emit_jump_to(done, BbJumpKind::Unconditional, self.bytecode.il_mut());

        bb.bind_label(have_handle, self.bytecode.il_mut());
        // Ok payload (Thread handle) on stack.
        let handle_tmp = self.alloc_temp_slot();
        self.bytecode.push_store_pop(handle_tmp);

        // Inline the other arm on this thread.
        let mut rhs_inline = self.do_compile(rhs);
        self.bytecode.append(&mut rhs_inline);
        let right_tmp = self.alloc_temp_slot();
        self.bytecode.push_store_pop(right_tmp);

        // join(handle) → Result<T, Error>
        self.bytecode
            .push(Byte::new(Instruction::CONST).with_value_u32(join_id as u32));
        self.bytecode.push_load(handle_tmp);
        self.bytecode.push_make_tuple(1);
        self.bytecode.push_host_invoke(1);
        self.emit_result_unwrap_or_panic();

        self.bytecode.push_load(right_tmp);
        self.bytecode.push(Byte::new(bin_op));

        bb.bind_label(done, self.bytecode.il_mut());
        bb.finalize()
            .expect("BlockBuilder::finalize: auto-par labels bound");
        true
    }

    /// Emit `HostInvoke` for a pipeline-registered host native by registry name.
    fn emit_host_native_invoke(&mut self, native_name: &str, args: &[Output]) {
        let Some(native_id) = self.native_id(native_name) else {
            let range = args.first().map(|a| a.0.into_range()).unwrap_or(0..0);
            let mut message = Message::error(
                ErrorCode::UnknownFunction,
                format!("Host native `{native_name}` is not registered with the pipeline"),
                range.clone(),
            );
            if range.start >= range.end {
                message.with_help(
                    "host natives are wired in Pipeline::register_io_natives / register_thread_natives"
                        .to_string(),
                );
            } else {
                message.push(DiagLabel::new(
                    "host natives are wired in Pipeline::register_io_natives / register_thread_natives"
                        .to_string(),
                    range,
                ));
            }
            self.messages.push(message);
            return;
        };
        let depth_on_entry = self.expr_depth;
        let mut arg_slots = Vec::with_capacity(args.len());
        for arg in args {
            // Nested HostInvoke / format / match write to `self.bytecode`; also
            // fold any bytes returned in the local vec (non-host subexprs).
            let mut arg_bc = self.do_compile(arg);
            self.bytecode.append(&mut arg_bc);
            let slot = self.alloc_temp_slot();
            self.bytecode.push_store_pop(slot);
            arg_slots.push(slot);
        }
        // Native id first, then reload staged args — nested HostInvoke in
        // args must not sit above the id on the runtime stack.
        self.bytecode
            .push(Byte::new(Instruction::CONST).with_value_u32(native_id as u32));
        self.expr_depth = depth_on_entry + 1;
        for slot in &arg_slots {
            self.bytecode.push_load(*slot);
            self.expr_depth += 1;
        }
        let arity = args.len();
        self.bytecode.push_make_tuple(arity as u32);
        self.bytecode.push_host_invoke(arity as u32);
        // Result stays on the stack for the caller (ExprStatement POPs it).
        self.expr_depth = depth_on_entry;
    }

    fn emit_prelude_host_call(&mut self, args: &[Output], native_name: &str) {
        self.emit_host_native_invoke(native_name, args);
    }

    /// Emit bytecode thunks for compiler-provided primitive instances.
    ///
    /// Shared generic bodies receive boxed type-parameter values, so numeric
    /// thunks unbox their two arguments and re-box a type-parameter result.
    /// Comparison methods return their concrete `bool` result directly. Every
    /// thunk accepts the ordinary hidden trailing dictionary argument, even
    /// though primitive implementations do not need to inspect it.
    fn emit_builtin_dict_thunks(&mut self) {
        use crate::typechecking::generics::Generics;

        let emit = |compiler: &mut Self,
                    class: &str,
                    ty: &str,
                    method: &str,
                    tag: ValueTag,
                    op: Instruction,
                    boxes_result: bool| {
            let fqn = Generics::builtin_instance_fqn(class, ty, method);
            if compiler.functions.contains_key(&fqn) {
                return;
            }
            compiler.bind_function_entry(fqn);
            for slot in 0..2 {
                compiler.bytecode.push_load(slot);
                compiler.bytecode.push_unbox_value(tag as u32);
            }
            compiler.bytecode.push(Byte::new(op));
            if boxes_result {
                compiler.bytecode.push_box_value(tag as u32);
            }
            compiler.bytecode.push_return();
        };

        for (ty, tag, arithmetic, comparisons) in [
            (
                "int",
                ValueTag::Int,
                [
                    ("Add", "add", Instruction::ADD),
                    ("Sub", "sub", Instruction::SUB),
                    ("Mul", "mul", Instruction::MUL),
                    ("Div", "div", Instruction::DIV),
                ],
                [
                    ("Lt", "lt", Instruction::LE),
                    ("Le", "le", Instruction::LEQ),
                    ("Gt", "gt", Instruction::GT),
                    ("Ge", "ge", Instruction::GEQ),
                    ("Eq", "eq", Instruction::EQ),
                    ("Eq", "ne", Instruction::NEQ),
                ],
            ),
            (
                "float",
                ValueTag::Float,
                [
                    ("Add", "add", Instruction::ADDF),
                    ("Sub", "sub", Instruction::SUBF),
                    ("Mul", "mul", Instruction::MULF),
                    ("Div", "div", Instruction::DIVF),
                ],
                [
                    ("Lt", "lt", Instruction::LEF),
                    ("Le", "le", Instruction::LEQF),
                    ("Gt", "gt", Instruction::GTF),
                    ("Ge", "ge", Instruction::GEQF),
                    ("Eq", "eq", Instruction::EQ),
                    ("Eq", "ne", Instruction::NEQ),
                ],
            ),
        ] {
            for (class, method, op) in arithmetic {
                emit(self, class, ty, method, tag, op, true);
            }
            for (class, method, op) in comparisons {
                emit(self, class, ty, method, tag, op, false);
            }
        }
        for (ty, tag) in [
            ("string", ValueTag::String),
            ("bool", ValueTag::Bool),
            ("byte", ValueTag::Int),
        ] {
            emit(self, "Eq", ty, "eq", tag, Instruction::EQ, false);
            emit(self, "Eq", ty, "ne", tag, Instruction::NEQ, false);
        }

        // Show thunks: accept a boxed (or heap-string) argument at slot 0,
        // ignore the trailing dictionary, and return an ObjString via STRINGIFY.
        for ty in ["int", "float", "string", "bool", "unit", "byte"] {
            let fqn = Generics::builtin_instance_fqn("Show", ty, "show");
            if self.functions.contains_key(&fqn) {
                continue;
            }
            self.bind_function_entry(fqn);
            self.bytecode.push_load(0);
            self.bytecode.push(Byte::new(Instruction::STRINGIFY));
            self.bytecode.push_return();
        }

        // Length__string__len: unbox (dict ABI) then ArrayLen (byte length).
        {
            let fqn = Generics::builtin_instance_fqn("Length", "string", "len");
            if !self.functions.contains_key(&fqn) {
                self.bind_function_entry(fqn);
                self.bytecode.push_load(0);
                self.bytecode.push_unbox_value(ValueTag::String as u32);
                self.bytecode.push(Byte::new(Instruction::ArrayLen));
                self.bytecode.push_return();
            }
        }

        // Hash thunks: boxed receiver at slot 0 → int. int/byte/bool identity
        // after unbox; float returns the float `Value` bits read via
        // `Value::as_int()` (IEEE bit pattern in the current Value encoding);
        // unit is 0; string uses the intern FNV via HostInvoke `hash_string`.
        for (ty, tag) in [
            ("int", ValueTag::Int),
            ("byte", ValueTag::Int),
            ("bool", ValueTag::Bool),
            ("float", ValueTag::Float),
        ] {
            let fqn = Generics::builtin_instance_fqn("Hash", ty, "hash");
            if self.functions.contains_key(&fqn) {
                continue;
            }
            self.bind_function_entry(fqn);
            self.bytecode.push_load(0);
            self.bytecode.push_unbox_value(tag as u32);
            self.bytecode.push_return();
        }
        {
            let fqn = Generics::builtin_instance_fqn("Hash", "unit", "hash");
            if !self.functions.contains_key(&fqn) {
                self.bind_function_entry(fqn);
                self.bytecode.push_const(0);
                self.bytecode.push_return();
            }
        }
        if let Some(native_id) = self.native_id("hash_string") {
            let fqn = Generics::builtin_instance_fqn("Hash", "string", "hash");
            if !self.functions.contains_key(&fqn) {
                self.bind_function_entry(fqn);
                self.bytecode
                    .push(Byte::new(Instruction::CONST).with_value_u32(native_id as u32));
                self.bytecode.push_load(0);
                self.bytecode.push_unbox_value(ValueTag::String as u32);
                self.bytecode.push_make_tuple(1);
                self.bytecode.push_host_invoke(1);
                self.bytecode.push_return();
            }
        }

        // Read/Write for Stream — lower to the same HostInvoke natives as
        // free functions `read` / `write`. Args may arrive boxed via the
        // dictionary ABI; unbox then call.
        for (class, method, native_name, arity) in [
            ("Read", "read", "read", 2u32),
            ("Write", "write", "write", 2u32),
        ] {
            let fqn = Generics::builtin_instance_fqn(class, "Stream", method);
            if self.functions.contains_key(&fqn) {
                continue;
            }
            let Some(native_id) = self.native_id(native_name) else {
                continue;
            };
            self.bind_function_entry(fqn);
            self.bytecode
                .push(Byte::new(Instruction::CONST).with_value_u32(native_id as u32));
            self.bytecode.push_load(0);
            self.bytecode.push_unbox_value(ValueTag::Instance as u32);
            self.bytecode.push_load(1);
            self.bytecode.push_unbox_value(ValueTag::Array as u32);
            self.bytecode.push_make_tuple(arity);
            self.bytecode.push_host_invoke(arity);
            self.bytecode.push_return();
        }

        let into_pairs = [
            (
                "int",
                "float",
                Instruction::CastIntToFloat,
                ValueTag::Int,
                ValueTag::Float,
            ),
            (
                "float",
                "int",
                Instruction::CastFloatToInt,
                ValueTag::Float,
                ValueTag::Int,
            ),
            (
                "int",
                "byte",
                Instruction::CastIntToByte,
                ValueTag::Int,
                ValueTag::Int,
            ),
            (
                "byte",
                "int",
                Instruction::CastByteToInt,
                ValueTag::Int,
                ValueTag::Int,
            ),
            (
                "int",
                "bool",
                Instruction::CastIntToBool,
                ValueTag::Int,
                ValueTag::Bool,
            ),
            (
                "bool",
                "int",
                Instruction::CastBoolToInt,
                ValueTag::Bool,
                ValueTag::Int,
            ),
        ];
        for (from, to, cast_op, from_tag, to_tag) in into_pairs {
            let fqn = into_primitive_fqn(from, to);
            if self.functions.contains_key(&fqn) {
                continue;
            }
            self.bind_function_entry(fqn);
            self.bytecode.push_load(0);
            self.bytecode.push_unbox_value(from_tag as u32);
            self.bytecode.push(Byte::new(cast_op));
            if from_tag != to_tag {
                self.bytecode.push_box_value(to_tag as u32);
            }
            self.bytecode.push_return();
        }
    }

    /// Map a fully-resolved `Ty` to a `ValueTag` for box/unbox
    /// emission at generic call boundaries.
    fn ty_to_value_tag(ty: &crate::typechecking::Ty) -> Option<ValueTag> {
        use crate::typechecking::{
            Ty, ty::BOOL, ty::BYTE, ty::FLOAT, ty::INT, ty::STRING, ty::UNIT,
        };
        match ty {
            Ty::Con(name) => match name.as_str() {
                INT | BYTE => Some(ValueTag::Int),
                FLOAT => Some(ValueTag::Float),
                STRING => Some(ValueTag::String),
                BOOL => Some(ValueTag::Bool),
                UNIT => Some(ValueTag::Unit),
                _ => Some(ValueTag::Instance), // user-defined class / enum
            },
            Ty::Sum { .. } => Some(ValueTag::Enum),
            Ty::Tuple(_) => Some(ValueTag::Tuple),
            Ty::Array { .. } => Some(ValueTag::Array),
            Ty::Record { .. } => Some(ValueTag::Record),
            // Open type vars — boxing is required but we don't know the tag yet
            Ty::Var(_) => None,
            _ => None,
        }
    }

    /// Peel `forall` / function arrows to the final return type.
    #[allow(dead_code)]
    fn peel_fn_return_ty(ty: &crate::typechecking::Ty) -> crate::typechecking::Ty {
        use crate::typechecking::Ty;
        let mut t = ty.clone();
        while let Ty::Forall { body, .. } = t {
            t = *body;
        }
        while let Ty::Fun(_, ret) = t {
            t = *ret;
        }
        t
    }

    /// Look up a function's scheme and peel to its return type.
    /// Lookup declared return type for diagnostics / tooling.
    #[allow(dead_code)]
    fn fn_return_ty(&self, name: &str) -> Option<crate::typechecking::Ty> {
        use crate::typechecking::subst::apply_ty_prune;
        let scheme = self
            .checker
            .env()
            .lookup(name)
            .or_else(|| {
                self.current_function_qualified
                    .as_deref()
                    .and_then(|q| self.checker.env().lookup(q))
            })
            .or_else(|| {
                self.current_function_table_key
                    .as_deref()
                    .and_then(|q| self.checker.env().lookup(q))
            })?;
        let applied = apply_ty_prune(self.checker.subst(), &scheme.ty);
        Some(Self::peel_fn_return_ty(&applied))
    }

    /// Emit defers + unit fall-through return when a body does not end in a return.
    ///
    /// Non-unit missing returns are diagnosed by HM (E0111). This epilogue only
    /// invents a unit/`0` sentinel (plus Result Ok-wrap in result-mode) so frames
    /// unwind and defers run. No Option/`None` invent.
    fn emit_fallthrough_return(&mut self, _name: &str, _span: SimpleSpan) {
        self.emit_run_defers();
        self.bytecode.push_const(0);
        if self.compiling_result_mode {
            Self::emit_ok_or_some_wrap(&mut self.bytecode, false);
        }
        self.bytecode.push_return();
    }

    /// True when IL ops in `[op_start, ops.len())` end with a return terminator
    /// (labels skipped). `op_start` must be an index into [`CodeBuf::ops`], not
    /// an emitting-code length from [`CodeBuf::len`].
    fn region_ends_with_return(&self, op_start: usize) -> bool {
        let ops = self.bytecode.ops();
        let mut i = ops.len();
        while i > op_start {
            i -= 1;
            match &ops[i] {
                IlOp::Label(_) => continue,
                IlOp::Return { .. }
                | IlOp::LoadReturnSlot { .. }
                | IlOp::ConstReturnImm { .. }
                | IlOp::BinReturn { .. }
                | IlOp::Halt { .. } => return true,
                op if op.is_plain_return() => return true,
                _ => return false,
            }
        }
        false
    }

    /// Emit a `BoxValue` instruction for a concrete `Ty` at a generic
    /// call argument boundary (concrete→generic).  Does nothing when the
    /// type is already open (Ty::Var), or if a tag cannot be determined.
    fn emit_box_if_needed(bytecode: &mut impl EmitBuf, ty: &crate::typechecking::Ty) {
        if let Some(tag) = Self::ty_to_value_tag(ty) {
            bytecode.push_box_value(tag as u32);
        }
    }

    /// Emit an `UnboxValue` instruction for a concrete `Ty` at a generic
    /// call return boundary (generic→concrete).  Does nothing when the
    /// type is open (`Ty::Var`) — the caller can't know the tag at compile
    /// time in that case (the boxed value stays boxed).
    fn emit_unbox_if_needed(bytecode: &mut Vec<Byte>, ty: &crate::typechecking::Ty) {
        if let Some(tag) = Self::ty_to_value_tag(ty) {
            // UnboxValue operand: [15:0] = ValueTag as u16.
            bytecode.push_unbox_value(tag as u32);
        }
    }

    fn compile_function_output_with_name<'compiler>(
        &mut self,
        method: &Output<'compiler>,
        qualified: String,
        argument_unbox_tys: &[Option<Ty>],
        dict_arity: usize,
    ) {
        let _method_id = self.next_emit_id();
        let Expression::Function {
            docs: _,
            name,
            is_coro,
            args,
            body,
            ..
        } = method.1.as_ref()
        else {
            let mut bc = self.do_compile(method);
            self.bytecode.append(&mut bc);
            return;
        };
        let Some(body) = body else {
            self.consume_function_signature_output(method);
            return;
        };

        self.bind_function_entry(qualified.clone());
        if *is_coro {
            self.coroutine_fns.insert(qualified.clone());
        }

        let prev_vars = std::mem::take(&mut self.context.variables);
        let prev_polyfn_vars = std::mem::take(&mut self.polyfn_vars);
        let prev_polyfn_sources = std::mem::take(&mut self.polyfn_sources);
        let prev_fn_table_key = self.current_function_table_key.take();
        self.current_function_table_key = Some(qualified.clone());
        self.context.variables = Interner::default();
        if self.compiling_method {
            let slot = self.context.variables.intern("self".to_string()) as u32;
            self.record_debug_local("self", slot);
        }

        let prev_result_mode = self.compiling_result_mode;
        self.compiling_result_mode = self.checker.fn_is_result_mode(name);
        let prev_fn_defers = std::mem::take(&mut self.fn_defers);

        let mut a = self.do_compile(args);
        self.bytecode.append(&mut a);
        for (slot, ty) in argument_unbox_tys.iter().enumerate() {
            if let Some(tag) = ty.as_ref().and_then(Self::ty_to_value_tag) {
                self.bytecode.push_load(slot as u32);
                self.bytecode.push_unbox_value(tag as u32);
                self.bytecode.push_store_pop(slot as u32);
            }
        }
        for dict_idx in 0..dict_arity {
            self.context.variables.intern(format!("__dict{}", dict_idx));
        }
        let body_op_start = self.bytecode.ops().len();
        let mut c = self.do_compile(body);
        self.bytecode.append(&mut c);

        if !self.region_ends_with_return(body_op_start) {
            self.emit_fallthrough_return(name, body.0);
        }

        self.fn_defers = prev_fn_defers;
        self.compiling_result_mode = prev_result_mode;
        self.context.variables = prev_vars;
        self.polyfn_vars = prev_polyfn_vars;
        self.polyfn_sources = prev_polyfn_sources;
        self.current_function_table_key = prev_fn_table_key;
    }

    fn instance_method_unbox_tys(
        &self,
        class: &str,
        method: &str,
        instance_args: &[Ty],
    ) -> Vec<Option<Ty>> {
        let Some(scheme) = self.checker.typeclass_method_scheme(class, method) else {
            return Vec::new();
        };
        let mut result = Vec::new();
        let mut current = &scheme.ty;
        while let Ty::Fun(param, ret) = current {
            let concrete = match param.as_ref() {
                Ty::Var(var) => scheme
                    .bounds
                    .iter()
                    .position(|bound| bound == var)
                    .and_then(|index| instance_args.get(index))
                    .cloned(),
                _ => None,
            };
            result.push(concrete);
            current = ret;
        }
        result
    }

    fn generic_return_depends_on_type_param(&self, name: &str) -> bool {
        let Some(scheme) = self.checker.env().lookup(name) else {
            return false;
        };
        let mut result = &scheme.ty;
        while let Ty::Fun(_, ret) = result {
            result = ret;
        }
        let free = crate::typechecking::subst::ftv(result);
        scheme.bounds.iter().any(|bound| free.contains(bound))
    }

    /// Whether a generic call's return value is boxed at the ABI boundary.
    ///
    /// Direct type-parameter arguments (`id<T>(T x) -> T`) are boxed at the
    /// call site, so the matching return must be unboxed. Type parameters that
    /// only appear nested under ADTs / HKT apps (`get<F, A>(F<A>) -> A`) keep
    /// the payload's native representation (e.g. a raw `int` inside
    /// `Option::Some`), so emitting `UnboxValue` would turn a valid immediate
    /// into `Value::default()`.
    fn generic_return_is_boxed(&self, name: &str) -> bool {
        let Some(scheme) = self.checker.env().lookup(name) else {
            return false;
        };
        let mut top_level_vars = std::collections::HashSet::new();
        let mut current = &scheme.ty;
        while let Ty::Fun(param, ret) = current {
            if let Ty::Var(v) = param.as_ref() {
                top_level_vars.insert(*v);
            }
            current = ret;
        }
        let free = crate::typechecking::subst::ftv(current);
        scheme
            .bounds
            .iter()
            .any(|bound| free.contains(bound) && top_level_vars.contains(bound))
    }

    /// Whether CallIndirect args through `local` must be `BoxValue`'d.
    ///
    /// Bare `let f = show` sets [`Self::polyfn_sources`]. Locals assigned from a
    /// *call* that returns a PolyFn (`let f = capture_show(0)`) are seeded into
    /// [`Self::polyfn_vars`] from the binder's span in
    /// [`Checker::is_polyfn_binding_at`] when the let is emitted. Both sets are
    /// snapshotted around `{ … }` blocks so an inner PolyFn cannot poison an
    /// outer same-named ObjFn. Mono partials / lambdas stay unboxed.
    fn local_call_needs_arg_boxing(&self, local: &str) -> bool {
        // Only the codegen-scoped sets — never a flat name table (that would
        // leak across `{ … }` block shadows).
        self.polyfn_sources.contains_key(local) || self.polyfn_vars.contains(local)
    }

    /// Whether a CallIndirect through a local needs a post-call `UnboxValue`.
    ///
    /// Direct `polyfn_sources` mappings consult the original generic scheme.
    /// Returned/captured PolyFns and rank-n parameters fall back to the local's
    /// recorded type: unbox only when that type's result is still a type
    /// parameter (boxed at runtime) and the call site resolved it concretely.
    fn local_polyfn_call_needs_unbox(&self, local: &str, call_ty: &Ty) -> bool {
        use crate::typechecking::subst::apply_ty_prune;

        if let Some(source) = self.polyfn_sources.get(local) {
            return self.generic_return_depends_on_type_param(source);
        }
        if Self::ty_to_value_tag(call_ty).is_none() {
            return false;
        }
        let Some(var_ty) = self.codegen_var_type_for(local) else {
            return false;
        };
        let pruned = apply_ty_prune(self.checker.subst(), &var_ty);
        let result_ty = match &pruned {
            Ty::Forall { body, .. } => {
                let mut result = body.as_ref();
                while let Ty::Fun(_, ret) = result {
                    result = ret.as_ref();
                }
                result.clone()
            }
            other => {
                let mut result = other;
                while let Ty::Fun(_, ret) = result {
                    result = ret.as_ref();
                }
                result.clone()
            }
        };
        // Shared generic bodies box type-parameter results; a Var return
        // means the value on the stack is still boxed at this call site.
        matches!(result_ty, Ty::Var(_))
    }

    fn emit_mono_specializations_for_function<'compiler>(
        &mut self,
        qualified: &str,
        type_params: &[parser::ast::TypeParam<'compiler>],
        args: &Output<'compiler>,
        body: Option<&Output<'compiler>>,
        source_name: &str,
    ) {
        let Some(body) = body else {
            return;
        };
        if type_params.is_empty() || self.mono_plan.is_empty() {
            return;
        }

        let specializations = self
            .mono_plan
            .specializations_for_fn(qualified)
            .cloned()
            .collect::<Vec<_>>();
        if specializations.is_empty() {
            return;
        }

        for specialization in specializations {
            if self.mono_offsets.contains_key(&specialization.key) {
                continue;
            }

            let overrides = self.mono_overrides_for_args(type_params, args, &specialization.key);
            if overrides.is_empty() {
                continue;
            }

            let mono_name = format!(
                "{}$mono${}",
                qualified,
                specialization.key.subst.join("$").replace(' ', "")
            );
            let (clone_offset, _) = self.bind_function_entry(mono_name);
            self.mono_offsets
                .insert(specialization.key.clone(), clone_offset);

            let prev_fn_vars = std::mem::take(&mut self.context.variables);
            let prev_fn_polyfn_vars = std::mem::take(&mut self.polyfn_vars);
            let prev_fn_polyfn_sources = std::mem::take(&mut self.polyfn_sources);
            let prev_result_mode = self.compiling_result_mode;
            self.context.variables = Interner::default();
            self.compiling_result_mode = self.checker.fn_is_result_mode(source_name);
            self.mono_codegen_var_types.push(overrides);

            let prev_fn_defers = std::mem::take(&mut self.fn_defers);
            let mut a = self.do_compile(args);
            self.bytecode.append(&mut a);
            let body_op_start = self.bytecode.ops().len();
            let prev_field_keys = std::mem::take(&mut self.field_key_slots);
            self.emit_field_key_prologue(body);
            let mut c = self.do_compile(body);
            self.bytecode.append(&mut c);

            if !self.region_ends_with_return(body_op_start) {
                self.emit_fallthrough_return(source_name, body.0);
            }

            self.fn_defers = prev_fn_defers;
            self.mono_codegen_var_types.pop();
            self.compiling_result_mode = prev_result_mode;
            self.field_key_slots = prev_field_keys;
            self.context.variables = prev_fn_vars;
            self.polyfn_vars = prev_fn_polyfn_vars;
            self.polyfn_sources = prev_fn_polyfn_sources;
        }
    }

    fn mono_overrides_for_args<'compiler>(
        &self,
        type_params: &[parser::ast::TypeParam<'compiler>],
        args: &Output<'compiler>,
        key: &MonoKey,
    ) -> HashMap<String, Ty> {
        let mut type_param_tys = HashMap::new();
        for (idx, tp) in type_params.iter().enumerate() {
            if let Some(ty_name) = key.subst.get(idx) {
                type_param_tys.insert(tp.name, Self::mono_ty_from_name(ty_name));
            }
        }

        let mut overrides = HashMap::new();
        if let Expression::Fragment(children) = args.1.as_ref() {
            for child in children {
                if let Expression::Argument {
                    ty,
                    name,
                    is_rest,
                    ..
                } = child.1.as_ref()
                    && let Some(ty) = ty
                    && let Expression::Type(tp_name) | Expression::Identifier(tp_name) =
                        ty.1.as_ref()
                    && let Some(concrete) = type_param_tys.get(tp_name)
                {
                    // Rest formals are packed arrays at runtime (`MakeArray`).
                    let ty = if *is_rest {
                        crate::typechecking::ty::array(concrete.clone())
                    } else {
                        concrete.clone()
                    };
                    overrides.insert(name.to_string(), ty);
                }
            }
        }
        overrides
    }

    fn mono_call_offset(&self, fn_name: &str, args: Option<&Vec<Output<'_>>>) -> Option<usize> {
        let args = args?;
        // Keep keying in sync with `crate::monomorphize::candidate_for_call`: one
        // ground type per formal, with rest contributing its *element* type.
        let (fixed, rest, pack_rest) = self.split_call_args_for_rest(fn_name, args);
        let mut arg_types = Vec::with_capacity(fixed.len() + usize::from(pack_rest));
        for arg in &fixed {
            arg_types.push(crate::monomorphize::ground_type_name(&self.checker, arg)?);
        }
        if pack_rest {
            if rest.is_empty() {
                // Empty rest: only match when a fixed formal already pinned T.
                // Without a rest element we can't invent a ground type here;
                // specializations with empty rest still key the rest slot from
                // subst — look up by trying each specialization's arg_types
                // prefix match below via exact equality, so require the rest
                // element type from the first fixed arg that shares the rest
                // type param. Fallback: skip mono (shared body).
                return None;
            }
            let elem = crate::monomorphize::ground_type_name(&self.checker, &rest[0])?;
            for arg in rest.iter().skip(1) {
                if crate::monomorphize::ground_type_name(&self.checker, arg)? != elem {
                    return None;
                }
            }
            arg_types.push(elem);
        }
        let spec = self
            .mono_plan
            .specialization_for_call(fn_name, &arg_types)?;
        self.mono_offsets.get(&spec.key).copied()
    }

    fn consume_function_signature_output<'compiler>(&mut self, method: &Output<'compiler>) {
        let _method_id = self.next_emit_id();
        if let Expression::Function { args, body, .. } = method.1.as_ref() {
            let mut args_bc = self.do_compile(args);
            self.bytecode.append(&mut args_bc);
            if let Some(body) = body {
                let mut body_bc = self.do_compile(body);
                self.bytecode.append(&mut body_bc);
            }
        } else {
            let mut bc = self.do_compile(method);
            self.bytecode.append(&mut bc);
        }
    }

    pub fn get_messages(&self) -> &Vec<Message> {
        &self.messages
    }

    /// Append a diagnostic produced outside the typechecker/codegen
    /// path (e.g. pipeline discovery parse errors). Callers that also
    /// emit via the reporting sink must bump their own
    /// `messages_emitted` cursor so [`Pipeline::emit_new_messages`]
    /// does not re-forward the same message.
    pub fn push_message(&mut self, message: Message) {
        self.messages.push(message);
    }

    pub fn c_structs(&self) -> &[CStructDef] {
        self.checker.c_structs()
    }

    pub fn register(&mut self, name: &str, params: &[Ty], returns: &Ty) -> &mut Self {
        let idx = self.native.len();
        self.native.insert(name.to_string(), idx);
        self.checker.register_native(name, params, returns);

        self
    }

    /// Bind a host-native name to a stable id for [`Instruction::HostInvoke`]
    /// without inserting a type into the HM env (virtual `io::*` schemes
    /// are bound via `use` instead).
    pub fn register_native_id(&mut self, name: &str, id: usize) {
        self.native.insert(name.to_string(), id);
    }

    /// Look up a registered host-native id by export name.
    pub fn native_id(&self, name: &str) -> Option<usize> {
        self.native.get(name).copied()
    }

    fn resolve_variable<'compiler>(
        &self,
        variable: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> String {
        match variable.1.borrow() {
            Expression::Identifier(n) => n.to_string(),
            _ => String::new(),
        }
    }

    /// Like `resolve_variable`, but records a diagnostic when the
    /// expression is not an identifier (replaces the old `todo!`).
    fn resolve_variable_checked<'compiler>(
        &mut self,
        variable: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> String {
        match variable.1.borrow() {
            Expression::Identifier(n) => n.to_string(),
            other => {
                let span = variable.0;
                let mut m = Message::error(
                    ErrorCode::InvalidAssignment,
                    "Cannot use this expression as a variable name".to_string(),
                    span.into_range(),
                );
                m.push(DiagLabel::new(
                    format!("expected an identifier, found `{other}`"),
                    span.into_range(),
                ));
                self.messages.push(m);
                String::new()
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
        let lhs_ty = self.codegen_expr_ty(lhs);
        let lhs_id = self.checker.id_table().ids().get(self.emit_idx).copied();
        bytecode.append(&mut self.do_compile(lhs));
        bytecode.append(&mut self.do_compile(rhs));
        if matches!(
            lhs_ty,
            Some(crate::typechecking::ty::Ty::Con(ref name))
                if name == crate::typechecking::ty::FLOAT
        ) {
            return true;
        }
        matches!(
            lhs_id.and_then(|id| self.checker.lookup_at(id)),
            Some(crate::typechecking::ty::Ty::Con(ref name))
                if name == crate::typechecking::ty::FLOAT
        )
    }

    /// Return `true` when the *next* node to be emitted has an open type variable
    /// type (i.e. it is a generic type parameter, not a concrete `int`/`float`/…).
    /// Used to choose `DynAdd`/`DynSub`/… over `ADD`/`ADDF`/… for generic bodies.
    #[allow(dead_code)] // kept for potential future use alongside operand_is_open_ty
    fn is_generic_ty_at_current_idx(&self) -> bool {
        let Some(id) = self.checker.id_table().ids().get(self.emit_idx).copied() else {
            return false;
        };
        matches!(
            self.checker.lookup_at(id),
            Some(crate::typechecking::ty::Ty::Var(_))
        )
    }

    /// Return `true` when `operand` resolves to an open type variable — i.e.
    /// the operand is a generic type parameter that is not yet concrete.
    ///
    /// Handles `Expression::Identifier` via span-preferring [`Self::codegen_ident_ty`].
    /// All other expression shapes return `false` conservatively; they are either
    /// concrete literals or sub-expressions whose containing `Identifier` was
    /// already flagged on the inner call.
    fn operand_is_open_ty(&self, operand: &Output) -> bool {
        match operand.1.as_ref() {
            Expression::Identifier(_) => match self.codegen_ident_ty(operand) {
                Some(ty) => matches!(ty, Ty::Var(_)),
                None => false,
            },
            _ => false,
        }
    }

    fn alloc_temp_slot(&mut self) -> u32 {
        self.temp_counter += 1;
        // Operand stack and locals share one buffer. A `CONST` left on
        // the stack by `emit_host_native_invoke` (native id before args)
        // occupies index `variables.len()` without being interned. If we
        // `StorePop` into that index from `new Class(...)`, we overwrite
        // the id and `HostInvoke` sees a heap address instead.
        let min_slot = self.context.variables.len() as u32 + self.expr_depth;
        while (self.context.variables.len() as u32) < min_slot {
            let pad = format!("__pad{}", self.context.variables.len());
            let _ = self.context.variables.intern(pad);
        }
        let name = format!("__tmp{}", self.temp_counter);
        self.context.variables.intern(name) as u32
    }

    /// Synthesize the packed rest-array type for a call's trailing args
    /// (`[T]` / `[T; N]`), mirroring typechecker `infer_and_reorder_call_args`.
    fn synthesize_rest_array_ty(&self, rest: &[Output<'_>]) -> crate::typechecking::Ty {
        use crate::typechecking::ty::{array, array_fixed};
        let mut elem: Option<crate::typechecking::Ty> = None;
        for arg in rest {
            if let Some(t) = self.codegen_expr_ty(arg) {
                match &elem {
                    None => elem = Some(t),
                    Some(prev) if prev != &t => {
                        // Prefer the first element type; unify already ran in TC.
                    }
                    _ => {}
                }
            }
        }
        let element = elem.unwrap_or_else(crate::typechecking::ty::int);
        if rest.is_empty() {
            array(element)
        } else {
            array_fixed(element, rest.len())
        }
    }

    /// Bind names from an irrefutable `let` pattern by reading from
    /// `src_slot` (tuple via `Index`, record via `GetField`).
    fn emit_let_pattern_binds(
        &mut self,
        pattern: &parser::ast::LetPattern<'_>,
        src_slot: u32,
        bytecode: &mut Vec<Byte>,
    ) {
        use parser::ast::LetPattern;
        match pattern {
            LetPattern::Wildcard => {}
            LetPattern::Binding { name } => {
                bytecode.push_load(src_slot);
                let slot = self.alloc_binding_slot(name);
                bytecode.push_store_pop(slot);
            }
            LetPattern::Tuple(parts) => {
                for (idx, part) in parts.iter().enumerate() {
                    match part {
                        LetPattern::Wildcard => {}
                        LetPattern::Binding { name } => {
                            bytecode.push_load(src_slot);
                            bytecode.push_const(idx as i32);
                            bytecode.push_index();
                            let slot = self.alloc_binding_slot(name);
                            bytecode.push_store_pop(slot);
                        }
                        nested @ (LetPattern::Tuple(_) | LetPattern::Record(_)) => {
                            bytecode.push_load(src_slot);
                            bytecode.push_const(idx as i32);
                            bytecode.push_index();
                            let nested_slot = self.alloc_temp_slot();
                            bytecode.push_store_pop(nested_slot);
                            self.emit_let_pattern_binds(nested, nested_slot, bytecode);
                        }
                    }
                }
            }
            LetPattern::Record(fields) => {
                for pf in fields {
                    match &pf.pattern {
                        LetPattern::Wildcard => {}
                        LetPattern::Binding { name } => {
                            bytecode.push_load(src_slot);
                            self.emit_raw_string_literal(bytecode, pf.name);
                            bytecode.push_get_field();
                            let slot = self.alloc_binding_slot(name);
                            bytecode.push_store_pop(slot);
                        }
                        nested @ (LetPattern::Tuple(_) | LetPattern::Record(_)) => {
                            bytecode.push_load(src_slot);
                            self.emit_raw_string_literal(bytecode, pf.name);
                            bytecode.push_get_field();
                            let nested_slot = self.alloc_temp_slot();
                            bytecode.push_store_pop(nested_slot);
                            self.emit_let_pattern_binds(nested, nested_slot, bytecode);
                        }
                    }
                }
            }
        }
    }

    /// `for x in` over an array already on the operand stack (or just
    /// compiled). Observationally identical to `ArrayIter::next`.
    ///
    /// Layout: StorePop arr; idx=0; [top] idx < len → else exit;
    /// `x = arr[idx]`; body; [continue] idx++; JMP top; [exit].
    fn emit_for_in_array_loop(
        &mut self,
        body: &Output<'_>,
        binding_name: &str,
        array_already_on_stack: bool,
        iterable: Option<&Output<'_>>,
    ) {
        let arr_slot = self.alloc_temp_slot();
        let idx_slot = self.alloc_temp_slot();
        if !array_already_on_stack {
            let iter_bc = self.do_compile(iterable.expect("iterable required when not on stack"));
            self.bytecode.extend(iter_bc);
        }
        self.bytecode.push_store_pop(arr_slot);
        self.bytecode.push_const(0);
        self.bytecode.push_store_pop(idx_slot);

        // Hoist ArrayLen once — the array slot is not mutated by for-in.
        let len_slot = self.alloc_temp_slot();
        self.bytecode.push_load(arr_slot);
        self.bytecode.push(Byte::new(Instruction::ArrayLen));
        self.bytecode.push_store_pop(len_slot);

        // Consume binding Identifier NodeId (iterable → binding → body).
        let _ = self.next_emit_id();
        let binding_slot = self.alloc_binding_slot(binding_name);

        let mut bb = BlockBuilder::new();
        let top_label = bb.fresh_label(self.bytecode.il_mut());
        let continue_label = bb.fresh_label(self.bytecode.il_mut());
        let exit_label = bb.fresh_label(self.bytecode.il_mut());
        bb.bind_label(top_label, self.bytecode.il_mut());

        // cond: idx < len  (LE is `<`)
        self.bytecode.push_load(idx_slot);
        self.bytecode.push_load(len_slot);
        self.bytecode.push(Byte::new(Instruction::LE));
        bb.emit_jump_to(exit_label, BbJumpKind::JumpIfFalse, self.bytecode.il_mut());

        // x = arr[idx]
        self.bytecode.push_load(arr_slot);
        self.bytecode.push_load(idx_slot);
        self.bytecode.push_index();
        self.bytecode.push_store_pop(binding_slot);

        self.loop_stack.push((continue_label, exit_label));
        self.loop_bbs.push(bb);
        let body_bc = self.do_compile(body);
        self.bytecode.extend(body_bc);
        let mut bb = self
            .loop_bbs
            .pop()
            .expect("loop builder stack balanced for for-in array");
        self.loop_stack
            .pop()
            .expect("loop label stack balanced for for-in array");
        bb.bind_label(continue_label, self.bytecode.il_mut());
        // idx = idx + 1
        self.bytecode.push_load(idx_slot);
        self.bytecode.push_const(1);
        self.bytecode.push(Byte::new(Instruction::ADD));
        self.bytecode.push_store_pop(idx_slot);

        bb.emit_jump_to(top_label, BbJumpKind::Unconditional, self.bytecode.il_mut());
        bb.bind_label(exit_label, self.bytecode.il_mut());
        bb.finalize()
            .expect("BlockBuilder::finalize: for-in array labels bound");
    }

    /// Homogeneous tuple → temp `[A; N]` via Index, then array for-in.
    fn emit_for_in_tuple(
        &mut self,
        iterable: &Output<'_>,
        body: &Output<'_>,
        binding_name: &str,
        arity: usize,
    ) {
        let tup_slot = self.alloc_temp_slot();
        let iter_bc = self.do_compile(iterable);
        self.bytecode.extend(iter_bc);
        self.bytecode.push_store_pop(tup_slot);
        for i in 0..arity {
            self.bytecode.push_load(tup_slot);
            self.bytecode.push_const(i as i32);
            self.bytecode.push_index();
        }
        self.bytecode.push_make_array(arity as u32);
        self.emit_for_in_array_loop(body, binding_name, true, None);
    }

    /// Dict → `DictEntries` → array of `(string, V)` pairs → array for-in.
    fn emit_for_in_dict(&mut self, iterable: &Output<'_>, body: &Output<'_>, binding_name: &str) {
        let iter_bc = self.do_compile(iterable);
        self.bytecode.extend(iter_bc);
        self.bytecode.push(Byte::new(Instruction::DictEntries));
        self.emit_for_in_array_loop(body, binding_name, true, None);
    }

    /// Lazy range for-in (`int`/`byte`/`float`).
    ///
    /// Fast path when the iterable is a `Range` literal: locals for
    /// `cur`/`end` only — no heap. First-class range values
    /// (`let r = 0..n; for x in r`) are dicts `{start,end,inclusive}`
    /// unpacked via `GetField`.
    ///
    /// `float` selects LEF/LEQF/ADDF with step `1.0`; otherwise LE/LEQ/ADD
    /// with step `1` (shared by `int` and `byte`).
    fn emit_for_in_range(
        &mut self,
        iterable: &Output<'_>,
        body: &Output<'_>,
        binding_name: &str,
        inclusive: bool,
        float: bool,
    ) {
        if !float {
            if let Expression::Range { start, end, .. } = iterable.1.as_ref()
                && !crate::const_fold::body_has_loop_control(body)
            {
                if let Some(trips) = crate::const_fold::range_trip_count(start, end, inclusive) {
                    let _ = self.next_emit_id();
                    let binding_slot = self.alloc_binding_slot(binding_name);
                    let _ = self.next_emit_id();
                    if let Some(ConstValue::Int(s)) = crate::const_fold::eval_expr(start, self.const_env())
                    {
                        for k in 0..trips {
                            let val = s + k as i64;
                            let mut trip_bc = Vec::new();
                            self.emit_const_value(&ConstValue::Int(val), &mut trip_bc);
                            trip_bc.push_store_pop(binding_slot);
                            let mut body_bc = self.do_compile(body);
                            trip_bc.append(&mut body_bc);
                            self.bytecode.append(&mut trip_bc);
                        }
                        return;
                    }
                }
            }
        }

        let cur_slot = self.alloc_temp_slot();
        let end_slot = self.alloc_temp_slot();

        match iterable.1.as_ref() {
            Expression::Range { start, end, .. } => {
                // Consume the Range node's ID (pre-walk: Range → start → end).
                let _ = self.next_emit_id();
                let start_bc = self.do_compile(start);
                self.bytecode.extend(start_bc);
                self.bytecode.push_store_pop(cur_slot);
                let end_bc = self.do_compile(end);
                self.bytecode.extend(end_bc);
                self.bytecode.push_store_pop(end_slot);
            }
            _ => {
                let range_slot = self.alloc_temp_slot();
                let iter_bc = self.do_compile(iterable);
                self.bytecode.extend(iter_bc);
                self.bytecode.push_store_pop(range_slot);

                self.bytecode.push_load(range_slot);
                let start_idx = self.intern_string("start");
                self.bytecode.push_string(start_idx);
                self.bytecode.push_get_field();
                self.bytecode.push_store_pop(cur_slot);

                self.bytecode.push_load(range_slot);
                let end_idx = self.intern_string("end");
                self.bytecode.push_string(end_idx);
                self.bytecode.push_get_field();
                self.bytecode.push_store_pop(end_slot);
            }
        }

        // Consume binding Identifier NodeId (iterable → binding → body).
        let _ = self.next_emit_id();
        let binding_slot = self.alloc_binding_slot(binding_name);

        let mut bb = BlockBuilder::new();
        let top_label = bb.fresh_label(self.bytecode.il_mut());
        let continue_label = bb.fresh_label(self.bytecode.il_mut());
        let exit_label = bb.fresh_label(self.bytecode.il_mut());
        bb.bind_label(top_label, self.bytecode.il_mut());

        // cond: cur < end  (half-open) or cur <= end (inclusive)
        self.bytecode.push_load(cur_slot);
        self.bytecode.push_load(end_slot);
        self.bytecode.push(Byte::new(if float {
            if inclusive {
                Instruction::LEQF
            } else {
                Instruction::LEF
            }
        } else if inclusive {
            Instruction::LEQ
        } else {
            Instruction::LE
        }));
        bb.emit_jump_to(exit_label, BbJumpKind::JumpIfFalse, self.bytecode.il_mut());

        // x = cur
        self.bytecode.push_load(cur_slot);
        self.bytecode.push_store_pop(binding_slot);

        self.loop_stack.push((continue_label, exit_label));
        self.loop_bbs.push(bb);
        let body_bc = self.do_compile(body);
        self.bytecode.extend(body_bc);
        let mut bb = self
            .loop_bbs
            .pop()
            .expect("loop builder stack balanced for for-in range");
        self.loop_stack
            .pop()
            .expect("loop label stack balanced for for-in range");
        bb.bind_label(continue_label, self.bytecode.il_mut());
        // cur = cur + 1  (or + 1.0 for float)
        self.bytecode.push_load(cur_slot);
        if float {
            let bits = Value::from(1.0_f64).raw() as u64;
            let idx = self.intern_constant(bits);
            self.bytecode.push_const_pool(idx);
            self.bytecode.push(Byte::new(Instruction::ADDF));
        } else {
            self.bytecode.push_const(1);
            self.bytecode.push(Byte::new(Instruction::ADD));
        }
        self.bytecode.push_store_pop(cur_slot);

        bb.emit_jump_to(top_label, BbJumpKind::Unconditional, self.bytecode.il_mut());
        bb.bind_label(exit_label, self.bytecode.il_mut());
        bb.finalize()
            .expect("BlockBuilder::finalize: for-in range labels bound");
    }

    /// Coroutine for-in: resume → bind; skip body when `done` (completion
    /// value excluded). Same layout as the Phase CORO for-in path.
    fn emit_for_in_coro(&mut self, iterable: &Output<'_>, body: &Output<'_>, binding_name: &str) {
        let handle_slot = self.alloc_temp_slot();
        let iter_bc = self.do_compile(iterable);
        self.bytecode.extend(iter_bc);
        self.bytecode.push_store_pop(handle_slot);

        let _ = self.next_emit_id();
        let binding_slot = self.alloc_binding_slot(binding_name);

        let mut bb = BlockBuilder::new();
        let top_label = bb.fresh_label(self.bytecode.il_mut());
        let exit_label = bb.fresh_label(self.bytecode.il_mut());
        bb.bind_label(top_label, self.bytecode.il_mut());

        self.bytecode.push_load(handle_slot);
        self.bytecode
            .push(Byte::new(Instruction::ResumeCoro).with_operand_u32(0));
        self.bytecode.push_store_pop(binding_slot);

        self.bytecode.push_load(handle_slot);
        self.bytecode.push(Byte::new(Instruction::DoneCoro));
        self.bytecode.push(Byte::new(Instruction::LogNot));
        bb.emit_jump_to(exit_label, BbJumpKind::JumpIfFalse, self.bytecode.il_mut());

        self.loop_stack.push((top_label, exit_label));
        self.loop_bbs.push(bb);
        let body_bc = self.do_compile(body);
        self.bytecode.extend(body_bc);
        let mut bb = self
            .loop_bbs
            .pop()
            .expect("loop builder stack balanced for for-in coro");
        self.loop_stack
            .pop()
            .expect("loop label stack balanced for for-in coro");

        bb.emit_jump_to(top_label, BbJumpKind::Unconditional, self.bytecode.il_mut());
        bb.bind_label(exit_label, self.bytecode.il_mut());
        bb.finalize()
            .expect("BlockBuilder::finalize: for-in coro labels bound");
    }

    /// User `IntoIterator` / `Iterator`: `into_iter` then `next` → Option.
    ///
    /// Trait instance methods unbox type-parameter args in their prologue
    /// (`ValueTag::Instance` for classes), so call sites must `BoxValue`
    /// the carrier before `CallIndirect`.
    fn emit_for_in_custom(
        &mut self,
        iterable: &Output<'_>,
        body: &Output<'_>,
        binding_name: &str,
        into_iter_fqn: &str,
        next_fqn: &str,
    ) {
        let into_off = self.functions.get(into_iter_fqn).copied().unwrap_or(0) as u32;
        let next_off = self.functions.get(next_fqn).copied().unwrap_or(0) as u32;
        let none_tag = self
            .checker
            .tag_for(common::BUILTIN_OPTION_ENUM, "None")
            .unwrap_or(0);
        let carrier_tag = ValueTag::Instance as u32;

        let it_slot = self.alloc_temp_slot();
        let iter_bc = self.do_compile(iterable);
        self.bytecode.extend(iter_bc);
        self.bytecode.push_box_value(carrier_tag);
        Self::emit_call_indirect(&mut self.bytecode, into_off, 1);
        self.bytecode.push_store_pop(it_slot);

        let _ = self.next_emit_id();
        let binding_slot = self.alloc_binding_slot(binding_name);

        let mut bb = BlockBuilder::new();
        let top_label = bb.fresh_label(self.bytecode.il_mut());
        let exit_label = bb.fresh_label(self.bytecode.il_mut());
        bb.bind_label(top_label, self.bytecode.il_mut());

        self.bytecode.push_load(it_slot);
        self.bytecode.push_box_value(carrier_tag);
        Self::emit_call_indirect(&mut self.bytecode, next_off, 1);

        // `Option::None` → exit (JumpIfMatch pops unit None).
        bb.emit_jump_to(
            exit_label,
            BbJumpKind::JumpIfMatch {
                tag: none_tag,
                arity: 0,
            },
            self.bytecode.il_mut(),
        );
        // Fall-through: Some(v) — unpack payload into binding.
        self.bytecode
            .push(Byte::new(Instruction::Unpack).with_operand_u32(1));
        self.bytecode.push_store_pop(binding_slot);

        self.loop_stack.push((top_label, exit_label));
        self.loop_bbs.push(bb);
        let body_bc = self.do_compile(body);
        self.bytecode.extend(body_bc);
        let mut bb = self
            .loop_bbs
            .pop()
            .expect("loop builder stack balanced for for-in custom");
        self.loop_stack
            .pop()
            .expect("loop label stack balanced for for-in custom");

        bb.emit_jump_to(top_label, BbJumpKind::Unconditional, self.bytecode.il_mut());
        bb.bind_label(exit_label, self.bytecode.il_mut());
        bb.finalize()
            .expect("BlockBuilder::finalize: for-in custom labels bound");
    }

    fn emit_field_name(&mut self, bytecode: &mut impl EmitBuf, field: &str) {
        if let Some(&slot) = self.field_key_slots.get(field) {
            bytecode.push_load(slot);
            return;
        }
        self.emit_raw_string_literal(bytecode, field);
    }

    /// Count GetField/SetField string-key uses in `node` (Access / OptionalAccess).
    fn count_field_key_uses(node: &Output<'_>, counts: &mut HashMap<String, u32>) {
        use Expression::*;
        match node.1.as_ref() {
            Access(recv, field) | OptionalAccess(recv, field) => {
                *counts.entry((*field).to_string()).or_insert(0) += 1;
                Self::count_field_key_uses(recv, counts);
            }
            CompoundAssign(target, _, rhs) | Assignment(target, rhs) => {
                Self::count_field_key_uses(target, counts);
                Self::count_field_key_uses(rhs, counts);
            }
            Adjust { target, .. } => Self::count_field_key_uses(target, counts),
            Negate(e)
            | Not(e)
            | LogicalNot(e)
            | Positive(e)
            | Return(e)
            | ImplicitReturn(e)
            | Raise(e)
            | Panic(e)
            | Yield(e)
            | YieldFrom(e)
            | Try(e)
            | Expr(e)
            | Group(e)
            | ExprStatement(e)
            | Statement(e)
            | Readonly(e)
            | Noop(e)
            | Dload(e)
            | Done(e)
            | Spread(e)
            | NamedArg(_, e)
            | Member(e)
            | Method(_, e)
            | Constant(e, _)
            | Variable(_, Some(e)) => Self::count_field_key_uses(e, counts),
            Variable(_, None) => {}
            Resume(e, Some(v)) | Coalesce(e, v) | Cast(e, v) | Index(e, Some(v)) => {
                Self::count_field_key_uses(e, counts);
                Self::count_field_key_uses(v, counts);
            }
            Resume(e, None) | Index(e, None) => Self::count_field_key_uses(e, counts),
            Add(a, b)
            | Sub(a, b)
            | Mul(a, b)
            | Div(a, b)
            | Mod(a, b)
            | Pow(a, b)
            | Shl(a, b)
            | Shr(a, b)
            | Xor(a, b)
            | And(a, b)
            | BitAnd(a, b)
            | Or(a, b)
            | BitOr(a, b)
            | Eq(a, b)
            | Neq(a, b)
            | Leq(a, b)
            | Geq(a, b)
            | Le(a, b)
            | Gt(a, b)
            | TypeFun(a, b) => {
                Self::count_field_key_uses(a, counts);
                Self::count_field_key_uses(b, counts);
            }
            Range { start, end, .. } => {
                Self::count_field_key_uses(start, counts);
                Self::count_field_key_uses(end, counts);
            }
            List(v) | Array(v) | Fragment(v) | Block(v) | Program(v) | Tuple(v) | If(v)
            | Declare(v) | Invoke(v) => {
                for c in v {
                    Self::count_field_key_uses(c, counts);
                }
            }
            Dict(fields) => {
                for f in fields {
                    Self::count_field_key_uses(&f.value, counts);
                }
            }
            Branch(cond, body) => {
                if let Some(c) = cond {
                    Self::count_field_key_uses(c, counts);
                }
                Self::count_field_key_uses(body, counts);
            }
            Call { name, args } => {
                Self::count_field_key_uses(name, counts);
                if let Some(as_) = args {
                    for a in as_ {
                        Self::count_field_key_uses(a, counts);
                    }
                }
            }
            For {
                init,
                cond,
                step,
                body,
            } => {
                if let Some(i) = init {
                    Self::count_field_key_uses(i, counts);
                }
                Self::count_field_key_uses(cond, counts);
                if let Some(s) = step {
                    Self::count_field_key_uses(s, counts);
                }
                Self::count_field_key_uses(body, counts);
            }
            Loop {
                identifier,
                iterable,
                body,
            } => {
                if let Some(id) = identifier {
                    Self::count_field_key_uses(id, counts);
                }
                Self::count_field_key_uses(iterable, counts);
                Self::count_field_key_uses(body, counts);
            }
            LetDestructure { rhs, .. } => Self::count_field_key_uses(rhs, counts),
            Defer { body, .. } | Lambda { body, .. } | TestCase { body, .. } => {
                Self::count_field_key_uses(body, counts);
            }
            Function { body: Some(b), .. } => Self::count_field_key_uses(b, counts),
            Function { body: None, .. } => {}
            Instantiate(recv, args) => {
                Self::count_field_key_uses(recv, counts);
                if let Some(as_) = args {
                    for a in as_ {
                        Self::count_field_key_uses(a, counts);
                    }
                }
            }
            Match { scrutinee, arms } => {
                Self::count_field_key_uses(scrutinee, counts);
                for arm in arms {
                    Self::count_field_key_uses(&arm.body, counts);
                }
            }
            Construct { fields, .. } => match fields {
                parser::ast::EnumConstructPayload::Tuple(parts) => {
                    for p in parts {
                        Self::count_field_key_uses(p, counts);
                    }
                }
                parser::ast::EnumConstructPayload::Record(fs) => {
                    for f in fs {
                        Self::count_field_key_uses(&f.value, counts);
                    }
                }
                parser::ast::EnumConstructPayload::Unit => {}
            },
            StaticDecl { init, .. } => Self::count_field_key_uses(init, counts),
            Field { init: Some(i), .. } => Self::count_field_key_uses(i, counts),
            // Type-only / declaration / leaf nodes — no runtime field keys.
            _ => {}
        }
    }

    /// Materialize field-name strings used ≥2 times into temp slots at fn entry.
    fn emit_field_key_prologue(&mut self, body: &Output<'_>) {
        let mut counts = HashMap::new();
        Self::count_field_key_uses(body, &mut counts);
        let mut keys: Vec<String> = counts
            .into_iter()
            .filter(|(_, n)| *n >= 2)
            .map(|(k, _)| k)
            .collect();
        keys.sort();
        self.field_key_slots.clear();
        for key in keys {
            let slot = self.alloc_temp_slot();
            let idx = self.intern_string(&key);
            self.bytecode.push_string(idx);
            self.bytecode.push_store_pop(slot);
            self.field_key_slots.insert(key, slot);
        }
    }

    fn emit_raw_string_literal(&mut self, bytecode: &mut impl EmitBuf, value: &str) {
        self.push_string_literal(bytecode, value);
    }

    fn variable_slot(&mut self, name: &str) -> Option<u32> {
        self.lookup_slot(name)
    }

    fn is_float_ty(&self, _node: &Output) -> bool {
        if matches!(
            self.codegen_ident_ty(_node),
            Some(crate::typechecking::ty::Ty::Con(ref ty))
                if ty == crate::typechecking::ty::FLOAT
        ) {
            return true;
        }
        let Some(id) = self.checker.id_table().ids().get(self.emit_idx).copied() else {
            return false;
        };
        matches!(
            self.checker.lookup_at(id),
            Some(crate::typechecking::ty::Ty::Con(ref name))
                if name == crate::typechecking::ty::FLOAT
        )
    }

    fn is_string_expr(&self, node: &Output) -> bool {
        matches!(
            self.codegen_expr_ty(node),
            Some(Ty::Con(ref name)) if name == crate::typechecking::ty::STRING
        )
    }

    /// Static length from a value's type (fixed arrays, tuples, records).
    fn static_len_of(&self, node: &Output) -> Option<usize> {
        use crate::typechecking::ty::{ArrayLength, strip_readonly};
        let ty = self.codegen_expr_ty(node)?;
        let pruned = crate::typechecking::subst::apply_ty_prune(self.checker.subst(), &ty);
        match strip_readonly(&pruned) {
            Ty::Array {
                length: ArrayLength::Static(n),
                ..
            } => Some(*n),
            Ty::Tuple(elems) => Some(elems.len()),
            Ty::Record { fields } => Some(fields.len()),
            _ => None,
        }
    }

    /// Resolve an `impl Class<…>` type argument to a [`Ty`] for FQN mangling.
    /// Mirrors the typechecker's `parse_instance_head` so `Option<int>`
    /// becomes `App(Option, [int])`, not `unknown`.
    fn codegen_instance_head_ty(&self, arg: &Output) -> Ty {
        match arg.1.as_ref() {
            Expression::Type(name) | Expression::Identifier(name) => {
                match name.to_ascii_lowercase().as_str() {
                    "option" => Ty::Con(common::BUILTIN_OPTION_ENUM.into()),
                    "result" => Ty::Con(common::BUILTIN_RESULT_ENUM.into()),
                    "int" => Ty::Con("int".into()),
                    "float" => Ty::Con("float".into()),
                    "string" => Ty::Con("string".into()),
                    "bool" => Ty::Con("bool".into()),
                    "void" | "unit" => Ty::Con("unit".into()),
                    _ => Ty::Con(name.to_string()),
                }
            }
            Expression::TypeApp { name, args } => {
                let head = match name.to_ascii_lowercase().as_str() {
                    "option" => Ty::Con(common::BUILTIN_OPTION_ENUM.into()),
                    "result" => Ty::Con(common::BUILTIN_RESULT_ENUM.into()),
                    _ => Ty::Con(name.to_string()),
                };
                let arg_tys: Vec<Ty> = args
                    .iter()
                    .map(|a| self.codegen_instance_head_ty(a))
                    .collect();
                Ty::App(Box::new(head), arg_tys)
            }
            _ => self
                .codegen_expr_ty(arg)
                .unwrap_or_else(|| Ty::Con("unknown".into())),
        }
    }

    fn codegen_expr_ty(&self, node: &Output) -> Option<Ty> {
        let resolved = match node.1.as_ref() {
            Expression::NamedArg(_, value) => return self.codegen_expr_ty(value),
            Expression::Integer(_) => Some(Ty::Con(crate::typechecking::ty::INT.into())),
            Expression::Float(_) => Some(Ty::Con(crate::typechecking::ty::FLOAT.into())),
            Expression::Bool(_) => Some(Ty::Con(crate::typechecking::ty::BOOL.into())),
            Expression::String(_) => Some(Ty::Con(crate::typechecking::ty::STRING.into())),
            Expression::Tuple(items) => {
                let mut tys = Vec::with_capacity(items.len());
                for item in items {
                    tys.push(self.codegen_expr_ty(item)?);
                }
                Some(Ty::Tuple(tys))
            }
            Expression::Dict(fields) => {
                let mut tys = Vec::with_capacity(fields.len());
                for field in fields {
                    tys.push((field.name.to_string(), self.codegen_expr_ty(&field.value)?));
                }
                tys.sort_by(|a, b| a.0.cmp(&b.0));
                Some(Ty::Record { fields: tys })
            }
            Expression::Identifier(_) => self.codegen_ident_ty(node),
            // `Construct` / `Instantiate` must NOT collapse to a bare
            // `Ty::Con(name)`: generic apps like `Option::Some(42)` are
            // `Option<int>` (`Ty::App`), and call-site dictionary emission
            // keys off that full type (`Collect<Option<int>>`). Falling
            // through to `lookup_for_codegen_span` preserves the HM result.
            Expression::Instantiate(class, _) => match class.1.as_ref() {
                Expression::Identifier(name) | Expression::Type(name) => {
                    // Non-generic `new Class(...)` is exactly `Con(Class)`.
                    // Prefer the span cache when present (covers `Class<T>`).
                    self.checker
                        .lookup_for_codegen_span(node.0.start, node.0.end)
                        .or_else(|| Some(Ty::Con((*name).to_string())))
                }
                _ => None,
            },
            Expression::Add(lhs, rhs) if self.is_string_expr(lhs) && self.is_string_expr(rhs) => {
                Some(Ty::Con(crate::typechecking::ty::STRING.into()))
            }
            Expression::Access(receiver, field) => {
                let receiver_ty = self.receiver_type(receiver)?;
                if let Ty::Record { fields } = &receiver_ty {
                    return fields
                        .iter()
                        .find(|(name, _)| name == field)
                        .map(|(_, ty)| ty.clone());
                }
                if let Some(name) = Checker::class_name_of_ty(&receiver_ty) {
                    if self.checker.is_class(name) {
                        return self.codegen_class_field_ty(name, field, &receiver_ty);
                    }
                }
                extract_enum_name(&receiver_ty)
                    .and_then(|name| self.checker.field_type_for(&name, field))
            }
            Expression::OptionalAccess(receiver, field) => {
                use crate::typechecking::ty::{is_option_ty, option_inner, option_ty};
                let recv_ty = self.codegen_expr_ty(receiver)?;
                let inner = if is_option_ty(&recv_ty) {
                    option_inner(&recv_ty)?
                } else {
                    return None;
                };
                let field_ty = if let Ty::Record { fields } = &inner {
                    fields
                        .iter()
                        .find(|(name, _)| name == field)
                        .map(|(_, ty)| ty.clone())
                } else if let Some(name) = Checker::class_name_of_ty(&inner) {
                    if self.checker.is_class(name) {
                        self.codegen_class_field_ty(name, field, &inner)
                    } else {
                        extract_enum_name(&inner)
                            .and_then(|n| self.checker.field_type_for(&n, field))
                    }
                } else {
                    extract_enum_name(&inner).and_then(|n| self.checker.field_type_for(&n, field))
                }?;
                Some(option_ty(field_ty))
            }
            Expression::Expr(inner)
            | Expression::Group(inner)
            | Expression::Statement(inner)
            | Expression::ExprStatement(inner) => self.codegen_expr_ty(inner),
            _ => None,
        };
        resolved.or_else(|| {
            self.checker
                .lookup_for_codegen_span(node.0.start, node.0.end)
        })
    }

    fn binop_for_assign_op(op: parser::ast::AssignOp, is_float: bool) -> Instruction {
        use parser::ast::AssignOp;
        match (op, is_float) {
            (AssignOp::Add, false) => Instruction::ADD,
            (AssignOp::Add, true) => Instruction::ADDF,
            (AssignOp::Sub, false) => Instruction::SUB,
            (AssignOp::Sub, true) => Instruction::SUBF,
            (AssignOp::Mul, false) => Instruction::MUL,
            (AssignOp::Mul, true) => Instruction::MULF,
            (AssignOp::Div, false) => Instruction::DIV,
            (AssignOp::Div, true) => Instruction::DIVF,
            (AssignOp::Mod, false) => Instruction::MOD,
            (AssignOp::Mod, true) => Instruction::MODF,
            (AssignOp::Pow, false) => Instruction::Pow,
            (AssignOp::Pow, true) => Instruction::PowF,
            (AssignOp::Shl, _) => Instruction::SHL,
            (AssignOp::Shr, _) => Instruction::SHR,
            (AssignOp::BitAnd, _) => Instruction::BITAND,
            (AssignOp::BitOr, _) => Instruction::BITOR,
            (AssignOp::BitXor, _) => Instruction::XOR,
        }
    }

    fn emit_read_lvalue(&mut self, bytecode: &mut Vec<Byte>, target: &Output) -> bool {
        match target.1.as_ref() {
            Expression::Identifier(name) => {
                if let Some(slot) = self.variable_slot(name) {
                    bytecode.push_load(slot);
                    self.is_float_ty(target)
                } else {
                    false
                }
            }
            Expression::Access(receiver, field) => {
                bytecode.append(&mut self.do_compile(receiver));
                self.emit_field_name(bytecode, field);
                bytecode.push_get_field();
                matches!(
                    self.receiver_type(receiver),
                    Some(crate::typechecking::Ty::Con(ref n))
                        if n == crate::typechecking::ty::FLOAT
                )
            }
            Expression::Index(arr, Some(idx)) => {
                let tmp_arr = self.alloc_temp_slot();
                let tmp_idx = self.alloc_temp_slot();
                bytecode.append(&mut self.do_compile(arr));
                bytecode.push_store_pop(tmp_arr);
                bytecode.append(&mut self.do_compile(idx));
                bytecode.push_store_pop(tmp_idx);
                bytecode.push_load(tmp_arr);
                bytecode.push_load(tmp_idx);
                bytecode.push_index();
                false
            }
            Expression::Index(_, None) => false,
            _ => false,
        }
    }

    fn emit_write_lvalue(
        &mut self,
        bytecode: &mut Vec<Byte>,
        target: &Output,
        leave_value_on_stack: bool,
    ) {
        match target.1.as_ref() {
            Expression::Identifier(name) => {
                if let Some(slot) = self.variable_slot(name) {
                    if leave_value_on_stack {
                        bytecode.push(Byte::new(Instruction::DUPLICATE));
                    }
                    bytecode.push_store_pop(slot);
                }
            }
            Expression::Access(receiver, field) => {
                if leave_value_on_stack {
                    bytecode.push(Byte::new(Instruction::DUPLICATE));
                }
                bytecode.append(&mut self.do_compile(receiver));
                self.emit_field_name(bytecode, field);
                bytecode.push_set_field();
                // SetField leaves the value; caller uses leave_value_on_stack /
                // discard_statement_value to keep or POP.
            }
            Expression::Index(arr, Some(idx)) => {
                // Always stash the RHS — `StoreIndex` pops value/index/array.
                // Dropping with POP when `leave_value_on_stack == false` left
                // StoreIndex without a value (stack underflow / wrong write).
                let tmp_arr = self.alloc_temp_slot();
                let tmp_idx = self.alloc_temp_slot();
                let tmp_val = self.alloc_temp_slot();
                bytecode.push_store_pop(tmp_val);
                bytecode.append(&mut self.do_compile(arr));
                bytecode.push_store_pop(tmp_arr);
                bytecode.append(&mut self.do_compile(idx));
                bytecode.push_store_pop(tmp_idx);
                bytecode.push_load(tmp_arr);
                bytecode.push_load(tmp_idx);
                bytecode.push_load(tmp_val);
                bytecode.push(Byte::new(Instruction::StoreIndex));
                if leave_value_on_stack {
                    // StoreIndex leaves the value on the stack; keep it.
                } else {
                    bytecode.push_pop();
                }
            }
            Expression::Index(_, None) => {}
            _ => {
                bytecode.push_pop();
            }
        }
    }

    fn emit_compound_assign(
        &mut self,
        bytecode: &mut Vec<Byte>,
        self_id: Option<crate::typechecking::id::NodeId>,
        span_start: usize,
        span_end: usize,
        target: &Output,
        op: parser::ast::AssignOp,
        rhs: &Output,
    ) {
        if matches!(op, parser::ast::AssignOp::Add)
            && self.is_string_expr(target)
            && self.is_string_expr(rhs)
        {
            self.emit_raw_string_literal(bytecode, "%s%s");
            let _ = self.emit_read_lvalue(bytecode, target);
            bytecode.append(&mut self.do_compile(rhs));
            bytecode.push(Byte::new(Instruction::FORMAT).with_operand_u32(2));
            self.emit_write_lvalue(bytecode, target, false);
            return;
        }

        let agg_op = match op {
            parser::ast::AssignOp::Add => Some(crate::typechecking::AggregateOp::Add),
            parser::ast::AssignOp::Sub => Some(crate::typechecking::AggregateOp::Sub),
            parser::ast::AssignOp::Mul => Some(crate::typechecking::AggregateOp::Mul),
            parser::ast::AssignOp::Div => Some(crate::typechecking::AggregateOp::Div),
            parser::ast::AssignOp::Mod => Some(crate::typechecking::AggregateOp::Mod),
            parser::ast::AssignOp::Pow => Some(crate::typechecking::AggregateOp::Pow),
            _ => None,
        };
        if let Some(agg_op) = agg_op {
            let mut tmp = Vec::new();
            if self.try_emit_matrix_op(&mut tmp, self_id, span_start, span_end, target, Some(rhs)) {
                bytecode.append(&mut tmp);
                self.emit_write_lvalue(bytecode, target, false);
                return;
            }
            if self.try_emit_aggregate_arith(
                &mut tmp,
                self_id,
                span_start,
                span_end,
                target,
                Some(rhs),
                agg_op,
            ) {
                bytecode.append(&mut tmp);
                self.emit_write_lvalue(bytecode, target, false);
                return;
            }
        }

        if let Expression::Index(arr, Some(idx)) = target.1.as_ref() {
            let tmp_arr = self.alloc_temp_slot();
            let tmp_idx = self.alloc_temp_slot();
            bytecode.append(&mut self.do_compile(arr));
            bytecode.push_store_pop(tmp_arr);
            bytecode.append(&mut self.do_compile(idx));
            bytecode.push_store_pop(tmp_idx);
            bytecode.push_load(tmp_arr);
            bytecode.push_load(tmp_idx);
            bytecode.push_index();
            bytecode.append(&mut self.do_compile(rhs));
            bytecode.push(Byte::new(Self::binop_for_assign_op(op, false)));
            let tmp_val = self.alloc_temp_slot();
            bytecode.push_store_pop(tmp_val);
            bytecode.push_load(tmp_arr);
            bytecode.push_load(tmp_idx);
            bytecode.push_load(tmp_val);
            bytecode.push(Byte::new(Instruction::StoreIndex));
            return;
        }

        let is_float = self.emit_read_lvalue(bytecode, target);
        bytecode.append(&mut self.do_compile(rhs));
        bytecode.push(Byte::new(Self::binop_for_assign_op(op, is_float)));
        self.emit_write_lvalue(bytecode, target, false);
    }

    fn emit_adjust(
        &mut self,
        bytecode: &mut Vec<Byte>,
        target: &Output,
        op: parser::ast::AdjustOp,
        prefix: bool,
    ) {
        if let Expression::Identifier(name) = target.1.as_ref() {
            if let Some(slot) = self.variable_slot(name) {
                let is_float = self.is_float_ty(target);
                let instr = match op {
                    parser::ast::AdjustOp::Inc => Instruction::INC,
                    parser::ast::AdjustOp::Dec => Instruction::DEC,
                };
                bytecode.push(Byte::new(instr).with_inc_dec(slot, prefix, is_float));
                return;
            }
        }

        let delta: i64 = match op {
            parser::ast::AdjustOp::Inc => 1,
            parser::ast::AdjustOp::Dec => -1,
        };

        if let Expression::Index(arr, Some(idx)) = target.1.as_ref() {
            let tmp_arr = self.alloc_temp_slot();
            let tmp_idx = self.alloc_temp_slot();
            bytecode.append(&mut self.do_compile(arr));
            bytecode.push_store_pop(tmp_arr);
            bytecode.append(&mut self.do_compile(idx));
            bytecode.push_store_pop(tmp_idx);
            bytecode.push_load(tmp_arr);
            bytecode.push_load(tmp_idx);
            bytecode.push_index();
            let tmp_old = if !prefix {
                let t = self.alloc_temp_slot();
                bytecode.push_store_pop(t);
                bytecode.push_load(tmp_arr);
                bytecode.push_load(tmp_idx);
                bytecode.push_index();
                t
            } else {
                0
            };
            bytecode.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(delta).raw() as _,
            ));
            bytecode.push(Byte::new(Instruction::ADD));
            let tmp_val = self.alloc_temp_slot();
            bytecode.push_store_pop(tmp_val);
            bytecode.push_load(tmp_arr);
            bytecode.push_load(tmp_idx);
            bytecode.push_load(tmp_val);
            bytecode.push(Byte::new(Instruction::StoreIndex));
            if prefix {
                bytecode.push_load(tmp_val);
            } else {
                bytecode.push_load(tmp_old);
            }
            return;
        }

        let is_float = self.emit_read_lvalue(bytecode, target);
        let tmp_old = if !prefix {
            let tmp = self.alloc_temp_slot();
            bytecode.push_store_pop(tmp);
            tmp
        } else {
            0
        };
        if is_float {
            bytecode.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(delta as f64).raw() as _,
            ));
            bytecode.push(Byte::new(Instruction::ADDF));
        } else {
            bytecode.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(delta).raw() as _,
            ));
            bytecode.push(Byte::new(Instruction::ADD));
        }
        self.emit_write_lvalue(bytecode, target, false);
        if prefix {
            self.emit_read_lvalue(bytecode, target);
        } else {
            bytecode.push_load(tmp_old);
        }
    }

    fn qualify_static_fqn(&self, name: &str) -> String {
        if self.namespace.is_empty() {
            name.to_string()
        } else {
            format!("{}::{}", self.namespace, name)
        }
    }

    fn emit_static_initializer(&mut self, fqn: &str, init: &Output) {
        let Some(slot) = self.checker.static_slot_index(fqn) else {
            return;
        };
        if let Some(val) = crate::const_fold::eval_expr(init, self.const_env()) {
            if self.checker.is_static_const_fqn(fqn) {
                self.static_const_values.insert(fqn.to_string(), val);
            }
        }
        let mut init_bc = self.do_compile(init);
        self.static_init_bytecode.append(&mut init_bc);
        self.static_init_bytecode
            .push(Byte::new(Instruction::StoreStatic).with_operand_u32(slot));
    }

    /// Resolve enum name for field access via the codegen side-table.
    /// Receiver enum name for field-access codegen (kept for tests / callers).
    #[allow(dead_code)]
    fn enum_name_for_receiver(&mut self, receiver: &Output) -> Option<String> {
        // Cannot use infer cache inside function bodies (ID misalignment) or env (frame popped).
        let ty = self.receiver_type(receiver)?;
        extract_enum_name(&ty)
    }

    /// Receiver type for field access / method calls.
    ///
    /// Handles identifiers, chained access, parentheses/`Group` wrappers, and
    /// falls back to [`Self::codegen_expr_ty`] for forms like `new Class(...)`
    /// so `(self).field` and `(new C(...)).method()` resolve as class
    /// instances (not the LoadField/empty-owner miscompile path).
    fn receiver_type(&self, receiver: &Output) -> Option<Ty> {
        match receiver.1.as_ref() {
            Expression::Expr(inner)
            | Expression::Group(inner)
            | Expression::Statement(inner)
            | Expression::ExprStatement(inner) => self.receiver_type(inner),
            Expression::Identifier(_) => self.codegen_ident_ty(receiver),
            Expression::Access(inner, field) => {
                let inner_ty = self.receiver_type(inner)?;
                if let Some(name) = Checker::class_name_of_ty(&inner_ty) {
                    if self.checker.is_class(name) {
                        return self.codegen_class_field_ty(name, field, &inner_ty);
                    }
                }
                if let Some(name) = extract_enum_name(&inner_ty) {
                    return self.checker.field_type_for(&name, field);
                }
                if let Ty::Record { fields } = &inner_ty {
                    return fields
                        .iter()
                        .find(|(n, _)| n == field)
                        .map(|(_, t)| t.clone());
                }
                None
            }
            // `new Class(...)`, calls, etc. — reuse the general expr-type helper
            // (span cache / Instantiate Con) instead of treating the receiver
            // as unknown and emitting LoadField(0).
            _ => self.codegen_expr_ty(receiver),
        }
    }

    /// Class field type for codegen, substituting type args from `App`.
    fn codegen_class_field_ty(&self, class: &str, field: &str, receiver_ty: &Ty) -> Option<Ty> {
        use crate::typechecking::ty::subst_ty_params;
        let fty = self.checker.class_field_ty(class, field)?.clone();
        let params = self
            .checker
            .generics()
            .generic_type_ctors
            .get(class)
            .cloned()
            .unwrap_or_default();
        if params.is_empty() {
            return Some(fty);
        }
        let args = match receiver_ty {
            Ty::App(_, args) => args.clone(),
            _ => return Some(fty),
        };
        let mut map = std::collections::HashMap::new();
        for (p, a) in params.iter().zip(args.iter()) {
            map.insert(p.clone(), a.clone());
        }
        Some(subst_ty_params(&fty, &map))
    }

    /// True when `expr` is (or produces) the built-in `Option` sum.
    fn expr_is_option(&self, expr: &Output) -> bool {
        use crate::typechecking::ty::is_option_ty;
        match expr.1.as_ref() {
            Expression::Construct { enum_name, .. } => common::is_builtin_option_enum(enum_name),
            Expression::Group(inner) | Expression::Expr(inner) => self.expr_is_option(inner),
            _ => self
                .codegen_expr_ty(expr)
                .map(|t| is_option_ty(&t))
                .unwrap_or(false),
        }
    }

    /// Wrap the top-of-stack value as `Ok(v)` (Result) or `Some(v)` (Option).
    fn emit_ok_or_some_wrap(bytecode: &mut impl EmitBuf, is_option: bool) {
        let tag = if is_option { 1u16 } else { 0u16 }; // Some=1, Ok=0
        bytecode.push_make_enum(tag, 1);
    }

    /// Wrap the top-of-stack value as `Result::Err(e)`.
    fn emit_result_err(bytecode: &mut impl EmitBuf) {
        bytecode.push_make_enum(1, 1); // Err tag=1 arity=1
    }

    /// Emit `Matrix` ops (`*`, `+`, `-`, unary `-`) when the typechecker
    /// recorded [`LinearAlgebraInfo`] on this node.
    fn try_emit_matrix_op(
        &mut self,
        bytecode: &mut Vec<Byte>,
        self_id: Option<crate::typechecking::id::NodeId>,
        span_start: usize,
        span_end: usize,
        lhs: &Output,
        rhs: Option<&Output>,
    ) -> bool {
        let Some(info) = self_id
            .and_then(|id| self.checker.linear_algebra_at(id))
            .or_else(|| self.checker.linear_algebra_for_span(span_start, span_end))
            .cloned()
        else {
            return false;
        };
        match &info.kind {
            crate::typechecking::LinearAlgebraKind::MatMul { .. }
            | crate::typechecking::LinearAlgebraKind::MatrixZip { .. } => {
                let Some(rhs) = rhs else {
                    return false;
                };
                let args = [lhs.clone(), rhs.clone()];
                self.emit_linear_algebra(bytecode, self_id, span_start, span_end, &args);
                true
            }
            crate::typechecking::LinearAlgebraKind::MatrixNeg { .. } => {
                let args = [lhs.clone()];
                self.emit_linear_algebra(bytecode, self_id, span_start, span_end, &args);
                true
            }
            _ => false,
        }
    }

    /// Emit `dot` / `matmul` / `cross` / Matrix ops from the linear-algebra side table.
    ///
    /// Approach A: Dot / MatMul / MatrixZip / MatrixNeg lower to packed fat
    /// opcodes when dims fit the operand packing; otherwise keep the scalar
    /// unroll. `cross` stays unrolled (fixed N=3).
    fn emit_linear_algebra(
        &mut self,
        bytecode: &mut Vec<Byte>,
        self_id: Option<crate::typechecking::id::NodeId>,
        span_start: usize,
        span_end: usize,
        args: &[Output],
    ) {
        use crate::typechecking::{AggregateOp, LinearAlgebraKind};

        let Some(info) = self_id
            .and_then(|id| self.checker.linear_algebra_at(id))
            .or_else(|| self.checker.linear_algebra_for_span(span_start, span_end))
            .cloned()
        else {
            return;
        };

        let needs_two = !matches!(info.kind, LinearAlgebraKind::MatrixNeg { .. });
        if needs_two && args.len() != 2 {
            return;
        }
        if !needs_two && args.is_empty() {
            return;
        }

        // Prefer packed HostInvoke kernels (Approach A) for Dot / MatMul / Matrix*.
        if self.try_emit_packed_linear_algebra(bytecode, &info.kind, args) {
            return;
        }

        let t0 = self.alloc_temp_slot();
        bytecode.append(&mut self.do_compile(&args[0]));
        bytecode.push_store_pop(t0);
        let t1 = if needs_two {
            let slot = self.alloc_temp_slot();
            bytecode.append(&mut self.do_compile(&args[1]));
            bytecode.push_store_pop(slot);
            Some(slot)
        } else {
            None
        };

        match info.kind {
            LinearAlgebraKind::Dot {
                length,
                elem_is_float,
                ..
            } => {
                let t1 = t1.expect("dot needs two args");
                let mul = if elem_is_float {
                    Instruction::MULF
                } else {
                    Instruction::MUL
                };
                let add = if elem_is_float {
                    Instruction::ADDF
                } else {
                    Instruction::ADD
                };
                for i in 0..length {
                    bytecode.push_load(t0);
                    bytecode.push_const(i as i32);
                    bytecode.push_index();
                    bytecode.push_load(t1);
                    bytecode.push_const(i as i32);
                    bytecode.push_index();
                    bytecode.push(Byte::new(mul));
                    if i > 0 {
                        bytecode.push(Byte::new(add));
                    }
                }
            }
            LinearAlgebraKind::Cross {
                left_is_tuple,
                elem_is_float,
            } => {
                let t1 = t1.expect("cross needs two args");
                let mul = if elem_is_float {
                    Instruction::MULF
                } else {
                    Instruction::MUL
                };
                let sub = if elem_is_float {
                    Instruction::SUBF
                } else {
                    Instruction::SUB
                };
                // Load components into temps for clarity.
                let ax = self.alloc_temp_slot();
                let ay = self.alloc_temp_slot();
                let az = self.alloc_temp_slot();
                let bx = self.alloc_temp_slot();
                let by = self.alloc_temp_slot();
                let bz = self.alloc_temp_slot();
                for (slot, src, i) in [
                    (ax, t0, 0),
                    (ay, t0, 1),
                    (az, t0, 2),
                    (bx, t1, 0),
                    (by, t1, 1),
                    (bz, t1, 2),
                ] {
                    bytecode.push_load(src);
                    bytecode.push_const(i);
                    bytecode.push_index();
                    bytecode.push_store_pop(slot);
                }
                // i = ay*bz - az*by
                bytecode.push_load(ay);
                bytecode.push_load(bz);
                bytecode.push(Byte::new(mul));
                bytecode.push_load(az);
                bytecode.push_load(by);
                bytecode.push(Byte::new(mul));
                bytecode.push(Byte::new(sub));
                // j = az*bx - ax*bz
                bytecode.push_load(az);
                bytecode.push_load(bx);
                bytecode.push(Byte::new(mul));
                bytecode.push_load(ax);
                bytecode.push_load(bz);
                bytecode.push(Byte::new(mul));
                bytecode.push(Byte::new(sub));
                // k = ax*by - ay*bx
                bytecode.push_load(ax);
                bytecode.push_load(by);
                bytecode.push(Byte::new(mul));
                bytecode.push_load(ay);
                bytecode.push_load(bx);
                bytecode.push(Byte::new(mul));
                bytecode.push(Byte::new(sub));
                if left_is_tuple {
                    bytecode.push_make_tuple(3);
                } else {
                    bytecode.push_make_array(3);
                }
            }
            LinearAlgebraKind::MatMul {
                m,
                k,
                n,
                outer_is_tuple,
                row_is_tuple,
                elem_is_float,
            } => {
                let t1 = t1.expect("matmul needs two args");
                let mul = if elem_is_float {
                    Instruction::MULF
                } else {
                    Instruction::MUL
                };
                let add = if elem_is_float {
                    Instruction::ADDF
                } else {
                    Instruction::ADD
                };
                for i in 0..m {
                    for j in 0..n {
                        for t in 0..k {
                            // A[i][t]
                            bytecode.push_load(t0);
                            bytecode.push_const(i as i32);
                            bytecode.push_index();
                            bytecode.push_const(t as i32);
                            bytecode.push_index();
                            // B[t][j]
                            bytecode.push_load(t1);
                            bytecode.push_const(t as i32);
                            bytecode.push_index();
                            bytecode.push_const(j as i32);
                            bytecode.push_index();
                            bytecode.push(Byte::new(mul));
                            if t > 0 {
                                bytecode.push(Byte::new(add));
                            }
                        }
                    }
                    if row_is_tuple {
                        bytecode.push_make_tuple(n as u32);
                    } else {
                        bytecode.push_make_array(n as u32);
                    }
                }
                if outer_is_tuple {
                    bytecode.push_make_tuple(m as u32);
                } else {
                    bytecode.push_make_array(m as u32);
                }
            }
            LinearAlgebraKind::MatrixZip {
                m,
                n,
                op,
                outer_is_tuple,
                row_is_tuple,
                elem_is_float,
            } => {
                let t1 = t1.expect("matrix zip needs two args");
                let cell_op = match (op, elem_is_float) {
                    (AggregateOp::Add, false) => Instruction::ADD,
                    (AggregateOp::Add, true) => Instruction::ADDF,
                    (AggregateOp::Sub, false) => Instruction::SUB,
                    (AggregateOp::Sub, true) => Instruction::SUBF,
                    _ => Instruction::ADD,
                };
                for i in 0..m {
                    for j in 0..n {
                        bytecode.push_load(t0);
                        bytecode.push_const(i as i32);
                        bytecode.push_index();
                        bytecode.push_const(j as i32);
                        bytecode.push_index();
                        bytecode.push_load(t1);
                        bytecode.push_const(i as i32);
                        bytecode.push_index();
                        bytecode.push_const(j as i32);
                        bytecode.push_index();
                        bytecode.push(Byte::new(cell_op));
                    }
                    if row_is_tuple {
                        bytecode.push_make_tuple(n as u32);
                    } else {
                        bytecode.push_make_array(n as u32);
                    }
                }
                if outer_is_tuple {
                    bytecode.push_make_tuple(m as u32);
                } else {
                    bytecode.push_make_array(m as u32);
                }
            }
            LinearAlgebraKind::MatrixNeg {
                m,
                n,
                outer_is_tuple,
                row_is_tuple,
                elem_is_float,
            } => {
                for i in 0..m {
                    for j in 0..n {
                        bytecode.push_load(t0);
                        bytecode.push_const(i as i32);
                        bytecode.push_index();
                        bytecode.push_const(j as i32);
                        bytecode.push_index();
                        self.emit_neg_tos(bytecode, elem_is_float);
                    }
                    if row_is_tuple {
                        bytecode.push_make_tuple(n as u32);
                    } else {
                        bytecode.push_make_array(n as u32);
                    }
                }
                if outer_is_tuple {
                    bytecode.push_make_tuple(m as u32);
                } else {
                    bytecode.push_make_array(m as u32);
                }
            }
        }
    }

    /// Emit Approach A packed LA via `HostInvoke` (no new opcodes) when dims fit.
    /// Returns false to fall back to scalar unroll.
    fn try_emit_packed_linear_algebra(
        &mut self,
        bytecode: &mut Vec<Byte>,
        kind: &crate::typechecking::LinearAlgebraKind,
        args: &[Output],
    ) -> bool {
        use crate::typechecking::{AggregateOp, LinearAlgebraKind};

        let (native_name, meta, value_args): (&str, u32, &[Output]) = match kind {
            LinearAlgebraKind::Dot {
                length,
                elem_is_float,
                ..
            } => {
                if *length == 0 || *length > u16::MAX as usize || args.len() != 2 {
                    return false;
                }
                let mut ops = (*length as u32) & 0xFFFF;
                if *elem_is_float {
                    ops |= 1 << 16;
                }
                (machine::PACKED_DOT, ops, args)
            }
            LinearAlgebraKind::MatMul {
                m,
                k,
                n,
                outer_is_tuple,
                row_is_tuple,
                elem_is_float,
            } => {
                if args.len() != 2
                    || *m == 0
                    || *k == 0
                    || *n == 0
                    || *m > u8::MAX as usize
                    || *k > u8::MAX as usize
                    || *n > u8::MAX as usize
                {
                    return false;
                }
                let mut ops = (*m as u32) | ((*k as u32) << 8) | ((*n as u32) << 16);
                if *elem_is_float {
                    ops |= 1 << 24;
                }
                if *outer_is_tuple {
                    ops |= 1 << 25;
                }
                if *row_is_tuple {
                    ops |= 1 << 26;
                }
                (machine::PACKED_MATMUL, ops, args)
            }
            LinearAlgebraKind::MatrixZip {
                m,
                n,
                op,
                outer_is_tuple,
                row_is_tuple,
                elem_is_float,
            } => {
                if args.len() != 2
                    || *m == 0
                    || *n == 0
                    || *m > u8::MAX as usize
                    || *n > u8::MAX as usize
                {
                    return false;
                }
                let zip_kind: u32 = match op {
                    AggregateOp::Add => 0,
                    AggregateOp::Sub => 1,
                    _ => return false,
                };
                let mut ops = (*m as u32) | ((*n as u32) << 8) | (zip_kind << 16);
                if *elem_is_float {
                    ops |= 1 << 24;
                }
                if *outer_is_tuple {
                    ops |= 1 << 25;
                }
                if *row_is_tuple {
                    ops |= 1 << 26;
                }
                (machine::PACKED_MATRIX_ZIP, ops, args)
            }
            LinearAlgebraKind::MatrixNeg {
                m,
                n,
                outer_is_tuple,
                row_is_tuple,
                elem_is_float,
            } => {
                if args.is_empty()
                    || *m == 0
                    || *n == 0
                    || *m > u8::MAX as usize
                    || *n > u8::MAX as usize
                {
                    return false;
                }
                let mut ops = (*m as u32) | ((*n as u32) << 8);
                if *elem_is_float {
                    ops |= 1 << 16;
                }
                if *outer_is_tuple {
                    ops |= 1 << 17;
                }
                if *row_is_tuple {
                    ops |= 1 << 18;
                }
                (machine::PACKED_MATRIX_NEG, ops, args)
            }
            LinearAlgebraKind::Cross { .. } => return false,
        };

        let Some(native_id) = self.native_id(native_name) else {
            return false;
        };

        // HostInvoke stack: [id, args_tuple]; tuple = [arg0, …, meta].
        // Meta is a full u32 bitfield — must use `with_operand_u32` (not
        // `with_value_u32`, which only keeps the low 16 bits).
        let depth_on_entry = self.expr_depth;
        bytecode.push(Byte::new(Instruction::CONST).with_operand_u32(native_id as u32));
        self.expr_depth = depth_on_entry + 1;
        for arg in value_args {
            bytecode.append(&mut self.do_compile(arg));
            self.expr_depth += 1;
        }
        bytecode.push(Byte::new(Instruction::CONST).with_operand_u32(meta));
        self.expr_depth += 1;
        let arity = value_args.len() + 1; // + meta
        bytecode.push_make_tuple(arity as u32);
        bytecode.push_host_invoke(arity as u32);
        self.expr_depth = depth_on_entry;
        true
    }

    /// Desugar `assert(cond[, msg])` to Ok(()) / Err(msg) via MakeEnum.
    ///
    /// Emits into `self.bytecode` so nested absolute jumps stay valid.
    fn emit_assert(&mut self, args: &[Output]) {
        if args.is_empty() || args.len() > 2 {
            return;
        }

        let mut bb = BlockBuilder::new();
        let fail = bb.fresh_label(self.bytecode.il_mut());
        let end = bb.fresh_label(self.bytecode.il_mut());

        let cond_bc = self.do_compile(&args[0]);
        self.bytecode.extend(cond_bc);
        bb.emit_jump_to(fail, BbJumpKind::JumpIfFalse, self.bytecode.il_mut());

        // Success: Ok(())
        self.bytecode.push(Byte::new_with_value(
            Instruction::CONST,
            Value::from(0i64).raw() as _,
        ));
        Self::emit_ok_or_some_wrap(&mut self.bytecode, false);
        bb.emit_jump_to(end, BbJumpKind::Unconditional, self.bytecode.il_mut());
        bb.bind_label(fail, self.bytecode.il_mut());
        if let Some(msg) = args.get(1) {
            let msg_bc = self.do_compile(msg);
            self.bytecode.extend(msg_bc);
        } else {
            self.emit_string_literal("assertion failed");
        }
        Self::emit_result_err(&mut self.bytecode);
        bb.bind_label(end, self.bytecode.il_mut());
        bb.finalize()
            .expect("BlockBuilder::finalize: assert labels bound");
    }

    /// Emit a synthetic `main` that runs every harness test case in one VM
    /// (standalone `cargo run -- tests/foo.hy`). Prints
    /// `> Test "<name>" failed` on soft failures and panics with
    /// `"tests failed"` if any case failed.
    fn emit_virtual_test_main(&mut self) {
        let cases: Vec<(String, u32)> = self.test_cases.clone();
        if cases.is_empty() {
            return;
        }

        self.bind_function_entry("main".to_string());

        let prev_vars = std::mem::take(&mut self.context.variables);
        self.context.variables = Interner::default();
        // slot 0 = failed count
        let failed_slot = self.context.variables.intern("failed".to_string()) as u32;
        self.bytecode.push(Byte::new_with_value(
            Instruction::CONST,
            Value::from(0i64).raw() as _,
        ));
        self.bytecode.push_store_pop(failed_slot);

        let mut bb = BlockBuilder::new();
        for (desc, offset) in &cases {
            if let Some(label) = self.bytecode.entry_label_for_offset(*offset as usize) {
                self.bytecode.emit_entry(EntryKind::Call, 0, label);
            } else {
                // Fallback for cases without a bound entry label (should be rare
                // after `bind_function_entry`); packed CALL(0, pc) keeps harness green.
                self.bytecode
                    .push(Byte::new(Instruction::CALL).with_call_packed(0, *offset));
            }
            // Jump if Result::Err (tag 1) — on match, payload (message) is pushed.
            let fail = bb.fresh_label(self.bytecode.il_mut());
            let done = bb.fresh_label(self.bytecode.il_mut());
            bb.emit_jump_to(
                fail,
                BbJumpKind::JumpIfMatch { tag: 1, arity: 1 },
                self.bytecode.il_mut(),
            );
            // Ok path: discard whole Result enum.
            self.bytecode.push_pop();
            bb.emit_jump_to(done, BbJumpKind::Unconditional, self.bytecode.il_mut());
            bb.bind_label(fail, self.bytecode.il_mut());
            // Discard Err message payload.
            self.bytecode.push_pop();
            let msg = format!("> Test \"{desc}\" failed\n");
            self.emit_string_literal(&msg);
            self.bytecode.push_print();
            // failed += 1
            self.bytecode.push_load(failed_slot);
            self.bytecode.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(1i64).raw() as _,
            ));
            self.bytecode.push(Byte::new(Instruction::ADD));
            self.bytecode.push_store_pop(failed_slot);
            bb.bind_label(done, self.bytecode.il_mut());
        }

        // if failed != 0 { panic "tests failed" }
        let panic_lbl = bb.fresh_label(self.bytecode.il_mut());
        let end_lbl = bb.fresh_label(self.bytecode.il_mut());
        self.bytecode.push_load(failed_slot);
        self.bytecode.push(Byte::new_with_value(
            Instruction::CONST,
            Value::from(0i64).raw() as _,
        ));
        self.bytecode.push(Byte::new(Instruction::EQ));
        // failed == 0 → EQ true → fall through JMPF; else JMPF → panic.
        bb.emit_jump_to(panic_lbl, BbJumpKind::JumpIfFalse, self.bytecode.il_mut());
        bb.emit_jump_to(end_lbl, BbJumpKind::Unconditional, self.bytecode.il_mut());
        bb.bind_label(panic_lbl, self.bytecode.il_mut());
        self.emit_string_literal("tests failed");
        self.bytecode.push(Byte::new(Instruction::Panic));
        bb.bind_label(end_lbl, self.bytecode.il_mut());
        self.bytecode.push_const(0);
        self.bytecode.push_return();

        bb.finalize()
            .expect("BlockBuilder::finalize: virtual test main labels bound");
        self.context.variables = prev_vars;
    }

    fn do_compile<'compiler>(
        &mut self,
        ast: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> Vec<Byte> {
        let mut bytecode = vec![];
        let self_id = self.next_emit_id();
        let (span, child) = ast;

        match child.borrow() {
            Expression::Comment(_) => (),
            // --- Modules ---
            Expression::Use {
                path: p,
                name,
                alias,
            } => {
                // Virtual modules are applied during typecheck
                // (`Checker::apply_virtual_use`); no disk FQN alias.
                if self.checker.virtual_modules().resolves_use(p, name) {
                    // Scope already populated by check_program.
                } else if name == "*" {
                    let module_ns = p.join("::");
                    let prefix = if module_ns.is_empty() {
                        String::new()
                    } else {
                        format!("{}::", module_ns)
                    };
                    let fqns: Vec<String> = self
                        .functions
                        .keys()
                        .filter(|fqn| {
                            fqn.starts_with(&prefix)
                                && !fqn[prefix.len()..].contains("::")
                                && !fqn[prefix.len()..].is_empty()
                        })
                        .cloned()
                        .collect();
                    for fqn in fqns {
                        let item_name = fqn[prefix.len()..].to_string();
                        self.aliases.insert(item_name, fqn);
                    }
                } else {
                    // Prefer the FQN that actually exists in the function
                    // table so both conventions work:
                    //   foo/sadge.hy  → foo::sadge::sadge  (one-item-per-file)
                    //   foo.hy        → foo::sadge         (item-in-module-file)
                    let module_ns = p.join("::");
                    let file_per_item = if module_ns.is_empty() {
                        format!("{name}::{name}")
                    } else {
                        format!("{module_ns}::{name}::{name}")
                    };
                    let item_in_module = if module_ns.is_empty() {
                        name.clone()
                    } else {
                        format!("{module_ns}::{name}")
                    };
                    let qualified = if self.functions.contains_key(&file_per_item) {
                        file_per_item
                    } else if self.functions.contains_key(&item_in_module) {
                        item_in_module
                    } else {
                        // Defensive: keep the historical one-item-per-file
                        // shape when the dependency has not been linked yet.
                        file_per_item
                    };
                    let local = alias.clone().unwrap_or_else(|| name.clone());
                    self.aliases.insert(local, qualified);
                }
            }
            Expression::Noop(_) => (),
            // `mod foo;` — pipeline loads the file; no bytecode.
            Expression::Module(_, _body) => {}
            Expression::Group(e) => bytecode.append(&mut self.do_compile(e)),
            // Named call-site arg — compile the value (defensive; Call reorders).
            Expression::NamedArg(_, value) => {
                bytecode.append(&mut self.do_compile(value));
            }
            Expression::Program(children) => {
                children.iter().for_each(|child| {
                    bytecode.append(&mut self.do_compile(child));
                });
                if !self.test_cases.is_empty() && !self.user_main_defined {
                    self.emit_virtual_test_main();
                }
            }
            // --- `let (a, b) = expr` / `let { x, y } = expr` ---
            Expression::LetDestructure { pattern, rhs } => {
                let rhs_is_match = Self::rhs_is_match_expr(rhs);
                if rhs_is_match {
                    self.emit_binding_rhs(rhs);
                } else {
                    self.append_binding_rhs(&mut bytecode, rhs);
                }
                let tmp = self.alloc_temp_slot();
                if rhs_is_match {
                    self.bytecode.push_store_pop(tmp);
                    let mut binds = Vec::new();
                    self.emit_let_pattern_binds(pattern, tmp, &mut binds);
                    self.bytecode.append(&mut binds);
                } else {
                    bytecode.push_store_pop(tmp);
                    self.emit_let_pattern_binds(pattern, tmp, &mut bytecode);
                }
            }

            // --- Let / const bindings ---
            Expression::Fragment(children) => {
                // `let x = expr` / `const x = expr` → compile RHS, then
                // StorePop into x's slot.
                let mut is_binding = false;
                if children.len() == 2 {
                    let binding = match children[0].1.as_ref() {
                        Expression::Variable(name, _ty) => Some((name.to_string(), false)),
                        Expression::Constant(name, _ty) => {
                            Some((self.resolve_variable(name), true))
                        }
                        _ => None,
                    };
                    if let Some((name, is_const)) = binding {
                        let binder_span = (children[0].0.start, children[0].0.end);
                        // Check if the RHS is a bare identifier that names a generic fn.
                        // If so, track this variable as holding an ObjPolyFn.
                        let polyfn_source = match unwrapped_identifier(&children[1]) {
                            Some(rhs_name) => {
                                let resolved = self
                                    .aliases
                                    .get(rhs_name)
                                    .cloned()
                                    .unwrap_or_else(|| rhs_name.to_string());
                                (self.checker.is_generic_fn(&resolved)
                                    || self.functions.get(&resolved).is_some()
                                        && self.checker.is_generic_fn(rhs_name))
                                .then_some(resolved)
                            }
                            _ => None,
                        };
                        if let Some(source) = polyfn_source {
                            self.polyfn_vars.insert(name.clone());
                            self.polyfn_sources.insert(name.clone(), source);
                        } else if self
                            .checker
                            .is_polyfn_binding_at(binder_span.0, binder_span.1)
                        {
                            // Returned/captured PolyFn (`let f = capture_show(0)`).
                            self.polyfn_vars.insert(name.clone());
                        }
                        // Compile the RHS BEFORE interning the binding name.
                        // Match payload slots use `variables.len()` as the first
                        // free slot; interning early (e.g. `let v = match e`)
                        // reserved a hole and made bindings land one slot too
                        // high while JumpIfMatch still pushed at the real
                        // cursor.
                        let rhs_is_match = Self::rhs_is_match_expr(&children[1]);
                        if rhs_is_match {
                            self.emit_binding_rhs(&children[1]);
                        } else {
                            self.append_binding_rhs(&mut bytecode, &children[1]);
                        }
                        if is_const {
                            if let Some(val) = crate::const_fold::eval_expr(&children[1], self.const_env())
                            {
                                self.const_env_mut().insert(name.clone(), val);
                            } else {
                                let slot = self.alloc_binding_slot(&name);
                                self.context.constants.insert(slot as usize, true);
                                if rhs_is_match {
                                    self.bytecode.push_store_pop(slot);
                                } else {
                                    bytecode.push_store_pop(slot);
                                }
                            }
                            is_binding = true;
                        } else {
                            let slot = self.alloc_binding_slot(&name);
                            if rhs_is_match {
                                self.bytecode.push_store_pop(slot);
                            } else {
                                bytecode.push_store_pop(slot);
                            }
                            is_binding = true;
                        }
                    }
                }
                if !is_binding {
                    children.iter().for_each(|child| {
                        bytecode.append(&mut self.do_compile(child));
                    });
                }
            }
            Expression::Block(children) => {
                // Isolate PolyFn tracking the same way function bodies do:
                // keep outer entries visible inside the block, then restore
                // so an inner `let f = capture_show(0)` cannot poison an
                // outer same-named ObjFn / mono local after the block.
                let saved_polyfn_vars = self.polyfn_vars.clone();
                let saved_polyfn_sources = self.polyfn_sources.clone();
                self.push_const_env();
                let ctx = self.context.child();
                self.context = ctx;
                // Append each child to self.bytecode (Print/control-flow emit in-place).
                for child in children {
                    let mut bc = self.do_compile(child);
                    self.bytecode.append(&mut bc);
                }

                self.context = *self.context.get_prev().clone().unwrap();
                self.pop_const_env();
                self.polyfn_vars = saved_polyfn_vars;
                self.polyfn_sources = saved_polyfn_sources;
            }
            Expression::Function {
                docs: _,
                attrs,
                name,
                is_coro,
                is_static: _,
                type_params,
                args,
                returns: _returns,
                where_constraints: _,
                body,
            } => {
                let Some(body) = body else {
                    return vec![];
                };
                let qualified = if self.namespace.is_empty() {
                    name.to_string()
                } else {
                    format!("{}::{}", self.namespace, name)
                };
                if *name == "main" {
                    self.user_main_defined = true;
                }
                self.module_items
                    .entry(self.namespace.clone())
                    .or_default()
                    .push(name.to_string());
                let (fixed_arity, has_rest) = fn_arity_from_args(args);
                let table_key =
                    if self.checker.is_overloaded(name) || self.checker.is_overloaded(&qualified) {
                        overload_fn_key(&qualified, fixed_arity, has_rest)
                    } else {
                        qualified.clone()
                    };
                let (fn_offset, _) = self.bind_function_entry(table_key.clone());
                let fn_offset = fn_offset as u32;
                self.fn_arities
                    .insert(table_key.clone(), (fixed_arity as u32, has_rest));
                if table_key != qualified {
                    self.fn_arities
                        .insert(qualified.clone(), (fixed_arity as u32, has_rest));
                }
                if let Some(desc) = parser::ast::attr_test_desc(attrs, name) {
                    self.test_cases.push((desc, fn_offset));
                }
                if *is_coro {
                    self.coroutine_fns.insert(qualified.clone());
                }

                // Fresh slot map per function so locals start at 0
                // (or 1 with `self`) for this frame. Sharing one
                // Interner across functions made later `let`s use high
                // slots; `StorePop` then left holes and match bindings
                // at slot 1 read garbage. Extern preload slots live in
                // the entry frame (bytecode before `main`).
                let prev_fn_vars = std::mem::take(&mut self.context.variables);
                let prev_fn_polyfn_vars = std::mem::take(&mut self.polyfn_vars);
                let prev_fn_polyfn_sources = std::mem::take(&mut self.polyfn_sources);
                let prev_fn_qualified = self.current_function_qualified.take();
                let prev_fn_table_key = self.current_function_table_key.take();
                self.current_function_qualified = Some(qualified.clone());
                self.current_function_table_key = Some(table_key.clone());
                self.push_const_env();
                self.context.variables = Interner::default();
                self.expr_depth = 0;
                if self.compiling_method {
                    let slot = self.context.variables.intern("self".to_string()) as u32;
                    self.record_debug_local("self", slot);
                }

                let prev_result_mode = self.compiling_result_mode;
                self.compiling_result_mode = self.checker.fn_is_result_mode(name);

                let mut a = self.do_compile(args);

                // ── Dictionary-passing prologue ────────────────────────────────
                // Generic functions with user-defined trait constraints receive
                // extra dict tuple arguments after the value params.  Reserve a
                // stack slot `__dictN` for each expected dict so that the Interner
                // assigns a slot number that can later be LOAD-ed by CallIndirect
                // dispatch paths.  The VM pushes these as the trailing elements of
                // the call frame, one per user constraint, in constraint order.
                // Every trait constraint (including builtin Num/Ord/Eq/Show)
                // gets a trailing `__dictN` slot for dictionary dispatch.
                let dict_arity = self.checker.dict_arity_for(name);
                for dict_idx in 0..dict_arity {
                    self.context.variables.intern(format!("__dict{}", dict_idx));
                }

                // Args + self + dicts occupy the shared stack at body entry.
                let entry_sp = self.context.variables.len() as u32;

                self.bytecode.append(&mut a);

                let body_start = self.bytecode.len();
                let body_op_start = self.bytecode.ops().len();
                let prev_field_keys = std::mem::take(&mut self.field_key_slots);
                self.emit_field_key_prologue(body);
                let prev_active = self.active_fn_name.take();
                let prev_fn_defers = std::mem::take(&mut self.fn_defers);
                self.active_fn_name = Some(name.to_string());
                let mut c = self.do_compile(body);
                self.active_fn_name = prev_active;
                self.bytecode.append(&mut c);

                if !self.region_ends_with_return(body_op_start) {
                    self.emit_fallthrough_return(name, body.0);
                }

                self.fn_defers = prev_fn_defers;
                self.compiling_result_mode = prev_result_mode;
                self.pop_const_env();
                self.current_function_qualified = prev_fn_qualified;
                self.current_function_table_key = prev_fn_table_key;
                self.field_key_slots = prev_field_keys;
                let body_end = self.bytecode.len();
                self.fn_bytecode_spans
                    .insert(table_key.clone(), (body_start, body_end));
                let entry = self.fn_entry_labels.get(&table_key).copied();
                self.bytecode
                    .record_func_with_sp(table_key, entry, body_start, body_end, entry_sp);
                self.context.variables = prev_fn_vars;
                self.polyfn_vars = prev_fn_polyfn_vars;
                self.polyfn_sources = prev_fn_polyfn_sources;

                self.emit_mono_specializations_for_function(
                    &qualified,
                    type_params,
                    args,
                    Some(body),
                    name,
                );
            }
            Expression::Lambda {
                args,
                captures,
                body,
            } => {
                // Layout in self.bytecode:
                //   JMP after_body
                //   entry: <captures slots 0..n> <params> <body> RETURN
                //   after_body: LOAD captures...; CONST 0; CodePtr entry; MakeFn
                use crate::block_builder::{BlockBuilder, JumpKind};
                let mut bb = BlockBuilder::new();
                let after = bb.fresh_label(self.bytecode.il_mut());
                bb.emit_jump_to(after, JumpKind::Unconditional, self.bytecode.il_mut());
                // Entry label keeps the lambda body alive for later dead_block.
                self.bytecode.bind_fresh_entry();
                let entry = self.bytecode.len() as u32;

                let prev_fn_vars = std::mem::take(&mut self.context.variables);
                for cap in captures {
                    self.context.variables.intern((*cap).to_string());
                }
                let mut a = self.do_compile(args);
                self.bytecode.append(&mut a);
                let (arity, is_rest) = fn_arity_from_args(args);
                let prev_field_keys = std::mem::take(&mut self.field_key_slots);
                self.emit_field_key_prologue(body);
                let mut b = self.do_compile(body);
                self.bytecode.append(&mut b);
                // Expression-bodied lambdas (`=> x + y` / `{ …; last }`) leave
                // the result on the stack — emit a bare RETURN. Pushing
                // `CONST 0; RETURN` (named-fn fall-through) would discard that
                // value; peephole then fuses it to `ConstReturnImm` and every
                // call returns 0.
                if !matches!(
                    self.bytecode.last_byte().map(|b| *b.bytecode()),
                    Some(Instruction::RETURN)
                ) {
                    let body_empty = matches!(
                        body.1.as_ref(),
                        Expression::Block(items) if items.is_empty()
                    );
                    if body_empty {
                        self.bytecode.push_const(0);
                    }
                    self.bytecode.push_return();
                }
                self.field_key_slots = prev_field_keys;
                self.context.variables = prev_fn_vars;

                bb.bind_label(after, self.bytecode.il_mut());
                let _ = bb.finalize();

                for cap in captures {
                    if let Some(slot) = self.lookup_slot(cap) {
                        bytecode.push_load(slot);
                    } else {
                        let mut message = Message::error(
                            ErrorCode::UnknownValue,
                            format!("Cannot find capture `{}`", cap),
                            span.into_range(),
                        );
                        message.push(DiagLabel::new(
                            format!("`{}` must be in scope at the lambda", cap),
                            span.into_range(),
                        ));
                        self.messages.push(message);
                    }
                }
                bytecode.push_const(0);
                bytecode.push(Byte::new(Instruction::CodePtr).with_operand_u32(entry));
                bytecode.push(
                    Byte::new(Instruction::MakeFn).with_operand_u32(make_fn_operand(
                        captures.len() as u32,
                        0,
                        arity as u32,
                        is_rest,
                    )),
                );
            }
            Expression::Expr(child) | Expression::Statement(child) => {
                bytecode.append(&mut self.do_compile(child))
            }
            Expression::ExprStatement(child) => {
                bytecode.append(&mut self.do_compile(child));
                // Also skip the POP for a bare `yield expr;` / `yield from expr;`
                // statement. The parser's `expr_statement()` alternative matches
                // `yield` before the dedicated (POP-free) `self.yield_()` statement
                // parser ever gets a chance (see `parser::statement`), so every
                // bare yield lands here. A trailing POP would be DEAD CODE at
                // compile time (nothing is pushed when the yield executes) but
                // becomes the coroutine's `resume_ip` — the NEXT time the
                // coroutine is resumed, the VM starts by executing that POP,
                // which pops whatever happens to be on top of the (shared)
                // stack at the resumer's call site. For a `resume` used inline
                // inside another expression (e.g. formatting `resume h`),
                // that top-of-stack value belongs to the RESUMER (e.g. the
                // format string), not the coroutine — corrupting it.
                Self::discard_statement_value(&mut bytecode);
            }
            // ---- Userland FFI builtins ----
            Expression::Dload(path) => {
                let bc = self.do_compile(path);
                self.bytecode.extend(bc);
                self.bytecode.push(Byte::new(Instruction::FfiLoad));
            }
            Expression::Done(handle) => {
                let bc = self.do_compile(handle);
                self.bytecode.extend(bc);
                self.bytecode.push(Byte::new(Instruction::DoneCoro));
            }
            // --- Aggregates ---
            Expression::Tuple(items) => {
                for c in items {
                    let mut bc = self.do_compile(c);
                    bytecode.append(&mut bc);
                }
                let arity = items.len() as u32;
                bytecode.push_make_tuple(arity);
            }
            Expression::Array(items) => {
                for c in items {
                    let mut bc = self.do_compile(c);
                    bytecode.append(&mut bc);
                }
                let arity = items.len() as u32;
                bytecode.push_make_array(arity);
            }
            // --- Dict literals ---
            Expression::Dict(items) => {
                // Eagerly resolve field names to strings before
                // any bytecode emission so the byte offsets
                // remain stable.
                let field_names: Vec<&str> = items.iter().map(|f| f.name).collect();
                for (f, name) in items.iter().zip(field_names.iter()) {
                    // value first (so it's UNDER the field name
                    // when both are pushed). MakeDict pops the
                    // top first (which is the field-name) and
                    // then the value, so they end up correctly
                    // paired in (name, value) order in the
                    // runtime's pair Vec.
                    let mut bc = self.do_compile(&f.value);
                    bytecode.append(&mut bc);
                    self.emit_raw_string_literal(&mut bytecode, name);
                }
                let arity = items.len() as u32;
                bytecode.push(Byte::new(Instruction::MakeDict).with_operand_u32(arity));
            }
            // Lazy range value: dict `{ start, end, inclusive }` so
            // first-class `let r = 0..n; for x in r` works via GetField.
            // Direct `for x in 0..n` uses the no-heap fast path instead.
            Expression::Range {
                start,
                end,
                inclusive,
            } => {
                let mut start_bc = self.do_compile(start);
                bytecode.append(&mut start_bc);
                self.emit_raw_string_literal(&mut bytecode, "start");
                let mut end_bc = self.do_compile(end);
                bytecode.append(&mut end_bc);
                self.emit_raw_string_literal(&mut bytecode, "end");
                bytecode.push_const(if *inclusive { 1 } else { 0 });
                self.emit_raw_string_literal(&mut bytecode, "inclusive");
                bytecode.push(Byte::new(Instruction::MakeDict).with_operand_u32(3));
            }
            // `t[i]` — pop the index (top), pop the target,
            // push the element at `target[index]`. The Index
            // opcode carries no operand (the index is at the top
            // of the operand stack at dispatch time).
            Expression::Index(target, Some(index)) => {
                let mut target_bc = self.do_compile(target);
                bytecode.append(&mut target_bc);
                let mut index_bc = self.do_compile(index);
                bytecode.append(&mut index_bc);
                bytecode.push_index();
            }
            Expression::Index(_, None) => {}
            Expression::Readonly(inner) => {
                let inner_bc = self.do_compile(inner);
                bytecode.extend(inner_bc);
            }
            Expression::QualifiedAccess { owner, member } => {
                let fqn = format!("{}::{}", owner, member);
                if let Some(slot) = self.checker.static_slot_index(&fqn) {
                    bytecode.push(Byte::new(Instruction::LoadStatic).with_operand_u32(slot));
                }
            }
            Expression::StaticDecl { name, init, .. } => {
                let fqn = self.qualify_static_fqn(name);
                self.emit_static_initializer(&fqn, init);
            }
            // --- FFI declare/invoke (legacy AST; prefer Call + use ffi::*) ---
            Expression::Declare(args) => self.emit_ffi_declare(*span, args),
            Expression::Invoke(args) => self.emit_ffi_invoke(*span, args),
            Expression::Return(expr) | Expression::ImplicitReturn(expr) => {
                if self.try_emit_tail_call_expr(expr, &mut bytecode) {
                    if self.compiling_result_mode {
                        Self::emit_ok_or_some_wrap(&mut bytecode, false);
                    }
                    return bytecode;
                }

                // Evaluate the return value first, then run defers (LIFO).
                // Each defer thunk returns a sentinel that we POP so the
                // pending return value stays on top for RETURN.
                // Flush the value into `self.bytecode` before labeled defers.
                self.append_with_existential_pack(&mut bytecode, expr);
                // Result-mode functions: bare `return v` becomes `Ok(v)`.
                if self.compiling_result_mode {
                    Self::emit_ok_or_some_wrap(&mut bytecode, false);
                }
                self.bytecode.append(&mut bytecode);
                self.emit_run_defers();
                if !matches!(child.borrow(), Expression::ImplicitReturn(_)) {
                    self.bytecode.push_return();
                }
            }
            Expression::Yield(expr) => {
                bytecode.append(&mut self.do_compile(expr));
                bytecode.push(Byte::new(Instruction::YieldCoro));
            }
            Expression::YieldFrom(expr) => {
                bytecode.append(&mut self.do_compile(expr));
                bytecode.push(Byte::new(Instruction::YieldFromCoro));
            }
            Expression::Resume(target, arg) => {
                if let Some(a) = arg {
                    bytecode.append(&mut self.do_compile(a));
                }
                bytecode.append(&mut self.do_compile(target));
                let has_send = if arg.is_some() { 1u32 } else { 0u32 };
                bytecode.push(Byte::new(Instruction::ResumeCoro).with_operand_u32(has_send));
            }
            Expression::Class {
                docs: _,
                name,
                fields: state,
                ..
            } => {
                use parser::ast::FieldModifier;
                let mut instance_fields: Vec<(String, usize)> = Vec::new();
                let mut idx = 0usize;
                for v in state {
                    match v.1.borrow() {
                        Expression::Field {
                            docs: _,
                            modifier,
                            name: n,
                            init,
                            ..
                        } => {
                            let fname = self.resolve_variable(n);
                            if matches!(modifier, FieldModifier::Static) {
                                if let Some(init_expr) = init {
                                    let fqn = format!("{}::{}", name, fname);
                                    self.emit_static_initializer(&fqn, init_expr);
                                }
                            } else {
                                instance_fields.push((fname, idx));
                                idx += 1;
                            }
                        }
                        _ => {
                            unreachable!("There should be only fields inside of a class definition")
                        }
                    }
                }
                self.context
                    .classes
                    .insert(name.to_string(), instance_fields);
                self.context.symbols.intern(name.to_string());
            }
            Expression::Implementation { owner, methods, .. } => {
                let saved_ns = self.namespace.clone();
                self.namespace = owner.to_string();

                for method_node in methods {
                    match method_node.1.borrow() {
                        Expression::Method(_, body) => {
                            if let Expression::Function {
                                docs: _,
                                name, is_static, ..
                            } = body.1.borrow()
                            {
                                let fqn = format!("{}::{}", owner, name);
                                // Instance methods reserve slot 0 for `self`;
                                // static methods start params at slot 0.
                                self.compiling_method = !*is_static;
                                self.do_compile(body);
                                self.compiling_method = false;
                                self.context
                                    .methods
                                    .entry(owner.to_string())
                                    .or_default()
                                    .insert(name.to_string(), fqn);
                            } else {
                                self.compiling_method = true;
                                self.do_compile(body);
                                self.compiling_method = false;
                            }
                        }
                        _ => {
                            self.do_compile(method_node);
                        }
                    }
                }

                self.context
                    .impementations
                    .insert(owner.to_string(), owner.to_string());
                self.namespace = saved_ns;
            }
            Expression::Method(_vis, body) => {
                let is_static = matches!(
                    body.1.borrow(),
                    Expression::Function {
                        is_static: true,
                        ..
                    }
                );
                self.compiling_method = !is_static;
                bytecode.append(&mut self.do_compile(body));
                self.compiling_method = false;
            }
            Expression::Instantiate(class, args) => {
                let name = self.resolve_variable_checked(class);
                let ctor_name = self
                    .decorated_class_ctors
                    .get(&name)
                    .filter(|ctor| self.active_fn_name.as_deref() != Some(ctor.as_str()))
                    .cloned();
                if let Some(ctor) = ctor_name {
                    let arg_slice = args.as_deref().unwrap_or(&[]);
                    if let Some(offset) = self.functions.get(ctor.as_str()).copied() {
                        let arity =
                            self.emit_call_args_with_rest(&ctor, arg_slice, &mut bytecode, false);
                        bytecode.push(
                            Byte::new(Instruction::CALL).with_call_packed(arity, offset as u32),
                        );
                    } else {
                        self.messages.push(Message::error(
                            ErrorCode::GenericTypeError,
                            format!(
                                "Decorated constructor `{ctor}` for class `{name}` was not found"
                            ),
                            class.0.into_range(),
                        ));
                    }
                } else {
                    let fields = self.context.classes.get(&name).cloned().unwrap_or_default();
                    bytecode
                        .push(Byte::new(Instruction::INIT).with_operand_u32(fields.len() as u32));
                    // SetField stack order is value, target, name (same as
                    // Assignment to Access). Stash the instance, then for
                    // each ctor arg emit that sequence and discard the
                    // value SetField pushes back.
                    //
                    // `StorePop` keeps the instance at `tmp` with the cursor
                    // past that slot — so the stashed value is already TOS
                    // for the expression result. Do **not** emit a final
                    // `LOAD tmp`: that would push a second copy and leave
                    // the stash sitting between any live values below
                    // (e.g. a HostInvoke native-id CONST) and the result,
                    // so `MakeTuple`/`HostInvoke` would pick up the instance
                    // as the native id.
                    let tmp_inst = self.alloc_temp_slot();
                    bytecode.push_store_pop(tmp_inst);
                    if let Some(arg_list) = args {
                        for (arg, (fname, _)) in arg_list.iter().zip(fields.iter()) {
                            bytecode.append(&mut self.do_compile(arg));
                            bytecode.push_load(tmp_inst);
                            self.emit_field_name(&mut bytecode, fname);
                            bytecode.push_set_field();
                            bytecode.push_pop();
                        }
                    }
                }
            }
            Expression::Adjust { op, prefix, target } => {
                self.emit_adjust(&mut bytecode, target, *op, *prefix);
            }
            Expression::CompoundAssign(target, op, rhs) => {
                self.emit_compound_assign(
                    &mut bytecode,
                    self_id,
                    span.start,
                    span.end,
                    target,
                    *op,
                    rhs,
                );
            }
            // --- Loop codegen ---
            // `while`: [top] cond, JMPF→exit, body, JMP→top, [exit]
            // `for x in`: IntoIterator/Iterator (array/tuple/dict/coro/custom)
            Expression::Loop {
                identifier,
                iterable,
                body,
            } => {
                if let Some(binding) = identifier {
                    let binding_name = match binding.1.as_ref() {
                        Expression::Identifier(n) => (*n).to_string(),
                        _ => "__for_in_x".to_string(),
                    };
                    let info = self_id
                        .and_then(|id| self.checker.for_in_info_at(id))
                        .or_else(|| self.checker.for_in_info_for_span(span.start, span.end))
                        .cloned();
                    let kind = info.map(|i| i.kind).unwrap_or(ForInKind::Coroutine);
                    match kind {
                        ForInKind::Array => {
                            self.emit_for_in_array_loop(body, &binding_name, false, Some(iterable));
                        }
                        ForInKind::Tuple { arity } => {
                            self.emit_for_in_tuple(iterable, body, &binding_name, arity);
                        }
                        ForInKind::Dict => {
                            self.emit_for_in_dict(iterable, body, &binding_name);
                        }
                        ForInKind::Coroutine => {
                            self.emit_for_in_coro(iterable, body, &binding_name);
                        }
                        ForInKind::Range { inclusive, float } => {
                            self.emit_for_in_range(iterable, body, &binding_name, inclusive, float);
                        }
                        ForInKind::Custom {
                            into_iter_fqn,
                            next_fqn,
                        } => {
                            self.emit_for_in_custom(
                                iterable,
                                body,
                                &binding_name,
                                &into_iter_fqn,
                                &next_fqn,
                            );
                        }
                    }
                } else {
                    if let Some(ConstValue::Bool(false)) =
                        crate::const_fold::eval_expr(iterable, self.const_env())
                    {
                        self.discard_compile(iterable);
                        self.discard_compile(body);
                        return bytecode;
                    }
                    let mut bb = BlockBuilder::new();
                    let top_label = bb.fresh_label(self.bytecode.il_mut());
                    let exit_label = bb.fresh_label(self.bytecode.il_mut());
                    bb.bind_label(top_label, self.bytecode.il_mut());

                    let iter_bc = self.do_compile(iterable);
                    self.bytecode.extend(iter_bc);

                    bb.emit_jump_to(exit_label, BbJumpKind::JumpIfFalse, self.bytecode.il_mut());

                    self.loop_stack.push((top_label, exit_label));
                    self.loop_bbs.push(bb);
                    let body_bc = self.do_compile(body);
                    self.bytecode.extend(body_bc);
                    let mut bb = self
                        .loop_bbs
                        .pop()
                        .expect("loop builder stack balanced for while");
                    self.loop_stack
                        .pop()
                        .expect("loop label stack balanced for while");

                    bb.emit_jump_to(top_label, BbJumpKind::Unconditional, self.bytecode.il_mut());
                    bb.bind_label(exit_label, self.bytecode.il_mut());

                    bb.finalize()
                        .expect("BlockBuilder::finalize: all targeted labels bound");
                }
            }
            // --- C-style for codegen ---
            // Layout: init, [top] cond, JMPF→exit, body, [continue] step, JMP→top, [exit]
            Expression::For {
                init,
                cond,
                step,
                body,
            } => {
                if let Some(trips) =
                    crate::const_fold::for_loop_trip_count(init.as_ref(), cond, step.as_ref())
                    && !crate::const_fold::body_has_loop_control(body)
                {
                    // Pre-walk order is init → cond → body → step. Emit init,
                    // discard cond (IDs only), then for each trip restore the
                    // emit cursor and emit body + step so the induction
                    // variable advances (e.g. `s = s + i` for `i < 4` → 6).
                    if let Some(init) = init {
                        let mut init_bc = self.do_compile(init);
                        Self::discard_statement_value(&mut init_bc);
                        self.bytecode.extend(init_bc);
                    }
                    self.discard_compile(cond);
                    let body_start_idx = self.emit_idx;
                    for _ in 0..trips {
                        self.emit_idx = body_start_idx;
                        let mut body_bc = self.do_compile(body);
                        self.bytecode.append(&mut body_bc);
                        if let Some(step) = step {
                            let mut step_bc = self.do_compile(step);
                            Self::discard_statement_value(&mut step_bc);
                            self.bytecode.extend(step_bc);
                        }
                    }
                    return bytecode;
                }
                if let Some(ConstValue::Bool(false)) = crate::const_fold::eval_expr(cond, self.const_env())
                {
                    if let Some(init) = init {
                        let mut init_bc = self.do_compile(init);
                        Self::discard_statement_value(&mut init_bc);
                        self.bytecode.extend(init_bc);
                    }
                    self.discard_compile(cond);
                    if let Some(step) = step {
                        self.discard_compile(step);
                    }
                    self.discard_compile(body);
                    return bytecode;
                }
                if let Some(init) = init {
                    let mut init_bc = self.do_compile(init);
                    Self::discard_statement_value(&mut init_bc);
                    self.bytecode.extend(init_bc);
                }

                let mut bb = BlockBuilder::new();
                let top_label = bb.fresh_label(self.bytecode.il_mut());
                let continue_label = bb.fresh_label(self.bytecode.il_mut());
                let exit_label = bb.fresh_label(self.bytecode.il_mut());
                bb.bind_label(top_label, self.bytecode.il_mut());

                let cond_bc = self.do_compile(cond);
                self.bytecode.extend(cond_bc);

                bb.emit_jump_to(exit_label, BbJumpKind::JumpIfFalse, self.bytecode.il_mut());

                self.loop_stack.push((continue_label, exit_label));
                self.loop_bbs.push(bb);
                let body_bc = self.do_compile(body);
                self.bytecode.extend(body_bc);
                let mut bb = self
                    .loop_bbs
                    .pop()
                    .expect("loop builder stack balanced for for");
                self.loop_stack
                    .pop()
                    .expect("loop label stack balanced for for");
                bb.bind_label(continue_label, self.bytecode.il_mut());

                if let Some(step) = step {
                    let mut step_bc = self.do_compile(step);
                    Self::discard_statement_value(&mut step_bc);
                    self.bytecode.extend(step_bc);
                }

                bb.emit_jump_to(top_label, BbJumpKind::Unconditional, self.bytecode.il_mut());
                bb.bind_label(exit_label, self.bytecode.il_mut());

                bb.finalize()
                    .expect("BlockBuilder::finalize: all targeted labels bound");
            }
            Expression::Break => {
                if let Some((_, break_label)) = self.loop_stack.last().copied() {
                    self.emit_loop_jump(Some(break_label), "break", span.into_range());
                } else {
                    self.emit_loop_jump(None, "break", span.into_range());
                }
            }
            Expression::Continue => {
                if let Some((continue_label, _)) = self.loop_stack.last().copied() {
                    self.emit_loop_jump(Some(continue_label), "continue", span.into_range());
                } else {
                    self.emit_loop_jump(None, "continue", span.into_range());
                }
            }
            Expression::Defer { captures, body } => {
                // Layout (emitted into `self.bytecode` so nested Blocks that
                // write in-place stay contiguous with the thunk):
                //   JMP after_thunk
                //   thunk:                ← fn_defers label bound here
                //     <thunk body>
                //     (slots 0..N-1 = use captures, pushed by emit_run_defers)
                //   CONST 0; RETURN
                // after_thunk:
                let mut bb = BlockBuilder::new();
                let after = bb.fresh_label(self.bytecode.il_mut());
                let thunk = bb.fresh_label(self.bytecode.il_mut());
                bb.emit_jump_to(after, BbJumpKind::Unconditional, self.bytecode.il_mut());

                bb.bind_label(thunk, self.bytecode.il_mut());
                let cap_names: Vec<String> = captures.iter().map(|c| (*c).to_string()).collect();
                self.fn_defers.push((thunk, cap_names));

                // Remap locals so capture names occupy slots 0..N-1 inside the
                // thunk (matching the CALL args pushed by emit_run_defers).
                let prev_vars = std::mem::take(&mut self.context.variables);
                for cap in captures {
                    self.context.variables.intern((*cap).to_string());
                }
                let mut body_bc = self.do_compile(body);
                self.bytecode.append(&mut body_bc);
                self.context.variables = prev_vars;

                self.bytecode.push_const(0);
                self.bytecode.push_return();

                bb.bind_label(after, self.bytecode.il_mut());
                bb.finalize()
                    .expect("BlockBuilder::finalize: defer after label bound");
            }
            Expression::Call { name, args } => {
                if let Expression::Identifier(fname) = name.1.as_ref() {
                    if let Some(kind) = self.string_builtin_for_call(fname) {
                        let arg_slice = args.as_deref().unwrap_or(&[]);
                        match kind {
                            crate::typechecking::StringBuiltin::Format => {
                                if let Some((format, rest)) = arg_slice.split_first() {
                                    let params = rest.to_vec();
                                    self.emit_format_expression(format, Some(&params));
                                }
                            }
                            crate::typechecking::StringBuiltin::FromBytes
                            | crate::typechecking::StringBuiltin::ToBytes => {
                                if let Some(native_name) = kind.native_name() {
                                    self.emit_host_native_invoke(native_name, arg_slice);
                                }
                            }
                        }
                        return bytecode;
                    }
                } else if let Expression::QualifiedAccess { owner, member } = name.1.as_ref() {
                    let fqn = format!("{}::{}", owner, member);
                    if let Some(kind) = self.string_builtin_for_call(&fqn) {
                        let arg_slice = args.as_deref().unwrap_or(&[]);
                        match kind {
                            crate::typechecking::StringBuiltin::Format => {
                                if let Some((format, rest)) = arg_slice.split_first() {
                                    let params = rest.to_vec();
                                    self.emit_format_expression(format, Some(&params));
                                }
                            }
                            crate::typechecking::StringBuiltin::FromBytes
                            | crate::typechecking::StringBuiltin::ToBytes => {
                                if let Some(native_name) = kind.native_name() {
                                    self.emit_host_native_invoke(native_name, arg_slice);
                                }
                            }
                        }
                        return bytecode;
                    }
                }
                // `assert` from `prelude::test` (auto-imported).
                if let Expression::Identifier(fname) = name.1.as_ref()
                    && let Some(kind) = self.checker.prelude_fn_in_scope(fname)
                {
                    let arg_slice = args.as_deref().unwrap_or(&[]);
                    match kind {
                        crate::typechecking::PreludeFn::Assert => {
                            self.emit_assert(arg_slice);
                        }
                        crate::typechecking::PreludeFn::Matrix => {
                            // Zero-cost wrap: runtime is the nested data.
                            if let Some(arg) = arg_slice.first() {
                                bytecode.append(&mut self.do_compile(arg));
                            }
                        }
                        crate::typechecking::PreludeFn::Dot
                        | crate::typechecking::PreludeFn::MatMul
                        | crate::typechecking::PreludeFn::Cross => {
                            self.emit_linear_algebra(
                                &mut bytecode,
                                self_id,
                                span.start,
                                span.end,
                                arg_slice,
                            );
                        }
                        crate::typechecking::PreludeFn::Ord
                        | crate::typechecking::PreludeFn::Char => {
                            self.emit_prelude_host_call(arg_slice, kind.as_str());
                        }
                    }
                    return bytecode;
                }
                // `dload` / `declare` / `invoke` after `use ffi::*`.
                if let Expression::Identifier(fname) = name.1.as_ref()
                    && let Some(kind) = self.checker.ffi_fn_in_scope(fname)
                {
                    let arg_slice = args.as_deref().unwrap_or(&[]);
                    match kind {
                        crate::typechecking::FfiBuiltin::Dload => {
                            if let Some(path) = arg_slice.first() {
                                let bc = self.do_compile(path);
                                self.bytecode.extend(bc);
                                self.bytecode.push(Byte::new(Instruction::FfiLoad));
                            }
                        }
                        crate::typechecking::FfiBuiltin::Declare => {
                            self.emit_ffi_declare(*span, arg_slice);
                        }
                        crate::typechecking::FfiBuiltin::Invoke => {
                            self.emit_ffi_invoke(*span, arg_slice);
                        }
                    }
                    return bytecode;
                }
                // `open` / `read` / … after `use io::*` (or `use io::read as …`).
                if let Expression::Identifier(fname) = name.1.as_ref()
                    && let Some(kind) = self.checker.io_fn_in_scope(fname)
                {
                    self.emit_io_host_invoke(kind, args.as_deref().unwrap_or(&[]));
                    return bytecode;
                }
                if let Expression::Identifier(fname) = name.1.as_ref()
                    && let Some(kind) = self.checker.thread_fn_in_scope(fname)
                {
                    self.emit_thread_host_invoke(kind, args.as_deref().unwrap_or(&[]));
                    return bytecode;
                }
                if let Expression::Identifier(fname) = name.1.as_ref()
                    && let Some(registry) = self.checker.host_fn_in_scope(fname)
                {
                    if registry == "env_exec" {
                        self.messages.push(Message::warn(
                            ErrorCode::GenericTypeError,
                            "env::exec runs an external program with the given arguments; only use with trusted inputs"
                                .to_string(),
                            span.into_range(),
                        ));
                    } else if registry == "env_exit" {
                        self.messages.push(Message::warn(
                            ErrorCode::GenericTypeError,
                            "env::exit terminates the process with the given exit code".to_string(),
                            span.into_range(),
                        ));
                    }
                    self.emit_host_native_invoke(registry, args.as_deref().unwrap_or(&[]));
                    return bytecode;
                }

                if let Some(hint) = self_id
                    .and_then(|id| self.checker.existential_method_call_at(id))
                    .or_else(|| {
                        self.checker
                            .existential_method_call_for_span(span.start, span.end)
                    })
                    .cloned()
                {
                    if self.emit_existential_method_call(&mut bytecode, name, args.as_ref(), &hint)
                    {
                        return bytecode;
                    }
                }

                if let Some(hint) = self_id
                    .and_then(|id| self.checker.bound_method_call_at(id))
                    .or_else(|| {
                        self.checker
                            .bound_method_call_for_span(span.start, span.end)
                    })
                    .cloned()
                {
                    if hint.has_receiver
                        && let Expression::Access(recv, _) = name.1.as_ref()
                    {
                        bytecode.append(&mut self.do_compile(recv));
                    }
                    if let Some(items) = args {
                        for arg in items {
                            self.append_with_existential_pack(&mut bytecode, arg);
                        }
                    }
                    let dict_name = format!("__dict{}", hint.dict_index);
                    if let Some(dict_slot) = self.lookup_slot(&dict_name) {
                        // Hidden trailing dictionary argument for sibling/default
                        // dispatch inside the selected implementation.
                        bytecode.push_load(dict_slot);
                        bytecode.push_load(dict_slot);
                        bytecode.push_const(hint.method_slot as i32);
                        bytecode.push_index();
                        bytecode.push(
                            Byte::new(Instruction::CallIndirect)
                                .with_operand_u32(hint.arity as u32 + 1),
                        );
                    } else {
                        let mut message = Message::error(
                            ErrorCode::UnknownFunction,
                            "Missing trait dictionary".to_string(),
                            span.into_range(),
                        );
                        message.push(DiagLabel::new(
                            format!("dictionary slot `{}` is not available", dict_name),
                            span.into_range(),
                        ));
                        self.messages.push(message);
                    }
                    return bytecode;
                }

                // Method call: `recv.method(args)`.
                if let Expression::Access(recv, method) = name.1.borrow() {
                    // Ground trait method (`recv.into()`, …): typechecker
                    // discharged a concrete instance into `call_dicts_at`.
                    // Emit receiver + args + dictionary, then CallIndirect to
                    // the instance method (dict ABI, `dict_arity = 1`).
                    let ground_trait = self_id
                        .and_then(|id| self.checker.call_dicts_at(id))
                        .or_else(|| self.checker.call_dicts_for_span(span.start, span.end))
                        .and_then(|dicts| dicts.first())
                        .and_then(|instance| {
                            let fqn = instance.method_fqns.get(*method)?.clone();
                            let offset = *self.functions.get(&fqn)?;
                            Some((instance.class.clone(), instance.args.clone(), offset))
                        });
                    if let Some((class, inst_args, offset)) = ground_trait {
                        bytecode.append(&mut self.do_compile(recv));
                        // Box the receiver when the instance method prologue
                        // expects an unbox (same contract as Eq/Ord direct calls).
                        // Prefer `receiver_type` for identifiers/access; fall
                        // back to `codegen_expr_ty` so inline receivers like
                        // `new Celsius(0).into()` still get boxed.
                        // Peel Constructor/Sum → Con so `ty_to_value_tag` matches
                        // instance-head unbox tags (Con(enum) → Instance). Raw
                        // Constructor types returned None and skipped boxing.
                        if let Some(recv_ty) = self
                            .receiver_type(recv)
                            .or_else(|| self.codegen_expr_ty(recv))
                        {
                            let box_ty = Self::show_lookup_ty_for_instance(&recv_ty);
                            Self::emit_box_if_needed(&mut bytecode, &box_ty);
                        }
                        let mut nargs = 1u32; // receiver
                        if let Some(items) = args {
                            for arg in items {
                                self.append_with_existential_pack(&mut bytecode, arg);
                                nargs += 1;
                            }
                        }
                        if Self::emit_instance_dict(
                            &mut bytecode,
                            &class,
                            &inst_args,
                            &self.checker,
                            &self.functions,
                        ) {
                            nargs += 1; // trailing dictionary
                        }
                        Self::emit_call_indirect(&mut bytecode, offset as u32, nargs);
                        return bytecode;
                    }

                    // Same fallback as ground-trait calls: inline receivers
                    // like `(new Point(1, 2)).sum()` are not identifiers, so
                    // `receiver_type` alone used to leave `owner` empty.
                    let recv_ty = self
                        .receiver_type(recv)
                        .or_else(|| self.codegen_expr_ty(recv));
                    let owner = recv_ty
                        .as_ref()
                        .and_then(|ty| {
                            Checker::class_name_of_ty(ty)
                                .filter(|n| self.checker.is_class(n))
                                .map(|n| n.to_string())
                        })
                        .unwrap_or_default();
                    let fqn = self
                        .context
                        .methods
                        .get(&owner)
                        .and_then(|m| m.get(*method))
                        .cloned();
                    if let Some(fqn_base) = fqn {
                        let nargs = args.as_ref().map(|items| items.len()).unwrap_or(0);
                        let fqn = if let Some((fa, is_rest)) =
                            self.checker.selected_overload_at(span.start, span.end)
                        {
                            let keyed = overload_fn_key(&fqn_base, fa, is_rest);
                            if self.functions.contains_key(&keyed) {
                                keyed
                            } else {
                                fqn_base.clone()
                            }
                        } else if self.checker.is_overloaded(&fqn_base) {
                            // Forward call inside an impl that later gained
                            // more overloads — TC may not have recorded a
                            // selection (set had size 1 at infer time).
                            self.checker
                                .select_overload(&fqn_base, nargs)
                                .map(|c| overload_fn_key(&fqn_base, c.fixed_arity, c.is_rest))
                                .filter(|k| self.functions.contains_key(k))
                                .unwrap_or(fqn_base)
                        } else {
                            fqn_base
                        };
                        if let Some(offset) = self.functions.get(&fqn).copied() {
                            // Push receiver first (slot 0), then args
                            // (reordered when any arg is named; rest packed).
                            bytecode.append(&mut self.do_compile(recv));
                            let nargs = if let Some(items) = args {
                                self.emit_call_args_with_rest(&fqn, items, &mut bytecode, false)
                            } else {
                                0
                            };
                            bytecode.push(
                                Byte::new(Instruction::CALL)
                                    .with_call_packed(1 + nargs, offset as u32),
                            );
                        } else {
                            let mut message = Message::error(
                                ErrorCode::UnknownFunction,
                                "Unknown method".to_string(),
                                span.into_range(),
                            );
                            message.push(DiagLabel::new(
                                format!("Unable to call unknown method '{}'", fqn),
                                span.into_range(),
                            ));
                            self.messages.push(message);
                        }
                    } else {
                        let mut message = Message::error(
                            ErrorCode::UnknownFunction,
                            "Unknown method".to_string(),
                            span.into_range(),
                        );
                        message.push(DiagLabel::new(
                            format!("Unable to call method '{}' on '{}'", method, owner),
                            span.into_range(),
                        ));
                        self.messages.push(message);
                    }
                } else {
                    if matches!(name.1.as_ref(), Expression::Lambda { .. }) {
                        let arg_slice = args.as_deref().unwrap_or(&[]);
                        self.consume_spread_emit_ids(arg_slice);
                        let flat_args = self.flatten_call_args_for_emit(arg_slice);
                        for arg in &flat_args {
                            bytecode.append(&mut self.do_compile(arg));
                        }
                        bytecode.append(&mut self.do_compile(name));
                        bytecode.push(
                            Byte::new(Instruction::CallIndirect)
                                .with_operand_u32(flat_args.len() as u32),
                        );
                        return bytecode;
                    }
                    if let Expression::Identifier(raw) = name.1.as_ref() {
                        if *raw == "len" {
                            let provided = args.as_ref().map(|items| items.len()).unwrap_or(0);
                            if let Some(items) = args
                                && items.len() == 1
                            {
                                // Prefer compile-time length when known from
                                // literals / static types.
                                if let Some(ConstValue::Int(n)) =
                                    crate::const_fold::eval_expr(ast, self.const_env())
                                {
                                    self.discard_compile(&items[0]);
                                    self.emit_const_value(
                                        &ConstValue::Int(n),
                                        &mut bytecode,
                                    );
                                    return bytecode;
                                }
                                if let Some(n) = self.static_len_of(&items[0]) {
                                    bytecode.append(&mut self.do_compile(&items[0]));
                                    bytecode.push_pop();
                                    self.emit_const_value(
                                        &ConstValue::Int(n as i64),
                                        &mut bytecode,
                                    );
                                    return bytecode;
                                }
                                // Structural aggregates → ArrayLen. Custom types
                                // with `Length` use the instance method below.
                                let arg_ty = self.codegen_expr_ty(&items[0]).map(|ty| {
                                    crate::typechecking::subst::apply_ty_prune(
                                        self.checker.subst(),
                                        &ty,
                                    )
                                });
                                let structural = arg_ty.as_ref().is_some_and(|ty| {
                                    Checker::is_structural_len_ty_for_codegen(ty)
                                });
                                if structural || arg_ty.is_none() {
                                    bytecode.append(&mut self.do_compile(&items[0]));
                                    bytecode.push(Byte::new(Instruction::ArrayLen));
                                    return bytecode;
                                }
                                if let Some(ty) = arg_ty.as_ref()
                                    && let Some(fqn) = self
                                        .checker
                                        .instance_method_fqn("Length", std::slice::from_ref(ty), "len")
                                        .map(str::to_string)
                                    && let Some(&offset) = self.functions.get(&fqn)
                                {
                                    bytecode.append(&mut self.do_compile(&items[0]));
                                    Self::emit_call_indirect(
                                        &mut bytecode,
                                        offset as u32,
                                        1,
                                    );
                                    return bytecode;
                                }
                                // Bound Length calls are handled earlier via
                                // BoundMethodCall; if we get here without an
                                // instance, fall through to report unknown fn.
                            } else {
                                let mut message = Message::error(
                                    ErrorCode::TooManyArguments,
                                    "Invalid len call".to_string(),
                                    span.into_range(),
                                );
                                message.push(DiagLabel::new(
                                    format!("len expects 1 argument, got {}", provided),
                                    span.into_range(),
                                ));
                                self.messages.push(message);
                                return bytecode;
                            }
                        }
                    }

                    let identifier = self.resolve_variable_checked(name);
                    let n = self
                        .aliases
                        .get(&identifier)
                        .cloned()
                        .unwrap_or_else(|| identifier.clone());
                    // Non-entry modules register `ns::name`, but sibling
                    // calls use the bare name. Typecheck inserts bare
                    // names so TC can pass while codegen misses — retry
                    // the current module FQN before reporting unknown.
                    let n = if self.functions.contains_key(&n)
                        || self.extern_runtime_functions.contains_key(&n)
                        || self.native.contains_key(&n)
                    {
                        n
                    } else if !self.namespace.is_empty() {
                        let qualified = format!("{}::{}", self.namespace, n);
                        if self.functions.contains_key(&qualified)
                            || self
                                .checker
                                .selected_overload_at(span.start, span.end)
                                .is_some()
                        {
                            qualified
                        } else {
                            n
                        }
                    } else {
                        n
                    };

                    // Arity-overload table key (when the typechecker selected one).
                    let n = if let Some((fa, is_rest)) =
                        self.checker.selected_overload_at(span.start, span.end)
                    {
                        let keyed = overload_fn_key(&n, fa, is_rest);
                        if self.functions.contains_key(&keyed) {
                            keyed
                        } else {
                            // Try bare-name key when FQN wasn't used at registration.
                            let simple = n.rsplit("::").next().unwrap_or(&n);
                            let keyed_simple = overload_fn_key(simple, fa, is_rest);
                            if self.functions.contains_key(&keyed_simple) {
                                keyed_simple
                            } else {
                                n
                            }
                        }
                    } else {
                        n
                    };

                    if let Some(&(lib_slot, fn_id_slot)) = self.extern_runtime_functions.get(&n) {
                        // Same discipline as HostInvoke: emit lib/fn_id first,
                        // then compile args onto `self.bytecode`. Nested IO
                        // HostInvoke writes directly to `self.bytecode` and
                        // returns an empty slice — staging args into a side
                        // Vec first left those bytes *before* the LOADs, so
                        // MakeTuple packed the wrong stack values.
                        let arity = if let Some(items) = args {
                            items.len()
                        } else {
                            0
                        };
                        let variadic = self.checker.is_extern_variadic(&n);
                        let depth_on_entry = self.expr_depth;
                        self.bytecode.push_load(lib_slot);
                        self.bytecode.push_load(fn_id_slot);
                        self.expr_depth = depth_on_entry + 2;
                        if let Some(items) = args {
                            for arg in items {
                                let mut arg_bc = self.do_compile(arg);
                                self.bytecode.append(&mut arg_bc);
                                self.expr_depth += 1;
                            }
                        }
                        self.bytecode.push_make_tuple(arity as u32);
                        let mut operand = arity as u32 & 0xFFFF;
                        if variadic {
                            let call_span = (span.start, span.end);
                            let arg_refs: Vec<_> = args
                                .as_ref()
                                .map(|items| items.iter().collect())
                                .unwrap_or_default();
                            if let Some(tags) = resolve_variadic_ffi_tags(
                                &self.checker,
                                call_span,
                                &arg_refs,
                                &mut self.messages,
                            ) {
                                for &(tag, aux) in &tags {
                                    emit_ffi_type_const(&mut self.bytecode, tag, aux);
                                }
                                self.bytecode.push_make_tuple(tags.len() as u32);
                                operand |= 1 << 16;
                            }
                        }
                        self.bytecode
                            .push(Byte::new(Instruction::FfiInvoke).with_operand_u32(operand));
                        self.expr_depth = depth_on_entry;
                        self.emit_result_unwrap_or_panic();
                    } else if let Some(&native_id) = self.native.get(&n) {
                        // Same stack order as `emit_io_host_invoke`: id first,
                        // then args (nested HostInvoke may write to `self.bytecode`).
                        let arity = if let Some(items) = args {
                            items.len()
                        } else {
                            0
                        };
                        let depth_on_entry = self.expr_depth;
                        self.bytecode
                            .push(Byte::new(Instruction::CONST).with_value_u32(native_id as u32));
                        self.expr_depth = depth_on_entry + 1;
                        if let Some(items) = args {
                            for arg in items {
                                let mut arg_bc = self.do_compile(arg);
                                self.bytecode.append(&mut arg_bc);
                                self.expr_depth += 1;
                            }
                        }
                        self.bytecode.push_make_tuple(arity as u32);
                        self.bytecode.push_host_invoke(arity as u32);
                        self.expr_depth = depth_on_entry;
                    } else if let Some(offset) = self.functions.get(&n).copied() {
                        let mono_offset = self.mono_call_offset(&n, args.as_ref());
                        let target_offset = mono_offset.unwrap_or(offset);
                        let lookup_name = strip_overload_key(&n).to_string();
                        let is_generic =
                            self.checker.is_generic_fn(&lookup_name) && mono_offset.is_none();
                        let arg_slice = args.as_deref().unwrap_or(&[]);
                        self.consume_spread_emit_ids(arg_slice);
                        let flat_arg_slice = self.flatten_call_args_for_emit(arg_slice);

                        if !is_generic
                            && !self.coroutine_fns.contains(&n)
                            && !self.coroutine_fns.contains(&lookup_name)
                            && self.try_emit_inline_direct_call(&n, Some(arg_slice), &mut bytecode)
                        {
                            return bytecode;
                        }

                        // One-level self-unroll: peel recursive callee body once;
                        // nested self-calls remain CALL/Entry.
                        if !is_generic
                            && !self.coroutine_fns.contains(&n)
                            && !self.coroutine_fns.contains(&lookup_name)
                            && self.try_emit_self_unroll_call(&n, Some(arg_slice), &mut bytecode)
                        {
                            return bytecode;
                        }

                        // Caller-side base-case peel: cmp-jmp before CALL when
                        // the callee opens with fused/unfused compare + imm/slot return.
                        let peel_indirect = is_instance_method_fqn(&self.checker, &lookup_name);
                        if !is_generic
                            && !self.coroutine_fns.contains(&n)
                            && !self.coroutine_fns.contains(&lookup_name)
                            && self.try_emit_predicate_peel_call(
                                &n,
                                Some(arg_slice),
                                &mut bytecode,
                                target_offset as u32,
                                peel_indirect,
                            )
                        {
                            return bytecode;
                        }

                        // Partial application → MakeFn (not CALL).
                        let (fa, is_rest) = self
                            .checker
                            .selected_overload_at(span.start, span.end)
                            .or_else(|| {
                                let names = self.checker.fn_param_names(&lookup_name)?;
                                let rest = self.checker.fn_has_rest(&lookup_name);
                                let fixed = if rest {
                                    names.len().saturating_sub(1)
                                } else {
                                    names.len()
                                };
                                Some((fixed, rest))
                            })
                            .or_else(|| {
                                self.fn_arities
                                    .get(&lookup_name)
                                    .or_else(|| self.fn_arities.get(&n))
                                    .map(|(a, r)| (*a as usize, *r))
                            })
                            .unwrap_or((0, false));
                        let fill_mask =
                            self.checker
                                .partial_fill_at(span.start, span.end)
                                .or_else(|| {
                                    // Spread args count as their expanded arity, not one slot.
                                    let argc = flat_arg_slice.len();
                                    if !is_rest && fa > 0 && argc < fa {
                                        Some((1u32 << argc).wrapping_sub(1))
                                    } else {
                                        None
                                    }
                                });
                        if let Some(mask) = fill_mask {
                            // Emit filled values in declaration order (already
                            // the order of `flat_arg_slice` after named reorder at TC).
                            for arg in &flat_arg_slice {
                                let value = match arg.1.as_ref() {
                                    Expression::NamedArg(_, v) => v,
                                    _ => arg,
                                };
                                bytecode.append(&mut self.do_compile(value));
                            }
                            let n_filled = mask.count_ones();
                            bytecode.push_const(mask as i32);
                            bytecode.push(
                                Byte::new(Instruction::CodePtr)
                                    .with_operand_u32(target_offset as u32),
                            );
                            bytecode.push(Byte::new(Instruction::MakeFn).with_operand_u32(
                                make_fn_operand(0, n_filled, fa as u32, is_rest),
                            ));
                            return bytecode;
                        }

                        let value_arity = self.emit_call_args_with_rest(
                            &lookup_name,
                            arg_slice,
                            &mut bytecode,
                            is_generic,
                        );

                        // ── Dictionary-passing calling convention ──────────────────
                        // For non-monomorphized generic calls, append one dict tuple
                        // per constraint after the value args. Each dict is a
                        // MakeTuple of method code offsets (CodePtr per method in
                        // declaration order). Builtin and user instances share this
                        // ABI; ground Num/Ord/Eq calls may still monomorphize away
                        // from the shared body, but Show-bound calls always take
                        // this path.
                        let dict_count = if is_generic {
                            let (fixed, rest, pack_rest) =
                                self.split_call_args_for_rest(&lookup_name, arg_slice);
                            let mut call_arg_tys: Vec<crate::typechecking::Ty> = fixed
                                .iter()
                                .map(|arg| {
                                    self.codegen_expr_ty(arg).expect(
                                        "typechecked call argument must have a codegen type",
                                    )
                                })
                                .collect();
                            // Rest-only generics (`T... xs`) have empty `fixed`;
                            // bind `T` from the packed `[T]` / `[T; N]` arg.
                            if pack_rest {
                                call_arg_tys.push(self.synthesize_rest_array_ty(&rest));
                            }
                            let mut forwarded = 0;
                            if let Some(indices) = self_id
                                .and_then(|id| self.checker.forwarded_dicts_at(id))
                                .or_else(|| {
                                    self.checker.forwarded_dicts_for_span(span.start, span.end)
                                })
                                .map(<[usize]>::to_vec)
                            {
                                for dict_index in indices {
                                    if let Some(slot) =
                                        self.lookup_slot(&format!("__dict{}", dict_index))
                                    {
                                        bytecode.push_load(slot);
                                        forwarded += 1;
                                    }
                                }
                            }
                            let call_ret_ty = self.codegen_expr_ty(ast);
                            forwarded
                                + Self::emit_call_site_dicts(
                                    &mut bytecode,
                                    &lookup_name,
                                    &call_arg_tys,
                                    call_ret_ty.as_ref(),
                                    &self.checker,
                                    &self.functions,
                                )
                        } else {
                            0
                        };

                        let arity = value_arity + dict_count as u32;
                        if is_instance_method_fqn(&self.checker, &lookup_name) {
                            Self::emit_call_indirect(&mut bytecode, target_offset as u32, arity);
                        } else if self.coroutine_fns.contains(&lookup_name)
                            || self.coroutine_fns.contains(&n)
                        {
                            bytecode.push(
                                Byte::new(Instruction::MakeCoro)
                                    .with_call_packed(arity, target_offset as u32),
                            );
                        } else {
                            // Packed CALL: arity + target in one opcode.
                            bytecode.push(
                                Byte::new(Instruction::CALL)
                                    .with_call_packed(arity, target_offset as u32),
                            );
                        }
                        // Generic→concrete unbox: only when the return type
                        // parameter was boxed as a top-level argument
                        // (`id<T>(T) -> T`). Nested params (`F<A> -> A`) are
                        // not boxed at construction, so unboxing would zero
                        // a valid immediate (Phase 5 HKT / Container::first).
                        if is_generic && self.generic_return_is_boxed(&lookup_name) {
                            if let Some(call_ty) = self.codegen_expr_ty(ast) {
                                Self::emit_unbox_if_needed(&mut bytecode, &call_ty);
                            }
                        }
                    } else if let Some(slot) = self.lookup_slot(&identifier) {
                        // Local holding a function value: escaped PolyFn
                        // (`let f = show` / `return show`), rank-n parameter, or
                        // a PolyFn returned from another call. Emit args, optional
                        // application dictionaries, then CallIndirect.
                        let arg_slice = args.as_deref().unwrap_or(&[]);
                        self.consume_spread_emit_ids(arg_slice);
                        let flat_args = self.flatten_call_args_for_emit(arg_slice);
                        let value_arity = flat_args.len() as u32;
                        let mut arg_tys = Vec::new();
                        let polyfn_source = self.polyfn_sources.get(&identifier).cloned();
                        // Box for PolyFn locals — including those assigned from a
                        // call that returns a captured PolyFn (no polyfn_sources
                        // entry). Mono ObjFn / partials / lambdas stay unboxed.
                        let needs_arg_box = self.local_call_needs_arg_boxing(&identifier);
                        for arg in &flat_args {
                            self.append_with_existential_pack(&mut bytecode, arg);
                            if needs_arg_box {
                                if let Some(arg_ty) = self.codegen_expr_ty(arg) {
                                    Self::emit_box_if_needed(&mut bytecode, &arg_ty);
                                    arg_tys.push(arg_ty);
                                }
                            }
                        }
                        let mut dict_count = 0u32;
                        if let Some(source) = polyfn_source.as_ref() {
                            if let Some(indices) = self_id
                                .and_then(|id| self.checker.forwarded_dicts_at(id))
                                .or_else(|| {
                                    self.checker.forwarded_dicts_for_span(span.start, span.end)
                                })
                                .map(<[usize]>::to_vec)
                            {
                                for dict_index in indices {
                                    if let Some(dict_slot) =
                                        self.lookup_slot(&format!("__dict{}", dict_index))
                                    {
                                        bytecode.push_load(dict_slot);
                                        dict_count += 1;
                                    }
                                }
                            }
                            let call_ret_ty = self.codegen_expr_ty(ast);
                            dict_count += Self::emit_call_site_dicts(
                                &mut bytecode,
                                source,
                                &arg_tys,
                                call_ret_ty.as_ref(),
                                &self.checker,
                                &self.functions,
                            ) as u32;
                        }
                        // Pack value arity + application dict arity so the VM can
                        // merge captured evidence with apply-site dictionaries.
                        bytecode.push_load(slot);
                        bytecode.push(
                            Byte::new(Instruction::CallIndirect)
                                .with_operand_u32(value_arity | (dict_count << 16)),
                        );
                        // Generic→concrete unbox for polyfn call site.
                        if let Some(call_ty) = self.codegen_expr_ty(ast) {
                            if self.local_polyfn_call_needs_unbox(&identifier, &call_ty) {
                                Self::emit_unbox_if_needed(&mut bytecode, &call_ty);
                            }
                        }
                    } else {
                        let mut message = Message::error(
                            ErrorCode::UnknownFunction,
                            "Unknown function".to_string(),
                            span.into_range(),
                        );
                        message.push(DiagLabel::new(
                            format!("Unable to call unknown function '{}'", n),
                            span.into_range(),
                        ));
                        self.messages.push(message);
                    }
                } // end non-method Call
            }
            Expression::Argument { ty, name: n, .. } => {
                let slot = self.context.variables.intern(n.to_string()) as u32;
                self.record_debug_local(n, slot);
                if ty
                    .as_ref()
                    .is_some_and(|t| matches!(t.1.as_ref(), Expression::Forall { .. }))
                {
                    self.polyfn_vars.insert(n.to_string());
                }
                // bytecode.push(Byte::new(Instruction::LOAD)
            }
            Expression::Type(_)
            | Expression::TypeFun(_, _)
            | Expression::TypeFnSig { .. }
            | Expression::Forall { .. } => {
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
            Expression::TypeClass { name, methods, .. } => {
                for method in methods {
                    match method.1.as_ref() {
                        Expression::AssocTypeDecl { .. } => {
                            // Type-level only — do_compile consumes the NodeId.
                            let _ = self.do_compile(method);
                        }
                        Expression::Function {
                            docs: _,
                            name: method_name,
                            body,
                            ..
                        } => {
                            let has_default = body.as_ref().is_some_and(|b| {
                                !matches!(b.1.as_ref(), Expression::Block(items) if items.is_empty())
                            });
                            if has_default {
                                let fqn =
                                    crate::typechecking::generics::Generics::default_method_fqn(
                                        name,
                                        method_name,
                                    );
                                self.compile_function_output_with_name(method, fqn, &[], 1);
                            } else {
                                self.consume_function_signature_output(method);
                            }
                        }
                        _ => {
                            self.consume_function_signature_output(method);
                        }
                    }
                }
            }
            Expression::TypeClassImpl {
                class,
                args,
                methods,
            } => {
                // Resolve instance heads by AST shape (not span cache). Bare
                // `Option`/`Result` must stay `Con(...)` so FQNs match the
                // typechecker (`Container__Option__first`). Preferring
                // `codegen_expr_ty` here can pick up a misaligned span type
                // (e.g. `unit`) and emit `Container__unit__first` instead.
                let arg_tys: Vec<Ty> = args
                    .iter()
                    .map(|arg| self.codegen_instance_head_ty(arg))
                    .collect();
                for arg in args {
                    bytecode.append(&mut self.do_compile(arg));
                }
                let ty_part = arg_tys
                    .iter()
                    .map(|ty| ty.to_string())
                    .collect::<Vec<_>>()
                    .join("_");
                for method in methods {
                    match method.1.as_ref() {
                        Expression::AssocTypeDef { .. } => {
                            // Type-level only — do_compile consumes wrapper + RHS IDs.
                            let _ = self.do_compile(method);
                        }
                        Expression::Function {
                            docs: _,
                            name: method_name, ..
                        } => {
                            let fqn = format!("{}__{}__{}", class, ty_part, method_name);
                            let unbox_tys =
                                self.instance_method_unbox_tys(class, method_name, &arg_tys);
                            self.compile_function_output_with_name(method, fqn, &unbox_tys, 1);
                        }
                        Expression::Method(_, body) => {
                            let _method_wrapper_id = self.checker.id_table().ids()[self.emit_idx];
                            self.emit_idx += 1;
                            if let Expression::Function {
                                docs: _,
                                name: method_name, ..
                            } = body.1.as_ref()
                            {
                                let fqn = format!("{}__{}__{}", class, ty_part, method_name);
                                let unbox_tys =
                                    self.instance_method_unbox_tys(class, method_name, &arg_tys);
                                self.compile_function_output_with_name(body, fqn, &unbox_tys, 1);
                            } else {
                                self.consume_function_signature_output(body);
                            }
                        }
                        _ => {
                            self.consume_function_signature_output(method);
                        }
                    }
                }
            }
            Expression::AssocTypeDecl { .. } | Expression::TypeProjection { .. } => {
                // Type-level only — no bytecode (NodeId already consumed by do_compile).
            }
            Expression::AssocTypeDef { ty, .. } => {
                bytecode.append(&mut self.do_compile(ty));
            }
            Expression::Identifier(n) => {
                let resolved = self
                    .aliases
                    .get(*n)
                    .cloned()
                    .unwrap_or_else(|| n.to_string());
                if let Some(v) = self
                    .const_env()
                    .get(&resolved)
                    .or_else(|| self.const_env().get(*n))
                    .cloned()
                {
                    self.emit_const_value(&v, &mut bytecode);
                } else if let Some(v) = self
                    .static_const_values
                    .get(&resolved)
                    .filter(|_| self.checker.is_static_const_fqn(&resolved))
                    .cloned()
                {
                    self.emit_const_value(&v, &mut bytecode);
                } else if let Some(v) = self
                    .static_const_values
                    .get(&self.qualify_static_fqn(n))
                    .filter(|_| {
                        self.checker
                            .is_static_const_fqn(&self.qualify_static_fqn(n))
                    })
                    .cloned()
                {
                    self.emit_const_value(&v, &mut bytecode);
                } else if let Some(static_slot) = self
                    .checker
                    .static_slot_index(&resolved)
                    .or_else(|| self.checker.static_slot_for_module_name(n))
                {
                    bytecode.push(Byte::new(Instruction::LoadStatic).with_operand_u32(static_slot));
                } else if let Some(slot) = self.lookup_slot(n) {
                    bytecode.push_load(slot);
                } else {
                    // Not a local variable — check if it's a generic function
                    // escaping into a non-call position (e.g. `let f = id;`).
                    // In that case, emit MakePolyFn instead of a direct CALL offset,
                    // so the variable holds an ObjPolyFn that CallIndirect can use.
                    let resolved_n = self
                        .aliases
                        .get(*n)
                        .cloned()
                        .unwrap_or_else(|| n.to_string());
                    if self.checker.is_generic_fn(&resolved_n) {
                        if let Some(&entry_offset) = self.functions.get(&resolved_n) {
                            // Phase 4: constrained generics always escape via
                            // MakePolyFnCapture. Fill slots from in-scope
                            // `__dictN` or concrete instance synthesis; leave
                            // null only when evidence is unavailable (e.g.
                            // top-level `let f = show`).
                            let escape_ty = self.codegen_expr_ty(ast);
                            let dict_arity = self.emit_polyfn_escape_dicts(
                                &mut bytecode,
                                &resolved_n,
                                escape_ty.as_ref(),
                            );
                            if dict_arity == 0 {
                                bytecode.push(
                                    Byte::new(Instruction::MakePolyFn)
                                        .with_operand_u32(entry_offset as u32),
                                );
                            } else {
                                bytecode.push(
                                    Byte::new(Instruction::CodePtr)
                                        .with_operand_u32(entry_offset as u32),
                                );
                                bytecode.push(
                                    Byte::new(Instruction::MakePolyFnCapture)
                                        .with_operand_u32(dict_arity as u32),
                                );
                            }
                        } else {
                            // Function not yet compiled (forward reference) — fall
                            // through to the unknown-variable diagnostic.
                            let mut message = Message::error(
                                ErrorCode::UnknownValue,
                                "Unknown generic function".to_string(),
                                span.into_range(),
                            );
                            message.push(DiagLabel::new(
                                format!("Generic function '{}' not found in bytecode", n),
                                span.into_range(),
                            ));
                            self.messages.push(message);
                        }
                    } else {
                        // Monomorphic function in value position → MakeFn.
                        let (fa, is_rest, entry_key) = if let Some((fa, is_rest)) =
                            self.checker.selected_overload_at(span.start, span.end)
                        {
                            let keyed = overload_fn_key(&resolved_n, fa, is_rest);
                            (fa, is_rest, keyed)
                        } else if self.checker.is_overloaded(&resolved_n) {
                            // Ambiguous — typechecker should have diagnosed.
                            let mut message = Message::error(
                                ErrorCode::UnknownValue,
                                "Ambiguous overload in value position".to_string(),
                                span.into_range(),
                            );
                            message.push(DiagLabel::new(
                                format!(
                                    "Cannot reify overloaded `{}` without a type annotation",
                                    n
                                ),
                                span.into_range(),
                            ));
                            self.messages.push(message);
                            return bytecode;
                        } else {
                            let rest = self.checker.fn_has_rest(&resolved_n);
                            let fa = self
                                .checker
                                .fn_param_names(&resolved_n)
                                .map(|names| {
                                    if rest {
                                        names.len().saturating_sub(1)
                                    } else {
                                        names.len()
                                    }
                                })
                                .unwrap_or(0);
                            (fa, rest, resolved_n.clone())
                        };
                        if let Some(&entry_offset) = self
                            .functions
                            .get(&entry_key)
                            .or_else(|| self.functions.get(&resolved_n))
                        {
                            // Prefer codegen-recorded arity: multi-file
                            // `check_program` clears `fn_param_names`, so
                            // imported names would otherwise MakeFn with
                            // arity 0 and break `spawn(f, arg)`.
                            let (fa, is_rest) = self
                                .fn_arities
                                .get(&entry_key)
                                .or_else(|| self.fn_arities.get(&resolved_n))
                                .copied()
                                .map(|(a, r)| (a as usize, r))
                                .unwrap_or((fa, is_rest));
                            bytecode.push_const(0);
                            bytecode.push(
                                Byte::new(Instruction::CodePtr)
                                    .with_operand_u32(entry_offset as u32),
                            );
                            bytecode.push(
                                Byte::new(Instruction::MakeFn)
                                    .with_operand_u32(make_fn_operand(0, 0, fa as u32, is_rest)),
                            );
                        } else {
                            let mut message = Message::error(
                                ErrorCode::UnknownValue,
                                "Unknown variable".to_string(),
                                span.into_range(),
                            );
                            message.push(DiagLabel::new(
                                format!("Unknown variable '{}'", n),
                                span.into_range(),
                            ));
                            self.messages.push(message);
                        }
                    }
                }
            }
            // --- If codegen ---
            // Layout: c1, JMPF1, b1, JMP1, c2, JMPF2, b2, JMP2, b3, [end]
            Expression::If(branches) => {
                if self.try_compile_const_if(branches) {
                    return bytecode;
                }
                // `if (!c) { A } else { B }` ≡ `if (c) { B } else { A }` — exposes
                // BinSlot*/Cmp JMPF fusion (avoids LogNotJmpf after fused cond).
                let inverted = Self::try_invert_not_if_else(branches);
                let branches: &[Output<'_>] = inverted.as_deref().unwrap_or(branches);

                let mut bb = BlockBuilder::new();
                let end_label = bb.fresh_label(self.bytecode.il_mut());
                let mut branch_start_labels: Vec<Option<crate::block_builder::Label>> =
                    Vec::with_capacity(branches.len());
                for i in 0..branches.len() {
                    if i + 1 < branches.len() {
                        branch_start_labels.push(Some(bb.fresh_label(self.bytecode.il_mut())));
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
                    if i > 0
                        && let Some(prev_label) = branch_start_labels[i - 1]
                    {
                        let _target = self.bytecode.len() as u32;
                        bb.bind_label(prev_label, self.bytecode.il_mut());
                    }

                    // Emit cond then JMPF (including single-branch if).
                    if let Some(cond) = cond_opt {
                        let cond_bc = self.do_compile(cond);
                        self.bytecode.extend(cond_bc);
                        let jmpf_target = branch_start_labels[i].unwrap_or(end_label);
                        bb.emit_jump_to(
                            jmpf_target,
                            BbJumpKind::JumpIfFalse,
                            self.bytecode.il_mut(),
                        );
                    }

                    // Body after cond+JMPF so Print/nested control-flow offsets stay correct.
                    let body_bc = self.do_compile(body);
                    self.bytecode.extend(body_bc);

                    // Emit a `JMP → end` placeholder for every
                    // branch except the last. The last branch falls
                    // through to `end_pos`.
                    if i + 1 < branches.len() {
                        bb.emit_jump_to(
                            end_label,
                            BbJumpKind::Unconditional,
                            self.bytecode.il_mut(),
                        );
                    }
                }

                // Bind `end_label` to the current bytecode position
                // (= past the last branch's body / JMP). This patches
                // every JMP → end placeholder AND the last JMPF
                // placeholder (if any).
                bb.bind_label(end_label, self.bytecode.il_mut());

                // Validate: every label that had a pending jump must
                // be bound. (Allocated-but-unused labels are allowed.)
                bb.finalize()
                    .expect("BlockBuilder::finalize: all targeted labels bound");
            }
            Expression::Le(lhs, rhs) => {
                let hint = self_id
                    .and_then(|id| self.checker.bound_operator_call_at(id))
                    .or_else(|| {
                        self.checker
                            .bound_operator_call_for_span(span.start, span.end)
                    })
                    .cloned();
                if let Some(hint) = hint
                    && self.emit_bound_operator_call(
                        &mut bytecode,
                        lhs,
                        rhs,
                        hint.dict_index,
                        hint.method_slot,
                    )
                {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else if self.emit_concrete_operator_call(&mut bytecode, lhs, rhs, "Lt", "lt") {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else {
                    let is_float = self.compile_binary_operands(&mut bytecode, lhs, rhs);
                    bytecode.push(Byte::new(if is_float {
                        Instruction::LEF
                    } else {
                        Instruction::LE
                    }));
                }
            }
            Expression::Gt(lhs, rhs) => {
                let hint = self_id
                    .and_then(|id| self.checker.bound_operator_call_at(id))
                    .or_else(|| {
                        self.checker
                            .bound_operator_call_for_span(span.start, span.end)
                    })
                    .cloned();
                if let Some(hint) = hint
                    && self.emit_bound_operator_call(
                        &mut bytecode,
                        lhs,
                        rhs,
                        hint.dict_index,
                        hint.method_slot,
                    )
                {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else if self.emit_concrete_operator_call(&mut bytecode, lhs, rhs, "Gt", "gt") {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else {
                    let is_float = self.compile_binary_operands(&mut bytecode, lhs, rhs);
                    bytecode.push(Byte::new(if is_float {
                        Instruction::GTF
                    } else {
                        Instruction::GT
                    }));
                }
            }
            Expression::Leq(lhs, rhs) => {
                let hint = self_id
                    .and_then(|id| self.checker.bound_operator_call_at(id))
                    .or_else(|| {
                        self.checker
                            .bound_operator_call_for_span(span.start, span.end)
                    })
                    .cloned();
                if let Some(hint) = hint
                    && self.emit_bound_operator_call(
                        &mut bytecode,
                        lhs,
                        rhs,
                        hint.dict_index,
                        hint.method_slot,
                    )
                {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else if self.emit_concrete_operator_call(&mut bytecode, lhs, rhs, "Le", "le") {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else {
                    let is_float = self.compile_binary_operands(&mut bytecode, lhs, rhs);
                    bytecode.push(Byte::new(if is_float {
                        Instruction::LEQF
                    } else {
                        Instruction::LEQ
                    }));
                }
            }
            Expression::Geq(lhs, rhs) => {
                let hint = self_id
                    .and_then(|id| self.checker.bound_operator_call_at(id))
                    .or_else(|| {
                        self.checker
                            .bound_operator_call_for_span(span.start, span.end)
                    })
                    .cloned();
                if let Some(hint) = hint
                    && self.emit_bound_operator_call(
                        &mut bytecode,
                        lhs,
                        rhs,
                        hint.dict_index,
                        hint.method_slot,
                    )
                {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else if self.emit_concrete_operator_call(&mut bytecode, lhs, rhs, "Ge", "ge") {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else {
                    let is_float = self.compile_binary_operands(&mut bytecode, lhs, rhs);
                    bytecode.push(Byte::new(if is_float {
                        Instruction::GEQF
                    } else {
                        Instruction::GEQ
                    }));
                }
            }
            Expression::Eq(lhs, rhs) => {
                let hint = self_id
                    .and_then(|id| self.checker.bound_operator_call_at(id))
                    .or_else(|| {
                        self.checker
                            .bound_operator_call_for_span(span.start, span.end)
                    })
                    .cloned();
                if let Some(hint) = hint
                    && self.emit_bound_operator_call(
                        &mut bytecode,
                        lhs,
                        rhs,
                        hint.dict_index,
                        hint.method_slot,
                    )
                {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else if self.emit_concrete_operator_call(&mut bytecode, lhs, rhs, "Eq", "eq") {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else {
                    binary!(bytecode, self, lhs, rhs, Byte::new(Instruction::EQ));
                }
            }
            Expression::Not(lhs) => {
                unary!(bytecode, self, lhs, Byte::new(Instruction::NOT));
            }
            Expression::LogicalNot(lhs) => {
                unary!(bytecode, self, lhs, Byte::new(Instruction::LogNot));
            }
            Expression::Negate(lhs) => {
                if self.try_emit_matrix_op(&mut bytecode, self_id, span.start, span.end, lhs, None)
                {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else if self.try_emit_aggregate_arith(
                    &mut bytecode,
                    self_id,
                    span.start,
                    span.end,
                    lhs,
                    None,
                    crate::typechecking::AggregateOp::Neg,
                ) {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else {
                    unary!(bytecode, self, lhs, Byte::new(Instruction::NEG));
                }
            }
            Expression::Add(lhs, rhs) => {
                // `allow_mul_shl` is irrelevant for Add (strength_mul_to_shl
                // only matches Mul); pass true for the shared helper API.
                if self.try_emit_folded_expr(ast, &mut bytecode, true) {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else if self.try_emit_auto_par_binop(
                    lhs,
                    rhs,
                    Instruction::ADD,
                    Instruction::ADDF,
                ) {
                    // Wrote fork-join control flow to self.bytecode.
                } else if self.try_emit_matrix_op(
                    &mut bytecode,
                    self_id,
                    span.start,
                    span.end,
                    lhs,
                    Some(rhs),
                ) {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else if self.try_emit_aggregate_arith(
                    &mut bytecode,
                    self_id,
                    span.start,
                    span.end,
                    lhs,
                    Some(rhs),
                    crate::typechecking::AggregateOp::Add,
                ) {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else if self.is_string_expr(lhs) && self.is_string_expr(rhs) {
                    self.emit_raw_string_literal(&mut bytecode, "%s%s");
                    bytecode.append(&mut self.do_compile(lhs));
                    bytecode.append(&mut self.do_compile(rhs));
                    bytecode.push(Byte::new(Instruction::FORMAT).with_operand_u32(2));
                } else if let Some(hint) = self_id
                    .and_then(|id| self.checker.bound_operator_call_at(id))
                    .or_else(|| {
                        self.checker
                            .bound_operator_call_for_span(span.start, span.end)
                    })
                    .cloned()
                    && self.emit_bound_operator_call(
                        &mut bytecode,
                        lhs,
                        rhs,
                        hint.dict_index,
                        hint.method_slot,
                    )
                {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else {
                    let is_float = likely(self.compile_binary_operands(&mut bytecode, lhs, rhs));
                    bytecode.push(Byte::new(if is_float {
                        Instruction::ADDF
                    } else {
                        Instruction::ADD
                    }));
                }
            }
            Expression::Sub(lhs, rhs) => {
                if self.try_emit_auto_par_binop(
                    lhs,
                    rhs,
                    Instruction::SUB,
                    Instruction::SUBF,
                ) {
                    // Wrote fork-join control flow to self.bytecode.
                } else if self.try_emit_matrix_op(
                    &mut bytecode,
                    self_id,
                    span.start,
                    span.end,
                    lhs,
                    Some(rhs),
                ) {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else if self.try_emit_aggregate_arith(
                    &mut bytecode,
                    self_id,
                    span.start,
                    span.end,
                    lhs,
                    Some(rhs),
                    crate::typechecking::AggregateOp::Sub,
                ) {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else if let Some(hint) = self_id
                    .and_then(|id| self.checker.bound_operator_call_at(id))
                    .or_else(|| {
                        self.checker
                            .bound_operator_call_for_span(span.start, span.end)
                    })
                    .cloned()
                    && self.emit_bound_operator_call(
                        &mut bytecode,
                        lhs,
                        rhs,
                        hint.dict_index,
                        hint.method_slot,
                    )
                {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else {
                    let is_float = likely(self.compile_binary_operands(&mut bytecode, lhs, rhs));
                    bytecode.push(Byte::new(if is_float {
                        Instruction::SUBF
                    } else {
                        Instruction::SUB
                    }));
                }
            }
            Expression::Mul(lhs, rhs) => {
                // Matrix / aggregate Mul take precedence over scalar fold and
                // `x * 2^n` → SHL (matmul and element-wise vector ops).
                if self.try_emit_matrix_op(
                    &mut bytecode,
                    self_id,
                    span.start,
                    span.end,
                    lhs,
                    Some(rhs),
                ) {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else if self.try_emit_auto_par_binop(
                    lhs,
                    rhs,
                    Instruction::MUL,
                    Instruction::MULF,
                ) {
                    // Wrote fork-join control flow to self.bytecode.
                } else if self.try_emit_aggregate_arith(
                    &mut bytecode,
                    self_id,
                    span.start,
                    span.end,
                    lhs,
                    Some(rhs),
                    crate::typechecking::AggregateOp::Mul,
                ) {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else {
                    // Prefer trait/`Mul` dictionary dispatch over primitive
                    // `x * 2^n` → SHL when the checker recorded a bound operator
                    // (non-primitive `T * 2^n` must not emit int SHL).
                    // `try_emit_folded_expr` also const-folds literal×literal and
                    // identity-reduces `* 1` before bound/primitive fallback.
                    let bound_mul = self_id
                        .and_then(|id| self.checker.bound_operator_call_at(id))
                        .or_else(|| {
                            self.checker
                                .bound_operator_call_for_span(span.start, span.end)
                        })
                        .cloned();
                    if self.try_emit_folded_expr(ast, &mut bytecode, bound_mul.is_none()) {
                        // Intentional empty body: the emit/try_emit call in the
                        // condition already wrote bytecode as a side effect.
                    } else if let Some(hint) = bound_mul
                        && self.emit_bound_operator_call(
                            &mut bytecode,
                            lhs,
                            rhs,
                            hint.dict_index,
                            hint.method_slot,
                        )
                    {
                        // Intentional empty body: the emit/try_emit call in the
                        // condition already wrote bytecode as a side effect.
                    } else {
                        let is_float =
                            likely(self.compile_binary_operands(&mut bytecode, lhs, rhs));
                        bytecode.push(Byte::new(if is_float {
                            Instruction::MULF
                        } else {
                            Instruction::MUL
                        }));
                    }
                }
            }
            Expression::Mod(lhs, rhs) => {
                if self.try_emit_aggregate_arith(
                    &mut bytecode,
                    self_id,
                    span.start,
                    span.end,
                    lhs,
                    Some(rhs),
                    crate::typechecking::AggregateOp::Mod,
                ) {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else if self.operand_is_open_ty(lhs) || self.operand_is_open_ty(rhs) {
                    let _ = self.compile_binary_operands(&mut bytecode, lhs, rhs);
                    bytecode.push(Byte::new(Instruction::DynMod));
                } else {
                    let is_float = likely(self.compile_binary_operands(&mut bytecode, lhs, rhs));
                    bytecode.push(Byte::new(if is_float {
                        Instruction::MODF
                    } else {
                        Instruction::MOD
                    }));
                }
            }
            Expression::Div(lhs, rhs) => {
                if self.try_emit_aggregate_arith(
                    &mut bytecode,
                    self_id,
                    span.start,
                    span.end,
                    lhs,
                    Some(rhs),
                    crate::typechecking::AggregateOp::Div,
                ) {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else if let Some(hint) = self_id
                    .and_then(|id| self.checker.bound_operator_call_at(id))
                    .or_else(|| {
                        self.checker
                            .bound_operator_call_for_span(span.start, span.end)
                    })
                    .cloned()
                    && self.emit_bound_operator_call(
                        &mut bytecode,
                        lhs,
                        rhs,
                        hint.dict_index,
                        hint.method_slot,
                    )
                {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else {
                    let is_float = likely(self.compile_binary_operands(&mut bytecode, lhs, rhs));
                    bytecode.push(Byte::new(if is_float {
                        Instruction::DIVF
                    } else {
                        Instruction::DIV
                    }));
                }
            }
            Expression::And(lhs, rhs) => {
                binary!(bytecode, self, lhs, rhs, Byte::new(Instruction::AND));
            }
            Expression::Positive(lhs) => {
                bytecode.append(&mut self.do_compile(lhs));
            }
            Expression::Pow(lhs, rhs) => {
                if self.try_emit_folded_expr(ast, &mut bytecode, true) {
                    // **0 / **1 / **2 strength-reduced or const-folded.
                } else if self.try_emit_aggregate_arith(
                    &mut bytecode,
                    self_id,
                    span.start,
                    span.end,
                    lhs,
                    Some(rhs),
                    crate::typechecking::AggregateOp::Pow,
                ) {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else {
                    let is_float = self.compile_binary_operands(&mut bytecode, lhs, rhs);
                    bytecode.push(Byte::new(if is_float {
                        Instruction::PowF
                    } else {
                        Instruction::Pow
                    }));
                }
            }
            Expression::Shl(lhs, rhs) => {
                binary!(bytecode, self, lhs, rhs, Byte::new(Instruction::SHL));
            }
            Expression::Shr(lhs, rhs) => {
                binary!(bytecode, self, lhs, rhs, Byte::new(Instruction::SHR));
            }
            Expression::Xor(lhs, rhs) => {
                binary!(bytecode, self, lhs, rhs, Byte::new(Instruction::XOR));
            }
            Expression::BitAnd(lhs, rhs) => {
                binary!(bytecode, self, lhs, rhs, Byte::new(Instruction::BITAND));
            }
            Expression::BitOr(lhs, rhs) => {
                binary!(bytecode, self, lhs, rhs, Byte::new(Instruction::BITOR));
            }
            Expression::Or(lhs, rhs) => {
                binary!(bytecode, self, lhs, rhs, Byte::new(Instruction::OR));
            }
            Expression::Neq(lhs, rhs) => {
                let hint = self_id
                    .and_then(|id| self.checker.bound_operator_call_at(id))
                    .or_else(|| {
                        self.checker
                            .bound_operator_call_for_span(span.start, span.end)
                    })
                    .cloned();
                if let Some(hint) = hint
                    && self.emit_bound_operator_call(
                        &mut bytecode,
                        lhs,
                        rhs,
                        hint.dict_index,
                        hint.method_slot,
                    )
                {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else if self.emit_concrete_operator_call(&mut bytecode, lhs, rhs, "Eq", "ne") {
                    // Intentional empty body: the emit/try_emit call in the
                    // condition already wrote bytecode as a side effect.
                } else {
                    binary!(bytecode, self, lhs, rhs, Byte::new(Instruction::NEQ));
                }
            }
            Expression::Integer(num) => bytecode.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(*num).raw() as _,
            )),
            Expression::Bool(state) => bytecode.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(*state).raw() as _,
            )),
            Expression::Float(num) => {
                let bits = Value::from(*num).raw() as u64;
                let idx = self.intern_constant(bits);
                bytecode.push_const_pool(idx);
            }
            Expression::String(str) => {
                let escaped = unescape_coil_string(str);
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
                self.emit_raw_string_literal(&mut bytecode, &escaped);
            }
            Expression::Variable(name, _ty) => {
                if unlikely(self.context.variables.contains(&name.to_string())) {
                    let mut message = Message::error(
                        ErrorCode::VariableRedeclaration,
                        "Variable redeclaration".to_string(),
                        span.into_range(),
                    );
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
                    let mut message = Message::error(
                        ErrorCode::VariableRedeclaration,
                        "Constand redeclaration".to_string(),
                        span.into_range(),
                    );
                    message.push(DiagLabel::new(
                        format!("Constant '{}' already declared", name),
                        span.into_range(),
                    ));
                    self.messages.push(message);
                }

                let symbol = self.context.variables.intern(name.clone());

                self.context.constants.insert(symbol, false);
            }
            Expression::Assignment(lhs, value) => match lhs.1.as_ref() {
                Expression::QualifiedAccess { owner, member } => {
                    let fqn = format!("{}::{}", owner, member);
                    if let Some(slot) = self.checker.static_slot_index(&fqn) {
                        self.append_binding_rhs(&mut bytecode, value);
                        bytecode.push(Byte::new(Instruction::StoreStatic).with_operand_u32(slot));
                    }
                }
                Expression::Construct {
                    enum_name,
                    variant_name,
                    fields,
                } if matches!(fields, parser::ast::EnumConstructPayload::Unit) => {
                    let fqn = format!("{}::{}", enum_name, variant_name);
                    if let Some(slot) = self.checker.static_slot_index(&fqn) {
                        self.append_binding_rhs(&mut bytecode, value);
                        bytecode.push(Byte::new(Instruction::StoreStatic).with_operand_u32(slot));
                    }
                }
                Expression::Access(target_expr, field) => {
                    self.append_binding_rhs(&mut bytecode, value);
                    bytecode.append(&mut self.do_compile(target_expr));
                    self.emit_field_name(&mut bytecode, field);
                    bytecode.push_set_field();
                    // Value left on stack for expression result; ExprStatement POPs.
                }
                Expression::Index(arr, None) => {
                    bytecode.append(&mut self.do_compile(arr));
                    self.append_binding_rhs(&mut bytecode, value);
                    bytecode.push(Byte::new(Instruction::ArrayPush));
                }
                Expression::Index(arr, Some(idx)) => {
                    let tmp_arr = self.alloc_temp_slot();
                    let tmp_idx = self.alloc_temp_slot();
                    let tmp_val = self.alloc_temp_slot();
                    self.append_binding_rhs(&mut bytecode, value);
                    bytecode.push_store_pop(tmp_val);
                    bytecode.append(&mut self.do_compile(arr));
                    bytecode.push_store_pop(tmp_arr);
                    bytecode.append(&mut self.do_compile(idx));
                    bytecode.push_store_pop(tmp_idx);
                    bytecode.push_load(tmp_arr);
                    bytecode.push_load(tmp_idx);
                    bytecode.push_load(tmp_val);
                    bytecode.push(Byte::new(Instruction::StoreIndex));
                }
                Expression::Identifier(name) => {
                    let resolved = self
                        .aliases
                        .get(*name)
                        .cloned()
                        .unwrap_or_else(|| name.to_string());
                    if let Some(static_slot) = self
                        .checker
                        .static_slot_index(&resolved)
                        .or_else(|| self.checker.static_slot_for_module_name(name))
                    {
                        self.append_binding_rhs(&mut bytecode, value);
                        bytecode.push(
                            Byte::new(Instruction::StoreStatic).with_operand_u32(static_slot),
                        );
                    } else {
                        self.context.assignments.insert(name.to_string(), true);
                        let symbol_opt = if let Some(map) = &self.context.match_bindings {
                            if let Some(&slot) = map.get(*name) {
                                Some(slot as usize)
                            } else {
                                self.context.variables.key(&name.to_string())
                            }
                        } else {
                            self.context.variables.key(&name.to_string())
                        };

                        if let Some(symbol) = symbol_opt {
                            if unlikely(self.context.constants.contains_key(&symbol)) {
                                let assigned =
                                    likely(*self.context.constants.get(&symbol).unwrap());
                                if !assigned {
                                    self.context.constants.entry(symbol).and_modify(|state| {
                                        *state = true;
                                    });
                                } else {
                                    let mut message = Message::error(
                                        ErrorCode::InvalidAssignment,
                                        "Assignment error".to_string(),
                                        span.into_range(),
                                    );
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
                            self.append_binding_rhs(&mut bytecode, value);
                            bytecode.push_store_pop(symbol as u32);
                        } else {
                            let mut message = Message::error(
                                ErrorCode::UnknownValue,
                                "Undefined variable".to_string(),
                                span.into_range(),
                            );
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
                }
                _ => {
                    bytecode.append(&mut self.do_compile(value));
                    bytecode.push_pop();
                }
            },

            // --- Sum types, extern, construct ---
            Expression::ExternBlock {
                library,
                declarations,
            } => {
                // Append to self.bytecode so extern bytes run before main.
                let lib_slot =
                    if let Some(&existing) = self.extern_runtime_libs.get(library.as_str()) {
                        existing
                    } else {
                        let name = format!("__ext_lib_{}", library);
                        let slot = self.context.variables.intern(name) as u32;
                        self.extern_runtime_libs.insert(library.clone(), slot);
                        slot
                    };
                // 2. dlopen the library (only on the first
                //    occurrence across the whole compile).
                if !self.extern_runtime_libs_loaded.contains(library.as_str()) {
                    self.extern_runtime_libs_loaded.insert(library.clone());
                    let span: SimpleSpan = (0..0).into();
                    let path_expr: parser::ast::Output = (
                        span,
                        Box::new(parser::ast::Expression::String(library.as_str())),
                    );
                    let mut bc = self.do_compile(&path_expr);
                    self.bytecode.append(&mut bc);
                    self.bytecode.push(Byte::new(Instruction::FfiLoad));
                    self.emit_result_unwrap_or_panic();
                    self.bytecode.push_store_pop(lib_slot);
                }
                // 3. For each declared function, emit declare(lib,
                //    name, (arg_tags...), ret) and store fn id.
                for decl in declarations {
                    let fn_name = decl.name.to_string();
                    let nfixed = if let Expression::Fragment(items) = decl.args.1.as_ref() {
                        items
                            .iter()
                            .filter(|a| matches!(a.1.as_ref(), Expression::Argument { .. }))
                            .count()
                    } else {
                        0
                    };
                    // Key fixed-arity overloads; keep bare name for
                    // single decls and for C-varargs (not overload members).
                    let table_name = if !decl.variadic && self.checker.is_overloaded(decl.name) {
                        overload_fn_key(&fn_name, nfixed, false)
                    } else {
                        fn_name.clone()
                    };
                    // First-wins on the same table key across blocks.
                    if self.extern_runtime_functions.contains_key(&table_name) {
                        continue;
                    }
                    let fn_id_slot_name = format!("__ext_fn_{}", table_name);
                    let fn_id_slot = self.context.variables.intern(fn_id_slot_name) as u32;
                    // Push the library handle.
                    self.bytecode.push_load(lib_slot);
                    // Push the function name (string literal).
                    let span: SimpleSpan = (0..0).into();
                    let sym = decl.symbol.unwrap_or(decl.name);
                    let name_expr: parser::ast::Output =
                        (span, Box::new(parser::ast::Expression::String(sym)));
                    let mut name_bc = self.do_compile(&name_expr);
                    self.bytecode.append(&mut name_bc);
                    // Push each arg type as a CONST tag.
                    let mut arg_type_tags: Vec<u32> = Vec::new();
                    if let Expression::Fragment(items) = decl.args.1.as_ref() {
                        for arg in items {
                            if let Expression::Argument {
                                ty: type_expr,
                                ..
                            } = arg.1.as_ref()
                                && let Some(type_expr) = type_expr
                            {
                                if let Some((tag, aux)) =
                                    ffi_type_tag_from_output(&self.checker, type_expr)
                                {
                                    emit_ffi_type_const(&mut self.bytecode, tag, aux);
                                    arg_type_tags.push(tag);
                                } else {
                                    self.messages.push({
                                        let mut m = Message::error(
                                           ErrorCode::GenericTypeError, "Unknown FFI argument type".to_string(),
                                            arg.0.into_range(),
                                        );
                                        m.push(DiagLabel::new(
                                            "use Int/Ptr after `use ffi::types::*;`, a bare type name, [T], (T, U), or an extern struct".to_string(),
                                            arg.0.into_range(),
                                        ));
                                        m
                                    });
                                    arg_type_tags.push(0);
                                }
                            } else {
                                self.messages.push({
                                    let mut m = Message::error(
                                        ErrorCode::GenericTypeError,
                                        "Extern fn argument must be `name: type` form".to_string(),
                                        arg.0.into_range(),
                                    );
                                    m.push(DiagLabel::new(
                                        "got an unexpected expression".to_string(),
                                        arg.0.into_range(),
                                    ));
                                    m
                                });
                                arg_type_tags.push(0);
                            }
                        }
                    }
                    let arity = arg_type_tags.len() as u32;
                    self.bytecode.push_make_tuple(arity);
                    // Push the ret type tag (top of stack for DeclareFFI).
                    let (ret_tag, ret_aux) = decl
                        .returns
                        .as_ref()
                        .and_then(|r| ffi_type_tag_from_output(&self.checker, r))
                        .unwrap_or((tag::VOID, 0));
                    emit_ffi_type_const(&mut self.bytecode, ret_tag, ret_aux);
                    // Emit DeclareFFI (bit 16 = C varargs).
                    let mut operand = arity & 0xFFFF;
                    if decl.variadic {
                        operand |= 1 << 16;
                    }
                    self.bytecode
                        .push(Byte::new(Instruction::DeclareFFI).with_operand_u32(operand));
                    self.emit_result_unwrap_or_panic();
                    // Store the function id.
                    self.bytecode.push_store_pop(fn_id_slot);
                    self.extern_runtime_functions
                        .insert(table_name, (lib_slot, fn_id_slot));
                }
            }
            Expression::EnumDecl {
                docs: _,
                name: _, variants, ..
            } => {
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
            Expression::TypeAlias { ty, .. } => {
                bytecode.append(&mut self.do_compile(ty));
            }
            Expression::TestCase { name, body } => {
                // Consume name NodeIds (discard emitted string bytes).
                let _ = self.do_compile(name);
                let desc = match name.1.as_ref() {
                    Expression::String(s) => (*s).to_string(),
                    Expression::Expr((_, inner)) => match inner.as_ref() {
                        Expression::String(s) => (*s).to_string(),
                        _ => format!("test_{}", self.test_cases.len()),
                    },
                    _ => format!("test_{}", self.test_cases.len()),
                };
                let case_index = self.test_cases.len();
                let fn_name = crate::typechecking::Checker::test_case_fn_name(case_index);
                let (offset, _) = self.bind_function_entry(fn_name.clone());
                let offset = offset as u32;
                self.test_cases.push((desc, offset));

                let prev_fn_vars = std::mem::take(&mut self.context.variables);
                let prev_fn_polyfn_vars = std::mem::take(&mut self.polyfn_vars);
                let prev_fn_polyfn_sources = std::mem::take(&mut self.polyfn_sources);
                self.context.variables = Interner::default();

                let prev_result_mode = self.compiling_result_mode;
                self.compiling_result_mode = self.checker.fn_is_result_mode(&fn_name);

                let body_op_start = self.bytecode.ops().len();
                let prev_field_keys = std::mem::take(&mut self.field_key_slots);
                self.emit_field_key_prologue(body);
                let mut body_bc = self.do_compile(body);
                self.bytecode.append(&mut body_bc);

                if !self.region_ends_with_return(body_op_start) {
                    // Test cases are typed as unit / Result<(), string> — zero is safe.
                    self.emit_fallthrough_return(&fn_name, body.0);
                }

                self.compiling_result_mode = prev_result_mode;
                self.field_key_slots = prev_field_keys;
                self.context.variables = prev_fn_vars;
                self.polyfn_vars = prev_fn_polyfn_vars;
                self.polyfn_sources = prev_fn_polyfn_sources;
            }
            Expression::ExternStruct(decl) => {
                for (_, ty) in &decl.fields {
                    bytecode.append(&mut self.do_compile(ty));
                }
            }
            Expression::EnumVariant { payload, .. } => {
                // Recurse into each payload's `Type` expression
                // (or `RecordFieldDecl`'s value type). We don't
                // emit bytecode — the variant's payload shape is
                // metadata that's already registered with the
                // typechecker (15B). the payload is
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
                // typechecker's tables. Unknown enum/variant is a
                // type error with recovery — still walk children for
                // NodeId alignment, but do not emit MakeEnum (and do
                // not panic: release builds use panic=abort).
                let Some(tag) = self.checker.tag_for(enum_name, variant_name) else {
                    let fqn = format!("{}::{}", enum_name, variant_name);
                    // Match typechecker order for Unit form: static field
                    // wins over a same-named 0-arg static method.
                    if matches!(fields, EnumConstructPayload::Unit)
                        && let Some(slot) = self.checker.static_slot_index(&fqn)
                    {
                        bytecode.push(Byte::new(Instruction::LoadStatic).with_operand_u32(slot));
                        return bytecode;
                    }
                    // `Class::static_method(...)` — same surface as enum
                    // Construct; lower to a direct CALL when registered.
                    if let Some(offset) = self.functions.get(&fqn).copied() {
                        let arg_slice: &[Output] = match fields {
                            EnumConstructPayload::Unit => &[],
                            EnumConstructPayload::Tuple(args) => args.as_slice(),
                            EnumConstructPayload::Record(parts) => {
                                for part in parts {
                                    bytecode.append(&mut self.do_compile(&part.value));
                                }
                                // Record form is a type error for static
                                // methods; still emit a CALL for recovery.
                                bytecode.push(
                                    Byte::new(Instruction::CALL)
                                        .with_call_packed(parts.len() as u32, offset as u32),
                                );
                                return bytecode;
                            }
                        };
                        let arity =
                            self.emit_call_args_with_rest(&fqn, arg_slice, &mut bytecode, false);
                        bytecode.push(
                            Byte::new(Instruction::CALL).with_call_packed(arity, offset as u32),
                        );
                        return bytecode;
                    }
                    match fields {
                        EnumConstructPayload::Unit => {}
                        EnumConstructPayload::Tuple(args) => {
                            for arg in args {
                                bytecode.append(&mut self.do_compile(arg));
                            }
                        }
                        EnumConstructPayload::Record(parts) => {
                            for part in parts {
                                bytecode.append(&mut self.do_compile(&part.value));
                            }
                        }
                    }
                    return bytecode;
                };
                let arity = self.checker.arity_for(enum_name, variant_name).unwrap_or(0);

                // Emit args in reverse declaration order for MAKE_ENUM stack discipline.
                match fields {
                    EnumConstructPayload::Unit => {}
                    EnumConstructPayload::Tuple(args) => {
                        for arg in args.iter().rev() {
                            bytecode.append(&mut self.do_compile(arg));
                        }
                    }
                    EnumConstructPayload::Record(parts) => {
                        // Build a name → &Output map for the call site.
                        let call_site: std::collections::HashMap<&str, &Output> =
                            parts.iter().map(|p| (p.name, &p.value)).collect();
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
                bytecode.push_make_enum(tag as u16, arity as u16);
            }
            // --- Match codegen (threaded layout) ---
            // Forward: scrutinee, JUMP_IF_MATCH cascade, last-arm UNPACK/POP/STORE.
            // Reverse: arm bindings + bodies; non-first arms JMP to end.
            Expression::Match { scrutinee, arms } => {
                if arms.is_empty() {
                    bytecode.append(&mut self.do_compile(scrutinee));
                    bytecode.push_pop();
                } else {
                    let mut bb = BlockBuilder::new();
                    let end_label = bb.fresh_label(self.bytecode.il_mut());

                    let tag_groups = group_arms_by_outer_tag(arms, &self.checker);
                    // Forward pass emits JUMP_IF_MATCH for every non-last
                    // group, and also for the last group when any group is
                    // multi-arm. Allocate labels for those targets — not
                    // merely for `!is_last && Constructor` in source order
                    // (that missed the last group's first arm when Err
                    // followed two Ok arms, panicking at emit time).
                    let any_multi_arm_group = tag_groups.iter().any(|g| g.arm_indices.len() > 1);
                    let mut arm_labels: Vec<Option<crate::block_builder::Label>> =
                        vec![None; arms.len()];
                    for (g_idx, group) in tag_groups.iter().enumerate() {
                        let is_last_group = g_idx == tag_groups.len() - 1;
                        if !is_last_group || any_multi_arm_group {
                            let first_arm_idx = group.arm_indices[0];
                            arm_labels[first_arm_idx] =
                                Some(bb.fresh_label(self.bytecode.il_mut()));
                        }
                    }

                    // Compile scrutinee before choosing payload_base —
                    // HostInvoke arg staging (`alloc_temp_slot`) grows
                    // `variables`, and bindings must start *after* those
                    // temps or Unpack/JumpIfMatch collide with them
                    // (e.g. `match try_recv(rx)` after print→write_all).
                    let scrutinee_bc = self.do_compile(scrutinee);
                    self.bytecode.extend(scrutinee_bc);

                    // First payload slot after locals + scrutinee temps.
                    // JumpIfMatch/Unpack push payloads onto the stack
                    // above those locals, so bindings must start here —
                    // not at the historical hardcoded slot 1.
                    let payload_base = self.context.variables.len() as u32;

                    // Forward pass: outer-tag dispatch + last-arm scrutinee consumer.
                    for (g_idx, group) in tag_groups.iter().enumerate() {
                        let is_last_group = g_idx == tag_groups.len() - 1;
                        if !is_last_group || any_multi_arm_group {
                            let first_arm_idx = group.arm_indices[0];
                            let label = arm_labels[first_arm_idx]
                                .expect("non-last group's first arm must have a Label");
                            bb.emit_jump_to(
                                label,
                                BbJumpKind::JumpIfMatch {
                                    tag: group.tag,
                                    arity: 0,
                                },
                                self.bytecode.il_mut(),
                            );
                        } else {
                            // Last group in a match with NO
                            // multi-arm groups — emit the
                            // scrutinee-consumer for the
                            // last arm in source order (the
                            // last element of the last
                            // group's `arm_indices`). This
                            // matches the pre-grouped
                            // behavior: the very last arm
                            // is reached by fall-through
                            // from every preceding
                            // JUMP_IF_MATCH miss, so the
                            // scrutinee is still on the
                            // stack and must be consumed
                            // (UNPACK for Constructor, POP
                            // for Wildcard, STORE 1 for
                            // Binding).
                            let last_arm_idx = *group
                                .arm_indices
                                .last()
                                .expect("last group must have at least one arm");
                            let last_arm = &arms[last_arm_idx];
                            match &last_arm.pattern {
                                Pattern::Constructor {
                                    enum_name,
                                    variant_name,
                                    ..
                                } => {
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
                                }
                                Pattern::Wildcard => {
                                    // Wildcard arm — POP the
                                    // scrutinee.
                                    self.bytecode.push_pop();
                                }
                                Pattern::Binding { name } => {
                                    // Binding arm — scrutinee already sits at
                                    // `payload_base` (shared stack/locals). No
                                    // STORE opcode; reverse pass records the
                                    // binding slot.
                                    let _ = name;
                                }
                            }
                        }
                    }

                    // Step 3.5: For multi-arm groups WITH
                    // runtime tests, emit the inner-pattern
                    // test chain. This sits between the
                    // forward pass (JUMP_IF_MATCH
                    // dispatch + scrutinee-consumer) and the
                    // reverse pass (binding + body emission).
                    //
                    // Why this pass is needed: when two or
                    // more arms share the same OUTER variant
                    // tag but differ on an INNER sub-pattern
                    // (e.g. `Result::Ok(Option::Some(v))` vs
                    // `Result::Ok(Option::None)`), a single
                    // `JUMP_IF_MATCH` on the outer tag can't
                    // disambiguate between them — both arms
                    // match the outer tag. The inner-pattern
                    // test chain adds a second dispatch step
                    // (a runtime test on the inner payload)
                    // to pick the right arm.
                    //
                    // Layout for a 3-arm group
                    // `[arm_0, arm_1, arm_2]` sharing the
                    // outer tag:
                    //
                    //   [REBIND arm_0_label here]
                    //   POP/STORE for arm_0's sub-patterns
                    //   JMP → pass_label_0
                    //   POP/STORE for arm_1's sub-patterns
                    //   JMP → pass_label_1
                    //   POP/STORE for arm_2's sub-patterns
                    //   (no JMP — pass_label is None)
                    //   → arm_2 body (fall-through)
                    //   JMP → end_label
                    //   [bind pass_label_1 here] arm_1 body
                    //   JMP → end_label
                    //   [bind pass_label_0 here] arm_0 body
                    //   [end_label: RETURN]
                    //
                    // The REBIND of `arm_0_label` redirects
                    // the outer `JUMP_IF_MATCH` (emitted in
                    // the forward pass) from landing at the
                    // first arm's BODY to landing at the
                    // START of the test chain. Each non-last
                    // arm's `JMP → pass_label_N` then routes
                    // a successful test to the arm's body
                    // (bound later in the reverse pass). The
                    // last arm's test chain falls through to
                    // its body (no JMP needed).
                    //
                    // Multi-arm groups WITHOUT runtime tests
                    // (every sub-pattern is `Wildcard` /
                    // `Binding`, no nested `Constructor`) are
                    // unaffected — the existing
                    // first-arm-wins behavior is preserved.
                    // Single-arm groups are also unaffected.
                    let mut pass_labels: HashMap<usize, Option<crate::block_builder::Label>> =
                        HashMap::new();
                    let mut test_chain_first_arms: std::collections::HashSet<usize> =
                        std::collections::HashSet::new();
                    // All arms that participate in a test chain
                    // group . The reverse pass uses
                    // this set to decide whether to skip
                    // POP/STORE/UNPACK emission in
                    // `emit_pattern_binding` — the test chain
                    // pass already consumed the values, so the
                    // reverse pass should NOT re-emit them.
                    // `test_chain_first_arms` (above) only tracks
                    // the FIRST arm of each group (for label
                    // re-binding); `test_chain_arms` tracks ALL
                    // arms in all test chain groups.
                    let mut test_chain_arms: std::collections::HashSet<usize> =
                        std::collections::HashSet::new();
                    // Per-arm binding map populated by
                    // `emit_inner_test` for arms in test chain
                    // groups. Keyed by arm_idx → name → slot.
                    // The reverse pass consults this map to
                    // install `self.context.match_bindings` for
                    // test chain arms, instead of re-emitting
                    // binding code (which would double-pop /
                    // double-store the payload values).
                    let mut match_bindings_per_arm: HashMap<usize, HashMap<String, u32>> =
                        HashMap::new();

                    for group in &tag_groups {
                        // Only groups with multiple arms AND
                        // at least one arm with a runtime test
                        // trigger the new test-chain
                        // emission.
                        if group.arm_indices.len() <= 1 {
                            continue;
                        }
                        let has_runtime_test = group
                            .arm_indices
                            .iter()
                            .any(|&i| arm_has_runtime_test(&arms[i]));
                        if !has_runtime_test {
                            continue;
                        }

                        let first_arm_idx = group.arm_indices[0];
                        let first_arm_label = arm_labels[first_arm_idx]
                            .expect("non-last group's first arm must have a Label");

                        // REBIND the first arm's label so the
                        // outer JUMP_IF_MATCH lands at the
                        // test chain start, not at the arm
                        // body. `bind_label` is idempotent —
                        // calling it again would re-patch the
                        // JUMP_IF_MATCH, which is exactly
                        // what we want here.
                        bb.bind_label(first_arm_label, self.bytecode.il_mut());
                        test_chain_first_arms.insert(first_arm_idx);
                        for &arm_idx in &group.arm_indices {
                            test_chain_arms.insert(arm_idx);
                        }

                        // Emit the test chain for each arm in
                        // source order. Every arm gets a
                        // `pass_label` JMP to its body —
                        // including the last arm in the group.
                        // Fall-through after the last arm is
                        // only safe when that arm's body is
                        // emitted immediately after the test
                        // chain (i.e. the group is source-last).
                        // With a later tag group (e.g. Ok/Ok
                        // then Err), fall-through would land in
                        // the wrong body's bytecode.
                        for (rank, &arm_idx) in group.arm_indices.iter().enumerate() {
                            let is_last_in_group = rank == group.arm_indices.len() - 1;

                            let pass_label = Some(bb.fresh_label(self.bytecode.il_mut()));

                            // `fail_label` is the NEXT arm's
                            // body label (so the runtime
                            // test can dispatch to the next
                            // arm's test chain on failure).
                            // For the LAST arm in the group
                            // (and for any arm whose NEXT
                            // sibling has no body label —
                            // e.g. it's the last arm of the
                            // entire match and was reached
                            // by fall-through), fall back to
                            // `end_label` so the jump is at
                            // least well-formed (the
                            // placeholder implementation
                            // currently doesn't emit a JMP to
                            // fail_label, but the operand
                            // still needs to be consistent
                            // with the placeholder value).
                            let fail_label = if !is_last_in_group {
                                let next_arm_idx = group.arm_indices[rank + 1];
                                arm_labels[next_arm_idx].unwrap_or(end_label)
                            } else {
                                end_label
                            };

                            pass_labels.insert(arm_idx, pass_label);

                            // Get the arm's payload (only
                            // Constructor arms are candidates
                            // for runtime tests, by the
                            // definition of
                            // `arm_has_runtime_test`).
                            let (enum_name, variant_name, payload) = match &arms[arm_idx].pattern {
                                Pattern::Constructor {
                                    enum_name,
                                    variant_name,
                                    payload,
                                    ..
                                } => (*enum_name, *variant_name, payload),
                                _ => continue,
                            };

                            emit_inner_test(
                                arm_idx,
                                &self.checker,
                                enum_name,
                                variant_name,
                                payload,
                                &mut match_bindings_per_arm,
                                &mut self.bytecode,
                                &mut bb,
                                pass_label,
                                fail_label,
                                payload_base,
                            );
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
                        //
                        // Exception: for the FIRST arm of a
                        // test-chain group, the label was
                        // already REBOUND by the test chain
                        // pass to the test-chain start. We
                        // MUST NOT bind it again here — that
                        // would redirect the outer
                        // JUMP_IF_MATCH from the test-chain
                        // start back to the arm body,
                        // bypassing the test chain entirely.
                        // The reverse pass for this arm binds
                        // `pass_label_0` instead (the
                        // forward-fallthrough target emitted
                        // by `emit_inner_test`).
                        if !test_chain_first_arms.contains(&i)
                            && let Some(label) = arm_labels[i]
                        {
                            bb.bind_label(label, self.bytecode.il_mut());
                        }

                        // For arms in test chain groups,
                        // bind the test chain's
                        // `pass_label` to the start of
                        // this arm's body. Every test-chain
                        // arm (including the last) gets a
                        // pass_label so dispatch works when
                        // another tag group follows.
                        if let Some(Some(label)) = pass_labels.get(&i) {
                            bb.bind_label(*label, self.bytecode.il_mut());
                        }

                        // Per-arm binding slots (`payload_base` = first
                        // payload). Payload order follows declaration
                        // order; record patterns may list fields in any
                        // source order.
                        let mut arm_bindings: HashMap<String, u32> = HashMap::new();
                        let mut next_slot: u32 = payload_base;
                        // Test-chain arms: payload already on stack from
                        // the forward pass — use `consume_values = false`
                        // to record bindings without re-emitting UNPACK/POP.
                        let in_test_chain = test_chain_arms.contains(&i);
                        if let Some(bindings) = match_bindings_per_arm.get(&i) {
                            // This arm is in a test chain
                            // group AND the test chain recorded
                            // bindings (Wildcard/Binding
                            // sub-patterns at the OUTER level).
                            // Use the recorded bindings and skip
                            // the reverse-pass binding code
                            // entirely.
                            arm_bindings = bindings.clone();
                        } else if in_test_chain {
                            // Test chain arm without recorded
                            // bindings — the test chain emitted
                            // JUMP_IF_MATCH for nested
                            // Constructor sub-patterns (no
                            // STORE). Walk the pattern to RECORD
                            // the bindings in `arm_bindings`
                            // (the body needs them for
                            // `Identifier` lookups), but with
                            // `consume_values = false` so we
                            // don't re-emit the bytecode (the
                            // test chain handled the values).
                            match &arm.pattern {
                                Pattern::Binding { name } => {
                                    arm_bindings.insert(name.to_string(), payload_base);
                                }
                                Pattern::Constructor {
                                    enum_name,
                                    variant_name,
                                    ..
                                } => {
                                    // Test-chain arm: the test
                                    // chain pass already emitted
                                    // POP / STORE / JUMP_IF_MATCH
                                    // for the OUTER level. Walk
                                    // the pattern with
                                    // `consume_values = false` to
                                    // RECORD the bindings (the
                                    // body needs them for
                                    // `Identifier` lookups) but
                                    // skip the redundant bytecode
                                    // emission. The function
                                    // handles Tuple (UNPACK skip
                                    // + sub-pattern walk) and
                                    // Record (decl-order walk +
                                    // sub-pattern walk) internally.
                                    let decl_order =
                                        self.checker.payload_tys_for(enum_name, variant_name);
                                    emit_pattern_binding(
                                        &self.checker,
                                        &mut arm_bindings,
                                        &mut next_slot,
                                        &arm.pattern,
                                        &decl_order,
                                        &mut self.bytecode,
                                        false,
                                        true, // is_outer = true (forward pass handled UNPACK/JUMP_IF_MATCH)
                                    );
                                }
                                Pattern::Wildcard => {}
                            }
                        } else {
                            // Not in a test chain: emit binding
                            // code at the outer level (consume
                            // the values via POP/STORE/UNPACK).
                            match &arm.pattern {
                                Pattern::Binding { name } => {
                                    // Binding arm: the forward pass
                                    // already emitted STORE at
                                    // `payload_base` for the
                                    // scrutinee. Record the binding
                                    // here so the body's
                                    // `Identifier` lookup finds it.
                                    arm_bindings.insert(name.to_string(), payload_base);
                                }
                                Pattern::Constructor {
                                    enum_name,
                                    variant_name,
                                    ..
                                } => {
                                    // Non-test-chain arm: emit full
                                    // binding code at the outer
                                    // level (consume the values via
                                    // POP/STORE/UNPACK). The
                                    // function handles Tuple (emit
                                    // UNPACK + sub-pattern walk) and
                                    // Record (decl-order walk + per-
                                    // field recursion — including
                                    // unbounded-depth nested record
                                    // patterns) internally.
                                    let decl_order =
                                        self.checker.payload_tys_for(enum_name, variant_name);
                                    emit_pattern_binding(
                                        &self.checker,
                                        &mut arm_bindings,
                                        &mut next_slot,
                                        &arm.pattern,
                                        &decl_order,
                                        &mut self.bytecode,
                                        true,
                                        true, // is_outer = true (forward pass handled UNPACK/JUMP_IF_MATCH)
                                    );
                                }
                                Pattern::Wildcard => {
                                    // No bindings — the forward pass
                                    // already emitted POP for the
                                    // scrutinee.
                                }
                            }
                        } // close `else` for test chain arms

                        // Install the per-arm bindings map so the
                        // body's `Identifier` / `Assignment` lookups
                        // resolve pattern bindings to
                        // `payload_base`, `payload_base+1`, …
                        // — matching the VM's payload-push
                        // positions. Cleared after the body emits.
                        let saved_bindings = self.context.match_bindings.take();
                        self.context.match_bindings = Some(arm_bindings);
                        if let Some(map) = self.context.match_bindings.clone() {
                            for (name, slot) in map {
                                self.record_debug_local(&name, slot);
                            }
                        }

                        // Per-arm binding types override the flat
                        // `codegen_var_types` side-table so Access on
                        // a reused binding name (`p.y` vs `p.h`) sees
                        // this arm's payload type, not the last arm's.
                        let mut arm_binding_tys = HashMap::new();
                        collect_pattern_binding_types(
                            &self.checker,
                            &arm.pattern,
                            &mut arm_binding_tys,
                        );
                        if let Pattern::Binding { name } = &arm.pattern {
                            if let Some(ty) = self.checker.codegen_var_type(name) {
                                arm_binding_tys.insert(name.to_string(), ty.clone());
                            }
                        }
                        self.mono_codegen_var_types.push(arm_binding_tys);

                        // Emit the arm body unless it is the sole bound name
                        // (`Ok(x) => x`): JumpIfMatch already left the payload
                        // on the stack at the binding slot.
                        if !Self::match_arm_body_is_identity_binding(&arm.pattern, &arm.body) {
                            let body_bc = self.do_compile(&arm.body);
                            self.bytecode.extend(body_bc);
                        }

                        self.mono_codegen_var_types.pop();

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
                                self.bytecode.il_mut(),
                            );
                        }
                    }

                    // Peephole / fuse-select safety: DUPLICATE;POP at the join
                    // (omitted for binding matches — see suppress_match_fusion_barrier).
                    // Label binds are also IL fusion barriers for return-match sites.
                    if !self.suppress_match_fusion_barrier {
                        self.bytecode.push(Byte::new(Instruction::DUPLICATE));
                        self.bytecode.push_pop();
                    }
                    bb.bind_label(end_label, self.bytecode.il_mut());

                    // Validate: every label that had a
                    // pending jump is bound.
                    bb.finalize()
                        .expect("BlockBuilder::finalize: all targeted labels bound");
                }
            }
            // Parser maps `_`/`default` to Pattern::Wildcard; arm consumes NodeId only.
            Expression::Default(_) => (),

            // --- Field access ---
            // receiver bytecode + LoadField(index) or GetField(name) for
            // dicts / class instances.
            Expression::Access(receiver, field) => {
                bytecode.append(&mut self.do_compile(receiver));

                let receiver_ty = self.receiver_type(receiver);
                let is_record =
                    matches!(&receiver_ty, Some(crate::typechecking::Ty::Record { .. }));
                let is_class = receiver_ty
                    .as_ref()
                    .is_some_and(|ty| self.checker.ty_is_class(ty));
                // LoadField only for confirmed sum record payloads.
                // Prefer GetField for classes and anonymous records.
                // `extract_enum_name` alone is unsafe (Ty::Con class
                // names look like enums) — require field_index_for
                // or an is_class check. Unknown receivers that are
                // not classes fall back to LoadField(0) (legacy
                // defensive path) rather than GetField, which would
                // corrupt ObjEnum stacks.
                let enum_field_index = if !is_record && !is_class {
                    self.receiver_type(receiver).and_then(|ty| {
                        use crate::typechecking::ty::Ty;
                        if self.checker.ty_is_class(&ty) {
                            return None;
                        }
                        match &ty {
                            Ty::Constructor { tag, owner, .. } => {
                                let name = extract_enum_name(owner)?;
                                if self.checker.is_class(&name) {
                                    return None;
                                }
                                self.checker
                                    .field_index_for_tagged(&name, field, Some(*tag))
                                    .map(|(_variant, idx)| idx)
                            }
                            _ => {
                                let name = extract_enum_name(&ty)?;
                                if self.checker.is_class(&name) {
                                    return None;
                                }
                                self.checker
                                    .field_index_for(&name, field)
                                    .map(|(_variant, idx)| idx)
                            }
                        }
                    })
                } else {
                    None
                };
                if let Some(field_index) = enum_field_index {
                    bytecode.push_load_field(field_index as u32);
                } else if is_record || is_class {
                    self.emit_field_name(&mut bytecode, field);
                    bytecode.push_get_field();
                } else {
                    // Unknown receiver — do not emit GetField (enum
                    // match bindings historically lacked side-table
                    // types). LoadField(0) keeps the stack balanced;
                    // VM hardens non-enum receivers.
                    bytecode.push_load_field(0);
                }
            }

            Expression::Field { .. } => {
                // Class field decls are metadata only — consumed for ID alignment.
            }

            // --- Error-handling operators (desugar to MakeEnum / JumpIfMatch) ---
            Expression::Raise(expr) => {
                // `raise e` → push e, wrap Err(e), RETURN.
                let expr_bc = self.do_compile(expr);
                self.emit_bytes(*span, &expr_bc);
                Self::emit_result_err(&mut self.bytecode);
                self.pad_debug_locs();
                let loc = self.loc_for_span(*span);
                self.bytecode.push_return_at(loc);
                self.debug_locs.push(loc);
            }
            Expression::Panic(expr) => {
                let expr_bc = self.do_compile(expr);
                self.emit_bytes(*span, &expr_bc);
                self.emit_byte(*span, Byte::new(Instruction::Panic));
            }
            Expression::TypeOf(inner) => {
                // Advance emit_idx through the operand without evaluating it.
                self.discard_compile(inner);
                match self.codegen_expr_ty(inner).and_then(|ty| {
                    let pruned =
                        crate::typechecking::subst::apply_ty_prune(self.checker.subst(), &ty);
                    crate::typechecking::pretty::format_ty_fqn(
                        &pruned,
                        &self.checker.generics().nominal_type_modules,
                    )
                }) {
                    Some(fqn) => {
                        self.emit_raw_string_literal(&mut bytecode, &fqn);
                    }
                    None => {
                        let mut message = Message::error(
                            ErrorCode::GenericTypeError,
                            "`typeof` requires a ground type".to_string(),
                            span.into_range(),
                        );
                        message.push(DiagLabel::new(
                            "type is not fully known at compile time".to_string(),
                            span.into_range(),
                        ));
                        self.messages.push(message);
                        self.emit_raw_string_literal(&mut bytecode, "<unknown>");
                    }
                }
            }
            Expression::Try(inner) => {
                // `e?` → if Ok/Some, leave payload; else RETURN the failure.
                let is_option = self.expr_is_option(inner);
                let success_tag: u32 = if is_option { 1 } else { 0 }; // Some=1, Ok=0

                let inner_bc = self.do_compile(inner);
                self.bytecode.extend(inner_bc);

                let mut bb = BlockBuilder::new();
                let success = bb.fresh_label(self.bytecode.il_mut());
                bb.emit_jump_to(
                    success,
                    BbJumpKind::JumpIfMatch {
                        tag: success_tag,
                        arity: 1,
                    },
                    self.bytecode.il_mut(),
                );
                // Miss: failure value still on stack — propagate via RETURN.
                self.bytecode.push_return();
                bb.bind_label(success, self.bytecode.il_mut());
                bb.finalize()
                    .expect("BlockBuilder::finalize: Try success label bound");
                // Payload left on stack for the caller (e.g. StorePop).
            }
            Expression::Cast(expr, ty_ann) => {
                bytecode.append(&mut self.do_compile(expr));
                let src_ty = self.codegen_expr_ty(expr);
                let dst_name = primitive_name_from_type_ann(ty_ann);
                if let (Some(from), Some(to)) =
                    (src_ty.as_ref().and_then(primitive_type_name), dst_name)
                {
                    if from != to {
                        if let Some(op) = primitive_cast_opcode(from, to) {
                            bytecode.push(Byte::new(op));
                        }
                    }
                }
            }
            Expression::Coalesce(lhs, rhs) => {
                // `a ?? b` → Ok/Some payload, else evaluate b.
                let is_option = self.expr_is_option(lhs);
                let success_tag: u32 = if is_option { 1 } else { 0 };

                let lhs_bc = self.do_compile(lhs);
                self.bytecode.extend(lhs_bc);

                let mut bb = BlockBuilder::new();
                let success = bb.fresh_label(self.bytecode.il_mut());
                let end = bb.fresh_label(self.bytecode.il_mut());
                bb.emit_jump_to(
                    success,
                    BbJumpKind::JumpIfMatch {
                        tag: success_tag,
                        arity: 1,
                    },
                    self.bytecode.il_mut(),
                );
                // Miss: discard failure, evaluate rhs, jump to end.
                self.bytecode.push_pop();
                let rhs_bc = self.do_compile(rhs);
                self.bytecode.extend(rhs_bc);
                bb.emit_jump_to(end, BbJumpKind::Unconditional, self.bytecode.il_mut());
                bb.bind_label(success, self.bytecode.il_mut());
                // Success: payload already on stack from JumpIfMatch.
                bb.bind_label(end, self.bytecode.il_mut());
                bb.finalize()
                    .expect("BlockBuilder::finalize: Coalesce labels bound");
            }
            Expression::OptionalAccess(receiver, field) => {
                // `opt?.field` → None if opt is None, else Some(opt.field).
                let recv_bc = self.do_compile(receiver);
                self.bytecode.extend(recv_bc);

                let mut bb = BlockBuilder::new();
                let success = bb.fresh_label(self.bytecode.il_mut());
                let end = bb.fresh_label(self.bytecode.il_mut());
                bb.emit_jump_to(
                    success,
                    BbJumpKind::JumpIfMatch {
                        tag: 1, // Some
                        arity: 1,
                    },
                    self.bytecode.il_mut(),
                );
                // Miss: None stays on stack; skip field access.
                bb.emit_jump_to(end, BbJumpKind::Unconditional, self.bytecode.il_mut());
                bb.bind_label(success, self.bytecode.il_mut());

                // Payload (inner of Some) on stack — read `.field` then re-wrap Some.
                use crate::typechecking::ty::{is_option_ty, option_inner};
                let inner_ty = self.codegen_expr_ty(receiver).and_then(|t| {
                    if is_option_ty(&t) {
                        option_inner(&t)
                    } else {
                        None
                    }
                });
                let is_record = matches!(&inner_ty, Some(crate::typechecking::Ty::Record { .. }));
                let is_class = inner_ty
                    .as_ref()
                    .is_some_and(|ty| self.checker.ty_is_class(ty));
                let enum_field_index = if !is_record && !is_class {
                    inner_ty
                        .as_ref()
                        .and_then(extract_enum_name)
                        .and_then(|name| {
                            if self.checker.is_class(&name) {
                                return None;
                            }
                            self.checker
                                .field_index_for(&name, field)
                                .map(|(_variant, idx)| idx)
                        })
                } else {
                    None
                };
                if let Some(field_index) = enum_field_index {
                    self.bytecode.push_load_field(field_index as u32);
                } else if is_record || is_class {
                    if let Some(&slot) = self.field_key_slots.get(*field) {
                        self.bytecode.push_load(slot);
                    } else {
                        let idx = self.intern_string(field);
                        self.bytecode.push_string(idx);
                    }
                    self.bytecode.push_get_field();
                } else {
                    self.bytecode.push_load_field(0);
                }
                Self::emit_ok_or_some_wrap(&mut self.bytecode, true);
                bb.bind_label(end, self.bytecode.il_mut());
                bb.finalize()
                    .expect("BlockBuilder::finalize: OptionalAccess labels bound");
            }
            Expression::TypeApp { args, .. } => {
                // Type-position only — consume child IDs, emit no bytes.
                for arg in args {
                    let _ = self.do_compile(arg);
                }
            }
            Expression::Spread(_) => {
                // Call sites flatten spread before emission; this arm keeps
                // ID alignment if a spread node is reached defensively.
            }

            _expr => {
                let mut message = Message::error(
                    ErrorCode::UnknownExpression,
                    "Unknown expression".to_string(),
                    span.into_range(),
                );
                message.push(DiagLabel::new(
                    "Unable to compile expression".to_string(),
                    span.into_range(),
                ));
                self.messages.push(message);
            }
        }

        bytecode
    }

    /// Emit unfused absolute-offset bytecode for `ast` (no peephole pass).
    ///
    /// Multi-file compilation uses this so earlier module tails remain valid
    /// until the pipeline finalizes the linked buffer once. Single-file
    /// [`compile`] calls [`finalize_bytecode`] afterwards for unit tests.
    fn compile_unfused<'compiler>(
        &mut self,
        module: &str,
        ast: &mut (SimpleSpan, Box<Expression<'compiler>>),
    ) {
        let ns = self.namespace.clone();
        self.namespace = module.to_string();

        self.emit_idx = 0;
        self.temp_counter = 0;
        self.expr_depth = 0;
        self.const_env_stack.clear();
        self.const_env_stack.push(HashMap::new());
        self.static_const_values.clear();
        self.current_function_qualified = None;
        self.current_function_table_key = None;
        self.fn_bytecode_spans.clear();
        if self.bytecode.len() <= PROLOGUE_BYTECODE_LEN {
            self.fn_debug_locals.clear();
        }
        self.loop_stack.clear();
        self.loop_bbs.clear();
        // Constant pool is shared across multi-file `compile_module`
        // calls. `JumpIfMatch` (and pool-backed `CONST`) store indices
        // into this vec; clearing between modules orphans earlier
        // instructions so the worker VM panics in
        // `Byte::jump_if_match_target` (e.g. index 2, len 1) when a
        // dependency uses `?` / match. Only reset on a fresh compile
        // (still just the CALL/JMP/HALT prologue).
        if self.bytecode.len() <= PROLOGUE_BYTECODE_LEN {
            self.constants.clear();
            self.strings.clear();
            self.string_indices.clear();
        }
        self.mono_offsets.clear();
        self.mono_codegen_var_types.clear();
        self.test_cases.clear();
        self.user_main_defined = false;
        if !self.include_tests {
            crate::strip_tests::strip_test_declarations(ast);
        }
        self.checker.set_current_module(module);
        // Expand `derive` clauses to synthetic `impl` AST before the
        // ID pre-walk / typecheck (see `crate::attrs::expand_program`).
        let expand = crate::attrs::expand_program(ast);
        self.messages.extend(expand.messages);
        self.decorated_class_ctors
            .extend(expand.decorated_class_ctors);
        let _program_ty = self.checker.check_program(ast);
        self.recursive_pure = if auto_par_enabled() {
            crate::typechecking::analyze_recursive_pure(ast)
        } else {
            HashSet::new()
        };
        self.emit_builtin_dict_thunks();
        // Builtin dictionary thunks are emitted immediately after the
        // prologue and before user code. Keep `program_start_offset`
        // pointing at the first user byte so `extern` prologue JMPs
        // don't fall into a Num/Ord/Eq/Show thunk body.
        self.program_start_offset = self.bytecode.len() as u32;
        self.setup_entry_offset = self.program_start_offset;
        // Label the setup / top-level region so `dead_block` keeps it
        // after prologue HALT / prelude RETURN (reachability is
        // label-based until entry-aware DCE).
        self.bytecode.bind_fresh_entry();
        self.mono_plan = crate::monomorphize::plan_monomorphization(module, ast, &self.checker);

        let mut program = self.do_compile(ast);
        self.namespace = ns.to_string();

        self.messages.extend(self.checker.take_messages());

        self.bytecode.append(&mut program);
        self.pad_debug_locs();
    }

    /// Lower stack IL to VM bytecode (fusion select + label resolution).
    ///
    /// Called once after multi-file linking by the pipeline, or at the end
    /// of single-file [`compile`] so unit tests observe fused output.
    pub fn finalize_bytecode(&mut self) {
        let _ = self.finalize_bytecode_inner(false);
    }

    /// Like [`finalize_bytecode`], but also returns a pre-opt IL snapshot for dissect.
    #[cfg(any(test, feature = "dissect"))]
    pub fn finalize_bytecode_capturing_il(&mut self) -> crate::dissect::IlSnapshot {
        self.finalize_bytecode_inner(true)
            .expect("capture_il requested")
    }

    fn finalize_bytecode_inner(&mut self, capture_il: bool) -> FinalizeIlOut {
        // Splice static initializers into the IL before lower — no absolute
        // target bumping required for symbolic jumps.
        let static_init_region = if !self.static_init_bytecode.is_empty() {
            let pos = self.program_start_offset as usize;
            self.setup_entry_offset = pos as u32;
            let inits = std::mem::take(&mut self.static_init_bytecode);
            let init_len = inits.len();
            self.bytecode.splice_bytes_at(pos, inits);
            self.bytecode.bump_absolute_entry_targets(pos, init_len);
            self.bytecode.bump_func_spans(pos, init_len);
            // Splice inserts before any label at `pos`; ensure the init
            // region itself is labeled so dead_block keeps it.
            self.bytecode.entry_label_at(pos);
            self.program_start_offset += init_len as u32;
            for offset in self.functions.values_mut() {
                if *offset >= pos {
                    *offset += init_len;
                }
            }
            for (_, offset) in self.test_cases.iter_mut() {
                if (*offset as usize) >= pos {
                    *offset += init_len as u32;
                }
            }
            for offset in self.mono_offsets.values_mut() {
                if *offset >= pos {
                    *offset += init_len;
                }
            }
            Some((pos, init_len))
        } else {
            None
        };

        // After static inits, insert JMP → main at the end of the init region.
        // Prefer the entry label already bound when `main` was emitted.
        let main_off = self.functions.get("main").copied();
        if let (Some((pos, init_len)), Some(main_off)) = (static_init_region, main_off) {
            let jmp_pos = pos + init_len;
            let main_label = self.bytecode.entry_label_at(main_off);
            self.bytecode.insert_jump_at(jmp_pos, main_label);
            self.bytecode.bump_absolute_entry_targets(jmp_pos, 1);
            self.bytecode.bump_func_spans(jmp_pos, 1);
            for offset in self.functions.values_mut() {
                if *offset >= jmp_pos {
                    *offset += 1;
                }
            }
            for (_, offset) in self.test_cases.iter_mut() {
                if (*offset as usize) >= jmp_pos {
                    *offset += 1;
                }
            }
            for offset in self.mono_offsets.values_mut() {
                if *offset >= jmp_pos {
                    *offset += 1;
                }
            }
            if (self.program_start_offset as usize) > jmp_pos {
                self.program_start_offset += 1;
            }
        }

        #[cfg(any(test, feature = "dissect"))]
        let il_snapshot = if capture_il {
            Some(crate::dissect::IlSnapshot::new(
                self.bytecode.ops().to_vec(),
                self.bytecode.funcs().to_vec(),
            ))
        } else {
            None
        };

        let lowered = self.bytecode.lower_in_place(&mut self.constants);
        let map = |t: usize| -> usize {
            if let Some(&p) = lowered.pre_to_post.get(&t) {
                return p;
            }
            let mut best = lowered.code_len;
            for (&pre, &post) in &lowered.pre_to_post {
                if pre >= t && post < best {
                    best = post;
                }
            }
            best
        };
        // Prefer entry labels: IL opts (dead_block) shift emitting indices
        // before fuse, so raw `functions` / `test_cases` PCs are stale.
        let resolve_entry = |pre: usize| -> usize {
            if let Some(label) = self.bytecode.entry_label_for_offset(pre) {
                if let Some(&pc) = lowered.label_pcs.get(&label.0) {
                    return pc;
                }
            }
            map(pre)
        };
        for (name, offset) in self.functions.iter_mut() {
            if let Some(label) = self.fn_entry_labels.get(name) {
                if let Some(&pc) = lowered.label_pcs.get(&label.0) {
                    *offset = pc;
                    continue;
                }
            }
            *offset = resolve_entry(*offset);
        }
        for (_, offset) in self.test_cases.iter_mut() {
            *offset = resolve_entry(*offset as usize) as u32;
        }
        for offset in self.mono_offsets.values_mut() {
            *offset = resolve_entry(*offset);
        }
        self.program_start_offset = resolve_entry(self.program_start_offset as usize) as u32;
        self.setup_entry_offset = resolve_entry(self.setup_entry_offset as usize) as u32;

        self.debug_locs = lowered.debug_locs;

        debug_assert_eq!(
            self.debug_locs.len(),
            self.bytecode.len(),
            "debug_locs / bytecode length mismatch after finalize"
        );

        #[cfg(any(test, feature = "dissect"))]
        return il_snapshot;
        #[cfg(not(any(test, feature = "dissect")))]
        {
            debug_assert!(!capture_il);
            ()
        }
    }

    /// Post-lower function symbols sorted by entry PC (for dissect / debug).
    #[cfg(any(test, feature = "dissect"))]
    pub fn function_symbols(&self) -> Vec<crate::dissect::FnSym> {
        let mut syms: Vec<_> = self
            .functions
            .iter()
            .map(|(name, &pc)| {
                let mut locals: Vec<(String, u32)> = self
                    .fn_debug_locals
                    .get(name)
                    .map(|m| m.iter().map(|(n, &s)| (n.clone(), s)).collect())
                    .unwrap_or_default();
                locals.sort_by_key(|(_, s)| *s);
                crate::dissect::FnSym {
                    name: name.clone(),
                    entry_pc: pc as u32,
                    locals,
                }
            })
            .collect();
        syms.sort_by_key(|s| s.entry_pc);
        syms
    }

    pub fn compile<'compiler>(
        &mut self,
        module: &str,
        ast: &mut (SimpleSpan, Box<Expression<'compiler>>),
    ) -> Vec<Byte> {
        self.compile_unfused(module, ast);
        self.finalize_bytecode();
        self.bytecode.clone_bytes()
    }

    /// Append this module's IL to the shared buffer (multi-file pipeline).
    ///
    /// Returns an empty vec for API compatibility; the pipeline should call
    /// [`finalize_bytecode`] once on the linked compiler buffer.
    pub fn compile_module<'compiler>(
        &mut self,
        module: &str,
        ast: &mut (SimpleSpan, Box<Expression<'compiler>>),
    ) -> Vec<Byte> {
        self.compile_unfused(module, ast);
        Vec::new()
    }

    /// Final lowered bytecode after [`finalize_bytecode`].
    pub fn bytecode_slice(&self) -> &[Byte] {
        self.bytecode.as_slice()
    }

    pub fn bytecode_vec(&self) -> Vec<Byte> {
        self.bytecode.clone_bytes()
    }
}

#[cfg(test)]
#[path = "lib.tests.rs"]
mod tests;
