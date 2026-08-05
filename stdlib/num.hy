// Scalar numeric helpers not provided by auto-imported `prelude::math`.
// Named `num` so workspace `examples/src/math.hy` does not shadow this module.

fn abs_int(int x) -> int {
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

fn min_int(int a, int b) -> int {
    if a < b {
        return a;
    }
    return b;
}

fn max_int(int a, int b) -> int {
    if a > b {
        return a;
    }
    return b;
}

fn min(float a, float b) -> float {
    if a < b {
        return a;
    }
    return b;
}

fn max(float a, float b) -> float {
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

fn clamp(float x, float lo, float hi) -> float {
    return min(max(x, lo), hi);
}

fn clamp_int(int x, int lo, int hi) -> int {
    return min_int(max_int(x, lo), hi);
}

/// Integer power `base ** exp` for `exp >= 0`. Negative exp → treat as 0 result via empty loop when exp==0; callers should pass non-negative.
fn pow_int(int base, int exp) -> int {
    let r = 1;
    let i = 0;
    while i < exp {
        r = r * base;
        i = i + 1;
    }
    return r;
}
