// Dynamic [T] policy: [byte] in lets, [int] in fn signatures only.
fn head([int] xs) -> int {
    return xs[0];
}

fn empty_row() -> [int] {
    let xs: [int; 0] = [];
    return xs;
}

test("dynamic int slice param indexes at runtime") {
    let n = head([7, 8, 9]);
    assert(n == 7)?;
}

test("byte slice from string literal") {
    let buf: [byte] = "ok";
    assert(len(buf) == 2)?;
}

test("fixed array keeps static length") {
    let xs: [int; 2] = [1, 2];
    assert(len(xs) == 2)?;
}

test("fn return of empty dynamic slice") {
    let row = empty_row();
    assert(len(row) == 0)?;
}
