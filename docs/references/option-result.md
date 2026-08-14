# `Option` and `Result`

Pre-registered enums with fixed tags, exported from the virtual `prelude` module (auto-imported into every file):

| Enum | Variants | Tags | Canonical path |
|------|----------|------|----------------|
| `Option` | `None`, `Some(T)` | 0, 1 | `prelude::Option` |
| `Result` | `Ok(T)`, `Err(E)` | 0, 1 | `prelude::Result` |

Bare `Option::Some(…)` works because of the implicit prelude. To redefine a prelude name, first free the short binding (`use prelude::Option as PreludeOption;`) then declare your own.

Use constructors / `match` as usual, plus `raise`, `?`, `??`, and `?.` — see [Tutorial: Error handling](../manual/tutorial/09-error-handling.md).

`match` **copies** the scrutinee. It does not move a local or a class field. Nested `match` on the same `Option` field is valid, and outer pattern bindings remain visible in the inner arm (an inner name shadows). See [Enums and Match](../manual/tutorial/03-enums-and-match.md#match-does-not-consume-the-scrutinee).

Runtime layout is an implementation detail (pointer niche, two-slot call return, or boxed enum). User code always uses constructors and `match` / `?` — do not match on raw `0` vs pointer. See [Option / Result runtime ABI](types.md#option--result-runtime-abi).

---

## Related

- [Error handling tutorial](../manual/tutorial/09-error-handling.md)
- [Types](types.md)
