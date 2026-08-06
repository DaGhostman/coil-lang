use num::{abs, min, max, clamp, round, pow};

test("abs min max") {
    assert(abs(0 - 5) == 5)?;
    assert(abs(0) == 0)?;
    assert(abs(0.0 - 2.5) == 2.5)?;
    assert(abs(0.0) == 0.0)?;
    assert(min(3, 1) == 1)?;
    assert(max(3, 1) == 3)?;
    assert(min(3.0, 1.0) == 1.0)?;
    assert(max(3.0, 1.0) == 3.0)?;
}

test("pow clamp round") {
    assert(pow(2, 8) == 256)?;
    assert(pow(2, 0) == 1)?;
    assert(pow(2.0, 10.0) == 1024.0)?;
    assert(clamp(5, 0, 3) == 3)?;
    assert(clamp(0 - 1, 0, 3) == 0)?;
    assert(clamp(2, 0, 3) == 2)?;
    assert(clamp(5.0, 0.0, 3.0) == 3.0)?;
    assert(round(3.6) == 4.0)?;
    assert(round(0.0 - 3.6) == 0.0 - 4.0)?;
}
