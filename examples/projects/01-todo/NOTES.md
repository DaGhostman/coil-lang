# 01-todo — notes

## What it shows

In-memory todo board: classes, arrays/`push`/`len`, status ints, format/print,
and a sibling module imported via `use board::*;`.

## Run

```bash
./examples/projects/01-todo/demo.sh
# board:3 done:1 | 1:write tests [Doing] | 2:ship demo [Todo] | 3:nap [Done] |
```

## Test

```bash
./examples/projects/run-tests.sh
# or: cd examples/projects/01-todo && …/zero-script test
```

## Ergonomics / gaps noticed

1. **Enum fields on classes** — `t.status == Status::Todo` has crashed the VM;
   status is stored as `int` instead.
2. **Empty `[Task] = []` is unreliable** — boards use a sentinel task at index 0.
3. **No `\n` string escapes** — demo separates fields with `" | "`.
4. **`zero-script test` only scans `./tests` relative to CWD** — must `cd` into
   the project (no `--project` flag yet).
