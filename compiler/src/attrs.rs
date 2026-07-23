//! Attribute expansion (`#[derive(...)]`, `#[ffi(...)]`, etc.).
//!
//! Runs before the ID pre-walk and typechecking: desugars `#[ffi]` signature-only
//! functions into `ExternBlock` nodes and expands `#[derive]` into synthetic
//! `TypeClassImpl` siblings.

use std::collections::{HashMap, HashSet};

use parser::{
    SimpleSpan,
    ast::{
        AttrArgs, AttrLit, Attribute, EnumVariantPayload, ExternFunction, Expression, MatchArm,
        Output, Pattern, PatternField, PatternPayload, Visibility,
    },
};
use reporting::{ErrorCode, Message};

/// Builtin traits the compiler knows how to synthesize.
const DERIVABLE: &[&str] = &["Show", "Eq", "Ord"];

const KNOWN_ATTRS: &[&str] = &["derive", "ffi", "test"];

/// Result of attribute expansion before typechecking.
#[derive(Default)]
pub struct ExpandResult {
    pub messages: Vec<Message>,
    /// Class name → decorated constructor function name.
    pub decorated_class_ctors: HashMap<String, String>,
}

/// Expand every supported attribute on a program AST.
pub fn expand_program(ast: &mut Output<'_>) -> ExpandResult {
    let Expression::Program(children) = ast.1.as_mut() else {
        return ExpandResult::default();
    };
    let mut user_attrs = HashSet::new();
    let mut attr_extra_names: HashMap<String, Vec<String>> = HashMap::new();
    let mut messages = Vec::new();
    collect_and_desugar_attr_decls(
        children,
        &mut user_attrs,
        &mut attr_extra_names,
        &mut messages,
    );
    let attr_bodies = collect_attr_function_bodies(children);
    let mut decorated_class_ctors = HashMap::new();
    messages.extend(expand_decls(
        children,
        &user_attrs,
        &attr_extra_names,
        &attr_bodies,
        &mut decorated_class_ctors,
    ));
    ExpandResult {
        messages,
        decorated_class_ctors,
    }
}

struct FfiMeta<'a> {
    lib: String,
    symbol: Option<&'a str>,
    variadic: bool,
}

fn derive_traits_from_attrs<'a>(attrs: &[Attribute<'a>]) -> Vec<&'a str> {
    let mut out = Vec::new();
    for attr in attrs {
        if attr.name == "derive" {
            if let AttrArgs::Idents(idents) = &attr.args {
                out.extend(idents.iter().copied());
            }
        }
    }
    out
}

fn strip_processed_attrs(attrs: &mut Vec<Attribute<'_>>) {
    attrs.retain(|a| a.name != "derive" && a.name != "ffi");
}

fn is_known_attr(name: &str, user_attrs: &HashSet<String>) -> bool {
    KNOWN_ATTRS.contains(&name) || user_attrs.contains(name)
}

fn validate_attrs(
    attrs: &[Attribute<'_>],
    target: &str,
    user_attrs: &HashSet<String>,
    messages: &mut Vec<Message>,
    span: SimpleSpan,
    is_ffi: bool,
) {
    for attr in attrs {
        if user_attrs.contains(attr.name) && is_ffi {
            messages.push(Message::error(
                ErrorCode::GenericTypeError,
                format!(
                    "User-defined attribute `{}` cannot be applied to FFI functions",
                    attr.name
                ),
                span.into_range(),
            ));
        }
        if attr.name == "test" && target != "function" {
            messages.push(Message::error(
                ErrorCode::GenericTypeError,
                format!("Attribute `test` is not valid on {}", target),
                span.into_range(),
            ));
        }
        if !is_known_attr(attr.name, user_attrs) {
            messages.push(Message::error(
                ErrorCode::GenericTypeError,
                format!("Unknown attribute `{}`", attr.name),
                span.into_range(),
            ));
        }
    }
}

fn parse_ffi_attr<'a>(
    attrs: &[Attribute<'a>],
    messages: &mut Vec<Message>,
    span: SimpleSpan,
) -> Option<FfiMeta<'a>> {
    let ffi_attr = attrs.iter().find(|a| a.name == "ffi")?;
    let AttrArgs::KeyValues(kvs) = &ffi_attr.args else {
        messages.push(Message::error(
            ErrorCode::GenericTypeError,
            "Attribute `ffi` requires key/value arguments: `#[ffi(lib = \"c\", name = \"sym\")]`"
                .to_string(),
            span.into_range(),
        ));
        return None;
    };
    let mut lib: Option<String> = None;
    let mut symbol: Option<&'a str> = None;
    let mut variadic = false;
    for (key, lit) in kvs {
        match *key {
            "lib" => match lit {
                AttrLit::String(s) => lib = Some((*s).to_string()),
                _ => messages.push(Message::error(
                    ErrorCode::GenericTypeError,
                    "`ffi` attribute `lib` must be a string literal".to_string(),
                    span.into_range(),
                )),
            },
            "name" => match lit {
                AttrLit::String(s) => symbol = Some(s),
                _ => messages.push(Message::error(
                    ErrorCode::GenericTypeError,
                    "`ffi` attribute `name` must be a string literal".to_string(),
                    span.into_range(),
                )),
            },
            "variadic" => match lit {
                AttrLit::Bool(b) => variadic = *b,
                _ => messages.push(Message::error(
                    ErrorCode::GenericTypeError,
                    "`ffi` attribute `variadic` must be `true` or `false`".to_string(),
                    span.into_range(),
                )),
            },
            other => messages.push(Message::error(
                ErrorCode::GenericTypeError,
                format!("Unknown key `{}` in `#[ffi(...)]`", other),
                span.into_range(),
            )),
        }
    }
    let lib = lib.or_else(|| {
        messages.push(Message::error(
            ErrorCode::GenericTypeError,
            "`#[ffi(...)]` requires `lib = \"...\"`".to_string(),
            span.into_range(),
        ));
        None
    })?;
    Some(FfiMeta {
        lib,
        symbol,
        variadic,
    })
}

fn make_extern_block<'a>(
    span: SimpleSpan,
    library: String,
    decl: ExternFunction<'a>,
) -> Output<'a> {
    at(
        span,
        Expression::ExternBlock {
            library,
            declarations: vec![decl],
        },
    )
}

fn collect_and_desugar_attr_decls(
    decls: &mut Vec<Output<'_>>,
    user_attrs: &mut HashSet<String>,
    attr_extra_names: &mut HashMap<String, Vec<String>>,
    messages: &mut Vec<Message>,
) {
    let mut i = 0;
    while i < decls.len() {
        if let Expression::AttrDecl {
            name,
            type_params,
            args,
            returns,
            where_constraints,
            body,
        } = decls[i].1.as_ref()
        {
            let span = decls[i].0;
            if let Err(msg) = validate_attr_protocol(args, span) {
                messages.push(msg);
            }
            let params = fn_param_nodes(args);
            if params.len() >= 2 {
                attr_extra_names.insert(
                    (*name).to_string(),
                    params[1..params.len() - 1]
                        .iter()
                        .map(|(_, n, _)| (*n).to_string())
                        .collect(),
                );
            }
            user_attrs.insert((*name).to_string());
            decls[i] = at(
                span,
                Expression::Function {
                    attrs: vec![],
                    name,
                    is_coro: false,
                    type_params: type_params.clone(),
                    args: args.clone(),
                    returns: returns.clone(),
                    where_constraints: where_constraints.clone(),
                    body: Some(body.clone()),
                },
            );
        }
        i += 1;
    }
}

