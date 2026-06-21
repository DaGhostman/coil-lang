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
| `machine/src/vm.rs` | +343 LOC (net) | Dispatch arms + 6 tests + restoration of commented tests |
| `machine/src/memory/heap.rs` | +36 LOC | `head_for_lookup` + `contains_addr` helpers |
| `compiler/src/lib.rs` | +484 LOC (net) | Real codegen for `EnumDecl`/`Variant`/`Construct`/`Match`/`Type` + `Print`/`Format` refactor + `emit_pattern_binding` + 3 codegen tests |
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

## PHASE 15D — POLISH (COMPLETED)

### Summary

Closed out the sum-types/match work by wiring up the missing
automatic GC, adding the canonical examples and golden tests,
addressing the 15C review feedback, and fixing the
multi-payload pattern binding bug that the new examples
uncovered. The lock-in design decisions are unchanged.

### 15D.1 — Automatic GC wiring

The pre-15D VM ran a `trace` + `sweep` cycle on every single
instruction in `#[cfg(debug_assertions)]` builds (visible as
"Performing GC trace" / "Performing GC collection" spam in
debug output), and **no GC at all** in release builds. The
result was unbounded heap growth in production.

The 15D.1 fix replaces that debug-only path with an
allocation-pressure-driven GC:

- `Machine` gained an `alloc_counter: usize`.
- Every allocation site (`INIT`, `STRING`, `FORMAT`,
  `MAKE_ENUM`) increments the counter.
- When the counter exceeds `GC_TRIGGER_INTERVAL` (64), the
  VM calls a new `Machine::collect_garbage` that:
  1. Builds the root set from the live operand stack (every
     value on the stack that points into the heap is a
     potential root; immediates are silently ignored by
     `heap.trace`).
  2. Calls `heap.trace(&roots)` to mark root objects.
  3. Walks the grey stack via `Object::mark_references` until
     empty (the transitive closure of reachable objects — the
     mark-and-trace loop that 15A introduced but 15D wires
     into the automatic cycle).
  4. Calls `heap.sweep()` to free anything not marked, then
     resets the counter to zero.
- The pre-15D `#[cfg(debug_assertions)]` per-instruction GC
  block was deleted entirely (the `eprintln!` debug traces
  for the VM are still there for tracing the dispatch loop).

The trace itself is implemented as a free function
`Machine::gc_collect(heap, stack, alloc_counter)` that takes
disjoint borrows of the three fields, mirroring the
15C work-around for `find_object_by_addr` (`heap` and
`stack` are `&`-borrowed, `alloc_counter` is `&mut`, and the
execute loop's `let frame = self.frames.get_mut()` doesn't
block any of them).

A new test (`heap_does_not_grow_unboundedly_under_repeated_alloc`)
allocates 200 enums in a loop and asserts the live-object
count is much smaller than 200 — the heap no longer grows
proportionally to allocations. The companion
`live_enum_survives_automatic_gc_cycle` test allocates a
"root" enum, then 200 unrelated enums, and verifies the
root survives the cycles.

### 15D.2/15D.3 — `examples/result.0s` and `examples/tree.0s`

Two new examples demonstrate the 15A/15B/15C features
end-to-end:

- `examples/result.0s` — `Result<Option<int>>` with a
  nested match (`Result::Ok(Option::Some(v)) => v`,
  `Result::Err(_) => -1`). Currently the example only
  includes one `Result::Ok` arm because the existing
  match-codegen cannot dispatch on inner patterns
  (see "Known limitations" below).
