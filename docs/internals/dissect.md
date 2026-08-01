# Dissect

`coil dissect` compiles a `.hy` entry (and its module graph) **in memory** — it never writes `out.hyc` — and prints a filtered view of the result for DX.

```bash
coil dissect examples/fib.hy --fn fib
coil dissect examples/fib.hy --fn fib --il
coil dissect examples/fib.hy --ast
```

| Flag | Effect |
|------|--------|
| (none) | Symbol index + full final fused bytecode (function headers interleaved) |
| `--fn <pat>` | Case-insensitive FQN match (exact, substring, trailing segment, `name#N`) |
| `--il` | Also print **pre-opt** stack IL (snapshot after finalize splices, before lower) |
| `--ast` | Also pretty-print the entry-file AST |

Name filtering uses live compiler FQNs (`fib`, `mod::fib`, `Foo::bar`, `Show__int__show`, …). Archive-side symbol tables are a follow-up; v1 is source-first only.
