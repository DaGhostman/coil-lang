# Approach A — Packed Fat Opcodes for Linear Algebra

**Status:** implemented  
**Depends on:** NT-7 named helpers + `Matrix`/`Mul` (already shipped)  
**Goal:** Replace compile-time scalar unrolls for `dot` / `matmul` /
`Matrix` `*` `+` `-` with **one fat opcode per op family**, whose VM
handler extracts nested aggregates into contiguous Rust buffers, runs a
tight packed kernel, then rebuilds the nested result. Same language
surface and observable results; enables future SIMD/FMA inside the
kernel without changing the opcode API.

---

## 1. Why Approach A

Today `emit_linear_algebra` unrolls every cell into `LOAD` / `CONST` /
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

| Op | Source | Fat opcode | Notes |
|----|--------|------------|-------|
| `dot(a,b)` | named helper | `PackedDot` | Equal-length vectors |
| `matmul(A,B)` / `Matrix` `*` | named / Mul | `PackedMatMul` | Row-major `m×k` × `k×n` |
| `Matrix` `+` / `-` | MatrixZip | `PackedMatrixZip` | Cell-wise add/sub |
| `Matrix` unary `-` | MatrixNeg | `PackedMatrixNeg` | Cell-wise neg |
| `cross(a,b)` | named helper | **keep unroll** | Fixed N=3; tiny; revisit later |

No new syntax. Typechecker / `LinearAlgebraInfo` side table unchanged.
Only codegen lowering + VM + `ARCHIVE_VERSION` bump.

---

## 3. Opcode layout (append-only)

Append after `TailCall`. Bump `ARCHIVE_VERSION` **27 → 28**. Raise the
release `promise!(opcode <= …)` ceiling to the new last variant.

### 3.1 `PackedDot`

- Stack: `[..., a, b]` (TOS = `b`)
- `operands[15:0]` = length (`u16`)
- `operands[16]` = `is_float`
- Push scalar `Σ a[i]*b[i]`

### 3.2 `PackedMatMul`

- Stack: `[..., A, B]` (TOS = `B`)
- Dims packed as `u8` (any dim > 255 → typechecker warns, codegen unrolls):
  - `operands[7:0]` = `m`, `[15:8]` = `k`, `[23:16]` = `n`
  - `[24]` = `is_float`, `[25]` = `outer_is_tuple`, `[26]` = `row_is_tuple`
- Push nested `m×n` result (array/tuple of rows per flags)

### 3.3 `PackedMatrixZip`

- Stack: `[..., A, B]`
- `operands[7:0]` = `m`, `[15:8]` = `n`, `[23:16]` = zip kind (`0` Add, `1` Sub)
- `[24]` = `is_float`, `[25]` = `outer_is_tuple`, `[26]` = `row_is_tuple`

### 3.4 `PackedMatrixNeg`

- Stack: `[..., A]`
- `operands[7:0]` = `m`, `[15:8]` = `n`
- `[16]` = `is_float`, `[17]` = `outer_is_tuple`, `[18]` = `row_is_tuple`

Dims that do not fit the packed fields emit a **compile-time warning**
(e.g. `matrix multiply dimensions \`2×256×2\` exceed the packed opcode
limit (255)`) and fall back to the existing scalar unroll (non-fatal —
unroll remains correct). Dot length uses a `u16` ceiling (`65535`).

VM handlers for `Packed*` live in `#[inline(never)]` helpers
(`exec_packed_dot` / `exec_packed_matmul` / …) so the hot `execute`
match stays small for scalar workloads (fib / numeric loops).

---

## 4. VM kernel contract

Shared helpers on `Machine` (or free fns with `&Heap` / `&mut Heap`):

1. **`aggregate_elements(heap, v) -> Option<&[Value]>`** — Array or Tuple.
2. **`extract_matrix_row_major(heap, v, m, n) -> Option<Vec<Value>>`** —
   walk outer × inner; fail soft → push `0` / empty defensive result
   (typechecker is source of truth).
3. **`packed_*_kernel`** — convert to `Vec<i64>` or `Vec<f64>`, loop
   with contiguous indexing (`c[i*n+j] += a[i*k+t] * b[t*n+j]` for
   matmul). **No SIMD in v1** — keep the loop obvious so SIMD can land
   later inside the same function.
4. **`alloc_nested_matrix(...)`** — allocate inner rows then outer
   container; bump `alloc_counter` / GC pressure like `MakeArray`.

Extract copies `Value`s (immediates or heap pointers for nested cells —
cells are scalars for these ops).

---

## 5. Codegen

`emit_linear_algebra` for Dot / MatMul / MatrixZip / MatrixNeg:

1. Compile args onto the stack (no temp `StorePop` required for the
   packed path — leave values on TOS in order `[a, b]`).
2. Emit the matching `Packed*` byte with packed dims/flags.
3. If any dim > `u16::MAX`, keep the pre-Approach-A unroll path.

`cross` stays on the unroll path.

Peephole: treat new opcodes as opaque (no fusion required).

---

## 6. Tests

| Suite | What |
|-------|------|
| `machine` unit | `PackedDot` int; `PackedMatMul` 2×2; zip add; neg |
| Pipeline goldens | Existing `vec_dot`, `vec_matmul`, `matrix_mul` unchanged outputs |
| Optional codegen | Assert `matmul` / `matrix *` emit `PackedMatMul` (not a cascade of `MUL`) |

---

## 7. Non-goals / follow-ups

- Contiguous `ObjMatrix` heap type (Approach B).
- SIMD / FMA / blocked matmul inside the kernel (same opcode later).
- `PackedCross`.
- Changing zip Hadamard on bare tuples/arrays (NT tower unroll stays).

---

## 8. Acceptance

- `examples/matrix_mul.hy` → `19,22,43,502`
- `examples/vec_matmul.hy` → `19,22,43,50`
- `examples/vec_dot.hy` → `32,001`
- `cargo test --workspace` green
- `ARCHIVE_VERSION == 28`; release opcode ceiling includes new variants
