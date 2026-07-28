# `test("…") { … }` (harness cases)

Top-level declaration used by `coil test`. The name must be a **string literal**. The body is typechecked in Result mode (`Result<(), string>`), so `assert(...)?` and `raise` work as in a result-mode function.

```coil
test("addition works") {
    assert(1 + 1 == 2)?;
}
```

Do **not** also define `fn main` in a file that uses `test(...)` cases — the compiler injects a virtual `main` for standalone runs. The `coil test` CLI runs each case in an isolated VM (so a `panic` in one case does not skip later cases) and prints `> Test "<description>" failed` on failure. Pass `--fail-fast` to stop after the first failed case.

### `#[test]` on functions

The same harness semantics apply when tests are declared as attributed functions:

```coil
#[test("addition works")]
fn add_works() {
    assert(1 + 1 == 2)?;
}

#[test]
fn multiply_works() {
    assert(3 * 4 == 12)?;
}
```

The optional string argument is the case description; when omitted, the function name is used. `#[test]` functions and `test("…") { … }` blocks may coexist in one file.

**Production compiles** (`compile`, default `cargo run`) strip harness declarations unless you pass `--include-tests`. The `coil test` command always compiles them.

---

## Related

- [assert](assert.md)
- [Getting Started](../manual/getting-started.md)
