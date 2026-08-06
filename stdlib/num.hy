// Scalar numeric helpers layered on auto-imported `prelude::math`.
// Named `num` so workspace `examples/src/math.hy` does not shadow this module.
//
// `abs` stays type-overloaded (numeric / negation, not Ord).
// `min` / `max` / `clamp` are generic over `Ord` (int, float, and derived orders).
// `pow` is userland: float wraps virtual `prelude::math::pow`; int is iterative.

use prelude::math::{pow as float_pow};

/// Absolute value of an integer.
fn abs(int x) -> int {
    if x < 0 {
        return 0 - x;
    }
    return x;
}

/// Absolute value of a float (preserves signed zero).
fn abs(float x) -> float {
    if x > 0.0 {
        return x;
    }
    if x == 0.0 {
        return 0.0;
    }
    return 0.0 - x;
}

/// Smaller of two `Ord` values.
fn min<T: Ord>(T a, T b) -> T {
    if a < b {
        return a;
    }
    return b;
}

/// Larger of two `Ord` values.
fn max<T: Ord>(T a, T b) -> T {
    if a > b {
        return a;
    }
    return b;
}

/// Nearest int as float; halves round away from zero (via trunc bias).
fn round(float x) -> float {
    if x >= 0.0 {
        return floor(x + 0.5);
    }
    return ceil(x - 0.5);
}

/// Clamp `x` into the inclusive range `[lo, hi]`.
fn clamp<T: Ord>(T x, T lo, T hi) -> T {
    return min(max(x, lo), hi);
}

/// Float power via libm (`base ** exp` IEEE semantics).
fn pow(float a, float b) -> float {
    return float_pow(a, b);
}

/// Integer power `base ** exp` for `exp >= 0`.
fn pow(int base, int exp) -> int {
    let r = 1;
    let i = 0;
    while i < exp {
        r = r * base;
        i = i + 1;
    }
    return r;
}
