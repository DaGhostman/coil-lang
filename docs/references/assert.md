# `assert` (`prelude::test`)

Auto-imported from the virtual `prelude::test` module. Returns a `Result` — it does **not** abort by itself.

### Forms

| Form | Result on success | Result on failure |
|------|-------------------|-------------------|
| `assert(cond)` | `Result::Ok(())` | `Result::Err("assertion failed")` |
| `assert(cond, msg)` | `Result::Ok(())` | `Result::Err(msg)` |

`cond` must be `bool`; `msg` must be `string`. Propagate with `?` in a result-mode function, or `match` the value:

```coil
fn must_be_pos(int n) {
    assert(n > 0, "expected positive")?;
    return n;
}
```

Rebind the short name with `use prelude::test::assert as check;` if you need `assert` free for something else.

See `examples/assert.hy`.

---

## Related

- [test harness](test-harness.md)
- [panic](panic.md)
