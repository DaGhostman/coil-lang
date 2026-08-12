# What is NOT a builtin

Compiler virtual modules cover systems I/O, threads, crypto, time, env, regex, FFI,
and IEEE float math. Collections, text/bytes helpers, decimal parse, path, blocking
IO adapters, whole-file helpers, and HTTP are **not** HostInvoke/opcodes — they
live in [coil-stdlib](https://github.com/ardax-corp/coil-stdlib)
([module catalog](https://github.com/ardax-corp/coil-stdlib/blob/main/docs/modules.md)).

Still not a compiler builtin (and not coil-stdlib either):

| Category | Examples | Where to look |
|----------|----------|----------------|
| Raw memory | `alloc`, `free` | [`gc::Root` / `gc::Weak`](gc.md) |
| HTTP in the VM | opcodes / natives | [coil-http](https://github.com/ardax-corp/coil-http) via [spool](../manual/http-client.md); TLS is virtual [`io::net::tls`](io.md) |

Use **`io`** for streams, **FFI** for C libraries, or **host natives** when embedding the VM in Rust.

---

## Related

- [coil-stdlib docs](https://github.com/ardax-corp/coil-stdlib/blob/main/docs/README.md)
- [io](io.md)
- [ffi](ffi.md)
- [host-natives](host-natives.md)
