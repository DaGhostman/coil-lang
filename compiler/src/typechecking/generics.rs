//! Typeclass and instance registry for userland generics.
//!
//! Stores the shapes of typeclasses (`Num<T>`, `Eq<T>`, …) and
//! registered implementations (`impl Num<int>`, `impl Num<float>`, …).
//! The [`Checker`](super::infer::Checker) owns one `Generics` value and
//! delegates type-class resolution to it.

use super::kind::Kind;
use super::subst::Subst;
use super::ty::Ty;
use super::unify::unify_with;
use std::collections::{HashMap, HashSet};

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
///
/// Superclasses (Phase 5): `typeclass Ordered<T: Equal>` stores
/// `superclasses: ["Equal"]`. Dictionary layout is flattened — subclass
/// methods first, then each superclass’s methods in declaration order
/// (transitively).
///
/// Associated types (Phase 6): `typeclass Collect<C> { type Elem; … }`
/// stores `assoc_types: ["Elem"]`. Method schemes quantify class params
/// first, then assoc types in this order.
#[derive(Debug, Clone)]
pub struct TypeClassDef {
    pub name: String,
    /// Type-parameter names in declaration order, e.g. `["T"]`.
    pub type_params: Vec<String>,
    /// Kinds of each type parameter (parallel to `type_params`).
    /// Empty means every parameter has kind `*`.
    pub param_kinds: Vec<Kind>,
    /// Direct superclass class names from single-parameter bounds
    /// (`typeclass Ord<T: Eq>` → `["Eq"]`). Empty for multi-param classes
    /// and classes without bounds.
    pub superclasses: Vec<String>,
    /// Associated type names in declaration order (Phase 6), e.g. `["Elem"]`.
    pub assoc_types: Vec<String>,
    pub methods: Vec<TypeClassMethodDef>,
}

impl TypeClassDef {
    /// Kind of parameter `i`, defaulting to `*`.
    pub fn kind_at(&self, i: usize) -> Kind {
        self.param_kinds.get(i).cloned().unwrap_or(Kind::Type)
    }

    /// True when parameter `i` is constructor-kinded.
    pub fn is_constructor_kind_at(&self, i: usize) -> bool {
        self.kind_at(i).is_constructor_kind()
    }

    /// Constructor arity of parameter `i`, if it is constructor-kinded.
    pub fn constructor_arity_at(&self, i: usize) -> Option<usize> {
        let kind = self.kind_at(i);
        kind.is_constructor_kind().then(|| kind.arity())
    }

    /// True when `name` is a transitive superclass of this class.
    pub fn has_superclass(&self, name: &str, generics: &Generics) -> bool {
        let mut seen = HashSet::new();
        let mut stack: Vec<&str> = self.superclasses.iter().map(String::as_str).collect();
        while let Some(super_name) = stack.pop() {
            if !seen.insert(super_name.to_string()) {
                continue;
            }
            if super_name == name {
                return true;
            }
            if let Some(super_def) = generics.typeclass(super_name) {
                stack.extend(super_def.superclasses.iter().map(String::as_str));
            }
        }
        false
    }

    /// Flattened dictionary method list: this class’s methods, then each
    /// superclass’s methods in declaration order (DFS, cycle-safe).
    ///
    /// Returns `(owning_class_name, method_def)` pairs. Slot index in the
    /// runtime dict is the position in this list.
    pub fn flattened_methods<'a>(
        &'a self,
        generics: &'a Generics,
    ) -> Vec<(&'a str, &'a TypeClassMethodDef)> {
        let mut out = Vec::new();
        let mut visited = HashSet::new();
        Self::collect_flattened_methods(self, generics, &mut out, &mut visited);
        out
    }

    fn collect_flattened_methods<'a>(
        class: &'a TypeClassDef,
        generics: &'a Generics,
        out: &mut Vec<(&'a str, &'a TypeClassMethodDef)>,
        visited: &mut HashSet<String>,
    ) {
        if !visited.insert(class.name.clone()) {
            return;
        }
        for method in &class.methods {
            out.push((class.name.as_str(), method));
        }
        for super_name in &class.superclasses {
            if let Some(super_def) = generics.typeclass(super_name) {
                Self::collect_flattened_methods(super_def, generics, out, visited);
            }
        }
    }
}

