# Automatic parallelization

coil can fork-join **independent parallel arms** (IPA) without a source-level
`par` / `spawn` annotation. Two shapes qualify today:

```coil
return fib(n - 1) + fib(n - 2);      // recursive IPA — sibling self-calls
while i < 100 { acc = acc + f(i); i = i + 1; }   // loop IPA — iteration arms
```

Both go through the same four gates — **purity**, **independence**,
**profitability**, **semantic identity** — and both are recognized structurally.
There are no function, module or program allowlists: a shape either proves out or
stays sequential.

## Purity analysis

After typecheck, codegen runs [`purity`](../../compiler/src/typechecking/purity.rs)
over the AST:

- A function is **locally impure** if it uses `panic` / `yield` / FFI / `defer`,
  mutates via index/field assignment, or calls a non-identifier callee.
- Calls to names that are not user `fn`s (e.g. imported `write_all`, `spawn`)
  are impure.
- Impurity propagates through the user-function call graph (fixed point).
- `analyze_pure_fns` returns everything that survives; `analyze_recursive_pure`
  keeps only the subset that **calls itself**.

Recursive IPA needs recursive-pure (the arms *are* self-calls). Loop IPA only
needs pure, so ordinary helpers such as `fn sq(int i) -> int { i * i }` qualify.

Disable both transforms with `COIL_AUTO_PAR=0` (or `false` / `off` / `no`).

## Recursive IPA: static profitability (no runtime threshold checks)

[`par_profit`](../../compiler/src/typechecking/par_profit.rs) detects **fork
sites** on recursive-pure functions — expressions whose operands are two or more
independent self-calls — and collects **constant** call-site arguments
(`fib(32)`, …). Three combine shapes are recognized:

| Combine | Source shape |
|---|---|
| `BinOp` | `f(n - a) ⊕ f(n - b)` |
| `EnumCtor` | `E::V(f(…), f(…))` |
| `SelfCall` | `f(f(…), f(…), f(…))` (tak-style) |

Arms are described structurally (`ArgForm::Const` / `Param` / `ParamMinus`), so
any arity works and child arg vectors are derived statically.

For each demanded constant argument vector whose **cost** (max component)
exceeds `COIL_PAR_THRESHOLD` (default **20**), and that still reaches the fork
under the site's path guards, codegen emits a nullary specialization
`__coil_par_{f}_{a}_{b}_…` that **always** forks:

1. `MakeFn` of a child specialization when one exists for an arm's derived args,
   otherwise `MakeFn` of the original `f` with those concrete args.
2. `thread_spawn` the first arm into the work-stealing reactor (no `GT` gate).
3. On `Ok(handle)`: evaluate remaining arms locally, `join` (help-steals), apply
   the site's combine (`BinOp`, rebuild `SelfCall`, or `MakeEnum` for `EnumCtor`).
4. On `Err` (spawn or non-sendable join): sequential fallback of all arms + combine.

Call sites with matching const args rewrite to `CALL` the specialization.
Below-threshold / dynamic args stay on the original sequential `f` (no hot-path
runtime threshold tax).

## Loop IPA: chunked fork-join over an induction range

A counted loop is the same idea with the arms spread over an induction range.
When the iterations only communicate through one **associative** reduction, any
partition of the range folds to the sequential result, so the range splits into
contiguous chunks that each accumulate a private partial.

[`loop_par`](../../compiler/src/typechecking/loop_par.rs) admits a `while` loop
only when **every** gate holds:

| Gate | Requirement |
|---|---|
| Shape | `while i < K` / `i <= K`; body is a statement list |
| Induction | exactly one `i = i + 1` / `i += 1` / `i++`; `i` is a const-initialized local |
| Trip count | `K` is compile-time, and `end - begin > COIL_PAR_THRESHOLD` |
| Reduction | exactly one `acc = acc + e` / `acc = acc * e` (or `+=` / `*=`) on a const-initialized local |
| Independence | `e` never reads `acc`; the body reads only `i`, its own `let` temps and int literals |
| Purity | body calls only pure user functions; no index / field / static writes, no branches, `break`, `return` or `yield` |
| Types | the induction variable and `e` both infer to `int` — float reduction is not associative |

Ranges are normalized half-open (`i <= K` becomes `end = K + 1`), so a split is
just a partition of `[begin, end)`.

Codegen emits one private **chunk worker** per site,
`__coil_par_loop_{n}(lo, hi, acc)`, holding the original body over `[lo, hi)` and
returning the partial. At the loop site:

1. `MakeFn` the worker, then `thread_spawn(worker, mid, end, identity)` — the
   upper chunk starts from the operator's identity (`0` for `+`, `1` for `*`) so
   the accumulator's initial value is counted exactly once.
2. Call the worker inline for `[begin, mid)` seeded with the live `acc`.
3. `thread_join` (help-steals), then fold the two partials with `ADD` / `MUL`.
4. Store the fold into `acc` and set `i` to `end`, the value the sequential loop
   would have left behind.

On a failed spawn or join, a single worker call covers `[begin, end)`.

## Deferred

- C-style `for (let i = 0; i < N; i = i + 1)` — the analysis shape is the same,
  but the step lives outside the body so the worker needs a second emit path.
  (Const trip counts up to 8 already fully unroll, well below the threshold.)
- Dynamic trip counts. Splitting `while i < n` needs a runtime `n > threshold`
  branch; the first slice refuses to pay that tax.
- More than two chunks, and nested / recursive chunking.
- Conditionals in the body, float reductions, and reductions over `min` / `max`
  or user operators.

## Work-stealing reactor

[`machine/src/reactor.rs`](../../machine/src/reactor.rs) owns a fixed pool of OS
threads (size = [`WorkerCap`](../../machine/src/thread.rs), default
`available_parallelism`). Jobs land on a crossbeam injector / local deques;
idle workers steal. `thread::spawn` / auto-par share this pool — no per-call
`std::thread::spawn`.

| Env | Effect |
|-----|--------|
| `COIL_MAX_WORKER_THREADS` | Pool size (1..=512). Default `available_parallelism` (min 2). |
| `COIL_AUTO_PAR` | `0` / `false` / `off` / `no` disables auto fork-join codegen. |
| `COIL_PAR_THRESHOLD` | Compile-time profitability cutoff — recursion arg size and loop trip count (default 20). |
