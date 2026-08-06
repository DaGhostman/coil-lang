use text::{
    trim, starts_with, ends_with, contains, split, to_lower, to_upper,
    replace, split_once, join, repeat, pad_left, pad_right, lines,
};

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

test("replace split once and join") {
    assert(replace("one two two", "two", "2")? == "one 2 2")?;
    let pair = split_once("key=value=rest", "=")?;
    assert(pair[0] == "key")?;
    assert(pair[1] == "value=rest")?;

    let parts: Vec<string> = Vec::new();
    parts.push("a");
    parts.push("b");
    parts.push("c");
    assert(join(parts, "::") == "a::b::c")?;
}

test("repeat text") {
    assert(repeat("ab", 3) == "ababab")?;
    assert(repeat("ab", -1) == "")?;
}

test("pad text left") {
    assert(pad_left("7", 3, "0")? == "007")?;
}

test("pad text right") {
    assert(pad_right("7", 3, "0")? == "700")?;
}

test("pad text preserves wide input") {
    assert(pad_right("wide", 2, "0")? == "wide")?;
}

test("lines handles crlf") {
    let rows = lines("a\r\nb")?;
    assert(len(rows) == 2)?;
    assert(rows[0] == "a")?;
    assert(rows[1] == "b")?;
}

test("lines preserves trailing empty line") {
    let rows = lines("a\r\nb\n")?;
    assert(len(rows) == 3)?;
    assert(rows[0] == "a")?;
    assert(rows[1] == "b")?;
    assert(rows[2] == "")?;
}
