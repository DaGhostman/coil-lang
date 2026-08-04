//! Packed linear-algebra kernels invoked via [`Instruction::HostInvoke`].
//!
//! Kept off the `Instruction` enum so fib/scalar dispatch matches `main`
//! (Approach A originally appended four opcodes and regressed branch
//! prediction on some CPUs). Dims / flags are packed into a meta `u32`
//! argument — same bit layout as the former packed opcodes.
//!
//! Numeric work is forwarded to [`coil_simd`] after packing nested
//! aggregates into contiguous `f64` / `i64` buffers.
//!
//! **Defensive posture:** malformed handles / shape mismatches do **not**
//! trap. Missing aggregates yield `0` (dot) or zero-filled cells
//! (matmul/zip/neg) so the VM stays aligned with other silent-fallback
//! arms. Correctness for well-typed programs relies on the typechecker
//! and codegen never emitting these kernels with bad static shapes.

use common::Value;

use crate::{Heap, ObjArray, ObjTuple, Object};

fn find_object(heap: &Heap, addr: u64) -> Option<Object> {
    heap.find_object_by_addr(addr)
}

fn aggregate_elements(heap: &Heap, v: Value) -> Option<Vec<Value>> {
    match find_object(heap, v.raw() as u64) {
        Some(Object::Array(gc)) => Some(gc.as_ref().elements.clone()),
        Some(Object::Tuple(gc)) => Some(gc.as_ref().elements.clone()),
        _ => None,
    }
}

fn extract_matrix_row_major(heap: &Heap, v: Value, m: usize, n: usize) -> Option<Vec<Value>> {
    let rows = aggregate_elements(heap, v)?;
    if rows.len() < m {
        return None;
    }
    let mut out = Vec::with_capacity(m.saturating_mul(n));
    for i in 0..m {
        let row = aggregate_elements(heap, rows[i])?;
        if row.len() < n {
            return None;
        }
        out.extend_from_slice(&row[..n]);
    }
    Some(out)
}

fn alloc_aggregate(heap: &mut Heap, values: Vec<Value>, is_tuple: bool) -> Value {
    let addr = if is_tuple {
        let (object, _) = heap.alloc(ObjTuple { elements: values }, Object::Tuple);
        object.addr()
    } else {
        let (object, _) = heap.alloc(ObjArray { elements: values }, Object::Array);
        object.addr()
    };
    Value::from(addr)
}

fn alloc_nested_matrix(
    heap: &mut Heap,
    cells: Vec<Value>,
    m: usize,
    n: usize,
    outer_is_tuple: bool,
    row_is_tuple: bool,
) -> Value {
    let mut rows = Vec::with_capacity(m);
    for i in 0..m {
        let start = i * n;
        let row = cells[start..start + n].to_vec();
        rows.push(alloc_aggregate(heap, row, row_is_tuple));
    }
    alloc_aggregate(heap, rows, outer_is_tuple)
}

fn values_to_f64(cells: &[Value]) -> Vec<f64> {
    cells.iter().map(|v| v.as_float()).collect()
}

fn values_to_i64(cells: &[Value]) -> Vec<i64> {
    cells.iter().map(|v| v.as_int()).collect()
}

fn f64_to_values(cells: &[f64]) -> Vec<Value> {
    cells.iter().copied().map(Value::from).collect()
}

fn i64_to_values(cells: &[i64]) -> Vec<Value> {
    cells.iter().copied().map(Value::from).collect()
}

/// `packed_dot(a, b, meta)` — `meta` bits match former `PackedDot` operands.
pub fn packed_dot(heap: &mut Heap, args: &[Value]) -> Value {
    if args.len() < 3 {
        return Value::default();
    }
    let a = args[0];
    let b = args[1];
    let ops = args[2].as_int() as u32;
    let len = (ops & 0xFFFF) as usize;
    let is_float = (ops & (1 << 16)) != 0;
    let av = aggregate_elements(heap, a).unwrap_or_default();
    let bv = aggregate_elements(heap, b).unwrap_or_default();
    let n = len.min(av.len()).min(bv.len());
    if is_float {
        let a = values_to_f64(&av[..n]);
        let b = values_to_f64(&bv[..n]);
        Value::from(coil_simd::dot_f64(&a, &b))
    } else {
        let a = values_to_i64(&av[..n]);
        let b = values_to_i64(&bv[..n]);
        Value::from(coil_simd::dot_i64(&a, &b))
    }
}