fn validate_attr_protocol(args: &Output, span: SimpleSpan) -> Result<(), Message> {
    let params = fn_param_nodes(args);
    if params.len() < 2 {
        return Err(Message::error(
            ErrorCode::GenericTypeError,
            "Attribute declaration requires at least `target` and trailing `...args` parameters"
                .to_string(),
            span.into_range(),
        ));
    }
    let last_rest = params.last().map(|(_, _, r)| *r).unwrap_or(false);
    if !last_rest {
        return Err(Message::error(
            ErrorCode::GenericTypeError,
            "Attribute declaration must end with a bare `...args` tuple-rest parameter".to_string(),
            span.into_range(),
        ));
    }
    Ok(())
}

fn fn_param_nodes<'a>(args: &'a Output<'a>) -> Vec<(Option<Output<'a>>, &'static str, bool)> {
    let mut out = Vec::new();
    if let Expression::Fragment(children) = args.1.as_ref() {
        for child in children {
            if let Expression::Argument(ty, name, is_rest) = child.1.as_ref() {
                out.push((ty.clone(), leak((*name).to_string()), *is_rest));
            }
        }
    }
    out
}

fn is_user_attr(attr: &Attribute<'_>, user_attrs: &HashSet<String>) -> bool {
    user_attrs.contains(attr.name) && !KNOWN_ATTRS.contains(&attr.name)
}

fn user_attrs_on<'a>(
    attrs: &'a [Attribute<'a>],
    user_attrs: &HashSet<String>,
) -> Vec<&'a Attribute<'a>> {
    attrs.iter().filter(|a| is_user_attr(a, user_attrs)).collect()
}

fn strip_user_attrs(attrs: &mut Vec<Attribute<'_>>, user_attrs: &HashSet<String>) {
    attrs.retain(|a| !is_user_attr(a, user_attrs));
}

fn attr_literal_expr<'a>(span: SimpleSpan, lit: &AttrLit<'a>) -> Output<'a> {
    match lit {
        AttrLit::String(s) => str_lit(span, s),
        AttrLit::Int(i) => at(span, Expression::Integer(*i)),
        AttrLit::Float(f) => at(span, Expression::Float(*f)),
        AttrLit::Bool(b) => at(span, Expression::Bool(*b)),
    }
}

fn resolve_attr_extras<'a>(
    attr: &Attribute<'a>,
    extra_params: &[(&'static str, Option<Output>)],
    span: SimpleSpan,
    messages: &mut Vec<Message>,
) -> Option<Vec<Output<'a>>> {
    let mut values: Vec<Option<Output<'a>>> = vec![None; extra_params.len()];
    match &attr.args {
        AttrArgs::KeyValues(kvs) => {
            for (key, lit) in kvs {
                match extra_params.iter().position(|(n, _)| *n == *key) {
                    Some(idx) => values[idx] = Some(attr_literal_expr(span, lit)),
                    None => messages.push(Message::error(
                        ErrorCode::GenericTypeError,
                        format!("Unknown key `{}` in `#[{}(...)]`", key, attr.name),
                        span.into_range(),
                    )),
                }
            }
        }
        AttrArgs::Positional(lits) => {
            for (i, lit) in lits.iter().enumerate() {
                if i < extra_params.len() {
                    values[i] = Some(attr_literal_expr(span, lit));
                }
            }
        }
        AttrArgs::String(s) => {
            if !extra_params.is_empty() {
                values[0] = Some(str_lit(span, s));
            }
        }
        AttrArgs::Idents(idents) => {
            for (i, id) in idents.iter().enumerate() {
                if i < extra_params.len() {
                    values[i] = Some(ident(span, id));
                }
            }
        }
        AttrArgs::Empty => {}
    }
    let mut out = Vec::new();
    let mut missing = false;
    for (i, (name, _)) in extra_params.iter().enumerate() {
        match values[i].take() {
            Some(v) => out.push(v),
            None => {
                missing = true;
                messages.push(Message::error(
                    ErrorCode::GenericTypeError,
                    format!("Missing argument `{}` for `#[{}(...)]`", name, attr.name),
                    span.into_range(),
                ));
            }
        }
    }
    if missing {
        None
    } else {
        Some(out)
    }
}

fn collect_attr_function_bodies<'a>(decls: &[Output<'a>]) -> HashMap<String, Output<'a>> {
    let mut out = HashMap::new();
    for node in decls {
        if let Expression::Function {
            name,
            body: Some(body),
            ..
        } = node.1.as_ref()
        {
            out.insert((*name).to_string(), body.clone());
        }
    }
    out
}

fn find_attr_function_body<'a>(
    attr_bodies: &HashMap<String, Output<'a>>,
    name: &str,
) -> Option<Output<'a>> {
    attr_bodies.get(name).cloned()
}

fn is_target_args_spread(args: &Option<Vec<Output<'_>>>) -> bool {
    args.as_ref().is_some_and(|items| {
        items.len() == 1
            && matches!(
                items[0].1.as_ref(),
                Expression::Spread(inner)
                    if matches!(inner.1.as_ref(), Expression::Identifier("args"))
            )
    })
}

