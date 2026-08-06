# `io::fs` module

`use io::fs::{exists, list_dir, realpath, metadata};` — `exists`, `metadata`, `list_dir`, `realpath` (canonical path when it exists), mkdir/remove/rename/copy, symlinks. Returns `prelude::Result` with `IoError`. No whole-file `read`/`write` helpers; open a `Stream` via `io::open` and use `read_to_end` / `write_all`.

---

## Related

- [io](io.md)
