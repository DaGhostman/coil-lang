# IO reactor

coil keeps two runtime facets on each root [`Machine`](../../machine/src/vm.rs):

| Facet | Module | Role |
|-------|--------|------|
| CPU | [`reactor.rs`](../../machine/src/reactor.rs) | Work-stealing Coil `Job`s (`spawn` / auto-par) |
| IO | [`io_reactor.rs`](../../machine/src/io_reactor.rs) | fd readiness for streams / TLS |

They share a lifecycle (cloned onto pool workers) but **never** put blocking IO onto stealable CPU jobs.

## Phase 1 — sync adapters

`read_exact`, `read_to_end`, `write_all`, `accept_wait`, UDP `recv_from_wait`, and TLS
handshake waits call [`IoReactor::wait_fd`](../../machine/src/io_reactor.rs) (via
[`reactor_wait_fd`](../../machine/src/io.rs) / host TLS). Single-fd `poll` is used so
regular files, pipes, and sockets all work.

When a CPU reactor is bound (`HostStateGuard`), waits use
[`wait_fd_helping`](../../machine/src/io_reactor.rs): short poll slices interleaved with
[`Reactor::help_once`](../../machine/src/reactor.rs) so fork-join work can progress
during IO.

## Phase 2 — true async await

| Native | Behavior |
|--------|----------|
| `await_readable(s)` / `await_writable(s)` | If already ready → `Ok(())`. Else park the VM (`PendingIoWait`, like deferred FFI), help-steal until ready, then push `Ok(())`. |
| `drive()` | `poll_once` on registered async waiters; returns newly-ready count. |

L0 `read` / `write` stay non-blocking (`WouldBlock`). Prefer:

```coil
use io::*;
fn read_fully(Stream s, [byte] buf) -> Result<(), IoError> {
    loop {
        match read(s, buf)? {
            Option::Some(_) => return Ok(()),
            Option::None => return Ok(()), // EOF — adjust as needed
        }
        // WouldBlock path uses `?` → Err; match WouldBlock then await:
    }
}
```

Or call `await_readable(s)?` before retrying L0 `read` after `WouldBlock`.

## Env / knobs

IO waits inherit the same `Machine` as CPU work; pool size is still
`COIL_MAX_WORKER_THREADS` (CPU facet only).