fn rewrite_expr_inline<'a>(
    expr: &Output<'a>,
    target: &Output<'a>,
    subs: &HashMap<&str, Output<'a>>,
    forward_idents: &[Output<'a>],
) -> Output<'a> {
    let span = expr.0;
    match expr.1.as_ref() {
        Expression::Call { name, args } => {
            if let Expression::Identifier(callee) = name.1.as_ref()
                && *callee == "target"
                && is_target_args_spread(args)
            {
                // Decoratee parameters are already in scope on the
                // expanded entry function — beta-reduce the call.
                return target.clone();
            }
            let new_name = rewrite_expr_inline(name, target, subs, forward_idents);
            let new_args = args.as_ref().map(|items| {
                items
                    .iter()
                    .map(|a| rewrite_expr_inline(a, target, subs, forward_idents))
                    .collect()
            });
            at(
                span,
                Expression::Call {
                    name: new_name,
                    args: new_args,
                },
            )
        }
        Expression::Identifier(name) => subs
            .get(*name)
            .cloned()
            .unwrap_or_else(|| expr.clone()),
        Expression::Return(inner) => at(
            span,
            Expression::Return(rewrite_expr_inline(inner, target, subs, forward_idents)),
        ),
        Expression::ImplicitReturn(inner) => at(
            span,
            Expression::ImplicitReturn(rewrite_expr_inline(inner, target, subs, forward_idents)),
        ),
        Expression::Print(fmt, params) => {
            let params = params.as_ref().map(|items| {
                items
                    .iter()
                    .map(|p| rewrite_expr_inline(p, target, subs, forward_idents))
                    .collect()
            });
            at(span, Expression::Print(fmt.clone(), params))
        }
        Expression::Format(fmt, params) => {
            let params = params.as_ref().map(|items| {
                items
                    .iter()
                    .map(|p| rewrite_expr_inline(p, target, subs, forward_idents))
                    .collect()
            });
            at(span, Expression::Format(fmt.clone(), params))
        }
        Expression::Block(items) => {
            let items = items
                .iter()
                .map(|s| rewrite_stmt_inline(s, target, subs, forward_idents))
                .collect();
            at(span, Expression::Block(items))
        }
        Expression::Expr(inner) => at(
            span,
            Expression::Expr(rewrite_expr_inline(inner, target, subs, forward_idents)),
        ),
        Expression::Statement(_inner) => rewrite_stmt_inline(expr, target, subs, forward_idents),
        Expression::ExprStatement(inner) => at(
            span,
            Expression::ExprStatement(rewrite_expr_inline(inner, target, subs, forward_idents)),
        ),
        Expression::Fragment(children) => {
            let children = children
                .iter()
                .map(|c| rewrite_expr_inline(c, target, subs, forward_idents))
                .collect();
            at(span, Expression::Fragment(children))
        }
        Expression::If(branches) => {
            let branches = branches
                .iter()
                .map(|branch| match branch.1.as_ref() {
                    Expression::Branch(cond, body) => at(
                        branch.0,
                        Expression::Branch(
                            cond.as_ref().map(|c| {
                                rewrite_expr_inline(c, target, subs, forward_idents)
                            }),
                            rewrite_expr_inline(body, target, subs, forward_idents),
                        ),
                    ),
                    _ => rewrite_expr_inline(branch, target, subs, forward_idents),
                })
                .collect();
            at(span, Expression::If(branches))
        }
        Expression::Match { scrutinee, arms } => {
            let scrutinee = rewrite_expr_inline(scrutinee, target, subs, forward_idents);
            let arms = arms
                .iter()
                .map(|arm| MatchArm {
                    pattern: arm.pattern.clone(),
                    body: rewrite_expr_inline(&arm.body, target, subs, forward_idents),
                })
                .collect();
            at(span, Expression::Match { scrutinee, arms })
        }
        Expression::Loop {
            identifier,
            iterable,
            body,
        } => at(
            span,
            Expression::Loop {
                identifier: identifier
                    .as_ref()
                    .map(|id| rewrite_expr_inline(id, target, subs, forward_idents)),
                iterable: rewrite_expr_inline(iterable, target, subs, forward_idents),
                body: rewrite_expr_inline(body, target, subs, forward_idents),
            },
        ),
        Expression::For {
            init,
            cond,
            step,
            body,
        } => at(
            span,
            Expression::For {
                init: init
                    .as_ref()
                    .map(|e| rewrite_expr_inline(e, target, subs, forward_idents)),
                cond: rewrite_expr_inline(cond, target, subs, forward_idents),
                step: step
                    .as_ref()
                    .map(|e| rewrite_expr_inline(e, target, subs, forward_idents)),
                body: rewrite_expr_inline(body, target, subs, forward_idents),
            },
        ),
        _ => expr.clone(),
    }
}

fn rewrite_stmt_inline<'a>(
    stmt: &Output<'a>,
    target: &Output<'a>,
    subs: &HashMap<&str, Output<'a>>,
    forward_idents: &[Output<'a>],
) -> Output<'a> {
    if let Expression::Statement(inner) = stmt.1.as_ref() {
        let span = stmt.0;
        at(
            span,
            Expression::Statement(rewrite_expr_inline(inner, target, subs, forward_idents)),
        )
    } else {
        rewrite_expr_inline(stmt, target, subs, forward_idents)
    }
}

fn inline_attr_body<'a>(
    attr_body: &Output<'a>,
    target: Output<'a>,
    extras: &[Output<'a>],
    extra_param_names: &[String],
    forward_idents: &[Output<'a>],
) -> Output<'a> {
    let mut subs = HashMap::new();
    for (name, expr) in extra_param_names.iter().zip(extras.iter()) {
        subs.insert(name.as_str(), expr.clone());
    }
    rewrite_expr_inline(attr_body, &target, &subs, forward_idents)
}

fn expand_function_user_attrs<'a>(
    attr_bodies: &HashMap<String, Output<'a>>,
    attrs: &mut Vec<Attribute<'a>>,
    args: &Output<'a>,
    body: &mut Option<Output<'a>>,
    user_attrs: &HashSet<String>,
    attr_extra_names: &HashMap<String, Vec<String>>,
    span: SimpleSpan,
    messages: &mut Vec<Message>,
) {
    let user_attrs_copy: Vec<Attribute<'static>> = user_attrs_on(attrs, user_attrs)
        .iter()
        .map(|a| clone_attr_static(a))
        .collect();
    if user_attrs_copy.is_empty() {
        return;
    }
    let Some(orig_body) = body.take() else {
        return;
    };
    strip_user_attrs(attrs, user_attrs);
    let orig_for_fallback = orig_body.clone();
    let mut wrapped = orig_body;
    for attr in user_attrs_copy.iter().rev() {
        let extra_params: Vec<(&'static str, Option<Output<'a>>)> = attr_extra_names
            .get(attr.name)
            .map(|names| {
                names
                    .iter()
                    .map(|n| (leak(n.clone()), None))
                    .collect()
            })
            .unwrap_or_default();
        let extra_names = attr_extra_names
            .get(attr.name)
            .cloned()
            .unwrap_or_default();
        let Some(extras) = resolve_attr_extras(attr, &extra_params, span, messages) else {
            continue;
        };
        if let Some(attr_body) = find_attr_function_body(attr_bodies, attr.name) {
            wrapped = inline_attr_body(
                &attr_body,
                wrapped,
                &extras,
                &extra_names,
                &[],
            );
        } else {
            let params = fn_param_nodes(args);
            let param_idents: Vec<Output<'a>> = params
                .iter()
                .filter(|(_, _, rest)| !*rest)
                .map(|(_, name, _)| ident(span, name))
                .collect();
            let inner = at(
                span,
                Expression::Lambda {
                    args: args.clone(),
                    captures: vec![],
                    body: orig_for_fallback.clone(),
                },
            );
            let mut call_args = vec![inner];
            call_args.extend(extras);
            call_args.extend(param_idents.iter().cloned());
            wrapped = at(
                span,
                Expression::Call {
                    name: ident(span, attr.name),
                    args: Some(call_args),
                },
            );
        }
    }
    *body = Some(block_return(span, wrapped));
}

fn synthesize_class_ctor<'a>(
    span: SimpleSpan,
    class_name: &'a str,
    fields: &[Output<'a>],
) -> Output<'a> {
    let mut args = Vec::new();
    let mut call_args = Vec::new();
    for field in fields {
        if let Expression::Field(_, name_expr, ty_expr) = field.1.as_ref() {
            if let Expression::Identifier(name) = name_expr.1.as_ref() {
                args.push(at(
                    span,
                    Expression::Argument(Some(ty_expr.clone()), name, false),
                ));
                call_args.push(ident(span, name));
            }
        }
    }
    let ctor_name = leak(format!("{class_name}__ctor"));
    let body = block_return(
        span,
        at(
            span,
            Expression::Instantiate(ident(span, class_name), Some(call_args)),
        ),
    );
    at(
        span,
        Expression::Function {
            attrs: vec![],
            name: ctor_name,
            is_coro: false,
            type_params: vec![],
            args: at(span, Expression::Fragment(args)),
            returns: Some(ty_name(span, class_name)),
            where_constraints: vec![],
            body: Some(body),
        },
    )
}

/// Shape info needed to synthesize derive methods (no borrow of the decl AST).
#[derive(Clone)]
enum VariantShape<'a> {
    Unit,
    Tuple(usize),
    Record(Vec<&'a str>),
}

