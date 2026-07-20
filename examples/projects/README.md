# Showcase example projects

Three escalating apps under `examples/projects/`. Each has its own `zero.toml`,
`src/`, co-located `tests/`, and `NOTES.md`. **Do not** put these suites under
repo-root `tests/` (that tree is for language/compiler harness cases).

| Project | Focus | Demo |
|---------|--------|------|
| `01-todo` | Core language (classes, arrays, modules) | prints a seeded board summary |
| `02-adventure` | Interactive stdin REPL + modules + save/load | playable text adventure |
| `03-echo` | TCP + coroutines + protocol module | single-process echo → `ok` |

## Run demos

```bash
rm -f out.c0s
cargo run --release -- examples/projects/01-todo/src/main.0s

# Adventure is interactive — for CI always wrap with timeout when piping:
printf 'look\ngo north\ntake key\ninventory\ngo south\ngo east\nlook\nquit\n' | \
  timeout 10s cargo run --release -- examples/projects/02-adventure/src/main.0s

timeout 10s cargo run --release -- examples/projects/03-echo/src/main.0s
```

### Playing the adventure

The adventure reads **all of stdin** then splits lines (batch/`read_to_end`).
On a TTY, type commands and send **Ctrl+D** when done (or pipe a transcript).

```bash
rm -f out.c0s
cargo run --release -- examples/projects/02-adventure/src/main.0s
```

Commands: `look`, `go north|south|east|west`, `take` / `take key`,
`inventory`, `save`, `load`, `help`, `quit` / `exit`.

## Per-project unit tests

`zero-script test` only scans **`./tests` relative to CWD**. From each project:

```bash
cd examples/projects/01-todo
cargo run --release --manifest-path ../../../Cargo.toml -- test

cd ../02-adventure
cargo run --release --manifest-path ../../../Cargo.toml -- test

cd ../03-echo
cargo run --release --manifest-path ../../../Cargo.toml -- test
```

That awkward `cd` + `--manifest-path` is intentional documentation of current
ergonomics (no `zero-script test --project …` yet).

## Rolled-up language / tooling gaps

See each project's `NOTES.md` for detail. Highlights:

1. No `read_line` builtin; adventure uses `read_to_end` + `\n` split (Ctrl+D / pipe).
2. No `\n` escapes in string literals (prompts use spaces).
3. Prefer `let s = stdin(); read_to_end(s)` — nested `read_to_end(stdin())` was a
   HostInvoke arg-order bug (fixed; regression: `examples/io_nested_host.0s`).
4. Avoid `x < 0` for EOF/sentinels (unsigned-style compares); use `==` / positive sentinels.
5. Test harness is CWD-`./tests` only; no fixtures / stdout assertions.
6. Adventure unit tests stay pure (parse/move); pipe a transcript with `timeout`
   for end-to-end checks.
