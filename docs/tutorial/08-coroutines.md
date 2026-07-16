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

`resume h` continues the coroutine until the next `yield` or `return`. The yielded (or returned) value becomes the result of the `resume` expression — `resume` has a single static result type covering both.

```0s
async fn two_step() {
    yield 10;
    yield 20;
    return 30; // completion value — same type as the yields above
}

fn main() {
    let h = two_step();
    let a = resume h;
    let b = resume h;
    let c = resume h;
    print "%i", a;  // 10
    print "%i", b;  // 20
    print "%i", c;  // 30 (the `return` value)
}
```

Resuming an already-**done** coroutine always returns `0` (`Value::default()`) — never the coroutine's last `return` value. There's no error-handling protocol yet to signal "resumed after completion", so this fixed sentinel avoids leaking a stale value (MVP protocol; a real error/`Result` mechanism is future work).

Use `done(h)` to ask whether a handle has completed (returns `bool`):

```0s
let h = two_step();
print "%z", done(h); // false
resume h;
resume h;
resume h;            // completes
print "%z", done(h); // true
```

`resume h` can be used inline anywhere an expression is expected, including directly as a `print` argument:

```0s
print "%i,", resume h;
```

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

Two handles are independent — resuming one does not advance the other, even when both handles come from the same (possibly parameterized) `async fn`:

```0s
async fn counter(int base) {
    yield base;
    yield base + 1;
    yield base + 2;
}

fn main() {
    let a = counter(1);
    let b = counter(100);

    print "%i,", resume a; // 1
    print "%i,", resume b; // 100
    print "%i,", resume a; // 2
    print "%i", resume b;  // 101
}
```

See `examples/coro_interleave.0s` for a longer alternating-`resume` example.

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
