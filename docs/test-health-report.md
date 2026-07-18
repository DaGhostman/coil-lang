# Test health report (2026-07-18)

Investigation of `main` after PRs #8–#17. This document records what was
broken, what looked flaky, incomplete implementations, and fixes applied.

## Broken tests

| Test | Symptom | Origin |
|------|---------|--------|
| `example_derive_show_eq_prints_expected` (`compiler/tests/pipeline.rs`) | Compile fails with E0119: `Ord` for `Color` requires `Lt`/`Le`/`Gt`/`Ge`; unknown methods `lt`/`le`/`gt`/`ge` on `Ord` | PR **#15** (`b34c15d`) header `derive Ord` expanded to a single `impl Ord` carrying comparison methods. PR **#14** (`84fd341`) had already split comparisons into `Lt`/`Le`/`Gt`/`Ge` with **empty** `Ord` as a convenience supertrait (see `compiler/src/typechecking/generics.rs`). |

**Fix:** `synth_ord_enum` / `synth_ord_class` in `compiler/src/derive.rs` now emit five synthetic impls (`Lt`, `Le`, `Gt`, `Ge`, empty `Ord`). Regression: `derive_ord_record_payload_lexicographic_compare`.

## Slow tests (not flaky)

| Path | Issue | Fix |
|------|-------|-----|
| `examples/fib.0s` used `fib(32)` | Millions of recursive calls; debug heap alloc traces made `example_fib_still_works` and shared suite wall time drag | Smoke example is now `fib(10)` → `55`; bench lives in `examples/fib_bench.0s` (`fib(32)` → `2178309`) for `poop` / `vm_bench.sh` / `perf_fib_dispatch_regression` |

## Flaky tests

**None observed.** Namespace suite (`compiler/tests/namespace.rs`) passed repeatedly under `--test-threads=16`. Residual risks (not failing today):

- Process-wide `CWD_LOCK` + `chdir` for `zero.toml` discovery
- Shared `examples/libsum.so` build among FFI tests (must not truncate with `File::create`)

## Incomplete / false-green patterns

| Pattern | Risk | Mitigation |
|---------|------|------------|
| FFI tests `eprintln!("skipping…"); return;` when `cc` / `.so` / `libc` missing | CI can go green without exercising FFI | Soft-skip **panics when `CI` is set**; GitHub Actions installs `libffi-dev` + `build-essential` |
| No `.github/workflows` before this work | Regressions like #15 Ord derive landed unnoticed | Added `.github/workflows/ci.yml` |
| CLI `out.c0s` cache | Stale bytecode on manual runs (not pipeline goldens) | Documented; CI does not rely on the cache for goldens |

## Coverage follow-ups applied

- In-tree `proptest` property tests (parser no-panic; small-program compile no-panic)
- Extra `./tests/*.0s` exercised by `zero-script test` (derive Ord, assert edges)
- GitHub Actions: `cargo test --workspace` + `cargo run -- test`

## Out of scope (still known)

- Documented `examples/strlen.0s` CLI segfault path (pipeline golden passes)
- Commented-out allocator unit tests in `machine/src/memory/allocator.rs`
- Overnight `cargo-fuzz` / libFuzzer corpus
