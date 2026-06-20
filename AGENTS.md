# zero-script — AGENTS

## PHASE 14 - HINDLEY–MILNER TYPECHECKER (COMPLETED)

### Summary

Replaced the hand-written structural typechecker at
`compiler/src/typechecker.rs` with a proper Hindley–Milner (Algorithm
W) inference pass. The new checker runs once per program after
parsing, produces a `Ty` for every expression, surfaces source-anchored
diagnostics via the existing `common::Message` / ariadne pipeline, and
exposes a `(NodeId) → Ty` cache to the bytecode emitter so codegen can
keep picking `ADD` vs `ADDF` without re-inferring.

Detailed design lives in [`HM_TYPECHECKER_PLAN.md`](./HM_TYPECHECKER_PLAN.md).

### Layout

```
compiler/src/typechecking/
    mod.rs        # public API: Checker, Ty
    arena.rs      # (folded into ty.rs — `typed_arena` was not needed)
    ty.rs         # Ty, Scheme, TyVarId, builtin constructors
    subst.rs      # Substitution maps, apply, compose, ftv
    unify.rs      # Robinson unification + occurs check
    env.rs        # Scoped environment, let-polymorphism
    infer.rs      # Algorithm W over the AST (phases 4–9)
    error.rs      # (folded into infer.rs — TypeError helpers)
    pretty.rs     # Ty → String for diagnostics
    id.rs         # Pre-walk, NodeId, IdTable
```

### Public API

```rust
pub use pipeline::*;
pub use typechecking::{Checker, Ty};
```

`Checker` exposes:

- `new()` — fresh checker with one top frame.
- `check_program(&ast) -> Ty` — run inference over a parsed AST.
- `register_native(name, params, ret)` — wire up a built-in.
- `lookup_at(NodeId) -> Option<Ty>` — span-indexed cache for codegen.
- `id_table()` — exposes the pre-walk-minted IDs in visit order.
- `take_messages()` / `messages()` — diagnostic drain.
- `env()` / `env_mut()` — inspection of declared bindings.

### Decisions locked in (during implementation)

1. **Non-recursive `apply_ty`.** A `apply_ty_prune` helper resolves
   chains fully for diagnostics. Without this, `compose` cannot be a
   faithful set-of-pairs representation.
2. **`Checker` owns the running substitution.** `infer` mutates
   `self.subst` and returns just the `Ty`. No threading `(Subst, Ty)`
   tuples up the call stack.
3. **Pre-walk `NodeId` minting.** Both `infer` and the pre-walk
   visit in pre-order, so the `n`-th call consumes the `n`-th ID.
   Wrapper siblings that share a span get distinct IDs.
4. **`check_program` keeps the top frame.** Callers (and tests) can
   inspect declared bindings afterwards.
5. **Error recovery.** Every error reports a `Message` and returns a
   fresh `TyVarId`. Inference continues past errors so users see every
   problem in one pass.
6. **Monomorphic recursion for `fn`.** Allocate `α_f`, insert
   `name : α_f -> ... -> ρ` in the outer frame, infer body, then unify
   the actual return with `ρ`. Decidable, handles `fn fib(...) { fib(...) }`.
7. **`self` is implicit in `impl` methods.** Bound to the owner class
   inside the method body; not user-shadable. Receiver prepended to
   the curried argument type.
8. **Per-visit-order IDs.** Originally `IdTable` was span-keyed; in
   practice wrapper nodes share spans, so it became a `Vec<NodeId>`
   filled in pre-order.
9. **Legacy typechecker deleted.** `compiler/src/typechecker.rs` is
   gone; `Compiler::typechecker: Typechecker` field is gone;
   `Compiler::register` delegates to the HM checker only.

### Diagnostics produced

| Site | Message format |
|------|----------------|
| Unknown identifier | `Cannot find value \`x\` in this scope` |
| Type mismatch (unify) | `Type mismatch: expected \`int\`, found \`string\`` + help |
| Infinite type (occurs) | `Cannot construct infinite type \`T\`` + help |
| Not a function / arity | `Function \`foo\` was called with too many arguments` or `Cannot call value of type \`T\`` |
| Unknown function | `Cannot find function \`foo\`` |
| Assignment to undeclared | `Cannot assign to undeclared variable \`x\`` + help |
| Invalid assignment / constant target | Same + help explaining the LHS shape |

### Test counts (final)

| Suite | Count |
|-------|-------|
| `compiler/src/typechecking/*` (unit) | 198 |
| `compiler/tests/diagnostics.rs` (golden integration) | 15 |
| `compiler/src/lib.rs::tests` (end-to-end pipeline) | included above |
| `compiler/src/pipeline.rs::tests` (ariadne integration) | included above |
| `common` | 2 |
| `parser` | 3 |
| doctests | 6 |
| **Total** | **224** |

### Files added / removed

**Added:**
- `compiler/src/typechecking/mod.rs` (~50 LOC)
- `compiler/src/typechecking/ty.rs` (~250 LOC)
- `compiler/src/typechecking/subst.rs` (~470 LOC)
- `compiler/src/typechecking/unify.rs` (~400 LOC)
- `compiler/src/typechecking/env.rs` (~610 LOC)
- `compiler/src/typechecking/infer.rs` (~2000 LOC)
- `compiler/src/typechecking/id.rs` (~310 LOC)
- `compiler/src/typechecking/pretty.rs` (~130 LOC)
- `compiler/tests/diagnostics.rs` (15 tests)
- `HM_TYPECHECKER_PLAN.md`
- `examples/fib.0s` (tweaked from `fib(32)` to `fib(7)`)

