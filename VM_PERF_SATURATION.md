# VM Performance Saturation Pass

`ARCHIVE_VERSION` was bumped to **13** for fused opcodes `BinSlotImmJmpf` and
`LogNotJmpf`. Current tree is **15** (also includes `DoneCoro`, `ArrayPush` /
`ArrayLen`, and later fusion work).

## Baseline corpus

| Workload | Class | Exercises | Golden output |
|----------|-------|-----------|---------------|
| `examples/fib_bench.0s` | CPU / calls | recursion, `BinReturn`, `BinSlotSlot` | `2178309` |
| `examples/perf/numeric.0s` | CPU / loop | `BinSlotImmJmpf`, `BinSlotImm`, `StorePop` | `1999000` |
| `examples/perf/operators_loop.0s` | CPU / ops | `Pow`, `BITAND`, `BITOR`, `LogNotJmpf` | `149912` |
| `examples/perf/array_mut.0s` | heap / agg | `StoreIndex`, compound update | `2000` |
| `examples/perf/dict_hot.0s` | heap / records | `GetField`, `SetField` | `6000` |
| `examples/perf/coro_ping.0s` | coroutines | `MakeCoro`, `ResumeCoro`, `YieldCoro` | `124750` |
| `examples/perf/match_sum.0s` | match (bytecode only) | `MakeEnum`, `JumpIfMatch` | *(runtime golden deferred — see below)* |
| `examples/operators.0s` | smoke | operator surface | `801125428falsetrue3` |

Run the full harness (64MB limit, fresh `out.c0s` per example):

```bash
./scripts/vm_bench.sh
```

CPU subset for `poop` timing is defined in `CPU_BENCH` inside that script.

Dispatch-count regressions live in `compiler/tests/perf_metrics.rs` (requires
`machine` dev-dep with `vm_profile`).

## Implemented optimizations

### Bytecode (peephole)

| Superinstruction | Convoy | Measured benefit |
|------------------|--------|------------------|
| `BinSlotImmJmpf` | `LOAD; CONST; <cmp>; JMPF` | Removes 3 dispatches per loop test on `numeric.0s` |
| `LogNotJmpf` | `LogNot; JMPF` | Fuses control flow in `operators_loop.0s` |
| `Pow` / `BITAND` / `BITOR` in `BinSlot*` / `BinReturn` | `LOAD; LOAD; <op>` | Enables fusion for operator-heavy loops |

Peephole also extends `is_bin_op` / `is_int_bin_op` for `Pow`, `BITAND`, `BITOR`.

### VM

| Change | Rationale |
|--------|-----------|
| `Heap::addr_index` (`HashMap<u64, Object>`) | Replaces O(n) `find_object_by_addr` / `contains_addr` list walks |
| `trace` uses `HashSet` roots | Avoids O(n×m) `Vec::contains` during GC mark |
| In-place `BinSlotSlot` handler | Drops double push/pop temporaries for fused binops |

## Residual convoys (not fused — saturation rationale)

| Pattern | Why left unfused |
|---------|------------------|
| `FORMAT; PRINT` | I/O-bound; no CPU win in hot loops |
| `StorePop; LOAD; Index; StoreIndex` chains | Long, shape-specific; index `++`/`--` already uses dedicated `INC`/`DEC` for locals |
| `CALL; …` / prologue sequences | Frame setup cost dominates; fusion would be fragile across functions |
| Second-order peephole (`BinSlotImm; JMPF` after pass 1) | Subsumed by direct 4-instruction `BinSlotImmJmpf` rule |
| `PowF` in `BinSlotImm` | Pool-backed float constants block `BinSlotImm` eligibility |

## Rejected / deferred VM ideas

| Idea | Reason |
|------|--------|
| GC roots via `Stack::as_slice()` only | Risks missing locals when `cursor` shrinks below live slots; kept full `buffer()` scan |
| Direct threading / computed goto | No measured win vs. fused opcode reduction; high portability cost |
| Tail-call elimination on `fib` | Calls are not tail positions (adds two recursive results) |
| `FORMAT; PRINT` fusion | Dominated by I/O latency |

## Known limitations

- **`let x = match { … }` binding** — fixed in P0 (Match no longer emits `RETURN` at `end_label`; arm value stays on the stack for `StorePop`). `match_sum` end-to-end output is valid when used with `return match` (as in the example).

## Verification

```bash
cargo test --workspace
cargo test -p compiler --test perf_metrics
cargo test -p compiler --test pipeline example_perf
cargo build --release && rm -f out.c0s && poop -d 6000 ./target/release/zero-script examples/fib_bench.0s
```

All workspace tests should pass; fib dispatch stays below 18M (`perf_fib_dispatch_regression`).
