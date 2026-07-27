//! Monomorphization planning for ground generic call sites.
//!
//! This module is intentionally analysis-first. It decides which generic calls
//! are safe and small enough to specialize; `Compiler` owns the bytecode
//! emission for each accepted specialization.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use parser::{
    SimpleSpan,
    ast::{Expression, Output, TypeParam},
};

use crate::typechecking::{Checker, Ty};

pub const MAX_SPECIALIZATIONS_PER_FN: usize = 8;
pub const MAX_TOTAL_SPECIALIZATIONS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MonoKey {
    pub fn_name: String,
    pub subst: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonoSpecialization {
    pub key: MonoKey,
    pub arg_types: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonoRetarget {
    pub call_span_start: usize,
    pub key: MonoKey,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MonoPlan {
    pub specializations: Vec<MonoSpecialization>,
    pub retargets: Vec<MonoRetarget>,
    pub escaped_generic_fns: BTreeSet<String>,
}

impl MonoPlan {
    pub fn is_empty(&self) -> bool {
        self.specializations.is_empty()
    }

    pub fn specialization_for_call(
        &self,
        fn_name: &str,
        arg_types: &[String],
    ) -> Option<&MonoSpecialization> {
        self.specializations
            .iter()
            .find(|spec| spec.key.fn_name == fn_name && spec.arg_types == arg_types)
    }

    pub fn specializations_for_fn<'a>(
        &'a self,
        fn_name: &'a str,
    ) -> impl Iterator<Item = &'a MonoSpecialization> + 'a {
        self.specializations
            .iter()
            .filter(move |spec| spec.key.fn_name == fn_name)
    }
}

#[derive(Clone, Debug)]
struct GenericFnSig {
    type_params: Vec<String>,
    type_param_bounds: Vec<Vec<String>>,
    /// For each formal: which type-parameter index it references (if any).
    param_type_params: Vec<Option<usize>>,
    /// Parallel to `param_type_params`: true when the formal is `T... name`.
    param_is_rest: Vec<bool>,
}

#[derive(Clone, Debug)]
struct MonoCandidate {
    span: SimpleSpan,
    specialization: MonoSpecialization,
}

/// Build a monomorphization plan for ground, bounded generic call sites.
///
/// The current MVP specializes only generic functions whose type parameters
/// carry at least one bound. That keeps unbounded `id<T>` on the existing
/// shared `BoxValue`/`UnboxValue` path while enabling the target `Num` case.
pub fn plan_monomorphization(module: &str, ast: &Output, checker: &Checker) -> MonoPlan {
    let mut sigs = HashMap::new();
    collect_generic_functions(module, ast, &mut sigs);

    let mut escaped = BTreeSet::new();
    collect_escaped_generic_refs(ast, &sigs, false, &mut escaped);

    let mut candidates = Vec::new();
    collect_candidates(ast, checker, &sigs, &mut candidates);

    let (specializations, retargets) = apply_caps(candidates);
    MonoPlan {
        specializations,
        retargets,
        escaped_generic_fns: escaped,
    }
}

fn collect_generic_functions(
    module: &str,
    node: &Output,
    sigs: &mut HashMap<String, GenericFnSig>,
) {
    match node.1.as_ref() {
        Expression::Function {
            name,
            type_params,
            args,
            body,
            ..
        } => {
            if !type_params.is_empty() {
                let sig = signature_from_function(type_params, args);
                sigs.insert(name.to_string(), sig.clone());
                if !module.is_empty() {
                    sigs.insert(format!("{module}::{name}"), sig);
                }
            }
            if let Some(body) = body {
                collect_generic_functions(module, body, sigs);
            }
        }
        _ => walk_children(node, &mut |child| {
            collect_generic_functions(module, child, sigs)
        }),
    }
}

fn signature_from_function(type_params: &[TypeParam<'_>], args: &Output) -> GenericFnSig {
    let type_param_names = type_params
        .iter()
        .map(|tp| tp.name.to_string())
        .collect::<Vec<_>>();
    let type_param_bounds = type_params
        .iter()
        .map(|tp| tp.bounds.iter().map(|b| b.to_string()).collect::<Vec<_>>())
        .collect::<Vec<_>>();

    let mut param_type_params = Vec::new();
    let mut param_is_rest = Vec::new();
    if let Expression::Fragment(children) = args.1.as_ref() {
        for child in children {
            if let Expression::Argument(ty, _, is_rest) = child.1.as_ref() {
                param_type_params.push(ty.as_ref().and_then(|t| type_param_ref_index(t, &type_param_names)));
                param_is_rest.push(*is_rest);
            }
        }
    }

    GenericFnSig {
        type_params: type_param_names,
        type_param_bounds,
        param_type_params,
        param_is_rest,
    }
}

fn type_param_ref_index(ty: &Output, type_params: &[String]) -> Option<usize> {
    match ty.1.as_ref() {
        Expression::Identifier(name) | Expression::Type(name) => {
            type_params.iter().position(|tp| tp == name)
        }
        _ => None,
    }
}

fn collect_candidates(
    node: &Output,
    checker: &Checker,
    sigs: &HashMap<String, GenericFnSig>,
    out: &mut Vec<MonoCandidate>,
) {
    if let Expression::Call { name, args } = node.1.as_ref() {
        if let Expression::Identifier(fn_name) = name.1.as_ref()
            && let Some(sig) = sigs.get(*fn_name)
            && let Some(specialization) =
                candidate_for_call(*fn_name, sig, args.as_deref(), checker)
        {
            out.push(MonoCandidate {
                span: node.0,
                specialization,
            });
        }
        if let Some(args) = args {
            for arg in args {
                collect_candidates(arg, checker, sigs, out);
            }
        }
        return;
    }

    walk_children(node, &mut |child| {
        collect_candidates(child, checker, sigs, out)
    });
}

fn candidate_for_call(
    fn_name: &str,
    sig: &GenericFnSig,
    args: Option<&[Output]>,
    checker: &Checker,
) -> Option<MonoSpecialization> {
    if sig.type_params.is_empty() || sig.type_param_bounds.iter().all(|bounds| bounds.is_empty()) {
        return None;
    }

    // Skip monomorphization when the shared body needs dictionary dispatch:
    // - user-defined typeclasses always use dict tuples
    // - `Show` always uses dict / `%v` CallIndirect (Phase 2–4); it must not
    //   be ground-specialized, or `%v` sites keep an open `Ty::Var` and skip
    //   boxing into the Show thunk
    // Num / Ord / Eq still monomorphize so arithmetic becomes direct opcodes.
    let requires_dictionary_body = sig.type_param_bounds.iter().any(|bounds| {
        bounds
            .iter()
            .any(|b| b == "Show" || !Checker::is_builtin_class(b))
    });
    if requires_dictionary_body {
        return None;
    }

    let args = args.unwrap_or(&[]);
    let has_rest = sig.param_is_rest.last().copied().unwrap_or(false);
    let fixed_count = if has_rest {
        sig.param_type_params.len().saturating_sub(1)
    } else {
        sig.param_type_params.len()
    };

    // Reorder/pack like codegen (`split_call_args_for_rest`) so named calls
    // (`add(b: 2, a: 1)`) and rest packs share the same mono key.
    let (fixed_args, rest_args, pack_rest) =
        split_call_args_for_mono(fn_name, args, checker, has_rest, fixed_count)?;

    if !has_rest {
        if fixed_args.len() != sig.param_type_params.len() {
            return None;
        }
    } else if fixed_args.len() < fixed_count {
        return None;
    }

    let mut subst: Vec<Option<String>> = vec![None; sig.type_params.len()];
    let mut arg_types = Vec::with_capacity(sig.param_type_params.len());

    let bind = |subst: &mut [Option<String>], tp_idx: Option<usize>, arg_ty: &str| -> Option<()> {
        if let Some(tp_idx) = tp_idx {
            match &subst[tp_idx] {
                Some(existing) if existing != arg_ty => return None,
                Some(_) => {}
                None => subst[tp_idx] = Some(arg_ty.to_string()),
            }
        }
        Some(())
    };

    let fixed_tps = if has_rest {
        &sig.param_type_params[..fixed_count]
    } else {
        sig.param_type_params.as_slice()
    };
    for (arg, tp_idx) in fixed_args.iter().zip(fixed_tps.iter()) {
        let arg_ty = ground_type_name(checker, arg)?;
        bind(&mut subst, *tp_idx, &arg_ty)?;
        arg_types.push(arg_ty);
    }

    if pack_rest {
        // One key slot per rest formal: the *element* ground type (not the
        // packed `[T]` / `[T; N]`), matching `mono_call_offset`.
        let rest_tp = sig.param_type_params.get(fixed_count).copied().flatten();
        if rest_args.is_empty() {
            let elem = rest_tp.and_then(|i| subst[i].clone())?;
            arg_types.push(elem);
        } else {
            let mut elem_ty: Option<String> = None;
            for arg in &rest_args {
                let t = ground_type_name(checker, arg)?;
                match &elem_ty {
                    None => elem_ty = Some(t.clone()),
                    Some(prev) if prev != &t => return None,
                    _ => {}
                }
                bind(&mut subst, rest_tp, &t)?;
            }
            arg_types.push(elem_ty?);
        }
    } else if has_rest {
        // Declares rest but call did not pack — not a mono candidate.
        return None;
    }

    let subst = subst.into_iter().collect::<Option<Vec<_>>>()?;
    Some(MonoSpecialization {
        key: MonoKey {
            fn_name: fn_name.to_string(),
            subst,
        },
        arg_types,
    })
}

/// Mirror of codegen `split_call_args_for_rest` for mono planning.
fn split_call_args_for_mono<'a>(
    fn_name: &str,
    args: &'a [Output<'a>],
    checker: &Checker,
    has_rest: bool,
    fixed_count: usize,
) -> Option<(Vec<&'a Output<'a>>, Vec<&'a Output<'a>>, bool)> {
    let has_named = args
        .iter()
        .any(|a| matches!(a.1.as_ref(), Expression::NamedArg(..)));
    if !has_named && !has_rest {
        return Some((args.iter().collect(), Vec::new(), false));
    }
    let param_names = checker.fn_param_names(fn_name)?;
    let rest_name = if has_rest {
        param_names.get(fixed_count).map(|s| s.as_str())
    } else {
        None
    };
    let mut slots: Vec<Option<&'a Output<'a>>> = vec![None; fixed_count];
    let mut rest = Vec::new();
    let mut next_pos = 0usize;
    for arg in args {
        match arg.1.as_ref() {
            Expression::NamedArg(name, value) => {
                if rest_name == Some(*name) {
                    rest.push(value);
                    continue;
                }
                if let Some(idx) = param_names[..fixed_count]
                    .iter()
                    .position(|p| p == *name)
                {
                    slots[idx] = Some(value);
                }
            }
            _ => {
                while next_pos < fixed_count && slots[next_pos].is_some() {
                    next_pos += 1;
                }
                if next_pos < fixed_count {
                    slots[next_pos] = Some(arg);
                    next_pos += 1;
                } else if has_rest {
                    rest.push(arg);
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
        Some((fixed, rest, true))
    } else {
        Some((fixed, Vec::new(), false))
    }
}

pub fn ground_type_name(checker: &Checker, expr: &Output) -> Option<String> {
    match expr.1.as_ref() {
        Expression::Integer(_) => Some("int".to_string()),
        Expression::Float(_) => Some("float".to_string()),
        Expression::String(_) => Some("string".to_string()),
        Expression::Bool(_) => Some("bool".to_string()),
        Expression::Identifier(name) => checker.codegen_var_type(name).and_then(|ty| {
            concrete_ty_name(&crate::typechecking::subst::apply_ty_prune(
                checker.subst(),
                ty,
            ))
        }),
        Expression::Tuple(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for it in items {
                parts.push(ground_type_name(checker, it)?);
            }
            // Homogeneous only — matches numeric-tower mono keys.
            if parts.is_empty() || parts.iter().any(|p| p != &parts[0]) {
                return None;
            }
            Some(format!("({})", parts.join(", ")))
        }
        Expression::Array(items) => {
            if items.is_empty() {
                return None;
            }
            let elem = ground_type_name(checker, &items[0])?;
            if items
                .iter()
                .any(|it| ground_type_name(checker, it).as_ref() != Some(&elem))
            {
                return None;
            }
            Some(format!("[{}; {}]", elem, items.len()))
        }
        Expression::NamedArg(_, inner)
        | Expression::Group(inner)
        | Expression::Expr(inner)
        | Expression::Statement(inner) => ground_type_name(checker, inner),
        _ => None,
    }
}

fn concrete_ty_name(ty: &Ty) -> Option<String> {
    if contains_var(ty) || matches!(ty, Ty::Fun(_, _)) {
        return None;
    }
    Some(ty.to_string())
}

/// Parse a monomorphization key fragment produced by [`ground_type_name`] /
/// [`concrete_ty_name`] back into a [`Ty`].
pub fn parse_mono_ty_name(name: &str) -> Option<Ty> {
    let name = name.trim();
    match name {
        "int" => return Some(Ty::Con("int".into())),
        "float" => return Some(Ty::Con("float".into())),
        "string" => return Some(Ty::Con("string".into())),
        "bool" => return Some(Ty::Con("bool".into())),
        "unit" | "()" => return Some(Ty::Con("unit".into())),
        _ => {}
    }
    if let Some(inner) = name.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        let parts: Vec<&str> = if inner.trim().is_empty() {
            Vec::new()
        } else {
            split_top_level_commas(inner)
        };
        if parts.is_empty() {
            return None;
        }
        let mut elems = Vec::with_capacity(parts.len());
        for p in parts {
            elems.push(parse_mono_ty_name(p.trim())?);
        }
        return Some(Ty::Tuple(elems));
    }
    if let Some(inner) = name.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        if let Some((elem_s, n_s)) = inner.rsplit_once(';') {
            let elem = parse_mono_ty_name(elem_s.trim())?;
            let n: usize = n_s.trim().parse().ok()?;
            return Some(crate::typechecking::ty::array_fixed(elem, n));
        }
        let elem = parse_mono_ty_name(inner.trim())?;
        return Some(crate::typechecking::ty::array(elem));
    }
    // Nominal / other constructors — leave as Con.
    if name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    {
        return Some(Ty::Con(name.to_string()));
    }
    None
}

fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0i32;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

fn contains_var(ty: &Ty) -> bool {
    match ty {
        Ty::Var(_) => true,
        Ty::Fun(a, b) => contains_var(a) || contains_var(b),
        Ty::App(head, args) => contains_var(head) || args.iter().any(contains_var),
        Ty::List(inner) => contains_var(inner),
        Ty::Sum { variants, .. } => variants.iter().any(|(_, payload)| match payload {
            crate::typechecking::ty::EnumVariantPayloadTy::Unit => false,
            crate::typechecking::ty::EnumVariantPayloadTy::Tuple(items) => {
                items.iter().any(contains_var)
            }
            crate::typechecking::ty::EnumVariantPayloadTy::Record(fields) => {
                fields.iter().any(|(_, ty)| contains_var(ty))
            }
        }),
        Ty::Constructor { owner, .. } => contains_var(owner),
        Ty::Tuple(items) => items.iter().any(contains_var),
        Ty::Array { element, .. } => contains_var(element),
        Ty::Record { fields } => fields.iter().any(|(_, ty)| contains_var(ty)),
        Ty::Forall { body, .. } => contains_var(body),
        Ty::Readonly(inner) => contains_var(inner),
        Ty::Con(_) | Ty::Existential { .. } => false,
    }
}

fn apply_caps(candidates: Vec<MonoCandidate>) -> (Vec<MonoSpecialization>, Vec<MonoRetarget>) {
    let mut seen = BTreeSet::new();
    let mut per_fn: BTreeMap<String, usize> = BTreeMap::new();
    let mut specializations = Vec::new();
    let mut retargets = Vec::new();

    for candidate in candidates {
        if specializations.len() >= MAX_TOTAL_SPECIALIZATIONS {
            continue;
        }

        let key = candidate.specialization.key.clone();
        let count = per_fn.entry(key.fn_name.clone()).or_default();
        if !seen.contains(&key) {
            if *count >= MAX_SPECIALIZATIONS_PER_FN {
                continue;
            }
            *count += 1;
            seen.insert(key.clone());
            specializations.push(candidate.specialization.clone());
        }
        retargets.push(MonoRetarget {
            call_span_start: candidate.span.start,
            key,
        });
    }

    (specializations, retargets)
}

fn collect_escaped_generic_refs(
    node: &Output,
    sigs: &HashMap<String, GenericFnSig>,
    is_call_target: bool,
    escaped: &mut BTreeSet<String>,
) {
    match node.1.as_ref() {
        Expression::Identifier(name) if !is_call_target && sigs.contains_key(*name) => {
            escaped.insert(name.to_string());
        }
        Expression::Call { name, args } => {
            collect_escaped_generic_refs(name, sigs, true, escaped);
            if let Some(args) = args {
                for arg in args {
                    collect_escaped_generic_refs(arg, sigs, false, escaped);
                }
            }
        }
        _ => walk_children(node, &mut |child| {
            collect_escaped_generic_refs(child, sigs, false, escaped)
        }),
    }
}

fn walk_children<F>(node: &Output, f: &mut F)
where
    F: FnMut(&Output),
{
    match node.1.as_ref() {
        Expression::Program(items)
        | Expression::Block(items)
        | Expression::Fragment(items)
        | Expression::List(items)
        | Expression::Tuple(items)
        | Expression::Array(items)
        | Expression::Declare(items)
        | Expression::Invoke(items) => {
            for item in items {
                f(item);
            }
        }
        Expression::Expr(e)
        | Expression::Group(e)
        | Expression::Statement(e)
        | Expression::ExprStatement(e)
        | Expression::Return(e)
        | Expression::ImplicitReturn(e)
        | Expression::Raise(e)
        | Expression::Panic(e)
        | Expression::Try(e)
        | Expression::Yield(e)
        | Expression::YieldFrom(e)
        | Expression::Negate(e)
        | Expression::Not(e)
        | Expression::LogicalNot(e)
        | Expression::Positive(e)
        | Expression::Adjust { target: e, .. }
        | Expression::Defer(e)
        | Expression::Member(e)
        | Expression::Dload(e)
        | Expression::Done(e)
        | Expression::Noop(e)
        | Expression::Method(_, e)
        | Expression::OptionalAccess(e, _)
        | Expression::Access(e, _) => f(e),
        Expression::Assignment(lhs, rhs) | Expression::CompoundAssign(lhs, _, rhs) => {
            f(lhs);
            f(rhs);
        }
        Expression::Add(l, r)
        | Expression::Sub(l, r)
        | Expression::Mul(l, r)
        | Expression::Div(l, r)
        | Expression::Mod(l, r)
        | Expression::Pow(l, r)
        | Expression::Shl(l, r)
        | Expression::Shr(l, r)
        | Expression::Xor(l, r)
        | Expression::And(l, r)
        | Expression::Or(l, r)
        | Expression::BitAnd(l, r)
        | Expression::BitOr(l, r)
        | Expression::Eq(l, r)
        | Expression::Neq(l, r)
        | Expression::Le(l, r)
        | Expression::Gt(l, r)
        | Expression::Leq(l, r)
        | Expression::Geq(l, r)
        | Expression::Coalesce(l, r) => {
            f(l);
            f(r);
        }
        Expression::Index(l, r) => {
            f(l);
            if let Some(r) = r {
                f(r);
            }
        }
        Expression::Range { start, end, .. } => {
            f(start);
            f(end);
        }
        Expression::Print(fmt, params) | Expression::Format(fmt, params) => {
            f(fmt);
            if let Some(params) = params {
                for param in params {
                    f(param);
                }
            }
        }
        Expression::Resume(target, arg) => {
            f(target);
            if let Some(arg) = arg {
                f(arg);
            }
        }
        Expression::If(branches) => {
            for branch in branches {
                f(branch);
            }
        }
        Expression::Branch(cond, body) => {
            if let Some(cond) = cond {
                f(cond);
            }
            f(body);
        }
        Expression::Call { name, args } => {
            f(name);
            if let Some(args) = args {
                for arg in args {
                    f(arg);
                }
            }
        }
        Expression::Loop {
            iterable,
            body,
            identifier,
        } => {
            f(iterable);
            if let Some(identifier) = identifier {
                f(identifier);
            }
            f(body);
        }
        Expression::For {
            init,
            cond,
            step,
            body,
        } => {
            if let Some(init) = init {
                f(init);
            }
            f(cond);
            if let Some(step) = step {
                f(step);
            }
            f(body);
        }
        Expression::Match { scrutinee, arms } => {
            f(scrutinee);
            for arm in arms {
                f(&arm.body);
            }
        }
        Expression::Function {
            args,
            body,
            returns,
            ..
        } => {
            f(args);
            if let Some(returns) = returns {
                f(returns);
            }
            if let Some(body) = body {
                f(body);
            }
        }
        Expression::Lambda { args, body, .. } => {
            f(args);
            f(body);
        }
        Expression::TestCase { name, body } => {
            f(name);
            f(body);
        }
        Expression::TypeApp { args, .. } => {
            for arg in args {
                f(arg);
            }
        }
        Expression::TypeFun(arg, ret) => {
            f(arg);
            f(ret);
        }
        Expression::Class { fields, .. } => {
            for field in fields {
                f(field);
            }
        }
        Expression::Implementation { methods, .. } | Expression::TypeClass { methods, .. } => {
            for method in methods {
                f(method);
            }
        }
        Expression::TypeClassImpl { args, methods, .. } => {
            for arg in args {
                f(arg);
            }
            for method in methods {
                f(method);
            }
        }
        Expression::TypeAlias { ty, .. } | Expression::AssocTypeDef { ty, .. } => f(ty),
        Expression::AssocTypeDecl { .. } => {}
        Expression::TypeProjection { args, .. } => {
            for arg in args {
                f(arg);
            }
        }
        Expression::EnumDecl { variants, .. } => {
            for variant in variants {
                f(variant);
            }
        }
        Expression::Dict(fields) => {
            for field in fields {
                f(&field.value);
            }
        }
        Expression::Instantiate(class, args) => {
            f(class);
            if let Some(args) = args {
                for arg in args {
                    f(arg);
                }
            }
        }
        Expression::Construct { fields, .. } => match fields {
            parser::ast::EnumConstructPayload::Unit => {}
            parser::ast::EnumConstructPayload::Tuple(items) => {
                for item in items {
                    f(item);
                }
            }
            parser::ast::EnumConstructPayload::Record(fields) => {
                for field in fields {
                    f(&field.value);
                }
            }
        },
        Expression::EnumVariant { .. }
        | Expression::Use { .. }
        | Expression::Module(_, _)
        | Expression::Comment(_)
        | Expression::Integer(_)
        | Expression::Float(_)
        | Expression::String(_)
        | Expression::Bool(_)
        | Expression::Identifier(_)
        | Expression::Type(_)
        | Expression::Default(_)
        | Expression::Break
        | Expression::Continue
        | Expression::Variable(_, _)
        | Expression::Constant(_, _)
        | Expression::Argument(_, _, _)
        | Expression::Field { .. }
        | Expression::QualifiedAccess { .. }
        | Expression::ExternBlock { .. }
        | Expression::ExternStruct(_)
        | Expression::Forall { .. } => {}
        Expression::LetDestructure { rhs, .. } => f(rhs),
        Expression::Readonly(inner) => f(inner),
        Expression::StaticDecl { ty, init, .. } => {
            if let Some(ty) = ty {
                f(ty);
            }
            f(init);
        }
        Expression::NamedArg(_, value) => f(value),
        Expression::Spread(inner) => f(inner),
        Expression::TypeFnSig { params, ret } => {
            f(params);
            f(ret);
        }
        Expression::AttrDecl { args, returns, body, .. } => {
            f(args);
            if let Some(returns) = returns {
                f(returns);
            }
            f(body);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parser::Pratt;

    fn plan(src: &str) -> MonoPlan {
        let ast = Pratt::default().parse(src).expect("parse failed");
        let mut checker = Checker::new();
        let _ = checker.check_program(&ast);
        plan_monomorphization("", &ast, &checker)
    }

    #[test]
    fn plans_ground_bounded_generic_call() {
        let plan = plan(
            "fn add<T: Num>(T a, T b) -> T { return a + b; } \
             fn main() { print \"%i\", add(1, 2); }",
        );
        assert_eq!(plan.specializations.len(), 1);
        assert_eq!(plan.specializations[0].key.fn_name, "add");
        assert_eq!(plan.specializations[0].key.subst, vec!["int"]);
        assert_eq!(plan.specializations[0].arg_types, vec!["int", "int"]);
        assert_eq!(plan.retargets.len(), 1);
    }

    #[test]
    fn plans_rest_only_num_generic_with_element_arg_type() {
        // Rest-only formals pack at the call site; the mono key uses the
        // *element* ground type (one slot per formal), not the packed array.
        let plan = plan(
            "fn twice_first<T: Num>(T... xs) -> T { return xs[0] + xs[0]; } \
             fn main() { print \"%i\", twice_first(21); }",
        );
        assert_eq!(plan.specializations.len(), 1);
        assert_eq!(plan.specializations[0].key.fn_name, "twice_first");
        assert_eq!(plan.specializations[0].key.subst, vec!["int"]);
        assert_eq!(plan.specializations[0].arg_types, vec!["int"]);
    }

    #[test]
    fn plans_named_arg_ground_bounded_generic_call() {
        let plan = plan(
            "fn add<T: Num>(T a, T b) -> T { return a + b; } \
             fn main() { print \"%i\", add(b: 2, a: 1); }",
        );
        assert_eq!(plan.specializations.len(), 1);
        assert_eq!(plan.specializations[0].key.subst, vec!["int"]);
        assert_eq!(plan.specializations[0].arg_types, vec!["int", "int"]);
    }

    #[test]
    fn leaves_unbounded_id_on_shared_path_for_mvp() {
        let plan = plan("fn id<T>(T x) -> T { return x; } fn main() { id(1); }");
        assert!(plan.specializations.is_empty());
    }

    #[test]
    fn rejects_conflicting_type_param_instantiation() {
        let plan = plan(
            "fn same<T: Eq>(T a, T b) -> T { return a; } \
             fn main() { same(1, \"x\"); }",
        );
        assert!(plan.specializations.is_empty());
    }

    #[test]
    fn records_escaped_generic_refs() {
        let plan =
            plan("fn add<T: Num>(T a, T b) -> T { return a + b; } fn main() { let f = add; }");
        assert!(plan.escaped_generic_fns.contains("add"));
        assert!(plan.specializations.is_empty());
    }

    #[test]
    fn per_function_cap_limits_specializations() {
        let candidates = (0..(MAX_SPECIALIZATIONS_PER_FN + 2))
            .map(|i| MonoCandidate {
                span: SimpleSpan::from(i..i + 1),
                specialization: MonoSpecialization {
                    key: MonoKey {
                        fn_name: "f".to_string(),
                        subst: vec![format!("T{i}")],
                    },
                    arg_types: vec![format!("T{i}")],
                },
            })
            .collect();

        let (specializations, retargets) = apply_caps(candidates);
        assert_eq!(specializations.len(), MAX_SPECIALIZATIONS_PER_FN);
        assert_eq!(retargets.len(), MAX_SPECIALIZATIONS_PER_FN);
    }

    #[test]
    fn total_cap_limits_specializations() {
        let candidates = (0..(MAX_TOTAL_SPECIALIZATIONS + 2))
            .map(|i| MonoCandidate {
                span: SimpleSpan::from(i..i + 1),
                specialization: MonoSpecialization {
                    key: MonoKey {
                        fn_name: format!("f{i}"),
                        subst: vec!["int".to_string()],
                    },
                    arg_types: vec!["int".to_string()],
                },
            })
            .collect();

        let (specializations, retargets) = apply_caps(candidates);
        assert_eq!(specializations.len(), MAX_TOTAL_SPECIALIZATIONS);
        assert_eq!(retargets.len(), MAX_TOTAL_SPECIALIZATIONS);
    }
}
