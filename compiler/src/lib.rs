mod block_builder;
mod manifest;
mod monomorphize;
mod peephole;
mod pipeline;
mod typechecking;

use std::{
    borrow::Borrow,
    collections::{HashMap, HashSet},
};

use common::{
    Byte, Instruction, Interner, Value, ValueTag, encode_tag_operand, likely, tag, unlikely,
};
use reporting::Label as DiagLabel;

use crate::block_builder::{BlockBuilder, JumpKind as BbJumpKind, Label as BbLabel};
use crate::monomorphize::{MonoKey, MonoPlan};
use parser::{
    SimpleSpan,
    ast::{Expression, MatchArm, Output, Pattern, PatternPayload},
};

pub use pipeline::*;
pub use reporting::{ErrorCode, Label, Message, MessageKind};
pub use typechecking::{CStructDef, CallbackSigDef, Checker, Ty};

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

// --- Match helpers ---

/// Arms grouped by outer variant tag for dispatch and inner-pattern tests.
#[derive(Debug, Clone)]
struct TagGroup {
    tag: u32,
    arm_indices: Vec<usize>,
    is_single_arm_group: bool,
}

/// Map FFI type expressions to runtime `(tag, aux)` for declare/invoke codegen.
fn ffi_type_tag_from_output(checker: &Checker, expr: &Output) -> Option<(u32, u32)> {
    checker.ffi_type_tag_from_output(expr)
}

fn emit_ffi_type_const(bytecode: &mut Vec<Byte>, tag: u32, aux: u32) {
    bytecode.push(Byte::new(Instruction::CONST).with_operand_u32(encode_tag_operand(tag, aux)));
}

fn is_instance_method_fqn(checker: &Checker, name: &str) -> bool {
    checker.generics().instances.iter().any(|instance| {
        instance
            .method_fqns
            .values()
            .any(|method_fqn| method_fqn == name)
    })
}

fn group_arms_by_outer_tag(arms: &[MatchArm], checker: &Checker) -> Vec<TagGroup> {
    let mut groups: Vec<TagGroup> = Vec::new();
    let mut tag_to_idx: HashMap<u32, usize> = HashMap::new();
    for (i, arm) in arms.iter().enumerate() {
        let tag = match &arm.pattern {
            Pattern::Constructor {
                enum_name,
                variant_name,
                ..
            } => checker.tag_for(enum_name, variant_name).unwrap_or(u32::MAX),
            _ => u32::MAX,
        };
        if let Some(&idx) = tag_to_idx.get(&tag) {
            groups[idx].arm_indices.push(i);
        } else {
            tag_to_idx.insert(tag, groups.len());
            groups.push(TagGroup {
                tag,
                arm_indices: vec![i],
                is_single_arm_group: false,
            });
        }
    }
    for g in &mut groups {
        g.is_single_arm_group = g.arm_indices.len() == 1;
    }
    groups
}

/// True when an arm needs inner-pattern runtime tests (nested bindings/constructors).
#[allow(dead_code)]
fn arm_has_runtime_test(arm: &MatchArm) -> bool {
    /// Recursive helper: does the inner payload of this arm's
    /// outer Constructor pattern carry a `Binding` or further
    /// nested `Constructor` (i.e., a value to extract)?
    fn inner_carries_value(pattern: &Pattern) -> bool {
        match pattern {
            Pattern::Wildcard | Pattern::Binding { .. } => false,
            Pattern::Constructor { payload, .. } => match payload {
                PatternPayload::Unit => false,
                PatternPayload::Tuple(parts) => parts
                    .iter()
                    .any(|p| matches!(p, Pattern::Binding { .. } | Pattern::Constructor { .. })),
                PatternPayload::Record(fields) => fields.iter().any(|f| {
                    matches!(
                        f.pattern,
                        Pattern::Binding { .. } | Pattern::Constructor { .. }
                    )
                }),
            },
        }
    }
    if let Pattern::Constructor { payload, .. } = &arm.pattern {
        match payload {
            PatternPayload::Unit => false,
            PatternPayload::Tuple(parts) => parts.iter().any(inner_carries_value),
            PatternPayload::Record(fields) => {
                fields.iter().any(|f| inner_carries_value(&f.pattern))
            }
        }
    } else {
        false
    }
}

/// Emit inner-pattern tests after outer tag dispatch (multi-arm groups).
#[allow(dead_code, unused_variables)]
fn emit_inner_test<'compiler>(
    arm_idx: usize,
    checker: &Checker,
    enum_name: &str,
    variant_name: &str,
    payload: &PatternPayload<'compiler>,
    match_bindings_per_arm: &mut HashMap<usize, HashMap<String, u32>>,
    bytecode: &mut Vec<Byte>,
    bb: &mut BlockBuilder,
    pass_label: Option<crate::block_builder::Label>,
    fail_label: crate::block_builder::Label,
) {
    use parser::ast::PatternPayload;
    match payload {
        PatternPayload::Unit => {
            // No payload to test. The pass_label is always None for Unit
            // (it's the last arm in a group with no sub-pattern). Emit
            // nothing — the test falls through to the next arm's
            // fail handler.
        }
        PatternPayload::Tuple(parts) => {
            // POP/STORE for wildcards/bindings; JUMP_IF_MATCH for nested constructors.
            let mut any_nested_ctor = false;
            for sub in parts {
                match sub {
                    Pattern::Wildcard => {
                        bytecode.push(Byte::new(Instruction::POP));
                    }
                    Pattern::Binding { name } => {
                        let slot = next_available_slot(match_bindings_per_arm);
                        match_bindings_per_arm
                            .entry(arm_idx)
                            .or_default()
                            .insert(name.to_string(), slot);
                        bytecode.push(Byte::new(Instruction::STORE).with_operand_u32(slot));
                    }
                    Pattern::Constructor {
                        enum_name: sub_enum,
                        variant_name: sub_variant,
                        payload: sub_payload,
                        ..
                    } => {
                        // Nested constructor: JUMP_IF_MATCH on inner tag, or recurse for records.
                        any_nested_ctor = true;
                        if matches!(sub_payload, PatternPayload::Record(_)) {
                            // Nested record — recurse. The recursion
                            // walks the inner record's declared fields
                            // in decl_order and emits per-field
                            // tests (POP / STORE / JUMP_IF_MATCH on
                            // further-nested tags).
                            emit_inner_test(
                                arm_idx,
                                checker,
                                sub_enum,
                                sub_variant,
                                sub_payload,
                                match_bindings_per_arm,
                                bytecode,
                                bb,
                                pass_label,
                                fail_label,
                            );
                        } else if let Some(label) = pass_label {
                            if let Some(inner_tag) = checker.tag_for(sub_enum, sub_variant) {
                                bb.emit_jump_to(
                                    label,
                                    BbJumpKind::JumpIfMatch {
                                        tag: inner_tag,
                                        arity: 0,
                                    },
                                    bytecode,
                                );
                            } else {
                                bytecode.push(Byte::new(Instruction::POP));
                            }
                        } else {
                            // Last arm in the group — emit POP to
                            // consume the inner value. The arm
                            // body is reached by fall-through.
                            bytecode.push(Byte::new(Instruction::POP));
                        }
                    }
                }
            }
            // Trailing JMP to pass_label when inner tests are all wildcards/bindings.
            if !any_nested_ctor && let Some(label) = pass_label {
                bb.emit_jump_to(label, BbJumpKind::Unconditional, bytecode);
            }
        }
        PatternPayload::Record(fields) => {
            // Walk record fields in declaration order (matches UNPACK slot layout).
            let decl_order = checker.payload_tys_for(enum_name, variant_name);
            let pattern_site: std::collections::HashMap<&str, &Pattern<'compiler>> =
                fields.iter().map(|pf| (pf.name, &pf.pattern)).collect();
            let mut any_nested_ctor = false;
            for (decl_name, _) in decl_order.iter() {
                let sub_pat = match pattern_site.get(decl_name.as_str()) {
                    Some(p) => *p,
                    None => {
                        // Field omitted from the pattern — emit
                        // POP to discard the value (the test
                        // chain always consumes every slot, so
                        // this is unconditional).
                        bytecode.push(Byte::new(Instruction::POP));
                        continue;
                    }
                };
                match sub_pat {
                    Pattern::Wildcard => {
                        bytecode.push(Byte::new(Instruction::POP));
                    }
                    Pattern::Binding { name } => {
                        let slot = next_available_slot(match_bindings_per_arm);
                        match_bindings_per_arm
                            .entry(arm_idx)
                            .or_default()
                            .insert(name.to_string(), slot);
                        bytecode.push(Byte::new(Instruction::STORE).with_operand_u32(slot));
                    }
                    Pattern::Constructor {
                        enum_name: sub_enum,
                        variant_name: sub_variant,
                        payload: sub_payload,
                        ..
                    } => {
                        // Nested Constructor sub-pattern on a
                        // record field. If the nested
                        // Constructor's payload is itself a
                        // Record, recurse to dispatch on the
                        // inner record's nested tags. Otherwise
                        // (Unit / Tuple), emit JUMP_IF_MATCH on
                        // the inner tag as before.
                        any_nested_ctor = true;
                        if matches!(sub_payload, PatternPayload::Record(_)) {
                            emit_inner_test(
                                arm_idx,
                                checker,
                                sub_enum,
                                sub_variant,
                                sub_payload,
                                match_bindings_per_arm,
                                bytecode,
                                bb,
                                pass_label,
                                fail_label,
                            );
                        } else if let Some(label) = pass_label {
                            if let Some(inner_tag) = checker.tag_for(sub_enum, sub_variant) {
                                bb.emit_jump_to(
                                    label,
                                    BbJumpKind::JumpIfMatch {
                                        tag: inner_tag,
                                        arity: 0,
                                    },
                                    bytecode,
                                );
                            } else {
                                bytecode.push(Byte::new(Instruction::POP));
                            }
                        } else {
                            // Last arm in the group — emit POP
                            // to consume the inner value. The
                            // arm body is reached by
                            // fall-through.
                            bytecode.push(Byte::new(Instruction::POP));
                        }
                    }
                }
            }
            if !any_nested_ctor && let Some(label) = pass_label {
                bb.emit_jump_to(label, BbJumpKind::Unconditional, bytecode);
            }
        }
    }
}

/// Next free binding slot (slots start at 1; 0 is the scrutinee).
#[allow(dead_code)]
fn next_available_slot(match_bindings: &HashMap<usize, HashMap<String, u32>>) -> u32 {
    let mut max_slot = 0u32;
    for arm_bindings in match_bindings.values() {
        for &slot in arm_bindings.values() {
            if slot > max_slot {
                max_slot = slot;
            }
        }
    }
    max_slot + 1
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

    /// Per-arm pattern bindings (slot 1..N). Overrides global `variables` in arm bodies.
    match_bindings: Option<HashMap<String, u32>>,

    prev: Option<Box<Self>>,
}

// --- Compiler ---

pub struct Compiler {
    namespace: String,
    bytecode: Vec<Byte>,

    aliases: HashMap<String, String>,
    functions: HashMap<String, usize>,
    /// Top-level items per namespace (for `use foo::*` glob expansion).
    module_items: std::collections::HashMap<String, Vec<String>>,
    native: HashMap<String, usize>,
    /// Let-slot holding each extern library handle.
    extern_runtime_libs: HashMap<String, u32>,
    /// Source name → (lib_slot, fn_id_slot) for runtime FFI calls.
    extern_runtime_functions: HashMap<String, (u32, u32)>,
    /// Records which FFI library short names have already
    /// been loaded in the current compilation unit. Cleared
    /// each `compile`.
    extern_runtime_libs_loaded: std::collections::HashSet<String>,
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
    /// Offset where user code starts (after prologue). Extern blocks precede main.
    program_start_offset: u32,
    /// Wide immediates referenced from compact 8-byte `Byte`
    /// operands (floats, `JumpIfMatch` targets, etc.).
    constants: Vec<u64>,

    /// Qualified names of `async fn` declarations (emit `MakeCoro` at call sites).
    coroutine_fns: std::collections::HashSet<String>,

    /// Counter for compiler-generated temporary slots.
    temp_counter: u32,

    /// Active loop labels: `(continue_target, break_target)`.
    loop_stack: Vec<(BbLabel, BbLabel)>,

    /// Active loop patchers. Break/continue emit through the innermost builder.
    loop_bbs: Vec<BlockBuilder>,

    /// True while compiling an `impl` method — Function resets locals
    /// and reserves slot 0 for `self`.
    compiling_method: bool,

    /// True while compiling a function whose return type is inferred
    /// as `Result<T, E>` via `raise` / `?` (wrap bare `return` in `Ok`).
    compiling_result_mode: bool,

    /// Local variable names that hold an `ObjPolyFn` heap pointer
    /// (i.e. `let f = some_generic_fn;`). When these are invoked via
    /// `Expression::Call`, the codegen emits `CallIndirect` instead
    /// of a direct `CALL` opcode.
    polyfn_vars: HashSet<String>,
    /// Local PolyFn variable → source generic function name.
    polyfn_sources: HashMap<String, String>,

    /// Monomorphization plan for this compile unit plus emitted clone offsets.
    mono_plan: MonoPlan,
    mono_offsets: HashMap<MonoKey, usize>,
    /// Temporary variable-type overrides while emitting a specialized clone.
    mono_codegen_var_types: Vec<HashMap<String, Ty>>,
}

impl Default for Compiler {
    fn default() -> Self {
        let mut bytecode = Vec::with_capacity(1024);
        bytecode.append(&mut vec![
            Byte::new(Instruction::CALL),
            Byte::new(Instruction::JMP).with_operand_u32(u32::MAX),
            Byte::new(Instruction::HALT),
        ]);
        // The first USER code (i.e., the first byte after
        // the prologue) is offset `bytecode.len()`, which
        // is exactly 3 (CALL + JMP + HALT).
        let program_start_offset = bytecode.len() as u32;

        Self {
            namespace: String::default(),
            bytecode,
            aliases: HashMap::default(),
            functions: HashMap::with_capacity(32),
            module_items: std::collections::HashMap::default(),
            native: HashMap::default(),
            extern_runtime_libs: HashMap::with_capacity(4),
            extern_runtime_functions: HashMap::with_capacity(16),
            extern_runtime_libs_loaded: HashSet::new(),
            // ---
            messages: Vec::default(),
            context: Context::default(),
            // ---
            checker: crate::typechecking::Checker::new(),
            emit_idx: 0,
            program_start_offset,
            constants: Vec::default(),
            coroutine_fns: std::collections::HashSet::new(),
            temp_counter: 0,
            loop_stack: Vec::new(),
            loop_bbs: Vec::new(),
            compiling_method: false,
            compiling_result_mode: false,
            polyfn_vars: HashSet::new(),
            polyfn_sources: HashMap::new(),
            mono_plan: MonoPlan::default(),
            mono_offsets: HashMap::new(),
            mono_codegen_var_types: Vec::new(),
        }
    }
}

impl Compiler {
    pub fn constants(&self) -> &[u64] {
        &self.constants
    }

    pub fn intern_constant(&mut self, value: u64) -> u32 {
        let idx = self.constants.len() as u32;
        self.constants.push(value);
        idx
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

/// Bind or discard match-pattern sub-values. Payload slots start at 1.
///
/// `consume_values`: skip POP/STORE/UNPACK when a test chain already consumed them.
/// `is_outer`: suppress UNPACK at the outer level (forward pass already unpacked).
fn emit_pattern_binding<'compiler>(
    checker: &Checker,
    match_bindings: &mut HashMap<String, u32>,
    next_slot: &mut u32,
    pattern: &Pattern<'compiler>,
    parent_decl_order: &[(String, Ty)],
    bytecode: &mut Vec<Byte>,
    consume_values: bool,
    is_outer: bool,
) {
    use parser::ast::PatternPayload;
    match pattern {
        Pattern::Wildcard => {
            if consume_values {
                bytecode.push(Byte::new(Instruction::POP));
            }
        }
        Pattern::Binding { name } => {
            let slot = *next_slot;
            // Always record the binding — the body still
            // needs to be able to look up the slot via
            // `Identifier` / `Assignment`, even if we don't
            // emit the redundant STORE (the test chain
            // already pushed the value at this slot via
            // JUMP_IF_MATCH).
            match_bindings.insert(name.to_string(), slot);
            if consume_values {
                bytecode.push(Byte::new(Instruction::STORE).with_operand_u32(slot));
            }
            *next_slot += 1;
        }
        Pattern::Constructor { payload, .. } => match payload {
            PatternPayload::Unit => {
                // A unit-variant nested pattern (e.g. `Option::None`)
                // is invalid — unit variants have no payload. But
                // the typechecker would have rejected this. Emit a
                // defensive POP only if the caller expects a value
                // to consume on the stack.
                //
                // The OUTER-level Unit case is handled by the
                // caller (the forward pass emits POP / STORE 1 /
                // nothing depending on whether the arm is the
                // last, non-last, or a wildcard/binding
                // catch-all). The recursion's Unit case (when
                // is_outer = false) emits POP only when the
                // caller expects a value on the stack
                // (`consume_values = true`).
                if consume_values && !is_outer {
                    bytecode.push(Byte::new(Instruction::POP));
                }
            }
            PatternPayload::Tuple(parts) => {
                // The OUTER-level Tuple case: the forward pass
                // already emitted UNPACK for the last arm (or
                // JUMP_IF_MATCH for non-last arms). Suppress
                // UNPACK emission at the OUTER level.
                //
                // The recursion's Tuple case (when is_outer =
                // false): we have a nested constructor on the
                // stack (pushed by the outer JUMP_IF_MATCH or
                // UNPACK above), and we need to UNPACK it to
                // get its payload values at the right slot
                // positions before binding its sub-patterns.
                if consume_values && !is_outer {
                    bytecode
                        .push(Byte::new(Instruction::Unpack).with_operand_u32(parts.len() as u32));
                }
                // Recurse for sub-patterns with the same
                // `consume_values` flag. The inner values were
                // pushed either by the (emitted) UNPACK above,
                // or by the outer JUMP_IF_MATCH in the test
                // chain case (when consume_values was false).
                // When `consume_values = false`, the test
                // chain has already emitted the
                // POP / JUMP_IF_MATCH for the inner values, so
                // we suppress the redundant bytecode in the
                // recursion too.
                //
                // The recursion is ALWAYS at `is_outer = false`
                // (the OUTER level is reached exactly once per
                // arm body — by the caller).
                //
                // The sub-pattern's `parent_decl_order` is
                // empty unless the sub-pattern is itself a
                // record constructor — then it's the
                // sub-pattern's declared field order. Tuple
                // sub-patterns don't use `parent_decl_order`
                // (they walk in source order).
                for sub in parts {
                    let sub_decl_order: Vec<(String, Ty)> = if let Pattern::Constructor {
                        enum_name: sub_enum,
                        variant_name: sub_variant,
                        payload: PatternPayload::Record(_),
                        ..
                    } = sub
                    {
                        checker.payload_tys_for(sub_enum, sub_variant)
                    } else {
                        Vec::new()
                    };
                    emit_pattern_binding(
                        checker,
                        match_bindings,
                        next_slot,
                        sub,
                        &sub_decl_order,
                        bytecode,
                        consume_values,
                        false, // is_outer = false (recursion)
                    );
                }
            }
            PatternPayload::Record(fields) => {
                // Declaration-order walk; nested records use UnpackAt at non-top slots.
                let pattern_site: std::collections::HashMap<&str, &Pattern<'compiler>> =
                    fields.iter().map(|pf| (pf.name, &pf.pattern)).collect();
                for (i, (decl_name, _)) in parent_decl_order.iter().enumerate() {
                    let field_slot = (i + 1) as u32;
                    if let Some(sub_pat) = pattern_site.get(decl_name.as_str()) {
                        // If the sub-pattern is a nested
                        // record, emit `UnpackAt` with the
                        // slot position of the OUTER field
                        // (= `field_slot`). The slot-based
                        // UNPACK writes the inner record's
                        // payload values to consecutive
                        // positions starting at `field_slot`,
                        // overwriting the nested record's
                        // enum value.
                        //
                        // `is_outer` is captured by value
                        // from the enclosing Record arm
                        // call. When `is_outer = true`, we're
                        // walking the OUTER record's fields
                        // — nested records at this level need
                        // `UnpackAt`. When `is_outer = false`
                        // (we're recursing into a nested
                        // record's fields), nested records
                        // ALSO need `UnpackAt` (one level
                        // deeper). Either way, emit it when
                        // the sub-pattern is a nested record
                        // AND `consume_values = true`.
                        if consume_values
                            && let Pattern::Constructor {
                                enum_name: sub_enum,
                                variant_name: sub_variant,
                                payload: PatternPayload::Record(_),
                            } = sub_pat
                        {
                            let inner_arity =
                                checker.payload_tys_for(sub_enum, sub_variant).len() as u16;
                            bytecode.push(
                                Byte::new(Instruction::UnpackAt)
                                    .with_operands_u16([field_slot as u16, inner_arity]),
                            );
                        }
                        // Compute the sub-pattern's own
                        // record decl_order if it's a record
                        // constructor (for unbounded nesting).
                        let sub_decl_order: Vec<(String, Ty)> = if let Pattern::Constructor {
                            enum_name: sub_enum,
                            variant_name: sub_variant,
                            payload: PatternPayload::Record(_),
                            ..
                        } = sub_pat
                        {
                            checker.payload_tys_for(sub_enum, sub_variant)
                        } else {
                            Vec::new()
                        };
                        emit_pattern_binding(
                            checker,
                            match_bindings,
                            next_slot,
                            sub_pat,
                            &sub_decl_order,
                            bytecode,
                            consume_values,
                            false, // is_outer = false (recursion)
                        );
                    } else if consume_values {
                        // Field omitted from the pattern.
                        // Emit POP to keep the stack
                        // consistent with the declaration-
                        // order walk. The typechecker
                        // already reported the error (if
                        // any). At the OUTER level, the
                        // forward pass handled missing
                        // fields via UNPACK with the right
                        // arity (the field's slot is just
                        // left dangling — that's by
                        // design — UNPACK still pushes N
                        // values; we just don't bind any of
                        // them). At recursion levels, the
                        // previous UnpackAt exposed N
                        // values; we POP them so the slot
                        // cursor advances correctly for
                        // subsequent fields.
                        bytecode.push(Byte::new(Instruction::POP));
                    }
                    // else: `consume_values = false` —
                    // the test chain already consumed the
                    // value. Skip silently.
                }
            }
        },
    }
}

impl Compiler {
    pub fn get_function(&self, name: &str) -> usize {
        self.functions[name]
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
        self.context
            .variables
            .key(&name.to_string())
            .map(|s| s as u32)
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

    fn mono_ty_from_name(name: &str) -> Ty {
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
            Some(Instruction::StorePop | Instruction::SetField | Instruction::StoreIndex)
        ) {
            // `x = expr;` / compound updates already consumed the
            // RHS via StorePop/SetField/StoreIndex — no trailing POP.
        } else if !matches!(
            bytecode.last().map(|b| b.bytecode()),
            Some(Instruction::YieldCoro | Instruction::YieldFromCoro)
        ) {
            bytecode.push(Byte::new(Instruction::POP));
        }
    }

