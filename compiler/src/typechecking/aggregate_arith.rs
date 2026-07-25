//! Aggregate (tuple / array) arithmetic shapes for the numeric tower.
//!
//! Homogeneous numeric tuples and fixed-length arrays support element-wise
//! and scalar-broadcast ops. Dynamic `[T] ⊕ [T]` is a hard type error.

use super::ty::{ArrayLength, Ty};

/// Which side of a broadcast is the scalar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarSide {
    Left,
    Right,
}

/// Arithmetic operator lowered element-wise on aggregates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AggregateOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Neg,
}

impl AggregateOp {
    pub fn from_str(op: &str) -> Option<Self> {
        match op {
            "+" => Some(Self::Add),
            "-" => Some(Self::Sub),
            "*" => Some(Self::Mul),
            "/" => Some(Self::Div),
            "%" => Some(Self::Mod),
            "**" => Some(Self::Pow),
            "neg" => Some(Self::Neg),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Mod => "%",
            Self::Pow => "**",
            Self::Neg => "-",
        }
    }
}

/// Codegen recipe for aggregate arithmetic at a node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AggregateArithKind {
    ZipTuple {
        arity: usize,
        elem_is_float: bool,
    },
    ZipArray {
        length: usize,
        elem_is_float: bool,
    },
    BroadcastTuple {
        arity: usize,
        scalar_on: ScalarSide,
        elem_is_float: bool,
    },
    /// `length: None` means dynamic `[T]` (broadcast only — never zip).
    BroadcastArray {
        length: Option<usize>,
        scalar_on: ScalarSide,
        elem_is_float: bool,
    },
    NegTuple {
        arity: usize,
        elem_is_float: bool,
    },
    NegArray {
        length: Option<usize>,
        elem_is_float: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AggregateArithInfo {
    pub kind: AggregateArithKind,
    pub op: AggregateOp,
}

/// Classification of one operand for arithmetic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArithShape {
    Scalar(Ty),
    Tuple { elem: Ty, arity: usize },
    Array { elem: Ty, length: ArrayLength },
}

/// How two shapes combine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZipMode {
    Zip,
    BroadcastLeft,
    BroadcastRight,
}

pub fn is_numeric_elem(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Con(n) if n == "int" || n == "float" || n == "byte"
    ) || matches!(ty, Ty::Var(_))
}

pub fn elem_is_float(ty: &Ty) -> bool {
    matches!(ty, Ty::Con(n) if n == "float")
}

/// Classify a pruned type as an arithmetic shape.
///
/// Homogeneous tuples only; heterogeneous tuples return `None` from the
/// caller after a separate homogeneity check.
pub fn classify_arith(ty: &Ty) -> ArithShape {
    match ty {
        Ty::Tuple(elems) if !elems.is_empty() => {
            // Caller must verify homogeneity; use first element as elem.
            ArithShape::Tuple {
                elem: elems[0].clone(),
                arity: elems.len(),
            }
        }
        Ty::Array { element, length } => ArithShape::Array {
            elem: element.as_ref().clone(),
            length: *length,
        },
        other => ArithShape::Scalar(other.clone()),
    }
}

#[allow(dead_code)]
pub fn is_aggregate_shape(shape: &ArithShape) -> bool {
    !matches!(shape, ArithShape::Scalar(_))
}

/// Traits that the numeric tower lifts from element to homogeneous aggregate.
pub fn is_liftable_arith_trait(class: &str) -> bool {
    matches!(class, "Add" | "Sub" | "Mul" | "Div" | "Num")
}

/// If `ty` is a homogeneous tuple or array of a single element type, return
/// that element type. Heterogeneous tuples yield `None`.
pub fn homogeneous_aggregate_elem(ty: &Ty) -> Option<Ty> {
    match ty {
        Ty::Tuple(elems) if !elems.is_empty() => {
            let first = &elems[0];
            if elems.iter().all(|e| e == first) {
                Some(first.clone())
            } else {
                None
            }
        }
        Ty::Array { element, .. } => Some(element.as_ref().clone()),
        _ => None,
    }
}

