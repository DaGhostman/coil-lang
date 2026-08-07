# Known limitations and workarounds

Actionable gaps in the compiler, VM, and language surface. For opcode/archive rules see [AGENTS.md](../../AGENTS.md); for typechecker error codes see [error-codes.md](../references/error-codes.md).

**Design rule:** prefer **method-based APIs** (`impl` methods on classes) over free functions for type-tied operations — stdlib, new language features, and codegen fixes should default to methods. Virtual-module host primitives (`io::read`) remain free functions.

**Priority key:** blocking = correctness crash or major feature blocked; high = reliability or significant DX gap; medium = partial support; low = deferred polish.

## Parser / surface (low–medium)

| Issue | Detail |
|-------|--------|
| `coil.toml` `preludes` / `strict` | Not implemented — [project-config.md](../references/project-config.md) |
| `import` keyword | Not implemented (use `use`) |
| `case` as `match` alias | Not in grammar |
| Range `collect` | `collect(0..5)` deferred; non-numeric `Ord` ranges not iterable |
| Duplicate record fields | Not rejected at parse time (typechecker reports) |

## Userland footguns

| Issue | Detail |
|-------|--------|
| `Option` field moves | `match` moves fields out — copy links before nested `match` on the same field |
| Cross-module user classes | Userland class types not importable across modules (e.g. `HashSet` wrapper) |
| User trait calls | Dictionary passing; only ground builtin bounds get direct monomorphized opcodes |

## IL optimizations (low)

GVN has no SSA slot rename; effectful ops are barriers. Per-function `multi_op_join_convoy` can mis-sink on JMPF diamonds — whole-buffer pass required. Fuse-select intentionally conservative during JMP migration.

## Test / CI reliability (high–medium)

| Issue | Detail |
|-------|--------|
| Parallel `thread_join` / mutex hang | `example_thread_join_prints_42` and `example_thread_mutex_prints_2` pass alone (~tens of ms) but can hang indefinitely — reproduced with a plain default-parallelism `cargo test -p compiler --test pipeline` (no extra load needed). Workaround: re-run, or `cargo test -p compiler --test pipeline -- --test-threads=1` / run the thread tests alone. |
| Parallel heap corruption | Occasional `corrupted double-linked list` / SIGABRT when many pipeline VMs run concurrently. Usually clears on retry; serialize pipeline if it blocks a gate. |
| Criterion vs `--all-targets` | Prefer `cargo test --workspace --lib --tests --bins` (CI and agent gate). `--all-targets` also builds `[[bench]]` targets; Criterion treats argv after `--` as its own CLI and aborts the suite. |
| Optional feature suites | Full `cargo test` under `--no-default-features` (or a single of `crypto`/`time`/`regex`/`tls`) fails or hangs on tests that need the other libs. CI compile-gates those with `cargo check --workspace --lib --tests --bins …`; keep full tests on the default stack (plus `dissect` / `debugger`). |
| `ulimit` leak-smoke wraps `cargo run` | `ulimit -v 65536 && cargo run --bin coil -- test` OOMs immediately (`memory allocation of N bytes failed`) — it's `cargo`'s own build-check machinery that exceeds 64MB, not the `coil test` binary. The compiled binary itself passes cleanly under the same cap (305/305). Invoke the built binary directly: `cargo build --bin coil && (ulimit -v 65536; ./target/debug/coil test)`. |

## Tracking

Most items have no inline `TODO`/`FIXME` — knowledge lives here and in [.cursor/skills/coil-contributor/reference.md](../../.cursor/skills/coil-contributor/reference.md). Update this file when closing a limitation.