/// `packed_matmul(a, b, meta)` — former `PackedMatMul` operand layout.
pub fn packed_matmul(heap: &mut Heap, args: &[Value]) -> Value {
    if args.len() < 3 {
        return Value::default();
    }
    let a = args[0];
    let b = args[1];
    let ops = args[2].as_int() as u32;
    let m = (ops & 0xFF) as usize;
    let k = ((ops >> 8) & 0xFF) as usize;
    let n = ((ops >> 16) & 0xFF) as usize;
    let is_float = (ops & (1 << 24)) != 0;
    let outer_is_tuple = (ops & (1 << 25)) != 0;
    let row_is_tuple = (ops & (1 << 26)) != 0;
    let a_cells = extract_matrix_row_major(heap, a, m, k)
        .unwrap_or_else(|| vec![Value::default(); m.saturating_mul(k)]);
    let b_cells = extract_matrix_row_major(heap, b, k, n)
        .unwrap_or_else(|| vec![Value::default(); k.saturating_mul(n)]);
    let c = if is_float {
        let a = values_to_f64(&a_cells);
        let b = values_to_f64(&b_cells);
        let mut c = vec![0.0; m.saturating_mul(n)];
        coil_simd::matmul_f64(&a, &b, &mut c, m, k, n);
        f64_to_values(&c)
    } else {
        let a = values_to_i64(&a_cells);
        let b = values_to_i64(&b_cells);
        let mut c = vec![0_i64; m.saturating_mul(n)];
        coil_simd::matmul_i64(&a, &b, &mut c, m, k, n);
        i64_to_values(&c)
    };
    alloc_nested_matrix(heap, c, m, n, outer_is_tuple, row_is_tuple)
}

/// `packed_matrix_zip(a, b, meta)` — former `PackedMatrixZip` operand layout.
pub fn packed_matrix_zip(heap: &mut Heap, args: &[Value]) -> Value {
    if args.len() < 3 {
        return Value::default();
    }
    let a = args[0];
    let b = args[1];
    let ops = args[2].as_int() as u32;
    let m = (ops & 0xFF) as usize;
    let n = ((ops >> 8) & 0xFF) as usize;
    let zip_kind = ((ops >> 16) & 0xFF) as u8;
    let is_float = (ops & (1 << 24)) != 0;
    let outer_is_tuple = (ops & (1 << 25)) != 0;
    let row_is_tuple = (ops & (1 << 26)) != 0;
    let a_cells = extract_matrix_row_major(heap, a, m, n)
        .unwrap_or_else(|| vec![Value::default(); m.saturating_mul(n)]);
    let b_cells = extract_matrix_row_major(heap, b, m, n)
        .unwrap_or_else(|| vec![Value::default(); m.saturating_mul(n)]);
    let len = m.saturating_mul(n);
    let c = if is_float {
        let a = values_to_f64(&a_cells[..len.min(a_cells.len())]);
        let b = values_to_f64(&b_cells[..len.min(b_cells.len())]);
        let mut out = vec![0.0; a.len().min(b.len())];
        if zip_kind == 1 {
            coil_simd::zip_sub_f64(&a, &b, &mut out);
        } else {
            coil_simd::zip_add_f64(&a, &b, &mut out);
        }
        // Pad if extract was short (defensive zero fill already in a_cells path).
        while out.len() < len {
            out.push(0.0);
        }
        f64_to_values(&out[..len])
    } else {
        let a = values_to_i64(&a_cells[..len.min(a_cells.len())]);
        let b = values_to_i64(&b_cells[..len.min(b_cells.len())]);
        let mut out = vec![0_i64; a.len().min(b.len())];
        if zip_kind == 1 {
            coil_simd::zip_sub_i64(&a, &b, &mut out);
        } else {
            coil_simd::zip_add_i64(&a, &b, &mut out);
        }
        while out.len() < len {
            out.push(0);
        }
        i64_to_values(&out[..len])
    };
    alloc_nested_matrix(heap, c, m, n, outer_is_tuple, row_is_tuple)
}

