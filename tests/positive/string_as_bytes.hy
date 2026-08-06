use string::{to_bytes};

test("non-literal string via to_bytes") {
    let s = "hi";
    let b = to_bytes(s);
    assert(b == to_bytes("hi"))?;
    assert(len(b) == 2)?;
}

test("literal cast to Vec<byte>") {
    let b = "ok" as Vec<byte>;
    assert(len(b) == 2)?;
    assert(b == to_bytes("ok"))?;
}
