//! Static recursion-depth / operand-stack bound analysis.
//!
//! For self-recursive functions with a proven decreasing int measure and a
//! base-case branch (`if n <= K`) plus **constant** entry call sites (e.g.
//! `fib(10)` / `fib(32)`), the maximum call-frame depth is computed and no
//! attribute is required. When depth cannot be proven (dynamic args, FFI-shaped
//! recursion, mutual cycles without a measure, …), `#[max_depth(N)]` is
//! required on the recursive function.

use std::collections::{BTreeSet, HashMap, HashSet};

use parser::ast::{AttrArgs, AttrLit, Attribute, Expression, Output};
use reporting::{ErrorCode, Message};

use super::par_profit::{RecParShape, analyze_rec_par_shapes};
use super::purity::analyze_recursive_fns;

/// Default / minimum operand-stack capacity for programs without deep recursion.
pub const DEFAULT_OPERAND_STACK_SLOTS: u32 = 256;

/// Hard ceiling matching [`machine::MAX_OPERAND_STACK_SLOTS`].
pub const MAX_OPERAND_STACK_SLOTS: u32 = 1_048_576;

/// Conservative per-frame slot estimate when IL footprints are not yet known.
const DEFAULT_FRAME_SLOTS: u32 = 16;

/// Compute operand-stack slots from a max live-frame count.
pub fn operand_slots_for_frames(max_frames: u32) -> u32 {
    let need = max_frames
        .saturating_mul(DEFAULT_FRAME_SLOTS)
        .saturating_add(DEFAULT_FRAME_SLOTS);
    need.max(DEFAULT_OPERAND_STACK_SLOTS)
        .min(MAX_OPERAND_STACK_SLOTS)
}

/// Proven or attributed max live frames for one recursive function.
#[derive(Debug, Clone)]
#[allow(dead_code)] // consumed by pipeline / future sizing; fields read in tests
pub struct FnStackBound {
    pub fn_name: String,
    /// Maximum simultaneous frames of this function (and its SCC peers).
    pub max_frames: u32,
    /// How the bound was obtained.
    pub source: BoundSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundSource {
    /// Decreasing measure + constant entry args.
    Proven,
    /// User `#[max_depth(N)]`.
    Attribute,
    /// All recursive calls are tail calls (`return f(...)`).
    TailOnly,
}

/// Result of whole-program recursion bound checking.
#[derive(Debug, Default)]
pub struct StackBoundReport {
    pub messages: Vec<Message>,
    pub bounds: Vec<FnStackBound>,
    /// Conservative operand-stack slots required (`max_frames * frame_slots` + slack).
    pub operand_slots_needed: u32,
}

/// Analyze recursion depth bounds; emit errors when `#[max_depth]` is required.
pub fn analyze_stack_bounds(ast: &Output<'_>) -> StackBoundReport {
    let mut report = StackBoundReport {
        operand_slots_needed: DEFAULT_OPERAND_STACK_SLOTS,
        ..StackBoundReport::default()
    };

    let recursive = analyze_recursive_fns(ast);
    if recursive.is_empty() {
        return report;
    }

    // Binary `f(n-a)⊕f(n-b)` shapes (same detector as auto-par).
    let bin_shapes = analyze_rec_par_shapes(ast, &recursive);
    let mut unary_shapes: HashMap<String, UnaryRecShape> = HashMap::new();
    let mut tail_only: HashSet<String> = HashSet::new();
    let mut fn_meta: HashMap<String, FnMeta<'_>> = HashMap::new();
    collect_fn_meta(ast, &recursive, &mut fn_meta, &mut unary_shapes, &mut tail_only);

    let mut const_args: HashMap<String, BTreeSet<i64>> = HashMap::new();
    let mut dynamic_calls: HashSet<String> = HashSet::new();
    collect_rec_call_sites(ast, &recursive, &mut const_args, &mut dynamic_calls);

    let mut max_frames_any: u32 = 1;

    for name in &recursive {
        let span = fn_meta
            .get(name)
            .map(|m| m.span.clone())
            .unwrap_or(0..0);
        let attr_depth = fn_meta.get(name).and_then(|m| m.max_depth);
        let attr_err = fn_meta.get(name).and_then(|m| m.attr_error.clone());
        if let Some(msg) = attr_err {
            report.messages.push(msg);
            continue;
        }

        // Mutual recursion without a self-measure: require attribute.
        let is_self = fn_meta.get(name).map(|m| m.self_recursive).unwrap_or(false);
        if !is_self {
            match attr_depth {
                Some(d) => {
                    max_frames_any = max_frames_any.max(d);
                    report.bounds.push(FnStackBound {
                        fn_name: name.clone(),
                        max_frames: d,
                        source: BoundSource::Attribute,
                    });
                }
                None => report.messages.push(Message::error(
                    ErrorCode::UnboundedRecursion,
                    format!(
                        "recursive function `{name}` participates in mutual recursion; \
                         add `#[max_depth(N)]` with a safe upper bound on call-frame depth"
                    ),
                    span.clone(),
                )),
            }
            continue;
        }

        if tail_only.contains(name) {
            let frames = attr_depth.unwrap_or(1);
            max_frames_any = max_frames_any.max(frames);
            report.bounds.push(FnStackBound {
                fn_name: name.clone(),
                max_frames: frames,
                source: if attr_depth.is_some() {
                    BoundSource::Attribute
                } else {
                    BoundSource::TailOnly
                },
            });
            continue;
        }

        let has_dynamic = dynamic_calls.contains(name);
        let consts = const_args.get(name);
        let has_const_entry = consts.is_some_and(|s| !s.is_empty());
        // No entry call sites in this unit (e.g. `test(...)` stripped for a
        // normal run): nothing to bound and no runtime depth risk here.
        if !has_dynamic && !has_const_entry {
            continue;
        }

        // Prefer binary shape, then unary.
        let proven = if let Some(shape) = bin_shapes.get(name) {
            prove_bin_depth(shape, consts, has_dynamic)
        } else if let Some(shape) = unary_shapes.get(name) {
            prove_unary_depth(shape, consts, has_dynamic)
        } else {
            DepthProof::Unprovable
        };

        match (proven, attr_depth) {
            (DepthProof::Frames(n), _) => {
                let frames = attr_depth.map(|a| a.max(n)).unwrap_or(n);
                max_frames_any = max_frames_any.max(frames);
                report.bounds.push(FnStackBound {
                    fn_name: name.clone(),
                    max_frames: frames,
                    source: if attr_depth.is_some_and(|a| a >= n) {
                        BoundSource::Attribute
                    } else {
                        BoundSource::Proven
                    },
                });
            }
            (DepthProof::Unprovable, Some(d)) => {
                max_frames_any = max_frames_any.max(d);
                report.bounds.push(FnStackBound {
                    fn_name: name.clone(),
                    max_frames: d,
                    source: BoundSource::Attribute,
                });
            }
            (DepthProof::Unprovable, None) => {
                let reason = if has_dynamic {
                    "is called with a non-constant argument"
                } else if bin_shapes.get(name).is_some_and(|s| s.base_bound.is_none())
                    || unary_shapes.get(name).is_some_and(|s| s.base_bound.is_none())
                {
                    "needs a recognizable base case (`if n <= K` / `n < K` / `n == K`)"
                } else if bin_shapes.contains_key(name) || unary_shapes.contains_key(name) {
                    "has no constant entry call site to bound its measure"
                } else {
                    "has no analyzable decreasing measure / base-case shape"
                };
                report.messages.push(Message::error(
                    ErrorCode::UnboundedRecursion,
                    format!(
                        "recursive function `{name}` {reason}; \
                         add `#[max_depth(N)]` with a safe upper bound on call-frame depth"
                    ),
                    span,
                ));
            }
        }
    }

    report.operand_slots_needed = operand_slots_for_frames(max_frames_any);
    if max_frames_any
        .saturating_mul(DEFAULT_FRAME_SLOTS)
        .saturating_add(DEFAULT_FRAME_SLOTS)
        > MAX_OPERAND_STACK_SLOTS
    {
        report.messages.push(Message::error(
            ErrorCode::StackDepthExceeded,
            format!(
                "estimated operand stack need exceeds the VM limit of {MAX_OPERAND_STACK_SLOTS} slots"
            ),
            0..0,
        ));
    }

    report
}

#[derive(Debug, Clone, Copy)]
enum DepthProof {
    Frames(u32),
    Unprovable,
}

#[derive(Debug, Clone)]
struct UnaryRecShape {
    #[allow(dead_code)]
    param: String,
    /// `f(n - sub)`.
    sub: i64,
    base_bound: Option<i64>,
}

struct FnMeta<'a> {
    span: std::ops::Range<usize>,
    max_depth: Option<u32>,
    attr_error: Option<Message>,
    self_recursive: bool,
    #[allow(dead_code)]
    attrs: &'a [Attribute<'a>],
}

