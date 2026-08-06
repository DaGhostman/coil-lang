use string::{to_bytes};

test("array deep equality") {
    let a = to_bytes("hi");
    let b = to_bytes("hi");
    let c = to_bytes("ho");
    assert(a == b)?;
    assert(!(a == c))?;
    assert(a != c)?;
}

test("int array deep equality") {
    let a: [int] = [];
    a[] = 1;
    a[] = 2;
    let b: [int] = [];
    b[] = 1;
    b[] = 2;
    let c: [int] = [];
    c[] = 1;
    c[] = 3;
    assert(a == b)?;
    assert(!(a == c))?;
}
