# Internals

How coil is structured for contributors and embedders. End-user language docs live in the [manual](../manual/getting-started.md) and [references](../references/README.md).

| Document | Contents |
|----------|----------|
| [Pipeline](pipeline.md) | Parse → typecheck → codegen → archive → execute |
| [Debug line table](debug-info.md) | `source_files` / `debug_locs` in `.hyc` |
| [Opcodes](opcodes.md) | Selected bytecode ops behind builtins |
| [Test health report](test-health-report.md) | Historical flaky/broken-test notes |
| [Grammar](grammar/) | tree-sitter grammar sources |
| [HTTP/TLS stdlib design](../superpowers/specs/2026-07-28-http-tls-stdlib-design.md) | Host TLS + userland HTTP client plan (three PRs) |

## Crate map

| Crate | Role |
|-------|------|
| `parser` | Pratt parser and AST |
| `compiler` | HM typechecker, codegen, pipeline, peephole |
| `machine` | VM, heap/GC, FFI (libffi), host natives |
| `common` | Opcodes, values, archive format |
