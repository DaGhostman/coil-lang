# Primitive casts (`expr as T`)

Narrowing conversions between `int`, `float`, `byte`, and `bool` (wrapping/truncation for non-literal values). Semantics match Rust for runtime casts:

- `float as int` truncates toward zero (not `round`/`floor`). `NaN` / `±inf` follow Rust `f64 as i64` (e.g. `NaN` → `0`).
- Non-literal `int as byte` keeps the low 8 bits (`let n = 257; n as byte` → `1`; negatives wrap the same way, e.g. `-1 as byte` when the operand is a variable).
- A **literal** `int as byte` outside `0..=255` is a compile-time type error (same message as a byte literal out of range).

Examples: `n as byte`, `f as int`, `flag as bool`, `"/" as byte`. The same matrix is available via `Into` (`n.into()` when the target type is known). See `examples/casts.hy`.

`as` is a Pratt **postfix** operator (see [Operators](operators.md)): it binds tighter than arithmetic and assignment, so `c = m as byte` means `c = (m as byte)`, and `1 + 2 as float` means `1 + (2 as float)`.

A **string literal** whose UTF-8 encoding is exactly one byte coerces to `byte` under an expected `byte` (or via `"/" as byte`). Escapes like `"\n"` and `"\""` work; multi-byte literals are a type error for `byte`.

The same literal also coerces to **`Vec<byte>` / `[byte; N]`** (full UTF-8 byte sequence) under an expected byte-buffer type or via `"hi" as Vec<byte>` / `"hi" as [byte; 2]`. Fixed `[byte; N]` requires exactly `N` decoded bytes. Literals rewrite at compile time to `CONST` + `MakeArray` (or the `Vec` path when the expectation is `Vec<byte>`).

A **non-literal** `string` may use `s as Vec<byte>`, which lowers to `string::to_bytes`. Fixed `[byte; N]` still requires a literal (or call `to_bytes` and check length yourself). `Vec<byte> as string` is not a total cast — keep fallible `from_bytes`.

---

## Related

- [Types](types.md)
- [Operators](operators.md)
