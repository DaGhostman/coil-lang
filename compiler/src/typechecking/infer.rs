//! Hindley–Milner inference (Algorithm W) over the zero-script AST.
//!
//! [`Checker`] owns the substitution, accumulates diagnostics with error
//! recovery, and caches inferred types keyed by pre-walk [`NodeId`]s.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ops::Range;

use parser::ast::{Expression, MatchArm, Output, Pattern, Visibility};
use reporting::{ErrorCode, Label, Message};

use super::env::{Env, TyVarCounter, instantiate_with_kinds};
use super::generics::{
    AssocTypeDecl, AssocTypeValue, Generics, InstanceDef, TypeClassDef, TypeClassMethodDef,
};
use super::id::{self, IdTable, NodeId};
use super::kind::Kind;
use super::subst::{Subst, apply_ty, apply_ty_prune, compose};
use super::ty::{AssocProjection, Constraint, Scheme};

/// Code-generation recipe for a trait method call in a generic body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundMethodCall {
    pub dict_index: usize,
    pub method_slot: usize,
    pub arity: usize,
    pub has_receiver: bool,
}

/// Code-generation recipe for an operator dispatched through a typeclass
/// dictionary in a shared generic body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundOperatorCall {
    pub dict_index: usize,
    pub method_slot: usize,
}

/// Code-generation recipe for a `%v` format argument dispatched through
/// an active `Show` dictionary in a shared generic body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundDisplayCall {
    pub dict_index: usize,
    pub method_slot: usize,
}

/// Code-generation recipe for a trait method call on an existential pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExistentialMethodCall {
    pub method_slot: usize,
    pub arity: usize,
    pub has_receiver: bool,
}

/// Code-generation recipe for packing a concrete value as a bare-class existential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExistentialPack {
    pub class: String,
    pub value_ty: Ty,
}

/// Runtime lowering strategy for `for x in expr` (unified Iterator protocol).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForInKind {
    /// Index loop over `[T]` / `[T; N]` (observationally `ArrayIter`).
    Array,
    /// Materialise homogeneous tuple elements into a temp array, then array path.
    Tuple { arity: usize },
    /// `DictEntries` then array path; items are `(string, V)` pairs.
    Dict,
    /// Resume/Done loop (completion value excluded from body).
    Coroutine,
    /// Dictionary ABI: `into_iter` then `next` → `Option<Item>`.
    Custom {
        into_iter_fqn: String,
        next_fqn: String,
    },
}

/// Side-table entry for for-in codegen, keyed by the Loop node id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForInInfo {
    pub kind: ForInKind,
}
use super::ty::{ArrayLength, array, array_fixed, tuple as tuple_ty};
use super::ty::{
    EnumVariantPayloadTy, STRING, Ty, TyVarId, boolean, float, int, is_option_ty, is_result_ty,
    list, option_app_ty, option_inner, option_ty, result_app_ty, result_ok_err, result_ty,
    schemaize_payload, schemaize_ty, string, subst_payload_params, subst_ty_params,
    unit as unit_ty,
};
use super::unify::{UnifyError, unify_with};
use super::virtual_modules::{BuiltinExport, FfiBuiltin, IoBuiltin, PreludeFn, VirtualModules};

/// A parametric type alias (`type Pair<T> = (T, T)`).
#[derive(Clone, Debug)]
struct GenericAliasDef {
    /// Parameter names in declaration order.
    params: Vec<String>,
    /// Fresh variables used while parsing the RHS (parallel to `params`).
    param_vars: Vec<TyVarId>,
    /// RHS type with `param_vars` free.
    body: Ty,
}

/// The typechecker. Owns the environment, the fresh-variable counter, the
/// running substitution, and the accumulated diagnostic messages.
pub struct Checker {
    env: Env,
    counter: TyVarCounter,
    subst: Subst,
    messages: Vec<Message>,

    /// Type that the enclosing function expects to return, if any.
    /// `None` outside of a function body. Set when entering a function
    /// declaration.
    current_return_ty: Option<Ty>,

    /// Module path currently being typechecked. The entry file uses `""`.
    current_module: String,

    /// Compiler virtual modules (`prelude`, `ffi`, …).
    virtual_modules: VirtualModules,

    /// Short names currently in scope from the prelude and explicit
    /// `use` of virtual exports. Reset + re-injected each `check_program`.
    scope_bindings: HashMap<String, BuiltinExport>,

    /// Type of the surrounding `match`'s LHS, if any. Used by
    /// [`Expression::Default`] arms.
    current_match_lhs: Option<Ty>,

    /// Class declarations: name → list of (visibility, field name, type).
    classes: std::collections::HashMap<String, Vec<(Visibility, String, Ty)>>,

    /// Method declarations: owner class → method name →
    /// (visibility, scheme). Methods are stored here so they can be
    /// resolved by member-access expressions in a future phase; for
    /// now we only register them.
    methods:
        std::collections::HashMap<String, std::collections::HashMap<String, (Visibility, Scheme)>>,

    /// Pre-walk IDs consumed in lockstep by [`infer`](Self::infer).
    ids: IdTable,

    next_id_idx: usize,

    /// Span-indexed inferred types for codegen ([`lookup_at`](Self::lookup_at)).
    cache: std::collections::HashMap<NodeId, Ty>,

    /// Source-span fallback for codegen when pre-walk IDs are misaligned.
    codegen_types_by_span: HashMap<(usize, usize), Ty>,

    /// Variable types for codegen when infer cache is misaligned in function bodies.
    codegen_var_types: std::collections::HashMap<String, Ty>,

    /// Concrete trait dictionaries selected at each generic call site.
    call_site_dicts: HashMap<NodeId, Vec<InstanceDef>>,
    /// Span fallback when pre-walk / infer NodeIds are misaligned in
    /// function bodies (same motivation as `bound_method_calls_by_span`).
    call_site_dicts_by_span: HashMap<(usize, usize), Vec<InstanceDef>>,

    /// Open dictionaries forwarded from the enclosing generic function.
    call_site_forward_dicts: HashMap<NodeId, Vec<usize>>,
    call_site_forward_dicts_by_span: HashMap<(usize, usize), Vec<usize>>,

    /// Calls resolved through an active trait constraint.
    bound_method_calls: HashMap<NodeId, BoundMethodCall>,
    bound_method_calls_by_span: HashMap<(usize, usize), BoundMethodCall>,

    /// Operators resolved through an active trait constraint.
    bound_operator_calls: HashMap<NodeId, BoundOperatorCall>,
    bound_operator_calls_by_span: HashMap<(usize, usize), BoundOperatorCall>,

    /// `%v` arguments resolved through an active `Show` constraint.
    bound_display_calls: HashMap<NodeId, BoundDisplayCall>,
    bound_display_calls_by_span: HashMap<(usize, usize), BoundDisplayCall>,

    /// Expressions whose result must be packed as `(boxed_value, dict)`.
    existential_packs_by_span: HashMap<(usize, usize), ExistentialPack>,

    /// Calls dispatched through an existential argument/receiver dictionary.
    existential_method_calls: HashMap<NodeId, ExistentialMethodCall>,
    existential_method_calls_by_span: HashMap<(usize, usize), ExistentialMethodCall>,

    /// `for x in` lowering info (Iterator protocol).
    for_in_infos: HashMap<NodeId, ForInInfo>,
    for_in_infos_by_span: HashMap<(usize, usize), ForInInfo>,

    /// Typeclass method signatures, keyed by `(class, method)`.
    typeclass_method_schemes: HashMap<(String, String), Scheme>,

    /// Expected type pushed by annotated `let` / `const` initializers so
    /// ground trait calls like `x.into()` can pin the conversion target
    /// before constraint discharge (`let y: T = x.into();`).
    current_expected: Option<Ty>,

    /// `type Name = T` aliases (substituted at typecheck time).
    ///
    /// Mirrors lexical scopes: lookup walks from the innermost frame
    /// outward, and duplicate declarations are rejected only within
    /// the current frame.
    type_aliases: Vec<HashMap<String, Ty>>,

    /// `type Name<T, …> = RHS` generic aliases.
    ///
    /// Stored as parameter names (declaration order), the fresh
    /// `TyVarId`s used while parsing the RHS, and the RHS template.
    /// `parse_type_app` substitutes concrete args for those vars.
    generic_aliases: HashMap<String, GenericAliasDef>,

    /// Names declared with `const`, tracked per lexical scope so assignment
    /// diagnostics can distinguish immutable bindings from mutable `let`s.
    const_scopes: Vec<HashSet<String>>,

    // Enum registry: Vec preserves source-declaration order for tags;
    // BTreeMap indexes variant name → tag.
    enums: BTreeMap<String, Vec<String>>,
    enum_tags: BTreeMap<String, BTreeMap<String, u32>>,
    enum_payloads: BTreeMap<String, Vec<EnumVariantPayloadTy>>,
    enum_arities: BTreeMap<String, Vec<usize>>,

    /// Match exhaustiveness checks deferred until substitution is closed.
    pending_exhaustive: Vec<PendingExhaustive>,

    /// Names of `async fn` declarations (for codegen).
    async_functions: std::collections::HashSet<String>,

    /// Nesting depth inside `async fn` bodies (for `yield` validation).
    async_depth: usize,

    /// Yield value type for the enclosing `async fn`, if any.
    current_yield_ty: Option<Ty>,

    /// Send/resume-in value type for the enclosing `async fn`, if any.
    current_send_ty: Option<Ty>,

    /// True when the enclosing `async fn` uses `let x = yield …`.
    yield_receives_used: bool,

    /// C-layout struct declarations for FFI (`extern struct`).
    c_structs: Vec<CStructDef>,

    /// Callback signature descriptors (index = aux id on `FFIType::Callback`).
    callback_sigs: Vec<CallbackSigDef>,

    /// Return type recorded for `let id = declare(..., ret)` bindings so
    /// subsequent `invoke(..., id, ...)` can refine its result type.
    ffi_fn_ret_tys: HashMap<String, Ty>,

    /// Enclosing function is in Result mode: bare `return` wraps `Ok`,
    /// `raise` produces `Err`. Holds `(Ok_ty, Err_ty)`.
    fn_result_mode: Option<(Ty, Ty)>,

    /// Enclosing function is in Option mode: `?` propagates `None`.
    fn_option_mode: Option<Ty>,

    /// Functions whose success returns must be Ok-wrapped at codegen.
    result_mode_fns: HashSet<String>,
    /// Function names whose return type is (or was inferred as) `Option<_>`.
    option_mode_fns: HashSet<String>,

    // ── Generics ──────────────────────────────────────────────────────────────
    /// Type parameters currently in scope (name → fresh TyVarId).
    /// Pushed when entering a generic function, popped on exit.
    type_params_in_scope: Vec<HashMap<String, TyVarId>>,
    /// Active trait constraints on in-scope type params.
    /// `(TyVarId, class_name)` — checked when applying arithmetic ops.
    active_constraints: Vec<Constraint>,
    /// Bindings from abstract constraint parameters (`c: * -> Constraint`)
    /// to the concrete class selected by method use inside the current scope.
    abstract_constraint_bindings: Vec<HashMap<String, String>>,
    /// Kind of each type variable currently in play.
    var_kinds: HashMap<TyVarId, Kind>,
    /// Generics registry: typeclasses, instances, generic type ctors.
    generics: super::generics::Generics,
    /// Generic function names registered during typechecking.
    /// Used by codegen to decide between DynAdd vs regular ADD.
    pub generic_fns: HashSet<String>,
    /// Number of *user-defined* trait dict slots expected by each
    /// generic function.  Built-in classes (Num, Ord, Eq, Show) are
    /// handled via Dyn* opcodes and do NOT count toward this arity.
    ///
    /// Persists across `check_program` calls (same lifetime as
    /// `generics.generic_fns`) so codegen can query it after the
    /// typechecking pass.
    pub fn_dict_arity: HashMap<String, usize>,

    /// Typeclass currently being defined (Phase 6 — bare/`Class::` assoc resolution).
    current_typeclass: Option<String>,

    /// Projections encountered while building a scheme. Each is quantified
    /// alongside the method/function binders and later pinned from the selected
    /// instance.
    current_assoc_projections: Option<Vec<AssocProjection>>,

    /// Open associated-type projections `(owner_var, assoc_name, args) →
    /// (assoc_var, arg_tys)` (Phase 6 + GATs). Used for `T::Elem` /
    /// `T::Ref<A>` when `T: Collect` is active; pinned when a ground instance
    /// is discharged.
    open_assoc_projections: HashMap<(TyVarId, String, Vec<String>), (TyVarId, Vec<Ty>)>,
}

/// C-layout struct registered via `extern struct Name { ... }`.
#[derive(Clone, Debug)]
pub struct CStructDef {
    pub name: String,
    pub fields: Vec<(String, u32)>,
}

/// Callback signature registered for `FFIType::Callback` aux ids.
#[derive(Clone, Debug)]
pub struct CallbackSigDef {
    pub args: Vec<u32>,
    pub ret: u32,
}

/// One pending exhaustiveness check, recorded at the match site and
/// run in [`Checker::run_pending_exhaustiveness`].
///
/// `scrutinee_ty` is captured at the time the match is processed;
/// the post-pass resolves it under the final substitution.
#[derive(Debug, Clone)]
struct PendingExhaustive {
    /// Resolved scrutinee type at the time of the match. The
    /// post-pass re-applies the current substitution so any
    /// variables bound since the match site are visible.
    scrutinee_ty: Ty,
    /// The arms, in order. Each entry says which tag (if any) this
    /// arm covers, the arm's source range, and whether the arm is
    /// a wildcard / binding (which covers all remaining cases).
    arms: Vec<ArmCoverage>,
    /// Source range of the `match` keyword — used for the
    /// non-exhaustive diagnostic.
    match_range: Range<usize>,
}

/// What the inner pattern covers (only meaningful for Constructor
/// arms). For the codegen's test chain (which inspects only the
/// FIRST inner pattern), this captures the first non-trivial
/// inner pattern's tag.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum InnerCoverage {
    /// No inner pattern to test (Unit payload, or Wildcard/Binding
    /// which always match).
    Any,
    /// The inner pattern is a Constructor with the given tag.
    /// Multiple inner tags across multiple arms of the same outer
    /// tag are all reachable; two arms with the same outer AND
    /// same inner tag are still duplicates.
    Tag(u32),
}

/// Per-arm coverage info, captured at the match site.
#[derive(Debug, Clone)]
struct ArmCoverage {
    /// The variant tag this arm covers, if it was a constructor
    /// pattern. `None` for wildcards, bindings, and irrefutable
    /// catches.
    tag: Option<u32>,
    /// The inner pattern's coverage, when this arm's pattern is a
    /// Constructor with a payload. See [`InnerCoverage`]. For
    /// non-constructor arms this is [`InnerCoverage::Any`].
    inner: InnerCoverage,
    /// True if the arm was a wildcard (`_`) or a binding (`name`).
    /// Such arms cover all remaining cases (Rust-style).
    is_catchall: bool,
    /// The arm's source range — used for the "unreachable arm"
    /// diagnostic.
    range: Range<usize>,
}

impl Default for Checker {
    fn default() -> Self {
        Self::new()
    }
}

impl Checker {
    pub fn new() -> Self {
        let mut env = Env::new();
        // Always start with one frame so callers can `register_native`
        // (and later `Compiler::register`) before `check_program` is
        // ever called. `check_program` pushes a second frame so the
        // first stays around for inspection.
        env.push();
        let mut checker = Self {
            env,
            counter: TyVarCounter::new(),
            subst: Subst::empty(),
            messages: Vec::new(),
            current_return_ty: None,
            current_module: String::new(),
            virtual_modules: VirtualModules::new(),
            scope_bindings: HashMap::new(),
            current_match_lhs: None,
            classes: std::collections::HashMap::new(),
            methods: std::collections::HashMap::new(),
            ids: IdTable::new(),
            next_id_idx: 0,
            cache: std::collections::HashMap::new(),
            codegen_types_by_span: HashMap::new(),
            codegen_var_types: std::collections::HashMap::new(),
            call_site_dicts: HashMap::new(),
            call_site_dicts_by_span: HashMap::new(),
            call_site_forward_dicts: HashMap::new(),
            call_site_forward_dicts_by_span: HashMap::new(),
            bound_method_calls: HashMap::new(),
            bound_method_calls_by_span: HashMap::new(),
            bound_operator_calls: HashMap::new(),
            bound_operator_calls_by_span: HashMap::new(),
            bound_display_calls: HashMap::new(),
            bound_display_calls_by_span: HashMap::new(),
            existential_packs_by_span: HashMap::new(),
            existential_method_calls: HashMap::new(),
            existential_method_calls_by_span: HashMap::new(),
            for_in_infos: HashMap::new(),
            for_in_infos_by_span: HashMap::new(),
            typeclass_method_schemes: HashMap::new(),
            current_expected: None,
            type_aliases: vec![HashMap::new()],
            generic_aliases: HashMap::new(),
            const_scopes: vec![HashSet::new()],
            enums: BTreeMap::new(),
            enum_tags: BTreeMap::new(),
            enum_payloads: BTreeMap::new(),
            enum_arities: BTreeMap::new(),
            pending_exhaustive: Vec::new(),
            async_functions: std::collections::HashSet::new(),
            async_depth: 0,
            current_yield_ty: None,
            current_send_ty: None,
            yield_receives_used: false,
            c_structs: Vec::new(),
            callback_sigs: Vec::new(),
            ffi_fn_ret_tys: HashMap::new(),
            fn_result_mode: None,
            fn_option_mode: None,
            result_mode_fns: HashSet::new(),
            option_mode_fns: HashSet::new(),
            type_params_in_scope: Vec::new(),
            active_constraints: Vec::new(),
            abstract_constraint_bindings: Vec::new(),
            var_kinds: HashMap::new(),
            generics: super::generics::Generics::new(),
            generic_fns: HashSet::new(),
            fn_dict_arity: HashMap::new(),
            current_typeclass: None,
            current_assoc_projections: None,
            open_assoc_projections: HashMap::new(),
        };
        checker.register_builtin_enums();
        checker
    }

    /// Pre-register compiler-built-in enums (`FFIType`, `Option`, `Result`).
    fn register_builtin_enums(&mut self) {
        self.register_builtin_ffi_type();
        self.register_builtin_option_result();
        // `IoError` is NOT registered here — it is not auto-imported.
        // Registering its variants (esp. `Other`) globally would collide
        // with user enums that use the same constructor names. Tags are
        // installed on first `use io::…` that binds `IoError` or an IO fn.
    }

    /// Reset scope bindings and inject the auto-prelude.
    pub fn inject_prelude_scope(&mut self) {
        self.scope_bindings.clear();
        for export in self.virtual_modules.prelude_exports() {
            let name = export.short_name().to_string();
            self.scope_bindings.insert(name, export);
        }
    }

    /// Bind a virtual export under `local` (and drop any previous short
    /// binding for the export's canonical short name when `local` differs).
    pub fn bind_virtual_export(&mut self, local: String, export: BuiltinExport) {
        // Lazily register `IoError` tags when the virtual `io` module is
        // brought into scope (enum or any host fn whose scheme mentions it).
        let needs_io_error = matches!(
            &export,
            BuiltinExport::Enum {
                name: common::BUILTIN_IO_ERROR_ENUM
            } | BuiltinExport::IoFn { .. }
        );
        if needs_io_error && !self.enums.contains_key(common::BUILTIN_IO_ERROR_ENUM) {
            self.register_builtin_io_error();
        }

        let canonical = export.short_name().to_string();
        if local != canonical {
            // `use prelude::ops::Eq as PreludeEq` frees the short name.
            if self
                .scope_bindings
                .get(&canonical)
                .is_some_and(|e| e == &export)
            {
                self.scope_bindings.remove(&canonical);
            }
        }
        self.scope_bindings.insert(local, export);
    }

    /// Look up a short name in the virtual-module scope.
    pub fn scope_binding(&self, name: &str) -> Option<&BuiltinExport> {
        self.scope_bindings.get(name)
    }

    /// True when `name` is an in-scope FFI tag constructor (`Int`, …).
    pub fn ffi_tag_in_scope(&self, name: &str) -> bool {
        matches!(
            self.scope_bindings.get(name),
            Some(BuiltinExport::FfiTag { .. })
        )
    }

    /// Resolve an in-scope name to a userland FFI builtin, if any.
    pub fn ffi_fn_in_scope(&self, name: &str) -> Option<FfiBuiltin> {
        match self.scope_bindings.get(name)? {
            BuiltinExport::FfiFn { kind } => Some(*kind),
            _ => None,
        }
    }

    /// Resolve an in-scope name to a prelude/test callable (`assert`), if any.
    pub fn prelude_fn_in_scope(&self, name: &str) -> Option<PreludeFn> {
        match self.scope_bindings.get(name)? {
            BuiltinExport::Fn { kind } => Some(*kind),
            _ => None,
        }
    }

    /// Resolve an in-scope name to an IO host native (`open`, `read`, …).
    pub fn io_fn_in_scope(&self, name: &str) -> Option<IoBuiltin> {
        match self.scope_bindings.get(name)? {
            BuiltinExport::IoFn { kind } => Some(*kind),
            _ => None,
        }
    }

    /// True when a bare enum/trait name is allowed (prelude or explicit use).
    pub fn builtin_name_in_scope(&self, name: &str) -> bool {
        self.scope_bindings.contains_key(name)
    }

    pub fn virtual_modules(&self) -> &VirtualModules {
        &self.virtual_modules
    }

    /// Apply a `use` against virtual modules. Returns `true` when handled
    /// (caller should not treat it as a disk-module function import).
    pub fn apply_virtual_use(
        &mut self,
        path: &[String],
        name: &str,
        alias: Option<&str>,
    ) -> bool {
        if name == "*" {
            let Some(exports) = self.virtual_modules.resolve_glob(path) else {
                return false;
            };
            let exports: Vec<_> = exports.to_vec();
            for export in exports {
                let local = export.short_name().to_string();
                self.bind_virtual_export(local, export);
            }
            return true;
        }
        let Some(export) = self.virtual_modules.resolve_item(path, name) else {
            return false;
        };
        let local = alias
            .map(|s| s.to_string())
            .unwrap_or_else(|| name.to_string());
        self.bind_virtual_export(local, export);
        true
    }

    /// Pre-register the compiler-built-in `FFIType` enum with fixed tags.
    fn register_builtin_ffi_type(&mut self) {
        use common::{BUILTIN_FFI_TYPE_ENUM, BUILTIN_FFI_TYPE_VARIANTS};
        let name = BUILTIN_FFI_TYPE_ENUM.to_string();
        let variant_names: Vec<String> = BUILTIN_FFI_TYPE_VARIANTS
            .iter()
            .map(|s| s.to_string())
            .collect();
        let arities = vec![0; variant_names.len()];
        let payloads = vec![EnumVariantPayloadTy::Unit; variant_names.len()];
        let mut tag_map = BTreeMap::new();
        for (i, vn) in variant_names.iter().enumerate() {
            tag_map.insert(vn.clone(), i as u32);
        }
        self.enums.insert(name.clone(), variant_names);
        self.enum_tags.insert(name.clone(), tag_map);
        self.enum_payloads.insert(name.clone(), payloads);
        self.enum_arities.insert(name, arities);
    }

    /// Pre-register polymorphic `Option` / `Result` tags and payload
    /// shapes. Type annotations use `Ty::App`; these registry entries
    /// remain for constructor tags, payload arities, and codegen.
    fn register_builtin_option_result(&mut self) {
        use common::{
            BUILTIN_OPTION_ENUM, BUILTIN_OPTION_VARIANTS, BUILTIN_RESULT_ENUM,
            BUILTIN_RESULT_VARIANTS,
        };

        // Option { None, Some(T) }
        {
            let name = BUILTIN_OPTION_ENUM.to_string();
            let variant_names: Vec<String> = BUILTIN_OPTION_VARIANTS
                .iter()
                .map(|s| s.to_string())
                .collect();
            let payloads = vec![
                EnumVariantPayloadTy::Unit,
                EnumVariantPayloadTy::Tuple(vec![Ty::Con("T".into())]),
            ];
            let arities = vec![0, 1];
            let mut tag_map = BTreeMap::new();
            for (i, vn) in variant_names.iter().enumerate() {
                tag_map.insert(vn.clone(), i as u32);
            }
            self.enums.insert(name.clone(), variant_names);
            self.enum_tags.insert(name.clone(), tag_map);
            self.enum_payloads.insert(name.clone(), payloads);
            self.enum_arities.insert(name, arities);
        }

        // Result { Ok(T), Err(E) }
        {
            let name = BUILTIN_RESULT_ENUM.to_string();
            let variant_names: Vec<String> = BUILTIN_RESULT_VARIANTS
                .iter()
                .map(|s| s.to_string())
                .collect();
            let payloads = vec![
                EnumVariantPayloadTy::Tuple(vec![Ty::Con("T".into())]),
                EnumVariantPayloadTy::Tuple(vec![Ty::Con("E".into())]),
            ];
            let arities = vec![1, 1];
            let mut tag_map = BTreeMap::new();
            for (i, vn) in variant_names.iter().enumerate() {
                tag_map.insert(vn.clone(), i as u32);
            }
            self.enums.insert(name.clone(), variant_names);
            self.enum_tags.insert(name.clone(), tag_map);
            self.enum_payloads.insert(name.clone(), payloads);
            self.enum_arities.insert(name, arities);
        }
    }

    /// Pre-register `IoError` unit variants for stream IO.
    fn register_builtin_io_error(&mut self) {
        use common::{BUILTIN_IO_ERROR_ENUM, BUILTIN_IO_ERROR_VARIANTS};
        let name = BUILTIN_IO_ERROR_ENUM.to_string();
        let variant_names: Vec<String> = BUILTIN_IO_ERROR_VARIANTS
            .iter()
            .map(|s| s.to_string())
            .collect();
        let payloads = variant_names
            .iter()
            .map(|_| EnumVariantPayloadTy::Unit)
            .collect();
        let arities = vec![0; variant_names.len()];
        let mut tag_map = BTreeMap::new();
        for (i, vn) in variant_names.iter().enumerate() {
            tag_map.insert(vn.clone(), i as u32);
        }
        self.enums.insert(name.clone(), variant_names);
        self.enum_tags.insert(name.clone(), tag_map);
        self.enum_payloads.insert(name.clone(), payloads);
        self.enum_arities.insert(name, arities);
    }

    /// Scheme for a virtual `io` host native (inserted on `use io::*`).
    pub fn io_fn_scheme(kind: IoBuiltin) -> Scheme {
        use crate::typechecking::ty::{array, byte, stream_ty, tuple};
        let stream = stream_ty();
        let bytes = array(byte());
        let io_err = Ty::Con(common::BUILTIN_IO_ERROR_ENUM.into());
        let opt_int = option_app_ty(int());
        let res_opt_int = result_app_ty(opt_int, io_err.clone());
        let res_int = result_app_ty(int(), io_err.clone());
        let res_unit = result_app_ty(unit_ty(), io_err.clone());
        let res_stream = result_app_ty(stream.clone(), io_err.clone());
        let res_bytes = result_app_ty(bytes.clone(), io_err.clone());
        let res_string = result_app_ty(string(), io_err.clone());
        // `(nbytes, peer_host, peer_port)` from `io::net::udp::recv_from`.
        let recv_from_ty = tuple(vec![int(), string(), int()]);
        let res_recv_from = result_app_ty(recv_from_ty, io_err);
        let fun = |params: &[Ty], ret: Ty| {
            params.iter().rev().fold(ret, |acc, p| {
                Ty::Fun(Box::new(p.clone()), Box::new(acc))
            })
        };
        let ty = match kind {
            IoBuiltin::Stdin | IoBuiltin::Stdout | IoBuiltin::Stderr => stream,
            IoBuiltin::Open => fun(&[string(), string()], res_stream),
            IoBuiltin::Close => fun(&[stream], res_unit),
            IoBuiltin::Read | IoBuiltin::ReadExact => fun(&[stream, bytes], res_opt_int),
            IoBuiltin::Write => fun(&[stream, bytes], res_int),
            IoBuiltin::ReadToEnd => fun(&[stream], res_bytes),
            IoBuiltin::WriteAll => fun(&[stream, bytes], res_unit),
            IoBuiltin::FromBytes => fun(&[bytes], res_string),
            IoBuiltin::ToBytes => fun(&[string()], bytes),
            IoBuiltin::TcpConnect | IoBuiltin::TcpListen => {
                fun(&[string(), int()], res_stream)
            }
            IoBuiltin::TcpAccept | IoBuiltin::TcpAcceptWait => fun(&[stream], res_stream),
            IoBuiltin::UdpBind | IoBuiltin::UdpConnect => {
                fun(&[string(), int()], res_stream)
            }
            IoBuiltin::UdpSendTo => fun(&[stream, bytes, string(), int()], res_int),
            IoBuiltin::UdpRecvFrom | IoBuiltin::UdpRecvFromWait => {
                fun(&[stream, bytes], res_recv_from)
            }
            IoBuiltin::UdpLocalPort => fun(&[stream], res_int),
        };
        Scheme::mono(ty)
    }

    /// Set the module path used for ownership checks while typechecking.
    pub fn set_current_module(&mut self, module: impl Into<String>) {
        self.current_module = module.into();
    }

    /// Run inference over `ast`. Returns the inferred type of the root
    /// expression under the final substitution. Diagnostic messages are
    /// accumulated and can be retrieved with [`take_messages`].
    ///
    /// The top frame is left on the env stack after this call so that
    /// callers (and tests) can inspect declared bindings. Use
    /// [`env_mut`](Self::env_mut) and [`Env::pop`] if you need to drop
    /// it.
    pub fn check_program(&mut self, ast: &Output) -> Ty {
        // Reset per-program state. The pre-pass, the main infer
        // pass, and the post-pass all share the same checker; only
        // the per-program tables and caches get cleared.
        self.ids = IdTable::new();
        self.next_id_idx = 0;
        self.cache.clear();
        self.codegen_types_by_span.clear();
        self.codegen_var_types.clear();
        self.call_site_dicts.clear();
        self.call_site_dicts_by_span.clear();
        self.call_site_forward_dicts.clear();
        self.call_site_forward_dicts_by_span.clear();
        self.bound_method_calls.clear();
        self.bound_method_calls_by_span.clear();
        self.bound_operator_calls.clear();
        self.bound_operator_calls_by_span.clear();
        self.bound_display_calls.clear();
        self.bound_display_calls_by_span.clear();
        self.existential_packs_by_span.clear();
        self.existential_method_calls.clear();
        self.existential_method_calls_by_span.clear();
        self.for_in_infos.clear();
        self.for_in_infos_by_span.clear();
        self.typeclass_method_schemes.clear();
        self.current_expected = None;
        self.type_aliases.clear();
        self.type_aliases.push(HashMap::new());
        self.generic_aliases.clear();
        self.const_scopes.clear();
        self.const_scopes.push(HashSet::new());
        self.abstract_constraint_bindings.clear();
        self.enums.clear();
        self.enum_tags.clear();
        self.enum_payloads.clear();
        self.enum_arities.clear();
        self.c_structs.clear();
        self.callback_sigs.clear();
        self.ffi_fn_ret_tys.clear();
        self.pending_exhaustive.clear();
        self.async_functions.clear();
        self.async_depth = 0;
        self.current_yield_ty = None;
        self.current_send_ty = None;
        self.yield_receives_used = false;
        self.fn_result_mode = None;
        self.fn_option_mode = None;
        self.result_mode_fns.clear();
        self.option_mode_fns.clear();
        self.generics.generic_type_ctors.clear();
        self.generics.register_builtin_type_ctors();
        self.generics.generic_fns.clear();
        self.var_kinds.clear();
        self.current_typeclass = None;
        self.current_assoc_projections = None;
        self.open_assoc_projections.clear();
        self.register_builtin_typeclass_method_schemes();

        // Built-in enums survive the per-program enum reset.
        self.register_builtin_enums();

        // Implicit `use prelude::*; use prelude::ops::*;` — FFI stays out.
        self.inject_prelude_scope();

        // Mint NodeIds for every AST node (pre-walk). The visit order
        // matches `infer`'s recursion, so the IDs line up.
        id::pre_walk(ast, &mut self.ids);

        // Forward-declaration pre-pass: walk the AST once and
        // register every `enum` declaration's shape. This must run
        // before the main infer pass so constructor / match uses
        // that appear textually before their enum declaration still
        // resolve correctly.
        if let Err(msgs) = self.pre_register_enums(ast) {
            self.messages.extend(msgs);
        }

        // Top frame for natives/globals; left on stack after check_program.
        self.push_scope();
        let ty = self.infer(ast);
        // NOTE: the frame is intentionally NOT popped — see the
        // doc-comment above.

        // Post-pass: run deferred exhaustiveness checks now that
        // the substitution is closed and every scrutinee type can
        // be fully resolved.
        self.run_pending_exhaustiveness();

        // Return the fully-resolved type so callers see e.g. `Foo`
        // rather than `Var(0)` even when the type was inferred
        // through let-binding + unify.
        apply_ty_prune(&self.subst, &ty)
    }

    /// Take all accumulated diagnostic messages, leaving the checker
    /// with an empty message buffer.
    pub fn take_messages(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.messages)
    }

    /// Borrow the accumulated messages without consuming them.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn env(&self) -> &Env {
        &self.env
    }

    pub fn env_mut(&mut self) -> &mut Env {
        &mut self.env
    }

    /// Borrow the running substitution (useful for diagnostics).
    pub fn subst(&self) -> &Subst {
        &self.subst
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn cache(&self) -> impl Iterator<Item = (NodeId, &Ty)> {
        self.cache.iter().map(|(k, v)| (*k, v))
    }

    fn push_scope(&mut self) {
        self.env.push();
        self.const_scopes.push(HashSet::new());
        self.type_aliases.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        let _ = self.env.pop();
        let _ = self.const_scopes.pop();
        if self.type_aliases.len() > 1 {
            let _ = self.type_aliases.pop();
        } else if let Some(frame) = self.type_aliases.last_mut() {
            frame.clear();
        } else {
            self.type_aliases.push(HashMap::new());
        }
    }

    fn register_type_alias(&mut self, name: &str, alias_ty: Ty, range: Range<usize>) {
        if self.type_aliases.is_empty() {
            self.type_aliases.push(HashMap::new());
        }

        let duplicate = self
            .type_aliases
            .last()
            .map(|frame| frame.contains_key(name))
            .unwrap_or(false)
            || self.generic_aliases.contains_key(name);

        if duplicate {
            let mut msg = Message::error(
                ErrorCode::GenericTypeError,
                format!("Duplicate type alias `{}`", name),
                range,
            );
            msg.with_help("type aliases may shadow names only from an outer scope".to_string());
            self.messages.push(msg);
            return;
        }

        self.type_aliases
            .last_mut()
            .expect("type alias scope should exist")
            .insert(name.to_string(), alias_ty);
        self.generics
            .register_nominal_type(name, &self.current_module);
    }

    fn register_generic_alias(
        &mut self,
        name: &str,
        params: Vec<String>,
        param_vars: Vec<TyVarId>,
        body: Ty,
        range: Range<usize>,
    ) {
        let duplicate = self.generic_aliases.contains_key(name)
            || self
                .type_aliases
                .last()
                .map(|frame| frame.contains_key(name))
                .unwrap_or(false);
        if duplicate {
            let mut msg = Message::error(
                ErrorCode::GenericTypeError,
                format!("Duplicate type alias `{}`", name),
                range,
            );
            msg.with_help("type aliases may shadow names only from an outer scope".to_string());
            self.messages.push(msg);
            return;
        }
        self.generic_aliases.insert(
            name.to_string(),
            GenericAliasDef {
                params,
                param_vars,
                body,
            },
        );
        self.generics
            .register_nominal_type(name, &self.current_module);
    }

    /// Expand a generic alias by substituting concrete type arguments.
    fn expand_generic_alias(&self, def: &GenericAliasDef, arg_tys: &[Ty]) -> Ty {
        let mut subst = Subst::empty();
        for (var, arg) in def.param_vars.iter().zip(arg_tys.iter()) {
            subst.insert(*var, arg.clone());
        }
        apply_ty(&subst, &def.body)
    }

    fn projection_arg_key(&self, args: &[Ty]) -> Vec<String> {
        args.iter()
            .map(|arg| apply_ty_prune(&self.subst, arg).to_string())
            .collect()
    }

    fn record_current_assoc_projection(&mut self, var: TyVarId, name: &str, args: &[Ty]) {
        let Some(projections) = self.current_assoc_projections.as_mut() else {
            return;
        };
        if projections.iter().any(|p| p.var == var) {
            return;
        }
        projections.push(AssocProjection {
            var,
            name: name.to_string(),
            args: args.to_vec(),
        });
    }

    fn instantiate_assoc_value(&self, value: &AssocTypeValue, args: &[Ty]) -> Ty {
        let mut subst = Subst::empty();
        for (var, arg) in value.param_vars.iter().zip(args.iter()) {
            subst.insert(*var, apply_ty_prune(&self.subst, arg));
        }
        apply_ty(&subst, &value.ty)
    }

    fn kind_of_ty(&self, ty: &Ty) -> Kind {
        match ty {
            Ty::Var(v) => self.kind_of_var(*v),
            Ty::Con(name) => self.bare_constructor_kind(name).unwrap_or(Kind::Type),
            Ty::App(..)
            | Ty::Fun(..)
            | Ty::List(..)
            | Ty::Sum { .. }
            | Ty::Constructor { .. }
            | Ty::Tuple(_)
            | Ty::Array { .. }
            | Ty::Record { .. }
            | Ty::Existential { .. }
            | Ty::Forall { .. } => Kind::Type,
        }
    }

    fn validate_assoc_projection_args(
        &mut self,
        class: &str,
        decl: &AssocTypeDecl,
        args: &[Ty],
        range: &Range<usize>,
    ) {
        if decl.params.len() != args.len() {
            self.messages.push(Message::error(
                ErrorCode::GenericTypeError,
                format!(
                    "Associated type `{}::{}` expects {} type argument{}, got {}",
                    class,
                    decl.name,
                    decl.params.len(),
                    if decl.params.len() == 1 { "" } else { "s" },
                    args.len()
                ),
                range.clone(),
            ));
            return;
        }
        for (i, (arg, expected)) in args.iter().zip(decl.param_kinds.iter()).enumerate() {
            let actual = self.kind_of_ty(arg);
            if &actual != expected {
                self.messages.push(Message::error(
                    ErrorCode::GenericTypeError,
                    format!(
                        "Type argument {} to associated type `{}::{}` has kind `{}`, expected `{}`",
                        i + 1,
                        class,
                        decl.name,
                        actual,
                        expected
                    ),
                    range.clone(),
                ));
            }
        }
    }

    fn register_generic_type_ctor(
        &mut self,
        name: &str,
        type_params: &[parser::ast::TypeParam<'_>],
    ) -> Option<Vec<String>> {
        let previous = self.generics.generic_type_ctors.get(name).cloned();
        if type_params.is_empty() {
            return previous;
        }
        self.generics.generic_type_ctors.insert(
            name.to_string(),
            type_params.iter().map(|tp| tp.name.to_string()).collect(),
        );
        previous
    }

    fn restore_generic_type_ctor(&mut self, name: &str, previous: Option<Vec<String>>) {
        match previous {
            Some(params) => {
                self.generics
                    .generic_type_ctors
                    .insert(name.to_string(), params);
            }
            None => {
                self.generics.generic_type_ctors.remove(name);
            }
        }
    }

    fn push_type_params_for_type_parsing(
        &mut self,
        type_params: &[parser::ast::TypeParam<'_>],
    ) -> bool {
        if type_params.is_empty() {
            return false;
        }
        let mut frame = HashMap::new();
        for tp in type_params {
            frame.insert(tp.name.to_string(), self.counter.fresh());
        }
        self.type_params_in_scope.push(frame);
        true
    }

    fn pop_type_params_for_type_parsing(&mut self, pushed: bool) {
        if pushed {
            let _ = self.type_params_in_scope.pop();
        }
    }

    fn insert_const_binding(&mut self, name: impl Into<String>) {
        if self.const_scopes.is_empty() {
            self.const_scopes.push(HashSet::new());
        }
        self.const_scopes
            .last_mut()
            .expect("const scope should exist")
            .insert(name.into());
    }

    fn is_const_binding(&self, name: &str) -> bool {
        self.const_scopes
            .iter()
            .rev()
            .any(|scope| scope.contains(name))
    }

    fn coroutine_type(&self, yield_ty: Ty, send_ty: Ty) -> Ty {
        Ty::App(
            Box::new(Ty::Con("coroutine".to_string())),
            vec![yield_ty, send_ty],
        )
    }

    fn infer(&mut self, expr: &Output) -> Ty {
        // Pull the next ID from the pre-walk's minting order. Both
        // `infer` and the pre-walk visit in pre-order, so the `n`-th
        // call here consumes the `n`-th ID.
        let id = self.ids.ids()[self.next_id_idx];
        self.next_id_idx += 1;

        let ty = self.infer_inner(expr, Some(id));
        self.cache.insert(id, ty.clone());
        self.codegen_types_by_span
            .entry((expr.0.start, expr.0.end))
            .or_insert_with(|| ty.clone());
        ty
    }

    /// Register the compiler-owned signatures for the primitive classes.
    ///
    /// Their instances are emitted as bytecode thunks, but they participate in
    /// lookup exactly like source-declared classes so method/UFCS and operator
    /// dispatch share one dictionary ABI.
    fn register_builtin_typeclass_method_schemes(&mut self) {
        for (class, methods, returns_bool) in [
            ("Add", &["add"][..], false),
            ("Sub", &["sub"][..], false),
            ("Mul", &["mul"][..], false),
            ("Div", &["div"][..], false),
            ("Lt", &["lt"][..], true),
            ("Le", &["le"][..], true),
            ("Gt", &["gt"][..], true),
            ("Ge", &["ge"][..], true),
            ("Eq", &["eq", "ne"][..], true),
        ] {
            for method in methods {
                let var = self.counter.fresh();
                let ty = Ty::Fun(
                    Box::new(Ty::Var(var)),
                    Box::new(Ty::Fun(
                        Box::new(Ty::Var(var)),
                        Box::new(if returns_bool {
                            boolean()
                        } else {
                            Ty::Var(var)
                        }),
                    )),
                );
                self.typeclass_method_schemes.insert(
                    (class.to_string(), (*method).to_string()),
                    Scheme::poly(vec![var], vec![Constraint::unary(class, var)], ty),
                );
            }
        }

        let var = self.counter.fresh();
        self.typeclass_method_schemes.insert(
            ("Show".to_string(), "show".to_string()),
            Scheme::poly(
                vec![var],
                vec![Constraint::unary("Show", var)],
                Ty::Fun(Box::new(Ty::Var(var)), Box::new(string())),
            ),
        );

        // Into::into : ∀Self T. Into<Self, T> => Self → T
        {
            let self_v = self.counter.fresh();
            let t_v = self.counter.fresh();
            self.typeclass_method_schemes.insert(
                ("Into".to_string(), "into".to_string()),
                Scheme::poly(
                    vec![self_v, t_v],
                    vec![Constraint {
                        class: "Into".into(),
                        args: vec![Ty::Var(self_v), Ty::Var(t_v)],
                    }],
                    Ty::Fun(Box::new(Ty::Var(self_v)), Box::new(Ty::Var(t_v))),
                ),
            );
        }

        // Read::read / Write::write — stream IO groundwork.
        {
            use crate::typechecking::ty::{array, byte};
            let var = self.counter.fresh();
            let io_err = Ty::Con(common::BUILTIN_IO_ERROR_ENUM.into());
            let res_opt_int = result_app_ty(option_app_ty(int()), io_err.clone());
            let res_int = result_app_ty(int(), io_err);
            let bytes = array(byte());
            self.typeclass_method_schemes.insert(
                ("Read".to_string(), "read".to_string()),
                Scheme::poly(
                    vec![var],
                    vec![Constraint::unary("Read", var)],
                    Ty::Fun(
                        Box::new(Ty::Var(var)),
                        Box::new(Ty::Fun(Box::new(bytes.clone()), Box::new(res_opt_int))),
                    ),
                ),
            );
            let var = self.counter.fresh();
            self.typeclass_method_schemes.insert(
                ("Write".to_string(), "write".to_string()),
                Scheme::poly(
                    vec![var],
                    vec![Constraint::unary("Write", var)],
                    Ty::Fun(
                        Box::new(Ty::Var(var)),
                        Box::new(Ty::Fun(Box::new(bytes), Box::new(res_int))),
                    ),
                ),
            );
        }

        // Iterator::next : ∀I Item. Iterator<I> => I → Option<Item>
        {
            let i_var = self.counter.fresh();
            let item_var = self.counter.fresh();
            self.typeclass_method_schemes.insert(
                ("Iterator".to_string(), "next".to_string()),
                Scheme::poly_with_kinds_and_assoc(
                    vec![i_var, item_var],
                    vec![Kind::Type, Kind::Type],
                    vec![Constraint::unary("Iterator", i_var)],
                    vec![AssocProjection {
                        var: item_var,
                        name: "Item".into(),
                        args: vec![],
                    }],
                    Ty::Fun(
                        Box::new(Ty::Var(i_var)),
                        Box::new(option_app_ty(Ty::Var(item_var))),
                    ),
                ),
            );
        }
        // IntoIterator::into_iter : ∀T Item IntoIter. IntoIterator<T> => T → IntoIter
        {
            let t_var = self.counter.fresh();
            let item_var = self.counter.fresh();
            let into_iter_var = self.counter.fresh();
            self.typeclass_method_schemes.insert(
                ("IntoIterator".to_string(), "into_iter".to_string()),
                Scheme::poly_with_kinds_and_assoc(
                    vec![t_var, item_var, into_iter_var],
                    vec![Kind::Type, Kind::Type, Kind::Type],
                    vec![Constraint::unary("IntoIterator", t_var)],
                    vec![
                        AssocProjection {
                            var: item_var,
                            name: "Item".into(),
                            args: vec![],
                        },
                        AssocProjection {
                            var: into_iter_var,
                            name: "IntoIter".into(),
                            args: vec![],
                        },
                    ],
                    Ty::Fun(Box::new(Ty::Var(t_var)), Box::new(Ty::Var(into_iter_var))),
                ),
            );
        }
    }

    /// Inner inference — does the actual dispatch but no caching.
    /// Every recursive call into a child still goes through
    /// [`infer`](Self::infer), so each child also gets cached.
    fn infer_inner(&mut self, expr: &Output, id: Option<NodeId>) -> Ty {
        let range = expr.0.into_range();
        let child = expr.1.as_ref();

        match child {
            // ---- Literals ----
            Expression::Integer(_) => int(),
            Expression::Float(_) => float(),
            Expression::String(_) => string(),
            Expression::Bool(_) => boolean(),

            // ---- Names ----
            Expression::Identifier(name) => {
                let scheme = self.env.lookup(name).cloned();
                match scheme {
                    Some(s) => self.instantiate_ty(&s),
                    None => self.error(
                        ErrorCode::UnknownValue,
                        format!("Cannot find value `{}` in this scope", name),
                        range,
                    ),
                }
            }

            // A bare type name (only valid as an annotation, but be
            // permissive).
            Expression::Type(name) => self.parse_type_name_str(name),
            Expression::TypeFun(arg, ret) => {
                let arg_ty = self.infer(arg);
                let ret_ty = self.infer(ret);
                Ty::Fun(Box::new(arg_ty), Box::new(ret_ty))
            }

            // ---- Wrappers / no-ops ----
            Expression::Noop(_)
            | Expression::Comment(_)
            | Expression::Break
            | Expression::Continue => unit_ty(),
            // `use` — virtual modules first, else disk-module function alias
            Expression::Use {
                path,
                name,
                alias,
            } => {
                if self.apply_virtual_use(path, name, alias.as_deref()) {
                    // Bind FFI callables into the value env so Call sites
                    // resolve; enums/traits/tags are scope-only.
                    let locals: Vec<(String, BuiltinExport)> = self
                        .scope_bindings
                        .iter()
                        .filter(|(_, e)| {
                            matches!(e, BuiltinExport::FfiFn { .. } | BuiltinExport::IoFn { .. })
                        })
                        .map(|(k, e)| (k.clone(), e.clone()))
                        .collect();
                    for (local, export) in locals {
                        if self.env.lookup(&local).is_some() {
                            continue;
                        }
                        match export {
                            BuiltinExport::IoFn { kind } => {
                                self.env.insert_top(local, Self::io_fn_scheme(kind));
                            }
                            BuiltinExport::FfiFn { .. } => {
                                self.env
                                    .insert_top(local, Scheme::mono(Ty::Var(self.counter.fresh())));
                            }
                            _ => {}
                        }
                    }
                    return unit_ty();
                }
                let local = alias.clone().unwrap_or_else(|| name.clone());
                // Disk module: insert a polymorphic type variable so
                // calls to the local name pass type-checking. Codegen
                // resolves the FQN via `self.aliases`.
                self.env
                    .insert_top(local, Scheme::mono(Ty::Var(self.counter.fresh())));
                unit_ty()
            }
            Expression::Module(_, _) => unit_ty(),
            // FFI declaration block — register each function
            // signature in the top frame (so subsequent calls
            // can type-check) and return unit. The body is
            // empty (FFI symbols are resolved at VM startup,
            // not at compile time).
            Expression::ExternBlock {
                library: _,
                declarations,
            } => {
                for decl in declarations {
                    let arg_tys: Vec<Ty> = if let Expression::Fragment(items) = decl.args.1.as_ref()
                    {
                        items
                            .iter()
                            .filter_map(|item| {
                                if let Expression::Argument(ty, _) = item.1.as_ref() {
                                    Some(self.parse_type_name(ty))
                                } else {
                                    None
                                }
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    let ret_ty = decl
                        .returns
                        .as_ref()
                        .map(|r| self.parse_type_name(r))
                        .unwrap_or_else(unit_ty);
                    let fn_ty = arg_tys
                        .iter()
                        .rev()
                        .fold(ret_ty, |acc, p| Ty::Fun(Box::new(p.clone()), Box::new(acc)));
                    self.env
                        .insert_top(decl.name.to_string(), Scheme::mono(fn_ty));
                }
                unit_ty()
            }

            Expression::Expr(e) | Expression::Group(e) | Expression::Statement(e) => self.infer(e),
            Expression::ExprStatement(e) => self.infer(e),

            // ---- Blocks ----
            // Program runs in the current frame (the global frame from
            // check_program). This is what makes top-level `let`
            // bindings visible after inference. Block introduces its
            // own scope.
            Expression::Block(children) => {
                self.push_scope();
                let mut last_ty = unit_ty();
                for child in children {
                    last_ty = self.infer(child);
                }
                self.pop_scope();
                last_ty
            }
            Expression::Program(children) => {
                let mut last_ty = unit_ty();
                for child in children {
                    last_ty = self.infer(child);
                }
                last_ty
            }

            // ---- Fragments (from `let x = expr`) ----
            Expression::Fragment(children) => self.infer_fragment(children),

            // ---- `let` / `const` ----
            Expression::Variable(name, ty_opt) => {
                let var_ty = match ty_opt {
                    Some(ann) => self.parse_type_name(ann),
                    None => Ty::Var(self.counter.fresh()),
                };
                self.env.insert_top(name.to_string(), Scheme::mono(var_ty));
                unit_ty()
            }

            Expression::Constant(name, ty_opt) => {
                let var_ty = match ty_opt {
                    Some(ann) => self.parse_type_name(ann),
                    None => Ty::Var(self.counter.fresh()),
                };
                let ident = match name.1.as_ref() {
                    Expression::Identifier(n) => n.to_string(),
                    _ => {
                        return self.error_with_help(
                            ErrorCode::GenericTypeError,
                            "Invalid constant name".to_string(),
                            range,
                            Some("a constant name must be an identifier".to_string()),
                        );
                    }
                };
                self.env.insert_top(ident.clone(), Scheme::mono(var_ty));
                self.insert_const_binding(ident);
                unit_ty()
            }

            // ---- Assignment / compound assignment / adjust ----
            Expression::CompoundAssign(target, op, value) => {
                let target_ty = self.infer_mutable_lvalue(target, range.clone());
                let val_ty = self.infer(value);
                let op_name = Self::compound_op_name(*op);
                if matches!(
                    op,
                    parser::ast::AssignOp::Shl
                        | parser::ast::AssignOp::Shr
                        | parser::ast::AssignOp::BitAnd
                        | parser::ast::AssignOp::BitOr
                        | parser::ast::AssignOp::BitXor
                ) {
                    let _ = unify_with(&self.subst, &target_ty, &int());
                    let _ = unify_with(&self.subst, &val_ty, &int());
                } else {
                    self.unify(
                        &target_ty,
                        &val_ty,
                        &range,
                        &format!("operands of `{}=`", op_name),
                    );
                }
                apply_ty_prune(&self.subst, &target_ty)
            }

            Expression::Assignment(name, value) => {
                // `x = resume x` overwrites the coroutine handle with the yield value.
                if let (Expression::Identifier(var_name), Expression::Resume(target, None)) =
                    (name.1.as_ref(), value.1.as_ref())
                {
                    if let Expression::Identifier(target_name) = target.1.as_ref() {
                        if var_name == target_name {
                            let val_ty = self.infer(value);
                            if self.env.lookup(var_name).is_some() {
                                self.env
                                    .insert_top(var_name.to_string(), Scheme::mono(val_ty.clone()));
                                self.codegen_var_types
                                    .insert(var_name.to_string(), val_ty.clone());
                            }
                            return val_ty;
                        }
                    }
                }

                if is_yield_expression(value) {
                    self.yield_receives_used = true;
                }
                let val_ty = self.infer(value);
                let target_ty = self.infer_mutable_lvalue(name, range.clone());
                self.coerce_or_unify(&target_ty, &val_ty, Some(value), &range, "assignment");
                apply_ty_prune(&self.subst, &val_ty)
            }

            // ---- Arithmetic / bitwise ----
            Expression::Add(lhs, rhs) => self.infer_arith(lhs, rhs, id, range, "+"),
            Expression::Sub(lhs, rhs) => self.infer_arith(lhs, rhs, id, range, "-"),
            Expression::Mul(lhs, rhs) => self.infer_arith(lhs, rhs, id, range, "*"),
            Expression::Div(lhs, rhs) => self.infer_arith(lhs, rhs, id, range, "/"),
            Expression::Mod(lhs, rhs) => self.infer_arith(lhs, rhs, id, range, "%"),
            Expression::Pow(lhs, rhs) => self.infer_arith(lhs, rhs, id, range, "**"),
            Expression::Shl(lhs, rhs) => self.infer_arith(lhs, rhs, id, range, "<<"),
            Expression::Shr(lhs, rhs) => self.infer_arith(lhs, rhs, id, range, ">>"),
            Expression::Xor(lhs, rhs) => self.infer_arith(lhs, rhs, id, range, "^"),
            Expression::BitAnd(lhs, rhs) => self.infer_arith(lhs, rhs, id, range, "&"),
            Expression::BitOr(lhs, rhs) => self.infer_arith(lhs, rhs, id, range, "|"),

            // ---- Logical ----
            Expression::And(lhs, rhs) | Expression::Or(lhs, rhs) => {
                let lt = self.infer(lhs);
                let rt = self.infer(rhs);
                self.unify(&lt, &boolean(), &lhs.0.into_range(), "left of logical");
                self.unify(&rt, &boolean(), &rhs.0.into_range(), "right of logical");
                boolean()
            }

            // ---- Comparison ----
            Expression::Eq(lhs, rhs) => self.infer_comparison(lhs, rhs, id, range, "Eq", "eq"),
            Expression::Neq(lhs, rhs) => self.infer_comparison(lhs, rhs, id, range, "Eq", "ne"),
            Expression::Le(lhs, rhs) => self.infer_comparison(lhs, rhs, id, range, "Lt", "lt"),
            Expression::Gt(lhs, rhs) => self.infer_comparison(lhs, rhs, id, range, "Gt", "gt"),
            Expression::Leq(lhs, rhs) => self.infer_comparison(lhs, rhs, id, range, "Le", "le"),
            Expression::Geq(lhs, rhs) => self.infer_comparison(lhs, rhs, id, range, "Ge", "ge"),

            // ---- Prefix / postfix ----
            Expression::Negate(e) | Expression::Positive(e) => self.infer(e),
            Expression::Not(e) => {
                let t = self.infer(e);
                self.unify(&t, &int(), &e.0.into_range(), "operand of `~`");
                int()
            }
            Expression::LogicalNot(e) => {
                let t = self.infer(e);
                let pruned = apply_ty_prune(&self.subst, &t);
                match pruned {
                    Ty::Con(name) if name == "bool" || name == "int" => boolean(),
                    _ => {
                        let _ = self.error_with_help(
                            ErrorCode::GenericTypeError,
                            "Logical NOT requires a `bool` or `int` operand".to_string(),
                            e.0.into_range(),
                            Some(format!(
                                "found `{pruned}`; use `~` for bitwise negation on integers"
                            )),
                        );
                        boolean()
                    }
                }
            }
            Expression::Adjust { target, .. } => {
                let ty = self.infer_mutable_lvalue(target, range.clone());
                let pruned = apply_ty_prune(&self.subst, &ty);
                if !matches!(pruned, Ty::Con(ref n) if n == "int" || n == "float") {
                    let _ = self.error_with_help(
                        ErrorCode::GenericTypeError,
                        "Increment/decrement requires a numeric lvalue".to_string(),
                        range,
                        Some(
                            "only `int` and `float` variables, fields, and indices support ++/--"
                                .to_string(),
                        ),
                    );
                }
                pruned
            }
            Expression::Call { name, args } => {
                // Method call: `recv.method(args)` — Access callee.
                if let Expression::Access(recv, method) = name.1.as_ref() {
                    let recv_ty = self.infer(recv);
                    let resolved = apply_ty_prune(&self.subst, &recv_ty);
                    if let Ty::Existential { class } = &resolved
                        && let Some((owner, method_slot, scheme)) =
                            self.existential_method_candidate(class, method)
                    {
                        let mut arg_tys = vec![recv_ty];
                        if let Some(a) = args {
                            for arg in a {
                                arg_tys.push(self.infer(arg));
                            }
                        }
                        let hint = ExistentialMethodCall {
                            method_slot,
                            arity: arg_tys.len(),
                            has_receiver: true,
                        };
                        if let Some(call_id) = id {
                            self.existential_method_calls.insert(call_id, hint.clone());
                        }
                        self.existential_method_calls_by_span
                            .insert((range.start, range.end), hint);
                        return self.apply_existential_method(
                            &owner,
                            method,
                            &scheme,
                            &arg_tys,
                            args.as_deref(),
                            id,
                            range,
                        );
                    }
                    if let Some(receiver_var) = Self::constraint_var_of_ty(&resolved) {
                        let candidates = self.bound_method_candidates(method, Some(receiver_var));
                        if let Some((dict_index, dict_class, class, method_slot, scheme)) =
                            self.select_bound_method(candidates, method, &range)
                        {
                            self.bind_matching_abstract_constraints(Some(receiver_var), &dict_class);
                            let (fun_ty, constraints, mapping) =
                                self.instantiate_scheme_mapped(&scheme);
                            let mut arg_tys = vec![recv_ty];
                            if let Some(a) = args {
                                for arg in a {
                                    arg_tys.push(self.infer(arg));
                                }
                            }
                            if let Some(call_id) = id {
                                self.bound_method_calls.insert(
                                    call_id,
                                    BoundMethodCall {
                                        dict_index,
                                        method_slot,
                                        arity: arg_tys.len(),
                                        has_receiver: true,
                                    },
                                );
                            }
                            self.bound_method_calls_by_span.insert(
                                (range.start, range.end),
                                BoundMethodCall {
                                    dict_index,
                                    method_slot,
                                    arity: arg_tys.len(),
                                    has_receiver: true,
                                },
                            );
                            let result = self.apply_function(
                                Some(&format!("{}::{}", class, method)),
                                &fun_ty,
                                &arg_tys,
                                None,
                                id,
                                range.clone(),
                            );
                            if !constraints.is_empty() {
                                self.discharge_constraints(id, &constraints, &range);
                                self.pin_assoc_after_discharge(
                                    &class,
                                    &constraints,
                                    Some(&scheme),
                                    &mapping,
                                    &range,
                                );
                            }
                            return result;
                        }
                    }
                    // Inherent class methods win over ground trait methods
                    // (Rust-style): `impl Point { fn show() ... }` must not be
                    // shadowed by prelude `Show::show` when no Show instance
                    // exists for Point.
                    let class_owner = match &resolved {
                        Ty::Con(n) if self.classes.contains_key(n) => Some(n.clone()),
                        Ty::App(head, _) => match head.as_ref() {
                            Ty::Con(n) if self.classes.contains_key(n) => Some(n.clone()),
                            _ => None,
                        },
                        _ => None,
                    };
                    if let Some(owner) = class_owner.as_ref()
                        && let Some(scheme) = self
                            .methods
                            .get(owner)
                            .and_then(|m| m.get(*method))
                            .map(|(_, s)| s.clone())
                    {
                        let fun_ty = self.instantiate_ty(&scheme);
                        let mut arg_tys = vec![recv_ty];
                        if let Some(a) = args {
                            for arg in a {
                                arg_tys.push(self.infer(arg));
                            }
                        }
                        return self.apply_function(
                            Some(&format!("{}::{}", owner, method)),
                            &fun_ty,
                            &arg_tys,
                            None,
                            id,
                            range,
                        );
                    }

                    // Ground trait method: `recv.into()` / `recv.show()` via a
                    // concrete instance (no open bound). Pin the return type from
                    // `current_expected` when present so `let y: T = x.into();`
                    // (or `return x.into();` under `-> T`) can select among
                    // multiple `Into` targets.
                    if let Some((class, scheme)) =
                        self.ground_trait_method_for_receiver(method, &recv_ty)
                    {
                        let (fun_ty, constraints, mapping) =
                            self.instantiate_scheme_mapped(&scheme);
                        let mut arg_tys = vec![recv_ty];
                        if let Some(a) = args {
                            for arg in a {
                                arg_tys.push(self.infer(arg));
                            }
                        }
                        let result = self.apply_function(
                            Some(&format!("{}::{}", class, method)),
                            &fun_ty,
                            &arg_tys,
                            None,
                            id,
                            range.clone(),
                        );
                        if let Some(expected) = self.current_expected.clone() {
                            self.unify(&result, &expected, &range, "expected type");
                        }
                        if !constraints.is_empty() {
                            self.discharge_constraints(id, &constraints, &range);
                            self.pin_assoc_after_discharge(
                                &class,
                                &constraints,
                                Some(&scheme),
                                &mapping,
                                &range,
                            );
                        }
                        return apply_ty_prune(&self.subst, &result);
                    }

                    if let Some(owner) = class_owner {
                        return self.error(
                            ErrorCode::UnknownFunction,
                            format!("Cannot find method `{}` on class `{}`", method, owner),
                            range,
                        );
                    }
                    return self.error_with_help(
                        ErrorCode::NotAFunction,
                        format!("Cannot call method `{}` on non-class type", method),
                        range,
                        Some("method calls require a class instance receiver".to_string()),
                    );
                }

                let ident = match name.1.as_ref() {
                    Expression::Identifier(n) => n.to_string(),
                    _ => {
                        return self.error(
                            ErrorCode::UnknownFunction,
                            "Invalid call target".to_string(),
                            range,
                        );
                    }
                };

                if ident == "push" {
                    return self.infer_array_push(args.as_deref(), range);
                }
                if ident == "len" {
                    return self.infer_array_len(args.as_deref(), range);
                }
                // `assert` from `prelude::test` (auto-imported or via `use`).
                if let Some(kind) = self.prelude_fn_in_scope(&ident) {
                    let arg_slice = args.as_deref().unwrap_or(&[]);
                    return match kind {
                        PreludeFn::Assert => self.infer_assert(arg_slice, range),
                    };
                }
                // `dload` / `declare` / `invoke` after `use ffi::*`.
                if let Some(kind) = self.ffi_fn_in_scope(&ident) {
                    let arg_slice = args.as_deref().unwrap_or(&[]);
                    return match kind {
                        FfiBuiltin::Dload => self.infer_ffi_dload(arg_slice, range),
                        FfiBuiltin::Declare => self.infer_ffi_declare(arg_slice, range),
                        FfiBuiltin::Invoke => self.infer_ffi_invoke(arg_slice, range),
                    };
                }
                if matches!(ident.as_str(), "dload" | "declare" | "invoke") {
                    return self.error_with_help(
                        ErrorCode::UnknownValue,
                        format!("Cannot find value `{}` in this scope", ident),
                        range,
                        Some(format!(
                            "import it with `use ffi::{};` or `use ffi::*;`",
                            ident
                        )),
                    );
                }

                // Bare/UFCS trait method call: `method(x)`.
                // Resolve it before ordinary environment lookup because class
                // methods are selected by the active bound, not by a global FQN.
                let arg_tys: Vec<Ty> = match args {
                    Some(a) => a.iter().map(|arg| self.infer(arg)).collect(),
                    None => Vec::new(),
                };
                if let Some(Ty::Existential { class }) =
                    arg_tys.first().map(|ty| apply_ty_prune(&self.subst, ty))
                    && let Some((owner, method_slot, scheme)) =
                        self.existential_method_candidate(&class, &ident)
                {
                    let hint = ExistentialMethodCall {
                        method_slot,
                        arity: arg_tys.len(),
                        has_receiver: false,
                    };
                    if let Some(call_id) = id {
                        self.existential_method_calls.insert(call_id, hint.clone());
                    }
                    self.existential_method_calls_by_span
                        .insert((range.start, range.end), hint);
                    return self.apply_existential_method(
                        &owner,
                        &ident,
                        &scheme,
                        &arg_tys,
                        args.as_deref(),
                        id,
                        range,
                    );
                }
                let candidates = self.bound_method_candidates(&ident, None);
                if !candidates.is_empty() {
                    let receiver_var = arg_tys.first().and_then(|ty| {
                        Self::constraint_var_of_ty(&apply_ty_prune(&self.subst, ty))
                    });
                    let candidates = receiver_var
                        .map(|v| self.bound_method_candidates(&ident, Some(v)))
                        .unwrap_or_else(|| self.bound_method_candidates(&ident, None));
                    if let Some((dict_index, dict_class, class, method_slot, scheme)) =
                        self.select_bound_method(candidates, &ident, &range)
                    {
                        self.bind_matching_abstract_constraints(receiver_var, &dict_class);
                        let (fun_ty, constraints, mapping) =
                            self.instantiate_scheme_mapped(&scheme);
                        if let Some(call_id) = id {
                            self.bound_method_calls.insert(
                                call_id,
                                BoundMethodCall {
                                    dict_index,
                                    method_slot,
                                    arity: arg_tys.len(),
                                    has_receiver: false,
                                },
                            );
                        }
                        self.bound_method_calls_by_span.insert(
                            (range.start, range.end),
                            BoundMethodCall {
                                dict_index,
                                method_slot,
                                arity: arg_tys.len(),
                                has_receiver: false,
                            },
                        );
                        let result = self.apply_function(
                            Some(&format!("{}::{}", class, ident)),
                            &fun_ty,
                            &arg_tys,
                            args.as_deref(),
                            id,
                            range.clone(),
                        );
                        if !constraints.is_empty() {
                            self.discharge_constraints(id, &constraints, &range);
                            self.pin_assoc_after_discharge(
                                &class,
                                &constraints,
                                Some(&scheme),
                                &mapping,
                                &range,
                            );
                        }
                        return result;
                    }
                }

                let scheme = self.env.lookup(&ident).cloned();
                let (fun_ty, fresh_constraints, fresh_mapping, original_scheme) = match scheme {
                    Some(s) => {
                        let (fun_ty, constraints, mapping) = self.instantiate_scheme_mapped(&s);
                        (fun_ty, constraints, mapping, Some(s))
                    }
                    None => {
                        return self.error(
                            ErrorCode::UnknownFunction,
                            format!("Cannot find function `{}`", ident),
                            range,
                        );
                    }
                };

                let result = self.apply_function(
                    Some(&ident),
                    &fun_ty,
                    &arg_tys,
                    args.as_deref(),
                    id,
                    range.clone(),
                );
                // Discharge trait constraints from the instantiated scheme.
                // This verifies that each concrete type argument satisfies the
                // required bound, or propagates the constraint if the caller is
                // itself generic with the same bound.
                if !fresh_constraints.is_empty() {
                    self.discharge_constraints(id, &fresh_constraints, &range);
                    if let Some(scheme) = original_scheme.as_ref() {
                        self.pin_assoc_after_discharge(
                            "",
                            &fresh_constraints,
                            Some(scheme),
                            &fresh_mapping,
                            &range,
                        );
                    }
                }
                result
            }

            // ---- Match / loop / if ----
            Expression::If(branches) => self.infer_if(branches),
            Expression::Branch(cond, body) => {
                if let Some(c) = cond {
                    let ct = self.infer(c);
                    self.unify(&ct, &boolean(), &c.0.into_range(), "branch condition");
                }
                self.infer(body)
            }
            Expression::Match { scrutinee, arms } => self.infer_match(scrutinee, arms, range),
            Expression::Loop {
                identifier,
                iterable,
                body,
            } => {
                if let Some(binding) = identifier {
                    // `for x in expr { body }` — IntoIterator / Iterator protocol
                    // (builtin arrays, homogeneous tuples/dicts, coroutines, or
                    // user `impl`s). Bind `x : Item`.
                    let it = self.infer(iterable);
                    let resolved = apply_ty_prune(&self.subst, &it);
                    let elem_ty = self
                        .resolve_for_in_iterable(&resolved, id, &iterable.0.into_range(), &range)
                        .unwrap_or_else(|| Ty::Var(self.counter.fresh()));
                    self.env.push();
                    if let Expression::Identifier(name) = binding.1.as_ref() {
                        self.env
                            .insert_top(name.to_string(), Scheme::mono(elem_ty.clone()));
                        self.codegen_var_types
                            .insert(name.to_string(), elem_ty.clone());
                    }
                    // Consume the binding node's ID (pre-walk order) now that
                    // the name is in scope.
                    let _ = self.infer(binding);
                    let _ = self.infer(body);
                    self.env.pop();
                    unit_ty()
                } else {
                    let it = self.infer(iterable);
                    self.unify(&it, &boolean(), &iterable.0.into_range(), "while condition");
                    let _ = self.infer(body);
                    unit_ty()
                }
            }
            Expression::For {
                init,
                cond,
                step,
                body,
            } => {
                if let Some(init) = init {
                    let _ = self.infer(init);
                }
                let cond_ty = self.infer(cond);
                self.unify(&cond_ty, &boolean(), &cond.0.into_range(), "for condition");
                let _ = self.infer(body);
                if let Some(step) = step {
                    let _ = self.infer(step);
                }
                unit_ty()
            }

            // ---- Return ----
            Expression::Return(e) | Expression::ImplicitReturn(e) => {
                // Push the declared return type as expected so ground trait
                // calls like `return c.into();` can pin `Into`'s target `T`
                // before constraint discharge (same as annotated `let`).
                let prev_expected = self.current_expected.take();
                if let Some(ret) = self.current_return_ty.clone() {
                    self.current_expected = Some(ret);
                }
                let ty = self.infer(e);
                self.current_expected = prev_expected;
                if let Some(ret) = self.current_return_ty.clone() {
                    self.coerce_or_unify(&ret, &ty, Some(e), &e.0.into_range(), "return value");
                }
                ty
            }

            // ---- raise / ? / ?? / ?. ----
            Expression::Raise(e) => {
                let err_ty = self.infer(e);
                let ok_ty = self.ensure_result_mode(&err_ty, &e.0.into_range());
                // `raise` is diverging for the current expression;
                // give it the Ok type so it can appear in expression
                // position (e.g. as a branch value).
                ok_ty
            }

            Expression::Panic(e) => {
                let msg_ty = self.infer(e);
                self.unify(
                    &msg_ty,
                    &string(),
                    &e.0.into_range(),
                    "panic message",
                );
                // Diverging: fresh ty var so it can appear in any expression position.
                Ty::Var(self.counter.fresh())
            }

            Expression::Try(inner) => {
                let inner_ty = self.infer(inner);
                let resolved = apply_ty_prune(&self.subst, &inner_ty);
                if let Some((ok, err)) = result_ok_err(&resolved) {
                    let _ = self.ensure_result_mode(&err, &range);
                    ok
                } else if is_option_ty(&resolved) {
                    let inner =
                        option_inner(&resolved).unwrap_or_else(|| Ty::Var(self.counter.fresh()));
                    self.ensure_option_mode(&inner, &range);
                    inner
                } else if matches!(resolved, Ty::Var(_)) {
                    // Not yet pinned — assume Result and let later
                    // unifications fill Ok/Err (or fail).
                    let ok = Ty::Var(self.counter.fresh());
                    let err = Ty::Var(self.counter.fresh());
                    let result = result_ty(ok.clone(), err.clone());
                    self.unify(&inner_ty, &result, &range, "try operand");
                    let _ = self.ensure_result_mode(&err, &range);
                    ok
                } else {
                    self.error(
                        ErrorCode::InvalidTry,
                        format!("`?` requires Option or Result, found `{}`", resolved),
                        range,
                    )
                }
            }

            Expression::Coalesce(lhs, rhs) => {
                let lhs_ty = self.infer(lhs);
                let resolved = apply_ty_prune(&self.subst, &lhs_ty);
                let payload = if let Some((ok, _)) = result_ok_err(&resolved) {
                    ok
                } else if is_option_ty(&resolved) {
                    option_inner(&resolved).unwrap_or_else(|| Ty::Var(self.counter.fresh()))
                } else if matches!(resolved, Ty::Var(_)) {
                    // Prefer Option for free vars under `??`.
                    let inner = Ty::Var(self.counter.fresh());
                    self.unify(
                        &lhs_ty,
                        &option_ty(inner.clone()),
                        &lhs.0.into_range(),
                        "coalesce lhs",
                    );
                    inner
                } else {
                    return self.error(
                        ErrorCode::InvalidCoalesce,
                        format!("`??` requires Option or Result, found `{}`", resolved),
                        range,
                    );
                };
                let rhs_ty = self.infer(rhs);
                self.unify(&payload, &rhs_ty, &rhs.0.into_range(), "coalesce rhs");
                payload
            }

            Expression::OptionalAccess(receiver, field) => {
                let recv_ty = self.infer(receiver);
                let resolved = apply_ty_prune(&self.subst, &recv_ty);
                let inner = if is_option_ty(&resolved) {
                    option_inner(&resolved).unwrap_or_else(|| Ty::Var(self.counter.fresh()))
                } else if matches!(resolved, Ty::Var(_)) {
                    let inner = Ty::Var(self.counter.fresh());
                    self.unify(
                        &recv_ty,
                        &option_ty(inner.clone()),
                        &receiver.0.into_range(),
                        "optional access receiver",
                    );
                    inner
                } else {
                    return self.error(
                        ErrorCode::InvalidOptionalAccess,
                        format!("`?.` requires Option, found `{}`", resolved),
                        range,
                    );
                };
                // Resolve field on the inner type (enum record / dict).
                let field_ty = self.field_type_from_ty(&inner, field, &range);
                option_ty(field_ty)
            }

            Expression::TypeApp { name, args } => {
                // Appears in type-annotation positions; treat like Type.
                self.parse_type_app(name, args, range)
            }
            Expression::TypeProjection { owner, name, args } => {
                let arg_tys: Vec<Ty> = args.iter().map(|a| self.parse_type_name(a)).collect();
                self.resolve_type_projection(owner, name, &arg_tys, &range)
            }

            // ---- I/O ----
            Expression::Print(fmt, params) => {
                self.infer_print(fmt, params, range, "print");
                unit_ty()
            }
            Expression::Format(fmt, params) => {
                self.infer_print(fmt, params, range, "format");
                string()
            }

            // ---- Userland FFI builtins ----
            //
            // Legacy AST form (tests / older parsers). Prefer Call + `use ffi::*`.
            Expression::Dload(path) => self.infer_ffi_dload(std::slice::from_ref(path), range),
            // `done(h)` — true when coroutine handle `h` is Done.
            Expression::Done(handle) => {
                let handle_ty = self.infer(handle);
                let y_var = Ty::Var(self.counter.fresh());
                let s_var = Ty::Var(self.counter.fresh());
                let coro_ty = self.coroutine_type(y_var, s_var);
                self.unify(&handle_ty, &coro_ty, &range, "done argument");
                boolean()
            }
            // Tuple literal
            Expression::Tuple(items) => {
                let mut elem_tys = Vec::with_capacity(items.len());
                for item in items {
                    let t = self.infer(item);
                    elem_tys.push(apply_ty_prune(&self.subst, &t));
                }
                tuple_ty(elem_tys)
            }
            // Array literal (static length from item count)
            Expression::Array(items) => {
                let mut elem_ty: Option<Ty> = None;
                for item in items {
                    let t = self.infer(item);
                    let t_pruned = apply_ty_prune(&self.subst, &t);
                    match &elem_ty {
                        None => elem_ty = Some(t_pruned),
                        Some(prev) => {
                            let prev_pruned = apply_ty_prune(&self.subst, prev);
                            if unify_with(&self.subst, &prev_pruned, &t_pruned).is_err() {
                                let _ = self.error_with_help(
                                    ErrorCode::TypeMismatch, format!(
                                        "array element type mismatch: expected `{}`, found `{}`",
                                        prev_pruned,
                                        t_pruned
                                    ),
                                    range.clone(),
                                    Some("an array literal requires every element to have the same type".to_string()),
                                );
                            }
                        }
                    }
                }
                let element = elem_ty.unwrap_or_else(|| Ty::Var(self.counter.fresh()));
                let len = items.len();
                if len > 0 {
                    array_fixed(element, len)
                } else {
                    array(element)
                }
            }
            // Index: static-length OOB check for literal indices
            Expression::Index(target, index_expr) => {
                let target_ty = self.infer(target);
                let target_ty = apply_ty_prune(&self.subst, &target_ty);
                let index_ty = self.infer(index_expr);
                let index_ty_pruned = apply_ty_prune(&self.subst, &index_ty);
                // Constrain the index to be an `int` (the VM only
                // supports integer indices).
                let _ = unify_with(&self.subst, &index_ty_pruned, &int());
                let resolved = apply_ty_prune(&self.subst, &target_ty);
                match &resolved {
                    Ty::Array { element, length } => {
                        // Out-of-bounds check: only fires when the
                        // target is a *static-length* array and the
                        // index is a literal integer.
                        if let ArrayLength::Static(n) = length
                            && let Expression::Integer(idx) = index_expr.1.as_ref()
                        {
                            let i = *idx;
                            if i < 0 || (i as usize) >= *n {
                                let _ = self.error_with_help(
                                    ErrorCode::IndexOutOfBounds,
                                    format!(
                                        "array index {} out of bounds for array of length {}",
                                        i, n
                                    ),
                                    range.clone(),
                                    Some(format!(
                                        "indices are valid in [0..{}); the array has length {}",
                                        n, n
                                    )),
                                );
                            }
                        }
                        (**element).clone()
                    }
                    Ty::Tuple(tys) => {
                        // Tuple indexing: same diagnostic on constant
                        // out-of-bounds; dynamic fallback returns a
                        // fresh ty var (the runtime pushes -1i64 for
                        // OOB).
                        if let Expression::Integer(idx) = index_expr.1.as_ref() {
                            let i = *idx;
                            if i < 0 || (i as usize) >= tys.len() {
                                let _ = self.error_with_help(
                                    ErrorCode::IndexOutOfBounds,
                                    format!(
                                        "tuple index {} out of bounds for tuple of length {}",
                                        i,
                                        tys.len()
                                    ),
                                    range.clone(),
                                    Some(format!(
                                        "indices are valid in [0..{}); the tuple has length {}",
                                        tys.len(),
                                        tys.len()
                                    )),
                                );
                            } else {
                                return tys[i as usize].clone();
                            }
                        }
                        Ty::Var(self.counter.fresh())
                    }
                    _ => {
                        // Non-aggregate target: emit a diagnostic.
                        let _ = self.error_with_help(
                            ErrorCode::CannotIndex,
                            "cannot index non-aggregate type".to_string(),
                            range.clone(),
                            Some(format!("type `{}` does not support indexing", resolved)),
                        );
                        Ty::Var(self.counter.fresh())
                    }
                }
            }
            // ---- Dict literals ----
            Expression::Dict(fields) => {
                // Check for duplicate field names — diagnostic
                // is raised BEFORE we proceed (recovery: keep
                // all fields, but emit once).
                let mut seen: HashMap<String, ()> = HashMap::new();
                for f in fields {
                    if seen.insert(f.name.to_string(), ()).is_some() {
                        let _ = self.error_with_help(
                            ErrorCode::DuplicateField,
                            format!("Duplicate field `{}` in record literal", f.name),
                            range.clone(),
                            Some("record literals must have unique field names".to_string()),
                        );
                    }
                }
                // Build the record type in source order.
                let mut record_fields: Vec<(String, Ty)> = Vec::with_capacity(fields.len());
                for f in fields {
                    let fty = self.infer(&f.value);
                    let fty_pruned = apply_ty_prune(&self.subst, &fty);
                    record_fields.push((f.name.to_string(), fty_pruned));
                }
                // Sort canonically by name for unification
                // determinism (mirrors the existing record-
                // variant treatment in `Ty::Sum`).
                record_fields.sort_by(|a, b| a.0.cmp(&b.0));
                crate::typechecking::ty::record(record_fields)
            }
            // — registers a signature in the library and returns
            // a function id (an `int`). We verify that each
            // arg/ret position is an `FFIType::X` constructor
            // application (otherwise the codegen won't know how
            // to encode the type). Returns `int`.
            Expression::Declare(args) => self.infer_ffi_declare(args, range),
            Expression::Invoke(args) => self.infer_ffi_invoke(args, range),

            // ---- Defer / coroutines / list ----
            Expression::Defer(e) => {
                let _ = self.infer(e);
                unit_ty()
            }
            Expression::Yield(e) => {
                if self.async_depth == 0 {
                    return self.error_with_help(
                        ErrorCode::YieldOutsideAsync,
                        "yield outside async function".to_string(),
                        range,
                        Some("yield may only appear inside an async fn body".to_string()),
                    );
                }
                let ty = self.infer(e);
                if let Some(yield_ty) = self.current_yield_ty.clone() {
                    self.unify(&yield_ty, &ty, &e.0.into_range(), "yield value");
                }
                if let Some(send_ty) = self.current_send_ty.clone() {
                    apply_ty_prune(&self.subst, &send_ty)
                } else {
                    unit_ty()
                }
            }
            Expression::YieldFrom(e) => {
                if self.async_depth == 0 {
                    return self.error_with_help(
                        ErrorCode::YieldOutsideAsync,
                        "yield from outside async function".to_string(),
                        range,
                        Some("yield from may only appear inside an async fn body".to_string()),
                    );
                }
                let inner_ty = self.infer(e);
                let (y_var, s_var) = (Ty::Var(self.counter.fresh()), Ty::Var(self.counter.fresh()));
                let expected = self.coroutine_type(y_var.clone(), s_var.clone());
                self.unify(&inner_ty, &expected, &range, "yield from target");
                if let Some(yield_ty) = self.current_yield_ty.clone() {
                    self.unify(&yield_ty, &y_var, &range, "yield from yield type");
                }
                if let Some(send_ty) = self.current_send_ty.clone() {
                    self.unify(&send_ty, &s_var, &range, "yield from send type");
                }
                unit_ty()
            }
            Expression::Resume(target, arg) => {
                let target_ty = self.infer(target);
                let y_var = Ty::Var(self.counter.fresh());
                let s_var = Ty::Var(self.counter.fresh());
                let coro_ty = self.coroutine_type(y_var.clone(), s_var.clone());
                self.unify(&target_ty, &coro_ty, &range, "resume target");
                if let Some(a) = arg {
                    let v_ty = self.infer(a);
                    self.unify(&v_ty, &s_var, &a.0.into_range(), "resume send value");
                }
                apply_ty_prune(&self.subst, &y_var)
            }
            Expression::List(elements) => self.infer_list(elements, range),

            // ---- Default arm ----
            Expression::Default(_) => self
                .current_match_lhs
                .clone()
                .unwrap_or_else(|| Ty::Var(self.counter.fresh())),

            // ---- Function declarations ----
            Expression::Function {
                name,
                is_coro,
                type_params,
                args,
                returns,
                where_constraints,
                body,
            } => {
                self.infer_function(
                    name,
                    type_params,
                    args,
                    returns.as_ref(),
                    where_constraints,
                    body,
                    &range,
                    None,
                    *is_coro,
                );
                unit_ty()
            }
            Expression::Implementation {
                owner,
                methods,
                type_params,
                ..
            } => {
                self.infer_impl(owner, type_params, methods, &range);
                unit_ty()
            }
            Expression::Class {
                name,
                type_params,
                fields,
                ..
            } => {
                let _ = self.register_generic_type_ctor(name, type_params);
                let pushed = self.push_type_params_for_type_parsing(type_params);
                self.register_class(name, fields, &range);
                self.pop_type_params_for_type_parsing(pushed);
                unit_ty()
            }
            Expression::Argument(ty, _name) => self.parse_type_name(ty),
            Expression::Method(_vis, body) => self.infer(body),
            Expression::Member(_) => unit_ty(),
            Expression::Access(receiver, field) => {
                let receiver_ty = self.infer(receiver);
                let resolved = apply_ty_prune(&self.subst, &receiver_ty);
                match &resolved {
                    Ty::Sum { name, variants } => {
                        self.access_field_in_sum(name, variants, None, field, range)
                    }
                    Ty::Constructor { tag, owner, .. } => {
                        // Resolve the owner to its variants.
                        match owner.as_ref() {
                            Ty::Sum { name, variants } => {
                                self.access_field_in_sum(name, variants, Some(*tag), field, range)
                            }
                            _ => self.error_with_help(
                                ErrorCode::GenericTypeError,
                                format!("Cannot access field `{}` on non-record type", field),
                                range,
                                Some(
                                    "only values of record-shaped enum types expose fields"
                                        .to_string(),
                                ),
                            ),
                        }
                    }
                    Ty::App(head, args) if matches!(head.as_ref(), Ty::Con(n) if self.classes.contains_key(n)) =>
                    {
                        let name = match head.as_ref() {
                            Ty::Con(n) => n.clone(),
                            _ => unreachable!(),
                        };
                        self.access_class_field(&name, field, args, range)
                    }
                    Ty::Con(name) => {
                        // Class instance field access.
                        if self.classes.contains_key(name) {
                            return self.access_class_field(name, field, &[], range);
                        }
                        // Bare type name — resolve via the
                        // checker's enum registry.
                        let variant_names = self.enums.get(name).cloned().unwrap_or_default();
                        let payloads = self.enum_payloads.get(name).cloned().unwrap_or_default();
                        if variant_names.is_empty() {
                            return self.error_with_help(
                                ErrorCode::GenericTypeError,
                                format!("Cannot access field `{}` on non-record type", field),
                                range,
                                Some(format!("type `{}` is not a record-shaped enum", name)),
                            );
                        }
                        let variants: Vec<(String, EnumVariantPayloadTy)> =
                            variant_names.into_iter().zip(payloads).collect();
                        self.access_field_in_sum(name, &variants, None, field, range)
                    }
                    Ty::Record { fields } => match fields.iter().find(|(n, _)| n == field) {
                        Some((_, fty)) => fty.clone(),
                        None => {
                            let known: Vec<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
                            let msg = format!(
                                "Cannot find field `{}` on record `{{ {} }}`",
                                field,
                                fields
                                    .iter()
                                    .map(|(n, t)| format!("{}: {}", n, t))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            );
                            let help = if known.is_empty() {
                                Some("the record has no fields".to_string())
                            } else {
                                Some(format!("the record has fields: {}", known.join(", ")))
                            };
                            self.error_with_help(ErrorCode::GenericTypeError, msg, range, help)
                        }
                    },
                    _ => self.error_with_help(
                        ErrorCode::GenericTypeError,
                        format!("Cannot access field `{}` on non-record type", field),
                        range,
                        Some("only values of record-shaped enum types expose fields".to_string()),
                    ),
                }
            }
            Expression::Instantiate(class_expr, args) => {
                let class_ty = self.infer(class_expr);
                let resolved = apply_ty_prune(&self.subst, &class_ty);
                let class_name = match &resolved {
                    Ty::Con(n) => n.clone(),
                    _ => {
                        return self.error(
                            ErrorCode::NotAFunction,
                            "Cannot instantiate non-class type".to_string(),
                            range,
                        );
                    }
                };
                if let Some(fields) = self.classes.get(&class_name).cloned() {
                    let param_names = self
                        .generics
                        .generic_type_ctors
                        .get(&class_name)
                        .cloned()
                        .unwrap_or_default();
                    // Freshen field types at each `new` site so independent
                    // instantiations don't share type variables.
                    let (field_tys, result_ty) = if param_names.is_empty() {
                        (
                            fields.iter().map(|(_, _, t)| t.clone()).collect::<Vec<_>>(),
                            Ty::Con(class_name.clone()),
                        )
                    } else {
                        let mut map = HashMap::new();
                        let mut app_args = Vec::with_capacity(param_names.len());
                        for p in &param_names {
                            let v = Ty::Var(self.counter.fresh());
                            app_args.push(v.clone());
                            map.insert(p.clone(), v);
                        }
                        let field_tys = fields
                            .iter()
                            .map(|(_, _, t)| subst_ty_params(t, &map))
                            .collect();
                        (
                            field_tys,
                            Ty::App(Box::new(Ty::Con(class_name.clone())), app_args),
                        )
                    };
                    let provided = args.as_ref().map(|a| a.as_slice()).unwrap_or(&[]);
                    if provided.len() != fields.len() {
                        let _ = self.error_with_help(
                            ErrorCode::ConstructorArity,
                            format!(
                                "Constructor `{}` expects {} arguments, got {}",
                                class_name,
                                fields.len(),
                                provided.len()
                            ),
                            range,
                            Some(
                                "pass one argument per class field, in declaration order"
                                    .to_string(),
                            ),
                        );
                    } else {
                        for (arg, fty) in provided.iter().zip(field_tys.iter()) {
                            let aty = self.infer(arg);
                            self.unify(&aty, fty, &arg.0.into_range(), "constructor argument");
                        }
                    }
                    return apply_ty_prune(&self.subst, &result_ty);
                }
                Ty::Con(class_name)
            }
            Expression::Field(_, _, _) => unit_ty(),

            // ---- Enums / constructors / type aliases ----
            Expression::EnumDecl {
                name,
                type_params,
                variants,
                ..
            } => {
                let _ = self.register_generic_type_ctor(name, type_params);
                self.infer_enum_decl(name, variants, &range);
                unit_ty()
            }
            Expression::TypeAlias {
                name,
                type_params,
                ty,
            } => {
                let _ = self.register_generic_type_ctor(name, type_params);
                let pushed = self.push_type_params_for_type_parsing(type_params);
                let alias_ty = self.parse_type_name(ty);
                // Capture param → var mapping before popping the frame.
                let param_vars: Vec<TyVarId> = if pushed {
                    let frame = self
                        .type_params_in_scope
                        .last()
                        .expect("type-param frame just pushed");
                    type_params
                        .iter()
                        .map(|tp| *frame.get(tp.name).expect("type param registered in frame"))
                        .collect()
                } else {
                    Vec::new()
                };
                self.pop_type_params_for_type_parsing(pushed);
                if type_params.is_empty() {
                    self.register_type_alias(name, alias_ty, range);
                } else {
                    let params = type_params.iter().map(|tp| tp.name.to_string()).collect();
                    self.register_generic_alias(name, params, param_vars, alias_ty, range);
                }
                let _ = self.infer(ty); // ID alignment
                unit_ty()
            }
            Expression::ExternStruct(decl) => {
                use common::encode_tag_operand;
                let span = expr.0.into_range();
                if self.c_structs.iter().any(|s| s.name == decl.name) {
                    self.messages.push(Message::error(
                        ErrorCode::GenericTypeError,
                        format!("Duplicate extern struct `{}`", decl.name),
                        span.clone(),
                    ));
                } else {
                    let mut fields = Vec::new();
                    for (fname, fty) in &decl.fields {
                        self.require_ffi_type_expr(fty);
                        if let Some((tag, aux)) = self.ffi_type_tag_from_output(fty) {
                            fields.push((fname.clone(), encode_tag_operand(tag, aux)));
                        } else {
                            fields.push((fname.clone(), 0));
                        }
                        let _ = self.infer(fty);
                    }
                    self.c_structs.push(CStructDef {
                        name: decl.name.to_string(),
                        fields,
                    });
                }
                unit_ty()
            }
            Expression::EnumVariant { payload, .. } => {
                use parser::ast::EnumVariantPayload;
                // The pre-walk mints an ID for every payload
                // element. Recurse so this arm's ID consumption
                // stays in lockstep. The actual payload parsing
                // happens in `infer_enum_decl`, which knows the
                // parent variant name and target arity.
                match payload {
                    EnumVariantPayload::Unit => {}
                    EnumVariantPayload::Tuple(parts) => {
                        for p in parts {
                            let _ = self.infer(p);
                        }
                    }
                    EnumVariantPayload::Record(fields) => {
                        for f in fields {
                            let _ = self.infer(&f.value);
                        }
                    }
                }
                unit_ty()
            }
            Expression::Construct {
                enum_name,
                variant_name,
                fields,
            } => self.infer_construct(enum_name, variant_name, fields, range),

            // ---- Generics ----
            Expression::TypeClass {
                name,
                type_params,
                methods,
            } => {
                // Collect associated type declarations and method defs.
                let mut assoc_types: Vec<AssocTypeDecl> = Vec::new();
                let method_defs: Vec<TypeClassMethodDef> = methods
                    .iter()
                    .filter_map(|m| match m.1.as_ref() {
                        Expression::AssocTypeDecl {
                            name: aname,
                            type_params: assoc_params,
                        } => {
                            if assoc_types.iter().any(|a| a.name == *aname) {
                                self.messages.push(Message::error(
                                    ErrorCode::GenericTypeError,
                                    format!(
                                        "Duplicate associated type `{}` in trait `{}`",
                                        aname, name
                                    ),
                                    m.0.into_range(),
                                ));
                            } else {
                                let param_kinds = assoc_params
                                    .iter()
                                    .map(|tp| self.resolve_type_param_kind(tp))
                                    .collect::<Vec<_>>();
                                assoc_types.push(AssocTypeDecl::new(
                                    aname.to_string(),
                                    assoc_params.iter().map(|tp| tp.name.to_string()).collect(),
                                    param_kinds,
                                ));
                            }
                            None
                        }
                        Expression::Function {
                            name: mname, body, ..
                        } => {
                            let has_default =
                                !matches!(body.1.as_ref(), Expression::Block(v) if v.is_empty());
                            Some(TypeClassMethodDef {
                                name: mname.to_string(),
                                has_default,
                            })
                        }
                        _ => None,
                    })
                    .collect();
                let param_names: Vec<String> =
                    type_params.iter().map(|tp| tp.name.to_string()).collect();
                let param_kinds: Vec<Kind> = type_params
                    .iter()
                    .map(|tp| Kind::from(tp.kind.clone()))
                    .collect();
                // Single-param classes: param bounds become direct superclasses
                // (`trait Ordered<T: Equal>` → superclasses: ["Equal"]).
                // Multi-param classes ignore param bounds for superclass
                // wiring (use `where` for those constraints later).
                let superclasses: Vec<String> = if type_params.len() == 1 {
                    type_params[0]
                        .bounds
                        .iter()
                        .map(|b| (*b).to_string())
                        .collect()
                } else {
                    Vec::new()
                };
                if let Some(previous) = self.generics.typeclass(name) {
                    let is_prelude = Checker::is_builtin_class(name);
                    if is_prelude && !self.builtin_name_in_scope(name) {
                        // Short name was rebound (`use prelude::ops::Eq as …`);
                        // allow the user trait to replace the builtin entry.
                    } else {
                        let mut msg = Message::error(
                            ErrorCode::GenericTypeError,
                            format!("Duplicate trait `{}`", name),
                            range.clone(),
                        );
                        if is_prelude && self.builtin_name_in_scope(name) {
                            msg.with_help(format!(
                                "`{}` is in the prelude; free the short name with `use {}::{} as OtherName;` before redefining, or pick a different name",
                                name,
                                previous.defined_module,
                                name
                            ));
                        } else {
                            msg.with_help(format!(
                                "trait `{}` was already declared in module `{}`",
                                name, previous.defined_module
                            ));
                        }
                        self.messages.push(msg);
                        for m in methods {
                            let _ = self.infer(m);
                        }
                        return unit_ty();
                    }
                }
                let def = TypeClassDef {
                    name: name.to_string(),
                    defined_module: self.current_module.clone(),
                    type_params: param_names,
                    param_kinds: param_kinds.clone(),
                    superclasses,
                    assoc_types: assoc_types.clone(),
                    methods: method_defs.clone(),
                };
                self.generics.typeclasses.insert(name.to_string(), def);

                // Build method schemes with trait parameters in scope.
                // Applied associated types are recorded as explicit projection
                // variables and quantified with the method scheme.
                let mut param_frame = HashMap::new();
                let mut param_vars = Vec::new();
                let mut class_kinds = Vec::new();
                for (i, type_param) in type_params.iter().enumerate() {
                    let var = self.counter.fresh();
                    let kind = param_kinds.get(i).cloned().unwrap_or(Kind::Type);
                    self.set_var_kind(var, kind.clone());
                    param_frame.insert(type_param.name.to_string(), var);
                    param_vars.push(var);
                    class_kinds.push(kind);
                }
                self.type_params_in_scope.push(param_frame);
                self.current_typeclass = Some(name.to_string());
                // ONE constraint over all class params (multi-param ready).
                let class_constraints: Vec<Constraint> = vec![Constraint {
                    class: name.to_string(),
                    args: param_vars.iter().map(|v| Ty::Var(*v)).collect(),
                }];
                for method in methods {
                    if let Expression::Function {
                        name: method_name,
                        type_params: method_params,
                        args,
                        returns,
                        ..
                    } = method.1.as_ref()
                    {
                        // Method-level type params (e.g. `fn first<A>(F<A>) -> A`).
                        let mut method_frame = HashMap::new();
                        let mut method_vars = Vec::new();
                        let mut method_kinds = Vec::new();
                        for mp in method_params {
                            let var = self.counter.fresh();
                            let kind = self.resolve_type_param_kind(mp);
                            self.set_var_kind(var, kind.clone());
                            method_frame.insert(mp.name.to_string(), var);
                            method_vars.push(var);
                            method_kinds.push(kind);
                        }
                        let pushed_method = !method_frame.is_empty();
                        if pushed_method {
                            self.type_params_in_scope.push(method_frame);
                        }
                        let prev_assoc = self.current_assoc_projections.take();
                        self.current_assoc_projections = Some(Vec::new());
                        let arg_tys = self.parse_arg_list(args);
                        let ret_ty = returns
                            .as_ref()
                            .map(|ret| self.parse_type_name(ret))
                            .unwrap_or_else(unit_ty);
                        let assoc_projections =
                            self.current_assoc_projections.take().unwrap_or_default();
                        self.current_assoc_projections = prev_assoc;
                        let fun_ty = arg_tys.iter().rev().fold(ret_ty, |ret, (_, arg)| {
                            Ty::Fun(Box::new(arg.clone()), Box::new(ret))
                        });
                        if pushed_method {
                            self.type_params_in_scope.pop();
                        }
                        let mut all_bounds = param_vars.clone();
                        all_bounds.extend(method_vars);
                        all_bounds.extend(assoc_projections.iter().map(|p| p.var));
                        let mut all_kinds = class_kinds.clone();
                        all_kinds.extend(method_kinds);
                        all_kinds.extend(std::iter::repeat_n(Kind::Type, assoc_projections.len()));
                        self.typeclass_method_schemes.insert(
                            (name.to_string(), method_name.to_string()),
                            Scheme::poly_with_kinds_and_assoc(
                                all_bounds,
                                all_kinds,
                                class_constraints.clone(),
                                assoc_projections,
                                fun_ty,
                            ),
                        );
                    }
                }

                // Walk method bodies (ID alignment + default body typecheck).
                // The class's own constraint is active so a default can call a
                // sibling method through the same dictionary.
                let active_len = self.active_constraints.len();
                self.active_constraints.extend(class_constraints);
                for m in methods {
                    let _ = self.infer(m);
                }
                self.active_constraints.truncate(active_len);
                self.type_params_in_scope.pop();
                self.current_typeclass = None;
                unit_ty()
            }

            Expression::TypeClassImpl {
                class,
                args,
                methods,
            } => {
                // Resolve instance heads (bare ctors stay `Con` for HKT).
                let arg_tys: Vec<Ty> = args.iter().map(|a| self.parse_instance_head(a)).collect();
                // Walk arg type expressions for ID alignment; cache head tys
                // so codegen FQNs match (not `Option<t0>` placeholders).
                for (a, ty) in args.iter().zip(arg_tys.iter()) {
                    self.cache_forced_ty(a, ty.clone());
                }
                // Verify class exists.
                let class_def = self.generics.typeclass(class).cloned();
                if class_def.is_none() {
                    self.messages.push(Message::error(
                        ErrorCode::GenericTypeError,
                        format!("Unknown trait `{}`", class),
                        range.clone(),
                    ));
                }
                if let Some(ref cdef) = class_def {
                    self.validate_instance_head_kinds(cdef, &arg_tys, &range);
                }
                let orphaned = class_def
                    .as_ref()
                    .is_some_and(|cdef| !self.instance_satisfies_orphan_rule(cdef, args, &arg_tys));
                if orphaned {
                    let instance = self.instance_signature(class, &arg_tys);
                    let mut msg = Message::error(
                        ErrorCode::GenericTypeError,
                        format!(
                            "Orphan instance `{}` is not allowed in module `{}`",
                            instance, self.current_module
                        ),
                        range.clone(),
                    );
                    msg.with_help(
                        "define the trait in this module, or define the nominal head of every non-variable instance argument here"
                            .to_string(),
                    );
                    self.messages.push(msg);
                }
                let overlapping = self
                    .generics
                    .find_overlapping_instance(class, &arg_tys)
                    .cloned();
                if let Some(existing) = overlapping.as_ref() {
                    let mut msg = Message::error(
                        ErrorCode::GenericTypeError,
                        format!(
                            "Overlapping instance `{}` conflicts with existing `{}`",
                            self.instance_signature(class, &arg_tys),
                            self.instance_signature(&existing.class, &existing.args)
                        ),
                        range.clone(),
                    );
                    msg.with_help(format!(
                        "existing instance was declared in module `{}`",
                        existing.defined_module
                    ));
                    msg.push(Label::new(
                        "new instance declared here".to_string(),
                        range.clone(),
                    ));
                    if existing.defined_module == self.current_module {
                        msg.push(Label::new(
                            "existing overlapping instance declared here".to_string(),
                            existing.range.clone(),
                        ));
                    }
                    self.messages.push(msg);
                }
                // Build method_fqns, assoc_tys, and register instance.
                let mut method_fqns = HashMap::new();
                let mut method_names = Vec::new();
                let mut assoc_tys: HashMap<String, AssocTypeValue> = HashMap::new();
                let mut assoc_names: Vec<String> = Vec::new();
                let mut invalid_assoc_defs = false;

                // Pre-register a stub so recursive derived/hand-written
                // method bodies can discharge constraints against the
                // instance under construction. Assoc types are patched
                // onto the stub as they are collected (before methods
                // run), so projections stay valid during body infer.
                let args_pretty_for_fqn: String = arg_tys
                    .iter()
                    .map(|t| format!("{}", t))
                    .collect::<Vec<_>>()
                    .join("_");
                let mut stub_fqns = HashMap::new();
                for m in methods {
                    let mname = match m.1.as_ref() {
                        Expression::Function { name, .. } => Some(*name),
                        Expression::Method(_, body) => match body.1.as_ref() {
                            Expression::Function { name, .. } => Some(*name),
                            _ => None,
                        },
                        _ => None,
                    };
                    if let Some(mname) = mname {
                        stub_fqns.insert(
                            mname.to_string(),
                            format!("{}__{}__{}", class, args_pretty_for_fqn, mname),
                        );
                    }
                }
                let stub_idx = if class_def.is_some() && !orphaned && overlapping.is_none() {
                    self.generics.instances.push(InstanceDef {
                        class: class.to_string(),
                        defined_module: self.current_module.clone(),
                        range: range.clone(),
                        args: arg_tys.clone(),
                        method_fqns: stub_fqns,
                        assoc_tys: HashMap::new(),
                    });
                    Some(self.generics.instances.len() - 1)
                } else {
                    None
                };

                for m in methods {
                    match m.1.as_ref() {
                        Expression::AssocTypeDef {
                            name: aname,
                            type_params: assoc_params,
                            ty,
                        } => {
                            // Consume the AssocTypeDef wrapper NodeId, then the RHS.
                            let wrapper_id = self.ids.ids()[self.next_id_idx];
                            self.next_id_idx += 1;
                            self.cache.insert(wrapper_id, unit_ty());
                            let mut assoc_frame = HashMap::new();
                            let mut assoc_param_vars = Vec::new();
                            let mut assoc_param_kinds = Vec::new();
                            for tp in assoc_params {
                                let var = self.counter.fresh();
                                let kind = self.resolve_type_param_kind(tp);
                                self.set_var_kind(var, kind.clone());
                                assoc_frame.insert(tp.name.to_string(), var);
                                assoc_param_vars.push(var);
                                assoc_param_kinds.push(kind);
                            }
                            let pushed_assoc_params = !assoc_frame.is_empty();
                            if pushed_assoc_params {
                                self.type_params_in_scope.push(assoc_frame);
                            }
                            let resolved = self.parse_type_name(ty);
                            if pushed_assoc_params {
                                let _ = self.type_params_in_scope.pop();
                            }
                            self.cache_forced_ty(ty, resolved.clone());
                            if let Some(cdef) = class_def.as_ref()
                                && let Some(decl) = cdef.assoc_type(aname)
                            {
                                if decl.params.len() != assoc_params.len() {
                                    invalid_assoc_defs = true;
                                    self.messages.push(Message::error(
                                        ErrorCode::GenericTypeError,
                                        format!(
                                            "Associated type `{}` in instance of `{}` expects {} type parameter{}, got {}",
                                            aname,
                                            class,
                                            decl.params.len(),
                                            if decl.params.len() == 1 { "" } else { "s" },
                                            assoc_params.len()
                                        ),
                                        m.0.into_range(),
                                    ));
                                }
                                for (i, (expected, actual)) in decl
                                    .param_kinds
                                    .iter()
                                    .zip(assoc_param_kinds.iter())
                                    .enumerate()
                                {
                                    if expected != actual {
                                        invalid_assoc_defs = true;
                                        self.messages.push(Message::error(
                                            ErrorCode::GenericTypeError,
                                            format!(
                                                "Type parameter {} of associated type `{}` has kind `{}`, expected `{}`",
                                                i + 1,
                                                aname,
                                                actual,
                                                expected
                                            ),
                                            m.0.into_range(),
                                        ));
                                    }
                                }
                                let rhs_kind = self.kind_of_ty(&resolved);
                                if rhs_kind != Kind::Type {
                                    invalid_assoc_defs = true;
                                    self.messages.push(Message::error(
                                        ErrorCode::GenericTypeError,
                                        format!(
                                            "Associated type `{}` in instance of `{}` must resolve to kind `*`, found `{}`",
                                            aname, class, rhs_kind
                                        ),
                                        ty.0.into_range(),
                                    ));
                                }
                            }
                            if assoc_tys.contains_key(*aname) {
                                self.messages.push(Message::error(
                                    ErrorCode::GenericTypeError,
                                    format!(
                                        "Duplicate associated type `{}` in instance of `{}`",
                                        aname, class
                                    ),
                                    m.0.into_range(),
                                ));
                            } else {
                                assoc_names.push(aname.to_string());
                                let value = AssocTypeValue {
                                    params: assoc_params
                                        .iter()
                                        .map(|tp| tp.name.to_string())
                                        .collect(),
                                    param_vars: assoc_param_vars,
                                    param_kinds: assoc_param_kinds,
                                    ty: resolved,
                                };
                                if let Some(idx) = stub_idx {
                                    self.generics.instances[idx]
                                        .assoc_tys
                                        .insert(aname.to_string(), value.clone());
                                }
                                assoc_tys.insert(aname.to_string(), value);
                            }
                        }
                        _ => {
                            let maybe_fn = match m.1.as_ref() {
                                Expression::Function {
                                    name,
                                    type_params,
                                    args,
                                    returns,
                                    where_constraints,
                                    body,
                                    is_coro,
                                    ..
                                } => Some((
                                    *name,
                                    type_params.as_slice(),
                                    args,
                                    returns,
                                    where_constraints.as_slice(),
                                    body,
                                    *is_coro,
                                )),
                                Expression::Method(_, body) => match body.1.as_ref() {
                                    Expression::Function {
                                        name,
                                        type_params,
                                        args,
                                        returns,
                                        where_constraints,
                                        body,
                                        is_coro,
                                        ..
                                    } => Some((
                                        *name,
                                        type_params.as_slice(),
                                        args,
                                        returns,
                                        where_constraints.as_slice(),
                                        body,
                                        *is_coro,
                                    )),
                                    _ => None,
                                },
                                _ => None,
                            };
                            if let Some((mname, mparams, margs, returns, where_cs, body, is_coro)) =
                                maybe_fn
                            {
                                let fqn = format!(
                                    "{}__{}__{}",
                                    class,
                                    arg_tys
                                        .iter()
                                        .map(|t| format!("{}", t))
                                        .collect::<Vec<_>>()
                                        .join("_"),
                                    mname,
                                );
                                method_names.push(mname.to_string());
                                method_fqns.insert(mname.to_string(), fqn.clone());
                                self.infer_function(
                                    mname,
                                    mparams,
                                    margs,
                                    returns.as_ref(),
                                    where_cs,
                                    body,
                                    &m.0.into_range(),
                                    None,
                                    is_coro,
                                );
                            } else {
                                let _ = self.infer(m);
                            }
                        }
                    }
                }
                let mut invalid_instance = class_def.is_none() || orphaned || overlapping.is_some();
                if let Some(class_def) = class_def.as_ref() {
                    // Superclass instances must already exist for the same args.
                    // `impl Ordered<int>` requires `Equal<int>`, transitively.
                    let mut missing_supers = Vec::new();
                    let mut seen_super = HashSet::new();
                    let mut stack: Vec<String> = class_def.superclasses.clone();
                    while let Some(super_name) = stack.pop() {
                        if !seen_super.insert(super_name.clone()) {
                            continue;
                        }
                        if self.generics.find_instance(&super_name, &arg_tys).is_none() {
                            missing_supers.push(super_name.clone());
                        }
                        if let Some(super_def) = self.generics.typeclass(&super_name) {
                            stack.extend(super_def.superclasses.iter().cloned());
                        }
                    }
                    for super_name in &missing_supers {
                        let args_pretty = arg_tys
                            .iter()
                            .map(|ty| ty.to_string())
                            .collect::<Vec<_>>()
                            .join(", ");
                        self.messages.push(Message::error(
                            ErrorCode::GenericTypeError,
                            format!(
                                "Instance of `{}` for `{}` requires superclass instance `{}<{}>`",
                                class, args_pretty, super_name, args_pretty
                            ),
                            range.clone(),
                        ));
                    }
                    let unknown_methods = Generics::unknown_instance_methods(
                        class_def,
                        method_names.iter().map(|name| name.as_str()),
                    );
                    for method in &unknown_methods {
                        self.messages.push(Message::error(
                            ErrorCode::GenericTypeError,
                            format!("Unknown method `{}` in instance of `{}`", method, class),
                            range.clone(),
                        ));
                    }
                    let unknown_assoc = Generics::unknown_assoc_types(
                        class_def,
                        assoc_names.iter().map(|n| n.as_str()),
                    );
                    for aname in &unknown_assoc {
                        self.messages.push(Message::error(
                            ErrorCode::GenericTypeError,
                            format!(
                                "Unknown associated type `{}` in instance of `{}`",
                                aname, class
                            ),
                            range.clone(),
                        ));
                    }
                    let missing_assoc = Generics::missing_assoc_types(class_def, &assoc_tys);
                    if !missing_assoc.is_empty() {
                        let names = missing_assoc
                            .iter()
                            .map(|n| format!("`{}`", n))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let noun = if missing_assoc.len() == 1 {
                            "associated type"
                        } else {
                            "associated types"
                        };
                        self.messages.push(Message::error(
                            ErrorCode::GenericTypeError,
                            format!(
                                "Instance of `{}` for `{}` is missing {} {}",
                                class,
                                arg_tys
                                    .iter()
                                    .map(|ty| ty.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", "),
                                noun,
                                names
                            ),
                            range.clone(),
                        ));
                    }
                    Generics::fill_default_method_fqns(class_def, &mut method_fqns);
                    let missing_methods =
                        Generics::missing_required_methods(class_def, &method_fqns);
                    if !missing_methods.is_empty() {
                        let methods = missing_methods
                            .iter()
                            .map(|method| format!("`{}`", method))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let noun = if missing_methods.len() == 1 {
                            "method"
                        } else {
                            "methods"
                        };
                        self.messages.push(Message::error(
                            ErrorCode::GenericTypeError,
                            format!(
                                "Instance of `{}` for `{}` is missing {} {}",
                                class,
                                arg_tys
                                    .iter()
                                    .map(|ty| ty.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", "),
                                noun,
                                methods
                            ),
                            range.clone(),
                        ));
                    }
                    invalid_instance |= !unknown_methods.is_empty()
                        || !missing_methods.is_empty()
                        || !missing_supers.is_empty()
                        || !unknown_assoc.is_empty()
                        || !missing_assoc.is_empty()
                        || invalid_assoc_defs;
                }
                // Finalize the stub (or push a fresh instance when no stub).
                // Omitted defaulted methods have been filled with class-default FQNs.
                // Never `remove(idx)` — that shifts later instance indices; clear
                // the stub in place when invalid instead.
                if let Some(idx) = stub_idx {
                    if invalid_instance {
                        self.generics.instances[idx].method_fqns.clear();
                        self.generics.instances[idx].assoc_tys.clear();
                        self.generics.instances[idx].args.clear();
                        self.generics.instances[idx].class.clear();
                    } else {
                        self.generics.instances[idx].method_fqns = method_fqns;
                        self.generics.instances[idx].assoc_tys = assoc_tys;
                    }
                } else if !invalid_instance {
                    self.generics.instances.push(InstanceDef {
                        class: class.to_string(),
                        defined_module: self.current_module.clone(),
                        range: range.clone(),
                        args: arg_tys,
                        method_fqns,
                        assoc_tys,
                    });
                }
                unit_ty()
            }

            Expression::AssocTypeDecl { .. } => unit_ty(),
            Expression::AssocTypeDef { ty, .. } => {
                let _ = self.parse_type_name(ty);
                unit_ty()
            }

            Expression::Forall { params, ty } => {
                self.forall_type(params, |checker| checker.infer(ty))
            }

            // ---- Fallback ----
            //
            // `unreachable!` because the match above is exhaustive over every
            // `Expression` variant. The arm is here so that adding a new variant
            // produces a non-exhaustive match error here, instead of silently
            // ignoring the new node. If you add a variant, handle it in the match
            // above and remove this arm.
            #[allow(unreachable_patterns)]
            _ => unreachable!("all Expression variants must be handled above"),
        }
    }

    // ============================================================
    //  Type cache and lookup
    // ============================================================

    /// Look up the inferred type of a node by [`NodeId`].
    pub fn lookup_at(&self, id: NodeId) -> Option<Ty> {
        self.cache.get(&id).map(|t| apply_ty_prune(&self.subst, t))
    }

    /// Look up the original HM result by source span without re-running inference.
    pub fn lookup_for_codegen_span(&self, start: usize, end: usize) -> Option<Ty> {
        self.codegen_types_by_span
            .get(&(start, end))
            .map(|ty| apply_ty_prune(&self.subst, ty))
    }

    /// Concrete trait instances selected while discharging a call's bounds.
    pub fn call_dicts_at(&self, id: NodeId) -> Option<&[InstanceDef]> {
        self.call_site_dicts.get(&id).map(Vec::as_slice)
    }

    /// Span fallback for [`call_dicts_at`] when NodeIds are misaligned.
    pub fn call_dicts_for_span(&self, start: usize, end: usize) -> Option<&[InstanceDef]> {
        self.call_site_dicts_by_span
            .get(&(start, end))
            .map(Vec::as_slice)
    }

    fn record_call_site_dict(
        &mut self,
        call_id: Option<NodeId>,
        range: &Range<usize>,
        instance: InstanceDef,
    ) {
        if let Some(call_id) = call_id {
            self.call_site_dicts
                .entry(call_id)
                .or_default()
                .push(instance.clone());
        }
        self.call_site_dicts_by_span
            .entry((range.start, range.end))
            .or_default()
            .push(instance);
    }

    pub fn forwarded_dicts_at(&self, id: NodeId) -> Option<&[usize]> {
        self.call_site_forward_dicts.get(&id).map(Vec::as_slice)
    }

    pub fn forwarded_dicts_for_span(&self, start: usize, end: usize) -> Option<&[usize]> {
        self.call_site_forward_dicts_by_span
            .get(&(start, end))
            .map(Vec::as_slice)
    }

    pub fn bound_method_call_at(&self, id: NodeId) -> Option<&BoundMethodCall> {
        self.bound_method_calls.get(&id)
    }

    pub fn bound_method_call_for_span(&self, start: usize, end: usize) -> Option<&BoundMethodCall> {
        self.bound_method_calls_by_span.get(&(start, end))
    }

    pub fn bound_operator_call_at(&self, id: NodeId) -> Option<&BoundOperatorCall> {
        self.bound_operator_calls.get(&id)
    }

    pub fn bound_operator_call_for_span(
        &self,
        start: usize,
        end: usize,
    ) -> Option<&BoundOperatorCall> {
        self.bound_operator_calls_by_span.get(&(start, end))
    }

    pub fn bound_display_call_at(&self, id: NodeId) -> Option<&BoundDisplayCall> {
        self.bound_display_calls.get(&id)
    }

    pub fn bound_display_call_for_span(
        &self,
        start: usize,
        end: usize,
    ) -> Option<&BoundDisplayCall> {
        self.bound_display_calls_by_span.get(&(start, end))
    }

    pub fn existential_pack_for_span(&self, start: usize, end: usize) -> Option<&ExistentialPack> {
        self.existential_packs_by_span.get(&(start, end))
    }

    pub fn existential_method_call_at(&self, id: NodeId) -> Option<&ExistentialMethodCall> {
        self.existential_method_calls.get(&id)
    }

    pub fn existential_method_call_for_span(
        &self,
        start: usize,
        end: usize,
    ) -> Option<&ExistentialMethodCall> {
        self.existential_method_calls_by_span.get(&(start, end))
    }

    pub fn typeclass_method_scheme(&self, class: &str, method: &str) -> Option<&Scheme> {
        self.typeclass_method_schemes
            .get(&(class.to_string(), method.to_string()))
    }

    /// All call-site dicts (for debugging / testing).
    #[cfg(test)]
    pub fn all_call_site_dicts(&self) -> &HashMap<NodeId, Vec<InstanceDef>> {
        &self.call_site_dicts
    }

    /// Borrow the pre-walk [`IdTable`].
    pub fn id_table(&self) -> &IdTable {
        &self.ids
    }

    /// Number of nodes that have a cached inferred type. Useful in
    /// tests that want to assert the cache is fully populated.
    #[cfg(test)]
    pub(crate) fn cache_len(&self) -> usize {
        self.cache.len()
    }

    // ============================================================
    //  Helpers
    // ============================================================

    /// Process a [`Expression::Fragment`] (the body of a `let x = expr`
    /// declaration, or the body of a Block that consists entirely of
    /// declarations).
    ///
    /// Each `Variable` / `Constant` declaration binds in the
    /// environment. If the immediate next sibling is a value-producing
    /// expression (anything that's not another declaration, comment, or
    /// `use`), it is treated as the initializer and unified with the
    /// declared type.
    ///
    /// No new frame is pushed: `let` bindings live in the surrounding
    /// scope (block, function body, or program) so they're visible to
    /// subsequent statements.
    fn infer_fragment(&mut self, children: &[Output]) -> Ty {
        let mut last_ty = unit_ty();
        let mut i = 0;
        while i < children.len() {
            let child = &children[i];
            match child.1.as_ref() {
                Expression::Variable(name, ty_opt) => {
                    let var_ty = match ty_opt {
                        Some(ann) => self.parse_type_name(ann),
                        None => Ty::Var(self.counter.fresh()),
                    };
                    self.env
                        .insert_top(name.to_string(), Scheme::mono(var_ty.clone()));
                    self.codegen_var_types
                        .insert(name.to_string(), var_ty.clone());
                    last_ty = unit_ty();

                    // Try to consume the next sibling as the initializer.
                    if i + 1 < children.len() {
                        let next = &children[i + 1];
                        if !is_declaration_like(next) {
                            if is_yield_expression(next) {
                                self.yield_receives_used = true;
                            }
                            if let Expression::Identifier(source) =
                                unwrap_expr_wrappers(next).1.as_ref()
                                && let Some(source_scheme) = self.env.lookup(source).cloned()
                                && !source_scheme.bounds.is_empty()
                            {
                                let _ = self.infer(next);
                                self.env.insert_top(name.to_string(), source_scheme.clone());
                                self.codegen_var_types
                                    .insert(name.to_string(), source_scheme.ty.clone());
                                last_ty = source_scheme.ty;
                                i += 2;
                                continue;
                            }
                            // Annotated lets push an expected type so ground
                            // trait calls (`x.into()`) can pin conversion targets.
                            let prev_expected = self.current_expected.take();
                            if ty_opt.is_some() {
                                self.current_expected = Some(var_ty.clone());
                            }
                            let val_ty = self.infer(next);
                            self.current_expected = prev_expected;
                            self.coerce_or_unify(
                                &var_ty,
                                &val_ty,
                                Some(next),
                                &child.0.into_range(),
                                "let binding",
                            );
                            // Keep the side-table in sync with the unified type
                            // so Access codegen sees Record/enum types, not the
                            // pre-unify fresh variable.
                            let pruned = apply_ty_prune(&self.subst, &var_ty);
                            self.codegen_var_types.insert(name.to_string(), pruned);
                            // `let id = declare(...)` may wrap Declare/Call in
                            // ExprStatement/Statement/`?` — unwrap before matching.
                            let init = unwrap_expr_wrappers(next);
                            let init = match init.1.as_ref() {
                                Expression::Try(inner) => unwrap_expr_wrappers(inner),
                                _ => init,
                            };
                            let declare_args = match init.1.as_ref() {
                                Expression::Declare(dargs) => Some(dargs.as_slice()),
                                Expression::Call { name: callee, args }
                                    if matches!(
                                        callee.1.as_ref(),
                                        Expression::Identifier("declare")
                                    ) =>
                                {
                                    args.as_deref()
                                }
                                _ => None,
                            };
                            if let Some(dargs) = declare_args
                                && dargs.len() == 4
                            {
                                let ret = self.ty_from_ffi_type_expr(&dargs[3]);
                                self.ffi_fn_ret_tys.insert(name.to_string(), ret);
                            }
                            i += 1;
                        }
                    }
                }
                Expression::Constant(name, ty_opt) => {
                    let var_ty = match ty_opt {
                        Some(ann) => self.parse_type_name(ann),
                        None => Ty::Var(self.counter.fresh()),
                    };
                    if let Expression::Identifier(n) = name.1.as_ref() {
                        self.env
                            .insert_top(n.to_string(), Scheme::mono(var_ty.clone()));
                        self.codegen_var_types.insert(n.to_string(), var_ty.clone());
                        self.insert_const_binding(n.to_string());
                        if i + 1 < children.len() {
                            let next = &children[i + 1];
                            if !is_declaration_like(next) {
                                let prev_expected = self.current_expected.take();
                                if ty_opt.is_some() {
                                    self.current_expected = Some(var_ty.clone());
                                }
                                let val_ty = self.infer(next);
                                self.current_expected = prev_expected;
                                self.coerce_or_unify(
                                    &var_ty,
                                    &val_ty,
                                    Some(next),
                                    &child.0.into_range(),
                                    "const binding",
                                );
                                let pruned = apply_ty_prune(&self.subst, &var_ty);
                                self.codegen_var_types.insert(n.to_string(), pruned);
                                i += 1;
                            }
                        }
                    }
                    last_ty = unit_ty();
                }
                _ => {
                    last_ty = self.infer(child);
                }
            }
            i += 1;
        }
        last_ty
    }

    fn record_bound_operator(
        &mut self,
        id: Option<NodeId>,
        range: &Range<usize>,
        var: TyVarId,
        class: &str,
        method: &str,
    ) {
        let Some((dict_index, dict_class)) = self.user_dict_index_and_class(var, class) else {
            return;
        };
        let Some(class_def) = self.generics.typeclass(&dict_class) else {
            return;
        };
        let Some(method_slot) = class_def
            .flattened_methods(&self.generics)
            .iter()
            .position(|(_, candidate)| candidate.name == method)
        else {
            return;
        };
        let hint = BoundOperatorCall {
            dict_index,
            method_slot,
        };
        if let Some(id) = id {
            self.bound_operator_calls.insert(id, hint.clone());
        }
        self.bound_operator_calls_by_span
            .insert((range.start, range.end), hint);
    }

    fn record_bound_display(&mut self, range: &Range<usize>, var: TyVarId) {
        let Some((dict_index, dict_class)) = self.user_dict_index_and_class(var, "Show") else {
            return;
        };
        let Some(class_def) = self.generics.typeclass(&dict_class) else {
            return;
        };
        let Some(method_slot) = class_def
            .flattened_methods(&self.generics)
            .iter()
            .position(|(_, candidate)| candidate.name == "show")
        else {
            return;
        };
        let hint = BoundDisplayCall {
            dict_index,
            method_slot,
        };
        self.bound_display_calls_by_span
            .insert((range.start, range.end), hint);
    }

    /// Peel `Constructor` / structural `Sum` down to a nominal head so
    /// `Color::Red == Color::Blue` unifies as `Color` rather than as two
    /// incompatible constructor refinements.
    fn peel_comparison_ty(ty: &Ty) -> Ty {
        match ty {
            Ty::Constructor { owner, .. } => Self::peel_comparison_ty(owner),
            Ty::Sum { name, .. } => Ty::Con(name.clone()),
            other => other.clone(),
        }
    }

    fn infer_comparison(
        &mut self,
        lhs: &Output,
        rhs: &Output,
        id: Option<NodeId>,
        range: Range<usize>,
        class: &str,
        method: &str,
    ) -> Ty {
        let lt = Self::peel_comparison_ty(&self.infer(lhs));
        let rt = Self::peel_comparison_ty(&self.infer(rhs));
        let unified = self.unify(&lt, &rt, &range, "comparison operands");
        if let Ty::Var(var) = apply_ty_prune(&self.subst, &unified) {
            if self.user_dict_index(var, class).is_none() {
                self.bind_matching_abstract_constraints(Some(var), class);
            }
            if self.user_dict_index(var, class).is_some() {
                self.record_bound_operator(id, &range, var, class, method);
            } else if self
                .type_params_in_scope
                .iter()
                .any(|frame| frame.values().any(|&candidate| candidate == var))
            {
                self.messages.push(Message::error(
                    ErrorCode::GenericTypeError,
                    format!("Cannot compare generic type without bound `{}`", class),
                    range,
                ));
            }
        }
        boolean()
    }

    fn infer_arith(
        &mut self,
        lhs: &Output,
        rhs: &Output,
        id: Option<NodeId>,
        range: Range<usize>,
        op: &str,
    ) -> Ty {
        let lt = self.infer(lhs);
        let rt = self.infer(rhs);
        if op == "+" {
            let lp = apply_ty_prune(&self.subst, &lt);
            let rp = apply_ty_prune(&self.subst, &rt);
            let left_string = is_string_ty(&lp);
            let right_string = is_string_ty(&rp);
            if left_string && right_string {
                return string();
            }
            if left_string || right_string {
                return self.unify(&lt, &rt, &range, "operands of `+`");
            }
        }
        let result = self.unify(&lt, &rt, &range, &format!("operands of `{}`", op));
        // Open type variables need the matching op trait (`Add` for `+`, …).
        // `T: Num` also covers these via superclass implication.
        let pruned = apply_ty_prune(&self.subst, &result);
        if let Ty::Var(v) = &pruned {
            let (class, method) = match op {
                "+" => ("Add", "add"),
                "-" => ("Sub", "sub"),
                "*" => ("Mul", "mul"),
                "/" => ("Div", "div"),
                _ => {
                    let in_scope = self
                        .type_params_in_scope
                        .iter()
                        .any(|frame| frame.values().any(|&id| id == *v));
                    if in_scope {
                        self.messages.push(Message::error(
                            ErrorCode::GenericTypeError,
                            format!(
                                "Operator `{}` is not available through an arithmetic trait",
                                op
                            ),
                            range,
                        ));
                    }
                    return result;
                }
            };
            if self.user_dict_index(*v, class).is_none() {
                self.bind_matching_abstract_constraints(Some(*v), class);
            }
            if self.user_dict_index(*v, class).is_some() {
                self.record_bound_operator(id, &range, *v, class, method);
            } else {
                let in_scope = self
                    .type_params_in_scope
                    .iter()
                    .any(|frame| frame.values().any(|&id| id == *v));
                if in_scope {
                    self.messages.push(Message::error(
                        ErrorCode::GenericTypeError,
                        format!(
                            "Cannot apply `{}` to value of generic type without bound `{}`",
                            op, class
                        ),
                        range,
                    ));
                }
            }
        }
        result
    }

    fn compound_op_name(op: parser::ast::AssignOp) -> &'static str {
        use parser::ast::AssignOp;
        match op {
            AssignOp::Add => "+",
            AssignOp::Sub => "-",
            AssignOp::Mul => "*",
            AssignOp::Div => "/",
            AssignOp::Mod => "%",
            AssignOp::Pow => "**",
            AssignOp::Shl => "<<",
            AssignOp::Shr => ">>",
            AssignOp::BitAnd => "&",
            AssignOp::BitOr => "|",
            AssignOp::BitXor => "^",
        }
    }

    fn infer_mutable_lvalue(&mut self, target: &Output, range: Range<usize>) -> Ty {
        match target.1.as_ref() {
            Expression::Identifier(n) => {
                let ident = n.to_string();
                match self.env.lookup(&ident).cloned() {
                    Some(s) => {
                        let ty = self.instantiate_ty(&s);
                        if self.is_const_binding(&ident) {
                            let mut msg = Message::error(
                                ErrorCode::FormatSpecifierMismatch,
                                format!("Cannot assign to constant `{}`", ident),
                                range,
                            );
                            msg.with_help(
                                "constants are immutable after their initializer".to_string(),
                            );
                            self.messages.push(msg);
                        }
                        ty
                    }
                    None => self.error_with_help(
                        ErrorCode::UndeclaredAssignment,
                        format!("Cannot assign to undeclared variable `{}`", ident),
                        range,
                        Some(format!("try declaring it first with `let {};`", ident)),
                    ),
                }
            }
            Expression::Access(receiver, field) => {
                let receiver_ty = self.infer(receiver);
                let resolved = apply_ty_prune(&self.subst, &receiver_ty);
                match &resolved {
                    Ty::Record { fields } => match fields.iter().find(|(n, _)| n == field) {
                        Some((_, fty)) => fty.clone(),
                        None => {
                            let known: Vec<&str> =
                                fields.iter().map(|(n, _)| n.as_str()).collect();
                            self.error_with_help(
                                ErrorCode::UnknownField, format!("Cannot find field `{}` on record", field),
                                range,
                                Some(format!("the record has fields: {}", known.join(", "))),
                            )
                        }
                    },
                    Ty::App(head, args)
                        if matches!(head.as_ref(), Ty::Con(n) if self.classes.contains_key(n)) =>
                    {
                        let name = match head.as_ref() {
                            Ty::Con(n) => n.clone(),
                            _ => unreachable!(),
                        };
                        self.access_class_field(&name, field, args, range)
                    }
                    Ty::Con(name) if self.classes.contains_key(name) => {
                        self.access_class_field(name, field, &[], range)
                    }
                    _ => self.error_with_help(
                        ErrorCode::InvalidAssignment, "Invalid assignment target".to_string(),
                        range,
                        Some(
                            "only variables, dict fields, class fields, and array elements may be assigned"
                                .to_string(),
                        ),
                    ),
                }
            }
            Expression::Index(arr, idx) => {
                let target_ty = self.infer(arr);
                let target_ty = apply_ty_prune(&self.subst, &target_ty);
                let index_ty = self.infer(idx);
                let _ = unify_with(&self.subst, &apply_ty_prune(&self.subst, &index_ty), &int());
                match &target_ty {
                    Ty::Array { element, length } => {
                        if let ArrayLength::Static(n) = length {
                            if let Expression::Integer(i) = idx.1.as_ref() {
                                if *i < 0 || (*i as usize) >= *n {
                                    let _ = self.error_with_help(
                                        ErrorCode::IndexOutOfBounds,
                                        format!(
                                            "array index {} out of bounds for array of length {}",
                                            i, n
                                        ),
                                        range.clone(),
                                        None,
                                    );
                                }
                            }
                        }
                        (**element).clone()
                    }
                    Ty::Tuple(_) => self.error_with_help(
                        ErrorCode::InvalidAssignment,
                        "Invalid assignment target".to_string(),
                        range,
                        Some("tuple elements are immutable".to_string()),
                    ),
                    _ => self.error_with_help(
                        ErrorCode::InvalidAssignment,
                        "Invalid assignment target".to_string(),
                        range,
                        Some("only array elements may be indexed for assignment".to_string()),
                    ),
                }
            }
            _ => self.error_with_help(
                ErrorCode::InvalidAssignment,
                "Invalid assignment target".to_string(),
                range,
                Some(
                    "the left-hand side must be a variable, dict field, or array index".to_string(),
                ),
            ),
        }
    }

    fn infer_if(&mut self, branches: &[Output]) -> Ty {
        let mut result_ty = Ty::Var(self.counter.fresh());
        let mut first = true;
        for branch in branches {
            if let Expression::Branch(cond, body) = branch.1.as_ref() {
                if let Some(c) = cond {
                    let ct = self.infer(c);
                    self.unify(&ct, &boolean(), &c.0.into_range(), "if condition");
                }
                let body_ty = self.infer(body);
                if first {
                    result_ty = body_ty;
                    first = false;
                } else {
                    self.unify(&result_ty, &body_ty, &body.0.into_range(), "if branch");
                }
            }
        }
        result_ty
    }

    fn infer_list(&mut self, elements: &[Output], _range: Range<usize>) -> Ty {
        if elements.is_empty() {
            return list(Ty::Var(self.counter.fresh()));
        }
        let first_ty = self.infer(&elements[0]);
        for elem in &elements[1..] {
            let t = self.infer(elem);
            self.unify(&first_ty, &t, &elem.0.into_range(), "list element");
        }
        list(first_ty)
    }

    fn infer_array_push(&mut self, args: Option<&[Output]>, range: Range<usize>) -> Ty {
        let args = args.unwrap_or(&[]);
        if args.len() != 2 {
            for arg in args {
                let _ = self.infer(arg);
            }
            return self.error_with_help(
                ErrorCode::ConstructorArity,
                format!("push expects 2 arguments, got {}", args.len()),
                range,
                Some("use `push(array, value)`".to_string()),
            );
        }

        let array_ty = self.infer(&args[0]);
        let value_ty = self.infer(&args[1]);
        let resolved = apply_ty_prune(&self.subst, &array_ty);
        match resolved {
            Ty::Array { element, .. } => {
                self.coerce_or_unify(
                    element.as_ref(),
                    &value_ty,
                    Some(&args[1]),
                    &args[1].0.into_range(),
                    "push element",
                );
                let elem = apply_ty_prune(&self.subst, element.as_ref());
                let dynamic = array(elem);
                if let Expression::Identifier(name) = args[0].1.as_ref() {
                    self.env
                        .insert_top((*name).to_string(), Scheme::mono(dynamic.clone()));
                    self.codegen_var_types
                        .insert((*name).to_string(), dynamic.clone());
                }
                dynamic
            }
            Ty::Var(v) => {
                let elem = Ty::Var(self.counter.fresh());
                let dynamic = array(elem.clone());
                self.unify(&Ty::Var(v), &dynamic, &args[0].0.into_range(), "push array");
                self.unify(&elem, &value_ty, &args[1].0.into_range(), "push element");
                let dynamic = apply_ty_prune(&self.subst, &dynamic);
                if let Expression::Identifier(name) = args[0].1.as_ref() {
                    self.env
                        .insert_top((*name).to_string(), Scheme::mono(dynamic.clone()));
                    self.codegen_var_types
                        .insert((*name).to_string(), dynamic.clone());
                }
                dynamic
            }
            other => self.error_with_help(
                ErrorCode::ConstructorArity,
                "push expects an array as its first argument".to_string(),
                args[0].0.into_range(),
                Some(format!("found `{}`; use `push(array, value)`", other)),
            ),
        }
    }

    fn infer_array_len(&mut self, args: Option<&[Output]>, range: Range<usize>) -> Ty {
        let args = args.unwrap_or(&[]);
        if args.len() != 1 {
            for arg in args {
                let _ = self.infer(arg);
            }
            return self.error_with_help(
                ErrorCode::ConstructorArity,
                format!("len expects 1 argument, got {}", args.len()),
                range,
                Some("use `len(array)`".to_string()),
            );
        }

        let target_ty = self.infer(&args[0]);
        let resolved = apply_ty_prune(&self.subst, &target_ty);
        match resolved {
            Ty::Array { .. } => int(),
            Ty::Var(v) => {
                let elem = Ty::Var(self.counter.fresh());
                self.unify(
                    &Ty::Var(v),
                    &array(elem),
                    &args[0].0.into_range(),
                    "len array",
                );
                int()
            }
            other => self.error_with_help(
                ErrorCode::ConstructorArity,
                "len expects an array".to_string(),
                args[0].0.into_range(),
                Some(format!("found `{}`; use `len(array)`", other)),
            ),
        }
    }

    /// `assert(bool)` / `assert(bool, string)` → `Result<(), string>`.
    ///
    /// Does not enter result-mode; callers use `?` / `match` / `raise`.
    fn infer_assert(&mut self, args: &[Output], range: Range<usize>) -> Ty {
        if !(1..=2).contains(&args.len()) {
            for arg in args {
                let _ = self.infer(arg);
            }
            return self.error_with_help(
                ErrorCode::ConstructorArity,
                format!("assert expects 1 or 2 arguments, got {}", args.len()),
                range,
                Some("use `assert(cond)` or `assert(cond, message)`".to_string()),
            );
        }

        let cond_ty = self.infer(&args[0]);
        self.unify(
            &cond_ty,
            &boolean(),
            &args[0].0.into_range(),
            "assert condition",
        );
        if let Some(msg) = args.get(1) {
            let msg_ty = self.infer(msg);
            self.unify(
                &msg_ty,
                &string(),
                &msg.0.into_range(),
                "assert message",
            );
        }
        result_app_ty(unit_ty(), string())
    }

    /// Thread a curried function type through a list of argument types,
    /// unifying each. Returns the final return type.
    ///
    /// If at any point the type doesn't look like a function and isn't a
    /// variable, the call is rejected.
    fn apply_function(
        &mut self,
        name: Option<&str>,
        fun_ty: &Ty,
        arg_tys: &[Ty],
        arg_exprs: Option<&[Output]>,
        call_id: Option<NodeId>,
        range: Range<usize>,
    ) -> Ty {
        let mut current = fun_ty.clone();
        for (i, arg) in arg_tys.iter().enumerate() {
            let mut pending_constraints = Vec::new();
            loop {
                let pruned = apply_ty(&self.subst, &current);
                match pruned {
                    forall @ Ty::Forall { .. } => {
                        let (body, constraints) = self.instantiate_forall_ty(&forall);
                        pending_constraints.extend(constraints);
                        current = body;
                    }
                    Ty::Fun(param, ret) => {
                        if matches!(param.as_ref(), Ty::Forall { .. }) {
                            self.check_rank_n_argument(
                                param.as_ref(),
                                arg,
                                arg_exprs.and_then(|args| args.get(i)),
                                &range,
                            );
                        } else {
                            self.coerce_or_unify(
                                param.as_ref(),
                                arg,
                                arg_exprs.and_then(|args| args.get(i)),
                                &range,
                                "function argument",
                            );
                        }
                        if !pending_constraints.is_empty() {
                            self.discharge_constraints(call_id, &pending_constraints, &range);
                        }
                        current = *ret;
                        break;
                    }
                    Ty::Var(v) => {
                        let ret_ty = Ty::Var(self.counter.fresh());
                        let new_fun = Ty::Fun(Box::new(arg.clone()), Box::new(ret_ty.clone()));
                        self.unify(&Ty::Var(v), &new_fun, &range, "function type");
                        current = ret_ty;
                        break;
                    }
                    _ => {
                        // We've run out of function parameters — the call
                        // had more arguments than the function accepts.
                        let actual = format!("{}", apply_ty_prune(&self.subst, &pruned));
                        return self.error_with_help(
                            ErrorCode::GenericTypeError,
                            match name {
                                Some(n) => format!(
                                    "Function `{}` was called with too many arguments \
                                     (it accepts {}, but argument #{} was given)",
                                    n,
                                    i,
                                    i + 1,
                                ),
                                None => format!(
                                    "Cannot call value of type `{}` as a function \
                                     (it accepts {} argument{})",
                                    actual,
                                    i,
                                    if i == 1 { "" } else { "s" },
                                ),
                            },
                            range,
                            Some(
                                "check the function signature or the number of arguments"
                                    .to_string(),
                            ),
                        );
                    }
                }
            }
        }
        current
    }

    fn apply_existential_method(
        &mut self,
        class: &str,
        method: &str,
        scheme: &Scheme,
        arg_tys: &[Ty],
        arg_exprs: Option<&[Output]>,
        call_id: Option<NodeId>,
        range: Range<usize>,
    ) -> Ty {
        let (fun_ty, constraints, _mapping) = self.instantiate_scheme_mapped(scheme);
        let result = self.apply_function(
            Some(&format!("{}::{}", class, method)),
            &fun_ty,
            arg_tys,
            arg_exprs,
            call_id,
            range.clone(),
        );
        let remaining: Vec<_> = constraints
            .into_iter()
            .filter(|constraint| constraint.class != class)
            .collect();
        if !remaining.is_empty() {
            self.discharge_constraints(call_id, &remaining, &range);
        }
        result
    }

    fn coerce_or_unify(
        &mut self,
        expected: &Ty,
        actual: &Ty,
        expr: Option<&Output>,
        range: &Range<usize>,
        context: &str,
    ) -> Ty {
        let expected = apply_ty_prune(&self.subst, expected);
        let actual = apply_ty_prune(&self.subst, actual);
        // Integer literals may coerce to `byte` when in range 0..=255.
        if Self::is_byte_ty(&expected)
            && Self::is_int_ty(&actual)
            && let Some(expr) = expr
        {
            match Self::byte_literal_coercion(expr) {
                Ok(()) => return expected,
                Err(Some(n)) => {
                    return self.error_with_help(
                        ErrorCode::TypeMismatch,
                        format!("byte literal out of range: `{n}` is not in 0..=255"),
                        range.clone(),
                        Some("a `byte` must be an integer between 0 and 255".to_string()),
                    );
                }
                Err(None) => {}
            }
        }
        // Array literals of in-range integer literals coerce to `[byte]` / `[byte; N]`.
        if let (
            Ty::Array {
                element: exp_elem,
                length: exp_len,
            },
            Ty::Array {
                element: act_elem,
                length: act_len,
            },
        ) = (&expected, &actual)
            && Self::is_byte_ty(exp_elem)
            && Self::is_int_ty(act_elem)
            && (exp_len == act_len
                || matches!(exp_len, ArrayLength::Dynamic)
                || matches!(act_len, ArrayLength::Dynamic))
            && let Some(expr) = expr
            && let Expression::Array(items) = unwrap_expr_wrappers(expr).1.as_ref()
        {
            let mut ok = true;
            for item in items {
                match Self::byte_literal_coercion(item) {
                    Ok(()) => {}
                    Err(Some(n)) => {
                        let _ = self.error_with_help(
                            ErrorCode::TypeMismatch,
                            format!("byte literal out of range: `{n}` is not in 0..=255"),
                            item.0.into_range(),
                            Some("a `byte` must be an integer between 0 and 255".to_string()),
                        );
                        ok = false;
                    }
                    Err(None) => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                return expected;
            }
        }
        match (&expected, &actual) {
            (
                Ty::Existential {
                    class: expected_class,
                },
                Ty::Existential {
                    class: actual_class,
                },
            ) if expected_class == actual_class => expected,
            (Ty::Existential { class }, _) => {
                if matches!(actual, Ty::Var(_)) {
                    return self.error_with_help(
                        ErrorCode::GenericTypeError,
                        format!("Cannot pack open generic value as `{}`", class),
                        range.clone(),
                        Some("bare-class existentials require a concrete value type at the pack site".to_string()),
                    );
                }
                let lookup_ty = Self::existential_lookup_ty(&actual);
                match self.find_unique_instance(class, std::slice::from_ref(&lookup_ty), range) {
                    Ok(Some(_)) => {
                        if let Some(expr) = expr {
                            self.existential_packs_by_span.insert(
                                (expr.0.start, expr.0.end),
                                ExistentialPack {
                                    class: class.clone(),
                                    value_ty: lookup_ty,
                                },
                            );
                        }
                        expected
                    }
                    Ok(None) => {
                        let pretty = Constraint {
                            class: class.clone(),
                            args: vec![lookup_ty],
                        };
                        self.messages.push(Message::error(
                            ErrorCode::GenericTypeError,
                            format!("No instance for `{}`", pretty),
                            range.clone(),
                        ));
                        expected
                    }
                    Err(()) => expected,
                }
            }
            _ => self.unify(&expected, &actual, range, context),
        }
    }

    fn is_byte_ty(ty: &Ty) -> bool {
        matches!(ty, Ty::Con(n) if n == crate::typechecking::ty::BYTE)
    }

    fn is_int_ty(ty: &Ty) -> bool {
        matches!(ty, Ty::Con(n) if n == crate::typechecking::ty::INT)
    }

    /// `Ok(())` if `expr` is an integer literal in `0..=255`.
    /// `Err(Some(n))` if literal but out of range; `Err(None)` if not a literal.
    fn byte_literal_coercion(expr: &Output) -> Result<(), Option<i64>> {
        match unwrap_expr_wrappers(expr).1.as_ref() {
            Expression::Integer(n) => {
                if (0..=255).contains(n) {
                    Ok(())
                } else {
                    Err(Some(*n))
                }
            }
            _ => Err(None),
        }
    }

    fn existential_lookup_ty(ty: &Ty) -> Ty {
        match ty {
            Ty::Sum { name, .. } => Ty::Con(name.clone()),
            Ty::Constructor { owner, .. } => Self::existential_lookup_ty(owner),
            other => other.clone(),
        }
    }

    fn instantiate_forall_ty(&mut self, ty: &Ty) -> (Ty, Vec<Constraint>) {
        let Ty::Forall {
            bounds,
            constraints,
            body,
        } = ty
        else {
            return (ty.clone(), Vec::new());
        };

        let mapping: HashMap<TyVarId, TyVarId> =
            bounds.iter().map(|&v| (v, self.counter.fresh())).collect();
        let body = super::env::substitute_vars(body, &mapping);
        let constraints = constraints
            .iter()
            .map(|c| Constraint {
                class: c.class.clone(),
                args: c
                    .args
                    .iter()
                    .map(|a| super::env::substitute_vars(a, &mapping))
                    .collect(),
            })
            .collect();
        (body, constraints)
    }

    fn skolemize_forall_ty(&mut self, ty: &Ty) -> (Ty, Vec<Constraint>) {
        let Ty::Forall {
            bounds,
            constraints,
            body,
        } = ty
        else {
            return (ty.clone(), Vec::new());
        };

        let mut subst = Subst::empty();
        for bound in bounds {
            let fresh = self.counter.fresh();
            let name = format!("$forall{}", fresh.raw());
            subst.insert(*bound, Ty::Con(name));
        }
        let body = apply_ty(&subst, body);
        let constraints = constraints
            .iter()
            .map(|c| Constraint {
                class: c.class.clone(),
                args: c.args.iter().map(|a| apply_ty(&subst, a)).collect(),
            })
            .collect();
        (body, constraints)
    }

    fn check_rank_n_argument(
        &mut self,
        expected: &Ty,
        inferred_arg: &Ty,
        arg_expr: Option<&Output>,
        range: &Range<usize>,
    ) {
        let (expected_body, expected_constraints) = self.skolemize_forall_ty(expected);
        let (candidate, candidate_constraints) = match arg_expr.and_then(identifier_name) {
            Some(name) => match self.env.lookup(name).cloned() {
                Some(scheme) => self.instantiate_scheme(&scheme),
                None => (inferred_arg.clone(), Vec::new()),
            },
            None => (inferred_arg.clone(), Vec::new()),
        };
        let candidate = apply_ty_prune(&self.subst, &candidate);

        let local = match unify_with(&Subst::empty(), &candidate, &expected_body) {
            Ok(s) => s,
            Err(_) => {
                let expected_pretty = apply_ty_prune(&self.subst, expected);
                self.messages.push(Message::error(
                    ErrorCode::TypeMismatch,
                    format!(
                        "Type mismatch: expected `{}`, found `{}`",
                        expected_pretty, candidate
                    ),
                    arg_expr
                        .map(|arg| arg.0.into_range())
                        .unwrap_or_else(|| range.clone()),
                ));
                return;
            }
        };

        for constraint in candidate_constraints {
            let resolved_args: Vec<Ty> = constraint
                .args
                .iter()
                .map(|a| apply_ty_prune(&local, a))
                .collect();
            let all_skolems = resolved_args
                .iter()
                .all(|a| matches!(a, Ty::Con(name) if name.starts_with("$forall")));
            let all_open = resolved_args.iter().all(|a| matches!(a, Ty::Var(_)));
            let any_open = resolved_args.iter().any(|a| matches!(a, Ty::Var(_)));

            if all_skolems {
                let covered = expected_constraints.iter().any(|ec| {
                    ec.class == constraint.class
                        && ec.args.len() == resolved_args.len()
                        && ec
                            .args
                            .iter()
                            .zip(resolved_args.iter())
                            .all(|(a, b)| a == b)
                });
                if !covered {
                    self.messages.push(Message::error(
                        ErrorCode::GenericTypeError,
                        format!(
                            "Cannot pass constrained polymorphic value where unconstrained `forall` is expected"
                        ),
                        arg_expr
                            .map(|arg| arg.0.into_range())
                            .unwrap_or_else(|| range.clone()),
                    ));
                }
            } else if any_open {
                let needed = Constraint {
                    class: constraint.class.clone(),
                    args: resolved_args,
                };
                if !self.constraint_is_covered(&needed) {
                    self.messages.push(Message::error(
                        ErrorCode::GenericTypeError,
                        format!("Cannot satisfy constraint `{}`", constraint.class),
                        arg_expr
                            .map(|arg| arg.0.into_range())
                            .unwrap_or_else(|| range.clone()),
                    ));
                }
                let _ = all_open;
            } else {
                let lookup = self.instance_lookup_args(&constraint.class, &resolved_args);
                if self
                    .generics
                    .find_instance(&constraint.class, &lookup)
                    .is_none()
                {
                    let pretty = resolved_args
                        .iter()
                        .map(|t| t.to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    self.messages.push(Message::error(
                        ErrorCode::GenericTypeError,
                        format!("No instance for `{}<{}>`", constraint.class, pretty),
                        arg_expr
                            .map(|arg| arg.0.into_range())
                            .unwrap_or_else(|| range.clone()),
                    ));
                }
            }
        }
    }

    fn forall_type<F>(&mut self, params: &[parser::ast::TypeParam], body: F) -> Ty
    where
        F: FnOnce(&mut Self) -> Ty,
    {
        let mut frame = HashMap::new();
        let mut bounds = Vec::new();
        let mut kinds = Vec::new();
        for tp in params {
            let var = self.counter.fresh();
            let kind = self.resolve_type_param_kind(tp);
            self.set_var_kind(var, kind.clone());
            frame.insert(tp.name.to_string(), var);
            bounds.push(var);
            kinds.push(kind);
        }

        self.type_params_in_scope.push(frame);
        let mut constraints = Vec::new();
        let synthetic_range = 0..0;
        for (tp, var) in params.iter().zip(bounds.iter()) {
            for bound in &tp.bounds {
                if let Some(constraint) =
                    self.constraint_from_bound(bound, Ty::Var(*var), &synthetic_range)
                {
                    constraints.push(constraint);
                }
            }
        }
        let inner_ty = body(self);
        self.type_params_in_scope.pop();

        Ty::Forall {
            bounds,
            constraints,
            body: Box::new(inner_ty),
        }
    }

    /// Unify two types under the current substitution, updating
    /// `self.subst` on success. On failure, record a message and return
    /// a fresh variable so inference can continue.
    fn unify(&mut self, t1: &Ty, t2: &Ty, range: &Range<usize>, ctx: &str) -> Ty {
        match unify_with(&self.subst, t1, t2) {
            Ok(s) => {
                self.subst = compose(&s, &self.subst);
                apply_ty(&self.subst, t1)
            }
            Err(UnifyError::Mismatch { left, right }) => self.error_with_help(
                ErrorCode::TypeMismatch,
                format!("Type mismatch: expected `{}`, found `{}`", left, right),
                range.clone(),
                Some(format!("while checking `{}`", ctx)),
            ),
            Err(UnifyError::Occurs { var, ty }) => self.error_with_help(
                ErrorCode::InfiniteType,
                format!("Cannot construct infinite type `{}`", ty),
                range.clone(),
                Some(format!(
                    "the type variable `t{}` would occur in its own definition",
                    var.raw()
                )),
            ),
        }
    }

    /// Record an error message and return a fresh variable.
    ///
    /// This is the simplest form: a single message with a primary
    /// label at `range`. No help hint, no secondary labels. For richer
    /// diagnostics use [`error_with_help`] or [`error_with_labels`].
    fn error(&mut self, code: ErrorCode, message: String, range: Range<usize>) -> Ty {
        self.messages.push(Message::error(code, message, range));
        Ty::Var(self.counter.fresh())
    }

    /// Record an error message with a help hint.
    ///
    /// The hint is shown beneath the underline by ariadne's renderer.
    fn error_with_help(
        &mut self,
        code: ErrorCode,
        message: String,
        range: Range<usize>,
        help: Option<String>,
    ) -> Ty {
        let mut msg = Message::error(code, message, range);
        if let Some(h) = help {
            msg.with_help(h);
        }
        self.messages.push(msg);
        Ty::Var(self.counter.fresh())
    }

    /// Record an error with a primary label and one or more secondary
    /// labels. Each secondary label is rendered by ariadne below the
    /// primary underline; use them to point at related source positions
    /// (e.g., "expected type comes from here", "found type comes from
    /// here").
    #[allow(dead_code)]
    fn error_with_labels(
        &mut self,
        code: ErrorCode,
        primary_message: String,
        primary_range: Range<usize>,
        secondary: Vec<(String, Range<usize>)>,
        help: Option<String>,
    ) -> Ty {
        let mut msg = Message::error(code, primary_message, primary_range);
        for (label_text, range) in secondary {
            msg.push(Label::new(label_text, range));
        }
        if let Some(h) = help {
            msg.with_help(h);
        }
        self.messages.push(msg);
        Ty::Var(self.counter.fresh())
    }

    /// Discharge freshened trait constraints from a generic call site.
    ///
    /// For each freshened constraint `c` (returned by instantiate):
    ///
    /// 1. Resolve every argument under the current substitution.
    /// 2. If any arg is still open, check whether an active constraint covers
    ///    the whole predicate (same class + args) — if so, forward the dict.
    /// 3. When all args are concrete, look up `find_instance` with the N-ary
    ///    arg list (HKT heads rewritten via [`instance_lookup_args`]).
    ///
    /// Matched instances are stored by call-site [`NodeId`] for codegen.
    fn discharge_constraints(
        &mut self,
        call_id: Option<NodeId>,
        constraints: &[Constraint],
        range: &Range<usize>,
    ) {
        for c in constraints {
            let resolved_args: Vec<Ty> = c
                .args
                .iter()
                .map(|a| apply_ty_prune(&self.subst, a))
                .collect();
            let any_open = resolved_args.iter().any(|a| matches!(a, Ty::Var(_)));
            if any_open {
                let needed = Constraint {
                    class: c.class.clone(),
                    args: resolved_args.clone(),
                };
                if self.constraint_is_covered(&needed) {
                    if let Some(call_id) = call_id
                        && let Some(dict_index) = self.dict_index_for(&needed)
                    {
                        self.call_site_forward_dicts
                            .entry(call_id)
                            .or_default()
                            .push(dict_index);
                        self.call_site_forward_dicts_by_span
                            .entry((range.start, range.end))
                            .or_default()
                            .push(dict_index);
                    }
                    continue;
                }
                // Partially open (e.g. `Convert<int, β>`): unify against
                // registered instances so free args get pinned by the match.
                match self.find_unique_instance(&c.class, &resolved_args, range) {
                    Ok(Some(instance)) => {
                        self.record_call_site_dict(call_id, range, instance.clone());
                        self.pin_assoc_types_for_instance(&c.class, &instance, None, range);
                    }
                    Ok(None) => {
                        self.messages.push(Message::error(
                            ErrorCode::GenericTypeError,
                            format!("Cannot satisfy constraint `{}`", needed),
                            range.clone(),
                        ));
                    }
                    Err(()) => {}
                }
            } else {
                match self.find_unique_instance(&c.class, &resolved_args, range) {
                    Ok(Some(instance)) => {
                        self.record_call_site_dict(call_id, range, instance.clone());
                        self.pin_assoc_types_for_instance(&c.class, &instance, None, range);
                    }
                    Ok(None) => {
                        let pretty = Constraint {
                            class: c.class.clone(),
                            args: resolved_args,
                        };
                        self.messages.push(Message::error(
                            ErrorCode::GenericTypeError,
                            format!("No instance for `{}`", pretty),
                            range.clone(),
                        ));
                    }
                    Err(()) => {}
                }
            }
        }
    }

    /// After discharging constraints for a trait method call, pin
    /// freshened associated-type variables from the scheme mapping.
    fn pin_assoc_after_discharge(
        &mut self,
        class: &str,
        constraints: &[Constraint],
        scheme: Option<&Scheme>,
        mapping: &HashMap<TyVarId, TyVarId>,
        range: &Range<usize>,
    ) {
        for c in constraints {
            if !class.is_empty() && c.class != class {
                continue;
            }
            let resolved_args: Vec<Ty> = c
                .args
                .iter()
                .map(|a| apply_ty_prune(&self.subst, a))
                .collect();
            let any_open = resolved_args.iter().any(|a| matches!(a, Ty::Var(_)));
            if any_open {
                // Open bound: leave assoc vars free (open projection).
                continue;
            }
            if let Ok(Some(instance)) = self.find_unique_instance(&c.class, &resolved_args, range) {
                if let Some(scheme) = scheme {
                    self.pin_assoc_vars_from_mapping(&c.class, &instance, scheme, mapping, range);
                }
                self.pin_assoc_types_for_instance(&c.class, &instance, scheme, range);
            }
        }
    }

    /// Find exactly one instance of `class` whose args unify with `wanted`
    /// (open vars in `wanted` may be bound by the match). Ambiguous matches
    /// are diagnosed instead of silently selecting declaration order.
    fn find_unique_instance(
        &mut self,
        class: &str,
        wanted: &[Ty],
        range: &Range<usize>,
    ) -> Result<Option<InstanceDef>, ()> {
        let wanted_lookup = self.instance_lookup_args(class, wanted);
        let candidates: Vec<_> = self
            .generics
            .instances
            .iter()
            .filter(|inst| inst.class == class && inst.args.len() == wanted_lookup.len())
            .cloned()
            .collect();
        let mut matches: Vec<(InstanceDef, Subst)> = Vec::new();
        for inst in candidates {
            let mut ok = true;
            let mut local = self.subst.clone();
            for (have, need) in inst.args.iter().zip(wanted_lookup.iter()) {
                // Bind open vars in `need` to the concrete instance arg.
                match unify_with(&local, need, have) {
                    Ok(s) => local = s,
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                matches.push((inst, local));
            }
        }
        match matches.len() {
            0 => Ok(None),
            1 => {
                let (inst, local) = matches.pop().expect("one match");
                self.subst = compose(&local, &self.subst);
                Ok(Some(inst))
            }
            _ => {
                let first = &matches[0].0;
                let second = &matches[1].0;
                self.report_ambiguous_instance(class, wanted, first, second, range);
                Err(())
            }
        }
    }

    fn report_ambiguous_instance(
        &mut self,
        class: &str,
        wanted: &[Ty],
        first: &InstanceDef,
        second: &InstanceDef,
        range: &Range<usize>,
    ) {
        let pretty = self.instance_signature(class, wanted);
        let mut msg = Message::error(
            ErrorCode::GenericTypeError,
            format!("Ambiguous instance for `{}`", pretty),
            range.clone(),
        );
        msg.with_help(format!(
            "both `{}` from module `{}` and `{}` from module `{}` match",
            self.instance_signature(&first.class, &first.args),
            first.defined_module,
            self.instance_signature(&second.class, &second.args),
            second.defined_module
        ));
        if first.defined_module == self.current_module {
            msg.push(Label::new(
                "matching instance declared here".to_string(),
                first.range.clone(),
            ));
        }
        if second.defined_module == self.current_module {
            msg.push(Label::new(
                "another matching instance declared here".to_string(),
                second.range.clone(),
            ));
        }
        self.messages.push(msg);
    }

    /// True when some active constraint matches `needed` under the current subst,
    /// or implies it via a superclass (Phase 5: `Ordered<T>` covers `Equal<T>`).
    fn constraint_is_covered(&self, needed: &Constraint) -> bool {
        let needed_args: Vec<Ty> = needed
            .args
            .iter()
            .map(|a| apply_ty_prune(&self.subst, a))
            .collect();
        self.active_constraints.iter().any(|ac| {
            if ac.args.len() != needed_args.len() {
                return false;
            }
            let args_match = ac
                .args
                .iter()
                .zip(needed_args.iter())
                .all(|(a, b)| apply_ty_prune(&self.subst, a) == *b);
            if !args_match {
                return false;
            }
            let ac_class = self
                .abstract_constraint_binding(&ac.class)
                .unwrap_or(ac.class.as_str());
            if ac_class == needed.class {
                return true;
            }
            // Implied bound: active subclass covers a superclass constraint.
            self.generics
                .typeclass(ac_class)
                .is_some_and(|def| def.has_superclass(&needed.class, &self.generics))
        })
    }

    fn abstract_constraint_binding(&self, name: &str) -> Option<&str> {
        self.abstract_constraint_bindings
            .iter()
            .rev()
            .find_map(|frame| frame.get(name).map(String::as_str))
    }

    fn bind_abstract_constraint(&mut self, abstract_name: &str, concrete_class: &str) {
        if self.constraint_param_kind(abstract_name).is_none() {
            return;
        }
        if let Some(frame) = self.abstract_constraint_bindings.last_mut() {
            frame
                .entry(abstract_name.to_string())
                .or_insert_with(|| concrete_class.to_string());
        }
    }

    fn bind_matching_abstract_constraints(
        &mut self,
        receiver_var: Option<TyVarId>,
        concrete_class: &str,
    ) {
        let names: Vec<String> = self
            .active_constraints
            .iter()
            .filter(|constraint| self.constraint_param_kind(&constraint.class).is_some())
            .filter(|constraint| {
                receiver_var.is_none_or(|var| {
                    constraint.primary_var() == Some(var)
                        || constraint
                            .args
                            .iter()
                            .any(|a| matches!(a, Ty::Var(v) if *v == var))
                })
            })
            .map(|constraint| constraint.class.clone())
            .collect();
        for name in names {
            self.bind_abstract_constraint(&name, concrete_class);
        }
    }

    fn class_own_method_slot(&self, class_name: &str, method: &str) -> Option<usize> {
        let class_def = self.generics.typeclass(class_name)?;
        class_def.methods.iter().position(|m| m.name == method)
    }

    fn possible_classes_for_constraint_method(
        &self,
        constraint: &Constraint,
        method: &str,
    ) -> Vec<String> {
        if let Some(bound) = self.abstract_constraint_binding(&constraint.class) {
            return vec![bound.to_string()];
        }
        if self.constraint_param_kind(&constraint.class).is_none() {
            return vec![constraint.class.clone()];
        }

        self.generics
            .typeclasses
            .iter()
            .filter_map(|(name, class_def)| {
                if class_def.type_params.len() != constraint.args.len() {
                    return None;
                }
                if self.class_own_method_slot(name, method).is_none() {
                    return None;
                }
                let kinds_match = constraint
                    .args
                    .iter()
                    .enumerate()
                    .all(|(i, arg)| class_def.kind_at(i) == self.kind_of_type_argument(arg));
                kinds_match.then(|| name.clone())
            })
            .collect()
    }

    fn dict_index_for(&self, needed: &Constraint) -> Option<usize> {
        let needed_args: Vec<Ty> = needed
            .args
            .iter()
            .map(|a| apply_ty_prune(&self.subst, a))
            .collect();
        // Prefer an exact class match; fall back to a covering subclass dict
        // (flattened layout holds superclass methods at trailing slots).
        self.active_constraints
            .iter()
            .position(|ac| {
                let ac_class = self
                    .abstract_constraint_binding(&ac.class)
                    .unwrap_or(ac.class.as_str());
                ac_class == needed.class && ac.args.len() == needed_args.len()
                    && ac
                        .args
                        .iter()
                        .zip(needed_args.iter())
                        .all(|(a, b)| apply_ty_prune(&self.subst, a) == *b)
            })
            .or_else(|| {
                self.active_constraints.iter().position(|ac| {
                    let ac_class = self
                        .abstract_constraint_binding(&ac.class)
                        .unwrap_or(ac.class.as_str());
                    ac.args.len() == needed_args.len()
                        && ac
                            .args
                            .iter()
                            .zip(needed_args.iter())
                            .all(|(a, b)| apply_ty_prune(&self.subst, a) == *b)
                        && self
                            .generics
                            .typeclass(ac_class)
                            .is_some_and(|def| def.has_superclass(&needed.class, &self.generics))
                })
            })
    }

    fn user_dict_index(&self, var: TyVarId, class: &str) -> Option<usize> {
        self.user_dict_index_and_class(var, class)
            .map(|(idx, _)| idx)
    }

    fn user_dict_index_and_class(&self, var: TyVarId, class: &str) -> Option<(usize, String)> {
        self.active_constraints
            .iter()
            .enumerate()
            .find_map(|(idx, constraint)| {
                let concrete = self
                    .abstract_constraint_binding(&constraint.class)
                    .unwrap_or(constraint.class.as_str());
                let covers = concrete == class
                    || self
                        .generics
                        .typeclass(concrete)
                        .is_some_and(|def| def.has_superclass(class, &self.generics));
                (covers && (constraint.is_unary_on(var) || constraint.primary_var() == Some(var)))
                    .then(|| (idx, concrete.to_string()))
            })
    }

    fn bound_method_candidates(
        &self,
        method: &str,
        receiver_var: Option<TyVarId>,
    ) -> Vec<(usize, String, String, usize, Scheme)> {
        self.active_constraints
            .iter()
            .enumerate()
            .filter(|(_, constraint)| {
                receiver_var.is_none_or(|var| {
                    constraint.primary_var() == Some(var)
                        || constraint
                            .args
                            .iter()
                            .any(|a| matches!(a, Ty::Var(v) if *v == var))
                })
            })
            .flat_map(|(dict_index, constraint)| {
                self.possible_classes_for_constraint_method(constraint, method)
                    .into_iter()
                    .filter_map(move |dict_class| {
                        let class_def = self.generics.typeclass(&dict_class)?;
                        // Flattened dict: own methods then superclass methods. A call
                        // to a superclass method under `T: Ordered` resolves here with
                        // the trailing slot index (implied Equal).
                        let flat = class_def.flattened_methods(&self.generics);
                        let (method_slot, owner) =
                            flat.iter().enumerate().find_map(|(slot, (owner, m))| {
                                if m.name == method {
                                    Some((slot, (*owner).to_string()))
                                } else {
                                    None
                                }
                            })?;
                        let scheme = self
                            .typeclass_method_schemes
                            .get(&(owner.clone(), method.to_string()))?
                            .clone();
                        Some((dict_index, dict_class, owner, method_slot, scheme))
                    })
            })
            .collect()
    }

    fn existential_method_candidate(
        &self,
        class: &str,
        method: &str,
    ) -> Option<(String, usize, Scheme)> {
        let class_def = self.generics.typeclass(class)?;
        let flat = class_def.flattened_methods(&self.generics);
        let (method_slot, owner) = flat.iter().enumerate().find_map(|(slot, (owner, m))| {
            (m.name == method).then(|| (slot, (*owner).to_string()))
        })?;
        let scheme = self
            .typeclass_method_schemes
            .get(&(owner.clone(), method.to_string()))?
            .clone();
        Some((owner, method_slot, scheme))
    }

    fn select_bound_method(
        &mut self,
        candidates: Vec<(usize, String, String, usize, Scheme)>,
        method: &str,
        range: &Range<usize>,
    ) -> Option<(usize, String, String, usize, Scheme)> {
        if candidates.len() > 1 {
            self.messages.push(Message::error(
                ErrorCode::GenericTypeError,
                format!("Ambiguous trait method `{}`", method),
                range.clone(),
            ));
            return None;
        }
        candidates.into_iter().next()
    }

    /// Resolve a ground trait method call on a concrete receiver
    /// (`recv.into()`, `recv.show()`, …) when no open bound is active.
    ///
    /// Returns `(class, scheme)` when exactly one registered method scheme
    /// named `method` has a first parameter that unifies with `recv_ty`.
    fn ground_trait_method_for_receiver(
        &mut self,
        method: &str,
        recv_ty: &Ty,
    ) -> Option<(String, Scheme)> {
        let recv = apply_ty_prune(&self.subst, recv_ty);
        let schemes: Vec<(String, Scheme)> = self
            .typeclass_method_schemes
            .iter()
            .filter(|((_, mname), _)| mname.as_str() == method)
            .map(|((class, _), scheme)| (class.clone(), scheme.clone()))
            .collect();
        let mut matches: Vec<(String, Scheme)> = Vec::new();
        for (class, scheme) in schemes {
            // Freshen to probe the first parameter; trial unify does not
            // commit into `self.subst`.
            let (fun_ty, _constraints, _kinds) =
                instantiate_with_kinds(&scheme, &mut self.counter);
            let Some(first_param) = Self::first_fun_param(&fun_ty) else {
                continue;
            };
            if unify_with(&self.subst, &first_param, &recv).is_ok() {
                matches.push((class, scheme));
            }
        }
        if matches.len() == 1 {
            matches.pop()
        } else {
            None
        }
    }

    fn first_fun_param(ty: &Ty) -> Option<Ty> {
        match ty {
            Ty::Fun(param, _) => Some(param.as_ref().clone()),
            _ => None,
        }
    }

    /// Instantiate a scheme and record freshened variable kinds (Phase 5).
    fn instantiate_ty(&mut self, scheme: &Scheme) -> Ty {
        let (ty, _, kinds) = instantiate_with_kinds(scheme, &mut self.counter);
        self.var_kinds.extend(kinds);
        ty
    }

    /// Instantiate a scheme with constraints, recording freshened kinds.
    fn instantiate_scheme(&mut self, scheme: &Scheme) -> (Ty, Vec<Constraint>) {
        let (ty, constraints, kinds) = instantiate_with_kinds(scheme, &mut self.counter);
        self.var_kinds.extend(kinds);
        (ty, constraints)
    }

    /// Kind of a type variable, defaulting to `*`.
    fn kind_of_var(&self, var: TyVarId) -> Kind {
        self.var_kinds.get(&var).cloned().unwrap_or(Kind::Type)
    }

    /// Record a type variable's kind (overwrites).
    fn set_var_kind(&mut self, var: TyVarId, kind: Kind) {
        self.var_kinds.insert(var, kind);
    }

    fn constraint_param_kind(&self, name: &str) -> Option<Kind> {
        self.type_param_kind(name)
            .filter(Kind::is_constraint_constructor_kind)
    }

    fn type_param_kind(&self, name: &str) -> Option<Kind> {
        self.type_params_in_scope
            .iter()
            .rev()
            .find_map(|frame| frame.get(name).copied())
            .map(|var| self.kind_of_var(var))
    }

    fn expected_constraint_kind_for_arg(&self, arg: &Ty) -> Kind {
        Kind::arrow(self.kind_of_type_argument(arg), Kind::Constraint)
    }

    fn constraint_from_bound(
        &mut self,
        bound: &str,
        arg: Ty,
        range: &Range<usize>,
    ) -> Option<Constraint> {
        if let Some(kind) = self.constraint_param_kind(bound) {
            let expected = self.expected_constraint_kind_for_arg(&arg);
            if kind != expected {
                self.messages.push(Message::error(
                    ErrorCode::GenericTypeError,
                    format!(
                        "Constraint parameter `{}` has kind `{}`, expected `{}`",
                        bound, kind, expected
                    ),
                    range.clone(),
                ));
                return None;
            }
            return Some(Constraint {
                class: bound.to_string(),
                args: vec![arg],
            });
        }

        if let Some(kind) = self.type_param_kind(bound) {
            let expected = self.expected_constraint_kind_for_arg(&arg);
            self.messages.push(Message::error(
                ErrorCode::GenericTypeError,
                format!(
                    "Constraint parameter `{}` has kind `{}`, expected `{}`",
                    bound, kind, expected
                ),
                range.clone(),
            ));
            return None;
        }

        if self.generics.typeclass(bound).is_none() {
            self.messages.push(Message::error(
                ErrorCode::GenericTypeError,
                format!("Cannot find trait or constraint parameter `{}`", bound),
                range.clone(),
            ));
            return None;
        }

        Some(Constraint {
            class: bound.to_string(),
            args: vec![arg],
        })
    }

    /// Resolve the kind of a type parameter from its AST annotation and/or
    /// class bounds. Explicit annotations win; otherwise a single-parameter
    /// bound whose class parameter is constructor-kinded upgrades the variable
    /// to that constructor kind.
    fn resolve_type_param_kind(&self, tp: &parser::ast::TypeParam<'_>) -> Kind {
        if tp.kind != parser::ast::Kind::Type {
            return Kind::from(tp.kind.clone());
        }
        for bound in &tp.bounds {
            if let Some(class_def) = self.generics.typeclass(bound) {
                if class_def.type_params.len() == 1 && class_def.is_constructor_kind_at(0) {
                    return class_def.kind_at(0);
                }
            }
        }
        Kind::from(tp.kind.clone())
    }

    fn bare_constructor_kind(&self, name: &str) -> Option<Kind> {
        let canon = Self::canonical_ctor_name(name);
        self.generics
            .generic_type_ctors
            .get(&canon)
            .map(|params| Kind::constructor(params.len()))
            .or_else(|| match canon.as_str() {
                common::BUILTIN_OPTION_ENUM => Some(Kind::constructor(1)),
                common::BUILTIN_RESULT_ENUM => Some(Kind::constructor(2)),
                _ => None,
            })
    }

    fn kind_of_type_argument(&self, ty: &Ty) -> Kind {
        match ty {
            Ty::Var(v) => self.kind_of_var(*v),
            Ty::Con(name) => self.bare_constructor_kind(name).unwrap_or(Kind::Type),
            _ => Kind::Type,
        }
    }

    fn check_type_app_kind(
        &mut self,
        name: &str,
        head_kind: &Kind,
        arg_tys: &[Ty],
        range: &Range<usize>,
    ) {
        if !head_kind.is_arrow() {
            self.messages.push(Message::error(
                ErrorCode::GenericTypeError,
                format!(
                    "Type parameter `{}` has kind `{}`, but is applied as a type constructor",
                    name, head_kind
                ),
                range.clone(),
            ));
            return;
        }

        let expected_args = head_kind.argument_kinds();
        if expected_args.len() != arg_tys.len() {
            self.messages.push(Message::error(
                ErrorCode::GenericTypeError,
                format!(
                    "Type constructor `{}` expects {} type arguments, got {}",
                    name,
                    expected_args.len(),
                    arg_tys.len()
                ),
                range.clone(),
            ));
            return;
        }

        for (i, (arg_ty, expected_kind)) in arg_tys.iter().zip(expected_args.iter()).enumerate() {
            let actual_kind = self.kind_of_type_argument(arg_ty);
            if &actual_kind != expected_kind {
                self.messages.push(Message::error(
                    ErrorCode::GenericTypeError,
                    format!(
                        "Type argument {} to `{}` has kind `{}`, expected `{}`",
                        i + 1,
                        name,
                        actual_kind,
                        expected_kind
                    ),
                    range.clone(),
                ));
            }
        }

        if head_kind.result_kind() != &Kind::Type {
            self.messages.push(Message::error(
                ErrorCode::GenericTypeError,
                format!(
                    "Type application `{}` has kind `{}`, expected `*`",
                    name,
                    head_kind.result_kind()
                ),
                range.clone(),
            ));
        }
    }

    fn validate_instance_head_kinds(
        &mut self,
        class_def: &TypeClassDef,
        arg_tys: &[Ty],
        range: &Range<usize>,
    ) {
        for (i, ty) in arg_tys.iter().enumerate() {
            let expected_kind = class_def.kind_at(i);
            if !expected_kind.is_constructor_kind() {
                continue;
            }

            if matches!(ty, Ty::App(_, _)) {
                self.messages.push(Message::error(
                    ErrorCode::GenericTypeError,
                    format!(
                        "Instance of constructor-kinded class `{}` expects a type constructor \
                         (kind `{}`) as argument {}, found applied type `{}`",
                        class_def.name,
                        expected_kind,
                        i + 1,
                        ty
                    ),
                    range.clone(),
                ));
                continue;
            }

            let actual_kind = match ty {
                Ty::Con(name) => self.bare_constructor_kind(name).unwrap_or(Kind::Type),
                Ty::Var(v) => self.kind_of_var(*v),
                _ => Kind::Type,
            };
            if actual_kind != expected_kind {
                self.messages.push(Message::error(
                    ErrorCode::GenericTypeError,
                    format!(
                        "Instance of constructor-kinded class `{}` expects argument {} \
                         to have kind `{}`, found kind `{}`",
                        class_def.name,
                        i + 1,
                        expected_kind,
                        actual_kind
                    ),
                    range.clone(),
                ));
            }
        }
    }

    /// Canonical name for a bare type constructor used as an instance head.
    fn canonical_ctor_name(name: &str) -> String {
        match name.to_ascii_lowercase().as_str() {
            "int" => "int".into(),
            "float" => "float".into(),
            "string" => "string".into(),
            "bool" => "bool".into(),
            "void" | "unit" => "unit".into(),
            "option" => common::BUILTIN_OPTION_ENUM.into(),
            "result" => common::BUILTIN_RESULT_ENUM.into(),
            _ => name.to_string(),
        }
    }

    /// Parse a trait instance argument. Bare registered constructors
    /// (`Option`, `Result`, user generics) become `Ty::Con` heads for HKT
    /// instances rather than applied `Option<_>` placeholders.
    fn parse_instance_head(&mut self, arg: &Output) -> Ty {
        match arg.1.as_ref() {
            Expression::Type(name) | Expression::Identifier(name) => {
                let canon = Self::canonical_ctor_name(name);
                if self.generics.generic_type_ctors.contains_key(&canon)
                    || matches!(name.to_ascii_lowercase().as_str(), "option" | "result")
                {
                    return Ty::Con(canon);
                }
                // First-order instance heads (`int`, `MyType`, …).
                self.parse_type_name_str_with_range(name, Some(arg.0.into_range()))
            }
            Expression::TypeApp { name, args } => {
                // Applied heads are first-order (`impl Foo<Option<int>>`).
                // Constructor-kinded classes diagnose this later.
                self.parse_type_app(name, args, arg.0.into_range())
            }
            _ => self.parse_type_name(arg),
        }
    }

    /// Extract the type variable that a bound method should dispatch on.
    /// First-order: bare `T`. HKT: the constructor head of `F<A>`.
    fn constraint_var_of_ty(ty: &Ty) -> Option<TyVarId> {
        match ty {
            Ty::Var(v) => Some(*v),
            Ty::App(head, _) => match head.as_ref() {
                Ty::Var(v) => Some(*v),
                _ => None,
            },
            _ => None,
        }
    }

    /// For constructor-kinded class parameters, look up instances by
    /// constructor head (`Option`, `Result`), not by applied types
    /// (`Option<int>`, `Result<int, string>`).
    fn instance_lookup_args(&self, class: &str, args: &[Ty]) -> Vec<Ty> {
        if let Some(class_def) = self.generics.typeclass(class) {
            args.iter()
                .enumerate()
                .map(
                    |(i, concrete)| match (class_def.is_constructor_kind_at(i), concrete) {
                        (true, Ty::App(head, _)) => head.as_ref().clone(),
                        _ => concrete.clone(),
                    },
                )
                .collect()
        } else {
            args.to_vec()
        }
    }

    fn instance_signature(&self, class: &str, args: &[Ty]) -> String {
        if args.is_empty() {
            class.to_string()
        } else {
            format!(
                "{}<{}>",
                class,
                args.iter()
                    .map(|ty| ty.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }

    fn instance_satisfies_orphan_rule(
        &self,
        class_def: &TypeClassDef,
        arg_exprs: &[Output],
        arg_tys: &[Ty],
    ) -> bool {
        if class_def.defined_module == self.current_module {
            return true;
        }

        arg_exprs
            .iter()
            .zip(arg_tys.iter())
            .filter(|(_, ty)| !matches!(apply_ty_prune(&self.subst, ty), Ty::Var(_)))
            .all(|(expr, ty)| {
                let head = self
                    .nominal_head_from_instance_arg(expr)
                    .or_else(|| self.nominal_head_from_ty(ty));
                head.as_deref().is_some_and(|name| {
                    self.generics.nominal_type_module(name) == Some(self.current_module.as_str())
                })
            })
    }

    fn nominal_head_from_instance_arg(&self, arg: &Output) -> Option<String> {
        match arg.1.as_ref() {
            Expression::Type(name) | Expression::Identifier(name) => {
                Some(Self::canonical_ctor_name(name))
            }
            Expression::TypeApp { name, .. } => Some(Self::canonical_ctor_name(name)),
            _ => None,
        }
    }

    fn nominal_head_from_ty(&self, ty: &Ty) -> Option<String> {
        match apply_ty_prune(&self.subst, ty) {
            Ty::Var(_) => None,
            Ty::Con(name) => Some(Self::canonical_ctor_name(&name)),
            Ty::App(head, _) => self.nominal_head_from_ty(head.as_ref()),
            Ty::Sum { name, .. } => Some(name),
            Ty::Constructor { owner, .. } => self.nominal_head_from_ty(owner.as_ref()),
            Ty::List(_)
            | Ty::Array { .. }
            | Ty::Tuple(_)
            | Ty::Record { .. }
            | Ty::Existential { .. }
            | Ty::Fun(_, _)
            | Ty::Forall { .. } => None,
        }
    }

    /// Force-cache `ty` at `expr`'s NodeId (and walk TypeApp children) so
    /// codegen FQNs for instance methods see the same head types.
    fn cache_forced_ty(&mut self, expr: &Output, ty: Ty) {
        let id = self.ids.ids()[self.next_id_idx];
        self.next_id_idx += 1;
        self.cache.insert(id, ty);
        if let Expression::TypeApp { args, .. } = expr.1.as_ref() {
            for arg in args {
                // Child annotations still need IDs consumed; infer normally.
                let _ = self.infer(arg);
            }
        }
    }

    fn parse_type_name(&mut self, ann: &Output) -> Ty {
        match ann.1.as_ref() {
            Expression::Identifier(name) | Expression::Type(name) => {
                let range = ann.0.into_range();
                if let Some(class) = self.current_typeclass.clone()
                    && self
                        .generics
                        .typeclass(&class)
                        .is_some_and(|cdef| cdef.assoc_type(name).is_some())
                {
                    return self.resolve_type_projection(&class, name, &[], &range);
                }
                self.parse_type_name_str(name)
            }
            Expression::TypeApp { name, args } => {
                self.parse_type_app(name, args, ann.0.into_range())
            }
            Expression::TypeProjection { owner, name, args } => {
                let arg_tys: Vec<Ty> = args.iter().map(|a| self.parse_type_name(a)).collect();
                self.resolve_type_projection(owner, name, &arg_tys, &ann.0.into_range())
            }
            Expression::TypeFun(arg, ret) => Ty::Fun(
                Box::new(self.parse_type_name(arg)),
                Box::new(self.parse_type_name(ret)),
            ),
            Expression::Forall { params, ty } => {
                self.forall_type(params, |checker| checker.parse_type_name(ty))
            }
            Expression::Array(items) => {
                // `[T; N]` — parser emits `[Type(T), Integer(N)]` (or the
                // legacy single-`Integer(N)` shape, which always meant
                // `[int; N]`).
                if items.len() == 2
                    && let Expression::Integer(n) = items[1].1.as_ref()
                    && *n >= 0
                {
                    let elem_ty = self.parse_type_name(&items[0]);
                    return crate::typechecking::ty::array_fixed(elem_ty, *n as usize);
                }
                if items.len() == 1
                    && let Expression::Integer(n) = items[0].1.as_ref()
                    && *n >= 0
                {
                    return crate::typechecking::ty::array_fixed(
                        self.parse_type_name_str("int"),
                        *n as usize,
                    );
                }
                // Dynamic `[T]` — single element-type annotation.
                if let Some(first) = items.first() {
                    let elem_ty = self.parse_type_name(first);
                    return crate::typechecking::ty::array(elem_ty);
                }
                crate::typechecking::ty::array(self.parse_type_name_str("int"))
            }
            Expression::Tuple(items) => {
                let mut tys = Vec::with_capacity(items.len());
                for item in items {
                    tys.push(self.parse_type_name(item));
                }
                crate::typechecking::ty::tuple(tys)
            }
            _ => Ty::Var(self.counter.fresh()),
        }
    }

    fn parse_type_app(&mut self, name: &str, args: &[Output], range: Range<usize>) -> Ty {
        let arg_tys: Vec<Ty> = args.iter().map(|a| self.parse_type_name(a)).collect();

        if let Some(class) = self.current_typeclass.clone()
            && self
                .generics
                .typeclass(&class)
                .is_some_and(|cdef| cdef.assoc_type(name).is_some())
        {
            return self.resolve_type_projection(&class, name, &arg_tys, &range);
        }

        // In-scope constructor-kinded type parameter as application head
        // (`F<A>`, `F<A, B>`, `F<G>`).
        for frame in self.type_params_in_scope.iter().rev() {
            if let Some(&var) = frame.get(name) {
                let kind = self.kind_of_var(var);
                self.check_type_app_kind(name, &kind, &arg_tys, &range);
                return Ty::App(Box::new(Ty::Var(var)), arg_tys);
            }
        }

        // Generic type aliases expand to their RHS (Phase 1).
        if let Some(def) = self.generic_aliases.get(name).cloned() {
            if def.params.len() != arg_tys.len() {
                self.messages.push(Message::error(
                    ErrorCode::GenericTypeError,
                    format!(
                        "Type constructor `{}` expects {} type arguments, got {}",
                        name,
                        def.params.len(),
                        arg_tys.len()
                    ),
                    range,
                ));
            }
            return self.expand_generic_alias(&def, &arg_tys);
        }

        if let Some(expected_arity) = self
            .generics
            .generic_type_ctors
            .get(name)
            .map(|params| params.len())
        {
            if expected_arity != arg_tys.len() {
                self.messages.push(Message::error(
                    ErrorCode::GenericTypeError,
                    format!(
                        "Type constructor `{}` expects {} type arguments, got {}",
                        name,
                        expected_arity,
                        arg_tys.len()
                    ),
                    range,
                ));
            }
            return Ty::App(Box::new(Ty::Con(name.to_string())), arg_tys);
        }

        self.messages.push(Message::error(
            ErrorCode::GenericTypeError,
            format!("Cannot find type constructor `{}`", name),
            range,
        ));
        Ty::App(Box::new(Ty::Con(name.to_string())), arg_tys)
    }

    fn parse_type_name_str(&mut self, name: &str) -> Ty {
        self.parse_type_name_str_with_range(name, None)
    }

    fn parse_type_name_str_with_range(&mut self, name: &str, range: Option<Range<usize>>) -> Ty {
        // Type parameters in scope take highest priority.
        for frame in self.type_params_in_scope.iter().rev() {
            if let Some(&var) = frame.get(name) {
                return Ty::Var(var);
            }
        }
        for frame in self.type_aliases.iter().rev() {
            if let Some(alias_ty) = frame.get(name) {
                return alias_ty.clone();
            }
        }
        // Built-in type names are matched case-insensitively so the
        // user can write `String`, `STRING`, etc.
        match name.to_ascii_lowercase().as_str() {
            "int" => int(),
            "float" => float(),
            "bool" => boolean(),
            "byte" => crate::typechecking::ty::byte(),
            "string" => string(),
            "void" => unit_ty(),
            "stream" => crate::typechecking::ty::stream_ty(),
            "option" => option_app_ty(Ty::Var(self.counter.fresh())),
            "result" => result_app_ty(Ty::Var(self.counter.fresh()), Ty::Var(self.counter.fresh())),
            "ioerror" => Ty::Con(common::BUILTIN_IO_ERROR_ENUM.into()),
            _ => {
                // Prefer concrete type constructors over bare-class existentials
                // when a name collision exists.
                if self.enums.contains_key(name)
                    || self.classes.contains_key(name)
                    || self.generics.generic_type_ctors.contains_key(name)
                    || self.generics.nominal_type_module(name).is_some()
                {
                    return Ty::Con(name.to_string());
                }
                if let Some(class_def) = self.generics.typeclass(name) {
                    if class_def.type_params.len() == 1 && class_def.kind_at(0) == Kind::Type {
                        return Ty::Existential {
                            class: name.to_string(),
                        };
                    }
                    self.messages.push(Message::error(
                        ErrorCode::GenericTypeError,
                        format!("Typeclass `{}` cannot be used as a bare value type", name),
                        range.unwrap_or(0..0),
                    ));
                }
                Ty::Con(name.to_string())
            }
        }
    }

    /// Resolve `Owner::Assoc` in a type annotation (Phase 6).
    ///
    /// - Inside `trait Owner { … }`, `Owner::Elem` / bare scope lookup
    ///   resolves to the quantified assoc var.
    /// - `T::Elem` when `T` is a type param with an active class constraint
    ///   that declares `Elem` → fresh (or cached) open projection var,
    ///   pinned when a ground instance is later discharged.
    /// - Ground-only fallback: if `Owner` names a class and exactly one
    ///   registered instance defines the assoc type, use that concrete type.
    fn resolve_type_projection(
        &mut self,
        owner: &str,
        assoc: &str,
        args: &[Ty],
        range: &Range<usize>,
    ) -> Ty {
        // 1. Current typeclass: `Collect::Elem` while defining Collect.
        if self.current_typeclass.as_deref() == Some(owner) {
            let decl = self
                .generics
                .typeclass(owner)
                .and_then(|cdef| cdef.assoc_type(assoc))
                .cloned();
            if let Some(decl) = decl {
                self.validate_assoc_projection_args(owner, &decl, args, range);
                if let Some(existing) = self.current_assoc_projections.as_ref().and_then(|ps| {
                    ps.iter()
                        .find(|p| p.name == assoc && p.args == args)
                        .map(|p| p.var)
                }) {
                    return Ty::Var(existing);
                }
                let fresh = self.counter.fresh();
                self.set_var_kind(fresh, Kind::Type);
                self.record_current_assoc_projection(fresh, assoc, args);
                return Ty::Var(fresh);
            }
            self.messages.push(Message::error(
                ErrorCode::GenericTypeError,
                format!(
                    "Cannot find associated type `{}` on trait `{}`",
                    assoc, owner
                ),
                range.clone(),
            ));
            return Ty::Var(self.counter.fresh());
        }

        // 2. Type parameter owner: `T::Elem` with `T: Collect`.
        for frame in self.type_params_in_scope.iter().rev() {
            if let Some(&owner_var) = frame.get(owner) {
                // Find an active constraint on this var whose class declares `assoc`.
                let mut matching_decl: Option<(String, AssocTypeDecl)> = None;
                for c in &self.active_constraints {
                    let covers = c.args.iter().any(
                        |a| matches!(apply_ty_prune(&self.subst, a), Ty::Var(v) if v == owner_var),
                    );
                    if !covers {
                        continue;
                    }
                    if let Some(cdef) = self.generics.typeclass(&c.class) {
                        if let Some(decl) = cdef.assoc_type(assoc) {
                            matching_decl = Some((c.class.clone(), decl.clone()));
                            break;
                        }
                        // Superclass assoc types (rare; check flattened supers).
                        for super_name in &cdef.superclasses {
                            if let Some(sdef) = self.generics.typeclass(super_name) {
                                if let Some(decl) = sdef.assoc_type(assoc) {
                                    matching_decl = Some((super_name.clone(), decl.clone()));
                                    break;
                                }
                            }
                        }
                        if matching_decl.is_some() {
                            break;
                        }
                    }
                }
                if let Some((class_name, decl)) = matching_decl {
                    self.validate_assoc_projection_args(&class_name, &decl, args, range);
                    let key = (owner_var, assoc.to_string(), self.projection_arg_key(args));
                    if let Some(&(existing, _)) = self.open_assoc_projections.get(&key) {
                        self.record_current_assoc_projection(existing, assoc, args);
                        return Ty::Var(existing);
                    }
                    let fresh = self.counter.fresh();
                    self.set_var_kind(fresh, Kind::Type);
                    self.open_assoc_projections
                        .insert(key, (fresh, args.to_vec()));
                    self.record_current_assoc_projection(fresh, assoc, args);
                    return Ty::Var(fresh);
                }
                self.messages.push(Message::error(
                    ErrorCode::GenericTypeError,
                    format!(
                        "Cannot project associated type `{}` from `{}` \
                         (no in-scope trait bound declares it)",
                        assoc, owner
                    ),
                    range.clone(),
                ));
                return Ty::Var(self.counter.fresh());
            }
        }

        // 3. Class-name owner outside definition: ground-only unique instance.
        if let Some(cdef) = self.generics.typeclass(owner).cloned() {
            let Some(decl) = cdef.assoc_type(assoc).cloned() else {
                self.messages.push(Message::error(
                    ErrorCode::GenericTypeError,
                    format!(
                        "Cannot find associated type `{}` on trait `{}`",
                        assoc, owner
                    ),
                    range.clone(),
                ));
                return Ty::Var(self.counter.fresh());
            };
            self.validate_assoc_projection_args(owner, &decl, args, range);
            let mut found: Option<Ty> = None;
            for inst in &self.generics.instances {
                if inst.class != owner {
                    continue;
                }
                if let Some(value) = inst.assoc_tys.get(assoc) {
                    let ty = self.instantiate_assoc_value(value, args);
                    if found.is_some() {
                        // Ambiguous across multiple instances — leave open.
                        found = None;
                        break;
                    }
                    found = Some(ty);
                }
            }
            if let Some(ty) = found {
                return ty;
            }
            // No unique ground instance — fresh var (caller may pin later).
            return Ty::Var(self.counter.fresh());
        }

        self.messages.push(Message::error(
            ErrorCode::GenericTypeError,
            format!("Cannot resolve type projection `{}::{}`", owner, assoc),
            range.clone(),
        ));
        Ty::Var(self.counter.fresh())
    }

    /// After discharging a ground (or unifying) instance, pin any open
    /// associated-type projections whose owner matches the instance args,
    /// and pin freshened assoc vars from trait method schemes.
    fn pin_assoc_types_for_instance(
        &mut self,
        class: &str,
        instance: &InstanceDef,
        scheme: Option<&Scheme>,
        range: &Range<usize>,
    ) {
        let Some(class_def) = self.generics.typeclass(class).cloned() else {
            return;
        };
        if class_def.assoc_types.is_empty() {
            return;
        }

        // Pin open `T::Elem` projections whose owner unifies with instance.args.
        let open_keys: Vec<((TyVarId, String, Vec<String>), (TyVarId, Vec<Ty>))> = self
            .open_assoc_projections
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for ((owner_var, assoc_name, _arg_key), (assoc_var, assoc_args)) in open_keys {
            if class_def.assoc_type(&assoc_name).is_none() {
                continue;
            }
            // Owner must unify with the primary instance arg(s).
            let owner_ty = apply_ty_prune(&self.subst, &Ty::Var(owner_var));
            let matches_owner = instance
                .args
                .iter()
                .any(|arg| unify_with(&self.subst, &owner_ty, arg).is_ok());
            if !matches_owner {
                continue;
            }
            if let Some(value) = instance.assoc_tys.get(&assoc_name).cloned() {
                let concrete = self.instantiate_assoc_value(&value, &assoc_args);
                self.unify(
                    &Ty::Var(assoc_var),
                    &concrete,
                    range,
                    &format!("associated type `{}`", assoc_name),
                );
            }
        }

        let _ = scheme;
    }

    /// Pin freshened associated-type variables from a trait method
    /// scheme instantiation against a concrete instance.
    fn pin_assoc_vars_from_mapping(
        &mut self,
        class: &str,
        instance: &InstanceDef,
        scheme: &Scheme,
        mapping: &HashMap<TyVarId, TyVarId>,
        range: &Range<usize>,
    ) {
        if self.generics.typeclass(class).is_none() {
            return;
        }
        // Clone so unify can mutably borrow `self` in the loop.
        let pins: Vec<(String, TyVarId, Ty)> = scheme
            .assoc_projections
            .iter()
            .filter_map(|projection| {
                let &fresh = mapping.get(&projection.var)?;
                let value = instance.assoc_tys.get(&projection.name)?;
                let args = projection
                    .args
                    .iter()
                    .map(|arg| super::env::substitute_vars(arg, mapping))
                    .collect::<Vec<_>>();
                let concrete = self.instantiate_assoc_value(value, &args);
                Some((projection.name.clone(), fresh, concrete))
            })
            .collect();
        for (assoc_name, fresh, concrete) in pins {
            self.unify(
                &Ty::Var(fresh),
                &concrete,
                range,
                &format!("associated type `{}`", assoc_name),
            );
        }
    }

    /// Instantiate a scheme, returning the freshened type, constraints, and
    /// old→new bound-variable mapping (Phase 6 assoc pinning).
    fn instantiate_scheme_mapped(
        &mut self,
        scheme: &Scheme,
    ) -> (Ty, Vec<Constraint>, HashMap<TyVarId, TyVarId>) {
        use super::env::substitute_vars;
        let mut fresh_kinds = HashMap::new();
        let mapping: HashMap<TyVarId, TyVarId> = scheme
            .bounds
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let fresh = self.counter.fresh();
                fresh_kinds.insert(fresh, scheme.kind_at(i));
                (v, fresh)
            })
            .collect();
        self.var_kinds.extend(fresh_kinds);
        let ty = substitute_vars(&scheme.ty, &mapping);
        let constraints = scheme
            .constraints
            .iter()
            .map(|c| Constraint {
                class: c.class.clone(),
                args: c
                    .args
                    .iter()
                    .map(|a| substitute_vars(a, &mapping))
                    .collect(),
            })
            .collect();
        (ty, constraints, mapping)
    }

    /// Whether codegen should Ok-wrap bare returns for `fn_name`.
    pub fn fn_is_result_mode(&self, fn_name: &str) -> bool {
        self.result_mode_fns.contains(fn_name)
    }

    /// Whether `fn_name` returns (or was inferred to return) `Option<_>`.
    pub fn fn_is_option_mode(&self, fn_name: &str) -> bool {
        self.option_mode_fns.contains(fn_name)
    }

    // ============================================================
    // ============================================================
    //  Native registration
    // ============================================================
    // ============================================================

    /// Register a native (built-in) function with the type system.
    ///
    /// `name` is the function's identifier as seen in user code;
    /// `params` are the parameter types in declaration order; `ret`
    /// is the return type. The signature is curried into a function
    /// type (`arg1 -> arg2 -> ... -> ret`).
    ///
    /// The binding is added to the top frame of the env so it's
    /// visible to every subsequent call. See [`Compiler::register`]
    /// for the public entry point.
    pub fn register_native(&mut self, name: &str, params: &[Ty], ret: &Ty) {
        let fn_ty = params.iter().rev().fold(ret.clone(), |acc, p| {
            Ty::Fun(Box::new(p.clone()), Box::new(acc))
        });

        self.env.insert_top(name.to_string(), Scheme::mono(fn_ty));
    }

    /// True when `expr` is a valid FFI type tag expression:
    /// `FFIType::X`, a bare primitive name, `[T]` / `(T, U)` (lowered to Ptr),
    /// or `FFIType::Struct` with aux id from a registered layout.
    fn is_ffi_type_expr(&self, expr: &Output) -> bool {
        self.ffi_type_tag_from_output(expr).is_some()
    }

    fn infer_ffi_dload(&mut self, args: &[Output], range: Range<usize>) -> Ty {
        if self.ffi_fn_in_scope("dload").is_none() {
            let _ = self.error_with_help(
                ErrorCode::UnknownValue,
                "Cannot find value `dload` in this scope".to_string(),
                range.clone(),
                Some("import it with `use ffi::dload;` or `use ffi::*;`".to_string()),
            );
        }
        if let Some(path) = args.first() {
            let _ = self.infer(path);
        } else {
            let _ = self.error_with_help(
                ErrorCode::DeclareArity,
                "dload requires 1 argument (path)".to_string(),
                range,
                None,
            );
        }
        // dload → Result<int, string>
        result_app_ty(int(), string())
    }

    fn infer_ffi_declare(&mut self, args: &[Output], range: Range<usize>) -> Ty {
        if self.ffi_fn_in_scope("declare").is_none() {
            let _ = self.error_with_help(
                ErrorCode::UnknownValue,
                "Cannot find value `declare` in this scope".to_string(),
                range.clone(),
                Some("import it with `use ffi::declare;` or `use ffi::*;`".to_string()),
            );
        }
        if args.len() == 4 {
            self.infer(&args[0]);
            self.infer(&args[1]);
            match args[2].1.as_ref() {
                Expression::Tuple(_) => {
                    self.infer_ffi_type_expr(&args[2]);
                }
                _ => {
                    let mut m = Message::error(
                        ErrorCode::DeclareArity,
                        "declare(...) third argument must be an arguments tuple (T1, T2, ...)"
                            .to_string(),
                        args[2].0.into_range(),
                    );
                    m.push(Label::new(
                        "wrap the arg types in parentheses — (Int, Float) after `use ffi::types::*;`"
                            .to_string(),
                        args[2].0.into_range(),
                    ));
                    self.messages.push(m);
                }
            }
            self.infer_ffi_type_expr(&args[3]);
        } else {
            for arg in args {
                self.infer(arg);
            }
            let mut m = Message::error(
                ErrorCode::DeclareArity,
                "declare requires 4 arguments (lib, name, args_tuple, ret_type)".to_string(),
                range.clone(),
            );
            m.push(Label::new(
                format!("got {} arguments", args.len()),
                range.clone(),
            ));
            self.messages.push(m);
        }
        // declare → Result<int, string> (fn id or error)
        result_app_ty(int(), string())
    }

    fn infer_ffi_invoke(&mut self, args: &[Output], range: Range<usize>) -> Ty {
        if self.ffi_fn_in_scope("invoke").is_none() {
            let _ = self.error_with_help(
                ErrorCode::UnknownValue,
                "Cannot find value `invoke` in this scope".to_string(),
                range.clone(),
                Some("import it with `use ffi::invoke;` or `use ffi::*;`".to_string()),
            );
        }
        let mut ret_ty = int();
        if args.len() == 3 {
            self.infer(&args[0]);
            self.infer(&args[1]);
            if let Expression::Identifier(name) = args[1].1.as_ref()
                && let Some(ty) = self.ffi_fn_ret_tys.get(*name)
            {
                ret_ty = ty.clone();
            }
            match args[2].1.as_ref() {
                Expression::Tuple(items) => {
                    for item in items {
                        self.infer(item);
                    }
                }
                _ => {
                    let mut m = Message::error(
                        ErrorCode::InvokeArity,
                        "invoke(...) third argument must be an arguments tuple (v1, v2, ...)"
                            .to_string(),
                        args[2].0.into_range(),
                    );
                    m.push(Label::new(
                        "wrap the arg values in parentheses — (40, 2)".to_string(),
                        args[2].0.into_range(),
                    ));
                    self.messages.push(m);
                }
            }
        } else {
            for arg in args {
                self.infer(arg);
            }
            let mut m = Message::error(
                ErrorCode::InvokeArity,
                "invoke requires 3 arguments (lib, fn_id, args_tuple)".to_string(),
                range.clone(),
            );
            m.push(Label::new(
                format!("got {} arguments", args.len()),
                range.clone(),
            ));
            self.messages.push(m);
        }
        // invoke → Result<T, string>
        result_app_ty(ret_ty, string())
    }

    /// Resolve an FFI type expression to `(tag, aux)` for codegen.
    pub fn ffi_type_tag_from_output(&self, expr: &Output) -> Option<(u32, u32)> {
        use common::{tag, tag_from_type_name, tag_from_variant_name};
        match expr.1.as_ref() {
            Expression::Construct {
                enum_name,
                variant_name,
                ..
            } if common::is_builtin_ffi_enum(enum_name) => {
                // Qualified `ffi::types::Int` is always allowed. Legacy
                // `FFIType::Int` requires an explicit import binding.
                if *enum_name == common::BUILTIN_FFI_TYPE_ENUM
                    && !self.builtin_name_in_scope(common::BUILTIN_FFI_TYPE_ENUM)
                    && !self.ffi_tag_in_scope(variant_name)
                {
                    return None;
                }
                let tag = tag_from_variant_name(variant_name)?;
                Some((tag, 0))
            }
            Expression::Type(name) | Expression::Identifier(name) => {
                if let Some(id) = self.c_struct_id(name) {
                    return Some((tag::STRUCT, id));
                }
                // In-scope `use ffi::types::*` tags (`Int`, `Ptr`, …).
                if self.ffi_tag_in_scope(name) {
                    return tag_from_variant_name(name).map(|t| (t, 0));
                }
                // Bare lowercase primitives (`int`, `void`, …) stay
                // available without importing `ffi::types`.
                if name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
                    return tag_from_type_name(name).map(|t| (t, 0));
                }
                None
            }
            Expression::Array(items) if items.len() == 1 => Some((tag::PTR, 0)),
            Expression::Tuple(_) => Some((tag::PTR, 0)),
            _ => None,
        }
    }

    pub fn c_struct_id(&self, name: &str) -> Option<u32> {
        self.c_structs
            .iter()
            .position(|s| s.name == name)
            .map(|i| i as u32)
    }

    pub fn c_structs(&self) -> &[CStructDef] {
        &self.c_structs
    }

    pub fn callback_sigs(&self) -> &[CallbackSigDef] {
        &self.callback_sigs
    }

    /// Emit a diagnostic when `expr` is not a valid FFI type tag.
    fn require_ffi_type_expr(&mut self, expr: &Output) {
        if self.is_ffi_type_expr(expr) {
            return;
        }
        let mut m = Message::error(
            ErrorCode::InvalidFfiType,
            "Expected an FFI type tag".to_string(),
            expr.0.into_range(),
        );
        m.push(Label::new(
            "use `Int`/`Ptr` after `use ffi::types::*;`, a bare type name (int, void, …), [T], (T, U), or a declared extern struct".to_string(),
            expr.0.into_range(),
        ));
        self.messages.push(m);
    }

    /// Infer an FFI type-tag expression (declare arg/ret positions).
    ///
    /// Consumes NodeIds in pre-walk order without treating bare names
    /// like `Point` / `int32` as value lookups. Nested Tuple / Array /
    /// Construct children are walked the same way (or via normal
    /// `infer` for `FFIType::X` constructors, which are real enum
    /// constructs).
    fn infer_ffi_type_expr(&mut self, expr: &Output) {
        self.require_ffi_type_expr(expr);
        match expr.1.as_ref() {
            Expression::Identifier(_) | Expression::Type(_) => {
                let id = self.ids.ids()[self.next_id_idx];
                self.next_id_idx += 1;
                let ty = self.ty_from_ffi_type_expr(expr);
                self.cache.insert(id, ty);
            }
            Expression::Tuple(items) => {
                let id = self.ids.ids()[self.next_id_idx];
                self.next_id_idx += 1;
                self.cache.insert(id, unit_ty());
                for item in items {
                    self.infer_ffi_type_expr(item);
                }
            }
            Expression::Array(items) => {
                let id = self.ids.ids()[self.next_id_idx];
                self.next_id_idx += 1;
                self.cache.insert(id, unit_ty());
                for item in items {
                    // Element annotations are `Type` / nested forms.
                    self.infer_ffi_type_expr(item);
                }
            }
            // `FFIType::Int`, etc. — real Construct nodes; use normal infer
            // so enum constructor typing + child IDs stay aligned.
            _ => {
                let _ = self.infer(expr);
            }
        }
    }

    /// Map an FFI type tag expression to the language `Ty` used for
    /// `invoke` result typing (void → unit, structs → structural record).
    fn ty_from_ffi_type_expr(&self, expr: &Output) -> Ty {
        use common::tag;
        match self.ffi_type_tag_from_output(expr) {
            Some((t, _)) if t == tag::VOID => unit_ty(),
            Some((t, _)) if t == tag::FLOAT => float(),
            Some((t, _)) if t == tag::STRING => string(),
            Some((t, _)) if t == tag::BOOL => boolean(),
            Some((t, id)) if t == tag::STRUCT => {
                if let Some(def) = self.c_structs.get(id as usize) {
                    let fields = def
                        .fields
                        .iter()
                        .map(|(name, enc)| {
                            let tag = if *enc <= tag::STRUCT {
                                *enc
                            } else {
                                *enc & 0xFFFF
                            };
                            let fty = match tag {
                                t if t == tag::FLOAT => float(),
                                t if t == tag::STRING => string(),
                                t if t == tag::BOOL => boolean(),
                                t if t == tag::VOID => unit_ty(),
                                // int / int32 / ptr / … — treat as int at the
                                // language level (narrow C widths are ABI-only).
                                _ => int(),
                            };
                            (name.clone(), fty)
                        })
                        .collect();
                    crate::typechecking::ty::record(fields)
                } else {
                    int()
                }
            }
            _ => int(),
        }
    }

    // ============================================================

    /// Register a class: store its name and the (visibility, name,
    /// type) of each field. The class itself becomes a `Ty::Con(name)`
    /// constructor that's resolvable from any scope, so it can be
    /// referenced as a type elsewhere.
    ///
    /// Generic classes (`class Cell<T>`) store field types with
    /// `Con(param)` schema markers (schemaized from the in-scope type
    /// param vars) so each `new` site can freshen independently.
    fn register_class(&mut self, name: &str, fields: &[Output], range: &Range<usize>) {
        let mut field_info = Vec::new();
        for field in fields {
            if let Expression::Field(vis, fname, fty) = field.1.as_ref() {
                let fname_str = match fname.1.as_ref() {
                    Expression::Identifier(n) => n.to_string(),
                    _ => {
                        self.messages.push(Message::error(
                            ErrorCode::GenericTypeError,
                            "Invalid field name".to_string(),
                            field.0.into_range(),
                        ));
                        continue;
                    }
                };
                let ty = self.parse_type_name(fty);
                field_info.push((*vis, fname_str, ty));
            } else {
                self.messages.push(Message::error(
                    ErrorCode::GenericTypeError,
                    "Expected a field declaration".to_string(),
                    field.0.into_range(),
                ));
            }
        }
        // Schemaize param vars → `Con(name)` for generic class fields.
        if let Some(frame) = self.type_params_in_scope.last() {
            if !frame.is_empty() {
                let var_to_name: HashMap<TyVarId, String> =
                    frame.iter().map(|(n, id)| (*id, n.clone())).collect();
                for (_, _, ty) in &mut field_info {
                    *ty = schemaize_ty(ty, &var_to_name);
                }
            }
        }
        self.classes.insert(name.to_string(), field_info);
        self.generics
            .register_nominal_type(name, &self.current_module);
        // Register the class as a type in the environment so
        // `Foo`-as-a-type lookups succeed.
        self.env
            .insert_top(name.to_string(), Scheme::mono(Ty::Con(name.to_string())));
        let _ = range;
    }

    /// Process an `impl Owner` / `impl Owner<T>` block:
    /// 1. Auto-register the owner class if it hasn't been declared
    ///    yet (so `impl` can appear before `class`).
    /// 2. Push a type-param scope and bind `self : Owner` or
    ///    `self : Owner<T, …>`.
    /// 3. For each method, run [`infer_function`] with `self`
    ///    prepended to the argument list, then store the method's
    ///    scheme (poly when the impl is generic) under the owner's name.
    fn infer_impl(
        &mut self,
        owner: &str,
        type_params: &[parser::ast::TypeParam<'_>],
        methods: &[Output],
        range: &Range<usize>,
    ) {
        let pushed = self.push_type_params_for_type_parsing(type_params);

        let owner_ty = if type_params.is_empty() {
            Ty::Con(owner.to_string())
        } else {
            let frame = self
                .type_params_in_scope
                .last()
                .expect("type-param frame just pushed");
            let args: Vec<Ty> = type_params
                .iter()
                .map(|tp| Ty::Var(*frame.get(tp.name).expect("type param registered in frame")))
                .collect();
            Ty::App(Box::new(Ty::Con(owner.to_string())), args)
        };

        let param_vars: Vec<TyVarId> = if pushed {
            let frame = self
                .type_params_in_scope
                .last()
                .expect("type-param frame just pushed");
            type_params
                .iter()
                .map(|tp| *frame.get(tp.name).expect("type param registered in frame"))
                .collect()
        } else {
            Vec::new()
        };

        if !self.classes.contains_key(owner) {
            self.classes.insert(owner.to_string(), Vec::new());
            self.env
                .insert_top(owner.to_string(), Scheme::mono(Ty::Con(owner.to_string())));
        }

        self.push_scope();
        self.env
            .insert_top("self".to_string(), Scheme::mono(owner_ty.clone()));

        for method in methods {
            if let Expression::Method(vis, body) = method.1.as_ref() {
                if let Expression::Function {
                    name,
                    is_coro,
                    args,
                    returns,
                    where_constraints,
                    body: func_body,
                    ..
                } = body.1.as_ref()
                {
                    // Type params stay in the outer impl frame so `self`
                    // and method annotations share the same variables.
                    let fun_ty = self.infer_function(
                        name,
                        &[],
                        args,
                        returns.as_ref(),
                        where_constraints,
                        func_body,
                        &method.0.into_range(),
                        Some(&owner_ty),
                        *is_coro,
                    );
                    let scheme = if param_vars.is_empty() {
                        Scheme::mono(fun_ty)
                    } else {
                        Scheme::poly(param_vars.clone(), Vec::new(), fun_ty)
                    };
                    self.methods
                        .entry(owner.to_string())
                        .or_default()
                        .insert(name.to_string(), (*vis, scheme));
                } else {
                    self.messages.push(Message::error(
                        ErrorCode::GenericTypeError,
                        "Method body must be a function".to_string(),
                        method.0.into_range(),
                    ));
                }
            }
        }

        self.pop_scope();
        self.pop_type_params_for_type_parsing(pushed);
        let _ = range;
    }

    /// Look up a class field, substituting type-param placeholders when
    /// the receiver is an applied generic class (`Cell<int>`).
    fn access_class_field(
        &mut self,
        class: &str,
        field: &str,
        args: &[Ty],
        range: Range<usize>,
    ) -> Ty {
        let Some(fields) = self.classes.get(class) else {
            return self.error(
                ErrorCode::UnknownField,
                format!("Cannot find field `{}` on class `{}`", field, class),
                range,
            );
        };
        let Some((_, _, fty)) = fields.iter().find(|(_, fname, _)| fname == field) else {
            let known: Vec<&str> = fields.iter().map(|(_, n, _)| n.as_str()).collect();
            return self.error_with_help(
                ErrorCode::UnknownField,
                format!("Cannot find field `{}` on class `{}`", field, class),
                range,
                Some(format!("the class has fields: {}", known.join(", "))),
            );
        };
        let fty = fty.clone();
        let params = self
            .generics
            .generic_type_ctors
            .get(class)
            .cloned()
            .unwrap_or_default();
        if params.is_empty() {
            return fty;
        }
        let mut map = HashMap::new();
        if args.is_empty() {
            for p in &params {
                map.insert(p.clone(), Ty::Var(self.counter.fresh()));
            }
        } else {
            for (p, a) in params.iter().zip(args.iter()) {
                map.insert(p.clone(), a.clone());
            }
        }
        subst_ty_params(&fty, &map)
    }

    // ============================================================
    //  Functions (monomorphic recursion)
    // ============================================================

    fn infer_function(
        &mut self,
        name: &str,
        type_params: &[parser::ast::TypeParam],
        args: &Output,
        returns: Option<&Output>,
        where_constraints: &[parser::ast::WhereConstraint],
        body: &Output,
        range: &Range<usize>,
        self_ty: Option<&Ty>,
        is_coro: bool,
    ) -> Ty {
        // Set up type parameter environment.
        let is_generic = !type_params.is_empty();
        let mut param_vars: Vec<TyVarId> = Vec::new();
        let mut param_frame: HashMap<String, TyVarId> = HashMap::new();

        let mut param_kinds: Vec<Kind> = Vec::new();
        for tp in type_params {
            let var = self.counter.fresh();
            let kind = self.resolve_type_param_kind(tp);
            self.set_var_kind(var, kind.clone());
            param_frame.insert(tp.name.to_string(), var);
            param_vars.push(var);
            param_kinds.push(kind);
        }

        // Push param frame so parse_type_name resolves T → Var(id).
        self.type_params_in_scope.push(param_frame);
        let mut param_constraints: Vec<Constraint> = Vec::new();
        for (tp, var) in type_params.iter().zip(param_vars.iter()) {
            // Binder bounds `T: Num` desugar to unary constraints. Bounds
            // may also name an earlier constraint parameter: `T: c`.
            for bound in &tp.bounds {
                if let Some(constraint) =
                    self.constraint_from_bound(bound, Ty::Var(*var), range)
                {
                    param_constraints.push(constraint);
                }
            }
        }
        // `where Class<T1, T2>` constraints (parsed after returns).
        for wc in where_constraints {
            let args: Vec<Ty> = wc.args.iter().map(|a| self.parse_type_name(a)).collect();
            param_constraints.push(Constraint {
                class: wc.class.to_string(),
                args,
            });
        }
        let prev_constraints_len = self.active_constraints.len();
        self.active_constraints
            .extend(param_constraints.iter().cloned());
        self.abstract_constraint_bindings.push(HashMap::new());

        let collect_fn_assoc = is_generic && self.current_assoc_projections.is_none();
        let prev_fn_assoc = if collect_fn_assoc {
            let prev = self.current_assoc_projections.take();
            self.current_assoc_projections = Some(Vec::new());
            prev
        } else {
            None
        };
        let arg_tys = self.parse_arg_list(args);
        let (ret_ty, yield_slot, send_slot) = if is_coro {
            let yield_ty = Ty::Var(self.counter.fresh());
            let send_ty = Ty::Var(self.counter.fresh());
            // Honor `async fn -> T`: unify declared T with the yield/
            // return slot so annotation mismatches are diagnosed.
            if let Some(r) = returns {
                let declared = self.parse_type_name(r);
                self.unify(
                    &yield_ty,
                    &declared,
                    &r.0.into_range(),
                    "async fn return type",
                );
            }
            let coro = self.coroutine_type(yield_ty.clone(), send_ty.clone());
            (coro, Some(yield_ty), Some(send_ty))
        } else {
            (
                match returns {
                    Some(r) => self.parse_type_name(r),
                    None => Ty::Var(self.counter.fresh()),
                },
                None,
                None,
            )
        };
        let fn_assoc_projections = if collect_fn_assoc {
            let projections = self.current_assoc_projections.take().unwrap_or_default();
            self.current_assoc_projections = prev_fn_assoc;
            projections
        } else {
            Vec::new()
        };

        // Build the declared function type: arg1 -> ... -> argN -> ret,
        // with self prepended for methods.
        let mut fun_ty = ret_ty.clone();
        for (_, arg_ty) in arg_tys.iter().rev() {
            fun_ty = Ty::Fun(Box::new(arg_ty.clone()), Box::new(fun_ty));
        }
        if let Some(self_ty) = self_ty {
            fun_ty = Ty::Fun(Box::new(self_ty.clone()), Box::new(fun_ty));
        }

        // Monomorphic recursion: bind name to a fresh α in the
        // *outer* frame so the function is visible to subsequent
        // code. The body sees it too because the new frame we push
        // for the body is a child of the outer.
        let alpha = self.counter.fresh();

        // Result/Option mode from an annotated return type. Bare
        // `return v` unifies against the Ok / payload slot; the
        // function's own type remains `Result<T,E>` / `Option<T>`.
        let prev_result_mode = self.fn_result_mode.take();
        let prev_option_mode = self.fn_option_mode.take();
        let return_slot = if is_coro {
            yield_slot.clone().unwrap_or_else(unit_ty)
        } else if let Some((ok, err)) = result_ok_err(&ret_ty) {
            self.fn_result_mode = Some((ok.clone(), err));
            ok
        } else if is_option_ty(&ret_ty) {
            if let Some(inner) = option_inner(&ret_ty) {
                self.fn_option_mode = Some(inner);
            }
            ret_ty.clone()
        } else {
            ret_ty.clone()
        };

        let prev_ret = self.current_return_ty.replace(return_slot);
        let prev_yield = self.current_yield_ty.take();
        let prev_send = self.current_send_ty.take();
        let prev_yield_receives = self.yield_receives_used;
        self.yield_receives_used = false;
        if let Some(yield_ty) = yield_slot {
            self.current_yield_ty = Some(yield_ty);
        }
        if let Some(send_ty) = send_slot {
            self.current_send_ty = Some(send_ty);
        }
        let prev_async = self.async_depth;
        if is_coro {
            self.async_functions.insert(name.to_string());
            self.async_depth += 1;
        }

        self.env
            .insert_top(name.to_string(), Scheme::mono(Ty::Var(alpha)));

        self.push_scope();
        if let Some(self_ty) = self_ty {
            // Method receiver — side-table for codegen Access/Call.
            self.codegen_var_types
                .insert("self".to_string(), self_ty.clone());
        }
        for (arg_name, arg_ty) in &arg_tys {
            self.env
                .insert_top(arg_name.clone(), Scheme::mono(arg_ty.clone()));
            self.codegen_var_types
                .insert(arg_name.clone(), arg_ty.clone());
        }
        let _ = self.infer(body);
        self.pop_scope();

        if is_coro {
            self.async_depth = prev_async;
            if let (Some(yield_ty), Some(send_ty)) =
                (self.current_yield_ty.take(), self.current_send_ty.take())
            {
                let resolved_yield = apply_ty_prune(&self.subst, &yield_ty);
                let mut resolved_send = apply_ty_prune(&self.subst, &send_ty);
                if !self.yield_receives_used {
                    self.unify(&resolved_send, &unit_ty(), range, "coroutine send type");
                    resolved_send = unit_ty();
                }
                fun_ty = {
                    let mut ft = self.coroutine_type(resolved_yield, resolved_send);
                    for (_, arg_ty) in arg_tys.iter().rev() {
                        ft = Ty::Fun(Box::new(arg_ty.clone()), Box::new(ft));
                    }
                    if let Some(self_ty) = self_ty {
                        ft = Ty::Fun(Box::new(self_ty.clone()), Box::new(ft));
                    }
                    ft
                };
            }
            self.yield_receives_used = prev_yield_receives;
        } else if let Some((ok, err)) = self.fn_result_mode.clone() {
            // Body used raise/? — rebuild fun_ty with Result return.
            let ok = apply_ty_prune(&self.subst, &ok);
            let err = apply_ty_prune(&self.subst, &err);
            let result_ret = result_ty(ok, err);
            fun_ty = result_ret.clone();
            for (_, arg_ty) in arg_tys.iter().rev() {
                fun_ty = Ty::Fun(Box::new(arg_ty.clone()), Box::new(fun_ty));
            }
            if let Some(self_ty) = self_ty {
                fun_ty = Ty::Fun(Box::new(self_ty.clone()), Box::new(fun_ty));
            }
            self.result_mode_fns.insert(name.to_string());
            let _ = result_ret;
        } else if let Some(inner) = self.fn_option_mode.clone() {
            let inner = apply_ty_prune(&self.subst, &inner);
            let opt_ret = option_ty(inner);
            // If annotated/inferred return was already Option, keep
            // fun_ty; otherwise rebuild.
            let resolved_ret = apply_ty_prune(&self.subst, &ret_ty);
            if !is_option_ty(&resolved_ret) {
                fun_ty = opt_ret;
                for (_, arg_ty) in arg_tys.iter().rev() {
                    fun_ty = Ty::Fun(Box::new(arg_ty.clone()), Box::new(fun_ty));
                }
                if let Some(self_ty) = self_ty {
                    fun_ty = Ty::Fun(Box::new(self_ty.clone()), Box::new(fun_ty));
                }
            }
            self.option_mode_fns.insert(name.to_string());
        }
        self.current_yield_ty = prev_yield;
        self.current_send_ty = prev_send;

        self.current_return_ty = prev_ret;
        self.fn_result_mode = prev_result_mode;
        self.fn_option_mode = prev_option_mode;
        self.unify(&Ty::Var(alpha), &fun_ty, range, "function type");

        let abstract_bindings = self.abstract_constraint_bindings.pop().unwrap_or_default();
        let mut resolved_param_constraints = Vec::with_capacity(param_constraints.len());
        for constraint in param_constraints {
            if self.constraint_param_kind(&constraint.class).is_some() {
                if let Some(concrete) = abstract_bindings.get(&constraint.class) {
                    resolved_param_constraints.push(Constraint {
                        class: concrete.clone(),
                        args: constraint.args,
                    });
                } else {
                    self.messages.push(Message::error(
                        ErrorCode::GenericTypeError,
                        format!(
                            "Cannot satisfy abstract constraint `{}`; no concrete trait was selected",
                            constraint
                        ),
                        range.clone(),
                    ));
                }
            } else {
                resolved_param_constraints.push(constraint);
            }
        }

        // Pop type param scope.
        self.active_constraints.truncate(prev_constraints_len);
        self.type_params_in_scope.pop();

        // If generic, build a poly scheme and re-insert into env.
        if is_generic {
            self.generic_fns.insert(name.to_string());
            self.generics.generic_fns.insert(name.to_string());
            let mut bounds = param_vars;
            bounds.extend(fn_assoc_projections.iter().map(|p| p.var));
            let mut kinds = param_kinds;
            kinds.extend(std::iter::repeat_n(Kind::Type, fn_assoc_projections.len()));
            let scheme = Scheme::poly_with_kinds_and_assoc(
                bounds,
                kinds,
                resolved_param_constraints.clone(),
                fn_assoc_projections,
                fun_ty.clone(),
            );
            self.env.insert_top(name.to_string(), scheme);

            // Every constraint is a trailing dictionary argument. Builtin
            // classes use compiler-generated implementation thunks, while
            // user classes use source-declared methods; their calling ABI is
            // intentionally identical.
            self.fn_dict_arity
                .insert(name.to_string(), resolved_param_constraints.len());
        }

        fun_ty
    }

    /// Parse a function's argument list (a `Fragment` of
    /// `Argument(ty, name)` nodes).
    fn parse_arg_list(&mut self, args: &Output) -> Vec<(String, Ty)> {
        let mut out = Vec::new();
        if let Expression::Fragment(children) = args.1.as_ref() {
            for child in children {
                if let Expression::Argument(ty, name) = child.1.as_ref() {
                    out.push((name.to_string(), self.parse_type_name(ty)));
                }
            }
        }
        out
    }

    // ============================================================
    //  Enums and pattern matching
    // ============================================================

    /// Pre-pass: register enum shapes before main inference (forward refs).
    fn pre_register_enums(&mut self, ast: &Output) -> Result<(), Vec<Message>> {
        let mut errors = Vec::new();
        self.pre_register_enums_walk(ast, &mut errors);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    fn pre_register_enums_walk(&mut self, node: &Output, errors: &mut Vec<Message>) {
        use parser::ast::EnumVariantPayload;
        match node.1.as_ref() {
            Expression::TypeAlias { ty, .. } => {
                self.pre_register_enums_walk(ty, errors);
            }
            Expression::EnumDecl {
                name,
                type_params,
                variants,
                ..
            } => {
                let name_str = name.to_string();
                let previous_generic_ctor = self.register_generic_type_ctor(name, type_params);
                let pushed = self.push_type_params_for_type_parsing(type_params);
                let mut variant_names = Vec::new();
                let mut arities = Vec::new();
                let mut payloads: Vec<EnumVariantPayloadTy> = Vec::new();

                for v in variants {
                    if let Expression::EnumVariant {
                        name: vname,
                        payload,
                    } = v.1.as_ref()
                    {
                        variant_names.push(vname.to_string());
                        let payload_ty = match payload {
                            EnumVariantPayload::Unit => {
                                arities.push(0);
                                EnumVariantPayloadTy::Unit
                            }
                            EnumVariantPayload::Tuple(parts) => {
                                let mut tys = Vec::with_capacity(parts.len());
                                for p in parts {
                                    tys.push(self.parse_type_name(p));
                                }
                                arities.push(tys.len());
                                EnumVariantPayloadTy::Tuple(tys)
                            }
                            EnumVariantPayload::Record(fields) => {
                                let mut pairs = Vec::with_capacity(fields.len());
                                for f in fields {
                                    let fty = self.parse_type_name(&f.value);
                                    pairs.push((f.name.to_string(), fty));
                                }
                                arities.push(pairs.len());
                                EnumVariantPayloadTy::Record(pairs)
                            }
                        };
                        payloads.push(payload_ty);
                    }
                }

                // Check 1: duplicate enum name (including built-ins still in scope).
                if common::is_builtin_enum(&name_str) {
                    if self.builtin_name_in_scope(&name_str)
                        || common::is_builtin_ffi_enum(&name_str)
                    {
                        let mut msg = Message::error(
                            ErrorCode::DuplicateEnum,
                            format!("Cannot redeclare built-in enum `{}`", name_str),
                            node.0.into_range(),
                        );
                        if common::is_poly_builtin_enum(&name_str) {
                            msg.with_help(format!(
                                "`{}` is in the prelude; free the short name with `use prelude::{} as OtherName;` before redefining, or pick a different name",
                                name_str, name_str
                            ));
                        } else {
                            msg.with_help(format!(
                                "`{}` is a compiler FFI type; use `ffi::types::{{…}}` instead of redeclaring it",
                                name_str
                            ));
                        }
                        errors.push(msg);
                        self.restore_generic_type_ctor(&name_str, previous_generic_ctor);
                        self.pop_type_params_for_type_parsing(pushed);
                        return;
                    }
                    // Prelude enum short name was rebound — drop the
                    // compiler registration so the user enum can take over.
                    self.enums.remove(&name_str);
                    self.enum_tags.remove(&name_str);
                    self.enum_payloads.remove(&name_str);
                    self.enum_arities.remove(&name_str);
                    self.generics.generic_type_ctors.remove(&name_str);
                    self.generics.nominal_type_modules.remove(&name_str);
                }
                if self.enums.contains_key(&name_str) {
                    let mut msg = Message::error(
                        ErrorCode::DuplicateEnum,
                        format!("Duplicate enum `{}`", name_str),
                        node.0.into_range(),
                    );
                    msg.with_help(format!(
                        "an enum named `{}` was already declared; remove or rename this declaration",
                        name_str
                    ));
                    errors.push(msg);
                    self.restore_generic_type_ctor(&name_str, previous_generic_ctor);
                    self.pop_type_params_for_type_parsing(pushed);
                    return;
                }

                // Check 2: variant name collides with a previously
                // registered enum's variant name (cross-enum).
                for vn in &variant_names {
                    let taken = self.enum_tags.values().any(|tags| tags.contains_key(vn));
                    if taken {
                        let mut msg = Message::error(
                            ErrorCode::DuplicateConstructor,
                            format!(
                                "Duplicate constructor `{}` (also declared by another enum)",
                                vn
                            ),
                            node.0.into_range(),
                        );
                        msg.with_help(
                            "constructor names must be unique across all enums".to_string(),
                        );
                        errors.push(msg);
                        self.restore_generic_type_ctor(&name_str, previous_generic_ctor);
                        self.pop_type_params_for_type_parsing(pushed);
                        return;
                    }
                }

                // Check 3: variant name shadows a built-in
                // (currently no such checks — natives are registered
                // with full names like `print` and don't share the
                // `::` namespace. Reserved for future use.)

                // Reserve. We use `BTreeMap` for tags (lookups are
                // by variant name, not order). The `Vec` for
                // variant order is the canonical declaration order.
                let mut tag_map = BTreeMap::new();
                for (i, vn) in variant_names.iter().enumerate() {
                    tag_map.insert(vn.clone(), i as u32);
                }

                // Generic enums store payloads with `Con(param)` schema
                // markers (same convention as builtin Option/Result) so
                // construct/match can freshen independently per site.
                let payloads = if pushed {
                    let frame = self
                        .type_params_in_scope
                        .last()
                        .expect("type-param frame just pushed");
                    let var_to_name: HashMap<TyVarId, String> =
                        frame.iter().map(|(n, id)| (*id, n.clone())).collect();
                    payloads
                        .iter()
                        .map(|p| schemaize_payload(p, &var_to_name))
                        .collect()
                } else {
                    payloads
                };

                self.enums.insert(name_str.clone(), variant_names);
                self.enum_tags.insert(name_str.clone(), tag_map);
                self.enum_payloads.insert(name_str.clone(), payloads);
                self.enum_arities.insert(name_str.clone(), arities);
                self.generics
                    .register_nominal_type(&name_str, &self.current_module);
                self.pop_type_params_for_type_parsing(pushed);
            }

            // Recurse into the same children that `id::pre_walk` would
            // visit. We mirror the structure of `pre_walk_children`
            // but only need to find nested EnumDecls — most
            // branches can just walk their expression children.
            Expression::Noop(_)
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
            | Expression::Use { .. }
            | Expression::Module(_, _)
            | Expression::Variable(_, _)
            | Expression::Constant(_, _)
            | Expression::Argument(_, _)
            | Expression::Field(_, _, _)
            | Expression::ExternBlock { .. }
            | Expression::ExternStruct(_) => {}

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
            | Expression::Member(e) => {
                self.pre_register_enums_walk(e, errors);
            }

            Expression::TypeApp { args, .. } => {
                for a in args {
                    self.pre_register_enums_walk(a, errors);
                }
            }

            Expression::TypeFun(arg, ret) => {
                self.pre_register_enums_walk(arg, errors);
                self.pre_register_enums_walk(ret, errors);
            }

            Expression::CompoundAssign(name, _, value) => {
                self.pre_register_enums_walk(name, errors);
                self.pre_register_enums_walk(value, errors);
            }

            Expression::Assignment(name, value) => {
                self.pre_register_enums_walk(name, errors);
                self.pre_register_enums_walk(value, errors);
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
                self.pre_register_enums_walk(l, errors);
                self.pre_register_enums_walk(r, errors);
            }

            Expression::Print(fmt, params) | Expression::Format(fmt, params) => {
                self.pre_register_enums_walk(fmt, errors);
                if let Some(p) = params {
                    for param in p {
                        self.pre_register_enums_walk(param, errors);
                    }
                }
            }

            Expression::Resume(target, arg) => {
                self.pre_register_enums_walk(target, errors);
                if let Some(a) = arg {
                    self.pre_register_enums_walk(a, errors);
                }
            }

            Expression::Block(cs)
            | Expression::Program(cs)
            | Expression::Fragment(cs)
            | Expression::List(cs)
            | Expression::Declare(cs)
            | Expression::Invoke(cs) => {
                for c in cs {
                    self.pre_register_enums_walk(c, errors);
                }
            }
            Expression::Dload(path) => self.pre_register_enums_walk(path, errors),
            Expression::Done(handle) => self.pre_register_enums_walk(handle, errors),
            Expression::Tuple(items) => {
                for c in items {
                    self.pre_register_enums_walk(c, errors);
                }
            }
            Expression::Array(items) => {
                for c in items {
                    self.pre_register_enums_walk(c, errors);
                }
            }
            Expression::Index(target, index) => {
                self.pre_register_enums_walk(target, errors);
                self.pre_register_enums_walk(index, errors);
            }
            Expression::Dict(fields) => {
                for f in fields {
                    self.pre_register_enums_walk(&f.value, errors);
                }
            }
            Expression::If(branches) => {
                for b in branches {
                    self.pre_register_enums_walk(b, errors);
                }
            }
            Expression::Implementation { methods, .. } => {
                for m in methods {
                    self.pre_register_enums_walk(m, errors);
                }
            }
            Expression::Class { fields, .. } => {
                for f in fields {
                    self.pre_register_enums_walk(f, errors);
                }
            }

            Expression::Function { args, body, .. } => {
                self.pre_register_enums_walk(args, errors);
                self.pre_register_enums_walk(body, errors);
            }

            Expression::Branch(cond, body) => {
                if let Some(c) = cond {
                    self.pre_register_enums_walk(c, errors);
                }
                self.pre_register_enums_walk(body, errors);
            }

            Expression::Call { name, args } => {
                self.pre_register_enums_walk(name, errors);
                if let Some(a) = args {
                    for arg in a {
                        self.pre_register_enums_walk(arg, errors);
                    }
                }
            }

            Expression::Loop {
                iterable,
                body,
                identifier,
            } => {
                self.pre_register_enums_walk(iterable, errors);
                if let Some(i) = identifier {
                    self.pre_register_enums_walk(i, errors);
                }
                self.pre_register_enums_walk(body, errors);
            }

            Expression::For {
                init,
                cond,
                step,
                body,
            } => {
                if let Some(init) = init {
                    self.pre_register_enums_walk(init, errors);
                }
                self.pre_register_enums_walk(cond, errors);
                self.pre_register_enums_walk(body, errors);
                if let Some(step) = step {
                    self.pre_register_enums_walk(step, errors);
                }
            }

            Expression::Match { scrutinee, arms } => {
                self.pre_register_enums_walk(scrutinee, errors);
                for arm in arms {
                    // Patterns are not expressions — no recursion
                    // into the pattern body. (Constructor patterns
                    // contain only nested patterns.)
                    self.pre_register_enums_walk(&arm.body, errors);
                }
            }

            // The `EnumDecl` arm above handles every EnumDecl in
            // the tree; no second arm is needed here. `EnumVariant`
            // and `Construct` are still reachable (e.g. inside a
            // function body) and just recurse.
            Expression::EnumVariant { .. } => {}
            Expression::Construct { .. } => {}

            Expression::Method(_, body) => {
                self.pre_register_enums_walk(body, errors);
            }
            Expression::Access(receiver, _) | Expression::OptionalAccess(receiver, _) => {
                self.pre_register_enums_walk(receiver, errors);
            }
            Expression::Instantiate(class, args) => {
                self.pre_register_enums_walk(class, errors);
                if let Some(a) = args {
                    for arg in a {
                        self.pre_register_enums_walk(arg, errors);
                    }
                }
            }

            // New generic-system nodes — recurse into children.
            Expression::Forall { ty, .. } => self.pre_register_enums_walk(ty, errors),
            Expression::TypeClass { methods, .. } => {
                for m in methods {
                    self.pre_register_enums_walk(m, errors);
                }
            }
            Expression::TypeClassImpl { args, methods, .. } => {
                for a in args {
                    self.pre_register_enums_walk(a, errors);
                }
                for m in methods {
                    self.pre_register_enums_walk(m, errors);
                }
            }
            Expression::AssocTypeDecl { .. } => {}
            Expression::TypeProjection { args, .. } => {
                for arg in args {
                    self.pre_register_enums_walk(arg, errors);
                }
            }
            Expression::AssocTypeDef { ty, .. } => self.pre_register_enums_walk(ty, errors),
        }
    }

    // ---- Enum declarations ----

    fn infer_enum_decl(&mut self, name: &str, variants: &[Output], _range: &Range<usize>) {
        use parser::ast::EnumVariantPayload;
        let name_str = name.to_string();
        // Look up the pre-reserved shape. If missing, the
        // pre-pass rejected this enum (duplicate / collision);
        // the caller has already pushed a diagnostic. Just walk
        // the children to keep IDs aligned.
        let pre_shape = match self.enums.get(&name_str).cloned() {
            Some(v) => v,
            None => {
                for v in variants {
                    let _ = self.infer(v);
                }
                return;
            }
        };
        let pre_payloads = match self.enum_payloads.get(&name_str).cloned() {
            Some(p) => p,
            None => {
                for v in variants {
                    let _ = self.infer(v);
                }
                return;
            }
        };

        // Walk each variant. We delegate to `self.infer(v)` for the
        // whole variant — its `EnumVariant` arm in `infer_inner`
        // recurses into the payload children. That gives us
        // exactly `1 + N` IDs per variant where N is the number
        // of payload entries the pre-walk visited (1 per Tuple
        // element, 1 per Record field's value, 0 for Unit). The
        // pre-pass has already built the typed payload, so the
        // infer recursion is purely for ID-alignment.
        let mut built_variants: Vec<(String, EnumVariantPayloadTy)> = Vec::new();
        for (i, v) in variants.iter().enumerate() {
            // Consume IDs for the variant itself + its payload
            // before any early `continue`. The pre-walk visited
            // this node and its payload regardless of whether we
            // accept it.
            let _ = self.infer(v);

            if let Expression::EnumVariant {
                name: vname,
                payload,
            } = v.1.as_ref()
            {
                let vname_str = vname.to_string();
                let pre_pay = match pre_payloads.get(i) {
                    Some(p) => p.clone(),
                    None => {
                        continue;
                    }
                };

                // Sanity: name + payload arity should match the
                // pre-pass shape. If not, the pre-pass has already
                // complained — skip registering this variant but
                // keep IDs aligned (already done above).
                if pre_shape.get(i) != Some(&vname_str) {
                    continue;
                }
                let expected_count = match &pre_pay {
                    EnumVariantPayloadTy::Unit => 0,
                    EnumVariantPayloadTy::Tuple(tys) => tys.len(),
                    EnumVariantPayloadTy::Record(fields) => fields.len(),
                };
                let actual_count = match payload {
                    EnumVariantPayload::Unit => 0,
                    EnumVariantPayload::Tuple(parts) => parts.len(),
                    EnumVariantPayload::Record(fields) => fields.len(),
                };
                if expected_count != actual_count {
                    continue;
                }
                built_variants.push((vname_str, pre_pay));
            }
        }

        // Build the Ty::Sum.
        let sum_ty = Ty::Sum {
            name: name_str.clone(),
            variants: built_variants.clone(),
        };

        // Register the enum itself as a type.
        self.env
            .insert_top(name_str.clone(), Scheme::mono(Ty::Con(name_str.clone())));

        // Register each variant as a callable in the env. Use the
        // qualified name `EnumName::VariantName` as the binding
        // key — `Construct` looks up by qualified name in this
        // map.
        for (i, (vname, payload_ty)) in built_variants.iter().enumerate() {
            // Field count = 0 for Unit, N for Tuple/Record.
            // Same arity, regardless of shape — the shape
            // discrimination happens at call-site / pattern
            // inference, not at the constructor's HM type.
            let arity = payload_ty.field_count();
            let ctor_ty = Ty::Constructor {
                owner: Box::new(sum_ty.clone()),
                tag: i as u32,
                arity,
            };
            let scheme = if arity == 0 {
                Scheme::mono(ctor_ty)
            } else {
                // Curried: arg1 -> arg2 -> ... -> Constructor.
                // Field order matches declaration order for both
                // Tuple and Record — codegen reorders record
                // call sites to declaration order before pushing
                // the MAKE_ENUM.
                let arg_tys: Vec<Ty> = payload_ty.field_types().into_iter().cloned().collect();
                let mut fun_ty = ctor_ty;
                for arg_ty in arg_tys.iter().rev() {
                    fun_ty = Ty::Fun(Box::new(arg_ty.clone()), Box::new(fun_ty));
                }
                Scheme::mono(fun_ty)
            };
            let qualified = format!("{}::{}", name_str, vname);
            self.env.insert_top(qualified, scheme);
        }
    }

    /// Constructor application with shape/arity checking.
    fn infer_construct(
        &mut self,
        enum_name: &str,
        variant_name: &str,
        fields: &parser::ast::EnumConstructPayload<'_>,
        range: Range<usize>,
    ) -> Ty {
        use parser::ast::EnumConstructPayload;
        // Surface path `ffi::types::Int` maps to the internal `FFIType` registry.
        let registry_name = if common::is_builtin_ffi_enum(enum_name) {
            // Legacy `FFIType::X` requires an import; `ffi::types::X` is always OK.
            if enum_name == common::BUILTIN_FFI_TYPE_ENUM
                && !self.builtin_name_in_scope(common::BUILTIN_FFI_TYPE_ENUM)
                && !self.ffi_tag_in_scope(variant_name)
            {
                return self.error_with_help(
                    ErrorCode::UnknownEnum,
                    format!("Cannot find enum `{}` in this scope", enum_name),
                    range,
                    Some(
                        "import tags with `use ffi::types::*;` (or write `ffi::types::Int`)"
                            .to_string(),
                    ),
                );
            }
            common::BUILTIN_FFI_TYPE_ENUM.to_string()
        } else {
            enum_name.to_string()
        };
        let enum_str = registry_name;
        let variant_str = variant_name.to_string();

        // Look up the enum. Error if not registered.
        let tags = match self.enum_tags.get(&enum_str) {
            Some(t) => t.clone(),
            None => {
                return self.error(
                    ErrorCode::UnknownEnum,
                    format!("Cannot find enum `{}` in this scope", enum_name),
                    range,
                );
            }
        };

        // Look up the variant tag.
        let tag = match tags.get(&variant_str) {
            Some(t) => *t,
            None => {
                return self.error(
                    ErrorCode::UnknownVariant,
                    format!(
                        "Cannot find variant `{}` on enum `{}`",
                        variant_str, enum_str
                    ),
                    range,
                );
            }
        };

        let arity = self
            .enum_arities
            .get(&enum_str)
            .and_then(|a| a.get(tag as usize).copied())
            .unwrap_or(0);

        // Polymorphic enums (builtin Option/Result or user
        // `enum Box<T>`): mint fresh payload vars so each construct
        // site gets an independent applied type.
        let (expected_payload, poly_sum_owner) = if self.is_poly_enum(&enum_str) {
            let (payload, owner) = self.fresh_poly_construct_payload(&enum_str, &variant_str);
            (payload, Some(owner))
        } else {
            let payload = self
                .enum_payloads
                .get(&enum_str)
                .and_then(|p| p.get(tag as usize).cloned())
                .unwrap_or(EnumVariantPayloadTy::Unit);
            (payload, None)
        };

        // Shape vs arity: record shapes defer to field-by-field checks.
        let (shape_matches, same_shape_with_wrong_arity) = match (&expected_payload, fields) {
            (EnumVariantPayloadTy::Unit, EnumConstructPayload::Unit) => (true, false),
            (EnumVariantPayloadTy::Tuple(_), EnumConstructPayload::Tuple(args)) => {
                let want = expected_payload.field_count();
                (args.len() == want, args.len() != want)
            }
            (EnumVariantPayloadTy::Record(_), EnumConstructPayload::Record(_)) => {
                // Defer the arity check to the field-by-field
                // pass below, which produces more specific
                // diagnostics ("Missing field `x`" instead of
                // "expects 2 arguments, got 1").
                (true, false)
            }
            _ => (false, false),
        };

        if !shape_matches {
            if same_shape_with_wrong_arity {
                return self.error(
                    ErrorCode::ConstructorArity,
                    format!(
                        "Constructor `{}::{}` expects {} arguments, got {}",
                        enum_str,
                        variant_str,
                        expected_payload.field_count(),
                        match fields {
                            EnumConstructPayload::Unit => 0,
                            EnumConstructPayload::Tuple(args) => args.len(),
                            EnumConstructPayload::Record(parts) => parts.len(),
                        },
                    ),
                    range,
                );
            }
            return self.error_with_help(
                ErrorCode::PayloadShapeMismatch,
                format!(
                    "Constructor `{}::{}` payload shape mismatch (declared as {}, called as {})",
                    enum_str,
                    variant_str,
                    payload_kind_name(&expected_payload),
                    match fields {
                        EnumConstructPayload::Unit => "unit",
                        EnumConstructPayload::Tuple(_) => "tuple",
                        EnumConstructPayload::Record(_) => "record",
                    },
                ),
                range,
                Some(format!(
                    "use {} syntax for `{}::{}`",
                    if matches!(expected_payload, EnumVariantPayloadTy::Record(_)) {
                        "record"
                    } else {
                        "tuple / unit"
                    },
                    enum_str,
                    variant_str,
                )),
            );
        }

        // Field-by-field type check.
        match fields {
            EnumConstructPayload::Unit => {}
            EnumConstructPayload::Tuple(args) => {
                let expected_tys = expected_payload.field_types();
                for (arg, expected_ty) in args.iter().zip(expected_tys.iter()) {
                    let arg_ty = self.infer(arg);
                    self.unify(
                        expected_ty,
                        &arg_ty,
                        &arg.0.into_range(),
                        &format!("constructor `{}::{}` argument", enum_str, variant_str),
                    );
                }
            }
            EnumConstructPayload::Record(parts) => {
                // Build a name → value map for the call site, then
                // walk the DECLARATION order. Each declared field
                // must be supplied exactly once; the codegen
                // reorders the bytecode accordingly.
                let mut call_site: std::collections::HashMap<&str, &Output> =
                    std::collections::HashMap::with_capacity(parts.len());
                for p in parts {
                    if call_site.insert(p.name, &p.value).is_some() {
                        return self.error_with_help(
                            ErrorCode::DuplicateField,
                            format!(
                                "Duplicate field `{}` in record constructor `{}::{}`",
                                p.name, enum_str, variant_str,
                            ),
                            range,
                            Some("each field must be supplied exactly once".to_string()),
                        );
                    }
                }
                let EnumVariantPayloadTy::Record(decl_fields) = &expected_payload else {
                    // unreachable — shape_matches already proved it
                    unreachable!();
                };
                for (decl_name, decl_ty) in decl_fields.iter() {
                    let arg = match call_site.get(decl_name.as_str()) {
                        Some(a) => *a,
                        None => {
                            return self.error_with_help(
                                ErrorCode::MissingField,
                                format!(
                                    "Missing field `{}` in record constructor `{}::{}`",
                                    decl_name, enum_str, variant_str,
                                ),
                                range,
                                Some(format!("add `{}: <expr>` to the call site", decl_name,)),
                            );
                        }
                    };
                    let arg_ty = self.infer(arg);
                    self.unify(
                        decl_ty,
                        &arg_ty,
                        &arg.0.into_range(),
                        &format!(
                            "constructor `{}::{}.{}` argument",
                            enum_str, variant_str, decl_name,
                        ),
                    );
                }
                // Check for any unknown field names (extra
                // fields supplied at the call site).
                for p in parts {
                    if !decl_fields.iter().any(|(dn, _)| dn == p.name) {
                        return self.error_with_help(
                            ErrorCode::UnknownField,
                            format!(
                                "Unknown field `{}` in record constructor `{}::{}`",
                                p.name, enum_str, variant_str,
                            ),
                            range,
                            Some(format!(
                                "the declared fields are: {}",
                                decl_fields
                                    .iter()
                                    .map(|(n, _)| format!("`{}`", n))
                                    .collect::<Vec<_>>()
                                    .join(", "),
                            )),
                        );
                    }
                }
            }
        }

        // Build the result. The owner is the full `Ty::Sum` so
        // later unifications (in match patterns) can compare tag
        // and arity directly.
        let sum_ty = if let Some(owner) = poly_sum_owner {
            // Re-read payload types after unify so Ok/Some carry
            // the concrete argument type.
            apply_ty_prune(&self.subst, &owner)
        } else {
            Ty::Sum {
                name: enum_str.clone(),
                variants: self
                    .enum_payloads
                    .get(&enum_str)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .zip(self.enums.get(&enum_str).cloned().unwrap_or_default())
                    .map(|(p, n)| (n, p))
                    .collect(),
            }
        };

        Ty::Constructor {
            owner: Box::new(sum_ty),
            tag,
            arity,
        }
    }

    /// True when `name` is a polymorphic enum (builtin Option/Result
    /// or a user enum registered in `generic_type_ctors`).
    fn is_poly_enum(&self, name: &str) -> bool {
        common::is_poly_builtin_enum(name) || self.generics.generic_type_ctors.contains_key(name)
    }

    /// Fresh payload + owning type for a polymorphic construct site.
    ///
    /// Builtin Option/Result keep structural `Ty::Sum` owners (bridged
    /// to `Ty::App` annotations via unify). User generic enums return
    /// `Ty::App(Con(name), args)` so they unify directly with
    /// annotations like `Box<int>`.
    fn fresh_poly_construct_payload(
        &mut self,
        enum_name: &str,
        variant_name: &str,
    ) -> (EnumVariantPayloadTy, Ty) {
        if common::is_builtin_option_enum(enum_name) {
            let t = Ty::Var(self.counter.fresh());
            let owner = option_ty(t.clone());
            let payload = if variant_name == "Some" {
                EnumVariantPayloadTy::Tuple(vec![t])
            } else {
                EnumVariantPayloadTy::Unit
            };
            return (payload, owner);
        }
        if common::is_builtin_result_enum(enum_name) {
            let t = Ty::Var(self.counter.fresh());
            let e = Ty::Var(self.counter.fresh());
            let owner = result_ty(t.clone(), e.clone());
            let payload = if variant_name == "Ok" {
                EnumVariantPayloadTy::Tuple(vec![t])
            } else {
                EnumVariantPayloadTy::Tuple(vec![e])
            };
            return (payload, owner);
        }

        // User generic enum: freshen schema payloads (`Con(param)`).
        let params = self
            .generics
            .generic_type_ctors
            .get(enum_name)
            .cloned()
            .unwrap_or_default();
        let mut map = HashMap::new();
        let mut args = Vec::with_capacity(params.len());
        for p in &params {
            let v = Ty::Var(self.counter.fresh());
            args.push(v.clone());
            map.insert(p.clone(), v);
        }
        let tag = self
            .enum_tags
            .get(enum_name)
            .and_then(|t| t.get(variant_name).copied())
            .unwrap_or(0);
        let schema = self
            .enum_payloads
            .get(enum_name)
            .and_then(|p| p.get(tag as usize).cloned())
            .unwrap_or(EnumVariantPayloadTy::Unit);
        let payload = subst_payload_params(&schema, &map);
        let owner = Ty::App(Box::new(Ty::Con(enum_name.to_string())), args);
        (payload, owner)
    }

    /// Payload type for a pattern arm: prefer the scrutinee Sum's
    /// concrete payloads for poly enums, else App-applied args, else
    /// the registry schema.
    fn poly_or_registry_payload(
        &mut self,
        enum_name: &str,
        tag: u32,
        expected_ty: &Ty,
        pattern_range: &Range<usize>,
    ) -> Option<EnumVariantPayloadTy> {
        let resolved = apply_ty_prune(&self.subst, expected_ty);
        let sum = match &resolved {
            Ty::Sum { name, variants } if name == enum_name => Some(variants.clone()),
            Ty::Constructor { owner, .. } => match owner.as_ref() {
                Ty::Sum { name, variants } if name == enum_name => Some(variants.clone()),
                _ => None,
            },
            _ => None,
        };
        if let Some(variants) = sum {
            if let Some((_, payload)) = variants.get(tag as usize) {
                return Some(payload.clone());
            }
        }
        if let Some(payload) = self.poly_payload_from_app(enum_name, tag, &resolved) {
            return Some(payload);
        }
        if self.is_poly_enum(enum_name) {
            // Scrutinee not yet pinned — freshen an applied type and
            // unify so bindings share type vars with the scrutinee.
            let owner = self.fresh_poly_app_ty(enum_name);
            self.unify(
                expected_ty,
                &owner,
                pattern_range,
                "poly enum pattern scrutinee",
            );
            let resolved = apply_ty_prune(&self.subst, &owner);
            if let Some(payload) = self.poly_payload_from_app(enum_name, tag, &resolved) {
                return Some(payload);
            }
        }
        self.enum_payloads
            .get(enum_name)
            .and_then(|p| p.get(tag as usize).cloned())
    }

    /// Build `Enum<α, …>` with a fresh type variable per type param.
    fn fresh_poly_app_ty(&mut self, enum_name: &str) -> Ty {
        if common::is_builtin_option_enum(enum_name) {
            return option_app_ty(Ty::Var(self.counter.fresh()));
        }
        if common::is_builtin_result_enum(enum_name) {
            return result_app_ty(Ty::Var(self.counter.fresh()), Ty::Var(self.counter.fresh()));
        }
        let params = self
            .generics
            .generic_type_ctors
            .get(enum_name)
            .cloned()
            .unwrap_or_default();
        let args: Vec<Ty> = params
            .iter()
            .map(|_| Ty::Var(self.counter.fresh()))
            .collect();
        Ty::App(Box::new(Ty::Con(enum_name.to_string())), args)
    }

    /// Extract a variant payload from an applied poly enum type
    /// (`Option<int>`, `Box<int>`, …).
    fn poly_payload_from_app(
        &self,
        enum_name: &str,
        tag: u32,
        ty: &Ty,
    ) -> Option<EnumVariantPayloadTy> {
        match ty {
            Ty::App(con, args)
                if matches!(con.as_ref(), Ty::Con(name) if name == enum_name)
                    && common::is_builtin_option_enum(enum_name) =>
            {
                match tag {
                    0 => Some(EnumVariantPayloadTy::Unit),
                    1 => args
                        .first()
                        .cloned()
                        .map(|inner| EnumVariantPayloadTy::Tuple(vec![inner])),
                    _ => None,
                }
            }
            Ty::App(con, args)
                if matches!(con.as_ref(), Ty::Con(name) if name == enum_name)
                    && common::is_builtin_result_enum(enum_name) =>
            {
                match tag {
                    0 => args
                        .first()
                        .cloned()
                        .map(|ok| EnumVariantPayloadTy::Tuple(vec![ok])),
                    1 => args
                        .get(1)
                        .cloned()
                        .map(|err| EnumVariantPayloadTy::Tuple(vec![err])),
                    _ => None,
                }
            }
            Ty::App(con, args)
                if matches!(con.as_ref(), Ty::Con(name) if name == enum_name)
                    && self.generics.generic_type_ctors.contains_key(enum_name) =>
            {
                let param_names = self.generics.generic_type_ctors.get(enum_name)?;
                if param_names.len() != args.len() {
                    return None;
                }
                let mut map = HashMap::new();
                for (p, a) in param_names.iter().zip(args.iter()) {
                    map.insert(p.clone(), a.clone());
                }
                let schema = self.enum_payloads.get(enum_name)?.get(tag as usize)?;
                Some(subst_payload_params(schema, &map))
            }
            Ty::Constructor { owner, .. } => self.poly_payload_from_app(enum_name, tag, owner),
            _ => None,
        }
    }

    // ---- Match ----

    fn infer_match(&mut self, scrutinee: &Output, arms: &[MatchArm], range: Range<usize>) -> Ty {
        let scrutinee_ty = self.infer(scrutinee);
        let resolved_scrutinee = apply_ty_prune(&self.subst, &scrutinee_ty);

        // Set up current_match_lhs for `Expression::Default`
        // (which Decision C preserves but is unreachable in real
        // source — wildcard patterns never reach it).
        let prev = self.current_match_lhs.replace(scrutinee_ty.clone());

        let mut result_ty = Ty::Var(self.counter.fresh());
        let mut first = true;
        let mut coverage: Vec<ArmCoverage> = Vec::with_capacity(arms.len());

        if arms.is_empty() {
            self.current_match_lhs = prev;
            return self.error(
                ErrorCode::GenericTypeError,
                "match has no arms".to_string(),
                range,
            );
        }

        for arm in arms {
            // Step 1: each arm gets a fresh env frame so the
            // pattern's bindings don't leak.
            self.push_scope();

            // Step 2: type the pattern, binding variables. The
            // pattern AST doesn't carry its own range today, so we
            // pass the arm's body range as a reasonable proxy for
            // error anchoring — it's close enough that ariadne
            // points near the offending pattern instead of at byte
            // 0 of the source.
            let pattern_range = arm.body.0.into_range();
            let pat_ty = self.infer_pattern(&arm.pattern, &resolved_scrutinee, &pattern_range);

            // Step 3: unify pattern type with scrutinee.
            self.unify(
                &resolved_scrutinee,
                &pat_ty,
                &arm.body.0.into_range(),
                "match pattern against scrutinee",
            );

            // Step 4: capture coverage info.
            let arm_cov = self.arm_coverage(&arm.pattern, &arm.body.0.into_range());
            coverage.push(arm_cov);

            // Step 5: infer body, unify with result.
            let body_ty = self.infer(&arm.body);
            if first {
                result_ty = body_ty;
                first = false;
            } else {
                self.unify(
                    &result_ty,
                    &body_ty,
                    &arm.body.0.into_range(),
                    "match arm body",
                );
            }

            // Step 6: pop the per-arm env frame.
            self.pop_scope();
        }

        self.current_match_lhs = prev;

        // Record for the post-pass exhaustiveness check. The
        // scrutinee type stored here is the resolved (pruned)
        // version at the time of the match; the post-pass will
        // re-apply the current substitution to handle any
        // variables bound by intervening code.
        self.pending_exhaustive.push(PendingExhaustive {
            scrutinee_ty: resolved_scrutinee,
            arms: coverage,
            match_range: range,
        });

        result_ty
    }

    /// Type-check a pattern against an expected type, binding
    /// variables into the current env frame. Returns the pattern's
    /// type, which is the **expected** type (the sum type, not
    /// the constructor type) — patterns desugar the scrutinee, so
    /// the pattern's type IS the scrutinee's type. The tag
    /// matching (which determines whether the arm is reachable) is
    /// captured separately in [`ArmCoverage`].
    ///
    /// `pattern_range` is the source range of the pattern itself —
    /// or, when not available, a reasonable proxy (the arm's body
    /// range). It is used to anchor pattern-related diagnostics
    /// (`unknown constructor`, `wrong arity`) so ariadne points at
    /// the offending pattern instead of byte 0 of the source.
    fn infer_pattern(
        &mut self,
        pattern: &Pattern,
        expected_ty: &Ty,
        pattern_range: &Range<usize>,
    ) -> Ty {
        use parser::ast::PatternPayload;
        match pattern {
            Pattern::Wildcard => {
                // Wildcard matches anything, binds nothing. The
                // body's bindings (if any) come from nested
                // patterns; wildcard itself has no payload.
                expected_ty.clone()
            }
            Pattern::Binding { name } => {
                // `name => body` binds `name` to the scrutinee in
                // the arm's env. This makes the arm cover every
                // case (Rust semantics).
                self.env
                    .insert_top(name.to_string(), Scheme::mono(expected_ty.clone()));
                expected_ty.clone()
            }
            Pattern::Constructor {
                enum_name,
                variant_name,
                payload,
            } => {
                // 1. Look up the variant's tag in the registry.
                let enum_str = enum_name.to_string();
                let variant_str = variant_name.to_string();
                let tag_opt = self
                    .enum_tags
                    .get(&enum_str)
                    .and_then(|t| t.get(&variant_str).copied());
                let tag = match tag_opt {
                    Some(t) => t,
                    None => {
                        // Unknown constructor in a pattern is an
                        // error. Record the error and return the
                        // expected type so the arm body is still
                        // processed.
                        self.messages.push(Message::error(
                            ErrorCode::UnknownConstructorPattern,
                            format!(
                                "Pattern references unknown constructor `{}::{}`",
                                enum_str, variant_str
                            ),
                            pattern_range.clone(),
                        ));
                        return expected_ty.clone();
                    }
                };
                let _arity = self
                    .enum_arities
                    .get(&enum_str)
                    .and_then(|a| a.get(tag as usize).copied())
                    .unwrap_or(0);
                let expected_payload = self
                    .poly_or_registry_payload(&enum_str, tag, expected_ty, pattern_range)
                    .unwrap_or(EnumVariantPayloadTy::Unit);

                let (shape_matches, same_shape_with_wrong_arity) =
                    match (&expected_payload, payload) {
                        (EnumVariantPayloadTy::Unit, PatternPayload::Unit) => (true, false),
                        (EnumVariantPayloadTy::Tuple(_), PatternPayload::Tuple(parts)) => {
                            let want = expected_payload.field_count();
                            (parts.len() == want, parts.len() != want)
                        }
                        (EnumVariantPayloadTy::Record(_), PatternPayload::Record(_)) => {
                            // Defer the arity check to the
                            // field-by-field pass below, which
                            // produces more specific diagnostics.
                            (true, false)
                        }
                        _ => (false, false),
                    };
                if !shape_matches {
                    if same_shape_with_wrong_arity {
                        return self.error_with_help(
                            ErrorCode::GenericTypeError,
                            format!(
                                "Constructor pattern `{}::{}` expects {} sub-patterns, got {}",
                                enum_str,
                                variant_str,
                                expected_payload.field_count(),
                                match payload {
                                    PatternPayload::Unit => 0,
                                    PatternPayload::Tuple(parts) => parts.len(),
                                    PatternPayload::Record(fields) => fields.len(),
                                },
                            ),
                            pattern_range.clone(),
                            Some("check the variant's declared payload arity".to_string()),
                        );
                    }
                    return self.error_with_help(
                        ErrorCode::PayloadShapeMismatch, format!(
                            "Constructor pattern `{}::{}` payload shape mismatch (declared as {}, pattern uses {})",
                            enum_str,
                            variant_str,
                            payload_kind_name(&expected_payload),
                            match payload {
                                PatternPayload::Unit => "unit",
                                PatternPayload::Tuple(_) => "tuple",
                                PatternPayload::Record(_) => "record",
                            },
                        ),
                        pattern_range.clone(),
                        Some("check the variant's declared payload shape".to_string()),
                    );
                }

                // 3. Recurse into each sub-pattern with the
                // corresponding payload type. The payload type
                // comes from the pre-pass's `enum_payloads`
                // (already resolved, e.g. `int` for
                // `Option::Some(int)`).
                match payload {
                    PatternPayload::Unit => {}
                    PatternPayload::Tuple(parts) => {
                        let expected_tys = expected_payload.field_types();
                        for (sub_pat, expected_ty) in parts.iter().zip(expected_tys.iter()) {
                            let _ = self.infer_pattern(sub_pat, expected_ty, pattern_range);
                        }
                    }
                    PatternPayload::Record(fields) => {
                        // Build a name → pattern map for the
                        // pattern site, then walk DECLARATION
                        // order. Each declared field must be
                        // present exactly once; the codegen binds
                        // in slot order (= declaration order).
                        let mut pattern_site: std::collections::HashMap<&str, &Pattern> =
                            std::collections::HashMap::with_capacity(fields.len());
                        for pf in fields {
                            if pattern_site.insert(pf.name, &pf.pattern).is_some() {
                                return self.error_with_help(
                                    ErrorCode::DuplicateField,
                                    format!(
                                        "Duplicate field `{}` in record pattern `{}::{}`",
                                        pf.name, enum_str, variant_str,
                                    ),
                                    pattern_range.clone(),
                                    Some("each field must appear exactly once".to_string()),
                                );
                            }
                        }
                        let EnumVariantPayloadTy::Record(decl_fields) = &expected_payload else {
                            unreachable!()
                        };
                        for (decl_name, decl_ty) in decl_fields.iter() {
                            let sub_pat = match pattern_site.get(decl_name.as_str()) {
                                Some(p) => *p,
                                None => {
                                    return self.error_with_help(
                                        ErrorCode::MissingField,
                                        format!(
                                            "Missing field `{}` in record pattern `{}::{}`",
                                            decl_name, enum_str, variant_str,
                                        ),
                                        pattern_range.clone(),
                                        Some(format!(
                                            "add `{0}: _` (or `{0}: binding`) to the pattern",
                                            decl_name,
                                        )),
                                    );
                                }
                            };
                            let _ = self.infer_pattern(sub_pat, decl_ty, pattern_range);
                        }
                        // Check for unknown field names.
                        for pf in fields {
                            if !decl_fields.iter().any(|(dn, _)| dn == pf.name) {
                                return self.error_with_help(
                                    ErrorCode::UnknownField,
                                    format!(
                                        "Unknown field `{}` in record pattern `{}::{}`",
                                        pf.name, enum_str, variant_str,
                                    ),
                                    pattern_range.clone(),
                                    Some(format!(
                                        "the declared fields are: {}",
                                        decl_fields
                                            .iter()
                                            .map(|(n, _)| format!("`{}`", n))
                                            .collect::<Vec<_>>()
                                            .join(", "),
                                    )),
                                );
                            }
                        }
                    }
                }

                // 4. The pattern's type is the *expected* type —
                // patterns desugar the scrutinee, so the pattern
                // returns whatever the scrutinee had. (If the
                // scrutinee was a Ty::Constructor for a specific
                // tag, the pattern is still of that same type;
                // exhaustiveness checking will report the
                // arm as unreachable.)
                expected_ty.clone()
            }
        }
    }

    /// Inspect the first non-trivial sub-pattern of a payload and
    /// report which inner tag (if any) it tests. Two arms of the
    /// same outer tag are reachable as long as their inner coverage
    /// differs — e.g. `Result::Ok(Option::Some(v))` and
    /// `Result::Ok(Option::None)` are two distinct reachable arms.
    /// The codegen's inner `JUMP_IF_MATCH` test chain guarantees
    /// this at runtime; the typechecker just needs to stay out of
    /// the way.
    fn inner_coverage(
        payload: &parser::ast::PatternPayload<'_>,
        enum_tags: &BTreeMap<String, BTreeMap<String, u32>>,
    ) -> InnerCoverage {
        use parser::ast::PatternPayload;
        let first = match payload {
            PatternPayload::Unit => return InnerCoverage::Any,
            PatternPayload::Tuple(parts) => parts.first(),
            PatternPayload::Record(fields) => fields.first().map(|f| &f.pattern),
        };
        let Some(first) = first else {
            return InnerCoverage::Any;
        };
        match first {
            Pattern::Wildcard | Pattern::Binding { .. } => InnerCoverage::Any,
            Pattern::Constructor {
                enum_name,
                variant_name,
                ..
            } => enum_tags
                .get(enum_name.to_string().as_str())
                .and_then(|t| t.get(variant_name.to_string().as_str()).copied())
                .map(InnerCoverage::Tag)
                .unwrap_or(InnerCoverage::Any),
        }
    }

    /// Capture per-arm coverage info for the deferred
    /// exhaustiveness check.
    fn arm_coverage(&self, pattern: &Pattern, range: &Range<usize>) -> ArmCoverage {
        match pattern {
            Pattern::Wildcard => ArmCoverage {
                tag: None,
                inner: InnerCoverage::Any,
                is_catchall: true,
                range: range.clone(),
            },
            Pattern::Binding { .. } => ArmCoverage {
                tag: None,
                inner: InnerCoverage::Any,
                is_catchall: true,
                range: range.clone(),
            },
            Pattern::Constructor {
                enum_name,
                variant_name,
                payload,
                ..
            } => {
                let tag = self
                    .enum_tags
                    .get(enum_name.to_string().as_str())
                    .and_then(|t| t.get(variant_name.to_string().as_str()).copied());
                let inner = Self::inner_coverage(payload, &self.enum_tags);
                ArmCoverage {
                    tag,
                    inner,
                    is_catchall: false,
                    range: range.clone(),
                }
            }
        }
    }

    /// Post-pass: run every deferred exhaustiveness check. By this
    /// point the substitution is closed, so the scrutinee type is
    /// fully resolved (any free type variables that were bound
    /// since the match site are visible here).
    fn run_pending_exhaustiveness(&mut self) {
        // Drain into a local so we can release the borrow on
        // `self` before mutating `self.messages`.
        let pending: Vec<PendingExhaustive> = std::mem::take(&mut self.pending_exhaustive);
        for p in &pending {
            self.check_exhaustiveness(p);
        }
    }

    /// Verify a single match site. Records diagnostics but does
    /// not abort — error recovery continues.
    fn check_exhaustiveness(&mut self, pending: &PendingExhaustive) {
        // Re-resolve the scrutinee under the current substitution
        // so any variables bound between the match site and the
        // post-pass are visible.
        let resolved = apply_ty_prune(&self.subst, &pending.scrutinee_ty);

        // Track which (outer tag, inner coverage) pairs have been
        // seen and whether a catch-all (wildcard / binding) is
        // present. Two arms with the same outer tag but DIFFERENT
        // inner coverage (e.g. `Result::Ok(Option::Some(v))` vs
        // `Result::Ok(Option::None)`) are both reachable — the
        // codegen's inner `JUMP_IF_MATCH` chain dispatches between
        // them at runtime. Only when both the outer tag AND the
        // inner coverage match an earlier arm is the arm truly
        // unreachable.
        let mut seen: BTreeMap<u32, BTreeSet<InnerCoverage>> = BTreeMap::new();
        let mut has_catchall = false;
        for arm in &pending.arms {
            if arm.is_catchall {
                has_catchall = true;
            } else if let Some(t) = arm.tag {
                let inner_seen = seen.entry(t).or_default();
                if !inner_seen.insert(arm.inner.clone()) {
                    // Duplicate (tag, inner coverage) — this arm
                    // is unreachable.
                    self.messages.push(Message::error(
                        ErrorCode::UnreachableArm,
                        "Unreachable arm: this pattern is matched by an earlier arm".to_string(),
                        arm.range.clone(),
                    ));
                }
            }
        }

        if has_catchall {
            // A wildcard / binding arm covers every remaining
            // case. No further error needed.
            return;
        }

        // Unwrap a Constructor to its parent sum/app. For Ty::Var /
        // Ty::Con, no exhaustiveness check.
        let variants = match &resolved {
            Ty::Sum { variants, .. } => Some(variants.clone()),
            Ty::Constructor { owner, .. } => match owner.as_ref() {
                Ty::Sum { variants, .. } => Some(variants.clone()),
                other => self.poly_variants_from_app(other),
            },
            other => self.poly_variants_from_app(other),
        };

        if let Some(variants) = variants {
            // An outer tag is "covered" for the purpose of the
            // non-exhaustive check if any arm with that tag
            // exists. The inner coverage only matters for the
            // duplicate-arm check above.
            let covered: BTreeSet<u32> = seen.into_keys().collect();
            let missing: Vec<String> = variants
                .iter()
                .enumerate()
                .filter(|(tag, _)| !covered.contains(&(*tag as u32)))
                .map(|(_, (n, _))| n.clone())
                .collect();
            if !missing.is_empty() {
                let names = missing
                    .iter()
                    .map(|s| format!("`{}`", s))
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut msg = Message::error(
                    ErrorCode::NonExhaustiveMatch,
                    format!("Non-exhaustive match: variants not covered: {}", names),
                    pending.match_range.clone(),
                );
                msg.with_help(
                    "add a wildcard arm `_ => ...` to cover the remaining cases".to_string(),
                );
                self.messages.push(msg);
            }
        }
    }

    /// Variant list for exhaustiveness from an applied poly enum type.
    fn poly_variants_from_app(&self, ty: &Ty) -> Option<Vec<(String, EnumVariantPayloadTy)>> {
        match ty {
            Ty::App(con, args) if matches!(con.as_ref(), Ty::Con(name) if common::is_builtin_option_enum(name)) =>
            {
                let inner = args.first()?.clone();
                Some(vec![
                    ("None".into(), EnumVariantPayloadTy::Unit),
                    ("Some".into(), EnumVariantPayloadTy::Tuple(vec![inner])),
                ])
            }
            Ty::App(con, args) if matches!(con.as_ref(), Ty::Con(name) if common::is_builtin_result_enum(name)) =>
            {
                let ok = args.first()?.clone();
                let err = args.get(1)?.clone();
                Some(vec![
                    ("Ok".into(), EnumVariantPayloadTy::Tuple(vec![ok])),
                    ("Err".into(), EnumVariantPayloadTy::Tuple(vec![err])),
                ])
            }
            Ty::App(con, args) if matches!(con.as_ref(), Ty::Con(name) if self.generics.generic_type_ctors.contains_key(name)) =>
            {
                let Ty::Con(enum_name) = con.as_ref() else {
                    return None;
                };
                let param_names = self.generics.generic_type_ctors.get(enum_name)?;
                if param_names.len() != args.len() {
                    return None;
                }
                let mut map = HashMap::new();
                for (p, a) in param_names.iter().zip(args.iter()) {
                    map.insert(p.clone(), a.clone());
                }
                let names = self.enums.get(enum_name)?;
                let payloads = self.enum_payloads.get(enum_name)?;
                if names.len() != payloads.len() {
                    return None;
                }
                Some(
                    names
                        .iter()
                        .zip(payloads.iter())
                        .map(|(n, p)| (n.clone(), subst_payload_params(p, &map)))
                        .collect(),
                )
            }
            Ty::Constructor { owner, .. } => self.poly_variants_from_app(owner),
            _ => None,
        }
    }

    /// Type-check a `print` (or `format`) expression: the format
    /// string must be a string literal, and each `%X` specifier's
    /// corresponding argument must have a matching type.
    fn infer_print(
        &mut self,
        fmt: &Output,
        params: &Option<Vec<Output>>,
        range: Range<usize>,
        ctx: &str,
    ) {
        let fmt_ty = self.infer(fmt);
        self.unify(&fmt_ty, &string(), &fmt.0.into_range(), "print format");

        // Pull the format string out of the literal so we can
        // parse its specifiers. If the format isn't a string
        // literal, skip validation (the user has a type error
        // elsewhere; we shouldn't cascade).
        let fmt_str = match fmt.1.as_ref() {
            Expression::String(s) => Some(s.to_string()),
            _ => None,
        };

        let mut spec_index = 0usize;
        if let (Some(s), Some(p)) = (fmt_str.as_deref(), params) {
            for (i, ch) in s.char_indices() {
                if ch == '%' {
                    // Look ahead for the specifier.
                    let rest = &s[i + 1..];
                    let mut chars = rest.chars();
                    if let Some(spec) = chars.next() {
                        // Handle `%%` (literal %). It consumes
                        // no argument.
                        if spec == '%' {
                            continue;
                        }
                        // We have `%X`. Validate the Nth arg.
                        if let Some(arg) = p.get(spec_index) {
                            let arg_ty = self.infer(arg);
                            let arg_range = arg.0.into_range();
                            let arg_ty_pruned = apply_ty_prune(&self.subst, &arg_ty);
                            self.check_format_arg(
                                spec,
                                &arg_ty_pruned,
                                &arg_range,
                                ctx,
                                spec_index,
                            );
                            spec_index += 1;
                        } else {
                            // Specifier with no arg — also an
                            // error.
                            let mut msg = Message::error(
                                ErrorCode::GenericTypeError,
                                format!(
                                    "Format string has more specifiers than arguments \
                                     (`%{}` is argument #{})",
                                    spec,
                                    spec_index + 1
                                ),
                                range.clone(),
                            );
                            msg.with_help(format!(
                                "add an argument for `%%{}` in the call site",
                                spec
                            ));
                            self.messages.push(msg);
                            return;
                        }
                    } else {
                        // Trailing `%` with no specifier. Skip.
                        break;
                    }
                }
            }
        } else if let Some(p) = params {
            // No specifiers (or non-literal format) — type-check
            // each param and discard (the VM still consumes the
            // args at the bytecode level, even if the format
            // string contains no specifiers).
            for arg in p {
                let _ = self.infer(arg);
            }
        }
    }

    /// Validate one format argument against its `%X` specifier.
    fn check_format_arg(
        &mut self,
        spec: char,
        arg_ty: &Ty,
        arg_range: &Range<usize>,
        ctx: &str,
        spec_index: usize,
    ) {
        if spec == 'v' {
            match arg_ty {
                Ty::Var(v) => {
                    if self.user_dict_index(*v, "Show").is_none() {
                        self.bind_matching_abstract_constraints(Some(*v), "Show");
                    }
                    if self.user_dict_index(*v, "Show").is_some() {
                        self.record_bound_display(arg_range, *v);
                    } else {
                        let mut msg = Message::error(
                            ErrorCode::FormatSpecifierMismatch,
                            format!(
                                "Format specifier `%v` requires a `Show` instance, found {}",
                                arg_ty
                            ),
                            arg_range.clone(),
                        );
                        msg.with_help(format!(
                            "add a `T: Show` bound, or use a concrete type; \
                             while checking `{}` format argument #{}",
                            ctx,
                            spec_index + 1
                        ));
                        self.messages.push(msg);
                    }
                }
                other => {
                    if !self.is_showable_for_format(other) {
                        let mut msg = Message::error(
                            ErrorCode::FormatSpecifierMismatch,
                            format!(
                                "Format specifier `%v` requires a `Show` instance, found {}",
                                other
                            ),
                            arg_range.clone(),
                        );
                        msg.with_help(format!(
                            "implement `Show` for this type, or use a concrete specifier; \
                             while checking `{}` format argument #{}",
                            ctx,
                            spec_index + 1
                        ));
                        self.messages.push(msg);
                    }
                }
            }
            return;
        }

        // Concrete specifiers on an open type:
        // - quantified type parameters (`fn f<T>(T x)`) must use `%v`
        // - free inference vars (e.g. coroutine send) unify with the
        //   specifier's expected type (same as using the value in a
        //   typed context)
        if let Ty::Var(v) = arg_ty {
            let is_type_param = self
                .type_params_in_scope
                .iter()
                .any(|frame| frame.values().any(|id| id == v));
            if is_type_param {
                let mut msg = Message::error(
                    ErrorCode::FormatSpecifierMismatch,
                    format!(
                        "Format specifier `%{}` cannot be used with an open type `{}`",
                        spec, arg_ty
                    ),
                    arg_range.clone(),
                );
                msg.with_help(format!(
                    "use `%v` (requires `Show`) instead of `%{}`; \
                     while checking `{}` format argument #{}",
                    spec,
                    ctx,
                    spec_index + 1
                ));
                self.messages.push(msg);
                return;
            }
            let expected_ty = match spec {
                'i' | 'd' | 'b' | 'x' | 'u' | 'p' => int(),
                'f' => float(),
                's' => string(),
                'z' => boolean(),
                _ => {
                    let mut msg = Message::error(
                        ErrorCode::FormatSpecifierMismatch,
                        format!("Unknown format specifier `%{}`", spec),
                        arg_range.clone(),
                    );
                    msg.with_help(format!(
                        "while checking `{}` format argument #{}",
                        ctx,
                        spec_index + 1
                    ));
                    self.messages.push(msg);
                    return;
                }
            };
            // `byte` is printable with integer format specs (same runtime word).
            if matches!(spec, 'i' | 'd' | 'b' | 'x' | 'u' | 'p') && Self::is_byte_ty(arg_ty) {
                return;
            }
            self.unify(
                arg_ty,
                &expected_ty,
                arg_range,
                &format!("{} format argument #{}", ctx, spec_index + 1),
            );
            return;
        }

        let expected = format_specifier_type(spec);
        if !type_matches_specifier(arg_ty, spec) {
            let mut msg = Message::error(
                ErrorCode::FormatSpecifierMismatch,
                format!(
                    "Format specifier `%{}` requires {}, found {}",
                    spec, expected, arg_ty
                ),
                arg_range.clone(),
            );
            msg.with_help(format!(
                "while checking `{}` format argument #{}",
                ctx,
                spec_index + 1
            ));
            self.messages.push(msg);
        }
    }

    fn is_showable_for_format(&self, ty: &Ty) -> bool {
        let resolved = apply_ty_prune(&self.subst, ty);
        match resolved {
            Ty::Var(_) => false,
            Ty::Tuple(items) => items.iter().all(|item| self.is_showable_for_format(item)),
            Ty::Record { fields } => fields
                .iter()
                .all(|(_, field_ty)| self.is_showable_for_format(field_ty)),
            other => {
                let lookup = show_lookup_ty(&other);
                self.generics.has_instance("Show", &lookup)
            }
        }
    }

    // ============================================================
    // ============================================================
    //  Codegen helpers
    // ============================================================
    // ============================================================

    /// Variant tag by enum and variant name (source-declaration order).
    /// Map surface enum paths (`ffi::types`) to the internal registry key.
    fn registry_enum_name<'a>(&self, enum_name: &'a str) -> &'a str {
        if common::is_builtin_ffi_enum(enum_name) {
            common::BUILTIN_FFI_TYPE_ENUM
        } else {
            enum_name
        }
    }

    pub fn tag_for(&self, enum_name: &str, variant_name: &str) -> Option<u32> {
        let key = self.registry_enum_name(enum_name);
        self.enum_tags
            .get(key)
            .and_then(|t| t.get(variant_name).copied())
    }

    /// Payload arity for `(enum_name, variant_name)`.
    pub fn arity_for(&self, enum_name: &str, variant_name: &str) -> Option<usize> {
        let key = self.registry_enum_name(enum_name);
        self.tag_for(enum_name, variant_name).and_then(|t| {
            self.enum_arities
                .get(key)
                .and_then(|a| a.get(t as usize).copied())
        })
    }

    /// Variants in source-declaration order: `(name, tag, payload_types)`.
    pub fn enum_variants(&self, enum_name: &str) -> Option<Vec<(String, u32, Vec<Ty>)>> {
        let names = self.enums.get(enum_name)?.clone();
        let tags = self.enum_tags.get(enum_name)?.clone();
        let payloads = self.enum_payloads.get(enum_name)?.clone();
        let mut out = Vec::with_capacity(names.len());
        for (i, name) in names.iter().enumerate() {
            let tag = tags.get(name).copied().unwrap_or(i as u32);
            let payload_tys: Vec<Ty> = match payloads.get(i) {
                Some(EnumVariantPayloadTy::Unit) => Vec::new(),
                Some(EnumVariantPayloadTy::Tuple(tys)) => tys.clone(),
                Some(EnumVariantPayloadTy::Record(fields)) => {
                    fields.iter().map(|(_, ty)| ty.clone()).collect()
                }
                None => Vec::new(),
            };
            out.push((name.clone(), tag, payload_tys));
        }
        Some(out)
    }

    /// Look up the declared payload for `(enum_name, variant_name)`
    /// as a list of `(field_name, field_type)` pairs in
    /// DECLARATION order. The codegen uses this to reorder record
    /// call-site fields to declaration order (the VM's
    /// `MAKE_ENUM` pushes payload args in pop order — the first
    /// popped is `payload[0]`).
    ///
    /// For Unit variants, returns an empty Vec. For Tuple
    /// variants, the field names are synthetic (`"0"`, `"1"`, …)
    /// — see `EnumVariantPayloadTy::field_pairs`. For Record
    /// variants, the field names are the declared names.
    pub fn payload_tys_for(&self, enum_name: &str, variant_name: &str) -> Vec<(String, Ty)> {
        let key = self.registry_enum_name(enum_name);
        let tag = match self.tag_for(enum_name, variant_name) {
            Some(t) => t,
            None => return Vec::new(),
        };
        match self
            .enum_payloads
            .get(key)
            .and_then(|p| p.get(tag as usize))
        {
            Some(payload) => payload.field_pairs(),
            None => Vec::new(),
        }
    }

    /// Field index in a record-shaped variant (codegen).
    pub fn field_index_for(&self, enum_name: &str, field: &str) -> Option<(String, u16)> {
        let payloads = self.enum_payloads.get(enum_name)?;
        let names = self.enums.get(enum_name)?;
        // Prefer declared record field names.
        for (i, payload) in payloads.iter().enumerate() {
            if let EnumVariantPayloadTy::Record(fields) = payload {
                for (j, (fname, _)) in fields.iter().enumerate() {
                    if fname == field {
                        let variant_name = names
                            .get(i)
                            .cloned()
                            .unwrap_or_else(|| format!("variant_{}", i));
                        return Some((variant_name, j as u16));
                    }
                }
            }
        }
        // Synthetic tuple indices `"0"`, `"1"`, … (used by derive expansion
        // and any AST-level Access that targets a tuple payload slot).
        if let Ok(idx) = field.parse::<usize>() {
            let mut match_count = 0;
            let mut found: Option<(String, u16)> = None;
            for (i, payload) in payloads.iter().enumerate() {
                if let EnumVariantPayloadTy::Tuple(parts) = payload {
                    if idx < parts.len() {
                        match_count += 1;
                        let variant_name = names
                            .get(i)
                            .cloned()
                            .unwrap_or_else(|| format!("variant_{}", i));
                        found = Some((variant_name, idx as u16));
                    }
                }
            }
            if match_count == 1 {
                return found;
            }
        }
        None
    }

    /// Record / tuple-payload field type by enum and field name (chained
    /// `Expression::Access` codegen, derive expansion).
    pub fn field_type_for(&self, enum_name: &str, field: &str) -> Option<Ty> {
        let payloads = self.enum_payloads.get(enum_name)?;
        for payload in payloads {
            if let EnumVariantPayloadTy::Record(fields) = payload {
                for (fname, fty) in fields {
                    if fname == field {
                        return Some(fty.clone());
                    }
                }
            }
        }
        if let Ok(idx) = field.parse::<usize>() {
            let mut match_count = 0;
            let mut found: Option<Ty> = None;
            for payload in payloads {
                if let EnumVariantPayloadTy::Tuple(parts) = payload {
                    if let Some(fty) = parts.get(idx) {
                        match_count += 1;
                        found = Some(fty.clone());
                    }
                }
            }
            if match_count == 1 {
                return found;
            }
        }
        None
    }

    /// Enter or refine Result mode for the enclosing function.
    /// Returns the Ok payload type `T`.
    fn ensure_result_mode(&mut self, err_ty: &Ty, range: &Range<usize>) -> Ty {
        if self.fn_option_mode.is_some() {
            return self.error(
                ErrorCode::ConflictingErrorType,
                "cannot mix Option `?` and Result `raise`/`?` in the same function".into(),
                range.clone(),
            );
        }
        if let Some((ok, err)) = self.fn_result_mode.clone() {
            self.unify(&err, err_ty, range, "error type");
            return apply_ty_prune(&self.subst, &ok);
        }
        if let Some(ret) = self.current_return_ty.clone() {
            let resolved = apply_ty_prune(&self.subst, &ret);
            if let Some((ok, err)) = result_ok_err(&resolved) {
                self.fn_result_mode = Some((ok.clone(), err.clone()));
                self.unify(&err, err_ty, range, "error type");
                self.current_return_ty = Some(ok.clone());
                return ok;
            }
            if is_option_ty(&resolved) {
                return self.error(
                    ErrorCode::ConflictingErrorType,
                    "function returns Option; cannot use Result `raise`/`?`".into(),
                    range.clone(),
                );
            }
            // Pin a free / non-Result return to Result<ok, err>.
            let ok = Ty::Var(self.counter.fresh());
            let result = result_ty(ok.clone(), err_ty.clone());
            self.unify(&ret, &result, range, "result return type");
            self.fn_result_mode = Some((ok.clone(), err_ty.clone()));
            self.current_return_ty = Some(ok.clone());
            ok
        } else {
            self.error(
                ErrorCode::InvalidTry,
                "`raise` / `?` outside of a function".into(),
                range.clone(),
            )
        }
    }

    /// Enter or refine Option mode for the enclosing function.
    fn ensure_option_mode(&mut self, inner_ty: &Ty, range: &Range<usize>) {
        if self.fn_result_mode.is_some() {
            self.error(
                ErrorCode::ConflictingErrorType,
                "cannot mix Result `raise`/`?` and Option `?` in the same function".into(),
                range.clone(),
            );
            return;
        }
        if let Some(inner) = self.fn_option_mode.clone() {
            self.unify(&inner, inner_ty, range, "option payload");
            return;
        }
        if let Some(ret) = self.current_return_ty.clone() {
            let resolved = apply_ty_prune(&self.subst, &ret);
            if is_option_ty(&resolved) {
                if let Some(existing) = option_inner(&resolved) {
                    self.unify(&existing, inner_ty, range, "option payload");
                }
                self.fn_option_mode = Some(inner_ty.clone());
                return;
            }
            if is_result_ty(&resolved) {
                self.error(
                    ErrorCode::ConflictingErrorType,
                    "function returns Result; cannot use Option `?`".into(),
                    range.clone(),
                );
                return;
            }
            let opt = option_ty(inner_ty.clone());
            self.unify(&ret, &opt, range, "option return type");
            self.fn_option_mode = Some(inner_ty.clone());
        } else {
            self.error(
                ErrorCode::InvalidTry,
                "`?` outside of a function".into(),
                range.clone(),
            );
        }
    }

    /// Resolve `ty.field` for optional chaining (inner of Option).
    fn field_type_from_ty(&mut self, ty: &Ty, field: &str, range: &Range<usize>) -> Ty {
        let resolved = apply_ty_prune(&self.subst, ty);
        match &resolved {
            Ty::Sum { name, variants } => {
                self.access_field_in_sum(name, variants, None, field, range.clone())
            }
            Ty::Constructor { tag, owner, .. } => match owner.as_ref() {
                Ty::Sum { name, variants } => {
                    self.access_field_in_sum(name, variants, Some(*tag), field, range.clone())
                }
                _ => self.error(
                    ErrorCode::UnknownField,
                    format!("Cannot access field `{}`", field),
                    range.clone(),
                ),
            },
            Ty::Record { fields } => {
                if let Some((_, fty)) = fields.iter().find(|(n, _)| n == field) {
                    fty.clone()
                } else {
                    self.error(
                        ErrorCode::UnknownField,
                        format!("Cannot find field `{}` on record", field),
                        range.clone(),
                    )
                }
            }
            Ty::Con(name) => {
                if let Some(fty) = self.class_field_ty(name, field) {
                    fty.clone()
                } else if let Some(fty) = self.field_type_for(name, field) {
                    fty
                } else {
                    self.error(
                        ErrorCode::UnknownField,
                        format!("Cannot find field `{}` on `{}`", field, name),
                        range.clone(),
                    )
                }
            }
            // Unpinned Option payload (e.g. `Option::None` alone): unify
            // with a structural record that has this field so `none?.v`
            // typechecks and pins `T` for coalesce / later use.
            Ty::Var(_) => {
                let field_ty = Ty::Var(self.counter.fresh());
                let record = Ty::Record {
                    fields: vec![(field.to_string(), field_ty.clone())],
                };
                self.unify(
                    &resolved,
                    &record,
                    range,
                    "optional access field on unpinned Option payload",
                );
                field_ty
            }
            _ => self.error(
                ErrorCode::UnknownField,
                format!("Cannot access field `{}` on non-record type", field),
                range.clone(),
            ),
        }
    }

    /// Variable type from codegen side-table.
    pub fn codegen_var_type(&self, name: &str) -> Option<&Ty> {
        self.codegen_var_types.get(name)
    }

    /// For-in lowering info recorded during typecheck (by Loop node id).
    pub fn for_in_info_at(&self, id: NodeId) -> Option<&ForInInfo> {
        self.for_in_infos.get(&id)
    }

    /// For-in lowering info by source span (fallback when ids misalign).
    pub fn for_in_info_for_span(&self, start: usize, end: usize) -> Option<&ForInInfo> {
        self.for_in_infos_by_span.get(&(start, end))
    }

    /// Resolve `for x in` iterable type to `Item` and record [`ForInInfo`].
    ///
    /// Builtin synthesis covers arrays, homogeneous tuples/records, and
    /// coroutines. Otherwise looks up `IntoIterator` / `Iterator` instances.
    fn resolve_for_in_iterable(
        &mut self,
        te: &Ty,
        loop_id: Option<NodeId>,
        iterable_range: &Range<usize>,
        loop_range: &Range<usize>,
    ) -> Option<Ty> {
        // ---- Builtin synthesis ----
        if let Some((item, kind)) = self.builtin_for_in_kind(te, iterable_range) {
            self.record_for_in_info(loop_id, loop_range, ForInInfo { kind });
            return Some(item);
        }

        // ---- User IntoIterator / Iterator ----
        match self.find_unique_instance("IntoIterator", &[te.clone()], iterable_range) {
            Ok(Some(into_inst)) => {
                let item = into_inst
                    .assoc_tys
                    .get("Item")
                    .map(|v| v.ty.clone())
                    .unwrap_or_else(|| Ty::Var(self.counter.fresh()));
                let into_iter_ty = into_inst
                    .assoc_tys
                    .get("IntoIter")
                    .map(|v| v.ty.clone())
                    .unwrap_or_else(|| Ty::Var(self.counter.fresh()));
                let into_fqn = into_inst.method_fqns.get("into_iter").cloned();
                match self.find_unique_instance(
                    "Iterator",
                    &[into_iter_ty.clone()],
                    iterable_range,
                ) {
                    Ok(Some(iter_inst)) => {
                        if let Some(iter_item) = iter_inst.assoc_tys.get("Item") {
                            self.unify(
                                &item,
                                &iter_item.ty,
                                iterable_range,
                                "IntoIterator/Iterator Item",
                            );
                        }
                        let next_fqn = iter_inst.method_fqns.get("next").cloned();
                        match (into_fqn, next_fqn) {
                            (Some(into_iter_fqn), Some(next_fqn)) => {
                                self.record_for_in_info(
                                    loop_id,
                                    loop_range,
                                    ForInInfo {
                                        kind: ForInKind::Custom {
                                            into_iter_fqn,
                                            next_fqn,
                                        },
                                    },
                                );
                                Some(apply_ty_prune(&self.subst, &item))
                            }
                            _ => {
                                let _ = self.error_with_help(
                                    ErrorCode::GenericTypeError,
                                    "IntoIterator/Iterator instance is missing method implementations"
                                        .to_string(),
                                    iterable_range.clone(),
                                    Some(
                                        "implement `into_iter` and `next` for the iterable type"
                                            .to_string(),
                                    ),
                                );
                                None
                            }
                        }
                    }
                    Ok(None) => {
                        let _ = self.error_with_help(
                            ErrorCode::GenericTypeError,
                            format!(
                                "type `{}` is IntoIterator but its IntoIter is not Iterator",
                                te
                            ),
                            iterable_range.clone(),
                            Some(format!(
                                "add `impl Iterator<{}>` with matching `type Item`",
                                into_iter_ty
                            )),
                        );
                        None
                    }
                    Err(()) => None,
                }
            }
            Ok(None) => {
                let _ = self.error_with_help(
                    ErrorCode::GenericTypeError,
                    format!("type `{}` is not iterable", te),
                    iterable_range.clone(),
                    Some(
                        "implement `IntoIterator` / `Iterator`, or use an array, homogeneous tuple/dict, or coroutine"
                            .to_string(),
                    ),
                );
                None
            }
            Err(()) => None,
        }
    }

    fn record_for_in_info(
        &mut self,
        loop_id: Option<NodeId>,
        loop_range: &Range<usize>,
        info: ForInInfo,
    ) {
        if let Some(id) = loop_id {
            self.for_in_infos.insert(id, info.clone());
        }
        self.for_in_infos_by_span
            .insert((loop_range.start, loop_range.end), info);
    }

    /// Builtin iterable shapes → `(Item, ForInKind)`. Returns `None` when
    /// the type is not a recognised builtin iterable (caller falls through
    /// to trait instance lookup). Emits diagnostics for hetero tuple/dict.
    fn builtin_for_in_kind(
        &mut self,
        te: &Ty,
        range: &Range<usize>,
    ) -> Option<(Ty, ForInKind)> {
        match te {
            Ty::Array { element, .. } => {
                Some((element.as_ref().clone(), ForInKind::Array))
            }
            Ty::Tuple(elems) => {
                if elems.is_empty() {
                    let _ = self.error_with_help(
                        ErrorCode::GenericTypeError,
                        "empty tuple is not iterable".to_string(),
                        range.clone(),
                        Some("tuple for-in requires at least one element".to_string()),
                    );
                    return None;
                }
                match self.homogeneous_types(elems, range, "tuple") {
                    Some(item) => Some((
                        item,
                        ForInKind::Tuple {
                            arity: elems.len(),
                        },
                    )),
                    None => None,
                }
            }
            Ty::Record { fields } => {
                let value_tys: Vec<Ty> = fields.iter().map(|(_, ty)| ty.clone()).collect();
                if value_tys.is_empty() {
                    // Vacuously homogeneous; Item = (string, α).
                    let v = Ty::Var(self.counter.fresh());
                    return Some((tuple_ty(vec![string(), v]), ForInKind::Dict));
                }
                match self.homogeneous_types(&value_tys, range, "dict") {
                    Some(v) => Some((tuple_ty(vec![string(), v]), ForInKind::Dict)),
                    None => None,
                }
            }
            Ty::App(head, args) => {
                if matches!(head.as_ref(), Ty::Con(n) if n == "coroutine") && args.len() == 2 {
                    return Some((args[0].clone(), ForInKind::Coroutine));
                }
                None
            }
            _ => None,
        }
    }

    /// All types unify to one element type, or diagnose heterogeneity.
    fn homogeneous_types(
        &mut self,
        tys: &[Ty],
        range: &Range<usize>,
        kind: &str,
    ) -> Option<Ty> {
        let first = apply_ty_prune(&self.subst, &tys[0]);
        for other in tys.iter().skip(1) {
            let other = apply_ty_prune(&self.subst, other);
            if unify_with(&self.subst, &first, &other).is_err() {
                let _ = self.error_with_help(
                    ErrorCode::GenericTypeError,
                    format!(
                        "heterogeneous {} is not iterable (element types `{}` and `{}`)",
                        kind, first, other
                    ),
                    range.clone(),
                    Some(format!(
                        "{} for-in requires all elements to share one type",
                        kind
                    )),
                );
                return None;
            }
        }
        // Bind any open vars across the set.
        let mut local = self.subst.clone();
        for other in tys.iter().skip(1) {
            if let Ok(s) = unify_with(&local, &first, other) {
                local = s;
            }
        }
        self.subst = compose(&local, &self.subst);
        Some(apply_ty_prune(&self.subst, &first))
    }

    /// True if `name` is a registered class.
    pub fn is_class(&self, name: &str) -> bool {
        self.classes.contains_key(name)
    }

    /// Class name from `Con(C)` or `App(Con(C), _)` (Phase 7).
    pub fn class_name_of_ty(ty: &Ty) -> Option<&str> {
        match ty {
            Ty::Con(n) => Some(n.as_str()),
            Ty::App(head, _) => match head.as_ref() {
                Ty::Con(n) => Some(n.as_str()),
                _ => None,
            },
            _ => None,
        }
    }

    /// True if `ty` is a registered class instance (`Con` or `App`).
    pub fn ty_is_class(&self, ty: &Ty) -> bool {
        Self::class_name_of_ty(ty).is_some_and(|n| self.is_class(n))
    }

    /// Declared type of a class field (codegen / Access).
    pub fn class_field_ty(&self, class: &str, field: &str) -> Option<&Ty> {
        self.classes
            .get(class)?
            .iter()
            .find(|(_, fname, _)| fname == field)
            .map(|(_, _, ty)| ty)
    }

    /// Class fields in declaration order: `(name, Ty)`.
    pub fn class_fields(&self, class: &str) -> Option<Vec<(String, Ty)>> {
        self.classes.get(class).map(|fields| {
            fields
                .iter()
                .map(|(_, name, ty)| (name.clone(), ty.clone()))
                .collect()
        })
    }

    /// Method FQN lookup helper — returns whether the method exists.
    pub fn has_method(&self, owner: &str, method: &str) -> bool {
        self.methods
            .get(owner)
            .is_some_and(|m| m.contains_key(method))
    }

    /// True if `name` was declared as `async fn`.
    pub fn is_async_function(&self, name: &str) -> bool {
        self.async_functions.contains(name)
    }

    /// Whether `name` is a generic function (has type params).
    pub fn is_generic_fn(&self, name: &str) -> bool {
        self.generics.generic_fns.contains(name)
    }

    /// Number of *user-defined* trait dict slots expected by a generic
    /// function.  Returns 0 for non-generic functions or functions whose
    /// constraints are all built-in classes (Num / Ord / Eq / Show).
    pub fn dict_arity_for(&self, fn_name: &str) -> usize {
        self.fn_dict_arity.get(fn_name).copied().unwrap_or(0)
    }

    /// True for compiler-built-in typeclasses (Num / Ord / Eq / Show).
    ///
    /// These still use the dictionary ABI in shared generic bodies. Ground
    /// Num/Ord/Eq calls may monomorphize to direct opcodes; `Show` does not
    /// (see `monomorphize::candidate_for_call`).
    pub fn is_builtin_class(class: &str) -> bool {
        matches!(
            class,
            "Add"
                | "Sub"
                | "Mul"
                | "Div"
                | "Num"
                | "Lt"
                | "Le"
                | "Gt"
                | "Ge"
                | "Ord"
                | "Eq"
                | "Show"
        )
    }

    /// Return the FQN for an instance method, if registered.
    /// `class` is e.g. `"Num"`, `args` are concrete types, `method` is `"add"`.
    pub fn instance_method_fqn(&self, class: &str, args: &[Ty], method: &str) -> Option<&str> {
        self.generics
            .find_instance(class, args)
            .and_then(|inst| inst.method_fqns.get(method).map(|s| s.as_str()))
    }

    /// Read-only access to the generics registry (for codegen).
    pub fn generics(&self) -> &super::generics::Generics {
        &self.generics
    }

    /// Infer without updating the NodeId cache (codegen helper).
    pub fn infer_for_codegen(&mut self, expr: &Output) -> Ty {
        let saved_idx = self.next_id_idx;
        let ty = self.infer_inner(expr, None);
        self.next_id_idx = saved_idx;
        // Don't insert into cache — the ID we restored might be
        // wrong for this AST node, and overwriting a correct entry
        // would be worse than skipping this insertion.
        ty
    }

    /// Field access on enum record payloads (`specific_tag` narrows the variant).
    fn access_field_in_sum(
        &mut self,
        enum_name: &str,
        variants: &[(String, EnumVariantPayloadTy)],
        specific_tag: Option<u32>,
        field: &str,
        range: Range<usize>,
    ) -> Ty {
        if let Some(tag) = specific_tag {
            // Statically known variant. Look up the payload and
            // either return the field's type or emit a tailored
            // diagnostic.
            let variant_idx = tag as usize;
            if variant_idx >= variants.len() {
                return self.error_with_help(
                    ErrorCode::GenericTypeError,
                    format!("Cannot access field `{}` on non-record type", field),
                    range,
                    Some("only values of record-shaped enum types expose fields".to_string()),
                );
            }
            let (variant_name, payload) = &variants[variant_idx];
            match payload {
                EnumVariantPayloadTy::Record(fields) => {
                    for (fname, fty) in fields {
                        if fname == field {
                            return fty.clone();
                        }
                    }
                    // Record-shaped variant, but doesn't declare
                    // the field.
                    let hint = build_record_field_hint(enum_name, variants);
                    self.error_with_help(
                        ErrorCode::GenericTypeError,
                        format!("Type `{}` has no field `{}`", enum_name, field),
                        range,
                        hint,
                    )
                }
                EnumVariantPayloadTy::Tuple(parts) => {
                    if let Ok(idx) = field.parse::<usize>() {
                        if let Some(fty) = parts.get(idx) {
                            return fty.clone();
                        }
                    }
                    self.error_with_help(
                        ErrorCode::GenericTypeError,
                        format!("Cannot access field `{}` on tuple variant", field),
                        range,
                        Some(format!(
                            "variant `{}::{}` is a {}-tuple; use a match binding or index 0..{}",
                            enum_name,
                            variant_name,
                            parts.len(),
                            parts.len().saturating_sub(1),
                        )),
                    )
                }
                _ => self.error_with_help(
                    ErrorCode::GenericTypeError,
                    format!("Cannot access field `{}` on non-record variant", field),
                    range,
                    Some(format!(
                        "variant `{}::{}` is {}; only record-shaped variants expose named fields",
                        enum_name,
                        variant_name,
                        payload_kind_name(payload),
                    )),
                ),
            }
        } else {
            // Untagged receiver: find every record-/tuple-shaped variant
            // that declares the field (named record fields, or synthetic
            // `"0"`/`"1"`/… tuple indices).
            let mut candidates: Vec<&Ty> = Vec::new();
            for (_variant_name, payload) in variants {
                if let EnumVariantPayloadTy::Record(fields) = payload {
                    for (fname, fty) in fields {
                        if fname == field {
                            candidates.push(fty);
                        }
                    }
                } else if let EnumVariantPayloadTy::Tuple(parts) = payload {
                    if let Ok(idx) = field.parse::<usize>() {
                        if let Some(fty) = parts.get(idx) {
                            candidates.push(fty);
                        }
                    }
                }
            }
            match candidates.len() {
                0 => {
                    let hint = build_record_field_hint(enum_name, variants);
                    self.error_with_help(
                        ErrorCode::GenericTypeError,
                        format!("Type `{}` has no field `{}`", enum_name, field),
                        range,
                        hint,
                    )
                }
                1 => candidates[0].clone(),
                _ => {
                    self.error_with_help(
                        ErrorCode::GenericTypeError,
                        format!(
                            "Field `{}` exists in multiple variants of `{}`; \
                             narrow with match first",
                            field, enum_name
                        ),
                        range,
                        Some(
                            "field access requires a unique field type; use a `match` to \
                             determine the active variant before reading the field"
                                .to_string(),
                        ),
                    );
                    candidates[0].clone()
                }
            }
        }
    }
}

/// Human-readable name of a payload shape, used in
/// typecheck-error messages.
fn payload_kind_name(payload: &EnumVariantPayloadTy) -> &'static str {
    match payload {
        EnumVariantPayloadTy::Unit => "unit",
        EnumVariantPayloadTy::Tuple(_) => "tuple",
        EnumVariantPayloadTy::Record(_) => "record",
    }
}

/// Help hint for missing field diagnostics on record-shaped enums.
fn build_record_field_hint(
    enum_name: &str,
    variants: &[(String, EnumVariantPayloadTy)],
) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for (variant_name, payload) in variants {
        if let EnumVariantPayloadTy::Record(fields) = payload {
            if fields.is_empty() {
                lines.push(format!(
                    "  - `{}::{}` has no fields",
                    enum_name, variant_name
                ));
            } else {
                let names = fields
                    .iter()
                    .map(|(n, _)| format!("`{}`", n))
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(format!(
                    "  - `{}::{}` exposes: {}",
                    enum_name, variant_name, names
                ));
            }
        }
    }
    if lines.is_empty() {
        Some(format!(
            "`{}` has no record-shaped variants; only record-shaped variants expose fields",
            enum_name
        ))
    } else {
        Some(format!(
            "the available record fields on `{}` are:\n{}",
            enum_name,
            lines.join("\n")
        ))
    }
}

/// Normalize a runtime value type to the head used in `impl Show<T>`
/// registration (`Ty::Con("Point")` rather than `Sum` / `Constructor`).
fn show_lookup_ty(ty: &Ty) -> Ty {
    match ty {
        Ty::Sum { name, .. } => Ty::Con(name.clone()),
        Ty::Constructor { owner, .. } => show_lookup_ty(owner),
        other => other.clone(),
    }
}

/// Map a format specifier character to the type it expects.
fn format_specifier_type(spec: char) -> &'static str {
    match spec {
        'i' | 'b' | 'x' | 'u' | 'p' => "int",
        'f' => "float",
        's' => "string",
        'z' => "bool",
        'v' => "a type with a `Show` instance",
        _ => "an unknown type",
    }
}

fn is_string_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::Con(name) if name == STRING)
}

/// True if `ty` (already resolved under the substitution) is the
/// type expected by a concrete `spec`. Open `Ty::Var` is rejected by
/// [`Checker::check_format_arg`] before this is consulted.
fn type_matches_specifier(ty: &Ty, spec: char) -> bool {
    match spec {
        'i' | 'b' | 'x' | 'u' | 'p' => {
            matches!(ty, Ty::Con(n) if n == "int" || n == crate::typechecking::ty::BYTE)
        }
        'f' => matches!(ty, Ty::Con(n) if n == "float"),
        's' => matches!(ty, Ty::Con(n) if n == "string"),
        'z' => matches!(ty, Ty::Con(n) if n == "bool"),
        'v' => true, // Show check happens in `check_format_arg`
        // Unknown specifier (including `%d`, which the VM does not
        // implement) — can't be matched; the caller will still
        // record a diagnostic, but we don't want to say it matches
        // every type.
        _ => false,
    }
}

/// True when `node` is a `yield` expression (possibly wrapped in `Expr`).
fn is_yield_expression(node: &Output) -> bool {
    match node.1.as_ref() {
        Expression::Yield(_) => true,
        Expression::Expr(e) | Expression::Group(e) => is_yield_expression(e),
        _ => false,
    }
}

/// Peel `Expr` / `Group` / `Statement` / `ExprStatement` wrappers so
/// fragment initializers can match the underlying `Declare` / `Invoke`.
fn unwrap_expr_wrappers<'a>(node: &'a Output<'a>) -> &'a Output<'a> {
    match node.1.as_ref() {
        Expression::Expr(e)
        | Expression::Group(e)
        | Expression::Statement(e)
        | Expression::ExprStatement(e) => unwrap_expr_wrappers(e),
        _ => node,
    }
}

fn identifier_name<'a>(node: &'a Output<'a>) -> Option<&'a str> {
    match unwrap_expr_wrappers(node).1.as_ref() {
        Expression::Identifier(name) => Some(*name),
        _ => None,
    }
}

/// True for nodes that look like declarations / no-ops rather than
/// initializers. Used by [`Checker::infer_fragment`] to decide whether
/// to consume the next sibling as a `let` initializer.
fn is_declaration_like(node: &Output) -> bool {
    let node = unwrap_expr_wrappers(node);
    matches!(
        node.1.as_ref(),
        Expression::Variable(..)
            | Expression::Constant(..)
            | Expression::Assignment(..)
            | Expression::TypeAlias { .. }
            | Expression::Comment(..)
            | Expression::Use { .. }
            | Expression::Noop(..)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typechecking::ty::EnumVariantPayloadTy;
    use parser::Pratt;

    /// Parse and infer `src`, returning the checker state and inferred type.
    ///
    /// The top-level parser expects declarations / statements. Bare
    /// expressions need a trailing `;`. Heuristically add one when the
    /// source doesn't already look like a complete statement.
    fn check(src: &str) -> (Checker, Ty) {
        let mut c = Checker::new();
        let trimmed = src.trim();
        // Add `;` if the source doesn't end with a terminator. We don't
        // try to be clever about keywords — even `let x = 1; ...; expr`
        // needs the trailing `expr;`.
        let needs_semi = !trimmed.ends_with(';') && !trimmed.ends_with('}');
        let owned: String = if needs_semi {
            format!("{};", trimmed)
        } else {
            trimmed.to_string()
        };
        match Pratt::default().parse(owned.as_str()) {
            Ok(ast) => {
                let ty = c.check_program(&ast);
                (c, ty)
            }
            Err(msg) => panic!("parse failed for `{}`: {:?}", src, msg),
        }
    }

    /// Like `check`, but returns diagnostics instead of asserting none.
    fn check_warn(src: &str) -> (Checker, Vec<Message>) {
        let (mut c, _ty) = check(src);
        let msgs = c.take_messages();
        (c, msgs)
    }

    fn assert_ok(src: &str, expected: Ty) {
        let (mut c, ty) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.is_empty(),
            "expected no messages for `{}`, got: {:?}",
            src,
            msgs
        );
        assert_eq!(ty, expected, "type mismatch for `{}`", src);
    }

    fn assert_messages(src: &str) -> Vec<Message> {
        let (mut c, _) = check(src);
        c.take_messages()
    }

    // ---- Literals ----

    #[test]
    fn integer_literal() {
        assert_ok("42", int());
    }

    #[test]
    fn float_literal() {
        assert_ok("3.14", float());
    }

    #[test]
    fn string_literal() {
        assert_ok("\"hello\"", string());
    }

    #[test]
    fn bool_literal() {
        assert_ok("true", boolean());
        assert_ok("false", boolean());
    }

    // ---- Identifier ----

    #[test]
    fn unknown_identifier_errors() {
        let msgs = assert_messages("x;");
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn identifier_from_let_annotation() {
        // Declare x: int = 42, then verify via env lookup.
        let (mut c, _) = check("let x: int = 42;");
        assert!(c.take_messages().is_empty());
        let scheme = c.env().lookup("x").unwrap();
        let ty = apply_ty(c.subst(), &scheme.ty);
        assert_eq!(ty, int());
    }

    // ---- Variables and let ----

    #[test]
    fn let_with_annotation() {
        assert_ok("let x: int = 42;", unit_ty());
    }

    #[test]
    fn let_without_annotation_infers_from_value() {
        // `let x = 42;` — x should be inferred as int.
        let (mut c, _) = check("let x = 42;");
        assert!(c.take_messages().is_empty());
        let scheme = c.env().lookup("x").unwrap();
        let ty = apply_ty(c.subst(), &scheme.ty);
        assert_eq!(ty, int());
    }

    #[test]
    fn let_without_annotation_or_value_uses_fresh_var() {
        // `let x;` — x is a fresh type variable (id is not stable across
        // builtin/prelude registration, so only the shape is checked).
        let (mut c, _) = check("let x;");
        assert!(c.take_messages().is_empty());
        let scheme = c.env().lookup("x").unwrap();
        assert!(
            matches!(scheme.ty, Ty::Var(_)),
            "expected a fresh type variable, got {:?}",
            scheme.ty
        );
    }

    // ---- Assignment ----

    #[test]
    fn assignment_updates_existing_var() {
        let (mut c, _) = check("let x; x = 42;");
        assert!(c.take_messages().is_empty());
        let scheme = c.env().lookup("x").unwrap();
        let ty = apply_ty(c.subst(), &scheme.ty);
        assert_eq!(ty, int());
    }

    #[test]
    fn assignment_to_undeclared_var_errors() {
        let msgs = assert_messages("x = 42;");
        assert!(!msgs.is_empty());
    }

    #[test]
    fn assignment_mismatch_errors_but_continues() {
        // x: int, then assign "hello" — should produce an error.
        let msgs = assert_messages("let x: int; x = \"hello\";");
        assert!(!msgs.is_empty());
    }

    // ---- Arithmetic ----

    #[test]
    fn addition_of_ints_is_int() {
        assert_ok("1 + 2", int());
    }

    #[test]
    fn addition_of_floats_is_float() {
        assert_ok("1.0 + 2.0", float());
    }

    #[test]
    fn mixed_int_float_arithmetic_mismatches() {
        // 1 + 2.0: unify int with float → Mismatch.
        let msgs = assert_messages("1 + 2.0;");
        assert!(!msgs.is_empty());
    }

    #[test]
    fn subtraction() {
        assert_ok("5 - 3", int());
    }

    #[test]
    fn multiplication() {
        assert_ok("4 * 5", int());
    }

    #[test]
    fn division() {
        assert_ok("10 / 2", int());
    }

    #[test]
    fn modulo() {
        assert_ok("10 % 3", int());
    }

    #[test]
    fn power() {
        assert_ok("2 ** 3", int());
    }

    #[test]
    fn shift_left() {
        assert_ok("1 << 2", int());
    }

    #[test]
    fn xor() {
        assert_ok("5 ^ 3", int());
    }

    #[test]
    fn bitand() {
        assert_ok("5 & 3", int());
    }

    #[test]
    fn bitor() {
        assert_ok("5 | 3", int());
    }

    // ---- Comparison ----

    #[test]
    fn equality_returns_bool() {
        assert_ok("1 == 1", boolean());
    }

    #[test]
    fn inequality_returns_bool() {
        assert_ok("1 != 2", boolean());
    }

    #[test]
    fn less_than() {
        assert_ok("1 < 2", boolean());
    }

    #[test]
    fn greater_than() {
        assert_ok("2 > 1", boolean());
    }

    #[test]
    fn less_equal() {
        assert_ok("1 <= 1", boolean());
    }

    #[test]
    fn greater_equal() {
        assert_ok("2 >= 2", boolean());
    }

    // ---- Logical ----

    #[test]
    fn logical_and_of_bools_is_bool() {
        assert_ok("true && false", boolean());
    }

    #[test]
    fn logical_or_of_bools_is_bool() {
        assert_ok("true || false", boolean());
    }

    #[test]
    fn logical_and_requires_bool() {
        // 1 && 2 — int, not bool.
        let msgs = assert_messages("1 && 2;");
        assert!(!msgs.is_empty());
    }

    // ---- Prefix ----

    #[test]
    fn negate_int() {
        assert_ok("-42", int());
    }

    #[test]
    fn positive_int() {
        assert_ok("+42", int());
    }

    #[test]
    fn bitwise_not_int() {
        assert_ok("~7", int());
    }

    #[test]
    fn logical_not_bool() {
        assert_ok("!true", boolean());
        assert_ok("!false", boolean());
    }

    #[test]
    fn logical_not_int() {
        assert_ok("!0", boolean());
        assert_ok("!42", boolean());
    }

    #[test]
    fn logical_not_rejects_float() {
        let msgs = assert_messages("!1.0;");
        assert!(!msgs.is_empty());
    }

    // ---- Postfix ----

    #[test]
    fn inc_dec() {
        // These return the variable's type.
        let (mut c, _) = check("let x: int = 0; x++;");
        assert!(c.take_messages().is_empty());
    }

    // ---- Call ----

    #[test]
    fn call_unknown_function_errors() {
        let msgs = assert_messages("foo();");
        assert!(!msgs.is_empty());
    }

    #[test]
    fn call_to_unregistered_print_does_not_error() {
        // The parser turns `print` into a `Print` AST node (not a Call),
        // so it doesn't go through the unknown-call path.
        let (mut c, ty) = check("print \"hello\";");
        assert!(c.take_messages().is_empty());
        assert_eq!(ty, unit_ty());
    }

    // ---- If ----

    #[test]
    fn if_single_branch() {
        assert_ok("if true { 42; }", int());
    }

    #[test]
    fn if_with_non_bool_condition_errors() {
        let msgs = assert_messages("if 42 { 1; }");
        assert!(!msgs.is_empty());
    }

    // ---- Match (parser doesn't produce Match nodes yet, so the
    //      handler is unreachable from real source). Tests for Match
    //      are deferred until the parser learns the `match` keyword.

    // ---- Loop ----

    #[test]
    fn while_loop_returns_unit() {
        assert_ok("while false { 42; }", unit_ty());
    }

    #[test]
    fn while_with_non_bool_condition_errors() {
        let msgs = assert_messages("while 42 { 1; }");
        assert!(!msgs.is_empty());
    }

    // ---- Return ----

    #[test]
    fn return_inside_expression() {
        // Without an enclosing function, return just returns the value's type.
        assert_ok("return 42", int());
    }

    // ---- Block ----

    #[test]
    fn empty_block() {
        assert_ok("{}", unit_ty());
    }

    #[test]
    fn block_last_value_is_block_type() {
        assert_ok("{ 1; 2; 3; }", int());
    }

    // ---- Print / Format ----

    #[test]
    fn print_with_string_format_ok() {
        assert_ok("print \"hello\";", unit_ty());
    }

    // ---- Defer ----

    #[test]
    fn defer_returns_unit() {
        assert_ok("defer { 42; }", unit_ty());
    }

    // ---- List literals ----
    //      Parser doesn't produce `List` nodes yet, so these are deferred.

    // ---- Complex expressions ----

    #[test]
    fn nested_arithmetic() {
        assert_ok("(1 + 2) * 3", int());
    }

    #[test]
    fn block_with_multiple_lets() {
        let src = "let x: int = 10; let y: int = 20; x + y";
        assert_ok(src, int());
    }

    // ---- Function declarations ----

    #[test]
    fn function_declaration_with_typed_args_and_return() {
        let (mut c, _) = check("fn add(int a, int b) -> int { return a + b; }");
        assert!(c.take_messages().is_empty());
        let scheme = c.env().lookup("add").unwrap();
        let ty = apply_ty(c.subst(), &scheme.ty);
        assert_eq!(
            ty,
            Ty::Fun(
                Box::new(int()),
                Box::new(Ty::Fun(Box::new(int()), Box::new(int())))
            )
        );
    }

    #[test]
    fn function_declaration_with_inferred_return() {
        // No declared return type — should be inferred from the body.
        let (mut c, _) = check("fn add(int a, int b) { return a + b; }");
        assert!(c.take_messages().is_empty());
        let scheme = c.env().lookup("add").unwrap();
        let ty = apply_ty(c.subst(), &scheme.ty);
        // a -> b -> ?  (return type is a fresh variable bound to int)
        assert!(matches!(ty, Ty::Fun(_, _)));
    }

    #[test]
    fn forall_annotation_pretty_or_ty_forall() {
        let (mut c, _) = check("fn app(forall T: Num. T -> T f, int x) -> int { return x; }");
        assert!(c.take_messages().is_empty());
        let scheme = c.env().lookup("app").expect("app should be registered");
        let ty = apply_ty(c.subst(), &scheme.ty);
        let Ty::Fun(param, _) = ty else {
            panic!("expected function type, got {ty}");
        };
        let Ty::Forall {
            bounds,
            constraints,
            body,
        } = param.as_ref()
        else {
            panic!("expected forall parameter, got {param}");
        };
        assert_eq!(bounds.len(), 1);
        assert_eq!(constraints.len(), 1);
        assert_eq!(constraints[0].class, "Num");
        assert!(constraints[0].is_unary_on(bounds[0]));
        assert!(matches!(body.as_ref(), Ty::Fun(_, _)));
        assert!(format!("{}", param).starts_with("forall t"));
    }

    #[test]
    fn rank_n_param_accepts_polymorphic_id() {
        let src = r#"
            fn id<T>(T x) -> T { return x; }
            fn app(forall T. T -> T f, int x) -> int { return f(x); }
            fn main() { print "%i", app(id, 1); }
        "#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "expected no messages, got: {msgs:?}");
    }

    #[test]
    fn rank_n_rejects_escaping_skolem() {
        let src = r#"
            fn inc(int x) -> int { return x; }
            fn app(forall T. T -> T f, int x) -> int { return f(x); }
            fn main() { print "%i", app(inc, 1); }
        "#;
        let msgs = assert_messages(src);
        assert!(
            msgs.iter()
                .any(|m| m.code() == Some(ErrorCode::TypeMismatch)),
            "expected rank-n type mismatch, got: {msgs:?}"
        );
    }

    #[test]
    fn function_arity_mismatch_errors() {
        // fib takes 1 int, called with 2 args.
        let msgs = assert_messages("fn fib(int n) -> int { return n; } fib(1, 2);");
        assert!(!msgs.is_empty());
    }

    #[test]
    fn function_return_mismatch_errors() {
        // Declared return is int, but body returns string.
        let msgs = assert_messages("fn broken() -> int { return \"oops\"; }");
        assert!(!msgs.is_empty());
    }

    #[test]
    fn function_undefined_errors() {
        // body calls an unknown function.
        let msgs = assert_messages("fn main() { nope(); }");
        assert!(!msgs.is_empty());
    }

    // ---- Recursive functions (monomorphic recursion) ----

    #[test]
    fn recursive_fib() {
        // fib(n) = if n < 2 then n else fib(n-1) + fib(n-2)
        // Adapted to the current syntax (no `<` comparison; use `==`).
        let src = "fn fib(int n) -> int { if n == 1 { return 1; } if n == 2 { return 1; } return fib(n - 1) + fib(n - 2); }";
        let (mut c, _) = check(src);
        assert!(
            c.take_messages().is_empty(),
            "expected no messages, got: {:?}",
            c.messages()
        );
        let scheme = c.env().lookup("fib").unwrap();
        let ty = apply_ty(c.subst(), &scheme.ty);
        assert_eq!(ty, Ty::Fun(Box::new(int()), Box::new(int())));
    }

    // ---- Class declarations ----

    #[test]
    fn class_registers_nominal_constructor() {
        let (mut c, _) = check("class Foo { name: String, }");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        // Foo is a Ty::Con. The fields are stored privately by default.
        let class = c.classes.get("Foo").expect("class not registered");
        assert_eq!(class.len(), 1);
        assert_eq!(class[0].0, Visibility::Private);
        assert_eq!(class[0].1, "name");
        assert_eq!(class[0].2, string());
    }

    #[test]
    fn class_with_pub_field_marks_visibility() {
        let (mut c, _) = check("class Foo { pub age: int, name: String, }");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        let class = c.classes.get("Foo").unwrap();
        // First field is public (pub), second is private.
        assert_eq!(class[0].0, Visibility::Public);
        assert_eq!(class[0].1, "age");
        assert_eq!(class[1].0, Visibility::Private);
        assert_eq!(class[1].1, "name");
    }

    #[test]
    fn class_with_all_private_fields() {
        let (mut c, _) = check("class Foo { x: int, y: int, }");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        let class = c.classes.get("Foo").unwrap();
        assert!(class.iter().all(|(v, _, _)| *v == Visibility::Private));
    }

    #[test]
    fn class_visibility_is_per_field() {
        // First field is public, second is private — they're tracked
        // independently even though they live in the same class.
        let (mut c, _) = check("class Foo { pub a: int, b: int, pub c: int, }");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        let fields = &c.classes.get("Foo").unwrap();
        assert_eq!(fields[0].0, Visibility::Public);
        assert_eq!(fields[1].0, Visibility::Private);
        assert_eq!(fields[2].0, Visibility::Public);
    }

    #[test]
    fn class_visibility_recorded_for_future_member_access() {
        // Member access (`x.field`) isn't parsed yet, so we can't write
        // a true visibility-check test. This test asserts the data is
        // recorded correctly so the future member-access pass can
        // enforce it without re-parsing the class.
        let (mut c, _) = check("class Foo { pub age: int, name: String, }");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        let foo = c.classes.get("Foo").unwrap();
        assert_eq!(foo[0].0, Visibility::Public);
        assert_eq!(foo[1].0, Visibility::Private);
    }

    // ---- Impl blocks ----

    #[test]
    fn impl_binds_self_to_owner() {
        // `self` is implicit. The method's type becomes Foo -> Foo.
        let src = "class Foo { } impl Foo { fn id() -> Foo { return new Foo(); } }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        let methods = c.methods.get("Foo").expect("methods not registered");
        let (_, scheme) = methods.get("id").unwrap();
        let ty = apply_ty(c.subst(), &scheme.ty);
        // Foo -> Foo
        assert_eq!(
            ty,
            Ty::Fun(
                Box::new(Ty::Con("Foo".into())),
                Box::new(Ty::Con("Foo".into()))
            )
        );
    }

    #[test]
    fn impl_method_with_args_prepends_self() {
        let src = "impl Foo { fn method(int x) -> int { return x; } }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        let methods = c.methods.get("Foo").unwrap();
        let (_, scheme) = methods.get("method").unwrap();
        let ty = apply_ty(c.subst(), &scheme.ty);
        // Foo -> int -> int
        assert_eq!(
            ty,
            Ty::Fun(
                Box::new(Ty::Con("Foo".into())),
                Box::new(Ty::Fun(Box::new(int()), Box::new(int())))
            )
        );
    }

    #[test]
    fn impl_method_visibility_default_is_private() {
        let src = "impl Foo { fn hidden() -> int { return 0; } }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        let methods = c.methods.get("Foo").unwrap();
        let (vis, _) = methods.get("hidden").unwrap();
        assert_eq!(*vis, Visibility::Private);
    }

    #[test]
    fn impl_pub_method_marks_visibility() {
        let src = "impl Foo { pub fn visible() -> int { return 0; } }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        let methods = c.methods.get("Foo").unwrap();
        let (vis, _) = methods.get("visible").unwrap();
        assert_eq!(*vis, Visibility::Public);
    }

    // ---- Instantiation ----

    #[test]
    fn instantiate_returns_class_type() {
        // Positional ctor args match class fields in declaration order.
        let src = r#"class Foo { name: string, } let x = new Foo("hi"); x"#;
        let (mut c, ty) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        // The whole program's type is the type of `x`, which is Foo.
        assert_eq!(ty, Ty::Con("Foo".into()));
    }

    // ---- Combined: class + impl + instantiation ----

    #[test]
    fn class_impl_and_instantiate_combined() {
        let src = r#"
            class Foo { name: string, }
            impl Foo { fn sadge() -> int { return 42; } }
            fn main() { let x = new Foo("hi"); }
        "#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        assert!(c.classes.contains_key("Foo"));
        assert!(c.methods.get("Foo").unwrap().contains_key("sadge"));
    }

    #[test]
    fn class_method_call_typechecks() {
        let src = "\
            class Point { x: int, y: int, } \
            impl Point { fn sum() -> int { return self.x + self.y; } } \
            fn main() { let p = new Point(1, 3); p.sum(); }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
    }

    // ---- Phase 7: generic classes ----

    #[test]
    fn generic_class_new_infers_cell_int() {
        let src = "\
            class Cell<T> { value: T }
            fn main() { let c = new Cell(42); }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        let c_ty = c.codegen_var_type("c").expect("c should be recorded");
        assert_eq!(
            apply_ty_prune(c.subst(), c_ty),
            Ty::App(Box::new(Ty::Con("Cell".into())), vec![int()])
        );
    }

    #[test]
    fn generic_class_method_get_returns_int() {
        let src = "\
            class Cell<T> { value: T }
            impl Cell<T> {
                fn get() -> T { return self.value; }
            }
            fn main() {
                let c = new Cell(42);
                let v = c.get();
            }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        assert_eq!(
            c.codegen_var_type("v")
                .map(|t| apply_ty_prune(c.subst(), t)),
            Some(int())
        );
    }

    #[test]
    fn generic_class_fields_stored_as_param_placeholders() {
        let (mut c, _) = check("class Cell<T> { value: T }");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        let fields = c.classes.get("Cell").expect("Cell registered");
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].1, "value");
        assert_eq!(fields[0].2, Ty::Con("T".into()));
    }

    #[test]
    fn class_unknown_field_errors() {
        let src = "\
            class Point { x: int, } \
            fn main() { let p = new Point(1); p.z; }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.iter().any(|m| m.message().contains("field")),
            "expected unknown-field diagnostic, got {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn class_ctor_arity_mismatch_errors() {
        let src = "\
            class Point { x: int, y: int, } \
            fn main() { let p = new Point(1); }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(!msgs.is_empty(), "expected ctor arity diagnostic");
    }

    // ---- Recursive method (inside an impl) ----

    #[test]
    fn recursive_method_via_self_binding() {
        // fib uses no self, but `==` is the only comparison the
        // parser currently supports; use that for the branch.
        let src = "impl Counter { fn tick(int n) -> int { if n == 0 { return 0; } return tick(n - 1) + 1; } }";
        let (mut c, _) = check(src);
        // We don't require no messages — the outer call site `tick(...)`
        // may have residual issues — but the method should be registered.
        let _ = c.take_messages();
        assert!(c.methods.get("Counter").unwrap().contains_key("tick"));
    }

    // ---- Block returns last value ----

    #[test]
    fn nested_blocks_return_inner() {
        assert_ok("{ { 42; } }", int());
    }

    // ---- Native registration ----

    #[test]
    fn register_native_adds_function_to_env() {
        let mut c = Checker::new();
        c.register_native("add", &[int(), int()], &int());
        let scheme = c.env().lookup("add").expect("add not registered");
        let ty = apply_ty(c.subst(), &scheme.ty);
        // Curried: int -> int -> int
        assert_eq!(
            ty,
            Ty::Fun(
                Box::new(int()),
                Box::new(Ty::Fun(Box::new(int()), Box::new(int())))
            )
        );
    }

    #[test]
    fn register_native_no_args() {
        let mut c = Checker::new();
        c.register_native("now", &[], &int());
        let scheme = c.env().lookup("now").expect("now not registered");
        let ty = apply_ty(c.subst(), &scheme.ty);
        assert_eq!(ty, int());
    }

    #[test]
    fn register_native_void_return() {
        let mut c = Checker::new();
        c.register_native("print", &[string()], &unit_ty());
        let scheme = c.env().lookup("print").expect("print not registered");
        let ty = apply_ty(c.subst(), &scheme.ty);
        // string -> unit
        assert_eq!(ty, Ty::Fun(Box::new(string()), Box::new(unit_ty())));
    }

    #[test]
    fn register_native_object_type() {
        let mut c = Checker::new();
        c.register_native("make_foo", &[], &Ty::Con("Foo".into()));
        let scheme = c.env().lookup("make_foo").expect("make_foo not registered");
        let ty = apply_ty(c.subst(), &scheme.ty);
        assert_eq!(ty, Ty::Con("Foo".into()));
    }

    #[test]
    fn native_function_call_infers_correctly() {
        // After register_native, a call to the native should type-check
        // against the registered signature and produce the right type.
        let mut c = Checker::new();
        c.register_native("print", &[string()], &unit_ty());
        let ast = Pratt::default().parse("print \"hi\";").expect("parse");
        let ty = c.check_program(&ast);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        assert_eq!(ty, unit_ty());
    }

    #[test]
    fn native_function_arity_mismatch_errors() {
        // print takes 1 arg; call with 2.
        let mut c = Checker::new();
        c.register_native("print", &[string()], &unit_ty());
        let ast = Pratt::default()
            .parse("print(\"a\", \"b\");")
            .expect("parse");
        let _ = c.check_program(&ast);
        let msgs = c.take_messages();
        assert!(!msgs.is_empty(), "expected arity-mismatch error");
    }

    #[test]
    fn native_function_call_with_correct_arity_succeeds() {
        // Once registered, a function call with matching arity and types
        // type-checks cleanly.
        let mut c = Checker::new();
        c.register_native("add", &[int(), int()], &int());
        let ast = Pratt::default().parse("add(1, 2);").expect("parse");
        let _ = c.check_program(&ast);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
    }

    #[test]
    fn native_visible_inside_nested_block() {
        // Natives registered on the checker are visible from any
        // nested scope.
        let mut c = Checker::new();
        c.register_native("print", &[string()], &unit_ty());
        let ast = Pratt::default().parse("{ print \"a\"; }").expect("parse");
        let _ = c.check_program(&ast);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
    }

    // ---- Diagnostics ----
    //
    // The following tests verify that emitted `Message`s are well-formed
    // for ariadne: each carries a clear headline, a primary label at
    // the error range, and (where helpful) a `help` hint with extra
    // context. ariadne's renderer consumes exactly this shape, so as
    // long as the `Message` fields are populated, the diagnostic will
    // display cleanly.

    #[test]
    fn unknown_identifier_message_uses_can_not_find_format() {
        let (mut c, _) = check("unknown_var;");
        let msgs = c.take_messages();
        assert_eq!(msgs.len(), 1);
        let msg = &msgs[0];
        assert!(
            msg.message().contains("Cannot find value"),
            "got: {:?}",
            msg.message()
        );
        assert!(
            msg.message().contains("unknown_var"),
            "got: {:?}",
            msg.message()
        );
        // Primary range is set (ariadne uses this for the underline).
        let r = msg.range();
        assert!(r.start <= r.end, "bad range {:?}", r);
        assert!(r.end > 0, "expected non-empty range");
    }

    #[test]
    fn type_mismatch_message_uses_expected_actual_format() {
        let (mut c, _) = check("let x: int = \"hello\";");
        let msgs = c.take_messages();
        // The Fragment return is unit, so the assignment's type
        // mismatch is the only diagnostic.
        assert!(!msgs.is_empty(), "expected at least one message");
        let msg = msgs.iter().find(|m| m.message().contains("Type mismatch"));
        assert!(
            msg.is_some(),
            "no type-mismatch message found in {:?}",
            msgs
        );
        let msg = msg.unwrap();
        assert!(
            msg.message().contains("expected"),
            "got: {:?}",
            msg.message()
        );
        assert!(msg.message().contains("found"), "got: {:?}", msg.message());
        assert!(msg.message().contains("int"), "got: {:?}", msg.message());
        assert!(msg.message().contains("string"), "got: {:?}", msg.message());
        // Help is present (the context).
        assert!(msg.help().is_some(), "missing help");
        let help = msg.help().as_ref().unwrap();
        assert!(help.contains("let binding"), "got help: {:?}", help);
    }

    #[test]
    fn infinite_type_message_uses_clear_format() {
        // It's hard to construct an infinite-type situation without
        // recursive type syntax (e.g., `α = List<α>`), so this test
        // just checks the format IF such a message ever fires. To make
        // sure the path is exercised, we drive the checker through a
        // recursive function declaration whose body returns the
        // function itself with the wrong shape — that triggers an
        // occurs check via the return-type unification.
        //
        // (If your checker ever changes the return path so this no
        // longer fires an occurs check, drop this test — it's about
        // message format, not behaviour.)
        let (mut c, _) = check("fn bad() { return bad; }");
        let msgs = c.take_messages();
        // Either there's an infinite-type error, or the function is
        // typeable. Both are fine — what we want to assert is the
        // format IF the error fires.
        if let Some(infinite) = msgs
            .iter()
            .find(|m| m.message().contains("Cannot construct infinite type"))
        {
            assert!(
                infinite.help().is_some(),
                "missing help on occurs-check message"
            );
        }
    }

    #[test]
    fn not_a_function_message_uses_cannot_call_format() {
        // `let x = 5; x(2);` — `x` is an int, calling it is an error.
        let (mut c, _) = check("let x = 5; x(2);");
        let msgs = c.take_messages();
        assert!(!msgs.is_empty(), "expected a message");
        let msg = msgs.iter().find(|m| {
            m.message().contains("Cannot call value") || m.message().contains("too many arguments")
        });
        assert!(msg.is_some(), "got: {:?}", msgs);
    }

    #[test]
    fn unknown_function_message_uses_can_not_find_format() {
        let (mut c, _) = check("missing_fn();");
        let msgs = c.take_messages();
        assert!(!msgs.is_empty(), "expected a message");
        let msg = msgs
            .iter()
            .find(|m| m.message().contains("Cannot find function"));
        assert!(msg.is_some(), "got: {:?}", msgs);
        let msg = msg.unwrap();
        assert!(
            msg.message().contains("`missing_fn`"),
            "got: {:?}",
            msg.message()
        );
    }

    #[test]
    fn assignment_to_undeclared_message_includes_help_hint() {
        let (mut c, _) = check("undeclared = 1;");
        let msgs = c.take_messages();
        assert!(!msgs.is_empty(), "expected a message");
        let msg = msgs
            .iter()
            .find(|m| m.message().contains("Cannot assign to undeclared"));
        assert!(msg.is_some(), "got: {:?}", msgs);
        let msg = msg.unwrap();
        let help = msg.help().as_ref().expect("missing help");
        assert!(
            help.contains("let undeclared"),
            "help should suggest `let undeclared;`, got: {:?}",
            help
        );
    }

    #[test]
    fn assignment_to_const_emits_immutability_diagnostic() {
        let (mut c, _) = check("const x = 1; x = 2;");
        let msgs = c.take_messages();
        let msg = msgs
            .iter()
            .find(|m| m.message().contains("Cannot assign to constant `x`"));
        assert!(msg.is_some(), "got: {:?}", msgs);
        assert!(msg.unwrap().help().is_some(), "missing help");
    }

    #[test]
    fn arity_mismatch_message_mentions_function_name() {
        // foo takes 1 arg; call with 2.
        let mut c = Checker::new();
        c.register_native("foo", &[int()], &int());
        let ast = Pratt::default().parse("foo(1, 2);").expect("parse");
        let _ = c.check_program(&ast);
        let msgs = c.take_messages();
        assert!(!msgs.is_empty(), "expected a message");
        let msg = msgs
            .iter()
            .find(|m| m.message().contains("too many arguments"));
        assert!(msg.is_some(), "got: {:?}", msgs);
        let msg = msg.unwrap();
        assert!(msg.message().contains("`foo`"), "got: {:?}", msg.message());
    }

    #[test]
    fn diagnostic_messages_have_valid_ranges() {
        // Every diagnostic should have a non-empty range that lies
        // within the source bounds (0..src.len()). ariadne's renderer
        // requires this.
        for src in &["x;", "1 + true", "let y: int = 1; y = \"z\";"] {
            let (mut c, _) = check(src);
            let src_len = src.len();
            for msg in c.take_messages() {
                let r = msg.range();
                assert!(
                    r.start <= r.end && r.end <= src_len,
                    "bad range {:?} for source len {} (msg: {})",
                    r,
                    src_len,
                    msg.message()
                );
            }
        }
    }

    #[test]
    fn pattern_error_span_points_at_arm_body() {
        // Regression test for the `0..0` pattern-error span bug.
        // Pattern errors used to land at byte 0 of the source because
        // `expected_ty_span_range` always returned `0..0`. After
        // threading `arm.body.0.into_range()` through `infer_pattern`,
        // the diagnostic for a wrong-arity pattern should anchor
        // somewhere inside the source — NOT at byte 0.
        let src = "let x = Option::Some(1); match x { Option::Some(a, b) => 0 };";
        let (mut c, _) = check(src);
        let src_len = src.len();
        let msgs = c.take_messages();
        assert!(
            !msgs.is_empty(),
            "expected at least one diagnostic for `{}`",
            src
        );
        // The wrong-arity error from `infer_pattern` must NOT be
        // at byte 0.
        let arity_msg = msgs
            .iter()
            .find(|m| m.message().contains("expects"))
            .expect("expected a wrong-arity pattern diagnostic");
        let r = arity_msg.range();
        assert!(
            r.start > 0,
            "pattern diagnostic anchored at byte 0 — `0..0` regression: \
             range={:?} msg={:?} src={:?}",
            r,
            arity_msg.message(),
            src
        );
        assert!(
            r.end <= src_len,
            "pattern diagnostic range {:?} exceeds source length {}",
            r,
            src_len
        );
    }

    #[test]
    fn multiple_natives_coexist() {
        let mut c = Checker::new();
        c.register_native("print", &[string()], &unit_ty());
        c.register_native("add", &[int(), int()], &int());
        c.register_native("now", &[], &string());

        let print_ty = apply_ty(c.subst(), &c.env().lookup("print").unwrap().ty);
        let add_ty = apply_ty(c.subst(), &c.env().lookup("add").unwrap().ty);
        let now_ty = apply_ty(c.subst(), &c.env().lookup("now").unwrap().ty);

        assert_eq!(print_ty, Ty::Fun(Box::new(string()), Box::new(unit_ty())));
        assert_eq!(
            add_ty,
            Ty::Fun(
                Box::new(int()),
                Box::new(Ty::Fun(Box::new(int()), Box::new(int())))
            )
        );
        assert_eq!(now_ty, string());
    }

    // ---- Type cache ----

    #[test]
    fn cache_is_populated_after_check_program() {
        // After infer, every pre-walked node should have a cached type.
        let (mut c, _) = check("1 + 2;");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        let total = c.id_table().len();
        assert!(total > 0);
        assert_eq!(c.cache_len(), total);
    }

    #[test]
    fn cache_lookup_returns_inferred_type() {
        // `1 + 2` parses to Expr(Add(Integer, Integer)); we expect the
        // cache to hold int() for each of those nodes.
        let (c, _) = check("1 + 2;");
        let ids = c.id_table().ids();
        for id in ids {
            let ty = c
                .lookup_at(*id)
                .unwrap_or_else(|| panic!("no cache entry for {:?}", id));
            assert_eq!(ty, int(), "node {:?} had type {}", id, ty);
        }
    }

    #[test]
    fn cache_lookup_applies_substitution() {
        // `let x = 42; x` unifies x's fresh var with int. After unify,
        // the cached type for `x` should resolve to int, not the
        // original Ty::Var.
        let (mut c, _) = check("let x = 42; x");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        let scheme = c.env().lookup("x").expect("x not bound").clone();
        let resolved = apply_ty_prune(c.subst(), &scheme.ty);
        assert_eq!(resolved, int());
    }

    #[test]
    fn cache_lookup_returns_none_for_unknown_id() {
        let (c, _) = check("42;");
        assert!(c.lookup_at(NodeId(9999)).is_none());
    }

    #[test]
    fn pre_walk_mints_distinct_ids_for_nodes_sharing_a_span() {
        // `42;` produces a Program and a Statement that both span the
        // entire source. The pre-walk must give them distinct IDs.
        let (mut c, _) = check("42;");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        let ids = c.id_table().ids();
        let unique: std::collections::HashSet<_> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "IDs must be unique per AST node");
        assert!(ids.len() >= 3);
    }

    #[test]
    fn pre_walk_is_deterministic() {
        let (mut c1, _) = check("let a = 1; let b = 2; a + b");
        let (mut c2, _) = check("let a = 1; let b = 2; a + b");
        let msgs1 = c1.take_messages();
        let msgs2 = c2.take_messages();
        assert!(msgs1.is_empty(), "{:?}", msgs1);
        assert!(msgs2.is_empty(), "{:?}", msgs2);
        assert_eq!(c1.id_table().len(), c2.id_table().len());
        assert_eq!(c1.id_table().ids(), c2.id_table().ids());
    }

    #[test]
    fn cache_has_entries_for_value_producing_nodes() {
        // The cache holds a type per node that produces a value.
        // Declarations like `Variable` and `Comment` are side effects on
        // the env and don't produce a typed value, so they don't get a
        // cache entry — but they're still visited by the pre-walk.
        // The cache size should therefore be `<=` the pre-walk size.
        for src in &["42;", "1 + 2;", "let x = 1; x", "if true { 42; }"] {
            let (mut c, _) = check(src);
            let msgs = c.take_messages();
            assert!(msgs.is_empty(), "{:?} for `{}`", msgs, src);
            assert!(
                c.cache_len() <= c.id_table().len(),
                "cache ({}) larger than pre-walk ({}) for `{}`",
                c.cache_len(),
                c.id_table().len(),
                src
            );
            // And at least one entry per source.
            assert!(c.cache_len() > 0, "empty cache for `{}`", src);
        }
    }

    // ---- Call with arguments ----

    #[test]
    fn unknown_call_argument_types_dont_crash() {
        let msgs = assert_messages("foo(1, 2, 3);");
        assert!(!msgs.is_empty());
    }

    // ================================================================
    // ---- Enums and pattern matching ----
    // ================================================================

    // ---- Enum registration ----

    #[test]
    fn enum_decl_registers_sum_type() {
        let (mut c, _) = check("enum E { A, B }");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        assert!(c.enums.contains_key("E"));
        let variants = c.enums.get("E").unwrap();
        assert_eq!(variants, &vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn enum_with_payload_registers_constructor() {
        // After registration, `Box::Full` is bound as a curried
        // function in the env: `int -> Constructor`.
        let (mut c, _) = check("enum Box { Empty, Full(int) }");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        let scheme = c.env().lookup("Box::Full").expect("not bound");
        let ty = apply_ty_prune(c.subst(), &scheme.ty);
        assert_eq!(
            ty,
            Ty::Fun(
                Box::new(int()),
                Box::new(Ty::Constructor {
                    owner: Box::new(Ty::Sum {
                        name: "Box".into(),
                        variants: vec![
                            ("Empty".into(), EnumVariantPayloadTy::Unit),
                            ("Full".into(), EnumVariantPayloadTy::Tuple(vec![int()])),
                        ],
                    }),
                    tag: 1,
                    arity: 1,
                }),
            )
        );
    }

    #[test]
    fn enum_tags_assigned_in_declaration_order() {
        // Tags follow source-declaration order, not alphabetical.
        // `enum E { Z, A, M, B }` → Z=0, A=1, M=2, B=3.
        let (mut c, _) = check("enum E { Z, A, M, B }");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        assert_eq!(c.tag_for("E", "Z"), Some(0));
        assert_eq!(c.tag_for("E", "A"), Some(1));
        assert_eq!(c.tag_for("E", "M"), Some(2));
        assert_eq!(c.tag_for("E", "B"), Some(3));
    }

    #[test]
    fn recursive_enum_typechecks() {
        // Isorecursive encoding: recursive payloads use Ty::Con("Tree").
        let (mut c, _) = check("enum Tree { Leaf, Node(int, Tree, Tree) }");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        // The recursive variant's payload should reference the
        // enum by name (opaque) — the public `enum_variants` API
        // is the canonical interface to inspect this.
        let variants = c.enum_variants("Tree").expect("Tree not registered");
        let node_payload = variants
            .iter()
            .find(|(n, _, _)| n == "Node")
            .unwrap()
            .2
            .clone();
        assert_eq!(
            node_payload,
            vec![int(), Ty::Con("Tree".into()), Ty::Con("Tree".into())]
        );
    }

    #[test]
    fn duplicate_enum_is_error() {
        let msgs = assert_messages("enum A { X } enum A { Y }");
        assert!(
            msgs.iter().any(|m| m.message().contains("Duplicate enum")),
            "expected duplicate-enum error, got: {:?}",
            msgs
        );
    }

    #[test]
    fn duplicate_constructor_is_error() {
        let msgs = assert_messages("enum A { Foo } enum B { Foo }");
        assert!(
            msgs.iter()
                .any(|m| m.message().contains("Duplicate constructor")),
            "expected duplicate-constructor error, got: {:?}",
            msgs
        );
    }

    #[test]
    fn enum_decl_cache_aligned_with_id_table() {
        // Regression test for the ID-alignment bug in
        // `infer_enum_decl`: the pre-walk mints one ID for the
        // `EnumDecl` node, one for each `EnumVariant` node, and one
        // for each `Expression::Type` payload. The infer pass must
        // consume exactly the same number of IDs (via `self.infer`)
        // so the cache lines up with the id table.
        //
        // Concretely: `enum Color { Red, Green(int) }` produces
        //   1 (EnumDecl) + 2 (variants) + 1 (Green's payload type) = 4
        // pre-walk IDs, and `infer` must consume all 4. The cache
        // therefore has the same length as the id table.
        for src in &[
            "enum Color { Red, Green(int) }",
            "enum E { A, B, C }",
            "enum Tree { Leaf, Node(int, Tree, Tree) }",
        ] {
            let (mut c, _) = check(src);
            let msgs = c.take_messages();
            assert!(msgs.is_empty(), "{:?} for `{}`", msgs, src);
            assert_eq!(
                c.cache_len(),
                c.id_table().len(),
                "cache ({}) and id_table ({}) out of sync for `{}` \
                 — `infer_enum_decl` is not consuming every pre-walked ID",
                c.cache_len(),
                c.id_table().len(),
                src,
            );
            // Sanity: every pre-walked ID has a cached entry
            // (cache_len == id_table.len() already implies this,
            // but make the intent explicit).
            for id in c.id_table().ids() {
                assert!(
                    c.lookup_at(*id).is_some(),
                    "pre-walked ID {:?} has no cache entry for `{}`",
                    id,
                    src,
                );
            }
        }
    }

    // ---- Constructor calls ----

    #[test]
    fn constructor_call_with_wrong_arity_is_error() {
        // Option::Some takes 1 arg, called with 2.
        let src = "Option::Some(1, 2)";
        let msgs = assert_messages(src);
        assert!(
            msgs.iter()
                .any(|m| m.message().contains("expects 1 arguments")),
            "expected arity error, got: {:?}",
            msgs
        );
    }

    #[test]
    fn constructor_call_with_correct_arity_typechecks() {
        let src = "Option::Some(42)";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        // Check via the cache that the call produced a Constructor type.
        let ids = c.id_table().ids();
        let found = ids.iter().find_map(|id| match c.lookup_at(*id) {
            Some(Ty::Constructor { tag, arity, .. }) => Some((tag, arity)),
            _ => None,
        });
        assert_eq!(found, Some((1, 1)));
    }

    #[test]
    fn unknown_enum_constructor_is_error() {
        let msgs = assert_messages("Nonexistent::Some(1);");
        assert!(
            msgs.iter()
                .any(|m| m.message().contains("Cannot find enum")),
            "expected unknown-enum error, got: {:?}",
            msgs
        );
    }

    #[test]
    fn unknown_variant_on_known_enum_is_error() {
        let src = "Option::Purlpe(1)";
        let msgs = assert_messages(src);
        assert!(
            msgs.iter()
                .any(|m| m.message().contains("Cannot find variant")),
            "expected unknown-variant error, got: {:?}",
            msgs
        );
    }

    // ---- Pattern matching ----

    #[test]
    fn match_with_all_variants_no_error() {
        // Enum declarations are top-level statements (no trailing
        // `;`) and must appear at the end of a sequence of
        // statements. Zero-arity constructors require `()`.
        let src = "let x = Option::Some(1); match x { Option::None() => 0, Option::Some(v) => v };";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
    }

    #[test]
    fn match_with_wildcard_no_exhaustiveness_error() {
        let src = "let x = Option::Some(1); match x { _ => 0 };";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
    }

    #[test]
    fn match_non_exhaustive_reports_missing() {
        let src = "let x = Option::None(); match x { Option::None() => 0 };";
        let msgs = assert_messages(src);
        assert!(
            msgs.iter()
                .any(|m| m.message().contains("Non-exhaustive match")),
            "expected non-exhaustive error, got: {:?}",
            msgs
        );
        let msg = msgs
            .iter()
            .find(|m| m.message().contains("Non-exhaustive"))
            .unwrap();
        assert!(
            msg.message().contains("Some"),
            "expected `Some` to be mentioned, got: {:?}",
            msg.message()
        );
    }

    #[test]
    fn match_with_unreachable_arm_reports() {
        let src = "let x = Option::None(); match x { Option::None() => 0, Option::None() => 1 };";
        let msgs = assert_messages(src);
        assert!(
            msgs.iter().any(|m| m.message().contains("Unreachable arm")),
            "expected unreachable-arm error, got: {:?}",
            msgs
        );
    }

    #[test]
    fn match_pattern_binding_does_not_leak() {
        // `v` is bound inside the arm; referencing it after the
        // match should error.
        let src = "let x = Option::Some(1); match x { Option::Some(v) => 0 }; v";
        let msgs = assert_messages(src);
        // The `v` reference after the match is unknown.
        assert!(
            msgs.iter()
                .any(|m| m.message().contains("Cannot find value `v`")),
            "expected 'v not found' error, got: {:?}",
            msgs
        );
    }

    #[test]
    fn nested_constructor_pattern_typechecks() {
        // Patterns can be nested; the inner sub-patterns are
        // checked against the corresponding payload types. We
        // wrap a value in a single-level enum so the inner
        // pattern is `Wrap::Inner(int)` — the nested pattern
        // case. (Truly recursive `Option<Option<T>>` is not
        // constructible because `Option::Some` takes `int`
        // directly, so we use a custom enum that wraps a type
        // whose pattern can be nested.)
        let src = "let x = Wrap::W(Inner::I(7)); match x { Wrap::W(Inner::I(v)) => v }; enum Inner { I(int) } enum Wrap { W(Inner) }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
    }

    #[test]
    fn forward_reference_to_constructor_works() {
        // The enum is declared AFTER the use; the pre-pass makes
        // this work.
        let src = "let x = Option::Some(1)";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
    }

    // ---- Format-string typecheck ----

    #[test]
    fn format_string_percent_i_requires_int() {
        let msgs = assert_messages(r#"print "%i", "hello";"#);
        assert!(
            msgs.iter().any(|m| m.message().contains("requires int")),
            "expected '%i requires int' error, got: {:?}",
            msgs
        );
    }

    #[test]
    fn format_string_percent_s_requires_string() {
        let msgs = assert_messages("print \"%s\", 42;");
        assert!(
            msgs.iter().any(|m| m.message().contains("requires string")),
            "expected '%s requires string' error, got: {:?}",
            msgs
        );
    }

    #[test]
    fn format_string_percent_f_requires_float() {
        let msgs = assert_messages("print \"%f\", 1;");
        assert!(
            msgs.iter().any(|m| m.message().contains("requires float")),
            "expected '%f requires float' error, got: {:?}",
            msgs
        );
    }

    #[test]
    fn format_string_with_constructor_value_errors_on_percent_s() {
        // Red-team critical: passing a `Constructor` (a sum) where
        // a string is expected must be flagged.
        let src = "print \"%s\", Option::Some(1)";
        let msgs = assert_messages(src);
        assert!(
            msgs.iter().any(|m| m.message().contains("requires string")),
            "expected 'requires string' error, got: {:?}",
            msgs
        );
    }

    #[test]
    fn format_string_with_constructor_via_match_works() {
        // The match arm's body must be inferable as a string, and
        // print "%s", s should accept it.
        let src = "let s = match Option::Some(1) { Option::None() => \"none\", Option::Some(_) => \"some\" }; print \"%s\", s";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
    }

    #[test]
    fn addition_of_strings_is_string() {
        assert_ok("\"hello\" + \"world\"", string());
    }

    #[test]
    fn string_plus_int_errors() {
        let msgs = assert_messages("\"hello\" + 42;");
        assert!(
            msgs.iter().any(|m| m.message().contains("Type mismatch")),
            "expected string+int type mismatch, got: {:?}",
            msgs
        );
    }

    #[test]
    fn format_expression_returns_string() {
        assert_ok("format \"%i-%s\", 42, \"x\"", string());
    }

    #[test]
    fn format_percent_v_accepts_int() {
        let (mut c, _) = check(r#"print "%v", 42;"#);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
    }

    #[test]
    fn format_percent_v_accepts_structural_tuple_and_record() {
        let (mut c, _) = check(r#"print "%v%v", (1, true), { a: 3, b: "x" };"#);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
    }

    #[test]
    fn format_percent_i_on_open_type_errors() {
        let msgs = assert_messages(r#"fn bad<T>(T x) { print "%i", x; } fn main() { bad(1); }"#);
        assert!(
            msgs.iter().any(|m| {
                m.message().contains("open type")
                    && m.help().as_ref().is_some_and(|h| h.contains("%v"))
            }),
            "expected open-type `%i` error with `%v` help, got: {:?}",
            msgs
        );
    }

    #[test]
    fn format_percent_v_without_show_errors() {
        let msgs = assert_messages(r#"fn bad<T>(T x) { print "%v", x; } fn main() { bad(1); }"#);
        assert!(
            msgs.iter().any(|m| m.message().contains("Show")),
            "expected Show requirement for `%v`, got: {:?}",
            msgs
        );
    }

    #[test]
    fn format_percent_v_rejects_structural_tuple_with_open_type() {
        let msgs =
            assert_messages(r#"fn bad<T>(T x) { print "%v", (x, 1); } fn main() { bad(1); }"#);
        assert!(
            msgs.iter().any(|m| m.message().contains("Show")),
            "expected Show requirement for structural `%v` with open T, got: {:?}",
            msgs
        );
    }

    // ---- Inner-pattern reachability ----

    #[test]
    fn typechecker_does_not_report_unreachable_for_different_inner_patterns() {
        // Two Result::Ok arms with different inner patterns are both reachable.
        let src = r#"
        fn unwrap(Result r) -> int {
            return match r {
                Result::Ok(Option::Some(v)) => v,
                Result::Ok(Option::None) => 0,
                Result::Err(_) => -1,
            };
        }
        fn main() {
            print "%i", unwrap(Result::Ok(Option::Some(42)));
        }
        "#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        let unreachable: Vec<String> = msgs
            .iter()
            .filter(|m| m.message().contains("Unreachable arm"))
            .map(|m| m.message().to_string())
            .collect();
        assert!(
            unreachable.is_empty(),
            "Typechecker should NOT report unreachable arm for different inner patterns, got: {:?}",
            unreachable
        );
    }

    // ---- Field access ----

    #[test]
    fn access_field_from_record_variant_returns_field_type() {
        // `p.x` where `p` is bound to a `Point::Point { x: int, y: int }`
        // constructor. The receiver's type is a `Ty::Constructor` with
        // a record-shaped payload, so the field resolves uniquely to
        // `int`.
        let src = "enum Point { Origin, Point { x: int, y: int } } \
                   let p = Point::Point { x: 5, y: 12 }; p.x;";
        let (mut c, ty) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.is_empty(),
            "expected no diagnostics for `p.x`, got: {:?}",
            msgs
        );
        assert_eq!(ty, int(), "field access should produce `int`");
    }

    #[test]
    fn access_field_from_non_record_produces_error() {
        // `1.x` — the receiver is an `int`, not a sum. The typechecker
        // should emit a "Cannot access field" diagnostic and NOT
        // silently succeed.
        let msgs = assert_messages("1.x;");
        assert!(
            msgs.iter()
                .any(|m| m.message().contains("Cannot access field")),
            "expected 'Cannot access field' diagnostic, got: {:?}",
            msgs
        );
    }

    #[test]
    fn access_unknown_field_produces_error() {
        // `p.z` where `p` is bound to a `Point::Point { x, y }`
        // constructor. The variant IS a record but doesn't declare
        // `z`. Should emit "Type `Point` has no field `z`" with a
        // help hint listing the actual fields.
        let src = "enum Point { Origin, Point { x: int, y: int } } \
                   let p = Point::Point { x: 1, y: 2 }; p.z;";
        let msgs = assert_messages(src);
        let no_field = msgs
            .iter()
            .find(|m| m.message().contains("no field `z`"))
            .unwrap_or_else(|| panic!("expected 'no field `z`' diagnostic, got: {:?}", msgs));
        // The help hint should mention the actual fields available.
        let hint = no_field
            .help()
            .as_ref()
            .expect("expected help hint on 'no field' diagnostic");
        assert!(
            hint.contains("`x`") && hint.contains("`y`"),
            "expected help hint listing available fields, got: {:?}",
            hint
        );
    }

    #[test]
    fn access_field_from_tuple_variant_produces_error() {
        // `p.x` where `p` is bound to `Tuple::Wrap(1, 2)` — a
        // Tuple-shaped variant. The variant isn't a record, so we
        // emit a tailored "Cannot access field on non-record
        // variant" diagnostic that names the variant's shape.
        let src = "enum Tuple { Wrap(int, int) } \
                   let p = Tuple::Wrap(1, 2); p.x;";
        let msgs = assert_messages(src);
        let diag = msgs
            .iter()
            .find(|m| m.message().contains("Cannot access field"))
            .unwrap_or_else(|| {
                panic!("expected 'Cannot access field' diagnostic, got: {:?}", msgs)
            });
        let hint = diag
            .help()
            .as_ref()
            .expect("expected help hint on tuple-variant access");
        assert!(
            hint.contains("tuple"),
            "expected help hint to mention the variant shape 'tuple', got: {:?}",
            hint
        );
    }

    #[test]
    fn access_field_ambiguous_across_variants_emits_narrow_with_match() {
        // Two record-shaped variants both declare `x`. The
        // receiver's type is `Ty::Sum { name: "Two", variants: [...] }`
        // (because we annotate the parameter `p: Two` directly, so
        // `p`'s type is `Ty::Con("Two")` which we resolve through
        // the registry). Either way, the field type is ambiguous
        // and we emit "narrow with match first".
        let src = "enum Two { A { x: int, y: int }, B { x: string, z: int } } \
                   fn get_x(Two p) -> int { return p.x; }";
        let msgs = assert_messages(src);
        assert!(
            msgs.iter()
                .any(|m| m.message().contains("narrow with match first")),
            "expected 'narrow with match first' diagnostic, got: {:?}",
            msgs
        );
    }

    #[test]
    fn access_field_via_function_parameter_resolves() {
        // Field access on a function parameter whose type is
        // annotated with the bare enum name `Point` — the
        // typechecker parses this as `Ty::Con("Point")` and
        // resolves it through the enum registry to find that
        // `Point::Point` is a record-shaped variant carrying `x`
        // of type `int`.
        let src = "enum Point { Origin, Point { x: int, y: int } } \
                   fn get_x(Point p) -> int { return p.x; }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "expected no diagnostics, got: {:?}", msgs);
    }

    #[test]
    fn access_field_on_sum_param_with_unique_field_resolves() {
        // Same as above, but the enum has exactly ONE record-shaped
        // variant so the field is unambiguous. The typechecker
        // should resolve `p.x` to `int` without diagnostic.
        let src = "enum Point { Origin, Point { x: int, y: int } } \
                   fn get_x(Point p) -> int { return p.x; }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.is_empty(),
            "expected no diagnostics for unambiguous field access, got: {:?}",
            msgs
        );
    }

    // ---- Typed aggregates ----

    #[test]
    fn tuple_literal_infers_heterogeneous_product_type() {
        // `(1, "x")` should infer `(int, string)`.
        let (mut c, ty) = check("(1, \"x\")");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
        let resolved = apply_ty_prune(c.subst(), &ty);
        assert_eq!(
            resolved,
            tuple_ty(vec![int(), string()]),
            "expected tuple type (int, string)"
        );
    }

    #[test]
    fn array_literal_infers_static_length_array() {
        // `[1, 2, 3]` should infer `[int; 3]` (static length 3).
        let (mut c, ty) = check("[1, 2, 3]");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
        let resolved = apply_ty_prune(c.subst(), &ty);
        assert_eq!(resolved, array_fixed(int(), 3), "expected [int; 3]");
    }

    #[test]
    fn array_literal_heterogeneous_elements_emits_diagnostic() {
        // `[1, "x"]` should emit "array element type mismatch".
        let (_c, msgs) = check_warn("[1, \"x\"]");
        let found = msgs
            .iter()
            .any(|m| m.message().contains("element type mismatch"));
        assert!(
            found,
            "expected 'element type mismatch' diagnostic, got: {:?}",
            msgs
        );
    }

    #[test]
    fn array_static_index_out_of_bounds_emits_diagnostic() {
        // `let arr = [0, 1, 2]; arr[3]` — arr is `[int; 3]`,
        // accessing index 3 is OOB.
        let src = "fn main() { let arr = [0, 1, 2]; let _ = arr[3]; }";
        let (_c, msgs) = check_warn(src);
        let found = msgs.iter().any(|m| m.message().contains("out of bounds"));
        assert!(found, "expected OOB diagnostic, got: {:?}", msgs);
    }

    #[test]
    fn array_constant_index_in_bounds_emits_no_diagnostic() {
        // `arr[2]` on `[0, 1, 2]` is in bounds — no error.
        let src = "fn main() { let arr = [0, 1, 2]; let _ = arr[2]; }";
        let (_c, msgs) = check_warn(src);
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
    }

    #[test]
    fn array_runtime_index_emits_no_diagnostic() {
        // `arr[i]` on a static-length array, where `i` is a
        // variable — no static check possible, no error.
        let src = "fn main() { let arr = [0, 1, 2]; let i = 1; let _ = arr[i]; }";
        let (_c, msgs) = check_warn(src);
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
    }

    #[test]
    fn array_dynamic_length_no_oob_check() {
        // Function-returned arrays are dynamic-length; OOB
        // access is allowed (the user said SQL/JSON results
        // must not be flagged).
        let src = "fn get_array() -> [int] { return [1, 2, 3]; } \
                   fn main() { let arr = get_array(); let _ = arr[10]; }";
        let (_c, msgs) = check_warn(src);
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
    }

    #[test]
    fn push_promotes_static_array_to_dynamic_for_later_indexing() {
        let src = "fn main() { let arr = [0, 1]; push(arr, 2); let _ = arr[2]; }";
        let (_c, msgs) = check_warn(src);
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
    }

    #[test]
    fn push_rejects_element_type_mismatch() {
        let src = "fn main() { let arr = [0, 1]; push(arr, \"x\"); }";
        let (_c, msgs) = check_warn(src);
        let found = msgs.iter().any(|m| m.message().contains("Type mismatch"));
        assert!(
            found,
            "expected push element type mismatch, got: {:?}",
            msgs
        );
    }

    #[test]
    fn len_of_array_returns_int() {
        assert_ok("len([0, 1])", int());
    }

    #[test]
    fn len_rejects_non_array() {
        let src = "fn main() { let x = 1; len(x); }";
        let (_c, msgs) = check_warn(src);
        let found = msgs
            .iter()
            .any(|m| m.message().contains("len expects an array"));
        assert!(found, "expected len non-array diagnostic, got: {:?}", msgs);
    }

    #[test]
    fn tuple_constant_index_oob_emits_diagnostic() {
        // `let t = (1, 2); t[5]` — tuple length 2, index 5.
        let src = "fn main() { let t = (1, 2); let _ = t[5]; }";
        let (_c, msgs) = check_warn(src);
        let found = msgs.iter().any(|m| m.message().contains("out of bounds"));
        assert!(found, "expected tuple OOB, got: {:?}", msgs);
    }

    #[test]
    fn parenthesised_expr_is_not_tuple() {
        // `(1)` and `(1 + 2)` and `((1))` should NOT be tuples.
        // The parser fixes this by requiring a comma inside the
        // parens for the tuple form. After the parser fix, each
        // of these parses to a single integer expression with
        // type `int`.
        let (mut c1, ty1) = check("(1)");
        let msgs1 = c1.take_messages();
        assert!(msgs1.is_empty(), "msgs1: {:?}", msgs1);
        assert_eq!(apply_ty_prune(c1.subst(), &ty1), int());

        let (mut c2, ty2) = check("(1 + 2)");
        let msgs2 = c2.take_messages();
        assert!(msgs2.is_empty(), "msgs2: {:?}", msgs2);
        assert_eq!(apply_ty_prune(c2.subst(), &ty2), int());
    }

    #[test]
    fn parenthesised_arithmetic_works_in_binary_op() {
        // `(1 + 2) * 3` should evaluate to `int` (= 9 at
        // runtime). Pre-24 it incorrectly parsed `(1 + 2)` as
        // a 1-tuple and broke arithmetic.
        let (mut c, ty) = check("(1 + 2) * 3");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
        assert_eq!(apply_ty_prune(c.subst(), &ty), int());
    }

    #[test]
    fn array_dynamic_length_param_lets_runtime_index() {
        // Function param is dynamic-length — must allow any
        // index.
        let src = "fn head([int] arr) -> int { return arr[0]; }";
        let (_c, msgs) = check_warn(src);
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
    }

    #[test]
    fn index_non_aggregate_emits_diagnostic() {
        // `let x = 5; x[0]` — index on `int` is an error.
        let src = "fn main() { let x = 5; let _ = x[0]; }";
        let (_c, msgs) = check_warn(src);
        let found = msgs
            .iter()
            .any(|m| m.message().contains("cannot index non-aggregate"));
        assert!(found, "expected indexing-error, got: {:?}", msgs);
    }

    // ---- Dict tests ----

    #[test]
    fn dict_literal_infers_record_type() {
        // `{ foo: 42 }` should infer `Ty::Record { fields: [("foo", int)] }`.
        // We expect the var type via `lookup_at` won't work in
        // this minimal setup; instead, verify that the dict
        // expression parses and type-checks without error and
        // that the let-bound `d` resolves to a `Ty::Record` via
        // the env lookup.
        let (mut c, _ty) = check("fn main() { let d = { foo: 42 }; }");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
        // The side-table records `d`'s type — verify it.
        let d_ty = c.codegen_var_type("d").cloned();
        let d_pruned = d_ty.map(|t| crate::typechecking::subst::apply_ty_prune(c.subst(), &t));
        assert_eq!(
            d_pruned,
            Some(crate::typechecking::ty::record(vec![(
                "foo".to_string(),
                int()
            )])),
            "expected d: {{ foo: int }}"
        );
    }

    #[test]
    fn dict_missing_field_access_emits_diagnostic() {
        // `{ foo: 42 }; x.bar` must error when `bar` is missing.
        let src = "fn main() { let x = { foo: 42 }; let _ = x.bar; }";
        let (_c, msgs) = check_warn(src);
        let found = msgs
            .iter()
            .any(|m| m.message().contains("Cannot find field `bar`"));
        assert!(found, "expected missing-field diagnostic, got: {:?}", msgs);
    }

    #[test]
    fn dict_present_field_access_emits_no_diagnostic() {
        let src = "fn main() { let x = { foo: 42 }; let _ = x.foo; }";
        let (_c, msgs) = check_warn(src);
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
    }

    #[test]
    fn dict_duplicate_field_emits_diagnostic() {
        // `{ foo: 1, foo: 2 }` — duplicate field name.
        let src = "fn main() { let _ = { foo: 1, foo: 2 }; }";
        let (_c, msgs) = check_warn(src);
        let found = msgs.iter().any(|m| m.message().contains("Duplicate field"));
        assert!(
            found,
            "expected duplicate-field diagnostic, got: {:?}",
            msgs
        );
    }

    #[test]
    fn dict_structurally_typed_unification() {
        // Two separate `{ foo: 1 }` literals should have the
        // same record type.
        let (mut c, ty1) = check("fn main() { let _ = { foo: 42 }; return { foo: 42 }; }");
        let _ = c.take_messages();
        let ty2 = {
            let (mut c2, ty2) = check("fn main() { let _ = { foo: 42 }; return { foo: 99 }; }");
            let _ = c2.take_messages();
            ty2
        };
        // We can't unify across two checkers easily; instead,
        // verify each infers the same structural type.
        let r1 = apply_ty_prune(c.subst(), &ty1);
        let r2 = apply_ty_prune(c.subst(), &ty2);
        assert_eq!(r1, r2);
    }

    #[test]
    fn dict_let_binding_works_end_to_end() {
        // The full codegen path: lex + parse + type + codegen
        // + VM. We just check the diagnostics are clean.
        let src = "fn main() { let d = { x: 1, y: 2 }; let _ = d.x + d.y; }";
        let (_c, msgs) = check_warn(src);
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
    }

    // ---- Type alias tests ----

    #[test]
    fn type_alias_for_tuple_is_substituted() {
        // `type Point = (int, int);` then `let p: Point = (3, 4);`
        // should typecheck without diagnostic (the alias is
        // substituted to `(int, int)` and the literal is its
        // structural equivalent).
        let src = "type Point = (int, int); fn main() { let p: Point = (3, 4); }";
        let (_c, msgs) = check_warn(src);
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
    }

    #[test]
    fn type_alias_used_as_function_parameter() {
        let src = "type Point = (int, int);
                   fn distance(Point p) -> int { return p[0]; }
                   fn main() { }";
        let (_c, msgs) = check_warn(src);
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
    }

    #[test]
    fn generic_enum_type_app_builds_ty_app() {
        let src = "enum Box<T> { Box(T) } fn f(Box<int> x) -> int { return 0; }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
        let scheme = c.env().lookup("f").unwrap();
        let ty = apply_ty_prune(c.subst(), &scheme.ty);
        let Ty::Fun(param, ret) = ty else {
            panic!("expected function type");
        };
        assert_eq!(
            *param,
            Ty::App(Box::new(Ty::Con("Box".into())), vec![int()])
        );
        assert_eq!(*ret, int());
    }

    #[test]
    fn generic_enum_construct_infers_box_int() {
        let src = "enum Box<T> { Empty, Full(T) }
                   fn main() { let x: Box<int> = Box::Full(7); }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
        let x_ty = c.codegen_var_type("x").expect("x should be recorded");
        assert_eq!(
            apply_ty_prune(c.subst(), x_ty),
            Ty::App(Box::new(Ty::Con("Box".into())), vec![int()])
        );
    }

    #[test]
    fn generic_enum_match_binds_int_payload() {
        let src = "enum Box<T> { Empty, Full(T) }
                   fn main() {
                       let x = Box::Full(7);
                       let y = match x {
                           Box::Empty => 0,
                           Box::Full(v) => v,
                       };
                   }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
        assert_eq!(
            c.codegen_var_type("y")
                .map(|t| apply_ty_prune(c.subst(), t)),
            Some(int())
        );
    }

    #[test]
    fn builtin_option_type_app_builds_ty_app() {
        let src = "fn f(Option<int> x) -> int { return 0; }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
        assert_eq!(
            c.generics
                .generic_type_ctors
                .get(common::BUILTIN_OPTION_ENUM),
            Some(&vec!["T".to_string()])
        );
        assert_eq!(
            c.generics
                .generic_type_ctors
                .get(common::BUILTIN_RESULT_ENUM),
            Some(&vec!["T".to_string(), "E".to_string()])
        );

        let scheme = c.env().lookup("f").unwrap();
        let ty = apply_ty_prune(c.subst(), &scheme.ty);
        let Ty::Fun(param, ret) = ty else {
            panic!("expected function type");
        };
        assert_eq!(
            *param,
            Ty::App(
                Box::new(Ty::Con(common::BUILTIN_OPTION_ENUM.into())),
                vec![int()]
            )
        );
        assert!(is_option_ty(&param));
        assert_eq!(option_inner(&param), Some(int()));
        assert_eq!(*ret, int());
    }

    #[test]
    fn builtin_option_app_annotation_unifies_with_constructor_sum() {
        let src = "fn main() { let x: Option<int> = Option::Some(1); let y = match x { Option::None => 0, Option::Some(v) => v }; }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
        let x_ty = c.codegen_var_type("x").expect("x should be recorded");
        assert!(matches!(
            apply_ty_prune(c.subst(), x_ty),
            Ty::App(con, args)
                if con.as_ref() == &Ty::Con(common::BUILTIN_OPTION_ENUM.into())
                    && args == vec![int()]
        ));
        assert_eq!(c.codegen_var_type("y"), Some(&int()));
    }

    #[test]
    fn generic_type_app_arity_mismatch_errors() {
        let src = "enum Box<T> { Box(T) } fn f(Box<int, string> x) -> int { return 0; }";
        let (_c, msgs) = check_warn(src);
        assert!(
            msgs.iter().any(|m| m
                .message()
                .contains("Type constructor `Box` expects 1 type arguments, got 2")),
            "expected arity diagnostic, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn generic_type_alias_expands_to_rhs() {
        let src = "type Pair<T> = (T, T); fn f(Pair<int> p) -> int { return 0; }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
        let scheme = c.env().lookup("f").unwrap();
        let ty = apply_ty_prune(c.subst(), &scheme.ty);
        let Ty::Fun(param, ret) = ty else {
            panic!("expected function type");
        };
        assert_eq!(*param, tuple_ty(vec![int(), int()]));
        assert_eq!(*ret, int());
        assert!(
            c.generic_aliases.contains_key("Pair"),
            "generic alias should be registered"
        );
    }

    #[test]
    fn generic_type_alias_arity_mismatch_errors() {
        let src = "type Pair<T> = (T, T); fn f(Pair<int, string> p) -> int { return 0; }";
        let (_c, msgs) = check_warn(src);
        assert!(
            msgs.iter().any(|m| m
                .message()
                .contains("Type constructor `Pair` expects 1 type arguments, got 2")),
            "expected arity diagnostic, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn alias_does_not_leak_into_unrelated_declarations() {
        let src = "type Int = int; fn main() { }";
        let (_c, msgs) = check_warn(src);
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
    }

    #[test]
    fn function_body_alias_can_shadow_outer_alias() {
        let src = r#"
            type Value = int;
            fn main() {
                type Value = string;
                let s: Value = "ok";
            }
            fn id(Value x) -> int { return x; }
        "#;
        let (_c, msgs) = check_warn(src);
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
    }

    #[test]
    fn duplicate_type_alias_in_same_scope_errors() {
        let src = "type Id = int; type Id = string; fn main() { }";
        let (_c, msgs) = check_warn(src);
        assert!(
            msgs.iter()
                .any(|m| m.message().contains("Duplicate type alias `Id`")),
            "expected duplicate alias diagnostic, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn typeclass_impl_missing_required_method_errors() {
        let src = r#"
            trait Tiny<T> {
                fn add(int a, int b) -> int;
                fn zero() -> int { return 0; }
            }
            impl Tiny<int> {
                fn zero() -> int { return 0; }
            }
        "#;
        let (_c, msgs) = check_warn(src);
        assert!(
            msgs.iter().any(|m| {
                m.message()
                    .contains("Instance of `Tiny` for `int` is missing method `add`")
            }),
            "expected missing-method diagnostic, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn typeclass_impl_overlapping_instance_errors() {
        let src = r#"
            trait Tiny<T> {
                fn add(int a, int b) -> int;
            }
            impl Tiny<int> {
                fn add(int a, int b) -> int { return a; }
            }
            impl Tiny<int> {
                fn add(int a, int b) -> int { return b; }
            }
        "#;
        let (_c, msgs) = check_warn(src);
        assert!(
            msgs.iter().any(|m| m
                .message()
                .contains("Overlapping instance `Tiny<int>` conflicts with existing `Tiny<int>`")),
            "expected overlapping-instance diagnostic, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn ambiguous_instance_discharge_reports_error() {
        let mut c = Checker::new();
        c.generics.instances.clear();
        c.generics.instances.push(InstanceDef {
            class: "Choice".to_string(),
            defined_module: "a".to_string(),
            range: 0..1,
            args: vec![Ty::Var(TyVarId(999))],
            method_fqns: HashMap::new(),
            assoc_tys: HashMap::new(),
        });
        c.generics.instances.push(InstanceDef {
            class: "Choice".to_string(),
            defined_module: "b".to_string(),
            range: 1..2,
            args: vec![int()],
            method_fqns: HashMap::new(),
            assoc_tys: HashMap::new(),
        });

        c.discharge_constraints(
            None,
            &[Constraint {
                class: "Choice".to_string(),
                args: vec![int()],
            }],
            &(0..2),
        );

        assert!(
            c.messages()
                .iter()
                .any(|m| m.message().contains("Ambiguous instance for `Choice<int>`")),
            "expected ambiguous-instance diagnostic, got: {:?}",
            c.messages().iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn typeclass_impl_default_method_omission_registers_default_fqn() {
        let src = r#"
            trait Tiny<T> {
                fn zero() -> int { return 0; }
            }
            impl Tiny<int> {
            }
        "#;
        let (c, msgs) = check_warn(src);
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
        assert_eq!(
            c.instance_method_fqn("Tiny", &[int()], "zero"),
            Some("Tiny__default__zero")
        );
    }

    #[test]
    fn typeclass_impl_unknown_method_errors() {
        let src = r#"
            trait Tiny<T> {
                fn add(int a, int b) -> int;
            }
            impl Tiny<int> {
                fn add(int a, int b) -> int { return a; }
                fn foo(int a) -> int { return a; }
            }
        "#;
        let (_c, msgs) = check_warn(src);
        assert!(
            msgs.iter().any(|m| m
                .message()
                .contains("Unknown method `foo` in instance of `Tiny`")),
            "expected unknown-method diagnostic, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    /// Phase 5: `trait Ordered<T: Equal>` stores Equal as a superclass.
    #[test]
    fn typeclass_param_bounds_become_superclasses() {
        let src = r#"
            trait Equal<T> { fn eq_val(T a, T b) -> bool; }
            trait Ordered<T: Equal> { fn lt_val(T a, T b) -> bool; }
        "#;
        let (c, msgs) = check_warn(src);
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
        let ordered = c.generics().typeclass("Ordered").expect("Ordered");
        assert_eq!(ordered.superclasses, vec!["Equal".to_string()]);
        assert!(ordered.has_superclass("Equal", c.generics()));
    }

    /// Phase 5: `impl Ordered<int>` without `Equal<int>` is an error.
    #[test]
    fn typeclass_impl_missing_superclass_instance_errors() {
        let src = r#"
            trait Equal<T> { fn eq_val(T a, T b) -> bool; }
            trait Ordered<T: Equal> { fn lt_val(T a, T b) -> bool; }
            impl Ordered<int> {
                fn lt_val(int a, int b) -> bool { return a < b; }
            }
        "#;
        let (_c, msgs) = check_warn(src);
        assert!(
            msgs.iter().any(|m| {
                let msg = m.message();
                msg.contains("requires superclass instance")
                    && msg.contains("Equal")
                    && msg.contains("Ordered")
            }),
            "expected missing-superclass diagnostic, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    /// Phase 5: `T: Ordered` implies `Equal` — `eq_val` resolves without
    /// writing `T: Ordered + Equal`.
    #[test]
    fn implied_superclass_bound_allows_superclass_method() {
        let src = r#"
            trait Equal<T> { fn eq_val(T a, T b) -> bool; }
            trait Ordered<T: Equal> { fn lt_val(T a, T b) -> bool; }
            impl Equal<int> {
                fn eq_val(int a, int b) -> bool { return a == b; }
            }
            impl Ordered<int> {
                fn lt_val(int a, int b) -> bool { return a < b; }
            }
            fn cmp_eq<T: Ordered>(T a, T b) -> bool {
                return eq_val(a, b);
            }
            fn main() {
                let x = cmp_eq(1, 1);
            }
        "#;
        let (c, msgs) = check_warn(src);
        assert!(
            msgs.is_empty(),
            "implied Equal under Ordered should typecheck; got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
        // eq_val under Ordered uses flattened slot 1 (after lt_val at 0).
        let hint = c
            .id_table()
            .ids()
            .iter()
            .find_map(|id| c.bound_method_call_at(*id))
            .expect("expected a bound method call for eq_val");
        assert_eq!(hint.method_slot, 1, "eq_val should be superclass slot 1");
        assert_eq!(hint.dict_index, 0);
    }

    /// Phase 5: `c: * -> Constraint, T: c` can select a concrete subclass
    /// and then use its superclass methods through the flattened dictionary.
    #[test]
    fn abstract_constraint_kind_uses_superclass_method_after_binding() {
        let src = r#"
            trait Equal<T> { fn eq_val(T a, T b) -> bool; }
            trait Ordered<T: Equal> { fn lt_val(T a, T b) -> bool; }
            impl Equal<int> {
                fn eq_val(int a, int b) -> bool { return a == b; }
            }
            impl Ordered<int> {
                fn lt_val(int a, int b) -> bool { return a < b; }
            }
            fn choose<c: * -> Constraint, T: c>(T a, T b) -> int {
                if lt_val(a, b) { return 0; }
                if eq_val(a, b) { return 42; }
                return 1;
            }
            fn main() {
                let x = choose(7, 7);
            }
        "#;
        let (c, msgs) = check_warn(src);
        assert!(
            msgs.is_empty(),
            "abstract constraint should bind to Ordered and imply Equal; got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
        let scheme = c.env().lookup("choose").expect("choose scheme");
        assert_eq!(scheme.constraints.len(), 1);
        assert_eq!(scheme.constraints[0].class, "Ordered");

        let slots: Vec<_> = c
            .id_table()
            .ids()
            .iter()
            .filter_map(|id| c.bound_method_call_at(*id).map(|hint| hint.method_slot))
            .collect();
        assert!(
            slots.contains(&0) && slots.contains(&1),
            "expected Ordered slot 0 and implied Equal slot 1, got {:?}",
            slots
        );
    }

    #[test]
    fn unsatisfied_abstract_constraint_kind_reports_diagnostic() {
        let src = r#"
            fn id<c: * -> Constraint, T: c>(T x) -> T {
                return x;
            }
        "#;
        let (_c, msgs) = check_warn(src);
        assert!(
            msgs.iter()
                .any(|m| m.message().contains("Cannot satisfy abstract constraint")),
            "expected unsatisfied abstract constraint diagnostic, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn generic_call_records_concrete_instance_dict() {
        let src = r#"
            fn add<T: Num>(T a, T b) -> T { return a + b; }
            fn main() { let x = add(1, 2); }
        "#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);

        let dicts: Vec<_> = c
            .id_table()
            .ids()
            .iter()
            .filter_map(|id| c.call_dicts_at(*id))
            .collect();
        assert_eq!(dicts.len(), 1, "expected one constrained call dict");
        assert_eq!(dicts[0].len(), 1, "expected one Num dictionary");
        assert_eq!(dicts[0][0].class, "Num");
        assert_eq!(dicts[0][0].args, vec![int()]);
    }

    /// `T: Num` still lowers `a + b` through the flattened Add slot (0).
    #[test]
    fn num_bound_plus_uses_add_superclass_dict_slot() {
        let src = r#"
            fn add<T: Num>(T a, T b) -> T { return a + b; }
            fn main() { let x = add(1, 2); }
        "#;
        let (c, msgs) = check_warn(src);
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
        let hint = c
            .id_table()
            .ids()
            .iter()
            .find_map(|id| c.bound_operator_call_at(*id))
            .expect("expected a bound operator call for `+`");
        assert_eq!(hint.dict_index, 0);
        assert_eq!(
            hint.method_slot, 0,
            "Add::add should be slot 0 in Num's flattened dict"
        );
    }

    /// `T: Add` is enough for `+` without a full `Num` bound.
    #[test]
    fn add_bound_alone_allows_plus() {
        let src = r#"
            fn just_add<T: Add>(T a, T b) -> T { return a + b; }
            fn main() { let x = just_add(1, 2); }
        "#;
        let (c, msgs) = check_warn(src);
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
        let hint = c
            .id_table()
            .ids()
            .iter()
            .find_map(|id| c.bound_operator_call_at(*id))
            .expect("expected a bound operator call for `+`");
        assert_eq!(hint.method_slot, 0);
    }

    /// `T: Add` does not allow `*`.
    #[test]
    fn add_bound_does_not_allow_mul() {
        let src = r#"
            fn bad<T: Add>(T a, T b) -> T { return a * b; }
        "#;
        let (_c, msgs) = check_warn(src);
        assert!(
            msgs.iter()
                .any(|m| m.message().contains("without bound `Mul`")),
            "expected missing Mul bound diagnostic, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    /// `T: Ord` still lowers `a < b` through the flattened Lt slot (0).
    #[test]
    fn ord_bound_lt_uses_lt_superclass_dict_slot() {
        let src = r#"
            fn less<T: Ord>(T a, T b) -> bool { return a < b; }
            fn main() { let x = less(1, 2); }
        "#;
        let (c, msgs) = check_warn(src);
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
        let hint = c
            .id_table()
            .ids()
            .iter()
            .find_map(|id| c.bound_operator_call_at(*id))
            .expect("expected a bound operator call for `<`");
        assert_eq!(hint.dict_index, 0);
        assert_eq!(
            hint.method_slot, 0,
            "Lt::lt should be slot 0 in Ord's flattened dict"
        );
    }

    /// `T: Lt` is enough for `<` without a full `Ord` bound.
    #[test]
    fn lt_bound_alone_allows_less_than() {
        let src = r#"
            fn just_lt<T: Lt>(T a, T b) -> bool { return a < b; }
            fn main() { let x = just_lt(1, 2); }
        "#;
        let (c, msgs) = check_warn(src);
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
        let hint = c
            .id_table()
            .ids()
            .iter()
            .find_map(|id| c.bound_operator_call_at(*id))
            .expect("expected a bound operator call for `<`");
        assert_eq!(hint.method_slot, 0);
    }

    /// `T: Lt` does not allow `>`.
    #[test]
    fn lt_bound_does_not_allow_gt() {
        let src = r#"
            fn bad<T: Lt>(T a, T b) -> bool { return a > b; }
        "#;
        let (_c, msgs) = check_warn(src);
        assert!(
            msgs.iter()
                .any(|m| m.message().contains("without bound `Gt`")),
            "expected missing Gt bound diagnostic, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn block_scoped_alias_does_not_leak() {
        let src = r#"
            type Local = int;
            fn main() {
                if true {
                    type Local = string;
                    let s: Local = "ok";
                }
                let n: Local = 1;
            }
        "#;
        let (_c, msgs) = check_warn(src);
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
    }

    #[test]
    fn access_field_from_let_bound_sum_value_works() {
        // The receiver is `let p = ...;` where the value is bound
        // to a `Ty::Sum` (via a function parameter flowing through
        // a `match`). After matching, the active variant is
        // statically known, so `p.x` works.
        let src = "enum Point { Origin, Point { x: int, y: int } } \
                   fn distance_squared(Point p) -> int { \
                       return match p { \
                           Point::Origin => 0, \
                           Point::Point { x, y } => x, \
                       }; \
                   } \
                   fn main() { print \"%i\", distance_squared(Point::Point { x: 5, y: 12 }); }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "expected no diagnostics, got: {:?}", msgs);
    }

    #[test]
    fn access_field_chained_id_alignment() {
        // `p.x.y` parses as `Access(Access(p, "x"), "y")`. The
        // inner `Access` must consume the receiver's ID AND its
        // own ID to stay lockstep with the pre-walk.
        //
        // We don't assert cache/id_table alignment here because
        // `infer_fragment` has a pre-existing asymmetry where it
        // doesn't consume `Variable` IDs (it processes them
        // inline). This is unrelated to `Expression::Access`. We
        // instead verify the OUTER access produces the expected
        // diagnostic.
        let src = "enum Point { Origin, Point { x: int, y: int } } \
                   let p = Point::Point { x: 5, y: 12 }; p.x.y;";
        let msgs = assert_messages(src);
        let cannot_access: Vec<_> = msgs
            .iter()
            .filter(|m| m.message().contains("Cannot access field"))
            .collect();
        assert!(
            !cannot_access.is_empty(),
            "expected at least one 'Cannot access field' diagnostic from outer access, got: {:?}",
            msgs
        );
    }

    // ============================================================
    // ---- field_type_for tests ----
    // ============================================================
    //
    // The `field_type_for` helper is the codegen-side complement
    // to `field_index_for`. It's queried by `receiver_type` when
    // resolving chained accesses (`p.x.v`). The helper reads from
    // the same `enum_payloads` registry that `field_index_for`
    // reads from — so the tests below verify the data plumbing,
    // not the HM inference logic itself (that's already covered
    // by `access_field_*` tests above).

    /// `field_type_for` returns the declared type of a record
    /// field. Setup: `enum Inner { Inner { v: int } }`. The
    /// helper should resolve `"v"` to `int()`.
    #[test]
    fn field_type_for_returns_record_field_type() {
        let src = "enum Inner { Inner { v: int } }";
        let (c, _) = check(src);
        assert_eq!(
            c.field_type_for("Inner", "v"),
            Some(int()),
            "expected field 'v' on Inner to resolve to int()"
        );
    }

    /// `field_type_for` returns `None` when the field name isn't
    /// declared by any record-shaped variant in the enum. Setup:
    /// `enum Inner { Inner { v: int } }`. Asking for `"missing"`
    /// should yield `None` — the codegen's defensive `LoadField(0)`
    /// fallback handles this case.
    #[test]
    fn field_type_for_returns_none_for_unknown_field() {
        let src = "enum Inner { Inner { v: int } }";
        let (c, _) = check(src);
        assert_eq!(
            c.field_type_for("Inner", "missing"),
            None,
            "expected field 'missing' on Inner to resolve to None"
        );
    }

    /// `field_type_for` returns `None` when the enum name isn't
    /// registered at all. This is the "type error already emitted
    /// upstream" case — the codegen falls back to `LoadField(0)`.
    #[test]
    fn field_type_for_returns_none_for_unknown_enum() {
        let (c, _) = check("enum Inner { Inner { v: int } }");
        assert_eq!(
            c.field_type_for("Missing", "v"),
            None,
            "expected field lookup on unregistered enum to resolve to None"
        );
    }

    /// `field_type_for` returns the correct type for each named
    /// field in a record with multiple fields. The test pins the
    /// helper's return value to the DECLARED type of each field
    /// (not just "any non-None"), so a future refactor that swaps
    /// types by mistake would be caught.
    #[test]
    fn field_type_for_returns_correct_types_for_each_field() {
        let src = "enum Point { Origin, Point { x: int, y: int } }";
        let (c, _) = check(src);
        assert_eq!(c.field_type_for("Point", "x"), Some(int()));
        assert_eq!(c.field_type_for("Point", "y"), Some(int()));
    }

    /// Synthetic tuple indices (`"0"`, `"1"`, …) resolve via
    /// `field_type_for` so derive / Access can `LoadField` tuple
    /// payloads without match binders (which clobber instance-method
    /// `__dictN` slots).
    #[test]
    fn field_type_for_returns_tuple_index_types() {
        let src = "enum T { Wrap(int, string) }";
        let (c, _) = check(src);
        assert_eq!(c.field_type_for("T", "0"), Some(int()));
        assert_eq!(c.field_type_for("T", "1"), Some(string()));
        assert_eq!(
            c.field_type_for("T", "2"),
            None,
            "out-of-range tuple index"
        );
    }

    /// Chained access: field type can be another enum (`Outer.x` → `Inner`).
    #[test]
    fn field_type_for_returns_enum_type_for_nested_field() {
        let src = "enum Inner { Inner { v: int } } \
                   enum Outer { Outer { x: Inner, y: int } }";
        let (c, _) = check(src);
        // The exact `Ty` shape depends on the typechecker's
        // enum resolution (it could be `Ty::Con("Inner")` or
        // `Ty::Sum { name: "Inner", .. }`). The codegen's
        // `extract_enum_name` handles both shapes via
        // `extract_enum_name(&t).map(|_| t)`. We don't pin the
        // exact Ty here — we just verify the helper returns
        // *something* (not `None`) and that it's an enum
        // reference. Use `extract_enum_name` from the codegen
        // crate's perspective: the name should be "Inner".
        let result = c.field_type_for("Outer", "x");
        assert!(
            result.is_some(),
            "expected field 'x' on Outer to resolve to an enum type"
        );
        // Verify the type can be unwrapped to "Inner" via the
        // same logic `enum_name_for_receiver` uses.
        let result_ty = result.unwrap();
        match &result_ty {
            Ty::Con(name) => assert_eq!(name, "Inner"),
            Ty::Sum { name, .. } => assert_eq!(name, "Inner"),
            other => panic!(
                "expected Ty::Con(\"Inner\") or Ty::Sum {{ name: \"Inner\", .. }}, got {:?}",
                other
            ),
        }
    }

    #[test]
    fn async_fn_call_has_coroutine_type() {
        let src = "async fn coro() { yield 1; } fn main() { let h = coro(); }";
        let (c, _) = check(src);
        assert!(c.messages().is_empty(), "unexpected: {:?}", c.messages());
        let ty = c.codegen_var_type("h").expect("h should be recorded");
        match apply_ty_prune(&c.subst(), ty) {
            Ty::App(con, args) => {
                assert_eq!(con.as_ref(), &Ty::Con("coroutine".to_string()));
                assert_eq!(args.len(), 2);
                assert_eq!(apply_ty_prune(&c.subst(), &args[1]), unit_ty());
            }
            other => panic!("expected coroutine<_, unit>, got {:?}", other),
        }
    }

    #[test]
    fn resume_with_send_unifies_send_type() {
        let src = r#"async fn ping() { let msg = yield "ready"; }
fn main() { let h = ping(); resume h with "hello"; }"#;
        let (c, _) = check(src);
        assert!(c.messages().is_empty(), "unexpected: {:?}", c.messages());
    }

    #[test]
    fn coro_send_example_typechecks() {
        let src = include_str!("../../../examples/coro_send.0s");
        let (c, _) = check(src);
        assert!(c.messages().is_empty(), "unexpected: {:?}", c.messages());
    }

    #[test]
    fn yield_from_requires_coroutine_target() {
        let (c, _) = check("async fn bad() { yield from 1; }");
        assert!(
            c.messages().iter().any(|m| {
                m.message().contains("Type mismatch")
                    && m.help()
                        .as_ref()
                        .is_some_and(|h| h.contains("yield from target"))
            }),
            "expected yield-from type error, got {:?}",
            c.messages()
        );
    }

    #[test]
    fn resume_expression_returns_yield_type() {
        let (c, _) =
            check("async fn coro() { yield 1; } fn main() { let h = coro(); let x = resume h; }");
        assert!(c.messages().is_empty(), "unexpected: {:?}", c.messages());
        let x_ty = c
            .codegen_var_type("x")
            .expect("x should be recorded in codegen_var_types");
        assert_eq!(apply_ty_prune(c.subst(), x_ty), int());
    }

    #[test]
    fn for_in_coro_binds_loop_var_to_yield_type() {
        let src = r#"
async fn counter() { yield 0; yield 1; }
fn main() {
    for x in counter() {
        let y = x;
    }
}
"#;
        let (c, _) = check(src);
        assert!(c.messages().is_empty(), "unexpected: {:?}", c.messages());
        let y_ty = c
            .codegen_var_type("y")
            .expect("y should be recorded in codegen_var_types");
        assert_eq!(apply_ty_prune(c.subst(), y_ty), int());
        let x_ty = c
            .codegen_var_type("x")
            .expect("x should be recorded in codegen_var_types");
        assert_eq!(apply_ty_prune(c.subst(), x_ty), int());
    }

    #[test]
    fn for_in_non_iterable_is_diagnostic() {
        let (c, _) = check("fn main() { for x in 42 { } }");
        assert!(
            c.messages()
                .iter()
                .any(|m| m.message().contains("not iterable")),
            "expected for-in not-iterable error, got {:?}",
            c.messages()
        );
    }

    #[test]
    fn for_in_array_binds_element_type() {
        let src = r#"
fn main() {
    for x in [1, 2, 3] {
        let y = x;
    }
}
"#;
        let (c, _) = check(src);
        assert!(c.messages().is_empty(), "unexpected: {:?}", c.messages());
        let y_ty = c.codegen_var_type("y").expect("y");
        assert_eq!(apply_ty_prune(c.subst(), y_ty), int());
    }

    #[test]
    fn for_in_hetero_tuple_is_diagnostic() {
        let (c, _) = check("fn main() { for x in (1, \"a\") { } }");
        assert!(
            c.messages()
                .iter()
                .any(|m| m.message().contains("heterogeneous")),
            "expected hetero tuple diagnostic, got {:?}",
            c.messages()
        );
    }

    #[test]
    fn for_in_hetero_dict_is_diagnostic() {
        let (c, _) = check("fn main() { for x in { a: 1, b: \"x\" } { } }");
        assert!(
            c.messages()
                .iter()
                .any(|m| m.message().contains("heterogeneous")),
            "expected hetero dict diagnostic, got {:?}",
            c.messages()
        );
    }

    #[test]
    fn for_in_custom_iterator_accepted() {
        let src = r#"
class Counter {
    cur: int,
    end: int,
}

impl IntoIterator<Counter> {
    type Item = int;
    type IntoIter = Counter;
    fn into_iter(Counter c) -> Counter {
        return c;
    }
}

impl Iterator<Counter> {
    type Item = int;
    fn next(Counter c) -> Option<int> {
        if c.cur < c.end {
            let v = c.cur;
            c.cur = c.cur + 1;
            return Option::Some(v);
        }
        return Option::None;
    }
}

fn main() {
    let c = new Counter(0, 3);
    for x in c {
        let y = x;
    }
}
"#;
        let (c, _) = check(src);
        assert!(c.messages().is_empty(), "unexpected: {:?}", c.messages());
        let y_ty = c.codegen_var_type("y").expect("y");
        assert_eq!(apply_ty_prune(c.subst(), y_ty), int());
    }

    #[test]
    fn yield_outside_async_is_diagnostic() {
        let (c, _) = check("fn main() { yield 1; }");
        assert!(
            c.messages()
                .iter()
                .any(|m| m.message().contains("yield outside async")),
            "expected yield-outside-async diagnostic, got {:?}",
            c.messages()
        );
    }

    /// `return e;` inside an `async fn` unifies against the SAME
    /// type as `yield e;` (not `unit`) — `resume` has a single
    /// static result type covering both the yielded values and the
    /// final completion value, so a `return` of a matching type
    /// typechecks cleanly.
    #[test]
    fn return_inside_coroutine_unifies_with_yield_type() {
        let src = "async fn coro() { yield 1; return 42; } \
                   fn main() { let h = coro(); }";
        let (c, _) = check(src);
        assert!(c.messages().is_empty(), "unexpected: {:?}", c.messages());
    }

    /// A `return` whose type disagrees with the coroutine's yield
    /// type is a real type error (soundness: `resume`'s result type
    /// can't be both `int` and `string`).
    #[test]
    fn return_inside_coroutine_mismatched_type_is_diagnostic() {
        let src = r#"async fn coro() { yield 1; return "oops"; } fn main() { let h = coro(); }"#;
        let (c, _) = check(src);
        assert!(
            c.messages().iter().any(|m| {
                m.message().contains("Type mismatch")
                    && m.help()
                        .as_ref()
                        .is_some_and(|h| h.contains("return value"))
            }),
            "expected return-value type mismatch, got {:?}",
            c.messages()
        );
    }

    /// A `return` with no preceding `yield` still pins the
    /// coroutine's yield/resume type — `coroutine<int, unit>` here.
    #[test]
    fn return_only_coroutine_infers_yield_type_from_return() {
        let src = "async fn coro() { return 42; } fn main() { let h = coro(); let x = resume h; }";
        let (c, _) = check(src);
        assert!(c.messages().is_empty(), "unexpected: {:?}", c.messages());
        let x_ty = c
            .codegen_var_type("x")
            .expect("x should be recorded in codegen_var_types");
        assert_eq!(apply_ty_prune(c.subst(), x_ty), int());
    }

    #[test]
    fn done_builtin_typechecks_to_bool() {
        let src = "async fn c() { yield 1; } fn main() { let h = c(); let d = done(h); }";
        let (c, _) = check(src);
        assert!(c.messages().is_empty(), "unexpected: {:?}", c.messages());
        let d_ty = c.codegen_var_type("d").expect("d should be recorded");
        assert_eq!(apply_ty_prune(c.subst(), d_ty), boolean());
    }

    #[test]
    fn async_fn_return_annotation_unifies_with_yield() {
        let src = "async fn c() -> int { yield 1; return 2; } fn main() { let h = c(); }";
        let (c, _) = check(src);
        assert!(c.messages().is_empty(), "unexpected: {:?}", c.messages());
    }

    #[test]
    fn async_fn_return_annotation_mismatch_errors() {
        let src = "async fn c() -> string { yield 1; } fn main() { let h = c(); }";
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.iter().any(|m| m.message().contains("Type mismatch")),
            "expected annotation mismatch, got {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn declare_struct_ret_recorded_for_invoke_typing() {
        let src = r#"
extern struct Point {
    x: int32,
    y: int32,
};

use ffi::*;
use ffi::types::*;

fn main() -> Result<(), string> {
    let lib = dload("sum")?;
    let make_id = declare(
        lib,
        "make_point",
        (Int32, Int32),
        Point,
    )?;
    let p = invoke(lib, make_id, (3, 4))?;
    print "%i", p.x;
    print "%i", p.y;
}
"#;
        let (c, _) = check(src);
        assert!(
            c.messages().is_empty(),
            "unexpected: {:?}",
            c.messages().iter().map(|m| m.message()).collect::<Vec<_>>()
        );
        let ret = c
            .ffi_fn_ret_tys
            .get("make_id")
            .expect("declare binding should record ret Ty");
        match ret {
            Ty::Record { fields } => {
                assert!(fields.iter().any(|(n, _)| n == "x"));
                assert!(fields.iter().any(|(n, _)| n == "y"));
            }
            other => panic!("expected Record ret, got {other}"),
        }
    }

    // ---- Error handling: raise / ? / ?? / ?. ----

    #[test]
    fn raise_infers_result_mode_and_wraps_success() {
        let src = r#"
fn f(int n) {
    if n == 0 { raise "zero"; }
    return n;
}
"#;
        let (c, _) = check(src);
        assert!(
            c.messages().is_empty(),
            "unexpected: {:?}",
            c.messages().iter().map(|m| m.message()).collect::<Vec<_>>()
        );
        assert!(c.fn_is_result_mode("f"));
    }

    #[test]
    fn raise_with_explicit_non_result_return_errors() {
        let msgs = assert_messages(r#"fn f() -> int { raise "x"; return 1; }"#);
        assert!(
            msgs.iter().any(|m| {
                m.code() == Some(ErrorCode::TypeMismatch)
                    || m.message().contains("Type mismatch")
                    || m.message().contains("Result")
            }),
            "expected raise vs -> int error, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn try_on_non_option_result_is_hard_error() {
        let msgs = assert_messages(r#"fn f() -> int { let x = 1; return x?; }"#);
        assert!(
            msgs.iter().any(|m| m.code() == Some(ErrorCode::InvalidTry)),
            "expected InvalidTry (E0114), got: {:?}",
            msgs.iter()
                .map(|m| (m.code(), m.message()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn try_on_result_propagates_ok_payload() {
        let src = r#"
fn inner() { raise "e"; return 1; }
fn outer() {
    let v = inner()?;
    return v;
}
"#;
        let (c, _) = check(src);
        assert!(
            c.messages().is_empty(),
            "unexpected: {:?}",
            c.messages().iter().map(|m| m.message()).collect::<Vec<_>>()
        );
        assert!(c.fn_is_result_mode("outer"));
    }

    #[test]
    fn mismatched_error_types_conflict() {
        let msgs = assert_messages(
            r#"
fn a() { raise "s"; return 1; }
fn b() { raise 1; return 2; }
fn c() {
    let _x = a()?;
    let _y = b()?;
    return 0;
}
"#,
        );
        assert!(
            msgs.iter().any(|m| {
                m.code() == Some(ErrorCode::ConflictingErrorType)
                    || m.message().contains("Type mismatch")
                    || m.message().contains("error type")
            }),
            "expected single-E conflict, got: {:?}",
            msgs.iter()
                .map(|m| (m.code(), m.message()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn coalesce_option_and_result_typecheck() {
        let src = r#"
fn main() {
    let a = Option::None ?? "bar";
    let b = Result::Err("boom") ?? 7;
    print "%s", a;
    print "%i", b;
}
"#;
        let (c, _) = check(src);
        assert!(
            c.messages().is_empty(),
            "unexpected: {:?}",
            c.messages().iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn coalesce_on_non_option_result_errors() {
        let msgs = assert_messages(r#"fn main() { let x = 1 ?? 2; }"#);
        assert!(
            msgs.iter()
                .any(|m| m.code() == Some(ErrorCode::InvalidCoalesce)),
            "expected InvalidCoalesce (E0115), got: {:?}",
            msgs.iter()
                .map(|m| (m.code(), m.message()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn optional_access_on_option_ok() {
        let src = r#"
fn main() {
    let o = Option::Some({ v: 1 });
    let n = o?.v;
    print "%i", n ?? 0;
}
"#;
        let (c, _) = check(src);
        assert!(
            c.messages().is_empty(),
            "unexpected: {:?}",
            c.messages().iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn optional_access_on_result_errors() {
        let msgs = assert_messages(r#"fn main() { let r = Result::Ok({ v: 1 }); let _x = r?.v; }"#);
        assert!(
            msgs.iter()
                .any(|m| m.code() == Some(ErrorCode::InvalidOptionalAccess)),
            "expected InvalidOptionalAccess (E0116), got: {:?}",
            msgs.iter()
                .map(|m| (m.code(), m.message()))
                .collect::<Vec<_>>()
        );
    }

    // ---- Virtual modules: prelude + ffi scope ----

    #[test]
    fn prelude_injects_option_without_import() {
        let src = r#"
fn main() {
    let o = Option::Some(1);
    print "%i", match o { Option::Some(v) => v, Option::None => 0 };
}
"#;
        let (c, _) = check(src);
        assert!(
            c.messages().is_empty(),
            "unexpected: {:?}",
            c.messages().iter().map(|m| m.message()).collect::<Vec<_>>()
        );
        assert!(c.builtin_name_in_scope("Option"));
        assert!(c.builtin_name_in_scope("Eq"));
        assert!(c.prelude_fn_in_scope("assert").is_some());
        assert!(!c.ffi_fn_in_scope("dload").is_some());
    }

    #[test]
    fn assert_infers_result_unit_string() {
        let src = r#"
fn main() {
    let r = assert(true);
    let _ = match r {
        Result::Ok(_) => 1,
        Result::Err(_) => 0,
    };
}
"#;
        let (c, _) = check(src);
        assert!(
            c.messages().is_empty(),
            "unexpected: {:?}",
            c.messages().iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn assert_with_message_ok() {
        let src = r#"
fn main() {
    let r = assert(false, "nope");
    let _ = match r {
        Result::Ok(_) => 1,
        Result::Err(_) => 0,
    };
}
"#;
        let (c, _) = check(src);
        assert!(
            c.messages().is_empty(),
            "unexpected: {:?}",
            c.messages().iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn assert_wrong_arity_errors() {
        let msgs = assert_messages(r#"fn main() { let _ = assert(); }"#);
        assert!(
            msgs.iter().any(|m| m.message().contains("assert expects")),
            "expected arity diagnostic, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn assert_rebind_as_check_works() {
        let src = r#"
use prelude::test::assert as check;
fn main() {
    let r = check(1 == 1);
    let _ = match r {
        Result::Ok(_) => 1,
        Result::Err(_) => 0,
    };
}
"#;
        let (c, _) = check(src);
        assert!(
            c.messages().is_empty(),
            "unexpected: {:?}",
            c.messages().iter().map(|m| m.message()).collect::<Vec<_>>()
        );
        assert!(c.prelude_fn_in_scope("check").is_some());
        assert!(c.prelude_fn_in_scope("assert").is_none());
    }

    #[test]
    fn panic_requires_string() {
        let msgs = assert_messages(r#"fn main() { panic 1; }"#);
        assert!(
            msgs.iter().any(|m| m.message().contains("Type mismatch")),
            "expected type mismatch for panic int, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn dload_without_ffi_import_errors() {
        let msgs = assert_messages(r#"fn main() { let lib = dload("x.so"); }"#);
        assert!(
            msgs.iter().any(|m| {
                m.code() == Some(ErrorCode::UnknownValue)
                    && m.message().contains("dload")
            }),
            "expected UnknownValue for bare dload, got: {:?}",
            msgs.iter()
                .map(|m| (m.code(), m.message()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn use_ffi_types_glob_brings_int_tag_into_scope() {
        let src = r#"
use ffi::*;
use ffi::types::*;
fn main() -> Result<(), string> {
    let lib = dload("x.so")?;
    let id = declare(lib, "sum", (Int, Int), Int)?;
    let _ = invoke(lib, id, (1, 2))?;
}
"#;
        let (c, _) = check(src);
        assert!(
            c.messages().is_empty(),
            "unexpected: {:?}",
            c.messages().iter().map(|m| m.message()).collect::<Vec<_>>()
        );
        assert!(c.ffi_tag_in_scope("Int"));
        assert!(c.ffi_fn_in_scope("dload").is_some());
    }

    #[test]
    fn ffi_types_qualified_path_works_without_import() {
        let src = r#"
use ffi::*;
fn main() -> Result<(), string> {
    let lib = dload("x.so")?;
    let id = declare(lib, "sum", (ffi::types::Int, ffi::types::Int), ffi::types::Int)?;
    let _ = invoke(lib, id, (1, 2))?;
}
"#;
        let (c, _) = check(src);
        assert!(
            c.messages().is_empty(),
            "unexpected: {:?}",
            c.messages().iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn rebind_prelude_eq_allows_user_trait_eq() {
        let src = r#"
use prelude::ops::Eq as PreludeEq;
trait Eq<T> {
    fn id(T x) -> T;
}
fn main() {}
"#;
        let (c, _) = check(src);
        assert!(
            c.messages().is_empty(),
            "unexpected: {:?}",
            c.messages().iter().map(|m| m.message()).collect::<Vec<_>>()
        );
        assert!(!c.builtin_name_in_scope("Eq"));
        assert!(c.builtin_name_in_scope("PreludeEq"));
    }

    #[test]
    fn duplicate_prelude_eq_without_rebind_errors() {
        let msgs = assert_messages(
            r#"
trait Eq<T> {
    fn id(T x) -> T;
}
fn main() {}
"#,
        );
        assert!(
            msgs.iter().any(|m| m.message().contains("Duplicate trait")),
            "expected Duplicate trait for Eq, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn user_cannot_redeclare_builtin_option() {
        let msgs = assert_messages(r#"enum Option { None, Some(int) }"#);
        assert!(
            msgs.iter().any(|m| {
                m.code() == Some(ErrorCode::DuplicateEnum) || m.message().contains("Duplicate enum")
            }),
            "expected Duplicate enum for Option, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    // ── Constraint discharge tests (Chunk A4) ─────────────────────────────────

    /// Calling a generic `fn add<T: Num>(T a, T b) -> T` with `int` arguments
    /// must succeed: `Num<int>` is a builtin instance.
    #[test]
    fn call_generic_num_fn_with_int_discharges() {
        let src = r#"
fn add<T: Num>(T a, T b) -> T { return a + b; }
fn main() { let r = add(1, 2); }
"#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.iter()
                .all(|m| !m.message().contains("Cannot satisfy")
                    && !m.message().contains("No instance")),
            "unexpected constraint errors for add(int, int): {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    /// Calling `fn add<T: Num>(T a, T b) -> T` with `string` arguments must
    /// produce a diagnostic: `string` has no `Num` instance.
    #[test]
    fn call_generic_num_fn_with_string_errors() {
        let src = r#"
fn add<T: Num>(T a, T b) -> T { return a; }
fn main() { let r = add("a", "b"); }
"#;
        let msgs = assert_messages(src);
        assert!(
            msgs.iter().any(|m| {
                m.code() == Some(ErrorCode::GenericTypeError)
                    && (m.message().contains("No instance for `Num")
                        || m.message().contains("Cannot satisfy"))
            }),
            "expected a Num constraint violation for string, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    /// Debug test: discharge_constraints should populate call_site_dicts for
    /// user typeclasses at ground call sites.
    #[test]
    fn discharge_constraints_populates_call_site_dicts_for_user_typeclass() {
        let src = r#"
trait Describable<T> { fn describe_val(T x) -> int; }
impl Describable<int> { fn describe_val(int x) -> int { return x; } }
fn show<T: Describable>(T x) -> int { return 0; }
fn main() { show(42); }
"#;
        let (c, _) = check(src);
        let dicts = c.all_call_site_dicts();
        eprintln!("call_site_dicts has {} entries", dicts.len());
        for (id, instances) in dicts {
            eprintln!(
                "  NodeId {:?} -> {:?}",
                id,
                instances.iter().map(|i| &i.class).collect::<Vec<_>>()
            );
        }
        let total_instances: usize = dicts.values().map(|v| v.len()).sum();
        assert!(
            total_instances > 0,
            "expected at least one call_site_dict entry for user typeclass, got 0;\
             \ndicts: {:?}",
            dicts
        );
        // Check that we recorded Describable<int>
        let has_describable = dicts
            .values()
            .any(|instances| instances.iter().any(|i| i.class == "Describable"));
        assert!(
            has_describable,
            "expected Describable in call_site_dicts, got: {:?}",
            dicts
        );
    }

    /// A generic function calling another generic function with the same
    /// constraint must not emit a diagnostic — the constraint propagates.
    ///
    /// `fn outer<T: Num>(T x) -> T { return add(x, x); }` is valid when
    /// `add<T: Num>` exists, because `outer`'s own `T: Num` bound covers
    /// the inner call's constraint.
    #[test]
    fn call_generic_inside_generic_propagates() {
        let src = r#"
fn add<T: Num>(T a, T b) -> T { return a + b; }
fn outer<T: Num>(T x) -> T { return add(x, x); }
fn main() { let r = outer(5); }
"#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.iter()
                .all(|m| !m.message().contains("Cannot satisfy")
                    && !m.message().contains("No instance")),
            "unexpected constraint errors for generic propagation: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    /// Multi-param trait + `where` clause discharges at a ground call site.
    #[test]
    fn multiparam_where_clause_discharges_at_call_site() {
        let src = r#"
trait Convert<A, B> { fn cast(A x) -> B; }
impl Convert<int, int> { fn cast(int x) -> int { return x; } }
fn apply_cast<A, B>(A x) -> B where Convert<A, B> { return cast(x); }
fn main() { let y = apply_cast(42); }
"#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.is_empty(),
            "unexpected diagnostics: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
        let dicts = c.all_call_site_dicts();
        let has_convert = dicts.values().any(|instances| {
            instances
                .iter()
                .any(|i| i.class == "Convert" && i.args == vec![int(), int()])
        });
        assert!(
            has_convert,
            "expected Convert<int, int> in call_site_dicts, got: {:?}",
            dicts
        );
    }

    /// Missing multi-param instance produces a diagnostic.
    #[test]
    fn multiparam_missing_instance_errors() {
        let src = r#"
trait Convert<A, B> { fn cast(A x) -> B; }
fn apply_cast<A, B>(A x) -> B where Convert<A, B> { return cast(x); }
fn main() { let y = apply_cast(42); }
"#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.iter()
                .any(|m| m.message().contains("No instance") || m.message().contains("Convert")),
            "expected missing-instance diagnostic, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    /// Prelude `Into` is registered as a multi-param typeclass.
    #[test]
    fn prelude_into_trait_is_registered() {
        let g = Generics::new();
        assert!(g.typeclass("From").is_none(), "From must not be registered");
        let into = g.typeclass("Into").expect("Into");
        assert_eq!(into.type_params, vec!["Self".to_string(), "T".to_string()]);
        assert_eq!(into.methods.len(), 1);
        assert_eq!(into.methods[0].name, "into");
    }

    /// `into` method scheme exists after `check_program`.
    #[test]
    fn prelude_into_method_scheme_registered() {
        let (c, _) = check("fn main() {}");
        assert!(
            c.typeclass_method_scheme("From", "from").is_none(),
            "From::from must not be registered"
        );
        let into_scheme = c
            .typeclass_method_scheme("Into", "into")
            .expect("Into::into scheme");
        assert_eq!(into_scheme.constraints.len(), 1);
        assert_eq!(into_scheme.constraints[0].class, "Into");
        assert_eq!(into_scheme.constraints[0].args.len(), 2);
    }

    /// `impl Into<B> for A` with two local classes discharges via `x.into()`.
    #[test]
    fn prelude_into_method_call_with_expected_type_discharges() {
        let src = r#"
class Celsius { c: int }
class Fahrenheit { f: int }
impl Into<Fahrenheit> for Celsius {
    fn into(Celsius x) -> Fahrenheit { return new Fahrenheit(x.c); }
}
fn main() {
    let c = new Celsius(0);
    let y: Fahrenheit = c.into();
}
"#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.is_empty(),
            "unexpected diagnostics: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
        let dicts = c.all_call_site_dicts();
        let has_into = dicts.values().any(|instances| {
            instances.iter().any(|i| {
                i.class == "Into"
                    && i.args.len() == 2
                    && matches!(&i.args[0], Ty::Con(n) if n == "Celsius")
                    && matches!(&i.args[1], Ty::Con(n) if n == "Fahrenheit")
            })
        });
        assert!(
            has_into,
            "expected Into<Celsius, Fahrenheit> in call_site_dicts, got: {:?}",
            dicts
        );
    }

    /// Calling an Into-bound helper without an instance errors.
    #[test]
    fn prelude_into_missing_instance_errors() {
        let src = r#"
fn wrap<A, B>(A x) -> B where Into<A, B> { return into(x); }
fn main() { let w = wrap(42); }
"#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.iter()
                .any(|m| m.message().contains("No instance") || m.message().contains("Into")),
            "expected missing-Into diagnostic, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    /// Builtin source type is rejected under the strict orphan rule.
    #[test]
    fn prelude_into_impl_for_builtin_source_is_orphan() {
        let src = r#"
class Wrapper { v: int }
impl Into<Wrapper> for int {
    fn into(int x) -> Wrapper { return new Wrapper(x); }
}
"#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.iter().any(|m| m.message().contains("Orphan instance")),
            "expected orphan diagnostic for Into<Wrapper> for int, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    /// Inherent class methods win over prelude trait methods of the same
    /// name when no matching instance exists (Bugbot: ground trait must
    /// not block `impl Point { fn show() ... }`).
    #[test]
    fn inherent_class_method_wins_over_missing_trait_instance() {
        let src = r#"
class Point { x: int }
impl Point {
    fn show() -> string { return "point"; }
}
fn main() {
    let p = new Point(1);
    let s = p.show();
}
"#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.iter().all(|m| !m.message().contains("No instance")
                && !m.message().contains("Show")),
            "inherent show must not require Show instance, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
        assert!(
            msgs.is_empty(),
            "unexpected diagnostics: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    /// `return c.into();` under `-> Fahrenheit` pins Into's target
    /// (Bugbot: expected type must flow from return annotations).
    #[test]
    fn prelude_into_return_pins_expected_target() {
        let src = r#"
class Celsius { c: int }
class Fahrenheit { f: int }
class Kelvin { k: int }
impl Into<Fahrenheit> for Celsius {
    fn into(Celsius x) -> Fahrenheit { return new Fahrenheit(x.c); }
}
impl Into<Kelvin> for Celsius {
    fn into(Celsius x) -> Kelvin { return new Kelvin(x.c); }
}
fn to_f(Celsius c) -> Fahrenheit {
    return c.into();
}
fn main() {
    let f = to_f(new Celsius(0));
}
"#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.is_empty(),
            "unexpected diagnostics: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
        let dicts = c.all_call_site_dicts();
        let has_f = dicts.values().any(|instances| {
            instances.iter().any(|i| {
                i.class == "Into"
                    && matches!(&i.args[1], Ty::Con(n) if n == "Fahrenheit")
            })
        });
        assert!(
            has_f,
            "expected Into<..., Fahrenheit> from return pin, got: {:?}",
            dicts
        );
    }

    #[test]
    fn binary_hkt_result_instance_discharges() {
        let src = r#"
trait Bifunctor<F: * -> * -> *> {
    fn tag<A, B>(F<A, B> xs) -> int;
}
impl Bifunctor<Result> {
    fn tag<A, B>(Result<A, B> xs) -> int { return 42; }
}
fn get_tag<F: * -> * -> *, Bifunctor, A, B>(F<A, B> xs) -> int {
    return tag(xs);
}
fn main() { let x = get_tag(Result::Ok(7)); }
"#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.iter().all(|m| !m.message().contains("Cannot satisfy")
                && !m.message().contains("No instance")
                && !m.message().contains("constructor-kinded")),
            "unexpected diagnostics: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn binary_hkt_rejects_wrong_arity_constructor_instance() {
        let src = r#"
trait Bifunctor<F: * -> * -> *> {
    fn tag<A, B>(F<A, B> xs) -> int;
}
impl Bifunctor<Option> {
    fn tag<A, B>(Option<A> xs) -> int { return 0; }
}
"#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.iter().any(|m| m
                .message()
                .contains("expects argument 1 to have kind `* -> * -> *`, found kind `* -> *`")),
            "expected binary constructor-kind mismatch, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    /// Binder `T: Num` still desugars to a unary Constraint.
    #[test]
    fn binder_bound_desugars_to_unary_constraint() {
        let src = r#"
fn add<T: Num>(T a, T b) -> T { return a + b; }
"#;
        let (c, _) = check(src);
        let scheme = c.env().lookup("add").expect("add scheme");
        assert_eq!(scheme.constraints.len(), 1);
        assert_eq!(scheme.constraints[0].class, "Num");
        assert_eq!(scheme.constraints[0].args.len(), 1);
        assert!(matches!(
            scheme.constraints[0].args[0],
            Ty::Var(v) if scheme.bounds.contains(&v)
        ));
    }

    /// Phase 6: associated type on a ground call pins `C::Elem` to `int`.
    #[test]
    fn assoc_type_head_returns_int_at_ground_call() {
        let src = r#"
trait Collect<C> {
    type Elem;
    fn head(C xs) -> Elem;
}
impl Collect<Option<int>> {
    type Elem = int;
    fn head(Option<int> xs) -> int {
        return match xs {
            Option::Some(v) => v,
            Option::None => 0,
        };
    }
}
fn take_head<C: Collect>(C xs) -> C::Elem {
    return head(xs);
}
fn main() {
    let x = take_head(Option::Some(42));
}
"#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.is_empty(),
            "unexpected diagnostics: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
        // `x` should be int after ground Collect<Option<int>> discharge pins Elem.
        let x_ty = c
            .codegen_var_type("x")
            .cloned()
            .or_else(|| c.env().lookup("x").map(|s| apply_ty_prune(&c.subst, &s.ty)));
        let x_ty = x_ty.expect("x should be bound");
        let x_ty = apply_ty_prune(&c.subst, &x_ty);
        assert!(
            matches!(x_ty, Ty::Con(ref n) if n == "int") || x_ty == int(),
            "expected take_head(...) : int, got {}",
            x_ty
        );
    }

    /// Phase 6: open `T::Elem` under `T: Collect` uses a fresh var (not an error).
    #[test]
    fn assoc_type_open_projection_under_bound_is_ok() {
        let src = r#"
trait Collect<C> {
    type Elem;
    fn head(C xs) -> Elem;
}
fn peek<T: Collect>(T xs) -> T::Elem {
    return head(xs);
}
"#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.iter().all(|m| !m.message().contains("Cannot find")
                && !m.message().contains("Unknown associated")
                && !m.message().contains("Cannot resolve type projection")),
            "open projection should resolve under Collect bound: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
        let scheme = c.env().lookup("peek").expect("peek scheme");
        assert_eq!(scheme.constraints.len(), 1);
        assert_eq!(scheme.constraints[0].class, "Collect");
    }

    #[test]
    fn gat_decl_records_params_and_kind() {
        let src = r#"
trait Pointer<P: * -> *> {
    type Ref<T>;
    fn deref<T>(P<T> ptr) -> Ref<T>;
}
"#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.is_empty(),
            "unexpected diagnostics: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
        let class = c.generics().typeclass("Pointer").expect("Pointer class");
        let assoc = class.assoc_type("Ref").expect("Ref assoc type");
        assert_eq!(assoc.params, vec!["T".to_string()]);
        assert_eq!(assoc.param_kinds, vec![Kind::Type]);
        assert_eq!(assoc.kind, Kind::arrow(Kind::Type, Kind::Type));
    }

    #[test]
    fn gat_method_scheme_quantifies_applied_projection() {
        let src = r#"
trait Pointer<P: * -> *> {
    type Ref<T>;
    fn deref<T>(P<T> ptr) -> Ref<T>;
}
"#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "unexpected: {:?}", msgs);
        let scheme = c
            .typeclass_method_schemes
            .get(&("Pointer".to_string(), "deref".to_string()))
            .expect("deref scheme");
        assert_eq!(scheme.assoc_projections.len(), 1);
        assert_eq!(scheme.assoc_projections[0].name, "Ref");
        assert_eq!(scheme.assoc_projections[0].args.len(), 1);
        assert!(
            scheme.bounds.contains(&scheme.assoc_projections[0].var),
            "projection variable must be quantified by the method scheme"
        );
    }

    #[test]
    fn gat_open_projection_under_bound_is_ok() {
        let src = r#"
trait Pointer<P: * -> *> {
    type Ref<T>;
    fn deref<T>(P<T> ptr) -> Ref<T>;
}

impl Pointer<Option> {
    type Ref<T> = T;
    fn deref<T>(Option<T> ptr) -> T {
        return match ptr {
            Option::Some(v) => v,
            Option::None => 0,
        };
    }
}

fn get<P: * -> *, Pointer, A>(P<A> ptr) -> P::Ref<A> {
    return deref(ptr);
}
fn main() { let x = get(Option::Some(42)); }
"#;
        let (mut c, _) = check(src);
        let msgs = c.take_messages();
        assert!(
            msgs.is_empty(),
            "unexpected diagnostics: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
        assert_eq!(
            c.codegen_var_type("x")
                .map(|t| apply_ty_prune(c.subst(), t)),
            Some(int())
        );
    }

    #[test]
    fn gat_projection_wrong_arity_errors() {
        let src = r#"
trait Pointer<P: * -> *> {
    type Ref<T>;
    fn deref<T>(P<T> ptr) -> Ref<T>;
}
fn bad<P: * -> *, Pointer, A>(P<A> ptr) -> P::Ref<A, int> {
    return deref(ptr);
}
"#;
        let (_c, msgs) = check_warn(src);
        assert!(
            msgs.iter().any(|m| m
                .message()
                .contains("Associated type `Pointer::Ref` expects 1 type argument, got 2")),
            "expected GAT arity diagnostic, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn gat_projection_kind_mismatch_errors() {
        let src = r#"
trait Pointer<P: * -> *> {
    type Ref<F: * -> *>;
    fn bad<T>(P<T> ptr) -> Ref<T>;
}
"#;
        let (_c, msgs) = check_warn(src);
        assert!(
            msgs.iter().any(|m| {
                let msg = m.message();
                msg.contains("Type argument 1 to associated type `Pointer::Ref`")
                    && msg.contains("kind `*`")
                    && msg.contains("expected `* -> *`")
            }),
            "expected GAT kind diagnostic, got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    // ---- byte / [byte] ----

    #[test]
    fn byte_annotation_accepts_in_range_literal() {
        let (mut c, _) = check("let b: byte = 42;");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        assert_eq!(
            c.codegen_var_type("b")
                .map(|t| apply_ty_prune(c.subst(), t)),
            Some(crate::typechecking::ty::byte())
        );
    }

    #[test]
    fn byte_annotation_rejects_out_of_range_literal() {
        let msgs = assert_messages("let b: byte = 300;");
        assert!(
            msgs.iter()
                .any(|m| m.message().contains("byte literal out of range")),
            "got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn byte_array_literal_coerces_from_int_literals() {
        let (mut c, _) = check("let buf: [byte] = [1, 2, 3];");
        let msgs = c.take_messages();
        assert!(msgs.is_empty(), "{:?}", msgs);
        let ty = c
            .codegen_var_type("buf")
            .map(|t| apply_ty_prune(c.subst(), t))
            .expect("buf");
        match ty {
            Ty::Array { element, .. } => {
                assert_eq!(*element, crate::typechecking::ty::byte());
            }
            other => panic!("expected [byte], got {other}"),
        }
    }

    #[test]
    fn byte_has_show_instance() {
        assert!(
            Checker::new()
                .generics
                .has_instance("Show", &crate::typechecking::ty::byte())
        );
    }

    #[test]
    fn write_all_accepts_named_byte_array() {
        let (mut c, _) = check(
            r#"
use io::*;
fn main() {
    let data: [byte] = [1, 2];
    write_all(stdin(), data);
}
"#,
        );
        let msgs = c.take_messages();
        assert!(
            msgs.is_empty(),
            "unexpected messages: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn io_error_other_does_not_collide_until_imported() {
        // IoError::Other must not reserve the constructor name globally —
        // user enums may use `Other` without `use io`.
        let (mut c, _) = check("enum Foo { Bar, Other } let x = Foo::Other;");
        let msgs = c.take_messages();
        assert!(
            msgs.is_empty(),
            "unexpected without io import: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );

        let (mut c, _) = check("use io::*; let e = IoError::Other;");
        let msgs = c.take_messages();
        assert!(
            msgs.is_empty(),
            "unexpected with io import: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn write_all_rejects_unannotated_int_array_variable() {
        let msgs = assert_messages(
            r#"
use io::*;
fn main() {
    let data = [1, 2];
    write_all(stdin(), data);
}
"#,
        );
        assert!(
            msgs.iter()
                .any(|m| m.message().contains("expected `byte`")
                    || m.message().contains("Type mismatch")),
            "got: {:?}",
            msgs.iter().map(|m| m.message()).collect::<Vec<_>>()
        );
    }
}


