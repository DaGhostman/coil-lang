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

1. If `a` is an `int`/`byte` and `a <= COIL_PAR_THRESHOLD` (default **20**),
   evaluate sequentially (avoids drowning the pool in tiny leaf tasks).
2. Otherwise `MakeFn` of `f`, then `thread_spawn(f, a)` (`HostInvoke`) into the
   **work-stealing reactor**.
3. On `Ok(handle)`: evaluate `f(b)` on the current thread, `join(handle)`
   (join **help-steals** while waiting), apply `⊕`.
4. On `Err`: fall back to sequential `f(a) ⊕ f(b)`.

Only one arm is submitted per binop so recursion does not double work per level.

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
| `COIL_PAR_THRESHOLD` | Int arg ceiling for sequential fallback (default 20). |