/// `packed_matrix_neg(a, meta)` — former `PackedMatrixNeg` operand layout.
pub fn packed_matrix_neg(heap: &mut Heap, args: &[Value]) -> Value {
    if args.len() < 2 {
        return Value::default();
    }
    let a = args[0];
    let ops = args[1].as_int() as u32;
    let m = (ops & 0xFF) as usize;
    let n = ((ops >> 8) & 0xFF) as usize;
    let is_float = (ops & (1 << 16)) != 0;
    let outer_is_tuple = (ops & (1 << 17)) != 0;
    let row_is_tuple = (ops & (1 << 18)) != 0;
    let a_cells = extract_matrix_row_major(heap, a, m, n)
        .unwrap_or_else(|| vec![Value::default(); m.saturating_mul(n)]);
    let c = if is_float {
        let a = values_to_f64(&a_cells);
        let mut out = vec![0.0; a.len()];
        coil_simd::zip_neg_f64(&a, &mut out);
        f64_to_values(&out)
    } else {
        let a = values_to_i64(&a_cells);
        let mut out = vec![0_i64; a.len()];
        coil_simd::zip_neg_i64(&a, &mut out);
        i64_to_values(&out)
    };
    alloc_nested_matrix(heap, c, m, n, outer_is_tuple, row_is_tuple)
}

/// Zip / broadcast / negate for 1-D aggregates (`[T; N]` / `(T,…)`).
///
/// Meta bitfield:
/// - bits 0..15: length
/// - bits 16..23: op — 0=add, 1=sub, 2=mul, 3=div, 4=neg
/// - bit 24: float elements
/// - bit 25: result is tuple (else array)
/// - bit 26: broadcast mode (`args[1]` is scalar; ignored for neg)
/// - bit 27: scalar is on the left (broadcast only; matters for sub/div)
pub fn packed_vec_arith(heap: &mut Heap, args: &[Value]) -> Value {
    if args.len() < 2 {
        return Value::default();
    }
    let ops = args[args.len() - 1].as_int() as u32;
    let len = (ops & 0xFFFF) as usize;
    let op = ((ops >> 16) & 0xFF) as u8;
    let is_float = (ops & (1 << 24)) != 0;
    let is_tuple = (ops & (1 << 25)) != 0;
    let broadcast = (ops & (1 << 26)) != 0;
    let scalar_left = (ops & (1 << 27)) != 0;

    if op == 4 {
        // Unary neg: args = [vec, meta]
        let cells = aggregate_elements(heap, args[0])
            .unwrap_or_else(|| vec![Value::default(); len]);
        let n = len.min(cells.len());
        let out = if is_float {
            let a = values_to_f64(&cells[..n]);
            let mut o = vec![0.0; a.len()];
            coil_simd::zip_neg_f64(&a, &mut o);
            while o.len() < len {
                o.push(0.0);
            }
            f64_to_values(&o[..len])
        } else {
            let a = values_to_i64(&cells[..n]);
            let mut o = vec![0_i64; a.len()];
            coil_simd::zip_neg_i64(&a, &mut o);
            while o.len() < len {
                o.push(0);
            }
            i64_to_values(&o[..len])
        };
        return alloc_aggregate(heap, out, is_tuple);
    }

    if args.len() < 3 {
        return Value::default();
    }
    let lhs = args[0];
    let rhs = args[1];

    if broadcast {
        let (vec_v, sc_v) = if scalar_left {
            (rhs, lhs)
        } else {
            (lhs, rhs)
        };
        let cells = aggregate_elements(heap, vec_v)
            .unwrap_or_else(|| vec![Value::default(); len]);
        let n = len.min(cells.len());
        let out = if is_float {
            let a = values_to_f64(&cells[..n]);
            let s = sc_v.as_float();
            let mut o = vec![0.0; a.len()];
            match op {
                2 if !scalar_left => coil_simd::scale_f64(&a, s, &mut o),
                2 => coil_simd::scale_f64(&a, s, &mut o), // mul commutative
                _ => {
                    let b = vec![s; a.len()];
                    match op {
                        0 if !scalar_left => coil_simd::zip_add_f64(&a, &b, &mut o),
                        0 => coil_simd::zip_add_f64(&b, &a, &mut o),
                        1 if !scalar_left => coil_simd::zip_sub_f64(&a, &b, &mut o),
                        1 => coil_simd::zip_sub_f64(&b, &a, &mut o),
                        3 if !scalar_left => coil_simd::zip_div_f64(&a, &b, &mut o),
                        3 => coil_simd::zip_div_f64(&b, &a, &mut o),
                        _ => coil_simd::zip_add_f64(&a, &b, &mut o),
                    }
                }
            }
            while o.len() < len {
                o.push(0.0);
            }
            f64_to_values(&o[..len])
        } else {
            let a = values_to_i64(&cells[..n]);
            let s = sc_v.as_int();
            let mut o = vec![0_i64; a.len()];
            match op {
                2 => coil_simd::scale_i64(&a, s, &mut o),
                _ => {
                    let b = vec![s; a.len()];
                    match op {
                        0 => coil_simd::zip_add_i64(&a, &b, &mut o),
                        1 if !scalar_left => coil_simd::zip_sub_i64(&a, &b, &mut o),
                        1 => coil_simd::zip_sub_i64(&b, &a, &mut o),
                        3 if !scalar_left => {
                            for i in 0..a.len() {
                                o[i] = a[i] / b[i];
                            }
                        }
                        3 => {
                            for i in 0..a.len() {
                                o[i] = b[i] / a[i];
                            }
                        }
                        _ => coil_simd::zip_add_i64(&a, &b, &mut o),
                    }
                }
            }
            while o.len() < len {
                o.push(0);
            }
            i64_to_values(&o[..len])
        };
        return alloc_aggregate(heap, out, is_tuple);
    }

    let av = aggregate_elements(heap, lhs).unwrap_or_else(|| vec![Value::default(); len]);
    let bv = aggregate_elements(heap, rhs).unwrap_or_else(|| vec![Value::default(); len]);
    let n = len.min(av.len()).min(bv.len());
    let out = if is_float {
        let a = values_to_f64(&av[..n]);
        let b = values_to_f64(&bv[..n]);
        let mut o = vec![0.0; a.len().min(b.len())];
        match op {
            1 => coil_simd::zip_sub_f64(&a, &b, &mut o),
            2 => coil_simd::zip_mul_f64(&a, &b, &mut o),
            3 => coil_simd::zip_div_f64(&a, &b, &mut o),
            _ => coil_simd::zip_add_f64(&a, &b, &mut o),
        }
        while o.len() < len {
            o.push(0.0);
        }
        f64_to_values(&o[..len])
    } else {
        let a = values_to_i64(&av[..n]);
        let b = values_to_i64(&bv[..n]);
        let mut o = vec![0_i64; a.len().min(b.len())];
        match op {
            1 => coil_simd::zip_sub_i64(&a, &b, &mut o),
            2 => coil_simd::zip_mul_i64(&a, &b, &mut o),
            3 => {
                for i in 0..o.len() {
                    o[i] = a[i] / b[i];
                }
            }
            _ => coil_simd::zip_add_i64(&a, &b, &mut o),
        }
        while o.len() < len {
            o.push(0);
        }
        i64_to_values(&o[..len])
    };
    alloc_aggregate(heap, out, is_tuple)
}

