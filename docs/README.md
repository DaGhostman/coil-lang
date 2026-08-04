# coil

**coil** is a statically typed scripting language with Hindley–Milner type inference. Programs are compiled to bytecode and executed on a custom virtual machine. Source files use the `.hy` extension (short for **henry**, the SI unit of inductance — the measure of a coil); compiled archives are stored as `.hyc` files.

The language targets embeddable scripting: you get real type checking and inference without a heavyweight build pipeline, plus optional FFI for calling into C libraries or host-provided Rust closures.

## Quick start

```coil
use io::{stdout, write_all};
use string::{format, to_bytes};
fn main() {
    write_all(stdout(), to_bytes("Hello, world!"));
}
```

Run any program from the repository root:

```bash
cargo build   # coil + coil-debug + coil-dissect + coil-fmt + coil-lsp + coil-embed
cargo run -- examples/print_literal.hy
```

Expected output: `hello`

See [Getting Started](manual/getting-started.md) for prerequisites, project layout, and a guided first run.

## How programs run

Parse → typecheck (HM) → stack IL codegen + lower/fuse-select → versioned `.hyc` archive (packed `major.minor`) → VM executes `main`. Cached `out.hyc` is reused until sources/version/entry change; delete it to force a rebuild. Full stage notes: [Internals — Pipeline](internals/pipeline.md).

## Language at a glance

