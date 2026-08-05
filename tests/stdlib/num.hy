use num::*;

test("abs min max floor ceil sqrt") {
    assert(abs_int(0 - 5) == 5)?;
    assert(min_int(3, 1) == 1)?;
    assert(max_int(3, 1) == 3)?;
    assert((floor(3.7) as int) == 3)?;
    assert((ceil(3.2) as int) == 4)?;
    assert((sqrt(9.0) as int) == 3)?;
}

test("pow_int clamp") {
    assert(pow_int(2, 8) == 256)?;
    assert(clamp_int(5, 0, 3) == 3)?;
}
