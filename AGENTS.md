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

### Test counts (15B + post-review fixes final)

| Suite | Count |
|-------|-------|
| `compiler/src/typechecking/*` (unit) | 239 |
| `compiler/tests/diagnostics.rs` (golden integration) | 24 |
| `compiler/src/lib.rs::tests` (end-to-end pipeline) | included above |
| `compiler/src/pipeline.rs::tests` (ariadne integration) | included above |
| `common` | 2 |
| `parser` | 9 |
| doctests | 6 |
| **Total** | **282** |

15B landed at 278 tests. Post-review (Fixes 1, 2, 4 in this
phase) added 4 regression tests: one for the ID-alignment
regression in `infer_enum_decl`, one for pattern-error span
anchoring, and two for the `%z` bool format specifier (positive
and negative).

### Build status (15B)

`cargo build` produces only the three pre-existing parser warnings
(no new compiler warnings). The compiler crate is warning-free.

## PHASE 15C - VM AND CODEGEN FOR SUM TYPES AND MATCH (COMPLETED)

### Summary

Wired sum types and pattern matching end-to-end: appended three new
VM opcodes (`MakeEnum`, `JumpIfMatch`, `Unpack`), implemented their
runtime semantics, replaced the 15A codegen stubs with a real
threaded-code match emitter, and verified end-to-end with
`examples/option.0s` (which prints `42`).

### New opcodes (appended, not inserted)

- `MakeEnum` — packs `tag` in upper 16 operand bits and `arity` in
  lower 16. Pops arity values (top of stack = `payload[0]` in source
  order because codegen emits args in reverse) and allocates an
  `ObjEnum` on the heap. Each popped `Value` is classified as an
  immediate (int/float/bool) or a heap pointer (string/instance/
  enum) via `Heap::contains_addr`, producing `Member::Value` or
  `Member::Object` accordingly so the GC traces correctly.
- `JumpIfMatch` — packs `expected_tag` in upper 16 and `target_offset`
  in lower 16. Peeks the scrutinee; on match, pops it, pushes the
  payload in declaration order, and seeks the bytecode iterator to
  the target. On miss, falls through with the scrutinee still on the
  stack.
- `Unpack` — operand carries the arity (redundant with
  `ObjEnum::payload.len()` but kept for symmetry). Pops the scrutinee
  and pushes its payload values in declaration order. Used by the
  last constructor arm of a `match` (reached by fall-through).

### Match codegen layout (canonical "threaded code")

The compiled bytecode for `match x { A => a, B => b, C => c }`:

```
<scrutinee bytecode>
JUMP_IF_MATCH tag_A target_body_A
JUMP_IF_MATCH tag_B target_body_B
UNPACK arity_C
body_c            <- reached by fall-through
JMP end            <- skipped after body_c
body_b            <- reached via JUMP_IF_MATCH B
JMP end            <- skipped after body_b
body_a            <- reached via JUMP_IF_MATCH A
<- match end here
```

Arms are emitted in reverse source order so the bytecode grows
downward; the LAST arm is reached by fall-through and is the first
body in the bytecode, each non-last arm terminates with a `JMP end`
to skip past the bodies placed earlier. Placeholders are patched in
a second pass after the arm-body offsets are known.

### Decisions locked in (during implementation)

1. **Append-only opcode additions.** New opcodes are APPENDED to
   the end of the `Instruction` enum to preserve every existing
   `#[repr(u8)]` discriminant. Inserting before `SET` would shift
   every later opcode's numeric value and silently corrupt every
   `.0s` archive ever compiled.
2. **Stack discipline: reverse-emit, no-reverse-pop.** Codegen emits
   payload args in reverse declaration order so the stack top holds
   `args[0]`. `MakeEnum` pops arity values top-first — first pop is
   `args[0]`, last pop is `args[arity-1]` — and the resulting buffer
   is already in declaration order without an explicit reversal.
3. **`find_object_by_addr` is a free function, not a `&self` method.**
   The borrow checker splits the `&Heap` borrow from other
   in-flight borrows on `Machine` fields (specifically the mutable
   `frames` borrow held by the `execute` loop).
4. **Heap-pointer classification by `Heap::contains_addr`.** O(n)
   walk over the intrusive linked list — acceptable because
   `MakeEnum` is only emitted at constructor call sites and the
   heap is typically small. A generation table or per-frame
   pointer map would make this O(1); deferred to 15D+.
5. **`Member::Object` carries a full `Object`, not a raw pointer.**
   The `Object` is reconstructed by walking the intrusive list to
   find the entry whose address matches the popped `Value`. If the
   lookup fails (object already freed?), the payload falls back to
   `Member::Value` (defensive — the GC will skip it).
6. **Print/Format emit directly to `self.bytecode`, not a local
   `Vec`.** Any nested match in the params list computes absolute
   jump targets in `self.bytecode`; the old "return a Vec" pattern
   would silently miscompile (the placeholder patch would write to
   the wrong buffer).
