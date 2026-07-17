//! Typeclass and instance registry for userland generics.
//!
//! Stores the shapes of typeclasses (`Num<T>`, `Eq<T>`, …) and
//! registered implementations (`impl Num<int>`, `impl Num<float>`, …).
//! The [`Checker`](super::infer::Checker) owns one `Generics` value and
//! delegates type-class resolution to it.

use std::collections::{HashMap, HashSet};
use super::ty::Ty;

// ──────────────────────────────────────────────────────────────────────────────
//  Public data types
// ──────────────────────────────────────────────────────────────────────────────

/// One method slot in a typeclass declaration.
#[derive(Debug, Clone)]
pub struct TypeClassMethodDef {
    /// Short method name (e.g. `"add"`).
    pub name: String,
    /// `true` when the class body contains a default implementation
    /// (a `Function` with a non-empty body).
    pub has_default: bool,
}

/// The shape of a typeclass: its name, type parameters, and methods.
///
/// E.g. `typeclass Num<T> { fn add(T a, T b) -> T; fn sub(…) -> T; }`
/// is stored as:
/// ```text
/// TypeClassDef { name: "Num", type_params: ["T"],
///     methods: [TypeClassMethodDef { name: "add", has_default: false }, …] }
/// ```
#[derive(Debug, Clone)]
pub struct TypeClassDef {
    pub name: String,
    /// Type-parameter names in declaration order, e.g. `["T"]`.
    pub type_params: Vec<String>,
    pub methods: Vec<TypeClassMethodDef>,
}

/// One registered typeclass instance (concrete type → implementation mapping).
///
/// E.g. `impl Num<int> { fn add(…) { … } }` is stored as:
/// ```text
/// InstanceDef { class: "Num", args: [int()],
///     method_fqns: { "add" → "Num__int__add" } }
/// ```
#[derive(Debug, Clone)]
pub struct InstanceDef {
    /// Class name this instance implements (e.g. `"Num"`).
    pub class: String,
    /// Concrete type arguments (e.g. `[int()]` for `impl Num<int>`).
    pub args: Vec<Ty>,
    /// Method name → fully-qualified codegen name.
    pub method_fqns: HashMap<String, String>,
}

// ──────────────────────────────────────────────────────────────────────────────
//  Generics registry
// ──────────────────────────────────────────────────────────────────────────────

/// Registry of typeclasses and their instances, owned by [`Checker`].
#[derive(Debug, Default)]
pub struct Generics {
    /// All declared typeclasses.
    pub typeclasses: HashMap<String, TypeClassDef>,
    /// All registered instances (in declaration order).
    pub instances: Vec<InstanceDef>,
    /// Generic type constructors: enum/class/alias name → type param names.
    pub generic_type_ctors: HashMap<String, Vec<String>>,
    /// Generic function names (have at least one type param).
    pub generic_fns: std::collections::HashSet<String>,
}

impl Generics {
    pub fn new() -> Self {
        let mut g = Self::default();
        g.register_builtins();
        g
    }

    /// Look up a typeclass by name.
    pub fn typeclass(&self, name: &str) -> Option<&TypeClassDef> {
        self.typeclasses.get(name)
    }

    /// Find an instance for `class` applied to `args` (exact type match).
    pub fn find_instance(&self, class: &str, args: &[Ty]) -> Option<&InstanceDef> {
        self.instances.iter().find(|inst| {
            inst.class == class && inst.args.len() == args.len()
                && inst.args.iter().zip(args.iter()).all(|(a, b)| a == b)
        })
    }

    /// True when an identical class + concrete-argument instance already exists.
    pub fn has_overlapping_instance(&self, class: &str, args: &[Ty]) -> bool {
        self.find_instance(class, args).is_some()
    }

    /// Required class methods omitted by an instance.
    pub fn missing_required_methods(
        class_def: &TypeClassDef,
        method_fqns: &HashMap<String, String>,
    ) -> Vec<String> {
        class_def
            .methods
            .iter()
            .filter(|method| !method.has_default && !method_fqns.contains_key(&method.name))
            .map(|method| method.name.clone())
            .collect()
    }

