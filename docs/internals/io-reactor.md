# IO reactor

coil keeps two runtime facets on each root [`Machine`](../../machine/src/vm.rs):

| Facet | Module | Role |
|-------|--------|------|
| CPU | [`reactor.rs`](../../machine/src/reactor.rs) | Work-stealing Coil `Job`s (`spawn` / auto-par) |
| IO | [`io_reactor.rs`](../../machine/src/io_reactor.rs) | fd readiness for streams / TLS |

They share a lifecycle (cloned onto pool workers) but **never** put blocking IO onto stealable CPU jobs.

## Async-first model

| Surface | Behavior |
|---------|----------|
| L0 `read` / `write` / `accept` | Always non-blocking; `WouldBlock` when not ready |
| `await_readable` / `await_writable` | Park the VM (`PendingIoWait`) until ready; CPU help-steals |
| `drive()` | `poll_once` on registered async waiters |
| Convenience `write_all` / `read_exact` / … | Host loops L0 + reactor wait (same IO reactor) |
| **`block_on(coro)`** (prelude) | Resume a coroutine until `done`; returns the completion value |

Preferred DX — async work, sync boundary:

```coil
use io::*;
async fn copy(Stream a, Stream b) -> Result<(), IoError> {
    // L0 + await_* …
}
fn main() {
    block_on(copy(in, out))?;
}
```

`block_on` is auto-imported from `prelude`. Intermediate `yield`s are discarded;
only the final `return` value is kept. IO `await_*` inside the coroutine still
parks via the IO reactor between resumes.

## Sync convenience adapters

`read_exact`, `read_to_end`, `write_all`, `accept_wait`, UDP `recv_from_wait`, and TLS
handshake waits call [`IoReactor::wait_fd`](../../machine/src/io_reactor.rs) (via
[`reactor_wait_fd`](../../machine/src/io.rs)). They are **not** a second IO API —
they are helpers that wait on the same reactor. Prefer `block_on` + `async fn`
when structuring concurrent / overlapping work.

When a CPU reactor is bound (`HostStateGuard`), waits use
[`wait_fd_helping`](../../machine/src/io_reactor.rs): short poll slices interleaved with
[`Reactor::help_once`](../../machine/src/reactor.rs).

## Env / knobs

IO waits inherit the same `Machine` as CPU work; pool size is still
`COIL_MAX_WORKER_THREADS` (CPU facet only).
