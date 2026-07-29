# `io` virtual module

Non-blocking file / stdio / TCP / UDP streams. **Not** auto-imported:

```coil
use io::*;
```

| Export | Kind | Notes |
|--------|------|-------|
| `Stream` | Opaque type | Heap handle; closed on GC drop |
| `IoError` | Builtin enum | `WouldBlock`, `NotFound`, `PermissionDenied`, `AlreadyClosed`, `InvalidInput`, `Other` |
| `Read` / `Write` | Typeclasses | `impl` for `Stream`; methods = free functions |
| `stdin` / `stdout` / `stderr` | `() -> Stream` | Dup'd fds |
| `open` / `close` / `read` / `write` | L0 | Never busy-spin; `read` → `Result<Option<int>, IoError>` (`None` = EOF) |
| `read_exact` / `read_to_end` / `write_all` | Sync adapters | May block in the host via `poll` |
| `from_bytes` / `to_bytes` | Text | UTF-8 `[byte] ↔ string` (`from_bytes` → `Result<string, IoError>`) |
| `io::net::tcp::{connect,listen,accept,accept_wait}` | TCP | Nested module — `use io::net::tcp::*;` |
| `io::net::udp::{bind,connect,send_to,recv_from,recv_from_wait,local_port}` | UDP | Nested module; `recv_from` → `(nbytes, host, port)` |
| `io::net::tls::client::{enable,disable}` | TLS client | Nested module (feature `tls`); in-place TCP↔TLS |
| `io::net::tls::server::{enable,disable}` | TLS server | Nested module (feature `tls`); PEM cert/key opts |

`enable(..., { verify: true })` trusts **webpki-roots** only in v1 — no custom CA /
PEM trust opts yet (private PKI needs a follow-up API or host embedding).
Server `enable` takes PEM `cert_pem` / `key_pem` on an accepted TCP stream (no
listen-time TLS; no client certs / mTLS in v1).


Buffers are **`[byte]`**. Use `from_bytes` / `to_bytes` for text. `print` still uses the `PRINT` opcode (not `stdout`). No HTTP in the VM — userland only later.

See [Tutorial 10 — IO streams](../manual/tutorial/10-io-streams.md) and `examples/io_*.hy`.

---

## Related

- [IO tutorial](../manual/tutorial/10-io-streams.md)
- [io::fs](io-fs.md)
