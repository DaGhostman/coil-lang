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

`print` still uses the `PRINT` opcode (not redirected through `stdout`).

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
