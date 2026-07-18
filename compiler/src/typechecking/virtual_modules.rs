//! Compiler-provided virtual modules (`prelude`, `ffi`, …).
//!
//! These are not `.0s` files on disk. `use` resolves against this
//! registry before falling back to [`crate::manifest::Manifest`] path
//! discovery, and every file gets an implicit prelude import.

use std::collections::HashMap;

/// Canonical module path for Option / Result.
pub const PRELUDE_MODULE: &str = "prelude";

/// Canonical module path for operator / comparison traits.
pub const PRELUDE_OPS_MODULE: &str = "prelude::ops";

/// Canonical module path for FFI callables (`dload` / `declare` / `invoke`).
pub const FFI_MODULE: &str = "ffi";

/// Canonical module path for FFI type-tag constructors (`Int`, `Ptr`, …).
pub const FFI_TYPES_MODULE: &str = "ffi::types";

/// Canonical module path for test helpers (`assert`).
pub const PRELUDE_TEST_MODULE: &str = "prelude::test";

/// Which userland FFI builtin a virtual export names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FfiBuiltin {
    Dload,
    Declare,
    Invoke,
}

impl FfiBuiltin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dload => "dload",
            Self::Declare => "declare",
            Self::Invoke => "invoke",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "dload" => Some(Self::Dload),
            "declare" => Some(Self::Declare),
            "invoke" => Some(Self::Invoke),
            _ => None,
        }
    }
}

/// Prelude/test callables exported from virtual modules (parallel to [`FfiBuiltin`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreludeFn {
    Assert,
}

impl PreludeFn {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Assert => "assert",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "assert" => Some(Self::Assert),
            _ => None,
        }
    }
}