#[derive(Clone)]
struct VariantMeta<'a> {
    name: &'a str,
    shape: VariantShape<'a>,
}

fn variant_metas<'a>(variants: &[Output<'a>]) -> Vec<VariantMeta<'a>> {
    variants
        .iter()
        .filter_map(|v| match v.1.as_ref() {
            Expression::EnumVariant { name, payload } => Some(VariantMeta {
                name,
                shape: match payload {
                    EnumVariantPayload::Unit => VariantShape::Unit,
                    EnumVariantPayload::Tuple(parts) => VariantShape::Tuple(parts.len()),
                    EnumVariantPayload::Record(fields) => {
                        VariantShape::Record(fields.iter().map(|f| f.name).collect())
                    }
                },
            }),
            _ => None,
        })
        .collect()
}

fn expand_decls<'a>(
    decls: &mut Vec<Output<'a>>,
    user_attrs: &HashSet<String>,
    attr_extra_names: &HashMap<String, Vec<String>>,
    attr_bodies: &HashMap<String, Output<'a>>,
    decorated_class_ctors: &mut HashMap<String, String>,
) -> Vec<Message> {
    let mut messages = Vec::new();
    let mut i = 0;
    while i < decls.len() {
        let span = decls[i].0;

        // `#[ffi]` signature-only function -> `extern` block lowering input.
        if let Expression::Function {
            attrs,
            name,
            is_coro,
            args,
            returns,
            body,
            ..
        } = decls[i].1.as_mut()
        {
            let is_ffi_sig = body.is_none();
            validate_attrs(attrs, "function", user_attrs, &mut messages, span, is_ffi_sig);
            if body.is_some() {
                if *is_coro && !user_attrs_on(attrs, user_attrs).is_empty() {
                    messages.push(Message::error(
                        ErrorCode::GenericTypeError,
                        "user-defined attributes are not supported on `async fn`".to_string(),
                        span.into_range(),
                    ));
                    strip_user_attrs(attrs, user_attrs);
                } else {
                    expand_function_user_attrs(
                        attr_bodies,
                        attrs,
                        args,
                        body,
                        user_attrs,
                        attr_extra_names,
                        span,
                        &mut messages,
                    );
                }
            }
            if is_ffi_sig {
                if let Some(ffi) = parse_ffi_attr(attrs, &mut messages, span) {
                    if *is_coro {
                        messages.push(Message::error(
                            ErrorCode::GenericTypeError,
                            "`#[ffi]` cannot be applied to `async fn`".to_string(),
                            span.into_range(),
                        ));
                    } else if attrs.iter().any(|a| a.name == "test") {
                        messages.push(Message::error(
                            ErrorCode::GenericTypeError,
                            "`#[ffi]` cannot be combined with `#[test]`".to_string(),
                            span.into_range(),
                        ));
                    } else {
                        let zs_name = *name;
                        let c_sym = ffi.symbol.unwrap_or(zs_name);
                        let extern_fn = ExternFunction {
                            name: zs_name,
                            symbol: if c_sym != zs_name {
                                Some(c_sym)
                            } else {
                                None
                            },
                            args: args.clone(),
                            returns: returns.clone(),
                            variadic: ffi.variadic,
                        };
                        decls[i] = make_extern_block(span, ffi.lib, extern_fn);
                        i += 1;
                        continue;
                    }
                } else if !attrs.iter().any(|a| a.name == "ffi") {
                    messages.push(Message::error(
                        ErrorCode::GenericTypeError,
                        "Signature-only function requires `#[ffi(...)]`".to_string(),
                        span.into_range(),
                    ));
                }
            } else if attrs.iter().any(|a| a.name == "ffi") {
                messages.push(Message::error(
                    ErrorCode::GenericTypeError,
                    "`#[ffi]` requires a signature-only function (`fn name(...) -> T;`)"
                        .to_string(),
                    span.into_range(),
                ));
            }
        }

        // Expand user attrs on impl methods.
        if let Expression::Implementation { methods, .. } = decls[i].1.as_mut() {
            for method in methods.iter_mut() {
                if let Expression::Method(_, func_out) = method.1.as_mut() {
                    if let Expression::Function {
                        attrs,
                        args,
                        body,
                        ..
                    } = func_out.1.as_mut()
                    {
                        validate_attrs(attrs, "function", user_attrs, &mut messages, span, false);
                        if body.is_some() {
                            expand_function_user_attrs(
                                attr_bodies,
                                attrs,
                                args,
                                body,
                                user_attrs,
                                attr_extra_names,
                                span,
                                &mut messages,
                            );
                        }
                    }
                }
            }
        }

        enum Job<'a> {
            Enum {
                name: &'a str,
                generic: bool,
                derives: Vec<&'a str>,
                variants: Vec<VariantMeta<'a>>,
            },
            Class {
                name: &'a str,
                generic: bool,
                derives: Vec<&'a str>,
                fields: Vec<&'a str>,
            },
        }
        let job = match decls[i].1.as_ref() {
            Expression::EnumDecl {
                name,
                type_params,
                attrs,
                variants,
            } => {
                validate_attrs(attrs, "enum", user_attrs, &mut messages, span, false);
                let derives = derive_traits_from_attrs(attrs);
                if derives.is_empty() {
                    None
                } else {
                    Some(Job::Enum {
                        name,
                        generic: !type_params.is_empty(),
                        derives,
                        variants: variant_metas(variants),
                    })
                }
            }
            Expression::Class {
                name,
                type_params,
                attrs,
                fields,
            } => {
                validate_attrs(attrs, "class", user_attrs, &mut messages, span, false);
                let derives = derive_traits_from_attrs(attrs);
                if derives.is_empty() {
                    None
                } else {
                    Some(Job::Class {
                        name,
                        generic: !type_params.is_empty(),
                        derives,
                        fields: class_field_names(fields),
                    })
                }
            }
            _ => None,
        };

        let mut ctor_insert: Option<Output> = None;
        if let Expression::Class {
            name,
            attrs,
            fields,
            ..
        } = decls[i].1.as_ref()
        {
            if !user_attrs_on(attrs, user_attrs).is_empty() {
                let class_name = *name;
                let fields_copy = fields.clone();
                let mut ctor = synthesize_class_ctor(span, class_name, &fields_copy);
                if let Expression::Function {
                    attrs: ctor_attrs,
                    args,
                    body,
                    ..
                } = ctor.1.as_mut()
                {
                    // User attrs live on the class decl; copy them onto the
                    // synthesized ctor so expansion wraps construction.
                    *ctor_attrs = attrs
                        .iter()
                        .filter(|a| is_user_attr(a, user_attrs))
                        .cloned()
                        .collect();
                    expand_function_user_attrs(
                        attr_bodies,
                        ctor_attrs,
                        args,
                        body,
                        user_attrs,
                        attr_extra_names,
                        span,
                        &mut messages,
                    );
                }
                decorated_class_ctors.insert(class_name.to_string(), format!("{class_name}__ctor"));
                strip_user_attrs(
                    match decls[i].1.as_mut() {
                        Expression::Class { attrs, .. } => attrs,
                        _ => unreachable!(),
                    },
                    user_attrs,
                );
                ctor_insert = Some(ctor);
            }
        }

        let synthesized = match job {
            Some(Job::Enum {
                name,
                generic,
                derives,
                variants,
            }) => Some(expand_enum(
                span,
                name,
                generic,
                &derives,
                &variants,
                &mut messages,
            )),
            Some(Job::Class {
                name,
                generic,
                derives,
                fields,
            }) => Some(expand_class(
                span,
                name,
                generic,
                &derives,
                &fields,
                &mut messages,
            )),
            None => None,
        };

        if let Some(impls) = synthesized {
            if let Expression::EnumDecl { attrs, .. } | Expression::Class { attrs, .. } =
                decls[i].1.as_mut()
            {
                strip_processed_attrs(attrs);
            }
            let n = impls.len();
            for (offset, impl_node) in impls.into_iter().enumerate() {
                decls.insert(i + 1 + offset, impl_node);
            }
            let mut advance = 1 + n;
            if let Some(ctor) = ctor_insert {
                decls.insert(i + advance, ctor);
                advance += 1;
            }
            i += advance;
        } else if let Some(ctor) = ctor_insert {
            decls.insert(i + 1, ctor);
            i += 2;
        } else {
            i += 1;
        }
    }
    messages
}

