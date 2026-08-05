use bytes::*;
use string::{to_bytes};

test("slice concat eq") {
    let b = to_bytes("hello");
    assert(eq(slice(b, 1, 4), to_bytes("ell")))?;
    assert(eq(concat(to_bytes("ab"), to_bytes("cd")), to_bytes("abcd")))?;
}

test("find contains affixes") {
    // `to_bytes` / buffer args may invalidate — rebuild hay per call.
    assert(find(to_bytes("abcdef"), to_bytes("cd")) == 2)?;
    assert(find(to_bytes("abcdef"), to_bytes("zz")) == -1)?;
    assert(contains(to_bytes("abcdef"), to_bytes("de")))?;
    assert(starts_with(to_bytes("abcdef"), to_bytes("ab")))?;
    assert(ends_with(to_bytes("abcdef"), to_bytes("ef")))?;
}

test("rfind and replace") {
    assert(rfind(to_bytes("ababa"), to_bytes("ba")) == 3)?;
    assert(rfind(to_bytes("abc"), to_bytes("z")) == -1)?;
    assert(rfind(to_bytes("abc"), to_bytes("")) == 3)?;
    assert(eq(
        replace(to_bytes("aaaa"), to_bytes("aa"), to_bytes("b")),
        to_bytes("bb"),
    ))?;
    assert(eq(
        replace(to_bytes("abc"), to_bytes(""), to_bytes("x")),
        to_bytes("abc"),
    ))?;
}

test("repeat and padding") {
    assert(eq(repeat(to_bytes("ab"), 3), to_bytes("ababab")))?;
    assert(eq(repeat(to_bytes("ab"), 0), to_bytes("")))?;
    assert(eq(pad_left(to_bytes("7"), 3, "0"), to_bytes("007")))?;
    assert(eq(pad_right(to_bytes("7"), 3, "0"), to_bytes("700")))?;
    assert(eq(pad_left(to_bytes("wide"), 2, "0"), to_bytes("wide")))?;
}

