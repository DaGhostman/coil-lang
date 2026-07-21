# Feature plan — variadics, named params, let-destructuring, ranges

Status: **draft for syntax / scope review** (no implementation yet).

This plan covers four related ergonomics features. Each section lists
recommended syntax, how it maps onto today's compiler/VM, confidence,
and what to defer. **Please confirm the syntax choices marked
"VERIFY"** before any implementation branch starts.

---

## Baseline (what exists today)

| Area | Today |
|------|--------|
| Fn params | Positional only: `fn f(int a, int b)`; call `f(1, 2)` |
| Defaults / rest / named | **None** |
| Partial application | Typechecker allows under-application (`apply_function` leaves a `Fun`) |
| `let` | Single binding: `Fragment([Variable, rhs])` → `StorePop` |
| Destructuring | **Match patterns only** (tuple / record / nested); no `let` patterns |
| Tuples / arrays | Literals + index; `for x in` over arrays / homogeneous tuples |
| Iteration protocol | `IntoIterator` / `Iterator` + `ForInKind::{Array,Tuple,Dict,Coroutine,Custom}` |
| Ranges | Explicitly **not implemented** (`docs/reference/syntax.md`) |
| Lazy generators | Coroutines + user `Iterator` impls (see `examples/for_in_custom.0s`) |

Key files: `parser/src/{ast,lib}.rs`, `compiler/src/typechecking/infer.rs`
(`infer_function`, `apply_function`, `infer_fragment`, `ForInKind`),
`compiler/src/lib.rs` (Call / Fragment / for-in emitters),
`common/src/opcode.rs` (`CALL`, `StorePop`, `MakeTuple`, `MakeArray`).

---

## 1. Named parameters

### Recommended syntax (VERIFY)

```0s
fn greet(string name, int age) { ... }

// call site — all named
greet(name: "Ada", age: 36);

// call site — positional prefix, then named
greet("Ada", age: 36);

// NOT allowed (ambiguous / surprising with partial app):
// greet(age: 36, "Ada");
```

Declaration site stays `Type name` (unchanged). Names are the
parameter identifiers; there is no separate "external name".

### Why this shape

- Record constructors already reorder named fields to declaration
  order (`Construct` + `payload_tys_for`). Call-site named args can
  mirror that.
- Keeps decl grammar stable; only `Call` / `params()` grow an optional
  `name:` prefix per arg.

### Implementation sketch

1. **Parser:** each call arg becomes either positional `expr` or
   `IDENT ':' expr` → AST e.g. `CallArg { name: Option<&'a str>, value }`.
2. **Typechecker:** resolve callee arity/names from `parse_arg_list`
   side data (extend `codegen_var_types` / new `fn_param_names` map).
   Reorder + fill positions; reject unknown names, duplicates, and
   "positional after named".
3. **Codegen:** after reorder, emit the same left→right push + `CALL`
   as today (no new opcode).
4. **Docs / examples:** tutorial + `examples/named_args.0s`.

### Interactions / rules to lock

| Rule | Proposal |
|------|----------|
| Positional after named | **Reject** |
| Mixing with under-application | Named calls require **all remaining** params supplied (no partial named calls in v1) |
| Methods | Same rules; `self` stays implicit / never named |
| FFI / `invoke` | Out of scope (tuple form stays positional) |

### Confidence: **high** for v1 (reorder-only named call args)

Low risk: pure frontend + TC bookkeeping; bytecode unchanged.

### Defer

- Declaration-site renaming (`fn f(int x as width)`).
- Default values (see §2 coupling note).
- Partial application of named-only suffixes.

---

## 2. Variadics (rest parameters)

### Recommended syntax (VERIFY)

```0s
// Trailing rest only — packs into a dynamic array
fn sum(int... xs) -> int {   // or: fn sum(...[int] xs)
    let n = len(xs);
    // ...
}

sum(1, 2, 3);           // xs == [1, 2, 3]
sum();                  // xs == []
```

