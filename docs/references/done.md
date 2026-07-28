# `done`

Test whether a coroutine handle has finished.

### Syntax

```coil
done(handle_expr)
```

| Argument | Type | Description |
|----------|------|-------------|
| `handle_expr` | `coroutine<Y, S>` | Handle from calling an `async fn` |

### Returns

`bool` — `true` after the coroutine body has returned (or fallen off the end); `false` while still suspended at a `yield` or before the first `resume`.

### Example

```coil
let h = counter();
print "%z", done(h); // false
resume h;
resume h;            // completes
print "%z", done(h); // true
```

---

## Related

- [Coroutines tutorial](../manual/tutorial/08-coroutines.md)
