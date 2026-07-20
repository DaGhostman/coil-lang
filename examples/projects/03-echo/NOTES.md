# 03-echo — notes

## What it shows

Single-process TCP echo: `io::net::tcp` listen/connect/accept_wait, length-prefixed
framing (`protocol.0s`), pure server/client helpers, and a coroutine that
supplies payload bytes. Stream IO stays in `main.0s`.

## Run

```bash
rm -f out.c0s
timeout 10s cargo run --release -- examples/projects/03-echo/src/main.0s
# ok
```

Always wrap with `timeout`.

## Test

```bash
cd examples/projects/03-echo
timeout 60s cargo run --release --manifest-path ../../../Cargo.toml -- test
```

## Layout

| File | Role |
|------|------|
| `src/protocol.0s` | `encode_frame` / `frame_len` / `payload_eq` (sibling calls) |
| `src/server.0s` | Pure echo policy (`echo_reply`) |
| `src/client.0s` | Pure request body + fixed port |
| `src/main.0s` | listen → connect → accept → exchange (all Stream IO) |

## Ergonomics / gaps noticed

1. **IO HostInvoke from a dependency module is broken** — TCP helpers that
   call `listen`/`write_all`/… must live in the entry file.
2. **`use sibling::*` inside a non-entry module** may not resolve free-fn
   calls (`payload_eq` from `server.0s` failed) — keep dep modules self-contained
   or call shared helpers only from the entry.
3. TCP has **no `local_port`** — fixed port `41235`.
4. Preferred order: `listen` → `connect` → `accept_wait`.
5. Prefer `push` / one-byte reads over index assign.
6. Test harness is CWD-`./tests` only.
