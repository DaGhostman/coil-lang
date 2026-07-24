# 03-echo — notes

## What it shows

Single-process TCP echo: `io::net::tcp` listen/connect/accept_wait, length-prefixed
framing (`protocol.hy`), pure server/client helpers, and a coroutine that
supplies payload bytes. Stream IO stays in `main.hy`.

## Run

```bash
./examples/projects/03-echo/demo.sh
# ok
```

Always under `timeout` (the script wraps it).

## Test

```bash
./examples/projects/run-tests.sh
# or: cd examples/projects/03-echo && …/coil test
```

## Layout

| File | Role |
|------|------|
| `src/protocol.hy` | `encode_frame` / `frame_len` / `payload_eq` (sibling calls) |
| `src/server.hy` | Pure echo policy (`echo_reply`) |
| `src/client.hy` | Pure request body + fixed port |
| `src/main.hy` | listen → connect → accept → exchange (all Stream IO) |

## Ergonomics / gaps noticed

1. **IO HostInvoke from a dependency module is broken** — TCP helpers that
   call `listen`/`write_all`/… must live in the entry file.
2. **`use sibling::*` inside a non-entry module** may not resolve free-fn
   calls (`payload_eq` from `server.hy` failed) — keep dep modules self-contained
   or call shared helpers only from the entry.
3. TCP has **no `local_port`** — fixed port `41235`.
4. Preferred order: `listen` → `connect` → `accept_wait`.
5. Prefer `push` / one-byte reads over index assign.
6. Test harness is CWD-`./tests` only.
