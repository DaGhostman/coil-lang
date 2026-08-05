# Userland standard library (`stdlib/`)

Compiler virtual modules cover systems primitives (`io`, `string`, `thread`, …).
This tree is **userland** Coil (`.hy`) layered on top — include `./stdlib` in
`[module].roots` (the workspace manifest already does).

| Module | Import | Role |
|--------|--------|------|
| `bytes` | `use bytes::*;` | `[byte]` slice / concat / find / eq / affixes |
| `text` | `use text::*;` | String helpers via UTF-8 bytes (virtual `string` owns `format` / `to_bytes`) |
| `collections` | `use collections::*;` | `sort` / `reverse` / `collect_ints` (range → array) |
| `num` | `use num::*;` | Scalar math (`abs`, `floor`/`ceil`, `sqrt`, `sin`/`cos`, …) |
| `random` | `use random::*;` | CSPRNG wrappers over virtual `crypto` |
| `path` | `use path::*;` | `join` / `dirname` / `basename` / `extension` |
| `json` | `use json::*;` | Minimal JSON parse / stringify |
| `io::sync` | `use io::sync::*;` | Blocking adapters + `print` / `println` / `read_line` |
| `io::file` | `use io::file::*;` | Whole-file `read_bytes` / `write_text` / … |
| `http` | `use http::client::*;` | HTTP/1.1 client (existing) |

## Notes

- Prefer **byte offsets** for `text::slice` / `find` (mid-codepoint slices error on decode).
- `json` enum constructors are globally unique (`JsonNull`, …); prefer `json_int` /
  `json_object` helpers. Glob `use json::*` imports functions; construct via helpers.
- `num` is named so workspace `examples/src/math.hy` does not shadow it.

Tests: `tests/stdlib/` (run via `coil test tests/stdlib` from the repo root, or
per-file under a temp project that points `roots` at this `stdlib/`).