fn prove_bin_depth(
    shape: &RecParShape,
    consts: Option<&BTreeSet<i64>>,
    has_dynamic: bool,
) -> DepthProof {
    if has_dynamic {
        return DepthProof::Unprovable;
    }
    let Some(set) = consts else {
        return DepthProof::Unprovable;
    };
    if set.is_empty() {
        return DepthProof::Unprovable;
    }
    let Some(base) = shape.base_bound else {
        return DepthProof::Unprovable;
    };
    let step = shape.left_sub.min(shape.right_sub).max(1);
    let mut max_d = 1u32;
    for &n in set {
        max_d = max_d.max(measure_depth(n, base, step));
    }
    DepthProof::Frames(max_d)
}

fn prove_unary_depth(
    shape: &UnaryRecShape,
    consts: Option<&BTreeSet<i64>>,
    has_dynamic: bool,
) -> DepthProof {
    if has_dynamic {
        return DepthProof::Unprovable;
    }
    let Some(set) = consts else {
        return DepthProof::Unprovable;
    };
    if set.is_empty() {
        return DepthProof::Unprovable;
    }
    let Some(base) = shape.base_bound else {
        return DepthProof::Unprovable;
    };
    let step = shape.sub.max(1);
    let mut max_d = 1u32;
    for &n in set {
        max_d = max_d.max(measure_depth(n, base, step));
    }
    DepthProof::Frames(max_d)
}

/// Worst-case frames along a chain that decreases `n` by `step` until `n <= base`.
fn measure_depth(n: i64, base: i64, step: i64) -> u32 {
    if n <= base {
        return 1;
    }
    let step = step.max(1) as u64;
    let delta = (n - base) as u64;
    (delta.div_ceil(step) as u32).saturating_add(1)
}

