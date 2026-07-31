# coil — AGENTS

## Learned User Preferences

- Run tests with a 64MB memory limit to catch leaks; exceeding it likely indicates a memory leak.
- Use `poop` for CPU performance baselines: `./scripts/poop_baseline.sh` (or `cargo build --release && poop -d 6000 "./target/release/coil examples/fib_bench.hy" && rm out.hyc`). Soft check before/after opt changes — not a hard CI fail.
- Use sub-agents for large tasks (scoped to disjoint files/modules); avoid over-parallelizing — give specific, scoped instructions so each agent can finish independently.
- For VM/runtime performance work, prefer alloc reduction, hot-loop tuning, bounds-check elimination, and `promise!` over adding opcodes or compile-time IL optimizations.
- Draft implementation plans before large language-feature work; do not edit attached plan files during implementation.
- New language features require full HM typechecker integration and updated user-facing docs in `docs/`.
- Include minimal runnable examples with clear expected output before committing feature work.
- Stage and commit only related changes; exclude unrelated modified files from commits.
- User is flexible on syntax when designing new language constructs.
- Prefer compiler-provided builtins over userland definitions for core type machinery (virtual `prelude` / `ffi` modules, not user-declared).

## Learned Workspace Facts

- coil is a statically typed language with HM inference, compiled to stack bytecode and run on a custom VM; sources use `.hy`, build with `cargo build --workspace`, run with `cargo run -- examples/foo.hy`.
- User-facing documentation lives in `docs/manual/` (tutorials, examples), `docs/references/` (language + API lookup), and `docs/internals/` (pipeline/VM).
- Single-pass stack codegen in `compiler/src/lib.rs` is the only compilation path; the register-VM migration was removed.
- Coroutines: `async fn`, `yield`, `resume`, `resume h with v`, `let x = yield e`, `yield from` via `MakeCoro`/`ResumeCoro`/`YieldCoro`/`YieldFromCoro`; `coroutine<Y, S>` types; resume-after-done returns `Value::default()`; `done(h)` → bool via `DoneCoro`.
- Virtual modules: `prelude` / `prelude::ops` (auto-imported); `ffi` / `ffi::types`, `io`, `thread`, `regex`, `crypto`, `time` (explicit `use`). Host natives go through `HostInvoke` — no new opcodes for those modules.
- FFI: `use ffi::*` for `dload`/`declare`/`invoke` (ordinary identifiers); tags are `ffi::types::{Int,Ptr,…}` (no global `FFIType`). Returns are `prelude::Result`. Compile-time `extern` blocks need no `use ffi`. `resolve_library` searches entry-script `base_dir`, `coil.toml` `[ffi] search_paths`, then system paths.
- Virtual `io`: opaque `Stream`/`IoError`; `stdin`/`stdout`/`stderr`/`open`/`read`/`write`/`close` plus sync adapters and `from_bytes`/`to_bytes`; nested `io::net::tcp` / `io::net::udp` / `io::net::tls::{client,server}` (`enable`/`disable` on a TCP `Stream`, feature `tls`). L0 is non-blocking; buffers are `[byte]`. Stream timeouts via `set_read_timeout`/`set_write_timeout` (ms≤0 clears); TCP helpers: `connect_timeout`, `accept_wait_timeout`, `peer_addr`/`local_addr`, `set_nodelay`, `shutdown`. `IoError` tags include `TimedOut`/`Truncated`/`Certificate`/`Handshake`. TLS client opts `{verify, ca_pem: Option<string>, ca_path: Option<string>, timeout_ms}` — extras **append** to webpki; server `{cert_pem, key_pem, timeout_ms, client_ca_pem}` (empty / ≤0 = defaults).
- Virtual `thread` (`use thread::*`): `spawn`/`join`/`detach`, channels, mutex/rwlock helpers; one `Machine` per OS worker; `Pipeline::wire_thread_program` shares bytecode. Nullary fns seal as `unit -> R` for `spawn(f)`.
- Virtual `regex` (`use regex::*`): PCRE2 via HostInvoke; opaque `Regex`; flags `i`/`m`/`s`/`x`/`u`.
- Cargo features `crypto`, `time`, `regex`, and `tls` (default-on) gate those virtual modules; embedders may use `default-features = false`.
- `byte` is `Ty::Con("byte")` (0..=255); integer literals coerce under expected `byte` / `[byte]`. Array annotations `[T]` / `[T; N]` preserve element type in the AST.
- `ARCHIVE_VERSION` is **34** (`common/src/archive.rs`); bump on incompatible bytecode, tag, or opcode changes.
- Packed LA (`dot` / `matmul` / `Matrix` ops) lowers to HostInvoke natives in `machine/src/packed_la.rs` — no LA opcodes.
- Primitive casts (`CastIntToFloat` … `CastBoolToInt`) are the last `Instruction` variants.
- Static slots: `LoadStatic`/`StoreStatic`; `static_slot_count` is in the archive envelope. Archives also carry `source_files` + `debug_locs` (one per bytecode slot).
- Codegen: scalar const folding (`ConstEnv`), constant branch/loop elimination, loop unroll (≤8 trips, no `break`/`continue`), optional tiny direct-call inlining, `TailCall` for eligible self-recursive returns.
- `prelude::test::assert` is auto-imported (`Result<(), string>`); `panic` aborts the VM (CLI/`coil test` treat it as failure).
- `examples/fib.hy` is the smoke Fibonacci (`fib(10)` → `55`); `examples/fib_bench.hy` is the primary `poop` / `vm_bench.sh` / `perf_metrics` entry.
- CLI caches bytecode in `out.hyc`; delete it before re-running after source edits.
- `coil test [path] [--fail-fast]` discovers `**/*.hy` (default `./tests`). Top-level `test("desc") { … }` cases replace per-file `main`; legacy `fn main()` files still count as one case.

