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

test("sort larger and empty") {
    let a = sort(Vec::from([9, 8, 7, 6, 5, 4, 3, 2, 1, 0]));
    assert(a[0] == 0)?;
    assert(a[9] == 9)?;
    let b = sort(Vec::from([2, 1, 2, 1, 2]));
    assert(b[0] == 1)?;
    assert(b[1] == 1)?;
    assert(b[4] == 2)?;
    let empty: Vec<int> = Vec::new();
    let sorted = sort(empty);
    assert(len(sorted) == 0)?;
}

test("sort preserves input and odd lengths") {
    let src = Vec::from([3, 1, 2]);
    let out = sort(src);
    assert(src[0] == 3)?;
    assert(src[1] == 1)?;
    assert(src[2] == 2)?;
    assert(out[0] == 1)?;
    assert(out[1] == 2)?;
    assert(out[2] == 3)?;
    let one = sort(Vec::from([7]));
    assert(len(one) == 1)?;
    assert(one[0] == 7)?;
    let sorted = sort(Vec::from([1, 2, 3, 4]));
    assert(sorted[0] == 1)?;
    assert(sorted[3] == 4)?;
    let odd = sort(Vec::from([4, 1, 7, 3, 2, 6, 5]));
    assert(odd[0] == 1)?;
    assert(odd[1] == 2)?;
    assert(odd[2] == 3)?;
    assert(odd[3] == 4)?;
    assert(odd[4] == 5)?;
    assert(odd[5] == 6)?;
    assert(odd[6] == 7)?;
}
