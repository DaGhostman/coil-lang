//! Packed linear-algebra kernels invoked via [`Instruction::HostInvoke`].
//!
//! Kept off the `Instruction` enum so fib/scalar dispatch matches `main`
//! (Approach A originally appended four opcodes and regressed branch
//! prediction on some CPUs). Dims / flags are packed into a meta `u32`
//! argument — same bit layout as the former packed opcodes.
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
        let mut sum = 0.0_f64;
        for i in 0..n {
            sum += av[i].as_float() * bv[i].as_float();
        }
        Value::from(sum)
    } else {
        let mut sum = 0_i64;
        for i in 0..n {
            sum = sum.wrapping_add(av[i].as_int().wrapping_mul(bv[i].as_int()));
        }
        Value::from(sum)
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
    let mut c = vec![Value::default(); m.saturating_mul(n)];
    if is_float {
        for i in 0..m {
            for j in 0..n {
                let mut acc = 0.0_f64;
                for t in 0..k {
                    acc += a_cells[i * k + t].as_float() * b_cells[t * n + j].as_float();
                }
                c[i * n + j] = Value::from(acc);
            }
        }
    } else {
        for i in 0..m {
            for j in 0..n {
                let mut acc = 0_i64;
                for t in 0..k {
                    acc = acc.wrapping_add(
                        a_cells[i * k + t]
                            .as_int()
                            .wrapping_mul(b_cells[t * n + j].as_int()),
                    );
                }
                c[i * n + j] = Value::from(acc);
            }
        }
    }
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
    let mut c = Vec::with_capacity(m.saturating_mul(n));
    for i in 0..m.saturating_mul(n) {
        let cell = if is_float {
            let av = a_cells[i].as_float();
            let bv = b_cells[i].as_float();
            Value::from(if zip_kind == 1 { av - bv } else { av + bv })
        } else {
            let av = a_cells[i].as_int();
            let bv = b_cells[i].as_int();
            Value::from(if zip_kind == 1 {
                av.wrapping_sub(bv)
            } else {
                av.wrapping_add(bv)
            })
        };
        c.push(cell);
    }
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
    let mut c = Vec::with_capacity(a_cells.len());
    for cell in a_cells {
        c.push(if is_float {
            Value::from(-cell.as_float())
        } else {
            Value::from(cell.as_int().wrapping_neg())
        });
    }
    alloc_nested_matrix(heap, c, m, n, outer_is_tuple, row_is_tuple)
}

/// Stable host-native names (also used by codegen `native_id` lookup).
pub const PACKED_DOT: &str = "packed_dot";
pub const PACKED_MATMUL: &str = "packed_matmul";
pub const PACKED_MATRIX_ZIP: &str = "packed_matrix_zip";
pub const PACKED_MATRIX_NEG: &str = "packed_matrix_neg";

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
}
