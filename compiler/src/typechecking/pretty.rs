//! Pretty-printing for [`Ty`] and [`Scheme`] (diagnostics and tests).

use std::fmt;

use super::ty::{ArrayLength, EnumVariantPayloadTy, Scheme, Ty};

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
