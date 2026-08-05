# 11 — OS threads

coil can run **native OS threads** alongside the main VM. Each worker gets its own `Machine` and heap; communication uses typed channels and mutexes from the virtual **`thread`** module (`use thread::{spawn, join, channel, …};`).

This is separate from **coroutines** ([08 — Coroutines](08-coroutines.md)): coroutines are cooperative handles on one VM; `spawn` starts a real thread with an isolated bytecode interpreter.

## Import

```coil
use thread::{spawn, join, channel, send, recv, mutex, with_lock};
```

All primitives return `prelude::Result<…, thread::Error>`. Use `?` in result-mode functions (omit an explicit `-> int` return type when you want `?` to propagate errors).

## Spawning and joining

`spawn(f)` runs nullary function `f` on a new thread. `spawn(f, arg)` passes one argument (the function must be `fn (A) -> R`).

```coil
use thread::{join, spawn};
use io::{stdout};
use io::sync::{write_all};
use string::{format, to_bytes};

fn work() -> int {
    return 40 + 2;
}

fn main() {
    let t = spawn(work)?;
    write_all(stdout(), to_bytes(format("%i", join(t)?)));
}
```

`join(t)` blocks until the worker finishes and returns its result value. `detach(t)` lets the thread run without a join (errors if you later `join` the same handle).

### Spawn capacity

`spawn` submits work to a fixed **work-stealing reactor** (default one OS
worker per CPU, override with `COIL_MAX_WORKER_THREADS`) instead of creating a
new OS thread per call. Pure recursive helpers such as `fib(n-1) + fib(n-2)`
may be auto-parallelized at **constant** call sites (`fib(32)`), via specialized
nullary clones that always fork; `COIL_PAR_THRESHOLD` (default 20) is a
compile-time cutoff for those specializations (see
[internals: auto-par](../../internals/auto-par.md)).

## Channels

`channel()` returns `(Sender, Receiver)` as a two-tuple. `send` / `recv` move values between threads; `close` drops the sender side.

`recv` **blocks** until a value arrives or the channel is closed (`Disconnected`). Prefer `try_recv` when you need a non-blocking poll (`WouldBlock` if empty).

```coil
use thread::{Sender, channel, join, recv, send, spawn};
use io::{stdout};
use io::sync::{write_all};
use string::{format, to_bytes};

fn producer(Sender tx) {
    send(tx, "hello")?;
}

fn main() {
    let pair = channel()?;
    let tx = pair[0];
    let rx = pair[1];
    let t = spawn(producer, tx)?;
    write_all(stdout(), to_bytes(format("%s", recv(rx)?)));
    join(t)?;
}
```

### Request / reply (send to a worker and get a result back)

One channel is one-way. To send work *and* receive a reply, create **two** channels and pass both ends the worker needs as a tuple (or any sendable aggregate of handles):

```coil
use thread::{Receiver, Sender, channel, join, recv, send, spawn};
use io::{stdout};
use io::sync::{write_all};
use string::{format, to_bytes};

fn worker((Receiver, Sender) ends) {
    let job = recv(ends[0])?;
    send(ends[1], job)?;
}

fn main() {
    let jobs = channel()?;
    let replies = channel()?;
    let t = spawn(worker, (jobs[1], replies[0]))?;
    send(jobs[0], "ping")?;
    write_all(stdout(), to_bytes(format("%s", recv(replies[1])?)));
    join(t)?;
}
```

`spawn` accepts nested `Sender` / `Receiver` / `Mutex` / `RwLock` handles inside tuples, arrays, records, and class instances whose fields are all sendable.

### Joining

Always `join(t)` (or `detach(t)`) when you care about the worker's return value. `join` **blocks until the worker function returns** — it is not a no-op. If `main` returns without an explicit `join`, the runtime still waits for undetached workers so a blocked `recv` is not killed by process exit — but you should still `join` to observe errors and results.

If prints from a worker appear only sometimes across runs while you are editing sources (especially imported modules) or switching entry files, delete `out.hyc` and re-run: the CLI caches bytecode in a single shared archive, and a stale cache can look like flaky threading.

`try_send` / `try_recv` are non-blocking variants when you need them.

Channels are **unbounded** today: `try_send` always enqueues (same as `send`) and only fails with `Disconnected` if the sender is closed. `try_recv` returns `WouldBlock` when the queue is empty and the channel is still open.

## Mutex and `with_lock`

`mutex(initial)` allocates a mutex holding a value. Prefer **`with_lock(m, callback)`**: the callback receives the current value and returns `(new_value, result)`; the mutex is updated and `result` is returned to the caller.

```coil
use thread::{join, mutex, spawn, with_lock};
use io::{stdout};
use io::sync::{write_all};
use string::{format, to_bytes};

fn bump(Mutex m) {
    with_lock(m, fn (int n) => (n + 1, 0))?;
}

fn main() {
    let m = mutex(0)?;
    let t1 = spawn(bump, m)?;
    let t2 = spawn(bump, m)?;
    join(t1)?;
    join(t2)?;
    let n = with_lock(m, fn (int x) => (x, x))?;
    write_all(stdout(), to_bytes(format("%i", n)));
}
```

Lower-level `lock` / `unlock` exist but are easy to misuse; `with_lock` is the safe default.

## RwLock

`rwlock(initial)` plus `with_read` / `with_write` (and `try_read` / `try_write`) mirror the mutex pattern for many readers or one writer.

- **`with_lock` / `with_write`** hold the lock for the whole callback. For writes, the callback returns `(new_value, result)` and the lock stores `new_value` before releasing — concurrent threads cannot observe or overwrite that update in between.
- **`with_read` / `try_read`** take a **snapshot**: the read guard is released before the callback runs, so another thread may change the protected value while your callback executes. Use `with_read` only when the callback does not need a consistent view of the live lock contents.
- **`try_write`** (like `with_write`) keeps the write lock held through the callback and commits the returned new value before releasing.

## Errors

`thread::Error` is a sum type registered with the typechecker (e.g. channel closed, lock poisoned, spawn failed). Match on it or propagate with `?` like any other `Result`.

## Runtime model (embedders)

Threading is implemented with **host natives** (`HostInvoke`) — no extra VM opcodes. The main program registers a shared bytecode archive and function table; workers deep-copy what they need and call into the same function offsets. Do not share heap objects across threads except through `Sender` / `Receiver` / `Mutex` / `RwLock` handles.

## Examples

| File | Output |
|------|--------|
| `examples/thread_join.hy` | `42` |
| `examples/thread_channel.hy` | `hello` |
| `examples/thread_reply.hy` | `ping` |
| `examples/thread_mutex.hy` | `2` |

See also the [examples catalog](../examples.md#os-threads).
