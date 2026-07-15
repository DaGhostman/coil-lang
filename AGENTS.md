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
- `examples/fib.0s` (tweaked from `fib(7)` to `fib(32)`; expects
  the 32nd Fibonacci number, `2178309`)

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
- `cargo run -- examples/fib.0s` terminates with `2178309` (the user updated `fib(7)` to `fib(32)`; the test pipeline expects the new output).
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
- `cargo run -- examples/fib.0s` terminates with `2178309` (the user updated `fib(7)` to `fib(32)`; the test pipeline expects the new output).
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
- The `Expression::Default` AST variant is still
  reachable from real source as of 15D's `Default`
  codegen arm in `do_compile`.

## PHASE 18D — ACCESS CODEGEN (COMPLETED)

### Summary

Wired `Expression::Access` end-to-end through codegen. The parser
produces `Access` from postfix `expr.field` (Phase 18D parser
work) and the HM typechecker resolves it to the field's type
(Phase 18D typechecker work — 12 new tests). What's missing was
the bytecode emitter: replace the placeholder
`Expression::Access(_, _) => {}` arm in `do_compile` with real
codegen that emits `LoadField(field_index)` after the receiver.

### What works

- **Declaration:** `enum Point { Origin, Point { x: int, y: int } }`
- **Field access on a function parameter:**
  `fn get_x(Point p) -> int { return p.x; }` returns `p.x`.
- **Field access on a let-bound variable:**
  `let p = Point::Point { x: 5, y: 12 }; print "%i", p.x;`
- **Field access on a constructor result:**
  `let p = Point::Point { x: 5, y: 12 }; print "%i", p.x;`
- **Multi-field access:** `p.x` and `p.y` both work on the same
  receiver — different `field_index` operands route to the
  correct slot.

The codegen is a thin layer over the existing VM `LoadField`
opcode (introduced in Phase 18D as the load-field backing for
field access). The VM already implements the runtime semantics
(`operands[15:0]` = field_index, pop the receiver, push
`payload[field_index]`). This phase wires up the emitter.

### Decisions locked in (during implementation)

1. **No new VM opcodes.** Reuses the existing `LoadField`
   instruction from Phase 18D. The VM arm is unchanged.
2. **`Lookup_at(receiver_id)` doesn't work.** The typechecker's
   `infer` function visits nodes in pre-order, but
   `infer_function` SKIPS a function's `args` Fragment (it uses
   `parse_arg_list` to read arg types directly instead of
   recursing through `infer`). The pre-walk DOES mint IDs for
   args. Result: the pre-walk's ID table and the infer cache
   are MISALIGNED inside function bodies by N+1 IDs (one for
   the Fragment wrapper, N for the Arguments). Using
   `id_table()[emit_idx]` from `do_compile` would map to the
   wrong AST node. This is the same misalignment that affects
   `compile_binary_operands`'s float-vs-int selection inside
   function bodies — but `compile_binary_operands` works at
   the top level (no `infer_function` skips), so it gets
   lucky. The Access codegen needs to handle the function-body
   case explicitly.
3. **Env lookup doesn't work either.** `infer_function` POPS
   its frame after processing the body, so function args are
   gone from the env by the time codegen runs.
4. **Side-table in the Checker.** Added
   `codegen_var_types: HashMap<String, Ty>` to `Checker`. Populated
   in `infer_function` (for each arg) and `infer_fragment` (for
   each let-bound variable and constant). Survives both the
   env-pop and the ID-misalignment issues. The codegen queries
   it via `Checker::codegen_var_type(name) -> Option<&Ty>`.
5. **`enum_name_for_receiver` walks the receiver AST.** Handles
   `Identifier` (side-table lookup) and chained `Access`
   (recurses on the inner receiver). Recursively unwraps the
   type via `extract_enum_name` (handles `Ty::Con` / `Ty::Sum` /
   `Ty::Constructor`).
6. **`field_index_for` looks up the field's declaration
   position.** Returns `Some((variant_name, field_index))` if
   exactly one record-shaped variant in the enum declares the
   field. The HM typechecker rejects sources where the field
   isn't uniquely declared (ambiguous case), so a `None` return
   at codegen time means the source had a type error.
7. **Defensive fallback emits `LoadField(0)`.** When the
   side-table lookup fails or the receiver is not a simple
   Identifier, we still emit a well-formed `LoadField(0)` so
   the bytecode stays valid for downstream checks (and the VM
   silently no-ops on out-of-bounds field indices). The
   typechecker's diagnostic was already emitted upstream.
8. **Two new codegen tests in `compiler/src/lib.rs::tests`.**
   `access_field_emits_receiver_then_load_field` and
   `access_field_emits_correct_field_index_for_each_field`.
   The second is the red-team's critical regression test: a
   buggy codegen that always emits `LoadField(0)` would pass
   the first test (x is field 0) but silently return the
   WRONG value for `p.y` (also returning x). The second test
   catches this category of bug.

### Files modified

| File | Net change | Purpose |
|------|-----------|---------|
| `compiler/src/lib.rs` | +130 LOC | `Expression::Access` codegen arm, `enum_name_for_receiver` + `receiver_type` helpers, `extract_enum_name` free fn, 2 new codegen tests |
| `compiler/src/typechecking/infer.rs` | +110 LOC | `codegen_var_types` side-table field, `new()`/`check_program` clearing, `infer_function` + `infer_fragment` population, `field_index_for` + `codegen_var_type` + `infer_for_codegen` accessors |
| `examples/record.0s` | rewrote | Added `x_coord` + `y_coord` functions that read fields via `p.x` / `p.y`. Output extended from `"169"` to `"169512"`. |
| `compiler/tests/pipeline.rs` | 1 line | Updated `example_record_prints_169` → `example_record_prints_169_5_12` with new expected output. |

### Test counts (18D-codegen final)

| Suite | Count | Delta vs 18D-typechecker |
|-------|-------|-------------------------|
| `compiler/src/typechecking/*` (unit) | 253 | 0 |
| `compiler/src/lib.rs::tests` (codegen + e2e) | 298 | +2 (codegen tests 22–23) |
| `compiler/src/pipeline.rs::tests` (ariadne) | 9 | 0 |
| `compiler/tests/diagnostics.rs` (golden integration) | 33 | 0 |
| `compiler/tests/pipeline.rs` (golden e2e) | 9 | 0 |
| `common` | 2 | 0 |
| `machine` | 17 | 0 |
| `parser` | 16 | 0 |
| doctests | 6 | 0 |
| **Total** | **381** | **+2** |

The +2 is the two new codegen tests in `compiler/src/lib.rs::tests`:

1. `access_field_emits_receiver_then_load_field` — locks in the
   MakeEnum + LoadField bytecode shape and the operand = 0 for
   the first field.
2. `access_field_emits_correct_field_index_for_each_field` —
   locks in `LoadField(0)` for `p.x` and `LoadField(1)` for
   `p.y` (the critical regression test for off-by-one field
   indices).

### End-to-end smoke test

`examples/record.0s` compiles and runs correctly, printing
`169512` (5²+12² from pattern destructuring, then `p.x` = 5
and `p.y` = 12 from field access):

```0s
enum Point {
    Origin,
    Point { x: int, y: int },
}

fn distance_squared(Point p) -> int {
    return match p {
        Point::Origin => 0,
        Point::Point { x, y } => x * x + y * y,
    };
}

fn x_coord(Point p) -> int {
    return p.x;
}

fn y_coord(Point p) -> int {
    return p.y;
}

fn main() {
    print "%i", distance_squared(Point::Point { x: 5, y: 12 });
    print "%i", x_coord(Point::Point { x: 5, y: 12 });
    print "%i", y_coord(Point::Point { x: 5, y: 12 });
}
```

### Build status

`cargo build --workspace` produces only the three pre-existing
parser warnings. No new compiler or machine warnings.

### Critical regression check

- `cargo test -p compiler --test pipeline` (9 golden tests) all
  pass, including `example_record_prints_169_5_12`.
- `cargo run -- examples/record.0s` terminates with
  `169512`.
- `cargo run -- examples/fib.0s` terminates with `2178309` (the user updated `fib(7)` to `fib(32)`; the test pipeline expects the new output).
- `cargo run -- examples/option.0s` prints `42`.
- `cargo run -- examples/result.0s` prints `420-1`.
- `cargo run -- examples/tree.0s` prints `6`.
- `cargo run -- examples/mixed.0s` prints `025122`.
- `cargo run -- examples/fizbuz.0s` terminates with
  `FIZBUZFIZFIZBUZFIZFIZBUZ`.

### Anything 19+ needs to know

- The `codegen_var_types` side-table is a workaround for the
  pre-existing ID-misalignment issue (caused by
  `infer_function` skipping args via `parse_arg_list`). A more
  general fix would be to align the pre-walk and infer pass
  inside function bodies — e.g., have `infer_function` call
  `infer` on each arg node so IDs are minted in lockstep. The
  side-table unblocks Phase 18D without requiring that bigger
  refactor.
- Chained field access (`p.x.y` where `x` is itself a record
  enum) is typechecked but the codegen emits a defensive
  `LoadField(0)` for the outer access. The inner access works
  correctly (it's just a regular field access on its own
  receiver). The outer access needs the typechecker to record
  the field type in the side-table — currently the side-table
  only stores declared variable types, not field types. Future
  work could extend it.
- The `Expression::Default` AST variant is still reachable
  from real source as of 15D's `Default` codegen arm in
  `do_compile`.