| Area | Status |
|------|--------|
| Primitives | `int`, `float`, `string`, `bool`, `byte` |
| Functions, `let` / `const`, `if`/`else`, `while` / `for` | Supported |
| Named call-site arguments (`f(name: v)`) | Supported (positional prefix then named; named holes on partials allowed) |
| Arity overloads / first-class fn values / lambdas (`use`) | Supported (`examples/overload.hy`, `fn_value.hy`, `lambda.hy`) |
| Rest parameters (`T... xs` / tuple `... xs`) | Supported (trailing only; `T...` packs to `[T]`, bare `...` packs to a tuple) |
| Call-site spread (`f(...pack)`) | Supported (tuple and array operands) |
| User-defined `attr` decorators | Supported (`attr` decl + `#[name(...)]` on `fn`, methods, class constructors) |
| Let destructuring (`let (a, b) = …`, `let { x, y } = …`) | Supported (tuple / record; no enum ctor patterns in `let`) |
| `break` / `continue` | Supported |
| Enums, `match`, record variants | Supported |
| Built-in `Option` / `Result`, `raise`, `?`, `??`, `?.` | Supported (desugar to match/return) |
| Tuples, arrays (`arr[] =` / `len`), dicts (anonymous records) | Supported |
| Type aliases (`type Name = T;`, lexically scoped) | Supported |
| Generics and traits | Supported: generic functions/enums/aliases/classes, higher-kinded type parameters, associated types/GATs, existentials, coherence checks |
| Modules / namespaces (`use`, `mod`) | Supported (multi-file CLI via `coil.toml`) |
| Field access (`p.x`, chained `p.x.y`) | Supported |
| FFI (`extern` blocks, `dload`/`declare`/`invoke`, C varargs `...`, struct/callback returns) | Supported (requires libffi) |
| IO streams (`use io::*;`, `[byte]`, files, sync adapters, TCP, UDP) | Supported (non-blocking L0; no HTTP in VM) |
| Classes (`class` / `impl` / `new`, fields, methods) | Supported |
| Coroutines (`async`, `yield`, `resume`, `yield from`, `done`) | Supported |
| `for x in` (Iterator / IntoIterator) | Supported (arrays, homogeneous tuples/dicts, ranges, coroutines, user `impl`s) |
| Ranges (`a..b` / `a..=b`) | Supported — lazy `Range<T: Ord>`; `for` steps `int`/`byte`/`float`; no auto array materialize ([syntax](references/syntax.md#ranges-lazy)) |
| String concat via `+` | Supported (`string + string` → `string`) |
| `string::format(...)` | Supported compiler intrinsic (returns `string`; literal specifiers are checked) |

Browse runnable demos in [Examples](manual/examples.md). Multi-file showcase apps (todo board, text adventure, TCP echo, HTTP client) live under [`examples/projects/`](../examples/projects/README.md). See also [HTTP/1.1 client](manual/http-client.md).

## Documentation

Docs are split into three trees:

| Tree | Audience | Start here |
|------|----------|------------|
| [Manual](manual/getting-started.md) | Learners | Getting started, tutorials 01–11, examples catalog |
| [References](references/README.md) | Lookup | Syntax, types, keywords, per-API builtin pages |
| [Internals](internals/README.md) | Contributors / embedders | Pipeline, debug info, opcodes, grammar |


### Manual (tutorial)

| Chapter | Topic |
|---------|-------|
| [Getting Started](manual/getting-started.md) | Build, first run, cache |
| [01 — Basics](manual/tutorial/01-basics.md) | Syntax, functions, `let`, control flow |
| [02 — Types & Variables](manual/tutorial/02-types-and-variables.md) | Primitives, inference, annotations |
| [03 — Enums & Match](manual/tutorial/03-enums-and-match.md) | Sum types and pattern matching |
| [04 — Records & Fields](manual/tutorial/04-records-and-fields.md) | Record variants, field access, nested patterns |
| [05 — Aggregates](manual/tutorial/05-aggregates.md) | Tuples, arrays, dicts, type aliases |
| [06 — Modules](manual/tutorial/06-modules.md) | `use`, `mod`, `coil.toml` |
| [07 — FFI](manual/tutorial/07-ffi.md) | `extern` blocks and dynamic loading |
| [08 — Coroutines](manual/tutorial/08-coroutines.md) | `async fn`, resume, send/receive, `yield from`, `for x in` |
| [09 — Error handling](manual/tutorial/09-error-handling.md) | Built-in Option/Result, `raise`, `?`, `??`, `?.` |
| [10 — IO streams](manual/tutorial/10-io-streams.md) | `byte` / `[byte]`, `Stream`, files, sync adapters, TCP |
| [11 — OS threads](manual/tutorial/11-threads.md) | `use thread::*`, `spawn` / `join`, channels, mutexes |
| [Examples catalog](manual/examples.md) | Every file in `examples/`, expected output |
| [Showcase projects](../examples/projects/README.md) | Multi-file apps + co-located tests |

Classes (`class`, `impl`, `new`) — see [02 — Types & Variables](manual/tutorial/02-types-and-variables.md) and `examples/classes.hy`. Full API index: [References](references/README.md).

## Repository layout

```
coil/
├── common/          # Shared types: opcodes, values, archive format
├── parser/          # Pratt parser and AST
├── compiler/        # HM typechecker, stack IL codegen, pipeline
├── machine/         # VM, heap/GC, FFI (libffi)
├── examples/        # Runnable .hy demos (see manual/examples.md)
│   └── projects/    # Showcase multi-file apps + co-located tests
├── docs/
│   ├── manual/      # End-user guide + tutorials
│   ├── references/  # Language + per-API lookup
│   └── internals/   # Pipeline, VM notes, grammar
├── src/main.rs      # CLI: default build+run, compile, run, test
└── coil.toml.example  # Example project manifest
```

## Building and running

```bash
# Build CLI binaries (coil + helpers)
cargo build
# Or every workspace crate:
cargo build --workspace

# Default: compile to out.hyc (cached) and run
cargo run -- examples/fib.hy

# Compile only / run archive / project tests
cargo run -- compile examples/fib.hy -o fib.hyc
cargo run -- run fib.hyc
cargo run -- test   # every .hy under ./tests

# Release build
cargo build --release --workspace
cargo run --release -- examples/fib.hy
```

| Command | Role |
|---------|------|
| *(no subcommand)* `<file.hy>` | Compile → `out.hyc` (cached) → run |
| `compile <file.hy> [-o path]` | Compile entry file to a `.hyc` archive |
| `run <file.hyc>` | Execute a compiled archive |
| `package <file.hy> [-o path] [--check-native]` | Single executable (embeds `.hyc` into `coil-embed` by default) |
| `test [path] [--fail-fast]` | Compile+run all `[path]/**/*.hy` (default `./tests`); continue after failures unless `--fail-fast` |
| `lsp` | Start the Coil language server over stdin/stdout |

For FFI examples you also need **libffi** (e.g. `libffi-dev` on Debian/Ubuntu, `libffi` on Arch). See [Getting Started](manual/getting-started.md).

## Learn by example

| Goal | Start with |
|------|------------|
| First program | [getting-started.md](manual/getting-started.md) → `examples/fib.hy` |
| Enums & pattern matching | `examples/option.hy`, `examples/result.hy` |
| Record-shaped variants | `examples/record.hy`, `examples/mixed.hy` |
| Dicts / anonymous records | `examples/dict.hy` |
| Generics & traits | `examples/generics.hy`, `examples/hkt_bifunctor.hy`, `examples/gat_pointer.hy`, `examples/existential_show.hy` |
| Modules | `examples/modules.hy` (see [examples.md](manual/examples.md) for setup) |
| FFI | `examples/strlen.hy`, `examples/ffi_sum.hy`, `examples/ffi_printf.hy` |
| IO streams | `examples/io_bytes.hy`, `examples/io_file.hy`, `examples/io_eof.hy`, `examples/io_udp.hy` |
| Coroutines | `examples/coro.hy`, `examples/coro_gen.hy`, `examples/coro_send.hy`, `examples/for_in_coro.hy` |
| Full catalog | [examples.md](manual/examples.md) |
