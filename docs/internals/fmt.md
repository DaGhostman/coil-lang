# Formatter (`coil fmt`)

`coil fmt` re-execs the sibling `coil-fmt` helper (git-style). That helper parses
`.hy` sources and pretty-prints from the AST (hardcoded 4-space indent).

```bash
cargo build   # coil + coil-fmt (+ other helpers)
coil fmt path/to/file.hy
coil fmt src/
coil fmt --check .
# or invoke the helper directly:
coil-fmt --check examples/fib.hy
```

## Behavior

| Mode | Effect |
|------|--------|
| default | Rewrite files in place |
| `--check` | Report paths that would change; exit `1` if any; no writes |

Directories are walked recursively for `*.hy`. Non-`.hy` files given explicitly are rejected.

## Line wrapping

Soft wraps kick in when a construct would exceed **100** columns:

| Construct | Break style |
|-----------|-------------|
| `&&` / `\|\|` / `??` chains | Operator stays at end of the previous line; continuation hangs under the first operand |
| `.` / `?.` member and method chains | Each `.name` / `.name(args)` on its own line at +1 indent |

Short chains stay on one line.

## Comments and docs

- `//` line comments are preserved (AST `Expression::Comment`).
- `///` doc comments attach to the following declaration (`fn`, `class`, `field`, `trait`, `enum`, …) as `docs: Vec<&str>`. Read them later via [`parser::item_docs`](../../parser/src/ast.rs).
- Orphan `///` (not immediately before a documentable item) is a **parse error**.
- Attributes may follow docs: `/// …` then `#[…]` then the keyword.

## Limitations

- Style is fixed (4 spaces); there is no config file yet.
- No `/* */` block comments.
- Parse errors abort that file (pretty diagnostics via the reporting crate); other files in a multi-path run still process.