fn parse_max_depth_attr(
    attrs: &[Attribute<'_>],
    span: std::ops::Range<usize>,
) -> (Option<u32>, Option<Message>) {
    let Some(attr) = attrs.iter().find(|a| a.name == "max_depth") else {
        return (None, None);
    };
    let n = match &attr.args {
        AttrArgs::Positional(lits) if lits.len() == 1 => match &lits[0] {
            AttrLit::Int(v) if *v > 0 && *v <= u32::MAX as i64 => Some(*v as u32),
            AttrLit::Int(_) => None,
            _ => None,
        },
        AttrArgs::KeyValues(kvs) => {
            let mut found = None;
            for (k, v) in kvs {
                if *k == "n" || *k == "depth" {
                    if let AttrLit::Int(i) = v
                        && *i > 0
                        && *i <= u32::MAX as i64
                    {
                        found = Some(*i as u32);
                    }
                }
            }
            found
        }
        _ => None,
    };
    match n {
        Some(v) => (Some(v), None),
        None => (
            None,
            Some(Message::error(
                ErrorCode::GenericTypeError,
                "`#[max_depth(N)]` requires a positive integer depth \
                 (e.g. `#[max_depth(64)]`)"
                    .to_string(),
                span,
            )),
        ),
    }
}

fn collect_fn_meta<'a>(
    ast: &'a Output<'a>,
    recursive: &HashSet<String>,
    out: &mut HashMap<String, FnMeta<'a>>,
    unary: &mut HashMap<String, UnaryRecShape>,
    tail_only: &mut HashSet<String>,
) {
    match ast.1.as_ref() {
        Expression::Program(items) | Expression::Block(items) | Expression::Fragment(items) => {
            for item in items {
                collect_fn_meta(item, recursive, out, unary, tail_only);
            }
        }
        Expression::Module(_, body) => collect_fn_meta(body, recursive, out, unary, tail_only),
        Expression::Statement(inner)
        | Expression::Expr(inner)
        | Expression::ExprStatement(inner)
        | Expression::Group(inner) => collect_fn_meta(inner, recursive, out, unary, tail_only),
        Expression::Function {
            attrs,
            name,
            args,
            body: Some(body),
            ..
        } if recursive.contains(*name) => {
            let span = ast.0.into_range();
            let (max_depth, attr_error) = parse_max_depth_attr(attrs, span.clone());
            let self_recursive = body_calls_self(body, name);
            out.insert(
                (*name).to_string(),
                FnMeta {
                    span,
                    max_depth,
                    attr_error,
                    self_recursive,
                    attrs,
                },
            );
            if let Some(shape) = detect_unary_shape(name, args, body) {
                unary.insert((*name).to_string(), shape);
            }
            if is_tail_only_recursive(body, name) {
                tail_only.insert((*name).to_string());
            }
            collect_fn_meta(body, recursive, out, unary, tail_only);
        }
        Expression::Function {
            body: Some(body), ..
        } => collect_fn_meta(body, recursive, out, unary, tail_only),
        Expression::Implementation { methods, .. } => {
            for m in methods {
                collect_fn_meta(m, recursive, out, unary, tail_only);
            }
        }
        Expression::Method(_, inner) | Expression::Member(inner) => {
            collect_fn_meta(inner, recursive, out, unary, tail_only);
        }
        _ => {}
    }
}

fn body_calls_self(body: &Output<'_>, name: &str) -> bool {
    let mut found = false;
    walk_calls(body, &mut |callee, _| {
        if callee == name {
            found = true;
        }
    });
    found
}

fn detect_unary_shape(name: &str, args: &Output<'_>, body: &Output<'_>) -> Option<UnaryRecShape> {
    let param = single_int_param(args)?;
    let mut base = None;
    let mut sub = None;
    walk_unary_shape(body, name, &param, &mut base, &mut sub);
    let sub = sub?;
    if sub <= 0 {
        return None;
    }
    // Dual recursive binop is handled by RecParShape; skip if both arms recurse.
    if looks_like_dual_rec(body, name) {
        return None;
    }
    Some(UnaryRecShape {
        param,
        sub,
        base_bound: base,
    })
}

fn looks_like_dual_rec(body: &Output<'_>, name: &str) -> bool {
    let mut count = 0;
    walk_calls(body, &mut |callee, _| {
        if callee == name {
            count += 1;
        }
    });
    count >= 2
}

fn walk_unary_shape(
    ast: &Output<'_>,
    fn_name: &str,
    param: &str,
    base: &mut Option<i64>,
    sub: &mut Option<i64>,
) {
    match ast.1.as_ref() {
        Expression::Program(items)
        | Expression::Block(items)
        | Expression::Fragment(items)
        | Expression::If(items) => {
            for item in items {
                walk_unary_shape(item, fn_name, param, base, sub);
            }
        }
        Expression::Branch(cond, body) => {
            if let Some(c) = cond
                && let Some(b) = match_base_bound(c, param)
            {
                *base = Some(b);
            }
            walk_unary_shape(body, fn_name, param, base, sub);
        }
        Expression::Statement(inner)
        | Expression::Expr(inner)
        | Expression::ExprStatement(inner)
        | Expression::Group(inner)
        | Expression::Return(inner)
        | Expression::ImplicitReturn(inner) => {
            if let Some(s) = match_unary_rec_call(inner, fn_name, param) {
                *sub = Some(s);
            }
            walk_unary_shape(inner, fn_name, param, base, sub);
        }
        Expression::Mul(a, b) | Expression::Add(a, b) | Expression::Sub(a, b) => {
            if let Some(s) = match_unary_rec_call(a, fn_name, param) {
                *sub = Some(s);
            }
            if let Some(s) = match_unary_rec_call(b, fn_name, param) {
                *sub = Some(s);
            }
            walk_unary_shape(a, fn_name, param, base, sub);
            walk_unary_shape(b, fn_name, param, base, sub);
        }
        _ => {}
    }
}

