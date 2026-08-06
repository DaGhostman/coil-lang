use collections::{sort, reverse, collect_ints, collect_ints_inclusive};

test("sort reverse collect") {
    let a = sort([3, 1, 4, 1, 5]);
    assert(a[0] == 1)?;
    assert(a[1] == 1)?;
    assert(a[4] == 5)?;
    let r = reverse([1, 2, 3]);
    assert(r[0] == 3)?;
    assert(r[2] == 1)?;
    let c = collect_ints(0..4);
    assert(len(c) == 4)?;
    assert(c[0] == 0)?;
    assert(c[3] == 3)?;
    let d = collect_ints_inclusive(1..=3);
    assert(len(d) == 3)?;
    assert(d[2] == 3)?;
}

test("sort larger and empty") {
    let a = sort([9, 8, 7, 6, 5, 4, 3, 2, 1, 0]);
    assert(a[0] == 0)?;
    assert(a[9] == 9)?;
    let b = sort([2, 1, 2, 1, 2]);
    assert(b[0] == 1)?;
    assert(b[1] == 1)?;
    assert(b[4] == 2)?;
    let empty: [int] = [];
    let sorted = sort(empty);
    assert(len(sorted) == 0)?;
}
