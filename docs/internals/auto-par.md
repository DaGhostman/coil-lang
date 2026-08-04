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

## Codegen shape

For `f(a) ⊕ f(b)` where `f` is unary recursive-pure and `a` is structurally
sendable:

1. `MakeFn` of `f`, then `thread_spawn(f, a)` (`HostInvoke`).
2. On `Ok(handle)`: evaluate `f(b)` on the current thread, `join(handle)`,
   apply `⊕`.
3. On `Err` (including `WouldBlock` when the worker cap is full): fall back to
   sequential `f(a) ⊕ f(b)`.

Only one arm is spawned so recursion depth does not double the thread count
per level. There is **no** work-stealing reactor.

## Worker cap

Each root `Machine` owns a [`WorkerCap`](../../machine/src/thread.rs) shared
with nested workers. `thread_spawn` try-acquires a slot; on failure it returns
`thread::Error::WouldBlock`. The slot is released when the OS worker finishes
(not only when `join` returns).

| Env | Effect |
|-----|--------|
| `COIL_MAX_WORKER_THREADS` | Max concurrent OS workers for that VM (1..=512). Default `2 * available_parallelism` (min 2). |
| `COIL_AUTO_PAR` | `0` / `false` / `off` / `no` disables auto fork-join codegen. |

Explicit `spawn` uses the same cap. Auto-par treats `WouldBlock` as a soft
signal to stay sequential.
