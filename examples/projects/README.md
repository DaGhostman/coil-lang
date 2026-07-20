# Showcase example projects

Three escalating apps under `examples/projects/`. Each has its own `zero.toml`,
`src/`, co-located `tests/`, and `NOTES.md`. **Do not** put these suites under
repo-root `tests/` (that tree is for language/compiler harness cases).

| Project | Focus | Demo |
|---------|--------|------|
| `01-todo` | Core language (classes, arrays, modules) | prints a seeded board summary |
| `02-adventure` | Interactive stdin REPL + modules + save/load | playable text adventure |
| `03-echo` | TCP + coroutines + protocol module | single-process echo → `ok` |

## Run demos (scripts)

From the repo root (builds a release binary if needed):

```bash
./examples/projects/run-demos.sh     # todo + adventure transcript + echo
./examples/projects/run-tests.sh     # co-located tests for all three
```

Per project:

```bash
./examples/projects/01-todo/demo.sh
./examples/projects/02-adventure/demo.sh          # interactive on a TTY
./examples/projects/02-adventure/demo.sh --ci     # pipe transcript.txt under timeout
./examples/projects/03-echo/demo.sh
```

Adventure CI input lives in `02-adventure/transcript.txt`.

### Playing the adventure

The adventure reads **all of stdin** then splits lines (batch/`read_to_end`).
On a TTY, type commands and send **Ctrl+D** when done (or pipe a transcript).

```bash
./examples/projects/02-adventure/demo.sh
```

Commands: `look`, `go north|south|east|west`, `take` / `take key`,
`inventory`, `save`, `load`, `help`, `quit` / `exit`.

Manual equivalent (no scripts):

```bash
rm -f out.c0s
cargo run --release -- examples/projects/01-todo/src/main.0s
timeout 10s cargo run --release -- examples/projects/02-adventure/src/main.0s \
  < examples/projects/02-adventure/transcript.txt
timeout 10s cargo run --release -- examples/projects/03-echo/src/main.0s
```

## Per-project unit tests

Prefer `./examples/projects/run-tests.sh`. The harness only scans
**`./tests` relative to CWD**, so the script `cd`s into each project:

```bash
cd examples/projects/01-todo
cargo run --release --manifest-path ../../../Cargo.toml -- test
```

That awkward `cd` + `--manifest-path` is intentional documentation of current
ergonomics (no `zero-script test --project …` yet).

## Module layout (matches the plan)

| Project | Modules |
|---------|---------|
| `01-todo` | `board.0s` + `main.0s` |
| `02-adventure` | `world.0s` + `commands.0s` + `save.0s` + `main.0s` |
| `03-echo` | `protocol.0s` + `server.0s` + `client.0s` + `main.0s` |

## Rolled-up language / tooling gaps

See each project's `NOTES.md` for detail. Highlights:

1. **IO HostInvoke from dependency modules is broken** — keep `open` / TCP /
   `stdin` calls in the entry `main.0s`; dep modules stay pure helpers.
   **Symptom:** a helper in a non-entry module that calls `open`/`write_all`
   can abort at runtime (`jump_if_match_target` with an empty constant pool —
   the `Result` unwrap after HostInvoke). Likely multi-file compile /
   constant-pool / `JumpIfMatch` wiring when the call site is not the entry
   file. Not fixed in this PR; work around by keeping Stream IO in `main.0s`.
2. **`use` of a sibling module from a non-entry file** may not resolve free-fn
   calls — call shared helpers from the entry, or keep dep modules self-contained.
3. No `read_line` builtin; adventure uses `read_to_end` + `\n` split (Ctrl+D / pipe).
4. No `\n` escapes in string literals (prompts use spaces).
5. Prefer `let s = stdin(); read_to_end(s)` — nested `read_to_end(stdin())` was a
   HostInvoke arg-order bug (fixed; regression: `examples/io_nested_host.0s`).
6. Avoid `x < 0` for EOF/sentinels; use `==` / positive sentinels.
7. Test harness is CWD-`./tests` only; pipe adventure transcripts under `timeout`.
