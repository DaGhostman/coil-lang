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
use io::{stdout};
use io::sync::{write_all};
use string::{format, to_bytes};
let h = counter();
write_all(stdout(), to_bytes(format("%z", done(h)))); // false
resume h;
resume h;            // completes
write_all(stdout(), to_bytes(format("%z", done(h)))); // true
```

---

## Related

- [Coroutines tutorial](../manual/tutorial/08-coroutines.md)
