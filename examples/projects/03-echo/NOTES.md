# 03-echo — notes

## What it shows

Single-process TCP echo: `io::net::tcp` listen/connect/accept_wait, length-prefixed
framing (`protocol.hy`), pure server/client helpers, and a coroutine that
supplies payload bytes. Stream IO currently lives in `main.hy` for clarity;
dependency modules may call IO HostInvoke + `?` (see regression
`multi_file_io_hostinvoke_try_in_dependency`).

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
| `src/main.hy` | listen → connect → accept → exchange (Stream IO) |

## Ergonomics / gaps noticed

1. TCP has **no `local_port`** — fixed port `41235`.
2. Preferred order: `listen` → `connect` → `accept_wait`.
3. Prefer `push` / one-byte reads over index assign.
4. Test harness is CWD-`./tests` only.
