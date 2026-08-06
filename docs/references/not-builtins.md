# What is NOT a builtin

Compiler virtual modules cover systems I/O, threads, crypto, time, env, regex, FFI,
and IEEE float math. A **userland** library under [`stdlib/`](../../stdlib/) adds
text/bytes helpers, collections, numeric convenience helpers, path/JSON, and IO
sugar — see [`stdlib/README.md`](../../stdlib/README.md).

Still not provided as **compiler builtins** (HostInvoke / opcodes):

| Category | Examples | Userland alternative |
|----------|----------|----------------------|
| Collections API | `sort`; range→array materialize | `collections::{sort,collect_ints}` (stable mergesort) |
| String ops | slice, trim, split, replace, lines | `text::*` (byte-oriented) |
| Bytes ops | find, replace, pad, repeat | `bytes::*` |
| ASCII / parse | digit classify; `parse_int` / `int_to_dec` | `ascii::*`, `conv::*` |
| Numeric conveniences | `abs`, `round`, `pow`, casual `random` | `num::{abs, min, pow, …}`, `random::{…}` (`sin` / `sqrt` / … are auto-imported from `prelude::math`; `pow` is userland) |
| High-level file helpers | whole-file read/write | `io::file::*` |
| HTTP | — | `http::client` (userland; TLS via virtual `io::net::tls`) |
| Memory | `alloc`, `free` | `gc::Root` / `gc::Weak` |

Use **`io`** for streams, **FFI** for C libraries, or **host natives** when embedding the VM in Rust.

---

## Related

- [io](io.md)
- [ffi](ffi.md)
- [host-natives](host-natives.md)
