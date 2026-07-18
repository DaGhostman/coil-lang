# 10 — IO streams

zero-script exposes non-blocking file, stdio, and TCP IO through the virtual
**`io`** module. Import it explicitly (like `ffi`):

```0s
use io::*;
```

Buffers use the **`byte`** primitive and **`[byte]`** arrays.

---

## `byte` and `[byte]`

| Type | Notes |
|------|--------|
| `byte` | Integer in `0..=255`. Literals coerce when annotated / expected. |
| `[byte]` | Homogeneous byte buffer for `read` / `write`. |

```0s
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

```0s
use io::*;

fn main() {
    let hello: [byte] = [104, 101, 108, 108, 111];
    print "%s", match from_bytes(hello) {
        Result::Ok(s) => s,
        Result::Err(_) => "err",
    };
}
```

See `examples/io_text.0s`.

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

`print` still uses the `PRINT` opcode (not redirected through `stdout`).

TCP and UDP live in nested virtual modules — import them explicitly
(like `ffi::types`):

```0s
use io::*;
use io::net::tcp::*;
use io::net::udp::*;
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

```0s
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

See `examples/io_udp.0s`.

---

## TCP (`io::net::tcp`)

| Function | Signature (simplified) | Behavior |
|----------|------------------------|----------|
| `connect(host, port)` | `→ Result<Stream, IoError>` | Connected stream; `read` / `write` |
| `listen(host, port)` | `→ Result<Stream, IoError>` | Listening socket |
| `accept(s)` | `→ Result<Stream, IoError>` | Non-blocking; `WouldBlock` if empty |
| `accept_wait(s)` | same | Blocks in the host via `poll` |

---

## File round-trip

```0s
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

See `examples/io_file.0s` and `examples/io_eof.0s`.

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

- [Built-ins — `io` module](../reference/built-ins.md#io-virtual-module)
- [Types — `byte`](../reference/types.md)
- [Examples catalog](../examples.md)