## Durable invariants

- **Append-only opcodes.** Never insert into the middle of `Instruction`; append at the end so `#[repr(u8)]` discriminants stay stable. When appending, bump `ARCHIVE_VERSION`, the release `promise!(*bc as u8 <= Instruction::…)` ceiling in `machine/src/vm.rs`, and `instruction_from_u8_covers_last_appended_variant`.
- **`STORE` vs `StorePop`.** `STORE` pops TOS into the slot and preserves `cursor = max(cursor, slot + 1)`. Match-arm bindings need no store opcode (value already in the slot via `UNPACK` / `JUMP_IF_MATCH`). `StorePop` is a deprecated discriminant alias with the same VM handler; the compiler never emits it. Packed multi-slot form (same discriminants): `[31:24]=n` (`1..=3`), `[23:16]=s2`, `[15:8]=s1`, `[7:0]=s0`; `n==0` is a wide single slot in low 24 bits. `LOAD` pushes s0…s{n-1}; `STORE` pops TOS into each listed slot in order.
- **Stack IL / lower.** Codegen emits `compiler/src/il` with symbolic labels; `finalize_bytecode` rebuilds an owning [`IlModule`] from the flat emit stream (`IlFunc` spans + entry map): flat emit → split → per-body `jump_thread`/`dead_block`/`stack_dce`/`mem_fwd`/`dead_store`/`algebraic`/`licm`/`return_convoy`/`bin_join_convoy` + CFG GVN (LoadField CSE, length-2 join tails) → concat → `multi_op_join_convoy` on the full buffer → **single** `il::lower` (fuse-select + PC assign). Prologue/glue stay outside bodies; empty `funcs` → whole-buffer opts. Hot-path ops are typed `IlOp` variants lifted on absorb (`Load`/`Const`/`Bin`/`BinSlot*`/`*Return`/…); long-tail typed forms include `Index`/`MakeTuple`/`MakeArray`/`Pop`/`MakeEnum` via `push_index`/`push_make_*`/`push_pop`, plus `BoxValue`/`UnboxValue`/`LoadField`, and residual-lifted `HostInvoke`/`Print`/`GetField`/`SetField`/`ConstPool` (`push_host_invoke` / `push_print` / `push_get_field` / `push_set_field` / `push_const_pool`). Residual `IlOp::Byte` is rare (FORMAT/FFI/…). Hot emit prefers typed `EmitBuf`/`CodeBuf` helpers. `return_convoy` sinks identical immediate join producers across a return-label cluster (`Label`+…+`RETURN`) into fused `*Return` (preds: `JMP`, or `JMPF`/`JMPT` with value-under-cond, or all-`JumpIfMatch`; not mixed across those classes; cond/match/jump-only preds require Known join SP). `bin_join_convoy` sinks identical plain binop tails to `BinReturn`, or identical `BinSlotImm`/`BinSlotSlot` to one copy before `RETURN` (same pred/SP gates). `multi_op_join_convoy` sinks identical length-2..=4 compute suffixes at return joins and unambiguous non-return continuations when SP-in agrees (`il::sp`); accepts `JMP`/`JMPF`/`JMPT`/`JumpIfMatch` preds (jump-pred template when no fall-through); refuse Unknown heights / jump-only joins. Label binds and absolute JMP targets inside a fuse window are barriers; `*Return` also refuses a stacked-value join on window[0]. Entry labels remap `functions` after opts; CALL/CodePtr absorb as `IlOp::Entry`. `IlFunc` metadata (name, entry, code span) is recorded on `CodeBuf`. Tiny inlining candidacy uses IL emitting ops (`code_slice_ops` / `is_tiny_inline_il`), may expand a sole `ConstReturnImm`/`LoadReturnSlot`/`BinReturn`, accepts pure micro-bodies (≤3 Load/Const/Bin/BinSlot* + Return), and remaps `BinSlotImm`/`BinSlotSlot` slots via caller temps. Per-func fuse-select (lower each `IlFunc` then stitch) was soft-poop'd / measured with no clear ≥3% fib_bench win — keep **single** lower after concat. Nested VM returns from fused `ConstReturnImm` / `BinReturn` / `LoadReturnSlot` must call `capture_nested_return`. Soft CPU baseline: `./scripts/poop_baseline.sh`.
- **BlockBuilder.** Thin wrapper over `IlBuilder` labels (`emit_jump_to` / `bind_label`); no absolute PC patching. Direct emitters must extend `self.bytecode` (`CodeBuf`) in source order.
- **Match.** Threaded-code layout with per-outer-tag groups; multi-arm groups emit an inner `JUMP_IF_MATCH` test chain. `JumpIfMatch`: `operands[31:16]` = tag, `[15:0]` = pool index → absolute target. Nested record patterns use `UnpackAt` (slot-based), not top-of-stack `Unpack`; nested multi-field records unpack into a scratch region past the outer field slots (LOAD+`STORE` then UnpackAt) so siblings are not clobbered.
- **Access / dicts.** Enum record fields use `LoadField` (index); anonymous records / class fields use `GetField`/`SetField` (string-keyed). `SetField` mutates in place.
- **Type aliases** are scoped (push/pop with function/block frames); same-frame duplicates are diagnostics, inner may shadow outer.

