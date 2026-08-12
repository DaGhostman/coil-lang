# Userland standard library (`stdlib/`)

Compiler virtual modules cover systems primitives (`io`, `string`, `thread`, …).
This tree is **userland** Coil (`.hy`) layered on top — include `./stdlib` in
`[module].roots` (the workspace manifest already does).

Import **explicitly** — `use path::*` is banned (`E0124`) for userland and
virtual modules alike. Prelude is auto-injected.

**API style:** prefer **method-based APIs** — operations on a type are `impl`
methods (`m.insert(k, v)`), not module-level free functions (`insert(m, k, v)`).
Virtual-module host primitives (`io::read`) stay as free fns.

| Module | Import | Role |
|--------|--------|------|
| `ascii` | `use ascii::{is_digit, …};` | ASCII classify / decimal digit helpers |
| `conv` | `use conv::{parse_int, …};` | `int_to_dec` / `parse_int` / `parse_float` |
| `bytes` | `use bytes::{slice, concat, …};` | `[byte]` slice / concat / find / find_from / replace / pad / eq |
| `text` | `use text::{trim, split, …};` | String helpers via UTF-8 bytes (virtual `string` owns `format` / `to_bytes`) |
| `collections` | `use collections::{sort, …};` | Stable mergesort / `reverse` / `collect_ints` (range → array) |
| `collections::map` | `use collections::map::{HashMap};` | `HashMap` (chaining; methods with `Eq`+`Hash`) |
| `collections::set` | `use collections::set::{HashSet};` | `HashSet` (`HashMap<T, bool>` wrapper) |
| `collections::list` | `use collections::list::{List};` | Mutable singly-linked list |
| `collections::tree` | `use collections::tree::{TreeMap};` | Mutable BST map over `Ord`+`Eq` |
| `num` | `use num::{abs, min, …};` | Numeric conveniences (`abs` overloads; `min`/`max`/`clamp` over `Ord`; `round`; `pow` int/float) |
| `random` | `use random::{u64, range, …};` | CSPRNG wrappers over virtual `crypto` |
| `path` | `use path::{join, dirname, …};` | `join` / `dirname` / `basename` / `extension` |
| `io::sync` | `use io::sync::{write_all, …};` | Blocking adapters + `print` / `println` / `read_line` (`write_all` via `io::write_from`) |
| `io::file` | `use io::file::{read_text, …};` | Whole-file `read_bytes` / `write_text` / … |
| `http` | `use http::client::{get, …};` | HTTP/1.1 client (existing) |

## Notes

- Prefer **byte offsets** for `text::slice` / `find` (mid-codepoint slices error on decode).
- Byte constants use single-byte string literals (`"/"`, `"\n"`) under `byte` /
  `[byte]` expected types; whole strings coerce to `[byte]` / `[byte; N]` too.
- `num` is named so workspace `examples/src/math.hy` does not shadow it.
- IEEE float math (`sin`, `cos`, `tan`, `sqrt`, `floor`, `ceil`, `exp`, `ln`)
  is auto-imported from virtual `prelude::math`. `pow` is userland in `num`
  (float wraps the virtual native; int is iterative) so both overloads share one name.

Tests: `tests/stdlib/` (run via `coil test tests/stdlib` from the repo root, or
per-file under a temp project that points `roots` at this `stdlib/`).
