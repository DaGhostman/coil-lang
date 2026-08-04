# Recursion stack bounds

The VM pre-allocates an operand stack sized from this analysis:

- non-recursive programs → [`DEFAULT_OPERAND_STACK_SLOTS`](../../compiler/src/typechecking/stack_bound.rs) (256)
- proven / attributed recursion → `max_frames × 16 + 16` (clamped to
  [`MAX_OPERAND_STACK_SLOTS`](../../machine/src/lib.rs))

[`Machine::with_operand_capacity`](../../machine/src/vm.rs) builds the VM; reactor
workers resize when a job needs a larger stack.

## Analysis

After typecheck, [`analyze_stack_bounds`](../../compiler/src/typechecking/stack_bound.rs)
walks the AST:

1. Build the user-function call graph and find **cycles** (self or mutual).
2. For each recursive function, try to prove a finite **frame depth**:
   - **Tail-only** self-calls (`return f(...)`) → depth `1` (matches `TailCall`).
   - **Binary measure** (same shape as auto-par): `f(n-a) ⊕ f(n-b)` with
     `if n <= K` / `n < K`, and **every** entry call site is a constant int
     (`fib(10)`, `fib(32)`). Depth ≈ `((max_n - K) / min(a,b)) + 1`.
   - **Unary measure**: `f(n-k)` (or `n * f(n-k)`, …) with the same base-case
     pattern and constant entries.
3. If depth is unprovable, require `#[max_depth(N)]` on that function.

Dynamic arguments (`fib(k)`), FFI / opaque callees, mutual recursion without a
self-measure, and unrecognized shapes are **unprovable** — not rejected
blindly when a bound *can* be shown.

## Attribute

```coil
#[max_depth(64)]
fn walk(int n) -> int {
    // …
}
```

`N` is a positive integer upper bound on simultaneous call frames of that
recursive function. Valid only on `fn` (see [Syntax — Attributes](../references/syntax.md#attributes)).

## Relation to auto-par

[`par_profit`](../../compiler/src/typechecking/par_profit.rs) reuses the same
binary shape detector for fork-join specialization. Stack-bound analysis runs
even when `COIL_AUTO_PAR=0`, and applies to impure recursive functions too.
