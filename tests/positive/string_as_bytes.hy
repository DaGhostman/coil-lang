use string::{to_bytes};

test("non-literal string as [byte]") {
    let s = "hi";
    let b = s as [byte];
    assert(b == to_bytes("hi"))?;
    assert(len(b) == 2)?;
}

test("literal still works") {
    let b = "ok" as [byte];
    assert(len(b) == 2)?;
    assert(b == to_bytes("ok"))?;
}
