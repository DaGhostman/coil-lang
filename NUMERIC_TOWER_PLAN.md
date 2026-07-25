# Numeric Tower — Implementation Plan

**Status:** planning only (no code in this change set)  
**Goal:** Treat homogeneous numeric tuples and arrays as vectors with a
full element-wise / broadcast arithmetic tower, so that e.g.
`(1, 1) + (1, 1) == (2, 2)` and `[1, 2] + 3 == [4, 5]`, including
generic/`T: Num` call sites where that is sound.

**Decisions locked (2026-07-25):**

1. Aggregate–aggregate length must be known and equal at **compile
   time**; mismatch or unknown dynamic length is a **hard type error**
   (no runtime length check).
2. Element-wise `**` is **in** v1 (same zip/broadcast rules as `*`).
3. Fixed arities **≤ 4** are **always unrolled** in codegen (loop only
   for larger static arities).
4. Tier C / NT-5 (constraint lifting) lands **after** NT-1…3 bake.

---

## 0. Problem statement

Today:

| Expression | Typecheck | Runtime |
|------------|-----------|---------|
| `1 + 1` | `int` | correct `ADD` |
| `"a" + "b"` | `string` | `FORMAT` concat |
| `(1, 1) + (1, 1)` | `(int, int)` (operands unify) | **wrong** — `ADD` on two heap pointers |
| `[1, 2] + [3, 4]` | same hole | same hole |

`infer_arith` unifies left and right and assumes a **scalar** opcode.
Tuple/array values are heap addresses, so the typed result is a lie.

Desired: a **numeric tower** over existing aggregates — not a new
`vector` type — covering element-wise ops, scalar broadcast, length
rules, trait/`Num` integration, codegen, docs, and tests.

---

## 1. Design principles (locked for this plan)

1. **Reuse existing types.** Vectors are:
   - homogeneous tuples `(T, T, …, T)` (arity ≥ 1), and
   - arrays `[T]` / `[T; N]`.
2. **Element type must be numeric** for arithmetic (`int` / `float`
   initially; `byte` when it joins `Num`).
3. **Same-shape ops are element-wise.** Result shape = left shape
   (after broadcast resolution).
4. **Scalar broadcast is one-sided.** `vec ⊕ scalar` and `scalar ⊕ vec`
   expand the scalar; two aggregates of different length never broadcast
   to each other in v1.
5. **`*` and `**` are element-wise**, not dot product / matrix power.
   Dot / cross / matrix are named helpers later (stretch), not operator
   overloads.
6. **Zip length is a compile-time property.** Only equal static lengths
   (tuple arity or `[T; N]`) may zip. Dynamic `[T] ⊕ [T]` is a hard
   error — promote to `[T; N]` (literals already carry static length)
   or broadcast a scalar.
7. **String `+` stays special-cased** and is never part of the tower.
8. **Prefer lowering to existing opcodes** for correctness first;
   fused VM ops are an optional later optimization.
9. **Codegen unrolls arity ≤ 4**; larger static arities use a loop.
10. **Trait story is lifting, not combinatorial instances.** Do not
    pre-register `impl Add<(int,int)>`, `impl Add<(int,int,int)>`, …
    for every arity.

---

## 2. Semantic model

### 2.1 Layers

```
          ┌─────────────────────────────────────┐
          │  Aggregates (homogeneous)           │
          │  (T,…,T)   [T]   [T; N]             │
          │  ops: element-wise + broadcast      │
          └─────────────────▲───────────────────┘
                            │ T : Num (or Add/…)
          ┌─────────────────┴───────────────────┐
          │  Scalars                            │
          │  int, float  (+ byte when in Num)   │
          │  ops: existing ADD/ADDF/…           │
          └─────────────────────────────────────┘
```

Comparisons (`Ord` / `Eq`) on aggregates are **out of scope for the
arithmetic tower** except where already defined (e.g. structural `Eq`
derive, tuple/array `Show`). Lexicographic / element-wise `<` on
vectors can be a follow-up; do not overload them in the first landing.

