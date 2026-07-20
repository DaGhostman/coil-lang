# 03-echo — notes

## What it shows

Single-process TCP echo: `io::net::tcp` listen/connect/accept_wait, a tiny
length-prefixed protocol module, and a coroutine that supplies payload bytes.

## Run

```bash
rm -f out.c0s
timeout 10s cargo run --release -- examples/projects/03-echo/src/main.0s
# ok
```

Always wrap with `timeout` — a wedged accept/connect must not hang the runner.

## Test

```bash
cd examples/projects/03-echo
cargo run --release --manifest-path ../../../Cargo.toml -- test
```

`protocol_roundtrip.0s` is pure (no sockets).

## Layout vs plan

Plan called for `protocol.0s` + `server.0s` + `client.0s`. Today **only the
first file-module `use` resolves** in an entry, so server/client orchestration
lives in `main.0s` and framing stays in `protocol.0s`.

## Ergonomics / gaps noticed

1. TCP has **no `local_port`** (UDP does) — demos must use a fixed port.
2. Preferred order: `listen` → `connect` → `accept_wait` (connect may complete
   while the connection sits in the accept backlog).
3. Array index assign (`buf[i] = x`) is unreliable — prefer `push` / one-byte reads.
4. Multiple file-module `use` from one entry is broken.
5. Test harness is CWD-`./tests` only.
