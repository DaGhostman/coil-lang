# `io` virtual module

Non-blocking file / stdio / TCP / UDP streams. **Not** auto-imported:

```coil
use io::*;
```

| Export | Kind | Notes |
|--------|------|-------|
| `Stream` | Opaque type | Heap handle; closed on GC drop |
| `IoError` | Builtin enum | `WouldBlock`, `NotFound`, `PermissionDenied`, `AlreadyClosed`, `InvalidInput`, `Other`, `NotADirectory`, `AlreadyExists`, `TimedOut`, `Truncated`, `Certificate`, `Handshake` |
| `Read` / `Write` | Typeclasses | `impl` for `Stream`; methods = free functions |
| `stdin` / `stdout` / `stderr` | `() -> Stream` | Dup'd fds |
| `open` / `close` / `read` / `write` | L0 | Never busy-spin; `read` → `Result<Option<int>, IoError>` (`None` = EOF) |
| `read_exact` / `read_to_end` / `write_all` | Sync adapters | May block in the host via `poll` |
| `set_read_timeout` / `set_write_timeout` | Sync adapter config | Millisecond soft deadlines; `ms <= 0` clears |
| `from_bytes` / `to_bytes` | Text | UTF-8 `[byte] ↔ string` (`from_bytes` → `Result<string, IoError>`) |
| `io::net::tcp::{connect,connect_timeout,listen,accept,accept_wait,accept_wait_timeout}` | TCP | Nested module — `use io::net::tcp::*;`; timeout `ms <= 0` waits forever |
| `io::net::tcp::{peer_addr,local_addr,set_nodelay,shutdown}` | TCP helpers | Address tuples, `TCP_NODELAY`, and half-close (`0` read, `1` write, `2` both) |
| `io::net::udp::{bind,connect,send_to,recv_from,recv_from_wait,local_port}` | UDP | Nested module; `recv_from` → `(nbytes, host, port)` |
| `io::net::tls::client::{enable,disable}` | TLS client | Nested module (feature `tls`); in-place TCP↔TLS with required opts |
| `io::net::tls::server::{enable,disable}` | TLS server | Nested module (feature `tls`); PEM cert/key opts, optional mTLS |

`connect` / `connect_timeout` try **every** DNS result under one absolute
deadline. `listen` / UDP `bind` still use the first resolved address — prefer
an explicit IP (e.g. `127.0.0.1`) when family order matters.

Socket / stream soft deadlines and OS `TimedOut` map to `IoError::TimedOut`
(not `WouldBlock`). Call sites that previously treated timeouts as
`WouldBlock` should match `TimedOut` instead.

TLS client enable takes `enable(s, host, { verify: bool, ca_pem: string, timeout_ms: int })`.
When `verify` is true, empty `ca_pem` uses **webpki-roots**; non-empty `ca_pem`
**replaces** those roots with the PEM trust bundle only (combine PEMs yourself
if you still need public CAs). `verify: false` skips cert **trust** only.
`timeout_ms <= 0` means no handshake deadline.

TLS server enable takes `enable(s, { cert_pem: string, key_pem: string, timeout_ms: int, client_ca_pem: string })`
on an accepted TCP stream. Empty `client_ca_pem` disables client certificate auth;
non-empty PEM enables mTLS. `timeout_ms <= 0` means no handshake deadline.

Buffers are **`[byte]`**. Use `from_bytes` / `to_bytes` for text. `print` still
uses the `PRINT` opcode (not `stdout`). HTTP remains userland (`stdlib/http`).

See [Tutorial 10 — IO streams](../manual/tutorial/10-io-streams.md) and `examples/io_*.hy`.

---

## Related

- [IO tutorial](../manual/tutorial/10-io-streams.md)
- [io::fs](io-fs.md)
