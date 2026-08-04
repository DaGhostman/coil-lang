# `io` virtual module

Non-blocking file / stdio / TCP / UDP streams. **Not** auto-imported:

```coil
use io::*;
use io::sync::*;   // optional blocking adapters (userland)
```

| Export | Kind | Notes |
|--------|------|-------|
| `Stream` | Opaque type | Heap handle; closed on GC drop |
| `IoError` | Builtin enum | `WouldBlock`, `NotFound`, `PermissionDenied`, `AlreadyClosed`, `InvalidInput`, `Other`, `NotADirectory`, `AlreadyExists`, `TimedOut`, `Truncated`, `Certificate`, `Handshake` |
| `Read` / `Write` | Typeclasses | `impl` for `Stream`; methods = free functions |
| `stdin` / `stdout` / `stderr` | `() -> Stream` | Dup'd fds |
| `open` / `close` / `read` / `write` | L0 | Never busy-spin; `read` → `Result<Option<int>, IoError>` (`None` = EOF) |
| `await_readable` / `await_writable` | Async await | Park VM until fd ready; see [io-reactor](../internals/io-reactor.md) |
| `drive` | `() -> int` | Poll async waiters once |
| `block_on` | Prelude | `block_on(coro) -> Y` — auto-imported; drives `async fn` to completion |
| `from_bytes` / `to_bytes` | Text aliases | UTF-8 `[byte] ↔ string` (`from_bytes` → `Result<string, IoError>`); also exported by [`string`](string.md) |
| `io::net::tcp::{connect,connect_timeout,listen,accept}` | TCP | Nested module — `use io::net::tcp::*`; timeout `ms <= 0` waits forever |
| `io::net::tcp::{peer_addr,local_addr,set_nodelay,shutdown}` | TCP helpers | Address tuples, `TCP_NODELAY`, and half-close (`0` read, `1` write, `2` both) |
| `io::net::udp::{bind,connect,send_to,recv_from,local_port}` | UDP | Nested module; `recv_from` → `(nbytes, host, port)` |
| `io::net::tls::client::{enable,disable}` | TLS client | Nested module (feature `tls`); in-place TCP↔TLS with required opts |
| `io::net::tls::server::{enable,disable}` | TLS server | Nested module (feature `tls`); PEM cert/key opts, optional mTLS |

## Userland sync adapters (`io::sync`)

Blocking helpers live in stdlib (`stdlib/io/sync.hy`), not as host natives:

| Function | Notes |
|----------|-------|
| `write_all` / `read_exact` / `read_to_end` | L0 + `await_*` loops |
| `accept_wait` | `accept` + `await_readable` |
| `recv_from_wait` | `recv_from` + `await_readable` |

```coil
use io::{stdout};
use io::sync::{write_all};
use string::{format, to_bytes};
```

Prefer `async fn` + prelude `block_on` when structuring concurrent IO.

`connect` / `connect_timeout` try **every** DNS result under one absolute
deadline. `listen` / UDP `bind` still use the first resolved address — prefer
an explicit IP (e.g. `127.0.0.1`) when family order matters.

OS `TimedOut` maps to `IoError::TimedOut` (not `WouldBlock`).

TLS client enable takes
`enable(s, host, { verify: bool, ca_pem: Option<string>, ca_path: Option<string>, timeout_ms: int })`.
When `verify` is true, trust always starts from **webpki-roots**.
`ca_pem: Option::Some(pem)` and/or `ca_path: Option::Some(path)` **append**
extra PEM trust anchors (they do not replace the defaults).
`Option::None` for both leaves webpki alone. `verify: false` skips cert
**trust** only. `timeout_ms <= 0` means no handshake deadline.

TLS server enable takes `enable(s, { cert_pem: string, key_pem: string, timeout_ms: int, client_ca_pem: string })`
on an accepted TCP stream. Empty `client_ca_pem` disables client certificate auth;
non-empty PEM enables mTLS. `timeout_ms <= 0` means no handshake deadline.

Buffers are **`[byte]`**. Use `string::{from_bytes, to_bytes}` for text; `io::{from_bytes, to_bytes}` remain aliases. Use `write_all(stdout(), to_bytes(...))` for stdout text. HTTP remains userland (`stdlib/http`).

See [Tutorial 10 — IO streams](../manual/tutorial/10-io-streams.md) and `examples/io_*.hy`.

---

## Related

- [IO tutorial](../manual/tutorial/10-io-streams.md)
- [io::fs](io-fs.md)
- [string](string.md)
