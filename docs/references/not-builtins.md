# What is NOT a builtin

There is **no general standard library** yet. The following are **not** built-in — you must provide your own functions or FFI:

| Category | Examples |
|----------|----------|
| Collections API | `sort`; range→array materialize (lazy `a..b` / `a..=b` as `Range<T: Ord>` is supported — see [Syntax — ranges](syntax.md#ranges-lazy); `arr[] =` append / `len` / `for-in` are builtins) |
| String ops | slice, trim (concat via `+` / `format`; UTF-8 via `io::from_bytes` / `to_bytes`) |
| Math | `sin`, `sqrt`, `random` |
| High-level file helpers | path utilities beyond `io::open` / `read_to_end` / `write_all` |
| HTTP / TLS | Not in the VM (use userland on top of `io` TCP later) |
| Concurrency | — (use virtual **`thread`** module for OS threads; coroutines via `async` / `yield` / `resume` / `done` — see [Tutorial 11](../manual/tutorial/11-threads.md) and [Tutorial 08](../manual/tutorial/08-coroutines.md)) |
| Memory | `alloc`, `free` |

Use **`io`** for streams, **FFI** for C libraries, or **host natives** when embedding the VM in Rust.

---

## Related

- [io](io.md)
- [ffi](ffi.md)
- [host-natives](host-natives.md)