fn expand_enum<'a>(
    span: SimpleSpan,
    name: &'a str,
    generic: bool,
    derives: &[&'a str],
    variants: &[VariantMeta<'a>],
    messages: &mut Vec<Message>,
) -> Vec<Output<'a>> {
    if generic {
        messages.push(Message::error(
            ErrorCode::GenericTypeError,
            format!(
                "Cannot derive traits for generic enum `{}`; write an explicit `impl`",
                name
            ),
            span.into_range(),
        ));
        return Vec::new();
    }

    let mut out = Vec::new();
    for &trait_name in derives {
        if let Some(msg) = check_derivable(trait_name, span) {
            messages.push(msg);
            continue;
        }
        match trait_name {
            "Show" => out.push(synth_show_enum(span, name, variants)),
            "Eq" => out.push(synth_eq_enum(span, name, variants)),
            // Ord is a no-method supertrait over Lt/Le/Gt/Ge (PR #14).
            "Ord" => out.extend(synth_ord_enum(span, name, variants)),
            _ => unreachable!(),
        }
    }
    out
}

fn expand_class<'a>(
    span: SimpleSpan,
    name: &'a str,
    generic: bool,
    derives: &[&'a str],
    field_names: &[&'a str],
    messages: &mut Vec<Message>,
) -> Vec<Output<'a>> {
    if generic {
        messages.push(Message::error(
            ErrorCode::GenericTypeError,
            format!(
                "Cannot derive traits for generic class `{}`; write an explicit `impl`",
                name
            ),
            span.into_range(),
        ));
        return Vec::new();
    }

    let mut out = Vec::new();
    for &trait_name in derives {
        if let Some(msg) = check_derivable(trait_name, span) {
            messages.push(msg);
            continue;
        }
        match trait_name {
            "Show" => out.push(synth_show_class(span, name, field_names)),
            "Eq" => out.push(synth_eq_class(span, name, field_names)),
            "Ord" => out.extend(synth_ord_class(span, name, field_names)),
            _ => unreachable!(),
        }
    }
    out
}

fn check_derivable(trait_name: &str, span: SimpleSpan) -> Option<Message> {
    if DERIVABLE.contains(&trait_name) {
        None
    } else {
        let mut msg = Message::error(
            ErrorCode::GenericTypeError,
            format!("Cannot derive unknown or non-derivable trait `{}`", trait_name),
            span.into_range(),
        );
        msg.with_help(format!(
            "derivable traits are: {}",
            DERIVABLE.join(", ")
        ));
        Some(msg)
    }
}

fn class_field_names<'a>(fields: &[Output<'a>]) -> Vec<&'a str> {
    fields
        .iter()
        .filter_map(|f| match f.1.as_ref() {
            Expression::Field(_, name_expr, _) => match name_expr.1.as_ref() {
                Expression::Identifier(n) => Some(*n),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

// ── string interning for synthetic AST ──────────────────────────────────────

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn clone_attr_lit_static(lit: &AttrLit<'_>) -> AttrLit<'static> {
    match lit {
        AttrLit::String(s) => AttrLit::String(leak(s.to_string())),
        AttrLit::Int(n) => AttrLit::Int(*n),
        AttrLit::Float(f) => AttrLit::Float(*f),
        AttrLit::Bool(b) => AttrLit::Bool(*b),
    }
}

fn clone_attr_args_static(args: &AttrArgs<'_>) -> AttrArgs<'static> {
    match args {
        AttrArgs::Empty => AttrArgs::Empty,
        AttrArgs::Idents(v) => AttrArgs::Idents(v.iter().map(|s| leak(s.to_string())).collect()),
        AttrArgs::KeyValues(kvs) => AttrArgs::KeyValues(
            kvs.iter()
                .map(|(k, lit)| (leak(k.to_string()), clone_attr_lit_static(lit)))
                .collect(),
        ),
        AttrArgs::Positional(lits) => AttrArgs::Positional(
            lits.iter().map(clone_attr_lit_static).collect(),
        ),
        AttrArgs::String(s) => AttrArgs::String(leak(s.to_string())),
    }
}

fn clone_attr_static(attr: &Attribute<'_>) -> Attribute<'static> {
    Attribute {
        name: leak(attr.name.to_string()),
        args: clone_attr_args_static(&attr.args),
    }
}

/// Mint a unique span for each synthetic node.
///
/// Sharing the owning `enum`/`class` span across every derived expression
/// makes span-keyed codegen lookups (`lookup_for_codegen_span`, `%v` Show
/// lowering) collide and pick up the declaration's `unit` type. Unique
/// micro-spans keep the ID/infer caches aligned; expand diagnostics still
/// use the real header span from `expand_decls`.
fn fresh_span() -> SimpleSpan {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0x4000_0000);
    let start = NEXT.fetch_add(1, Ordering::Relaxed);
    SimpleSpan::from(start..start + 1)
}

fn at<'a>(_diag_span: SimpleSpan, expr: Expression<'a>) -> Output<'a> {
    (fresh_span(), Box::new(expr))
}

fn ty_name<'a>(span: SimpleSpan, name: &'a str) -> Output<'a> {
    at(span, Expression::Type(name))
}

fn ident<'a>(span: SimpleSpan, name: &'a str) -> Output<'a> {
    at(span, Expression::Identifier(name))
}

fn str_lit<'a>(span: SimpleSpan, s: &'a str) -> Output<'a> {
    at(span, Expression::String(s))
}

fn stmt<'a>(span: SimpleSpan, inner: Output<'a>) -> Output<'a> {
    at(span, Expression::Statement(inner))
}

fn block_return<'a>(span: SimpleSpan, value: Output<'a>) -> Output<'a> {
    at(
        span,
        Expression::Block(vec![stmt(
            span,
            at(span, Expression::Return(value)),
        )]),
    )
}

fn method_fn<'a>(
    span: SimpleSpan,
    name: &'a str,
    args: Vec<Output<'a>>,
    ret: &'a str,
    body: Output<'a>,
) -> Output<'a> {
    let func = at(
        span,
        Expression::Function {
            attrs: vec![],
            name,
            is_coro: false,
            type_params: vec![],
            args: at(span, Expression::Fragment(args)),
            returns: Some(ty_name(span, ret)),
            where_constraints: vec![],
            body: Some(body),
        },
    );
    at(span, Expression::Method(Visibility::Private, func))
}

