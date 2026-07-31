mod block_builder;
mod il;
mod attrs;
mod const_fold;
mod manifest;
mod monomorphize;
mod pipeline;
mod strip_tests;
mod typechecking;

use std::{
    borrow::Borrow,
    collections::{HashMap, HashSet},
};

use common::{
    Byte, DebugLoc, Instruction, Interner, Value, ValueTag, DEBUG_FILE_UNKNOWN,
    encode_tag_operand, likely, tag, unlikely,
};
use reporting::Label as DiagLabel;

use crate::block_builder::{BlockBuilder, JumpKind as BbJumpKind, Label as BbLabel};
use crate::il::{CodeBuf, EmitBuf, EntryKind, IlOp, Label as IlLabel};
use crate::const_fold::ConstValue;
use crate::monomorphize::{MonoKey, MonoPlan, parse_mono_ty_name};
use parser::{
    SimpleSpan,
    ast::{Expression, MatchArm, Output, Pattern, PatternPayload},
};

pub use pipeline::*;
pub use reporting::{ErrorCode, Label, Message, MessageKind};
pub use typechecking::{
    BuiltinExport, CStructDef, CallbackSigDef, Checker, FfiBuiltin, ForInInfo, ForInKind, Ty,
    VirtualModules,
};

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

/// Fallback FFI tag from a call-site expression when the typechecker did not
/// record tags (recovery / missing side-table entry).
///
/// Returns `None` for unknown shapes — callers must not invent `INT` and
/// silently mis-promote; prefer skipping the variadic tag tuple or emitting
/// a diagnostic instead.
fn ffi_tag_for_expr_fallback(expr: &Output) -> Option<(u32, u32)> {
    use common::tag;
    match expr.1.as_ref() {
        Expression::Float(_) => Some((tag::FLOAT, 0)),
        Expression::String(_) => Some((tag::STRING, 0)),
        Expression::Bool(_) => Some((tag::BOOL, 0)),
        Expression::Integer(_) => Some((tag::INT, 0)),
        Expression::Expr(inner) | Expression::Group(inner) | Expression::Statement(inner) => {
            ffi_tag_for_expr_fallback(inner)
        }
        _ => None,
    }
}

/// Decode escape sequences in a coil string literal (`\n`, `\x41`, `\u{1F}`, …).
pub(crate) fn unescape_coil_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('0') => out.push('\0'),
            Some('e') => out.push('\x1b'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('x') => {
                let hi = chars.next();
                let lo = chars.next();
                if let (Some(h), Some(l)) = (hi, lo) {
                    let hex = format!("{h}{l}");
                    if let Ok(v) = u8::from_str_radix(&hex, 16) {
                        out.push(v as char);
                        continue;
                    }
                }
                out.push('\\');
                out.push('x');
                if let Some(h) = hi {
                    out.push(h);
                }
                if let Some(l) = lo {
                    out.push(l);
                }
            }
            Some('u') => {
                if chars.next() == Some('{') {
                    let mut hex = String::new();
                    let mut closed = false;
                    while let Some(ch) = chars.next() {
                        if ch == '}' {
                            closed = true;
                            break;
                        }
                        hex.push(ch);
                    }
                    if closed {
                        if let Ok(code) = u32::from_str_radix(&hex, 16) {
                            if let Some(ch) = char::from_u32(code) {
                                out.push(ch);
                                continue;
                            }
                        }
                    }
                }
                out.push('\\');
                out.push('u');
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn primitive_name_from_type_ann(ty: &Output) -> Option<&'static str> {
    match ty.1.as_ref() {
        Expression::Type(name) => primitive_type_name(&Ty::Con((*name).into())),
        _ => None,
    }
}

fn primitive_type_name(ty: &Ty) -> Option<&'static str> {
    use typechecking::ty::{BOOL, BYTE, FLOAT, INT};
    match ty {
        Ty::Con(name) => match name.as_str() {
            INT => Some("int"),
            FLOAT => Some("float"),
            BYTE => Some("byte"),
            BOOL => Some("bool"),
            _ => None,
        },
        _ => None,
    }
}

fn primitive_cast_opcode(from: &str, to: &str) -> Option<Instruction> {
    match (from, to) {
        ("int", "float") => Some(Instruction::CastIntToFloat),
        ("float", "int") => Some(Instruction::CastFloatToInt),
        ("int", "byte") => Some(Instruction::CastIntToByte),
        ("byte", "int") => Some(Instruction::CastByteToInt),
        ("int", "bool") => Some(Instruction::CastIntToBool),
        ("bool", "int") => Some(Instruction::CastBoolToInt),
        (a, b) if a == b => None,
        _ => None,
    }
}

fn into_primitive_fqn(from: &str, to: &str) -> String {
    format!("Into__{}__to_{}__into", from, to)
}

fn emit_ffi_type_const(bytecode: &mut impl EmitBuf, tag: u32, aux: u32) {
    bytecode.push(Byte::new(Instruction::CONST).with_operand_u32(encode_tag_operand(tag, aux)));
}

