# 11 — OS threads

coil can run **native OS threads** alongside the main VM. Each worker gets its own `Machine` and heap; communication uses typed channels and mutexes from the virtual **`thread`** module (`use thread::*;`).

This is separate from **coroutines** ([08 — Coroutines](08-coroutines.md)): coroutines are cooperative handles on one VM; `spawn` starts a real thread with an isolated bytecode interpreter.

## Import

```coil
use thread::*;
```

All primitives return `prelude::Result<…, thread::Error>`. Use `?` in result-mode functions (omit an explicit `-> int` return type when you want `?` to propagate errors).

## Spawning and joining

`spawn(f)` runs nullary function `f` on a new thread. `spawn(f, arg)` passes one argument (the function must be `fn (A) -> R`).

```coil
use thread::*;

fn work() -> int {
    return 40 + 2;
}

fn main() {
    let t = spawn(work)?;
    print "%i", join(t)?;
}
```

`join(t)` blocks until the worker finishes and returns its result value. `detach(t)` lets the thread run without a join (errors if you later `join` the same handle).

## Channels

`channel()` returns `(Sender, Receiver)` as a two-tuple. `send` / `recv` move values between threads; `close` drops the sender side.

```coil
use thread::*;

fn producer(Sender tx) {
    send(tx, "hello")?;
}

fn main() {
    let pair = channel()?;
    let tx = pair[0];
    let rx = pair[1];
    let t = spawn(producer, tx)?;
    print "%s", recv(rx)?;
    join(t)?;
}
```

`try_send` / `try_recv` are non-blocking variants when you need them.

Channels are **unbounded** today: `try_send` always enqueues (same as `send`) and only fails with `Disconnected` if the sender is closed. `try_recv` returns `WouldBlock` when the queue is empty and the channel is still open.

## Mutex and `with_lock`

`mutex(initial)` allocates a mutex holding a value. Prefer **`with_lock(m, callback)`**: the callback receives the current value and returns `(new_value, result)`; the mutex is updated and `result` is returned to the caller.

```coil
use thread::*;

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
    print "%i", n;
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
| `examples/thread_mutex.hy` | `2` |

See also the [examples catalog](../examples.md#os-threads).
