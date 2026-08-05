fn approx(float actual, float expected, float epsilon) -> bool {
    let delta = actual - expected;
    if delta < 0.0 {
        return (0.0 - delta) < epsilon;
    }
    return delta < epsilon;
}

fn epsilon() -> float {
    return 1.0 / 1000000.0;
}

test("prelude math trigonometry") {
    let pi = 3.141592653589793;
    assert(approx(sin(pi / 2.0), 1.0, epsilon()))?;
    assert(approx(cos(0.0), 1.0, epsilon()))?;
    assert(approx(tan(pi / 4.0), 1.0, epsilon()))?;
}

test("prelude math scalar functions") {
    assert(approx(sqrt(2.0), 1.4142135623730951, epsilon()))?;
    assert(floor(0.0 - 1.25) == 0.0 - 2.0)?;
    assert(ceil(0.0 - 1.25) == 0.0 - 1.0)?;
    assert(approx(exp(1.0), 2.718281828459045, epsilon()))?;
    assert(approx(ln(2.718281828459045), 1.0, epsilon()))?;
    assert(pow(2.0, 10.0) == 1024.0)?;
    assert(approx(pow(9.0, 0.5), 3.0, epsilon()))?;
}

test("nested prelude math host invokes") {
    assert(approx(sqrt(pow(3.0, 2.0) + pow(4.0, 2.0)), 5.0, epsilon()))?;
}

test("prelude math preserves IEEE exceptional values") {
    let sqrt_nan = sqrt(0.0 - 1.0);
    let ln_nan = ln(0.0 - 1.0);
    assert(sqrt_nan != sqrt_nan)?;
    assert(ln_nan != ln_nan)?;
    assert(ln(0.0) < 0.0 - 1000000.0)?;
    assert(exp(1000.0) > 1000000.0)?;
    assert(pow(0.0, 0.0 - 1.0) > 1000000.0)?;
}