/// Stable host-native names (also used by codegen `native_id` lookup).
pub const PACKED_DOT: &str = "packed_dot";
pub const PACKED_MATMUL: &str = "packed_matmul";
pub const PACKED_MATRIX_ZIP: &str = "packed_matrix_zip";
pub const PACKED_MATRIX_NEG: &str = "packed_matrix_neg";
pub const PACKED_VEC_ARITH: &str = "packed_vec_arith";

#[cfg(test)]
mod tests {
    use super::*;
    use common::Value;

    fn alloc_array(heap: &mut Heap, elems: Vec<i64>) -> Value {
        let values: Vec<Value> = elems.into_iter().map(Value::from).collect();
        alloc_aggregate(heap, values, false)
    }

    fn alloc_matrix2(heap: &mut Heap, rows: [[i64; 2]; 2]) -> Value {
        let r0 = alloc_array(heap, vec![rows[0][0], rows[0][1]]);
        let r1 = alloc_array(heap, vec![rows[1][0], rows[1][1]]);
        alloc_aggregate(heap, vec![r0, r1], false)
    }

    fn alloc_array_f(heap: &mut Heap, elems: Vec<f64>) -> Value {
        let values: Vec<Value> = elems.into_iter().map(Value::from).collect();
        alloc_aggregate(heap, values, false)
    }

    fn alloc_matrix2_f(heap: &mut Heap, rows: [[f64; 2]; 2]) -> Value {
        let r0 = alloc_array_f(heap, vec![rows[0][0], rows[0][1]]);
        let r1 = alloc_array_f(heap, vec![rows[1][0], rows[1][1]]);
        alloc_aggregate(heap, vec![r0, r1], false)
    }

