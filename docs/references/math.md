# Linear algebra (`dot` / `matmul` / `cross` / `Matrix`)

Auto-imported from virtual `prelude::math` (implicit `use prelude::math::*;`).

**Named helpers** do **not** overload `*` / `**` on bare tuples or arrays
(those stay element-wise; see [Operators](operators.md)).

| Helper | Arguments | Result |
|--------|-----------|--------|
| `dot(a, b)` | Equal-length homogeneous numeric vectors (tuple↔tuple or `[T; N]`↔`[T; N]`) | scalar `T` |
| `cross(a, b)` | Length-3 vectors (same container kind) | length-3 vector |
| `matmul(A, B)` | Nested fixed-length matrices: `[[T; K]; M]` × `[[T; N]; K]` | `[[T; N]; M]` (row-major) |
| `matrix(rows)` | Nested fixed-length matrix data | `Matrix<Data>` |

### `Matrix` and `*`

`matrix(...)` wraps nested static rows as a nominal `Matrix<Data>` type
(runtime is still the nested data — zero-cost). On `Matrix`:

| Op | Meaning |
|----|---------|
| `*` | **Matmul** (via `Mul`, not element-wise) |
| `+` / `-` | Element-wise zip |
| `/`, `%`, `**` | **Rejected** — `Matrix` is not `Num` |

```coil
dot((1, 2, 3), (4, 5, 6));           // 32
cross((1, 0, 0), (0, 1, 0));         // (0, 0, 1)
matmul([[1, 2], [3, 4]], [[5, 6], [7, 8]]);  // [[19, 22], [43, 50]]

let a = matrix([[1, 2], [3, 4]]);
let b = matrix([[5, 6], [7, 8]]);
let c = a * b;   // matmul → Matrix
let d = a + a;   // element-wise
```

See `examples/vec_dot.hy`, `examples/vec_matmul.hy`, and `examples/matrix_mul.hy`.

Packed kernels (`packed_dot` / `packed_matmul` / matrix zip) run as HostInvoke
natives and use the workspace `coil-simd` crate (stable `std::arch`, runtime
AVX2/NEON dispatch) — see [internals/simd.md](../internals/simd.md).

---

## Related

- [Operators](operators.md)
