# coil contributor reference

## Key files

| File | Role |
|------|------|
| `common/src/opcode.rs` | `Instruction` enum — append-only |
| `common/src/archive.rs` | `ARCHIVE_VERSION`, archive envelope |
| `machine/src/vm.rs` | Main dispatch loop, `promise!` opcode ceiling |
| `compiler/src/lib.rs` | Codegen driver, `finalize_bytecode` |
| `compiler/src/il/` | IL ops, lower, fuse-select, opts |
| `compiler/src/typechecking/` | HM Algorithm W |
| `parser/src/` | Pratt parser |
| `machine/src/packed_la.rs` | LA ops via HostInvoke (no LA opcodes) |

## Codegen notes

- `BlockBuilder`: thin wrapper over `IlBuilder` labels; no absolute PC patching in emitters.
- `ConstEnv`: scalar const folding; constant branch/loop elimination; loop unroll ≤8.
- Tiny direct-call inlining; one-level self-`CALL` peel; `TailCall` for eligible recursion.
- Match: threaded layout, `JumpIfMatch`, nested records use `UnpackAt` (slot-based).
- Enum fields: `LoadField` (index); dict/class fields: `GetField`/`SetField` (string keys).

## VM / values

- Static slots: `LoadStatic`/`StoreStatic`; count in archive envelope.
- Coroutines: `MakeCoro`, `ResumeCoro`, `YieldCoro`, `YieldFromCoro`, `DoneCoro`.
- `panic` aborts VM; test harness treats as failure.
- GC: addr index O(1) lookup; mark walks intrusive list.

## Typechecker limitations (known)

- `codegen_var_types` side table still used for some Identifier paths.
- Path completeness (`E0111`) for concrete non-unit returns on named fns.
- Unreachable code `E0118`; defer in infinite loop `E0123`.

## ARCHIVE_VERSION bump triggers

- New or reordered opcode discriminants
- Incompatible bytecode encoding
- Tag layout changes
- Archive envelope field changes

Current version: check `common/src/archive.rs` (documented in AGENTS.md).

## Perf philosophy

Prefer over new opcodes / IL opts:
- Allocation reduction in hot paths
- Bounds-check elimination
- Hot-loop tuning in VM
- `promise!` for release assertions

Soft baseline: `./scripts/poop_baseline.sh` on `examples/fib_bench.hy`.

## Sub-agent guidance

For large tasks, scope sub-agents to disjoint files/modules with explicit boundaries. Avoid over-parallelizing conflicting edits.

## Docs trees

| Tree | When to update |
|------|----------------|
| `docs/manual/` | Tutorials, getting started, examples catalog |
| `docs/references/` | Syntax, per-API builtin pages, error codes |
| `docs/internals/` | Pipeline, opcodes, debugger (contributor-facing) |

New stable diagnostics: add to `docs/references/error-codes.md`.