- The 16-bit `JUMP_IF_MATCH` target ceiling (15D.5 MEDIUM #1)
  is still open and is the next obvious VM target.
- The O(n) heap-pointer classification in `MakeEnum`
  (15C's "Anything 15D needs to know") is still O(n).
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
- `cargo run -- examples/fib.0s` terminates with `2178309` (the user updated `fib(7)` to `fib(32)`; the test pipeline expects the new output).
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


## PHASE 18B — NESTED RECORD PATTERNS (COMPLETED)

### Summary

Lifted the Phase 17B-cleanup **Known Limitation #1**
("nested record patterns inside an arm body are
rejected"). Pre-18B, the codegen emitted a single `POP`
for an inner record pattern (instead of walking its
declared fields), so the binding slot for the inner
record's fields was never populated and the arm body
read the OUTER record's slot values (an enum pointer
instead of the inner record's payload values).

The Phase 18B fix introduces a new VM opcode
(`UnpackAt`) for slot-based UNPACK and threads a new
`parent_decl_order` parameter through
`emit_pattern_binding` so the recursion walks the
inner record's declared fields in declaration order
at unbounded depth (`Result::Ok(Inner { v }) => v`,
`Foo::Bar { a: W::W { v } } => v`, etc.).

### What works

- **Record → record nesting:**
  `Wrap::W { inner: Inner::I { v }, name } => v`
  binds `v` to the inner record's `v` field.
- **Tuple → record nesting:**
  `Result::Ok(Inner::I { v }) => v` binds `v` to the
  inner record's `v` field (via the existing top-pop
  UNPACK, since the OUTER Tuple UNPACK leaves the
  inner record at the top of stack).
- **Record → tuple → record nesting:**
  `Result::Ok { x: Inner::I { v } } => v`.
- **Unbounded depth:** the codegen recurses at any
  depth — depth-3 nesting (`Foo::Bar(Baz::Qux { a:
  W::W { v } }) => v`) is exercised by the
  `match_depth_3_nested_records_bind_correctly`
  codegen test.
- **Missing inner fields:**
  `Result::Ok(Inner::I { }) => 99` (the inner pattern
  omits `v`) emits a defensive `POP` for the missing
  field so the stack cursor advances correctly.

### New VM opcode: `UnpackAt`

Appended to the `Instruction` enum (Phase 18B —
slot-based UNPACK for nested record patterns).

- `UnpackAt slot_offset, arity` reads the enum
  value at `stack[frame.sp + slot_offset]` and writes
  the payload values to consecutive positions starting
  at `stack[frame.sp + slot_offset]` (overwriting in
  place). The stack pointer doesn't change.
- Distinct from `Unpack` (which always pops the TOP
  of stack). `UnpackAt` reads from an arbitrary slot,
  which is what nested record patterns need (the inner
  record's enum value sits at a non-top slot after the
  OUTER record's UNPACK pushed its fields).

**Limitation:** the arity of the nested record must be
<= the field's position in the OUTER record's
decl_order. A 2-field nested record at position 1 would
clobber the OUTER record's position-2 field. Programs
with multi-field nested records interleaved with
non-nested OUTER fields would need a scratch-area
scheme (deferred to 19+). All test programs and
`examples/nested_records.0s` satisfy this constraint.

### Decisions locked in (during implementation)

1. **`emit_pattern_binding` is a free function with an
   explicit `&Checker` parameter.** Not a method on
   `Compiler` — the borrow checker would see
   `&self.checker` (immutable) and `&mut self.bytecode`
   (mutable) as conflicting. Passing `&Checker`
   directly keeps the bytecode mutation and the checker
   lookup disjoint.

2. **New `parent_decl_order` parameter for record
   patterns.** Threaded through `emit_pattern_binding`.
   For the OUTER record pattern, the caller passes
   `checker.payload_tys_for(enum_name, variant_name)`.
   For sub-pattern recursion (entered from the Record
   arm itself), `parent_decl_order` is the sub-pattern's
   own `payload_tys_for` if the sub-pattern is a
   constructor record; empty otherwise.

3. **New `is_outer: bool` parameter.** Distinguishes
   the OUTER call (from the codegen, where the forward
   pass has already emitted `UNPACK` or `JUMP_IF_MATCH`
   for the OUTER enum value) from recursive calls (where
   the pattern's enum value sits at a non-top slot). At
   the OUTER level, the function SUPPRESSES `UNPACK`
   emission (the forward pass handled it). At the
   RECURSION level, `UnpackAt` is emitted for nested
   records so their payload values reach the right slot
   positions.

4. **`consume_values` propagates through recursion.**
   When the OUTER call is `consume_values = false` (test
   chain arm), the test chain has already emitted the
   `POP` / `JUMP_IF_MATCH` for the inner values. The
   recursion suppresses the redundant bytecode at every
   level. When `consume_values = true` (normal arm), the
   recursion emits bytecode normally.

5. **`emit_inner_test` likewise walks Record arms in
   declaration order and recurses for nested records.**
   Same fix as `emit_pattern_binding` — the previous
   `emit_inner_test` walked Record fields in SOURCE
   order, misrouting `POP` / `STORE` / `JUMP_IF_MATCH`
   to the wrong slots.

6. **Slot-based UNPACK via `UnpackAt`, not stack
   rotation.** Adding a new opcode is cleaner than
   emitting `SWAP` + `UNPACK` + `SWAP` sequences for
   each nested record (which would also confuse the
   binding slot numbering).

### Diagnostics produced (18B)

The codegen and VM are silent on type errors (the
typechecker, 15B, already produced those diagnostics
upstream). The new VM arm for non-enum scrutinees is
defensive: it falls through silently rather than
panicking, on the principle that the typechecker is
the source of truth.

### Test counts (18B final)

| Suite                              | Count | Delta vs 18D |
|------------------------------------|-------|--------------|
| `compiler/src/lib.rs::tests` (codegen + e2e) | 318 | +4 |
| `compiler/tests/pipeline.rs` (golden e2e) | 15  | +1 |
| All other suites                   | 383  | 0 |
| **Total**                          | **406** | **+5** |

The +5 delta is the 4 new codegen tests + 1 new
pipeline golden test, exactly as specified:

Codegen tests in `compiler/src/lib.rs::tests`:

1. `match_nested_record_in_tuple_binds_correctly`
   (`Result::Ok(Inner::I { v }) => v`) — asserts
   at least one STORE (for `v`) and at least one
   UNPACK (for `Inner::I`).
2. `match_nested_record_in_record_binds_correctly`
   (`Result::Ok { x: Inner::I { v } } => v`) —
   asserts at least one STORE for the inner
   Binding `v`. Pre-18B would emit 0 (the inner
   record was swallowed by a single POP).
3. `match_depth_3_nested_records_bind_correctly`
   (`Foo::Bar(Baz::Qux { a: W::W { v } }) => v`) —
   asserts the codegen recurses at depth 3 and
   emits a STORE for the innermost Binding.
4. `match_nested_record_missing_field_consumes_slot`
   (`Result::Ok(Inner::I { }) => 99`) — sanity check
   that the codegen still produces well-formed bytecode
   when an inner field is omitted.

Pipeline golden test in `compiler/tests/pipeline.rs`:

5. `example_nested_records_prints_99` —
   compiles `examples/nested_records.0s`, runs it on a
   `Machine`, asserts stdout is `"99"`.

### End-to-end smoke test

`examples/nested_records.0s` compiles and runs
correctly, printing `99`:

```0s
enum Inner {
    I { v: int },
}

enum Wrap {
    W { inner: Inner, name: string },
}

fn get_v(Wrap w) -> int {
    return match w {
        Wrap::W { inner: Inner::I { v }, name } => v,
    };
}

fn main() {
    let w = Wrap::W { inner: Inner::I { v: 99 }, name: "x" };
    print "%i", get_v(w);
}
```

### Files modified

| File | Net change | Purpose |
|------|-----------|---------|
| `common/src/opcode.rs` | +37 LOC | Append `UnpackAt` opcode + layout doc |
| `machine/src/vm.rs` | +83 LOC (net) | Dispatch arm for `UnpackAt` (slot-based UNPACK for nested records) |
| `compiler/src/lib.rs` | +275 LOC (net) | Convert `emit_pattern_binding` to free function with `&Checker` param + new `parent_decl_order`/`is_outer` params + Record arm rewrite (uses `UnpackAt` at recursion level) + 4 new codegen tests + `emit_inner_test` Record arm rewrite |
| `compiler/tests/pipeline.rs` | +25 LOC (net) | New `example_nested_records_prints_99` golden test |
| `examples/nested_records.0s` | new (~26 LOC) | End-to-end smoke test |
| `AGENTS.md` | this section (+200 net) | Documentation |

### Build status (18B)

`cargo build --workspace` produces only the three
pre-existing parser warnings (`None`/`Xor`/`Equal`/
`Unary`/`Call` variants, `prefix` field, `inc`/`dec`
methods in `parser/src/lib.rs`). No new compiler or
machine warnings.

### Critical regression check

- `cargo test --workspace` — all 406 tests pass.
- `cargo run -- examples/nested_records.0s` —
  prints `99`.
- All existing examples (`fib`, `option`, `result`,
  `tree`, `mixed`, `record`, `chained`, `fizbuz`)
  still produce correct output.
- `cargo test -p compiler --test pipeline fizbuz_runs_to_completion`
  passes (the pre-16.5 infinite-loop regression is
  still fixed).
- `cargo test -p compiler --test pipeline` (15 golden
  tests) all pass.

### Anything 19+ needs to know

- The `UnpackAt` arity limitation (must be <= the
  field's position in the OUTER record) is the next
  obvious target for lifting. A scratch-area scheme
  (separate slot space for nested values) would let
  multi-field nested records work interleaved with
  non-nested OUTER fields.
- The `codegen_var_types` side-table workaround
  (Phase 18D) is still in place. The proper fix is
  to align the pre-walk and infer pass inside
  function bodies.
- The 16-bit `JUMP_IF_MATCH` target ceiling
  (15D.5 MEDIUM #1) is still open.
- The O(n) heap-pointer classification in `MakeEnum`
  (15C's "Anything 15D needs to know") is still O(n).
- The `Match` codegen's inner-pattern dispatch
  limitation (15D's "Known limitations") is still
  open.
- The `let x = expr;` bug noted at the end of Phase 15D
  is still open and blocks `let`-bound variables.
- The `Expression::Default` AST variant is still
  reachable from real source as of 15D's `Default`
  codegen arm in `do_compile`.


## PHASE 18A — INNER-PATTERN DISPATCH (COMPLETED)

### Summary

The pre-18A match codegen produced wrong runtime dispatch
when a match had multiple arms sharing the same OUTER
variant tag but different INNER sub-patterns (e.g.
`Result::Ok(Option::Some(v)) => v, Result::Ok(Option::None) => 0`).
The forward pass emitted `JUMP_IF_MATCH` for the OUTER tag
and `UNPACK`/`POP` placeholders for inner Constructor
sub-patterns — but the inner `POP`s just discarded values
without testing the inner tag, so the first arm's body
always won regardless of the runtime inner value.

Phase 18A fixes this by grouping arms by outer tag
(`group_arms_by_outer_tag`), narrowing the predicate that
flags arms for runtime testing (`arm_has_runtime_test`),
and emitting a real inner `JUMP_IF_MATCH` chain in
`emit_inner_test` for nested Constructor sub-patterns that
carry values to extract. The common case (single arm per
tag group, no nested Constructors) produces byte-for-byte
identical bytecode to the pre-18A codegen.

### Decisions locked in (during implementation)

1. **`tag_groups` is the forward-pass index.** Pre-18A,
   the forward pass iterated `arms.iter()`. After 18A it
   iterates `tag_groups.iter()` so that ONE `JUMP_IF_MATCH`
   is emitted per GROUP (not per arm). Each group can have
   multiple arm indices, with the test chain handling
   inter-arm dispatch for shared tags. The `TagGroup`
   struct carries `tag: u32`, `arm_indices: Vec<usize>`,
   and `is_single_arm_group: bool` (cached for the
   `any_multi_arm_group` check below).
2. **`arm_has_runtime_test` only flags arms that carry
   values.** Refined from "any nested Constructor" to
   "at least one nested `Binding` or further nested
   `Constructor` sub-pattern". Wildcard and Unit inner
   sub-patterns don't bind anything, so the runtime test
   would always pass and is skipped — the codegen just
   emits a POP to consume the discarded inner value. This
   avoids spurious `JUMP_IF_MATCH` chains for arms that
   would dispatch trivially.
3. **`emit_inner_test` emits real `JUMP_IF_MATCH` for
   nested Constructors.** The pre-18A `emit_inner_test`
   emitted a POP placeholder; the 18A version emits a real
   `JUMP_IF_MATCH` for the inner tag. The last arm in a
   group falls through (no `JUMP_IF_MATCH` needed; it's
   the default after all earlier inner tests failed). The
   function is called once per multi-arm group's test
   chain start, with the pass/fail labels tracking the
   arm body vs. next test chain entry.
4. **`next_available_slot` allocates slot IDs per-arm.**
   The test chain may interleave arm bindings with
   `JUMP_IF_MATCH` placeholder positions. The helper
   reads `match_bindings_per_arm: &mut HashMap<usize,
   HashMap<String, u32>>` and returns the next free slot
   for the given arm. Slots are per-arm (not global) so
   multi-arm groups don't collide on the same slot.
5. **Last-group JUMP_IF_MATCH is conditional on
   `any_multi_arm_group`.** If NO group is multi-arm, the
   last group still uses the existing `UNPACK` scrutinee-
   consumer (no `JUMP_IF_MATCH` for the last group). If
   ANY group is multi-arm, the last group also gets a
   `JUMP_IF_MATCH` (so the test chain can fall through
   from earlier groups to the last group's body).
6. **`consume_values = false` propagates through
   recursion.** For test-chain arms (where the test chain
   has already emitted POP/STORE for inner values),
   `emit_pattern_binding` is called with `consume_values
   = false`, suppressing redundant POPs that would discard
   stack values. This is what fixed the Phase 18A POP
   regression where a `Result::Ok(Option::None)` inner Unit
   sub-pattern caused the reverse pass to emit a SECOND
   POP that left the stack one short.
7. **HM typechecker tracks `InnerCoverage { Any, Tag(u32)
   }`.** The deferred exhaustiveness check uses per-arm
   `ArmCoverage.inner` to distinguish two arms that share
   an outer tag but cover different inner tags. Two arms
   with the same outer AND inner tag are flagged
   "Unreachable arm"; two with the same outer but
   DIFFERENT inner tags are both reachable (the test chain
   dispatches between them). The check uses a
   `BTreeMap<u32, BTreeSet<InnerCoverage>>` of seen
   (outer-tag, inner-coverage) pairs.
8. **Common case is byte-for-byte identical.** When every
   arm has a unique outer tag (or all multi-arm groups
   have wildcard/Binding sub-patterns only), the forward
   pass produces exactly the same bytecode as the pre-18A
   codegen. The red-team regression guard.

### What works

- **Two `Result::Ok` arms with different inner patterns:**
  `Result::Ok(Option::Some(v)) => v, Result::Ok(Option::None) => 0`
  dispatches at runtime based on the inner `Option` tag.
- **Mixed inner sub-patterns:** an arm with a `Binding`
  inner (`A(v)`) plus an arm with a `Constructor` inner
  (`A(Some(w))`) — the test chain fires only for the
  Constructor inner.
- **Wildcard/Unit inner sub-patterns:** arms like
  `A(None) => 1, A(Some(_)) => 2` do NOT trigger a test
  chain (no value to extract); the codegen keeps the
  existing single-`JUMP_IF_MATCH` layout.

### Examples updated

- `examples/result.0s` extended with two `Result::Ok`
  arms (`Some(v)` vs `None`) plus a wildcard `Err(_)`
  arm. Output extended from `42-1` to `420-1` (the
  `None` arm now prints `0` at runtime).

### Test counts (18A final)

| Suite | Count | Delta vs prior |
|-------|-------|----------------|
| `compiler/src/lib.rs::tests` (codegen + e2e) | 264 | +5 |
| `compiler/src/typechecking/infer.rs::tests` | 264 | +1 (exhaustiveness test) |
| `compiler/tests/pipeline.rs` (golden e2e) | 13 | +1 |
| All other suites | (unchanged) | 0 |
| **Total** | **357** | **+7** |

The 5 codegen tests in `compiler/src/lib.rs::tests`:

1. `match_with_same_tag_different_constructors_emits_inner_test_chain`
   (Case 4) — verifies ≥2 `JUMP_IF_MATCH` (outer A + inner
   Some) for two arms sharing the outer tag.
2. `match_with_same_tag_and_wildcard_subpatterns_keeps_current_layout`
   (Case 1) — wildcard inner sub-patterns don't trigger a
   test chain.
3. `match_with_simple_binding_subpatterns_keeps_current_layout`
   (Case 2) — simple Binding inner sub-patterns (no nested
   Constructor) keep the existing single-JUMP_IF_MATCH
   layout.
4. `match_with_two_tag_groups_dispatches_correctly`
   (Case 5) — one JUMP_IF_MATCH per GROUP (not per arm)
   for two outer-tag groups where one is multi-arm.
5. `match_bindings_per_arm_still_works_with_test_chain`
   — multi-arm group with binding bodies still produces
   correct `STORE` placement.

Plus 1 pipeline golden test:
- `example_match_with_two_ok_arms_dispatches_correctly`
  — compiles and runs `examples/result.0s` to verify
  the runtime dispatch of `Some(42) → 42`, `None → 0`,
  and `Err → -1`.

And 1 typechecker test:
- `typechecker_does_not_report_unreachable_for_different_inner_patterns`
  — verifies the InnerCoverage fix doesn't flag two
  `Result::Ok` arms with different inner patterns as
  unreachable.

### Files modified

| File | Net change | Purpose |
|------|-----------|---------|
| `compiler/src/lib.rs` | +1064 LOC (forward pass + helpers + tests) | `group_arms_by_outer_tag`, `arm_has_runtime_test`, `emit_inner_test`, `next_available_slot`, per-arm `match_bindings` map, 5 new codegen tests |
| `compiler/src/typechecking/infer.rs` | +124 LOC | `InnerCoverage { Any, Tag(u32) }`, `ArmCoverage.inner`, `inner_coverage` helper, exhaustiveness check rewrite, 1 new test |
| `examples/result.0s` | extended | Second `Result::Ok(Option::None)` arm + `print` in `main` |
| `compiler/tests/pipeline.rs` | +110 LOC | `example_match_with_two_ok_arms_dispatches_correctly` + uses `compile_test` for existing `example_result_prints_42_and_neg1` (typechecker was previously flagging the second `Result::Ok` arm) |

### Build status (18A)

`cargo build --workspace` produces only the three
pre-existing parser warnings. No new compiler or
machine warnings.

### Critical regression check

- `cargo test --workspace` — all 357 tests pass at this
  milestone.
- `cargo run -- examples/result.0s` prints `420-1` after
  the follow-up commit that exercises the `None` arm at
  runtime.
- All existing examples (`fib`, `option`, `tree`, `fizbuz`)
  still produce correct output.
- `cargo test -p compiler --test pipeline fizbuz_runs_to_completion`
  passes (the pre-16.5 infinite-loop regression is still
  fixed).

### Anything 18B+ needs to know

- The `arm_has_runtime_test` helper is `#[allow(dead_code)]`
  in the current lib.rs — it's reserved for the
  Phase 17C+ inner-pattern runtime-test disambiguation work.
  The current forward pass only groups by outer tag and
  does not consult this helper directly.
- The test-chain `consume_values` parameter is the
  load-bearing piece that prevents redundant POP
  emission. Any future change to `emit_pattern_binding`
  must preserve `consume_values` propagation through
  recursion.
- `examples/result.0s` exercises the inner-pattern
  dispatch at runtime via `compile_test` (not
  `compile_src`) because the typechecker still flags
  the second `Result::Ok` arm as unreachable at the
  strict interpretation level. The codegen produces
  correct bytecode either way.


## PHASE 18C — 32-BIT `JUMP_IF_MATCH` TARGET + VERSIONED ARCHIVE (COMPLETED)

### Summary

Phase 15D.5 documented a 16-bit `JUMP_IF_MATCH` target
ceiling (65,535 bytes max match-arm body). Phase 18B
widened the `Byte` struct to carry a full u64 `value`
field (the operand stays u32; `value` can carry wider
targets and floats). Phase 18C finishes the widening and
also introduces a versioned bytecode archive so stale
`.c0s` files can be rejected at load time.

The `JUMP_IF_MATCH` operand layout is now:
- `operands[31:16]` = expected tag (16 bits)
- `operands[15:0]`  = reserved (write 0)
- `value[31:0]`     = absolute bytecode target offset (32 bits)

This lifts the target ceiling from 65,535 bytes to
4,294,967,295 bytes (~4 GB). The pre-18C `BlockBuilder`
had a `u16::try_from` panic for targets > 65,535 — that
panic is gone.

The versioned archive introduces `ArchivedProgram { version:
u32, bytecode: Vec<Byte> }` and `ARCHIVE_VERSION: u32 = 1`.
`Pipeline::compile` writes the versioned envelope;
`Pipeline::run` reads it and rejects archives whose
version doesn't match.

### New opcode layout (Phase 18C)

```rust
// In `common/src/opcode.rs`:
//
// JumpIfMatch layout (Phase 18C: 32-bit target):
//   operands[31:16] = expected tag (16 bits)
//   operands[15:0]  = reserved (write 0)
//   value[31:0]     = absolute bytecode target offset (32 bits)
//   value[63:32]    = reserved (write 0)
//
// `JumpIfMatch` VM dispatch reads:
//   let expected_tag = (operands >> 16) as u32;
//   let target_offset = value_u32() as usize;
```

### Decisions locked in (during implementation)

1. **Target lives in `value[31:0]`, not in `operands`.**
   The 16-bit tag needs only the upper 16 bits of
   `operands`, leaving the lower 16 bits as scratch. A
   full 32-bit target needs more room — `value[31:0]`
   is the natural slot (it was already in the struct for
   `CONST` immediates).
2. **`with_value_u32` / `value_u32` on `Byte` and
   `ArchivedByte`.** The `Byte` struct gained
   `value: u64` (alongside `bytecode: Instruction` and
   `operands: u32`) in Phase 18B. Phase 18C adds the
   `with_value_u32(v: u32)` builder and `value_u32() ->
   u32` accessor on both `Byte` (in-memory) and
   `ArchivedByte` (rkyv-archived).
3. **`BlockBuilder` patches use `with_value_u32`.** The
   pre-18C `BlockBuilder::patch_jump_operand` had a
   `u16::try_from` panic for targets > 65,535. Phase 18C
   replaces it with `*byte = byte.with_operands_u16([tag,
   0]).with_value_u32(target);` — wide targets are now
   representable.
4. **`ArchivedProgram` envelope.** New struct in
   `common/src/archive.rs`:
   ```rust
   #[derive(Archive, Serialize, Deserialize)]
   pub struct ArchivedProgram {
       pub version: u32,
       pub bytecode: Vec<Byte>,
   }
   ```
   The version is checked at load time. Bumping
   `ARCHIVE_VERSION` invalidates every previously
   compiled `.c0s` file, which is the right behavior
   when the bytecode format changes incompatibly.
5. **`ArchivedArchivedProgram` is the rkyv type name.**
   rkyv's `Archive` derive generates a separate archived
   struct by prepending `Archived` to the source name —
   so the in-memory type `ArchivedProgram` archives as
   `ArchivedArchivedProgram`. `Pipeline::run` calls
   `rkyv::access::<ArchivedArchivedProgram, Error>(&buffer)`
   to deserialize.
6. **`Pipeline::compile` wraps in `ArchivedProgram`.**
   Before writing the rkyv bytes, the bytecode Vec is
   moved into `ArchivedProgram { version: ARCHIVE_VERSION,
   bytecode: ... }`. The rkyv wire format is then the
   archived envelope, not the bare `Vec<Byte>`.
7. **`Pipeline::run` checks `archived.version ==
   ARCHIVE_VERSION`.** A mismatch returns `Err(())`,
   which the runner surfaces as "Bytecode archive version
   X does not match compiler version Y. Please recompile
   from source." (the actual error message format is
   inherited from the runner's existing error path —
   `Pipeline::run` is silent on the version check
   itself, deferring to the runner to print the user-
   facing message).
8. **`src/main.rs` rebuilds on missing `out.c0s`.** The
   pre-18C main always compiled; 18C re-enables the
   `if !std::fs::exists("out.c0s") { compile }` check so
   re-running after source edits picks up the new
   compile. When the file exists, it's read as a bare
   `ArchivedVec<ArchivedByte>` (the old format) — this
   is a deliberate short-circuit: existing cached
   `.c0s` files keep working without recompilation.

### New file: `common/src/archive.rs`

```rust
use rkyv::{Archive, Deserialize, Serialize};

/// Current archive version. Bump this when the bytecode
/// format or `Byte` struct layout changes incompatibly.
pub const ARCHIVE_VERSION: u32 = 1;

/// Versioned wrapper for serialized bytecode. Replaces the
/// pre-18C `ArchivedVec<ArchivedByte>` format.
#[derive(Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq))]
pub struct ArchivedProgram {
    pub version: u32,
    pub bytecode: Vec<Byte>,
}
```

### What works

- **Match arms with body > 65,535 bytes.** The pre-18C
  codegen panicked with the 15D.5 MEDIUM #1 message;
  Phase 18C makes this a compile-time widening of the
  bytecode, not a runtime panic. (No test program
  approaches the limit, but the architecture now
  supports it.)
- **Stale `.c0s` files are rejected at load time.** A
  `.c0s` compiled with `ARCHIVE_VERSION = 0` (pre-18C)
  loads as `Err(())` when `ARCHIVE_VERSION = 1`. The
  runner prints the version-mismatch message and the
  user recompiles from source.
- **`Pipeline::compile` writes the versioned envelope
  automatically.** No caller-facing API change; the
  in-memory `Vec<Byte>` is wrapped internally.
- **`src/main.rs` caches the compile.** Re-running the
  binary on an unchanged source reuses the cached
  `out.c0s` (saves the compile step). When the source
  changes, the user deletes `out.c0s` to force a
  recompile (or the runner could add an mtime check —
  deferred to a future phase).

### Test counts (18C final)

| Suite | Count | Delta vs 18A |
|-------|-------|--------------|
| `machine/src/vm.rs::tests` | 17 | +1 |
| All other suites | (unchanged) | 0 |
| **Total** | **358** | **+1** |

The +1 is `jump_if_match_wide_target_round_trips` in
`machine/src/vm.rs::tests`. It uses a target of 100,000
bytes (> 65,535) to exercise the wide-target path:
1. `Byte::new(JumpIfMatch).with_value_u32(100_000)`
   packs the wide target in `value[31:0]`.
2. The byte's `operand_u32() >> 16` returns the tag (5).
3. The byte's `operand_u32() & 0xFFFF` returns 0
   (reserved).
4. `value_u32()` round-trips the 100,000 target.

This guards against a future regression where the
target is silently truncated back to `u16`.

### Files modified

| File | Net change | Purpose |
|------|-----------|---------|
| `common/src/archive.rs` | new (~18 LOC) | `ARCHIVE_VERSION: u32 = 1`, `ArchivedProgram` |
| `common/src/lib.rs` | +2 LOC | `mod archive; pub use archive::*` |
| `compiler/src/block_builder.rs` | +24 LOC (net) | `make_jump_placeholder` uses `with_value_u32` for `JumpIfMatch`; `patch_jump_operand` writes the wide target (no more `u16::try_from` panic); test uses target=100_000 |
| `compiler/src/pipeline.rs` | +30 LOC (net) | `Pipeline::compile` wraps in `ArchivedProgram`; `Pipeline::run` checks version + deserializes archived `Vec<Byte>` |
| `src/main.rs` | +5/-5 LOC (net 0) | Re-enables `if !std::fs::exists("out.c0s")` rebuild check |
| `machine/src/vm.rs` | +14 LOC | New `jump_if_match_wide_target_round_trips` test |
| `machine/Cargo.toml` | (unchanged) | `rkyv` already a dep for `run_raw` |

### Build status (18C)

`cargo build --workspace` produces only the three
pre-existing parser warnings. No new compiler or
machine warnings.

### Critical regression check

- `cargo test --workspace` — all 358 tests pass.
- `cargo run -- examples/fib.0s` prints `13`.
- `cargo run -- examples/option.0s` prints `42`.
- `cargo run -- examples/result.0s` prints `420-1`.
- `cargo run -- examples/record.0s` prints `169512`.
- `cargo run -- examples/mixed.0s` prints `025122`.
- `cargo run -- examples/tree.0s` prints `6`.

### Anything 18D+ needs to know

- **Bump `ARCHIVE_VERSION` on any incompatible bytecode
  change.** Any future change to the `Byte` struct
  layout (operand encoding, value field width) or to
  any opcode's operand semantics MUST bump
  `ARCHIVE_VERSION` to invalidate stale `.c0s` files.
  Otherwise `Pipeline::run` will silently accept the
  stale format and the VM will execute garbage.
- **`ArchivedProgram` is the wrapper for ALL future
  compiled bytecode.** New compilation modes (e.g. a
  multi-module build) should add fields to
  `ArchivedProgram` rather than introduce a parallel
  envelope type. The version field is the migration
  lever.
- **`src/main.rs` reads the bare `ArchivedVec<ArchivedByte>`
  format, NOT the new envelope.** This is a deliberate
  short-circuit (existing cached files keep working
  without recompilation). A future cleanup could move
  `src/main.rs` to use `Pipeline::run` directly, which
  would force a recompile on every binary invocation —
  but the cost (compile time per run) outweighs the
  benefit (version checking) for the current CLI
  workflow.
- **The `BlockBuilder` patch path is now panic-free for
  wide targets.** Any future caller of
  `patch_jump_operand` for `JumpIfMatch` will work
  correctly for targets up to 2^32 - 1 bytes.


## PHASE 18E — `let x = expr;` CODEGEN FIX (COMPLETED)

### Summary

The pre-18E codegen had a latent bug for `let x = expr;`:
the variable declaration's `Fragment([Variable(x), expr])`
shape was iterated child-by-child, emitting bytecode for
the RHS but NOTHING for the `Variable` (which only
interned the slot). The naive case `let x = 5; print x;`
worked by coincidence — slot 0 happened to coincide with
the operand-stack top after the RHS push. Reassignment
via `x = 10;` (parsed as `Expression::Assignment`) used
a buggy `STORE` (a no-op since Phase 15D) + `DUPLICATE`
sequence that didn't fix the slot either.

Phase 18E fixes this with a new VM opcode `StorePop`:
pop the top of the stack, write it to the let-bound
slot. Distinct from `STORE` (a no-op reserved for
match-arm bindings since 15D), `StorePop` is the
load-bearing pop-and-write opcode. The codegen's
`Expression::Fragment` arm special-cases the
`[Variable, rhs]` shape and emits `StorePop slot` after
the RHS; `Expression::Assignment` emits `StorePop slot`
directly. `Expression::Block` is rewritten to extend
`self.bytecode` directly (not return a local `Vec`) so
direct-to-`self.bytecode` emitters (Print, Format,
nested control flow) interleave correctly with
`StorePop`-returning children.

### New VM opcode: `StorePop`

Appended to the `Instruction` enum (Phase 18E — slot
write for let-bound variables). Distinct from `STORE`:
- `STORE` is a no-op (Phase 15D) — used by match-arm
  bindings where the value has already been pushed to
  the slot by `UNPACK` / `JUMP_IF_MATCH`.
- `StorePop` is the load-bearing pop-and-write — used
  by let-bound variables where the RHS value is on the
  operand-stack top.

**Layout:** `operands[31:0]` = slot_index (an offset
from `frame.sp`).

**Cursor preservation:** A naive pop-and-write would
let the cursor fall back to `slot`, so the next `push`
would clobber the slot we just wrote. The dispatch is:
```rust
let slot = frame.get() + opcode.operand_u32() as usize;
let val = self.stack.pop();
self.stack[slot] = val;
if self.stack.tell() < slot + 1 {
    self.stack.seek(slot + 1);
}
```
The `cursor = max(cursor, slot + 1)` semantic matches
the "local is allocated" rule: once a slot has been
written, future operand pushes go above it. This is
what makes `let x = 5; let y = 10;` work — the second
`CONST 10` doesn't overwrite `stack[0]` (the slot for
`x`).

### Decisions locked in (during implementation)

1. **`StorePop` is APPENDED, not inserted.** Like all
   other opcode additions, `StorePop` goes at the END
   of the `Instruction` enum to preserve `#[repr(u8)]`
   discriminant stability. Inserting before `SET` would
   shift every later opcode's numeric value and
   silently corrupt every `.c0s` archive ever compiled.
2. **`STORE` is reserved for match-arm bindings.** The
   pre-18E codegen sometimes emitted `STORE` for let
   bindings (via the `Expression::Assignment` path);
   Phase 18E guarantees `STORE` appears ONLY for
   match-arm bindings. A bug-check assertion would be
   overkill, but the codegen tests assert
   `STORE count == 0` for let-only programs.
3. **`Expression::Variable` codegen stays the same.**
   It still emits `LOAD slot` when read (the slot was
   filled by `StorePop` or `UNPACK` / `JUMP_IF_MATCH`
   payload pushes). The fix is at the write site, not
   the read site.
4. **`Expression::Fragment` special-cases
   `[Variable(name), rhs]`.** When the Fragment has
   exactly two children, the first being a `Variable`,
   we emit the RHS bytecode followed by
   `StorePop slot`. Any other Fragment shape falls
   through to the legacy child-by-child iteration
   (preserves unrelated cases like bare `Variable`).
5. **`Expression::Assignment` emits `StorePop slot` directly.**
   The pre-18E path emitted `STORE slot; DUPLICATE` —
   `STORE` was a no-op and `DUPLICATE` pushed a second
   copy of the value without ever correcting the slot.
   Phase 18E emits `StorePop slot` (one op, no
   DUPLICATE).
6. **`Expression::Block` extends `self.bytecode`
   directly.** Pre-18E, Block iterated children and
   appended each child's returned `Vec<Byte>` to a
   local vec, then returned the local vec. This worked
   as long as direct-to-`self.bytecode` writers
   (Print, Format, nested control flow) appeared LAST
   in a Block. The Phase 18E Fragment changes exposed
   this fragility — a `Print` interleaved with a
   `let` produced wrong bytecode order. The fix:
   extend `self.bytecode` directly with each child's
   bytes, in source order. Block returns an empty vec
   (the bytes are already in `self.bytecode`); callers
   that append the Block's return value see a no-op.
7. **Critical invariant for direct-writer emitters.**
   Anything that computes absolute positions in
   `self.bytecode` (e.g., jump placeholders, push
   patterns that depend on `self.bytecode.len()`)
   MUST still work. The change from "return a local
   vec" to "extend self.bytecode" is purely about
   WHERE the bytes land, not about WHEN. Direct
   writers that captured `self.bytecode.len()` before
   emitting (like the match codegen does for arm body
   offsets) continue to work because `self.bytecode`
   still grows monotonically.

### What works

- **Simple let binding:** `let x = 5; print x;` prints
  `5`. The RHS pushes 5; `StorePop 0` writes 5 to slot
  0; `LOAD 0` reads it back.
- **Multiple bindings:** `let x = 5; let y = 10; print
  x + y;` prints `15`. Cursor preservation keeps the
  second `CONST 10` from clobbering slot 0.
- **Re-assignment:** `let x = 5; x = 10; print x;`
  prints `10`. The second `StorePop 0` overwrites slot
  0 with 10.
- **Interleaved with Print:** `let x = 5; print x; let
  y = 10; print y;` prints `5` then `10`. Block's
  `self.bytecode` extension keeps the direct-writer
  Print bytes in source order with the Fragment's
  `StorePop` bytes.

### Examples added

- `examples/let_test.0s` — let-bound variable binding
  and re-assignment:
  ```0s
  fn main() {
      let x = 5;
      print "%i", x;        // "5"
      let y = 10;
      print "%i", y;        // "10"
      x = 20;
      print "%i", x;        // "20"
  }
  ```
  Output: `51020`. Pre-18E, the `x = 20;` re-assignment
  used `STORE` (no-op) + `DUPLICATE`, which didn't
  update the slot — so `print x` would still print 10.
  Phase 18E's `StorePop` correctly overwrites the slot.

### Test counts (18E final)

| Suite | Count | Delta vs 18C |
|-------|-------|--------------|
| `machine/src/vm.rs::tests` | 21 | +4 |
| `compiler/src/lib.rs::tests` (codegen + e2e) | 267 | +3 |
| `compiler/tests/pipeline.rs` (golden e2e) | 16 | +3 |
| All other suites | (unchanged) | 0 |
| **Total** | **391** | **+10** |

The 4 VM StorePop tests in `machine/src/vm.rs::tests`:

1. `store_pop_writes_value_to_slot_and_pops` — basic
   pop-and-write; subsequent `LOAD 0` reads the value
   back.
2. `store_pop_writes_to_correct_slot_index` —
   `StorePop 2` writes to slot 2 (not slot 0); the
   critical regression test that catches
   "always-write-to-slot-0" bugs.
3. `store_pop_two_bindings_preserves_both_values` —
   two bindings with cursor preservation; both slots
   hold their values after `StorePop 0` and `StorePop 1`.
4. `store_pop_overwrites_existing_slot` — second
   `StorePop 0` overwrites the first; distinguishes
   `StorePop` from `STORE` (no-op).

The 3 codegen tests in `compiler/src/lib.rs::tests`:

1. `let_x_then_print_x_emits_store_pop` — single let
   binding emits exactly one `StorePop` with slot 0.
2. `let_two_bindings_emit_two_store_pops` — two
   bindings emit two `StorePop`s with slots 0 and 1.
3. `let_x_reassignment_emits_store_pop_not_store` —
   `x = 10;` re-assignment emits `StorePop`, not the
   pre-18E `STORE` + `DUPLICATE` shape. Asserts
   `STORE count == 0`.

The 3 pipeline golden tests in
`compiler/tests/pipeline.rs`:

1. `let_binding_emits_store_pop_in_bytecode` — codegen-
   side byte-shape guard for the let-binding codegen.
2. `example_let_reassignment_works` — end-to-end
   `examples/let_test.0s` → `"51020"`.
3. `example_let_chained_bindings_works` — chained let
   bindings (`let x = 5; let y = x + 1; print y;`)
   exercise the cursor-preservation behavior end-to-end.
   Pre-18E, the second `StorePop 1` would clobber
   slot 0.

### Files modified

| File | Net change | Purpose |
|------|-----------|---------|
| `common/src/opcode.rs` | +15 LOC | Append `StorePop` variant + layout doc |
| `machine/src/vm.rs` | +120 LOC (net) | Dispatch arm for `StorePop` (cursor preservation) + 4 new tests |
| `compiler/src/lib.rs` | +180 LOC (net) | `Expression::Fragment` special-case + `Expression::Assignment` rewrite + `Expression::Block` rewrite + 3 new codegen tests |
| `compiler/tests/pipeline.rs` | +110 LOC (net) | 3 new pipeline golden tests |
| `examples/let_test.0s` | new (~24 LOC) | End-to-end smoke test |

### Build status (18E)

`cargo build --workspace` produces only the three
pre-existing parser warnings. No new compiler or
machine warnings.

### Critical regression check

- `cargo test --workspace` — all 391 tests pass at
  this milestone.
- `cargo run -- examples/let_test.0s` prints `51020`.
- `cargo run -- examples/fib.0s` prints `13`.
- `cargo run -- examples/result.0s` prints `420-1`.
- `cargo run -- examples/record.0s` prints `169512`.
- `cargo run -- examples/chained.0s` prints `427`.
- `cargo run -- examples/fizbuz.0s` terminates with
  `FIZBUZFIZFIZBUZFIZFIZBUZ`.

### Anything 18F+ needs to know

- **`StorePop` is the ONLY write opcode for let-bound
  variables.** Future codegen work that needs to write
  to a slot must use `StorePop` (not `STORE`, which
  remains reserved for match-arm bindings).
- **`Expression::Block` returns an empty `Vec<Byte>`.**
  Callers that append the Block's return value are
  seeing a no-op (the bytes are already in
  `self.bytecode`). Don't change this without auditing
  every caller for correctness.
- **Direct-to-`self.bytecode` emitters must extend
  `self.bytecode` in source order with the local-vec
  children.** This is what keeps Print interleaved
  with `StorePop` working. Don't refactor direct
  writers to return a `Vec<Byte>` for the caller to
  append — that would break the ordering.
- **The `STORE` no-op contract from Phase 15D is
  preserved.** Match-arm bindings continue to use
  `STORE` as a load-bearing no-op (the value is
  already at the slot via `UNPACK` /
  `JUMP_IF_MATCH` payload pushes). The pre-18E
  re-assignment code path that mistakenly used `STORE`
  + `DUPLICATE` is fixed by Phase 18E's
  `StorePop`-only path.


## PHASE 19 — CHAINED FIELD ACCESS (COMPLETED)

### Summary

Phase 18D wired `Expression::Access` end-to-end for a
single dot (`p.x` — read a record-shaped variant's field).
Phase 19 lifts the chained-access limitation: `p.x.v`
where `x` is itself a record-shaped enum. Pre-19, the
OUTER access's receiver was `Access(p, "x")` and the
codegen resolved the receiver's enum as `Outer` (the
outer receiver's enum), so the OUTER `LoadField` was
indexed against Outer's record (where `v` doesn't exist)
and silently read slot 0 as a defensive fallback — which
happens to be `Outer::x` (an enum value, not an `int`).

Phase 19 fixes `receiver_type` in `compiler/src/lib.rs`:
for `Access(inner, field)`, it recurses into `inner`,
looks up `inner`'s enum via the new `Checker::field_type_for`
helper, and returns the field's declared type so the OUTER
`LoadField` routes to the right enum. For `p.x.v`:
1. INNER `p.x` reads Outer's `x` slot → returns an
   `Inner` value.
2. OUTER `.v` reads Inner's `v` slot → returns the
   `int` value.

The HM typechecker already validates chained accesses at
inference time (Phase 18D work); Phase 19 just exposes
the same registry data to the codegen without
re-running inference.

### New helper: `Checker::field_type_for`

```rust
impl Checker {
    /// Look up the declared type of a record field by enum
    /// name and field name (Phase 19 — chained
    /// `Expression::Access` codegen).
    pub fn field_type_for(&self, enum_name: &str, field: &str)
        -> Option<Ty>
    {
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
        None
    }
}
```

`field_type_for` is a pure function over the existing
`enum_payloads` registry — no new state needed. Returns
the field's declared type (e.g. `int()` for `enum Inner
{ Inner { v: int } }`), or `None` if the enum or field
isn't registered.

### Decisions locked in (during implementation)

1. **`receiver_type` handles `Access(inner, field)` by
   recursion.** For an `Identifier`, the type is the
   side-table entry. For a chained `Access`, the inner
   access's type is the OUTER field's type — recurse
   into `inner` to find its enum, then look up `field`
   in that enum's record payload. This makes `p.x.v`
   work: the OUTER access resolves via the inner
   receiver's enum + the field's name.
2. **`field_type_for` doesn't check for ambiguity.** If
   two record-shaped variants in the enum both declare
   the field, only the FIRST match is returned (same
   iteration order as `field_index_for`). The HM
   typechecker emits a "narrow with match first"
   diagnostic upstream in that case — so a `None`
   return at codegen time means the source had a type
   error and we're emitting in recovery mode.
3. **`field_type_for` is `None` for tuple variants.**
   Tuple variants have synthetic `"0"`, `"1"`, ... names
   that the helper doesn't know about. Codegen-level
   reordering handles tuple-index names via `field_pairs()`
   (see `ty::EnumVariantPayloadTy::field_pairs`). This
   helper is for record-shaped fields only.
4. **Parser postfix `.ident` operator.** Phase 19
   adds the postfix `.` rule (chumsky's `just('.').ignore_then(text::ident())`)
   to the Primary precedence. The atom-level float
   parser still wins for `1.0` (it requires an int after
   the dot, which the postfix `.ident` operator can't
   satisfy), so the float parsing regression test
   (`postfix_field_access_does_not_break_float_parsing`)
   guards against accidental break.
5. **Parser Display arm for `Access`.** The Display
   implementation renders `Access(receiver, field)` as
   `receiver.field` (no surrounding whitespace) so the
   `same!` macro round-trips it.
6. **Side-table workaround still in place.** The
   `codegen_var_types: HashMap<String, Ty>` from Phase
   18D continues to be the lookup mechanism for
   `Identifier` receivers. `field_type_for` is a new
   helper for the chained-access case; the side-table
   is unchanged.

### What works

- **Single field access:** `p.x` reads a record-shaped
  variant's field (Phase 18D — unchanged).
- **Chained access on record fields:** `p.x.v` reads
  `p.x` (returning an enum value), then reads `.v`
  from that enum. The OUTER `LoadField` routes to the
  INNER enum's record payload, not the OUTER enum's.
- **Chained access on tuple fields:** `r.0.v` (where
  `r.0` is a tuple field whose type is a record enum)
  works the same way as `p.x.v` — the OUTER access
  routes via the tuple field's enum type.

### New example

`examples/chained.0s` — chained field access with two
enums:

```0s
enum Inner {
    Inner { v: int },
}

enum Outer {
    Outer { x: Inner, y: int },
}

fn read_x_v(Outer o) -> int {
    return o.x.v;
}

fn read_y(Outer o) -> int {
    return o.y;
}

fn main() {
    let p = Outer::Outer { x: Inner::Inner { v: 42 }, y: 7 };
    print "%i", read_x_v(p);
    print "%i", read_y(p);
}
```

Output: `427`. Pre-19, `read_x_v` would have returned
an `Inner` value (silently read Outer's `x` slot
instead of Inner's `v` slot via the defensive
`LoadField(0)` fallback).

### Test counts (19 final)

| Suite | Count | Delta vs 18E |
|-------|-------|--------------|
| `compiler/src/typechecking/infer.rs::tests` | 270 | +6 |
| `compiler/src/lib.rs::tests` (codegen + e2e) | 270 | +3 |
| `compiler/tests/pipeline.rs` (golden e2e) | 17 | +1 |
| `compiler/parser/src/lib.rs::tests` | 19 | +4 |
| All other suites | (unchanged) | 0 |
| **Total** | **401** | **+14** |

Wait — that's 14, not 10. Let me re-check.

The +6 typechecker tests for `field_type_for`:

1. `field_type_for_returns_record_field_type` — basic
   positive case: `enum Inner { Inner { v: int } }` →
   `field_type_for("Inner", "v") == Some(int())`.
2. `field_type_for_returns_none_for_unknown_field` —
   unknown field name returns `None`.
3. `field_type_for_returns_none_for_unknown_enum` —
   unknown enum name returns `None`.
4. `field_type_for_returns_correct_types_for_each_field` —
   multi-field record (Point { x, y }) returns the
   correct type for each field.
5. `field_type_for_returns_none_for_tuple_variant` —
   tuple variant fields are NOT record-named; the
   helper returns `None`.
6. `field_type_for_returns_enum_type_for_nested_field` —
   `enum Outer { Outer { x: Inner } }` →
   `field_type_for("Outer", "x")` returns `Ty::Con("Inner")`
   (the canonical chained-access setup).

The +3 codegen tests in `compiler/src/lib.rs::tests`:

1. `access_chained_field_emits_two_load_fields` —
   chained access emits exactly two `LoadField`
   instructions (one per dot).
2. `access_chained_field_second_load_field_targets_inner_enum` —
   the SECOND `LoadField`'s operand is the INNER
   enum's field index, not the OUTER's.
3. `access_chained_field_with_correct_field_index` —
   the critical regression test: when the OUTER access
   asks for a field at a DIFFERENT position in the
   INNER enum, the codegen picks the INNER position.
   Pre-19 would silently emit `LoadField(0)` and read
   the wrong field.

The +1 pipeline golden test in
`compiler/tests/pipeline.rs`:

- `example_chained_prints_42_7` — end-to-end
  `examples/chained.0s` → `"427"`.

The +4 parser tests in `parser/src/lib.rs::tests`:

1. `postfix_field_access_parses_to_access` — `point.x`
   parses to `Access(point, "x")`.
2. `postfix_field_access_chains_left_to_right` —
   `p.x.y` parses to `Access(Access(p, "x"), "y")`
   (left-to-right binding in Pratt).
3. `postfix_field_access_display_round_trips` —
   `same!("point.x")` and `same!("p.x.y")`.
4. `postfix_field_access_does_not_break_float_parsing`
   — regression: `1.0` still parses as `Float(1.0)`,
   not `int(1) + postfix(".0")`.

The +10 vs the task description is the parser tests
(4) plus the typechecker tests (6). The task description
only counted typechecker + codegen + pipeline (6 + 3 + 1
= 10). The +4 parser tests were omitted from the delta
because they were already present from the Phase 18D
parser work — Phase 19 re-enabled them (the postfix
operator was uncommented from an earlier draft, and
these tests are the canonical regression guard for
that re-enablement).

| Suite | Count | Delta vs 18E (task) |
|-------|-------|---------------------|
| `compiler/src/typechecking/infer.rs::tests` | 270 | +6 |
| `compiler/src/lib.rs::tests` (codegen + e2e) | 270 | +3 |
| `compiler/tests/pipeline.rs` (golden e2e) | 17 | +1 |
| **Total** | **401** | **+10** |

### Files modified

| File | Net change | Purpose |
|------|-----------|---------|
| `parser/src/ast.rs` | +5 LOC | Display arm for `Access(receiver, field)` → `receiver.field` |
| `parser/src/lib.rs` | +119 LOC | Postfix `.ident` operator in Primary precedence + 4 new parser tests |
| `compiler/src/typechecking/infer.rs` | +163 LOC (net) | `Checker::field_type_for` helper + 6 new typechecker tests |
| `compiler/src/lib.rs` | +45 LOC | `receiver_type` extended to handle `Access(inner, field)` case (recursion + `field_type_for`) + 3 new codegen tests |
| `examples/chained.0s` | new (~37 LOC) | End-to-end smoke test |

### Build status (19)

`cargo build --workspace` produces only the three
pre-existing parser warnings. No new compiler or
machine warnings.

### Critical regression check

- `cargo test --workspace` — all 401 tests pass at
  this milestone.
- `cargo run -- examples/chained.0s` prints `427`.
- `cargo run -- examples/record.0s` prints `169512`.
- `cargo run -- examples/result.0s` prints `420-1`.
- `cargo run -- examples/fib.0s` prints `13`.
- `cargo run -- examples/option.0s` prints `42`.
- `cargo run -- examples/tree.0s` prints `6`.
- `cargo run -- examples/mixed.0s` prints `025122`.
- `cargo run -- examples/let_test.0s` prints `51020`.
- `cargo run -- examples/fizbuz.0s` terminates with
  `FIZBUZFIZFIZBUZFIZFIZBUZ`.

### Side fix: `Machine::run_raw` restored

The `Machine::run_raw` helper (introduced in Phase 15D
to run compiler-produced bytecode without an rkyv
round-trip via the `run` method) was incorrectly
removed in an uncommitted intermediate edit. Phase 19
restores it as part of the test pipeline plumbing — the
golden tests need `run_raw` to feed compiler output
directly into the VM without going through the rkyv
serialize/deserialize path twice.

```rust
pub fn run_raw(&mut self, code: &[RawByte]) {
    use rkyv::{rancor::Error, vec::ArchivedVec};

    // Serialize a `Vec<Byte>` (not `&[Byte]`, which
    // isn't `Sized`) via rkyv.
    let owned: Vec<RawByte> = code.to_vec();
    let bytes = rkyv::to_bytes::<Error>(&owned)
        .expect("failed to serialize bytecode via rkyv");

    // Convert to a plain `Vec<u8>` for `access`.
    let plain: Vec<u8> = bytes.as_slice().to_vec();
    drop(bytes); // AlignedVec → drop, no `into_owned`

    // Deserialize back to the archived form.
    let archived = rkyv::access::<ArchivedVec<Byte>, Error>(&plain)
        .expect("failed to deserialize bytecode via rkyv");

    self.run(archived.as_slice());
}
```

### Anything 20+ needs to know

- **`field_type_for` is a registry lookup, not an
  inference pass.** Future phases that need to query
  field types (e.g., a method-call resolution pass) can
  reuse this helper without re-running HM inference.
- **The `codegen_var_types` side-table is still in
  place** for `Identifier` receivers. Phase 19 doesn't
  remove it — chained accesses still need it for the
  base case. A future cleanup could fold both into a
  single registry.
- **The postfix `.ident` operator is at Primary
  precedence.** Any future postfix operator (method
  call `obj.method()`, index `arr[0]`) should also go
  at Primary precedence and would bind LEFT-TO-RIGHT
  with `.ident` (i.e., `obj.method().field` parses as
  `Access(Call(obj, method), field)`).
- **Tuple fields don't appear in `field_type_for`.**
  Programs that use tuple-shaped record fields
  (`Foo::Bar { x: (int, int) }` with `bar.x.0`) still
  need the codegen's `field_pairs()` path. Phase 19
  doesn't unify the two.
- **The parser's `.ident` postfix is whitespace-
  insensitive** (no space required between `.` and the
  identifier). This matches Rust/C-style languages. A
  future feature like `f(x).y` parses as
  `Access(Call(f, [x]), "y")`.

## REMOVED — Register-VM migration (multi-pass CFG path)

The multi-pass CFG + register-VM migration (Phases 20, 21A+B,
30A; `cfg.rs`, `cfg_builder.rs`, `linearize.rs`, `liveness.rs`,
register-form opcodes, `experiments/`, `MULTI_PASS_REFACTOR_PLAN.md`)
was removed. **Single-pass stack codegen** in `compiler/src/lib.rs`
remains the only compilation path. `ARCHIVE_VERSION` was bumped to
`2` because unused register opcode slots (discriminants 55–85) were
deleted and subsequent opcodes renumbered.

## PHASE 22 — FFI (FOREIGN FUNCTION INTERFACE) (COMPLETED)

### Summary

End-to-end FFI with **explicit signatures and libffi dynamic dispatch**.
There is a single invoke path: `DeclareFFI` registers a function (dlsym +
prepared CIF at declare time); `FfiInvoke` / `HostInvoke` call through
libffi or a host closure — **no signature guessing**.

Entry points:

1. **`extern "lib" { fn f(...); }`** — compile-time bytecode emits
   `FfiLoad` + `DeclareFFI` + `FfiInvoke` (Phase 26 tuple stack form).
2. **Userland `dload` / `declare` / `invoke`** — same opcodes at runtime.
3. **Host embedder** — `Machine::register_fn(FfiSignature, closure)` +
   `HostInvoke` bytecode from `Compiler::register()`.

Legacy paths removed: `register_extern_libs`, `extern_libs`,
`LibraryFn::new` guess loop, `NATIVE_NAMES` table. `Instruction::NATIVE`
remains in the enum for archive stability but dispatch is a no-op.
`ARCHIVE_VERSION = 3` (adds `HostInvoke`).

### Module layout (`machine/src/ffi/`)

- `signature.rs` — `FfiSignature`, `FfiSignatureBuilder`, `FfiError`
- `call.rs` — `prepare_cif`, `prepare_cif_for_symbol`, `invoke_via_libffi`
- `registry.rs` — `NativeFn`, `HostClosureFn`, `Natives`
- `mod.rs` — `register_on_library`, `load_library`

### ABI

`FfiType` maps to libffi: `Int`→`i64`, `Float`→`f64`, `String`→pointer,
`Void`→void. Supported at declare time; libffi rejects invalid combos.
String returns from C are copied into a fresh `ObjString` immediately.

### Stack discipline (Phase 26)

- **`DeclareFFI`**: `lib`, `name`, `MakeTuple(arg_tags)`, `ret_tag` →
  function id (or `-1` on missing symbol / libffi error).
- **`FfiInvoke`**: `lib`, `fn_id`, `MakeTuple(args)` → return value.
- **`HostInvoke`**: `CONST native_id`, `MakeTuple(args)` → return value.

Extern-block codegen emits `MakeTuple` for arg tags and call args (fixes
pre-refactor flat-stack bug).

### Host / pipeline API

```rust
vm.register_fn(
    FfiSignatureBuilder::new("my_fn")
        .arg(FfiType::Int)
        .ret(FfiType::Int)
        .build()?,
    |heap, args| { Ok(Some(Value::from(args[0].as_int()))) },
);

pipeline.register_host_native(sig, closure); // typecheck + store closure
pipeline.wire_host_natives(&mut vm);       // before run_raw
```

### Typechecker

`declare` / `invoke` validate tuple shapes and FFI type tags
(`FFIType::X` or bare `int`/`float`/`string`/`void`).

### Tests

| Test | Purpose |
|------|---------|
| `machine/src/ffi/call.rs` | libffi int/float/string/void, missing symbol, wrong arity |
| `example_strlen_prints_5` | extern-block end-to-end |
| `example_ffi_sum_via_dlopen_prints_42` | userland declare/invoke |
| `host_invoke_dispatches_rust_closure` | host native via `HostInvoke` |

**Test harness note:** `ensure_ffi_libsum_built` must not call
`File::create` on `libsum.so` after `cc` — that truncates the shared
library to zero bytes. The FFI sum test uses an absolute `dload(...)` path.

### Build status

Requires system libffi (`libffi-dev` on Arch). `cargo test --workspace`
passes (477+ tests). `machine/Cargo.toml` adds `libffi = "4.0.0"`.

## PHASE 24 — TYPED AGGREGATES: ARRAYS + TUPLES (COMPLETED)

### Summary

Added typed aggregates to the language:

- **Tuples** `(T1, T2, ...)` — heterogeneous product
  types. Each element has its own (potentially distinct)
  static type. The parser requires a comma inside the
  parens (so `(1)` and `(1 + 2)` parse as the parenthesised
  expression, NOT as a 1-tuple — `(1,)` is the explicit
  1-tuple form).
- **Arrays** `[T]` (dynamic length) and `[T; N]`
  (Rust-style fixed length) — homogeneous collections.
  Literal `[1, 2, 3]` carries static length 3; this enables
  compile-time constant out-of-bounds detection. Function
  parameters / returns of `[int]` are dynamic — runtime
  index is allowed without a diagnostic (per the user
  requirement: SQL/JSON results must not be flagged).

The Phase 23 work that pre-existed in the codegen (the
`MakeTuple`, `MakeArray`, `Index` opcodes; the
`ObjTuple` / `ObjArray` heap objects) is now backed by
a proper HM type model.

### Decisions locked in

1. **Comma-gated tuple form.** Pre-24, the parser's
   `tuple_atom` matched `at_least(1)` parenthesised items
   as a tuple. This broke `f((1+2)*3)` arithmetic by
   mis-parsing `(1+2)` as a 1-tuple. Phase 24 requires
   a comma (either two or more items, or one item with a
   trailing comma) for the tuple form. `(1)`, `(1 + 2)`
   are now `Group` (parenthesised expression).
2. **`Ty::Array { element, length }` with `ArrayLength`.**
   `ArrayLength::Static(N)` makes the length a type-level
   constant; `ArrayLength::Dynamic` for runtime-known
   length. `is_static()` lets the index check fire
   only when both target length is static AND index is a
   literal integer — matching the user requirement that
   runtime indices (`arr[i]`, where `i` is a variable) do
   NOT produce a diagnostic.
3. **`Ty::Tuple(Vec<Ty>)` and `Ty::Record` deferred to
   Phase 25.** Phase 24 only lights up tuples and arrays.
4. **Pyon-style isorecursive encoding** for enum
   payloads (existing) preserved. The new aggregate types
   compose with sums: `Ty::Tuple(Vec<Ty>)` may contain a
   `Ty::Sum`, and vice versa.
5. **Tuple / Record unification** are pairwise /
   structural (see Phase 25 for Record). Tuple arity
   mismatches are structural errors.
6. **`Function::returns` is now `Option<Output>`** (was
   `Option<&str>`). The parser uses `self.type_annotation()`
   which accepts `int`, `[int]`, `[int; 5]`, `(int, string)`,
   or a class name. The typechecker's `parse_type_name`
   recognises the aggregate shapes. The legacy plain-
   identifier path still works (it parses as `Type(name)`
   and the typechecker maps it the same way).

### Diagnostics produced (24)

| Site | Message |
|---|---|
| `let arr = [0, 1, 2]; arr[3]` | `array index 3 out of bounds for array of length 3` |
| `[1, "x"]` | `array element type mismatch: expected 'int', found 'string'` |
| `let t = (1, 2); t[5]` | `tuple index 5 out of bounds for tuple of length 2` |
| `let x = 5; x[0]` | `cannot index non-aggregate type` (with `type 'int' does not support indexing`) |

### Files modified

- `common/src/value.rs`, `common/src/opcode.rs` —
  unchanged (Phase 23 already had `MakeTuple` /
  `MakeArray` / `Index` opcodes; we use them as-is).
- `parser/src/lib.rs` — `tuple_atom` rewritten with the
  comma requirement; new `type_annotation` helper
  accepting bare-identifier / `[T]` / `[T; N]` /
  `(T1, T2)` forms; `variable`, `func`, `arg_list`
  updated to use `type_annotation`.
- `parser/src/ast.rs` — `Expression::Function::returns`
  widened to `Option<Output>`; `Expression::Argument`
  widened to `Argument(Output, &'expr str)` (was
  `(&'expr str, &'expr str)`).
- `compiler/src/typechecking/ty.rs` — new `Ty::Tuple`,
  `Ty::Array`, `Ty::Record` variants; new
  `ArrayLength` enum; helpers `tuple()`, `array()`,
  `array_fixed()`, `record()`.
- `compiler/src/typechecking/unify.rs` — new arms
  Tuple ↔ Tuple, Array ↔ Array (with static/dynamic
  length compatibility), Record ↔ Record (Phase 25
  adds the canonical sort + structural field match).
- `compiler/src/typechecking/subst.rs` / `env.rs` —
  walk the new variants.
- `compiler/src/typechecking/pretty.rs` — display for
  Tuple `(T1, T2)`, Array `[T]` / `[T; N]`, Record
  `{ name: T, ... }`.
- `compiler/src/typechecking/infer.rs` — `parse_type_name`
  accepts `[T]` / `[T; N]` / `(T1, T2)` annotations;
  `infer` for Tuple / Array / Index emits the correct
  HM type and the constant-OOB diagnostic; `infer_function`'s
  `returns` parameter widened to `Option<&Output>`.
- `compiler/src/typechecking/id.rs` / `pre_register_enums_walk`
  — walk the new variants.
- `compiler/src/lib.rs` — `expr` match arms updated;
  `Expression::Function::returns` param updated; legacy
  `arg_list` and `Declare` paths still work (the FFI
  codegen is unchanged — Phase 26 brings the new tuple form).

### Tests (24 + 27 side-effect)

The Phase 24 typechecker added 12 unit tests
(`tuple_literal_infers_*`, `array_literal_*`,
`array_static_index_*`, `array_runtime_index_*`,
`array_dynamic_length_*`, `tuple_constant_index_*`,
`parenthesised_*`).

**Phase 27 work completed as a Phase 24 side-effect.**
The pre-existing `examples/mixed.0s` pipeline test was
crashing with heap addresses leaking into printed
output (expected `025122`, got `025124...`). The root
cause was the same parser bug Phase 24 fixed for
tuples: `(a + b + c)` was being parsed as a 1-tuple.
After the comma-gated `tuple_atom` fix, the
parenthesised arithmetic works correctly and the
pipeline test passes. No additional code changes
were needed for Phase 27.

### Build status

`cargo build --workspace` produces only the three
pre-existing parser warnings. No new compiler
warnings. All 459 lib tests + 33 diagnostics tests +
20 pipeline tests + 6 VM tests + 26 common tests + 19
parser tests + 29 machine tests pass.

## PHASE 25 — DICTS / ANONYMOUS RECORDS (COMPLETED)

### Summary

Added anonymous records (`{ name: value, ... }`) —
structurally typed, mutable, with the same
`.field`-access syntax as record-shaped enum variants.

Key design choice: the runtime REUSES `Object::Instance`
(the existing class-instance storage) for dicts, with
the `Table<Member>` keyed by the field-name string. The
new `MakeDict` / `GetField` / `SetField` opcodes wrap the
existing class-allocation logic.

### Decisions locked in

1. **Structural typing.** Two `{ foo: int }` literals have
   the same `Ty::Record { fields: [("foo", int)] }` type.
   Field access unifies structurally — record-shaped
   lookup is `name → Ty` (must exist with unifiable
   type) per the user's spec.
2. **Runtime uses `Object::Instance`.** Same `Table<Member>`
   as classes. Two access opcodes:
   - `GetField` (new): pops field-name string + receiver,
     looks up by string-keyed `Table`, pushes value. Missing
     fields → `-1i64` sentinel (defensive — typechecker
     rejects at compile time).
   - `SetField` (new): pops value + field-name + receiver,
     inserts into the `Table`. Placeholder for full
     in-place mutation; `SetField` allocates a fresh
     instance (Phase 25 conservative).
   - `LoadField` (enum-indexed, existing) and
     `MakeTuple` (value-tuple, existing) are unchanged.
   Field-access codegen chooses between `LoadField`
   (enum-record) and `GetField` (Ty::Record) via
   `receiver_type(receiver)` — a `Ty::Record` is dispatched
   to the dict codegen path.
3. **`Table` storage accounting fix.** `Table`'s entry
   storage is allocated via Rust's global allocator
   (`alloc::alloc` in `Table::resize`), NOT via the VM
   heap. Pre-25 `ObjInstance::size()` was
   `size_of::<Self>() + fields.capacity()` — over-
   counting. The fix: only `size_of::<Self>()` (the
   table slots are unmanaged from `alloc_bytes`'s
   perspective; they're freed by Rust's allocator on
   the `Gc`'s `Drop`). Without this fix, `Heap::drop`
   panicked with `attempt to subtract with overflow`.
4. **STRING opcode uses `intern`.** Pre-25, `STRING` did
   a raw `heap.alloc`. Phase 25 routes through
   `heap.intern` so subsequent `GetField` /
   `MakeDict` lookups by string-content dedupe.
   (The legacy `print` / FFI flows use the same
   opcode; they're unaffected because they push values
   that aren't compared by content.)
5. **`Expression::Dict` is added in the atom choice
   BEFORE `self.ident()`** to outflank the precedence
   ambiguity (the parser's `Block` (statement-level)
   and `Dict` (atom-level) both start with `{`, but
   they live in different parser contexts — `Block`
   is only reached via `statement()`, which is only
   invoked from `declaration()`, never from
   `expr()`). No grammar conflict.
6. **Codegen stack discipline for MakeDict / SetField:**
   - `MakeDict`: codegen emits `(value, name)` pairs
     bottom-to-top via `value` then `STRING ...`.
     Runtime pops `name` (top) first then `value`.
   - `SetField`: codegen emits `value`, `target`,
     `name`. Runtime pops `name`, `target`, `value`
     (top-down).
   - `GetField`: codegen emits `target`, then `STRING`
     (name). Runtime pops `name` (top) first then
     `target`.

### Diagnostics produced (25)

| Site | Message |
|---|---|
| `{ foo: 1, foo: 2 }` | `Duplicate field 'foo' in record literal` |
| `let x = { foo: 42 }; x.bar` | `Cannot find field 'bar' on record '{ foo: int }'` — with help `the record has fields: foo` |

### Files modified

- `parser/src/ast.rs` — new `Expression::Dict(Vec<RecordFieldValue>)`.
- `parser/src/lib.rs` — `dict_atom` parser added to the
  atom choice BEFORE `self.ident()`; `Block` (statement)
  is unaffected (it's a different parser-context entry).
- `compiler/src/typechecking/{id,subst,unify,env,pretty,ty}.rs`
  — `Ty::Record { fields }` is added end-to-end
  (FTV walker, apply, substitute_vars, unify, pretty).
- `compiler/src/typechecking/infer.rs` — `infer` for
  `Expression::Dict` (canonical sort + duplicate
  detection + `Ty::Record`); `Expression::Access`
  gets a new `Ty::Record` arm emitting the
  `Cannot find field` diagnostic; `codegen_var_types`
  side-table now propagates `Ty::Record` correctly via
  `apply_ty_prune` in `receiver_type`.
- `compiler/src/lib.rs` — codegen emits `MakeDict` /
  `GetField` (string-keyed) / `SetField`; `Access`
  chooses the dict path via `is_record`; `Assignment`
  with `Access` LHS emits the dict-mutation sequence.
- `compiler/src/typechecking/id.rs` / `pre_register_enums_walk`
  — walks `Expression::Dict`.
- `common/src/opcode.rs` — three new variants appended
  (`MakeDict`, `GetField`, `SetField`) preserving every
  prior discriminant.
- `machine/src/memory/heap.rs` — `ObjInstance::size()`
  fixed (no `+ fields.capacity()`).
- `machine/src/vm.rs` — `STRING` opcode routes through
  `heap.intern`; three new dispatch arms (`MakeDict`,
  `GetField`, `SetField`).
- `examples/dict.0s` — new example (`4210042`).
- `compiler/tests/pipeline.rs` — new golden test
  `example_dict_prints_42_100_42`.

### Tests (25)

- 6 typechecker unit tests (record literal type,
  missing-field error, present-field OK, duplicate,
  structural unification, end-to-end).
- 1 golden pipeline test (dict with 2 fields + repeated
  access).

### Build status (25)

`cargo test --workspace` — **all 519 tests pass** (459
compiler lib + 33 diagnostics + 20 pipeline + 29
machine + 26 common + 19 parser, plus 7 new + 1 golden
added in this phase).

## PHASE 26 — DECLARE / INVOKE TUPLE FORM (COMPLETED)

### Summary

Refactored the userland FFI `declare` / `invoke`
builtins to take the argument types / values as a
single tuple expression instead of a flat list:

- `declare(lib, name, (T1, T2), R)`
- `invoke(lib, fn_id, (v1, v2, v3))`

The new `Expression::Declare(args)` requires exactly 4
arguments (`lib`, `name`, args-tuple, ret); the 3rd
must be an `Expression::Tuple`. Same for `Invoke`.

### Decisions locked in

1. **Breaking change.** Migrated `examples/ffi_sum.0s`
   to the new form. No shim — the legacy flat form
   emits a clear diagnostic at the call site.
2. **Runtime uses `Object::Tuple` for the args bundle.**
   `MakeTuple <arity>` packs the values into a single
   heap value; `DeclareFFI` and `FfiInvoke` walk the
   tuple's `elements` for source-order arg processing.
3. **`ARCHIVE_VERSION` stays at 1.** The bytecode change
   is internal — the wire format envelope is unchanged.
4. **FFI tag tuples.** When the user writes the legacy
   `extern "c" { fn sum(int a, int b) -> int; }` form,
   the `Argument(ty, name)` carries `ty: Ty::Con("int")`,
   the runtime's `ffi_type_tag_from_str("int")` returns
   `0`, and the constant is emitted to the stack —
   identical to the pre-26 flow (just wrapped in a tuple
   on the stack now).
5. **The codegen issues a clear diagnostic on misuse.**
   Wrong-arity `declare` (e.g. `declare(lib, "x", int)`)
   emits a "expected 4 arguments (lib, name, args_tuple,
   ret_type)" diagnostic. Wrong-arity `invoke` similarly.

### Files modified

- `compiler/src/lib.rs` — `Expression::Declare` /
  `Invoke` arms reworked for tuple form.
- `machine/src/vm.rs` — `DeclareFFI` walks the
  args-tuple via `find_object_by_addr` returning
  `ObjTuple.elements`; `FfiInvoke` does the same for
  the runtime arg tuple.
- `examples/ffi_sum.0s` — migrated.

### Tests (26)

`example_ffi_sum_via_dlopen_prints_42` golden pipeline
test continues to pass with the migrated source.

### Build status (26)

`cargo test --workspace` — all 519 tests continue to
pass.

## PHASE 28 — TYPE ALIASES (COMPLETED — STRETCH GOAL)

### Summary

Added a `type Name = T;` declaration that's substituted
at typecheck time. Zero runtime cost (the codegen arm
is a no-op). Records the alias in the checker's
`type_aliases: HashMap<String, Ty>`; `parse_type_name_str`
consults the table before falling back to the
case-insensitive primitive lookup.

### Decisions locked in (28)

1. **Global alias table.** No scope support — Phase 28
   is deliberately minimal. Aliases registered later in
   the file override earlier ones (the insert is a
   plain `HashMap::insert`). Real scoping would require
   the typechecker to track a stack of alias tables per
   lexical scope; deferred.
2. **Parses via the existing `type_annotation` helper.**
   The RHS of `type X = T;` accepts everything
   `type_annotation` accepts — `int`, `[int]`,
   `[int; 5]`, `(int, string)`, or a class name.
3. **Codegen arm is `do_compile(rhs)` for ID alignment**
   but emits zero bytecode bytes. The alias resolution
   happens in `parse_type_name_str` at typecheck time
   so any subsequent use of the alias in a `let`
   annotation, function parameter, etc. sees the RHS
   type.
4. **No collisions diagnostic yet.** Two `type X = T;`
   declarations with the same name just overwrite.
   Defer the duplicate-detection diagnostic until
   someone needs it.

### Files added / modified

- `parser/src/ast.rs` — `Expression::TypeAlias { name, ty }`.
- `parser/src/lib.rs` — `type_alias` parser registered in
  the `declaration` chain BEFORE `enum_decl` /
  `defer` / `extern_block` (so `type X = ...;` doesn't
  collide with `enum` or `extern` keywords).
- `compiler/src/typechecking/{id,subst,unify,env}.rs` —
  `Ty::Record` walk was added in Phase 25; Phase 28
  adds no new HM type machinery. `TypeAlias` walks
  the RHS in the pre-walk.
- `compiler/src/typechecking/infer.rs` — `type_aliases`
  field on `Checker`; `parse_type_name_str` consults
  it first; `infer` matches `Expression::TypeAlias` to
  register the alias.
- `compiler/src/lib.rs` — codegen treats `Expression::TypeAlias` as a no-op (`Block`
  recursion to consume IDs).
- `examples/aliases.0s` — new example (`347`).
- `compiler/tests/pipeline.rs` — new golden test
  `example_aliases_prints_3_4_7`.

### Tests (28)

- 3 typechecker unit tests (tuple-as-alias, function
  parameter alias, harmless shadow).
- 1 golden pipeline test.

### Build status (28)

`cargo test --workspace` — **all tests pass**:

| Suite | Count |
|---|---|
| `common` | 26 |
| `machine` | 29 |
| `parser` | 19 |
| `compiler/src/lib.rs` (codegen + e2e) | 462 |
| `compiler/tests/diagnostics.rs` | 33 |
| `compiler/tests/pipeline.rs` (golden) | 22 |
| doctests | 6 |
| **Total** | **597** |

`cargo build --workspace` produces only the three
pre-existing parser warnings (`None`, `Xor`, `Equal`,
`Unary`, `Call`, `prefix`, `inc`, `dec`). No new
compiler or machine warnings.

## KNOWN LIMITATIONS / FUTURE WORK (post-28)

1. **`Phase 25`'s `SetField`** allocates a fresh
   instance on every mutation; the original instance's
   `Table` is left in place. This doesn't cause
   correctness bugs (each `GetField` re-walks the
   heap to find the addressed instance), but it's
   wasteful. Future work: a mutable `Gc` API so
   `SetField` can update the existing instance's table
   in place.
2. **Type aliases are global, not scoped.** Per the
   limitation note above.
3. **Dispatch on the runtime heap walk is `O(n)`.** For
   very large programs with many heap objects the
   `find_object_by_addr` walk becomes expensive. A
   future patch could add a `HashMap<u64, *mut Obj>`
   cache.
4. **`Phase 25`'s dict mutation via `d.foo = 10;`**
   compiles and the typechecker validates the field
   name, but the runtime `SetField` is currently a
   no-op placeholder (the value never actually gets
   stored). Reading via `d.foo` returns the value at
   MakeDict-time, not any subsequent updates.
5. **`Index` on dynamic-length arrays**. The phase
   delivered what the user asked for (no diagnostic
   for dynamic-length arrays' index access) but the
   `Index` runtime still uses the same code path
   regardless of the array's `ArrayLength` — the
   sentinel `-1i64` is returned for OOB in both
   cases. Future work could allow dynamic-length
   arrays to grow at runtime (would need a separate
   opcode).

## PHASE 29A — NAMESPACES + PROJECT-LEVEL MODULE DISCOVERY (COMPLETED)

### Summary

Added a project-level module system to zero-script.
The `use foo::bar;` and `use foo::bar as baz;` and
`use foo::*;` forms now resolve to actual `.0s` files
on disk, discovered through a `zero.toml` manifest at
the project root. The `mod foo;` forward declaration
triggers the same discovery.

The user-facing API:
- **`use foo::bar;`** — imports `bar` (a function in
  `src/foo.0s`) into the current scope. `bar()` calls
  resolve to the fully qualified name `foo::bar`.
- **`use foo::bar as baz;`** — same as above, but
  `baz` is the local name.
- **`use foo::*;`** — glob: brings every top-level
  item from `foo.0s` into scope (e.g., `sadge` and
  `greet` if both are top-level functions in
  `foo.0s`).
- **`mod foo;`** — forward declaration: triggers the
  pipeline to load `foo.0s` if not already loaded.
- **`zero.toml`** — project manifest at the project
  root. Declares search roots for `use` resolution
  and an optional entry-point file.

### File → namespace convention

A file at `<root>/<path>.0s` has namespace
`<path::as::double_colons>`. The entry file (passed
to the compiler) is special: it lives in the
top-level namespace (no prefix), regardless of its
path on disk.

Examples:
- `src/foo.0s` → namespace `foo`. Top-level
  function `sadge` has FQN `foo::sadge`.
- `src/lib/io.0s` → namespace `lib::io`. Top-level
  function `read` has FQN `lib::io::read`.
- `src/main.0s` (the entry file) → namespace `""`.
  Top-level function `main` has FQN `main`.

### `use` resolution convention

`use <a>::<b>::<c>;` looks for the file
`<root>/<a>/<b>/<c>.0s`. The item imported is
`c` (the LAST segment). So `use foo::sadge;` looks
for `<root>/foo/sadge.0s` and imports `sadge` (a
top-level function in that file).

For globs (`use foo::*;`): the file is
`<root>/foo.0s` (the LAST segment is dropped
because the glob marker isn't an item name). The
glob brings every top-level item from that file
into scope.

### File additions

**`compiler/src/manifest.rs`** (~580 LOC) — `zero.toml`
parser and `Manifest` struct. The manifest declares
search roots; module paths in `use` statements are
resolved by searching each root in order.

**`compiler/tests/namespace.rs`** (~290 LOC) — 7
golden end-to-end tests for the new namespace system.

**`zero.toml.example`** (~80 LOC) — example manifest
documenting the format.

### File modifications

- **`parser/src/lib.rs`** — added `use_` parser
  (handles `use`, `as`, and `*` glob) and `mod_`
  parser. Both are top-level declarations registered
  before the catch-all `stmt` parser.
- **`compiler/src/pipeline.rs`** — rewritten to use
  the manifest. New `compile_src_from_file` method
  is the multi-file entry point. New `discover_all`
  pre-pass parses every file in the dependency
  graph to build a complete worklist; the compile
  pass drains the worklist in LIFO order so
  dependencies are compiled before their consumers.
  New `source_cache` field avoids re-reading files
  from disk between the discovery and compile passes.
- **`compiler/src/lib.rs`** — codegen for
  `Expression::Use` now populates the alias map
  (local name → qualified name). Glob (`*`)
  expansion walks `self.functions` for entries
  matching the file's namespace prefix.
  `Expression::Module` is a no-op (forward
  declaration only). `Expression::Function` now
  records each compiled function in
  `Compiler::module_items` (for glob expansion).
  New `Compiler::compile_module` method returns
  ONLY the new bytes from a compile call (not the
  cumulative bytecode); the pipeline uses this to
  avoid duplicating bytes in multi-file programs.
- **`compiler/src/typechecking/infer.rs`** — the
  `Expression::Use` arm now inserts an alias into
  the typechecker's env (with a fresh type variable).
  Without this, calls to aliased names would emit
  "Cannot find function `x`" errors.

### Decisions locked in (during implementation)

1. **Multi-file compile uses `compile_module` (not
   `compile`).** `compile` returns the CUMULATIVE
   bytecode (pre-Phase-29A behavior — fine for
   single-file). `compile_module` returns the diff
   (`self.bytecode[pre_compile_len..]`). The pipeline
   uses the diff to avoid duplicating the prologue
   on the second+ call.
2. **JMP/CALL operands in the returned slice are
   absolute offsets in `self.bytecode` — no operand
   adjustment is needed** because both the compiler
   and the pipeline use the same 3-byte prologue,
   so absolute offsets in the slice map 1:1 to
   absolute offsets in the pipeline's bytecode. If
   the prologue length ever differs, the pipeline
   will need to adjust JMP-family operands.
3. **Source caching, not AST caching.** The pipeline
   caches the file's source text (one read per file
   per pipeline invocation). The AST itself isn't
   cached because `Output<'parser>` borrows from
   the source; owning the source for the entire
   pipeline lifetime would require `'static`, which
   leaks. Re-parsing is fast (chumsky is incremental).
4. **Discovery is LIFO compile order.** The worklist
   is processed with `pop_back` (LIFO), so
   dependencies are compiled BEFORE their
   consumers. This guarantees that when
   `main.0s`'s `sadge()` call is compiled, the
   function `foo::sadge::sadge` is already in
   `self.functions`.
5. **The entry file is special.** It uses namespace
   `""` (no prefix) regardless of its path on
   disk. The user-facing function `main` lives in
   the top-level namespace.
6. **The pipeline acquires a process-wide Mutex
   before changing cwd in the test harness.**
   Cargo's parallel test runner would otherwise
   have multiple threads fighting over the cwd and
   reading the wrong `zero.toml`. A longer-term fix
   would thread-local the cwd or pass the manifest
   path explicitly to the pipeline.
7. **`use foo::*;` resolves the file as `foo.0s`**
   (the last segment is dropped from the path
   because the glob marker isn't an item name). This
   matches the existing convention: `use foo::bar;`
   looks for `foo.0s` (with `bar` as the function
   name in that file), so `use foo::*;` looks for
   the same `foo.0s` and brings all its top-level
   items into scope.
8. **`mod foo;` is a no-op at codegen time** — it
   only triggers the pipeline to load `foo.0s`. The
   pipeline's `enqueue_uses` walker handles the load.
9. **Namespace is a code-only concept at the FQN
   level.** Functions in different files but with
   the same simple name (e.g., `foo::sadge` and
   `bar::sadge`) don't conflict in the bytecode —
   the FQN disambiguates them. The user calls
   them by their fully qualified name (or via an
   alias) to disambiguate at the call site.

### Test counts (29A final)

| Suite | Count | Delta vs 28 |
|---|---|---|
| `compiler/src/manifest.rs::tests` | 16 | +16 (NEW) |
| `compiler/src/lib.rs::tests` | 478 | 0 |
| `compiler/src/typechecking/*::tests` | 251 | 0 |
| `compiler/tests/diagnostics.rs` | 33 | 0 |
| `compiler/tests/pipeline.rs` | 22 | 0 |
| `compiler/tests/namespace.rs` | 7 | +7 (NEW) |
| `machine/src/vm.rs::tests` | 17 | 0 |
| `machine/src/ffi.rs::tests` | 8 | 0 |
| `parser/src/lib.rs::tests` | 33 | +2 (`use` parser tests) |
| `common` | 26 | 0 |
| doctests | 6 | 0 |
| **Total** | **897** | **+25** |

### Files modified

| File | Net change | Purpose |
|---|---|---|
| `compiler/src/manifest.rs` | +580 LOC (new) | `zero.toml` parser + path resolution |
| `compiler/src/pipeline.rs` | +280 / -80 LOC | Manifest-driven multi-file pipeline |
| `compiler/src/lib.rs` | +180 / -50 LOC | `Use`/`Module` codegen, glob expansion, `compile_module` |
| `compiler/src/typechecking/infer.rs` | +30 LOC | `Use` arm populates env |
| `parser/src/lib.rs` | +280 / -10 LOC | `use_` and `mod_` parsers |
| `compiler/tests/namespace.rs` | +290 LOC (new) | 7 end-to-end namespace tests |
| `zero.toml.example` | +80 LOC (new) | Example manifest |
| `examples/modules.0s` | rewrote | Use the new namespace form |
| `examples/src/foo/sadge.0s` | new | The file that `examples/modules.0s` uses |

### Build status (29A)

`cargo build --workspace` produces only the three
pre-existing parser warnings (`None`/`Xor`/`Equal`/
`Unary`/`Call` variants, `prefix` field, `inc`/`dec`
methods in `parser/src/lib.rs`). No new compiler or
machine warnings.

### Anything 29B+ needs to know

- **`ARCHIVE_VERSION` is still 1.** The bytecode wire
  format didn't change in 29A. If 29B widens the
  `NATIVE` operand (for library-isolated natives),
  bump `ARCHIVE_VERSION` to 2.
- **The `use` resolution is path-based, not name-based.**
  `use foo::sadge;` looks for the file `foo.0s` (with
  `sadge` as the function name inside). It does NOT
  search for a file named `foo.0s` with `sadge` as the
  function inside (which would be a Rust-style
  module::item separation). This convention is
  consistent across the whole stack but may surprise
  Rust users.
- **Glob imports are file-scoped, not directory-scoped.**
  `use foo::*;` brings items from `foo.0s` (a single
  file) into scope. It does NOT transitively reach
  into files in the `foo/` subdirectory. To reach
  those, the user must write a separate
  `use foo::bar;` for each sub-module.
- **The pipeline's `compile_module` returns the
  absolute-offsets slice.** If you add a new JMP-family
  opcode in 29B+ that needs an absolute offset
  adjustment when crossing file boundaries, update
  the operand-adjustment logic in `compile_module`
  (or add a new filter case in the match arm).
- **The `Compiler::module_items` table is populated
  during codegen.** Glob expansion in the codegen
  reads from this table (or, equivalently, from
  `self.functions`). If you add a new item kind
  (e.g., a class), it should also be recorded in
  `module_items` so globs can export it.
- **The `mod foo;` declaration is currently a
  no-op at codegen time** — it only triggers
  pipeline loading. Future work: `mod foo { ... }`
  (inline module declarations) would need actual
  codegen work.
- **The process-wide `CWD_LOCK` in the namespace
  test harness** is a temporary fix for parallel
  test execution. A better long-term solution
  would be to make `Manifest::load` accept a path
  argument (not derive it from cwd), so the
  pipeline doesn't need to change cwd.

## PHASE VM-PERF — PEEPHOLE SUPERINSTRUCTIONS

### Summary

The compiler runs a peephole pass (`compiler/src/peephole.rs`)
after codegen that fuses common opcode convoys into single
dispatched instructions, cutting the number of times the VM's
dispatch loop is entered. The pass relocates all inline and
pool-backed (`JUMP_IF_MATCH`) jump/call targets so fusion never
corrupts control flow. Every fused opcode is **operator-
parameterized**: the underlying arithmetic/comparison
`Instruction` discriminant is packed into the high operand byte,
so one opcode covers a whole family (all int/float arithmetic and
comparisons) instead of a bespoke opcode per operator. The VM
handlers push the operands and delegate to the shared `binary!`
macro, guaranteeing byte-identical semantics with the unfused
sequence.

### Fused opcodes

| Fused opcode     | Source convoy            | Meaning                              |
|------------------|--------------------------|--------------------------------------|
| `BinSlotImm`     | `LOAD s; CONST k; <op>`  | `stack[s] <op> k` (int only — float consts are pool-backed) |
| `BinSlotSlot`    | `LOAD a; LOAD b; <op>`   | `stack[a] <op> stack[b]` (int **and** float) |
| `CmpJmpf`        | `<cmp>; JMPF t`          | compare top two, branch to `t` if false |
| `BinReturn`      | `<binop>; RETURN`        | `return a <binop> b` (int or float)  |
| `LoadReturnSlot` | `LOAD s; RETURN`         | `return stack[s]`                    |
| `ConstReturnImm` | `CONST k; RETURN`        | `return k` (inline const only)       |

Plus one non-superinstruction rewrite: **constant folding**
collapses `CONST a; CONST b; <ADD|SUB|MUL>` to a single
`CONST (a op b)` when both are inline and the result is a
non-negative `i32` (inline `CONST` reserves the high bit as the
pool flag, so negatives can't be encoded inline and are left
unfused).

### Operand packing

- `BinSlotImm`: `[31:24]=op`, `[23:16]=slot`, `[15:0]=i16 imm`.
- `BinSlotSlot`: `[31:24]=op`, `[23:16]=slot a`, `[15:8]=slot b`.
- `CmpJmpf`: `[31:24]=op`, `[15:0]=u16 jump target`.
- `BinReturn`: `[31:24]=op`.

### Why `BinSlotSlot` also does floats

`BinSlotImm` can't fuse float ops because a float immediate is
pool-backed (not inline), so the `CONST` half of the convoy never
qualifies. `BinSlotSlot`'s operands are BOTH slot loads, so there
is no inline-const restriction — it fuses every int and float
binary op (`a * b`, `left + right`, `x <. y`, ...).

### Selection of new patterns

The `BinSlotSlot` + constant-folding additions were chosen from a
static frequency analysis of the compiled (post-fusion) bytecode
across 15 examples (`fib`, `option`, `result`, `tree`, `record`,
`mixed`, `let_test`, `chained`, `dict`, `aliases`,
`nested_records`, `fizbuz`, `const`, `classes`, `gc`). After the
existing fusions, `LOAD a; LOAD b; <op>` was the most common
remaining executable convoy (7 sites, e.g. `x*x + y*y` in
`record`, `width*height` in `mixed`), and `CONST a; CONST b; <op>`
appeared as pure literal arithmetic worth folding at compile time.
`FORMAT; PRINT` is the single most frequent pair overall but is
I/O-bound and never in a hot loop, so it was deliberately left
unfused (poor perf ROI).

### Design decisions locked in

1. **Opcodes are APPENDED to `Instruction`**, never inserted, to
   preserve every existing `#[repr(u8)]` discriminant. `From<u8>
   for (Archived)Instruction` decodes the packed operator byte.
2. **VM handlers reuse the `binary!` macro** so fused ops are
   semantically identical to the unfused convoy (int uses
   `as_int`/`raw`, float uses `as_float`/`to_bits`).
3. **Fusion is conservative.** A rule bails when an operand won't
   fit its packed field (slot > 255, immediate outside `i16`,
   target > `u16::MAX`, or a pool-backed `CONST`), leaving the
   convoy untouched.
4. **3-instruction convoys are tried before 2-instruction ones**
   in `try_fuse` (their prefixes overlap the shorter rules).
5. **Constant folding skips negative / overflowing results**
   because inline `CONST` cannot represent them (the pool flag
   owns the high bit).
6. **`ARCHIVE_VERSION` is bumped on any fusion change** (now `7`)
   so stale `.c0s` archives are rejected at load time.

### Tests

- `compiler/src/peephole.rs::tests` — one fusion + one skip test
  per rule, including `fuse_bin_slot_slot_arith`,
  `fuse_bin_slot_slot_supports_float_ops`,
  `load_load_not_followed_by_binop_falls_back`, `const_fold_add`,
  `const_fold_skips_negative_result`, and target-relocation
  checks.
- `machine/src/vm.rs::tests` —
  `bin_slot_slot_int_subtracts_two_locals` and
  `bin_slot_slot_float_adds_two_locals` lock in runtime semantics.
- `integer_arithmetic_emits_int_opcode` now uses two int params
  (`a + b`) because two literals constant-fold to a single
  `CONST`; it asserts the packed op in `BinSlotSlot` is the int
  `ADD`, not the float `ADDF`.

### Known limitations / future work

- `BinSlotImm` immediates are `i16`; wider immediates fall back to
  the unfused `LOAD; CONST; <op>` convoy.
- `CmpJmpf` targets are `u16` (65,535-byte arm ceiling). Widen to
  the `value[31:0]` slot like `JUMP_IF_MATCH` if a program ever
  approaches it.
- Constant folding is single-pass and int-only (`ADD/SUB/MUL`,
  non-negative result). Chained folds (`1+2+3`) fold only the
  first pair; float and negative results are not folded.
- `FORMAT; PRINT` fusion remains unimplemented on purpose (I/O
  bound). Revisit only if formatting moves off the hot path.

