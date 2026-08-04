# Automatic parallelization

coil can fork-join **pure self-recursive** binary ops such as

```coil
return fib(n - 1) + fib(n - 2);
```

without a source-level `par` / `spawn` annotation.

## Purity analysis

After typecheck, codegen runs [`analyze_recursive_pure`](../../compiler/src/typechecking/purity.rs)
over the AST:

- A function is **locally impure** if it uses `panic` / `yield` / FFI / `defer`,
  mutates via index/field assignment, or calls a non-identifier callee.
- Calls to names that are not user `fn`s (e.g. imported `write_all`, `spawn`)
  are impure.
- Impurity propagates through the user-function call graph (fixed point).
- Remaining functions that **call themselves** are **recursive-pure** and
  eligible for auto-par.

Disable the transform with `COIL_AUTO_PAR=0` (or `false` / `off` / `no`).

## Static profitability (no runtime threshold checks)

[`par_profit`](../../compiler/src/typechecking/par_profit.rs) detects the unary
shape `f(n - a) ⊕ f(n - b)` on recursive-pure functions and collects **constant**
call-site arguments (`fib(32)`, …).

For each demanded `N > COIL_PAR_THRESHOLD` (default **20**), codegen emits a
nullary specialization `__coil_par_{f}_{N}` that **always** forks:

1. `MakeFn` of `__coil_par_f_{N-a}` when that level is also specialized,
   otherwise `MakeFn` of the original `f` with constant arg `N-a`.
2. `thread_spawn` into the work-stealing reactor (no `GT` gate).
3. On `Ok(handle)`: evaluate the other arm (spec or `f(N-b)`), `join`
   (help-steals), apply `⊕`.
4. On `Err`: sequential fallback of both arms.

Call sites `f(N)` rewrite to `CALL __coil_par_f_N`. Levels at or below the
threshold stay on the original sequential `f`. Dynamic `f(n)` with unknown `n`
is **not** auto-parallelized (correctness-preserving; no runtime check tax).

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
| `COIL_PAR_THRESHOLD` | Compile-time profitability cutoff for specialization (default 20). |