## Known limitations

- `Heap::find_object_by_addr` / VM lookup use the addr index (O(1)). GC mark still walks the intrusive list; `Gc::payload_mut` remains a clippy lint exception (`cargo check` is the gate).
- `codegen_var_types` remains for match/method/free-fn Identifier codegen. Lambdas consume arg NodeIds via `assign_fn_arg_node_ids` (no Identifier span prefer). Free-fn/method assign deferred — enabling it broke Hash derive and constraint-kind; side-table retirement deferred.
- Path completeness (HM): named functions (not async, not trait sig stubs) must `always_exits` when the return is a concrete non-unit type — missing paths are `E0111` (`ReturnMismatch`). `return`/`raise`/`panic` and proven-infinite `while`/`for` (bool const-fold) type as `never` and join via absorbing Never. Unreachable code after an exit is `E0118` (warning); `defer` dominated by / inside an infinite loop is `E0123` (warning). Bare `return;` returns unit. Unit / open-var returns may still fall through: codegen emits defers + `CONST 0; RETURN` (Result-mode Ok-wraps unit Ok only) — no invent for `Option`/`int`/`string`/ADTs.
- Soft CPU baseline: `./scripts/poop_baseline.sh`.
- `cargo clippy` fails on a pre-existing `#[deny(clippy::mut_from_ref)]` in `Gc::payload_mut`; use `cargo check --workspace` as the lint gate.

## Cursor Cloud specific instructions

Cloud agents use `.cursor/environment.json` + `.cursor/Dockerfile`. Pre-installed tools:

| Tool | Purpose |
|------|---------|
| `poop` | Instruction-count / HW-counter baselines (`perf_event_open`) |
| `valgrind` | Memcheck leak runs, Callgrind profiles (`callgrind_annotate`) |
| `heaptrack` | Heap allocation tracing (`heaptrack ./target/release/coil …`) |
| `hyperfine` | Wall-clock timing comparisons |
| `lua` | Baseline comparison in `scripts/vm_bench.sh` |

Common commands (delete `out.hyc` after source edits):

```bash
cargo test --workspace
cargo build --release --workspace
./scripts/poop_baseline.sh
./scripts/vm_bench.sh
ulimit -v 65536 && ./target/debug/coil test
valgrind --leak-check=full --error-exitcode=1 ./target/debug/coil examples/fib.hy
valgrind --tool=callgrind --callgrind-out-file=callgrind.out ./target/release/coil examples/fib_bench.hy
```

Use `--release` for benchmarks and `poop`; use debug builds for valgrind memcheck (heap alloc traces are noisy in debug).

## Dev gotchas

Standard commands: [`docs/manual/getting-started.md`](docs/manual/getting-started.md).

- Crates use `edition = "2024"` (Rust ≥ 1.85). No `rust-toolchain.toml`; if cargo complains about edition 2024, run `rustup default stable`.
- Build: `cargo build --workspace` / `cargo build --release --workspace`. Test: `cargo test --workspace` (pipeline + `perf_metrics` each take ~25–30s).
- Debug builds print heap `alloc`/`free` traces to stdout; use `--release` for clean program output.
- FFI needs system libffi; regex needs libpcre2 (`libpcre2-dev` / Arch `pcre2`). `examples/strlen.hy` CLI may segfault — suite FFI paths still pass.
- Delete `out.hyc` when switching examples or after editing source, or you run stale bytecode.
