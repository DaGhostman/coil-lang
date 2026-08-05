// Scalar numeric helpers (userland). Named `num` so workspace `examples/src/math.hy`
// does not shadow this module; end-user projects with `./stdlib` in roots are fine.
// Trig / sqrt use iterative approximations (no libm HostInvoke).

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

/// Truncate toward −∞.
fn floor(float x) -> float {
    let i = x as int;
    let f = i as float;
    if x >= 0.0 {
        return f;
    }
    if f == x {
        return f;
    }
    return f - 1.0;
}

/// Truncate toward +∞.
fn ceil(float x) -> float {
    let i = x as int;
    let f = i as float;
    if x <= 0.0 {
        return f;
    }
    if f == x {
        return f;
    }
    return f + 1.0;
}

/// Nearest int as float; halves round away from zero (via trunc bias).
fn round(float x) -> float {
    if x >= 0.0 {
        return floor(x + 0.5);
    }
    return ceil(x - 0.5);
}

/// Newton–Raphson square root. Negative → `0.0`.
fn sqrt(float x) -> float {
    if x <= 0.0 {
        return 0.0;
    }
    let g = x;
    let i = 0;
    while i < 32 {
        g = 0.5 * (g + x / g);
        i = i + 1;
    }
    return g;
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

/// `e ** x` via Taylor series (reasonable for |x| ≲ 20).
fn exp(float x) -> float {
    // Reduce via e^x = e^(k*ln2) * e^r with coarse int scaling when |x| large.
    let ln2 = 0.6931471805599453;
    let k = 0;
    let r = x;
    if x > 0.0 {
        while r > ln2 {
            r = r - ln2;
            k = k + 1;
            if k > 1023 {
                break;
            }
        }
    }
    if x < 0.0 {
        while r < 0.0 - ln2 {
            r = r + ln2;
            k = k - 1;
            if k < 0 - 1023 {
                break;
            }
        }
    }
    let term = 1.0;
    let sum = 1.0;
    let n = 1;
    while n < 40 {
        term = term * r / (n as float);
        sum = sum + term;
        n = n + 1;
    }
    // Multiply/divide by 2^k
    if k > 0 {
        let i = 0;
        while i < k {
            sum = sum * 2.0;
            i = i + 1;
        }
    }
    if k < 0 {
        let i = 0;
        while i < 0 - k {
            sum = sum / 2.0;
            i = i + 1;
        }
    }
    return sum;
}

/// Natural log via artanh series for x > 0. Non-positive → `0.0`.
fn ln(float x) -> float {
    if x <= 0.0 {
        return 0.0;
    }
    // Normalize to [0.5, 1) * 2^exp
    let y = x;
    let exp2 = 0;
    while y >= 2.0 {
        y = y / 2.0;
        exp2 = exp2 + 1;
        if exp2 > 1023 {
            break;
        }
    }
    while y < 0.5 {
        y = y * 2.0;
        exp2 = exp2 - 1;
        if exp2 < 0 - 1023 {
            break;
        }
    }
    // ln(y) for y in [0.5, 2) via atanh: ln(y) = 2 * (z + z^3/3 + …), z = (y-1)/(y+1)
    let z = (y - 1.0) / (y + 1.0);
    let z2 = z * z;
    let term = z;
    let sum = z;
    let n = 1;
    while n < 32 {
        term = term * z2;
        let denom = (2 * n + 1) as float;
        sum = sum + term / denom;
        n = n + 1;
    }
    let ln2 = 0.6931471805599453;
    return 2.0 * sum + (exp2 as float) * ln2;
}

fn pow(float base, float expn) -> float {
    if base == 0.0 {
        if expn <= 0.0 {
            return 0.0;
        }
        return 0.0;
    }
    if base < 0.0 {
        // Only integer exponents for negatives in this MVP.
        let ei = expn as int;
        if (ei as float) != expn {
            return 0.0;
        }
        return pow_int(base as int, ei) as float;
    }
    return exp(expn * ln(base));
}

/// Sine via Taylor after reduction into [-π, π].
fn sin(float x) -> float {
    let pi = 3.141592653589793;
    let two_pi = 6.283185307179586;
    let t = x;
    while t > pi {
        t = t - two_pi;
        if t < 0.0 - 1000.0 {
            break;
        }
    }
    while t < 0.0 - pi {
        t = t + two_pi;
        if t > 1000.0 {
            break;
        }
    }
    let term = t;
    let sum = t;
    let n = 1;
    while n < 20 {
        let n2 = (2 * n) as float;
        let n2p = (2 * n + 1) as float;
        term = 0.0 - term * t * t / (n2 * n2p);
        sum = sum + term;
        n = n + 1;
    }
    return sum;
}

fn cos(float x) -> float {
    let pi = 3.141592653589793;
    return sin(x + pi / 2.0);
}

fn tan(float x) -> float {
    let c = cos(x);
    if c == 0.0 {
        return 0.0;
    }
    return sin(x) / c;
}
