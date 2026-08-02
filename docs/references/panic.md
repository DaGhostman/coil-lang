# `panic`

Keyword that aborts the program with a string message. Writes `panic: <msg>` and stops the VM; the CLI exits with code `1`. Under `coil test`, a language panic fails the current case only (the next case still runs unless `--fail-fast` is set).

```coil
use io::{stdout, write_all};
use string::{format, to_bytes};
panic "unreachable";
panic format("bad index %i", i);
```

Unlike `raise`, `panic` is not recoverable with `?` / `match`. Prefer `assert` + `?` when callers should handle failure.

See `examples/panic.hy`.

---

## Related

- [assert](assert.md)
- [Keywords](keywords.md)