    /// Methods provided by an instance that are not declared by its class.
    pub fn unknown_instance_methods<'a, I>(class_def: &TypeClassDef, method_names: I) -> Vec<String>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let known: HashSet<&str> = class_def
            .methods
            .iter()
            .map(|method| method.name.as_str())
            .collect();
        method_names
            .into_iter()
            .filter(|name| !known.contains(*name))
            .map(str::to_string)
            .collect()
    }

    /// FQN used for default typeclass methods. The class default lives outside
    /// any concrete instance, so its slot is keyed as `{Class}__default__{method}`.
    pub fn default_method_fqn(class: &str, method: &str) -> String {
        format!("{}__default__{}", class, method)
    }

    /// Populate omitted defaulted methods so every satisfiable class method has
    /// a dict slot FQN after instance checking.
    pub fn fill_default_method_fqns(
        class_def: &TypeClassDef,
        method_fqns: &mut HashMap<String, String>,
    ) {
        for method in &class_def.methods {
            if method.has_default {
                method_fqns
                    .entry(method.name.clone())
                    .or_insert_with(|| Self::default_method_fqn(&class_def.name, &method.name));
            }
        }
    }

    /// Register the built-in typeclasses and their builtin instances.
    fn register_builtins(&mut self) {
        use super::ty::{int, float, string, boolean};

        // ---- Num ----
        self.typeclasses.insert("Num".into(), TypeClassDef {
            name: "Num".into(),
            type_params: vec!["T".into()],
            methods: vec![
                TypeClassMethodDef { name: "add".into(), has_default: false },
                TypeClassMethodDef { name: "sub".into(), has_default: false },
                TypeClassMethodDef { name: "mul".into(), has_default: false },
                TypeClassMethodDef { name: "div".into(), has_default: false },
            ],
        });

        // ---- Ord ----
        self.typeclasses.insert("Ord".into(), TypeClassDef {
            name: "Ord".into(),
            type_params: vec!["T".into()],
            methods: vec![
                TypeClassMethodDef { name: "lt".into(), has_default: false },
                TypeClassMethodDef { name: "le".into(), has_default: false },
                TypeClassMethodDef { name: "gt".into(), has_default: false },
                TypeClassMethodDef { name: "ge".into(), has_default: false },
            ],
        });

        // ---- Eq ----
        self.typeclasses.insert("Eq".into(), TypeClassDef {
            name: "Eq".into(),
            type_params: vec!["T".into()],
            methods: vec![
                TypeClassMethodDef { name: "eq".into(), has_default: false },
                TypeClassMethodDef { name: "ne".into(), has_default: false },
            ],
        });

        // ---- Show ----
        self.typeclasses.insert("Show".into(), TypeClassDef {
            name: "Show".into(),
            type_params: vec!["T".into()],
            methods: vec![
                TypeClassMethodDef { name: "show".into(), has_default: false },
            ],
        });

        // ---- builtin instances ----
        for ty in [int(), float()] {
            self.instances.push(InstanceDef {
                class: "Num".into(),
                args: vec![ty.clone()],
                method_fqns: HashMap::new(),
            });
            self.instances.push(InstanceDef {
                class: "Ord".into(),
                args: vec![ty.clone()],
                method_fqns: HashMap::new(),
            });
            self.instances.push(InstanceDef {
                class: "Eq".into(),
                args: vec![ty.clone()],
                method_fqns: HashMap::new(),
            });
            self.instances.push(InstanceDef {
                class: "Show".into(),
                args: vec![ty],
                method_fqns: HashMap::new(),
            });
        }
        // string: Eq + Show
        for class in ["Eq", "Show"] {
            self.instances.push(InstanceDef {
                class: class.into(),
                args: vec![string()],
                method_fqns: HashMap::new(),
            });
        }
        // bool: Eq
        self.instances.push(InstanceDef {
            class: "Eq".into(),
            args: vec![boolean()],
            method_fqns: HashMap::new(),
        });
    }

    /// Whether `class` is satisfied for `ty` (builtin or registered instance).
    pub fn has_instance(&self, class: &str, ty: &Ty) -> bool {
        self.find_instance(class, std::slice::from_ref(ty)).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typechecking::ty::{float, int};

    fn method(name: &str, has_default: bool) -> TypeClassMethodDef {
        TypeClassMethodDef {
            name: name.to_string(),
            has_default,
        }
    }

    fn class_def() -> TypeClassDef {
        TypeClassDef {
            name: "Num".to_string(),
            type_params: vec!["T".to_string()],
            methods: vec![method("add", false), method("zero", true)],
        }
    }

    #[test]
    fn has_overlapping_instance_detects_exact_class_and_args() {
        let mut generics = Generics::new();
        generics.instances.clear();
        generics.instances.push(InstanceDef {
            class: "Num".to_string(),
            args: vec![int()],
            method_fqns: HashMap::new(),
        });

        assert!(generics.has_overlapping_instance("Num", &[int()]));
        assert!(!generics.has_overlapping_instance("Num", &[float()]));
        assert!(!generics.has_overlapping_instance("Show", &[int()]));
    }

    #[test]
    fn missing_required_methods_ignores_default_methods() {
        let mut method_fqns = HashMap::new();
        method_fqns.insert("zero".to_string(), "Num__int__zero".to_string());

        assert_eq!(
            Generics::missing_required_methods(&class_def(), &method_fqns),
            vec!["add".to_string()]
        );
    }

    #[test]
    fn unknown_instance_methods_reports_methods_not_in_class() {
        let unknown =
            Generics::unknown_instance_methods(&class_def(), ["add", "foo"].into_iter());

        assert_eq!(unknown, vec!["foo".to_string()]);
    }

    #[test]
    fn fill_default_method_fqns_registers_omitted_defaults() {
        let mut method_fqns = HashMap::new();
        method_fqns.insert("add".to_string(), "Num__int__add".to_string());

        Generics::fill_default_method_fqns(&class_def(), &mut method_fqns);

        assert_eq!(method_fqns.get("add").unwrap(), "Num__int__add");
        assert_eq!(method_fqns.get("zero").unwrap(), "Num__default__zero");
    }
}