fn arg<'a>(span: SimpleSpan, ty: &'a str, name: &'a str) -> Output<'a> {
    at(
        span,
        Expression::Argument(Some(ty_name(span, ty)), name, false),
    )
}

fn typeclass_impl<'a>(
    span: SimpleSpan,
    class: &'a str,
    self_ty: &'a str,
    methods: Vec<Output<'a>>,
) -> Output<'a> {
    at(
        span,
        Expression::TypeClassImpl {
            class,
            args: vec![ty_name(span, self_ty)],
            methods,
        },
    )
}

// ── Show (enum) ─────────────────────────────────────────────────────────────

fn synth_show_enum<'a>(
    span: SimpleSpan,
    enum_name: &'a str,
    variants: &[VariantMeta<'a>],
) -> Output<'a> {
    // Unique param name — `codegen_var_types` is a flat map keyed by
    // simple name; two derived `show(p)` methods would clobber each other.
    let p = leak(format!("__show_{}", enum_name));
    let mut arms = Vec::new();
    for v in variants {
        let (pattern, fmt, fmt_args) = show_variant_arm(span, enum_name, v.name, &v.shape, p);
        let body = at(
            span,
            Expression::Format(str_lit(span, fmt), Some(fmt_args)),
        );
        arms.push(MatchArm { pattern, body });
    }
    let match_expr = at(
        span,
        Expression::Match {
            scrutinee: ident(span, p),
            arms,
        },
    );
    let body = block_return(span, match_expr);
    let show_m = method_fn(span, "show", vec![arg(span, enum_name, p)], "string", body);
    typeclass_impl(span, "Show", enum_name, vec![show_m])
}

fn show_variant_arm<'a>(
    span: SimpleSpan,
    enum_name: &'a str,
    vname: &'a str,
    shape: &VariantShape<'a>,
    recv: &'a str,
) -> (Pattern<'a>, &'static str, Vec<Output<'a>>) {
    match shape {
        VariantShape::Unit => {
            let fmt = leak(format!("{}::{}", enum_name, vname));
            (
                Pattern::Constructor {
                    enum_name,
                    variant_name: vname,
                    payload: PatternPayload::Unit,
                },
                fmt,
                vec![],
            )
        }
        VariantShape::Tuple(arity) => {
            // Wildcard payload + `recv.<i>` Access (synthetic tuple field
            // names `"0"`, `"1"`, …). Match binders in instance methods
            // disagree with JUMP_IF_MATCH push slots once `__dictN` is
            // present; LoadField via Access avoids that bug.
            let mut fmt_args = Vec::new();
            let mut specs = Vec::new();
            for i in 0..*arity {
                let fname = leak(i.to_string());
                fmt_args.push(at(span, Expression::Access(ident(span, recv), fname)));
                specs.push("%v");
            }
            let fmt = leak(format!(
                "{}::{}({})",
                enum_name,
                vname,
                specs.join(", ")
            ));
            (
                Pattern::Constructor {
                    enum_name,
                    variant_name: vname,
                    payload: PatternPayload::Tuple(vec![Pattern::Wildcard; *arity]),
                },
                fmt,
                fmt_args,
            )
        }
        VariantShape::Record(fields) => {
            // Wildcard payload + `recv.field` access — avoids match-binding
            // slots overwriting `__dict0` / sibling args in instance methods.
            let mut fmt_args = Vec::new();
            let mut specs = Vec::new();
            for &fname in fields {
                fmt_args.push(at(span, Expression::Access(ident(span, recv), fname)));
                specs.push(format!("{}: %v", fname));
            }
            let fmt = leak(format!(
                "{}::{} {{ {} }}",
                enum_name,
                vname,
                specs.join(", ")
            ));
            (
                Pattern::Constructor {
                    enum_name,
                    variant_name: vname,
                    payload: PatternPayload::Record(
                        fields
                            .iter()
                            .map(|fname| PatternField {
                                name: fname,
                                pattern: Pattern::Wildcard,
                            })
                            .collect(),
                    ),
                },
                fmt,
                fmt_args,
            )
        }
    }
}

// ── Eq (enum) ───────────────────────────────────────────────────────────────

fn synth_eq_enum<'a>(
    span: SimpleSpan,
    enum_name: &'a str,
    variants: &[VariantMeta<'a>],
) -> Output<'a> {
    let a = leak(format!("__eq_a_{}", enum_name));
    let b = leak(format!("__eq_b_{}", enum_name));
    let mut arms = Vec::new();
    for v in variants {
        let (pat_a, body) = eq_variant_arm(span, enum_name, v.name, &v.shape, a, b);
        arms.push(MatchArm {
            pattern: pat_a,
            body,
        });
    }
    // Defensive fallback (should be unreachable if exhaustive).
    arms.push(MatchArm {
        pattern: Pattern::Wildcard,
        body: at(span, Expression::Bool(false)),
    });
    let match_expr = at(
        span,
        Expression::Match {
            scrutinee: ident(span, a),
            arms,
        },
    );
    let eq_body = block_return(span, match_expr);
    let eq_m = method_fn(
        span,
        "eq",
        vec![arg(span, enum_name, a), arg(span, enum_name, b)],
        "bool",
        eq_body,
    );

    // ne(a, b) = !(a == b) — uses the Eq instance once `==` is wired.
    let ne_cmp = at(span, Expression::Eq(ident(span, a), ident(span, b)));
    let ne_body = block_return(span, at(span, Expression::LogicalNot(ne_cmp)));
    let ne_m = method_fn(
        span,
        "ne",
        vec![arg(span, enum_name, a), arg(span, enum_name, b)],
        "bool",
        ne_body,
    );

    typeclass_impl(span, "Eq", enum_name, vec![eq_m, ne_m])
}