/// One registered typeclass instance (concrete type → implementation mapping).
///
/// E.g. `impl Num<int> { fn add(…) { … } }` is stored as:
/// ```text
/// InstanceDef { class: "Num", args: [int()],
///     method_fqns: { "add" → "Num__int__add" } }
/// ```
///
/// Associated types (Phase 6): `impl Collect<Option<int>> { type Elem = int; … }`
/// stores `assoc_tys: { "Elem" → int() }`.
#[derive(Debug, Clone)]
pub struct InstanceDef {
    /// Class name this instance implements (e.g. `"Num"`).
    pub class: String,
    /// Concrete type arguments (e.g. `[int()]` for `impl Num<int>`).
    pub args: Vec<Ty>,
    /// Method name → fully-qualified codegen name.
    pub method_fqns: HashMap<String, String>,
    /// Associated type name → concrete type (Phase 6).
    pub assoc_tys: HashMap<String, Ty>,
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

    /// Register compiler-provided generic type constructors that are
    /// always in scope.
    pub fn register_builtin_type_ctors(&mut self) {
        self.generic_type_ctors
            .insert(common::BUILTIN_OPTION_ENUM.into(), vec!["T".into()]);
        self.generic_type_ctors.insert(
            common::BUILTIN_RESULT_ENUM.into(),
            vec!["T".into(), "E".into()],
        );
    }

    /// Look up a typeclass by name.
    pub fn typeclass(&self, name: &str) -> Option<&TypeClassDef> {
        self.typeclasses.get(name)
    }

    /// Find an instance for `class` applied to `args` (exact type match).
    pub fn find_instance(&self, class: &str, args: &[Ty]) -> Option<&InstanceDef> {
        self.instances.iter().find(|inst| {
            inst.class == class
                && inst.args.len() == args.len()
                && inst.args.iter().zip(args.iter()).all(|(a, b)| a == b)
        })
    }

    /// Find an instance whose args unify with `args` (e.g. `Option::Some(42)`'s
    /// Constructor type against `impl Collect<Option<int>>`). Does not mutate
    /// any global substitution.
    pub fn find_unifying_instance(&self, class: &str, args: &[Ty]) -> Option<&InstanceDef> {
        self.instances.iter().find(|inst| {
            if inst.class != class || inst.args.len() != args.len() {
                return false;
            }
            let mut local = Subst::empty();
            for (have, need) in inst.args.iter().zip(args.iter()) {
                match unify_with(&local, need, have) {
                    Ok(s) => local = s,
                    Err(_) => return false,
                }
            }
            true
        })
    }

