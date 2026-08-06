# String table, `string::format`, and retiring `print`

## Goals

1. Archive a dedicated **string table**; `STRING` is an index (no inline `DATA` runs).
2. Virtual **`string`** module: `format`, `from_bytes`, `to_bytes` (text helpers also remain as `io` aliases).
3. Remove **`print`** / **`format`** keywords; write via `io::write` / `write_all` on `stdout()`.
4. Keep `FORMAT` opcode for compile-time-checked formatting (`%v` / `Show`).

## Archive (version **35**)

`ArchivedProgram` gains `strings: Vec<String>`.  
`STRING` operand = index into that table. `DATA` stays as a tombstone discriminant (never emitted).

## Runtime

- Machine holds `program_strings` next to `program_constants`.
- `STRING` → `heap.intern(program_strings[idx])`.
- Stdout/stderr `write`/`write_all` honor `Machine::with_output` via a thread-local redirect (tests keep capturing).

## Language surface

```coil
use io::{stdout};
use io::sync::{write_all};
use string::{format, to_bytes};

write_all(stdout(), to_bytes(format("%i", n)));
let s = format("%s-%i", name, n);
```

- `string::format` is a compiler intrinsic (same rules as old `format` / `print` specs).
- `io::{from_bytes,to_bytes}` remain aliases of the string natives for one cycle.

## Opcode policy

No middle inserts. Redefine `STRING` under the version bump; leave `DATA` / `PRINT` discriminants unused by the compiler.