### 2.2 Operator rules (v1)

Let `⊕ ∈ {+, -, *, /, %, **}` and unary `-`.

| Left | Right | Result | Rule name |
|------|-------|--------|-----------|
| scalar `T` | scalar `T` | `T` | existing |
| `(T)^n` | `(T)^n` | `(T)^n` | zip (equal arity) |
| `[T; N]` | `[T; N]` | `[T; N]` | zip (static) |
| `[T]` | `[T]` | — | **hard error** (length not known at compile time) |
| `[T; N]` | `[T]` | — | **hard error** |
| `[T]` | `[T; N]` | — | **hard error** |
| `[T; N]` | `[T; M]` (`N ≠ M`) | — | **hard error** |
| `(T)^n` | `T` | `(T)^n` | broadcast-right |
| `T` | `(T)^n` | `(T)^n` | broadcast-left |
| `[T; N]` | `T` | `[T; N]` | broadcast-right |
| `T` | `[T; N]` | `[T; N]` | broadcast-left |
| `[T]` | `T` | `[T]` | broadcast-right (length from LHS only) |
| `T` | `[T]` | `[T]` | broadcast-left (length from RHS only) |
| otherwise | | | diagnostic |

`T` must support that operator as a scalar (`int`/`float` hardwired, or
open `T: Add` / `Num` / pow trait as applicable — §4).

**Unary `-vec`:** element-wise negate; shape preserved. Applies to
tuples and both `[T; N]` and `[T]` (length from the single operand).

**Compound assign** (`+=`, `**=`, …): same rules with LHS shape fixed
(broadcast RHS into LHS shape; reject shape-changing or dynamic-zip RHS).

**Literal note:** `[1, 2] + [3, 4]` is fine — array literals already
infer `[int; 2]`, not dynamic `[int]`. Dynamic `[T]` arises from
parameters / growing arrays / annotations; those must use scalar
broadcast or be retyped as `[T; N]` to zip.

### 2.3 Static vs dynamic array lengths (locked)

| Pair | Typecheck | Runtime |
|------|-----------|---------|
| `[T; N] ⊕ [T; N]` | OK | zip |
| `[T; N] ⊕ [T; M]` (`N ≠ M`) | **hard error** | — |
| `[T] ⊕ [T]` | **hard error** | — |
| `[T; N] ⊕ [T]` / `[T] ⊕ [T; N]` | **hard error** | — |
| `[T] ⊕ T` / `T ⊕ [T]` | OK | broadcast (no length compare) |

Rationale: zip length is always a compile-time fact. No runtime
length check, panic, or `Result` for aggregate–aggregate ops.

### 2.4 Heterogeneous tuples

`(1, "x") + …` and `(1, 2.0) + …` are **errors**.
Reuse / extend `homogeneous_types` (already used for `for-in`).

Optional later: per-position zip if every position independently
supports `⊕` — **not** in v1 (complicates codegen and traits).

### 2.5 Explicit non-goals (v1)

- Dot product via `*`, matrix multiply, BLAS-style APIs
- Mixing `int` and `float` inside one vector (no promotion)
- User `impl Add for (int, int)` coherence vs compiler lifting
  (compiler owns aggregate arithmetic; user instances for
  builtin aggregate heads stay rejected per coherence rules)
- New surface syntax (`@[1,2]`, `vec2`, etc.)
- Changing tuple literal parsing / comma rules

---

## 3. Typechecker changes

### 3.1 Core helper: classify arithmetic operands

Add something like:

```text
enum ArithShape {
  Scalar(Ty),
  Tuple { elem: Ty, arity: usize },
  Array { elem: Ty, length: ArrayLength },
}

fn classify_arith(ty: &Ty) -> Option<ArithShape>
fn resolve_arith_shapes(op, left, right, range) -> Result<(ArithShape /*result*/, ZipMode), Diagnostic>
```

`ZipMode`: `Scalar` | `Zip` | `BroadcastLeft` | `BroadcastRight`.

