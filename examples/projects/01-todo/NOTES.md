# 01-todo — notes

## What it shows

In-memory todo board: classes, arrays/`push`/`len`, match-free status ints,
format/print, and a sibling module imported via `use board::*;`.

## Run

```bash
rm -f out.c0s
cargo run --release -- examples/projects/01-todo/src/main.0s
# board:3 done:1 | 1:write tests [Doing] | 2:ship demo [Todo] | 3:nap [Done] |
```

## Test

```bash
cd examples/projects/01-todo
cargo run --release --manifest-path ../../../Cargo.toml -- test
```

(or `/workspace/target/release/zero-script test` from this directory)

## Ergonomics / gaps noticed

1. **Sibling free-fn calls in non-entry modules fail** (`Unknown function`) —
   helpers that call each other must live in the entry file, or be fully inlined.
2. **Enum fields on classes** — `t.status == Status::Todo` can crash the VM;
   status is stored as `int` instead.
3. **Empty `[Task] = []` is unreliable** — boards use a sentinel task at index 0.
4. **No `\n` string escapes** — demo separates fields with `" | "`.
5. **`zero-script test` only scans `./tests` relative to CWD** — must `cd` into
   the project (no `--project` flag yet).