fn match_unary_rec_call(expr: &Output<'_>, fn_name: &str, param: &str) -> Option<i64> {
    let expr = peel(expr);
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
    let callee = match peel(name).1.as_ref() {
        Expression::Identifier(n) => *n,
        _ => return None,
    };
    if callee != fn_name {
        return None;
    }
    match_param_minus_const(peel(&args[0]), param)
}

fn is_tail_only_recursive(body: &Output<'_>, name: &str) -> bool {
    let mut self_calls = 0;
    let mut non_tail = false;
    walk_tail_rec(body, name, true, &mut self_calls, &mut non_tail);
    self_calls > 0 && !non_tail
}

fn walk_tail_rec(
    ast: &Output<'_>,
    name: &str,
    tail_ctx: bool,
    self_calls: &mut i32,
    non_tail: &mut bool,
) {
    match ast.1.as_ref() {
        Expression::Program(items) | Expression::Block(items) | Expression::Fragment(items) => {
            let n = items.len();
            for (i, item) in items.iter().enumerate() {
                walk_tail_rec(item, name, tail_ctx && i + 1 == n, self_calls, non_tail);
            }
        }
        Expression::If(branches) => {
            for b in branches {
                walk_tail_rec(b, name, tail_ctx, self_calls, non_tail);
            }
        }
        Expression::Branch(_, body) => walk_tail_rec(body, name, tail_ctx, self_calls, non_tail),
        Expression::Return(inner) | Expression::ImplicitReturn(inner) => {
            walk_tail_rec(inner, name, true, self_calls, non_tail);
        }
        Expression::Statement(inner)
        | Expression::Expr(inner)
        | Expression::ExprStatement(inner)
        | Expression::Group(inner) => walk_tail_rec(inner, name, tail_ctx, self_calls, non_tail),
        Expression::Call {
            name: callee,
            args,
        } => {
            let is_self = matches!(peel(callee).1.as_ref(), Expression::Identifier(n) if *n == name);
            if is_self {
                *self_calls += 1;
                if !tail_ctx {
                    *non_tail = true;
                }
            }
            if let Some(args) = args {
                for a in args {
                    walk_tail_rec(a, name, false, self_calls, non_tail);
                }
            }
        }
        Expression::Add(a, b)
        | Expression::Sub(a, b)
        | Expression::Mul(a, b)
        | Expression::Div(a, b)
        | Expression::Mod(a, b) => {
            walk_tail_rec(a, name, false, self_calls, non_tail);
            walk_tail_rec(b, name, false, self_calls, non_tail);
        }
        _ => {}
    }
}

fn collect_rec_call_sites(
    ast: &Output<'_>,
    recursive: &HashSet<String>,
    consts: &mut HashMap<String, BTreeSet<i64>>,
    dynamic: &mut HashSet<String>,
) {
    walk_entry_calls(ast, recursive, None, consts, dynamic);
}

