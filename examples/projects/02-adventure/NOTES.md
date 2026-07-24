# 02-adventure — notes

## What it shows

Playable text adventure over **stdin/stdout**: rooms/items (`world.hy`),
byte-line command parse (`commands.hy`), save encode/decode (`save.hy`),
REPL + file/stdin IO in `main.hy`. Multiple `use …::*` imports resolve on
the **entry** file.

## Play (interactive)

The REPL reads **all of stdin** with `read_to_end` then splits on `\n`. On a
TTY type commands and end with **Ctrl+D** (EOF), or pipe a transcript:

```bash
./examples/projects/02-adventure/demo.sh
```

Commands: `look`, `go north` / `south` / `east` / `west`, `take` / `take key`,
`inventory`, `save`, `load`, `help`, `quit`.

## CI / canned transcript (always use `timeout`)

```bash
./examples/projects/02-adventure/demo.sh --ci
# or: ./examples/projects/run-demos.sh
```

Input file: `transcript.txt` (Hall → Library → key → Garden → quit).

Expected gist: Hall → Library (take key) → inventory → Hall → Garden → look →
`Bye.`

## Test

```bash
./examples/projects/run-tests.sh
# or: cd examples/projects/02-adventure && …/coil test
```

## Layout

| File | Role |
|------|------|
| `src/world.hy` | Player, rooms, move, key |
| `src/commands.hy` | `Cmd` + `parse_line` / `bytes_eq` |
| `src/save.hy` | Pure 2-byte encode/decode (`SaveData`) |
| `src/main.hy` | REPL + stdin/`open`/`write_all`/`read_to_end` |

## Ergonomics / gaps noticed

1. **IO HostInvoke from a dependency module is broken** — `open`/`write_all`
   must live in the entry file (hence thin `save.hy` + IO wrappers in `main`).
2. No `read_line` builtin — batch `read_to_end` + `\n` split (Ctrl+D / pipe).
3. No `\n` string escapes; prompts use spaces.
4. Commands compared as `[byte]` (not `from_bytes`).
5. Avoid `x < 0` for EOF/sentinels; use `==` / positive sentinels (`99`).
6. Prefer `let s = stdin(); read_to_end(s)` (nested HostInvoke was buggy; fixed).
7. Test harness is CWD-`./tests` only.
