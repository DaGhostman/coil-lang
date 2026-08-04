# Internals

How coil is structured for contributors and embedders. End-user language docs live in the [manual](../manual/getting-started.md) and [references](../references/README.md).

| Document | Contents |
|----------|----------|
| [Pipeline](pipeline.md) | Parse → typecheck → codegen → archive → execute |
| [SIMD](simd.md) | `coil-simd` — stable `std::arch` kernels for packed LA |
| [Auto-par](auto-par.md) | Purity analysis + capped fork-join for recursive binops |
| [Debug line table](debug-info.md) | `source_files` / `debug_locs` in `.hyc` |
| [Opcodes](opcodes.md) | Selected bytecode ops behind builtins |
| [Dissect](dissect.md) | `coil dissect` — in-memory bytecode / IL / AST dump |
| [Debugger](debugger.md) | `coil debug` — GDB-style REPL / batch debugger |
| [Formatter](fmt.md) | `coil fmt` — AST pretty-printer for `.hy` |
| [Test health report](test-health-report.md) | Historical flaky/broken-test notes |
| [Grammar](grammar/) | tree-sitter grammar sources |

## Crate map

| Crate | Role |
|-------|------|
| `parser` | Pratt parser and AST |
| `compiler` | HM typechecker, stack IL codegen, pipeline |
| `machine` | VM, heap/GC, FFI (libffi), host natives |
| `common` | Opcodes, values, archive format |
| `coil-simd` | Stable SIMD helpers (`std::arch`) for numeric / byte kernels |
