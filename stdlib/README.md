# Userland standard library (`stdlib/`)

Compiler virtual modules cover systems primitives (`io`, `string`, `thread`, …).
This tree is **userland** Coil (`.hy`) layered on top — include `./stdlib` in
`[module].roots` (the workspace manifest already does).

Import **explicitly** (no `use mod::*` for userland disks). Virtual modules such
as `io` / `string` / `ffi` may still use `::*`; prelude is auto-imported.

| Module | Import | Role |
|--------|--------|------|
| `ascii` | `use ascii::{is_digit, …};` | ASCII classify / decimal digit helpers |
| `conv` | `use conv::{parse_int, …};` | `int_to_dec` / `parse_int` / `parse_float` |
| `bytes` | `use bytes::{slice, concat, …};` | `[byte]` slice / concat / find / replace / pad / eq |
| `text` | `use text::{trim, split, …};` | String helpers via UTF-8 bytes (virtual `string` owns `format` / `to_bytes`) |
| `collections` | `use collections::{sort, …};` | `sort` / `reverse` / `collect_ints` (range → array) |
| `num` | `use num::{abs, min, …};` | Numeric conveniences (`abs` overloads; `min`/`max`/`clamp` over `Ord`; `round`, `pow_int`) |
| `random` | `use random::{u64, range, …};` | CSPRNG wrappers over virtual `crypto` |
| `path` | `use path::{join, dirname, …};` | `join` / `dirname` / `basename` / `extension` |
| `json` | `use json::{parse, stringify, …};` | Minimal JSON parse / stringify |
| `io::sync` | `use io::sync::{write_all, …};` | Blocking adapters + `print` / `println` / `read_line` |
| `io::file` | `use io::file::{read_text, …};` | Whole-file `read_bytes` / `write_text` / … |
| `http` | `use http::client::{get, …};` | HTTP/1.1 client (existing) |

## Notes

- Prefer **byte offsets** for `text::slice` / `find` (mid-codepoint slices error on decode).
- Byte constants use single-byte string literals (`"/"`, `"\n"`) under `byte` /
  `[byte]` expected types; whole strings coerce to `[byte]` / `[byte; N]` too.
- `json` enum constructors are globally unique (`JsonNull`, …); prefer `json_int` /
  `json_object` helpers. Import functions explicitly; construct via helpers.
- `num` is named so workspace `examples/src/math.hy` does not shadow it.
- IEEE float math (`sin`, `cos`, `tan`, `sqrt`, `floor`, `ceil`, `exp`, `ln`,
  `pow`) is auto-imported from virtual `prelude::math`; it is not defined by `num`.

Tests: `tests/stdlib/` (run via `coil test tests/stdlib` from the repo root, or
per-file under a temp project that points `roots` at this `stdlib/`).
