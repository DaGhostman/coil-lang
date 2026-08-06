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
| `await_readable` / `await_writable` | **Top-level:** park the VM (`PendingIoWait`) until ready. **Inside a coroutine:** register a waiter and yield so many awaits can share one `poll` |
| `drive()` | Non-blocking `poll_once` on registered async waiters |
| `wait_ready()` | Block until ≥1 registered waiter is ready (batch); no-op when none registered |
| **`block_on(coro)`** (prelude) | Resume until `done`; calls `wait_ready` between resumes |
| Userland `io::sync::{write_all, …}` | Coil loops over L0 + `await_*` (`stdlib/io/sync.hy`) — top-level park path |

Preferred DX — async work, sync boundary:

```coil
use io::{Stream};
async fn copy(Stream a, Stream b) -> Result<(), IoError> {
    // L0 + await_* …
}
fn main() {
    block_on(copy(in, out))?;
}
```

`block_on` is auto-imported from `prelude`. Intermediate `yield`s are discarded;
only the final `return` value is kept. IO `await_*` inside the coroutine yields
cooperatively; `block_on` parks on `wait_ready` between resumes.

## Batching without `block_on`

Multiple coroutine handles can register waiters and share one poll:

```coil
use io::{wait_ready, ...};

fn main() {
    let h1 = serve(c1);
    let h2 = serve(c2);
    while !done(h1) || !done(h2) {
        if !done(h1) { resume h1; }
        if !done(h2) { resume h2; }
        wait_ready();
    }
}
```

Each `await_*` inside `serve` yields after registering interest; `wait_ready`
runs one multiplexed `poll` over all outstanding fds.

## Waiting on readiness

TLS handshake waits and top-level `await_*` call
[`IoReactor::wait_fd`](../../machine/src/io_reactor.rs) (via
[`reactor_wait_fd`](../../machine/src/io.rs)). Cooperative awaits use
[`register_wait`](../../machine/src/io_reactor.rs) + yield.
Userland sync adapters (`write_all`, …) reach the park path through top-level
`await_readable` / `await_writable`.

When a CPU reactor is bound (`HostStateGuard`), blocking waits use
[`wait_fd_helping`](../../machine/src/io_reactor.rs): short poll slices interleaved with
[`Reactor::help_once`](../../machine/src/reactor.rs).

## Env / knobs

IO waits inherit the same `Machine` as CPU work; pool size is still
`COIL_MAX_WORKER_THREADS` (CPU facet only).