7. **`emit_pattern_binding` is a free function** with the
   `&mut Interner<String>` and `&mut Vec<Byte>` parameters split
   out, so the borrow checker doesn't see `self.context.variables`
   and `self.bytecode` as simultaneously borrowed inside the helper.
8. **`Expression::Type` consumes an ID but emits empty bytecode.**
   The pre-walk mints a `NodeId` for each `Expression::Type`
   wrapper inside enum payload lists so the HM typechecker's ID
   table stays aligned; the codegen arm is a no-op.
9. **`JumpIfMatch` falls through on a non-enum scrutinee** (e.g.,
   if a type error slipped through typechecking). The arm is
   unreachable but the VM doesn't crash — same defensive posture
   as `Unpack`.
10. **No new `Instruction::Pattern` arm.** Patterns are not
    standalone expressions; they only appear as `arm.pattern` and
    are handled by `emit_pattern_binding` directly. The pre-walk's
    `pre_walk_pattern` correctly doesn't mint IDs for them.

### Diagnostics produced

The codegen and VM are silent on type errors (the typechecker, 15B,
already produced those diagnostics upstream). The new VM arm for
non-enum scrutinees is defensive: it falls through silently rather
than panicking, on the principle that the typechecker is the source
of truth.

### Test counts (15C final)

| Suite | Count | Delta vs 15B |
|-------|-------|--------------|
| `compiler/src/typechecking/*` (unit) | 231 | 0 |
| `compiler/src/lib.rs::tests` (codegen + e2e) | 9 | +3 |
| `compiler/src/pipeline.rs::tests` (ariadne) | 2 | 0 |
| `compiler/tests/diagnostics.rs` (golden integration) | 24 | 0 |
| `common` | 2 | 0 |
| `machine` | 8 | +6 |
| `parser` | 9 | 0 |
| doctests | 6 | 0 |
| **Total** | **291** | **+9** |

The 9-test delta is exactly the 6 new VM tests + 3 new codegen
tests specified in 15C.5 and 15C.6:

VM tests (`machine/src/vm.rs::tests`):
1. `make_enum_allocates_enum_with_correct_tag`
2. `make_enum_with_payload_populates_payload`
3. `jump_if_match_taken_advances_ip`
4. `jump_if_match_not_taken_falls_through`
5. `unpack_pops_enum_and_pushes_payload`
6. `nested_enum_gc_traces_correctly`

Codegen tests (`compiler/src/lib.rs::tests`):
1. `construct_emits_make_enum_with_correct_tag_and_arity`
2. `match_emits_jump_if_match_cascade`
3. `wildcard_match_arm_emits_pop`

### End-to-end smoke test

`examples/option.0s` compiles and runs correctly, printing `42`:

```0s
enum Option {
    None,
    Some(int),
}

fn unwrap(Option o) -> int {
    return match o {
        Option::None => 0,
        Option::Some(v) => v,
    };
}

fn main() {
    print "%i", unwrap(Option::Some(42));
}
```

### Files modified

| File | Net change | Purpose |
|------|-----------|---------|
| `common/src/opcode.rs` | +21 LOC | Append 3 new opcodes |
| `machine/src/vm.rs` | +575 LOC (net) | Dispatch arms + 6 tests + restoration of commented tests |
| `machine/src/memory/heap.rs` | +36 LOC | `head_for_lookup` + `contains_addr` helpers |
| `compiler/src/lib.rs` | +556 LOC (net) | Real codegen for `EnumDecl`/`Variant`/`Construct`/`Match`/`Type` + `Print`/`Format` refactor + `emit_pattern_binding` + 3 codegen tests |
| `examples/option.0s` | new | End-to-end smoke test |
| `AGENTS.md` | this section | Documentation |

### Build status (15C)

`cargo build` produces only the three pre-existing parser warnings
(no new compiler or machine warnings). Both the compiler and
machine crates are warning-free for the new work.

### Anything 15D needs to know

- The heap-pointer-classification in `MakeEnum` is O(n) in the
  number of live heap objects. A generation table or per-frame
  pointer map would let `MakeEnum` and `Unpack` classify in O(1).
- `Object::mark_references` for `Object::Enum` already traces its
  `payload` (15A work), but the VM's automatic `trace`/`sweep`
  cycle is NOT yet wired into `Machine::execute`. 15D's first task
  should be to call `heap.trace(roots)` + `heap.sweep()` at
  allocation pressure points (or after every N allocations).
- The `Match` codegen handles `Wildcard`, `Binding`, and nested
  `Constructor` patterns. Nested constructor patterns work via
  `emit_pattern_binding` which emits `UNPACK` + recurses. Nested
  patterns have no dedicated test yet — consider adding one
  alongside the first 15D `mark_references` test.
- `Expression::Pattern` does not exist as a standalone variant;
  patterns only appear via `arm.pattern`. The `_expr =>` catch-all
  in `do_compile` already produces an "Unknown expression"
  diagnostic for any new variant — useful as a safety net if 15D
  introduces new pattern shapes.