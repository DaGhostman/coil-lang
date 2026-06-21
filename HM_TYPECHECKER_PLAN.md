# HM_TYPECHECKER_PLAN.md (Phase 15D addition)

## Phase 15D — Polish (in progress)

### Goal

Wire up the automatic GC in `Machine::execute` (the missing
piece flagged by 15C's "Anything 15D needs to know" #2), add
golden end-to-end tests for `.0s` files, document the
`JUMP_IF_MATCH` 16-bit target ceiling, clean up review nits, and
land the polish that closes out the sum-types/match work.

### 15D.1 — Automatic GC wiring

`Object::mark_references` for `Object::Enum` already traces
payloads (15A work), but the VM's automatic `trace`/`sweep`
cycle was NOT wired into `Machine::execute` at the time of 15C
— the only GC that ran was the debug-only per-instruction sweep
inside `#[cfg(debug_assertions)]` (visible as "Performing GC
trace" / "Performing GC collection" spam in debug builds). In
release builds the heap grew unboundedly.

This phase replaces that debug-only path with a proper
allocation-pressure-driven GC. Strategy:

- `Machine` gains a `Vec<u64>` of recent allocation sites (we
  don't track them, just count) and an `alloc_counter: usize`.
- Every allocation site (`Instruction::INIT`, `STRING`, `FORMAT`,
  `MAKE_ENUM`) increments `alloc_counter`.
- When `alloc_counter > GC_TRIGGER_INTERVAL` (default 64),
  the VM calls a new `Machine::collect_garbage` that:
    1. Builds the root set from the live operand stack —
       addresses of all stack values that fall in the heap's
       intrusive-list range.
    2. Calls `heap.trace(&roots)` to mark root objects.
    3. Walks the grey stack via `Object::mark_references` until
       empty (the transitive closure of reachable objects).
    4. Calls `heap.sweep()` to free anything not marked.
- The previous `#[cfg(debug_assertions)]` per-instruction GC
  block is removed.

The trace root set is the operand stack: every value the VM
has on the stack that points into the heap is a potential root.
Immediates (ints, floats, bools) aren't roots but the trace
function already ignores non-heap addresses.

### 15D.2 — examples/result.0s

Sum-of-sum (`Option<Result<int>>`) demonstrating nested match
with both a binding pattern (`Some(v)`) and a nested
constructor pattern (`Result::Ok(Option::Some(v))`).

### 15D.3 — examples/tree.0s

Recursive enum (binary tree) to verify the isorecursive
encoding (MUST-HAVE #1 from the red-team): a `Tree` enum whose
`Node` variant contains two `Tree` payloads.

### 15D.4 — Golden pipeline tests

A new `compiler/tests/pipeline.rs` integration test that:

1. Reads an `.0s` file from disk.
2. Compiles it in-memory via a new `Pipeline::compile_src`
   helper (avoids the `out.c0s` round-trip).
3. Runs the resulting bytecode through a new
   `Machine::with_output(...)` builder that captures stdout.
4. Asserts on the exact captured output.

Four tests:
- `example_option_prints_42`
- `example_result_prints_42_0_neg1`
- `example_tree_prints_6`
- `example_fib_still_works` (regression test)

### 15D.5 — 15C review feedback

- MEDIUM #1 — document the 16-bit `JUMP_IF_MATCH` target
  ceiling in `common/src/opcode.rs` (and `AGENTS.md`).
- LOW #3 — fix the LOC accounting in `AGENTS.md` 15C section
  to use net (insertions − deletions) not raw insertions.
- LOW #4 — add `Expression::Default` codegen arm with a TODO
  in `compiler/src/lib.rs`.
- LOW #5 — add a 4th codegen test for nested constructor
  patterns.

### 15D.6 — `case` keyword

Deferred (per the task description): the parser comment
correctly notes that registering `case` cleanly requires either
including a no-op `keyword!` in a `choice` (changing the output
type) or a typed `text::keyword::<...>` call that leaks chumsky
internals. Either is intrusive and not worth the few lines of
risk-free benefit; document and skip.

### 15D.7 — Documentation

Add a "PHASE 15D — POLISH" section to `AGENTS.md`.

### 15D.8 — Final test count update

See the test counts table in the 15D AGENTS section.

## Phase 15A — Parser + AST

### New keywords
`enum`, `match`, `default`, `=>`, `::` (qualified-name operator).

### New AST nodes
- `Expression::EnumDecl { name, variants }` — top-level sum type declaration.
- `Expression::Variant { name, payload }` — a variant inside an enum.
- `Expression::Construct { enum_name, variant_name, args }` — qualified constructor application.
- `Expression::Match { scrutinee, arms }` — match expression (replaces dead flat shape).
- `Pattern::Wildcard`, `Pattern::Binding { name }`, `Pattern::Constructor { enum_name, variant_name, payload }` — top-level enum.

### Namespaced constructors (Decision A)
`Option::Some(42)` is the only valid form. Bare `Some(42)` is parsed as `Call`, and the
typechecker produces a misleading "Cannot find function" error. Users must qualify.

### GC placeholder (MUST-HAVE #3)
`Object::Enum` was added to the `Object` enum and all 9 method arms (mark, mark_references,
dealloc, etc.) in 15A — BEFORE any code allocates an enum — to prevent runtime UB from
the GC mishandling the new variant.

## Phase 15B — HM Typechecker for Sum Types and Match

### MUST-HAVE #1: Isorecursive encoding for `Ty::Sum`
Recursive enum payloads (`enum Tree { Leaf, Node(int, Tree, Tree) }`) use `Ty::Con(name)`
(opaque name reference) inside the payload, NOT the unfolded `Ty::Sum(...)`. The HM
occurs check at `unify.rs:130-132` would otherwise reject recursive enums. The
`ftv_ty` walks variant payloads but treats `Ty::Con(name)` as opaque (zero free vars),
which breaks the cycle.

### MUST-HAVE #2: Dual data structure for variant tags
`Vec<String>` for insertion order + `BTreeMap<String, u32>` for name→tag lookup.
A pure `BTreeMap` orders by key alphabetically, which would silently miscompile
programs with non-alphabetical variant declarations. The `Vec` is append-only
during enum declaration; the `BTreeMap` is built from the `Vec`.

### Public API
After `enum Option { None, Some(int) }` is inferred:
- `Checker::tag_for("Option", "None")` → `Some(0)`
- `Checker::tag_for("Option", "Some")` → `Some(1)`
- `Checker::arity_for("Option", "None")` → `Some(0)`
- `Checker::arity_for("Option", "Some")` → `Some(1)`
- `Checker::enum_variants("Option")` → `Some(vec![("None", 0, vec![]), ("Some", 1, vec![Ty::Con("int")])])`

Codegen reads these via `lookup_at(NodeId)` to get `Ty::Constructor { owner, tag, arity }`.