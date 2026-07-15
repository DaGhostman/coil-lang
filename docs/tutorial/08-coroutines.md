# 08 — Coroutines

zero-script supports **stackful coroutines** via `async fn`, `yield`, `resume`, and (Phase 2) bidirectional send/receive and `yield from` delegation.

## Creating a coroutine

An `async fn` returns a **handle** with type `coroutine<Y>` when it only yields values out, or `coroutine<Y, S>` when it also receives values on resume (`S` defaults to `unit` when unused).

```0s
async fn counter() {
    yield 0;
    yield 1;
    yield 2;
}

fn main() {
    let h = counter();
    let v = resume h;
    print "%i", v;  // 0
}
```

Calling an async function emits `MakeCoro` — it allocates a suspended coroutine object and pushes a handle. Nothing runs until you `resume`.

## Resuming

`resume h` continues the coroutine until the next `yield` or `return`. The yielded value becomes the result of the `resume` expression.

```0s
async fn two_step() {
    yield 10;
    yield 20;
}

fn main() {
    let h = two_step();
    let a = resume h;
    let b = resume h;
    print "%i", a;  // 10
    print "%i", b;  // 20
}
```

Resuming a **done** coroutine returns `0` (MVP protocol).

> **Tip:** Bind the result of `resume` to a local (`let v = resume h;`) before passing it to `print`. Inline `print "%i", resume h` can leave extra handles on the operand stack and corrupt later resumes.

## Send and receive (Phase 2)

Resume with a value:

```0s
resume h with expr
```

Receive at a yield site:

```0s
let msg = yield "ready";
```

The send type `S` in `coroutine<Y, S>` is inferred from binding-yield patterns and `resume h with v` sites.

Example (`examples/coro_send.0s`):

```0s
async fn ping() {
    let msg = yield "ready";
    print "%s", msg;
}

fn main() {
    let h = ping();
    resume h;
    resume h with "hello";
}
```

Output: `hello`

## Yield from

Delegate to another coroutine; values and sends propagate through the delegate chain.

```0s
async fn inner() {
    yield 0;
    yield 1;
}

async fn outer() {
    yield from inner();
}

fn main() {
    let h = outer();
    let v0 = resume h;
    let v1 = resume h;
    print "%i", v0;
    print "%i", v1;
}
```

Output: `01` (from `examples/coro_yield_from.0s`).

## Interleaving

Two handles are independent — resuming one does not advance the other. See `examples/coro_interleave.0s` for alternating `resume` calls.

## Recompiling

Coroutines added new VM opcodes; delete stale archives after upgrading:

```bash
rm -f out.c0s
cargo run -- examples/coro_send.0s
```

`ARCHIVE_VERSION` is **9** (Phase CORO-2).

## Related

- [Keywords — coroutines](../reference/keywords.md)
- [Types — coroutine<Y, S>](../reference/types.md)
- [Examples catalog](../examples.md)