    #[test]
    fn packed_dot_int_sums_products() {
        let mut heap = Heap::default();
        let a = alloc_array(&mut heap, vec![1, 2, 3]);
        let b = alloc_array(&mut heap, vec![4, 5, 6]);
        let meta = Value::from(3_i64); // length=3, int
        let out = packed_dot(&mut heap, &[a, b, meta]);
        assert_eq!(out.as_int(), 32); // 1*4+2*5+3*6
    }

    #[test]
    fn packed_dot_float_long() {
        let mut heap = Heap::default();
        let a_vals: Vec<f64> = (0..64).map(|i| i as f64).collect();
        let b_vals: Vec<f64> = (0..64).map(|i| (i as f64) * 0.5).collect();
        let expect: f64 = a_vals.iter().zip(&b_vals).map(|(x, y)| x * y).sum();
        let a = alloc_array_f(&mut heap, a_vals);
        let b = alloc_array_f(&mut heap, b_vals);
        let meta = Value::from((64_i64) | (1 << 16));
        let out = packed_dot(&mut heap, &[a, b, meta]);
        assert!((out.as_float() - expect).abs() < 1e-6);
    }

    #[test]
    fn packed_matmul_2x2_int() {
        let mut heap = Heap::default();
        let a = alloc_matrix2(&mut heap, [[1, 2], [3, 4]]);
        let b = alloc_matrix2(&mut heap, [[5, 6], [7, 8]]);
        // m=2,k=2,n=2
        let meta = Value::from((2 | (2 << 8) | (2 << 16)) as i64);
        let out = packed_matmul(&mut heap, &[a, b, meta]);
        let rows = aggregate_elements(&heap, out).expect("rows");
        assert_eq!(rows.len(), 2);
        let r0 = aggregate_elements(&heap, rows[0]).expect("r0");
        let r1 = aggregate_elements(&heap, rows[1]).expect("r1");
        assert_eq!(r0[0].as_int(), 19);
        assert_eq!(r0[1].as_int(), 22);
        assert_eq!(r1[0].as_int(), 43);
        assert_eq!(r1[1].as_int(), 50);
    }

    #[test]
    fn packed_matmul_2x2_float() {
        let mut heap = Heap::default();
        let a = alloc_matrix2_f(&mut heap, [[1.0, 2.0], [3.0, 4.0]]);
        let b = alloc_matrix2_f(&mut heap, [[5.0, 6.0], [7.0, 8.0]]);
        let meta = Value::from((2 | (2 << 8) | (2 << 16) | (1 << 24)) as i64);
        let out = packed_matmul(&mut heap, &[a, b, meta]);
        let rows = aggregate_elements(&heap, out).expect("rows");
        let r0 = aggregate_elements(&heap, rows[0]).expect("r0");
        assert!((r0[0].as_float() - 19.0).abs() < 1e-9);
        assert!((r0[1].as_float() - 22.0).abs() < 1e-9);
    }

    #[test]
    fn packed_matrix_zip_add_and_neg() {
        let mut heap = Heap::default();
        let a = alloc_matrix2(&mut heap, [[1, 2], [3, 4]]);
        let b = alloc_matrix2(&mut heap, [[1, 1], [1, 1]]);
        let add_meta = Value::from((2 | (2 << 8) | (0 << 16)) as i64); // Add
        let sum = packed_matrix_zip(&mut heap, &[a, b, add_meta]);
        let rows = aggregate_elements(&heap, sum).expect("rows");
        let r0 = aggregate_elements(&heap, rows[0]).expect("r0");
        assert_eq!(r0[0].as_int(), 2);

        let neg_meta = Value::from((2 | (2 << 8)) as i64);
        let neg = packed_matrix_neg(&mut heap, &[a, neg_meta]);
        let nrows = aggregate_elements(&heap, neg).expect("nrows");
        let n0 = aggregate_elements(&heap, nrows[0]).expect("n0");
        assert_eq!(n0[0].as_int(), -1);
    }