fn eq_variant_arm<'a>(
    span: SimpleSpan,
    enum_name: &'a str,
    vname: &'a str,
    shape: &VariantShape<'a>,
    a_name: &'a str,
    b_name: &'a str,
) -> (Pattern<'a>, Output<'a>) {
    match shape {
        VariantShape::Unit => {
            let inner_arms = vec![
                MatchArm {
                    pattern: Pattern::Constructor {
                        enum_name,
                        variant_name: vname,
                        payload: PatternPayload::Unit,
                    },
                    body: at(span, Expression::Bool(true)),
                },
                MatchArm {
                    pattern: Pattern::Wildcard,
                    body: at(span, Expression::Bool(false)),
                },
            ];
            let body = at(
                span,
                Expression::Match {
                    scrutinee: ident(span, b_name),
                    arms: inner_arms,
                },
            );
            (
                Pattern::Constructor {
                    enum_name,
                    variant_name: vname,
                    payload: PatternPayload::Unit,
                },
                body,
            )
        }
        VariantShape::Tuple(arity) => {
            // Same-tag confirmation via wildcards, then compare through
            // synthetic Access indices on the original `a` / `b` params
            // (avoids nested-match binder + instance-method slot bugs).
            let mut cmp: Option<Output<'a>> = None;
            for i in 0..*arity {
                let fname = leak(i.to_string());
                let l = at(span, Expression::Access(ident(span, a_name), fname));
                let r = at(span, Expression::Access(ident(span, b_name), fname));
                let eq = at(span, Expression::Eq(l, r));
                cmp = Some(match cmp {
                    None => eq,
                    Some(prev) => at(span, Expression::And(prev, eq)),
                });
            }
            let cmp = cmp.unwrap_or_else(|| at(span, Expression::Bool(true)));
            let wild_tuple = PatternPayload::Tuple(vec![Pattern::Wildcard; *arity]);
            let inner_arms = vec![
                MatchArm {
                    pattern: Pattern::Constructor {
                        enum_name,
                        variant_name: vname,
                        payload: wild_tuple.clone(),
                    },
                    body: cmp,
                },
                MatchArm {
                    pattern: Pattern::Wildcard,
                    body: at(span, Expression::Bool(false)),
                },
            ];
            let body = at(
                span,
                Expression::Match {
                    scrutinee: ident(span, b_name),
                    arms: inner_arms,
                },
            );
            (
                Pattern::Constructor {
                    enum_name,
                    variant_name: vname,
                    payload: wild_tuple,
                },
                body,
            )
        }
        VariantShape::Record(fields) => {
            // Field access on `a` / `b` after same-tag confirmation — avoids
            // the nested-match binding limitation.
            let mut cmp: Option<Output<'a>> = None;
            for &fname in fields {
                let l = at(span, Expression::Access(ident(span, a_name), fname));
                let r = at(span, Expression::Access(ident(span, b_name), fname));
                let eq = at(span, Expression::Eq(l, r));
                cmp = Some(match cmp {
                    None => eq,
                    Some(prev) => at(span, Expression::And(prev, eq)),
                });
            }
            let cmp = cmp.unwrap_or_else(|| at(span, Expression::Bool(true)));
            let wild_record = PatternPayload::Record(
                fields
                    .iter()
                    .map(|fname| PatternField {
                        name: fname,
                        pattern: Pattern::Wildcard,
                    })
                    .collect(),
            );
            let inner_arms = vec![
                MatchArm {
                    pattern: Pattern::Constructor {
                        enum_name,
                        variant_name: vname,
                        payload: wild_record.clone(),
                    },
                    body: cmp,
                },
                MatchArm {
                    pattern: Pattern::Wildcard,
                    body: at(span, Expression::Bool(false)),
                },
            ];
            let body = at(
                span,
                Expression::Match {
                    scrutinee: ident(span, b_name),
                    arms: inner_arms,
                },
            );
            (
                Pattern::Constructor {
                    enum_name,
                    variant_name: vname,
                    payload: wild_record,
                },
                body,
            )
        }
    }
}

// ── Ord (enum) ──────────────────────────────────────────────────────────────

/// Expand `derive Ord` into the four comparison instances plus an empty
/// `Ord` marker, matching the builtin `int`/`float` layout after PR #14
/// (`Ord` has no methods; `T: Ord` implies `Lt`/`Le`/`Gt`/`Ge`).
fn synth_ord_enum<'a>(
    span: SimpleSpan,
    enum_name: &'a str,
    variants: &[VariantMeta<'a>],
) -> Vec<Output<'a>> {
    // Encode tag order via nested matches: compare tags by walking variants
    // in declaration order. For equal tags, compare payloads field-wise.
    // Per-op param names avoid clobbering the flat `codegen_var_types` map.
    let mut out = Vec::with_capacity(5);
    for op in [OrdOp::Lt, OrdOp::Le, OrdOp::Gt, OrdOp::Ge] {
        let a = leak(format!("__ord_{}_a_{}", op.name(), enum_name));
        let b = leak(format!("__ord_{}_b_{}", op.name(), enum_name));
        let method = ord_method(span, enum_name, variants, a, b, op);
        out.push(typeclass_impl(span, op.trait_name(), enum_name, vec![method]));
    }
    out.push(typeclass_impl(span, "Ord", enum_name, vec![]));
    out
}

#[derive(Clone, Copy)]
enum OrdOp {
    Lt,
    Le,
    Gt,
    Ge,
}

impl OrdOp {
    fn name(self) -> &'static str {
        match self {
            OrdOp::Lt => "lt",
            OrdOp::Le => "le",
            OrdOp::Gt => "gt",
            OrdOp::Ge => "ge",
        }
    }

    fn trait_name(self) -> &'static str {
        match self {
            OrdOp::Lt => "Lt",
            OrdOp::Le => "Le",
            OrdOp::Gt => "Gt",
            OrdOp::Ge => "Ge",
        }
    }

    /// Strict field inequality used in the lexicographic fold.
    ///
    /// AST note: `Expression::Le` / `Gt` are the strict `<` / `>` operators
    /// (see `infer_comparison`); inclusive `<=` / `>=` are `Leq` / `Geq`.
    /// Inclusive `Le`/`Ge` still use strict `<`/`>` here so equal prefixes
    /// fall through to the `(== && rest)` arm; the inclusive case is handled
    /// by [`eq_payload_result`] on the final empty-payload base.
    fn primary(self) -> for<'a> fn(SimpleSpan, Output<'a>, Output<'a>) -> Output<'a> {
        match self {
            OrdOp::Lt | OrdOp::Le => |s, l, r| at(s, Expression::Le(l, r)),
            OrdOp::Gt | OrdOp::Ge => |s, l, r| at(s, Expression::Gt(l, r)),
        }
    }

    /// When tags differ and left tag index < right tag index.
    fn when_left_tag_less(self) -> bool {
        matches!(self, OrdOp::Lt | OrdOp::Le)
    }

    /// When tags are equal — use `<=`/`>=`/`</>` on fields; for Le/Ge
    /// equal payloads must return true.
    fn eq_payload_result(self) -> bool {
        matches!(self, OrdOp::Le | OrdOp::Ge)
    }
}

fn ord_method<'a>(
    span: SimpleSpan,
    enum_name: &'a str,
    variants: &[VariantMeta<'a>],
    a: &'a str,
    b: &'a str,
    op: OrdOp,
) -> Output<'a> {
    // Record/Unit: tag-only wildcards + `a.field` / `b.field` after same-tag.
    // Tuple: outer binders → `let` bridge → inner binders (nested match arms
    // do not see outer pattern bindings at codegen time; enums are not
    // indexable, so `a[i]` is not an option).
    let mut arms = Vec::new();
    for (i, v) in variants.iter().enumerate() {
        let (pattern, body) = ord_outer_arm(span, enum_name, variants, i, v, a, b, op);
        arms.push(MatchArm { pattern, body });
    }
    arms.push(MatchArm {
        pattern: Pattern::Wildcard,
        body: at(span, Expression::Bool(false)),
    });
    let match_expr = at(
        span,
        Expression::Match {
            scrutinee: ident(span, a),
            arms,
        },
    );
    method_fn(
        span,
        op.name(),
        vec![arg(span, enum_name, a), arg(span, enum_name, b)],
        "bool",
        block_return(span, match_expr),
    )
}