Call this from `infer_arith` **before** the current “unify and hope”
path when either side is `Tuple` / `Array`. Scalar–scalar path stays
unchanged (including string `+`).

### 3.2 Element constraint

After resolving shapes, constrain `elem`:

- Ground `int` / `float` → existing opcode path.
- Open `Ty::Var` → bind `Add`/`Sub`/… (or `Num`) as today for scalars,
  so `fn f<T: Num>(T a, T b)` still works for scalars; for
  `fn g<T: Num>((T, T) a, (T, T) b)` see §4.
- Non-numeric ground (`string`, `bool`, enums, …) → diagnostic:
  `cannot apply '+' element-wise to tuple of 'string'`.

### 3.3 Diagnostics (suggested copy)

| Case | Message sketch |
|------|----------------|
| Length mismatch (static) | `cannot zip tuples of length 2 and 3 with '+'` |
| Heterogeneous tuple | `element-wise '+' requires a homogeneous numeric tuple` |
| Non-numeric element | `element type 'string' does not support '+'` |
| Dynamic `[T] ⊕ [T]` | `cannot zip dynamic-length arrays with '+'; use fixed-length '[T; N]' or broadcast a scalar` |
| Array static⊕dynamic | `cannot mix fixed-length and dynamic arrays in '+'; convert both to '[T; N]' or broadcast a scalar` |
| Static `[T; N] ⊕ [T; M]` | `cannot zip arrays of length N and M with '+'` |

### 3.4 Side tables for codegen

Mirror existing patterns (`bound_operator_call_at`, `ForInKind`):

```text
enum AggregateArithKind {
  ZipTuple { arity },           // arity known; unroll if ≤ 4
  ZipArray { length: Static(N) }, // never Dynamic for zip
  BroadcastTuple { arity, scalar_on: Left|Right },
  BroadcastArray { length: Static(N) | Dynamic, scalar_on: Left|Right },
}
```

Record per `NodeId` / span so codegen does not re-infer under the
ID-misalignment caveats inside function bodies.

For `**`, record the same shape kind and select `Pow` / `PowF` (or the
scalar pow bound) per element.

### 3.5 Where to hook

| Site | File / fn | Change |
|------|-----------|--------|
| Binary arith (`+ - * / % **`) | `infer_arith` / pow arm | shape resolve + element constraint |
| Unary `-` | unary infer arm | element-wise for aggregates |
| `+=` / `**=` etc. | compound-assign infer | same shapes; LHS drives result |
| Exhaustiveness / pretty | `pretty.rs` | no change required |
| Unify | `unify.rs` | unchanged (result still normal `Tuple`/`Array`) |

---

## 4. Trait / generic integration (the “tower”)

### 4.1 What “full tower” means here

Three tiers of support:

| Tier | Example | Mechanism |
|------|---------|-----------|
| **A. Ground** | `(1,2)+(3,4)` | Hardwired shapes in `infer_arith` + codegen lower |
| **B. Element-generic** | `fn add2<T: Num>((T,T) a, (T,T) b)` | Shape rules + element constraint `T: Num`; codegen zips calling scalar `Add` (monomorphize or dict) |
| **C. Shape-generic** | `fn add<V: Num>(V a, V b) -> V` with `V = (int,int)` | **Requires** aggregate types to satisfy `Num`/`Add` |

Tier A+B deliver the user-facing vector math without lying about types.
Tier C is what makes `(int,int)` a first-class `Num` citizen in
`fn f<T: Num>(T,T)->T`.

### 4.2 Recommended approach for Tier C: **constraint lifting**

Do **not** enumerate instances per arity. At constraint discharge time:

```text
want: Add<(T, T, …, T)>     (homogeneous, arity n ≥ 1)
have: Add<T>
⇒ discharge Add<(T,…,T)> with a compiler-synthesized dictionary
  whose `add` method is a zip thunk over Add<T>::add
```

Same for `Sub`/`Mul`/`Div` and thus `Num` (empty methods; superclass
dict flattening already exists in `generics.rs`).

Arrays:

