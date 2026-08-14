# Error handling: Result, Option, raise, ?, ??, ?.

coil ships compiler-built-in **`Option`** and **`Result`** sum types, plus operators that desugar to ordinary `match` / `return` (no new VM opcodes).

This chapter builds on [Enums and Match](03-enums-and-match.md).

---

## Built-in types

You do **not** declare these yourself — they live in the virtual `prelude` module and are auto-imported into every file. Redeclaring `enum Option { ... }` or `enum Result { ... }` while the short name is still prelude-bound is a duplicate-enum error (free the name first with `use prelude::Option as PreludeOption;`).

| Type | Variants | Meaning |
|------|----------|---------|
| `Option` | `None`, `Some(T)` | Present or absent value |
| `Result` | `Ok(T)`, `Err(E)` | Success or failure |

Construct and match them like any other enum:

```coil
fn unwrap(Option o) -> int {
    return match o {
        Option::None => 0,
        Option::Some(v) => v,
    };
}
```

Annotations may use type applications: `Option<int>`, `Result<int, string>`.

---

## `raise` and result mode

`raise e` early-returns `Result::Err(e)` from the enclosing function.

A function enters **result mode** when it uses `raise`, uses `?` on a `Result`, or is annotated `-> Result<...>`. In result mode, ordinary `return v` (and success paths) are wrapped as `Result::Ok(v)` by codegen.

Do **not** chain postfix `?` after `raise` (`raise err?` parses as `raise (err?)`). Write `raise err;` — `raise` already early-returns `Err`.

```coil
fn parse_pos(int n, int is_neg) {
    if is_neg == 1 {
        raise "neg";
    }
    return n; // becomes Ok(n)
}
```

One error type **`E` per function**. Mixing `raise "s"` with `raise 1` (or `?` on Results with different `E`) is a type error — there are no `string | int` unions.

Annotating `-> int` (or any non-`Result` type) while using `raise` / Result-`?` is also a type error.

---

## `?` — strict propagation

Postfix `x?` unwraps success and propagates failure:

| Operand | Enclosing return | Expression type |
|---------|------------------|-----------------|
| `Result<T, E>` | `Result<T, E>` (same `E`) | `T` |
| `Option<T>` | `Option<T>` | `T` |
| neither | any | **hard type error** (E0114) |

```coil
fn double_pos(int n, int is_neg) {
    let v = parse_pos(n, is_neg)?;
    return v * 2;
}
```

Do **not** mix Option-`?` and Result-`?` in the same function without a clear matching return type — that is a conflicting-error-type diagnostic.

---

## `??` — coalesce (Option and Result)

`a ?? b` evaluates to the unwrapped success value, or `b` on failure:

| LHS | On success | On failure |
|-----|------------|------------|
| `Option<T>` | `Some` payload | `b` (must unify with `T`) |
| `Result<T, E>` | `Ok` payload | `b` — **`Err` is discarded** |

```coil
let a = Option::None ?? "bar";       // "bar"
let b = Result::Err("boom") ?? 7;    // 7 — error swallowed
```

Prefer `?` or `match` when failure must be observed. Coalesce on a non-Option/non-Result value is a type error (E0115).

---

## `?.` — optional field access (Option only)

`a?.field` requires `a : Option<R>` where `R` has field `field : U`. The expression type is `Option<U>`:

- `None` → `None`
- `Some(x)` → `Some(x.field)`

```coil
use io::{stdout};
use io::sync::{write_all};
use string::{format, to_bytes};
let some = Option::Some({ v: 42 });
let none = Option::None;
write_all(stdout(), to_bytes(format("%i,", show(some?.v))));   // Some(42) → unwrap in helper
write_all(stdout(), to_bytes(format("%i", none?.v ?? 0)));     // None → 0
```

`?.` on `Result` or a non-Option is a type error (E0116). Use `?` then `.`, or `match`.

---

## `assert` and `panic`

`assert(cond)` / `assert(cond, msg)` (from `prelude::test`, auto-imported) returns `Result<(), string>` — use `?` or `match` like any other Result. It does not abort.

`panic msg` aborts immediately with `panic: <msg>` (CLI exit code 1). Prefer `assert` when failure should be recoverable.

See [Built-ins — assert / panic](../../references/assert.md) and `examples/assert.hy` / `examples/panic.hy`.

---

## Worked example

See `examples/raise_try.hy` (output `10,neg`), `examples/coalesce.hy` (`bar,hi,7,9`), and `examples/optional_chain.hy` (`42,0`).

---

## Related documents

| Document | Contents |
|----------|----------|
| [Operators](../../references/operators.md) | Precedence for `?`, `?.`, `??` |
| [Types](../../references/types.md) | Built-in Option / Result |
| [Built-ins](../../references/README.md) | Compiler-provided enums, `assert`, `panic` |
| [Keywords](../../references/keywords.md) | `raise`, `panic` |
| [Error codes](../../references/error-codes.md) | E0114–E0117 |
