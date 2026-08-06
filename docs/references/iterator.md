# Iterator / IntoIterator

Prelude traits (virtual module — not `.hy` sources) power `for x in expr`:

```coil
trait Iterator<I> {
    type Item;
    fn next(I it) -> Option<Item>;
}

trait IntoIterator<T> {
    type Item;
    type IntoIter;
    fn into_iter(T t) -> IntoIter;
}
```

`for x in e` resolves `IntoIterator<Te>` then `Iterator<IntoIter>` with matching
`Item`, and binds `x : Item` in the body. Builtin synthesis (no ground `impl`
required) covers:

| Source | `Item` | Notes |
|--------|--------|-------|
| `[T; N]` | `T` | Index loop (`len` / `Index`) |
| `Vec<T>` | `T` | Index loop (`len` / `Index`) |
| Homogeneous `(A, …, A)` | `A` | Materialised to a temp array; hetero → diagnostic |
| Homogeneous `{ k: V, … }` | `(string, V)` | `DictEntries` then array path; hetero values → diagnostic |
| `coroutine<Y, S>` | `Y` | Resume/Done; completion value excluded from the body |

Users write ordinary `impl IntoIterator` / `impl Iterator` for custom types
(see `examples/for_in_custom.hy`). Methods are callable as UFCS
(`into_iter(x)`, `next(it)`).

---

## Related

- [Coroutines tutorial](../manual/tutorial/08-coroutines.md)
- [Syntax — ranges](syntax.md#ranges-lazy)
