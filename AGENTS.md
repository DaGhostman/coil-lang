# coil — AGENTS

Statically typed scripting language (HM inference) → stack IL → `.hyc` archive → custom VM. Sources: `.hy`. Deep syntax/API: `docs/` and `.cursor/skills/coil-language`. Compiler/VM work: `.cursor/skills/coil-contributor`. Debugging: `.cursor/skills/coil-debug`.

## User preferences

- Run tests with 64MB limit: `ulimit -v 65536 && cargo run -- test` (leak smoke).
- Soft CPU baseline: `./scripts/poop_baseline.sh` — not a hard CI gate.
- Large tasks: scoped sub-agents on disjoint modules; avoid over-parallelizing.
- VM perf: prefer alloc reduction, hot-loop tuning, bounds-check elimination, `promise!` over new opcodes or IL opts; reject benchmark-shaped opcodes unless universal.
- Language features: draft plans first; full HM integration; update `docs/`; minimal runnable example with expected output.
- Granular conventional commits; stage only related files.
- Prefer compiler virtual modules over userland for core machinery.

## Workspace

| Area | Location |
|------|----------|
| Parser | `parser/` |
| Typechecker + codegen | `compiler/` (`lib.rs`, `il/`, `pipeline.rs`) |
| VM / FFI / natives | `machine/` |
| Opcodes / archive | `common/` (`opcode.rs`, `archive.rs`) |
| CLI | `src/main.rs` |
| User docs | `docs/manual/`, `docs/references/`, `docs/internals/` |
| Examples / tests | `examples/`, `tests/`, `coil test` |

Single compilation path: stack codegen in `compiler/src/lib.rs` (no register VM).

`ARCHIVE_VERSION` is **35** (`common/src/archive.rs`) — bump on incompatible bytecode, tags, or opcodes.

Smoke: `examples/fib.hy` (`fib(10)` → `55`). Bench: `examples/fib_bench.hy` (`poop`, `vm_bench.sh`, `perf_metrics`).

## CLI

| Invocation | Behavior |
|------------|----------|
| `coil <file.hy>` | In-memory compile + run (**no `out.hyc`**) |
| `coil compile [file] [-o path]` | Writes archive (default `out.hyc`); always recompiles |
| `coil run <file.hyc>` | Run archive only; rejects version mismatch |
| `coil test [path] [--fail-fast]` | `**/*.hy` under path (default `./tests`); in-memory |
| `coil dissect` / `debug` | In-memory compile; no archive file |

`coil compile` + `coil run`: delete or re-`compile` when sources change — `run` does not recompile.

No `print` statement — use `use io::*` + `use string::*` (`format`, `to_bytes`).

## Virtual modules

Auto-imported every file: `prelude`, `prelude::ops`, `prelude::test`, `prelude::math`.

Explicit `use`: `string`, `ffi` (+ `ffi::types`), `io` (+ `io::fs`, `io::net::*`), `thread`, `env`, and feature-gated `time`, `crypto`, `regex`. Host APIs via `HostInvoke` — no new opcodes for these.

Cargo features `crypto`, `time`, `regex`, `tls` default-on; embedders may `default-features = false`.

Coroutines: `async fn`, `yield`, `resume` / `resume h with v`, `yield from`, `done(h)`; `coroutine<Y, S>`; resume-after-done → default value.

FFI: `extern { … }` or `use ffi::*` (`dload`/`declare`/`invoke`); tags `ffi::types::*`; `resolve_library` searches entry dir, `coil.toml` `[ffi] search_paths`, system paths.

Tests: `test("desc") { … }` or `#[test]` — Result mode; no `main` in same file. `assert` auto-imported; `panic` aborts VM.

Details: `docs/references/modules.md`, `docs/references/not-builtins.md`.

## Durable invariants

- **Append-only opcodes** in `common/src/opcode.rs`. New variants at end only (`#[repr(u8)]`). Then bump `ARCHIVE_VERSION`, `promise!` ceiling in `machine/src/vm.rs`, and `instruction_from_u8_covers_last_appended_variant`.
- **`STORE`**: pops TOS into slot(s); `cursor = max(cursor, slot + 1)`. Match bindings skip store (`UNPACK` / `JUMP_IF_MATCH`). `StorePop` is deprecated alias — compiler never emits. Packed multi-slot `LOAD`/`STORE`: `[31:24]=n` (1..=3), three slot bytes; `n==0` → wide single slot in low 24 bits.
- **Stack IL**: symbolic labels until `finalize_bytecode` → per-body opts + **single** `il::lower` after concat (not per-function lower). Nested fused returns must `capture_nested_return`. Full pipeline: `docs/internals/pipeline.md`.
- **BlockBuilder**: `IlBuilder` labels only; no absolute PC patching in emitters.
- **Match**: threaded layout; `JumpIfMatch` tag + pool index; nested record patterns use slot `UnpackAt`.
- **Fields**: enum records `LoadField` (index); dicts/classes `GetField`/`SetField` (string keys); `SetField` in-place.
- **Type aliases**: scoped; same-frame duplicates are errors; inner may shadow outer.

Packed LA (`dot`/`matmul`/`Matrix`) → `HostInvoke` in `machine/src/packed_la.rs` (no LA opcodes).

## Known limitations

- GC mark walks intrusive list; `Gc::payload_mut` clippy exception — gate: `cargo check --workspace` (not clippy).
- `codegen_var_types` side table still used for some Identifier paths.
- Path completeness: concrete non-unit returns need all paths to exit (`E0111`). `never` from exit/infinite loops; unreachable `E0118`; defer in infinite loop `E0123`.
- Unit/open-var fall-through may emit `CONST 0; RETURN` (Result-mode Ok-wraps unit only).

## Cloud / local commands

Pre-installed in cloud image: `poop`, `valgrind`, `heaptrack`, `hyperfine`, `lua` (see `.cursor/Dockerfile`).

```bash
cargo check --workspace
cargo test --workspace          # pipeline + perf_metrics ~25–30s each
cargo build --release --workspace
./scripts/poop_baseline.sh
ulimit -v 65536 && cargo run -- test
valgrind --leak-check=full --error-exitcode=1 ./target/debug/coil examples/fib.hy
```

Use `--release` for benchmarks/`poop`; debug builds print heap alloc traces.

## Dev gotchas

- Rust edition **2024** (≥ 1.85); no `rust-toolchain.toml`.
- System libffi for FFI; libpcre2 for `regex`. `examples/strlen.hy` CLI may segfault; suite FFI paths pass.
- `byte` is 0..=255; literals coerce under `byte`/`[byte]`. Buffers are `[byte]`, not `string`.

Getting started: `docs/manual/getting-started.md`.