/// Collect *entry* call sites (calls from outside a function into a recursive
/// function). Self-calls inside the body (`fib(n - 1)`) are not entries.
fn walk_entry_calls(
    ast: &Output<'_>,
    recursive: &HashSet<String>,
    inside: Option<&str>,
    consts: &mut HashMap<String, BTreeSet<i64>>,
    dynamic: &mut HashSet<String>,
) {
    match ast.1.as_ref() {
        Expression::Program(items)
        | Expression::Block(items)
        | Expression::Fragment(items)
        | Expression::List(items)
        | Expression::Array(items)
        | Expression::Tuple(items)
        | Expression::If(items) => {
            for item in items {
                walk_entry_calls(item, recursive, inside, consts, dynamic);
            }
        }
        Expression::Module(_, body)
        | Expression::Statement(body)
        | Expression::Expr(body)
        | Expression::ExprStatement(body)
        | Expression::Group(body)
        | Expression::Return(body)
        | Expression::ImplicitReturn(body)
        | Expression::Negate(body)
        | Expression::Not(body)
        | Expression::LogicalNot(body)
        | Expression::Positive(body)
        | Expression::Cast(body, _)
        | Expression::Try(body)
        | Expression::Readonly(body)
        | Expression::Panic(body)
        | Expression::Raise(body)
        | Expression::Yield(body)
        | Expression::YieldFrom(body)
        | Expression::TypeOf(body) => {
            walk_entry_calls(body, recursive, inside, consts, dynamic);
        }
        Expression::Add(a, b)
        | Expression::Sub(a, b)
        | Expression::Mul(a, b)
        | Expression::Div(a, b)
        | Expression::Mod(a, b)
        | Expression::Assignment(a, b)
        | Expression::Eq(a, b)
        | Expression::Neq(a, b)
        | Expression::Le(a, b)
        | Expression::Gt(a, b)
        | Expression::Leq(a, b)
        | Expression::Geq(a, b)
        | Expression::Coalesce(a, b) => {
            walk_entry_calls(a, recursive, inside, consts, dynamic);
            walk_entry_calls(b, recursive, inside, consts, dynamic);
        }
        Expression::Call { name, args } => {
            walk_entry_calls(name, recursive, inside, consts, dynamic);
            let callee = match peel(name).1.as_ref() {
                Expression::Identifier(n) => Some(*n),
                _ => None,
            };
            if let Some(args) = args {
                for a in args {
                    walk_entry_calls(a, recursive, inside, consts, dynamic);
                }
            }
            let Some(cname) = callee else {
                return;
            };
            if !recursive.contains(cname) {
                return;
            }
            // Self-calls inside the recursive body are the measure, not entries.
            if inside == Some(cname) {
                return;
            }
            match args {
                Some(args) if args.len() == 1 => {
                    if let Expression::Integer(n) = peel(&args[0]).1.as_ref() {
                        consts.entry(cname.to_string()).or_default().insert(*n);
                    } else {
                        dynamic.insert(cname.to_string());
                    }
                }
                _ => {
                    dynamic.insert(cname.to_string());
                }
            }
        }
        Expression::Branch(cond, body) => {
            if let Some(c) = cond {
                walk_entry_calls(c, recursive, inside, consts, dynamic);
            }
            walk_entry_calls(body, recursive, inside, consts, dynamic);
        }
        Expression::Match { scrutinee, arms } => {
            walk_entry_calls(scrutinee, recursive, inside, consts, dynamic);
            for arm in arms {
                walk_entry_calls(&arm.body, recursive, inside, consts, dynamic);
            }
        }
        Expression::For {
            init,
            cond,
            step,
            body,
        } => {
            if let Some(i) = init {
                walk_entry_calls(i, recursive, inside, consts, dynamic);
            }
            walk_entry_calls(cond, recursive, inside, consts, dynamic);
            if let Some(s) = step {
                walk_entry_calls(s, recursive, inside, consts, dynamic);
            }
            walk_entry_calls(body, recursive, inside, consts, dynamic);
        }
        Expression::Loop {
            identifier,
            iterable,
            body,
        } => {
            if let Some(id) = identifier {
                walk_entry_calls(id, recursive, inside, consts, dynamic);
            }
            walk_entry_calls(iterable, recursive, inside, consts, dynamic);
            walk_entry_calls(body, recursive, inside, consts, dynamic);
        }
        Expression::Variable(_, Some(init)) | Expression::Constant(_, Some(init)) => {
            walk_entry_calls(init, recursive, inside, consts, dynamic);
        }
        Expression::Function {
            name,
            body: Some(body),
            ..
        } => {
            walk_entry_calls(body, recursive, Some(*name), consts, dynamic);
        }
        Expression::Lambda { body, .. } | Expression::Defer { body, .. } => {
            walk_entry_calls(body, recursive, inside, consts, dynamic);
        }
        Expression::TestCase { body, .. } => {
            // Harness cases are entry contexts (not inside the recursive fn).
            walk_entry_calls(body, recursive, None, consts, dynamic);
        }
        Expression::Implementation { methods, .. } => {
            for m in methods {
                walk_entry_calls(m, recursive, inside, consts, dynamic);
            }
        }
        Expression::Method(_, inner) | Expression::Member(inner) => {
            walk_entry_calls(inner, recursive, inside, consts, dynamic);
        }
        _ => {}
    }
}

fn walk_calls(ast: &Output<'_>, f: &mut dyn FnMut(&str, Option<i64>)) {
    match ast.1.as_ref() {
        Expression::Program(items)
        | Expression::Block(items)
        | Expression::Fragment(items)
        | Expression::List(items)
        | Expression::Array(items)
        | Expression::Tuple(items)
        | Expression::If(items) => {
            for item in items {
                walk_calls(item, f);
            }
        }
        Expression::Module(_, body)
        | Expression::Statement(body)
        | Expression::Expr(body)
        | Expression::ExprStatement(body)
        | Expression::Group(body)
        | Expression::Return(body)
        | Expression::ImplicitReturn(body)
        | Expression::Negate(body)
        | Expression::Not(body)
        | Expression::LogicalNot(body)
        | Expression::Positive(body)
        | Expression::Cast(body, _)
        | Expression::Try(body)
        | Expression::Readonly(body) => walk_calls(body, f),
        Expression::Add(a, b)
        | Expression::Sub(a, b)
        | Expression::Mul(a, b)
        | Expression::Div(a, b)
        | Expression::Mod(a, b)
        | Expression::Assignment(a, b)
        | Expression::Eq(a, b)
        | Expression::Neq(a, b)
        | Expression::Le(a, b)
        | Expression::Gt(a, b)
        | Expression::Leq(a, b)
        | Expression::Geq(a, b)
        | Expression::Coalesce(a, b) => {
            walk_calls(a, f);
            walk_calls(b, f);
        }
        Expression::Call { name, args } => {
            walk_calls(name, f);
            let callee = match peel(name).1.as_ref() {
                Expression::Identifier(n) => Some(*n),
                _ => None,
            };
            if let Some(args) = args {
                for a in args {
                    walk_calls(a, f);
                }
                if let Some(cname) = callee {
                    if args.len() == 1 {
                        if let Expression::Integer(n) = peel(&args[0]).1.as_ref() {
                            f(cname, Some(*n));
                        } else {
                            f(cname, None);
                        }
                    } else {
                        f(cname, None);
                    }
                }
            } else if let Some(cname) = callee {
                f(cname, None);
            }
        }
        Expression::Branch(cond, body) => {
            if let Some(c) = cond {
                walk_calls(c, f);
            }
            walk_calls(body, f);
        }
        Expression::Match { scrutinee, arms } => {
            walk_calls(scrutinee, f);
            for arm in arms {
                walk_calls(&arm.body, f);
            }
        }
        Expression::For {
            init,
            cond,
            step,
            body,
        } => {
            if let Some(i) = init {
                walk_calls(i, f);
            }
            walk_calls(cond, f);
            if let Some(s) = step {
                walk_calls(s, f);
            }
            walk_calls(body, f);
        }
        Expression::Loop {
            identifier,
            iterable,
            body,
        } => {
            if let Some(id) = identifier {
                walk_calls(id, f);
            }
            walk_calls(iterable, f);
            walk_calls(body, f);
        }
        Expression::Variable(_, Some(init)) | Expression::Constant(_, Some(init)) => {
            walk_calls(init, f);
        }
        Expression::Function {
            body: Some(body), ..
        }
        | Expression::Lambda { body, .. }
        | Expression::Defer { body, .. }
        | Expression::TestCase { body, .. } => walk_calls(body, f),
        Expression::Implementation { methods, .. } => {
            for m in methods {
                walk_calls(m, f);
            }
        }
        Expression::Method(_, inner) | Expression::Member(inner) => walk_calls(inner, f),
        _ => {}
    }
}

