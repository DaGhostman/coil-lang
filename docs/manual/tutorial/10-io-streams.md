# 10 — IO streams

coil exposes non-blocking file, stdio, and TCP IO through the virtual
**`io`** module. Import it explicitly (like `ffi`):

```coil
use io::{stdout, open, close, from_bytes};
use io::sync::{write_all, read_to_end};
```

Buffers use the **`byte`** primitive and **`Vec<byte>`** vectors.

---

## `byte` and `Vec<byte>`

| Type | Notes |
|------|--------|
| `byte` | Integer in `0..=255`. Literals coerce when annotated / expected. |
| `Vec<byte>` | Growable byte buffer for `read` / `write`. |

```coil
use io::{stdout};
use io::sync::{write_all};
use string::{format, to_bytes};
fn main() {
    let b: byte = 255;
    let arr = Vec::from([1 as byte, 2 as byte, 3 as byte]);
    write_all(stdout(), to_bytes(format("%i", b)));
    write_all(stdout(), to_bytes(format("%i", len(arr))));
}
```

`byte` implements `Show` and `Eq`. It is **not** in `Num` / `Add` yet — use
`int` for arithmetic and convert at the boundary if needed.

### Text helpers

| Function | Signature | Notes |
|----------|-----------|--------|
| `from_bytes` | `Vec<byte> → Result<string, IoError>` | UTF-8 decode; invalid sequences → `InvalidInput` |
| `to_bytes` | `string → Vec<byte>` | UTF-8 encode (always succeeds) |

```coil
use io::{stdout, from_bytes};
use io::sync::{write_all};
use string::{format, to_bytes};

fn main() {
    let hello = Vec::from([104 as byte, 101 as byte, 108 as byte, 108 as byte, 111 as byte]);
    write_all(stdout(), to_bytes(format("%s", match from_bytes(hello) {
        Result::Ok(s) => s,
        Result::Err(_) => "err",
    })));
}
```

See `examples/io_text.hy`.

---

## Streams

`Stream` is an opaque heap handle. L0 ops never busy-spin: they return
`Err(IoError::WouldBlock)` when the OS would block.