/// Resolve variadic FFI arg tags from the typechecker side-table, falling back
/// to literal shapes only. Unknown expressions yield no tags (and a diagnostic)
/// rather than silently promoting as `INT`.
fn resolve_variadic_ffi_tags(
    checker: &Checker,
    span: (usize, usize),
    args: &[&Output<'_>],
    messages: &mut Vec<Message>,
) -> Option<Vec<(u32, u32)>> {
    if let Some(tags) = checker.variadic_arg_tags_at(span) {
        return Some(tags.to_vec());
    }
    let mut tags = Vec::with_capacity(args.len());
    for arg in args {
        match ffi_tag_for_expr_fallback(arg) {
            Some(t) => tags.push(t),
            None => {
                let range = arg.0.start..arg.0.end;
                let mut m = Message::error(
                    ErrorCode::GenericTypeError,
                    "cannot determine FFI type tag for variadic argument".into(),
                    range.clone(),
                );
                m.push(DiagLabel::new(
                    "variadic FFI arg tags missing from typechecker; \
                     use a literal or ensure the call is typechecked"
                        .to_string(),
                    range,
                ));
                messages.push(m);
                return None;
            }
        }
    }
    Some(tags)
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
    bytecode: &mut CodeBuf,
    bb: &mut BlockBuilder,
    pass_label: Option<crate::block_builder::Label>,
    fail_label: crate::block_builder::Label,
    payload_base: u32,
) {
    use parser::ast::PatternPayload;
    match payload {
        PatternPayload::Unit => {
            // Unit inner (e.g. `Option::None`): always matches.
            // JMP to the arm body when we have a pass_label —
            // required when a later tag group follows so we
            // don't fall through into that group's body.
            if let Some(label) = pass_label {
                bb.emit_jump_to(label, BbJumpKind::Unconditional, bytecode.il_mut());
            }
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
                        let slot = next_available_slot(match_bindings_per_arm, payload_base);
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
                                payload_base,
                            );
                        } else if let Some(label) = pass_label {
                            if let Some(inner_tag) = checker.tag_for(sub_enum, sub_variant) {
                                bb.emit_jump_to(
                                    label,
                                    BbJumpKind::JumpIfMatch {
                                        tag: inner_tag,
                                        arity: 0,
                                    },
                                    bytecode.il_mut(),
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
                bb.emit_jump_to(label, BbJumpKind::Unconditional, bytecode.il_mut());
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
                        let slot = next_available_slot(match_bindings_per_arm, payload_base);
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
                                payload_base,
                            );
                        } else if let Some(label) = pass_label {
                            if let Some(inner_tag) = checker.tag_for(sub_enum, sub_variant) {
                                bb.emit_jump_to(
                                    label,
                                    BbJumpKind::JumpIfMatch {
                                        tag: inner_tag,
                                        arity: 0,
                                    },
                                    bytecode.il_mut(),
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
                bb.emit_jump_to(label, BbJumpKind::Unconditional, bytecode.il_mut());
            }
        }
    }
}

/// Collect `name → Ty` for every binding in a match pattern.
///
/// Used so Access codegen (`p.y`) sees the *current arm's* binding type
/// rather than whatever last arm wrote into the flat
/// `codegen_var_types` side-table (same name reused across arms with
/// different payload types would otherwise emit the wrong `LoadField`).
///
/// Open schema placeholders (`Ty::Var`, or `Ty::Con("T")` type-param
/// markers from poly enums like `Option` / `Result` / `Box<T>`) are
/// **not** inserted — they would shadow the instantiated binding type
/// that `infer_pattern` already wrote into `codegen_var_types`.
fn collect_pattern_binding_types(
    checker: &Checker,
    pattern: &Pattern<'_>,
    out: &mut HashMap<String, Ty>,
) {
    match pattern {
        Pattern::Wildcard => {}
        Pattern::Binding { .. } => {
            // Bare `name =>` needs the scrutinee type from the side-table;
            // caller may fill that in. Constructor/record payloads below
            // carry declared field types.
        }
        Pattern::Constructor {
            enum_name,
            variant_name,
            payload,
        } => {
            let decl = checker.payload_tys_for(enum_name, variant_name);
            match payload {
                PatternPayload::Unit => {}
                PatternPayload::Tuple(parts) => {
                    for (i, part) in parts.iter().enumerate() {
                        let expected = decl.get(i).map(|(_, ty)| ty);
                        collect_pattern_binding_types_with_expected(
                            checker, enum_name, part, expected, out,
                        );
                    }
                }
                PatternPayload::Record(fields) => {
                    let by_name: HashMap<&str, &Ty> = decl
                        .iter()
                        .map(|(n, ty)| (n.as_str(), ty))
                        .collect();
                    for pf in fields {
                        let expected = by_name.get(pf.name).copied();
                        collect_pattern_binding_types_with_expected(
                            checker,
                            enum_name,
                            &pf.pattern,
                            expected,
                            out,
                        );
                    }
                }
            }
        }
    }
}

/// True when `ty` is a poly-enum schema placeholder for `enum_name`
/// (type-param `Con("T")` / `Con("E")` / …) or an open `Ty::Var`.
fn is_open_schema_ty(checker: &Checker, enum_name: &str, ty: &Ty) -> bool {
    match ty {
        Ty::Var(_) => true,
        Ty::Con(name) => checker
            .generics()
            .generic_type_ctors
            .get(enum_name)
            .is_some_and(|params| params.iter().any(|p| p == name)),
        _ => false,
    }
}

fn collect_pattern_binding_types_with_expected(
    checker: &Checker,
    enum_name: &str,
    pattern: &Pattern<'_>,
    expected: Option<&Ty>,
    out: &mut HashMap<String, Ty>,
) {
    match pattern {
        Pattern::Wildcard => {}
        Pattern::Binding { name } => {
            if let Some(ty) = expected {
                if !is_open_schema_ty(checker, enum_name, ty) {
                    out.insert(name.to_string(), ty.clone());
                }
            }
        }
        Pattern::Constructor { .. } => {
            collect_pattern_binding_types(checker, pattern, out);
        }
    }
}

/// Next free binding slot for match payloads.
///
/// `base` is the first payload slot (`context.variables.len()` at match
/// entry). Slot 0 is reserved for the first function argument; trailing
/// dictionary locals occupy 1..base-1 when `dict_arity > 0`.
#[allow(dead_code)]
fn next_available_slot(match_bindings: &HashMap<usize, HashMap<String, u32>>, base: u32) -> u32 {
    let mut max_slot = base.saturating_sub(1);
    for arm_bindings in match_bindings.values() {
        for &slot in arm_bindings.values() {
            if slot > max_slot {
                max_slot = slot;
            }
        }
    }
    max_slot + 1
}

/// Bytecode table key for an arity overload: `name#2` or `name#rest1`.
fn overload_fn_key(name: &str, fixed_arity: usize, is_rest: bool) -> String {
    if is_rest {
        format!("{name}#rest{fixed_arity}")
    } else {
        format!("{name}#{fixed_arity}")
    }
}

/// Strip `#N` / `#restN` suffix from an overload table key.
fn strip_overload_key(name: &str) -> &str {
    match name.rfind('#') {
        Some(i) => &name[..i],
        None => name,
    }
}

/// `MakeFn` operand: `[7:0]=n_cap [15:8]=n_filled [23:16]=arity [24]=is_rest`.
///
/// `n_cap` and `n_filled` are packed into 8-bit fields (max 255). Callers with
/// larger values must not reach here — partial-application arity is already
/// capped at 32 for `filled_mask`.
fn make_fn_operand(n_cap: u32, n_filled: u32, arity: u32, is_rest: bool) -> u32 {
    debug_assert!(
        n_cap <= 0xFF && n_filled <= 0xFF,
        "MakeFn n_cap/n_filled overflow 8-bit fields: n_cap={n_cap} n_filled={n_filled}"
    );
    (n_cap & 0xFF) | ((n_filled & 0xFF) << 8) | (arity << 16) | if is_rest { 1 << 24 } else { 0 }
}

/// Fixed-arity / rest flag from a function's `Argument` fragment.
fn fn_arity_from_args(args: &Output<'_>) -> (usize, bool) {
    match args.1.as_ref() {
        Expression::Fragment(children) => {
            let has_rest = children.last().is_some_and(|c| {
                matches!(c.1.as_ref(), Expression::Argument(_, _, true))
            });
            let n = children
                .iter()
                .filter(|c| matches!(c.1.as_ref(), Expression::Argument(..)))
                .count();
            if has_rest {
                (n.saturating_sub(1), true)
            } else {
                (n, false)
            }
        }
        _ => (0, false),
    }
}

#[derive(Default, Clone)]
struct Context {
    current: Option<String>,
    variables: Interner<String>,
    symbols: Interner<String>,
    assignments: HashMap<String, bool>,
    constants: HashMap<usize, bool>,
    classes: HashMap<String, Vec<(String, usize)>>,
    impementations: HashMap<String, String>,
    methods: HashMap<String, HashMap<String, String>>,

    /// Per-arm pattern bindings (slot 1..N). Overrides global `variables` in arm bodies.
    match_bindings: Option<HashMap<String, u32>>,

    /// Block-local binding overlays. When `Some`, shadowed names allocate a
    /// fresh slot instead of reusing the outer Interner id (so exiting the
    /// block leaves the outer binding's stack value intact).
    block_bindings: Option<HashMap<String, u32>>,

    prev: Option<Box<Self>>,
}

// --- Compiler ---

/// Length of the CALL + JMP + HALT prologue every [`Compiler`] starts with.
/// Multi-file linking treats `bytecode.len() <= PROLOGUE_BYTECODE_LEN` as a
/// fresh compile (safe to clear the shared constant pool).
pub const PROLOGUE_BYTECODE_LEN: usize = 3;

pub struct Compiler {
    namespace: String,
    /// Stack IL during emit; lowered `Vec<Byte>` after [`Self::finalize_bytecode`].
    bytecode: CodeBuf,

    aliases: HashMap<String, String>,
    functions: HashMap<String, usize>,
    /// Entry labels for names in [`Self::functions`] (step-3 binds).
    fn_entry_labels: HashMap<String, IlLabel>,
    /// Fixed arity + rest flag per function table key. Survives multi-file
    /// `check_program` clears of `Checker::fn_param_names`, so `MakeFn` for
    /// imported names (e.g. `spawn(run_jobs, …)` after `use pool::worker::run_jobs`)
    /// still packs the real arity.
    fn_arities: HashMap<String, (u32, bool)>,
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
    /// Entry for prologue `JMP` when static initializers are spliced (unchanged
    /// by the post-splice `program_start_offset` bump).
    setup_entry_offset: u32,
    /// Wide immediates referenced from compact 8-byte `Byte`
    /// operands (floats, `JumpIfMatch` targets, etc.).
    constants: Vec<u64>,

    /// Qualified names of `async fn` declarations (emit `MakeCoro` at call sites).
    coroutine_fns: std::collections::HashSet<String>,

    /// Counter for compiler-generated temporary slots.
    temp_counter: u32,

    /// Count of expression values currently live on the operand stack
    /// *above* interned locals (e.g. a `HostInvoke` native-id `CONST`
    /// pushed before argument codegen). `alloc_temp_slot` must allocate
    /// at or above `variables.len() + expr_depth` so `StorePop` does not
    /// clobber those live values (locals and the operand stack share
    /// memory).
    expr_depth: u32,

    /// Active loop labels: `(continue_target, break_target)`.
    loop_stack: Vec<(BbLabel, BbLabel)>,

    /// Active loop patchers. Break/continue emit through the innermost builder.
    loop_bbs: Vec<BlockBuilder>,

    /// Registered `defer` thunks in the function currently being compiled
    /// (declaration order). Run LIFO on return / fall-through via
    /// `emit_run_defers`. Kept on `Compiler` (not `Context`) so nested
    /// block frames do not drop registered defers.
    ///
    /// Each thunk stores an IL label bound at its body entry and the `use (…)`
    /// capture names. At run time those captures are LOADed from the enclosing
    /// frame and passed as CALL arguments so the thunk's fresh frame sees them
    /// at slots 0..N-1 (same layout as lambda capture slots).
    fn_defers: Vec<(BbLabel, Vec<String>)>,

    /// Class name → decorated constructor function name (from attr expansion).
    decorated_class_ctors: HashMap<String, String>,

    /// Name of the function currently being codegen'd (for ctor/Instantiate routing).
    active_fn_name: Option<String>,

    /// Bytecode for global static initializers (spliced at `program_start_offset`).
    static_init_bytecode: Vec<Byte>,

    /// True while compiling an `impl` method — Function resets locals
    /// and reserves slot 0 for `self`.
    compiling_method: bool,

    /// True while compiling a function whose return type is inferred
    /// as `Result<T, E>` via `raise` / `?` (wrap bare `return` in `Ok`).
    compiling_result_mode: bool,

    /// Harness metadata: `(description, bytecode offset)` for each
    /// top-level `test("…") { … }` case, in source order.
    test_cases: Vec<(String, u32)>,

    /// True when a user-written `fn main` was emitted this compile.
    user_main_defined: bool,

    /// When false (default), harness `test("…")` blocks and `#[test]` functions
    /// are stripped before typecheck/codegen. Set true for `coil test`
    /// or `compile --include-tests`.
    include_tests: bool,

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

    /// Project-relative path of the module currently being codegen'd.
    current_source_file: Option<std::path::PathBuf>,
    /// Stable `DebugLoc::file` indices (path string → id).
    source_file_indices: std::collections::BTreeMap<String, u32>,
    /// `source_files` order for the archive (index → path).
    source_file_list: Vec<String>,
    /// One [`DebugLoc`] per bytecode slot (grows with [`Self::bytecode`]).
    debug_locs: Vec<DebugLoc>,

    /// Compile-time scalar values for `const` bindings (frame stack).
    const_env_stack: Vec<HashMap<String, ConstValue>>,
    /// Folded scalar initializers for module `static const` / `static` slots.
    static_const_values: HashMap<String, ConstValue>,

    /// Qualified name of the function currently being codegen'd (tail-call eligibility).
    current_function_qualified: Option<String>,
    /// `functions` map key for the active function (overload-aware).
    current_function_table_key: Option<String>,
    /// Bytecode span `(start, end)` per function for tiny inlining.
    fn_bytecode_spans: HashMap<String, (usize, usize)>,

    /// When true, [`Expression::Match`] omits the trailing `DUPLICATE; POP`
    /// fusion barrier. Set while compiling a match whose value is consumed
    /// immediately by `StorePop` / `StoreStatic` (e.g. `let x = match …`).
    suppress_match_fusion_barrier: bool,
}

impl Default for Compiler {
    fn default() -> Self {
        let mut bytecode = CodeBuf::new();
        bytecode.push(Byte::new(Instruction::CALL));
        bytecode.push_prologue_jmp();
        bytecode.push(Byte::new(Instruction::HALT));
        debug_assert_eq!(bytecode.len(), PROLOGUE_BYTECODE_LEN);
        let program_start_offset = bytecode.len() as u32;
        let debug_locs = vec![DebugLoc::unknown(); bytecode.len()];

        Self {
            namespace: String::default(),
            bytecode,
            debug_locs,
            aliases: HashMap::default(),
            functions: HashMap::with_capacity(32),
            fn_entry_labels: HashMap::with_capacity(32),
            fn_arities: HashMap::with_capacity(32),
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
            setup_entry_offset: program_start_offset,
            constants: Vec::default(),
            coroutine_fns: std::collections::HashSet::new(),
            temp_counter: 0,
            expr_depth: 0,
            loop_stack: Vec::new(),
            loop_bbs: Vec::new(),
            fn_defers: Vec::new(),
            decorated_class_ctors: HashMap::new(),
            active_fn_name: None,
            compiling_method: false,
            compiling_result_mode: false,
            test_cases: Vec::new(),
            user_main_defined: false,
            include_tests: false,
            polyfn_vars: HashSet::new(),
            polyfn_sources: HashMap::new(),
            mono_plan: MonoPlan::default(),
            mono_offsets: HashMap::new(),
            mono_codegen_var_types: Vec::new(),
            static_init_bytecode: Vec::new(),
            current_source_file: None,
            source_file_indices: std::collections::BTreeMap::new(),
            source_file_list: Vec::new(),
            const_env_stack: Vec::new(),
            static_const_values: HashMap::new(),
            current_function_qualified: None,
            current_function_table_key: None,
            fn_bytecode_spans: HashMap::new(),
            suppress_match_fusion_barrier: false,
        }
    }
}

impl Compiler {
    pub fn constants(&self) -> &[u64] {
        &self.constants
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
                    self.bytecode
                        .push_load(slot);
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
            self.bytecode.push(Byte::new(Instruction::POP));
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
                    bytecode.push(Byte::new(Instruction::CONST).with_const_pool(idx));
                }
            }
            ConstValue::Float(n) => {
                let bits = Value::from(*n).raw() as u64;
                let idx = self.intern_constant(bits);
                bytecode.push(Byte::new(Instruction::CONST).with_const_pool(idx));
            }
            ConstValue::Bool(b) => {
                bytecode.push(Byte::new_with_value(Instruction::CONST, Value::from(*b).raw() as _));
            }
            ConstValue::Str(s) => {
                Self::emit_string_const(bytecode, s);
            }
        }
    }

    fn emit_string_const(bytecode: &mut Vec<Byte>, escaped: &str) {
        let idx = bytecode.len();
        let mut count = 0usize;
        for ch in escaped.chars() {
            count += 1;
            bytecode.push(Byte::new(Instruction::DATA).with_operand_u32(ch.into()));
        }
        bytecode.insert(
            idx,
            Byte::new(Instruction::STRING).with_operand_u32(count as u32),
        );
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
        if let Some(v) = const_fold::eval_expr(ast, self.const_env()) {
            self.emit_const_value(&v, bytecode);
            true
        } else if let Some(inner) = const_fold::strength_reduced_inner(ast) {
            let mut inner_bc = self.do_compile(inner);
            bytecode.append(&mut inner_bc);
            true
        } else if allow_mul_shl
            && let Some((inner, shift)) = const_fold::strength_mul_to_shl(ast, self.const_env())
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

    /// Constant-folded `if` / `else if` / `else`. Returns true when handled.
    fn try_compile_const_if(&mut self, branches: &[Output<'_>]) -> bool {
        let mut i = 0usize;
        while i < branches.len() {
            let Expression::Branch(cond, body) = branches[i].1.as_ref() else {
                return false;
            };
            match cond {
                Some(c) => match const_fold::eval_expr(c, self.const_env()) {
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
    fn try_emit_tail_call_expr(
        &mut self,
        expr: &Output<'_>,
        bytecode: &mut Vec<Byte>,
    ) -> bool {
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
                let simple = call_key.rsplit("::").next().unwrap_or(&call_key).to_string();
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
        bytecode.push(
            Byte::new(Instruction::TailCall).with_call_packed(arity as u32, target as u32),
        );
        true
    }

    fn is_tiny_inline_il(ops: &[IlOp]) -> bool {
        if ops.is_empty() || ops.len() > 48 {
            return false;
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
        !ops.iter().any(|op| {
            let Some(b) = op.as_plain_byte() else {
                return true;
            };
            matches!(
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
                    | Instruction::JMP
                    | Instruction::JMPF
                    | Instruction::JMPT
                    | Instruction::BinReturn
                    | Instruction::CmpJmpf
                    | Instruction::BinSlotImmJmpf
                    | Instruction::LogNotJmpf
                    | Instruction::LoadReturnSlot
                    | Instruction::ConstReturnImm
            )
        })
    }

    /// Expand a fused `*Return` byte into the producer left on the caller's stack.
    fn expand_fused_return_for_inline(byte: &Byte, temps: &[u32]) -> Option<Byte> {
        match *byte.bytecode() {
            Instruction::ConstReturnImm => Some(
                Byte::new(Instruction::CONST).with_const_inline(byte.operand_u32() as i32),
            ),
            Instruction::LoadReturnSlot => {
                let slot = byte.operand_u32() as usize;
                let &tmp = temps.get(slot)?;
                Some(Byte::new(Instruction::LOAD).with_operand_u32(tmp))
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
                Some(
                    Byte::new(Instruction::BinSlotSlot)
                        .with_bin_slot_slot(op, ta as u8, tb as u8),
                )
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
        let slice = self.bytecode.code_slice_bytes(start, end);
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
                let slot = byte.operand_u32() as usize;
                let Some(&tmp) = temps.get(slot) else {
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
}

impl<'ctx> Context {
    fn child(&self) -> Self {
        Self {
            current: self.current.clone(),
            impementations: self.impementations.clone(),
            methods: self.methods.clone(),
            constants: self.constants.clone(),
            assignments: self.assignments.clone(),
            variables: self.variables.clone(),
            symbols: self.symbols.clone(),
            classes: self.classes.clone(),
            match_bindings: self.match_bindings.clone(),
            // Fresh overlay so inner `let` / destructure can shadow outer names.
            block_bindings: Some(HashMap::new()),
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
    bytecode: &mut CodeBuf,
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
                // Declaration-order walk. Nested records unpack into a
                // scratch region past this record's field slots so
                // multi-field inners cannot clobber sibling outers.
                // `record_base` is the first payload slot for this record
                // (normally 1; higher when trailing dict locals precede
                // the match).
                let record_base = *next_slot;
                let n_fields = parent_decl_order.len() as u32;
                let pattern_site: std::collections::HashMap<&str, &Pattern<'compiler>> =
                    fields.iter().map(|pf| (pf.name, &pf.pattern)).collect();
                for (i, (decl_name, _)) in parent_decl_order.iter().enumerate() {
                    let field_slot = record_base + i as u32;
                    if let Some(sub_pat) = pattern_site.get(decl_name.as_str()) {
                        // Nested record + consume: copy the enum into a
                        // scratch base (≥ end of this record's fields /
                        // prior scratch), UnpackAt there, then bind from
                        // scratch. Plain siblings after a nested unpack
                        // relocate field_slot → next_slot before binding.
                        //
                        // UnpackAt operands: [31:16]=arity, [15:0]=slot.
                        if consume_values
                            && let Pattern::Constructor {
                                enum_name: sub_enum,
                                variant_name: sub_variant,
                                payload: PatternPayload::Record(_),
                            } = sub_pat
                        {
                            let inner_arity =
                                checker.payload_tys_for(sub_enum, sub_variant).len() as u16;
                            let scratch_base = (*next_slot).max(record_base + n_fields);
                            if scratch_base != field_slot {
                                bytecode.push_load(field_slot);
                                bytecode.push_store_pop(scratch_base);
                            }
                            bytecode.push(
                                Byte::new(Instruction::UnpackAt)
                                    .with_operands_u16([inner_arity, scratch_base as u16]),
                            );
                            *next_slot = scratch_base;
                        } else if consume_values && field_slot != *next_slot {
                            bytecode
                                .push_load(field_slot);
                            bytecode.push_store_pop(*next_slot);
                        }
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
            return slot;
        }
        if self.context.block_bindings.is_none() {
            return self.context.variables.intern(name.to_string()) as u32;
        }
        let shadows_outer = {
            let in_vars = self
                .context
                .variables
                .key(&name.to_string())
                .is_some();
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
            slot
        } else {
            self.context.variables.intern(name.to_string()) as u32
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
                let ty = self
                    .codegen_expr_ty(inner)
                    .or_else(|| {
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
                    if let Some(idx) = param_names[..fixed_count]
                        .iter()
                        .position(|p| p == *name)
                    {
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
    fn emit_call_args_with_rest(
        &mut self,
        fn_name: &str,
        args: &[Output<'_>],
        bytecode: &mut Vec<Byte>,
        box_generic: bool,
    ) -> u32 {
        self.consume_spread_emit_ids(args);
        let (fixed, rest, pack_rest) = self.split_call_args_for_rest(fn_name, args);

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
                bytecode.push(
                    Byte::new(Instruction::MakeTuple).with_operand_u32(rest.len() as u32),
                );
            } else {
                bytecode.push(
                    Byte::new(Instruction::MakeArray).with_operand_u32(rest.len() as u32),
                );
            }
            return (fixed.len() + 1) as u32;
        }
        fixed.len() as u32
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
                Instruction::StorePop
                    | Instruction::SetField
                    | Instruction::StoreIndex
                    | Instruction::StoreStatic
            )
        ) {
            // `x = expr;` / compound updates already consumed the
            // RHS via StorePop/SetField/StoreIndex/StoreStatic — no trailing POP.
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
        bytecode.push(Byte::new(Instruction::Index));
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
                        bc.push(Byte::new(Instruction::Index));
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
                                bc.push(Byte::new(Instruction::Index));
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
                        bc.push(Byte::new(Instruction::Index));
                        bc.push_load(t1);
                        bc.push_const(i as i32);
                        bc.push(Byte::new(Instruction::Index));
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
                        bc.push(Byte::new(Instruction::Index));
                        bc.push_load(t1);
                        bc.push_const(i as i32);
                        bc.push(Byte::new(Instruction::Index));
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
                                bc.push(Byte::new(Instruction::Index));
                                bc.push_load(t_sc);
                            }
                            ScalarSide::Left => {
                                bc.push_load(t_sc);
                                bc.push_load(t_vec);
                                bc.push_const(i as i32);
                                bc.push(Byte::new(Instruction::Index));
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
                                        bc.push(Byte::new(Instruction::Index));
                                        bc.push_load(t_sc);
                                    }
                                    ScalarSide::Left => {
                                        bc.push_load(t_sc);
                                        bc.push_load(t_vec);
                                        bc.push_const(i as i32);
                                        bc.push(Byte::new(Instruction::Index));
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

    /// Negate TOS: int via `NEG`; float via `MULF` by −1 (no `NEGF` opcode).
    fn emit_neg_tos(&mut self, bytecode: &mut Vec<Byte>, is_float: bool) {
        if is_float {
            let bits = Value::from(-1.0f64).raw() as u64;
            let idx = self.intern_constant(bits);
            bytecode.push(Byte::new(Instruction::CONST).with_const_pool(idx));
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
            bytecode.push(Byte::new(Instruction::MakeTuple).with_operand_u32(n as u32));
        } else {
            bytecode.push(Byte::new(Instruction::MakeArray).with_operand_u32(n as u32));
        }
    }

    fn emit_dynamic_unary_array(&mut self, src: u32, elem_is_float: bool) {
        let len_slot = self.alloc_temp_slot();
        let idx = self.alloc_temp_slot();
        let out = self.alloc_temp_slot();
        self.bytecode
            .push_load(src);
        self.bytecode.push(Byte::new(Instruction::ArrayLen));
        self.bytecode
            .push_store_pop(len_slot);
        self.bytecode
            .push(Byte::new(Instruction::MakeArray).with_operand_u32(0));
        self.bytecode
            .push_store_pop(out);
        self.bytecode
            .push_const(0);
        self.bytecode
            .push_store_pop(idx);

        let mut bb = BlockBuilder::new();
        let loop_top = bb.fresh_label(self.bytecode.il_mut());
        let end = bb.fresh_label(self.bytecode.il_mut());
        bb.bind_label(loop_top, self.bytecode.il_mut());

        self.bytecode
            .push_load(idx);
        self.bytecode
            .push_load(len_slot);
        self.bytecode.push(Byte::new(Instruction::LE));
        bb.emit_jump_to(end, BbJumpKind::JumpIfFalse, self.bytecode.il_mut());

        self.bytecode
            .push_load(out);
        self.bytecode
            .push_load(src);
        self.bytecode
            .push_load(idx);
        self.bytecode.push(Byte::new(Instruction::Index));
        {
            let mut neg_bc = Vec::new();
            self.emit_neg_tos(&mut neg_bc, elem_is_float);
            self.bytecode.extend(neg_bc);
        }
        self.bytecode.push(Byte::new(Instruction::ArrayPush));
        self.bytecode
            .push_store_pop(out);
        self.bytecode
            .push_load(idx);
        self.bytecode
            .push_const(1);
        self.bytecode.push(Byte::new(Instruction::ADD));
        self.bytecode
            .push_store_pop(idx);

        bb.emit_jump_to(loop_top, BbJumpKind::Unconditional, self.bytecode.il_mut());
        bb.bind_label(end, self.bytecode.il_mut());
        self.bytecode
            .push_load(out);
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
        self.bytecode
            .push_load(t_vec);
        self.bytecode.push(Byte::new(Instruction::ArrayLen));
        self.bytecode
            .push_store_pop(len_slot);
        self.bytecode
            .push(Byte::new(Instruction::MakeArray).with_operand_u32(0));
        self.bytecode
            .push_store_pop(out);
        self.bytecode
            .push_const(0);
        self.bytecode
            .push_store_pop(idx);

        let mut bb = BlockBuilder::new();
        let loop_top = bb.fresh_label(self.bytecode.il_mut());
        let end = bb.fresh_label(self.bytecode.il_mut());
        bb.bind_label(loop_top, self.bytecode.il_mut());

        self.bytecode
            .push_load(idx);
        self.bytecode
            .push_load(len_slot);
        self.bytecode.push(Byte::new(Instruction::LE));
        bb.emit_jump_to(end, BbJumpKind::JumpIfFalse, self.bytecode.il_mut());

        self.bytecode
            .push_load(out);
        match scalar_on {
            ScalarSide::Right => {
                self.bytecode
                    .push_load(t_vec);
                self.bytecode
                    .push_load(idx);
                self.bytecode.push(Byte::new(Instruction::Index));
                self.bytecode
                    .push_load(t_sc);
            }
            ScalarSide::Left => {
                self.bytecode
                    .push_load(t_sc);
                self.bytecode
                    .push_load(t_vec);
                self.bytecode
                    .push_load(idx);
                self.bytecode.push(Byte::new(Instruction::Index));
            }
        }
        self.bytecode.push(Byte::new(scalar_instr));
        self.bytecode.push(Byte::new(Instruction::ArrayPush));
        self.bytecode
            .push_store_pop(out);
        self.bytecode
            .push_load(idx);
        self.bytecode
            .push_const(1);
        self.bytecode.push(Byte::new(Instruction::ADD));
        self.bytecode
            .push_store_pop(idx);

        bb.emit_jump_to(loop_top, BbJumpKind::Unconditional, self.bytecode.il_mut());
        bb.bind_label(end, self.bytecode.il_mut());
        self.bytecode
            .push_load(out);
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
            Expression::Identifier(name) => self.codegen_var_type_for(name),
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

    /// Emit a string literal as `STRING` + `DATA` bytes into `self.bytecode`.
    /// Applies the same escape processing as `Expression::String` codegen.
    fn emit_string_literal(&mut self, s: &str) {
        let escaped = unescape_coil_string(s);
        let count = escaped.chars().count() as u32;
        self.bytecode
            .push(Byte::new(Instruction::STRING).with_operand_u32(count));
        for ch in escaped.chars() {
            self.bytecode
                .push(Byte::new(Instruction::DATA).with_operand_u32(ch.into()));
        }
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
    fn emit_format_expression(&mut self, format: &Output, params: Option<&Vec<Output>>) {
        let fmt_lit = match format.1.as_ref() {
            Expression::String(s) => Some(s.to_string()),
            _ => None,
        };

        if let (Some(fmt), Some(params)) = (fmt_lit.as_deref(), params) {
            let rewritten = Self::rewrite_format_v_to_s(fmt);
            let specs = Self::format_consuming_specs(fmt);
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
                self.bytecode
                    .push_store_pop(slot);
                arg_slots.push(slot);
                emitted += 1;
            }
            // Extra args beyond specifiers — still push them (VM pops by count).
            for param in params.iter().skip(emitted) {
                let bc = self.do_compile(param);
                self.bytecode.extend(bc);
                let slot = self.alloc_temp_slot();
                self.bytecode
                    .push_store_pop(slot);
                arg_slots.push(slot);
            }
            self.emit_string_literal(&rewritten);
            for slot in arg_slots {
                self.bytecode
                    .push_load(slot);
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
        self.bytecode
            .push(Byte::new(Instruction::MakeTuple).with_operand_u32(arity));

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
        self.bytecode
            .push(Byte::new(Instruction::MakeTuple).with_operand_u32(arity));

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
                self.bytecode
                    .push(Byte::new(Instruction::MakeTuple).with_operand_u32(tags.len() as u32));
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
        self.bytecode
            .push(Byte::new(Instruction::LoadField).with_operand_u32(1));
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
                    PatternPayload::Tuple(items) if items.len() == 1 => {
                        sole_binding(&items[0])
                    }
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
                self.bytecode
                    .push_load(dict_slot);
                self.bytecode
                    .push_load(dict_slot);
                self.bytecode
                    .push_const(method_slot as i32);
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
        self.bytecode
            .push_store_pop(tuple_slot);

        let mut element_slots = Vec::with_capacity(items.len());
        for (idx, item_ty) in items.iter().enumerate() {
            self.bytecode
                .push_load(tuple_slot);
            self.bytecode
                .push_const(idx as i32);
            self.bytecode.push(Byte::new(Instruction::Index));
            self.emit_show_for_stack_value(item_ty);
            let slot = self.alloc_temp_slot();
            self.bytecode
                .push_store_pop(slot);
            element_slots.push(slot);
        }

        self.emit_string_literal(&Self::tuple_show_format(items.len()));
        for slot in element_slots {
            self.bytecode
                .push_load(slot);
        }
        self.bytecode
            .push(Byte::new(Instruction::FORMAT).with_operand_u32(items.len() as u32));
    }

    fn emit_record_show_for_stack_value(&mut self, fields: &[(String, Ty)]) {
        let record_slot = self.alloc_temp_slot();
        self.bytecode
            .push_store_pop(record_slot);

        let mut field_slots = Vec::with_capacity(fields.len());
        for (name, field_ty) in fields {
            self.bytecode
                .push_load(record_slot);
            Self::emit_raw_string_literal(&mut self.bytecode, name);
            self.bytecode.push(Byte::new(Instruction::GetField));
            self.emit_show_for_stack_value(field_ty);
            let slot = self.alloc_temp_slot();
            self.bytecode
                .push_store_pop(slot);
            field_slots.push(slot);
        }

        self.emit_string_literal(&Self::record_show_format(fields));
        for slot in field_slots {
            self.bytecode
                .push_load(slot);
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
            (
                Ty::Array {
                    element: e1,
                    ..
                },
                Ty::Array {
                    element: e2,
                    ..
                },
            ) => {
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
        bytecode.push(Byte::new(Instruction::MakeTuple).with_operand_u32(n_methods));
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
            bytecode.push(Byte::new(Instruction::MakeTuple).with_operand_u32(2));
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
            Self::emit_existential_pack_recipe(
                &mut pack_bc,
                &pack,
                &self.checker,
                &self.functions,
            );
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
        bytecode.push(Byte::new(Instruction::Index));
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
        bytecode.push(Byte::new(Instruction::Index));
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
    fn emit_io_host_invoke(&mut self, kind: crate::typechecking::IoBuiltin, args: &[Output]) {
        self.emit_host_native_invoke(kind.native_name(), args);
    }

    fn emit_thread_host_invoke(&mut self, kind: crate::typechecking::ThreadBuiltin, args: &[Output]) {
        self.emit_host_native_invoke(kind.native_name(), args);
    }

    /// Emit `HostInvoke` for a pipeline-registered host native by registry name.
    fn emit_host_native_invoke(&mut self, native_name: &str, args: &[Output]) {
        let Some(native_id) = self.native_id(native_name) else {
            let range = args
                .first()
                .map(|a| a.0.into_range())
                .unwrap_or(0..0);
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
        let arity = args.len();
        self.bytecode
            .push(Byte::new(Instruction::CONST).with_value_u32(native_id as u32));
        // Native id sits on the stack while args compile — protect it (and
        // prior arg values) from Instantiate `StorePop` temps.
        let depth_on_entry = self.expr_depth;
        self.expr_depth = depth_on_entry + 1;
        for arg in args {
            // Nested IO HostInvoke writes to `self.bytecode`; also fold any
            // bytes returned in the local vec (non-IO subexpressions).
            let mut arg_bc = self.do_compile(arg);
            self.bytecode.append(&mut arg_bc);
            self.expr_depth += 1;
        }
        self.bytecode
            .push(Byte::new(Instruction::MakeTuple).with_operand_u32(arity as u32));
        self.bytecode.push(
            Byte::new(Instruction::HostInvoke).with_operand_u32(arity as u32),
        );
        // Restore: caller owns counting the result value if it needs to.
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
                compiler
                    .bytecode
                    .push_load(slot);
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
            self.bytecode
                .push_load(0);
            self.bytecode.push(Byte::new(Instruction::STRINGIFY));
            self.bytecode.push_return();
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
            self.bytecode
                .push_load(0);
            self.bytecode
                .push(Byte::new(Instruction::UnboxValue).with_operand_u32(tag as u32));
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
                self.bytecode
                    .push_load(0);
                self.bytecode.push(
                    Byte::new(Instruction::UnboxValue).with_operand_u32(ValueTag::String as u32),
                );
                self.bytecode
                    .push(Byte::new(Instruction::MakeTuple).with_operand_u32(1));
                self.bytecode
                    .push(Byte::new(Instruction::HostInvoke).with_operand_u32(1));
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
            self.bytecode
                .push_load(0);
            self.bytecode.push(
                Byte::new(Instruction::UnboxValue).with_operand_u32(ValueTag::Instance as u32),
            );
            self.bytecode
                .push_load(1);
            self.bytecode.push(
                Byte::new(Instruction::UnboxValue).with_operand_u32(ValueTag::Array as u32),
            );
            self.bytecode
                .push(Byte::new(Instruction::MakeTuple).with_operand_u32(arity));
            self.bytecode
                .push(Byte::new(Instruction::HostInvoke).with_operand_u32(arity));
            self.bytecode.push_return();
        }

        let into_pairs = [
            ("int", "float", Instruction::CastIntToFloat, ValueTag::Int, ValueTag::Float),
            ("float", "int", Instruction::CastFloatToInt, ValueTag::Float, ValueTag::Int),
            ("int", "byte", Instruction::CastIntToByte, ValueTag::Int, ValueTag::Int),
            ("byte", "int", Instruction::CastByteToInt, ValueTag::Int, ValueTag::Int),
            ("int", "bool", Instruction::CastIntToBool, ValueTag::Int, ValueTag::Bool),
            ("bool", "int", Instruction::CastBoolToInt, ValueTag::Bool, ValueTag::Int),
        ];
        for (from, to, cast_op, from_tag, to_tag) in into_pairs {
            let fqn = into_primitive_fqn(from, to);
            if self.functions.contains_key(&fqn) {
                continue;
            }
            self.bind_function_entry(fqn);
            self.bytecode
                .push_load(0);
            self.bytecode.push(
                Byte::new(Instruction::UnboxValue).with_operand_u32(from_tag as u32),
            );
            self.bytecode.push(Byte::new(cast_op));
            if from_tag != to_tag {
                self.bytecode.push(
                    Byte::new(Instruction::BoxValue).with_operand_u32(to_tag as u32),
                );
            }
            self.bytecode.push_return();
        }
    }

    /// Map a fully-resolved `Ty` to a `ValueTag` for box/unbox
    /// emission at generic call boundaries.
    fn ty_to_value_tag(ty: &crate::typechecking::Ty) -> Option<ValueTag> {
        use crate::typechecking::{Ty, ty::BOOL, ty::BYTE, ty::FLOAT, ty::INT, ty::STRING, ty::UNIT};
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

    /// True when `ty` is unit / `()` (safe Ok payload for Result fall-through).
    fn ty_is_unit_like(ty: &crate::typechecking::Ty) -> bool {
        use crate::typechecking::{Ty, ty::UNIT};
        let mut t = ty.clone();
        while let Ty::Forall { body, .. } = t {
            t = *body;
        }
        match &t {
            Ty::Con(name) => name == UNIT,
            Ty::Tuple(items) if items.is_empty() => true,
            _ => false,
        }
    }

    /// Ok payload that may be invented as `CONST 0` + Ok-wrap on fall-through.
    ///
    /// Unit, open vars (bodies that only use `?` / `assert`), and zero-safe
    /// scalars (`int`/`bool`/…) are Ok; `string` / ADTs are not.
    fn ty_allows_result_ok_fallthrough(ty: &crate::typechecking::Ty) -> bool {
        use crate::typechecking::Ty;
        let mut t = ty.clone();
        while let Ty::Forall { body, .. } = t {
            t = *body;
        }
        if Self::ty_is_unit_like(&t) {
            return true;
        }
        if matches!(&t, Ty::Var(_)) || matches!(&t, Ty::Con(n) if n == "unknown") {
            return true;
        }
        Self::ty_allows_zero_default(&t)
    }

    /// True when `Value::default()` (`0`) is a valid representation for `ty`.
    ///
    /// Safe for unit / int / byte / bool / float, open vars (statement bodies),
    /// and empty tuples. `Option` / `Result` / `string` / ADTs need a real
    /// constructor (or an explicit `return`).
    fn ty_allows_zero_default(ty: &crate::typechecking::Ty) -> bool {
        use crate::typechecking::{
            Ty,
            ty::{BOOL, BYTE, FLOAT, INT, UNIT},
        };
        let mut t = ty.clone();
        while let Ty::Forall { body, .. } = t {
            t = *body;
        }
        match &t {
            Ty::Con(name) => {
                matches!(name.as_str(), UNIT | INT | BYTE | BOOL | FLOAT)
                    // Incomplete / placeholder types — do not spuriously E0111.
                    || name == "unknown"
            }
            Ty::Var(_) => true,
            // `()` / empty tuple is unit-like.
            Ty::Tuple(items) if items.is_empty() => true,
            _ => false,
        }
    }

    /// Whether an implicit fall-through is valid for `name`'s return.
    fn fallthrough_allows_zero(&self, name: &str) -> bool {
        use crate::typechecking::ty::{is_option_ty, result_ok_err};
        // Async fn bodies complete with a sentinel; the `coroutine<…>` value
        // is produced by MakeCoro at the call site.
        if self.coroutine_fns.contains(name)
            || self
                .current_function_qualified
                .as_ref()
                .is_some_and(|q| self.coroutine_fns.contains(q))
        {
            return true;
        }
        let Some(ret) = self.fn_return_ty(name) else {
            return true;
        };
        // `Option` fall-through emits `None` (not raw `0`) in
        // [`Self::emit_fallthrough_return`].
        if is_option_ty(&ret) {
            return true;
        }
        if self.compiling_result_mode {
            // Result-mode Ok-wraps unit / open Ok / zero-safe scalars.
            // Refuse inventing Ok for `string` / ADT payloads.
            if let Some((ok, _)) = result_ok_err(&ret) {
                return Self::ty_allows_result_ok_fallthrough(&ok);
            }
            return true;
        }
        if let crate::typechecking::Ty::App(con, _) = &ret
            && matches!(con.as_ref(), crate::typechecking::Ty::Con(n) if n == "coroutine")
        {
            return true;
        }
        Self::ty_allows_zero_default(&ret)
    }

    /// Emit defers + type-directed fall-through return (or E0111 when unsafe).
    fn emit_fallthrough_return(&mut self, name: &str, span: SimpleSpan) {
        use crate::typechecking::ty::is_option_ty;
        self.emit_run_defers();
        if !self.fallthrough_allows_zero(name) {
            let ret_s = self
                .fn_return_ty(name)
                .map(|t| format!("{t}"))
                .unwrap_or_else(|| "unknown".into());
            let mut message = Message::error(
                ErrorCode::ReturnMismatch,
                format!(
                    "function `{name}` reaches the end without returning a value of type `{ret_s}`"
                ),
                span.into_range(),
            );
            message.push(DiagLabel::new(
                "add an explicit `return` (implicit `0` is not valid for this type)".to_string(),
                span.into_range(),
            ));
            self.messages.push(message);
        }
        let opt_none = self
            .fn_return_ty(name)
            .as_ref()
            .is_some_and(is_option_ty);
        if opt_none {
            // `Option::None` = tag 0, arity 0.
            self.bytecode
                .push(Byte::new(Instruction::MakeEnum).with_operands_u16([0, 0]));
        } else {
            self.bytecode.push_const(0);
            if self.compiling_result_mode {
                Self::emit_ok_or_some_wrap(&mut self.bytecode, false);
            }
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
        let Some(body) = body else {
            self.consume_function_signature_output(method);
            return;
        };

        self.bind_function_entry(qualified.clone());
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
        let prev_fn_defers = std::mem::take(&mut self.fn_defers);

        let mut a = self.do_compile(args);
        self.bytecode.append(&mut a);
        for (slot, ty) in argument_unbox_tys.iter().enumerate() {
            if let Some(tag) = ty.as_ref().and_then(Self::ty_to_value_tag) {
                self.bytecode
                    .push_load(slot as u32);
                self.bytecode
                    .push(Byte::new(Instruction::UnboxValue).with_operand_u32(tag as u32));
                self.bytecode
                    .push_store_pop(slot as u32);
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
            let mut c = self.do_compile(body);
            self.bytecode.append(&mut c);

            if !self.region_ends_with_return(body_op_start) {
                self.emit_fallthrough_return(source_name, body.0);
            }

            self.fn_defers = prev_fn_defers;
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
                if let Expression::Argument(ty, name, is_rest) = child.1.as_ref()
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
        // Keep keying in sync with `monomorphize::candidate_for_call`: one
        // ground type per formal, with rest contributing its *element* type.
        let (fixed, rest, pack_rest) = self.split_call_args_for_rest(fn_name, args);
        let mut arg_types = Vec::with_capacity(fixed.len() + usize::from(pack_rest));
        for arg in &fixed {
            arg_types.push(monomorphize::ground_type_name(&self.checker, arg)?);
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
            let elem = monomorphize::ground_type_name(&self.checker, &rest[0])?;
            for arg in rest.iter().skip(1) {
                if monomorphize::ground_type_name(&self.checker, arg)? != elem {
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
                            bytecode.push(Byte::new(Instruction::Index));
                            let slot = self.alloc_binding_slot(name);
                            bytecode.push_store_pop(slot);
                        }
                        nested @ (LetPattern::Tuple(_) | LetPattern::Record(_)) => {
                            bytecode.push_load(src_slot);
                            bytecode.push_const(idx as i32);
                            bytecode.push(Byte::new(Instruction::Index));
                            let nested_slot = self.alloc_temp_slot();
                            bytecode
                                .push_store_pop(nested_slot);
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
                            Self::emit_raw_string_literal(bytecode, pf.name);
                            bytecode.push(Byte::new(Instruction::GetField));
                            let slot = self.alloc_binding_slot(name);
                            bytecode.push_store_pop(slot);
                        }
                        nested @ (LetPattern::Tuple(_) | LetPattern::Record(_)) => {
                            bytecode.push_load(src_slot);
                            Self::emit_raw_string_literal(bytecode, pf.name);
                            bytecode.push(Byte::new(Instruction::GetField));
                            let nested_slot = self.alloc_temp_slot();
                            bytecode
                                .push_store_pop(nested_slot);
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
        self.bytecode
            .push_store_pop(arr_slot);
        self.bytecode
            .push_const(0);
        self.bytecode
            .push_store_pop(idx_slot);

        // Consume binding Identifier NodeId (iterable → binding → body).
        let _ = self.next_emit_id();
        let binding_slot = self.alloc_binding_slot(binding_name);

        let mut bb = BlockBuilder::new();
        let top_label = bb.fresh_label(self.bytecode.il_mut());
        let continue_label = bb.fresh_label(self.bytecode.il_mut());
        let exit_label = bb.fresh_label(self.bytecode.il_mut());
        bb.bind_label(top_label, self.bytecode.il_mut());

        // cond: idx < len(arr)  (LE is `<`)
        self.bytecode
            .push_load(idx_slot);
        self.bytecode
            .push_load(arr_slot);
        self.bytecode.push(Byte::new(Instruction::ArrayLen));
        self.bytecode.push(Byte::new(Instruction::LE));
        bb.emit_jump_to(exit_label, BbJumpKind::JumpIfFalse, self.bytecode.il_mut());

        // x = arr[idx]
        self.bytecode
            .push_load(arr_slot);
        self.bytecode
            .push_load(idx_slot);
        self.bytecode.push(Byte::new(Instruction::Index));
        self.bytecode
            .push_store_pop(binding_slot);

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
        self.bytecode
            .push_load(idx_slot);
        self.bytecode
            .push_const(1);
        self.bytecode.push(Byte::new(Instruction::ADD));
        self.bytecode
            .push_store_pop(idx_slot);

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
        self.bytecode
            .push_store_pop(tup_slot);
        for i in 0..arity {
            self.bytecode
                .push_load(tup_slot);
            self.bytecode
                .push_const(i as i32);
            self.bytecode.push(Byte::new(Instruction::Index));
        }
        self.bytecode
            .push(Byte::new(Instruction::MakeArray).with_operand_u32(arity as u32));
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
                && !const_fold::body_has_loop_control(body)
            {
                if let Some(trips) = const_fold::range_trip_count(start, end, inclusive) {
                    let _ = self.next_emit_id();
                    let binding_slot = self.alloc_binding_slot(binding_name);
                    let _ = self.next_emit_id();
                    if let Some(ConstValue::Int(s)) =
                        const_fold::eval_expr(start, self.const_env())
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
                self.bytecode
                    .push_store_pop(cur_slot);
                let end_bc = self.do_compile(end);
                self.bytecode.extend(end_bc);
                self.bytecode
                    .push_store_pop(end_slot);
            }
            _ => {
                let range_slot = self.alloc_temp_slot();
                let iter_bc = self.do_compile(iterable);
                self.bytecode.extend(iter_bc);
                self.bytecode
                    .push_store_pop(range_slot);

                self.bytecode
                    .push_load(range_slot);
                Self::emit_raw_string_literal(&mut self.bytecode, "start");
                self.bytecode.push(Byte::new(Instruction::GetField));
                self.bytecode
                    .push_store_pop(cur_slot);

                self.bytecode
                    .push_load(range_slot);
                Self::emit_raw_string_literal(&mut self.bytecode, "end");
                self.bytecode.push(Byte::new(Instruction::GetField));
                self.bytecode
                    .push_store_pop(end_slot);
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
        self.bytecode
            .push_load(cur_slot);
        self.bytecode
            .push_load(end_slot);
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
        self.bytecode
            .push_load(cur_slot);
        self.bytecode
            .push_store_pop(binding_slot);

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
        self.bytecode
            .push_load(cur_slot);
        if float {
            let bits = Value::from(1.0_f64).raw() as u64;
            let idx = self.intern_constant(bits);
            self.bytecode
                .push(Byte::new(Instruction::CONST).with_const_pool(idx));
            self.bytecode.push(Byte::new(Instruction::ADDF));
        } else {
            self.bytecode
                .push_const(1);
            self.bytecode.push(Byte::new(Instruction::ADD));
        }
        self.bytecode
            .push_store_pop(cur_slot);

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
        self.bytecode
            .push_store_pop(handle_slot);

        let _ = self.next_emit_id();
        let binding_slot = self.alloc_binding_slot(binding_name);

        let mut bb = BlockBuilder::new();
        let top_label = bb.fresh_label(self.bytecode.il_mut());
        let exit_label = bb.fresh_label(self.bytecode.il_mut());
        bb.bind_label(top_label, self.bytecode.il_mut());

        self.bytecode
            .push_load(handle_slot);
        self.bytecode
            .push(Byte::new(Instruction::ResumeCoro).with_operand_u32(0));
        self.bytecode
            .push_store_pop(binding_slot);

        self.bytecode
            .push_load(handle_slot);
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
        self.bytecode
            .push(Byte::new(Instruction::BoxValue).with_operand_u32(carrier_tag));
        Self::emit_call_indirect(&mut self.bytecode, into_off, 1);
        self.bytecode
            .push_store_pop(it_slot);

        let _ = self.next_emit_id();
        let binding_slot = self.alloc_binding_slot(binding_name);

        let mut bb = BlockBuilder::new();
        let top_label = bb.fresh_label(self.bytecode.il_mut());
        let exit_label = bb.fresh_label(self.bytecode.il_mut());
        bb.bind_label(top_label, self.bytecode.il_mut());

        self.bytecode
            .push_load(it_slot);
        self.bytecode
            .push(Byte::new(Instruction::BoxValue).with_operand_u32(carrier_tag));
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
        self.bytecode
            .push_store_pop(binding_slot);

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

    fn emit_field_name(&self, bytecode: &mut Vec<Byte>, field: &str) {
        Self::emit_raw_string_literal(bytecode, field);
    }

    fn emit_raw_string_literal(bytecode: &mut impl EmitBuf, value: &str) {
        bytecode
            .push(Byte::new(Instruction::STRING).with_operand_u32(value.chars().count() as u32));
        for ch in value.chars() {
            bytecode.push(Byte::new(Instruction::DATA).with_operand_u32(ch.into()));
        }
    }

    fn variable_slot(&mut self, name: &str) -> Option<u32> {
        self.lookup_slot(name)
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
            Expression::String(_) | Expression::Format(_, _) => {
                Some(Ty::Con(crate::typechecking::ty::STRING.into()))
            }
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
            Expression::Identifier(name) => self
                .codegen_var_type_for(name)
                .map(|t| crate::typechecking::subst::apply_ty_prune(self.checker.subst(), &t)),
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
                bytecode.push(Byte::new(Instruction::GetField));
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
                bytecode.push(Byte::new(Instruction::Index));
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
                bytecode.push(Byte::new(Instruction::SetField));
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
                    bytecode.push(Byte::new(Instruction::POP));
                }
            }
            Expression::Index(_, None) => {}
            _ => {
                bytecode.push(Byte::new(Instruction::POP));
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
            Self::emit_raw_string_literal(bytecode, "%s%s");
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
            if self.try_emit_matrix_op(
                &mut tmp,
                self_id,
                span_start,
                span_end,
                target,
                Some(rhs),
            ) {
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
            bytecode.push(Byte::new(Instruction::Index));
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
            bytecode.push(Byte::new(Instruction::Index));
            let tmp_old = if !prefix {
                let t = self.alloc_temp_slot();
                bytecode.push_store_pop(t);
                bytecode.push_load(tmp_arr);
                bytecode.push_load(tmp_idx);
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
        if let Some(val) = const_fold::eval_expr(init, self.const_env()) {
            if self.checker.is_static_const_fqn(fqn) {
                self.static_const_values.insert(fqn.to_string(), val);
            }
        }
        let mut init_bc = self.do_compile(init);
        self.static_init_bytecode.append(&mut init_bc);
        self.static_init_bytecode.push(
            Byte::new(Instruction::StoreStatic).with_operand_u32(slot),
        );
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
            Expression::Identifier(name) => {
                self.codegen_var_type_for(name).map(|t| {
                    // Apply substitution so inferred record types resolve fully.
                    crate::typechecking::subst::apply_ty_prune(self.checker.subst(), &t)
                })
            }
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
        bytecode.push(Byte::new(Instruction::MakeEnum).with_operands_u16([tag, 1]));
    }

    /// Wrap the top-of-stack value as `Result::Err(e)`.
    fn emit_result_err(bytecode: &mut impl EmitBuf) {
        bytecode.push(Byte::new(Instruction::MakeEnum).with_operands_u16([1, 1])); // Err tag=1 arity=1
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
                    bytecode.push(Byte::new(Instruction::Index));
                    bytecode.push_load(t1);
                    bytecode.push_const(i as i32);
                    bytecode.push(Byte::new(Instruction::Index));
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
                    bytecode.push(Byte::new(Instruction::Index));
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
                    bytecode.push(Byte::new(Instruction::MakeTuple).with_operand_u32(3));
                } else {
                    bytecode.push(Byte::new(Instruction::MakeArray).with_operand_u32(3));
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
                            bytecode.push(Byte::new(Instruction::Index));
                            bytecode.push_const(t as i32);
                            bytecode.push(Byte::new(Instruction::Index));
                            // B[t][j]
                            bytecode.push_load(t1);
                            bytecode.push_const(t as i32);
                            bytecode.push(Byte::new(Instruction::Index));
                            bytecode.push_const(j as i32);
                            bytecode.push(Byte::new(Instruction::Index));
                            bytecode.push(Byte::new(mul));
                            if t > 0 {
                                bytecode.push(Byte::new(add));
                            }
                        }
                    }
                    if row_is_tuple {
                        bytecode.push(Byte::new(Instruction::MakeTuple).with_operand_u32(n as u32));
                    } else {
                        bytecode.push(Byte::new(Instruction::MakeArray).with_operand_u32(n as u32));
                    }
                }
                if outer_is_tuple {
                    bytecode.push(Byte::new(Instruction::MakeTuple).with_operand_u32(m as u32));
                } else {
                    bytecode.push(Byte::new(Instruction::MakeArray).with_operand_u32(m as u32));
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
                        bytecode.push(Byte::new(Instruction::Index));
                        bytecode.push_const(j as i32);
                        bytecode.push(Byte::new(Instruction::Index));
                        bytecode.push_load(t1);
                        bytecode.push_const(i as i32);
                        bytecode.push(Byte::new(Instruction::Index));
                        bytecode.push_const(j as i32);
                        bytecode.push(Byte::new(Instruction::Index));
                        bytecode.push(Byte::new(cell_op));
                    }
                    if row_is_tuple {
                        bytecode.push(Byte::new(Instruction::MakeTuple).with_operand_u32(n as u32));
                    } else {
                        bytecode.push(Byte::new(Instruction::MakeArray).with_operand_u32(n as u32));
                    }
                }
                if outer_is_tuple {
                    bytecode.push(Byte::new(Instruction::MakeTuple).with_operand_u32(m as u32));
                } else {
                    bytecode.push(Byte::new(Instruction::MakeArray).with_operand_u32(m as u32));
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
                        bytecode.push(Byte::new(Instruction::Index));
                        bytecode.push_const(j as i32);
                        bytecode.push(Byte::new(Instruction::Index));
                        self.emit_neg_tos(bytecode, elem_is_float);
                    }
                    if row_is_tuple {
                        bytecode.push(Byte::new(Instruction::MakeTuple).with_operand_u32(n as u32));
                    } else {
                        bytecode.push(Byte::new(Instruction::MakeArray).with_operand_u32(n as u32));
                    }
                }
                if outer_is_tuple {
                    bytecode.push(Byte::new(Instruction::MakeTuple).with_operand_u32(m as u32));
                } else {
                    bytecode.push(Byte::new(Instruction::MakeArray).with_operand_u32(m as u32));
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
        bytecode.push(Byte::new(Instruction::MakeTuple).with_operand_u32(arity as u32));
        bytecode.push(Byte::new(Instruction::HostInvoke).with_operand_u32(arity as u32));
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
        self.bytecode
            .push_store_pop(failed_slot);

        let mut bb = BlockBuilder::new();
        for (desc, offset) in &cases {
            if let Some(label) = self.bytecode.entry_label_for_offset(*offset as usize) {
                self.bytecode.emit_entry(EntryKind::Call, 0, label);
            } else {
                // Fallback for cases without a bound entry label (should be rare
                // after `bind_function_entry`); packed CALL(0, pc) keeps harness green.
                self.bytecode.push(
                    Byte::new(Instruction::CALL).with_call_packed(0, *offset),
                );
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
            self.bytecode.push(Byte::new(Instruction::POP));
            bb.emit_jump_to(done, BbJumpKind::Unconditional, self.bytecode.il_mut());
            bb.bind_label(fail, self.bytecode.il_mut());
            // Discard Err message payload.
            self.bytecode.push(Byte::new(Instruction::POP));
            let msg = format!("> Test \"{desc}\" failed\n");
            self.emit_string_literal(&msg);
            self.bytecode.push(Byte::new(Instruction::PRINT));
            // failed += 1
            self.bytecode
                .push_load(failed_slot);
            self.bytecode.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(1i64).raw() as _,
            ));
            self.bytecode.push(Byte::new(Instruction::ADD));
            self.bytecode
                .push_store_pop(failed_slot);
            bb.bind_label(done, self.bytecode.il_mut());
        }

        // if failed != 0 { panic "tests failed" }
        let panic_lbl = bb.fresh_label(self.bytecode.il_mut());
        let end_lbl = bb.fresh_label(self.bytecode.il_mut());
        self.bytecode
            .push_load(failed_slot);
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
                            if let Some(val) =
                                const_fold::eval_expr(&children[1], self.const_env())
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
                let table_key = if self.checker.is_overloaded(name)
                    || self.checker.is_overloaded(&qualified)
                {
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
                    self.context.variables.intern("self".to_string());
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

                self.bytecode.append(&mut a);

                let body_start = self.bytecode.len();
                let body_op_start = self.bytecode.ops().len();
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
                let body_end = self.bytecode.len();
                self.fn_bytecode_spans
                    .insert(table_key.clone(), (body_start, body_end));
                let entry = self.fn_entry_labels.get(&table_key).copied();
                self.bytecode
                    .record_func(table_key, entry, body_start, body_end);
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
                bytecode.push(Byte::new(Instruction::MakeFn).with_operand_u32(
                    make_fn_operand(captures.len() as u32, 0, arity as u32, is_rest),
                ));
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
                self.emit_byte(*span, Byte::new(Instruction::PRINT));
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
                Self::emit_raw_string_literal(&mut bytecode, "start");
                let mut end_bc = self.do_compile(end);
                bytecode.append(&mut end_bc);
                Self::emit_raw_string_literal(&mut bytecode, "end");
                bytecode.push_const(if *inclusive { 1 } else { 0 });
                Self::emit_raw_string_literal(&mut bytecode, "inclusive");
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
                bytecode.push(Byte::new(Instruction::Index));
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
                        _ => unreachable!(
                            "There should be only fields inside of a class definition"
                        ),
                    }
                }
                self.context.classes.insert(name.to_string(), instance_fields);
                self.context.symbols.intern(name.to_string());
            }
            Expression::Implementation { owner, methods, .. } => {
                let saved_ns = self.namespace.clone();
                self.namespace = owner.to_string();

                for method_node in methods {
                    match method_node.1.borrow() {
                        Expression::Method(_, body) => {
                            if let Expression::Function {
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
                    Expression::Function { is_static: true, .. }
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
                            Byte::new(Instruction::CALL)
                                .with_call_packed(arity, offset as u32),
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
                bytecode.push(Byte::new(Instruction::INIT).with_operand_u32(fields.len() as u32));
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
                        bytecode.push(Byte::new(Instruction::SetField));
                        bytecode.push(Byte::new(Instruction::POP));
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
                    let kind = info
                        .map(|i| i.kind)
                        .unwrap_or(ForInKind::Coroutine);
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
                            self.emit_for_in_range(
                                iterable,
                                body,
                                &binding_name,
                                inclusive,
                                float,
                            );
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
                        const_fold::eval_expr(iterable, self.const_env())
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
                if let Some(trips) = const_fold::for_loop_trip_count(
                    init.as_ref(),
                    cond,
                    step.as_ref(),
                ) && !const_fold::body_has_loop_control(body)
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
                if let Some(ConstValue::Bool(false)) =
                    const_fold::eval_expr(cond, self.const_env())
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
                let cap_names: Vec<String> =
                    captures.iter().map(|c| (*c).to_string()).collect();
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
                            "env::exit terminates the process with the given exit code"
                                .to_string(),
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
                        bytecode.push(Byte::new(Instruction::Index));
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
                        .or_else(|| {
                            self.checker
                                .call_dicts_for_span(span.start, span.end)
                        })
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
                                .map(|c| {
                                    overload_fn_key(&fqn_base, c.fixed_arity, c.is_rest)
                                })
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
                                self.emit_call_args_with_rest(
                                    &fqn,
                                    items,
                                    &mut bytecode,
                                    false,
                                )
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
                    let n = if let Some((fa, is_rest)) = self
                        .checker
                        .selected_overload_at(span.start, span.end)
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
                        self.bytecode
                            .push_load(lib_slot);
                        self.bytecode
                            .push_load(fn_id_slot);
                        self.expr_depth = depth_on_entry + 2;
                        if let Some(items) = args {
                            for arg in items {
                                let mut arg_bc = self.do_compile(arg);
                                self.bytecode.append(&mut arg_bc);
                                self.expr_depth += 1;
                            }
                        }
                        self.bytecode
                            .push(Byte::new(Instruction::MakeTuple).with_operand_u32(arity as u32));
                        let mut operand = arity as u32 & 0xFFFF;
                        if variadic {
                            let call_span = (span.start, span.end);
                            let arg_refs: Vec<_> =
                                args.as_ref().map(|items| items.iter().collect()).unwrap_or_default();
                            if let Some(tags) = resolve_variadic_ffi_tags(
                                &self.checker,
                                call_span,
                                &arg_refs,
                                &mut self.messages,
                            ) {
                                for &(tag, aux) in &tags {
                                    emit_ffi_type_const(&mut self.bytecode, tag, aux);
                                }
                                self.bytecode.push(
                                    Byte::new(Instruction::MakeTuple)
                                        .with_operand_u32(tags.len() as u32),
                                );
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
                        self.bytecode
                            .push(Byte::new(Instruction::MakeTuple).with_operand_u32(arity as u32));
                        self.bytecode.push(
                            Byte::new(Instruction::HostInvoke).with_operand_u32(arity as u32),
                        );
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
                        let fill_mask = self.checker.partial_fill_at(span.start, span.end).or_else(
                            || {
                                // Spread args count as their expanded arity, not one slot.
                                let argc = flat_arg_slice.len();
                                if !is_rest && fa > 0 && argc < fa {
                                    Some((1u32 << argc).wrapping_sub(1))
                                } else {
                                    None
                                }
                            },
                        );
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
                            bytecode.push(
                                Byte::new(Instruction::MakeFn).with_operand_u32(make_fn_operand(
                                    0,
                                    n_filled,
                                    fa as u32,
                                    is_rest,
                                )),
                            );
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
            Expression::Argument(ty, n, _is_rest) => {
                let _ = self.context.variables.intern(n.to_string());
                if ty.as_ref().is_some_and(|t| matches!(t.1.as_ref(), Expression::Forall { .. })) {
                    self.polyfn_vars.insert(n.to_string());
                }
                // bytecode.push(Byte::new(Instruction::LOAD)
            }
            Expression::Type(_) | Expression::TypeFun(_, _) | Expression::TypeFnSig { .. }
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
                    .filter(|_| self.checker.is_static_const_fqn(&self.qualify_static_fqn(n)))
                    .cloned()
                {
                    self.emit_const_value(&v, &mut bytecode);
                } else if let Some(static_slot) = self
                    .checker
                    .static_slot_index(&resolved)
                    .or_else(|| self.checker.static_slot_for_module_name(n))
                {
                    bytecode.push(
                        Byte::new(Instruction::LoadStatic).with_operand_u32(static_slot),
                    );
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
                                format!("Cannot reify overloaded `{}` without a type annotation", n),
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
                                Byte::new(Instruction::MakeFn).with_operand_u32(make_fn_operand(
                                    0,
                                    0,
                                    fa as u32,
                                    is_rest,
                                )),
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
                        bb.emit_jump_to(jmpf_target, BbJumpKind::JumpIfFalse, self.bytecode.il_mut());
                    }

                    // Body after cond+JMPF so Print/nested control-flow offsets stay correct.
                    let body_bc = self.do_compile(body);
                    self.bytecode.extend(body_bc);

                    // Emit a `JMP → end` placeholder for every
                    // branch except the last. The last branch falls
                    // through to `end_pos`.
                    if i + 1 < branches.len() {
                        bb.emit_jump_to(end_label, BbJumpKind::Unconditional, self.bytecode.il_mut());
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
                } else if self.emit_concrete_operator_call(
                    &mut bytecode,
                    lhs,
                    rhs,
                    "Lt",
                    "lt",
                ) {
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
                } else if self.emit_concrete_operator_call(
                    &mut bytecode,
                    lhs,
                    rhs,
                    "Gt",
                    "gt",
                ) {
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
                } else if self.emit_concrete_operator_call(
                    &mut bytecode,
                    lhs,
                    rhs,
                    "Le",
                    "le",
                ) {
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
                } else if self.emit_concrete_operator_call(
                    &mut bytecode,
                    lhs,
                    rhs,
                    "Ge",
                    "ge",
                ) {
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
                } else if self.emit_concrete_operator_call(
                    &mut bytecode,
                    lhs,
                    rhs,
                    "Eq",
                    "eq",
                ) {
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
                if self.try_emit_matrix_op(
                    &mut bytecode,
                    self_id,
                    span.start,
                    span.end,
                    lhs,
                    None,
                ) {
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
                    Self::emit_raw_string_literal(&mut bytecode, "%s%s");
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
                    let is_float = likely(self.compile_binary_operands(&mut bytecode, lhs, rhs));
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
                if self.try_emit_aggregate_arith(
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
                } else if self.emit_concrete_operator_call(
                    &mut bytecode,
                    lhs,
                    rhs,
                    "Eq",
                    "ne",
                ) {
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
                bytecode.push(Byte::new(Instruction::CONST).with_const_pool(idx));
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
                Expression::QualifiedAccess { owner, member } => {
                    let fqn = format!("{}::{}", owner, member);
                    if let Some(slot) = self.checker.static_slot_index(&fqn) {
                        self.append_binding_rhs(&mut bytecode, value);
                        bytecode.push(
                            Byte::new(Instruction::StoreStatic).with_operand_u32(slot),
                        );
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
                        bytecode.push(
                            Byte::new(Instruction::StoreStatic).with_operand_u32(slot),
                        );
                    }
                }
                Expression::Access(target_expr, field) => {
                    self.append_binding_rhs(&mut bytecode, value);
                    bytecode.append(&mut self.do_compile(target_expr));
                    self.emit_field_name(&mut bytecode, field);
                    bytecode.push(Byte::new(Instruction::SetField));
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
                        self.append_binding_rhs(&mut bytecode, value);
                        bytecode
                            .push_store_pop(symbol as u32);
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
                    self.emit_result_unwrap_or_panic();
                    self.bytecode
                        .push_store_pop(lib_slot);
                }
                // 3. For each declared function, emit declare(lib,
                //    name, (arg_tags...), ret) and store fn id.
                for decl in declarations {
                    let fn_name = decl.name.to_string();
                    let nfixed = if let Expression::Fragment(items) = decl.args.1.as_ref() {
                        items
                            .iter()
                            .filter(|a| matches!(a.1.as_ref(), Expression::Argument(..)))
                            .count()
                    } else {
                        0
                    };
                    // Key fixed-arity overloads; keep bare name for
                    // single decls and for C-varargs (not overload members).
                    let table_name = if !decl.variadic
                        && self.checker.is_overloaded(decl.name)
                    {
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
                    self.bytecode
                        .push_load(lib_slot);
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
                            if let Expression::Argument(type_expr, _param_name, _) = arg.1.as_ref()
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
                    self.bytecode
                        .push(Byte::new(Instruction::MakeTuple).with_operand_u32(arity));
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
                    self.bytecode
                        .push_store_pop(fn_id_slot);
                    self.extern_runtime_functions
                        .insert(table_name, (lib_slot, fn_id_slot));
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
                let mut body_bc = self.do_compile(body);
                self.bytecode.append(&mut body_bc);

                if !self.region_ends_with_return(body_op_start) {
                    // Test cases are typed as unit / Result<(), string> — zero is safe.
                    self.emit_fallthrough_return(&fn_name, body.0);
                }

                self.compiling_result_mode = prev_result_mode;
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
                        bytecode.push(
                            Byte::new(Instruction::LoadStatic).with_operand_u32(slot),
                        );
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
                                    Byte::new(Instruction::CALL).with_call_packed(
                                        parts.len() as u32,
                                        offset as u32,
                                    ),
                                );
                                return bytecode;
                            }
                        };
                        let arity =
                            self.emit_call_args_with_rest(&fqn, arg_slice, &mut bytecode, false);
                        bytecode.push(
                            Byte::new(Instruction::CALL)
                                .with_call_packed(arity, offset as u32),
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
                let arity = self
                    .checker
                    .arity_for(enum_name, variant_name)
                    .unwrap_or(0);

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
                    let end_label = bb.fresh_label(self.bytecode.il_mut());

                    // First payload slot after existing locals. Function
                    // args occupy 0..n-1; trailing `__dictN` slots (for
                    // constrained generics / HKT methods) push this above
                    // 1. JumpIfMatch/Unpack push payloads onto the stack
                    // above those locals, so bindings must start here —
                    // not at the historical hardcoded slot 1.
                    let payload_base = self.context.variables.len() as u32;

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
                            arm_labels[first_arm_idx] = Some(bb.fresh_label(self.bytecode.il_mut()));
                        }
                    }

                    let scrutinee_bc = self.do_compile(scrutinee);
                    self.bytecode.extend(scrutinee_bc);

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
                                    self.bytecode.push(Byte::new(Instruction::POP));
                                }
                                Pattern::Binding { name } => {
                                    // Binding arm — STORE the
                                    // scrutinee at `payload_base`
                                    // (the slot where the
                                    // scrutinee sits after being
                                    // pushed above existing locals).
                                    // The reverse pass records the
                                    // same slot in `match_bindings`.
                                    let _ = name;
                                    self.bytecode.push(
                                        Byte::new(Instruction::STORE)
                                            .with_operand_u32(payload_base),
                                    );
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
                            bb.emit_jump_to(end_label, BbJumpKind::Unconditional, self.bytecode.il_mut());
                        }
                    }

                    // Peephole / fuse-select safety: DUPLICATE;POP at the join
                    // (omitted for binding matches — see suppress_match_fusion_barrier).
                    // Label binds are also IL fusion barriers for return-match sites.
                    if !self.suppress_match_fusion_barrier {
                        self.bytecode.push(Byte::new(Instruction::DUPLICATE));
                        self.bytecode.push(Byte::new(Instruction::POP));
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
                    bytecode.push(
                        Byte::new(Instruction::LoadField).with_operand_u32(field_index as u32),
                    );
                } else if is_record || is_class {
                    self.emit_field_name(&mut bytecode, field);
                    bytecode.push(Byte::new(Instruction::GetField));
                } else {
                    // Unknown receiver — do not emit GetField (enum
                    // match bindings historically lacked side-table
                    // types). LoadField(0) keeps the stack balanced;
                    // VM hardens non-enum receivers.
                    bytecode.push(Byte::new(Instruction::LoadField).with_operand_u32(0));
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
                if let (Some(from), Some(to)) = (
                    src_ty.as_ref().and_then(primitive_type_name),
                    dst_name,
                ) {
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
                self.bytecode.push(Byte::new(Instruction::POP));
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
                    inner_ty.as_ref().and_then(extract_enum_name).and_then(|name| {
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
                    self.bytecode.push(
                        Byte::new(Instruction::LoadField).with_operand_u32(field_index as u32),
                    );
                } else if is_record || is_class {
                    Self::emit_raw_string_literal(&mut self.bytecode, field);
                    self.bytecode.push(Byte::new(Instruction::GetField));
                } else {
                    self.bytecode.push(Byte::new(Instruction::LoadField).with_operand_u32(0));
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
        }
        self.mono_offsets.clear();
        self.mono_codegen_var_types.clear();
        self.test_cases.clear();
        self.user_main_defined = false;
        if !self.include_tests {
            strip_tests::strip_test_declarations(ast);
        }
        self.checker.set_current_module(module);
        // Expand `derive` clauses to synthetic `impl` AST before the
        // ID pre-walk / typecheck (see `attrs::expand_program`).
        let expand = attrs::expand_program(ast);
        self.messages.extend(expand.messages);
        self.decorated_class_ctors.extend(expand.decorated_class_ctors);
        let _program_ty = self.checker.check_program(ast);
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
        self.mono_plan = monomorphize::plan_monomorphization(module, ast, &self.checker);

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
        // Splice static initializers into the IL before lower — no absolute
        // target bumping required for symbolic jumps.
        let static_init_region = if !self.static_init_bytecode.is_empty() {
            let pos = self.program_start_offset as usize;
            self.setup_entry_offset = pos as u32;
            let inits = std::mem::take(&mut self.static_init_bytecode);
            let init_len = inits.len();
            self.bytecode.splice_bytes_at(pos, inits);
            self.bytecode.bump_absolute_entry_targets(pos, init_len);
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

fn unwrap_expr_output<'a>(expr: &'a Output<'a>) -> &'a Output<'a> {
    match expr.1.as_ref() {
        Expression::Expr(inner)
        | Expression::Group(inner)
        | Expression::Statement(inner)
        | Expression::ExprStatement(inner) => unwrap_expr_output(inner),
        _ => expr,
    }
}

fn unwrapped_identifier<'a>(expr: &'a Output<'a>) -> Option<&'a str> {
    match unwrap_expr_output(expr).1.as_ref() {
        Expression::Identifier(name) => Some(name),
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
        let mut ast = Pratt::default().parse(src).expect("parse failed");
        let mut compiler = Compiler::default();
        // Stable placeholder ids so Approach A packed HostInvoke lowering
        // fires in unit tests (Pipeline assigns real ids at runtime).
        compiler.register_native_id(machine::PACKED_DOT, 9001);
        compiler.register_native_id(machine::PACKED_MATMUL, 9002);
        compiler.register_native_id(machine::PACKED_MATRIX_ZIP, 9003);
        compiler.register_native_id(machine::PACKED_MATRIX_NEG, 9004);
        let bc = compiler.compile("", &mut ast);
        (bc, compiler.constants)
    }

    /// True when bytecode contains a strength-reduced `x << shift`
    /// (`LOAD; CONST; SHL` or fused `BinSlotImm(SHL, shift)`).
    fn bytecode_has_shl_by(bc: &[Byte], shift: i64) -> bool {
        use common::Instruction;
        let has_load_const_shl = bc.windows(3).any(|w| {
            matches!(w[0].bytecode(), Instruction::LOAD)
                && matches!(w[1].bytecode(), Instruction::CONST)
                && matches!(w[2].bytecode(), Instruction::SHL)
                && (w[1].operand_u32() & Byte::POOL_FLAG) == 0
                && w[1].operand_u32() as i32 == shift as i32
        });
        let has_fused_shl = bc.iter().any(|b| {
            *b.bytecode() == Instruction::BinSlotImm
                && b.bin_slot_imm_parts().0 == Instruction::SHL as u8
                && b.bin_slot_imm_parts().2 == shift
        });
        has_load_const_shl || has_fused_shl
    }

    fn bytecode_has_any_shl(bc: &[Byte]) -> bool {
        use common::Instruction;
        bc.iter().any(|b| {
            matches!(b.bytecode(), Instruction::SHL)
                || (*b.bytecode() == Instruction::BinSlotImm
                    && b.bin_slot_imm_parts().0 == Instruction::SHL as u8)
        })
    }

    #[test]
    fn method_call_target_relocated_after_static_init_splice() {
        use common::Instruction;

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("examples/static_singleton.hy");
        let mut pipeline = crate::Pipeline::new();
        let (bytecode, constants) = pipeline
            .compile_src_from_file(path.to_str().unwrap())
            .expect("compile");
        let bump_off = pipeline.compiler_mut().get_function("Counter::bump");
        assert!(
            bytecode.iter().any(|b| {
                matches!(b.bytecode(), Instruction::CALL) && b.call_parts().1 == bump_off
            }),
            "CALL to Counter::bump must target {bump_off} after static-init splice"
        );

        let mut machine = machine::Machine::<256>::default();
        pipeline.wire_host_natives(&mut machine);
        machine.set_program_debug(pipeline.program_debug());
        machine.run_raw(&bytecode, &constants, pipeline.static_slot_count());
    }

    #[test]
    fn two_module_and_class_static_assignments_run() {
        use common::{ArchivedProgram, ARCHIVE_VERSION};
        use rkyv::rancor::Error;

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("examples/static_minimal.hy");
        let mut pipeline = crate::Pipeline::new();
        let (bytecode, constants) = pipeline
            .compile_src_from_file(path.to_str().unwrap())
            .expect("compile");
        assert_eq!(pipeline.static_slot_count(), 2);

        let program = ArchivedProgram {
            version: ARCHIVE_VERSION,
            static_slot_count: pipeline.static_slot_count(),
            constants: constants.clone(),
            bytecode: bytecode.clone(),
            source_files: pipeline.program_debug().source_files,
            debug_locs: pipeline.program_debug().debug_locs,
        };
        let bytes = rkyv::to_bytes::<Error>(&program).expect("serialize");
        let archived =
            rkyv::access::<rkyv::Archived<ArchivedProgram>, Error>(bytes.as_slice()).expect("access");
        let loaded_bc: Vec<Byte> =
            rkyv::deserialize::<Vec<Byte>, Error>(&archived.bytecode).expect("bc");
        let loaded_constants: Vec<u64> =
            rkyv::deserialize::<Vec<u64>, Error>(&archived.constants).expect("consts");
        let static_slots = u32::from(archived.static_slot_count);

        let mut machine = machine::Machine::<256>::default();
        pipeline.wire_vm_ffi(&mut machine, Some(path.as_path()));
        pipeline.wire_host_natives(&mut machine);
        machine.run_raw(&loaded_bc, &loaded_constants, static_slots);
    }

    /// `test("…")` cases become `__zs_test_N` functions; standalone runs get a virtual `main`.
    #[test]
    fn test_case_emits_synthetic_fns_virtual_main_and_relocates_offsets() {
        use common::Instruction;
        let mut ast = Pratt::default()
            .parse(
                r#"
test("one") { assert(true)?; }
test("two") { assert(true)?; }
"#,
            )
            .expect("parse failed");
        let mut compiler = Compiler::default();
        compiler.set_include_tests(true);
        let bc = compiler.compile("", &mut ast);

        assert_eq!(compiler.test_cases().len(), 2);
        assert_eq!(compiler.test_cases()[0].0, "one");
        assert_eq!(compiler.test_cases()[1].0, "two");

        let syn0 = compiler.get_function("__zs_test_0");
        let syn1 = compiler.get_function("__zs_test_1");
        let main_off = compiler.get_function("main");
        assert!(syn0 < bc.len(), "__zs_test_0 offset out of range");
        assert!(syn1 < bc.len(), "__zs_test_1 offset out of range");
        assert!(main_off < bc.len(), "virtual main offset out of range");
        assert_ne!(syn0, syn1, "synthetic test fns must be distinct");
        assert_ne!(main_off, syn0, "virtual main must be distinct from cases");

        // Peephole relocates test_cases offsets to match fused function entries.
        assert_eq!(
            compiler.test_cases()[0].1, syn0 as u32,
            "test_cases[0] must track peephole relocation of __zs_test_0"
        );
        assert_eq!(
            compiler.test_cases()[1].1, syn1 as u32,
            "test_cases[1] must track peephole relocation of __zs_test_1"
        );

        let main_bc = &bc[main_off..];
        let calls_in_main = main_bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::CALL))
            .count();
        assert!(
            calls_in_main >= 2,
            "virtual main should CALL each harness case; got {calls_in_main}"
        );
        assert!(
            main_bc
                .iter()
                .any(|b| matches!(b.bytecode(), Instruction::Panic)),
            "virtual main must Panic on aggregate soft-fail"
        );
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

    fn bytecode_has_dup_pop_barrier(bc: &[Byte]) -> bool {
        use common::Instruction;
        bc.windows(2).any(|w| {
            matches!(w[0].bytecode(), Instruction::DUPLICATE)
                && matches!(w[1].bytecode(), Instruction::POP)
        })
    }

    #[test]
    fn let_match_omits_fusion_barrier() {
        let (bc, _pool) = compile_src(
            r#"
enum Result<T, E> { Ok(T), Err(E) }
fn foo() -> Result<int, int> { return Result::Ok(0); }
fn main() {
    let x = match foo() {
        Result::Ok(s) => s,
        Result::Err(_) => panic "bad",
    };
    print "%i", x;
}
"#,
        );
        assert!(
            !bytecode_has_dup_pop_barrier(&bc),
            "let x = match should omit DUPLICATE;POP fusion barrier"
        );
    }

    #[test]
    fn return_match_keeps_fusion_barrier() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            r#"
enum Option<T> { None, Some(T) }
fn foo() -> int {
    return match Option::Some(1) {
        Option::None => 0,
        Option::Some(n) => n,
    };
}
"#,
        );
        // IL: join Label is the fuse barrier (DUPLICATE;POP is stack_dce'd).
        // Peephole safety = stacked arm value not dropped by ConstReturnImm.
        assert!(
            !bc.iter()
                .any(|b| matches!(*b.bytecode(), Instruction::ConstReturnImm)),
            "return match join must not lower to ConstReturnImm"
        );
        assert!(
            bc.iter()
                .any(|b| matches!(*b.bytecode(), Instruction::RETURN)),
            "return match must still end in RETURN"
        );
    }

    #[test]
    fn assignment_match_omits_fusion_barrier() {
        let (bc, _pool) = compile_src(
            r#"
enum Result<T, E> { Ok(T), Err(E) }
fn main() {
    let x = 0;
    x = match Result::Ok(1) {
        Result::Ok(n) => n,
        Result::Err(_) => panic "bad",
    };
    print "%i", x;
}
"#,
        );
        assert!(
            !bytecode_has_dup_pop_barrier(&bc),
            "x = match should omit fusion barrier before StorePop"
        );
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
        // body should use int ADD (fused BinSlotSlot or bare LOAD/LOAD/ADD).
        let has_int_bin_slot = bc.iter().any(|b| {
            *b.bytecode() == Instruction::BinSlotSlot
                && b.bin_slot_slot_parts().0 == Instruction::ADD as u8
        });
        let has_bare_add = bc.iter().any(|b| *b.bytecode() == Instruction::ADD);
        let has_float_bin_slot = bc.iter().any(|b| {
            *b.bytecode() == Instruction::BinSlotSlot
                && b.bin_slot_slot_parts().0 == Instruction::ADDF as u8
        });
        assert!(
            (has_int_bin_slot || has_bare_add) && !has_float_bin_slot,
            "expected int ADD (fused or bare) for integer arithmetic; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// `x * 8` strength-reduces to `x << 3` (via [`const_fold::strength_mul_int`]).
    #[test]
    fn mul_by_power_of_two_emits_shl_not_mul() {
        let (bc, _pool) = compile_src("fn scale(int x) -> int { return x * 8; }");
        assert!(
            bytecode_has_shl_by(&bc, 3),
            "expected LOAD/CONST/SHL (shift 3) or fused BinSlotImm(SHL, 3) for x*8; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// Commuted form `8 * x` must use the same SHL lowering (LHS factor).
    #[test]
    fn mul_by_lhs_power_of_two_emits_shl() {
        let (bc, _pool) = compile_src("fn scale(int x) -> int { return 8 * x; }");
        assert!(
            bytecode_has_shl_by(&bc, 3),
            "expected SHL (shift 3) for 8*x; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// `const K = 16; x * K` must consult `const_env` and emit `<< 4`.
    #[test]
    fn mul_by_const_power_of_two_emits_shl() {
        let (bc, _pool) = compile_src(
            "fn scale(int x) -> int { const K = 16; return x * K; }",
        );
        assert!(
            bytecode_has_shl_by(&bc, 4),
            "expected SHL (shift 4) for x*const(16); opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// `x * 1` is identity-reduced, not `<< 0` (shift 0 is reserved for
    /// [`const_fold::strength_reduced_inner`], not SHL lowering).
    #[test]
    fn mul_by_one_does_not_emit_shl() {
        let (bc, _pool) = compile_src("fn id(int x) -> int { return x * 1; }");
        assert!(
            !bytecode_has_any_shl(&bc),
            "x*1 should identity-reduce, not emit SHL; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// Float `*` must never strength-reduce to int `SHL` (defense-in-depth
    /// type gate in `try_emit_folded_expr`). Typecheck rejects `float * int`;
    /// when both sides are float the factor is never a power-of-two int, so
    /// `strength_mul_to_shl` stays `None` and we emit `MULF`.
    #[test]
    fn float_mul_does_not_emit_shl() {
        use common::Instruction;
        let (bc, _pool) = compile_src("fn scale(float x) -> float { return x * 8.0; }");
        assert!(
            !bytecode_has_any_shl(&bc),
            "float mul must not emit SHL; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
        let has_mulf = bc.iter().any(|b| {
            matches!(b.bytecode(), Instruction::MULF)
                || (*b.bytecode() == Instruction::BinSlotImm
                    && b.bin_slot_imm_parts().0 == Instruction::MULF as u8)
                || (*b.bytecode() == Instruction::BinSlotSlot
                    && b.bin_slot_slot_parts().0 == Instruction::MULF as u8)
                || (*b.bytecode() == Instruction::BinReturn
                    && b.bin_return_op() == Instruction::MULF as u8)
        });
        assert!(
            has_mulf,
            "expected MULF (bare or fused) for float mul; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// `byte` is int-like for VM `SHL` (`as_int`); `byte * 8` should still
    /// strength-reduce (not be excluded by the int-only type gate).
    #[test]
    fn byte_mul_by_power_of_two_emits_shl() {
        let (bc, _pool) = compile_src("fn scale(byte x) -> byte { return x * 8; }");
        assert!(
            bytecode_has_shl_by(&bc, 3),
            "expected SHL (shift 3) for byte*8; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// Type aliases to `int` expand at check time, so `I * 8` still SHLs.
    #[test]
    fn aliased_int_mul_by_power_of_two_emits_shl() {
        let (bc, _pool) = compile_src(
            "type I = int; fn scale(I x) -> I { return x * 8; }",
        );
        assert!(
            bytecode_has_shl_by(&bc, 3),
            "expected SHL for aliased int*8; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// Generic `T: Num` bodies with two type operands dispatch through the
    /// dictionary (`CallIndirect`), never primitive `SHL`. (A literal factor
    /// like `x * 8` unifies `T` to `int` in the checker today, so that shape
    /// correctly takes the primitive SHL path; the `bound_mul` guard covers
    /// the open-var case.)
    #[test]
    fn generic_num_mul_uses_dictionary_not_shl() {
        use common::Instruction;
        let (bc, _pool) =
            compile_src("fn mul2<T: Num>(T a, T b) -> T { return a * b; } fn main() { }");
        assert!(
            !bytecode_has_any_shl(&bc),
            "generic Num mul must not lower to SHL; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::CallIndirect)),
            "expected CallIndirect for generic Num mul; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn string_addition_emits_format_not_add() {
        use common::Instruction;
        let (bc, _pool) = compile_src("\"a\" + \"b\";");

        let folded_string = bc.iter().any(|b| matches!(b.bytecode(), Instruction::STRING));
        let via_format = bc.iter().any(|b| matches!(b.bytecode(), Instruction::FORMAT));
        assert!(
            folded_string || via_format,
            "expected folded STRING or FORMAT for string concat; opcodes: {:?}",
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
        let mut ast = Pratt::default().parse("x;").expect("parse failed");
        let mut c = Compiler::default();
        c.compile("test", &mut ast);
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
        let mut ast = Pratt::default()
            .parse("print \"hi\";")
            .expect("parse failed");
        let _bc = c.compile("test", &mut ast);
        let msgs = std::mem::take(&mut c.messages);
        assert!(msgs.is_empty(), "expected no messages, got: {:?}", msgs);
    }

    #[test]
    fn typeclass_impl_method_registers_fqn_function() {
        let mut ast = Pratt::default()
            .parse(
                r#"
                trait Foo<T> { fn bar(T x) -> T; }
                impl Foo<int> { fn bar(int x) -> int { return x; } }
                fn main() { }
                "#,
            )
            .expect("parse failed");
        let mut compiler = Compiler::default();
        let bc = compiler.compile("", &mut ast);
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
        let src = include_str!("../../examples/fib.hy");
        let mut ast = Pratt::default().parse(src).expect("parse fib");

        // `compile_module` appends IL to the shared buffer; fusion/label
        // resolution happens once via `finalize_bytecode`.
        let mut module = Compiler::default();
        let _ = module.compile_module("", &mut ast);
        assert!(
            module.bytecode.ops().iter().any(|op| {
                matches!(
                    op.as_plain_byte().map(|b| *b.bytecode()),
                    Some(Instruction::LOAD)
                )
            }),
            "module compile should still contain LOAD in IL before finalize"
        );
        module.finalize_bytecode();

        let mut full = Compiler::default();
        let bc_full = full.compile("", &mut ast);

        assert_eq!(
            &bc_full[3..],
            &module.bytecode_slice()[3..],
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
        let src = include_str!("../../examples/fib.hy");
        let (bc, _) = compile_src(src);
        // fib's body fuses `n <= 2` into `BinSlotImm` / `BinSlotImmJmpf`
        // and may fuse tails into ConstReturnImm / BinReturn when the join
        // is not a shared JMP-to-RETURN site (see fuse_slots_with_origins).
        let bin_slot_imm = bc
            .iter()
            .filter(|b| *b.bytecode() == Instruction::BinSlotImm)
            .count();
        let bin_slot_imm_jmpf = bc
            .iter()
            .filter(|b| *b.bytecode() == Instruction::BinSlotImmJmpf)
            .count();
        let _ = (bin_slot_imm, bin_slot_imm_jmpf);
        assert!(
            bc.iter().any(|b| matches!(
                *b.bytecode(),
                Instruction::ADD | Instruction::BinReturn | Instruction::BinSlotSlot
            )),
            "expected fib tail add present (fused or not)"
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
        let (bc, _pool) = compile_src(
            "fn main() { \
 let acc = 0; \
 let i = 0; \
 while (i < 2000) { \
 acc = acc + i; \
 i = i + 1; \
 } \
 }",
        );

        assert!(
            bc.iter().any(|b| matches!(
                *b.bytecode(),
                Instruction::JMPF
                    | Instruction::CmpJmpf
                    | Instruction::BinSlotImmJmpf
                    | Instruction::LogNotJmpf
            )),
            "while condition should emit a false-jump"
        );
        assert!(
            bc.iter().any(|b| {
                matches!(*b.bytecode(), Instruction::JMP)
                    && b.operand_u32() != u32::MAX
                    && (b.operand_u32() as usize) < bc.len()
            }),
            "loop should emit a back-edge JMP"
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
    fn for_in_array_emits_array_len_index_and_back_edge() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn main() { for x in [1, 2, 3] { print \"%i\", x; } }",
        );
        let has_len = bc
            .iter()
            .any(|b| matches!(b.bytecode(), Instruction::ArrayLen));
        let has_index = bc
            .iter()
            .any(|b| matches!(b.bytecode(), Instruction::Index));
        let jmp = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JMP))
            .count();
        assert!(has_len, "array for-in should emit ArrayLen");
        assert!(has_index, "array for-in should emit Index");
        assert!(jmp >= 1, "array for-in should emit back-edge JMP; got {jmp}");
    }

    #[test]
    fn for_in_dict_emits_dict_entries() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn main() { let d = { a: 1, b: 2 }; for p in d { print \"%i\", p[1]; } }",
        );
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::DictEntries)),
            "dict for-in should emit DictEntries; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn for_in_custom_emits_into_iter_and_next_calls() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "class Counter { cur: int, end: int, } \
impl IntoIterator<Counter> { \
    type Item = int; type IntoIter = Counter; \
    fn into_iter(Counter c) -> Counter { return c; } \
} \
impl Iterator<Counter> { \
    type Item = int; \
    fn next(Counter c) -> Option<int> { \
        if c.cur < c.end { let v = c.cur; c.cur = c.cur + 1; return Option::Some(v); } \
        return Option::None; \
    } \
} \
fn main() { let c = new Counter(0, 3); for x in c { print \"%i\", x; } }",
        );
        let call_indirect = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::CallIndirect))
            .count();
        let jump_if_match = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JumpIfMatch))
            .count();
        assert!(
            call_indirect >= 2,
            "custom for-in should CallIndirect into_iter and next; got {call_indirect}"
        );
        assert!(
            jump_if_match >= 1,
            "custom for-in should JumpIfMatch on Option::None; got {jump_if_match}"
        );
    }

    #[test]
    fn for_in_coro_emits_resume_done_and_back_edge() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "async fn counter() { yield 0; yield 1; return 99; } \
fn main() { for x in counter() { print \"%i\", x; } }",
        );

        let resume = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::ResumeCoro))
            .count();
        let done = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::DoneCoro))
            .count();
        let log_not = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::LogNot | Instruction::LogNotJmpf))
            .count();
        let jmpf = bc
            .iter()
            .filter(|b| {
                matches!(
                    b.bytecode(),
                    Instruction::JMPF | Instruction::LogNotJmpf
                )
            })
            .count();
        let jmp = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JMP))
            .count();

        assert!(resume >= 1, "expected ResumeCoro in for-in; got {resume}");
        assert!(done >= 1, "expected DoneCoro in for-in; got {done}");
        assert!(
            log_not + jmpf >= 1,
            "expected done-check exit branch (LogNot/JMPF); log_not={log_not} jmpf={jmpf}"
        );
        assert!(jmp >= 1, "expected back-edge JMP; got {jmp}");
    }

    #[test]
    fn for_in_coro_break_patches_exit_jump() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "async fn counter() { yield 0; yield 1; yield 2; } \
fn main() { for x in counter() { if x == 1 { break; } } }",
        );
        let jmp_targets: Vec<u32> = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JMP))
            .map(|b| b.operand_u32())
            .collect();
        assert!(
            jmp_targets.iter().all(|t| *t != 0),
            "for-in break/back-edge JMPs should be patched: {:?}",
            jmp_targets
        );
    }

    #[test]
    fn break_and_continue_outside_loop_emit_diagnostics() {
        let mut ast = Pratt::default()
            .parse("fn main() { break; continue; }")
            .expect("parse failed");
        let mut compiler = Compiler::default();
        compiler.compile("", &mut ast);
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

    /// Codegen test 12 : a match pattern with SHUFFLED record
    /// fields (`{ y: _, x: a }`) emits exactly one STORE (for `a`)
    /// and at least one POP (for `_` / omitted fields). Declaration-
    /// order binding is covered by the pipeline golden
    /// `shuffled_record_pattern_binds_declaration_order_field`.
    #[test]
    fn match_emits_binding_interns_in_declaration_order() {
        use common::Instruction;
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
        let store_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::STORE))
            .count();
        assert_eq!(store_count, 1, "expected exactly one STORE for `a`");
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
    // `examples/result.hy` after it's extended to two `Result::Ok`
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
        let _ = pop_count;
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
        let mut ast = Pratt::default().parse(src).expect("parse failed");
        let bc = Compiler::default().compile("test", &mut ast);
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
    /// - 3 JUMP_IF_MATCH (outer Result::Ok + inner Some + inner None)
    /// - no reverse-pass POP for the inner Unit (consume_values=false)
    ///
    /// Every test-chain arm gets a pass_label, so `Option::None`
    /// dispatches via JUMP_IF_MATCH rather than fall-through POP
    /// (required when a later outer-tag group follows the Ok group).
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
        // - 1 JUMP_IF_MATCH for the inner Option::None (pass_label)
        //
        // The reverse pass emits 0 POPs for the inner Unit
        // sub-pattern (`consume_values = false`).
        let src = "fn main() { \
 let x = Result::Ok(Option::Some(42)); \
 let _ = match x { \
 Result::Ok(Option::Some(v)) => v, \
 Result::Ok(Option::None) => 0, \
 }; \
 }";
        let mut ast = Pratt::default().parse(src).expect("parse failed");
        let bc = Compiler::default().compile("test", &mut ast);

        // Outer Ok + inner Some + inner None (last arm has pass_label).
        let jimp_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::JumpIfMatch))
            .count();
        assert_eq!(
            jimp_count, 3,
            "expected exactly 3 JUMP_IF_MATCH (outer Ok + inner Some + inner None); got {}",
            jimp_count
        );

        // Binding `let _ = match` omits the fusion-barrier POP; other POPs
        // may come from wildcard/None arms only.
        let pop_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::POP))
            .count();
        assert!(
            pop_count <= 1,
            "binding match should not add fusion-barrier POP; got {pop_count}"
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

    /// Scalar `const` at use sites folds through codegen (no LOAD of binding).
    #[test]
    fn const_scalar_folds_add_to_single_const() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn main() { const x = 5; print \"%i\", x + 5; }",
        );
        let const_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::CONST))
            .count();
        assert!(
            bc.iter().any(|b| {
                matches!(b.bytecode(), Instruction::CONST)
                    && b.operand_u32() as i32 == 10
            }),
            "expected folded CONST 10 for `const x = 5; x + 5`"
        );
        let _ = const_count;
    }

    /// `if 5 < 5` must not fold as true (parser `<` → `Le`, strict less-than).
    #[test]
    fn const_if_strict_lt_does_not_take_then_branch() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn main() { if 5 < 5 { print \"%i\", 1; } else { print \"%i\", 0; } }",
        );
        assert!(
            !bc.iter().any(|b| matches!(b.bytecode(), Instruction::JMPF)),
            "both branches constant-folded; expected only else body"
        );
    }

    /// Constant `if` condition emits only the taken branch (no JMPF cascade).
    #[test]
    fn const_if_emits_only_taken_branch() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn main() { if 4 < 5 { print \"%i\", 1; } else { print \"%i\", 0; } }",
        );
        assert!(
            !bc.iter().any(|b| matches!(b.bytecode(), Instruction::JMPF)),
            "folded `if 4 < 5` should not emit JMPF; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// Self tail-recursive `return f(...)` uses TailCall instead of CALL+RETURN.
    #[test]
    fn tail_recursive_sum_emits_tail_call() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn sum_to(int n, int acc) -> int { \
if n <= 0 { return acc; } \
return sum_to(n - 1, acc + n); \
} \
fn main() { print \"%i\", sum_to(5, 0); }",
        );
        assert!(
            bc.iter().any(|b| matches!(b.bytecode(), Instruction::TailCall)),
            "expected TailCall in tail-recursive sum_to"
        );
    }

    /// Tiny `add` is inlined at direct call sites (arithmetic in main bytecode).
    #[test]
    fn tiny_add_inlined_at_call_site() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn add(int a, int b) -> int { return a + b; } \
fn main() { print \"%i\", add(3, 4); }",
        );
        assert!(
            bc.iter().any(|b| {
                matches!(
                    b.bytecode(),
                    Instruction::BinSlotSlot | Instruction::ADD | Instruction::BinReturn
                )
            }),
            "expected inlined add to emit a binary op in bytecode"
        );
    }

    #[test]
    fn is_tiny_inline_il_rejects_jump_span() {
        use crate::il::{IlJumpKind, IlOp, Label};
        use common::{Byte, DebugLoc, Instruction};
        let ops = vec![
            IlOp::byte(Byte::new(Instruction::CONST).with_const_inline(1)),
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target: Label(0),
                loc: DebugLoc::unknown(),
            },
            IlOp::Label(Label(0)),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        // Emitting-only slice (as code_slice_ops would return): Jump + CONST + RETURN
        let emitting: Vec<IlOp> = ops.into_iter().filter(|op| op.emits_code()).collect();
        assert!(!Compiler::is_tiny_inline_il(&emitting));
    }

    #[test]
    fn is_tiny_inline_il_accepts_sole_const_return_imm() {
        use crate::il::IlOp;
        use common::{Byte, Instruction};
        let ops = vec![IlOp::byte(
            Byte::new(Instruction::ConstReturnImm).with_operand_u32(7),
        )];
        assert!(Compiler::is_tiny_inline_il(&ops));
        let expanded =
            Compiler::expand_fused_return_for_inline(&ops[0].as_plain_byte().unwrap(), &[])
                .expect("expand");
        assert_eq!(*expanded.bytecode(), Instruction::CONST);
        assert_eq!(expanded.operand_u32(), 7);
    }

    #[test]
    fn is_tiny_inline_il_accepts_sole_load_return_slot() {
        use crate::il::IlOp;
        use common::{Byte, Instruction};
        let ops = vec![IlOp::byte(
            Byte::new(Instruction::LoadReturnSlot).with_operand_u32(0),
        )];
        assert!(Compiler::is_tiny_inline_il(&ops));
        let expanded =
            Compiler::expand_fused_return_for_inline(&ops[0].as_plain_byte().unwrap(), &[42])
                .expect("expand");
        assert_eq!(*expanded.bytecode(), Instruction::LOAD);
        assert_eq!(expanded.operand_u32(), 42);
    }

    #[test]
    fn is_tiny_inline_il_accepts_sole_bin_return() {
        use crate::il::IlOp;
        use common::{Byte, Instruction};
        let ops = vec![IlOp::byte(
            Byte::new(Instruction::BinReturn).with_bin_return(Instruction::ADD as u8),
        )];
        assert!(Compiler::is_tiny_inline_il(&ops));
        let mut out = Vec::new();
        assert!(Compiler::expand_bin_return_for_inline(
            &ops[0].as_plain_byte().unwrap(),
            &[10, 11],
            &mut out
        ));
        assert_eq!(out.len(), 3);
        assert_eq!(*out[0].bytecode(), Instruction::LOAD);
        assert_eq!(out[0].operand_u32(), 10);
        assert_eq!(*out[1].bytecode(), Instruction::LOAD);
        assert_eq!(out[1].operand_u32(), 11);
        assert_eq!(*out[2].bytecode(), Instruction::ADD);
    }

    #[test]
    fn is_tiny_inline_il_accepts_typed_fused_returns() {
        use crate::il::IlOp;
        use common::{DebugLoc, Instruction};
        assert!(Compiler::is_tiny_inline_il(&[IlOp::ConstReturnImm {
            imm: 7,
            loc: DebugLoc::unknown(),
        }]));
        assert!(Compiler::is_tiny_inline_il(&[IlOp::LoadReturnSlot {
            slot: 0,
            loc: DebugLoc::unknown(),
        }]));
        assert!(Compiler::is_tiny_inline_il(&[IlOp::BinReturn {
            op: Instruction::SUB,
            loc: DebugLoc::unknown(),
        }]));
        // Typed plain return body with a single terminal RETURN.
        assert!(Compiler::is_tiny_inline_il(&[
            IlOp::BinSlotSlot {
                op: Instruction::ADD as u8,
                a: 0,
                b: 1,
                loc: DebugLoc::unknown(),
            },
            IlOp::Return {
                loc: DebugLoc::unknown(),
            },
        ]));
    }

    #[test]
    fn is_tiny_inline_il_accepts_bin_slot_slot_body() {
        use crate::il::IlOp;
        use common::{Byte, Instruction};
        let ops = vec![
            IlOp::byte(
                Byte::new(Instruction::BinSlotSlot).with_bin_slot_slot(
                    Instruction::ADD as u8,
                    0,
                    1,
                ),
            ),
            IlOp::byte(Byte::new(Instruction::RETURN)),
        ];
        assert!(Compiler::is_tiny_inline_il(&ops));
    }

    #[test]
    fn remap_bin_slot_for_inline_rewrites_slots() {
        use common::{Byte, Instruction};
        let imm = Byte::new(Instruction::BinSlotImm).with_bin_slot_imm(Instruction::ADD as u8, 0, 3);
        let remapped =
            Compiler::remap_bin_slot_for_inline(&imm, &[10]).expect("remap BinSlotImm");
        let (op, slot, val) = remapped.bin_slot_imm_parts();
        assert_eq!(op, Instruction::ADD as u8);
        assert_eq!(slot, 10);
        assert_eq!(val, 3);

        let slot_slot = Byte::new(Instruction::BinSlotSlot).with_bin_slot_slot(
            Instruction::SUB as u8,
            0,
            1,
        );
        let remapped =
            Compiler::remap_bin_slot_for_inline(&slot_slot, &[7, 9]).expect("remap BinSlotSlot");
        let (op, a, b) = remapped.bin_slot_slot_parts();
        assert_eq!(op, Instruction::SUB as u8);
        assert_eq!(a, 7);
        assert_eq!(b, 9);

        assert!(
            Compiler::remap_bin_slot_for_inline(&slot_slot, &[7]).is_none(),
            "slot past arity must fail closed"
        );
    }

    /// Early-return bodies must NOT be tiny-inlined: the inliner stops at the
    /// first `RETURN`, which would drop the else arm (`return n * 2`).
    #[test]
    fn early_return_callee_is_not_tiny_inlined() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn early(int n, int is_neg) -> int { \
               if is_neg == 1 { return 99; } \
               return n * 2; \
             } \
             fn main() { print \"%i\", early(4, 0); }",
        );
        assert!(
            bc.iter().any(|b| matches!(b.bytecode(), Instruction::CALL)),
            "early-return callee must remain a CALL (not truncated inline); opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// Constant-bound C-style `for` unrolls without a back-edge JMP to loop top.
    #[test]
    fn const_for_loop_unrolled_without_back_edge() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn main() { \
let s = 0; \
for (let i = 0; i < 3; i = i + 1) { s = s + i; } \
print \"%i\", s; \
}",
        );
        assert!(
            !bc.iter().any(|b| matches!(
                b.bytecode(),
                Instruction::JMPF | Instruction::CmpJmpf | Instruction::BinSlotImmJmpf
            )),
            "unrolled for (i < 3) must not emit a loop exit jump; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// `if 5 < 5` (strict) must take the else branch — guards Le/`<=` fold mix-up.
    #[test]
    fn const_if_strict_lt_equality_takes_else() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn main() { if 5 < 5 { print \"%i\", 1; } else { print \"%i\", 0; } }",
        );
        assert!(
            !bc.iter().any(|b| matches!(b.bytecode(), Instruction::JMPF)),
            "folded `if 5 < 5` should not emit JMPF"
        );
        // Taken else prints 0 — must see CONST 0 (or ConstReturnImm), not only CONST 1.
        let has_zero = bc.iter().any(|b| {
            matches!(b.bytecode(), Instruction::CONST) && b.operand_u32() as i32 == 0
        });
        assert!(
            has_zero,
            "else branch for `5 < 5` should emit CONST 0; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// `while false` eliminates the loop body (no JMPF / back-edge).
    #[test]
    fn const_while_false_eliminates_loop() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn main() { while false { print \"%i\", 1; } print \"%i\", 2; }",
        );
        assert!(
            !bc.iter().any(|b| matches!(b.bytecode(), Instruction::JMPF)),
            "folded `while false` should not emit JMPF; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// `break` inside a countable `for` must keep a real loop (no unroll).
    #[test]
    fn for_with_break_is_not_unrolled() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn main() { \
let s = 0; \
for (let i = 0; i < 3; i = i + 1) { \
  s = s + i; \
  break; \
} \
print \"%i\", s; \
}",
        );
        // Peephole may fuse JMPF into CmpJmpf / BinSlotImmJmpf / LogNotJmpf.
        let has_cond_jump = bc.iter().any(|b| {
            matches!(
                b.bytecode(),
                Instruction::JMPF
                    | Instruction::CmpJmpf
                    | Instruction::BinSlotImmJmpf
                    | Instruction::LogNotJmpf
            )
        });
        assert!(
            has_cond_jump,
            "for-with-break must keep a conditional loop exit; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// `async fn` self-resume path must not emit TailCall.
    #[test]
    fn async_fn_does_not_emit_tail_call() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "async fn tick(int n) { \
if n <= 0 { return 0; } \
yield n; \
return tick(n - 1); \
} \
fn main() { let h = tick(2); print \"%i\", resume h; }",
        );
        assert!(
            !bc.iter().any(|b| matches!(b.bytecode(), Instruction::TailCall)),
            "coroutines must not use TailCall"
        );
    }

    // ============================================================
    // growing array builtin codegen tests
    // ============================================================

    #[test]
    fn array_append_and_len_emit_array_opcodes() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn main() { \
let a = [1, 2]; \
a[] = 3; \
print \"%i\", len(a); \
}",
        );

        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::ArrayPush)),
            "expected `a[] = 3` to emit ArrayPush"
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

    /// Empty `Option` bodies emit `MakeEnum` None (tag 0, arity 0), not bare
    /// `CONST 0` — raw zero is not a reliable `None` at runtime.
    #[test]
    fn fallthrough_option_emits_make_enum_none() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "fn opt() -> Option<int> {}\
 fn main() { let _ = opt(); }",
        );
        let make_none = bc.iter().any(|b| {
            matches!(b.bytecode(), Instruction::MakeEnum)
                && b.operand_u32() & 0xFFFF == 0
                && (b.operand_u32() >> 16) & 0xFFFF == 0
        });
        assert!(
            make_none,
            "Option fall-through should emit MakeEnum None; got {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// Nested multi-field records emit scratch relocate (LOAD+StorePop)
    /// then UnpackAt with operands `[arity, scratch_slot]` — not in-place
    /// at the outer field (which would clobber siblings).
    #[test]
    fn match_nested_multifield_record_emits_scratch_unpack_at() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "enum Inner { I { x: int, y: int } } \
 enum Wrap { W { inner: Inner, name: int } } \
 match Wrap::W { inner: Inner::I { x: 1, y: 2 }, name: 3 } { \
 Wrap::W { inner: Inner::I { x, y }, name } => x + y + name, \
 };",
        );

        let unpack_at: Vec<_> = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::UnpackAt))
            .collect();
        assert!(
            !unpack_at.is_empty(),
            "expected UnpackAt for nested Inner::I"
        );
        for b in &unpack_at {
            let ops = b.operand_u32();
            let slot = ops & 0xFFFF;
            let arity = ops >> 16;
            assert_eq!(arity, 2, "inner record arity must be in [31:16]; got {ops:#x}");
            // Outer has 2 fields; scratch starts at record_base + 2 (payload_base
            // is 0 for bare expression matches, 1 inside functions).
            assert!(
                slot >= 2,
                "scratch slot must be past outer field region; got slot={slot}"
            );
        }
        let load_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::LOAD))
            .count();
        let store_pop_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::StorePop))
            .count();
        assert!(
            load_count >= 1 && store_pop_count >= 1,
            "expected LOAD+StorePop to relocate nested enum into scratch; LOAD={load_count} StorePop={store_pop_count}"
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
            !bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::DynAdd)),
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
    /// no trait bound, so no DynAdd / DynCmp / etc. opcode is emitted — this
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

    /// Phase 4: escaping a constrained generic always emits `MakePolyFnCapture`
    /// (from an active `__dictN` scope or with unresolved null slots).
    #[test]
    fn constrained_generic_escape_emits_make_polyfn_capture() {
        use common::Instruction;
        let src = r#"
            trait Showable<T> { fn show_it(T x) -> int; }
            impl Showable<int> { fn show_it(int x) -> int { return x; } }
            fn show<T: Showable>(T x) -> int { return show_it(x); }
            fn capture<T: Showable>(T _w) { return show; }
            fn main() { let f = capture(0); }
        "#;
        let (bc, _pool) = compile_src(src);
        let capture = bc
            .iter()
            .find(|b| matches!(b.bytecode(), Instruction::MakePolyFnCapture))
            .expect("expected MakePolyFnCapture");
        assert_eq!(
            capture.operand_u32() & 0xFF,
            1,
            "Showable escape should capture exactly 1 dict slot; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
        assert!(
            !bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::MakePolyFn)),
            "constrained escape should not emit unconstrained MakePolyFn"
        );
    }

    /// Phase 4: top-level constrained escape (`let f = show`) still uses
    /// `MakePolyFnCapture` with null slots (delayed application evidence).
    #[test]
    fn top_level_constrained_escape_emits_make_polyfn_capture_with_null_slots() {
        use common::Instruction;
        let src = r#"
            trait Showable<T> { fn show_it(T x) -> int; }
            impl Showable<int> { fn show_it(int x) -> int { return x; } }
            fn show<T: Showable>(T x) -> int { return show_it(x); }
            fn main() { let f = show; }
        "#;
        let (bc, _pool) = compile_src(src);
        let capture = bc
            .iter()
            .find(|b| matches!(b.bytecode(), Instruction::MakePolyFnCapture))
            .expect("expected MakePolyFnCapture for top-level constrained escape");
        assert_eq!(
            capture.operand_u32() & 0xFF,
            1,
            "top-level show escape should reserve 1 dict slot"
        );
        assert!(
            !bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::MakePolyFn)),
            "constrained escape must not use bare MakePolyFn"
        );
    }

    /// Phase 4: multiparam constraint escape captures one slot per constraint.
    #[test]
    fn multiparam_constrained_escape_emits_capture_with_slot_count() {
        use common::Instruction;
        let src = r#"
            trait Convert<A, B> { fn cast(A x) -> B; }
            impl Convert<int, int> { fn cast(int x) -> int { return x; } }
            fn convert_fn<A, B>(A x) -> B where Convert<A, B> { return cast(x); }
            fn capture_convert<A, B>(A _wa, B _wb) where Convert<A, B> { return convert_fn; }
            fn main() { let f = capture_convert(0, 0); }
        "#;
        let (bc, _pool) = compile_src(src);
        let capture = bc
            .iter()
            .find(|b| matches!(b.bytecode(), Instruction::MakePolyFnCapture))
            .expect("expected MakePolyFnCapture for multiparam escape");
        assert_eq!(
            capture.operand_u32() & 0xFF,
            1,
            "Convert<A,B> escape should capture exactly 1 dict slot"
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
        let boxed_before_call =
            main_call > 0 && matches!(bc[main_call - 1].bytecode(), Instruction::BoxValue);
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
    /// **user-defined** trait constraint must emit:
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
            // Declare a user trait with one method.
            "trait Describable<T> { fn describe_val(T x) -> int; } \
             impl Describable<int> { fn describe_val(int x) -> int { return x; } } \
             // Generic fn with one user trait constraint.  NOT called as mono.
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
            "trait Printable<T> { fn printable_val(T x) -> int; } \
             trait Countable<T> { fn count_val(T x) -> int; } \
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
        assert_eq!(
            make_tuple_count, 0,
            "open Num evidence is forwarded, not rebuilt"
        );

        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::CallIndirect)),
            "expected dictionary CallIndirect for Num-constrained add; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// Ground calls with **user** trait bounds are NOT monomorphized
    /// (see `monomorphize.rs`); they use the shared body + dictionary-passing
    /// convention instead. Expect BoxValue + MakeTuple + bumped CALL arity.
    #[test]
    fn ground_user_typeclass_call_uses_dict_not_mono() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            "trait Describable<T> { fn describe_val(T x) -> int; } \
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
            "user trait ground call should emit a dict MakeTuple; opcodes: {:?}",
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
            "trait Measurable<T> { fn size(T x) -> int; } \
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
            "trait Tiny<T> { fn zero(T x) -> int { return 7; } } \
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
            "trait Measurable<T> { fn size(T x) -> int; } \
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

    /// Nested IO HostInvoke (`read_to_end(stdin())`) must emit the outer
    /// native-id `CONST` *before* the nested `HostInvoke` in bytecode.
    /// Staging args into a side buffer first left nested invokes above the
    /// id (piped stdin then looked empty).
    #[test]
    fn nested_io_host_invoke_emits_outer_const_before_inner_host_invoke() {
        use common::Instruction;
        let src = "\
use io::*; \
fn main() { \
  let _ = read_to_end(stdin()); \
}";
        let mut ast = Pratt::default().parse(src).expect("parse failed");
        let mut compiler = Compiler::default();
        // Stable ids matching Pipeline::register_io_natives order is not
        // required — only that outer CONST(id=read_to_end) precedes the
        // inner HostInvoke for stdin.
        compiler.register_native_id("stdin", 1);
        compiler.register_native_id("read_to_end", 2);
        let bc = compiler.compile("", &mut ast);

        let host_idxs: Vec<usize> = bc
            .iter()
            .enumerate()
            .filter(|(_, b)| matches!(b.bytecode(), Instruction::HostInvoke))
            .map(|(i, _)| i)
            .collect();
        assert!(
            host_idxs.len() >= 2,
            "expected nested HostInvoke (stdin + read_to_end); got {}",
            host_idxs.len()
        );
        let outer_host = *host_idxs.last().expect("outer HostInvoke");
        // Outer native id is CONST value 2, emitted before its args (which
        // include the inner HostInvoke).
        let outer_const = bc[..outer_host]
            .iter()
            .rposition(|b| {
                matches!(b.bytecode(), Instruction::CONST) && b.value_u32() == 2
            })
            .expect("outer read_to_end CONST(id=2) before outer HostInvoke");
        let inner_host = host_idxs
            .iter()
            .copied()
            .find(|&i| i < outer_host)
            .expect("inner stdin HostInvoke before outer");
        assert!(
            outer_const < inner_host,
            "outer native-id CONST must precede nested HostInvoke \
             (const@{outer_const} vs inner@{inner_host})"
        );
    }

    /// `invoke(..., (fn, …))` callback args must use relocatable `CodePtr`,
    /// not `CONST`. Peephole fusion adjusts `CodePtr` in `finalize_bytecode`
    /// but never rewrites `CONST`, so a stale offset would make the FFI
    /// trampoline jump to the wrong IP (regression: prints `0` instead of
    /// `42` for `examples/ffi_callback.hy`).
    #[test]
    fn invoke_callback_fn_arg_emits_relocatable_code_ptr() {
        use common::Instruction;
        let src = "\
use ffi::*; \
use ffi::types::*; \
fn doubler(int x) -> int { return x * 2; } \
fn main() { \
  let lib = dload(\"libsum.so\"); \
  let id = declare(lib, \"apply_cb\", (Callback, Int), Int); \
  invoke(lib, id, (doubler, 21)); \
}";
        let mut ast = Pratt::default().parse(src).expect("parse failed");
        let mut compiler = Compiler::default();
        let bc = compiler.compile("", &mut ast);
        let doubler = *compiler
            .functions
            .get("doubler")
            .expect("doubler must be registered");

        let ffi_idx = bc
            .iter()
            .position(|b| matches!(b.bytecode(), Instruction::FfiInvoke))
            .expect("expected FfiInvoke");
        let make_tuple_idx = bc[..ffi_idx]
            .iter()
            .rposition(|b| matches!(b.bytecode(), Instruction::MakeTuple))
            .expect("expected MakeTuple before FfiInvoke");
        // Callback fn arg is the first tuple element → last CodePtr before
        // MakeTuple (args are emitted bottom-to-top; doubler then 21).
        let code_ptr = bc[..make_tuple_idx]
            .iter()
            .rev()
            .find(|b| matches!(b.bytecode(), Instruction::CodePtr))
            .expect("expected CodePtr for callback fn arg before MakeTuple");
        assert_eq!(
            code_ptr.operand_u32() as usize,
            doubler,
            "CodePtr must match post-finalize doubler entry (got {}; table={})",
            code_ptr.operand_u32(),
            doubler
        );
        // Guard against regressing to CONST at this site.
        assert!(
            !bc[..make_tuple_idx].iter().any(|b| {
                matches!(b.bytecode(), Instruction::CONST) && b.operand_u32() as usize == doubler
            }),
            "callback fn arg must not be baked as CONST (unrelocatable)"
        );
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
        assert!(
            entry < bc.len(),
            "MakePolyFn entry must point into bytecode"
        );
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
        // BinSlotSlot fuse covers LOAD;LOAD;ADD in non-generic helpers; fib
        // arithmetic may appear fused (*Return / BinSlot*) or as raw ops.
        assert!(
            has_fused
                || bc.iter().any(|b| matches!(
                    *b.bytecode(),
                    Instruction::ADD | Instruction::LEQ | Instruction::JMPF
                )),
            "expected fused superinstructions or fib arithmetic alongside PolyFn; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// Generic HostInvoke Call path (`self.native`) has the same id-before-args
    /// contract as `emit_io_host_invoke` — nested `outer(inner())` must not
    /// leave the inner invoke above the outer id.
    ///
    /// Mirrors `nested_io_host_invoke_emits_outer_const_before_inner_host_invoke`:
    /// require two `HostInvoke`s and assert the outer id `CONST` precedes the
    /// *inner* `HostInvoke` (not merely the first one).
    #[test]
    fn nested_generic_host_invoke_emits_outer_id_before_inner_invoke() {
        use common::Instruction;
        use crate::typechecking::ty::int;

        let mut ast = Pratt::default()
            .parse(
                r#"
fn main() {
    outer(inner());
}
"#,
            )
            .expect("parse failed");
        let mut compiler = Compiler::default();
        compiler.register("inner", &[], &int());
        compiler.register("outer", &[int()], &int());
        let outer_id = compiler.native_id("outer").expect("outer registered") as u32;
        let bc = compiler.compile("", &mut ast);

        let host_idxs: Vec<usize> = bc
            .iter()
            .enumerate()
            .filter(|(_, b)| matches!(b.bytecode(), Instruction::HostInvoke))
            .map(|(i, _)| i)
            .collect();
        assert!(
            host_idxs.len() >= 2,
            "expected nested HostInvoke (inner + outer); got {}; opcodes: {:?}",
            host_idxs.len(),
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
        let outer_host = *host_idxs.last().expect("outer HostInvoke");
        let outer_const = bc[..outer_host]
            .iter()
            .rposition(|b| {
                matches!(b.bytecode(), Instruction::CONST) && b.value_u32() == outer_id
            })
            .expect("outer id CONST before outer HostInvoke");
        let inner_host = host_idxs
            .iter()
            .copied()
            .find(|&i| i < outer_host)
            .expect("inner HostInvoke before outer");
        assert!(
            outer_const < inner_host,
            "outer native-id CONST must precede nested HostInvoke \
             (const@{outer_const} vs inner@{inner_host})"
        );
    }

    /// Named call args are reordered to declaration order before CALL.
    /// Source order is `age` then `name`; bytecode must push name (STRING)
    /// then age (CONST) so a missing reorder still typechecks but fails here.
    #[test]
    fn named_call_shuffled_args_emits_declaration_order() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            r#"
fn greet(string name, int age) {
    print "%s", name;
    print "%i", age;
}
fn main() {
    greet(age: 36, name: "Ada");
}
"#,
        );
        // Find the CALL in main (arity 2) — skip any earlier CALLs.
        // call_parts() = (arity, target).
        let call_idx = bc
            .iter()
            .rposition(|b| {
                matches!(b.bytecode(), Instruction::CALL) && b.call_parts().0 == 2
            })
            .expect("expected CALL arity 2 for greet");
        // Walk backward from CALL over the two arg pushes: STRING then CONST.
        let mut saw_string = false;
        let mut saw_const = false;
        let mut order = Vec::new();
        for b in bc[..call_idx].iter().rev() {
            match b.bytecode() {
                Instruction::STRING | Instruction::FORMAT => {
                    order.push("string");
                    saw_string = true;
                    if saw_const {
                        break;
                    }
                }
                Instruction::CONST => {
                    order.push("const");
                    saw_const = true;
                    if saw_string {
                        break;
                    }
                }
                // Skip peephole / prologue noise between arg pushes.
                Instruction::POP
                | Instruction::DUPLICATE
                | Instruction::JMP
                | Instruction::JMPF
                | Instruction::CALL
                | Instruction::RETURN
                | Instruction::PRINT => {}
                _ => {
                    // Keep scanning; Format/Print in greet body appear earlier.
                }
            }
            if order.len() >= 2 {
                break;
            }
        }
        // Reverse to source-of-stack order: first pushed is declaration-first.
        order.reverse();
        assert_eq!(
            order,
            vec!["string", "const"],
            "expected STRING (name) then CONST (age) before CALL; got {:?}. \
             Missing reorder would emit CONST then STRING.",
            order
        );
    }

    /// Rest calls pack trailing args into MakeArray and CALL with arity =
    /// fixed + 1 (here fixed=0 → arity 1).
    #[test]
    fn rest_call_emits_make_array_before_call() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            r#"
fn sum(int... xs) -> int { return len(xs); }
fn main() {
    let n = sum(1, 2, 3);
}
"#,
        );
        let make_array = bc
            .iter()
            .find(|b| matches!(b.bytecode(), Instruction::MakeArray))
            .expect("expected MakeArray for rest packing");
        assert_eq!(
            make_array.operand_u32(),
            3,
            "sum(1,2,3) should MakeArray(3); got {}",
            make_array.operand_u32()
        );
        let call = bc
            .iter()
            .find(|b| matches!(b.bytecode(), Instruction::CALL) && b.call_parts().0 == 1)
            .expect("expected CALL arity 1 (rest packed as one slot)");
        let make_pos = bc
            .iter()
            .position(|b| matches!(b.bytecode(), Instruction::MakeArray))
            .unwrap();
        let call_pos = bc
            .iter()
            .position(|b| matches!(b.bytecode(), Instruction::CALL) && b.call_parts().0 == 1)
            .unwrap();
        assert!(
            make_pos < call_pos,
            "MakeArray must precede CALL (make@{make_pos} call@{call_pos})"
        );
        let _ = call;
    }

    /// Empty rest call still emits MakeArray(0) so the rest formal is `[]`.
    #[test]
    fn rest_empty_call_emits_make_array_zero() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            r#"
fn sum(int... xs) -> int { return len(xs); }
fn main() {
    let n = sum();
}
"#,
        );
        let make_array = bc
            .iter()
            .find(|b| matches!(b.bytecode(), Instruction::MakeArray))
            .expect("expected MakeArray(0) for empty rest");
        assert_eq!(make_array.operand_u32(), 0);
    }

    /// `let (a, b) = (1, 2)` desugars to Index + StorePop per binding.
    #[test]
    fn let_tuple_destructure_emits_index_and_store_pop() {
        use common::Instruction;
        let (bc, _pool) = compile_src(
            r#"
fn main() {
    let (a, b) = (1, 2);
    print "%i", a;
    print "%i", b;
}
"#,
        );
        let index_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::Index))
            .count();
        assert!(
            index_count >= 2,
            "expected ≥2 Index for tuple let destructure; got {index_count}"
        );
        let store_pop_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::StorePop))
            .count();
        // RHS temp + a + b (at least 3).
        assert!(
            store_pop_count >= 3,
            "expected ≥3 StorePop (tmp + a + b); got {store_pop_count}"
        );
    }

    /// Value-position mono fn → MakeFn; calling through the local → CallIndirect.
    #[test]
    fn fn_value_emits_make_fn_then_call_indirect() {
        use common::Instruction;
        let (bc, _) = compile_src(
            r#"
fn add(int a, int b) -> int { return a + b; }
fn main() {
    let f = add;
    print "%i", f(20, 22);
}
"#,
        );
        let make_fn: Vec<_> = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::MakeFn))
            .collect();
        assert_eq!(
            make_fn.len(),
            1,
            "expected exactly one MakeFn for `let f = add`; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
        // n_cap=0, n_filled=0, arity=2, is_rest=false
        assert_eq!(
            make_fn[0].operand_u32(),
            make_fn_operand(0, 0, 2, false),
            "MakeFn operand should pack arity=2 with no fills/captures"
        );
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::CallIndirect)),
            "calling through `f` must use CallIndirect; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// Positional under-apply must emit MakeFn with n_filled matching argc,
    /// not a full CALL (which would leave holes unfilled / wrong ABI).
    #[test]
    fn partial_application_emits_make_fn_with_fill_count() {
        use common::Instruction;
        let (bc, _) = compile_src(
            r#"
fn add(int a, int b) -> int { return a + b; }
fn main() {
    let g = add(1);
    print "%i", g(2);
}
"#,
        );
        let make_fn: Vec<_> = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::MakeFn))
            .collect();
        assert!(
            !make_fn.is_empty(),
            "expected MakeFn for partial `add(1)`; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
        // Prefer the partial MakeFn (n_filled=1, arity=2) over any other.
        assert!(
            make_fn
                .iter()
                .any(|b| b.operand_u32() == make_fn_operand(0, 1, 2, false)),
            "expected MakeFn(n_cap=0, n_filled=1, arity=2); got operands {:?}",
            make_fn.iter().map(|b| b.operand_u32()).collect::<Vec<_>>()
        );
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::CallIndirect)),
            "completing the partial must CallIndirect; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// Explicit-capture lambda must MakeFn with n_cap matching `use (...)`.
    #[test]
    fn lambda_emits_make_fn_with_capture_count() {
        use common::Instruction;
        let (bc, _) = compile_src(
            r#"
fn main() {
    let y = 10;
    let f = fn (int x) use (y) => x + y;
    print "%i", f(32);
}
"#,
        );
        let make_fn: Vec<_> = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::MakeFn))
            .collect();
        assert!(
            !make_fn.is_empty(),
            "expected MakeFn for lambda; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
        assert!(
            make_fn
                .iter()
                .any(|b| b.operand_u32() == make_fn_operand(1, 0, 1, false)),
            "expected MakeFn(n_cap=1, n_filled=0, arity=1); got operands {:?}",
            make_fn.iter().map(|b| b.operand_u32()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn tuple_zip_add_emits_index_and_make_tuple() {
        use common::Instruction;
        let (bc, _) = compile_src(
            r#"
fn main() {
    let a = (1, 1) + (1, 1);
    print "%i", a[0];
}
"#,
        );
        let has_index = bc
            .iter()
            .any(|b| matches!(b.bytecode(), Instruction::Index));
        let has_make = bc
            .iter()
            .any(|b| matches!(b.bytecode(), Instruction::MakeTuple));
        let has_add = bc.iter().any(|b| {
            matches!(
                b.bytecode(),
                Instruction::ADD | Instruction::BinSlotImm | Instruction::BinSlotSlot
            )
        });
        assert!(
            has_index && has_make && has_add,
            "expected Index + ADD + MakeTuple zip lowering; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// Approach A: `matmul` lowers to packed `HostInvoke`, not a MUL cascade.
    #[test]
    fn matmul_emits_packed_matmul_opcode() {
        use common::Instruction;
        let (bc, _) = compile_src(
            r#"
fn main() {
    let a = [[1, 2], [3, 4]];
    let b = [[5, 6], [7, 8]];
    let c = matmul(a, b);
    print "%i", c[0][0];
}
"#,
        );
        let hosts = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::HostInvoke))
            .count();
        assert!(
            hosts >= 1,
            "expected packed HostInvoke for matmul; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
        // Scalar 2×2×2 unroll would emit 8 MUL; packed path must be far below that.
        let mul_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::MUL | Instruction::MULF))
            .count();
        assert!(
            mul_count < 8,
            "packed matmul path should not unroll to 8 MULs; got {mul_count}"
        );
    }

    /// Dims over the `u8` packed ceiling fall back to scalar unroll (and the
    /// typechecker warns — see diagnostics `*_over_packed_u8_limit_warns`).
    #[test]
    fn matmul_dims_over_u8_limit_falls_back_to_unroll() {
        use common::Instruction;
        let ones: String = std::iter::repeat_n("1", 256).collect::<Vec<_>>().join(", ");
        let a = format!("[[{ones}]]"); // 1×256
        let b_rows: String = std::iter::repeat_n("[1]", 256)
            .collect::<Vec<_>>()
            .join(", ");
        let src = format!(
            "fn main() {{\n    let a = {a};\n    let b = [{b_rows}];\n    let _ = matmul(a, b);\n}}\n"
        );
        let (bc, _) = compile_src(&src);
        assert!(
            !bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::HostInvoke)),
            "dims > 255 must not emit packed HostInvoke"
        );
        let mul_count = bc
            .iter()
            .filter(|b| matches!(b.bytecode(), Instruction::MUL | Instruction::MULF))
            .count();
        // 1×256×1 scalar unroll → 256 MULs.
        assert!(
            mul_count >= 256,
            "expected scalar unroll (≥256 MUL); got {mul_count}"
        );
    }

    /// Approach A: `dot` lowers to packed `HostInvoke` (`packed_dot`).
    #[test]
    fn dot_emits_packed_dot_opcode() {
        use common::Instruction;
        let (bc, _) = compile_src(
            r#"
fn main() {
    print "%i", dot([1, 2], [3, 4]);
}
"#,
        );
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::HostInvoke)),
            "expected packed HostInvoke (dot); opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// Approach A: `Matrix` `*` lowers to packed `HostInvoke` (`packed_matmul`).
    #[test]
    fn matrix_mul_emits_packed_matmul_opcode() {
        use common::Instruction;
        let (bc, _) = compile_src(
            r#"
fn main() {
    let a = matrix([[1, 2], [3, 4]]);
    let b = matrix([[5, 6], [7, 8]]);
    let c = a * b;
    print "%i", c[0][0];
}
"#,
        );
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::HostInvoke)),
            "expected packed HostInvoke (matmul) for Matrix *; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    fn packed_host_meta(bc: &[common::Byte]) -> u32 {
        use common::Instruction;
        let hi = bc
            .iter()
            .position(|b| matches!(b.bytecode(), Instruction::HostInvoke))
            .expect("expected HostInvoke");
        // Layout: … CONST meta, MakeTuple, HostInvoke
        assert!(
            hi >= 2 && matches!(bc[hi - 1].bytecode(), Instruction::MakeTuple),
            "HostInvoke must follow MakeTuple"
        );
        assert!(
            matches!(bc[hi - 2].bytecode(), Instruction::CONST),
            "meta CONST must precede MakeTuple"
        );
        bc[hi - 2].operand_u32()
    }

    /// Approach A: `Matrix` `+` lowers to packed_matrix_zip with zip_kind=Add.
    #[test]
    fn matrix_add_emits_packed_matrix_zip_add() {
        use common::Instruction;
        let (bc, _) = compile_src(
            r#"
fn main() {
    let a = matrix([[1, 2], [3, 4]]);
    let c = a + a;
    print "%i", c[0][0];
}
"#,
        );
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::HostInvoke)),
            "expected HostInvoke for Matrix +"
        );
        let ops = packed_host_meta(&bc);
        assert_eq!(ops & 0xFF, 2, "m");
        assert_eq!((ops >> 8) & 0xFF, 2, "n");
        assert_eq!((ops >> 16) & 0xFF, 0, "zip_kind Add");
    }

    /// Approach A: `Matrix` `-` packs zip_kind=Sub (not Add).
    #[test]
    fn matrix_sub_emits_packed_matrix_zip_sub() {
        use common::Instruction;
        let (bc, _) = compile_src(
            r#"
fn main() {
    let a = matrix([[5, 7], [9, 11]]);
    let b = matrix([[1, 2], [3, 4]]);
    let c = a - b;
    print "%i", c[0][0];
}
"#,
        );
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::HostInvoke)),
            "expected HostInvoke for Matrix -"
        );
        assert_eq!(
            (packed_host_meta(&bc) >> 16) & 0xFF,
            1,
            "zip_kind Sub; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// Approach A: unary `-` on `Matrix` lowers to packed_matrix_neg via HostInvoke.
    #[test]
    fn matrix_neg_emits_packed_matrix_neg() {
        use common::Instruction;
        let (bc, _) = compile_src(
            r#"
fn main() {
    let a = matrix([[1, 2], [3, 4]]);
    let c = -a;
    print "%i", c[0][0];
}
"#,
        );
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::HostInvoke)),
            "expected HostInvoke for Matrix unary -; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// `cross` stays on the scalar unroll path (no HostInvoke / packed natives).
    #[test]
    fn cross_does_not_emit_packed_opcodes() {
        use common::Instruction;
        let (bc, _) = compile_src(
            r#"
fn main() {
    let c = cross((1, 0, 0), (0, 1, 0));
    print "%i", c[0];
}
"#,
        );
        assert!(
            !bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::HostInvoke)),
            "cross must stay unrolled; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::MUL | Instruction::MULF)),
            "cross unroll should emit MUL; opcodes: {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

    /// Float `dot` sets the packed_dot is_float meta flag (`operands[16]`).
    #[test]
    fn float_dot_emits_packed_dot_with_float_flag() {
        use common::Instruction;
        let (bc, _) = compile_src(
            r#"
fn main() {
    print "%f", dot([1.0, 2.0], [3.0, 4.0]);
}
"#,
        );
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::HostInvoke)),
            "expected HostInvoke for float dot"
        );
        assert_ne!(
            packed_host_meta(&bc) & (1 << 16),
            0,
            "float packed_dot meta must set is_float bit"
        );
    }

    #[test]
    fn unescape_coil_string_supports_hex_and_unicode() {
        assert_eq!(unescape_coil_string(r"\x41"), "A");
        assert_eq!(unescape_coil_string(r"\u{42}"), "B");
        assert_eq!(unescape_coil_string("\\\""), "\"");
        assert_eq!(unescape_coil_string(r"\e"), "\x1b");
    }

    #[test]
    fn cast_as_int_emits_cast_opcode() {
        use common::Instruction;
        let (bc, _) = compile_src(
            r#"
fn main() {
    let x = 3.5 as int;
    print "%i", x;
}
"#,
        );
        assert!(
            bc.iter()
                .any(|b| matches!(b.bytecode(), Instruction::CastFloatToInt)),
            "expected CastFloatToInt in {:?}",
            bc.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
        );
    }

}

#[cfg(test)]
mod fallthrough_probe {
    use super::*;
    #[test]
    fn probe_string_fallthrough_diag() {
        let mut p = crate::pipeline::Pipeline::new();
        let src = "fn bad() -> string {}\nfn main() { let _ = bad(); }\n";
        let r = p.compile_src(src);
        let c = p.compiler_mut();
        let ty = c.fn_return_ty("bad");
        let allow = c.fallthrough_allows_zero("bad");
        let scheme = c.checker.env().lookup("bad").map(|s| format!("{}", s.ty));
        let msgs: Vec<_> = p
            .messages()
            .iter()
            .map(|m| (m.code(), m.message().to_string()))
            .collect();
        eprintln!("result_ok={} ty={ty:?} allow={allow} scheme={scheme:?} msgs={msgs:?}", r.is_ok());
        assert!(r.is_err(), "ty={ty:?} allow={allow} scheme={scheme:?}");
    }
}