Alternative decl forms (pick one):

| Form | Pros | Cons |
|------|------|------|
| `int... xs` | Short, C++/JS-familiar | New `...` token in type position |
| `...[int] xs` | Makes `[int]` explicit | Noisier |
| `xs: [int]...` | — | Fights existing `Type name` order |

**Recommendation:** `Type... name` as a special `Argument` shape
(not a general type operator).

### Call-site spread (VERIFY — optional v1)

```0s
let xs = [1, 2, 3];
sum(...xs);             // unpack array into rest
sum(0, ...xs, 9);       // if we allow mid-list spread — harder
```

**Recommendation for v1:** support **decl rest only**; defer call-site
`...expr` spread (needs runtime arity + copy into the rest array, and
interacts badly with `CALL`'s compile-time packed arity).

### Implementation sketch

1. **AST:** `Argument` gains `is_rest: bool` (must be last param).
2. **Typechecker:** function type becomes
   `(T1 → … → Tn → [T] → R)` for rest of element `T`, **or** keep a
   non-curried "variadic call" form:
   - Prefer: **calls with rest are never partially applied**; TC
     treats rest as "zero-or-more trailing `T`" at the call site and
     synthesizes `[T]` for the body.
3. **Codegen:** at each call, emit trailing args into `MakeArray`,
   push that one value as the last formal, then `CALL` with
   `arity = fixed_count + 1`.
4. No new VM opcode if the rest value is always one stack slot
   (`[T]`).

### Coupling with named params

```0s
fn f(int a, int... xs) { }
f(a: 1, 2, 3);          // OK? or require xs: ...
f(a: 1, xs: [2, 3]);    // alternate — rest only by name as array
```

**v1 proposal:** rest is **positional-only** at the call site (cannot
be named); named args may only bind the fixed prefix. Avoids needing
a second way to pass the array.

### Confidence: **medium**

Rest-as-`[T]` + call-site packing is straightforward. Confidence drops
if we also want:

- mid-list spread,
- multiple rest params,
- interaction with curried under-application,
- rest of non-array type (tuples).

### Defer

- Call-site `...expr` spread.
- Variadic overloads / arity-based dispatch.
- Rest of tuple type `(T...)`.
- Combining rest + default args in the same function.

---

## 3. Let-destructuring

### Your sketch vs alternatives (VERIFY — please pick)

You wrote:

```0s
let a, b = ...(1, 2);
```

| Option | Example | Fit with today |
|--------|---------|----------------|
| **A. Tuple pattern (recommended)** | `let (a, b) = (1, 2);` | Reuses match `Pattern` / `PatternPayload::Tuple` |
| **B. Bare multi-bind** | `let a, b = (1, 2);` | Needs new Fragment shape; RHS must be tuple |
| **C. Explicit unpack** | `let a, b = ...(1, 2);` | Invents `...` as unary unpack; clashes with rest/spread meaning |
| **D. Record pattern** | `let { x, y } = p;` | Reuses record patterns; `p` must be record/dict/enum-record |

**Recommendation: A (+ D in the same phase).**

Reasons against **C**:

1. `...` is the natural token for rest/spread (§2); overloading it as
   "unpack RHS of let" is confusing.
2. Match already teaches `(a, b)` / `{ x, y }` patterns — `let` should
   use the same grammar.
3. Option A needs almost no new user mental model.

Also allow nested / wildcard / rest-in-pattern later:

```0s
let (a, b) = pair;
let (head, ...) = xs;          // DEFER — pattern rest
let Point::Point { x, y } = p; // DEFER or v2 — full constructor patterns in let
let { x, y } = { x: 1, y: 2 }; // v1 if dict/record
```

### v1 scope (recommended)

```0s
let (a, b) = (1, 2);
let (a, _) = t;
let { x, y } = d;              // Ty::Record / dict
```

**Out of v1:** enum constructor patterns in `let` (keep using `match`),
nested record-in-tuple let patterns that need `UnpackAt` edge cases,
pattern rest (`...`).

### Implementation sketch

1. **Parser:** extend `variable()` / add `let_pattern`:
   `let` + pattern + `=` + expr + `;`
   → AST e.g. `Expression::LetPattern { pattern, rhs }`
   (cleaner than overloading `Fragment`).
2. **Typechecker:** reuse `infer` pattern helpers from match
   (bind names in current frame, unify pattern type with RHS).
3. **Codegen:**
   - Tuple: RHS → temp slot (or keep on stack) → `Index`/`LOAD` each
     element → `StorePop` per binding **or** emit `MakeTuple`-inverse
     via existing `Index` for small arities.
   - Prefer for tuples: compile to `let tmp = rhs; let a = tmp[0]; …`
     desugar in codegen (no new opcode) for v1.
   - Record/dict: `GetField` / `LoadField` per name → `StorePop`.
4. Irrefutable patterns only in v1 (no `Option::Some(x)` in `let` —
   that needs failure semantics).

### Confidence: **high** for irrefutable tuple/record `let (a,b)` / `let {x,y}`

Pattern + binding machinery exists; desugaring to index/field loads
avoids VM work. Confidence **medium** for nested constructor lets;
**low** for refutable lets without `match`-like exhaustiveness.

### Defer

- `let a, b = ...(1, 2)` syntax (option C).
- Refutable `let` / `let else`.
- Pattern rest / slice patterns.
- Enum constructor lets (use `match`).

---

## 4. Range types (static vs lazy)

### Goal (as stated)

Ranges that are either:

1. **Statically created** — both bounds known at compile time, or
2. **Lazy** — resolve to a generator / iterator of the element type.

### Recommended surface syntax (VERIFY)

```0s
0..10        // half-open Range { start: 0, end: 10 }  → 0..9
0..=10       // closed RangeInclusive                 → 0..10
a..b         // runtime bounds (same type)
```

Precedence: `..` / `..=` below comparisons, non-associative
(reject `a..b..c`).

### Type model (recommended)

```0s
// Prelude nominals (compiler-provided, like Iterator), not userland:
struct Range<T> { start: T, end: T }             // half-open
struct RangeInclusive<T> { start: T, end: T }
```

v1: **`T = int` only** (matches `for_in_custom.0s` Counter).

Lazy semantics:

```0s
impl IntoIterator for Range<int> { ... }
impl Iterator for RangeIter { type Item = int; fn next(...) -> Option<int>; }
```

So `for x in 0..10 { }` uses existing `ForInKind::Custom` **or** a
new fast-path `ForInKind::Range` (better perf, no heap iterator — same
idea as array for-in).

### "Statically created" — two interpretations

| Meaning | Proposal |
|---------|----------|
| **S1. Const bounds → type-level length** | `0..5` has known length 5; may coerce to `[int; 5]` via explicit `collect` **or** stay a range value whose length is `Static(5)` for OOB-style checks | Optional stretch |
| **S2. Const bounds → eager array materialization** | `let xs = [0..5];` or `array(0..5)` builds `[0,1,2,3,4]` at compile/runtime | Separate API; don't make `0..5` itself an array |

**Recommendation:** ranges are **always lazy values** (cheap struct /
iterator). Add an explicit conversion later:

```0s
let xs = collect(0..5);   // → [int] dynamic, or [int; 5] if const
```

Do **not** make `0..n` silently allocate an array — that surprises
anyone writing `for x in 0..1_000_000`.

If you specifically want a static array sugar, prefer a distinct form
(VERIFY):

```0s
let xs = [0..5];          // sugar for fixed array fill — DEFER or separate feature
```

### Implementation sketch (lazy int ranges — v1)

1. **Parser:** infix `..` / `..=` → `Expression::Range { start, end, inclusive }`.
2. **Typechecker:** unify start/end as `int`; result `Ty::Con("Range")`
   or a small `Ty::Range { inclusive, elem }` if we want length
   metadata for const bounds.
3. **Runtime:** either
   - **(A)** class instance + prelude `impl Iterator` (works today with
     Custom for-in), or
   - **(B)** `ForInKind::Range` codegen: counter in locals, compare,
     increment — **no heap**, best for `for x in 0..n`.
4. Prefer **(B)** for the common `for` case + **(A)** so `0..n` is a
   first-class value you can pass around / `resume`-style consume via
   `next`.

Coroutine alternative (`async fn range(s,e) { while s < e { yield s; s = s + 1; } }`)
already works for laziness but is heavier; keep as user workaround,
not the builtin.

### Confidence

| Piece | Confidence |
|-------|------------|
| Lazy `0..n` / `0..=n` + `for x in` (`ForInKind::Range`) | **high** |
| First-class `Range` value + `Iterator` impl | **high** (pattern already in tree) |
| Const-length / `[int; N]` materialization | **medium** — defer to stretch |
| `Range<T>` for arbitrary `Ord` / floats / chars | **low** — **defer** |
| Float ranges with step | **low** — **defer** |
| Decreasing ranges (`10..0`) without explicit step | Define as empty (Rust-like) in v1 |

### Defer

- Generic `Range<T: Ord>`.
- Step syntax (`0..10 step 2`) — use C-style `for` or a `range_step` helper later.
- Eager `[0..n]` array sugar until collect API exists.
- Infinite ranges / open-ended `0..`.

---

## Suggested delivery order

Implement in this order so each piece lands on stable ground:

```
Phase P1 — Let destructuring (tuple + record, irrefutable)
           [high confidence, unblocks ergonomic examples]

Phase P2 — Named call-site args (reorder only)
           [high confidence, no VM changes]

Phase P3 — Lazy int ranges (`..` / `..=` + for-in fast path)
           [high confidence; docs already list the gap]

Phase P4 — Variadic rest (`Type... name` → `[T]`)
           [medium; do after named so arity rules are settled]

Stretch   — collect(range) / const-length arrays
           — call-site spread `...xs`
           — defaults (not in this plan — needs arity + partial-app design)
```

### Explicitly out of this plan

| Feature | Why |
|---------|-----|
| Default parameter values | Needs call-arity lowering + interaction with partial application; design not locked |
| Refutable `let` / `let else` | Needs failure path; use `match` |
| Mid-list / multi spread | Runtime arity explosion |
| Generic / float ranges | Low confidence without `Ord` + step story |

---

## Decision checklist (please reply)

Confirm or rewrite each:

1. **Named calls:** `f(a: 1, b: 2)` with positional-then-named only — OK?
2. **Rest decl:** `fn sum(int... xs)` vs `fn sum(...[int] xs)` — which?
3. **Rest at calls:** defer `sum(...xs)` spread — OK?
4. **Destructure:** prefer `let (a, b) = (1, 2);` over `let a, b = ...(1, 2);` — OK?
5. **Record let:** include `let { x, y } = d;` in same phase as tuple let — OK?
6. **Ranges:** half-open `a..b` + closed `a..=b`, int-only, always lazy; no silent array alloc — OK?
7. **Static ranges:** stretch `collect(0..n)` later, not part of v1 — OK?

Once these are locked, implementation can proceed phase-by-phase with
HM diagnostics, a minimal example per feature, and docs updates under
`docs/` as usual.

---

## Confidence summary

| Feature | Ship readiness | Notes |
|---------|----------------|-------|
| Named params (call reorder) | Ready to implement | After syntax OK |
| Let tuple/record destructure | Ready to implement | Prefer pattern syntax A/D |
| Lazy int ranges + `for` | Ready to implement | Fast-path or Iterator |
| Variadic rest → `[T]` | Implement after P2 | Medium; keep rules tight |
| Call-site spread / defaults / generic ranges / unpack `...` let | **Defer** | Low confidence or design conflict |
