# 02-adventure — notes

## What it shows

Playable text adventure over **stdin/stdout**: rooms/items (`world.0s`),
byte-line command parse (`commands.0s`), save encode/decode (`save.0s`),
REPL + file/stdin IO in `main.0s`. Multiple `use …::*` imports resolve on
the **entry** file.

## Play (interactive)

The REPL reads **all of stdin** with `read_to_end` then splits on `\n`. On a
TTY type commands and end with **Ctrl+D** (EOF), or pipe a transcript:

```bash
rm -f out.c0s
cargo run --release -- examples/projects/02-adventure/src/main.0s
```

Commands: `look`, `go north` / `south` / `east` / `west`, `take` / `take key`,
`inventory`, `save`, `load`, `help`, `quit`.

## CI / canned transcript (always use `timeout`)

```bash
rm -f out.c0s
printf 'look\ngo north\ntake key\ninventory\ngo south\ngo east\nlook\nquit\n' | \
  timeout 10s ./target/release/zero-script examples/projects/02-adventure/src/main.0s
```

Expected gist: Hall → Library (take key) → inventory → Hall → Garden → look →
`Bye.`

## Test

```bash
cd examples/projects/02-adventure
timeout 60s cargo run --release --manifest-path ../../../Cargo.toml -- test
```

## Layout

| File | Role |
|------|------|
| `src/world.0s` | Player, rooms, move, key |
| `src/commands.0s` | `Cmd` + `parse_line` / `bytes_eq` |
| `src/save.0s` | Pure 2-byte encode/decode (`SaveData`) |
| `src/main.0s` | REPL + stdin/`open`/`write_all`/`read_to_end` |

## Ergonomics / gaps noticed

1. **IO HostInvoke from a dependency module is broken** — `open`/`write_all`
   must live in the entry file (hence thin `save.0s` + IO wrappers in `main`).
2. No `read_line` builtin — batch `read_to_end` + `\n` split (Ctrl+D / pipe).
3. No `\n` string escapes; prompts use spaces.
4. Commands compared as `[byte]` (not `from_bytes`).
5. Avoid `x < 0` for EOF/sentinels; use `==` / positive sentinels (`99`).
6. Prefer `let s = stdin(); read_to_end(s)` (nested HostInvoke was buggy; fixed).
7. Test harness is CWD-`./tests` only.