| Function | Signature (simplified) | Behavior |
|----------|------------------------|----------|
| `stdin` / `stdout` / `stderr` | `() -> Stream` | Dup'd process stdio |
| `open(path, mode)` | `→ Result<Stream, IoError>` | Modes: `"r"`, `"w"`, `"a"`, `"rw"` |
| `close(s)` | `→ Result<(), IoError>` | Idempotent close on GC drop too |
| `read` / `write` | L0 | Never busy-spin; `WouldBlock` when not ready |
| `await_readable` / `await_writable` | async await | Top-level parks; inside a coro yields + registers for batch poll |
| `drive` | `() -> int` | Poll registered async waiters once; returns newly-ready count |
| `wait_ready` | `() -> int` | Block until ≥1 registered waiter is ready (multiplex) |
| `block_on` | prelude | Drive an `async fn` handle to completion (see [IO reactor](../../internals/io-reactor.md)) |
| `io::sync::{write_all,read_exact,read_to_end}` | [coil-stdlib](https://github.com/ardax-corp/coil-stdlib/blob/main/docs/io.md) | Blocking adapters over L0 + `await_*` |
| `io::net::tcp::{connect,listen,accept,…}` | TCP | `connect` / `connect_timeout` / `listen` / `accept`, plus address / shutdown helpers |
| `io::net::udp::{bind,send_to,recv_from,…}` | UDP | Datagram sockets; see below |
| `io::net::tls::{alpn_protocol}` | TLS ALPN | Negotiated protocol after handshake (feature `tls`) |
| `io::net::tls::client::{enable,disable}` | TLS client | `enable` / `disable` (feature `tls`) |
| `io::net::tls::server::{enable,disable}` | TLS server | `enable` / `disable` (feature `tls`) |

For stdout text, call `write_all(stdout(), to_bytes(...))`. Blocking adapters
and `io::file` are documented in
[coil-stdlib IO](https://github.com/ardax-corp/coil-stdlib/blob/main/docs/io.md).

TCP, UDP, and TLS live in nested virtual modules — import them explicitly
(like `ffi::types`):

```coil
use io::{stdout};
use io::net::tcp::{connect};
use io::net::udp::{bind, local_port, send_to};
use io::net::tls::{alpn_protocol};
use io::net::tls::client::{enable, disable};
use io::net::tls::server::{enable, disable};
```

---

## UDP (`io::net::udp`)

UDP sockets are also `Stream` handles. Prefer the datagram helpers when
you need peer addresses:

| Function | Signature (simplified) | Behavior |
|----------|------------------------|----------|
| `bind(host, port)` | `→ Result<Stream, IoError>` | `port` may be `0` (ephemeral) |
| `local_port(s)` | `→ Result<int, IoError>` | Assigned local port after bind |
| `connect(host, port)` | `→ Result<Stream, IoError>` | Connected peer; then `read` / `write` work |
| `send_to(s, buf, host, port)` | `→ Result<int, IoError>` | Non-blocking `sendto` |
| `recv_from(s, buf)` | `→ Result<(int, string, int), IoError>` | `(nbytes, peer_host, peer_port)` |
| `io::sync::recv_from_wait(s, buf)` | same | Userland: `recv_from` + `await_readable` |

```coil
use io::{close, stdout};
use io::net::udp::{bind, local_port, send_to};
use io::sync::{recv_from_wait, write_all};
use string::{format, to_bytes};

fn main() {
    let server = bind("127.0.0.1", 0)?;
    let port = local_port(server)?;
    let client = bind("127.0.0.1", 0)?;
    let msg = Vec::from([72 as byte, 105 as byte]);
    send_to(client, msg, "127.0.0.1", port)?;
    let buf: Vec<byte> = Vec::from([0 as byte, 0 as byte, 0 as byte, 0 as byte, 0 as byte, 0 as byte, 0 as byte, 0 as byte]);
    let t = recv_from_wait(server, buf)?;
    write_all(stdout(), to_bytes(format("%i", t[0])));
}
```

See `examples/io_udp.hy`.

---

## TCP (`io::net::tcp`)

| Function | Signature (simplified) | Behavior |
|----------|------------------------|----------|
| `connect(host, port)` | `→ Result<Stream, IoError>` | Connected stream; `read` / `write` |
| `connect_timeout(host, port, ms)` | same | Connect deadline; `ms <= 0` waits forever |
| `listen(host, port)` | `→ Result<Stream, IoError>` | Listening socket |
| `accept(s)` | `→ Result<Stream, IoError>` | Non-blocking; `WouldBlock` if empty |
| `io::sync::accept_wait(s)` | same | Userland: `accept` + `await_readable` |
| `peer_addr(s)` / `local_addr(s)` | `→ Result<(string, int), IoError>` | Connected peer / local socket address |
| `set_nodelay(s, enabled)` | `→ Result<(), IoError>` | Toggle `TCP_NODELAY` on TCP/TLS streams |
| `shutdown(s, how)` | `→ Result<(), IoError>` | Half-close: `0` read, `1` write, `2` both |

---

## TLS (`io::net::tls::{client,server}`)

TLS via rustls (Cargo feature `tls`, default-on). Upgrade a connected TCP
`Stream` in place (opportunistic); afterwards use the normal `Stream` APIs
(`write_all` / `read` / `read_exact` / `read_to_end` / `close`).
Client and server share the names `enable` / `disable` under separate modules.

| Module | Function | Behavior |
|--------|----------|----------|
| `tls` | `alpn_protocol(s)` | Selected ALPN or `""`; `InvalidInput` if not TLS |
| `tls::client` | `enable(s, host, opts)` | TCP→TLS; `opts.verify`, `ca_pem`, `ca_path`, `timeout_ms` **required** |
| `tls::client` | `disable(s)` | Tear TLS down; plaintext on same fd |
| `tls::server` | `enable(s, opts)` | TCP→TLS; `opts.cert_pem`, `key_pem`, `timeout_ms`, `client_ca_pem` **required** |
| `tls::server` | `disable(s)` | Same teardown as client `disable` |

```coil
use io::net::tcp::{connect};
use io::net::tls::client::{enable, disable};

let s = connect("example.com", 443)?;
let s = enable(s, "example.com", { verify: true, ca_pem: Option::None, ca_path: Option::None, timeout_ms: 0 })?;
let s = disable(s)?;
```

```coil
use io::net::tcp::{accept_wait};
use io::net::tls::server::{enable, disable};

let s = accept_wait(listener)?;
let s = enable(s, { cert_pem: cert, key_pem: key, timeout_ms: 0, client_ca_pem: "" })?;
let s = disable(s)?;
```

`{ verify: false, ca_pem: Option::None, ca_path: Option::None, timeout_ms: 0 }` skips cert **trust** only
(signatures still checked) — local/dev; never use in production. When
`verify` is true, trust always includes webpki roots; `ca_pem` /
`ca_path` as `Option::Some(...)` **append** extra PEM anchors (from a
string or file path). `Option::None` means no extras. Server
`client_ca_pem: ""` means no mTLS; non-empty PEM enables client certificate
auth. Empty `{}` / unknown keys are rejected, and `timeout_ms <= 0` means no
handshake deadline.
Handshake / TLS failures map to `IoError::Certificate`, `IoError::Handshake`,
or `IoError::TimedOut`. Prefer a DNS `host` for client SNI. Failed handshakes
restore non-blocking on the TCP fd.
`client::disable` and `server::disable` share the same teardown (either name
works on a TLS stream). `disable` discards unread TLS plaintext. Prefer
explicit `close(s)` for a clean shutdown; GC drop still sends a best-effort
TLS `close_notify`.


See `examples/io_tls.hy`.

---

## File round-trip

```coil
use io::{close, open, stdout};
use io::sync::{read_to_end, write_all};
use string::{format, to_bytes};

fn main() {
    let path = "/tmp/demo.bin";
    let data = Vec::from([72 as byte, 105 as byte]);
    let s = open(path, "w")?;
    write_all(s, data)?;
    close(s)?;

    let s = open(path, "r")?;
    let buf = read_to_end(s)?;
    close(s)?;
    write_all(stdout(), to_bytes(format("%i", len(buf))));
}
```

See `examples/io_file.hy` and `examples/io_eof.hy`.

---

## `Read` / `Write`

The virtual module registers typeclasses **`Read`** and **`Write`** with
`impl` for `Stream`. Free functions (`read`, `write`, …) and trait methods
lower to the same host natives (`HostInvoke`).

---

## Errors

`IoError` variants (unit payloads): `WouldBlock`, `NotFound`,
`PermissionDenied`, `AlreadyClosed`, `InvalidInput`, `Other`, `NotADirectory`,
`AlreadyExists`, `TimedOut`, `Truncated`, `Certificate`, `Handshake`.

`TimedOut` is distinct from `WouldBlock` (deadlines / OS timeouts vs
“try again”). Prefer `?` in `Result`-mode helpers (see
[Error handling](09-error-handling.md)).

---

## Related

- [Built-ins — `io` module](../../references/io.md)
- [Types — `byte`](../../references/types.md)
- [Examples catalog](../examples.md)