/// Result type for a successful shape resolution.
pub fn result_ty_for(shape: &ArithShape) -> Ty {
    match shape {
        ArithShape::Scalar(t) => t.clone(),
        ArithShape::Tuple { elem, arity } => {
            Ty::Tuple(vec![elem.clone(); *arity])
        }
        ArithShape::Array { elem, length } => Ty::Array {
            element: Box::new(elem.clone()),
            length: *length,
        },
    }
}

/// Codegen recipe for named linear-algebra helpers (`dot` / `matmul` / `cross`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinearAlgebraKind {
    /// Equal-length vectors → scalar.
    Dot {
        length: usize,
        left_is_tuple: bool,
        elem_is_float: bool,
    },
    /// Length-3 vectors → length-3 vector (same container kind as left).
    Cross {
        left_is_tuple: bool,
        elem_is_float: bool,
    },
    /// Nested static matrices (row-major): `(m×k) × (k×n) → (m×n)`.
    MatMul {
        m: usize,
        k: usize,
        n: usize,
        /// Outer container is a tuple-of-rows (vs array-of-rows).
        outer_is_tuple: bool,
        /// Each row is a tuple (vs array).
        row_is_tuple: bool,
        elem_is_float: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinearAlgebraInfo {
    pub kind: LinearAlgebraKind,
}

/// Classify a homogeneous numeric vector for `dot` / `cross`.
pub fn classify_vector(ty: &Ty) -> Option<(Ty /*elem*/, usize /*len*/, bool /*is_tuple*/)> {
    match ty {
        Ty::Tuple(elems) if !elems.is_empty() => {
            let elem = homogeneous_aggregate_elem(ty)?;
            Some((elem, elems.len(), true))
        }
        Ty::Array {
            element,
            length: ArrayLength::Static(n),
        } if *n > 0 => Some((element.as_ref().clone(), *n, false)),
        _ => None,
    }
}

/// Classify a nested static matrix (rows × cols) for `matmul`.
///
/// Accepts `[[T; N]; M]` or a homogeneous tuple of equal-arity row tuples/arrays.
pub fn classify_matrix(
    ty: &Ty,
) -> Option<(Ty /*elem*/, usize /*m*/, usize /*n*/, bool /*outer_tuple*/, bool /*row_tuple*/)> {
    match ty {
        Ty::Array {
            element,
            length: ArrayLength::Static(m),
        } if *m > 0 => match element.as_ref() {
            Ty::Array {
                element: cell,
                length: ArrayLength::Static(n),
            } if *n > 0 => Some((cell.as_ref().clone(), *m, *n, false, false)),
            Ty::Tuple(row) if !row.is_empty() => {
                let elem = homogeneous_aggregate_elem(element)?;
                Some((elem, *m, row.len(), false, true))
            }
            _ => None,
        },
        Ty::Tuple(rows) if !rows.is_empty() => {
            let first = &rows[0];
            if !rows.iter().all(|r| r == first) {
                return None;
            }
            match first {
                Ty::Array {
                    element,
                    length: ArrayLength::Static(n),
                } if *n > 0 => Some((element.as_ref().clone(), rows.len(), *n, true, false)),
                Ty::Tuple(cols) if !cols.is_empty() => {
                    let elem = homogeneous_aggregate_elem(first)?;
                    Some((elem, rows.len(), cols.len(), true, true))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typechecking::ty::{array_fixed, int};

    #[test]
    fn classify_tuple_and_array() {
        let t = classify_arith(&Ty::Tuple(vec![int(), int()]));
        assert!(matches!(t, ArithShape::Tuple { arity: 2, .. }));
        let a = classify_arith(&array_fixed(int(), 3));
        assert!(matches!(
            a,
            ArithShape::Array {
                length: ArrayLength::Static(3),
                ..
            }
        ));
    }

    #[test]
    fn homogeneous_aggregate_elem_requires_uniform_tuple() {
        assert_eq!(
            homogeneous_aggregate_elem(&Ty::Tuple(vec![int(), int()])),
            Some(int())
        );
        assert!(
            homogeneous_aggregate_elem(&Ty::Tuple(vec![int(), Ty::Con("float".into())])).is_none()
        );
    }
}
