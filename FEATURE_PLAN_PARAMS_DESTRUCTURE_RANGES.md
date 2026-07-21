# Feature plan — variadics, named params, let-destructuring, ranges

Status: **implemented** (P1 let-destructure, P2 named args, P3 ranges, P4 rest, generic `Range<T: Ord>` + float for-in) — call-site spread still deferred.

This plan covers four related ergonomics features. Decisions below
reflect user confirmation; see [Locked decisions](#locked-decisions).

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

### Syntax (locked)

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

### Syntax (locked)

```0s
// Trailing rest only — packs into a dynamic array
fn sum(int... xs) -> int {
    let n = len(xs);
    // ...
}

sum(1, 2, 3);           // xs == [1, 2, 3]
sum();                  // xs == []
```

Decl form is `Type... name` as a special `Argument` shape (not a
general type operator). Works for any element type (`int...`,
`byte...`, `string...`, etc.).

### Call-site spread — deferred

```0s
let xs = [1, 2, 3];
sum(...xs);             // NOT in v1
```

Decl rest only for now. Call-site `...expr` needs runtime arity +
copy into the rest array and fights `CALL`'s compile-time packed
arity.

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

### Syntax (locked)

Tuple + record patterns on `let` (same grammar as match irrefutable
shapes). Rejected: `let a, b = ...(1, 2)` (`...` reserved for rest).

```0s
let (a, b) = (1, 2);
let (a, _) = t;
let { x, y } = d;              // Ty::Record / dict
```

Deferred for later:

```0s
let (head, ...) = xs;          // pattern rest
let Point::Point { x, y } = p; // enum constructor patterns in let
```

### v1 scope

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

### Syntax (locked)

```0s
0..10        // half-open  → yields 0,1,…,9
0..=10       // closed     → yields 0,1,…,10
a..b         // runtime bounds (same element type)
```

Precedence: `..` / `..=` below comparisons, non-associative
(reject `a..b..c`).

### Element types (locked)

**`T: Ord`** for construction (`Range<T>` / `RangeInclusive<T>`). Both
bounds unify. Builtins with `Ord`: `int`, `byte`, `float`. **`for`
iteration** steps `+1` / `+1.0` for `int` / `byte` / `float` only;
other `Ord` types may form values but are not iterable yet.

### Type model

```0s
// Prelude nominals (compiler-provided), not userland:
// Range<T: Ord> / RangeInclusive<T: Ord>
```

Lazy semantics via `IntoIterator` / `Iterator`, **or** a
`ForInKind::Range` fast path for `for x in 0..n` (preferred for
perf — no heap iterator).

### What “static” meant (clarification — not a separate feature)

Your original ask mentioned ranges that are “statically created **OR**
resolve to a generator lazily.” That was two possible *implementations*
of the same syntax, not two user-facing APIs:

| Reading | Meaning | Locked choice |
|---------|---------|---------------|
| Lazy (always) | `0..n` is a small range value; `for` / `next` pulls one element at a time | **This is v1** |
| “Static” / eager | Somehow turn `0..5` into the array `[0,1,2,3,4]` up front | **Not implied by `0..n`** |

So: writing `0..1_000_000` must **not** allocate a million-element
array. A later helper (name TBD — e.g. `collect(0..5)` → `[int]`)
could materialize when you *ask* for an array. That helper is
**optional stretch**, not required for ranges to ship.

If you never need “range → array”, we simply never add `collect`.

### Implementation sketch (lazy int/byte ranges — v1)

1. **Parser:** infix `..` / `..=` → `Expression::Range { start, end, inclusive }`.
2. **Typechecker:** unify start/end as `int` or `byte` (same for both);
   result is a range type carrying that element type.
3. **Runtime:** either
   - **(A)** class instance + prelude `impl Iterator` (works today with
     Custom for-in), or
   - **(B)** `ForInKind::Range` codegen: counter in locals, compare,
     increment — **no heap**, best for `for x in 0..n`.
4. Prefer **(B)** for the common `for` case + **(A)** so `0..n` is a
   first-class value you can pass around / consume via `next`.

Coroutine alternative (`async fn range(s,e) { while s < e { yield s; s = s + 1; } }`)
already works for laziness but is heavier; keep as user workaround,
not the builtin.

### Confidence

| Piece | Confidence |
|-------|------------|
| Lazy `0..n` / `0..=n` + `for x in` (`ForInKind::Range`) | **high** |
| `byte` ranges (same codegen, `byte` element type) | **high** (byte is already a 0..=255 immediate) |
| First-class `Range` value + `Iterator` impl | **high** (pattern already in tree) |
| Optional later: materialize range → `[T]` | **medium** — only if needed |
| `Range<T: Ord>` construction + float for-in step | **done** (non-numeric Ord for-in still deferred) |
| Float ranges with custom step | **low** — **defer** |
| Decreasing ranges (`10..0`) without explicit step | Define as empty (Rust-like) in v1 |

### Defer

- Iterating non-numeric `Ord` types (needs `Add`/`Step` story).
- Step syntax (`0..10 step 2`) — use C-style `for` or a `range_step` helper later.
- Eager `[0..n]` array sugar / range→array materialize (only if ever needed).
- Infinite ranges / open-ended `0..`.

---

## Suggested delivery order

Implement in this order so each piece lands on stable ground:

```
Phase P1 — Let destructuring (tuple + record, irrefutable)
           [high confidence, unblocks ergonomic examples]

Phase P2 — Named call-site args (reorder only)
           [high confidence, no VM changes]

Phase P3 — Lazy `int`/`byte` ranges (`..` / `..=` + for-in fast path)
           [high confidence; docs already list the gap]

Phase P4 — Variadic rest (`Type... name` → `[T]`)
           [medium; do after named so arity rules are settled]

Stretch   — optional range→array materialize (only if wanted)
           — call-site spread `...xs`
           — defaults (not in this plan — needs arity + partial-app design)
```

### Explicitly out of this plan

| Feature | Why |
|---------|-----|
| Default parameter values | Needs call-arity lowering + interaction with partial application; design not locked |
| Refutable `let` / `let else` | Needs failure path; use `match` |
| Mid-list / multi spread | Runtime arity explosion |
| Custom-step / non-numeric Ord for-in | Needs `Add`/`Step` story |
| Call-site `...xs` spread | Deferred by decision |
| Silent eager array from `0..n` | Surprising; ranges stay lazy |

---

## Locked decisions

| # | Decision |
|---|----------|
| 1 | Named calls: `f(a: 1, b: 2)`; positional prefix then named; no positional after named |
| 2 | Rest decl: `fn sum(int... xs)` (`Type... name`) |
| 3 | Call-site `sum(...xs)` spread: **deferred** |
| 4 | Destructure: `let (a, b) = …` (not `let a, b = ...(… )`) |
| 5 | Same phase: `let { x, y } = d;` for records/dicts |
| 6 | Ranges: `a..b` / `a..=b`, elements **`int` or `byte`**, always lazy |
| 7 | “Static” ≠ auto-array; `0..n` never allocates the full sequence. Optional later materialize helper only if needed — **not required to ship ranges** |

Implementation can proceed phase-by-phase with HM diagnostics, a
minimal example per feature, and docs updates under `docs/`.

---

## Confidence summary

| Feature | Ship readiness | Notes |
|---------|----------------|-------|
| Named params (call reorder) | Ready | Locked |
| Let tuple/record destructure | Ready | Locked |
| Lazy `int`/`byte` ranges + `for` | Ready | Locked |
| Variadic rest → `[T]` | After P2 | Medium; keep rules tight |
| Call-site spread / defaults / generic ranges / unpack `...` let | **Defer** | By decision or low confidence |