```text
want: Add<[T]> or Add<[T; N]>
have: Add<T>
⇒ synthesize zip/broadcast-aware thunk
```

**Coherence:** these are compiler-owned lifts for builtin aggregate
heads (already called out as non-local for user `impl`s in
`docs/reference/types.md`). Document that users cannot write
`impl Add for (int, int)`.

### 4.3 Alternative rejected for v1

Hand-written `impl Add<(int,int)>` thunks for arities 2..=4 only —
simpler codegen demos, but does not scale to Tier B/C or `[T]`.

### 4.4 Monomorphization

Ground calls with builtin bounds may already monomorphize to unboxed
opcodes (`monomorphize::candidate_for_call`). Extend candidates so
that ground `(int,int) + (int,int)` becomes the zip lowering (or a
specialized clone), not a pointer `ADD`.

Shared generic bodies that receive `V: Num` where `V` is an aggregate
must use the synthesized dict (CallIndirect), same ABI as today.

### 4.5 `byte`

Docs already note `byte` is not in `Num`/`Add`. Sequence:

1. Land aggregate tower for `int`/`float`.
2. Separately add `byte: Num` (or at least `Add`/`Sub`/…) with
   wrapping or checked semantics — **explicit follow-up**, not blocked
   on aggregates, but aggregates automatically pick it up via lifting
   once scalars do.

---

## 5. Codegen strategy

### 5.1 Phase 1 lowering (no new opcodes)

**Arity ≤ 4:** always emit an **unrolled** straight-line zip (locked).

**Arity > 4 (static only):** emit a counted loop over indices.

For zip of two tuples arity `n` (temps = slots), unrolled form:

```text
evaluate lhs → StorePop t0
evaluate rhs → StorePop t1
for i in 0..n:          // compile-time unroll when n ≤ 4
  LOAD t0; CONST i; Index
  LOAD t1; CONST i; Index
  <scalar op: ADD/ADDF/Pow/… or bound-op call>
MakeTuple n
```

Broadcast scalar on right:

```text
evaluate vec → t0
evaluate scalar → t1
for i in 0..n:          // same unroll rule
  LOAD t0; CONST i; Index
  LOAD t1
  <scalar op>
MakeTuple n
```

Arrays: same with `Index` / `MakeArray`, but **only** for
`ArrayLength::Static(N)`. Never emit a runtime `ArrayLen` compare for
zip — dynamic–dynamic zip is rejected in the typechecker.

Reuse patterns from `emit_for_in_tuple` (already materializes tuple
elements via `Index`).

Pow: use `Instruction::Pow` / `PowF` (or the existing scalar pow
codegen path) in the element op slot.

### 5.2 Expression::Add (and siblings) dispatch order

Update the existing priority in `compiler/src/lib.rs`:

1. Constant fold (unchanged)
2. String `+` (unchanged)
3. **NEW:** aggregate arith hint → zip/broadcast emitter
4. Bound operator / dict call (generic scalar / lifted)
5. Float vs int hardwired opcode

### 5.3 Optional VM fuse (later)

Append something like `ZipBin` / `BroadcastBin` only if benchmarks
show the `Index` loop is hot (e.g. large `[T]` in a tight loop).
Requires `ARCHIVE_VERSION` bump. **Not required for correctness.**

Peephole: fixed small-arity (≤ 4) zip is already unrolled by codegen
(§5.1); further peephole fusion of the unrolled `Index` convoys is
optional polish, not required for NT-1.

### 5.4 Compound assign

`a += b` for aggregates: load `a`, compute `a ⊕ b`, `StorePop` back
(or `SetField`/`StoreIndex` for nested — v1 only whole-binding LHS
identifiers / locals, same as scalar `+=` today).

---

## 6. Implementation phases

### Phase NT-0 — Stop the lie (prerequisite)

- In `infer_arith`, if either side is `Tuple`/`Array` and the zip
  rules are not yet implemented, **reject** with a clear diagnostic
  instead of unifying and emitting pointer `ADD`.
