# Dissect

`coil dissect` re-execs the sibling `coil-dissect` helper (git-style). That helper
compiles a `.hy` entry (and its module graph) **in memory** — it never writes
`out.hyc` — and prints a filtered view of the result for DX (`compiler` feature
`dissect`).

```bash
cargo build   # coil + coil-dissect (+ other helpers)
coil dissect examples/fib.hy --fn fib
coil dissect examples/fib.hy --fn fib --il
coil dissect examples/fib.hy --ast
# or:
coil-dissect examples/fib.hy --fn fib --il
```

| Flag | Effect |
|------|--------|
| (none) | Symbol index + full final fused bytecode (function headers interleaved) |
| `--fn <pat>` | Case-insensitive FQN match (exact, substring, trailing segment, `name#N`) |
| `--il` | Also print **pre-opt** stack IL (snapshot after finalize splices, before lower) |
| `--ast` | Also pretty-print the entry-file AST |

Name filtering uses live compiler FQNs (`fib`, `mod::fib`, `Foo::bar`, `Show__int__show`, …). Archive-side symbol tables are a follow-up; v1 is source-first only.
