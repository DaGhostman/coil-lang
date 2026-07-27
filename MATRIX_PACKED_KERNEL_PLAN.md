# Approach A — Packed Linear-Algebra Kernels

**Status:** implemented (via `HostInvoke`, not new opcodes)  
**Depends on:** NT-7 named helpers + `Matrix`/`Mul` (already shipped)  
**Goal:** Replace compile-time scalar unrolls for `dot` / `matmul` /
`Matrix` `*` `+` `-` with **one packed kernel per op family**, whose
handler extracts nested aggregates into contiguous Rust buffers, runs a
tight loop, then rebuilds the nested result. Same language surface and
observable results; enables future SIMD/FMA inside the kernel without
growing the `Instruction` enum.

### Why not dedicated opcodes?

Appending `PackedDot` / `PackedMatMul` / `PackedMatrixZip` /
`PackedMatrixNeg` to `Instruction` grew the `execute` match and
regressed fib branch prediction on some CPUs (~+25% wall / ~+40%
branch misses vs `main` with nearly identical instruction counts).
Kernels now live in `machine/src/packed_la.rs` and are registered as
ordinary host natives (`packed_dot`, …). Codegen emits
`CONST id` + args + `CONST meta` + `MakeTuple` + `HostInvoke`.
`Instruction` stays identical to `main` (`TailCall` last).
`ARCHIVE_VERSION` is **29** (invalidate archives that encoded the
short-lived Packed\* opcodes).

### Release LTO / `execute` outlining

`Machine::execute` must stay `#[inline(never)]`. With fat LTO,
`#[inline(always)]` pasted the giant dispatch `match` into
`run_with_pool` / callers and reshaped branch layout enough to blow
mispredict rates on some CPUs while keeping dynamic instruction counts
identical. Outlining matches non-LTO `machine` codegen (parity with
`main`). Do not “optimize” this back to always-inline without
re-checking `poop` on `fib_bench.hy`.

---

## 1. Why Approach A

Today `emit_linear_algebra` can unroll every cell into `LOAD` / `CONST` /
`Index` / `MUL` / `ADD` / `MakeArray`. That is correct but:

- Dispatch cost scales with `m·k·n` (matmul) or `length` (dot).
- Nested `ObjArray`/`ObjTuple` is not contiguous, so SIMD cannot run
  over live heap layout.
- Future “pack multiple multiplications into one pass” (tiling, FMA,
  autovectorization) needs a **single kernel entry** that owns both
  operands’ data for the whole op.

Approach A keeps heap layout unchanged (nested arrays/tuples) and
copies out → compute → allocate back. Approach B (full vector ISA /
contiguous matrix heap objects) is deferred.

---

## 2. Scope

| Op | Source | Host native | Notes |
|----|--------|-------------|-------|
| `dot(a,b)` | named helper | `packed_dot` | Equal-length vectors |
| `matmul(A,B)` / `Matrix` `*` | named / Mul | `packed_matmul` | Row-major `m×k` × `k×n` |
| `Matrix` `+` / `-` | MatrixZip | `packed_matrix_zip` | Cell-wise add/sub |
| `Matrix` unary `-` | MatrixNeg | `packed_matrix_neg` | Cell-wise neg |
| `cross(a,b)` | named helper | **keep unroll** | Fixed N=3; tiny; revisit later |

No new syntax. Typechecker / `LinearAlgebraInfo` side table unchanged.
Codegen prefers HostInvoke when dims fit; otherwise falls back to unroll.

---

## 3. Meta `u32` layouts (same bits as the former opcodes)

Dims that do not fit emit a **compile-time warning** and fall back to
scalar unroll. Dot length uses a `u16` ceiling (`65535`); matrix dims
use `u8` (255).

### 3.1 `packed_dot` — args `[a, b, meta]`

- `meta[15:0]` = length (`u16`)
- `meta[16]` = `is_float`
- Returns scalar `Σ a[i]*b[i]`

### 3.2 `packed_matmul` — args `[A, B, meta]`

- `meta[7:0]` = `m`, `[15:8]` = `k`, `[23:16]` = `n`
- `[24]` = `is_float`, `[25]` = `outer_is_tuple`, `[26]` = `row_is_tuple`
- Returns nested `m×n` result

### 3.3 `packed_matrix_zip` — args `[A, B, meta]`

- `meta[7:0]` = `m`, `[15:8]` = `n`, `[23:16]` = zip kind (`0` Add, `1` Sub)
- `[24]` = `is_float`, `[25]` = `outer_is_tuple`, `[26]` = `row_is_tuple`

### 3.4 `packed_matrix_neg` — args `[A, meta]`

- `meta[7:0]` = `m`, `[15:8]` = `n`
- `[16]` = `is_float`, `[17]` = `outer_is_tuple`, `[18]` = `row_is_tuple`

---

## 4. VM / host contract

Shared helpers in `machine/src/packed_la.rs`:

1. **`aggregate_elements`** — Array or Tuple.
2. **`extract_matrix_row_major`** — walk outer × inner.
3. **Kernel loops** — contiguous indexing; **no SIMD in v1**.
4. **`alloc_nested_matrix`** — allocate inner rows then outer container.

Pipeline wires natives in `Pipeline::register_packed_la_natives`
(after IO natives). Codegen looks up ids via `Compiler::native_id`.

Meta CONST bytes **must** use `with_operand_u32` (full 32 bits). Do not
use `with_value_u32` for meta — it only keeps the low 16 bits.

---

## 5. Codegen

`try_emit_packed_linear_algebra` for Dot / MatMul / MatrixZip / MatrixNeg:

1. Resolve host-native id; bail to unroll if unregistered.
2. Emit `CONST id`, compile value args, `CONST meta`, `MakeTuple(n+1)`,
   `HostInvoke(n+1)`.
3. If any dim exceeds packed field width, keep the unroll path.

`cross` stays on the unroll path.

---

## 6. Tests

| Suite | What |
|-------|------|
| `machine` unit | `packed_dot` int; `packed_matmul` 2×2; zip add; neg |
| Pipeline goldens | Existing `vec_dot`, `vec_matmul`, `matrix_mul` unchanged outputs |
| Codegen | Assert packed ops emit `HostInvoke` (not a MUL cascade); `cross` does not |

---

## 7. Non-goals / follow-ups

- Contiguous `ObjMatrix` heap type (Approach B).
- SIMD / FMA / blocked matmul inside the kernel.
- Dedicated opcodes again (rejected for fib dispatch cost).
- Changing zip Hadamard on bare tuples/arrays (NT tower unroll stays).

---

## 8. Acceptance

- `examples/matrix_mul.hy` → `19,22,43,502`
- `examples/vec_matmul.hy` → `19,22,43,50`
- `examples/vec_dot.hy` → `32,001`
- `cargo test --workspace` green
- `ARCHIVE_VERSION == 29`; `Instruction` last variant remains `TailCall`
  (same as `main`)