/// One item exported by a virtual module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuiltinExport {
    /// Built-in sum type (`Option`, `Result`). Internal registry key is `name`.
    Enum { name: &'static str },
    /// Built-in typeclass (`Eq`, `Num`, …). Internal key is `name`.
    TypeClass { name: &'static str },
    /// FFI tag constructor (`Int`, `Ptr`, …) → same tags as historical `FFIType::X`.
    FfiTag { variant: &'static str },
    /// Userland FFI callable.
    FfiFn { kind: FfiBuiltin },
    /// Prelude/test callable (`assert`, …).
    Fn { kind: PreludeFn },
}

impl BuiltinExport {
    pub fn short_name(&self) -> &str {
        match self {
            Self::Enum { name } => name,
            Self::TypeClass { name } => name,
            Self::FfiTag { variant } => variant,
            Self::FfiFn { kind } => kind.as_str(),
            Self::Fn { kind } => kind.as_str(),
        }
    }
}

/// Path → exports for compiler virtual modules.
#[derive(Debug, Clone)]
pub struct VirtualModules {
    modules: HashMap<&'static str, Vec<BuiltinExport>>,
}

impl Default for VirtualModules {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualModules {
    pub fn new() -> Self {
        let mut modules: HashMap<&'static str, Vec<BuiltinExport>> = HashMap::new();

        modules.insert(
            PRELUDE_MODULE,
            vec![
                BuiltinExport::Enum {
                    name: common::BUILTIN_OPTION_ENUM,
                },
                BuiltinExport::Enum {
                    name: common::BUILTIN_RESULT_ENUM,
                },
            ],
        );

        modules.insert(
            PRELUDE_OPS_MODULE,
            vec![
                BuiltinExport::TypeClass { name: "Add" },
                BuiltinExport::TypeClass { name: "Sub" },
                BuiltinExport::TypeClass { name: "Mul" },
                BuiltinExport::TypeClass { name: "Div" },
                BuiltinExport::TypeClass { name: "Num" },
                BuiltinExport::TypeClass { name: "Eq" },
                BuiltinExport::TypeClass { name: "Ord" },
                BuiltinExport::TypeClass { name: "Lt" },
                BuiltinExport::TypeClass { name: "Le" },
                BuiltinExport::TypeClass { name: "Gt" },
                BuiltinExport::TypeClass { name: "Ge" },
                BuiltinExport::TypeClass { name: "Show" },
            ],
        );

        modules.insert(
            FFI_MODULE,
            vec![
                BuiltinExport::FfiFn {
                    kind: FfiBuiltin::Dload,
                },
                BuiltinExport::FfiFn {
                    kind: FfiBuiltin::Declare,
                },
                BuiltinExport::FfiFn {
                    kind: FfiBuiltin::Invoke,
                },
            ],
        );

        let ffi_tags: Vec<BuiltinExport> = common::BUILTIN_FFI_TYPE_VARIANTS
            .iter()
            .map(|variant| BuiltinExport::FfiTag { variant })
            .collect();
        modules.insert(FFI_TYPES_MODULE, ffi_tags);

        modules.insert(
            PRELUDE_TEST_MODULE,
            vec![BuiltinExport::Fn {
                kind: PreludeFn::Assert,
            }],
        );

        Self { modules }
    }

    /// True when `module_path` is a known virtual module (`"prelude"`, `"ffi::types"`, …).
    pub fn is_virtual_module(&self, module_path: &str) -> bool {
        self.modules.contains_key(module_path)
    }

    /// Join `use` path segments (+ optional final item that is not `*`) into a module path.
    pub fn module_path_of(path: &[String], name: &str) -> String {
        if name == "*" {
            path.join("::")
        } else if path.is_empty() {
            name.to_string()
        } else {
            format!("{}::{}", path.join("::"), name)
        }
    }

    /// Resolve a concrete `use path::name` (not glob) against virtual modules.
    ///
    /// `path` is the directory segments; `name` is the last segment (item).
    /// For `use prelude::ops::Eq`, path=`["prelude","ops"]`, name=`"Eq"`.
    pub fn resolve_item(&self, path: &[String], name: &str) -> Option<BuiltinExport> {
        if name == "*" {
            return None;
        }
        let module = path.join("::");
        self.modules
            .get(module.as_str())?
            .iter()
            .find(|e| e.short_name() == name)
            .cloned()
    }

    /// Resolve `use module::*` — returns every export of that module.
    pub fn resolve_glob(&self, path: &[String]) -> Option<&[BuiltinExport]> {
        let module = path.join("::");
        self.modules.get(module.as_str()).map(|v| v.as_slice())
    }

    /// True when this `use` targets a virtual module (concrete or glob).
    ///
    /// Used by the pipeline to skip disk discovery.
    pub fn resolves_use(&self, path: &[String], name: &str) -> bool {
        if name == "*" {
            self.resolve_glob(path).is_some()
        } else {
            self.resolve_item(path, name).is_some()
        }
    }

    /// Exports injected into every file (implicit
    /// `use prelude::*; use prelude::ops::*; use prelude::test::*;`).
    pub fn prelude_exports(&self) -> Vec<BuiltinExport> {
        let mut out = Vec::new();
        if let Some(e) = self.modules.get(PRELUDE_MODULE) {
            out.extend(e.iter().cloned());
        }
        if let Some(e) = self.modules.get(PRELUDE_OPS_MODULE) {
            out.extend(e.iter().cloned());
        }
        if let Some(e) = self.modules.get(PRELUDE_TEST_MODULE) {
            out.extend(e.iter().cloned());
        }
        out
    }

    /// Look up a typeclass by qualified path (`prelude::ops::Eq` → `"Eq"`).
    pub fn resolve_typeclass_path(&self, segments: &[&str]) -> Option<&'static str> {
        if segments.len() < 2 {
            return None;
        }
        let (module_segs, name) = segments.split_at(segments.len() - 1);
        let module = module_segs.join("::");
        match self.modules.get(module.as_str())?.iter().find(|e| {
            matches!(e, BuiltinExport::TypeClass { name: n } if n == &name[0])
        })? {
            BuiltinExport::TypeClass { name } => Some(*name),
            _ => None,
        }
    }

    /// Look up an enum by qualified path (`prelude::Option` → `"Option"`).
    pub fn resolve_enum_path(&self, segments: &[&str]) -> Option<&'static str> {
        if segments.len() < 2 {
            return None;
        }
        let (module_segs, name) = segments.split_at(segments.len() - 1);
        let module = module_segs.join("::");
        match self.modules.get(module.as_str())?.iter().find(|e| {
            matches!(e, BuiltinExport::Enum { name: n } if n == &name[0])
        })? {
            BuiltinExport::Enum { name } => Some(*name),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prelude_exports_option_result_and_ops() {
        let vm = VirtualModules::new();
        let exports = vm.prelude_exports();
        assert!(
            exports
                .iter()
                .any(|e| matches!(e, BuiltinExport::Enum { name: "Option" }))
        );
        assert!(
            exports
                .iter()
                .any(|e| matches!(e, BuiltinExport::TypeClass { name: "Eq" }))
        );
        assert!(
            exports.iter().any(|e| matches!(
                e,
                BuiltinExport::Fn {
                    kind: PreludeFn::Assert
                }
            ))
        );
        assert!(
            !exports
                .iter()
                .any(|e| matches!(e, BuiltinExport::FfiFn { .. }))
        );
    }

    #[test]
    fn resolve_concrete_prelude_test_assert() {
        let vm = VirtualModules::new();
        let e = vm
            .resolve_item(&["prelude".into(), "test".into()], "assert")
            .expect("prelude::test::assert");
        assert_eq!(
            e,
            BuiltinExport::Fn {
                kind: PreludeFn::Assert
            }
        );
        assert!(vm.resolves_use(&["prelude".into(), "test".into()], "*"));
    }

    #[test]
    fn ffi_types_glob_lists_tags() {
        let vm = VirtualModules::new();
        let tags = vm
            .resolve_glob(&["ffi".into(), "types".into()])
            .expect("ffi::types");
        assert!(
            tags.iter()
                .any(|e| matches!(e, BuiltinExport::FfiTag { variant: "Int" }))
        );
        assert!(
            tags.iter()
                .any(|e| matches!(e, BuiltinExport::FfiTag { variant: "Ptr" }))
        );
    }

    #[test]
    fn resolve_concrete_ffi_dload() {
        let vm = VirtualModules::new();
        let e = vm
            .resolve_item(&["ffi".into()], "dload")
            .expect("ffi::dload");
        assert_eq!(
            e,
            BuiltinExport::FfiFn {
                kind: FfiBuiltin::Dload
            }
        );
    }

    #[test]
    fn resolves_use_detects_virtual_paths() {
        let vm = VirtualModules::new();
        assert!(vm.resolves_use(&["prelude".into(), "ops".into()], "Eq"));
        assert!(vm.resolves_use(&["ffi".into(), "types".into()], "*"));
        assert!(!vm.resolves_use(&["foo".into()], "sadge"));
    }
}
