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
    assert(find(to_bytes("abcdef"), to_bytes("zz")) == (0 - 1))?;
    assert(contains(to_bytes("abcdef"), to_bytes("de")))?;
    assert(starts_with(to_bytes("abcdef"), to_bytes("ab")))?;
    assert(ends_with(to_bytes("abcdef"), to_bytes("ef")))?;
}
