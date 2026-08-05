use text::*;

test("trim and affixes") {
    let t = match trim("  hi  ") {
        Result::Ok(s) => s,
        Result::Err(_) => panic "trim",
    };
    assert(t == "hi")?;
    assert(starts_with("hello", "he"))?;
    assert(ends_with("hello", "lo"))?;
    assert(contains("hello", "ell"))?;
}

test("split and case") {
    let parts = match split("a,b,c", ",") {
        Result::Ok(p) => p,
        Result::Err(_) => panic "split",
    };
    assert(len(parts) == 3)?;
    assert(parts[0] == "a")?;
    assert(parts[2] == "c")?;
    let low = match to_lower("AbC") {
        Result::Ok(s) => s,
        Result::Err(_) => panic "lower",
    };
    assert(low == "abc")?;
    let up = match to_upper("AbC") {
        Result::Ok(s) => s,
        Result::Err(_) => panic "upper",
    };
    assert(up == "ABC")?;
}