fn ord_outer_arm<'a>(
    span: SimpleSpan,
    enum_name: &'a str,
    variants: &[VariantMeta<'a>],
    left_idx: usize,
    left: &VariantMeta<'a>,
    a: &'a str,
    b: &'a str,
    op: OrdOp,
) -> (Pattern<'a>, Output<'a>) {
    let mut inner_arms = Vec::new();
    for (j, rv) in variants.iter().enumerate() {
        let body = if j == left_idx {
            ord_payload_cmp(span, &left.shape, a, b, op)
        } else if j > left_idx {
            at(span, Expression::Bool(op.when_left_tag_less()))
        } else {
            at(span, Expression::Bool(!op.when_left_tag_less()))
        };
        inner_arms.push(MatchArm {
            pattern: ord_wildcard_pattern(enum_name, rv.name, &rv.shape),
            body,
        });
    }
    inner_arms.push(MatchArm {
        pattern: Pattern::Wildcard,
        body: at(span, Expression::Bool(false)),
    });
    let body = at(
        span,
        Expression::Match {
            scrutinee: ident(span, b),
            arms: inner_arms,
        },
    );
    (
        ord_wildcard_pattern(enum_name, left.name, &left.shape),
        body,
    )
}

fn ord_wildcard_pattern<'a>(
    enum_name: &'a str,
    vname: &'a str,
    shape: &VariantShape<'a>,
) -> Pattern<'a> {
    match shape {
        VariantShape::Unit => Pattern::Constructor {
            enum_name,
            variant_name: vname,
            payload: PatternPayload::Unit,
        },
        VariantShape::Tuple(arity) => Pattern::Constructor {
            enum_name,
            variant_name: vname,
            payload: PatternPayload::Tuple(vec![Pattern::Wildcard; *arity]),
        },
        VariantShape::Record(fields) => Pattern::Constructor {
            enum_name,
            variant_name: vname,
            payload: PatternPayload::Record(
                fields
                    .iter()
                    .map(|fname| PatternField {
                        name: fname,
                        pattern: Pattern::Wildcard,
                    })
                    .collect(),
            ),
        },
    }
}

fn ord_payload_cmp<'a>(
    span: SimpleSpan,
    left: &VariantShape<'a>,
    a: &'a str,
    b: &'a str,
    op: OrdOp,
) -> Output<'a> {
    // Lexicographic compare via `a.field` / `b.field` (records) or
    // synthetic Access indices `"0"`, `"1"`, … (tuples).
    let primary = op.primary();
    let mut acc = at(span, Expression::Bool(op.eq_payload_result()));
    match left {
        VariantShape::Unit => acc,
        VariantShape::Tuple(arity) => {
            for i in (0..*arity).rev() {
                let fname = leak(i.to_string());
                let l = at(span, Expression::Access(ident(span, a), fname));
                let r = at(span, Expression::Access(ident(span, b), fname));
                let l2 = at(span, Expression::Access(ident(span, a), fname));
                let r2 = at(span, Expression::Access(ident(span, b), fname));
                let prim = primary(span, l, r);
                let eq = at(span, Expression::Eq(l2, r2));
                let and_rest = at(span, Expression::And(eq, acc));
                acc = at(span, Expression::Or(prim, and_rest));
            }
            acc
        }
        VariantShape::Record(fields) => {
            for &fname in fields.iter().rev() {
                let l = at(span, Expression::Access(ident(span, a), fname));
                let r = at(span, Expression::Access(ident(span, b), fname));
                let l2 = at(span, Expression::Access(ident(span, a), fname));
                let r2 = at(span, Expression::Access(ident(span, b), fname));
                let prim = primary(span, l, r);
                let eq = at(span, Expression::Eq(l2, r2));
                let and_rest = at(span, Expression::And(eq, acc));
                acc = at(span, Expression::Or(prim, and_rest));
            }
            acc
        }
    }
}

// ── Show / Eq / Ord (class) ─────────────────────────────────────────────────

fn synth_show_class<'a>(span: SimpleSpan, name: &'a str, fields: &[&'a str]) -> Output<'a> {
    let p = leak(format!("__show_{}", name));
    let mut specs = Vec::new();
    let mut fmt_args = Vec::new();
    for f in fields {
        specs.push(format!("{}: %v", f));
        fmt_args.push(at(span, Expression::Access(ident(span, p), f)));
    }
    let fmt = leak(format!("{} {{ {} }}", name, specs.join(", ")));
    let format = at(
        span,
        Expression::Format(str_lit(span, fmt), Some(fmt_args)),
    );
    let show_m = method_fn(
        span,
        "show",
        vec![arg(span, name, p)],
        "string",
        block_return(span, format),
    );
    typeclass_impl(span, "Show", name, vec![show_m])
}

fn synth_eq_class<'a>(span: SimpleSpan, name: &'a str, fields: &[&'a str]) -> Output<'a> {
    let a = leak(format!("__eq_a_{}", name));
    let b = leak(format!("__eq_b_{}", name));
    let mut cmp: Option<Output<'a>> = None;
    for f in fields {
        let l = at(span, Expression::Access(ident(span, a), f));
        let r = at(span, Expression::Access(ident(span, b), f));
        let eq = at(span, Expression::Eq(l, r));
        cmp = Some(match cmp {
            None => eq,
            Some(prev) => at(span, Expression::And(prev, eq)),
        });
    }
    let cmp = cmp.unwrap_or_else(|| at(span, Expression::Bool(true)));
    let eq_m = method_fn(
        span,
        "eq",
        vec![arg(span, name, a), arg(span, name, b)],
        "bool",
        block_return(span, cmp),
    );
    // ne(a, b) = !(a == b) — same as enum derive (do not call `eq` by name).
    let ne_cmp = at(span, Expression::Eq(ident(span, a), ident(span, b)));
    let ne_m = method_fn(
        span,
        "ne",
        vec![arg(span, name, a), arg(span, name, b)],
        "bool",
        block_return(span, at(span, Expression::LogicalNot(ne_cmp))),
    );
    typeclass_impl(span, "Eq", name, vec![eq_m, ne_m])
}

fn synth_ord_class<'a>(span: SimpleSpan, name: &'a str, fields: &[&'a str]) -> Vec<Output<'a>> {
    let mut out = Vec::with_capacity(5);
    for op in [OrdOp::Lt, OrdOp::Le, OrdOp::Gt, OrdOp::Ge] {
        let a = leak(format!("__ord_{}_a_{}", op.name(), name));
        let b = leak(format!("__ord_{}_b_{}", op.name(), name));
        let body = class_ord_body(span, a, b, fields, op);
        let method = method_fn(
            span,
            op.name(),
            vec![arg(span, name, a), arg(span, name, b)],
            "bool",
            block_return(span, body),
        );
        out.push(typeclass_impl(span, op.trait_name(), name, vec![method]));
    }
    out.push(typeclass_impl(span, "Ord", name, vec![]));
    out
}

fn class_ord_body<'a>(
    span: SimpleSpan,
    a: &'a str,
    b: &'a str,
    fields: &[&'a str],
    op: OrdOp,
) -> Output<'a> {
    if fields.is_empty() {
        return at(span, Expression::Bool(op.eq_payload_result()));
    }
    let primary = op.primary();
    let mut acc = at(span, Expression::Bool(op.eq_payload_result()));
    for f in fields.iter().rev() {
        let l = at(span, Expression::Access(ident(span, a), f));
        let r = at(span, Expression::Access(ident(span, b), f));
        let l2 = at(span, Expression::Access(ident(span, a), f));
        let r2 = at(span, Expression::Access(ident(span, b), f));
        let prim = primary(span, l, r);
        let eq = at(span, Expression::Eq(l2, r2));
        let and_rest = at(span, Expression::And(eq, acc));
        acc = at(span, Expression::Or(prim, and_rest));
    }
    acc
}