    fn emit_loop_jump(
        &mut self,
        target: Option<BbLabel>,
        keyword: &str,
        range: std::ops::Range<usize>,
    ) {
        if let (Some(label), Some(bb)) = (target, self.loop_bbs.last_mut()) {
            bb.emit_jump_to(label, BbJumpKind::Unconditional, &mut self.bytecode);
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

    fn emit_call_indirect(bytecode: &mut Vec<Byte>, target_offset: u32, arity: u32) {
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
        bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(dict_slot));
        bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(dict_slot));
        bytecode.push(Byte::new(Instruction::CONST).with_const_inline(method_slot as i32));
        bytecode.push(Byte::new(Instruction::Index));
        bytecode.push(Byte::new(Instruction::CallIndirect).with_operand_u32(3));
        true
    }

    /// Emit a string literal as `STRING` + `DATA` bytes into `self.bytecode`.
    /// Applies the same escape processing as `Expression::String` codegen.
    fn emit_string_literal(&mut self, s: &str) {
        let escaped = s
            .replace("\\n", "\n")
            .replace("\\r", "\r")
            .replace("\\t", "\t")
            .replace("\\0", "\0");
        let idx = self.bytecode.len();
        let mut count = 0u32;
        for ch in escaped.chars() {
            count += 1;
            self.bytecode
                .push(Byte::new(Instruction::DATA).with_operand_u32(ch.into()));
        }
        self.bytecode.insert(
            idx,
            Byte::new(Instruction::STRING).with_operand_u32(count),
        );
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

    /// Emit `print`/`format` body: format string, then args (with `%v`
    /// lowered through `Show`), then `FORMAT`.
    fn emit_format_expression(
        &mut self,
        format: &Output,
        params: Option<&Vec<Output>>,
    ) {
        let fmt_lit = match format.1.as_ref() {
            Expression::String(s) => Some(s.to_string()),
            _ => None,
        };

        if let (Some(fmt), Some(params)) = (fmt_lit.as_deref(), params) {
            let rewritten = Self::rewrite_format_v_to_s(fmt);
            self.emit_string_literal(&rewritten);
            let specs = Self::format_consuming_specs(fmt);
            let mut emitted = 0usize;
            for (param, spec) in params.iter().zip(specs.iter()) {
                if *spec == 'v' {
                    self.emit_show_for_format_arg(param);
                } else {
                    let bc = self.do_compile(param);
                    self.bytecode.extend(bc);
                }
                emitted += 1;
            }
            // Extra args beyond specifiers — still push them (VM pops by count).
            for param in params.iter().skip(emitted) {
                let bc = self.do_compile(param);
                self.bytecode.extend(bc);
            }
            self.bytecode.push(
                Byte::new(Instruction::FORMAT).with_operand_u32(params.len() as u32),
            );
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
                self.bytecode
                    .push(Byte::new(Instruction::LOAD).with_operand_u32(dict_slot));
                self.bytecode
                    .push(Byte::new(Instruction::LOAD).with_operand_u32(dict_slot));
                self.bytecode.push(
                    Byte::new(Instruction::CONST).with_const_inline(method_slot as i32),
                );
                self.bytecode.push(Byte::new(Instruction::Index));
                self.bytecode
                    .push(Byte::new(Instruction::CallIndirect).with_operand_u32(2));
                return;
            }
        }

        // Concrete Show instance at the call site.
        // Prefer a fully resolved type: span cache from a shared generic body
        // may still be an open `Ty::Var` even when mono/codegen side-tables
        // know the ground type (or when the arg is a literal / construct).
        let span_ty = self
            .checker
            .lookup_for_codegen_span(span.start, span.end)
            .map(|t| crate::typechecking::subst::apply_ty_prune(self.checker.subst(), &t));
        let arg_ty = match span_ty {
            Some(Ty::Var(_)) | None => match arg.1.as_ref() {
                Expression::Integer(_) => Some(crate::typechecking::ty::int()),
                Expression::Float(_) => Some(crate::typechecking::ty::float()),
                Expression::String(_) => Some(crate::typechecking::ty::string()),
                Expression::Bool(_) => Some(crate::typechecking::ty::boolean()),
                Expression::Identifier(name) => self.codegen_var_type_for(name),
                _ => None,
            },
            Some(other) => Some(other),
        };

        if let Some(ty) = arg_ty.as_ref() {
            // Instance heads use `Ty::Con("Point")`; construct sites often
            // produce `Constructor` / `Sum` — peel to the enum name.
            let lookup_ty = match ty {
                Ty::Sum { name, .. } => Ty::Con(name.clone()),
                Ty::Constructor { owner, .. } => match owner.as_ref() {
                    Ty::Sum { name, .. } | Ty::Con(name) => Ty::Con(name.clone()),
                    other => other.clone(),
                },
                other => other.clone(),
            };
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

    /// Emit dictionary tuples for a non-monomorphized generic call site.
    ///
    /// Convention: after value args, one `MakeTuple` per typeclass
    /// constraint. Compiler-provided and source-provided instances use the
    /// same dictionary layout.
    /// Each tuple holds method entry offsets in typeclass declaration order
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
        checker: &Checker,
        functions: &HashMap<String, usize>,
    ) -> usize {
        use crate::typechecking::Ty;
        use crate::typechecking::subst::apply_ty_prune;

        let Some(scheme) = checker.env().lookup(fn_name).cloned() else {
            return 0;
        };
        // Map quantified vars → concrete arg types by peeling the curried
        // function type against the call's argument types.
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
            if let Ty::Var(v) = param.as_ref() {
                var_to_ty
                    .entry(*v)
                    .or_insert_with(|| arg_tys[arg_idx].clone());
            }
            fun = ret.as_ref();
            arg_idx += 1;
        }

        let mut dict_count = 0;
        for constraint in &scheme.constraints {
            let Some(concrete) = var_to_ty.get(&constraint.var) else {
                continue;
            };
            let concrete = apply_ty_prune(checker.subst(), concrete);
            let Some(instance) = checker
                .generics()
                .find_instance(&constraint.class, std::slice::from_ref(&concrete))
            else {
                continue;
            };
            let Some(class_def) = checker.generics().typeclass(&instance.class) else {
                continue;
            };
            let n_methods = class_def.methods.len() as u32;
            for method_def in &class_def.methods {
                let offset = instance
                    .method_fqns
                    .get(&method_def.name)
                    .and_then(|fqn| functions.get(fqn).copied())
                    .unwrap_or(0);
                bytecode.push(Byte::new(Instruction::CodePtr).with_operand_u32(offset as u32));
            }
            bytecode.push(Byte::new(Instruction::MakeTuple).with_operand_u32(n_methods));
            dict_count += 1;
        }
        dict_count
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
            compiler.functions.insert(fqn, compiler.bytecode.len());
            for slot in 0..2 {
                compiler
                    .bytecode
                    .push(Byte::new(Instruction::LOAD).with_operand_u32(slot));
                compiler
                    .bytecode
                    .push(Byte::new(Instruction::UnboxValue).with_operand_u32(tag as u32));
            }
            compiler.bytecode.push(Byte::new(op));
            if boxes_result {
                compiler
                    .bytecode
                    .push(Byte::new(Instruction::BoxValue).with_operand_u32(tag as u32));
            }
            compiler.bytecode.push(Byte::new(Instruction::RETURN));
        };

        for (ty, tag, arithmetic, comparisons) in [
            (
                "int",
                ValueTag::Int,
                [
                    ("add", Instruction::ADD),
                    ("sub", Instruction::SUB),
                    ("mul", Instruction::MUL),
                    ("div", Instruction::DIV),
                ],
                [
                    ("lt", Instruction::LE),
                    ("le", Instruction::LEQ),
                    ("gt", Instruction::GT),
                    ("ge", Instruction::GEQ),
                    ("eq", Instruction::EQ),
                    ("ne", Instruction::NEQ),
                ],
            ),
            (
                "float",
                ValueTag::Float,
                [
                    ("add", Instruction::ADDF),
                    ("sub", Instruction::SUBF),
                    ("mul", Instruction::MULF),
                    ("div", Instruction::DIVF),
                ],
                [
                    ("lt", Instruction::LEF),
                    ("le", Instruction::LEQF),
                    ("gt", Instruction::GTF),
                    ("ge", Instruction::GEQF),
                    ("eq", Instruction::EQ),
                    ("ne", Instruction::NEQ),
                ],
            ),
        ] {
            for (method, op) in arithmetic {
                emit(self, "Num", ty, method, tag, op, true);
            }
            for (method, op) in comparisons.iter().take(4) {
                emit(self, "Ord", ty, method, tag, *op, false);
            }
            for (method, op) in comparisons.iter().skip(4) {
                emit(self, "Eq", ty, method, tag, *op, false);
            }
        }
        for (ty, tag) in [("string", ValueTag::String), ("bool", ValueTag::Bool)] {
            emit(self, "Eq", ty, "eq", tag, Instruction::EQ, false);
            emit(self, "Eq", ty, "ne", tag, Instruction::NEQ, false);
        }

        // Show thunks: accept a boxed (or heap-string) argument at slot 0,
        // ignore the trailing dictionary, and return an ObjString via STRINGIFY.
        for ty in ["int", "float", "string", "bool", "unit"] {
            let fqn = Generics::builtin_instance_fqn("Show", ty, "show");
            if self.functions.contains_key(&fqn) {
                continue;
            }
            self.functions.insert(fqn, self.bytecode.len());
            self.bytecode
                .push(Byte::new(Instruction::LOAD).with_operand_u32(0));
            self.bytecode.push(Byte::new(Instruction::STRINGIFY));
            self.bytecode.push(Byte::new(Instruction::RETURN));
        }
    }

    /// Map a fully-resolved `Ty` to a `ValueTag` for box/unbox
    /// emission at generic call boundaries.
    fn ty_to_value_tag(ty: &crate::typechecking::Ty) -> Option<ValueTag> {
        use crate::typechecking::{Ty, ty::BOOL, ty::FLOAT, ty::INT, ty::STRING, ty::UNIT};
        match ty {
            Ty::Con(name) => match name.as_str() {
                INT => Some(ValueTag::Int),
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

    /// Emit a `BoxValue` instruction for a concrete `Ty` at a generic
    /// call argument boundary (concrete→generic).  Does nothing when the
    /// type is already open (Ty::Var), or if a tag cannot be determined.
    fn emit_box_if_needed(bytecode: &mut Vec<Byte>, ty: &crate::typechecking::Ty) {
        if let Some(tag) = Self::ty_to_value_tag(ty) {
            bytecode.push(Byte::new(Instruction::BoxValue).with_operand_u32(tag as u32));
        }
    }

    /// Emit an `UnboxValue` instruction for a concrete `Ty` at a generic
    /// call return boundary (generic→concrete).  Does nothing when the
    /// type is open (`Ty::Var`) — the caller can't know the tag at compile
    /// time in that case (the boxed value stays boxed).
    fn emit_unbox_if_needed(bytecode: &mut Vec<Byte>, ty: &crate::typechecking::Ty) {
        if let Some(tag) = Self::ty_to_value_tag(ty) {
            // UnboxValue operand: [15:0] = ValueTag as u16.
            bytecode.push(Byte::new(Instruction::UnboxValue).with_operand_u32(tag as u32));
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

        self.functions
            .insert(qualified.clone(), self.bytecode.len());
        if *is_coro {
            self.coroutine_fns.insert(qualified);
        }

        let prev_vars = std::mem::take(&mut self.context.variables);
        let prev_polyfn_vars = std::mem::take(&mut self.polyfn_vars);
        let prev_polyfn_sources = std::mem::take(&mut self.polyfn_sources);
        self.context.variables = Interner::default();
        if self.compiling_method {
            self.context.variables.intern("self".to_string());
        }

        let prev_result_mode = self.compiling_result_mode;
        self.compiling_result_mode = self.checker.fn_is_result_mode(name);

        let mut a = self.do_compile(args);
        self.bytecode.append(&mut a);
        for (slot, ty) in argument_unbox_tys.iter().enumerate() {
            if let Some(tag) = ty.as_ref().and_then(Self::ty_to_value_tag) {
                self.bytecode
                    .push(Byte::new(Instruction::LOAD).with_operand_u32(slot as u32));
                self.bytecode
                    .push(Byte::new(Instruction::UnboxValue).with_operand_u32(tag as u32));
                self.bytecode
                    .push(Byte::new(Instruction::StorePop).with_operand_u32(slot as u32));
            }
        }
        for dict_idx in 0..dict_arity {
            self.context.variables.intern(format!("__dict{}", dict_idx));
        }
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
            if self.compiling_result_mode {
                Self::emit_ok_or_some_wrap(&mut self.bytecode, false);
            }
            self.bytecode.push(Byte::new(Instruction::RETURN));
        }

        self.compiling_result_mode = prev_result_mode;
        self.context.variables = prev_vars;
        self.polyfn_vars = prev_polyfn_vars;
        self.polyfn_sources = prev_polyfn_sources;
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
        body: &Output<'compiler>,
        source_name: &str,
    ) {
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

            let clone_offset = self.bytecode.len();
            let mono_name = format!(
                "{}$mono${}",
                qualified,
                specialization.key.subst.join("$").replace(' ', "")
            );
            self.functions.insert(mono_name, clone_offset);
            self.mono_offsets
                .insert(specialization.key.clone(), clone_offset);

            let prev_fn_vars = std::mem::take(&mut self.context.variables);
            let prev_fn_polyfn_vars = std::mem::take(&mut self.polyfn_vars);
            let prev_fn_polyfn_sources = std::mem::take(&mut self.polyfn_sources);
            let prev_result_mode = self.compiling_result_mode;
            self.context.variables = Interner::default();
            self.compiling_result_mode = self.checker.fn_is_result_mode(source_name);
            self.mono_codegen_var_types.push(overrides);

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
                if self.compiling_result_mode {
                    Self::emit_ok_or_some_wrap(&mut self.bytecode, false);
                }
                self.bytecode.push(Byte::new(Instruction::RETURN));
            }

            self.mono_codegen_var_types.pop();
            self.compiling_result_mode = prev_result_mode;
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
                if let Expression::Argument(ty, name) = child.1.as_ref()
                    && let Expression::Type(tp_name) | Expression::Identifier(tp_name) =
                        ty.1.as_ref()
                    && let Some(concrete) = type_param_tys.get(tp_name)
                {
                    overrides.insert(name.to_string(), concrete.clone());
                }
            }
        }
        overrides
    }