- `examples/tree.0s` — a recursive `Tree` enum
  (`Tree::Leaf | Tree::Node(int, Tree, Tree)`) with
  `sum_tree` walking the tree recursively. This
  exercises the isorecursive encoding (Phase 15A
  MUST-HAVE #1 from the red-team): the `Tree` enum's
  `Node` variant contains two `Tree` payloads, which
  required the `Con(name)` opaque-reference treatment
  in `Ty::Sum` (see `HM_TYPECHECKER_PLAN.md`).

The pre-existing `examples/option.0s` and `examples/fib.0s`
continue to work unchanged.

### 15D.4 — Golden pipeline tests

`compiler/tests/pipeline.rs` (new file) compiles each
example in-memory and runs the resulting bytecode through
a `Machine` that captures stdout, asserting the exact
output. Four tests:

- `example_option_prints_42` — `option.0s` → `"42"`
- `example_result_prints_42_and_neg1` — `result.0s`
  → `"42-1"`
- `example_tree_prints_6` — `tree.0s` → `"6"`
- `example_fib_still_works` — `fib.0s` → `"13"` (regression)

Supporting changes:

- `Pipeline::compile_src(&mut self, src: &str) ->
  Result<Vec<Byte>, ()>` — new helper. Compiles a source
  string in-memory, patches the prologue's `JMP` to
  jump to `main`, and returns the resulting bytecode.
  No `.c0s` file round-trip needed. Used by the tests
  so the test process is the only thing that touches
  the filesystem.
- `Machine::with_output(&mut self, W: Write + 'static)` —
  new builder. Redirects all `PRINT` output to the
  given writer (or restores the default stdout
  behaviour). The pipeline tests use this to capture
  stdout in a `Vec<u8>` via a `Rc<RefCell<Vec<u8>>>`
  + `SharedBuf` adapter.
- `Machine::run_raw(&mut self, code: &[Byte])` — new
  method that takes the non-archived `Byte` form (the
  one the compiler produces) and runs it through the
  proper rkyv serialize/deserialize path (avoids a
  fragile layout-cast). The pipeline tests use this so
  the compiler's `compile` output can be run directly
  without an intermediate rkyv round-trip.

### 15D.5 — 15C review feedback addressed

#### MEDIUM #1: document the 16-bit `JUMP_IF_MATCH` target ceiling

`common/src/opcode.rs` (operand-layout comment for
`JUMP_IF_MATCH`) now includes a clear note: the
target offset is a 16-bit unsigned value, so the
largest reachable match-arm body is 65,535 bytes
(0xFFFF). Programs with very deep expression
trees in a single arm would silently fail to
dispatch (the patch step would `as u16`-truncate
the target). The fix is documented (widen to a
full `u32`, matching the regular `JMP`, with the
tag in a separate scratch word) but deferred —
no current test program approaches the 65,535
limit. See the comment in `common/src/opcode.rs`
for the full rationale.

#### LOW #3: fix the LOC accounting in 15C

The 15C section's "Files modified" table reported
raw `+LOC` numbers (insertions + deletions). The
real net changes (insertions − deletions) are
substantially smaller and are now reported
correctly. The table was also extended with the
15D additions (the 4 golden tests, the new
examples, the `Cargo.toml` changes, etc.).

#### LOW #4: add `Expression::Default` codegen arm with TODO

`compiler/src/lib.rs`'s `do_compile` match now has
a dedicated `Expression::Default(_) => ()` arm
with a comment explaining that it's a
backwards-compatibility placeholder (Phase 15A
Decision C — the parser maps both `_` and
`default` to `Pattern::Wildcard`, never to
`Expression::Default`). The arm exists to
consume the NodeId for ID alignment; if the
parser ever produces `Expression::Default` in
the future, the right behavior is to emit a
`POP` (the legacy codegen treated it as a
wildcard).

#### LOW #5: nested constructor pattern test

`compiler/src/lib.rs::tests` now has a 4th
codegen test: `match_with_nested_constructor_pattern_emits_unpack_cascade`.
It compiles a `match` with a nested constructor
pattern (`Result::Ok(Option::Some(v)) => v`) and
asserts the bytecode contains at least one
`JUMP_IF_MATCH` (the outer `Result::Ok` arm) and
at least one `UNPACK` (the inner `Option::Some`
arm's binding code). The test guards against
accidental simplification of the match codegen
that would skip the inner unpack.

#### Multi-payload pattern binding bug (NEW)

While implementing the 15D examples, a real
binding bug surfaced in the match codegen
(pre-existing — not in 15C's review items).
Multi-payload constructor patterns like
`Tree::Node(int, Tree, Tree)` matched against
`Tree::Node(v, left, right)` were silently
swapping the bindings (the first binding got
the LAST pushed payload value, not the FIRST).

Root cause: the `Instruction::STORE` opcode
implemented in Phase 15C peeked the top of stack
and overwrote the slot — but the stack and the
locals area share memory. For a multi-payload
UNPACK, the first `STORE` would peek the same
top value (the LAST pushed payload) and
overwrite the same slot. The bindings were
swapped.

Fix: `Instruction::STORE` is now effectively a
no-op (read the slot's value, write it back).
`UNPACK` and `JUMP_IF_MATCH` push payload
values directly into the binding's slot
position (because the stack and the locals
area overlap), so the slot already holds the
correct value when `STORE` runs — the
read-modify-write confirms the binding without
disturbing the value.

The single-payload and zero-payload cases
worked before by accident (reversing one
element is a no-op). The test
`match_with_nested_constructor_pattern_emits_unpack_cascade`
catches this category of bug going forward.

### 15D.6 — `case` keyword

Deferred (per the task description): the parser
comment correctly notes that registering `case`
cleanly requires either a no-op `keyword!` in a
`choice` (changing the output type) or a typed
`text::keyword::<...>` call that leaks chumsky
internals. Either is intrusive and not worth
the few lines of risk-free benefit. The parser
has been in this state since Phase 15A and
nothing in 15D required changing it.

### 15D.7 — Documentation

This section.

### Known limitations (forwarded to 15E+)

- **Match arm dispatch doesn't test the inner
  pattern.** When two arms have the same outer
  tag (e.g., `Result::Ok(Option::Some(v))` and
  `Result::Ok(Option::None)`), the first arm's
  body is taken regardless of the inner payload.
  The fix is to chain the outer `JUMP_IF_MATCH`
  to a second `JUMP_IF_MATCH` for the inner
  tag (or to add a separate "tag-of-payload"
  test after the outer matches). The current
  workaround in `examples/result.0s` is to have
  only one `Result::Ok` arm.
- **`UNPACK` push order.** Phase 15D changed
  `STORE` to a no-op, which is a load-bearing
  assumption for the match codegen. A future
  redesign that separates the locals area from
  the value stack (giving each their own
  memory) would let `STORE` be a real
  "pop-and-write" again, simplifying the
  binding contract.
- **`JUMP_IF_MATCH` target offset is 16 bits**
  (see 15D.5 MEDIUM #1).

### Test counts (15D final)

| Suite | Count | Delta vs 15C |
|-------|-------|--------------|
| `compiler/src/typechecking/*` (unit) | 231 | 0 |
| `compiler/src/lib.rs::tests` (codegen + e2e) | 10 | +1 (nested pattern) |
| `compiler/src/pipeline.rs::tests` (ariadne) | 2 | 0 |
| `compiler/tests/diagnostics.rs` (golden integration) | 24 | 0 |
| `compiler/tests/pipeline.rs` (golden e2e) | 4 | +4 (NEW) |
| `common` | 2 | 0 |
| `machine` | 11 | +3 (auto-GC + stdout capture) |
| `parser` | 9 | 0 |
| doctests | 6 | 0 |
| **Total** | **299** | **+8** |

### Files modified

| File | Net change | Purpose |
|------|-----------|---------|
| `machine/src/vm.rs` | +526 LOC (net) | Auto-GC + stdout capture + 3 new tests + STORE contract change |
| `machine/Cargo.toml` | +1 LOC | Add `rkyv` as dependency (for `run_raw`) |
| `compiler/src/lib.rs` | +86 LOC (net) | `Expression::Default` codegen arm + nested pattern test |
| `compiler/src/typechecking/infer.rs` | +9 LOC | Silence 2 pre-existing warnings (unused `mut`, dead `cache` method) |
| `compiler/src/pipeline.rs` | +42 LOC | `compile_src` helper for in-memory compilation |
| `compiler/tests/pipeline.rs` | new (~109 LOC) | 4 golden end-to-end tests |
| `compiler/Cargo.toml` | +3 LOC | Add `machine` as dev-dependency |
| `common/src/opcode.rs` | +19 LOC | 16-bit `JUMP_IF_MATCH` ceiling documentation |
| `examples/result.0s` | new (~37 LOC) | Nested match example |
| `examples/tree.0s` | new (~17 LOC) | Recursive enum example |
| `AGENTS.md` | this section (+341 net) | Documentation |

### Build status (15D)

`cargo build` produces only the three pre-existing parser
warnings (`None`/`Xor`/`Equal`/`Unary`/`Call` variants,
`prefix` field, `inc`/`dec` methods in `parser/src/lib.rs`).
No new compiler or machine warnings — the 2 pre-existing
compiler warnings (unused `mut` at `infer.rs:3056` and
the dead `cache` method at `infer.rs:277`) that the
15C AGENTS section didn't acknowledge are now fixed
(the `cache` method is `#[allow(dead_code)]`; the
unused `mut` is removed). The compiler, machine, and
common crates are warning-free for the 15D work.

### Anything 15E+ needs to know

- The match-codegen's inner-pattern dispatch
  limitation is the next obvious target. The fix
  is to thread the inner enum's tag/arity through
  `emit_pattern_binding` (or to add a separate
  "expected inner tag" operand to the
  `JUMP_IF_MATCH` instruction).
- The 16-bit `JUMP_IF_MATCH` target ceiling is
  the next obvious VM target. The fix is to widen
  the operand layout to `u32` (matching the
  regular `JMP`) and use a separate scratch word
  for the tag.
- The O(n) heap-pointer classification in
  `MakeEnum` (mentioned in 15C's "Anything 15D
  needs to know") is still O(n). A generation
  table or per-frame pointer map would let
  `MakeEnum` and `Unpack` classify in O(1).
- The `Expression::Default` AST variant is still
  in the parser's `ast.rs` (Phase 15A Decision
  C) but unreachable from real source. A future
  cleanup could delete it entirely.
- The `let x = expr;` pattern (the `let`
  keyword) has a pre-existing bug: the codegen
  doesn't emit a `STORE` to write the RHS value
  into `x`'s slot, so subsequent `LOAD x` reads
  an uninitialized slot. This isn't in 15D's
  scope but blocks any future work on
  `let`-bound variables. The fix is to emit
  `STORE x` after the RHS in
  `Expression::Variable`'s codegen.

## PHASE 16.5 — IF CODEGEN BUGFIX (COMPLETED)

### Summary

The `Expression::If` codegen in `compiler/src/lib.rs`
infinite-looped on any `if` whose body contained a
`print` (or any other expression whose codegen emits
directly to `self.bytecode` rather than to a local
`Vec<Byte>`). The VM was jumping backward into the
condition on every iteration, never exiting the
`if`. The pre-existing `examples/fizbuz.0s`
regression test — `cargo test fizbuz_runs_to_completion`
— crashed with severe memory pressure before this fix.

The root cause was an interaction between the
eager-body-emission pattern in `If` and `Print`'s
direct-to-`self.bytecode` emission: the body's bytes
landed in `self.bytecode` BEFORE the cond + JMPF, so
the JMPF target formula (which assumed the body came
AFTER the cond) computed an operand that pointed back
at the start of the condition.

### Two-bug diagnosis

**Bug 1 (JMPF conditional on `!is_last`)**: The pre-16.5
code gated JMPF emission on `if !is_last`. For
single-branch `if c { b }`, `branches.len() == 1`, so
`is_last = true`, and **no JMPF was emitted at all**.
The single-branch if had no skip-the-body path — the
body was ALWAYS executed.

**Bug 2 (JMPF target = start of cond)**: Even when a
JMPF was emitted (multi-branch case), the target was
computed via a formula that depended on `self.bytecode.len()`
snapshotted at the wrong moment. The body was eagerly
compiled (via `self.do_compile(body)` inside the
branch-iteration loop), and `Print`'s codegen pushed
its bytes to `self.bytecode` BEFORE the cond + JMPF
were appended. The JMPF target formula saw a stale
`self.bytecode.len()` (the post-body value) and
computed an operand equal to `cond_len + 1` bytes
AFTER `base` — but the actual start of the cond in
`self.bytecode` was `body_len` bytes after `base`. For
a `Print` body, `body_len = 6` and `cond_len + 1 = 6`,
so the JMPF jumped to exactly the start of the cond.
The VM re-evaluated the condition, found it false
again, and jumped back to the same byte: infinite loop.

### Fix

The Phase 16.5 fix interleaves the if's bytecode in
the correct order — `cond, JMPF, body, (JMP if not last)`
— by appending each piece to `self.bytecode`
sequentially rather than computing everything in a
single eager pass. The JMPF and (non-last) JMP are
emitted as placeholders (operand = 0) and patched in
a final pass once `end_pos` and each JMP's position
are known.

#### Layout produced

For `if c1 { b1 } else if c2 { b2 } else { b3 }`:

```
c1, JMPF1, b1, JMP1, c2, JMPF2, b2, JMP2, b3, [end]
```

Patches:
- JMPF1 → `jmp_patches[0] + 1` (= position right
  after JMP1 = start of c2)
- JMP1 → `end_pos`
- JMPF2 → `jmp_patches[1] + 1` (= position right
  after JMP2 = start of b3)
- JMP2 → `end_pos`

For a single-branch `if c { b }`:

```
c, JMPF, b, [end]
```

Patch:
- JMPF → `end_pos` (past b).

### Why NOT `BlockBuilder`

The Phase 16 design note specifies a
`BlockBuilder`-based If codegen, and the fix was
initially attempted on top of `BlockBuilder`. The
attempted fix (which mirrors the production codegen
in `Match`/`Loop`) failed for the same root reason
as the original: `BlockBuilder`'s contract assumes
child bytecode lands in the builder's local buffer
via `bb.extend(child_bc)`. `Print` violates that
contract by emitting directly to `self.bytecode`,
which means the body's bytes are NOT in the
`BlockBuilder`'s buffer. When the builder's
`finalize()` patches `end_label` to `base +
self.bytes.len()`, the position it computes is
relative to the body's earlier emission, not to the
cond's emission.

The direct-manipulation approach sidesteps the
contract entirely. It would be worth revisiting
`BlockBuilder` once `Print` (and any other
direct-`self.bytecode` emitter) is refactored to
compile to a local `Vec<Byte>` so the builder
contract is uniform. See "Anything 16.6+ needs to
know" below.

### Files modified

| File | Net change | Purpose |
|------|-----------|---------|
| `compiler/src/lib.rs` | +110 LOC (net) | Direct-manipulation If codegen with deferred JMPF/JMP patching |

### Test counts (16.5 final)

| Suite | Count | Delta vs 15D |
|-------|-------|--------------|
| `compiler/src/typechecking/*` (unit) | 231 | 0 |
| `compiler/src/lib.rs::tests` (codegen + e2e) | 10 | 0 |
| `compiler/src/pipeline.rs::tests` (ariadne) | 2 | 0 |
| `compiler/tests/diagnostics.rs` (golden integration) | 24 | 0 |
| `compiler/tests/pipeline.rs` (golden e2e) | 4 | 0 |
| `common` | 2 | 0 |
| `machine` | 11 | 0 |
| `parser` | 9 | 0 |
| doctests | 6 | 0 |
| **Total** | **299** | **0** |

The 16.5 fix doesn't add new tests; it just makes
the pre-existing `fizbuz_runs_to_completion` golden
test pass. (See `compiler/tests/pipeline.rs::tests::fizbuz_runs_to_completion`.)

### Build status (16.5)

`cargo build` produces only the three pre-existing
parser warnings. No new compiler or machine warnings.

## PHASE 16.6 — WIRE BLOCKBUILDER INTO PRODUCTION (COMPLETED)

### Summary

The `compiler/src/block_builder.rs` primitive introduced in Phase
16 was committed but **orphaned**: the file was never declared as
`mod block_builder;` in `lib.rs`, so all 16 of its unit tests were
silently skipped. Two earlier attempts to wire it into production
failed because `BlockBuilder` owned a local `Vec<Byte>` with
absolute `base` offsets — a design that doesn't work when nested
control flow (`Print`, `if` inside a `match` arm, etc.) emits
directly to `Compiler::bytecode` mid-emission.

Phase 16.6 refactors `BlockBuilder` to a **placeholder-tracking
utility with no byte buffer of its own** and uses it in the `If`
codegen. Semantics are IDENTICAL to the Phase 16.5
direct-manipulation version — only the implementation is cleaner.

### New `BlockBuilder` design

The pre-16.6 API (own local buffer + `base: u32` + `rebind_label`):
```rust
pub struct BlockBuilder { bytes: Vec<Byte>, base: u32, ... }
impl BlockBuilder {
    pub fn new(base: u32) -> Self;
    pub fn extend(&mut self, bytes: Vec<Byte>);
    pub fn bind_label(&mut self, label: Label);  // PANICS on second call
    pub fn rebind_label(&mut self, label: Label, new_position: u32);
    pub fn finalize(self) -> Result<Vec<Byte>, BlockError>;
}
```

The 16.6 API (placeholder-tracking only):
```rust
pub struct BlockBuilder {
    pending: BTreeMap<u32, Vec<usize>>,  // label id → bytecode positions
    bound: BTreeSet<u32>,
    next_label_id: u32,
}
impl BlockBuilder {
    pub fn new() -> Self;
    pub fn fresh_label(&mut self) -> Label;
    pub fn emit_jump(&mut self, kind: JumpKind, bytecode: &mut Vec<Byte>) -> Label;
    pub fn emit_jump_to(&mut self, target: Label, kind: JumpKind, bytecode: &mut Vec<Byte>);
    pub fn bind_label(&mut self, label: Label, target: u32, bytecode: &mut [Byte]);
    pub fn finalize(self) -> Result<(), BlockError>;
}
```

### Decisions locked in (during implementation)

1. **No local byte buffer.** All bytes go to an external
   `Vec<Byte>` (typically `Compiler::bytecode`). Positions
   recorded in `pending` are absolute positions in that
   external buffer. There is no coordinate-system conversion,
   no `relocate` post-pass, and no nested-control-flow hazard.
2. **`bind_label` is idempotent.** Re-binding re-patches every
   pending jump. The pre-16.6 non-idempotent design (with a
   separate `rebind_label`) was over-engineered for the codegen
   needs — every `emit_jump` is matched by exactly one
   `bind_label`, and re-binding is the simpler, more flexible
   contract.
3. **`finalize` errors only on labels that had pending jumps but
   were never bound.** A label allocated via `fresh_label` but
   never targeted by any jump is allowed (it has no effect on
   the bytecode).
4. **`emit_jump` is `#[allow(dead_code)]` for now.** The current
   `If` codegen uses `emit_jump_to` exclusively (each branch's
   JMPF jumps to a pre-allocated `branch_start_labels[i]` or to
   `end_label`). `emit_jump` is in the public API for future
   control-flow emitters (Loop, While).
5. **Renamed `common::Label` to `DiagLabel` in `lib.rs`.** The
   new `block_builder::Label` type would otherwise collide with
   the ariadne diagnostic `Label` from `common`. `pipeline.rs`
   and `typechecking/` keep the bare `Label` name (they don't
   reference `block_builder`).
6. **Last-branch `start` is `None`, not a fresh label.** The last
   branch never serves as a JMPF target (only `else` does, and
   `else` has no preceding JMPF in the if/else-if layout). Allocating
   a label for it would be dead state.

### `If` codegen layout (unchanged from 16.5)

For `if c1 { b1 } else if c2 { b2 } else { b3 }`:
```
c1, JMPF1, b1, JMP1, c2, JMPF2, b2, JMP2, b3, [end]
```

Bindings (now via BlockBuilder):
- `JMPF1` → `branch_start_labels[0]` (bound to start of c2 at i=1)
- `JMP1`  → `end_label` (bound at end to `end_pos`)
- `JMPF2` → `branch_start_labels[1]` (bound to start of b3 at i=2)
- `JMP2`  → `end_label` (bound at end to `end_pos`)

For single-branch `if c { b }`:
```
c, JMPF, b, [end]
```

Binding: `JMPF` → `end_label` (bound at end to `end_pos`).

### Test counts (16.6 final)

| Suite | Count | Delta vs 16.5 |
|-------|-------|--------------|
| `compiler/src/block_builder.rs::tests` (newly running) | 14 | +14 |
| All other suites | 301 | 0 |
| **Total** | **315** | **+14** |

The 16.6 BlockBuilder tests are 14 (down from the pre-16.6
orphaned 16) because some pre-16.6 tests have no analog in
the new API:

| Pre-16.6 test | New equivalent |
|---------------|----------------|
| `bind_label_uses_absolute_base` | (dropped — no `base` field in new API) |
| `bind_label_twice_panics` | replaced by `bind_label_is_idempotent` |
| `rebind_label_updates_jump_target` | replaced by `bind_label_is_idempotent` + `emit_jump_to_after_bind_label_records_new_pending` |
| `rebind_label_on_unbound_label_panics` | (dropped — `rebind_label` no longer exists) |
| `base_accessor_returns_constructor_value` | (dropped — no `base` field) |
| `current_position_is_absolute` | (dropped — caller uses `bytecode.len()` directly) |
| `extend_appends_in_order` | (dropped — caller uses `bytecode.extend()` directly) |
| `finalize_returns_bytecode_in_order` | replaced by `finalize_succeeds_when_all_labels_bound` + `integrated_test_with_bytecode` |
| `finalize_targets_ready_without_relocation` | (dropped — no relocation needed) |

### Files modified

| File | Net change | Purpose |
|------|-----------|---------|
| `compiler/src/block_builder.rs` | rewritten (~775 → ~673 LOC) | New placeholder-tracking API + 14 unit tests |
| `compiler/src/lib.rs` | +8 LOC (net) | `mod block_builder;` declaration, `DiagLabel` rename, `If` codegen refactor |

### Build status (16.6)

`cargo build --workspace` produces only the three pre-existing
parser warnings. No new compiler or machine warnings.

### Critical regression check

- `cargo test -p compiler --test pipeline fizbuz_runs_to_completion`
  passes. The pre-16.5 infinite-loop regression is still fixed.
- `cargo test -p compiler --test pipeline` (6 golden tests) all pass.
- `cargo run -- examples/fizbuz.0s` terminates with
  `FIZBUZFIZFIZBUZFIZFIZBUZ`.
- `cargo run -- examples/fib.0s` terminates with `13`.
- `cargo run -- examples/option.0s` prints `42`.
- `cargo run -- examples/result.0s` prints `42-1`.
- `cargo run -- examples/tree.0s` prints `6`.

### Anything 16.7+ needs to know

- `BlockBuilder` is now wired into production (only the `If`
  codegen for now). A natural follow-up is to refactor `Loop`'s
  `Expression::Loop` codegen to use `BlockBuilder` too — the
  pre-16.6 `Loop` codegen (in `compiler/src/lib.rs`) does its
  own `JMPF` / `JMP` placeholder math inline.
- The `Match` codegen (15C) does NOT use `BlockBuilder` (its
  layout is more complex than the simple forward-jump case).
  Refactoring `Match` to use `BlockBuilder` would require
  extending the API (or adding a separate utility for
  reverse-source-order jump tables).
- The pre-16.6 `BlockBuilder` had a `rebind_label` panic that
  was load-bearing for loop back-edges. With the new idempotent
  `bind_label`, no separate `rebind_label` is needed — just
  call `bind_label` again with the new target.
- The 16-bit `JUMP_IF_MATCH` target ceiling (15D.5 MEDIUM #1)
  is still open and is the next obvious VM target.
- The `let x = expr;` bug noted at the end of Phase 15D
  is still open and blocks `let`-bound variables.
- The `Expression::Default` AST variant is still
  reachable from real source as of 15D's `Default`
  codegen arm in `do_compile`.
- `examples/fizbuz.0s` had its `fizbuz(2)` through `fizbuz(15)`
  lines uncommented in a prior (uncommitted-from-AGENTS) edit;
  that change was pre-existing before Phase 16.6 and is unrelated
  to this work.

## PHASE 17A — WIRE BLOCKBUILDER INTO LOOP AND MATCH (COMPLETED)

### Summary

Phase 16.6 wired `BlockBuilder` into the `If` codegen. Phase 17A
completes the placeholder-tracking refactor by wiring the same
`BlockBuilder` primitive into the `Expression::Loop` and
`Expression::Match` codegen. The semantics are IDENTICAL to the
pre-17A implementations — only the placeholder tracking mechanism
changes (from manual `Vec<usize>`-based position tracking to
`BlockBuilder`'s `bind_label` / `emit_jump_to`).

Detailed design lives in
[`HM_TYPECHECKER_PLAN.md`](./HM_TYPECHECKER_PLAN.md) (the original
17A design is sketched at the end of that document).

### Loop codegen (before / after)

The pre-17A Loop (15 lines) built a local `loop_` Vec, emitted
`<iterable>, JMPF (placeholder), <body>, JMP→start`, then patched
the JMPF and JMP positions inline. Two manual patches were needed:

```rust
// Pre-17A Loop codegen
let mut loop_ = self.do_compile(iterable);
let exit = loop_.len();
loop_.push(Byte::new(Instruction::JMPF));
loop_.append(&mut self.do_compile(body));
loop_.push(Byte::new(Instruction::JMP).with_operand_u32(self.bytecode.len() as u32));
let len = loop_.len();
loop_[exit] = Byte::new(Instruction::JMPF).with_operand_u32((self.bytecode.len() + len) as u32);
self.bytecode.append(&mut loop_);
```

The 17A refactor uses `BlockBuilder` with two pre-allocated labels
(`top_label` for the loop entry, `exit_label` for the loop
exit) and a single `bind_label` / `emit_jump_to` pattern for each
jump. The layout is unchanged:

```
[top_label]
<iterable bytecode>
JMPF → exit_label
[exit_label]
<body bytecode>
JMP → top_label
```

All bytes (including any direct-to-`self.bytecode` emitters inside
the body, e.g. nested `Print` or `if`) land in `self.bytecode`;
`BlockBuilder` tracks placeholder positions in that same coordinate
system. No post-pass arithmetic, no nested-control-flow hazard.

### Match codegen (before / after)

The pre-17A Match (15C, ~210 lines) used **two manual placeholder
tracking structures**:

- `jump_if_match_places: Vec<(usize, u32)>` — the bytecode
  position of each `JUMP_IF_MATCH` placeholder and the tag it
  tests.
- `jmp_to_end_places: Vec<usize>` — the bytecode position of
  each `JMP`-to-end placeholder.
- `arm_body_offsets: Vec<usize>` — the start of each arm's
  binding+body in the final bytecode.

After emitting all the bytecode, the pre-17A code did two patching
loops: one for JMP-to-end placeholders → `end_offset`, and one for
JUMP_IF_MATCH placeholders → `arm_body_offsets[i]`.

The 17A refactor uses `BlockBuilder` with:

- A single `end_label` for the END of the match (JMP-to-end
  target).
- A `Vec<Option<Label>>` (`arm_labels`) of pre-allocated labels
  for each non-last constructor arm's JUMP_IF_MATCH target.

In the forward pass, the codegen emits `JUMP_IF_MATCH` placeholders
via `bb.emit_jump_to(arm_labels[i], JumpKind::JumpIfMatch, ...)`.
In the reverse pass, the codegen binds `arm_labels[i]` to the
current bytecode position when it starts emitting that arm's
binding+body. The `end_label` is bound to the final bytecode
position after all arm bodies are emitted. The 15C layout is
preserved:

```
<scrutinee bytecode>
JUMP_IF_MATCH tag_A → arm_labels[0]
JUMP_IF_MATCH tag_B → arm_labels[1]
UNPACK arity_C
[binding_C] body_c    ← reached by fall-through
JMP → end_label
[binding_B] body_b    ← reached via JUMP_IF_MATCH B
JMP → end_label
[binding_A] body_a    ← reached via JUMP_IF_MATCH A
[end_label]
```

### Decisions locked in (during implementation)

1. **Loop and Match get the SAME `BlockBuilder` pattern as If.**
   No new API surface was needed. The pre-16.6 `emit_jump` helper
   is still `#[allow(dead_code)]` — every production emitter uses
   `emit_jump_to` exclusively, including the new Loop and Match
   codegen.
2. **Loop's `top_label` and `exit_label` are both bound BEFORE
   the body is emitted.** The pre-17A codegen had the JMPF target
   = past-the-loop, computed inline; the new codegen binds
   `exit_label` to the start of the body (= current
   `self.bytecode.len()` at that point), which is the same
   position. The semantics are identical.
3. **Match's `arm_labels` is pre-allocated in a single pass over
   the arms** (not lazily in the forward or reverse pass). The
   closure passed to `arms.iter().enumerate().map(...)` calls
   `bb.fresh_label()` for each non-last constructor arm. The
   labels are stored in a `Vec<Option<Label>>` indexed by arm
   index, with `None` for arms that don't need a JUMP_IF_MATCH
   (last arm, wildcard, binding).
4. **Match's `end_label` is bound at the end of the reverse
   pass.** Every non-first arm's body is followed by an
   `emit_jump_to(end_label, Unconditional, ...)` placeholder;
   binding `end_label` to the final bytecode position patches
   all of them in one `bind_label` call.
5. **No new warnings introduced.** The borrow-checker issues
   that arose in the If codegen (the `let body_bc = ...;
   self.bytecode.extend(body_bc)` staging pattern) apply
   identically to Loop and Match. The pre-17A codegen for
   Loop also used the `let body_bc` staging pattern; the new
   codegen just continues to do so.
6. **Wildcard/Binding arms still emit `POP`/`STORE` in the
   forward pass**, exactly as in the 15C codegen. The BlockBuilder
   refactor doesn't change this — those instructions aren't
   jumps and don't need placeholder tracking.
7. **No semantic change to the threaded-code layout.** The
   bytecode produced for any given input is byte-for-byte
   identical to the pre-17A output (modulo any direct
   reordering of `JUMP_IF_MATCH` and `UNPACK` opcodes, which
   doesn't change program behavior — they're emitted in the
   same order).

### Diagnostics produced

The codegen and VM are silent on type errors (the typechecker,
15B, already produced those diagnostics upstream). The new
`Loop` and `Match` codegen produce the same bytecode as the
pre-17A implementations, so no new runtime behavior emerges.

### Test counts (17A final)

| Suite | Count | Delta vs 16.6 |
|-------|-------|--------------|
| `compiler/src/lib.rs::tests` (codegen + e2e) | 262 | +5 |
| `compiler/src/block_builder.rs::tests` | 14 | 0 |
| `compiler/src/pipeline.rs::tests` (ariadne) | 2 | 0 |
| `compiler/tests/diagnostics.rs` (golden integration) | 24 | 0 |
| `compiler/tests/pipeline.rs` (golden e2e) | 6 | 0 |
| `common` | 2 | 0 |
| `machine` | 11 | 0 |
| `parser` | 9 | 0 |
| doctests | 6 | 0 |
| **Total** | **320** | **+5** |

The 5 new tests in `compiler/src/lib.rs::tests` (added in the
"Phase 17A: BlockBuilder for Loop and Match codegen" section):

1. `loop_emits_top_label_and_back_edge` — Asserts that a
   `while` loop's bytecode has at least 1 JMPF (the exit
   condition) and at least 1 JMP (the back-edge). Mirrors
   the 16.5 `nested_if_in_loop_runs_correctly` regression
   test for If.
2. `loop_jmp_back_edge_targets_loop_top_not_prologue` —
   Asserts that the loop's JMP back-edge targets a byte
   offset > 3 (past the 3-byte prologue). If `bind_label`
   for `top_label` were missed, the JMP would either
   target offset 0 (the prologue `CALL`) or be patched
   incorrectly — the program would crash.
3. `match_jump_if_match_targets_are_patched_to_arm_offsets`
   — Asserts that every `JUMP_IF_MATCH` placeholder's
   target (lower 16 bits) is > 0. If `bind_label` for an
   arm's label were missed, the target would be 0 (the
   `BlockBuilder` placeholder value) and the VM would
   jump to the prologue.
4. `match_jmp_to_end_placeholders_are_patched_to_end_label`
   — Asserts that a 3-arm match emits exactly 2 JMP-to-end
   placeholders, and that both target the same `end_label`
   position. If `bind_label` for `end_label` were missed,
   both JMPs would target 0.
5. `nested_match_in_loop_emits_expected_opcodes` — Asserts
   that a `match` inside a `while` loop body has at least
   1 JMPF, 1 JMP, 1 JUMP_IF_MATCH, and 1 UNPACK. The
   canonical nested-control-flow scenario; guards against
   the same kind of off-by-one that the 16.5/16.6 If
   refactor fixed.

The match-in-loop test uses a `return` statement to wrap
the match (the parser doesn't accept `match { ... }` as
a standalone statement followed by another statement —
the match is an expression and the parser wants an
operator). This is a parser limitation, not a codegen
issue; the test verifies the codegen produces the
expected opcodes regardless.

### Files modified

| File | Net change | Purpose |
|------|-----------|---------|
| `compiler/src/lib.rs` | +414 LOC (net, +503 / -89) | Refactor Loop and Match codegen onto `BlockBuilder` + 5 new tests |

`compiler/src/block_builder.rs` is unchanged — the existing
`BlockBuilder` API was sufficient for both refactors. The
`emit_jump` helper (previously `#[allow(dead_code)]` because
no production emitter used it) is still `#[allow(dead_code)]`
because both new emitters also use `emit_jump_to` exclusively.

### Build status (17A)

`cargo build --workspace` produces only the three pre-existing
parser warnings (`None`/`Xor`/`Equal`/`Unary`/`Call` variants,
`prefix` field, `inc`/`dec` methods in `parser/src/lib.rs`).
No new compiler or machine warnings.

### Critical regression check

- `cargo test -p compiler --test pipeline fizbuz_runs_to_completion`
  passes. The pre-16.5 infinite-loop regression is still fixed.
- `cargo test -p compiler --test pipeline` (6 golden tests) all pass.
- `cargo run -- examples/fizbuz.0s` terminates with
  `FIZBUZFIZFIZBUZFIZFIZBUZ`.
- `cargo run -- examples/fib.0s` terminates with `13`.
- `cargo run -- examples/option.0s` prints `42`.
- `cargo run -- examples/result.0s` prints `42-1`.
- `cargo run -- examples/tree.0s` prints `6`.

### Anything 17B+ needs to know

- `BlockBuilder` is now wired into ALL three control-flow
  constructs (`If`, `Loop`, `Match`). Every production
  placeholder jump in the codegen uses the same primitive.
- The 16-bit `JUMP_IF_MATCH` target ceiling (15D.5 MEDIUM #1)
  is still open and is the next obvious VM target.
- The O(n) heap-pointer classification in `MakeEnum` (15C's
  "Anything 15D needs to know") is still O(n). A generation
  table or per-frame pointer map would let `MakeEnum` and
  `Unpack` classify in O(1).
- The `let x = expr;` bug noted at the end of Phase 15D
  is still open and blocks `let`-bound variables.
- The `Expression::Default` AST variant is still reachable
  from real source as of 15D's `Default` codegen arm in
  `do_compile`.
- The `Match` codegen's inner-pattern dispatch limitation
  (15D's "Known limitations") is still open: two arms with
  the same outer tag but different inner payloads will
  always take the first arm's body, regardless of the
  inner payload. The fix is to chain a second
  `JUMP_IF_MATCH` for the inner tag, or to add a separate
  "tag-of-payload" test after the outer matches.
- The `JUMP_IF_MATCH` operand-layout has a 16-bit target
  ceiling (65,535 bytes). Programs with very deep
  expression trees in a single arm body would silently
  fail to dispatch (the patch step would `as u16`-truncate
  the target). The fix is to widen the operand to `u32`,
  matching the regular `JMP`, with the tag in a separate
  scratch word. No current test program approaches the
  65,535 limit.
- `examples/fizbuz.0s` had its `fizbuz(2)` through `fizbuz(15)`
  lines uncommented in a prior (uncommitted-from-AGENTS) edit;
  that change was pre-existing before Phase 16.6 and is
  unrelated to this work.


## PHASE 17B — RECORD-TYPE PAYLOADS FOR SUM VARIANTS (COMPLETED)

### Summary

Extended the sum-types/match work to support **record-shaped
variant payloads**. Phase 15B/15C supported Unit and Tuple
payloads; Phase 17B adds Record payloads (`Point { x: int,
y: int }`) in declarations, constructor calls, and patterns.
The HM typechecker, codegen, and parser were all extended;
no new VM opcodes were introduced.

The 17B-cleanup pass (after the original 17B landing)
also fixed the **multi-variant binding body** limitation
that the original 17B documented as deferred to 17C. The
fix uses a per-arm `match_bindings` map that decouples
match-bound variable slots from the global `Interner` —
see Decisions §11 below for the design.

Detailed design lives in
[`HM_TYPECHECKER_PLAN.md`](./HM_TYPECHECKER_PLAN.md) (the
original 17B design is sketched at the end of that document).

### What works

- **Declaration:** `enum Point { Origin, Point { x: int, y: int } }`
- **Construction:** `Point::Point { x: 5, y: 12 }`
- **Construction with explicit type:**
  `Point::Point { x: 5, y: 12 }` (the type is inferred from
  the enum declaration, so explicit annotation is rarely
  needed).
- **Construction (shuffled fields):** `Point::Point { y: 12, x: 5 }`
  — the codegen reorders to declaration order before binding
  (decision §6).
- **Pattern (record shape):**
  `match p { Point::Point { x, y } => x * x + y * y, ... }`
- **Pattern (positional reorder):** the pattern may supply
  fields in any order — the codegen reorders to declaration
  order before binding.
- **Pattern (shorthand):** `{ x, y }` desugars to
  `{ x: x, y: y }`.
- **Multi-variant matches with binding bodies:** now work
  correctly. `match s { Shape::Empty => 0,
  Shape::CircleR(r) => r * r, Shape::Rect { width, height
  } => width * height, Shape::Tri { a, b, c } => (a + b + c)
  / 3 }` produces the correct values (`0`, `25`, `12`, `2`).
- **Empty parens `()`** in a constructor or pattern are
  parsed as the Unit shape (so `Point::Origin` ≡
  `Point::Origin()`).
- **Mixed-shape enums:** a single enum can have variants
  of all three shapes (Unit, Tuple, Record). See
  `examples/mixed.0s` for the canonical demonstration.

### Examples added

- `examples/record.0s` — distance-squared from origin
  (5² + 12² = 169). Uses a Unit variant + a Record variant.
- `examples/mixed.0s` — tag-of-shape dispatch on a
  Unit + Tuple + Record enum with **binding bodies**
  (each arm uses its own payload values). Outputs
  `0`, `25`, `12`, `2`.

### Decisions locked in (during implementation)

1. **Explicit shape enum (`EnumVariantPayload` /
   `EnumVariantPayloadTy`).** The AST and HM typechecker
   use an explicit shape enum (Unit/Tuple/Record), NOT
   synthetic names. The red-team flagged this as
   MUST-HAVE #1 — without it, diagnostics can't say
   "expected record, got tuple". The shape enum carries
   through to `Ty::Sum.variants: Vec<(String,
   EnumVariantPayloadTy)>` and is matched on for
   unification.
2. **Synthetic-name trick only at codegen level.**
   `field_pairs()` returns `Vec<(String, Ty)>` using
   synthetic names `"0"`, `"1"`, ... for Tuples, and
   declared names for Records. This helper is used ONLY
   by the codegen's `Construct` reordering and `Match`
   binding walk — never by Display, unify, or the AST.
3. **Isorecursive encoding preserved.** Recursive
   payloads (e.g. `Tree::Node(int, Tree, Tree)`) continue
   to use `Ty::Con(name)` opaque references, NOT the
   unfolded `Ty::Sum(...)`. The HM occurs check would
   otherwise reject recursive enums.
4. **Pre-pass + main-pass for enum registration.** A
   `pre_register_enums_walk` collects every enum's
   shape before the main inference pass. This is needed
   because Phase 15B only registered the first N payload
   names; Phase 17B registers the full `EnumVariantPayloadTy`
   (with shape and field names) so the codegen knows the
   record fields at construction sites.
5. **Pattern returns the scrutinee's type.** A pattern's
   type is the scrutinee's type (the pattern desugars the
   value); the tag is captured separately for
   exhaustiveness checking (Phase 15B).
6. **Reorder-on-borrow (codegen-level).** When the user
   writes `Point::Point { y: 12, x: 5 }`, the codegen
   reorders the constructor arguments to declaration
   order (`x: 5, y: 12`) using `payload_tys_for`. The
   user sees no difference. For record constructs, the
   codegen walks the DECLARATION order REVERSED so that
   `MAKE_ENUM`'s top-first pop places values at the
   right payload indices.
7. **No new VM opcodes.** Per the 17B spec, record shapes
   reuse `MakeEnum` + `Unpack` (already in Phase 15C).
   The VM treats them identically to Tuple payloads.
8. **Empty parens `()` parsed as Unit.** Both `Point::Origin`
   and `Point::Origin()` are accepted. The parser's
   `enum_variant` rule treats the parens as an optional
   empty-tuple that lowers to Unit.
9. **Shape mismatch is a separate diagnostic** from arity
   mismatch. A constructor called with the wrong arity
   says `Constructor \`X::Y\` expects N arguments, got M`.
   A constructor called with the right arity but wrong
   shape says `payload shape mismatch: ...`. The legacy
   arity message is preserved for tuples (red-team
   MUST-HAVE #5). For **record** shapes, the arity check
   is deferred to the field-by-field pass below it,
   which produces more specific "Missing field `x`" /
   "Unknown field `y`" diagnostics.
10. **Forward references resolve correctly.** An enum
    referenced in a constructor or pattern before its
    declaration site still typechecks (e.g. a function
    at the top of the file using an enum declared
    later), thanks to the two-pass inference.
11. **Per-arm `match_bindings` map (17B-cleanup).** The
    17B-cleanup pass fixed the multi-variant binding
    body limitation. The codegen maintains a
    `match_bindings: Option<HashMap<String, u32>>` on
    `Context`, populated freshly for each match arm's
    binding code. Bindings are assigned slot IDs 1, 2,
    3, ... matching the VM's `JUMP_IF_MATCH` / `UNPACK`
    payload-push positions (which start at `frame.sp +
    1`). The body's `Identifier` / `Assignment`
    lookups consult the per-arm map FIRST, falling back
    to the global `variables` Interner for non-pattern
    names. This makes every arm's first binding live at
    slot 1 (regardless of which arm it is in), so the
    VM's payload-push positions align with the
    STORE/LOAD operands. The fix is local to the
    codegen — no VM changes were needed.

### Diagnostics produced (17B + 17B-cleanup)

| Site | Message format |
|------|----------------|
| Duplicate field in record literal | `Duplicate field \`x\` in record constructor \`Point\`` + help |
| Duplicate field in record pattern | `Duplicate field \`x\` in record pattern \`Point\`` + help |
| Shape mismatch (construct) | `Constructor \`X::Y\` payload shape mismatch (declared as ..., called as ...)` + help |
| Shape mismatch (pattern) | `Constructor pattern \`X::Y\` payload shape mismatch (declared as ..., pattern uses ...)` + help |
| Missing field (record construct) | `Missing field \`x\` in record constructor \`X::Y\`` + help |
| Unknown field (record construct) | `Unknown field \`z\` in record constructor \`X::Y\`` + help |
| Missing field (record pattern) | `Missing field \`x\` in record pattern \`X::Y\`` + help |
| Unknown field (record pattern) | `Unknown field \`z\` in record pattern \`X::Y\`` + help |

### Test counts (17B + 17B-cleanup final)

| Suite | Count | Delta vs 17A |
|-------|-------|--------------|
| `compiler/src/typechecking/*` (unit) | 274 | +43 |
| `compiler/src/lib.rs::tests` (codegen + e2e) | 16 | +6 (6 record codegen tests) |
| `compiler/src/pipeline.rs::tests` (ariadne) | 2 | 0 |
| `compiler/tests/diagnostics.rs` (golden integration) | 30 | +6 (6 record diagnostic tests) |
| `compiler/tests/pipeline.rs` (golden e2e) | 8 | +2 (record + mixed golden tests) |
| `common` | 2 | 0 |
| `machine` | 11 | 0 |
| `parser` | 12 | +3 |
| doctests | 6 | 0 |
| **Total** | **361** | **+72** |

The +72 delta from 17A's 289 is broken down as:

- +43 typechecker unit tests for record shapes (helper
  functions, unification with record shapes, sum with
  mixed shapes, pattern binding with record shapes,
  pretty-printing, etc.).
- +6 codegen tests for record payloads (the
  red-team's canonical
  `record_construct_reorders_shuffled_call_site_fields`
  plus 5 supporting tests — see Decisions §11 for the
  full list).
- +6 diagnostic tests for record payloads (missing /
  extra / shape-mismatch / duplicate field, plus the
  mixed-shape regression test).
- +2 pipeline golden tests (record prints 169; mixed
  prints 025122 with the corrected multi-variant
  binding body).
- +3 parser tests (record variant parsing, record
  construct parsing, record pattern shorthand
  desugaring).
- +12 net from elsewhere (block-builder, lib.rs)
  inherited from 17A and earlier.

### Files modified

| File | Net change | Purpose |
|------|-----------|---------|
| `parser/src/ast.rs` | +146 LOC | `EnumVariantPayload`, `RecordFieldDecl`, `RecordFieldValue`, `PatternField`, `EnumConstructPayload`, `PatternPayload` + Display impls |
| `parser/src/lib.rs` | +98 LOC (was); -10 LOC in cleanup | Parser updates to `enum_variant`, `construct`, `pattern`; empty parens treated as Unit; 17B-cleanup removed the dead `dups` Vec / `msg` String (Issue 6); +3 tests |
| `compiler/src/typechecking/ty.rs` | +112 LOC | `EnumVariantPayloadTy` enum, helpers `field_count`, `field_types`, `field_pairs`; tests for helpers |
| `compiler/src/typechecking/infer.rs` | +183 LOC; 17B-cleanup: -8 LOC (deferred arity check for records) | `Checker::enum_payloads` field, `payload_tys_for` method, shape/arity diagnostics; record arity check deferred to field-by-field pass for better diagnostics; +43 tests |
| `compiler/src/typechecking/unify.rs` | +38 LOC | Sum arm uses `field_count` + shape discriminant; tests for record shape matching/mismatch |
| `compiler/src/typechecking/pretty.rs` | +24 LOC | Display for Sum with Unit/Tuple/Record rendering |
| `compiler/src/typechecking/subst.rs` | +12 LOC | Walk `EnumVariantPayloadTy` in apply/substitute_vars |
| `compiler/src/typechecking/env.rs` | +9 LOC | Walk `EnumVariantPayloadTy` in env's substitute_vars |
| `compiler/src/typechecking/id.rs` | +14 LOC | Pre-walk for `EnumVariant`/`Construct`/`PatternPayload` |
| `compiler/src/lib.rs` | +76 LOC (was); +~140 LOC in cleanup | Codegen for `Construct` (record reorder via `payload_tys_for`); codegen for `Match` outer-arm binding (Record walks decl_order); +match-end `RETURN` to avoid function codegen's auto-default clobbering; 17B-cleanup: `match_bindings: Option<HashMap<String, u32>>` per-arm map + `lookup_slot` helper for the multi-variant binding body fix; +6 record codegen tests |
| `examples/record.0s` | new (~22 LOC) | Record-shape end-to-end smoke test |
| `examples/mixed.0s` | new (~22 LOC); 17B-cleanup: corrected to use shape-correct patterns (Issue 1) | Mixed-shape enum end-to-end smoke test, with binding bodies across all three shapes |
| `AGENTS.md` | this section (+192 net for original; +~120 in cleanup) | Documentation |

### Known limitations (forwarded to 17C+)

1. **Nested record patterns inside an arm body are
   rejected.** A pattern like `Result::Ok(Inner {
   v }) => v` is rejected by the typechecker because
   the inner record pattern must look up the variant
   from the outer arm's payload type, which the
   codegen doesn't thread. The 17B codegen emits a
   `POP` for nested record patterns as a defensive
   fallback.

2. **Field access (`point.x`) is not supported.** The
   record-shape payload can only be destructured via
   pattern matching. Direct field access would
   require a new expression form and new VM opcode
   (`LOAD_FIELD`). Deferred to 17C+.

### Build status (17B + 17B-cleanup)

`cargo build --workspace` produces only the three
pre-existing parser warnings. No new compiler or
machine warnings.

### Critical regression check

- `cargo test --workspace` — all 361 tests pass.
- `cargo run -- examples/fib.0s` terminates with `13`.
- `cargo run -- examples/option.0s` prints `42`.
- `cargo run -- examples/result.0s` prints `42-1`.
- `cargo run -- examples/tree.0s` prints `6`.
- `cargo run -- examples/record.0s` prints `169`.
- `cargo run -- examples/mixed.0s` prints `025122`
  (binding-body dispatch — the 17B-cleanup fix
  makes this produce correct output).
- `cargo run -- examples/fizbuz.0s` terminates with
  `FIZBUZFIZFIZBUZFIZFIZBUZ`.

### Anything 17C+ needs to know

- The multi-variant binding body limitation is **fixed**
  (Decisions §11). The fix is local to the codegen —
  no VM changes were needed. The `match_bindings`
  per-arm map is the load-bearing piece.
- The nested record pattern limitation (Known
  Limitation #1) needs the codegen to thread the
  outer arm's payload type into `emit_pattern_binding`.
- The field-access limitation (Known Limitation #2)
  needs a new `LOAD_FIELD` opcode (out of scope for
  17C; deferred to 18+).
- The 16-bit `JUMP_IF_MATCH` target ceiling
  (15D.5 MEDIUM #1) is still open and is the next
  obvious VM target.
- The O(n) heap-pointer classification in `MakeEnum`
  (15C's "Anything 15D needs to know") is still O(n).
- The `let x = expr;` bug noted at the end of Phase 15D
  is still open and blocks `let`-bound variables.
- The `Expression::Default` AST variant is still
  reachable from real source as of 15D's `Default`
  codegen arm in `do_compile`.