- Tiny diagnostic golden test.
- Unblocks “safe incompleteness” before full feature lands.

### Phase NT-1 — Homogeneous tuple zip (ground int/float)

- `classify` + zip for equal-arity homogeneous numeric tuples
- Codegen lowering (§5.1): **unroll arity ≤ 4**
- Ops: `+ - * / % **`, unary `-`
- Example: `examples/vec_tuple.hy` → `(2,2)` printed via `%v` or indices
- Pipeline + codegen tests

### Phase NT-2 — Array zip (static length only)

- Static `[T; N] ⊕ [T; N]` only
- **Hard-error** `[T] ⊕ [T]`, static/dynamic mix, and `N ≠ M`
- Example: `examples/vec_array.hy` using literals / `[T; N]` annotations
- Diagnostic goldens for dynamic zip rejection

### Phase NT-3 — Scalar broadcast

- `vec ⊕ scalar` / `scalar ⊕ vec` for tuples, `[T; N]`, and dynamic `[T]`
- Compound `+=` / `**=` with scalar RHS
- Tests for associativity / precedence unchanged (`(1,2)+3*4`)

### Phase NT-4 — Element-generic (Tier B)

- Open element `T: Num` inside fixed tuple/`[T; N]` shapes in signatures
- Codegen uses scalar bound-op / monomorphized ADD (etc.) per element
- Golden: `fn scale<T: Num>((T,T) v, T s) -> (T,T)`

### Phase NT-5 — Constraint lifting (Tier C)

**Prerequisite:** NT-1…3 have baked (examples + goldens green).

- Discharge `Add`/`Num`/… (and pow if modeled as a trait) for
  homogeneous tuple and `[T; N]` when the element satisfies the class
- Synthesize zip dictionaries / thunks
- Monomorphization candidates for ground aggregate `Num` calls
- Golden: `fn add<T: Num>(T a, T b) -> T` used at `(int,int)` and
  `[float; 2]` call sites
- Dynamic `[T]` does **not** lift for aggregate–aggregate zip; it may
  still lift for scalar ops only if we expose broadcast via the
  operator path (ground) rather than `T: Num` with `T = [int]`

### Phase NT-6 — Polish

- Docs: `docs/reference/operators.md`, `types.md`, tutorial 05,
  feature matrix / examples catalog
- `byte` in scalar `Num` (if not done elsewhere) so `[byte; N]` lifts
- Optional: further peephole on unrolled `Index` convoys; VM fuse only
  if needed
- AGENTS.md learned facts

---

## 7. Decisions

### Locked (user, 2026-07-25)

| # | Decision |
|---|----------|
| D1 | Dynamic `[T] ⊕ [T]` (and any aggregate–aggregate zip whose lengths are not equal static facts) → **compile-time hard error**. No runtime length check. |
| D3 | Element-wise `**` is **in** v1. |
| D5 | Codegen **always unrolls arity ≤ 4**; larger static arities use a loop. |
| D6 | Tier C / NT-5 lands **after** NT-1…3 bake. |

### Still open / minor

| # | Question | Recommendation |
|---|----------|----------------|
| D2 | `%` on floats in zip | **allow** — mirror scalars (`MODF`) |
| D4 | Empty tuple `()` | **error** — not numeric; empty `[T; 0]` ⊕ `[T; 0]` → `[]` OK if we allow arity 0 arrays |
| D7 | Dicts / records field-wise `+` | **out of scope** |

---

## 8. Files likely touched

| Area | Paths |
|------|--------|
| Infer / shapes | `compiler/src/typechecking/infer.rs` |
| Generics / lifting | `compiler/src/typechecking/generics.rs`, discharge helpers in `infer.rs` |
| Codegen | `compiler/src/lib.rs` (`Expression::Add`/…/`Pow`, new `emit_aggregate_arith`) |
| Monomorphize | `compiler/src/monomorphize.rs` |
| Diagnostics goldens | `compiler/tests/diagnostics.rs` |
| Pipeline goldens | `compiler/tests/pipeline.rs` |
| Examples | `examples/vec_tuple.hy`, `examples/vec_array.hy`, maybe `examples/vec_generic.hy` |
| Docs | `docs/reference/operators.md`, `types.md`, `tutorial/05-aggregates.md`, `docs/examples.md` |
| VM (only if fuse) | `common/src/opcode.rs`, `machine/src/vm.rs`, `ARCHIVE_VERSION` |

