//! Pretty-printing for [`Ty`] and [`Scheme`] (diagnostics and tests).

use std::collections::HashMap;
use std::fmt;

use super::subst::{Subst, apply_ty_prune};
use super::ty::{ArrayLength, EnumVariantPayloadTy, Scheme, Ty, TyVarId};

/// Format a type for user-facing diagnostics.
///
/// Applies the full substitution, then renames any remaining free
/// type variables to `a`, `b`, `c`, … so messages never show raw
/// counters like `` `t43` ``.
pub fn format_ty_for_diag(subst: &Subst, ty: &Ty) -> String {
    let pruned = apply_ty_prune(subst, ty);
    let mut rename = HashMap::new();
    let mut next = 0u32;
    format_ty_renamed(&pruned, &mut rename, &mut next)
}

fn fresh_diag_name(next: &mut u32) -> String {
    // a..z, then a1, b1, …
    let n = *next;
    *next += 1;
    if n < 26 {
        ((b'a' + n as u8) as char).to_string()
    } else {
        let letter = (b'a' + (n % 26) as u8) as char;
        format!("{}{}", letter, n / 26)
    }
}

fn format_ty_renamed(
    ty: &Ty,
    rename: &mut HashMap<TyVarId, String>,
    next: &mut u32,
) -> String {
    match ty {
        Ty::Var(v) => rename
            .entry(*v)
            .or_insert_with(|| fresh_diag_name(next))
            .clone(),
        Ty::Con(name) => name.clone(),
        Ty::Fun(a, b) => {
            let left = format_ty_renamed(a, rename, next);
            let right = format_ty_renamed(b, rename, next);
            if needs_paren(a) {
                format!("({}) -> {}", left, right)
            } else {
                format!("{} -> {}", left, right)
            }
        }
        Ty::App(c, args) => {
            if args.is_empty() {
                format_ty_renamed(c, rename, next)
            } else if let Ty::Con(name) = c.as_ref() {
                if name == "coroutine" && args.len() == 2 {
                    let y = format_ty_renamed(&args[0], rename, next);
                    let s = format_ty_renamed(&args[1], rename, next);
                    if matches!(&args[1], Ty::Con(n) if n == "unit") {
                        return format!("coroutine<{}>", y);
                    }
                    return format!("coroutine<{}, {}>", y, s);
                }
                let inner = args
                    .iter()
                    .map(|t| format_ty_renamed(t, rename, next))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}<{}>", name, inner)
            } else {
                let head = format_ty_renamed(c, rename, next);
                let inner = args
                    .iter()
                    .map(|t| format_ty_renamed(t, rename, next))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}<{}>", head, inner)
            }
        }
        Ty::List(inner) => format!("[{}]", format_ty_renamed(inner, rename, next)),
        Ty::Sum { name, variants } => {
            let mut out = format!("enum {} {{ ", name);
            for (i, (vname, payload)) in variants.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(vname);
                match payload {
                    EnumVariantPayloadTy::Unit => {}
                    EnumVariantPayloadTy::Tuple(tys) => {
                        out.push('(');
                        for (j, p) in tys.iter().enumerate() {
                            if j > 0 {
                                out.push_str(", ");
                            }
                            out.push_str(&format_ty_renamed(p, rename, next));
                        }
                        out.push(')');
                    }
                    EnumVariantPayloadTy::Record(fields) => {
                        out.push_str(" { ");
                        for (j, (fname, fty)) in fields.iter().enumerate() {
                            if j > 0 {
                                out.push_str(", ");
                            }
                            out.push_str(fname);
                            out.push_str(": ");
                            out.push_str(&format_ty_renamed(fty, rename, next));
                        }
                        out.push_str(" }");
                    }
                }
            }
            out.push_str(" }");
            out
        }
        Ty::Constructor { owner, tag, .. } => {
            format!("{}::v{}", format_ty_renamed(owner, rename, next), tag)
        }
        Ty::Tuple(tys) => {
            let inner = tys
                .iter()
                .map(|t| format_ty_renamed(t, rename, next))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({})", inner)
        }
        Ty::Array { element, length } => {
            let elem = format_ty_renamed(element, rename, next);
            match length {
                ArrayLength::Static(n) => format!("[{}; {}]", elem, n),
                ArrayLength::Dynamic => format!("[{}]", elem),
            }
        }
        Ty::Record { fields } => {
            let inner = fields
                .iter()
                .map(|(name, ty)| format!("{}: {}", name, format_ty_renamed(ty, rename, next)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ {} }}", inner)
        }
        Ty::Readonly(inner) => format!("readonly {}", format_ty_renamed(inner, rename, next)),
        Ty::Existential { class } => class.clone(),
        Ty::Forall {
            bounds,
            constraints,
            body,
        } => {
            for v in bounds {
                rename
                    .entry(*v)
                    .or_insert_with(|| fresh_diag_name(next));
            }
            let vars = bounds
                .iter()
                .map(|v| {
                    let mut s = rename
                        .get(v)
                        .cloned()
                        .unwrap_or_else(|| fresh_diag_name(next));
                    let classes = constraints
                        .iter()
                        .filter(|c| c.is_unary_on(*v))
                        .map(|c| c.class.as_str())
                        .collect::<Vec<_>>();
                    if !classes.is_empty() {
                        s.push_str(": ");
                        s.push_str(&classes.join(" + "));
                    }
                    s
                })
                .collect::<Vec<_>>()
                .join(", ");
            let body_s = format_ty_renamed(body, rename, next);
            let multi: Vec<String> = constraints
                .iter()
                .filter(|c| {
                    c.args.len() != 1 || c.primary_var().is_none_or(|v| !bounds.contains(&v))
                })
                .map(|c| c.to_string())
                .collect();
            if multi.is_empty() {
                format!("forall {}. {}", vars, body_s)
            } else {
                format!("forall {}. {} where {}", vars, body_s, multi.join(", "))
            }
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Var(v) => write!(f, "t{}", v.raw()),
            Ty::Con(name) => write!(f, "{}", name),
            Ty::Fun(a, b) => {
                if needs_paren(a) {
                    write!(f, "({}) -> {}", a, b)
                } else {
                    write!(f, "{} -> {}", a, b)
                }
            }
            Ty::App(c, args) => {
                if args.is_empty() {
                    write!(f, "{}", c)
                } else if let Ty::Con(name) = c.as_ref() {
                    if name == "coroutine" && args.len() == 2 {
                        let y = &args[0];
                        let s = &args[1];
                        if matches!(s, Ty::Con(n) if n == "unit") {
                            return write!(f, "coroutine<{}>", y);
                        }
                        return write!(f, "coroutine<{}, {}>", y, s);
                    }
                    write!(
                        f,
                        "{}<{}>",
                        c,
                        args.iter()
                            .map(|t| t.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                } else {
                    let inner = args
                        .iter()
                        .map(|t| t.to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    write!(f, "{}<{}>", c, inner)
                }
            }
            Ty::List(inner) => write!(f, "[{}]", inner),
            Ty::Sum { name, variants } => {
                write!(f, "enum {} {{ ", name)?;
                for (i, (vname, payload)) in variants.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", vname)?;
                    match payload {
                        EnumVariantPayloadTy::Unit => {}
                        EnumVariantPayloadTy::Tuple(tys) => {
                            write!(f, "(")?;
                            for (j, p) in tys.iter().enumerate() {
                                if j > 0 {
                                    write!(f, ", ")?;
                                }
                                write!(f, "{}", p)?;
                            }
                            write!(f, ")")?;
                        }
                        EnumVariantPayloadTy::Record(fields) => {
                            write!(f, " {{ ")?;
                            for (j, (fname, fty)) in fields.iter().enumerate() {
                                if j > 0 {
                                    write!(f, ", ")?;
                                }
                                write!(f, "{}: {}", fname, fty)?;
                            }
                            write!(f, " }}")?;
                        }
                    }
                }
                write!(f, " }}")
            }
            Ty::Constructor { owner, tag, .. } => {
                write!(f, "{}::v{}", owner, tag)
            }
            Ty::Tuple(tys) => {
                write!(f, "(")?;
                for (i, t) in tys.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", t)?;
                }
                write!(f, ")")
            }
            Ty::Array { element, length } => match length {
                ArrayLength::Static(n) => write!(f, "[{}; {}]", element, n),
                ArrayLength::Dynamic => write!(f, "[{}]", element),
            },
            Ty::Record { fields } => {
                write!(f, "{{ ")?;
                for (i, (name, ty)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", name, ty)?;
                }
                write!(f, " }}")
            }
            Ty::Readonly(inner) => write!(f, "readonly {}", inner),
            Ty::Existential { class } => write!(f, "{}", class),
            Ty::Forall {
                bounds,
                constraints,
                body,
            } => {
                let vars = bounds
                    .iter()
                    .map(|v| {
                        let mut s = format!("t{}", v.raw());
                        // Unary binder-style constraints (`T: Num`) attach to the var.
                        let classes = constraints
                            .iter()
                            .filter(|c| c.is_unary_on(*v))
                            .map(|c| c.class.as_str())
                            .collect::<Vec<_>>();
                        if !classes.is_empty() {
                            s.push_str(": ");
                            s.push_str(&classes.join(" + "));
                        }
                        s
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                // Multi-arg / non-binder constraints render as a trailing where.
                let multi: Vec<String> = constraints
                    .iter()
                    .filter(|c| {
                        c.args.len() != 1 || c.primary_var().is_none_or(|v| !bounds.contains(&v))
                    })
                    .map(|c| c.to_string())
                    .collect();
                if multi.is_empty() {
                    write!(f, "forall {}. {}", vars, body)
                } else {
                    write!(f, "forall {}. {} where {}", vars, body, multi.join(", "))
                }
            }
        }
    }
}

fn needs_paren(t: &Ty) -> bool {
    matches!(t, Ty::Fun(_, _) | Ty::Forall { .. })
}

impl fmt::Display for Scheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.bounds.is_empty() {
            write!(f, "{}", self.ty)
        } else {
            let vars = self
                .bounds
                .iter()
                .map(|v| format!("t{}", v.raw()))
                .collect::<Vec<_>>()
                .join(" ");
            write!(f, "forall {}. {}", vars, self.ty)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typechecking::ty::{TyVarId, float, int, list, string};

    #[test]
    fn display_var() {
        assert_eq!(format!("{}", Ty::Var(TyVarId(0))), "t0");
        assert_eq!(format!("{}", Ty::Var(TyVarId(42))), "t42");
    }

    #[test]
    fn display_con() {
        assert_eq!(format!("{}", int()), "int");
        assert_eq!(format!("{}", float()), "float");
        assert_eq!(format!("{}", string()), "string");
        assert_eq!(format!("{}", Ty::Con("Foo".into())), "Foo");
    }

    #[test]
    fn display_fun() {
        let ty = Ty::Fun(Box::new(int()), Box::new(string()));
        assert_eq!(format!("{}", ty), "int -> string");
    }

    #[test]
    fn display_fun_with_nested_fun_adds_parens() {
        // (int -> string) -> bool
        let inner = Ty::Fun(Box::new(int()), Box::new(string()));
        let ty = Ty::Fun(Box::new(inner), Box::new(Ty::Con("bool".into())));
        assert_eq!(format!("{}", ty), "(int -> string) -> bool");
    }

    #[test]
    fn display_app_no_args() {
        let ty = Ty::App(Box::new(Ty::Con("Foo".into())), vec![]);
        assert_eq!(format!("{}", ty), "Foo");
    }

    #[test]
    fn display_app_with_args() {
        let ty = Ty::App(Box::new(Ty::Con("Foo".into())), vec![int(), string()]);
        assert_eq!(format!("{}", ty), "Foo<int, string>");
    }

    #[test]
    fn display_list() {
        assert_eq!(format!("{}", list(int())), "[int]");
    }

    #[test]
    fn display_scheme_mono() {
        let s = Scheme::mono(int());
        assert_eq!(format!("{}", s), "int");
    }

    #[test]
    fn display_scheme_poly() {
        let s = Scheme {
            bounds: vec![TyVarId(0), TyVarId(1)],
            kinds: vec![],
            constraints: vec![],
            assoc_projections: vec![],
            ty: Ty::Fun(Box::new(Ty::Var(TyVarId(0))), Box::new(Ty::Var(TyVarId(1)))),
        };
        assert_eq!(format!("{}", s), "forall t0 t1. t0 -> t1");
    }

    #[test]
    fn format_ty_for_diag_renames_free_vars() {
        use crate::typechecking::subst::Subst;
        let ty = Ty::Fun(
            Box::new(Ty::Var(TyVarId(43))),
            Box::new(Ty::Var(TyVarId(99))),
        );
        let s = format_ty_for_diag(&Subst::empty(), &ty);
        assert_eq!(s, "a -> b");
        assert!(!s.contains('t'), "diagnostics must not show raw tN ids: {s}");
    }

    #[test]
    fn format_ty_for_diag_applies_subst_before_rename() {
        use crate::typechecking::subst::Subst;
        let mut subst = Subst::empty();
        subst.insert(TyVarId(43), int());
        let s = format_ty_for_diag(&subst, &Ty::Var(TyVarId(43)));
        assert_eq!(s, "int");
    }

    // ---- Sum / Constructor Display ----

    #[test]
    fn display_sum_with_no_payloads() {
        let ty = Ty::Sum {
            name: "E".into(),
            variants: vec![
                ("A".into(), EnumVariantPayloadTy::Unit),
                ("B".into(), EnumVariantPayloadTy::Unit),
            ],
        };
        assert_eq!(format!("{}", ty), "enum E { A, B }");
    }

    #[test]
    fn display_sum_with_payloads() {
        let ty = Ty::Sum {
            name: "Option".into(),
            variants: vec![
                ("None".into(), EnumVariantPayloadTy::Unit),
                ("Some".into(), EnumVariantPayloadTy::Tuple(vec![int()])),
            ],
        };
        assert_eq!(format!("{}", ty), "enum Option { None, Some(int) }");
    }

    #[test]
    fn display_sum_with_record_payloads() {
        // enum E { Unit, Rec { x: int, y: string } }
        let ty = Ty::Sum {
            name: "E".into(),
            variants: vec![
                ("Unit".into(), EnumVariantPayloadTy::Unit),
                (
                    "Rec".into(),
                    EnumVariantPayloadTy::Record(vec![("x".into(), int()), ("y".into(), string())]),
                ),
            ],
        };
        assert_eq!(
            format!("{}", ty),
            "enum E { Unit, Rec { x: int, y: string } }"
        );
    }

    #[test]
    fn display_constructor() {
        let sum = Ty::Sum {
            name: "Option".into(),
            variants: vec![
                ("None".into(), EnumVariantPayloadTy::Unit),
                ("Some".into(), EnumVariantPayloadTy::Tuple(vec![int()])),
            ],
        };
        let ctor = Ty::Constructor {
            owner: Box::new(sum),
            tag: 1,
            arity: 1,
        };
        assert_eq!(format!("{}", ctor), "enum Option { None, Some(int) }::v1");
    }
}
