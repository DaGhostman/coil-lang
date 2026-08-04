# SIMD (`coil-simd`)

Stable-Rust helpers over [`std::arch`](https://doc.rust-lang.org/stable/std/arch/)
for dense numeric and byte kernels. Coil does **not** use nightly
`portable_simd`.

## Why a workspace crate

HostInvoke packed linear algebra (`machine/src/packed_la.rs`) already batches
matrix work off the opcode dispatcher. Contiguous `f64` / `i64` loops are the
right place for SIMD — not new bytecode ops (see AGENTS.md: prefer kernel
tuning over benchmark-shaped opcodes).

`coil-simd` keeps the unsafe ISA details out of the VM and exposes a small
safe API:

| API | Role |
|-----|------|
| `detect()` / `SimdLevel` | Cached runtime probe (`SSE2` / `AVX2` / `AVX-512` / `NEON` / scalar) |
| `dot_f64` / `dot_i64` | Dot products |
| `matmul_f64` / `matmul_i64` | Row-major GEMM (`C = A·B`) |
| `zip_{add,sub,neg}_{f64,i64}` | Element-wise matrix zip / negate |
| `bytes::eq` / `bytes::xor` | Byte equality and XOR |

## Dispatch

- **x86_64:** SSE2 is ABI baseline; AVX2 (+FMA when present) and AVX-512
  (`avx512f` + `avx512dq` + `avx512bw`) are selected at runtime via
  `is_x86_feature_detected!` — no `RUSTFLAGS=-C target-cpu=…` required for
  correctness. Partial AVX-512 (missing DQ/BW) falls back to AVX2.
- **aarch64:** NEON.
- Tiny inputs fall back to scalar to avoid call overhead.

`i64` multiply stays scalar on SSE2/AVX2 (no general 64-bit `mullo`); AVX-512DQ
enables vectorized `i64` dot / matmul. Zip add/sub/neg vectorize on all levels.

## Callers today

- `machine::packed_la` — `packed_dot` / `packed_matmul` / `packed_matrix_zip` /
  `packed_matrix_neg` pack nested `Value` aggregates, call `coil-simd`, then
  rebuild matrices.
- String intern table lookup (`Heap` hash map) uses `bytes::eq` for key
  compares.

## Extending

Add ISA-specific modules under `coil-simd/src/{x86_64,aarch64}/`, keep a scalar
reference in `scalar.rs`, and dispatch from `kernels.rs` / `bytes.rs`. Prefer
runtime feature checks over forcing wider ISAs at compile time.