No parser changes expected for v1.

---

## 9. Test plan

### Unit / infer

- zip success `(int,int)`, `(float,float)`, `[int; 2] ⊕ [int; 2]`
- element-wise `**` on tuples/arrays
- arity / static length mismatch → hard error
- `[int] ⊕ [int]` (dynamic params) → hard error
- heterogeneous / non-numeric element → hard error
- broadcast left/right (including dynamic `[T] ⊕ T`)
- unary `-` on tuple/array
- Tier B: `(T,T)` with `T: Num`
- Tier C: `T: Num` instantiated at `(int,int)` and `[int; 2]`

### Codegen

- zip emits unrolled `Index` × 2n + scalar ops + `MakeTuple` for n ≤ 4
  (not a single `ADD` on pointers)
- arity 5+ uses a loop form
- broadcast emits one scalar load reused
- regression: `integer_arithmetic_emits_int_opcode` still scalar

### Pipeline / examples

| Program | Expected |
|---------|----------|
| `(1,1)+(1,1)` | `(2, 2)` or prints of components |
| `(2,3)**2` | `(4, 9)` |
| `(1,2)*3` | `(3, 6)` |
| `[1,2]+[3,4]` | `[4, 6]` (literals → `[int; 2]`) |
| dynamic `[int]` params zipped | compile error |
| `add_generic((1,2),(3,4))` (NT-5) | `(4, 6)` |
| existing `fib`, `aliases`, `for_in_tuple` | unchanged |

### Memory

- Run new examples under the usual 64MB limit; zip temps must not leak.

---

## 10. Risks

1. **ID / type cache misalignment** inside functions (known Access issue) —
   use an explicit arith-shape side table, not `lookup_at` alone.
2. **Peephole / fusion** assuming scalar `LOAD;LOAD;ADD` — zip sequences
   are different; ensure peephole does not mis-fuse across `Index`.
3. **GC / temps** — holding two aggregates in locals during zip is fine;
   avoid leaving stale heap roots on the operand stack.
4. **Tier C complexity** — synthesized dicts must match flattened
   superclass layout for `Num` (Add+Sub+Mul+Div method order).
5. **Coherence surprises** — document compiler-owned lifts early so
   users do not attempt `impl Add for (int, int)`.

---

## 11. Success criteria

- `(1, 1) + (1, 1)` evaluates to `(2, 2)`; no pointer arithmetic.
- `(2, 3) ** 2` evaluates to `(4, 9)`.
- Mismatched / heterogeneous / **dynamic-length zip** fail at
  typecheck with clear messages (NT-0 even before full zip).
- Static arrays and scalar broadcast (including `[T] ⊕ scalar`) work
  per §2.
- Codegen unrolls zip for arity ≤ 4.
- `fn add<T: Num>(T a, T b) -> T` works for `int`, `float`,
  homogeneous numeric tuples, and `[T; N]` numeric arrays (NT-5).
- Docs and examples catalog updated; `cargo test --workspace` green.

---

## 12. Suggested implementation order (checklist)

- [ ] NT-0 reject aggregate `+`/`**`/… without zip (incl. dynamic `[T]⊕[T]`)
- [ ] NT-1 tuple zip ground (`+ - * / % **`, unroll ≤ 4)
- [ ] NT-2 static array zip + hard errors for dynamic zip
- [ ] NT-3 broadcast + compound assign (bake before NT-5)
- [ ] NT-4 element-generic
- [ ] NT-5 constraint lifting + monomorphize (after NT-1…3 bake)
- [ ] NT-6 docs / byte / optional perf polish

Each phase: tests + minimal example before moving on; stage only
related files per commit.
