//! Trait derive expansion (`enum Point derive Show, Eq { … }`).
//!
//! Expands header `derive` clauses into synthetic `TypeClassImpl` AST nodes
//! inserted immediately after the owning `enum` / `class` declaration, before
//! the ID pre-walk and typechecking.

use parser::{
    SimpleSpan,
    ast::{
        EnumVariantPayload, Expression, MatchArm, Output, Pattern, PatternField, PatternPayload,
        Visibility,
    },
};
use reporting::{ErrorCode, Message};

/// Builtin traits the compiler knows how to synthesize.
const DERIVABLE: &[&str] = &["Show", "Eq", "Ord"];

/// Expand every `derive` clause in a program AST.
///
/// Mutates `ast` in place when it is a `Program`. Returns diagnostics for
/// unknown / non-derivable traits and unsupported shapes (e.g. generics).
pub fn expand_program(ast: &mut Output<'_>) -> Vec<Message> {
    let Expression::Program(children) = ast.1.as_mut() else {
        return Vec::new();
    };
    expand_decls(children)
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

fn expand_decls(decls: &mut Vec<Output<'_>>) -> Vec<Message> {
    let mut messages = Vec::new();
    let mut i = 0;
    while i < decls.len() {
        let span = decls[i].0;
        // Snapshot everything we need, then drop the borrow of `decls[i]`
        // before inserting synthetic siblings.
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
                derives,
                variants,
            } if !derives.is_empty() => Some(Job::Enum {
                name,
                generic: !type_params.is_empty(),
                derives: derives.clone(),
                variants: variant_metas(variants),
            }),
            Expression::Class {
                name,
                type_params,
                derives,
                fields,
            } if !derives.is_empty() => Some(Job::Class {
                name,
                generic: !type_params.is_empty(),
                derives: derives.clone(),
                fields: class_field_names(fields),
            }),
            _ => None,
        };

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
            let n = impls.len();
            for (offset, impl_node) in impls.into_iter().enumerate() {
                decls.insert(i + 1 + offset, impl_node);
            }
            i += 1 + n;
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
            name,
            is_coro: false,
            type_params: vec![],
            args: at(span, Expression::Fragment(args)),
            returns: Some(ty_name(span, ret)),
            where_constraints: vec![],
            body,
        },
    );
    at(span, Expression::Method(Visibility::Private, func))
}

fn arg<'a>(span: SimpleSpan, ty: &'a str, name: &'a str) -> Output<'a> {
    at(span, Expression::Argument(ty_name(span, ty), name))
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