fn single_int_param(args: &Output<'_>) -> Option<String> {
    let items = match args.1.as_ref() {
        Expression::Fragment(items) | Expression::Block(items) => items.as_slice(),
        _ => return None,
    };
    let mut found = None;
    for item in items {
        let arg = peel(item);
        let Expression::Argument { name, ty, .. } = arg.1.as_ref() else {
            continue;
        };
        let Some(ty) = ty else {
            return None;
        };
        let ty_name = match peel(ty).1.as_ref() {
            Expression::Type(t) | Expression::Identifier(t) => *t,
            _ => continue,
        };
        if !matches!(ty_name, "int" | "byte") {
            return None;
        }
        if found.is_some() {
            return None;
        }
        found = Some((*name).to_string());
    }
    found
}

fn match_base_bound(cond: &Output<'_>, param: &str) -> Option<i64> {
    let cond = peel(cond);
    match cond.1.as_ref() {
        Expression::Leq(lhs, rhs) | Expression::Le(lhs, rhs) => {
            let lhs = peel(lhs);
            let rhs = peel(rhs);
            let is_le = matches!(cond.1.as_ref(), Expression::Leq(_, _));
            match (lhs.1.as_ref(), rhs.1.as_ref()) {
                (Expression::Identifier(p), Expression::Integer(k)) if *p == param => {
                    Some(if is_le { *k } else { *k - 1 })
                }
                _ => None,
            }
        }
        Expression::Eq(lhs, rhs) => {
            let lhs = peel(lhs);
            let rhs = peel(rhs);
            match (lhs.1.as_ref(), rhs.1.as_ref()) {
                (Expression::Identifier(p), Expression::Integer(k)) if *p == param => Some(*k),
                (Expression::Integer(k), Expression::Identifier(p)) if *p == param => Some(*k),
                _ => None,
            }
        }
        _ => None,
    }
}