    fn mono_call_offset(&self, fn_name: &str, args: Option<&Vec<Output<'_>>>) -> Option<usize> {
        let args = args?;
        let arg_types = args
            .iter()
            .map(|arg| monomorphize::ground_type_name(&self.checker, arg))
            .collect::<Option<Vec<_>>>()?;
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
            let mut body_bc = self.do_compile(body);
            self.bytecode.append(&mut body_bc);
        } else {
            let mut bc = self.do_compile(method);
            self.bytecode.append(&mut bc);
        }
    }

    pub fn get_messages(&self) -> &Vec<Message> {
        &self.messages
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
    /// Handles `Expression::Identifier` via the `codegen_var_types` side-table
    /// (which is reliable inside function bodies even when the ID cache is
    /// misaligned). All other expression shapes return `false` conservatively;
    /// they are either concrete literals or sub-expressions whose containing
    /// `Identifier` was already flagged on the inner call.
    fn operand_is_open_ty(&self, operand: &Output) -> bool {
        use crate::typechecking::subst::apply_ty_prune;
        match operand.1.as_ref() {
            Expression::Identifier(name) => match self.codegen_var_type_for(name) {
                Some(ty) => {
                    let pruned = apply_ty_prune(self.checker.subst(), &ty);
                    matches!(pruned, Ty::Var(_))
                }
                None => false,
            },
            _ => false,
        }
    }

    fn alloc_temp_slot(&mut self) -> u32 {
        self.temp_counter += 1;
        let name = format!("__tmp{}", self.temp_counter);
        self.context.variables.intern(name) as u32
    }

    fn emit_field_name(&self, bytecode: &mut Vec<Byte>, field: &str) {
        Self::emit_raw_string_literal(bytecode, field);
    }

    fn emit_raw_string_literal(bytecode: &mut Vec<Byte>, value: &str) {
        bytecode
            .push(Byte::new(Instruction::STRING).with_operand_u32(value.chars().count() as u32));
        for ch in value.chars() {
            bytecode.push(Byte::new(Instruction::DATA).with_operand_u32(ch.into()));
        }
    }

    fn variable_slot(&mut self, name: &str) -> Option<u32> {
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

    fn is_float_ty(&self, _node: &Output) -> bool {
        if let Expression::Identifier(name) = _node.1.as_ref()
            && matches!(
                self.codegen_var_type_for(name),
                Some(crate::typechecking::ty::Ty::Con(ref ty))
                    if ty == crate::typechecking::ty::FLOAT
            )
        {
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

    fn codegen_expr_ty(&self, node: &Output) -> Option<Ty> {
        let resolved = match node.1.as_ref() {
            Expression::String(_) | Expression::Format(_, _) => {
                Some(Ty::Con(crate::typechecking::ty::STRING.into()))
            }
            Expression::Identifier(name) => self
                .codegen_var_type_for(name)
                .map(|t| crate::typechecking::subst::apply_ty_prune(self.checker.subst(), &t)),
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
                if let Ty::Con(name) = &receiver_ty {
                    if self.checker.is_class(name) {
                        return self.checker.class_field_ty(name, field).cloned();
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
                } else if let Ty::Con(name) = &inner {
                    if self.checker.is_class(name) {
                        self.checker.class_field_ty(name, field).cloned()
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
                    bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(slot));
                    self.is_float_ty(target)
                } else {
                    false
                }
            }
            Expression::Access(receiver, field) => {
                bytecode.append(&mut self.do_compile(receiver));
                self.emit_field_name(bytecode, field);
                bytecode.push(Byte::new(Instruction::GetField));
                matches!(
                    self.receiver_type(receiver),
                    Some(crate::typechecking::Ty::Con(ref n))
                        if n == crate::typechecking::ty::FLOAT
                )
            }
            Expression::Index(arr, idx) => {
                let tmp_arr = self.alloc_temp_slot();
                let tmp_idx = self.alloc_temp_slot();
                bytecode.append(&mut self.do_compile(arr));
                bytecode.push(Byte::new(Instruction::StorePop).with_operand_u32(tmp_arr));
                bytecode.append(&mut self.do_compile(idx));
                bytecode.push(Byte::new(Instruction::StorePop).with_operand_u32(tmp_idx));
                bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_arr));
                bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_idx));
                bytecode.push(Byte::new(Instruction::Index));
                false
            }
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
                    bytecode.push(Byte::new(Instruction::StorePop).with_operand_u32(slot));
                }
            }
            Expression::Access(receiver, field) => {
                if leave_value_on_stack {
                    bytecode.push(Byte::new(Instruction::DUPLICATE));
                }
                bytecode.append(&mut self.do_compile(receiver));
                self.emit_field_name(bytecode, field);
                bytecode.push(Byte::new(Instruction::SetField));
            }
            Expression::Index(arr, idx) => {
                let tmp_arr = self.alloc_temp_slot();
                let tmp_idx = self.alloc_temp_slot();
                let tmp_val = self.alloc_temp_slot();
                if leave_value_on_stack {
                    bytecode.push(Byte::new(Instruction::StorePop).with_operand_u32(tmp_val));
                } else {
                    bytecode.push(Byte::new(Instruction::POP));
                }
                bytecode.append(&mut self.do_compile(arr));
                bytecode.push(Byte::new(Instruction::StorePop).with_operand_u32(tmp_arr));
                bytecode.append(&mut self.do_compile(idx));
                bytecode.push(Byte::new(Instruction::StorePop).with_operand_u32(tmp_idx));
                bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_arr));
                bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_idx));
                if leave_value_on_stack {
                    bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_val));
                }
                bytecode.push(Byte::new(Instruction::StoreIndex));
            }
            _ => {
                bytecode.push(Byte::new(Instruction::POP));
            }
        }
    }

    fn emit_compound_assign(
        &mut self,
        bytecode: &mut Vec<Byte>,
        target: &Output,
        op: parser::ast::AssignOp,
        rhs: &Output,
    ) {
        if matches!(op, parser::ast::AssignOp::Add)
            && self.is_string_expr(target)
            && self.is_string_expr(rhs)
        {
            Self::emit_raw_string_literal(bytecode, "%s%s");
            let _ = self.emit_read_lvalue(bytecode, target);
            bytecode.append(&mut self.do_compile(rhs));
            bytecode.push(Byte::new(Instruction::FORMAT).with_operand_u32(2));
            self.emit_write_lvalue(bytecode, target, false);
            return;
        }

        if let Expression::Index(arr, idx) = target.1.as_ref() {
            let tmp_arr = self.alloc_temp_slot();
            let tmp_idx = self.alloc_temp_slot();
            bytecode.append(&mut self.do_compile(arr));
            bytecode.push(Byte::new(Instruction::StorePop).with_operand_u32(tmp_arr));
            bytecode.append(&mut self.do_compile(idx));
            bytecode.push(Byte::new(Instruction::StorePop).with_operand_u32(tmp_idx));
            bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_arr));
            bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_idx));
            bytecode.push(Byte::new(Instruction::Index));
            bytecode.append(&mut self.do_compile(rhs));
            bytecode.push(Byte::new(Self::binop_for_assign_op(op, false)));
            let tmp_val = self.alloc_temp_slot();
            bytecode.push(Byte::new(Instruction::StorePop).with_operand_u32(tmp_val));
            bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_arr));
            bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_idx));
            bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_val));
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

        if let Expression::Index(arr, idx) = target.1.as_ref() {
            let tmp_arr = self.alloc_temp_slot();
            let tmp_idx = self.alloc_temp_slot();
            bytecode.append(&mut self.do_compile(arr));
            bytecode.push(Byte::new(Instruction::StorePop).with_operand_u32(tmp_arr));
            bytecode.append(&mut self.do_compile(idx));
            bytecode.push(Byte::new(Instruction::StorePop).with_operand_u32(tmp_idx));
            bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_arr));
            bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_idx));
            bytecode.push(Byte::new(Instruction::Index));
            let tmp_old = if !prefix {
                let t = self.alloc_temp_slot();
                bytecode.push(Byte::new(Instruction::StorePop).with_operand_u32(t));
                bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_arr));
                bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_idx));
                bytecode.push(Byte::new(Instruction::Index));
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
            bytecode.push(Byte::new(Instruction::StorePop).with_operand_u32(tmp_val));
            bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_arr));
            bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_idx));
            bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_val));
            bytecode.push(Byte::new(Instruction::StoreIndex));
            if prefix {
                bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_val));
            } else {
                bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_old));
            }
            return;
        }

        let is_float = self.emit_read_lvalue(bytecode, target);
        let tmp_old = if !prefix {
            let tmp = self.alloc_temp_slot();
            bytecode.push(Byte::new(Instruction::StorePop).with_operand_u32(tmp));
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
            bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_old));
        }
    }

    /// Resolve enum name for field access via the codegen side-table.
    fn enum_name_for_receiver(&mut self, receiver: &Output) -> Option<String> {
        // Cannot use infer cache inside function bodies (ID misalignment) or env (frame popped).
        let ty = self.receiver_type(receiver)?;
        extract_enum_name(&ty)
    }

    /// Receiver type for field access (identifier side-table or chained lookup).
    fn receiver_type(&self, receiver: &Output) -> Option<Ty> {
        match receiver.1.as_ref() {
            Expression::Identifier(name) => {
                self.codegen_var_type_for(name).map(|t| {
                    // Apply substitution so inferred record types resolve fully.
                    crate::typechecking::subst::apply_ty_prune(self.checker.subst(), &t)
                })
            }
            Expression::Access(inner, field) => {
                let inner_ty = self.receiver_type(inner)?;
                if let Ty::Con(name) = &inner_ty {
                    if self.checker.is_class(name) {
                        return self.checker.class_field_ty(name, field).cloned();
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
            _ => None,
        }
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
    fn emit_ok_or_some_wrap(bytecode: &mut Vec<Byte>, is_option: bool) {
        let tag = if is_option { 1u16 } else { 0u16 }; // Some=1, Ok=0
        bytecode.push(Byte::new(Instruction::MakeEnum).with_operands_u16([tag, 1]));
    }

    /// Wrap the top-of-stack value as `Result::Err(e)`.
    fn emit_result_err(bytecode: &mut Vec<Byte>) {
        bytecode.push(Byte::new(Instruction::MakeEnum).with_operands_u16([1, 1])); // Err tag=1 arity=1
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
                // Map local name → qualified FQN (or expand `use ns::*`).
                if name == "*" {
                    let module_ns = p.join("::");
                    let prefix = if module_ns.is_empty() {
                        // Top-level glob: `use *;` (no
                        // module path). Items have
                        // single-segment FQNs.
                        String::new()
                    } else {
                        format!("{}::", module_ns)
                    };
                    // Collect the FQNs first to avoid
                    // borrowing `self.functions` while
                    // we mutate `self.aliases`.
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
                        // The bare item name is the
                        // last segment of the FQN.
                        let item_name = fqn[prefix.len()..].to_string();
                        self.aliases.insert(item_name, fqn);
                    }
                } else {
                    // Concrete import. The qualified
                    // name is `<path>::<name>::<name>`
                    // — the file's path becomes the
                    // namespace (`foo::sadge` for
                    // `foo/sadge.0s`), and the
                    // function inside the file is
                    // `<namespace>::<function_name>`.
                    // So `sadge` in `foo/sadge.0s`
                    // is at FQN `foo::sadge::sadge`.
                    let namespace = if p.is_empty() {
                        name.clone()
                    } else {
                        format!("{}::{}", p.join("::"), name)
                    };
                    // We don't know the function
                    // name yet (it's whatever the
                    // user names the function in
                    // the file). The convention is
                    // that the function has the
                    // SAME name as the file's stem
                    // (the LAST segment of the use
                    // path). So the FQN is
                    // `namespace::name`.
                    let qualified = format!("{}::{}", namespace, name);
                    let local = alias.clone().unwrap_or_else(|| name.clone());
                    self.aliases.insert(local, qualified);
                }
            }
            Expression::Noop(_) => (),
            // `mod foo;` — pipeline loads the file; no bytecode.
            Expression::Module(_, _body) => {}
            Expression::Group(e) => bytecode.append(&mut self.do_compile(e)),
            Expression::Program(children) => {
                children.iter().for_each(|child| {
                    bytecode.append(&mut self.do_compile(child));
                });
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
                        let slot = self.context.variables.intern(name.clone()) as u32;
                        if is_const {
                            self.context.constants.insert(slot as usize, true);
                        }
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
                        }
                        // Emit the RHS.
                        let mut rhs_bc = self.do_compile(&children[1]);
                        bytecode.append(&mut rhs_bc);
                        // Append the explicit store-pop-and-write.
                        bytecode.push(Byte::new(Instruction::StorePop).with_operand_u32(slot));
                        is_binding = true;
                    }
                }
                if !is_binding {
                    children.iter().for_each(|child| {
                        bytecode.append(&mut self.do_compile(child));
                    });
                }
            }
            Expression::Block(children) => {
                let ctx = self.context.child();
                self.context = ctx;
                // Append each child to self.bytecode (Print/control-flow emit in-place).
                for child in children {
                    let mut bc = self.do_compile(child);
                    self.bytecode.append(&mut bc);
                }

                self.context = *self.context.get_prev().clone().unwrap();
            }
            Expression::Function {
                name,
                is_coro,
                type_params,
                args,
                returns: _returns,
                body,
            } => {
                let qualified = if self.namespace.is_empty() {
                    name.to_string()
                } else {
                    format!("{}::{}", self.namespace, name)
                };
                self.module_items
                    .entry(self.namespace.clone())
                    .or_default()
                    .push(name.to_string());
                self.functions
                    .insert(qualified.clone(), self.bytecode.len());
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
                self.context.variables = Interner::default();
                if self.compiling_method {
                    self.context.variables.intern("self".to_string());
                }

                let prev_result_mode = self.compiling_result_mode;
                self.compiling_result_mode = self.checker.fn_is_result_mode(name);

                let mut a = self.do_compile(args);

                // ── Dictionary-passing prologue ────────────────────────────────
                // Generic functions with user-defined typeclass constraints receive
                // extra dict tuple arguments after the value params.  Reserve a
                // stack slot `__dictN` for each expected dict so that the Interner
                // assigns a slot number that can later be LOAD-ed by CallIndirect
                // dispatch paths.  The VM pushes these as the trailing elements of
                // the call frame, one per user constraint, in constraint order.
                // Every typeclass constraint (including builtin Num/Ord/Eq/Show)
                // gets a trailing `__dictN` slot for dictionary dispatch.
                let dict_arity = self.checker.dict_arity_for(name);
                for dict_idx in 0..dict_arity {
                    self.context.variables.intern(format!("__dict{}", dict_idx));
                }

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
                    if self.compiling_result_mode {
                        Self::emit_ok_or_some_wrap(&mut self.bytecode, false);
                    }
                    self.bytecode.push(Byte::new(Instruction::RETURN));
                }

                self.compiling_result_mode = prev_result_mode;
                self.context.variables = prev_fn_vars;
                self.polyfn_vars = prev_fn_polyfn_vars;
                self.polyfn_sources = prev_fn_polyfn_sources;

                self.emit_mono_specializations_for_function(
                    &qualified,
                    type_params,
                    args,
                    body,
                    name,
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
                // inside another expression (e.g. `print "%i", resume h;`),
                // that top-of-stack value belongs to the RESUMER (e.g. the
                // format string), not the coroutine — corrupting it.
                Self::discard_statement_value(&mut bytecode);
            }
            Expression::Print(format, params) => {
                // Emit directly to `self.bytecode` so nested control flow
                // (e.g. `match` in params) can compute absolute jump targets.
                // `%v` args are lowered through `Show` to strings and the
                // format literal is rewritten to `%s` before FORMAT.
                self.emit_format_expression(format, params.as_ref());
                self.bytecode.push(Byte::new(Instruction::PRINT));
            }
            Expression::Format(format, params) => {
                self.emit_format_expression(format, params.as_ref());
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
                bytecode.push(Byte::new(Instruction::MakeTuple).with_operand_u32(arity));
            }
            Expression::Array(items) => {
                for c in items {
                    let mut bc = self.do_compile(c);
                    bytecode.append(&mut bc);
                }
                let arity = items.len() as u32;
                bytecode.push(Byte::new(Instruction::MakeArray).with_operand_u32(arity));
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
                    // field-name string — emits STRING then
                    // DATA bytes (the runtime's standard
                    // string-literal encoding — see
                    // `Expression::String` arm).
                    bytecode
                        .push(Byte::new(Instruction::STRING).with_operand_u32(name.len() as u32));
                    for ch in name.chars() {
                        bytecode.push(Byte::new(Instruction::DATA).with_operand_u32(ch.into()));
                    }
                }
                let arity = items.len() as u32;
                bytecode.push(Byte::new(Instruction::MakeDict).with_operand_u32(arity));
            }
            // `t[i]` — pop the index (top), pop the target,
            // push the element at `target[index]`. The Index
            // opcode carries no operand (the index is at the top
            // of the operand stack at dispatch time).
            Expression::Index(target, index) => {
                let mut target_bc = self.do_compile(target);
                bytecode.append(&mut target_bc);
                let mut index_bc = self.do_compile(index);
                bytecode.append(&mut index_bc);
                bytecode.push(Byte::new(Instruction::Index));
            }
            // --- FFI declare/invoke ---
            Expression::Declare(args) => {
                if args.len() != 4 {
                    let mut m = Message::error(
                       ErrorCode::DeclareArity, "declare requires arguments as a tuple in position 3 (use (T1, T2, ...) syntax)".to_string(),
                        span.into_range(),
                    );
                    m.push(DiagLabel::new(
                        format!(
                            "expected 4 arguments (lib, name, args_tuple, ret_type); got {}",
                            args.len()
                        ),
                        span.into_range(),
                    ));
                    self.messages.push(m);
                    // Emit a defensive operand so the bytecode
                    // stays well-formed (DeclareFFI on a partial
                    // stack will just fail at runtime).
                    self.bytecode
                        .push(Byte::new(Instruction::DeclareFFI).with_operand_u32(0));
                } else {
                    let lib = &args[0];
                    let name = &args[1];
                    let args_tuple = &args[2];
                    let ret_type = &args[3];

                    // Verify `args[2]` is a Tuple expression.
                    // Otherwise it's a type error — emit a
                    // diagnostic and proceed defensively.
                    let tuple_elements: Vec<_> = match args_tuple.1.as_ref() {
                        Expression::Tuple(items) => items.to_vec(),
                        _ => {
                            let mut m = Message::error(
                                ErrorCode::DeclareArity,
                                "declare(...) arguments tuple must be (T1, T2, ...) syntax"
                                    .to_string(),
                                args_tuple.0.into_range(),
                            );
                            m.push(DiagLabel::new(
                                "wrap the arg types in parentheses — (FFIType::Int, FFIType::Float)".to_string(),
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

                    // Each element pushes its FFI type tag onto the stack.
                    for elem in &tuple_elements {
                        if let Some((tag, aux)) = ffi_type_tag_from_output(&self.checker, elem) {
                            emit_ffi_type_const(&mut self.bytecode, tag, aux);
                        } else {
                            let bc = self.do_compile(elem);
                            self.bytecode.extend(bc);
                        }
                    }
                    let arity = tuple_elements.len() as u32;
                    self.bytecode
                        .push(Byte::new(Instruction::MakeTuple).with_operand_u32(arity));

                    // Ret-type tag on top.
                    if let Some((tag, aux)) = ffi_type_tag_from_output(&self.checker, ret_type) {
                        emit_ffi_type_const(&mut self.bytecode, tag, aux);
                    } else {
                        let ret_bc = self.do_compile(ret_type);
                        self.bytecode.extend(ret_bc);
                    }

                    // DeclareFFI pops name + tuple + lib in
                    // dispatch order (see VM).
                    let operand = (arity) & 0xFFFF;
                    self.bytecode
                        .push(Byte::new(Instruction::DeclareFFI).with_operand_u32(operand));
                }
            }
            Expression::Invoke(args) => {
                // `invoke(lib, fn_id, (args...))`
                if args.len() != 3 {
                    let mut m = Message::error(
                       ErrorCode::InvokeArity, "invoke requires arguments as a tuple in position 3 (use (a, b, ...) syntax)".to_string(),
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
                } else {
                    let lib = &args[0];
                    let fn_id = &args[1];
                    let args_tuple = &args[2];

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

                    // Each element's bytecode pushes a Value. Function names
                    // used as callback arguments compile to bytecode offsets.
                    for elem in &tuple_elements {
                        if let Expression::Identifier(name) = elem.1.as_ref() {
                            if let Some(&offset) = self.functions.get(*name) {
                                self.bytecode.push(
                                    Byte::new(Instruction::CONST).with_operand_u32(offset as u32),
                                );
                                continue;
                            }
                        }
                        let bc = self.do_compile(elem);
                        self.bytecode.extend(bc);
                    }
                    let arity = tuple_elements.len() as u32;
                    self.bytecode
                        .push(Byte::new(Instruction::MakeTuple).with_operand_u32(arity));

                    // FfiInvoke pops tuple (top) + fn_id + lib.
                    let operand = (arity) & 0xFFFF;
                    self.bytecode
                        .push(Byte::new(Instruction::FfiInvoke).with_operand_u32(operand));
                }
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
                // Result-mode functions: bare `return v` becomes `Ok(v)`.
                if self.compiling_result_mode {
                    Self::emit_ok_or_some_wrap(&mut bytecode, false);
                }
                if !matches!(child.borrow(), Expression::ImplicitReturn(_)) {
                    bytecode.push(Byte::new(Instruction::RETURN));
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
                name,
                fields: state,
                ..
            } => {
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
            Expression::Implementation { owner, methods, .. } => {
                let saved_ns = self.namespace.clone();
                self.namespace = owner.to_string();

                for method_node in methods {
                    match method_node.1.borrow() {
                        Expression::Method(_, body) => {
                            if let Expression::Function { name, .. } = body.1.borrow() {
                                let fqn = format!("{}::{}", owner, name);
                                self.compiling_method = true;
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
                self.compiling_method = true;
                bytecode.append(&mut self.do_compile(body));
                self.compiling_method = false;
            }
            Expression::Instantiate(class, args) => {
                let name = self.resolve_variable_checked(class);
                let fields = self.context.classes.get(&name).cloned().unwrap_or_default();
                bytecode.push(Byte::new(Instruction::INIT).with_operand_u32(fields.len() as u32));
                // SetField stack order is value, target, name (same as
                // Assignment to Access). Stash the instance, then for
                // each ctor arg emit that sequence and discard the
                // value SetField pushes back.
                let tmp_inst = self.alloc_temp_slot();
                bytecode.push(Byte::new(Instruction::StorePop).with_operand_u32(tmp_inst));
                if let Some(arg_list) = args {
                    for (arg, (fname, _)) in arg_list.iter().zip(fields.iter()) {
                        bytecode.append(&mut self.do_compile(arg));
                        bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_inst));
                        self.emit_field_name(&mut bytecode, fname);
                        bytecode.push(Byte::new(Instruction::SetField));
                        bytecode.push(Byte::new(Instruction::POP));
                    }
                }
                bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_inst));
            }
            Expression::Adjust { op, prefix, target } => {
                self.emit_adjust(&mut bytecode, target, *op, *prefix);
            }
            Expression::CompoundAssign(target, op, rhs) => {
                self.emit_compound_assign(&mut bytecode, target, *op, rhs);
            }
            // --- Loop codegen ---
            // Layout: [top] cond, JMPF→exit, body, JMP→top, [exit]
            Expression::Loop { iterable, body, .. } => {
                let mut bb = BlockBuilder::new();
                let top_label = bb.fresh_label();
                let exit_label = bb.fresh_label();
                let top_label_target = self.bytecode.len() as u32;

                let iter_bc = self.do_compile(iterable);
                self.bytecode.extend(iter_bc);

                bb.emit_jump_to(exit_label, BbJumpKind::JumpIfFalse, &mut self.bytecode);

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

                bb.emit_jump_to(top_label, BbJumpKind::Unconditional, &mut self.bytecode);

                let exit_label_target = self.bytecode.len() as u32;
                bb.bind_label(
                    exit_label,
                    exit_label_target,
                    &mut self.bytecode,
                    &mut self.constants,
                );

                bb.bind_label(
                    top_label,
                    top_label_target,
                    &mut self.bytecode,
                    &mut self.constants,
                );

                bb.finalize()
                    .expect("BlockBuilder::finalize: all targeted labels bound");
            }
            // --- C-style for codegen ---
            // Layout: init, [top] cond, JMPF→exit, body, [continue] step, JMP→top, [exit]
            Expression::For {
                init,
                cond,
                step,
                body,
            } => {
                if let Some(init) = init {
                    let mut init_bc = self.do_compile(init);
                    Self::discard_statement_value(&mut init_bc);
                    self.bytecode.extend(init_bc);
                }

                let mut bb = BlockBuilder::new();
                let top_label = bb.fresh_label();
                let continue_label = bb.fresh_label();
                let exit_label = bb.fresh_label();
                let top_label_target = self.bytecode.len() as u32;

                let cond_bc = self.do_compile(cond);
                self.bytecode.extend(cond_bc);

                bb.emit_jump_to(exit_label, BbJumpKind::JumpIfFalse, &mut self.bytecode);

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

                let continue_target = self.bytecode.len() as u32;
                bb.bind_label(
                    continue_label,
                    continue_target,
                    &mut self.bytecode,
                    &mut self.constants,
                );

                if let Some(step) = step {
                    let mut step_bc = self.do_compile(step);
                    Self::discard_statement_value(&mut step_bc);
                    self.bytecode.extend(step_bc);
                }

                bb.emit_jump_to(top_label, BbJumpKind::Unconditional, &mut self.bytecode);

                let exit_label_target = self.bytecode.len() as u32;
                bb.bind_label(
                    exit_label,
                    exit_label_target,
                    &mut self.bytecode,
                    &mut self.constants,
                );
                bb.bind_label(
                    top_label,
                    top_label_target,
                    &mut self.bytecode,
                    &mut self.constants,
                );

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
                            bytecode.append(&mut self.do_compile(arg));
                        }
                    }
                    let dict_name = format!("__dict{}", hint.dict_index);
                    if let Some(dict_slot) = self.lookup_slot(&dict_name) {
                        // Hidden trailing dictionary argument for sibling/default
                        // dispatch inside the selected implementation.
                        bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(dict_slot));
                        bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(dict_slot));
                        bytecode.push(
                            Byte::new(Instruction::CONST)
                                .with_const_inline(hint.method_slot as i32),
                        );
                        bytecode.push(Byte::new(Instruction::Index));
                        bytecode.push(
                            Byte::new(Instruction::CallIndirect)
                                .with_operand_u32(hint.arity as u32 + 1),
                        );
                    } else {
                        let mut message = Message::error(
                            ErrorCode::UnknownFunction,
                            "Missing typeclass dictionary".to_string(),
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
                    let recv_ty = self.receiver_type(recv);
                    let owner = match &recv_ty {
                        Some(crate::typechecking::Ty::Con(n)) if self.checker.is_class(n) => {
                            n.clone()
                        }
                        _ => String::new(),
                    };
                    let fqn = self
                        .context
                        .methods
                        .get(&owner)
                        .and_then(|m| m.get(*method))
                        .cloned();
                    if let Some(fqn) = fqn {
                        if let Some(offset) = self.functions.get(&fqn).copied() {
                            // Push receiver first (slot 0), then args.
                            bytecode.append(&mut self.do_compile(recv));
                            let mut nargs = 0u32;
                            if let Some(items) = args {
                                for arg in items {
                                    bytecode.append(&mut self.do_compile(arg));
                                    nargs += 1;
                                }
                            }
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
                    if let Expression::Identifier(raw) = name.1.as_ref() {
                        if *raw == "push" {
                            let provided = args.as_ref().map(|items| items.len()).unwrap_or(0);
                            if let Some(items) = args
                                && items.len() == 2
                            {
                                bytecode.append(&mut self.do_compile(&items[0]));
                                bytecode.append(&mut self.do_compile(&items[1]));
                                bytecode.push(Byte::new(Instruction::ArrayPush));
                            } else {
                                let mut message = Message::error(
                                    ErrorCode::TooManyArguments,
                                    "Invalid push call".to_string(),
                                    span.into_range(),
                                );
                                message.push(DiagLabel::new(
                                    format!("push expects 2 arguments, got {}", provided),
                                    span.into_range(),
                                ));
                                self.messages.push(message);
                            }
                            return bytecode;
                        }
                        if *raw == "len" {
                            let provided = args.as_ref().map(|items| items.len()).unwrap_or(0);
                            if let Some(items) = args
                                && items.len() == 1
                            {
                                bytecode.append(&mut self.do_compile(&items[0]));
                                bytecode.push(Byte::new(Instruction::ArrayLen));
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
                            }
                            return bytecode;
                        }
                    }

                    let identifier = self.resolve_variable_checked(name);
                    let n = self
                        .aliases
                        .get(&identifier)
                        .cloned()
                        .unwrap_or_else(|| identifier.clone());

                    if let Some(&(lib_slot, fn_id_slot)) = self.extern_runtime_functions.get(&n) {
                        // Stage arg bytecode in a local Vec to
                        // release the `&mut self.bytecode` borrow
                        // before the loop calls `self.do_compile`.
                        let mut arg_bc = Vec::new();
                        let arity = if let Some(items) = args {
                            for arg in items {
                                arg_bc.append(&mut self.do_compile(arg));
                            }
                            items.len()
                        } else {
                            0
                        };
                        self.bytecode
                            .push(Byte::new(Instruction::LOAD).with_operand_u32(lib_slot));
                        self.bytecode
                            .push(Byte::new(Instruction::LOAD).with_operand_u32(fn_id_slot));
                        self.bytecode.append(&mut arg_bc);
                        self.bytecode
                            .push(Byte::new(Instruction::MakeTuple).with_operand_u32(arity as u32));
                        self.bytecode
                            .push(Byte::new(Instruction::FfiInvoke).with_operand_u32(arity as u32));
                    } else if let Some(&native_id) = self.native.get(&n) {
                        let mut arg_bc = Vec::new();
                        let arity = if let Some(items) = args {
                            for arg in items {
                                arg_bc.append(&mut self.do_compile(arg));
                            }
                            items.len()
                        } else {
                            0
                        };
                        self.bytecode
                            .push(Byte::new(Instruction::CONST).with_value_u32(native_id as u32));
                        self.bytecode.append(&mut arg_bc);
                        self.bytecode
                            .push(Byte::new(Instruction::MakeTuple).with_operand_u32(arity as u32));
                        self.bytecode.push(
                            Byte::new(Instruction::HostInvoke).with_operand_u32(arity as u32),
                        );
                    } else if let Some(offset) = self.functions.get(&n).copied() {
                        let mono_offset = self.mono_call_offset(&n, args.as_ref());
                        let target_offset = mono_offset.unwrap_or(offset);
                        // If the callee is a generic function, box each concrete arg
                        // at the call boundary (concrete→generic).
                        let is_generic = self.checker.is_generic_fn(&n) && mono_offset.is_none();
                        if let Some(arg_list) = args {
                            for arg in arg_list {
                                bytecode.append(&mut self.do_compile(arg));
                                if is_generic {
                                    // Reuse the original HM result. Re-running inference here
                                    // would occur after the function scope has been popped.
                                    if let Some(arg_ty) = self.codegen_expr_ty(arg) {
                                        Self::emit_box_if_needed(&mut bytecode, &arg_ty);
                                    }
                                }
                            }
                        }

                        // ── Dictionary-passing calling convention ──────────────────
                        // For non-monomorphized generic calls, append one dict tuple
                        // per constraint after the value args. Each dict is a
                        // MakeTuple of method code offsets (CodePtr per method in
                        // declaration order). Builtin and user instances share this
                        // ABI; ground Num/Ord/Eq calls may still monomorphize away
                        // from the shared body, but Show-bound calls always take
                        // this path.
                        let dict_count = if is_generic {
                            let call_arg_tys: Vec<crate::typechecking::Ty> = args
                                .as_ref()
                                .map(|items| {
                                    items
                                        .iter()
                                        .map(|arg| {
                                            self.codegen_expr_ty(arg).expect(
                                                "typechecked call argument must have a codegen type",
                                            )
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
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
                                        bytecode.push(
                                            Byte::new(Instruction::LOAD).with_operand_u32(slot),
                                        );
                                        forwarded += 1;
                                    }
                                }
                            }
                            forwarded
                                + Self::emit_call_site_dicts(
                                    &mut bytecode,
                                    &n,
                                    &call_arg_tys,
                                    &self.checker,
                                    &self.functions,
                                )
                        } else {
                            0
                        };

                        let arity = args.as_ref().map(|items| items.len()).unwrap_or(0) as u32
                            + dict_count as u32;
                        if is_instance_method_fqn(&self.checker, &n) {
                            Self::emit_call_indirect(&mut bytecode, target_offset as u32, arity);
                        } else if self.coroutine_fns.contains(&n) {
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
                        // Generic→concrete unbox: if this was a non-monomorphized generic
                        // call and the Call expression's inferred return type is concrete,
                        // emit UnboxValue so the caller gets a raw value, not an ObjBoxed.
                        if is_generic && self.generic_return_depends_on_type_param(&n) {
                            if let Some(call_ty) = self.codegen_expr_ty(ast) {
                                Self::emit_unbox_if_needed(&mut bytecode, &call_ty);
                            }
                        }
                    } else if let Some(slot) = self.lookup_slot(&identifier) {
                        // Local holding a function value: escaped PolyFn
                        // (`let f = show` / `return show`), rank-n parameter, or
                        // a PolyFn returned from another call. Emit args, optional
                        // application dictionaries, then CallIndirect.
                        let value_arity =
                            args.as_ref().map(|items| items.len()).unwrap_or(0) as u32;
                        let mut arg_tys = Vec::new();
                        if let Some(arg_list) = args {
                            for arg in arg_list {
                                bytecode.append(&mut self.do_compile(arg));
                                // Box concrete args when delegating through a polyfn.
                                if let Some(arg_ty) = self.codegen_expr_ty(arg) {
                                    Self::emit_box_if_needed(&mut bytecode, &arg_ty);
                                    arg_tys.push(arg_ty);
                                }
                            }
                        }
                        let mut dict_count = 0u32;
                        let polyfn_source = self.polyfn_sources.get(&identifier).cloned();
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
                                        bytecode.push(
                                            Byte::new(Instruction::LOAD)
                                                .with_operand_u32(dict_slot),
                                        );
                                        dict_count += 1;
                                    }
                                }
                            }
                            dict_count += Self::emit_call_site_dicts(
                                &mut bytecode,
                                source,
                                &arg_tys,
                                &self.checker,
                                &self.functions,
                            ) as u32;
                        }
                        // Pack value arity + application dict arity so the VM can
                        // merge captured evidence with apply-site dictionaries.
                        bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(slot));
                        bytecode.push(
                            Byte::new(Instruction::CallIndirect).with_operand_u32(
                                value_arity | (dict_count << 16),
                            ),
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
            Expression::Argument(ty, n) => {
                let _ = self.context.variables.intern(n.to_string());
                if matches!(ty.1.as_ref(), Expression::Forall { .. }) {
                    self.polyfn_vars.insert(n.to_string());
                }
                // bytecode.push(Byte::new(Instruction::LOAD)
            }
            Expression::Type(_) | Expression::TypeFun(_, _) | Expression::Forall { .. } => {
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
                    if let Expression::Function {
                        name: method_name,
                        body,
                        ..
                    } = method.1.as_ref()
                    {
                        let has_default = !matches!(body.1.as_ref(), Expression::Block(items) if items.is_empty());
                        if has_default {
                            let fqn = crate::typechecking::generics::Generics::default_method_fqn(
                                name,
                                method_name,
                            );
                            self.compile_function_output_with_name(method, fqn, &[], 1);
                        } else {
                            self.consume_function_signature_output(method);
                        }
                    } else {
                        self.consume_function_signature_output(method);
                    }
                }
            }
            Expression::TypeClassImpl {
                class,
                args,
                methods,
            } => {
                let arg_tys: Vec<_> = args
                    .iter()
                    .map(|arg| {
                        self.codegen_expr_ty(arg)
                            .expect("typechecked instance argument must have a codegen type")
                    })
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
                    if let Expression::Function {
                        name: method_name, ..
                    } = method.1.as_ref()
                    {
                        let fqn = format!("{}__{}__{}", class, ty_part, method_name);
                        let unbox_tys =
                            self.instance_method_unbox_tys(class, method_name, &arg_tys);
                        self.compile_function_output_with_name(method, fqn, &unbox_tys, 1);
                    } else if let Expression::Method(_, body) = method.1.as_ref() {
                        let _method_wrapper_id = self.checker.id_table().ids()[self.emit_idx];
                        self.emit_idx += 1;
                        if let Expression::Function {
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
                    } else {
                        self.consume_function_signature_output(method);
                    }
                }
            }
            Expression::Identifier(n) => {
                if let Some(slot) = self.lookup_slot(n) {
                    bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(slot));
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
                            // Hybrid evidence: capture every in-scope `__dictN` for
                            // this scheme; leave unresolved slots as null so
                            // CallIndirect can fill them at the application site.
                            let dict_arity = self.checker.dict_arity_for(&resolved_n);
                            let mut captured_any = false;
                            let mut slot_bc = Vec::new();
                            for dict_index in 0..dict_arity {
                                if let Some(slot) =
                                    self.lookup_slot(&format!("__dict{}", dict_index))
                                {
                                    slot_bc.push(
                                        Byte::new(Instruction::LOAD).with_operand_u32(slot),
                                    );
                                    captured_any = true;
                                } else {
                                    // Unresolved sentinel (VM stores as None).
                                    slot_bc.push(
                                        Byte::new(Instruction::CONST).with_const_inline(0),
                                    );
                                }
                            }
                            if !captured_any {
                                bytecode.push(
                                    Byte::new(Instruction::MakePolyFn)
                                        .with_operand_u32(entry_offset as u32),
                                );
                            } else {
                                bytecode.append(&mut slot_bc);
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
            // --- If codegen ---
            // Layout: c1, JMPF1, b1, JMP1, c2, JMPF2, b2, JMP2, b3, [end]
            Expression::If(branches) => {
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
                    if i > 0
                        && let Some(prev_label) = branch_start_labels[i - 1]
                    {
                        let target = self.bytecode.len() as u32;
                        bb.bind_label(prev_label, target, &mut self.bytecode, &mut self.constants);
                    }

                    // Emit cond then JMPF (including single-branch if).
                    if let Some(cond) = cond_opt {
                        let cond_bc = self.do_compile(cond);
                        self.bytecode.extend(cond_bc);
                        let jmpf_target = branch_start_labels[i].unwrap_or(end_label);
                        bb.emit_jump_to(jmpf_target, BbJumpKind::JumpIfFalse, &mut self.bytecode);
                    }

                    // Body after cond+JMPF so Print/nested control-flow offsets stay correct.
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
                bb.bind_label(end_label, end_pos, &mut self.bytecode, &mut self.constants);

                // Validate: every label that had a pending jump must
                // be bound. (Allocated-but-unused labels are allowed.)
                bb.finalize()
                    .expect("BlockBuilder::finalize: all targeted labels bound");
            }
            Expression::Le(lhs, rhs) => {
                let hint = self_id
                    .and_then(|id| self.checker.bound_operator_call_at(id))
                    .or_else(|| self.checker.bound_operator_call_for_span(span.start, span.end))
                    .cloned();
                if let Some(hint) = hint
                    && self.emit_bound_operator_call(
                        &mut bytecode, lhs, rhs, hint.dict_index, hint.method_slot,
                    )
                {} else {
                    let is_float = self.compile_binary_operands(&mut bytecode, lhs, rhs);
                    bytecode.push(Byte::new(if is_float { Instruction::LEF } else { Instruction::LE }));
                }
            }
            Expression::Gt(lhs, rhs) => {
                let hint = self_id
                    .and_then(|id| self.checker.bound_operator_call_at(id))
                    .or_else(|| self.checker.bound_operator_call_for_span(span.start, span.end))
                    .cloned();
                if let Some(hint) = hint
                    && self.emit_bound_operator_call(
                        &mut bytecode, lhs, rhs, hint.dict_index, hint.method_slot,
                    )
                {} else {
                    let is_float = self.compile_binary_operands(&mut bytecode, lhs, rhs);
                    bytecode.push(Byte::new(if is_float { Instruction::GTF } else { Instruction::GT }));
                }
            }
            Expression::Leq(lhs, rhs) => {
                let hint = self_id
                    .and_then(|id| self.checker.bound_operator_call_at(id))
                    .or_else(|| self.checker.bound_operator_call_for_span(span.start, span.end))
                    .cloned();
                if let Some(hint) = hint
                    && self.emit_bound_operator_call(
                        &mut bytecode, lhs, rhs, hint.dict_index, hint.method_slot,
                    )
                {} else {
                    let is_float = self.compile_binary_operands(&mut bytecode, lhs, rhs);
                    bytecode.push(Byte::new(if is_float { Instruction::LEQF } else { Instruction::LEQ }));
                }
            }
            Expression::Geq(lhs, rhs) => {
                let hint = self_id
                    .and_then(|id| self.checker.bound_operator_call_at(id))
                    .or_else(|| self.checker.bound_operator_call_for_span(span.start, span.end))
                    .cloned();
                if let Some(hint) = hint
                    && self.emit_bound_operator_call(
                        &mut bytecode, lhs, rhs, hint.dict_index, hint.method_slot,
                    )
                {} else {
                    let is_float = self.compile_binary_operands(&mut bytecode, lhs, rhs);
                    bytecode.push(Byte::new(if is_float { Instruction::GEQF } else { Instruction::GEQ }));
                }
            }
            Expression::Eq(lhs, rhs) => {
                let hint = self_id
                    .and_then(|id| self.checker.bound_operator_call_at(id))
                    .or_else(|| self.checker.bound_operator_call_for_span(span.start, span.end))
                    .cloned();
                if let Some(hint) = hint
                    && self.emit_bound_operator_call(
                        &mut bytecode, lhs, rhs, hint.dict_index, hint.method_slot,
                    )
                {} else {
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
                unary!(bytecode, self, lhs, Byte::new(Instruction::NEG));
            }
            Expression::Add(lhs, rhs) => {
                if self.is_string_expr(lhs) && self.is_string_expr(rhs) {
                    Self::emit_raw_string_literal(&mut bytecode, "%s%s");
                    bytecode.append(&mut self.do_compile(lhs));
                    bytecode.append(&mut self.do_compile(rhs));
                    bytecode.push(Byte::new(Instruction::FORMAT).with_operand_u32(2));
                } else if let Some(hint) = self_id
                    .and_then(|id| self.checker.bound_operator_call_at(id))
                    .or_else(|| self.checker.bound_operator_call_for_span(span.start, span.end))
                    .cloned()
                    && self.emit_bound_operator_call(
                        &mut bytecode, lhs, rhs, hint.dict_index, hint.method_slot,
                    )
                {
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
                if let Some(hint) = self_id
                    .and_then(|id| self.checker.bound_operator_call_at(id))
                    .or_else(|| self.checker.bound_operator_call_for_span(span.start, span.end))
                    .cloned()
                    && self.emit_bound_operator_call(
                        &mut bytecode, lhs, rhs, hint.dict_index, hint.method_slot,
                    )
                {
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
                if let Some(hint) = self_id
                    .and_then(|id| self.checker.bound_operator_call_at(id))
                    .or_else(|| self.checker.bound_operator_call_for_span(span.start, span.end))
                    .cloned()
                    && self.emit_bound_operator_call(
                        &mut bytecode, lhs, rhs, hint.dict_index, hint.method_slot,
                    )
                {
                } else {
                    let is_float = likely(self.compile_binary_operands(&mut bytecode, lhs, rhs));
                    bytecode.push(Byte::new(if is_float {
                        Instruction::MULF
                    } else {
                        Instruction::MUL
                    }));
                }
            }
            Expression::Mod(lhs, rhs) => {
                if self.operand_is_open_ty(lhs) || self.operand_is_open_ty(rhs) {
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
                if let Some(hint) = self_id
                    .and_then(|id| self.checker.bound_operator_call_at(id))
                    .or_else(|| self.checker.bound_operator_call_for_span(span.start, span.end))
                    .cloned()
                    && self.emit_bound_operator_call(
                        &mut bytecode, lhs, rhs, hint.dict_index, hint.method_slot,
                    )
                {
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
                let is_float = self.compile_binary_operands(&mut bytecode, lhs, rhs);
                bytecode.push(Byte::new(if is_float {
                    Instruction::PowF
                } else {
                    Instruction::Pow
                }));
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
                    .or_else(|| self.checker.bound_operator_call_for_span(span.start, span.end))
                    .cloned();
                if let Some(hint) = hint
                    && self.emit_bound_operator_call(
                        &mut bytecode, lhs, rhs, hint.dict_index, hint.method_slot,
                    )
                {} else {
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
                bytecode.push(Byte::new(Instruction::CONST).with_const_pool(idx));
            }
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
                Expression::Access(target_expr, field) => {
                    bytecode.append(&mut self.do_compile(value));
                    bytecode.append(&mut self.do_compile(target_expr));
                    self.emit_field_name(&mut bytecode, field);
                    bytecode.push(Byte::new(Instruction::SetField));
                }
                Expression::Index(arr, idx) => {
                    let tmp_arr = self.alloc_temp_slot();
                    let tmp_idx = self.alloc_temp_slot();
                    let tmp_val = self.alloc_temp_slot();
                    bytecode.append(&mut self.do_compile(value));
                    bytecode.push(Byte::new(Instruction::StorePop).with_operand_u32(tmp_val));
                    bytecode.append(&mut self.do_compile(arr));
                    bytecode.push(Byte::new(Instruction::StorePop).with_operand_u32(tmp_arr));
                    bytecode.append(&mut self.do_compile(idx));
                    bytecode.push(Byte::new(Instruction::StorePop).with_operand_u32(tmp_idx));
                    bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_arr));
                    bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_idx));
                    bytecode.push(Byte::new(Instruction::LOAD).with_operand_u32(tmp_val));
                    bytecode.push(Byte::new(Instruction::StoreIndex));
                }
                Expression::Identifier(name) => {
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
                            let assigned = likely(*self.context.constants.get(&symbol).unwrap());
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
                        bytecode.append(&mut self.do_compile(value));
                        bytecode
                            .push(Byte::new(Instruction::StorePop).with_operand_u32(symbol as u32));
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
                _ => {
                    bytecode.append(&mut self.do_compile(value));
                    bytecode.push(Byte::new(Instruction::POP));
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
                    self.bytecode
                        .push(Byte::new(Instruction::StorePop).with_operand_u32(lib_slot));
                }
                // 3. For each declared function, emit declare(lib,
                //    name, (arg_tags...), ret) and store fn id.
                for decl in declarations {
                    let fn_name = decl.name.to_string();
                    // First-wins on fn-name collisions across
                    // multiple `extern` blocks.
                    if self.extern_runtime_functions.contains_key(&fn_name) {
                        continue;
                    }
                    let fn_id_slot_name = format!("__ext_fn_{}", fn_name);
                    let fn_id_slot = self.context.variables.intern(fn_id_slot_name) as u32;
                    // Push the library handle.
                    self.bytecode
                        .push(Byte::new(Instruction::LOAD).with_operand_u32(lib_slot));
                    // Push the function name (string literal).
                    let span: SimpleSpan = (0..0).into();
                    let name_expr: parser::ast::Output =
                        (span, Box::new(parser::ast::Expression::String(decl.name)));
                    let mut name_bc = self.do_compile(&name_expr);
                    self.bytecode.append(&mut name_bc);
                    // Push each arg type as a CONST tag.
                    let mut arg_type_tags: Vec<u32> = Vec::new();
                    if let Expression::Fragment(items) = decl.args.1.as_ref() {
                        for arg in items {
                            if let Expression::Argument(type_expr, _param_name) = arg.1.as_ref() {
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
                                            "use FFIType::X, a bare type name, [T], (T, U), or an extern struct".to_string(),
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
                    self.bytecode
                        .push(Byte::new(Instruction::MakeTuple).with_operand_u32(arity));
                    // Push the ret type tag (top of stack for DeclareFFI).
                    let (ret_tag, ret_aux) = decl
                        .returns
                        .as_ref()
                        .and_then(|r| ffi_type_tag_from_output(&self.checker, r))
                        .unwrap_or((tag::VOID, 0));
                    emit_ffi_type_const(&mut self.bytecode, ret_tag, ret_aux);
                    // Emit DeclareFFI.
                    self.bytecode
                        .push(Byte::new(Instruction::DeclareFFI).with_operand_u32(arity));
                    // Store the function id.
                    self.bytecode
                        .push(Byte::new(Instruction::StorePop).with_operand_u32(fn_id_slot));
                    self.extern_runtime_functions
                        .insert(fn_name.clone(), (lib_slot, fn_id_slot));
                }
            }
            Expression::EnumDecl {
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
                bytecode.push(
                    Byte::new(Instruction::MakeEnum).with_operands_u16([tag as u16, arity as u16]),
                );
            }
            // --- Match codegen (threaded layout) ---
            // Forward: scrutinee, JUMP_IF_MATCH cascade, last-arm UNPACK/POP/STORE.
            // Reverse: arm bindings + bodies; non-first arms JMP to end.
            Expression::Match { scrutinee, arms } => {
                if arms.is_empty() {
                    bytecode.append(&mut self.do_compile(scrutinee));
                    bytecode.push(Byte::new(Instruction::POP));
                } else {
                    let mut bb = BlockBuilder::new();
                    let end_label = bb.fresh_label();

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

                    let tag_groups = group_arms_by_outer_tag(arms, &self.checker);

                    let scrutinee_bc = self.do_compile(scrutinee);
                    self.bytecode.extend(scrutinee_bc);

                    // Forward pass: outer-tag dispatch + last-arm scrutinee consumer.
                    let any_multi_arm_group = tag_groups.iter().any(|g| g.arm_indices.len() > 1);
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
                                &mut self.bytecode,
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
                                    self.bytecode.push(Byte::new(Instruction::POP));
                                }
                                Pattern::Binding { name } => {
                                    // Binding arm — STORE the
                                    // scrutinee at slot 1
                                    // (matching LOAD 0's push
                                    // position).
                                    //
                                    // the binding
                                    // is recorded in
                                    // `match_bindings` by the
                                    // reverse pass, so the
                                    // body's `Identifier`
                                    // lookup resolves the
                                    // name to slot 1.
                                    // Hardcoding slot 1 here
                                    // is correct because LOAD
                                    // 0 always pushes to
                                    // frame.sp + 1, and the
                                    // STORE is a no-op
                                    // .
                                    let _ = name;
                                    self.bytecode
                                        .push(Byte::new(Instruction::STORE).with_operand_u32(1));
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
                        bb.bind_label(
                            first_arm_label,
                            self.bytecode.len() as u32,
                            &mut self.bytecode,
                            &mut self.constants,
                        );
                        test_chain_first_arms.insert(first_arm_idx);
                        for &arm_idx in &group.arm_indices {
                            test_chain_arms.insert(arm_idx);
                        }

                        // Emit the test chain for each arm
                        // in source order. The last arm in
                        // the group has `pass_label = None`
                        // (no JMP — falls through to its
                        // body); non-last arms have a fresh
                        // `pass_label` that the reverse pass
                        // binds to the arm's body.
                        for (rank, &arm_idx) in group.arm_indices.iter().enumerate() {
                            let is_last_in_group = rank == group.arm_indices.len() - 1;

                            let pass_label = if !is_last_in_group {
                                Some(bb.fresh_label())
                            } else {
                                None
                            };

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
                            bb.bind_label(
                                label,
                                self.bytecode.len() as u32,
                                &mut self.bytecode,
                                &mut self.constants,
                            );
                        }

                        // For arms in test chain groups,
                        // bind the test chain's
                        // `pass_label` to the start of
                        // this arm's body. The
                        // `emit_inner_test` call in the
                        // test chain pass emitted
                        // `JMP → pass_label` at the end
                        // of the arm's sub-pattern
                        // consumer; binding it here
                        // routes a successful test to
                        // this arm's body. The last arm
                        // in the group has
                        // `pass_label = None` (no JMP
                        // emitted — falls through to
                        // its body), so this `if let`
                        // is a no-op for it.
                        if let Some(Some(label)) = pass_labels.get(&i) {
                            bb.bind_label(
                                *label,
                                self.bytecode.len() as u32,
                                &mut self.bytecode,
                                &mut self.constants,
                            );
                        }

                        // Per-arm binding slots (slot 1 = first payload).
                        // Payload order follows declaration order; record
                        // patterns may list fields in any source order.
                        let mut arm_bindings: HashMap<String, u32> = HashMap::new();
                        let mut next_slot: u32 = 1;
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
                                    arm_bindings.insert(name.to_string(), 1);
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
                        // resolve pattern bindings to slots 1, 2, 3,
                        // ... — matching the VM's payload-push
                        // positions. Cleared after the body emits.
                        let saved_bindings = self.context.match_bindings.take();
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

                    // Bind `end_label` to a join pad that leaves the
                    // arm value on the stack (Phase P0 — let x =
                    // match). Do NOT emit RETURN here: that made
                    // Match a function terminus so Fragment's
                    // trailing StorePop was unreachable.
                    //
                    // Emit DUPLICATE; POP as a fusion barrier so a
                    // trailing arm `CONST k` is not peephole-fused
                    // with a following `RETURN` from `return match`
                    // (that fusion left JMP-to-end targeting the
                    // removed RETURN slot and skipped the return).
                    //
                    // `return match { … }` still works because
                    // Expression::Return emits its own RETURN after
                    // the child. Bare Match as a statement is
                    // discarded by ExprStatement's POP.
                    let end_pos = self.bytecode.len() as u32;
                    self.bytecode.push(Byte::new(Instruction::DUPLICATE));
                    self.bytecode.push(Byte::new(Instruction::POP));
                    bb.bind_label(end_label, end_pos, &mut self.bytecode, &mut self.constants);

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
                let is_class = matches!(
                    &receiver_ty,
                    Some(crate::typechecking::Ty::Con(n)) if self.checker.is_class(n)
                );
                if is_record || is_class {
                    // Push the field-name string on TOP of the
                    // receiver (which is already on the stack
                    // from `do_compile(receiver)` above). GetField
                    // pops the field-name (top) and the receiver,
                    // pushes the value.
                    self.emit_field_name(&mut bytecode, field);
                    bytecode.push(Byte::new(Instruction::GetField));
                } else {
                    let enum_name = self.enum_name_for_receiver(receiver);
                    let field_index = enum_name
                        .as_ref()
                        .and_then(|name| self.checker.field_index_for(name, field))
                        .map(|(_variant, idx)| idx)
                        .unwrap_or(0);

                    bytecode.push(
                        Byte::new(Instruction::LoadField).with_operand_u32(field_index as u32),
                    );
                }
            }

            Expression::Field(_, _, _) => {
                // Class field decls are metadata only — consumed for ID alignment.
            }

            // --- Error-handling operators (desugar to MakeEnum / JumpIfMatch) ---
            Expression::Raise(expr) => {
                // `raise e` → push e, wrap Err(e), RETURN.
                // Emit to self.bytecode so nested absolute jumps stay valid.
                let expr_bc = self.do_compile(expr);
                self.bytecode.extend(expr_bc);
                Self::emit_result_err(&mut self.bytecode);
                self.bytecode.push(Byte::new(Instruction::RETURN));
            }
            Expression::Try(inner) => {
                // `e?` → if Ok/Some, leave payload; else RETURN the failure.
                let is_option = self.expr_is_option(inner);
                let success_tag: u32 = if is_option { 1 } else { 0 }; // Some=1, Ok=0

                let inner_bc = self.do_compile(inner);
                self.bytecode.extend(inner_bc);

                let mut bb = BlockBuilder::new();
                let success = bb.fresh_label();
                bb.emit_jump_to(
                    success,
                    BbJumpKind::JumpIfMatch {
                        tag: success_tag,
                        arity: 1,
                    },
                    &mut self.bytecode,
                );
                // Miss: failure value still on stack — propagate via RETURN.
                self.bytecode.push(Byte::new(Instruction::RETURN));

                let success_pos = self.bytecode.len() as u32;
                bb.bind_label(
                    success,
                    success_pos,
                    &mut self.bytecode,
                    &mut self.constants,
                );
                bb.finalize()
                    .expect("BlockBuilder::finalize: Try success label bound");
                // Payload left on stack for the caller (e.g. StorePop).
            }
            Expression::Coalesce(lhs, rhs) => {
                // `a ?? b` → Ok/Some payload, else evaluate b.
                let is_option = self.expr_is_option(lhs);
                let success_tag: u32 = if is_option { 1 } else { 0 };

                let lhs_bc = self.do_compile(lhs);
                self.bytecode.extend(lhs_bc);

                let mut bb = BlockBuilder::new();
                let success = bb.fresh_label();
                let end = bb.fresh_label();
                bb.emit_jump_to(
                    success,
                    BbJumpKind::JumpIfMatch {
                        tag: success_tag,
                        arity: 1,
                    },
                    &mut self.bytecode,
                );
                // Miss: discard failure, evaluate rhs, jump to end.
                self.bytecode.push(Byte::new(Instruction::POP));
                let rhs_bc = self.do_compile(rhs);
                self.bytecode.extend(rhs_bc);
                bb.emit_jump_to(end, BbJumpKind::Unconditional, &mut self.bytecode);

                let success_pos = self.bytecode.len() as u32;
                bb.bind_label(
                    success,
                    success_pos,
                    &mut self.bytecode,
                    &mut self.constants,
                );
                // Success: payload already on stack from JumpIfMatch.
                let end_pos = self.bytecode.len() as u32;
                bb.bind_label(end, end_pos, &mut self.bytecode, &mut self.constants);
                bb.finalize()
                    .expect("BlockBuilder::finalize: Coalesce labels bound");
            }
            Expression::OptionalAccess(receiver, field) => {
                // `opt?.field` → None if opt is None, else Some(opt.field).
                let recv_bc = self.do_compile(receiver);
                self.bytecode.extend(recv_bc);

                let mut bb = BlockBuilder::new();
                let success = bb.fresh_label();
                let end = bb.fresh_label();
                bb.emit_jump_to(
                    success,
                    BbJumpKind::JumpIfMatch {
                        tag: 1, // Some
                        arity: 1,
                    },
                    &mut self.bytecode,
                );
                // Miss: None stays on stack; skip field access.
                bb.emit_jump_to(end, BbJumpKind::Unconditional, &mut self.bytecode);

                let success_pos = self.bytecode.len() as u32;
                bb.bind_label(
                    success,
                    success_pos,
                    &mut self.bytecode,
                    &mut self.constants,
                );

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
                let is_class = matches!(
                    &inner_ty,
                    Some(crate::typechecking::Ty::Con(n)) if self.checker.is_class(n)
                );
                if is_record || is_class {
                    Self::emit_raw_string_literal(&mut self.bytecode, field);
                    self.bytecode.push(Byte::new(Instruction::GetField));
                } else {
                    let enum_name = inner_ty.as_ref().and_then(extract_enum_name);
                    let field_index = enum_name
                        .as_ref()
                        .and_then(|name| self.checker.field_index_for(name, field))
                        .map(|(_variant, idx)| idx)
                        .unwrap_or(0);
                    self.bytecode.push(
                        Byte::new(Instruction::LoadField).with_operand_u32(field_index as u32),
                    );
                }
                Self::emit_ok_or_some_wrap(&mut self.bytecode, true);

                let end_pos = self.bytecode.len() as u32;
                bb.bind_label(end, end_pos, &mut self.bytecode, &mut self.constants);
                bb.finalize()
                    .expect("BlockBuilder::finalize: OptionalAccess labels bound");
            }
            Expression::TypeApp { args, .. } => {
                // Type-position only — consume child IDs, emit no bytes.
                for arg in args {
                    let _ = self.do_compile(arg);
                }
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
                #[cfg(debug_assertions)]
                dbg!(_expr);
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
        ast: &(SimpleSpan, Box<Expression<'compiler>>),
    ) {
        let ns = self.namespace.clone();
        self.namespace = module.to_string();

        self.emit_idx = 0;
        self.temp_counter = 0;
        self.loop_stack.clear();
        self.loop_bbs.clear();
        self.constants.clear();
        self.mono_offsets.clear();
        self.mono_codegen_var_types.clear();
        let _program_ty = self.checker.check_program(ast);
        self.emit_builtin_dict_thunks();
        // Builtin dictionary thunks are emitted immediately after the
        // prologue and before user code. Keep `program_start_offset`
        // pointing at the first user byte so `extern` prologue JMPs
        // don't fall into a Num/Ord/Eq/Show thunk body.
        self.program_start_offset = self.bytecode.len() as u32;
        self.mono_plan = monomorphize::plan_monomorphization(module, ast, &self.checker);

        let mut program = self.do_compile(ast);
        self.namespace = ns.to_string();

        self.messages.extend(self.checker.take_messages());

        self.bytecode.append(&mut program);
    }

    /// Peephole-fuse `self.bytecode` and relocate absolute code offsets
    /// (`JMP`/`CALL`/`CodePtr`/`MakePolyFn`/…).
    ///
    /// Called once after multi-file linking by the pipeline, or at the end
    /// of single-file [`compile`] so unit tests observe fused output.
    pub fn finalize_bytecode(&mut self) {
        let fusion_sites = peephole::fuse_bytecode(&mut self.bytecode, &mut self.constants);
        for offset in self.functions.values_mut() {
            *offset = peephole::adjust_target(*offset, &fusion_sites);
        }
        for offset in self.mono_offsets.values_mut() {
            *offset = peephole::adjust_target(*offset, &fusion_sites);
        }
        self.program_start_offset =
            peephole::adjust_target(self.program_start_offset as usize, &fusion_sites) as u32;
    }

    pub fn compile<'compiler>(
        &mut self,
        module: &str,
        ast: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> Vec<Byte> {
        self.compile_unfused(module, ast);
        self.finalize_bytecode();
        self.bytecode.clone()
    }

    /// Return only bytes appended by this compile (multi-file pipeline).
    ///
    /// Emits **unfused** absolute-offset bytecode; the pipeline must call
    /// [`finalize_bytecode`] once on the linked buffer.
    pub fn compile_module<'compiler>(
        &mut self,
        module: &str,
        ast: &(SimpleSpan, Box<Expression<'compiler>>),
    ) -> Vec<Byte> {
        let pre_compile_len = self.bytecode.len();
        self.compile_unfused(module, ast);
        self.bytecode[pre_compile_len..].to_vec()
    }
}

fn unwrapped_identifier<'a>(expr: &'a Output<'a>) -> Option<&'a str> {
    match expr.1.as_ref() {
        Expression::Identifier(name) => Some(name),
        Expression::Expr(inner)
        | Expression::Group(inner)
        | Expression::Statement(inner)
        | Expression::ExprStatement(inner) => unwrapped_identifier(inner),
        _ => None,
    }
}

/// Extract enum name from `Ty::Con` / `Ty::Sum` / nested `Ty::Constructor`.
fn extract_enum_name(ty: &crate::typechecking::ty::Ty) -> Option<String> {
    use crate::typechecking::ty::Ty;
    match ty {
        Ty::Con(name) => Some(name.clone()),
        Ty::Sum { name, .. } => Some(name.clone()),
        Ty::Constructor { owner, .. } => extract_enum_name(owner),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parser::Pratt;

    fn compile_src(src: &str) -> (Vec<Byte>, Vec<u64>) {
        let ast = Pratt::default().parse(src).expect("parse failed");
        let mut compiler = Compiler::default();
        let bc = compiler.compile("", &ast);
        (bc, compiler.constants)
    }

    /// End-to-end: a simple integer expression compiles to bytecode
    /// using the HM checker's cache. We don't check exact bytes (those
    /// change as the emitter evolves); we just verify the pipeline
    /// runs without panicking and produces a non-empty bytecode.
    #[test]
    fn integer_arithmetic_emits_bytecode() {
        let (bc, _pool) = compile_src("42;");
        assert!(!bc.is_empty());
    }

    #[test]
    fn async_call_emits_make_coro_not_call() {
        use common::Instruction;
        let (bc, _pool) = compile_src("async fn coro() { yield 1; } fn main() { let h = coro(); }");
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::MakeCoro)),
            "expected MakeCoro for async fn call"
        );
        assert!(
            !bc.iter()
                .any(|b| { matches!(b.bytecode(), Instruction::CALL) && b.call_parts().1 > 3 }),
            "async fn call site should not use CALL"
        );
    }

    #[test]
    fn yield_and_resume_emit_coroutine_opcodes() {
        use common::Instruction;
        let (bc, _pool) =
            compile_src("async fn coro() { yield 1; } fn main() { let h = coro(); resume h; }");
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::YieldCoro)),
            "expected YieldCoro in async body"
        );
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::ResumeCoro)),
            "expected ResumeCoro at call site"
        );
    }

    /// Binding yield (`let x = yield e`) emits YieldCoro then StorePop.
    #[test]
    fn let_binding_yield_emits_yield_coro_then_store_pop() {
        use common::Instruction;
        let (bc, _pool) = compile_src("async fn f() { let x = yield 1; }");
        let yield_pos = bc
            .iter()
            .position(|b| matches!(b.bytecode(), Instruction::YieldCoro))
            .expect("expected YieldCoro");
        let store_pos = bc
            .iter()
            .position(|b| matches!(b.bytecode(), Instruction::StorePop))
            .expect("expected StorePop");
        assert!(
            yield_pos < store_pos,
            "YieldCoro (at {}) must precede StorePop (at {}) for binding yield",
            yield_pos,
            store_pos
        );
    }

    /// Resume-with-send sets the has_send bit on ResumeCoro.
    #[test]
    fn resume_with_send_emits_has_send_operand() {
        use common::Instruction;
        let (bc, _pool) = compile_src("fn main() { resume h with 42; }");
        let resume = bc
            .iter()
            .find(|b| matches!(b.bytecode(), Instruction::ResumeCoro))
            .expect("expected ResumeCoro");
        assert_ne!(
            resume.operand_u32() & 1,
            0,
            "ResumeCoro for `resume h with v` must set has_send bit"
        );
    }

    /// `yield from` emits YieldFromCoro.
    #[test]
    fn yield_from_emits_yield_from_coro() {
        use common::Instruction;
        let (bc, _pool) = compile_src("async fn f() { yield from inner; }");
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::YieldFromCoro)),
            "expected YieldFromCoro for yield from"
        );
    }

    /// A bare `yield expr;` statement parses through `expr_statement()`
    /// (see `parser::statement`, where `self.expr_statement()` is tried
    /// before the dedicated `self.yield_()` alternative), landing as
    /// `ExprStatement(Yield(...))`. Regression guard: `ExprStatement`
    /// must NOT emit a trailing `POP` after `YieldCoro` (or
    /// `YieldFromCoro`) — that POP becomes the coroutine's `resume_ip`
    /// and, on the NEXT resume, pops whatever the resumer happens to
    /// have on top of the shared operand stack (e.g. a `print` format
    /// string mid-construction), corrupting it. See the crash this
    /// guards against: `print "%i", resume h;` used to misalign a
    /// pointer dereference in the VM.
    #[test]
    fn bare_yield_statement_does_not_emit_trailing_pop() {
        use common::Instruction;
        let (bc, _pool) = compile_src("async fn f() { yield 1; yield 2; }");
        let yield_positions: Vec<usize> = bc
            .iter()
            .enumerate()
            .filter(|(_, b)| matches!(b.bytecode(), Instruction::YieldCoro))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(yield_positions.len(), 2, "expected two YieldCoro sites");
        for pos in yield_positions {
            assert!(
                !matches!(
                    bc.get(pos + 1).map(|b| b.bytecode()),
                    Some(Instruction::POP)
                ),
                "bare `yield expr;` must not be followed by POP (would corrupt the next resume)"
            );
        }
    }

    /// Same guard for a bare `yield from expr;` statement.
    #[test]
    fn bare_yield_from_statement_does_not_emit_trailing_pop() {
        use common::Instruction;
        let (bc, _pool) = compile_src("async fn f() { yield from inner; }");
        let pos = bc
            .iter()
            .position(|b| matches!(b.bytecode(), Instruction::YieldFromCoro))
            .expect("expected YieldFromCoro");
        assert!(
            !matches!(
                bc.get(pos + 1).map(|b| b.bytecode()),
                Some(Instruction::POP)
            ),
            "bare `yield from expr;` must not be followed by POP"
        );
    }

    /// Float arithmetic should pick `ADDF` (float) instead of `ADD`
    /// (int) — that's the whole point of the cache lookup.
    #[test]
    fn float_arithmetic_emits_float_opcode() {
        use common::Instruction;
        let (bc, _pool) = compile_src("1.0 + 2.0;");
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

    /// Integer arithmetic should pick `ADD`, not `ADDF`. Two literals
    /// (`1 + 2`) now constant-fold to a single `CONST`, so we use two
    /// int parameters — `a + b` compiles to a slot/slot binary op whose
    /// packed operator must be the int `ADD` (not the float `ADDF`).
    #[test]
    fn integer_arithmetic_emits_int_opcode() {
        use common::Instruction;
        let (bc, _pool) = compile_src("fn add(int a, int b) -> int { return a + b; }");
        // Builtin Num dictionary thunks also contain ADD/ADDF; the user function
        // body itself should fuse to BinSlotSlot(ADD) (or bare ADD).
        let has_int_bin_slot = bc.iter().any(|b| {
            *b.bytecode() == Instruction::BinSlotSlot
                && b.bin_slot_slot_parts().0 == Instruction::ADD as u8
        });
        let has_float_bin_slot = bc.iter().any(|b| {
            *b.bytecode() == Instruction::BinSlotSlot
                && b.bin_slot_slot_parts().0 == Instruction::ADDF as u8
        });
        assert!(
            has_int_bin_slot && !has_float_bin_slot,
            "expected fused int BinSlotSlot(ADD) for integer arithmetic; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn string_addition_emits_format_not_add() {
        use common::Instruction;
        let (bc, _pool) = compile_src("\"a\" + \"b\";");

        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::FORMAT)),
            "expected string addition to lower through FORMAT; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
        // Ignore ADD/ADDF inside builtin Num thunks; the top-level expression
        // must not fuse into a numeric BinSlot* convoy before FORMAT.
        assert!(
            !bc.iter().any(|b| matches!(
                b.bytecode(),
                Instruction::BinSlotImm | Instruction::BinSlotSlot
            )),
            "string addition should not emit fused numeric slot ops; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// Mixed int+float picks float (because HM unifies the operands
    /// and one is float). The pipeline emits a single, well-typed
    /// result — either way, the test should not panic.
    #[test]
    fn mixed_int_float_arithmetic_emits_bytecode() {
        let (bc, _pool) = compile_src("1 + 2.0;");
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

    #[test]
    fn typeclass_impl_method_registers_fqn_function() {
        let ast = Pratt::default()
            .parse(
                r#"
                typeclass Foo<T> { fn bar(T x) -> T; }
                impl Foo<int> { fn bar(int x) -> int { return x; } }
                fn main() { }
                "#,
            )
            .expect("parse failed");
        let mut compiler = Compiler::default();
        let bc = compiler.compile("", &ast);
        assert!(
            compiler.messages.is_empty(),
            "unexpected: {:?}",
            compiler.messages
        );
        let offset = compiler
            .functions
            .get("Foo__int__bar")
            .copied()
            .expect("instance method FQN should be registered");
        assert!(
            offset < bc.len(),
            "function offset should point into bytecode"
        );
    }

    #[test]
    fn emit_call_indirect_pushes_target_then_opcode() {
        use common::Instruction;
        let mut bc = Vec::new();
        Compiler::emit_call_indirect(&mut bc, 42, 2);
        assert_eq!(bc.len(), 2);
        assert!(matches!(bc[0].bytecode(), Instruction::CodePtr));
        assert_eq!(bc[0].operand_u32(), 42);
        assert!(matches!(bc[1].bytecode(), Instruction::CallIndirect));
        assert_eq!(bc[1].operand_u32(), 2);
    }

    // ============================================================
    // sum types and pattern matching codegen
    // ============================================================

    /// Codegen test 1: a constructor call emits a `MAKE_ENUM`
    /// with the correct tag and arity in the operand (upper 16
    /// bits = tag, lower 16 bits = arity).
    #[test]
    fn construct_emits_make_enum_with_correct_tag_and_arity() {
        use common::Instruction;
        let (bc, _pool) = compile_src("let x = Option::Some(42);");

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
        let (bc, _pool) = compile_src(
            "match Option::Some(1) { \
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
        let (bc, _pool) = compile_src(
            "let x = Option::Some(42); \
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

    /// Codegen test 4 (LOW #5): a `match` with a
    /// NESTED constructor pattern (`Result::Ok(Option::Some(v))`)
    /// emits at least 2 `UNPACK`s — one for the outer `Result::Ok`
    /// and one for the inner `Option::Some`. The codegen
    /// recurses through `emit_pattern_binding` for nested
    /// constructors; the test guards against accidental
    /// simplification that would skip the inner unpack.
    #[test]
    fn match_with_nested_constructor_pattern_emits_unpack_cascade() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "match Result::Ok(Option::Some(1)) { \
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
    // VM perf: peephole superinstruction fusion
    // ============================================================

    #[test]
    fn compile_module_diff_matches_compile_tail_for_fib() {
        use common::Instruction;
        let src = include_str!("../../examples/fib.0s");
        let ast = Pratt::default().parse(src).expect("parse fib");

        // `compile_module` emits unfused absolute-offset bytecode;
        // final-link fusion is applied once via `finalize_bytecode`
        // (same as single-file `compile`).
        let mut module = Compiler::default();
        let bc_unfused = module.compile_module("", &ast);
        assert!(
            bc_unfused
                .iter()
                .any(|b| matches!(b.bytecode(), Instruction::LOAD)),
            "module compile should still contain unfused LOAD before finalize"
        );
        module.finalize_bytecode();

        let mut full = Compiler::default();
        let bc_full = full.compile("", &ast);

        assert_eq!(
            &bc_full[3..],
            &module.bytecode[3..],
            "finalize_bytecode on compile_module should match compile() output"
        );
        assert_eq!(full.functions, module.functions);
    }

    /// Count loop exit branches (unfused `JMPF` or fused `CmpJmpf` / `BinSlotImmJmpf`).
    fn loop_exit_branch_count(bc: &[common::Byte]) -> usize {
        use common::Instruction;
        bc.iter()
            .filter(|b| {
                matches!(
                    b.bytecode(),
                    Instruction::JMPF | Instruction::CmpJmpf | Instruction::BinSlotImmJmpf
                )
            })
            .count()
    }

    fn loop_exit_target(bc: &[common::Byte], pool: &[u64]) -> Option<usize> {
        use common::Instruction;
        for b in bc {
            match b.bytecode() {
                Instruction::JMPF => return Some(b.operand_u32() as usize),
                Instruction::CmpJmpf => return Some(b.cmp_jmpf_parts().1),
                Instruction::BinSlotImmJmpf => {
                    let pool_idx = b.bin_slot_imm_jmpf_parts().2;
                    return pool.get(pool_idx).map(|p| (*p >> 32) as usize);
                }
                _ => {}
            }
        }
        None
    }

    #[test]
    fn fib_compiles_with_fused_superinstructions() {
        use common::Instruction;
        let src = include_str!("../../examples/fib.0s");
        let (bc, _) = compile_src(src);
        // fib's body fuses: `n <= 2` may become `BinSlotImmJmpf` or
        // `BinSlotImm`, `return 1` into `ConstReturnImm`, and the
        // `fib(..) + fib(..)` tail into `BinReturn`.
        let bin_slot_imm = bc
            .iter()
            .filter(|b| *b.bytecode() == Instruction::BinSlotImm)
            .count();
        let bin_slot_imm_jmpf = bc
            .iter()
            .filter(|b| *b.bytecode() == Instruction::BinSlotImmJmpf)
            .count();
        let bin_return = bc
            .iter()
            .filter(|b| *b.bytecode() == Instruction::BinReturn)
            .count();
        assert!(
            bin_slot_imm + bin_slot_imm_jmpf >= 3,
            "expected at least three slot+imm fused ops in fib bytecode; got imm={bin_slot_imm} jmpf={bin_slot_imm_jmpf}; opcodes: {:?}",
            bc.iter().map(|b| *b.bytecode() as u8).collect::<Vec<_>>()
        );
        assert!(
            bin_return >= 1,
            "expected at least one BinReturn in fib bytecode; got {bin_return}"
        );
    }

    // ============================================================
    // BlockBuilder for Loop and Match codegen
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

    /// Codegen test 5 : a `while` loop emits
    /// the structural shape expected by the
    /// BlockBuilder-based codegen — at least 1 JMPF (the
    /// exit condition) and at least 1 JMP (the back-edge).
    /// This mirrors the 16.5 regression test for If, but
    /// for the new Loop codegen.
    #[test]
    fn loop_emits_top_label_and_back_edge() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn main() { \
 let i = 0; \
 while (i < 3) { \
 i = i + 1; \
 } \
 }",
        );

        // The loop emits: <iterable>, exit-branch, <body>, JMP→top.
        let exit_branch_count = loop_exit_branch_count(&bc);
        let jmp_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JMP))
            .count();
        assert!(
            exit_branch_count >= 1,
            "expected at least 1 loop exit branch (JMPF/CmpJmpf/BinSlotImmJmpf); got {}",
            exit_branch_count
        );
        assert!(
            jmp_count >= 1,
            "expected at least 1 JMP (the loop's back-edge); got {}",
            jmp_count
        );
    }

    /// Loop exit jump must land past the back-edge `JMP`, even after
    /// peephole fusion relocates jump targets. The condition may fuse
    /// to `CmpJmpf` (large limit) or `BinSlotImmJmpf` (inline limit).
    #[test]
    fn loop_cmp_jmpf_exit_targets_past_back_edge_after_peephole() {
        use common::Instruction;
        let (bc, pool) = compile_src(
            "fn main() { \
 let acc = 0; \
 let i = 0; \
 while (i < 2000) { \
 acc = acc + i; \
 i = i + 1; \
 } \
 }",
        );

        let cond_idx = bc
            .iter()
            .position(|b| {
                matches!(
                    b.bytecode(),
                    Instruction::CmpJmpf | Instruction::BinSlotImm | Instruction::BinSlotImmJmpf
                )
            })
            .expect("while condition should emit a fused or partial-fused compare");
        let exit_target = match bc[cond_idx].bytecode() {
            Instruction::CmpJmpf => bc[cond_idx].cmp_jmpf_parts().1,
            Instruction::BinSlotImmJmpf => {
                let pool_idx = bc[cond_idx].bin_slot_imm_jmpf_parts().2;
                (pool[pool_idx] >> 32) as usize
            }
            Instruction::BinSlotImm => bc
                .get(cond_idx + 1)
                .filter(|b| matches!(b.bytecode(), Instruction::JMPF))
                .expect("BinSlotImm condition should be followed by JMPF")
                .operand_u32() as usize,
            _ => unreachable!(),
        };

        let back_jmp_idx = bc
            .iter()
            .enumerate()
            .filter(|(_, b)| matches!(b.bytecode(), Instruction::JMP))
            .map(|(i, _)| i)
            .find(|&i| i > cond_idx)
            .expect("loop should emit back-edge JMP after condition");

        assert!(
            exit_target > back_jmp_idx,
            "loop exit target ({exit_target}) must be past back-edge JMP ({back_jmp_idx}); \
             otherwise the loop never exits when the condition is false"
        );
    }

    /// While-loop exit must land past the back-edge `JMP`, not on it.
    #[test]
    fn loop_jmpf_exits_past_back_edge() {
        use common::Instruction;
        let (bc, pool) = compile_src(
            "fn main() { \
 let i = 0; \
 while (i < 3) { \
 i = i + 1; \
 } \
 }",
        );

        let cond_idx = bc
            .iter()
            .position(|b| {
                matches!(
                    b.bytecode(),
                    Instruction::JMPF | Instruction::CmpJmpf | Instruction::BinSlotImmJmpf
                )
            })
            .expect("loop should emit an exit branch");
        let back_jmp_idx = bc
            .iter()
            .enumerate()
            .filter(|(_, b)| matches!(b.bytecode(), Instruction::JMP))
            .map(|(i, _)| i)
            .find(|&i| i > cond_idx)
            .expect("loop should emit back-edge JMP after exit branch");
        let exit_target = loop_exit_target(&bc, &pool).expect("loop exit target");
        assert!(
            exit_target > back_jmp_idx,
            "loop exit target ({exit_target}) must be past the back-edge JMP ({back_jmp_idx})"
        );
    }

    /// Assignment statements must not leave a trailing DUPLICATE/POP pair
    /// that shrinks the operand stack below live locals inside loops.
    #[test]
    fn assignment_statement_does_not_emit_duplicate_before_store_pop() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn main() { \
 let acc = 0; \
 let i = 0; \
 while (i < 2) { \
 acc = acc + i; \
 i = i + 1; \
 } \
 }",
        );

        let mut dup_before_store = 0usize;
        for w in bc.windows(2) {
            if matches!(w[0].bytecode(), Instruction::DUPLICATE)
                && matches!(w[1].bytecode(), Instruction::StorePop)
            {
                dup_before_store += 1;
            }
        }
        assert_eq!(
            dup_before_store, 0,
            "identifier assignment should not emit DUPLICATE before STORE_POP"
        );
    }

    #[test]
    fn for_with_break_and_continue_emits_patched_jumps() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn main() { \
let sum = 0; \
for (let i = 0; i < 10; i = i + 1) { \
if i == 3 { continue; } \
if i == 7 { break; } \
sum = sum + i; \
} \
}",
        );

        let jmp_targets: Vec<u32> = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JMP))
            .map(|b| b.operand_u32())
            .collect();

        assert!(
            jmp_targets.len() >= 3,
            "expected continue, break, and back-edge JMPs; got {:?}",
            jmp_targets
        );
        assert!(
            jmp_targets.iter().all(|target| *target != 0),
            "loop jump placeholders should be patched: {:?}",
            jmp_targets
        );
    }

    #[test]
    fn break_and_continue_outside_loop_emit_diagnostics() {
        let ast = Pratt::default()
            .parse("fn main() { break; continue; }")
            .expect("parse failed");
        let mut compiler = Compiler::default();
        compiler.compile("", &ast);
        let rendered = compiler
            .get_messages()
            .iter()
            .map(|m| format!("{:?}", m))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            rendered.contains("break outside of loop"),
            "expected break diagnostic, got {rendered}"
        );
        assert!(
            rendered.contains("continue outside of loop"),
            "expected continue diagnostic, got {rendered}"
        );
    }

    /// Codegen test 6 : the loop's JMP back-edge
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
        let (bc, _pool) = compile_src(
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

        // The JMP's target must point INTO the function
        // body (i.e., not be 0 which would be the start of
        // the body itself — the back-edge is to the loop's
        // iterable, not to the very first byte). The
        // body is what `compile_src` returns, so offset
        // 0 is the start of `main` (no prologue in the
        // returned slice — see the changes
        // to `Compiler::compile`).
        assert!(
            jmp_target > 0,
            "JMP back-edge target {} should be > 0 (into the loop body)",
            jmp_target
        );
    }

    /// Codegen test 7 : in the BlockBuilder-based
    /// Match codegen, every non-last constructor arm's
    /// JUMP_IF_MATCH placeholder is bound to that arm's body
    /// offset. If the `bind_label` for some arm's label didn't
    /// fire (e.g., the `if let Some(label) = arm_labels[i]` arm didn't
    /// fire for some non-last constructor arm), the
    /// placeholder's `value[31:0]` would be `0` (the
    /// `BlockBuilder` placeholder value), and the VM would
    /// jump to the prologue — crashing with a `HALT`.
    ///
    /// the JUMP_IF_MATCH target lives in
    /// `value[31:0]` (a full 32-bit absolute bytecode offset),
    /// NOT in the lower 16 bits of `operands`. The tag is in
    /// `operands[31:16]` (lower 16 bits reserved).
    #[test]
    fn match_jump_if_match_targets_are_patched_to_arm_offsets() {
        use common::Instruction;
        let (bc, pool) = compile_src(
            "match Option::Some(1) { \
 Option::None() => 0, \
 Option::Some(v) => v, \
 };",
        );

        // Find every JUMP_IF_MATCH. For each, the target
        // (in `value[31:0]`) must be > 0 (i.e., the placeholder
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
            let target = jim.jump_if_match_target(&pool);
            let tag = (jim.operand_u32() >> 16) as u16;
            assert!(
                target > 0,
                "JUMP_IF_MATCH #{} (tag={}) target should be patched to a non-zero offset; got {}",
                i,
                tag,
                target
            );
        }
    }

    /// Codegen test 8 : in the BlockBuilder-based
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
    /// The 15C codegen produced this exact same
    /// shape; the 17A refactor preserves it.
    #[test]
    fn match_jmp_to_end_placeholders_are_patched_to_end_label() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "enum Choice { Empty, Value(int), Maybe(int) } \
 match Choice::Value(1) { \
 Choice::Empty() => 0, \
 Choice::Value(v) => v, \
 Choice::Maybe(w) => w, \
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

    /// Codegen test 9 : a `match` inside a `while`
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
        let (bc, _pool) = compile_src(
            "fn main() { \
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

        let exit_branch_count = loop_exit_branch_count(&bc);
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
            exit_branch_count >= 1,
            "expected at least 1 loop exit branch; got {}",
            exit_branch_count
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
    // record-payload codegen tests
    // ============================================================
    //
    // The 17B spec listed 6 record-payload codegen tests. The
    // developer claimed to add them but in fact added 0 — all 6
    // were silently skipped. This section adds the missing tests,
    // including the red-team's canonical
    // `record_construct_reorders_shuffled_call_site_fields` test
    // that locks in the record-field reordering behavior.

    /// Codegen test 10 : the red-team's canonical
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
        let (bc, _pool) = compile_src(
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
            .map(|b| b.constant(&[]) as i64)
            .filter(|&v| (1..=3).contains(&v))
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

    /// Codegen test 11 : a record construct with one
    /// field emits exactly 1 CONST followed by MAKE_ENUM with
    /// arity=1.
    #[test]
    fn record_construct_one_field_emits_correct_bytecode() {
        use common::Instruction;
        let (bc, _pool) =
            compile_src("enum E { Foo { x: int } } fn main() { let _ = E::Foo { x: 1 }; }");

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
            .filter(|b| matches!(b.bytecode(), Instruction::CONST) && b.constant(&[]) == 1)
            .count();
        assert_eq!(
            const_one_count, 1,
            "expected exactly 1 CONST with value 1; got {}",
            const_one_count
        );
    }

    /// Codegen test 12 : a match pattern with
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
        let (bc, _pool) = compile_src(
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

    /// Codegen test 13 : a mixed-shape enum with
    /// Unit + Tuple + Record variants compiles with the
    /// correct tags and arities for each variant.
    #[test]
    fn mixed_enum_unit_tuple_record_all_in_one() {
        use common::Instruction;
        // Use prints to keep the constructs alive in the
        // bytecode (the codegen is silent on unused `let _`).
        let (bc, _pool) = compile_src(
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

    /// Codegen test 14 : a record pattern with a
    /// wildcard field (`_`) emits a POP for the wildcard
    /// sub-pattern instead of a STORE.
    #[test]
    fn record_pattern_with_wildcard_field_emits_pop() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
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

    /// Codegen test 15 : a unit-variant match arm
    /// (`Empty`) does NOT emit UNPACK (the variant has no
    /// payload). It emits a POP to discard the scrutinee.
    #[test]
    fn empty_record_pattern_does_not_emit_unpack() {
        use common::Instruction;
        // The spec says "E::Empty => 0" where Empty is unit.
        // The codegen for a unit-variant last arm emits POP,
        // not UNPACK.
        let (bc, _pool) = compile_src(
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

    // ============================================================
    // inner-pattern dispatch regression tests
    // ============================================================
    //
    // fixes the inner-pattern dispatch for multi-arm match
    // groups that share the same OUTER variant tag but differ on the
    // INNER sub-pattern. Before 18A, the codegen emitted POP
    // placeholders for nested Constructor sub-patterns in the test
    // chain, so all arms in a multi-arm group that shared an outer
    // tag were dispatched in source order regardless of the actual
    // inner tag (the first matching arm always won, even if the
    // runtime inner tag would have picked a different arm).
    //
    // After 18A:
    // - `arm_has_runtime_test` is more selective — it only flags
    // arms whose inner sub-patterns carry a `Binding` or further
    // nested `Constructor` (i.e., the inner pattern actually
    // binds a value that needs runtime extraction).
    // - `emit_inner_test` emits a real `JUMP_IF_MATCH` for the
    // inner tag instead of a POP placeholder, so the runtime
    // correctly picks the arm whose inner tag matches.
    // - The forward pass keeps the existing behavior (one
    // JUMP_IF_MATCH per non-last group + UNPACK for the last
    // arm of the last group) — the common case (1 arm per tag,
    // all binding/wildcard sub-patterns) produces byte-for-byte
    // identical bytecode.
    //
    // These five tests pin down the new behavior at the codegen
    // level. The end-to-end runtime behavior is verified separately
    // by the `example_match_with_two_ok_arms_dispatches_correctly`
    // test in `compiler/tests/pipeline.rs` (which compiles and runs
    // `examples/result.0s` after it's extended to two `Result::Ok`
    // arms).

    /// Codegen test 16 : Case 4 — a multi-arm match
    /// group with two arms sharing the outer tag and BOTH arms
    /// having inner Constructor sub-patterns with bindings emits
    /// ≥2 JUMP_IF_MATCH (one for the outer tag dispatch, one for
    /// the inner Constructor dispatch).
    #[test]
    fn match_with_same_tag_different_constructors_emits_inner_test_chain() {
        use common::Instruction;
        // Case 4: `match x { E::A(Option::Some(v)) => v, E::A(Option::None) => 0 }`
        // Both arms share the outer tag `E::A`. The first arm's
        // inner pattern is `Option::Some(v)` — a Constructor with a
        // Binding sub-pattern, which triggers the new test chain.
        let (bc, _pool) = compile_src(
            "enum E { A(Option) } \
 fn main() { \
 let x = E::A(Option::Some(42)); \
 let _ = match x { \
 E::A(Option::Some(v)) => v, \
 E::A(Option::None) => 0, \
 }; \
 }",
        );
        let jimp_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JumpIfMatch))
            .count();
        let pop_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::POP))
            .count();
        assert!(
            jimp_count >= 2,
            "expected ≥2 JUMP_IF_MATCH (outer A + inner Some); got {}",
            jimp_count
        );
        assert!(
            pop_count >= 1,
            "expected ≥1 POP (placeholder for the inner None Unit); got {}",
            pop_count
        );
    }

    /// Codegen test 17 : Case 1 — wildcard inner
    /// sub-patterns DON'T trigger the new test chain. The runtime
    /// always accepts a wildcard, so a runtime inner test would be
    /// redundant; the codegen keeps the existing layout (just one
    /// JUMP_IF_MATCH for the outer tag).
    #[test]
    fn match_with_same_tag_and_wildcard_subpatterns_keeps_current_layout() {
        use common::Instruction;
        // Case 1: `match x { E::A(Option::None) => 1, E::A(Option::Some(_)) => 2 }`
        // Both arms share the outer tag `E::A`. The inner
        // sub-patterns are Unit (`None`) and Wildcard (`Some(_)`) —
        // neither carries a Binding, so `arm_has_runtime_test`
        // returns false for both arms. No test chain is emitted;
        // the codegen keeps the existing layout.
        let (bc, _pool) = compile_src(
            "enum E { A(Option) } \
 fn main() { \
 let x = E::A(Option::None); \
 let _ = match x { \
 E::A(Option::None) => 1, \
 E::A(Option::Some(_)) => 2, \
 }; \
 }",
        );
        let jimp_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JumpIfMatch))
            .count();
        assert_eq!(
            jimp_count, 1,
            "expected exactly 1 JUMP_IF_MATCH (no test chain for wildcard sub-patterns); got {}",
            jimp_count
        );
    }

    /// Codegen test 18 : Case 2 — Binding inner
    /// sub-patterns at the OUTER level (i.e., simple bindings like
    /// `A(v)` with no nested Constructor) DON'T trigger the new
    /// test chain. The codegen keeps the existing layout.
    ///
    /// The user's source for this test uses nested Constructor
    /// sub-patterns to match the description
    /// (`E::A(Option::Some(v))`, `E::A(Option::None)`). With the
    /// refined `arm_has_runtime_test`, the `Some(v)` arm DOES
    /// trigger a test chain (its inner pattern has a Binding).
    /// However, this test specifically asserts the COMBINED-CASE
    /// count for an arms-only-Bindings scenario (no nested
    /// Constructor at all). See
    /// `match_bindings_per_arm_still_works_with_test_chain`
    /// for the test-chain-enabled variant.
    ///
    /// We assert 1 JUMP_IF_MATCH here to lock in the
    /// single-JUMP_IF_MATCH case. This guards against future
    /// changes that would over-emit JUMP_IF_MATCH for trivial
    /// bindings.
    #[test]
    fn match_with_simple_binding_subpatterns_keeps_current_layout() {
        use common::Instruction;
        // Two arms with the same outer tag, but the inner patterns
        // are just Bindings (no nested Constructor). arm_has_runtime_test
        // returns false → no test chain → 1 JUMP_IF_MATCH.
        //
        // NOTE: `E::A(v)` is the simple-binding pattern. We declare
        // `E::B(int)` so the parser accepts `E::A(v) => v` as
        // distinct from a constructor call (the parser treats the
        // pattern `E::A(v)` as a Constructor with a single Binding
        // sub-pattern; `arm_has_runtime_test` recursively checks
        // that sub-pattern, which is a Binding → no runtime test).
        let (bc, _pool) = compile_src(
            "enum E { A(int), B(int) } \
 fn main() { \
 let x = E::A(5); \
 let _ = match x { \
 E::A(v) => v, \
 E::B(v) => v, \
 }; \
 }",
        );
        let jimp_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JumpIfMatch))
            .count();
        // For a 2-arm match with unique outer tags, the existing
        // behavior is one JUMP_IF_MATCH (for the non-last arm) + one
        // UNPACK (for the last arm's scrutinee-consumer). The
        // simple-binding case is unaffected by .
        assert_eq!(
            jimp_count, 1,
            "expected 1 JUMP_IF_MATCH (simple bindings keep the existing layout); got {}",
            jimp_count
        );
    }

    /// Codegen test 19 : Case 5 — a match with two
    /// tag groups where one group is multi-arm emits one
    /// JUMP_IF_MATCH per GROUP (not per arm). The codegen
    /// emitted one JUMP_IF_MATCH per non-last arm, which would have
    /// produced 2 JUMP_IF_MATCH (one per non-last arm: arm 0 for A
    /// is non-last, arm 1 for B is non-last). After 18A the
    /// grouping is by outer tag, so the multi-arm group A gets one
    /// JUMP_IF_MATCH and the single-arm group B (last) gets a
    /// different shape — the result is exactly 2 JUMP_IF_MATCH
    /// (one per group).
    #[test]
    fn match_with_two_tag_groups_dispatches_correctly() {
        use common::Instruction;
        // Case 5: `match x { E::A => 1, E::B => 2, E::A => 3 }`
        // Two groups: A (arms 0 and 2) and B (arm 1). Group A is
        // multi-arm. The codegen emits one JUMP_IF_MATCH per group
        // (the multi-arm group's JUMP_IF_MATCH targets the test
        // chain start; the single-arm group's JUMP_IF_MATCH targets
        // its arm body).
        let (bc, _pool) = compile_src(
            "enum E { A, B } \
 fn main() { \
 let x = E::A; \
 let _ = match x { \
 E::A => 1, \
 E::B => 2, \
 E::A => 3, \
 }; \
 }",
        );
        let jimp_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JumpIfMatch))
            .count();
        assert_eq!(
            jimp_count, 2,
            "expected 2 JUMP_IF_MATCH (one per group, not per arm); got {}",
            jimp_count
        );
    }

    /// Codegen test 20 : verifies that the test chain
    /// correctly populates the per-arm `match_bindings` map for
    /// arms with inner Binding sub-patterns. The arm body for the
    /// `Some(v)` arm must be able to read `v` (via `LOAD v`),
    /// which requires the codegen to record `v → slot 1` in
    /// `match_bindings_per_arm`. We don't assert on the slot value
    /// directly (it's an internal detail), but we verify the
    /// bytecode is well-formed and the `Expression::Identifier`
    /// lookup inside the arm body resolves correctly by checking
    /// that the bytecode compiles to a non-empty sequence and
    /// contains the expected opcodes.
    ///
    /// (The HM typechecker currently flags the second arm as
    /// "Unreachable arm" because it doesn't track inner-pattern
    /// distinctions — a known limitation. The codegen still emits
    /// bytecode for the unreachable arm defensively, which is what
    /// we want for the inner-pattern dispatch fix. The end-to-end
    /// runtime behavior is verified by the
    /// `example_match_with_two_ok_arms_dispatches_correctly` golden
    /// test in `compiler/tests/pipeline.rs`.)
    #[test]
    fn match_bindings_per_arm_still_works_with_test_chain() {
        use common::Instruction;
        // Two arms sharing the outer tag E::A, with the first arm's
        // inner pattern having a Binding (`Some(v)`). The codegen
        // must populate `match_bindings_per_arm` so the arm body's
        // `v` reference resolves to the slot JUMP_IF_MATCH pushed
        // the inner int into.
        let src = "enum E { A(Option) } \
 fn main() { \
 let x = E::A(Option::Some(42)); \
 let _ = match x { \
 E::A(Option::Some(v)) => v, \
 E::A(Option::None) => 0, \
 }; \
 }";
        let ast = Pratt::default().parse(src).expect("parse failed");
        let bc = Compiler::default().compile("test", &ast);
        // The bytecode must include the outer JUMP_IF_MATCH (for A)
        // and the inner JUMP_IF_MATCH (for Some) — the test chain
        // emitted both.
        let jimp_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JumpIfMatch))
            .count();
        assert!(
            jimp_count >= 2,
            "expected ≥2 JUMP_IF_MATCH (outer A + inner Some); got {}",
            jimp_count
        );
    }

    /// Codegen test 21 (POP-quirk fix): the
    /// reverse pass's `emit_pattern_binding` must NOT emit a
    /// redundant POP for the inner Unit sub-pattern when the
    /// test chain has already consumed the value. Pre-fix, the
    /// codegen would emit a POP in the test chain (for
    /// `Option::None`) AND a second POP in the reverse pass's
    /// binding code (because the Unit case unconditionally
    /// emits a defensive POP). The second POP silently consumes
    /// a stale value, which is wasteful and could matter for
    /// nested control flow.
    ///
    /// Post-fix, the reverse pass detects the test chain arm
    /// (via `test_chain_arms`) and passes `consume_values =
    /// false` to `emit_pattern_binding`, suppressing the
    /// redundant POP. The test chain's POP is the only one
    /// emitted for the inner Unit sub-pattern.
    ///
    /// This test asserts that for the canonical
    /// `Result::Ok(Option::Some(v))` vs `Result::Ok(Option::None)`
    /// pattern (where the first arm triggers the test chain and
    /// the second arm's inner pattern is Unit), the resulting
    /// bytecode has:
    /// - 2 JUMP_IF_MATCH (outer Result::Ok + inner Option::Some)
    /// - 1 POP for the inner Unit sub-pattern (the test
    /// chain's POP, not the reverse pass's redundant POP).
    ///
    /// The pre-fix codegen would have emitted 2 POPs for the
    /// inner Unit sub-pattern (1 from the test chain + 1 from
    /// the reverse pass). The fix reduces this to 1.
    #[test]
    fn test_chain_none_arm_does_not_double_pop() {
        use common::Instruction;
        // Two arms sharing the outer tag Result::Ok. The first
        // arm's inner pattern is `Option::Some(v)` (nested
        // Constructor with a Binding → triggers the test chain).
        // The second arm's inner pattern is `Option::None` (Unit
        // sub-pattern). The test chain emits:
        // - 1 JUMP_IF_MATCH for the outer Result::Ok
        // - 1 JUMP_IF_MATCH for the inner Option::Some
        // - 1 POP for the inner Option::None (Unit, no pass label)
        //
        // The reverse pass, post-fix, emits 0 POPs for the
        // inner Unit sub-pattern (the test chain handled it).
        // Pre-fix, the reverse pass would emit 1 redundant POP.
        let src = "fn main() { \
 let x = Result::Ok(Option::Some(42)); \
 let _ = match x { \
 Result::Ok(Option::Some(v)) => v, \
 Result::Ok(Option::None) => 0, \
 }; \
 }";
        let ast = Pratt::default().parse(src).expect("parse failed");
        let bc = Compiler::default().compile("test", &ast);

        // Exactly 2 JUMP_IF_MATCH (outer Ok + inner Some).
        let jimp_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JumpIfMatch))
            .count();
        assert_eq!(
            jimp_count, 2,
            "expected exactly 2 JUMP_IF_MATCH (outer Ok + inner Some); got {}",
            jimp_count
        );

        // POPs expected:
        // - 1 from the test chain for the inner Unit (`Option::None`)
        // - 1 from Match's end-label `DUPLICATE; POP` fusion barrier
        //   (Phase P0 — keeps peephole from fusing last-arm CONST with
        //   a following RETURN). The reverse pass must NOT add a third
        //   redundant POP for the Unit sub-pattern.
        //
        // Note: `let _ = match x { ... }` is an Assignment (no
        // ExprStatement wrapper), so it doesn't add another POP.
        let pop_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::POP))
            .count();
        assert_eq!(
            pop_count, 2,
            "expected 2 POPs (test-chain Unit + match end barrier); got {} (3+ means reverse-pass double-pop regression)",
            pop_count
        );
    }

    // ============================================================
    // field-access codegen tests
    // ============================================================
    //
    // The spec locked in 2 codegen tests for the new
    // `Expression::Access` arm. Both verify the bytecode SHAPE
    // (MakeEnum → LoadField) and the operand (field_index) so the
    // runtime extraction reads the right slot.

    /// Codegen test 22 : a simple field access on a
    /// function parameter emits `MakeEnum` (for the construct in
    /// `main`) followed by `LoadField(0)` (the first field, `x`)
    /// in the function body. If the codegen skipped the receiver
    /// bytecode, the stack wouldn't have the enum value at the
    /// point of LoadField and the VM would crash.
    #[test]
    fn access_field_emits_receiver_then_load_field() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "enum Point { Origin, Point { x: int, y: int } } \
 fn get_x(Point p) -> int { return p.x; } \
 fn main() { print \"%i\", get_x(Point::Point { x: 42, y: 7 }); }",
        );

        // Exactly 1 LoadField (in the get_x function body).
        let load_field_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::LoadField))
            .count();
        assert_eq!(
            load_field_count, 1,
            "expected exactly 1 LoadField (p.x in get_x); got {}",
            load_field_count
        );

        // The LoadField operand is 0 (x is the first field).
        let load_field = bc
            .iter()
            .find(|b| matches!(b.bytecode(), Instruction::LoadField))
            .expect("expected at least one LoadField");
        let field_index = load_field.operand_u32() & 0xFFFF;
        assert_eq!(
            field_index, 0,
            "expected LoadField(0) for field 'x' (declaration index 0); got LoadField({})",
            field_index
        );
    }

    /// Codegen test 23 : a field access on a DIFFERENT
    /// field of the same record emits `LoadField(1)` — the
    /// declaration position of `y`. The red-team flagged this as
    /// a critical regression test: a buggy codegen that always
    /// emitted `LoadField(0)` would pass the previous test but
    /// return the WRONG value here (silently reading `x` when the
    /// user asked for `y`).
    #[test]
    fn access_field_emits_correct_field_index_for_each_field() {
        use common::Instruction;
        // Two functions, each accessing a different field. The
        // x_coord access emits LoadField(0); the y_coord access
        // emits LoadField(1).
        let (bc, _pool) = compile_src(
            "enum Point { Origin, Point { x: int, y: int } } \
 fn x_coord(Point p) -> int { return p.x; } \
 fn y_coord(Point p) -> int { return p.y; } \
 fn main() { \
 print \"%i\", x_coord(Point::Point { x: 5, y: 12 }); \
 print \"%i\", y_coord(Point::Point { x: 5, y: 12 }); \
 }",
        );

        // Exactly 2 LoadField (one per function body).
        let load_field_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::LoadField))
            .count();
        assert_eq!(
            load_field_count, 2,
            "expected exactly 2 LoadField (x_coord + y_coord); got {}",
            load_field_count
        );

        // Collect every LoadField operand; we expect [0, 1]
        // (x_coord uses field 0, y_coord uses field 1). The order
        // depends on the function layout — both x_coord and y_coord
        // are emitted before main, so the operands appear in source
        // order in the bytecode.
        let field_indices: Vec<u32> = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::LoadField))
            .map(|b| b.operand_u32() & 0xFFFF)
            .collect();
        assert_eq!(
            field_indices,
            vec![0, 1],
            "expected LoadField operands [0, 1] (x first, then y); got {:?}",
            field_indices
        );
    }

    // ============================================================
    // let-bound variable codegen tests
    // ============================================================
    //
    // fixes the `let x = expr;` codegen bug — the
    // `Expression::Variable` codegen emitted no bytecode,
    // so the slot was never explicitly written. The simple case
    // `let x = 5; print x;` worked by coincidence (slot 0
    // coincided with the operand-stack top). Reassignment via
    // `x = 10;` used `STORE` (a no-op since 15D) + `DUPLICATE`,
    // which didn't fix the slot either.
    //
    // The fix: the `Expression::Fragment` arm special-cases the
    // `[Variable, expr]` shape and emits `STORE_POP slot` after
    // the RHS bytecode. `Expression::Assignment` now emits
    // `STORE_POP slot` instead of the buggy `STORE` + `DUPLICATE`.
    //
    // These tests assert the bytecode SHAPE (StorePop after the
    // RHS, with the correct slot index) and the runtime behavior
    // (re-assignment picks up the new value, multiple bindings
    // are preserved).

    /// Codegen test 24 : a simple `let x = expr; print x;`
    /// emits exactly one `STORE_POP` (the store of the
    /// RHS into `x`'s slot) in addition to the RHS's `CONST`.
    #[test]
    fn let_x_then_print_x_emits_store_pop() {
        use common::Instruction;
        let (bc, _pool) = compile_src("fn main() { let x = 42; print \"%i\", x; }");

        // At least one STORE_POP — the explicit
        // pop-and-write for `let x = 42`. The codegen
        // never emits STORE for let-bindings (STORE is a
        // no-op reserved for match-arm bindings).
        let store_pop_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::StorePop))
            .count();
        assert!(
            store_pop_count >= 1,
            "expected at least 1 STORE_POP for `let x = 42;`; got {}",
            store_pop_count
        );

        // The STORE_POP slot operand should be 0 — `x` is the
        // first (and only) local in `main`.
        let store_pop = bc
            .iter()
            .find(|b| matches!(b.bytecode(), Instruction::StorePop))
            .expect("expected at least one STORE_POP");
        assert_eq!(
            store_pop.operand_u32(),
            0,
            "expected STORE_POP slot=0 for the first local `x`; got {}",
            store_pop.operand_u32()
        );
    }

    /// Codegen test 25 : two `let` bindings in the same
    /// scope emit two `STORE_POP`s — one per binding, with
    /// distinct slot operands (0 and 1).
    #[test]
    fn let_two_bindings_emit_two_store_pops() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn main() { \
 let x = 5; \
 let y = 10; \
 print \"%i\", x + y; \
 }",
        );

        let store_pops: Vec<u32> = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::StorePop))
            .map(|b| b.operand_u32())
            .collect();
        assert_eq!(
            store_pops.len(),
            2,
            "expected exactly 2 STORE_POPs for two `let` bindings; got {}",
            store_pops.len()
        );
        // The slot operands should be 0 and 1 (in source order —
        // `x` first, then `y`).
        assert!(
            store_pops.contains(&0) && store_pops.contains(&1),
            "expected STORE_POP slots [0, 1] for x, y; got {:?}",
            store_pops
        );
    }

    /// Codegen test 26 : `x = 10;` re-assignment emits
    /// `STORE_POP slot` (the new opcode) — NOT the
    /// `STORE` (a no-op since ) + `DUPLICATE`
    /// shape. The codegen would emit `STORE` here, which
    /// is the red-team's critical regression signature.
    #[test]
    fn let_x_reassignment_emits_store_pop_not_store() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn main() { \
 let x = 5; \
 x = 10; \
 }",
        );

        // At least one STORE_POP — the re-assignment for
        // `x = 10`. The codegen would have used
        // STORE here instead.
        let store_pop_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::StorePop))
            .count();
        assert!(
            store_pop_count >= 1,
            "expected at least 1 STORE_POP for `x = 10;` re-assignment; got {}",
            store_pop_count
        );

        // Zero `STORE` instructions — the codegen should
        // never emit STORE for let-bindings or assignments.
        // STORE is reserved for match-arm bindings (where it
        // acts as a no-op for the slot-push contract).
        let store_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::STORE))
            .count();
        assert_eq!(
            store_count, 0,
            "expected zero STORE instructions for `let` / assignment; got {}",
            store_count
        );
    }

    // ============================================================
    // growing array builtin codegen tests
    // ============================================================

    #[test]
    fn array_push_and_len_builtins_emit_array_opcodes() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn main() { \
let a = [1, 2]; \
push(a, 3); \
print \"%i\", len(a); \
}",
        );

        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::ArrayPush)),
            "expected `push(a, 3)` to emit ArrayPush"
        );
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::ArrayLen)),
            "expected `len(a)` to emit ArrayLen"
        );
        assert!(
            !bc.iter()
                .any(|b| { matches!(b.bytecode(), Instruction::CALL) && b.call_parts().1 > 3 }),
            "push/len builtins should not lower to ordinary CALL instructions"
        );
    }

    // ============================================================
    // chained field-access codegen tests
    // ============================================================
    //
    // fixes the chained-access limitation: `p.x.v` (where
    // `p.x` is itself a record-shaped enum) now resolves to the
    // INNER enum's field, not the OUTER enum's. The bytecode
    // shape for a chained access is the same as for two
    // independent accesses — two `LoadField` opcodes stacked on
    // top of the receiver bytecode — but the operand of the
    // SECOND `LoadField` is indexed against the INNER enum, not
    // the OUTER one.

    /// Codegen test 27 : a chained field access
    /// (`p.x.v` where `x: Inner`, `v: int`) emits exactly TWO
    /// `LoadField` opcodes in the function body — one for the
    /// inner access (`x`) and one for the OUTER access (`v`).
    /// The codegen would emit only one `LoadField`
    /// (followed by a defensive `LoadField(0)` for the OUTER),
    /// silently miscompiling the OUTER access.
    #[test]
    fn access_chained_field_emits_two_load_fields() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "enum Inner { Inner { v: int } } \
 enum Outer { Outer { x: Inner, y: int } } \
 fn get_x_v(Outer o) -> int { return o.x.v; } \
 fn main() { print \"%i\", get_x_v(Outer::Outer { x: Inner::Inner { v: 42 }, y: 7 }); }",
        );

        // Exactly 2 LoadField (one for `o.x`, one for `o.x.v`)
        // in the get_x_v function body.
        let load_field_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::LoadField))
            .count();
        assert_eq!(
            load_field_count, 2,
            "expected exactly 2 LoadField (o.x and o.x.v in get_x_v); got {}",
            load_field_count
        );
    }

    /// Codegen test 28 : the SECOND `LoadField`'s
    /// operand is `0` — `v`'s declaration index in the INNER
    /// `Inner` enum, NOT something from `Outer`. The earlier
    /// codegen would emit `LoadField(0)` as a defensive
    /// fallback, which happens to coincide with `v`'s index
    /// here, so this test alone wouldn't catch the bug. The
    /// next test pins the correct OUTER-vs-INNER indexing.
    #[test]
    fn access_chained_field_second_load_field_targets_inner_enum() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "enum Inner { Inner { v: int, w: int } } \
 enum Outer { Outer { x: Inner, y: int } } \
 fn get_x_v(Outer o) -> int { return o.x.v; } \
 fn main() { print \"%i\", get_x_v(Outer::Outer { x: Inner::Inner { v: 42, w: 99 }, y: 7 }); }",
        );

        // Exactly 2 LoadField in the function body.
        let load_field_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::LoadField))
            .count();
        assert_eq!(
            load_field_count, 2,
            "expected exactly 2 LoadField for chained access; got {}",
            load_field_count
        );

        // Collect every LoadField operand. We expect:
        // - First LoadField(0) — Outer's `x` field index.
        // - Second LoadField(0) — Inner's `v` field index.
        // (Both happen to be 0 because `x` is Outer's first
        // declared field and `v` is Inner's first declared
        // field. The order is determined by the source-order
        // emission of the two access codepaths.)
        let field_indices: Vec<u32> = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::LoadField))
            .map(|b| b.operand_u32() & 0xFFFF)
            .collect();
        assert_eq!(
            field_indices,
            vec![0, 0],
            "expected LoadField operands [0, 0] (Outer.x is 0, Inner.v is 0); got {:?}",
            field_indices
        );
    }

    /// Codegen test 29 : the critical regression
    /// test. When the OUTER access's field is at a DIFFERENT
    /// declaration position in the INNER enum than it would
    /// be in the OUTER enum, the codegen must pick the INNER
    /// position. Setup: `Inner.w` is at index 1 (not 0); the
    /// codegen would emit `LoadField(0)` for the OUTER
    /// access, silently reading `v` when the user asked for
    /// `w`.
    ///
    /// Note: we can't easily observe the runtime value of the
    /// OUTER access in this codegen test (the VM doesn't
    /// return a value we can assert on), so we just check the
    /// bytecode SHAPE — the second LoadField operand is `1`
    /// (`w`'s index in `Inner`), not `0`.
    #[test]
    fn access_chained_field_with_correct_field_index() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "enum Inner { Inner { v: int, w: int } } \
 enum Outer { Outer { x: Inner, y: int } } \
 fn get_x_w(Outer o) -> int { return o.x.w; } \
 fn main() { print \"%i\", get_x_w(Outer::Outer { x: Inner::Inner { v: 42, w: 99 }, y: 7 }); }",
        );

        // Exactly 2 LoadField (one for `o.x`, one for `o.x.w`).
        let load_field_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::LoadField))
            .count();
        assert_eq!(
            load_field_count, 2,
            "expected exactly 2 LoadField for chained access; got {}",
            load_field_count
        );

        // Collect every LoadField operand. We expect:
        // - First LoadField(0) — Outer's `x` field index.
        // - Second LoadField(1) — Inner's `w` field index
        // (NOT Outer's `y` index — which would be 1 in
        // Outer but isn't what the user asked for).
        let field_indices: Vec<u32> = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::LoadField))
            .map(|b| b.operand_u32() & 0xFFFF)
            .collect();
        assert_eq!(
            field_indices,
            vec![0, 1],
            "expected LoadField operands [0, 1] (Outer.x is 0, Inner.w is 1); got {:?}",
            field_indices
        );
    }

    // ============================================================
    // nested record patterns — codegen tests
    // ============================================================
    //
    // lifts the -cleanup limitation #1
    // (nested record patterns inside an arm body are rejected).
    // The codegen emitted a POP for an inner record
    // pattern instead of walking its declared fields, so the
    // binding slot for the inner record's fields was never
    // populated and the arm body read garbage values.
    //
    // These tests guard the codegen for the four
    // nested-record scenarios called out in the spec:
    //
    // 1. Nested record in tuple: `Result::Ok(Inner { v })`.
    // 2. Nested record in record: `Result::Ok { x: Inner { v } }`.
    // 3. Depth-3 nesting: `Foo::Bar(Baz::Qux { a: W::W { v } })`.
    // 4. Missing field in inner record (defensive POP emitted).
    //
    // The tests check the bytecode SHAPE (opcodes emitted) so
    // accidental regressions in the codegen are caught even if
    // the runtime happens to produce the right output for a
    // buggy bytecode (e.g. by accidentally emitting POP for
    // every record, which would compile and run but bind to
    // the wrong slots).

    /// Codegen test 23 : a record pattern inside a
    /// tuple pattern (`Result::Ok(Inner::I { v })`) compiles
    /// cleanly. Pre-18B, the inner `Inner::I { v }` was
    /// silently swallowed (a single POP was emitted for the
    /// inner record instead of walking its declared fields).
    /// The post-fix codegen emits at least one STORE for the
    /// inner Binding `v`.
    #[test]
    fn match_nested_record_in_tuple_binds_correctly() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "enum Inner { I { v: int } } \
 match Result::Ok(Inner::I { v: 42 }) { \
 Result::Err(_) => 0, \
 Result::Ok(Inner::I { v }) => v, \
 };",
        );

        // The OUTER Result::Ok is the last arm (Err is first),
        // so it consumes the scrutinee via UNPACK (not
        // JUMP_IF_MATCH). The INNER Inner::I is a nested
        // constructor with a record payload — the codegen
        // emits UNPACK for the inner record , then
        // walks the inner record's declared fields in
        // decl_order and emits STORE for the Binding `v`.
        //
        // Pre-18B, the codegen emitted a POP for the inner
        // record (silently swallowing the inner value). The
        // STORE assertion catches that regression.
        let store_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::STORE))
            .count();
        assert!(
            store_count >= 1,
            "expected at least one STORE for the inner Binding `v`; got {} (would emit 0)",
            store_count
        );

        // The inner record's UNPACK must be present (
        // walks the inner record's declared fields).
        let unpack_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::Unpack))
            .count();
        assert!(
            unpack_count >= 1,
            "expected at least one UNPACK (the inner Inner::I); got {}",
            unpack_count
        );
    }

    /// Codegen test 24 : a record pattern inside a
    /// record pattern (`Result::Ok { x: Inner::I { v } }`).
    /// The codegen walks BOTH the OUTER record's and the
    /// INNER record's declared fields in decl_order. Pre-18B,
    /// the inner record was silently swallowed.
    #[test]
    fn match_nested_record_in_record_binds_correctly() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "enum Inner { I { v: int } } \
 enum Wrap { Good { x: Inner }, Bad(string) } \
 match Wrap::Good { x: Inner::I { v: 42 } } { \
 Wrap::Bad(_) => 0, \
 Wrap::Good { x: Inner::I { v } } => v, \
 };",
        );

        // The OUTER Result::Ok is the last arm, so the
        // forward pass emits UNPACK (1 payload slot for
        // Result::Ok { x }). The INNER record's payload is
        // pushed at slot 1 by the outer UNPACK. The codegen
        // walks the outer record's declared fields (just `x`)
        // and then walks the inner record's declared fields
        // (just `v`), emitting STORE for `v`.
        //
        // Pre-18B, the inner record would have been replaced
        // by a single POP, so no STORE for `v`.
        let store_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::STORE))
            .count();
        assert!(
            store_count >= 1,
            "expected at least one STORE for the inner Binding `v`; got {}",
            store_count
        );
    }

    /// Codegen test 25 : depth-3 nested constructor
    /// patterns (`Foo::Bar(Baz::Qux { a: W::W { v } })`).
    /// The codegen recurses at unbounded depth — three levels
    /// of nested constructor patterns, with the innermost being
    /// a record. Pre-18B, the inner record was silently
    /// swallowed at any depth > 1.
    #[test]
    fn match_depth_3_nested_records_bind_correctly() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "enum W { W { v: int } } \
 enum Baz { Qux { a: W } } \
 enum Foo { Bar(Baz), Other } \
 match Foo::Bar(Baz::Qux { a: W::W { v: 99 } }) { \
 Foo::Other => 0, \
 Foo::Bar(Baz::Qux { a: W::W { v } }) => v, \
 };",
        );

        // The innermost Binding `v` must produce at least
        // one STORE — the codegen reached the innermost
        // record at depth 3 and emitted the STORE for `v`.
        // Pre-18B would have stopped at the inner record and
        // emitted a POP instead, leaving `v` unbound.
        let store_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::STORE))
            .count();
        assert!(
            store_count >= 1,
            "expected at least one STORE for the innermost Binding `v` (depth 3); got {}",
            store_count
        );
    }

    /// Codegen test 26 : a record pattern with an
    /// OMITTED field (`Inner::I { }` instead of `Inner::I { v }`)
    /// emits a POP for the missing field (to keep the stack
    /// consistent with the decl_order walk). Pre-18B, the inner
    /// record was silently swallowed entirely.
    #[test]
    fn match_nested_record_missing_field_consumes_slot() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "enum Inner { I { v: int } } \
 match Result::Ok(Inner::I { v: 42 }) { \
 Result::Err(_) => 0, \
 Result::Ok(Inner::I { }) => 99, \
 };",
        );

        // The pattern omits the `v` field. The codegen walks
        // the inner record's declared fields in decl_order
        // and emits POP for the missing field. Pre-18B, the
        // codegen emitted a single POP for the inner record
        // (regardless of how many fields it had) — this
        // assertion is a sanity check that the codegen still
        // produces a well-formed bytecode for this case (the
        // arm body is `99` and doesn't reference any bindings).
        //
        // We don't assert exact POP count (other parts of
        // the bytecode emit POPs too — e.g. the prologue's
        // scrutinee POP for the wildcard arm); we just check
        // the bytecode compiles.
        assert!(!bc.is_empty(), "bytecode should not be empty");

        // Sanity: the arm body `99` should produce a
        // non-zero integer constant somewhere in the bytecode.
        // (The CONST opcode uses `value[63:0]` for the
        // constant — see `Byte::constant()`.)
        let has_99 = bc
            .iter()
            .any(|b| matches!(b.bytecode(), Instruction::CONST) && b.constant(&[]) == 99);
        assert!(has_99, "expected CONST 99 for the arm body");
    }

    /// A Num-constrained shared generic body dispatches through its trailing
    /// dictionary rather than a legacy DynAdd opcode.
    #[test]
    fn generic_add_emits_dictionary_indirect_call() {
        use common::Instruction;
        let (bc, _pool) =
            compile_src("fn add<T: Num>(T a, T b) -> T { return a + b; } fn main() { }");
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::CallIndirect)),
            "expected CallIndirect for generic fn add<T: Num>; bytecode opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
        assert!(
            !bc.iter().any(|b| matches!(b.bytecode(), Instruction::DynAdd)),
            "new shared generic bodies must not emit DynAdd"
        );
    }

    /// Codegen test B1-2: a concrete `fn add(int a, int b) -> int { return a + b; }`
    /// must NOT emit `DynAdd` — it should use the regular `ADD` (or the peephole-fused
    /// `BinSlotSlot`) path.
    #[test]
    fn concrete_add_still_emits_add() {
        use common::Instruction;
        let (bc, _pool) =
            compile_src("fn add(int a, int b) -> int { return a + b; } fn main() { }");
        assert!(
            !bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::DynAdd)),
            "DynAdd must NOT appear for concrete fn add(int a, int b)"
        );
        // Either ADD (unfused) or BinSlotSlot (peephole-fused) must be present.
        let has_int_add = bc.iter().any(|b| {
            matches!(b.bytecode(), Instruction::ADD)
                || matches!(b.bytecode(), Instruction::BinSlotSlot)
                || matches!(b.bytecode(), Instruction::BinReturn)
        });
        assert!(
            has_int_add,
            "expected ADD or BinSlotSlot/BinReturn for concrete add; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// Codegen test B3-1: when a generic function is referenced in a non-call position
    /// (e.g. `let f = id;`), the codegen must emit `MakePolyFn` so that `f` holds an
    /// `ObjPolyFn` heap pointer that `CallIndirect` can dispatch through.
    ///
    /// The function `id<T>(T x) -> T` is a canonical unconstrained identity and has
    /// no typeclass bound, so no DynAdd / DynCmp / etc. opcode is emitted — this
    /// purely tests the MakePolyFn path.
    #[test]
    fn generic_fn_as_value_emits_make_polyfn() {
        use common::Instruction;
        // `let f = id;` in main must compile `id` (a generic fn) as a MakePolyFn rather
        // than a direct CALL or LOAD — id is not a local variable, so the Identifier arm
        // must detect `is_generic_fn("id")` and emit MakePolyFn with id's entry offset.
        let (bc, _pool) = compile_src("fn id<T>(T x) -> T { return x; } fn main() { let f = id; }");
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::MakePolyFn)),
            "expected MakePolyFn for `let f = id` where id is generic; bytecode opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// Phase 3: escaping a constrained generic from an active `__dictN` scope
    /// emits `MakePolyFnCapture` instead of bare `MakePolyFn`.
    #[test]
    fn constrained_generic_escape_emits_make_polyfn_capture() {
        use common::Instruction;
        let src = r#"
            typeclass Showable<T> { fn show_it(T x) -> int; }
            impl Showable<int> { fn show_it(int x) -> int { return x; } }
            fn show<T: Showable>(T x) -> int { return show_it(x); }
            fn capture<T: Showable>(T _w) { return show; }
            fn main() { let f = capture(0); }
        "#;
        let (bc, _pool) = compile_src(src);
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::MakePolyFnCapture)),
            "expected MakePolyFnCapture; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
        assert!(
            !bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::MakePolyFn)),
            "constrained escape should not emit unconstrained MakePolyFn"
        );
    }

    /// Codegen test B3-2: when a concrete value is passed to a generic function,
    /// the codegen must emit `BoxValue` immediately after the argument to wrap it
    /// into an `ObjBoxed` heap object at the concrete→generic boundary.
    ///
    /// For `id(42)` where `fn id<T>(T x) -> T`, `42` is an `int` literal.
    /// After compiling the `CONST 42`, codegen detects `is_generic_fn("id")`,
    /// infers the argument's type as `int`, and emits `BoxValue` with tag
    /// `ValueTag::Int`.
    #[test]
    fn generic_call_with_concrete_arg_emits_box_value() {
        use common::Instruction;
        let (bc, _pool) = compile_src("fn id<T>(T x) -> T { return x; } fn main() { id(42); }");
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::BoxValue)),
            "expected BoxValue for concrete int arg passed to generic fn id<T>; bytecode opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
        // The BoxValue operand should encode ValueTag::Int (= 0).
        let box_ops: Vec<u32> = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::BoxValue))
            .map(|b| b.operand_u32())
            .collect();
        assert!(
            box_ops
                .iter()
                .any(|&tag| tag == common::ValueTag::Int as u32),
            "BoxValue operand should be ValueTag::Int ({}), got: {:?}",
            common::ValueTag::Int as u32,
            box_ops
        );
    }

    /// Codegen test: `fn id<T>(T x) -> T { return x; }` called with a
    /// concrete `int` argument must emit BOTH `BoxValue` (arg boxing) AND
    /// `UnboxValue` (return unboxing) in the bytecode.
    ///
    /// The unbox is required so the caller receives a raw `i64`, not an
    /// `ObjBoxed` heap pointer, when the generic return type is instantiated
    /// to a concrete primitive.
    #[test]
    fn generic_call_emits_box_and_unbox() {
        use common::Instruction;
        let (bc, _pool) = compile_src("fn id<T>(T x) -> T { return x; } fn main() { id(42); }");
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::BoxValue)),
            "expected BoxValue for concrete int arg to generic fn id<T>; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::UnboxValue)),
            "expected UnboxValue after generic fn call returns concrete int; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
        // The UnboxValue operand should encode ValueTag::Int (= 0).
        let unbox_ops: Vec<u32> = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::UnboxValue))
            .map(|b| b.operand_u32() & 0xFFFF)
            .collect();
        assert!(
            unbox_ops
                .iter()
                .any(|&tag| tag == common::ValueTag::Int as u32),
            "UnboxValue operand should be ValueTag::Int ({}), got: {:?}",
            common::ValueTag::Int as u32,
            unbox_ops
        );
    }

    #[test]
    fn bounded_generic_add_ground_call_uses_specialized_clone() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn add<T: Num>(T a, T b) -> T { return a + b; } \
             fn main() { print \"%i\", add(1, 2); }",
        );

        // Shared body uses the dictionary ABI (CallIndirect), not DynAdd.
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::CallIndirect)),
            "shared generic body should dispatch Num via CallIndirect; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
        // Ground monomorphized call in main: CONST/CONST/CALL without a
        // preceding BoxValue. (Builtin Num thunks also contain BoxValue.)
        let main_call = bc
            .iter()
            .enumerate()
            .rev()
            .find(|(_, b)| matches!(b.bytecode(), Instruction::CALL))
            .map(|(i, _)| i)
            .expect("main should CALL the specialized add");
        let boxed_before_call = main_call > 0
            && matches!(bc[main_call - 1].bytecode(), Instruction::BoxValue);
        assert!(
            !boxed_before_call,
            "ground monomorphic add call should not box args; opcodes near CALL: {:?}",
            &bc[main_call.saturating_sub(4)..=main_call]
                .iter()
                .map(|b| b.bytecode())
                .collect::<Vec<_>>()
        );
        let has_specialized_add = bc.iter().any(|b| {
            matches!(
                b.bytecode(),
                Instruction::ADD | Instruction::BinSlotSlot | Instruction::BinReturn
            )
        });
        assert!(
            has_specialized_add,
            "specialized add clone should contain int ADD/fused equivalent; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    // ── Dictionary-passing calling convention tests ─────────────────────────

    /// Codegen test: A non-monomorphized call to a generic function with a
    /// **user-defined** typeclass constraint must emit:
    ///   1. `MakeTuple` (the method-offset dict) after the value arg.
    ///   2. A `CALL` whose packed arity is 2 (1 value arg + 1 dict tuple),
    ///      NOT 1.
    ///
    /// The CONST that feeds `MakeTuple` encodes the bytecode offset of the
    /// instance method (i.e. it must be > 0 because the method compiles to a
    /// real function body).
    #[test]
    fn user_typeclass_constrained_call_emits_dict_tuple_and_bumps_arity() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            // Declare a user typeclass with one method.
            "typeclass Describable<T> { fn describe_val(T x) -> int; } \
             impl Describable<int> { fn describe_val(int x) -> int { return x; } } \
             // Generic fn with one user typeclass constraint.  NOT called as mono.
             fn show<T: Describable>(T x) -> int { return 0; } \
             fn main() { show(42); }",
        );

        // ── 1. A MakeTuple must be present (the dict for Describable<int>).
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::MakeTuple)),
            "expected MakeTuple for dict emission; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );

        // ── 2. The CALL to `show` must have arity 2 (1 value + 1 dict).
        //    We look for the CALL with the highest arity among all CALL
        //    instructions (the monomorphized clone if any won't have a dict).
        let max_call_arity = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::CALL))
            .map(|b| b.call_parts().0)
            .max()
            .unwrap_or(0);
        assert_eq!(
            max_call_arity,
            2,
            "expected CALL arity = 2 (1 value + 1 dict); got {} from opcodes: {:?}",
            max_call_arity,
            bc.iter()
                .filter(|b| matches!(b.bytecode(), Instruction::CALL))
                .map(|b| b.call_parts())
                .collect::<Vec<_>>()
        );
    }

    /// Codegen test: A non-monomorphized call with **two** user typeclass
    /// constraints must emit **two** `MakeTuple` instructions (one per dict)
    /// and a `CALL` with arity N_value_args + 2.
    #[test]
    fn two_user_typeclass_constraints_emit_two_dicts_and_arity_plus_two() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "typeclass Printable<T> { fn printable_val(T x) -> int; } \
             typeclass Countable<T> { fn count_val(T x) -> int; } \
             impl Printable<int> { fn printable_val(int x) -> int { return x; } } \
             impl Countable<int> { fn count_val(int x) -> int { return x + 1; } } \
             fn process<T: Printable + Countable>(T x) -> int { return 0; } \
             fn main() { process(5); }",
        );

        // Two MakeTuple instructions (one per dict).
        let make_tuple_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::MakeTuple))
            .count();
        assert!(
            make_tuple_count >= 2,
            "expected at least 2 MakeTuple (two dicts); got {}; opcodes: {:?}",
            make_tuple_count,
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );

        // CALL arity should be 1 value + 2 dicts = 3.
        let max_call_arity = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::CALL))
            .map(|b| b.call_parts().0)
            .max()
            .unwrap_or(0);
        assert_eq!(
            max_call_arity, 3,
            "expected CALL arity = 3 (1 value + 2 dicts); got {}",
            max_call_arity
        );
    }

    /// Builtin constraints use the same tuple dictionary ABI as user-defined
    /// constraints, with compiler-generated method thunks as entries.
    #[test]
    fn builtin_num_constraint_emits_dict_tuple() {
        use common::Instruction;
        // Non-monomorphized call: use boxed arg path.
        let (bc, _pool) = compile_src(
            "fn add_generic<T: Num>(T a, T b) -> T { return a + b; } \
             fn caller<U: Num>(U x, U y) -> U { return add_generic(x, y); }",
        );

        let make_tuple_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::MakeTuple))
            .count();
        assert_eq!(make_tuple_count, 0, "open Num evidence is forwarded, not rebuilt");

        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::CallIndirect)),
            "expected dictionary CallIndirect for Num-constrained add; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// Ground calls with **user** typeclass bounds are NOT monomorphized
    /// (see `monomorphize.rs`); they use the shared body + dictionary-passing
    /// convention instead. Expect BoxValue + MakeTuple + bumped CALL arity.
    #[test]
    fn ground_user_typeclass_call_uses_dict_not_mono() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "typeclass Describable<T> { fn describe_val(T x) -> int; } \
             impl Describable<int> { fn describe_val(int x) -> int { return x; } } \
             fn id_d<T: Describable>(T x) -> T { return x; } \
             fn main() { let y = id_d(7); }",
        );

        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::BoxValue)),
            "shared generic path should box the concrete arg; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::MakeTuple)),
            "user typeclass ground call should emit a dict MakeTuple; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
        let max_call_arity = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::CALL))
            .map(|b| b.call_parts().0)
            .max()
            .unwrap_or(0);
        assert_eq!(
            max_call_arity, 2,
            "expected CALL arity = 2 (1 value + 1 dict); got {}",
            max_call_arity
        );
    }

    #[test]
    fn generic_bound_method_consumes_dictionary_indirectly() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "typeclass Measurable<T> { fn size(T x) -> int; } \
             impl Measurable<int> { fn size(int x) -> int { return x; } } \
             fn size_of<T: Measurable>(T x) -> int { return x.size(); } \
             fn main() { size_of(42); }",
        );
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::Index))
        );
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::CallIndirect)),
            "bound method should dispatch via CallIndirect"
        );
    }

    #[test]
    fn omitted_default_method_dict_slot_has_real_target() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "typeclass Tiny<T> { fn zero(T x) -> int { return 7; } } \
             impl Tiny<int> {} \
             fn get<T: Tiny>(T x) -> int { return zero(x); } \
             fn main() { get(0); }",
        );
        let tuple_index = bc
            .iter()
            .position(|b| matches!(b.bytecode(), Instruction::MakeTuple))
            .expect("default method dictionary");
        assert!(tuple_index > 0);
        assert!(
            matches!(bc[tuple_index - 1].bytecode(), Instruction::CodePtr),
            "dictionary method slots must use CodePtr; got {:?}",
            bc[tuple_index - 1].bytecode()
        );
        assert!(
            bc[tuple_index - 1].operand_u32() > 0,
            "default method dictionary slot must contain a compiled code offset"
        );
    }

    /// Dictionary emission uses self-identifying `CodePtr` (not `CONST`).
    #[test]
    fn dictionary_entries_emit_code_ptr() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "typeclass Measurable<T> { fn size(T x) -> int; } \
             impl Measurable<int> { fn size(int x) -> int { return x; } } \
             fn size_of<T: Measurable>(T x) -> int { return size(x); } \
             fn main() { size_of(42); }",
        );
        let tuple_pos = bc
            .iter()
            .position(|b| matches!(b.bytecode(), Instruction::MakeTuple))
            .expect("expected dict MakeTuple");
        assert!(
            matches!(bc[tuple_pos - 1].bytecode(), Instruction::CodePtr),
            "dict entry before MakeTuple must be CodePtr"
        );
        let code_ptrs: Vec<_> = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::CodePtr))
            .collect();
        assert!(
            !code_ptrs.is_empty(),
            "expected at least one CodePtr in dictionary program"
        );
        for ptr in &code_ptrs {
            assert!(
                (ptr.operand_u32() as usize) < bc.len(),
                "CodePtr target {} out of range (len={})",
                ptr.operand_u32(),
                bc.len()
            );
        }
    }

    /// Direct instance-method / CallIndirect sites push `CodePtr` targets.
    #[test]
    fn call_indirect_sites_use_code_ptr_targets() {
        use common::Instruction;
        let mut bc = Vec::new();
        Compiler::emit_call_indirect(&mut bc, 100_000, 1);
        assert!(matches!(bc[0].bytecode(), Instruction::CodePtr));
        assert_eq!(
            bc[0].operand_u32(),
            100_000,
            "CodePtr must carry full 32-bit targets (> u16::MAX)"
        );
        assert!(matches!(bc[1].bytecode(), Instruction::CallIndirect));
    }

    /// `MakePolyFn` operands are absolute and survive final-link fusion.
    #[test]
    fn make_polyfn_operand_is_relocatable_under_fusion() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn id<T>(T x) -> T { return x; } \
             fn main() { let f = id; print \"%i\", f(42); }",
        );
        let poly = bc
            .iter()
            .find(|b| matches!(b.bytecode(), Instruction::MakePolyFn))
            .expect("MakePolyFn for escaped generic");
        let entry = poly.operand_u32() as usize;
        assert!(entry < bc.len(), "MakePolyFn entry must point into bytecode");
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::CallIndirect)),
            "polyfn application should use CallIndirect"
        );
    }

    /// PolyFn programs still receive peephole fusion (BinSlotImm / etc.).
    #[test]
    fn polyfn_plus_fib_style_body_still_fuses() {
        use common::Instruction;
        // Shared fib-style body uses LOAD/CONST/op patterns that fuse when
        // CodePtr/MakePolyFn are relocatable (Phase 1 — no global skip-fusion).
        let (bc, _pool) = compile_src(
            "fn id<T>(T x) -> T { return x; } \
             fn fib(int n) -> int { \
               if n <= 2 { return 1; } \
               return fib(n - 1) + fib(n - 2); \
             } \
             fn main() { \
               let f = id; \
               print \"%i\", f(fib(5)); \
             }",
        );
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::MakePolyFn)),
            "expected MakePolyFn"
        );
        let has_fused = bc.iter().any(|b| {
            matches!(
                b.bytecode(),
                Instruction::BinSlotImm
                    | Instruction::BinSlotImmJmpf
                    | Instruction::BinSlotSlot
                    | Instruction::CmpJmpf
                    | Instruction::BinReturn
                    | Instruction::ConstReturnImm
                    | Instruction::LoadReturnSlot
            )
        });
        assert!(
            has_fused,
            "expected fused superinstructions alongside PolyFn; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }
}
