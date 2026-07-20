# 02-adventure — notes

## What it shows

Playable text adventure over **stdin/stdout**: rooms/items, byte-line command
parse, optional file save/load via `io`, modules (`use world::*`).

## Play (interactive)

The REPL reads **all of stdin** with `read_to_end` then splits on `\n`. On a
TTY you type commands and end with **Ctrl+D** (EOF), or pipe a transcript
(below). Always delete stale bytecode first:

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

EOF without `quit` also ends the loop.

## Test

```bash
cd examples/projects/02-adventure
timeout 60s cargo run --release --manifest-path ../../../Cargo.toml -- test
```

Unit tests are **pure** (parse/move) — they do not open a REPL.

## Input / parse notes

1. Commands are compared as `[byte]` (`parse_line` / `bytes_eq` in `world.0s`),
   not via `from_bytes` (that path has been unreliable after dense IO loops).
2. Prefer `let s = stdin(); read_to_end(s)` over nesting. Nested
   `read_to_end(stdin())` was a HostInvoke codegen bug (native id / arg order);
   fixed in the compiler — keep the split form for readability.
3. Avoid negative relational compares for EOF/sentinels (`st < 0` is wrong);
   use `==` and positive sentinels (dir unused = `99`).
4. No `read_line` builtin; no `\n` string escapes in source (spaces in prompts).
5. Test harness is CWD-`./tests` only — `cd` into the project to run tests.
