// Scalar numeric helpers not provided by auto-imported `prelude::math`.
// Named `num` so workspace `examples/src/math.hy` does not shadow this module.
//
// `abs` stays type-overloaded (numeric / negation, not Ord).
// `min` / `max` / `clamp` are generic over `Ord` (int, float, and derived orders).
// Integer power stays `pow_int` until prelude `pow(float, float)` joins the
// same overload family (prelude still special-cases bare `pow`).

fn abs(int x) -> int {
    if x < 0 {
        return 0 - x;
    }
    return x;
}

fn abs(float x) -> float {
    if x > 0.0 {
        return x;
    }
    if x == 0.0 {
        return 0.0;
    }
    return 0.0 - x;
}

fn min<T: Ord>(T a, T b) -> T {
    if a < b {
        return a;
    }
    return b;
}

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

fn clamp<T: Ord>(T x, T lo, T hi) -> T {
    return min(max(x, lo), hi);
}

/// Integer power `base ** exp` for `exp >= 0`.
fn pow_int(int base, int exp) -> int {
    let r = 1;
    let i = 0;
    while i < exp {
        r = r * base;
        i = i + 1;
    }
    return r;
}
