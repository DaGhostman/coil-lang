# coil

Statically typed scripting language with Hindley–Milner inference. Source files use the `.hy` extension; compiled archives are `.hyc`.

```bash
git clone --recurse-submodules git@github.com:ardax-corp/coil-lang.git
cd coil-lang
cargo build
cargo run -- examples/fib.hy    # prints 55
```

## Documentation

| Audience | Start here |
|----------|------------|
| Users | [docs/README.md](docs/README.md) → [Getting started](docs/manual/getting-started.md) |
| Language reference | [docs/references/README.md](docs/references/README.md) |
| Contributors | [AGENTS.md](AGENTS.md) · [CONTRIBUTING.md](CONTRIBUTING.md) · [docs/internals/](docs/internals/README.md) |
| Userland stdlib | [coil-stdlib](https://github.com/ardax-corp/coil-stdlib) ([stdlib/README.md](stdlib/README.md) submodule) |

## Features

Primitives (`int`, `float`, `string`, `bool`, `byte`), enums and `match`, records and dicts, generics and traits, classes, coroutines, `for x in`, ranges, FFI, non-blocking IO with sync adapters, OS threads, and a userland stdlib (`collections`, `text`, `bytes`, `http`, …).

Full feature matrix: [docs/README.md](docs/README.md#language-at-a-glance).

## Repository layout

```
coil/
├── common/     # Opcodes, values, archive format
├── parser/     # Pratt parser, AST
├── compiler/   # HM typechecker, stack IL codegen, pipeline
├── machine/    # VM, heap/GC, FFI, host natives
├── coil-*/     # CLI helpers (debug, dissect, fmt, lsp, embed)
├── stdlib/     # git submodule: ardax-corp/coil-stdlib (`src/` is the module root)
├── examples/   # Runnable demos
├── tests/      # Integration tests (`coil test`)
└── docs/       # Manual, references, internals
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Agent-oriented invariants live in [AGENTS.md](AGENTS.md).
