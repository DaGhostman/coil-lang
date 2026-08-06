//! IEEE-754 scalar math host natives for virtual `prelude::math`.

use common::Value;

use crate::Heap;

pub fn math_sin(_heap: &mut Heap, args: &[Value]) -> Value {
    Value::from(args[0].as_float().sin())
}

pub fn math_cos(_heap: &mut Heap, args: &[Value]) -> Value {
    Value::from(args[0].as_float().cos())
}

pub fn math_tan(_heap: &mut Heap, args: &[Value]) -> Value {
    Value::from(args[0].as_float().tan())
}

pub fn math_sqrt(_heap: &mut Heap, args: &[Value]) -> Value {
    Value::from(args[0].as_float().sqrt())
}

pub fn math_floor(_heap: &mut Heap, args: &[Value]) -> Value {
    Value::from(args[0].as_float().floor())
}

pub fn math_ceil(_heap: &mut Heap, args: &[Value]) -> Value {
    Value::from(args[0].as_float().ceil())
}

pub fn math_exp(_heap: &mut Heap, args: &[Value]) -> Value {
    Value::from(args[0].as_float().exp())
}

pub fn math_ln(_heap: &mut Heap, args: &[Value]) -> Value {
    Value::from(args[0].as_float().ln())
}

pub fn math_pow(_heap: &mut Heap, args: &[Value]) -> Value {
    Value::from(args[0].as_float().powf(args[1].as_float()))
}

/// Append-only registry wiring for the scalar math HostInvoke natives.
pub const MATH_LIBM_WIRING: &[(&str, usize, fn(&mut Heap, &[Value]) -> Value)] = &[
    ("math_sin", 1, math_sin),
    ("math_cos", 1, math_cos),
    ("math_tan", 1, math_tan),
    ("math_sqrt", 1, math_sqrt),
    ("math_floor", 1, math_floor),
    ("math_ceil", 1, math_ceil),
    ("math_exp", 1, math_exp),
    ("math_ln", 1, math_ln),
    ("math_pow", 2, math_pow),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn call(host: fn(&mut Heap, &[Value]) -> Value, args: &[f64]) -> f64 {
        let mut heap = Heap::default();
        let values: Vec<Value> = args.iter().copied().map(Value::from).collect();
        host(&mut heap, &values).as_float()
    }

    #[test]
    fn math_libm_unary_functions_match_f64() {
        let cases: &[(fn(&mut Heap, &[Value]) -> Value, f64, f64)] = &[
            (math_sin, std::f64::consts::FRAC_PI_2, 1.0),
            (math_cos, std::f64::consts::PI, -1.0),
            (math_tan, 0.0, 0.0),
            (math_sqrt, 9.0, 3.0),
            (math_floor, -1.25, -2.0),
            (math_ceil, -1.25, -1.0),
            (math_exp, 1.0, std::f64::consts::E),
            (math_ln, std::f64::consts::E, 1.0),
        ];

        for &(host, input, expected) in cases {
            let actual = call(host, &[input]);
            assert!(
                (actual - expected).abs() < 1e-12,
                "input {input}: expected {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn math_libm_pow_uses_powf() {
        assert_eq!(call(math_pow, &[2.0, 3.0]), 8.0);
        assert_eq!(call(math_pow, &[9.0, 0.5]), 3.0);
    }

    #[test]
    fn math_libm_preserves_ieee_nan_and_infinity() {
        assert!(call(math_sqrt, &[-1.0]).is_nan());
        assert!(call(math_ln, &[-1.0]).is_nan());
        assert_eq!(call(math_ln, &[0.0]), f64::NEG_INFINITY);
        assert_eq!(call(math_exp, &[1000.0]), f64::INFINITY);
        assert!(call(math_pow, &[-2.0, 0.5]).is_nan());
    }
}
