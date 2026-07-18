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
| `tcp_connect` / `tcp_listen` / `tcp_accept` | TCP | Same `Stream` + `Read`/`Write` |
| `udp_bind` / `udp_connect` / `udp_send_to` / `udp_recv_from` | UDP | Datagram sockets; see below |

`print` still uses the `PRINT` opcode (not redirected through `stdout`).

---

## UDP

UDP sockets are also `Stream` handles (`StreamKind::Udp`). Prefer the
datagram helpers when you need peer addresses:

| Function | Signature (simplified) | Behavior |
|----------|------------------------|----------|
| `udp_bind(host, port)` | `→ Result<Stream, IoError>` | `port` may be `0` (ephemeral) |
| `udp_local_port(s)` | `→ Result<int, IoError>` | Assigned local port after bind |
| `udp_connect(host, port)` | `→ Result<Stream, IoError>` | Connected peer; then `read` / `write` work |
| `udp_send_to(s, buf, host, port)` | `→ Result<int, IoError>` | Non-blocking `sendto` |
| `udp_recv_from(s, buf)` | `→ Result<(int, string, int), IoError>` | `(nbytes, peer_host, peer_port)` |
| `udp_recv_from_wait(s, buf)` | same | Blocks in the host via `poll` |

```0s
use io::*;

fn main() {
    let server = udp_bind("127.0.0.1", 0)?;
    let port = udp_local_port(server)?;
    let client = udp_bind("127.0.0.1", 0)?;
    let msg: [byte] = [72, 105];
    udp_send_to(client, msg, "127.0.0.1", port)?;
    let buf: [byte] = [0, 0, 0, 0, 0, 0, 0, 0];
    let t = udp_recv_from_wait(server, buf)?;
    print "%i", t[0];
}
```

See `examples/io_udp.0s`.

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
