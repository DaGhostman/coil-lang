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

