# Primitive casts (`expr as T`)

Narrowing conversions between `int`, `float`, `byte`, and `bool` (wrapping/truncation for non-literal values). Semantics match Rust for runtime casts:

- `float as int` truncates toward zero (not `round`/`floor`). `NaN` / `±inf` follow Rust `f64 as i64` (e.g. `NaN` → `0`).
- Non-literal `int as byte` keeps the low 8 bits (`let n = 257; n as byte` → `1`; negatives wrap the same way, e.g. `-1 as byte` when the operand is a variable).
- A **literal** `int as byte` outside `0..=255` is a compile-time type error (same message as a byte literal out of range).

Examples: `n as byte`, `f as int`, `flag as bool`. The same matrix is available via `Into` (`n.into()` when the target type is known). See `examples/casts.hy`.

`as` is a Pratt **postfix** operator (see [Operators](operators.md)): it binds tighter than arithmetic and assignment, so `c = m as byte` means `c = (m as byte)`, and `1 + 2 as float` means `1 + (2 as float)`.

---

## Related

- [Types](types.md)
- [Operators](operators.md)
