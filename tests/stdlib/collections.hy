use collections::{sort, reverse, collect_ints, collect_ints_inclusive};

test("sort reverse collect") {
    let a = sort(Vec::from([3, 1, 4, 1, 5]));
    assert(a[0] == 1)?;
    assert(a[1] == 1)?;
    assert(a[4] == 5)?;
    let r = reverse(Vec::from([1, 2, 3]));
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
