# Userland standard library (`stdlib/`)

Compiler virtual modules cover systems primitives (`io`, `string`, `thread`, …).
This tree is **userland** Coil (`.hy`) layered on top — include `./stdlib` in
`[module].roots` (the workspace manifest already does).

| Module | Import | Role |
|--------|--------|------|
| `ascii` | `use ascii::*;` | ASCII classify / decimal digit helpers |
| `conv` | `use conv::*;` | `int_to_dec` / `parse_int` / `parse_float` |
| `bytes` | `use bytes::*;` | `[byte]` slice / concat / find / replace / pad / eq |
| `text` | `use text::*;` | String helpers via UTF-8 bytes (virtual `string` owns `format` / `to_bytes`) |
| `collections` | `use collections::*;` | `sort` / `reverse` / `collect_ints` (range → array) |
| `num` | `use num::*;` | Numeric conveniences (`abs`/`min`/`max`/`clamp` overloads, `round`, `pow_int`) |
| `random` | `use random::*;` | CSPRNG wrappers over virtual `crypto` |
| `path` | `use path::*;` | `join` / `dirname` / `basename` / `extension` |
| `json` | `use json::*;` | Minimal JSON parse / stringify |
| `io::sync` | `use io::sync::*;` | Blocking adapters + `print` / `println` / `read_line` |
| `io::file` | `use io::file::*;` | Whole-file `read_bytes` / `write_text` / … |
| `http` | `use http::client::*;` | HTTP/1.1 client (existing) |

## Notes

- Prefer **byte offsets** for `text::slice` / `find` (mid-codepoint slices error on decode).
- Byte constants use single-byte string literals (`"/"`, `"\n"`) under `byte` /
  `[byte]` expected types; whole strings coerce to `[byte]` / `[byte; N]` too.
- `json` enum constructors are globally unique (`JsonNull`, …); prefer `json_int` /
  `json_object` helpers. Glob `use json::*` imports functions; construct via helpers.
- `num` is named so workspace `examples/src/math.hy` does not shadow it.
- IEEE float math (`sin`, `cos`, `tan`, `sqrt`, `floor`, `ceil`, `exp`, `ln`,
  `pow`) is auto-imported from virtual `prelude::math`; it is not defined by `num`.

Tests: `tests/stdlib/` (run via `coil test tests/stdlib` from the repo root, or
per-file under a temp project that points `roots` at this `stdlib/`).