fn match_param_minus_const(expr: &Output<'_>, param: &str) -> Option<i64> {
    let expr = peel(expr);
    match expr.1.as_ref() {
        Expression::Sub(lhs, rhs) => {
            let lhs = peel(lhs);
            let rhs = peel(rhs);
            match (lhs.1.as_ref(), rhs.1.as_ref()) {
                (Expression::Identifier(p), Expression::Integer(k)) if *p == param && *k > 0 => {
                    Some(*k)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn peel<'a>(expr: &'a Output<'a>) -> &'a Output<'a> {
    match expr.1.as_ref() {
        Expression::Expr(inner)
        | Expression::Group(inner)
        | Expression::Statement(inner)
        | Expression::ExprStatement(inner) => peel(inner),
        Expression::Fragment(items) if items.len() == 1 => peel(&items[0]),
        _ => expr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parser::Pratt;

    fn parse(src: &str) -> Output<'static> {
        let owned = Box::leak(src.to_string().into_boxed_str());
        Pratt::default().parse(owned).expect("parse")
    }

    #[test]
    fn fib_const_entry_is_proven_without_attr() {
        let ast = parse(
            r#"
fn fib(int n) -> int {
    if n <= 2 { return 1; }
    return fib(n - 1) + fib(n - 2);
}
fn main() {
    let x = fib(10);
    return;
}
"#,
        );
        let report = analyze_stack_bounds(&ast);
        assert!(
            report.messages.is_empty(),
            "unexpected errors: {:?}",
            report.messages
        );
        let b = report
            .bounds
            .iter()
            .find(|b| b.fn_name == "fib")
            .expect("fib bound");
        assert_eq!(b.source, BoundSource::Proven);
        // (10 - 2) / 1 + 1 = 9
        assert_eq!(b.max_frames, 9);
    }

    #[test]
    fn fib_bench_32_proven() {
        let ast = parse(
            r#"
fn fib(int n) -> int {
    if n <= 2 { return 1; }
    return fib(n - 1) + fib(n - 2);
}
fn main() {
    let x = fib(32);
    return;
}
"#,
        );
        let report = analyze_stack_bounds(&ast);
        assert!(report.messages.is_empty(), "{:?}", report.messages);
        let b = report.bounds.iter().find(|b| b.fn_name == "fib").unwrap();
        assert_eq!(b.max_frames, 31); // (32-2)/1 + 1
    }

    #[test]
    fn dynamic_rec_requires_max_depth() {
        let ast = parse(
            r#"
fn fib(int n) -> int {
    if n <= 2 { return 1; }
    return fib(n - 1) + fib(n - 2);
}
fn main() {
    let k = 10;
    let x = fib(k);
    return;
}
"#,
        );
        let report = analyze_stack_bounds(&ast);
        assert!(
            report
                .messages
                .iter()
                .any(|m| m.message().contains("max_depth")),
            "{:?}",
            report.messages
        );
    }

    #[test]
    fn max_depth_attr_satisfies_dynamic() {
        let ast = parse(
            r#"
#[max_depth(64)]
fn fib(int n) -> int {
    if n <= 2 { return 1; }
    return fib(n - 1) + fib(n - 2);
}
fn main() {
    let k = 10;
    let x = fib(k);
    return;
}
"#,
        );
        let report = analyze_stack_bounds(&ast);
        assert!(report.messages.is_empty(), "{:?}", report.messages);
        let b = report.bounds.iter().find(|b| b.fn_name == "fib").unwrap();
        assert_eq!(b.max_frames, 64);
        assert_eq!(b.source, BoundSource::Attribute);
    }

    #[test]
    fn tail_rec_needs_no_attr() {
        let ast = parse(
            r#"
fn countdown(int n) -> int {
    if n <= 0 { return 0; }
    return countdown(n - 1);
}
fn main() {
    let x = countdown(100);
    return;
}
"#,
        );
        let report = analyze_stack_bounds(&ast);
        assert!(report.messages.is_empty(), "{:?}", report.messages);
        let b = report
            .bounds
            .iter()
            .find(|b| b.fn_name == "countdown")
            .unwrap();
        assert_eq!(b.source, BoundSource::TailOnly);
        assert_eq!(b.max_frames, 1);
    }

    #[test]
    fn fib_bench_sizes_operand_stack_above_default() {
        let ast = parse(
            r#"
fn fib(int n) -> int {
    if n <= 2 { return 1; }
    return fib(n - 1) + fib(n - 2);
}
fn main() {
    let x = fib(32);
    return;
}
"#,
        );
        let report = analyze_stack_bounds(&ast);
        assert!(report.messages.is_empty(), "{:?}", report.messages);
        // (32-2)/1+1 = 31 frames → 31*16+16 = 512
        assert_eq!(report.operand_slots_needed, 512);
        assert!(report.operand_slots_needed > DEFAULT_OPERAND_STACK_SLOTS);
    }

    #[test]
    fn measure_depth_helpers() {
        assert_eq!(measure_depth(2, 2, 1), 1);
        assert_eq!(measure_depth(10, 2, 1), 9);
        assert_eq!(measure_depth(32, 2, 1), 31);
    }

    #[test]
    fn mutual_recursion_requires_max_depth() {
        let ast = parse(
            r#"
fn ping(int n) -> int {
    if n <= 0 { return 0; }
    return pong(n - 1);
}
fn pong(int n) -> int {
    if n <= 0 { return 1; }
    return ping(n - 1);
}
fn main() {
    let x = ping(3);
    return;
}
"#,
        );
        let report = analyze_stack_bounds(&ast);
        assert!(
            report
                .messages
                .iter()
                .any(|m| m.code() == Some(ErrorCode::UnboundedRecursion)
                    && m.message().contains("mutual")),
            "{:?}",
            report.messages
        );
    }

    #[test]
    fn mutual_recursion_with_max_depth_ok() {
        let ast = parse(
            r#"
#[max_depth(8)]
fn ping(int n) -> int {
    if n <= 0 { return 0; }
    return pong(n - 1);
}
#[max_depth(8)]
fn pong(int n) -> int {
    if n <= 0 { return 1; }
    return ping(n - 1);
}
fn main() {
    let x = ping(3);
    return;
}
"#,
        );
        let report = analyze_stack_bounds(&ast);
        assert!(report.messages.is_empty(), "{:?}", report.messages);
        assert!(
            report
                .bounds
                .iter()
                .any(|b| b.fn_name == "ping" && b.source == BoundSource::Attribute && b.max_frames == 8)
        );
    }

    #[test]
    fn unary_fact_const_entry_is_proven() {
        let ast = parse(
            r#"
fn fact(int n) -> int {
    if n <= 1 { return 1; }
    return n * fact(n - 1);
}
fn main() {
    let x = fact(5);
    return;
}
"#,
        );
        let report = analyze_stack_bounds(&ast);
        assert!(report.messages.is_empty(), "{:?}", report.messages);
        let b = report.bounds.iter().find(|b| b.fn_name == "fact").unwrap();
        assert_eq!(b.source, BoundSource::Proven);
        // (5 - 1) / 1 + 1 = 5
        assert_eq!(b.max_frames, 5);
    }

    #[test]
    fn base_case_lt_and_eq_shapes_are_proven() {
        let lt = parse(
            r#"
fn f(int n) -> int {
    if n < 3 { return 1; }
    return f(n - 1) + f(n - 2);
}
fn main() { let x = f(5); return; }
"#,
        );
        let lt_report = analyze_stack_bounds(&lt);
        assert!(lt_report.messages.is_empty(), "{:?}", lt_report.messages);
        // base_bound for n < 3 is 2; (5-2)/1+1 = 4
        assert_eq!(
            lt_report.bounds.iter().find(|b| b.fn_name == "f").unwrap().max_frames,
            4
        );

        let eq = parse(
            r#"
fn g(int n) -> int {
    if n == 0 { return 1; }
    return g(n - 1) + 1;
}
fn main() { let x = g(4); return; }
"#,
        );
        let eq_report = analyze_stack_bounds(&eq);
        assert!(eq_report.messages.is_empty(), "{:?}", eq_report.messages);
        // base 0; (4-0)/1+1 = 5
        assert_eq!(
            eq_report.bounds.iter().find(|b| b.fn_name == "g").unwrap().max_frames,
            5
        );
    }

    #[test]
    fn missing_base_case_requires_max_depth() {
        let ast = parse(
            r#"
fn f(int n) -> int {
    return 1 + f(n - 1);
}
fn main() {
    let x = f(3);
    return;
}
"#,
        );
        let report = analyze_stack_bounds(&ast);
        assert!(
            report.messages.iter().any(|m| {
                m.code() == Some(ErrorCode::UnboundedRecursion)
                    && m.message().contains("base case")
            }),
            "{:?}",
            report.messages
        );
    }

    #[test]
    fn unrecognized_shape_requires_max_depth() {
        let ast = parse(
            r#"
fn boom(int n) -> int {
    return boom(n + 1) + 1;
}
fn main() {
    let x = boom(1);
    return;
}
"#,
        );
        let report = analyze_stack_bounds(&ast);
        assert!(
            report.messages.iter().any(|m| {
                m.code() == Some(ErrorCode::UnboundedRecursion)
                    && m.message().contains("analyzable decreasing measure")
            }),
            "{:?}",
            report.messages
        );
    }

    #[test]
    fn invalid_max_depth_attr_rejected() {
        let ast = parse(
            r#"
#[max_depth(0)]
fn fib(int n) -> int {
    if n <= 2 { return 1; }
    return fib(n - 1) + fib(n - 2);
}
fn main() {
    let k = 10;
    let x = fib(k);
    return;
}
"#,
        );
        let report = analyze_stack_bounds(&ast);
        assert!(
            report
                .messages
                .iter()
                .any(|m| m.message().contains("positive integer")),
            "{:?}",
            report.messages
        );
    }

    #[test]
    fn absurd_max_depth_emits_stack_depth_exceeded() {
        // frames*16+16 > MAX ⇒ frames > 65535
        let ast = parse(
            r#"
#[max_depth(65536)]
fn fib(int n) -> int {
    if n <= 2 { return 1; }
    return fib(n - 1) + fib(n - 2);
}
fn main() {
    let k = 10;
    let x = fib(k);
    return;
}
"#,
        );
        let report = analyze_stack_bounds(&ast);
        assert!(
            report
                .messages
                .iter()
                .any(|m| m.code() == Some(ErrorCode::StackDepthExceeded)),
            "{:?}",
            report.messages
        );
        assert_eq!(report.operand_slots_needed, MAX_OPERAND_STACK_SLOTS);
    }

    #[test]
    fn attr_larger_than_proven_wins_source() {
        let ast = parse(
            r#"
#[max_depth(20)]
fn fib(int n) -> int {
    if n <= 2 { return 1; }
    return fib(n - 1) + fib(n - 2);
}
fn main() {
    let x = fib(10);
    return;
}
"#,
        );
        let report = analyze_stack_bounds(&ast);
        assert!(report.messages.is_empty(), "{:?}", report.messages);
        let b = report.bounds.iter().find(|b| b.fn_name == "fib").unwrap();
        assert_eq!(b.max_frames, 20);
        assert_eq!(b.source, BoundSource::Attribute);
    }

    #[test]
    fn attr_smaller_than_proven_still_uses_proven_frames() {
        let ast = parse(
            r#"
#[max_depth(5)]
fn fib(int n) -> int {
    if n <= 2 { return 1; }
    return fib(n - 1) + fib(n - 2);
}
fn main() {
    let x = fib(10);
    return;
}
"#,
        );
        let report = analyze_stack_bounds(&ast);
        assert!(report.messages.is_empty(), "{:?}", report.messages);
        let b = report.bounds.iter().find(|b| b.fn_name == "fib").unwrap();
        // Proven depth 9 > attributed 5 → keep proven frames.
        assert_eq!(b.max_frames, 9);
        assert_eq!(b.source, BoundSource::Proven);
    }

    #[test]
    fn operand_slots_for_frames_clamps_and_floors() {
        assert_eq!(operand_slots_for_frames(1), DEFAULT_OPERAND_STACK_SLOTS);
        assert_eq!(operand_slots_for_frames(9), DEFAULT_OPERAND_STACK_SLOTS); // 9*16+16=160
        assert_eq!(operand_slots_for_frames(31), 512);
        assert_eq!(
            operand_slots_for_frames(u32::MAX),
            MAX_OPERAND_STACK_SLOTS
        );
    }

    #[test]
    fn non_recursive_program_keeps_default_slots() {
        let ast = parse(
            r#"
fn add(int a, int b) -> int { return a + b; }
fn main() { let x = add(1, 2); return; }
"#,
        );
        let report = analyze_stack_bounds(&ast);
        assert!(report.messages.is_empty());
        assert!(report.bounds.is_empty());
        assert_eq!(report.operand_slots_needed, DEFAULT_OPERAND_STACK_SLOTS);
    }
}