    /// Exact match, then unifying match (Constructor ↔ App/Sum).
    pub fn find_instance_relaxed(&self, class: &str, args: &[Ty]) -> Option<&InstanceDef> {
        self.find_instance(class, args)
            .or_else(|| self.find_unifying_instance(class, args))
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

    /// Associated types required by the class but missing from the instance.
    pub fn missing_assoc_types(
        class_def: &TypeClassDef,
        assoc_tys: &HashMap<String, Ty>,
    ) -> Vec<String> {
        class_def
            .assoc_types
            .iter()
            .filter(|name| !assoc_tys.contains_key(name.as_str()))
            .cloned()
            .collect()
    }

    /// Associated types provided by an instance that the class does not declare.
    pub fn unknown_assoc_types<'a, I>(class_def: &TypeClassDef, assoc_names: I) -> Vec<String>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let known: HashSet<&str> = class_def.assoc_types.iter().map(String::as_str).collect();
        assoc_names
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

    /// Build the compiler-generated FQN for a builtin instance method.
    ///
    /// Convention: `{Class}__{concrete_type_str}__{method}`
    /// e.g. `Num__int__add`, `Ord__float__lt`, `Eq__string__eq`.
    pub fn builtin_instance_fqn(class: &str, ty_str: &str, method: &str) -> String {
        format!("{}__{}__{}", class, ty_str, method)
    }

    /// Register the built-in typeclasses and their builtin instances.
    fn register_builtins(&mut self) {
        use super::ty::{boolean, float, int, string, unit};

        self.register_builtin_type_ctors();

        // ---- Num ----
        self.typeclasses.insert(
            "Num".into(),
            TypeClassDef {
                name: "Num".into(),
                type_params: vec!["T".into()],
                param_kinds: vec![Kind::Type],
                superclasses: vec![],
                assoc_types: vec![],
                methods: vec![
                    TypeClassMethodDef {
                        name: "add".into(),
                        has_default: false,
                    },
                    TypeClassMethodDef {
                        name: "sub".into(),
                        has_default: false,
                    },
                    TypeClassMethodDef {
                        name: "mul".into(),
                        has_default: false,
                    },
                    TypeClassMethodDef {
                        name: "div".into(),
                        has_default: false,
                    },
                ],
            },
        );

        // ---- Ord ----
        // Builtin Ord does not declare Eq as a superclass (dict layouts for
        // existing Num/Ord/Eq callers stay unchanged). User-defined classes
        // use `typeclass Ordered<T: Equal>` for the superclass path.
        self.typeclasses.insert(
            "Ord".into(),
            TypeClassDef {
                name: "Ord".into(),
                type_params: vec!["T".into()],
                param_kinds: vec![Kind::Type],
                superclasses: vec![],
                assoc_types: vec![],
                methods: vec![
                    TypeClassMethodDef {
                        name: "lt".into(),
                        has_default: false,
                    },
                    TypeClassMethodDef {
                        name: "le".into(),
                        has_default: false,
                    },
                    TypeClassMethodDef {
                        name: "gt".into(),
                        has_default: false,
                    },
                    TypeClassMethodDef {
                        name: "ge".into(),
                        has_default: false,
                    },
                ],
            },
        );

        // ---- Eq ----
        self.typeclasses.insert(
            "Eq".into(),
            TypeClassDef {
                name: "Eq".into(),
                type_params: vec!["T".into()],
                param_kinds: vec![Kind::Type],
                superclasses: vec![],
                assoc_types: vec![],
                methods: vec![
                    TypeClassMethodDef {
                        name: "eq".into(),
                        has_default: false,
                    },
                    TypeClassMethodDef {
                        name: "ne".into(),
                        has_default: false,
                    },
                ],
            },
        );

        // ---- Show ----
        self.typeclasses.insert(
            "Show".into(),
            TypeClassDef {
                name: "Show".into(),
                type_params: vec!["T".into()],
                param_kinds: vec![Kind::Type],
                superclasses: vec![],
                assoc_types: vec![],
                methods: vec![TypeClassMethodDef {
                    name: "show".into(),
                    has_default: false,
                }],
            },
        );

        // Helper: build method_fqns map for an instance.
        let make_fqns = |class: &str, ty_str: &str, methods: &[&str]| -> HashMap<String, String> {
            methods
                .iter()
                .map(|m| (m.to_string(), Self::builtin_instance_fqn(class, ty_str, m)))
                .collect()
        };

        // ---- builtin instances: int ----
        self.instances.push(InstanceDef {
            class: "Num".into(),
            args: vec![int()],
            method_fqns: make_fqns("Num", "int", &["add", "sub", "mul", "div"]),
            assoc_tys: HashMap::new(),
        });
        self.instances.push(InstanceDef {
            class: "Ord".into(),
            args: vec![int()],
            method_fqns: make_fqns("Ord", "int", &["lt", "le", "gt", "ge"]),
            assoc_tys: HashMap::new(),
        });
        self.instances.push(InstanceDef {
            class: "Eq".into(),
            args: vec![int()],
            method_fqns: make_fqns("Eq", "int", &["eq", "ne"]),
            assoc_tys: HashMap::new(),
        });
        self.instances.push(InstanceDef {
            class: "Show".into(),
            args: vec![int()],
            method_fqns: make_fqns("Show", "int", &["show"]),
            assoc_tys: HashMap::new(),
        });

        // ---- builtin instances: float ----
        self.instances.push(InstanceDef {
            class: "Num".into(),
            args: vec![float()],
            method_fqns: make_fqns("Num", "float", &["add", "sub", "mul", "div"]),
            assoc_tys: HashMap::new(),
        });
        self.instances.push(InstanceDef {
            class: "Ord".into(),
            args: vec![float()],
            method_fqns: make_fqns("Ord", "float", &["lt", "le", "gt", "ge"]),
            assoc_tys: HashMap::new(),
        });
        self.instances.push(InstanceDef {
            class: "Eq".into(),
            args: vec![float()],
            method_fqns: make_fqns("Eq", "float", &["eq", "ne"]),
            assoc_tys: HashMap::new(),
        });
        self.instances.push(InstanceDef {
            class: "Show".into(),
            args: vec![float()],
            method_fqns: make_fqns("Show", "float", &["show"]),
            assoc_tys: HashMap::new(),
        });

        // ---- string: Eq + Show ----
        self.instances.push(InstanceDef {
            class: "Eq".into(),
            args: vec![string()],
            method_fqns: make_fqns("Eq", "string", &["eq", "ne"]),
            assoc_tys: HashMap::new(),
        });
        self.instances.push(InstanceDef {
            class: "Show".into(),
            args: vec![string()],
            method_fqns: make_fqns("Show", "string", &["show"]),
            assoc_tys: HashMap::new(),
        });

        // ---- bool: Eq + Show ----
        self.instances.push(InstanceDef {
            class: "Eq".into(),
            args: vec![boolean()],
            method_fqns: make_fqns("Eq", "bool", &["eq", "ne"]),
            assoc_tys: HashMap::new(),
        });
        self.instances.push(InstanceDef {
            class: "Show".into(),
            args: vec![boolean()],
            method_fqns: make_fqns("Show", "bool", &["show"]),
            assoc_tys: HashMap::new(),
        });

        // ---- unit: Show ----
        self.instances.push(InstanceDef {
            class: "Show".into(),
            args: vec![unit()],
            method_fqns: make_fqns("Show", "unit", &["show"]),
            assoc_tys: HashMap::new(),
        });
    }

    /// Whether `class` is satisfied for `ty` (builtin or registered instance).
    pub fn has_instance(&self, class: &str, ty: &Ty) -> bool {
        self.find_instance(class, std::slice::from_ref(ty))
            .is_some()
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
            param_kinds: vec![Kind::Type],
            superclasses: vec![],
            assoc_types: vec![],
            methods: vec![method("add", false), method("zero", true)],
        }
    }

