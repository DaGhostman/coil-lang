# 10 — IO streams

coil exposes non-blocking file, stdio, and TCP IO through the virtual
**`io`** module. Import it explicitly (like `ffi`):

```coil
use io::*;
```

Buffers use the **`byte`** primitive and **`[byte]`** arrays.

---

## `byte` and `[byte]`

| Type | Notes |
|------|--------|
| `byte` | Integer in `0..=255`. Literals coerce when annotated / expected. |
| `[byte]` | Homogeneous byte buffer for `read` / `write`. |

```coil
fn main() {
    let b: byte = 255;
    let arr: [byte] = [1, 2, 3];
    print "%i", b;
    print "%i", len(arr);
}
```

`byte` implements `Show` and `Eq`. It is **not** in `Num` / `Add` yet — use
`int` for arithmetic and convert at the boundary if needed.

### Text helpers

| Function | Signature | Notes |
|----------|-----------|--------|
| `from_bytes` | `[byte] → Result<string, IoError>` | UTF-8 decode; invalid sequences → `InvalidInput` |
| `to_bytes` | `string → [byte]` | UTF-8 encode (always succeeds) |

```coil
use io::*;

fn main() {
    let hello: [byte] = [104, 101, 108, 108, 111];
    print "%s", match from_bytes(hello) {
        Result::Ok(s) => s,
        Result::Err(_) => "err",
    };
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
| `read(s, buf)` | `→ Result<Option<int>, IoError>` | `Ok(Some(n))`, `Ok(None)` = EOF |
| `write(s, buf)` | `→ Result<int, IoError>` | Partial writes OK |
| `read_exact` / `read_to_end` / `write_all` | sync adapters | May **block in the host** via `poll` |
| `io::net::tcp::*` | TCP | `connect` / `listen` / `accept` / `accept_wait` |
| `io::net::udp::*` | UDP | Datagram sockets; see below |
| `io::net::tls::client::*` | TLS client | `enable` / `disable` (feature `tls`) |
| `io::net::tls::server::*` | TLS server | `enable` / `disable` (feature `tls`) |

`print` still uses the `PRINT` opcode (not redirected through `stdout`).

TCP, UDP, and TLS live in nested virtual modules — import them explicitly
(like `ffi::types`):

```coil
use io::*;
use io::net::tcp::*;
use io::net::udp::*;
use io::net::tls::client::*;
use io::net::tls::server::*;
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
| `recv_from_wait(s, buf)` | same | Blocks in the host via `poll` |

```coil
use io::*;
use io::net::udp::*;

fn main() {
    let server = bind("127.0.0.1", 0)?;
    let port = local_port(server)?;
    let client = bind("127.0.0.1", 0)?;
    let msg: [byte] = [72, 105];
    send_to(client, msg, "127.0.0.1", port)?;
    let buf: [byte] = [0, 0, 0, 0, 0, 0, 0, 0];
    let t = recv_from_wait(server, buf)?;
    print "%i", t[0];
}
```

See `examples/io_udp.hy`.

---

## TCP (`io::net::tcp`)

| Function | Signature (simplified) | Behavior |
|----------|------------------------|----------|
| `connect(host, port)` | `→ Result<Stream, IoError>` | Connected stream; `read` / `write` |
| `listen(host, port)` | `→ Result<Stream, IoError>` | Listening socket |
| `accept(s)` | `→ Result<Stream, IoError>` | Non-blocking; `WouldBlock` if empty |
| `accept_wait(s)` | same | Blocks in the host via `poll` |

---

## TLS (`io::net::tls::{client,server}`)

TLS via rustls (Cargo feature `tls`, default-on). Upgrade a connected TCP
`Stream` in place (opportunistic); afterwards use the normal `Stream` APIs.
Client and server share the names `enable` / `disable` under separate modules.

| Module | Function | Behavior |
|--------|----------|----------|
| `tls::client` | `enable(s, host, opts)` | TCP→TLS; `opts.verify: bool` **required** |
| `tls::client` | `disable(s)` | Tear TLS down; plaintext on same fd |
| `tls::server` | `enable(s, opts)` | TCP→TLS; `opts.cert_pem` / `key_pem` **required** |
| `tls::server` | `disable(s)` | Same teardown as client `disable` |

```coil
use io::net::tcp::*;
use io::net::tls::client::*;

let s = connect("example.com", 443)?;
let s = enable(s, "example.com", { verify: true })?;
let s = disable(s)?;
```

```coil
use io::net::tcp::*;
use io::net::tls::server::*;

let s = accept_wait(listener)?;
let s = enable(s, { cert_pem: cert, key_pem: key })?;
let s = disable(s)?;
```

`{ verify: false }` skips cert **trust** only (signatures still checked) —
local/dev; never use in production. Empty `{}` / unknown keys are rejected.
Handshake / TLS failures map to `IoError::Other` in v1. Prefer a DNS `host`
for client SNI. `enable` blocks the host thread for the handshake (no timeout
in v1). Teardown discards unread TLS plaintext.

See `examples/io_tls.hy`.

---

## File round-trip

```coil
use io::*;

fn main() {
    let path = "/tmp/demo.bin";
    let data: [byte] = [72, 105];
    let s = open(path, "w")?;
    write_all(s, data)?;
    close(s)?;

    let s = open(path, "r")?;
    let buf = read_to_end(s)?;
    close(s)?;
    print "%i", len(buf);
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
`PermissionDenied`, `AlreadyClosed`, `InvalidInput`, `Other`.

Prefer `?` in `Result`-mode helpers (see [Error handling](09-error-handling.md)).

---

## Related

- [Built-ins — `io` module](../../references/io.md)
- [Types — `byte`](../../references/types.md)
- [Examples catalog](../examples.md)
