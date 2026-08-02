# `Option` and `Result`

Pre-registered enums with fixed tags, exported from the virtual `prelude` module (auto-imported into every file):

| Enum | Variants | Tags | Canonical path |
|------|----------|------|----------------|
| `Option` | `None`, `Some(T)` | 0, 1 | `prelude::Option` |
| `Result` | `Ok(T)`, `Err(E)` | 0, 1 | `prelude::Result` |

Bare `Option::Some(…)` works because of the implicit prelude. To redefine a prelude name, first free the short binding (`use prelude::Option as PreludeOption;`) then declare your own.

Use constructors / `match` as usual, plus `raise`, `?`, `??`, and `?.` — see [Tutorial: Error handling](../manual/tutorial/09-error-handling.md).

---

## Related

- [Error handling tutorial](../manual/tutorial/09-error-handling.md)
- [Types](types.md)