**Removed:**
- `compiler/src/typechecker.rs` (~600 LOC)
- `Compiler::typechecker: Typechecker` field
- `Compiler::typecheck` stub method

### Build status

`cargo build` produces only three pre-existing parser warnings
(`None`/`Xor`/`Equal`/`Unary`/`Call` variants unused, `prefix`
field unused, `inc`/`dec` methods unused) — all in `parser/src/lib.rs`
and outside the scope of this work. The compiler crate is warning-free
for the typechecking pass.

## PHASE 15B - HM TYPECHECKER FOR SUM TYPES AND MATCH (COMPLETED)

### Summary

Extended the HM typechecker to recognize and validate sum types
(`enum`), qualified constructor applications (`Enum::Variant(args)`),
and `match` expressions. The forward-declaration pre-pass walks the
AST once to collect every `enum` declaration's shape (variant
names, arities, payload types) before the main inference pass, so
constructors and matches that appear textually before their enum
declaration still resolve correctly.

Detailed design lives in
[`HM_TYPECHECKER_PLAN.md`](./HM_TYPECHECKER_PLAN.md) and the
Phase 15B plan appended to that document.

### Public API additions

`Checker` now also exposes:

- `tag_for(&str, &str) -> Option<u32>` — variant tag by enum and
  variant name (source-declaration order; see MUST-HAVE #2).
- `arity_for(&str, &str) -> Option<usize>` — payload arity, cached
  at registration.
- `enum_variants(&str) -> Option<Vec<(String, u32, Vec<Ty>)>>` —
  full variant list for codegen and exhaustiveness tools.

### Decisions locked in (during implementation)

1. **Isorecursive encoding for `Ty::Sum` payloads.** Recursive
   enum payloads use `Ty::Con(name)` (opaque) rather than the
   unfolded `Ty::Sum(...)`. The HM occurs check would otherwise
   reject recursive enums (see `unify.rs:bind_var`'s occurs check).
2. **Dual data structure for variant tags.** `Vec<String>` holds
   the source-declaration order; `BTreeMap<String, u32>` indexes
   by variant name. A pure `BTreeMap` would silently miscompile
   (alphabetical iteration).
3. **Pre-pass + main-pass two-phase inference.** A
   `pre_register_enums` walker collects enum shapes before the
   main infer pass so forward references resolve correctly. The
   pre-pass parses payload types directly from
   `Expression::Type(name)` AST nodes (no ID consumption).
4. **Pattern returns the expected type.** A pattern's type is
   the scrutinee's type (the pattern desugars the value); the
   tag is captured separately for exhaustiveness checking.
5. **Constructor vs Constructor unification.** Added an arm so
   pattern-vs-scrutinee unification succeeds when both sides are
   `Constructor` with the same tag.
6. **`Con(name)` vs `Sum{name,..}` unification.** A
   `Ty::Con(name)` from a recursive reference unifies with the
   matching `Sum` or `Constructor` of that name. Required for
   cross-enum payload types.
7. **Deferred exhaustiveness check.** `infer_match` records a
   `PendingExhaustive` per match site; the post-pass (after the
   main infer returns) re-resolves the scrutinee under the
   closed substitution and emits non-exhaustive / unreachable-arm
   diagnostics. This means the scrutinee's type variables are
   fully resolved by the time the check runs.
8. **Pattern bindings don't leak.** Each match arm gets a fresh
   env frame (push / pop around the arm's pattern + body).
9. **Format-string typecheck.** `print` and `format` now
   validate every `%X` specifier against the corresponding
   argument's type. Supported: `%i`/`%d`/`%b`/`%x`/`%u`/`%p` (int),
   `%f` (float), `%s` (string), `%z` (bool), `%%` (literal).
10. **GC work already landed in 15A.** `Object::Enum` is in the
    `Object` enum with all 9 method arms updated. No additional
    GC work needed in 15B.

### Diagnostics produced (15B)

| Site | Message |
|------|---------|
| Duplicate enum | `Duplicate enum \`X\`` + help |
| Cross-enum variant collision | `Duplicate constructor \`X\` (also declared by enum \`Y\`)` + help |
| Unknown enum in constructor call | `Cannot find enum \`X\` in this scope` |
| Unknown variant | `Cannot find variant \`Y\` on enum \`X\`` |
| Constructor arity mismatch | `Constructor \`X::Y\` expects N arguments, got M` |
| Unknown constructor in pattern | `Pattern references unknown constructor \`X::Y\`` |
| Non-exhaustive match | `Non-exhaustive match: variants not covered: \`A\`, \`B\`` + help |
| Unreachable arm | `Unreachable arm: this pattern is matched by an earlier arm` |
| Format specifier mismatch | `Format specifier \`%s\` requires string, found int` + help |

### Test counts (15B final)

| Suite | Count |
|-------|-------|
| `compiler/src/typechecking/*` (unit) | 237 |
| `compiler/tests/diagnostics.rs` (golden integration) | 22 |
| `compiler/src/lib.rs::tests` (end-to-end pipeline) | included above |
| `compiler/src/pipeline.rs::tests` (ariadne integration) | included above |
| `common` | 2 |
| `parser` | 3 |
| doctests | 6 |
| **Total** | **278** |

### Build status (15B)

`cargo build` produces only the three pre-existing parser warnings
(no new compiler warnings). The compiler crate is warning-free.