    #[test]
    fn flattened_methods_subclass_then_superclass() {
        let mut generics = Generics::new();
        generics.typeclasses.insert(
            "Equal".into(),
            TypeClassDef {
                name: "Equal".into(),
                type_params: vec!["T".into()],
                param_kinds: vec![Kind::Type],
                superclasses: vec![],
                assoc_types: vec![],
                methods: vec![method("eq_val", false)],
            },
        );
        generics.typeclasses.insert(
            "Ordered".into(),
            TypeClassDef {
                name: "Ordered".into(),
                type_params: vec!["T".into()],
                param_kinds: vec![Kind::Type],
                superclasses: vec!["Equal".into()],
                assoc_types: vec![],
                methods: vec![method("lt_val", false)],
            },
        );
        let ordered = generics.typeclass("Ordered").unwrap();
        let flat: Vec<_> = ordered
            .flattened_methods(&generics)
            .into_iter()
            .map(|(c, m)| (c.to_string(), m.name.clone()))
            .collect();
        assert_eq!(
            flat,
            vec![
                ("Ordered".into(), "lt_val".into()),
                ("Equal".into(), "eq_val".into()),
            ]
        );
        assert!(ordered.has_superclass("Equal", &generics));
        assert!(!ordered.has_superclass("Num", &generics));
    }

    #[test]
    fn has_overlapping_instance_detects_exact_class_and_args() {
        let mut generics = Generics::new();
        generics.instances.clear();
        generics.instances.push(InstanceDef {
            class: "Num".to_string(),
            args: vec![int()],
            method_fqns: HashMap::new(),
            assoc_tys: HashMap::new(),
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
        let unknown = Generics::unknown_instance_methods(&class_def(), ["add", "foo"].into_iter());

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

    #[test]
    fn find_instance_matches_multi_arg_class() {
        let mut generics = Generics::new();
        generics.instances.push(InstanceDef {
            class: "Convert".to_string(),
            args: vec![int(), int()],
            method_fqns: HashMap::from([(
                "cast".to_string(),
                "Convert__int_int__cast".to_string(),
            )]),
            assoc_tys: HashMap::new(),
        });

        assert!(generics.find_instance("Convert", &[int(), int()]).is_some());
        assert!(
            generics
                .find_instance("Convert", &[int(), float()])
                .is_none()
        );
        assert!(generics.find_instance("Convert", &[int()]).is_none());
    }
}
