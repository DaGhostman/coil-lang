# Known limitations and workarounds

Actionable gaps in the compiler, VM, and language surface. For opcode/archive rules see [AGENTS.md](../../AGENTS.md); for typechecker error codes see [error-codes.md](../references/error-codes.md).

**Design rule:** prefer **method-based APIs** (`impl` methods on classes) over free functions for type-tied operations — stdlib, new language features, and codegen fixes should default to methods. Virtual-module host primitives (`io::read`) remain free functions.

**Priority key:** blocking = correctness crash or major feature blocked; high = reliability or significant DX gap; medium = partial support; low = deferred polish.

## Blocking

| Issue | Detail | Workaround |
|-------|--------|------------|
| Result-mode string concat SEGV | `int_to_dec` + string concat in Result-mode can crash at runtime | HTTP client uses lookup tables for `Content-Length`; avoid `host?q=` paths (use `/?q=`) — see [http-client.md](../manual/http-client.md) |
| Generic enum return (free fn) | Free `fn f<T>(T) -> Option<T>` can corrupt payloads | Use methods on classes instead — [collections-vm-split.md](collections-vm-split.md) |
| Array element types | Parser accepts `[ident]` / `[ident; N]` only — not `[Option<T>]` | Use a type alias or class wrapper |
| `strlen.hy` segfault | CLI path for `examples/strlen.hy` can segfault (pipeline golden passes) | See [test-health-report.md](test-health-report.md) |

## Codegen / compiler (high)

| Issue | Detail |
|-------|--------|
| `codegen_var_types` side table | Some `Identifier` paths still use a flat name→type map instead of per-binding types |
| `call_arg_is_pure` | Pure-arg reordering excludes identifiers — reordering can corrupt frames |
| Partial application cap | Fixed at 32 parameters (`filled_mask` is `u32`) |
| Named arguments | Not supported on some builtins (`len`, etc.) |
| Matmul >255 dims | LA metadata limit; scalar unroll fallback; nested `[[int; N]; M]` types may not parse |
| Match inner coverage | Exhaustiveness may not track all nested inner-pattern distinctions |

## Typechecker (medium)

| Issue | Detail |
|-------|--------|
| `operator` on aggregates | Not supported on `Matrix` — use `matmul`, element-wise ops |
| Scrutinee pinning | Result scrutinees use heuristics when not yet pinned |
| Pattern range | Pattern AST has no dedicated range node for diagnostics |
| `async fn` attributes | User-defined `attr` decorators not supported on `async fn` |
| Stack bounds | Conservative analysis; dynamic recursion needs `#[max_depth(N)]` — [stack-bounds.md](stack-bounds.md) |

## VM / runtime (medium)

| Issue | Detail |
|-------|--------|
| Coroutine resume after done | Resuming a completed coroutine returns default sentinel; no error protocol yet |
| GC coroutine stacks | Conservatively roots all suspended coroutine stacks |
| Debug line table | Many unknown locations; fused insn keeps first span; no panic backtrace yet — [debug-info.md](debug-info.md) |
| Functional `List` recursion | Deep recursion can exhaust VM stack |

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

## LSP (medium)

Navigation is conservative — unresolved imports and cross-project refs incomplete. See [lsp.md](lsp.md).

## Tracking

Most items have no inline `TODO`/`FIXME` — knowledge lives here and in [.cursor/skills/coil-contributor/reference.md](../../.cursor/skills/coil-contributor/reference.md). Update this file when closing a limitation.