    #[test]
    fn packed_vec_arith_zip_mul_and_broadcast() {
        let mut heap = Heap::default();
        let a = alloc_array(&mut heap, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        let b = alloc_array(&mut heap, vec![2, 2, 2, 2, 2, 2, 2, 2]);
        // len=8, op=mul(2), int, array
        let mul_meta = Value::from((8 | (2 << 16)) as i64);
        let prod = packed_vec_arith(&mut heap, &[a, b, mul_meta]);
        let elems = aggregate_elements(&heap, prod).expect("prod");
        assert_eq!(elems[0].as_int(), 2);
        assert_eq!(elems[7].as_int(), 16);

        // broadcast mul: vec * 3
        let scale_meta = Value::from((8 | (2 << 16) | (1 << 26)) as i64);
        let scaled = packed_vec_arith(&mut heap, &[a, Value::from(3_i64), scale_meta]);
        let se = aggregate_elements(&heap, scaled).expect("scaled");
        assert_eq!(se[0].as_int(), 3);
        assert_eq!(se[7].as_int(), 24);
    }

    #[test]
    fn packed_vec_arith_neg_and_float_div() {
        let mut heap = Heap::default();
        let a = alloc_array(&mut heap, vec![1, -2, 3, -4, 5, -6, 7, -8]);
        // unary neg: args = [vec, meta]; op=4
        let neg_meta = Value::from((8 | (4 << 16) | (1 << 25)) as i64); // tuple result
        let neg = packed_vec_arith(&mut heap, &[a, neg_meta]);
        let ne = aggregate_elements(&heap, neg).expect("neg");
        assert_eq!(ne.len(), 8);
        assert_eq!(ne[0].as_int(), -1);
        assert_eq!(ne[7].as_int(), 8);

        let fa = alloc_array_f(
            &mut heap,
            vec![2.0, 4.0, 6.0, 8.0, 10.0, 12.0, 14.0, 16.0],
        );
        let fb = alloc_array_f(&mut heap, vec![2.0; 8]);
        let div_meta = Value::from((8 | (3 << 16) | (1 << 24)) as i64); // div + float
        let quot = packed_vec_arith(&mut heap, &[fa, fb, div_meta]);
        let qe = aggregate_elements(&heap, quot).expect("quot");
        assert!((qe[0].as_float() - 1.0).abs() < 1e-12);
        assert!((qe[7].as_float() - 8.0).abs() < 1e-12);
    }

    #[test]
    fn packed_vec_arith_broadcast_scalar_left_sub() {
        let mut heap = Heap::default();
        let v = alloc_array(&mut heap, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        // 10 - v  (broadcast + scalar_left + sub)
        let meta = Value::from((8 | (1 << 16) | (1 << 26) | (1 << 27)) as i64);
        let out = packed_vec_arith(&mut heap, &[Value::from(10_i64), v, meta]);
        let elems = aggregate_elements(&heap, out).expect("sub");
        assert_eq!(elems[0].as_int(), 9);
        assert_eq!(elems[7].as_int(), 2);
    }

    #[test]
    fn packed_vec_arith_short_args_and_bad_handle_are_silent() {
        let mut heap = Heap::default();
        assert_eq!(packed_vec_arith(&mut heap, &[]).as_int(), 0);
        // Unary path with too-few args still needs meta; binary needs 3.
        assert_eq!(
            packed_vec_arith(&mut heap, &[Value::from(1_i64)]).as_int(),
            0
        );
        let meta = Value::from((8 | (2 << 16)) as i64); // zip mul
        let missing = packed_vec_arith(
            &mut heap,
            &[Value::from(0_i64), Value::from(0_i64), meta],
        );
        let elems = aggregate_elements(&heap, missing).expect("zero-filled");
        assert_eq!(elems.len(), 8);
        assert!(elems.iter().all(|v| v.as_int() == 0));
    }

    #[test]
    fn packed_matrix_zip_sub_float() {
        let mut heap = Heap::default();
        let a = alloc_matrix2_f(&mut heap, [[5.0, 7.0], [9.0, 11.0]]);
        let b = alloc_matrix2_f(&mut heap, [[1.0, 2.0], [3.0, 4.0]]);
        let meta = Value::from((2 | (2 << 8) | (1 << 16) | (1 << 24)) as i64); // sub + float
        let diff = packed_matrix_zip(&mut heap, &[a, b, meta]);
        let rows = aggregate_elements(&heap, diff).expect("rows");
        let r0 = aggregate_elements(&heap, rows[0]).expect("r0");
        assert!((r0[0].as_float() - 4.0).abs() < 1e-12);
        assert!((r0[1].as_float() - 5.0).abs() < 1e-12);
    }
}
