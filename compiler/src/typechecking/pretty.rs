//! Pretty-printing for `Ty` (and `Scheme`, for diagnostics).
//!
//! Used by tests today and by diagnostic messages in Phase 8. Kept simple
//! on purpose — we can add parenthesisation / precedence rules later if
//! error messages need them.

use std::fmt;

use super::ty::{Scheme, Ty};

/// Format a `Ty` the way a user would read it:
///
/// - `Var(t0)`           → `t0`
/// - `Con("int")`        → `int`
/// - `Fun(a, b)`         → `a -> b` (with parens around nested `Fun`s)
/// - `App(Foo, [...])`   → `Foo<args>` (omits `<>` when arg list is empty)
/// - `List(t)`           → `[t]`
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
        }
    }
}

fn needs_paren(t: &Ty) -> bool {
    matches!(t, Ty::Fun(_, _))
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
    use crate::typechecking::ty::{float, int, list, string, TyVarId};

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
        let ty = Ty::App(
            Box::new(Ty::Con("Foo".into())),
            vec![int(), string()],
        );
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
            ty: Ty::Fun(Box::new(Ty::Var(TyVarId(0))), Box::new(Ty::Var(TyVarId(1)))),
        };
        assert_eq!(format!("{}", s), "forall t0 t1. t0 -> t1");
    }
}